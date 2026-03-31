use anyhow::Result;
use serde_json::json;

pub fn emit_pretool_deny(reason: &str) -> Result<()> {
    println!(
        "{}",
        serde_json::to_string(&json!({
            "hookSpecificOutput": {
                "hookEventName": "PreToolUse",
                "permissionDecision": "deny",
                "permissionDecisionReason": reason,
            }
        }))?
    );
    Ok(())
}

pub fn emit_stop_block(reason: &str) -> Result<()> {
    println!(
        "{}",
        serde_json::to_string(&json!({
            "decision": "block",
            "reason": reason,
        }))?
    );
    Ok(())
}
