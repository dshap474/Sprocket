use std::io::Write;
use std::path::Path;
use std::process::{Command, Output, Stdio};

use serde_json::Value;

use super::env::HermeticEnv;

#[derive(Debug)]
pub struct CmdOutput {
    pub output: Output,
}

impl CmdOutput {
    pub fn stdout_string(&self) -> String {
        String::from_utf8_lossy(&self.output.stdout).to_string()
    }

    pub fn stderr_string(&self) -> String {
        String::from_utf8_lossy(&self.output.stderr).to_string()
    }

    pub fn assert_success(&self) {
        assert!(
            self.output.status.success(),
            "command failed with {}\nstdout:\n{}\nstderr:\n{}",
            self.output
                .status
                .code()
                .map(|code| code.to_string())
                .unwrap_or_else(|| "signal".to_string()),
            self.stdout_string(),
            self.stderr_string()
        );
    }
}

fn base_command(cwd: &Path, hermetic: &HermeticEnv) -> Command {
    hermetic.ensure_dirs();
    let mut command = Command::new(env!("CARGO_BIN_EXE_sprocket"));
    command.current_dir(cwd);
    for (key, value) in hermetic.pairs() {
        command.env(key, value);
    }
    command
}

pub fn run(
    cwd: &Path,
    hermetic: &HermeticEnv,
    args: &[&str],
    payload: Option<&Value>,
    extra_env: &[(&str, String)],
) -> CmdOutput {
    let mut command = base_command(cwd, hermetic);
    command
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (key, value) in extra_env {
        command.env(key, value);
    }
    if payload.is_some() {
        command.stdin(Stdio::piped());
    }
    let mut child = command.spawn().unwrap();
    if let Some(value) = payload {
        let body = serde_json::to_vec(value).unwrap();
        child.stdin.as_mut().unwrap().write_all(&body).unwrap();
    }
    CmdOutput {
        output: child.wait_with_output().unwrap(),
    }
}
