//! Hand-rolled argument parser (no extra dependencies). Platform-independent
//! so it is unit-tested on every host.

use std::path::PathBuf;
use std::time::Duration;

pub const USAGE: &str = "\
usage:
  pipe-probe pipe <name> [--timeout-ms <ms>] [--out <file>]
  pipe-probe tcp-probe <port> [--out <file>]
  pipe-probe squat <name> <sddl|-> [--set-owner <sid|SY|BA>]... [--hold <secs>]
                   [--ready-file <file>] [--out <file>]
  pipe-probe --help | --version

<name> is either a bare pipe name (`verge-mihomo-...`) or a full `\\\\.\\pipe\\...` path.
The probe prints exactly one JSON document to stdout and never decides allow/deny.";

/// Default deadline for opening the pipe (ERROR_PIPE_BUSY retries) and for the HTTP exchange.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_millis(5000);
/// Upper bound for `--hold` so a forgotten squatter cannot outlive a CI job by much.
pub const MAX_HOLD_SECS: u64 = 3600;
/// Upper bound for `--timeout-ms`.
pub const MAX_TIMEOUT_MS: u64 = 120_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Parsed {
    Help,
    Version,
    Run(Command),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Command {
    pub kind: Kind,
    pub out: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Kind {
    Pipe {
        name: String,
        timeout: Duration,
    },
    TcpProbe {
        port: u16,
    },
    Squat {
        name: String,
        /// `None` when the user passed `-` (create with the default security descriptor).
        sddl: Option<String>,
        set_owner: Vec<String>,
        hold: Duration,
        ready_file: Option<PathBuf>,
    },
}

impl Kind {
    pub fn name(&self) -> &'static str {
        match self {
            Kind::Pipe { .. } => "pipe",
            Kind::TcpProbe { .. } => "tcp-probe",
            Kind::Squat { .. } => "squat",
        }
    }
}

/// Full `\\.\pipe\` path for a user-supplied name. A name that already starts
/// with the local pipe prefix (any case) is returned unchanged.
pub fn full_pipe_name(name: &str) -> String {
    const PREFIX: &str = r"\\.\pipe\";
    if name.len() >= PREFIX.len()
        && name.is_char_boundary(PREFIX.len())
        && name[..PREFIX.len()].eq_ignore_ascii_case(PREFIX)
    {
        name.to_string()
    } else {
        format!("{PREFIX}{name}")
    }
}

pub fn parse(args: &[String]) -> Result<Parsed, String> {
    let mut it = args.iter();
    let sub = match it.next() {
        None => return Err("missing subcommand".into()),
        Some(s) => s.as_str(),
    };
    match sub {
        "-h" | "--help" | "help" => return Ok(Parsed::Help),
        "-V" | "--version" => return Ok(Parsed::Version),
        _ => {}
    }

    let mut positional: Vec<String> = Vec::new();
    let mut out: Option<PathBuf> = None;
    let mut timeout: Option<Duration> = None;
    let mut set_owner: Vec<String> = Vec::new();
    let mut hold: Option<Duration> = None;
    let mut ready_file: Option<PathBuf> = None;

    while let Some(a) = it.next() {
        let mut value = |flag: &str| -> Result<String, String> {
            it.next()
                .cloned()
                .ok_or_else(|| format!("{flag} requires a value"))
        };
        match a.as_str() {
            "--out" => set_once(&mut out, PathBuf::from(value("--out")?), "--out")?,
            "--timeout-ms" => {
                let ms = parse_u64(&value("--timeout-ms")?, "--timeout-ms", 1, MAX_TIMEOUT_MS)?;
                set_once(&mut timeout, Duration::from_millis(ms), "--timeout-ms")?;
            }
            "--set-owner" => {
                let v = value("--set-owner")?;
                if v.is_empty() {
                    return Err("--set-owner requires a non-empty SID or alias".into());
                }
                set_owner.push(v);
            }
            "--hold" => {
                let s = parse_u64(&value("--hold")?, "--hold", 0, MAX_HOLD_SECS)?;
                set_once(&mut hold, Duration::from_secs(s), "--hold")?;
            }
            "--ready-file" => set_once(
                &mut ready_file,
                PathBuf::from(value("--ready-file")?),
                "--ready-file",
            )?,
            // `-` is a positional (squat's "no SDDL"), every other dash-arg is unknown.
            s if s.starts_with("--") => return Err(format!("unknown option {s}")),
            _ => positional.push(a.clone()),
        }
    }

    let only = |allowed: &[&str]| -> Result<(), String> {
        let used = [
            ("--timeout-ms", timeout.is_some()),
            ("--set-owner", !set_owner.is_empty()),
            ("--hold", hold.is_some()),
            ("--ready-file", ready_file.is_some()),
        ];
        for (flag, present) in used {
            if present && !allowed.contains(&flag) {
                return Err(format!("{flag} is not valid for `{sub}`"));
            }
        }
        Ok(())
    };

    let kind = match sub {
        "pipe" => {
            only(&["--timeout-ms"])?;
            let [name] = take::<1>(positional, "pipe <name>")?;
            Kind::Pipe {
                name: checked_name(name)?,
                timeout: timeout.unwrap_or(DEFAULT_TIMEOUT),
            }
        }
        "tcp-probe" => {
            only(&[])?;
            let [port] = take::<1>(positional, "tcp-probe <port>")?;
            let port = parse_u64(&port, "<port>", 1, u64::from(u16::MAX))?;
            Kind::TcpProbe {
                port: u16::try_from(port).map_err(|_| "<port> out of range".to_string())?,
            }
        }
        "squat" => {
            only(&["--set-owner", "--hold", "--ready-file"])?;
            let [name, sddl] = take::<2>(positional, "squat <name> <sddl|->")?;
            if sddl.is_empty() {
                return Err("<sddl> must be an SDDL string or `-`".into());
            }
            Kind::Squat {
                name: checked_name(name)?,
                sddl: if sddl == "-" { None } else { Some(sddl) },
                set_owner,
                hold: hold.unwrap_or(Duration::ZERO),
                ready_file,
            }
        }
        other => return Err(format!("unknown subcommand `{other}`")),
    };
    Ok(Parsed::Run(Command { kind, out }))
}

fn take<const N: usize>(v: Vec<String>, shape: &str) -> Result<[String; N], String> {
    let n = v.len();
    v.try_into()
        .map_err(|_| format!("expected `{shape}` ({N} positional argument(s)), got {n}"))
}

fn set_once<T>(slot: &mut Option<T>, v: T, flag: &str) -> Result<(), String> {
    if slot.is_some() {
        return Err(format!("{flag} given more than once"));
    }
    *slot = Some(v);
    Ok(())
}

fn parse_u64(s: &str, what: &str, min: u64, max: u64) -> Result<u64, String> {
    let v: u64 = s
        .parse()
        .map_err(|_| format!("{what}: `{s}` is not a non-negative integer"))?;
    if v < min || v > max {
        return Err(format!("{what}: {v} is outside {min}..={max}"));
    }
    Ok(v)
}

fn checked_name(name: String) -> Result<String, String> {
    if name.is_empty() {
        return Err("pipe name must not be empty".into());
    }
    if name.chars().any(|c| c == '\0') {
        return Err("pipe name must not contain NUL".into());
    }
    Ok(full_pipe_name(&name))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(a: &[&str]) -> Result<Parsed, String> {
        parse(&a.iter().map(|s| s.to_string()).collect::<Vec<_>>())
    }

    #[test]
    fn pipe_defaults_and_prefix() {
        let Ok(Parsed::Run(c)) = p(&["pipe", "verge-mihomo"]) else {
            panic!()
        };
        assert_eq!(
            c.kind,
            Kind::Pipe {
                name: r"\\.\pipe\verge-mihomo".into(),
                timeout: DEFAULT_TIMEOUT
            }
        );
        assert_eq!(c.out, None);
        let Ok(Parsed::Run(c)) = p(&[
            "pipe",
            r"\\.\PIPE\x",
            "--timeout-ms",
            "250",
            "--out",
            "o.json",
        ]) else {
            panic!()
        };
        assert_eq!(
            c.kind,
            Kind::Pipe {
                name: r"\\.\PIPE\x".into(),
                timeout: Duration::from_millis(250)
            }
        );
        assert_eq!(c.out, Some(PathBuf::from("o.json")));
    }

    #[test]
    fn tcp_probe_port() {
        let Ok(Parsed::Run(c)) = p(&["tcp-probe", "19097"]) else {
            panic!()
        };
        assert_eq!(c.kind, Kind::TcpProbe { port: 19097 });
        assert!(p(&["tcp-probe", "0"]).is_err());
        assert!(p(&["tcp-probe", "65536"]).is_err());
        assert!(p(&["tcp-probe", "1", "--hold", "3"]).is_err());
    }

    #[test]
    fn squat_forms() {
        let Ok(Parsed::Run(c)) = p(&[
            "squat",
            "n",
            "-",
            "--set-owner",
            "SY",
            "--set-owner",
            "BA",
            "--hold",
            "30",
        ]) else {
            panic!()
        };
        assert_eq!(
            c.kind,
            Kind::Squat {
                name: r"\\.\pipe\n".into(),
                sddl: None,
                set_owner: vec!["SY".into(), "BA".into()],
                hold: Duration::from_secs(30),
                ready_file: None,
            }
        );
        let Ok(Parsed::Run(c)) = p(&["squat", "n", "O:SYD:(A;;GA;;;WD)"]) else {
            panic!()
        };
        assert!(
            matches!(c.kind, Kind::Squat { sddl: Some(ref s), .. } if s == "O:SYD:(A;;GA;;;WD)")
        );
    }

    #[test]
    fn errors() {
        assert!(p(&[]).is_err());
        assert!(p(&["pipe"]).is_err());
        assert!(p(&["pipe", "a", "b"]).is_err());
        assert!(p(&["pipe", "a", "--bogus"]).is_err());
        assert!(p(&["pipe", "a", "--out"]).is_err());
        assert!(p(&["pipe", "a", "--out", "x", "--out", "y"]).is_err());
        assert!(p(&["pipe", "a", "--set-owner", "SY"]).is_err());
        assert!(p(&["squat", "n"]).is_err());
        assert!(p(&["squat", "n", "-", "--hold", "999999"]).is_err());
        assert!(p(&["nope"]).is_err());
        assert_eq!(p(&["--help"]), Ok(Parsed::Help));
    }
}
