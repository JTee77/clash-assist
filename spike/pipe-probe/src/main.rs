//! V5 pipe spike probe (windows-plan r4 §4.1, C-2, S-R9).
//!
//! The probe only *records* facts as one JSON document; it never makes an
//! allow/deny decision. See README.md for the CLI contract and JSON schema.

#![cfg_attr(not(windows), allow(dead_code))]

mod cli;
mod http;
mod report;
#[cfg(windows)]
mod win;

use std::process::ExitCode;

/// Exit code: CLI usage error (no JSON document is produced).
pub const EXIT_USAGE: u8 = 2;
/// Exit code: report emitted on stdout but writing `--out` failed.
pub const EXIT_OUT_FILE: u8 = 1;
/// Exit code: not running on Windows.
pub const EXIT_UNSUPPORTED: u8 = 3;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let cmd = match cli::parse(&args) {
        Ok(cli::Parsed::Help) => {
            println!("{}", cli::USAGE);
            return ExitCode::SUCCESS;
        }
        Ok(cli::Parsed::Version) => {
            println!("pipe-probe {}", env!("CARGO_PKG_VERSION"));
            return ExitCode::SUCCESS;
        }
        Ok(cli::Parsed::Run(cmd)) => cmd,
        Err(e) => {
            eprintln!("pipe-probe: {e}\n\n{}", cli::USAGE);
            return ExitCode::from(EXIT_USAGE);
        }
    };
    run(cmd)
}

#[cfg(windows)]
fn run(cmd: cli::Command) -> ExitCode {
    win::run(cmd)
}

#[cfg(not(windows))]
fn run(cmd: cli::Command) -> ExitCode {
    let _ = cmd;
    eprintln!("pipe-probe: this probe only works on Windows (named pipes, Win32 security APIs)");
    ExitCode::from(EXIT_UNSUPPORTED)
}
