use membank::{Kind, Message};
use serde_json::Value;
use std::io::{self, ErrorKind};

fn main() {
    membank_convert::run("membank-claude", message);
}

fn message(record: Value) -> io::Result<Option<Message>> {
    if record["isSidechain"] == true || record["message"]["model"] == "<synthetic>" {
        return Ok(None);
    }
    let kind = match record["type"].as_str() {
        Some("user") => Kind::User,
        Some("assistant") => Kind::Assistant,
        _ => return Ok(None),
    };
    let content = &record["message"]["content"];
    let text = match content.as_str() {
        Some(text) => text.to_string(),
        None => content.as_array().ok_or_else(|| io::Error::new(ErrorKind::InvalidData, "message content must be text or an array"))?
            .iter().filter_map(|block| match block["type"].as_str() {
                Some("text") => block["text"].as_str(),
                Some("image") => Some("[image]"),
                _ => None,
            }).collect::<Vec<_>>().join("\n"),
    };
    if text.trim().is_empty() {
        return Ok(None);
    }
    let field = |name: &str| record[name].as_str().map(str::to_string).ok_or_else(|| io::Error::new(ErrorKind::InvalidData, format!("message has no {name}")));
    Ok(Some(Message { kind, text, date: field("timestamp")?, src: field("uuid")?, session: field("sessionId")? }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn keeps_text_and_excludes_tools_and_sidechains() {
        let mut record = json!({"type": "assistant", "uuid": "answer", "sessionId": "chat", "timestamp": "2026-10-15", "message": {
            "content": [{"type": "thinking", "thinking": "private"}, {"type": "text", "text": "answer"}]
        }});
        let entry = message(record.clone()).unwrap().unwrap();
        assert_eq!(entry.kind, Kind::Assistant);
        assert_eq!(entry.text, "answer");
        assert_eq!(entry.session, "chat");
        record["isSidechain"] = json!(true);
        assert!(message(record).unwrap().is_none());
        let record = json!({"type": "user", "message": {"content": [{"type": "tool_result", "content": "tool output"}]}});
        assert!(message(record).unwrap().is_none());
    }
}
