use serde_json::{Value, json};

pub fn session_start(cwd: &std::path::Path, session_id: &str) -> Value {
    json!({
        "cwd": cwd.display().to_string(),
        "session_id": session_id,
    })
}

pub fn baseline(cwd: &std::path::Path, session_id: &str, turn_id: &str) -> Value {
    json!({
        "cwd": cwd.display().to_string(),
        "session_id": session_id,
        "turn_id": turn_id,
    })
}

pub fn checkpoint(cwd: &std::path::Path, session_id: &str, turn_id: &str) -> Value {
    baseline(cwd, session_id, turn_id)
}

pub fn pre_tool_use(cwd: &std::path::Path, command: &str) -> Value {
    json!({
        "cwd": cwd.display().to_string(),
        "tool_input": {
            "command": command
        }
    })
}
