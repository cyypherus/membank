use membank::{Kind, Message};
use serde_json::Value;
use std::io::{self, ErrorKind};

fn main() {
    membank_convert::run("membank-codex", message);
}

fn message(record: Value) -> io::Result<Option<Message>> {
    let payload = &record["payload"];
    if record["type"] != "event_msg" || payload["type"] != "item_completed" {
        return Ok(None);
    }
    let item = &payload["item"];
    let kind = match item["type"].as_str() {
        Some("UserMessage") => Kind::User,
        Some("AgentMessage") => Kind::Assistant,
        _ => return Ok(None),
    };
    let (src, session) = match (payload["thread_id"].as_str(), payload["turn_id"].as_str(), item["id"].as_str()) {
        (Some(thread), Some(turn), Some(id)) if !thread.is_empty() && !turn.is_empty() && !id.is_empty() => (format!("codex:{thread}:{turn}:{id}"), format!("codex:{thread}")),
        _ => return Err(io::Error::new(ErrorKind::InvalidData, "Codex message is missing its identity")),
    };
    let content = &item["content"];
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
    let date = record["timestamp"].as_str().ok_or_else(|| io::Error::new(ErrorKind::InvalidData, "message has no timestamp"))?;
    Ok(Some(Message { kind, text, date: date.to_string(), src, session }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn reads_completed_messages_and_rejects_missing_identity() {
        let mut record = json!({
            "type": "event_msg", "timestamp": "2026-10-15T01:00:00Z",
            "payload": {"type": "item_completed", "thread_id": "chat", "turn_id": "turn",
                "item": {"type": "UserMessage", "id": "item-1", "content": [{"type": "text", "text": "question"}]}}
        });
        let entry = message(record.clone()).unwrap().unwrap();
        assert_eq!(entry.kind, Kind::User);
        assert_eq!(entry.text, "question");
        assert_eq!(entry.src, "codex:chat:turn:item-1");
        assert_eq!(entry.session, "codex:chat");
        record["payload"]["item"]["type"] = json!("AgentMessage");
        assert_eq!(message(record.clone()).unwrap().unwrap().kind, Kind::Assistant);
        record["payload"]["item"]["id"] = Value::Null;
        assert!(message(record).is_err());
        for record in [
            json!({"type": "response_item", "payload": {"type": "message", "role": "user", "content": "injected"}}),
            json!({"type": "event_msg", "payload": {"type": "item_completed", "item": {"type": "Reasoning", "content": "private"}}}),
        ] {
            assert!(message(record).unwrap().is_none());
        }
    }
}
