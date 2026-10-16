use membank::Message;
use serde_json::Value;
use std::fs::File;
use std::io::{self, BufRead, BufReader, ErrorKind, Write};
use std::{env, process};

pub fn run(name: &str, message: impl Fn(Value) -> io::Result<Option<Message>>) {
    let transcripts: Vec<_> = env::args_os().skip(1).collect();
    if transcripts.is_empty() {
        eprintln!("usage: {name} TRANSCRIPT.jsonl... > chat.jsonl");
        process::exit(2);
    }
    let mut out = io::stdout().lock();
    let converted = transcripts.iter().try_for_each(|path| {
        for line in BufReader::new(File::open(path)?).lines() {
            let record = serde_json::from_str(&line?).map_err(|error| io::Error::new(ErrorKind::InvalidData, error))?;
            if let Some(message) = message(record)? {
                serde_json::to_writer(&mut out, &message)?;
                writeln!(out)?;
            }
        }
        Ok::<_, io::Error>(())
    });
    if let Err(e) = converted {
        eprintln!("{name}: {e}");
        process::exit(1);
    }
}
