#![cfg_attr(not(unix), allow(dead_code))]

#[cfg(not(unix))]
compile_error!("Sprocket currently supports Unix targets only.");

pub mod app;
mod cli;
pub mod codex;
pub mod domain;
pub mod engine;
pub mod infra;

use anyhow::Result;

pub fn run<I>(args: I) -> Result<()>
where
    I: IntoIterator<Item = String>,
{
    cli::run(args)
}

#[cfg(test)]
mod tests;
