use std::io::{self, Read};
use std::path::PathBuf;

use anyhow::Result;
use serde_json::Value;

pub fn read_payload() -> Result<Value> {
    let mut raw = String::new();
    io::stdin().read_to_string(&mut raw)?;
    if raw.trim().is_empty() {
        return Ok(Value::Object(Default::default()));
    }
    Ok(serde_json::from_str(&raw)?)
}

pub fn cwd(payload: &Value) -> Option<PathBuf> {
    find_first_string(payload, &["cwd", "working_dir", "workingDirectory"]).map(PathBuf::from)
}

pub fn session_id(payload: &Value) -> String {
    explicit_session_id(payload)
        .unwrap_or("session-current")
        .to_string()
}

pub fn explicit_session_id(payload: &Value) -> Option<&str> {
    find_first_string(payload, &["session_id", "sessionId"])
}

pub fn turn_id(payload: &Value) -> String {
    find_first_string(
        payload,
        &[
            "turn_id",
            "turnId",
            "conversationTurnId",
            "request_id",
            "requestId",
        ],
    )
    .unwrap_or("turn-current")
    .to_string()
}

pub fn command_text(payload: &Value) -> Option<String> {
    for (parent, child) in [
        ("tool_input", "command"),
        ("toolInput", "command"),
        ("input", "command"),
        ("tool_input", "cmd"),
        ("toolInput", "cmd"),
        ("input", "cmd"),
    ] {
        if let Some(value) = payload
            .get(parent)
            .and_then(|parent_value| parent_value.get(child))
            .and_then(Value::as_str)
        {
            return Some(value.to_string());
        }
    }
    find_first_string(payload, &["command", "cmd"]).map(ToOwned::to_owned)
}

fn find_first_string<'a>(value: &'a Value, keys: &[&str]) -> Option<&'a str> {
    match value {
        Value::Object(map) => {
            for key in keys {
                if let Some(string) = map.get(*key).and_then(Value::as_str) {
                    return Some(string);
                }
            }
            for nested in map.values() {
                if let Some(found) = find_first_string(nested, keys) {
                    return Some(found);
                }
            }
            None
        }
        Value::Array(items) => items.iter().find_map(|item| find_first_string(item, keys)),
        _ => None,
    }
}
