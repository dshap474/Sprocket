use std::ffi::OsString;
use std::path::{Path, PathBuf};

use anyhow::{Result, anyhow, bail};
use serde::Serialize;

use crate::app;
use crate::app::support::{load_policy, unsupported_repo_reason};
use crate::codex::payload::read_payload;
use crate::domain::repopath::RepoPath;
use crate::domain::session_tracker::{BlockedPath, CommitBlockReason, CommitPlan, SessionTracker};
use crate::engine::observe::capture_strict_snapshot;
use crate::engine::session_commit::{BuildCommitPlanInput, build_commit_plan};
use crate::engine::{init_stream::resolve_stream, session_commit::stable_session_id};
use crate::infra::clock::{Clock, SystemClock};
use crate::infra::git::GitBackend;
use crate::infra::git_cli::GitCli;
use crate::infra::store::{RuntimeLayout, Stores, find_session_tracker};

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
        "session" => run_session(&filtered[1..]),
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

fn run_session(args: &[OsString]) -> Result<()> {
    let Some(subcommand) = args.first().map(|arg| arg.to_string_lossy().to_string()) else {
        bail!(
            "usage: sprocket session <status|plan-commit|commit-now> --session-id <id> [--target-repo <path>]"
        );
    };

    let mut target_repo: Option<PathBuf> = None;
    let mut session_id: Option<String> = None;
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
            "--session-id" => {
                index += 1;
                let value = args
                    .get(index)
                    .ok_or_else(|| anyhow!("--session-id requires a value"))?;
                session_id = Some(value.to_string_lossy().to_string());
            }
            unknown => bail!("unknown session argument: {unknown}"),
        }
        index += 1;
    }

    let session_id = session_id.ok_or_else(|| anyhow!("--session-id is required"))?;
    if !stable_session_id(&session_id) {
        bail!("session attribution requires a stable session id");
    }

    let repo = resolve_repo_from_target(target_repo.as_deref())?;
    let git = GitCli::discover(&repo)?;
    let (_, current_stream) = resolve_stream(&git)?;
    let runtime = RuntimeLayout::from_git(&git)?;
    let found = find_session_tracker(&runtime, &session_id)?;
    let stream_id = found
        .as_ref()
        .map(|(stream_id, _)| stream_id.clone())
        .unwrap_or_else(|| current_stream.stream_id.clone());
    let stores = Stores::for_stream(runtime, &stream_id);
    let now = SystemClock.now_unix();
    let policy = load_policy(&git, &stores, &current_stream, "session", now)?;

    match subcommand.as_str() {
        "status" => {
            let tracker = found.as_ref().map(|(_, tracker)| tracker);
            println!(
                "{}",
                serde_json::to_string_pretty(&SessionStatusReport::from_tracker(tracker))?
            );
            Ok(())
        }
        "plan-commit" => {
            let (head, stream) = resolve_stream(&git)?;
            let snapshot = capture_strict_snapshot(git.repo_root(), &git, &policy)?;
            stores.manifests.put(&snapshot.manifest_id, &snapshot)?;
            let mut plan = build_commit_plan(BuildCommitPlanInput {
                git: &git,
                stores: &stores,
                stream: &stream,
                head: &head,
                current_snapshot: &snapshot,
                tracker: found.as_ref().map(|(_, tracker)| tracker),
                now,
                turn_threshold: policy.checkpoint.turn_threshold,
                file_threshold: policy.checkpoint.file_threshold,
                age_seconds: (policy.checkpoint.age_minutes as i64) * 60,
            })?;
            if unsupported_repo_reason(&git, &head, &git.repo_state()?, &policy, git.repo_root())?
                .is_some()
            {
                plan.safe_to_commit = false;
                plan.blocking_reasons
                    .push(CommitBlockReason::UnsupportedRepoState);
                plan.blocking_reasons.sort();
                plan.blocking_reasons.dedup();
            }
            println!(
                "{}",
                serde_json::to_string_pretty(&SessionCommitPlanReport::from_plan(&plan))?
            );
            Ok(())
        }
        "commit-now" => {
            println!("session commit-now is not implemented yet.");
            Ok(())
        }
        other => bail!("unknown session subcommand: {other}"),
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
    println!("  session status                Show tracker state for a session");
    println!("  session plan-commit           Compute a safe commit plan for a session");
    println!("  session commit-now            Not implemented yet");
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

#[derive(Serialize)]
struct SessionStatusReport {
    found: bool,
    session_id: Option<String>,
    stream_id: Option<String>,
    epoch: Option<u32>,
    status: Option<String>,
    touched_paths: Vec<TrackedPathReport>,
}

impl SessionStatusReport {
    fn from_tracker(tracker: Option<&SessionTracker>) -> Self {
        let Some(tracker) = tracker else {
            return Self {
                found: false,
                session_id: None,
                stream_id: None,
                epoch: None,
                status: None,
                touched_paths: Vec::new(),
            };
        };
        let mut touched_paths = tracker
            .touched_paths
            .iter()
            .map(|(path, state)| TrackedPathReport {
                path: path.display_lossy(),
                claim_state: format!("{:?}", state.claim_state).to_lowercase(),
                other_sessions: state.other_sessions.clone(),
            })
            .collect::<Vec<_>>();
        touched_paths.sort_by(|left, right| left.path.cmp(&right.path));

        Self {
            found: true,
            session_id: Some(tracker.session_id.clone()),
            stream_id: Some(tracker.stream_id.clone()),
            epoch: Some(tracker.epoch),
            status: Some(format!("{:?}", tracker.status).to_lowercase()),
            touched_paths,
        }
    }
}

#[derive(Serialize)]
struct TrackedPathReport {
    path: String,
    claim_state: String,
    other_sessions: Vec<String>,
}

#[derive(Serialize)]
struct SessionCommitPlanReport {
    session_id: String,
    stream_id: String,
    epoch: u32,
    thresholds_met: bool,
    safe_to_commit: bool,
    eligible_paths: Vec<String>,
    blocked_paths: Vec<BlockedPathReport>,
    blocking_reasons: Vec<String>,
}

impl SessionCommitPlanReport {
    fn from_plan(plan: &CommitPlan) -> Self {
        let mut eligible_paths = plan
            .eligible_paths
            .iter()
            .map(RepoPath::display_lossy)
            .collect::<Vec<_>>();
        eligible_paths.sort();
        let mut blocked_paths = plan
            .blocked_paths
            .iter()
            .map(BlockedPathReport::from_blocked)
            .collect::<Vec<_>>();
        blocked_paths.sort_by(|left, right| left.path.cmp(&right.path));

        Self {
            session_id: plan.session_id.clone(),
            stream_id: plan.stream_id.clone(),
            epoch: plan.epoch,
            thresholds_met: plan.thresholds_met,
            safe_to_commit: plan.safe_to_commit,
            eligible_paths,
            blocked_paths,
            blocking_reasons: plan.blocking_reasons.iter().map(reason_to_string).collect(),
        }
    }
}

#[derive(Serialize)]
struct BlockedPathReport {
    path: String,
    reasons: Vec<String>,
}

impl BlockedPathReport {
    fn from_blocked(blocked: &BlockedPath) -> Self {
        Self {
            path: blocked.path.display_lossy(),
            reasons: blocked.reasons.iter().map(reason_to_string).collect(),
        }
    }
}

fn reason_to_string(reason: &CommitBlockReason) -> String {
    match reason {
        CommitBlockReason::UnstableSessionId => "unstable_session_id",
        CommitBlockReason::MissingTracker => "missing_tracker",
        CommitBlockReason::HeadMoved => "head_moved",
        CommitBlockReason::StagedChangesPresent => "staged_changes_present",
        CommitBlockReason::UnsupportedRepoState => "unsupported_repo_state",
        CommitBlockReason::ThresholdsNotMet => "thresholds_not_met",
        CommitBlockReason::PathInheritedDirty => "path_inherited_dirty",
        CommitBlockReason::PathContended => "path_contended",
        CommitBlockReason::ExternalInterference => "external_interference",
    }
    .to_string()
}
