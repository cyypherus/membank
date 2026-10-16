use clap::{Parser, Subcommand};
use log::{debug, info};
use membank::{Bank, Kind, Node, SYSTEM};
use serde_json::Value;
use std::collections::HashSet;
use std::fs::{self, File};
use std::io::{self, BufRead, BufReader, ErrorKind, Write};
use std::path::{Path, PathBuf};
use std::process::{self, Command, Stdio};
use std::time::Instant;

#[derive(Parser)]
#[command(about = "A memory bank that compacts a chat log into a binary tree of summaries")]
struct Cli {
    bank: PathBuf,
    #[command(subcommand)]
    command: Action,
}

#[derive(Subcommand)]
enum Action {
    Import { transcripts: Vec<PathBuf> },
    Compact {
        #[arg(long, default_value_t = 8)]
        jobs: usize,
        #[arg(long, default_value = "sonnet")]
        model: String,
    },
    View {
        #[arg(long, default_value_t = usize::MAX)]
        budget: usize,
    },
    Zoom { id: u32, n: u32 },
    Prune {
        #[arg(required = true)]
        ids: Vec<u32>,
    },
    Date { id: u32 },
    Search {
        #[arg(required = true)]
        words: Vec<String>,
        #[arg(long, default_value_t = 20)]
        limit: usize,
    },
}

fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    let cli = Cli::parse();
    if let Err(e) = run(&cli.bank, cli.command) {
        eprintln!("membank: {e}");
        process::exit(1);
    }
}

fn run(dir: &Path, action: Action) -> io::Result<()> {
    match action {
        Action::Import { transcripts } => {
            let _lock = lock(dir)?;
            let mut bank = Bank::open(dir)?;
            let before = bank.messages.len();
            import(&mut bank, &transcripts)?;
            bank.refresh()?;
            info!("imported {} messages", bank.messages.len() - before);
        }
        Action::Compact { jobs, model } => {
            let _lock = lock(dir)?;
            let mut bank = Bank::open(dir)?;
            let mut failed = 0;
            bank.compact(jobs, |prompt| ask(&model, prompt), |_, line| failed += line.is_err() as usize)?;
            bank.refresh()?;
            if failed > 0 {
                return Err(io::Error::other(format!("{failed} compactions failed; run compact again to retry them")));
            }
        }
        Action::View { budget } => {
            let bank = Bank::open(dir)?;
            let mut out = io::stdout().lock();
            writeln!(out, "<chat>")?;
            let lines: Vec<String> = bank.shrink(&bank.view, bank.view.last().map_or(0, |n| n.end()), budget).into_iter().map(|n| bank.line(n)).collect();
            let mut room = budget;
            let kept = lines.iter().rev().take_while(|line| room.checked_sub(line.len() + 1).map(|r| room = r).is_some()).count();
            for line in &lines[lines.len() - kept..] {
                writeln!(out, "{line}")?;
            }
            writeln!(out, "</chat>")?;
        }
        Action::Zoom { id, n } => {
            let bank = Bank::open(dir)?;
            let l = n.trailing_zeros();
            if !n.is_power_of_two() || id % n != 0 || id + n > bank.messages.len() as u32 {
                return Err(io::Error::new(ErrorKind::InvalidInput, format!("{id}+{n} is not a line of this bank")));
            }
            match Node(l as u8, id / n).children() {
                None => {
                    let message = &bank.messages[id as usize];
                    println!("{}: {}", message.kind, message.text);
                }
                Some(children) => children.iter().for_each(|&c| println!("{}", bank.line(c))),
            }
        }
        Action::Prune { ids } => {
            let _lock = lock(dir)?;
            let mut bank = Bank::open(dir)?;
            if let Some(id) = ids.iter().find(|&&id| id as usize >= bank.messages.len()) {
                return Err(io::Error::new(ErrorKind::InvalidInput, format!("no message {id}")));
            }
            for id in ids {
                bank.prune(id)?;
            }
        }
        Action::Date { id } => {
            let bank = Bank::open(dir)?;
            let message = bank.messages.get(id as usize).ok_or_else(|| io::Error::new(ErrorKind::InvalidInput, format!("no message {id}")))?;
            println!("{}", message.date);
        }
        Action::Search { words, limit } => {
            let bank = Bank::open(dir)?;
            let mut out = io::stdout().lock();
            for (node, text) in bank.search(&words.join(" ")).into_iter().take(limit) {
                writeln!(out, "{node}|{}", text[..text.floor_char_boundary(240)].replace('\n', " "))?;
            }
        }
    }
    Ok(())
}

