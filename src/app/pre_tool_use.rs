use anyhow::Result;
use serde_json::Value;

use crate::codex::payload::command_text;
use crate::codex::responses::emit_pretool_deny;

pub fn run(payload: &Value) -> Result<()> {
    if !should_block_git_command(command_text(payload).as_deref()) {
        return Ok(());
    }
    emit_pretool_deny("Commits are owned by Sprocket. Make code changes only.")
}

pub(crate) fn should_block_git_command(command_text: Option<&str>) -> bool {
    let Some(command_text) = command_text else {
        return false;
    };
    let Some(argv) = shlex::split(command_text) else {
        return false;
    };
    let Some(subcommand) = git_subcommand(&argv) else {
        return false;
    };
    if matches!(
        subcommand,
        "add"
            | "commit"
            | "merge"
            | "rebase"
            | "cherry-pick"
            | "push"
            | "tag"
            | "stash"
            | "am"
            | "checkout"
            | "switch"
            | "restore"
            | "pull"
    ) {
        return true;
    }
    subcommand == "reset"
}

fn git_subcommand(argv: &[String]) -> Option<&str> {
    if argv.first().map(String::as_str) != Some("git") {
        return None;
    }
    let mut index = 1;
    while index < argv.len() {
        let token = argv[index].as_str();
        if matches!(
            token,
            "-c" | "-C" | "--git-dir" | "--work-tree" | "--namespace" | "--super-prefix"
        ) {
            index += 2;
            continue;
        }
        if token.starts_with("--") || (token.starts_with('-') && token.len() > 1) {
            index += 1;
            continue;
        }
        return Some(token);
    }
    None
}
