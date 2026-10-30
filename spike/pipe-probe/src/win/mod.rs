//! Windows implementation. Every subcommand fills one `Report` and prints it once.

mod context;
mod pipe;
mod process;
mod security;
mod sid;
mod squat;
mod tcp;
mod util;

use std::path::Path;
use std::process::ExitCode;

use serde_json::json;

use crate::cli::{Command, Kind};
use crate::report::{write_file_atomic, Report, Step};

pub fn run(cmd: Command) -> ExitCode {
    let args = match &cmd.kind {
        Kind::Pipe { name, timeout } => {
            json!({ "name": name, "timeout_ms": timeout.as_millis() as u64 })
        }
        Kind::TcpProbe { port } => json!({ "port": port }),
        Kind::Squat {
            name,
            sddl,
            set_owner,
            hold,
            ready_file,
        } => json!({
            "name": name,
            "sddl": sddl,
            "set_owner": set_owner,
            "hold_secs": hold.as_secs(),
            "ready_file": ready_file.as_ref().map(|p| p.display().to_string()),
        }),
    };
    let mut report = Report::new(cmd.kind.name(), args);
    let out = cmd.out.as_deref();

    let ctx = context::collect();
    report.context = ctx.json.clone();

    match tokio::runtime::Builder::new_current_thread()
        .enable_io()
        .enable_time()
        .build()
    {
        Err(e) => report.push(Step::failed("runtime", e.to_string())),
        Ok(rt) => {
            let checkpoint = |r: &Report| {
                if let Some(p) = out {
                    // Best effort; the final write below reports failures.
                    let _ = write_file_atomic(p, &r.to_pretty());
                }
            };
            match &cmd.kind {
                Kind::Pipe { name, timeout } => {
                    rt.block_on(pipe::run(name, *timeout, &ctx, &mut report))
                }
                Kind::TcpProbe { port } => tcp::run(*port, &ctx, &mut report),
                Kind::Squat {
                    name,
                    sddl,
                    set_owner,
                    hold,
                    ready_file,
                } => rt.block_on(squat::run(
                    squat::Args {
                        name,
                        sddl: sddl.as_deref(),
                        set_owner,
                        hold: *hold,
                        ready_file: ready_file.as_deref(),
                    },
                    &ctx,
                    &mut report,
                    &checkpoint,
                )),
            }
        }
    }

    report.complete = true;
    emit(&report, out)
}

fn emit(report: &Report, out: Option<&Path>) -> ExitCode {
    let text = report.to_pretty();
    println!("{text}");
    if let Some(p) = out {
        if let Err(e) = write_file_atomic(p, &text) {
            eprintln!("pipe-probe: failed to write --out {}: {e}", p.display());
            return ExitCode::from(crate::EXIT_OUT_FILE);
        }
    }
    ExitCode::SUCCESS
}
