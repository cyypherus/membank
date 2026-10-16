use membank::Message;
use serde_json::Value;
use std::fs::File;
use std::io::{self, BufRead, BufReader, ErrorKind, Write};
use std::{env, process};

pub fn run(name: &str, message: impl Fn(Value) -> io::Result<Option<Message>>) {
    let mut transcripts: Vec<_> = env::args_os().skip(1).collect();
    if transcripts.is_empty() {
        match serde_json::from_reader::<_, Value>(io::stdin()).ok().and_then(|hook| hook["transcript_path"].as_str().map(Into::into)) {
            Some(path) => transcripts.push(path),
            None => {
                eprintln!("usage: {name} TRANSCRIPT.jsonl... > chat.jsonl, or a hook's JSON with transcript_path on stdin");
                process::exit(2);
            }
        }
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
