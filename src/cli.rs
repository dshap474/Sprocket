use std::ffi::OsString;
use std::path::{Path, PathBuf};

use anyhow::{Result, anyhow, bail};

use crate::app;
use crate::codex::payload::read_payload;
use crate::infra::git_cli::GitCli;

const SPROCKET_HOOK_MARKER: &str = "--sprocket-managed";

pub fn run<I>(args: I) -> Result<()>
where
    I: IntoIterator<Item = String>,
{
    let mut args = args.into_iter();
    let _bin = args.next();
    dispatch(args.map(OsString::from).collect())
}

fn dispatch(args: Vec<OsString>) -> Result<()> {
    let filtered: Vec<OsString> = args
        .into_iter()
        .filter(|arg| arg != SPROCKET_HOOK_MARKER)
        .collect();

    if filtered.is_empty() {
        print_help();
        return Ok(());
    }

    match filtered[0].to_string_lossy().as_ref() {
        "install" => run_install(&filtered[1..]),
        "hook" => run_hook(&filtered[1..]),
        "--help" | "-h" => {
            print_help();
            Ok(())
        }
        "--version" | "-V" => {
            println!("sprocket {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        "init" | "migrate" | "doctor" | "repair" | "validate" => {
            println!("{} is not implemented yet.", filtered[0].to_string_lossy());
            Ok(())
        }
        other => bail!("unknown command: {other}"),
    }
}

fn run_install(args: &[OsString]) -> Result<()> {
    if args.first().map(|arg| arg.to_string_lossy()) != Some("codex".into()) {
        bail!("usage: sprocket install codex [--target-repo <path>]");
    }

    let mut target_repo: Option<PathBuf> = None;
    let mut index = 1;
    while index < args.len() {
        match args[index].to_string_lossy().as_ref() {
            "--target-repo" => {
                index += 1;
                let value = args
                    .get(index)
                    .ok_or_else(|| anyhow!("--target-repo requires a path"))?;
                target_repo = Some(PathBuf::from(value));
            }
            unknown => bail!("unknown install argument: {unknown}"),
        }
        index += 1;
    }

    let repo = resolve_repo_from_target(target_repo.as_deref())?;
    app::install_codex::run(&repo)?;
    println!("Installed Sprocket Codex adapter into {}", repo.display());
    Ok(())
}

fn run_hook(args: &[OsString]) -> Result<()> {
    if args.len() < 2 || args[0].to_string_lossy() != "codex" {
        bail!("usage: sprocket hook codex <session-start|baseline|pre-tool-use|checkpoint>");
    }

    let payload = read_payload()?;
    match args[1].to_string_lossy().as_ref() {
        "session-start" => app::session_start::run(&payload),
        "baseline" => app::baseline::run(&payload),
        "pre-tool-use" => app::pre_tool_use::run(&payload),
        "checkpoint" => app::checkpoint::run(&payload),
        unknown => bail!("unknown codex hook subcommand: {unknown}"),
    }
}

fn print_help() {
    println!("Sprocket");
    println!("Hidden-checkpoint engine for Codex-driven repositories.");
    println!();
    println!("USAGE:");
    println!("  sprocket <COMMAND>");
    println!();
    println!("COMMANDS:");
    println!("  install codex                 Install or update the Codex adapter");
    println!("  hook codex session-start      Observe a session start");
    println!("  hook codex baseline           Capture a turn baseline");
    println!("  hook codex pre-tool-use       Block direct git mutations");
    println!("  hook codex checkpoint         Evaluate hidden checkpoint state");
    println!("  init                          Not implemented yet");
    println!("  migrate                       Not implemented yet");
    println!("  doctor                        Not implemented yet");
    println!("  repair                        Not implemented yet");
    println!("  validate                      Not implemented yet");
}

pub fn resolve_repo_from_target(target: Option<&Path>) -> Result<PathBuf> {
    let cwd = target
        .map(PathBuf::from)
        .unwrap_or(std::env::current_dir()?);
    Ok(GitCli::discover(&cwd)?.repo_root().to_path_buf())
}

pub fn hook_marker() -> &'static str {
    SPROCKET_HOOK_MARKER
}
