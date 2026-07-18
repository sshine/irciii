//! Git `commit-msg` hook: check that a commit message follows conventional commits.

use std::path::PathBuf;
use std::process::ExitCode;

use clap::Parser;
use conventional_commit_check::{DEFAULT_TYPES, Policy, validate};

/// Check that a commit message follows conventional commits.
#[derive(Parser)]
#[command(version, about)]
struct Args {
    /// Path to the commit message file (as passed to the commit-msg hook).
    commit_msg_file: PathBuf,

    /// Require a scope: '<type>(<scope>): <description>'.
    #[arg(long)]
    require_scope: bool,

    /// Comma-separated set of allowed commit types.
    #[arg(long, value_delimiter = ',', default_values_t = DEFAULT_TYPES.iter().map(|s| s.to_string()))]
    types: Vec<String>,
}

fn main() -> ExitCode {
    let args = Args::parse();
    let policy = Policy {
        types: args.types,
        require_scope: args.require_scope,
    };
    let message = match std::fs::read_to_string(&args.commit_msg_file) {
        Ok(message) => message,
        Err(err) => {
            eprintln!(
                "conventional-commit-check: cannot read {}: {err}",
                args.commit_msg_file.display()
            );
            return ExitCode::FAILURE;
        }
    };
    match validate(&message, &policy) {
        Ok(()) => ExitCode::SUCCESS,
        Err(violation) => {
            eprintln!("conventional-commit-check: {violation}");
            ExitCode::FAILURE
        }
    }
}
