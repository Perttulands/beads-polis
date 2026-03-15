//! beads-v2 (br) — Event-sourced work tracker for Polis.
//!
//! JSONL is the source of truth. SQLite is a derived, disposable index.
//! See PRD.md for the full design.

use beads_polis::cli;
use clap::Parser;
use std::io::IsTerminal;

fn main() {
    let parsed = cli::Cli::parse();
    let json_mode = parsed.json;

    match cli::dispatch(&parsed) {
        Ok(Some(value)) => {
            if cli::suppress_stdout(&parsed.command) {
                // V1 compat: suppress success output while preserving stderr errors.
            } else if json_mode {
                println!(
                    "{}",
                    serde_json::to_string(&value).expect("failed to serialize output")
                );
            } else {
                // Human-friendly formatting
                cli::format_human(&parsed.command, &value);
            }
        }
        Ok(None) => {}
        Err(e) => {
            let exit_code = e.exit_code();
            if matches!(&e, cli::CliError::LintFailed(_)) {
                // Lint results go to stdout (not stderr) for machine consumption
                println!(
                    "{}",
                    serde_json::to_string(&e.to_json()).expect("failed to serialize lint result")
                );
            } else if json_mode || !std::io::stderr().is_terminal() {
                eprintln!(
                    "{}",
                    serde_json::to_string(&e.to_json()).expect("failed to serialize error")
                );
            } else {
                eprintln!("\x1b[31merror:\x1b[0m {e}");
            }
            std::process::exit(exit_code);
        }
    }
}
