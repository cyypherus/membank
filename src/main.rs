use clap::{Parser, Subcommand, ValueEnum};
use log::{debug, info};
use membank::{Bank, Kind, Node, SYSTEM};
use serde::Deserialize;
use serde_json::{Value, json};
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
    Import {
        #[arg(long, value_enum, default_value = "membank")]
        format: TranscriptFormat,
        transcripts: Vec<PathBuf>,
    },
    Compact {
        #[arg(long, default_value_t = 8)]
        jobs: usize,
        #[arg(required = true, last = true, num_args = 1..)]
        command: Vec<String>,
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

#[derive(Clone, Copy, ValueEnum)]
enum TranscriptFormat {
    Membank,
    Claude,
    Codex,
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
        Action::Import { format, transcripts } => {
            let _lock = lock(dir)?;
            let mut bank = Bank::open(dir)?;
            let before = bank.messages.len();
            import(&mut bank, &transcripts, format)?;
            bank.refresh()?;
            info!("imported {} messages", bank.messages.len() - before);
        }
        Action::Compact { jobs, command } => {
            let _lock = lock(dir)?;
            let mut bank = Bank::open(dir)?;
            let mut failed = 0;
            bank.compact(jobs, |prompt| ask(&command, prompt), |_, line| failed += line.is_err() as usize)?;
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

fn ask(command: &[String], prompt: &str) -> io::Result<String> {
    let asked = Instant::now();
    let (program, args) = command.split_first().ok_or_else(|| io::Error::new(ErrorKind::InvalidInput, "missing compactor command"))?;
    let mut child = Command::new(program)
        .args(args)
        .current_dir(std::env::temp_dir())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    child.stdin.take().map_or(Ok(()), |mut stdin| write!(stdin, "{SYSTEM}\n\n{prompt}"))?;
    let output = child.wait_with_output()?;
    debug!("{program}: {}B in, {}B out, {} in {:.1?}", SYSTEM.len() + 2 + prompt.len(), output.stdout.len(), output.status, asked.elapsed());
    match output.status.success() {
        true => Ok(String::from_utf8_lossy(&output.stdout).into_owned()),
        false => Err(io::Error::other(String::from_utf8_lossy(&output.stderr).trim().to_string())),
    }
}

#[derive(Deserialize)]
struct Entry {
    kind: Kind,
    text: String,
    date: String,
    src: String,
}

fn import(bank: &mut Bank, transcripts: &[PathBuf], format: TranscriptFormat) -> io::Result<()> {
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
            let record: Value = serde_json::from_str(&line?).map_err(|error| io::Error::new(ErrorKind::InvalidData, error))?;
            let Some(entry) = transcript_entry(record, format)? else { continue };
            if entry.text.trim().is_empty() || entry.date.trim().is_empty() || entry.src.trim().is_empty() {
                return Err(io::Error::new(ErrorKind::InvalidData, "message text, date and src must be nonempty"));
            }
            if seen.insert(entry.src.clone()) {
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

fn transcript_entry(record: Value, format: TranscriptFormat) -> io::Result<Option<Entry>> {
    let (kind, content, identity) = match format {
        TranscriptFormat::Membank => {
            return serde_json::from_value(record).map(Some).map_err(|error| io::Error::new(ErrorKind::InvalidData, error));
        }
        TranscriptFormat::Claude => {
            if record["isSidechain"] == true || record["message"]["model"] == "<synthetic>" {
                return Ok(None);
            }
            let kind = match record["type"].as_str() {
                Some("user") => "user",
                Some("assistant") => "assistant",
                _ => return Ok(None),
            };
            (kind, &record["message"]["content"], record["uuid"].clone())
        }
        TranscriptFormat::Codex => {
            let payload = &record["payload"];
            if record["type"] != "event_msg" || payload["type"] != "item_completed" {
                return Ok(None);
            }
            let item = &payload["item"];
            let kind = match item["type"].as_str() {
                Some("UserMessage") => "user",
                Some("AgentMessage") => "assistant",
                _ => return Ok(None),
            };
            let identity = match (payload["thread_id"].as_str(), payload["turn_id"].as_str(), item["id"].as_str()) {
                (Some(thread), Some(turn), Some(id)) if !thread.is_empty() && !turn.is_empty() && !id.is_empty() => {
                    json!(format!("codex:{thread}:{turn}:{id}"))
                }
                _ => return Err(io::Error::new(ErrorKind::InvalidData, "Codex message is missing its identity")),
            };
            (kind, &item["content"], identity)
        }
    };
    let text = match content.as_str() {
        Some(text) => text.to_string(),
        None => content.as_array().ok_or_else(|| io::Error::new(ErrorKind::InvalidData, "message content must be text or an array"))?
            .iter().filter_map(|block| match block["type"].as_str() {
                Some("text" | "Text") => block["text"].as_str(),
                Some("image" | "Image" | "localImage" | "LocalImage") => Some("[image]"),
                _ => None,
            }).collect::<Vec<_>>().join("\n"),
    };
    if text.trim().is_empty() {
        return Ok(None);
    }
    serde_json::from_value(json!({ "kind": kind, "text": text, "date": record["timestamp"], "src": identity }))
        .map(Some).map_err(|error| io::Error::new(ErrorKind::InvalidData, error))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codex_adapter_reads_completed_messages_and_rejects_missing_identity() {
        let mut record = json!({
            "type": "event_msg", "timestamp": "2026-10-15T01:00:00Z",
            "payload": {"type": "item_completed", "thread_id": "chat", "turn_id": "turn",
                "item": {"type": "UserMessage", "id": "item-1", "content": [{"type": "text", "text": "question"}]}}
        });
        let entry = transcript_entry(record.clone(), TranscriptFormat::Codex).unwrap().unwrap();
        assert_eq!(entry.kind, Kind::User);
        assert_eq!(entry.text, "question");
        assert_eq!(entry.src, "codex:chat:turn:item-1");
        record["payload"]["item"]["type"] = json!("AgentMessage");
        assert_eq!(transcript_entry(record.clone(), TranscriptFormat::Codex).unwrap().unwrap().kind, Kind::Assistant);
        record["payload"]["item"]["id"] = Value::Null;
        assert!(transcript_entry(record, TranscriptFormat::Codex).is_err());
        for record in [
            json!({"type": "response_item", "payload": {"type": "message", "role": "user", "content": "injected"}}),
            json!({"type": "event_msg", "payload": {"type": "item_completed", "item": {"type": "Reasoning", "content": "private"}}}),
        ] {
            assert!(transcript_entry(record, TranscriptFormat::Codex).unwrap().is_none());
        }
    }

    #[test]
    fn claude_adapter_preserves_text_and_excludes_tools_and_sidechains() {
        let mut record = json!({"type": "assistant", "uuid": "answer", "timestamp": "2026-10-15", "message": {
            "content": [{"type": "thinking", "thinking": "private"}, {"type": "text", "text": "answer"}]
        }});
        let entry = transcript_entry(record.clone(), TranscriptFormat::Claude).unwrap().unwrap();
        assert_eq!(entry.kind, Kind::Assistant);
        assert_eq!(entry.text, "answer");
        record["isSidechain"] = json!(true);
        assert!(transcript_entry(record, TranscriptFormat::Claude).unwrap().is_none());
        let record = json!({"type": "user", "message": {"content": [{"type": "tool_result", "content": "tool output"}]}});
        assert!(transcript_entry(record, TranscriptFormat::Claude).unwrap().is_none());
    }

    #[test]
    fn imports_neutral_messages_once_in_date_order() {
        let dir = std::env::temp_dir().join(format!("membank-import-{}", process::id()));
        fs::create_dir_all(&dir).unwrap();
        let transcript = dir.join("input.jsonl");
        let input = concat!(
            "{\"kind\":\"assistant\",\"text\":\"answer\",\"date\":\"2026-10-15T01:00:01Z\",\"src\":\"answer-id\"}\n",
            "{\"kind\":\"user\",\"text\":\"question\",\"date\":\"2026-10-15T01:00:00Z\",\"src\":\"question-id\"}\n"
        );
        fs::write(&transcript, input).unwrap();
        let bank_dir = dir.join("bank");
        for _ in 0..2 {
            run(&bank_dir, Action::Import { format: TranscriptFormat::Membank, transcripts: vec![transcript.clone()] }).unwrap();
        }
        let bank = Bank::open(&bank_dir).unwrap();
        assert_eq!(bank.messages.len(), 2);
        assert_eq!(bank.messages[0].text, "question");
        assert_eq!(bank.messages[1].kind.to_string(), "assistant");
        assert_eq!(bank.messages[1].src, "answer-id");
        assert_eq!(bank.view, [Node(0, 0), Node(0, 1)]);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn rejects_invalid_imports_without_adding_messages() {
        let dir = std::env::temp_dir().join(format!("membank-invalid-{}", process::id()));
        fs::create_dir_all(&dir).unwrap();
        let transcript = dir.join("input.jsonl");
        let bank_dir = dir.join("bank");
        for input in [
            "{\"type\":\"assistant\",\"uuid\":\"legacy\",\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"answer\"}]}}",
            "{\"kind\":\"user\",\"text\":\"question\",\"date\":\"2026-10-15\",\"src\":\"\"}",
            "{\"kind\":\"user\",\"text\":\"question\",\"date\":\"\",\"src\":\"id\"}",
            "{\"kind\":\"assistant\",\"text\":\" \\n\",\"date\":\"2026-10-15\",\"src\":\"id\"}",
            "{\"kind\":\"user\",\"text\":\"question\",\"date\":\"2026-10-15\",\"src\":\"id\"}\nnot json",
        ] {
            fs::write(&transcript, input).unwrap();
            let error = run(&bank_dir, Action::Import { format: TranscriptFormat::Membank, transcripts: vec![transcript.clone()] }).unwrap_err();
            assert_eq!(error.kind(), ErrorKind::InvalidData);
            assert!(Bank::open(&bank_dir).unwrap().messages.is_empty());
        }
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn compaction_uses_the_supplied_command() {
        let dir = std::env::temp_dir().join(format!("membank-command-{}", process::id()));
        let mut bank = Bank::open(&dir).unwrap();
        bank.add(Kind::User, &"question ".repeat(100), "2026-10-15", "id").unwrap();
        let prompt = dir.join("prompt.txt");
        let cli = Cli::try_parse_from([
            "membank", dir.to_str().unwrap(), "compact", "--jobs", "1", "--", "sh", "-c",
            "cat > \"$1\"; printf 'assistant: compacted'", "sh", prompt.to_str().unwrap(),
        ]).unwrap();
        run(&dir, cli.command).unwrap();
        assert_eq!(Bank::open(&dir).unwrap().text(Node(0, 0)), Some("assistant: compacted"));
        let input = fs::read_to_string(&prompt).unwrap();
        assert!(input.starts_with(SYSTEM));
        assert!(input.contains("user: question"));
        let cli = Cli::try_parse_from([
            "membank", dir.to_str().unwrap(), "compact", "--", "sh", "-c",
            "cat >/dev/null; printf rejected >&2; exit 7",
        ]).unwrap();
        bank.add(Kind::User, &"another question ".repeat(100), "2026-10-15", "next").unwrap();
        assert!(run(&dir, cli.command).unwrap_err().to_string().contains("compactions failed"));
        fs::remove_dir_all(dir).unwrap();
    }
}
