use clap::{Parser, Subcommand};
use log::{debug, info};
use membank::{Bank, Dir, Limits, Message, Node, SYSTEM};
use std::collections::HashSet;
use std::fs::{self, File};
use std::io::{self, BufRead, BufReader, ErrorKind, Write};
use std::path::{Path, PathBuf};
use std::process::{self, Command, Stdio};
use std::time::Instant;

#[derive(Parser)]
#[command(about = "A memory bank that compacts a chat log into a binary tree of summaries")]
struct Cli {
    #[arg(long, global = true, env = "MEMBANK")]
    bank: Option<PathBuf>,
    #[command(subcommand)]
    command: Action,
}

#[derive(Subcommand)]
enum Action {
    Import { transcripts: Vec<PathBuf> },
    Compact {
        #[arg(long, default_value_t = 8)]
        jobs: usize,
        #[arg(long)]
        system: Option<PathBuf>,
        #[arg(required = true, last = true, num_args = 1..)]
        command: Vec<String>,
    },
    Context {
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
    let Some(bank) = cli.bank.or_else(default_bank) else {
        eprintln!("membank: no home directory; pass --bank or set MEMBANK");
        process::exit(1);
    };
    if let Err(e) = run(&bank, cli.command) {
        eprintln!("membank: {e}");
        process::exit(1);
    }
}

fn run(dir: &Path, action: Action) -> io::Result<()> {
    let open = || Bank::open(Dir(dir.to_path_buf()), Limits::default());
    match action {
        Action::Import { transcripts } => {
            let _lock = lock(dir)?;
            let mut bank = open()?;
            let before = bank.messages().len();
            import(&mut bank, &transcripts)?;
            bank.refresh()?;
            info!("imported {} messages", bank.messages().len() - before);
        }
        Action::Compact { jobs, system, command } => {
            let _lock = lock(dir)?;
            let system = system.map(fs::read_to_string).transpose()?;
            let system = system.as_deref().unwrap_or(SYSTEM);
            let mut bank = open()?;
            let mut failed = 0;
            bank.compact(jobs, |prompt| ask(&command, system, prompt), |_, line| failed += line.is_err() as usize)?;
            bank.refresh()?;
            if failed > 0 {
                return Err(io::Error::other(format!("{failed} compactions failed; run compact again to retry them")));
            }
        }
        Action::Context { budget } => {
            let bank = open()?;
            let mut out = io::stdout().lock();
            let mut m = std::env::current_exe()?.display().to_string();
            if default_bank().as_deref() != Some(dir) {
                m = format!("{m} --bank {}", dir.display());
            }
            writeln!(out, "<membank>
The <chat> below is the user's entire history with you, every chat in every
project. What they told you before is here, not in the code or memory files.
It is compacted into a binary tree: each line is id+n|text, a summary of the n
messages from id on, oldest first. Its latest word on a thing is the truth.
Lines are lossy, so zoom until you have the thing whole. Check here first,
before any other source and before you act, guess or ask, whenever the user
refers back to something, corrects you, or names a person, ID, branch, ticket
or file you don't recognize. Never ask the user a question until you have
searched here for its answer; ask only what this history cannot answer:
- {m} zoom <id> <n> opens line id+n into the two lines it was made from
- {m} zoom <id> 1 gives message id whole
- {m} date <id> gives the date and time of message id
Only when no line here points to it, {m} search <words>
lists the best fuzzy matches as lines to zoom. Tool calls are not kept, so say
in your reply what you learned that will matter later.
</membank>")?;
            writeln!(out, "<chat>")?;
            let lines: Vec<String> = bank.visible(&bank.shrink(bank.context(), bank.context().last().map_or(0, |n| n.end()), budget)).into_iter().map(|n| bank.line(n)).collect();
            let mut room = budget;
            let kept = lines.iter().rev().take_while(|line| room.checked_sub(line.len() + 1).map(|r| room = r).is_some()).count();
            for line in &lines[lines.len() - kept..] {
                writeln!(out, "{line}")?;
            }
            writeln!(out, "</chat>")?;
        }
        Action::Zoom { id, n } => {
            let bank = open()?;
            let l = n.trailing_zeros();
            if !n.is_power_of_two() || id % n != 0 || id + n > bank.messages().len() as u32 {
                return Err(io::Error::new(ErrorKind::InvalidInput, format!("{id}+{n} is not a line of this bank")));
            }
            match (Node { level: l as u8, index: id / n }).children() {
                None => {
                    if let Some(message) = &bank.messages()[id as usize] {
                        println!("{}: {}", message.kind, message.text);
                    }
                }
                Some(children) => children.iter().for_each(|&c| println!("{}", bank.line(c))),
            }
        }
        Action::Prune { ids } => {
            let _lock = lock(dir)?;
            let mut bank = open()?;
            if let Some(id) = ids.iter().find(|&&id| id as usize >= bank.messages().len()) {
                return Err(io::Error::new(ErrorKind::InvalidInput, format!("no message {id}")));
            }
            for id in ids {
                bank.prune(id)?;
            }
        }
        Action::Date { id } => {
            let bank = open()?;
            let message = bank.messages().get(id as usize).and_then(Option::as_ref).ok_or_else(|| io::Error::new(ErrorKind::InvalidInput, format!("no message {id}")))?;
            println!("{}", message.date);
        }
        Action::Search { words, limit } => {
            let bank = open()?;
            let mut out = io::stdout().lock();
            for (node, text) in bank.search(&words.join(" ")).into_iter().take(limit) {
                writeln!(out, "{node}|{}", text[..text.floor_char_boundary(240)].replace('\n', " "))?;
            }
        }
    }
    Ok(())
}

fn default_bank() -> Option<PathBuf> {
    std::env::home_dir().map(|home| home.join(".membank/global"))
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

fn ask(command: &[String], system: &str, prompt: &str) -> io::Result<String> {
    let asked = Instant::now();
    let (program, args) = command.split_first().ok_or_else(|| io::Error::new(ErrorKind::InvalidInput, "missing compactor command"))?;
    let mut child = Command::new(program)
        .args(args)
        .current_dir(std::env::temp_dir())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    child.stdin.take().map_or(Ok(()), |mut stdin| write!(stdin, "{system}\n\n{prompt}"))?;
    let output = child.wait_with_output()?;
    debug!("{program}: {}B in, {}B out, {} in {:.1?}", system.len() + 2 + prompt.len(), output.stdout.len(), output.status, asked.elapsed());
    match output.status.success() {
        true => Ok(String::from_utf8_lossy(&output.stdout).into_owned()),
        false => Err(io::Error::other(String::from_utf8_lossy(&output.stderr).trim().to_string())),
    }
}

fn import(bank: &mut Bank<Dir>, transcripts: &[PathBuf]) -> io::Result<()> {
    let mut files = Vec::new();
    for path in transcripts {
        match path.is_dir() {
            true => files.extend(fs::read_dir(path)?.map(|e| e.map(|e| e.path())).collect::<io::Result<Vec<_>>>()?.into_iter().filter(|p| p.extension().is_some_and(|e| e == "jsonl"))),
            false => files.push(path.clone()),
        }
    }
    let mut seen: HashSet<String> = bank.messages().iter().flatten().map(|m| m.src.clone()).collect();
    let mut entries = Vec::new();
    for file in files {
        for line in BufReader::new(File::open(&file)?).lines() {
            let entry: Message = serde_json::from_str(&line?).map_err(|error| io::Error::new(ErrorKind::InvalidData, error))?;
            if [&entry.text, &entry.date, &entry.src, &entry.session].iter().any(|field| field.trim().is_empty()) {
                return Err(io::Error::new(ErrorKind::InvalidData, "message text, date, src and session must be nonempty"));
            }
            if seen.insert(entry.src.clone()) {
                entries.push(entry);
            }
        }
    }
    bank.add(entries)
}

#[cfg(test)]
mod tests {
    use super::*;
    use membank::Kind;

    #[test]
    fn imports_neutral_messages_once_in_date_order() {
        let dir = std::env::temp_dir().join(format!("membank-import-{}", process::id()));
        fs::create_dir_all(&dir).unwrap();
        let transcript = dir.join("input.jsonl");
        let input = concat!(
            "{\"kind\":\"assistant\",\"text\":\"answer\",\"date\":\"2026-10-15T01:00:01Z\",\"src\":\"answer-id\",\"session\":\"s\"}\n",
            "{\"kind\":\"user\",\"text\":\"question\",\"date\":\"2026-10-15T01:00:00Z\",\"src\":\"question-id\",\"session\":\"s\"}\n"
        );
        fs::write(&transcript, input).unwrap();
        let bank_dir = dir.join("bank");
        for _ in 0..2 {
            run(&bank_dir, Action::Import { transcripts: vec![transcript.clone()] }).unwrap();
        }
        let bank = Bank::open(Dir(bank_dir.clone()), Limits::default()).unwrap();
        assert_eq!(bank.messages().len(), 2);
        let messages: Vec<&Message> = bank.messages().iter().flatten().collect();
        assert_eq!(messages[0].text, "question");
        assert_eq!(messages[1].kind.to_string(), "assistant");
        assert_eq!(messages[1].src, "answer-id");
        assert_eq!(bank.context(), [Node::message(0), Node::message(1)]);
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
            "{\"kind\":\"user\",\"text\":\"question\",\"date\":\"2026-10-15\",\"src\":\"id\"}",
            "{\"kind\":\"user\",\"text\":\"question\",\"date\":\"2026-10-15\",\"src\":\"id\",\"session\":\"s\"}\nnot json",
        ] {
            fs::write(&transcript, input).unwrap();
            let error = run(&bank_dir, Action::Import { transcripts: vec![transcript.clone()] }).unwrap_err();
            assert_eq!(error.kind(), ErrorKind::InvalidData);
            assert!(Bank::open(Dir(bank_dir.clone()), Limits::default()).unwrap().messages().is_empty());
        }
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn compaction_uses_the_supplied_command() {
        let dir = std::env::temp_dir().join(format!("membank-command-{}", process::id()));
        let mut bank = Bank::open(Dir(dir.clone()), Limits::default()).unwrap();
        bank.add(vec![Message { kind: Kind::User, text: "question ".repeat(100), date: "2026-10-15".into(), src: "id".into(), session: "s".into() }]).unwrap();
        let prompt = dir.join("prompt.txt");
        let system = dir.join("system.txt");
        fs::write(&system, "custom system prompt").unwrap();
        let cli = Cli::try_parse_from([
            "membank", "--bank", dir.to_str().unwrap(), "compact", "--jobs", "1", "--system", system.to_str().unwrap(), "--", "sh", "-c",
            "cat > \"$1\"; printf 'assistant: compacted'", "sh", prompt.to_str().unwrap(),
        ]).unwrap();
        run(&dir, cli.command).unwrap();
        assert_eq!(Bank::open(Dir(dir.clone()), Limits::default()).unwrap().text(Node::message(0)), Some("assistant: compacted"));
        let input = fs::read_to_string(&prompt).unwrap();
        assert!(input.starts_with("custom system prompt\n\n"));
        assert!(input.contains("user: question"));
        let cli = Cli::try_parse_from([
            "membank", "--bank", dir.to_str().unwrap(), "compact", "--", "sh", "-c",
            "cat >/dev/null; printf rejected >&2; exit 7",
        ]).unwrap();
        bank.add(vec![Message { kind: Kind::User, text: "another question ".repeat(100), date: "2026-10-15".into(), src: "next".into(), session: "s".into() }]).unwrap();
        assert!(run(&dir, cli.command).unwrap_err().to_string().contains("compactions failed"));
        fs::remove_dir_all(dir).unwrap();
    }
}
