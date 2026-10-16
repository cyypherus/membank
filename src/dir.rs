use crate::{Message, Node, Saved, Span, Store};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufRead, BufReader, ErrorKind, Write};
use std::path::{Path, PathBuf};

pub struct Dir(pub PathBuf);

#[derive(Serialize, Deserialize)]
#[serde(untagged)]
enum Logged<M> {
    Message {
        i: u32,
        #[serde(flatten)]
        message: M,
    },
    Padding(Padding),
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Padding {
    i: u32,
}

impl<M> Logged<M> {
    fn id(&self) -> u32 {
        match self {
            Logged::Message { i, .. } | Logged::Padding(Padding { i }) => *i,
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(untagged)]
enum Built {
    Span { l: u8, first: u32, last: u32, text: Option<String> },
    Node { l: u8, i: u32, text: Option<String> },
}

impl Store for Dir {
    fn load(&self) -> io::Result<Saved> {
        let mut logged: Vec<Logged<Message>> = records(&self.0.join("main"))?;
        logged.reverse();
        logged.sort_by_key(|r| (r.id(), matches!(r, Logged::Padding(_))));
        logged.dedup_by_key(|r| r.id());
        if let Some((n, r)) = logged.iter().enumerate().find(|(n, r)| r.id() as usize != *n) {
            return Err(io::Error::new(ErrorKind::InvalidData, format!("message {} is logged where {n} belongs", r.id())));
        }
        let mut summaries = BTreeMap::new();
        for built in records(&self.0.join("tree"))? {
            let (span, text) = match built {
                Built::Span { l, first, last, text } => (Span { level: l, first_id: first, last_id: last }, text),
                Built::Node { l, i, text } => (Span { level: l, first_id: i << l, last_id: (i << l) + (1 << l) - 1 }, text),
            };
            match text {
                Some(text) => summaries.insert(span, text),
                None => summaries.remove(&span),
            };
        }
        let context: Vec<(u8, u32)> = read(&self.0.join("view.json"))?;
        let messages = logged.into_iter().map(|r| match r {
            Logged::Message { message, .. } => Some(message),
            Logged::Padding(_) => None,
        }).collect();
        Ok(Saved {
            messages,
            order: read(&self.0.join("order.json"))?,
            summaries: summaries.into_iter().collect(),
            context: context.into_iter().map(|(level, index)| Node { level, index }).collect(),
        })
    }

    fn save_message(&mut self, id: u32, message: Option<&Message>) -> io::Result<()> {
        match message {
            Some(message) => append(&self.0.join("main"), &message.date, &Logged::Message { i: id, message }),
            None => append(&self.0.join("main"), "", &Logged::<&Message>::Padding(Padding { i: id })),
        }
    }

    fn save_order(&mut self, order: &[u32]) -> io::Result<()> {
        replace(&self.0.join("order.json"), &order)
    }

    fn save_summary(&mut self, span: Span, text: Option<&str>, date: &str) -> io::Result<()> {
        append(&self.0.join("tree"), date, &Built::Span { l: span.level, first: span.first_id, last: span.last_id, text: text.map(str::to_string) })
    }

    fn save_context(&mut self, context: &[Node]) -> io::Result<()> {
        let context: Vec<(u8, u32)> = context.iter().map(|n| (n.level, n.index)).collect();
        replace(&self.0.join("view.json"), &context)
    }
}

fn read<T: DeserializeOwned>(path: &Path) -> io::Result<Vec<T>> {
    match fs::read(path) {
        Ok(bytes) => Ok(serde_json::from_slice(&bytes)?),
        Err(e) if e.kind() == ErrorKind::NotFound => Ok(Vec::new()),
        Err(e) => Err(e),
    }
}

fn replace(path: &Path, value: &impl Serialize) -> io::Result<()> {
    fs::create_dir_all(path.parent().unwrap_or(Path::new(".")))?;
    fs::write(path.with_extension("tmp"), serde_json::to_vec(value)?)?;
    fs::rename(path.with_extension("tmp"), path)
}

fn records<T: DeserializeOwned>(dir: &Path) -> io::Result<Vec<T>> {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e),
    };
    let mut out = Vec::new();
    for entry in entries {
        for line in BufReader::new(File::open(entry?.path())?).lines() {
            out.push(serde_json::from_str(&line?)?);
        }
    }
    Ok(out)
}

fn append(dir: &Path, date: &str, record: &impl Serialize) -> io::Result<()> {
    fs::create_dir_all(dir)?;
    let mut line = serde_json::to_vec(record)?;
    line.push(b'\n');
    OpenOptions::new().create(true).append(true).open(dir.join(format!("{}.jsonl", date.get(..10).unwrap_or("undated"))))?.write_all(&line)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Kind;

    #[test]
    fn saves_and_loads_the_file_layout_and_reads_older_summaries() {
        let path = std::env::temp_dir().join(format!("membank-dir-{}", std::process::id()));
        let _ = fs::remove_dir_all(&path);
        let mut dir = Dir(path.clone());
        let mut message = Message { kind: Kind::User, text: "hi".into(), date: "2026-10-15T01:00:00Z".into(), src: "a".into(), session: "s".into() };
        let (kept, pruned) = (Span { level: 1, first_id: 4, last_id: 7 }, Span { level: 1, first_id: 6, last_id: 7 });
        dir.save_message(0, Some(&message)).unwrap();
        message.text.clear();
        dir.save_message(0, Some(&message)).unwrap();
        dir.save_message(1, None).unwrap();
        dir.save_message(2, None).unwrap();
        dir.save_message(2, Some(&message)).unwrap();
        fs::create_dir_all(path.join("tree")).unwrap();
        fs::write(path.join("tree/2026-10-16.jsonl"), "{\"l\":1,\"i\":3,\"text\":\"older\"}\n").unwrap();
        dir.save_summary(kept, Some("both"), "2026-10-16T00:00:00Z").unwrap();
        dir.save_summary(pruned, Some("gone"), "2026-10-16T00:00:00Z").unwrap();
        dir.save_summary(pruned, None, "2026-10-16T00:00:00Z").unwrap();
        dir.save_order(&[0]).unwrap();
        dir.save_context(&[Node { level: 1, index: 2 }]).unwrap();
        assert_eq!(fs::read_to_string(path.join("main/2026-10-15.jsonl")).unwrap().lines().next(), Some("{\"i\":0,\"kind\":\"user\",\"text\":\"hi\",\"date\":\"2026-10-15T01:00:00Z\",\"src\":\"a\",\"session\":\"s\"}"));
        assert_eq!(fs::read_to_string(path.join("tree/2026-10-16.jsonl")).unwrap().lines().last(), Some("{\"l\":1,\"first\":6,\"last\":7,\"text\":null}"));
        assert_eq!(fs::read_to_string(path.join("view.json")).unwrap(), "[[1,2]]");
        let saved = dir.load().unwrap();
        assert_eq!(fs::read_to_string(path.join("main/undated.jsonl")).unwrap(), "{\"i\":1}\n{\"i\":2}\n");
        assert_eq!(saved.messages.len(), 3);
        assert!(saved.messages[2].is_some());
        assert_eq!(saved.messages[0].as_ref().map(|m| m.text.as_str()), Some(""));
        assert!(saved.messages[1].is_none());
        assert_eq!(saved.order, [0]);
        assert_eq!(saved.summaries, [(kept, "both".to_string())]);
        assert_eq!(saved.context, [Node { level: 1, index: 2 }]);
        fs::remove_dir_all(&path).unwrap();
    }
}