fn lock(dir: &Path) -> io::Result<File> {
    fs::create_dir_all(dir)?;
    let file = File::create(dir.join("lock"))?;
    if file.try_lock().is_err() {
        info!("waiting for another membank process to finish with this bank");
        file.lock()?;
    }
    Ok(file)
}

fn ask(model: &str, prompt: &str) -> io::Result<String> {
    let asked = Instant::now();
    let mut child = Command::new("claude")
        .args(["-p", "--model", model, "--system-prompt", SYSTEM, "--tools", "", "--no-session-persistence", "--strict-mcp-config", "--setting-sources", ""])
        .current_dir(std::env::temp_dir())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    child.stdin.take().map_or(Ok(()), |mut stdin| stdin.write_all(prompt.as_bytes()))?;
    let output = child.wait_with_output()?;
    debug!("claude --model {model}: {}B in, {}B out, {} in {:.1?}", prompt.len(), output.stdout.len(), output.status, asked.elapsed());
    match output.status.success() {
        true => Ok(String::from_utf8_lossy(&output.stdout).into_owned()),
        false => Err(io::Error::other(String::from_utf8_lossy(&output.stderr).trim().to_string())),
    }
}

struct Entry {
    kind: Kind,
    text: String,
    date: String,
    src: String,
}

fn import(bank: &mut Bank, transcripts: &[PathBuf]) -> io::Result<()> {
    let mut files = Vec::new();
    for path in transcripts {
        match path.is_dir() {
            true => files.extend(fs::read_dir(path)?.map(|e| e.map(|e| e.path())).collect::<io::Result<Vec<_>>>()?.into_iter().filter(|p| p.extension().is_some_and(|e| e == "jsonl"))),
            false => files.push(path.clone()),
        }
    }
    let mut seen: HashSet<String> = bank.messages.iter().map(|m| m.src.clone()).collect();
    let mut entries = Vec::new();
    for file in files {
        for line in BufReader::new(File::open(&file)?).lines() {
            if let Some(entry) = serde_json::from_str(&line?).ok().and_then(|v| entry(&v)).filter(|e| seen.insert(e.src.clone())) {
                entries.push(entry);
            }
        }
    }
    entries.sort_by(|a, b| a.date.cmp(&b.date));
    for entry in entries {
        bank.add(entry.kind, &entry.text, &entry.date, &entry.src)?;
    }
    Ok(())
}

fn entry(v: &Value) -> Option<Entry> {
    if v["isSidechain"] == true || v["message"]["model"] == "<synthetic>" {
        return None;
    }
    let content = &v["message"]["content"];
    let blocks = || content.as_array().into_iter().flatten();
    let (kind, text) = match v["type"].as_str()? {
        "user" if v["origin"]["kind"] == "human" => {
            let text = match content.as_str() {
                Some(text) => text.to_string(),
                None => blocks()
                    .filter_map(|b| match b["type"].as_str() {
                        Some("text") => b["text"].as_str().map(str::to_string),
                        Some("image") => Some("[image]".to_string()),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join("\n"),
            };
            let text = match between(&text, "<command-name>", "</command-name>") {
                Some(name) => format!("{name} {}", between(&text, "<command-args>", "</command-args>").unwrap_or_default()),
                None => text,
            };
            (Kind::User, text)
        }
        "assistant" => (Kind::Claude, blocks().filter_map(|b| (b["type"] == "text").then(|| b["text"].as_str()).flatten()).collect::<Vec<_>>().join("\n\n")),
        _ => return None,
    };
    let text = text.trim();
    (!text.is_empty()).then(|| Entry { kind, text: text.to_string(), date: v["timestamp"].as_str().unwrap_or_default().to_string(), src: v["uuid"].as_str().unwrap_or_default().to_string() })
}

fn between<'a>(text: &'a str, open: &str, close: &str) -> Option<&'a str> {
    let start = text.find(open)? + open.len();
    Some(&text[start..start + text[start..].find(close)?])
}
