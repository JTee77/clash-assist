//! JSON report model. One document per run:
//!
//! ```json
//! { "schema": "pipe-probe/v1", "probe_version": "...", "command": "pipe",
//!   "args": {...}, "context": {...}, "complete": true, "steps": [ Step... ] }
//! ```
//!
//! Every step is `{step, ok, win32_error?, win32_error_name?, ntstatus?, api?,
//! error?, pid?, target?, value?}`. Absent optional fields are omitted.

use serde_json::{json, Map, Value};
use std::io::Write;
use std::path::Path;

pub const SCHEMA: &str = "pipe-probe/v1";

#[derive(Debug, Clone)]
pub struct Step {
    pub step: &'static str,
    pub ok: bool,
    pub win32_error: Option<u32>,
    pub ntstatus: Option<i32>,
    /// The Win32/NT API whose failure produced `win32_error`/`ntstatus`.
    pub api: Option<&'static str>,
    /// Non-Win32 failure description (timeout, not-found, parse error, ...).
    pub error: Option<String>,
    /// Process the step is about (tcp-probe has several; pipe has the server PID).
    pub pid: Option<u32>,
    /// Free-form target label (e.g. the `--set-owner` target).
    pub target: Option<String>,
    pub value: Option<Value>,
}

impl Step {
    fn base(step: &'static str, ok: bool) -> Self {
        Step {
            step,
            ok,
            win32_error: None,
            ntstatus: None,
            api: None,
            error: None,
            pid: None,
            target: None,
            value: None,
        }
    }

    pub fn ok(step: &'static str, value: Value) -> Self {
        Step {
            value: Some(value),
            ..Self::base(step, true)
        }
    }

    pub fn win32(step: &'static str, api: &'static str, code: u32) -> Self {
        Step {
            win32_error: Some(code),
            api: Some(api),
            ..Self::base(step, false)
        }
    }

    pub fn failed(step: &'static str, error: impl Into<String>) -> Self {
        Step {
            error: Some(error.into()),
            ..Self::base(step, false)
        }
    }

    pub fn with_pid(mut self, pid: u32) -> Self {
        self.pid = Some(pid);
        self
    }

    pub fn with_target(mut self, target: impl Into<String>) -> Self {
        self.target = Some(target.into());
        self
    }

    pub fn with_value(mut self, value: Value) -> Self {
        self.value = Some(value);
        self
    }

    pub fn with_ntstatus(mut self, status: i32) -> Self {
        self.ntstatus = Some(status);
        self
    }

    pub fn to_json(&self) -> Value {
        let mut m = Map::new();
        m.insert("step".into(), json!(self.step));
        m.insert("ok".into(), json!(self.ok));
        if let Some(c) = self.win32_error {
            m.insert("win32_error".into(), json!(c));
            if let Some(n) = win32_error_name(c) {
                m.insert("win32_error_name".into(), json!(n));
            }
        }
        if let Some(s) = self.ntstatus {
            m.insert("ntstatus".into(), json!(format!("0x{:08X}", s as u32)));
        }
        if let Some(a) = self.api {
            m.insert("api".into(), json!(a));
        }
        if let Some(e) = &self.error {
            m.insert("error".into(), json!(e));
        }
        if let Some(p) = self.pid {
            m.insert("pid".into(), json!(p));
        }
        if let Some(t) = &self.target {
            m.insert("target".into(), json!(t));
        }
        if let Some(v) = &self.value {
            m.insert("value".into(), v.clone());
        }
        Value::Object(m)
    }
}

/// Symbolic names for the Win32 error codes this spike is expected to see.
pub fn win32_error_name(code: u32) -> Option<&'static str> {
    Some(match code {
        0 => "ERROR_SUCCESS",
        2 => "ERROR_FILE_NOT_FOUND",
        5 => "ERROR_ACCESS_DENIED",
        6 => "ERROR_INVALID_HANDLE",
        31 => "ERROR_GEN_FAILURE",
        87 => "ERROR_INVALID_PARAMETER",
        109 => "ERROR_BROKEN_PIPE",
        122 => "ERROR_INSUFFICIENT_BUFFER",
        161 => "ERROR_BAD_PATHNAME",
        183 => "ERROR_ALREADY_EXISTS",
        231 => "ERROR_PIPE_BUSY",
        232 => "ERROR_NO_DATA",
        233 => "ERROR_PIPE_NOT_CONNECTED",
        535 => "ERROR_PIPE_CONNECTED",
        1307 => "ERROR_INVALID_OWNER",
        1308 => "ERROR_INVALID_PRIMARY_GROUP",
        1332 => "ERROR_NONE_MAPPED",
        1336 => "ERROR_INVALID_ACL",
        1337 => "ERROR_INVALID_SID",
        1338 => "ERROR_INVALID_SECURITY_DESCR",
        1347 => "ERROR_CANT_OPEN_ANONYMOUS",
        _ => return None,
    })
}

#[derive(Debug)]
pub struct Report {
    pub command: &'static str,
    pub args: Value,
    pub context: Value,
    pub steps: Vec<Step>,
    pub complete: bool,
}

impl Report {
    pub fn new(command: &'static str, args: Value) -> Self {
        Report {
            command,
            args,
            context: Value::Null,
            steps: Vec::new(),
            complete: false,
        }
    }

    pub fn push(&mut self, step: Step) {
        self.steps.push(step);
    }

    pub fn to_json(&self) -> Value {
        json!({
            "schema": SCHEMA,
            "probe_version": env!("CARGO_PKG_VERSION"),
            "command": self.command,
            "args": self.args,
            "context": self.context,
            "complete": self.complete,
            "steps": self.steps.iter().map(Step::to_json).collect::<Vec<_>>(),
        })
    }

    pub fn to_pretty(&self) -> String {
        // Serializing a `Value` built from strings/numbers/bools cannot fail;
        // fall back to the compact form rather than panicking regardless.
        let v = self.to_json();
        serde_json::to_string_pretty(&v).unwrap_or_else(|_| v.to_string())
    }
}

/// Write `contents` to `path` atomically-ish (temp file in the same directory, then rename),
/// so a reader never observes a half-written document even if the process is killed.
pub fn write_file_atomic(path: &Path, contents: &str) -> std::io::Result<()> {
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = std::path::PathBuf::from(tmp);
    {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(contents.as_bytes())?;
        f.write_all(b"\n")?;
        f.sync_all()?;
    }
    std::fs::rename(&tmp, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn step_omits_absent_fields() {
        let s = Step::ok("owner", json!({"sid": "S-1-5-18"})).to_json();
        assert_eq!(
            s,
            json!({"step": "owner", "ok": true, "value": {"sid": "S-1-5-18"}})
        );
        let s = Step::win32("open", "CreateFileW", 5).to_json();
        assert_eq!(
            s,
            json!({"step": "open", "ok": false, "win32_error": 5,
                   "win32_error_name": "ERROR_ACCESS_DENIED", "api": "CreateFileW"})
        );
        let s = Step::failed("l2_2", "x")
            .with_ntstatus(0xC0000022u32 as i32)
            .with_pid(4)
            .to_json();
        assert_eq!(s["ntstatus"], json!("0xC0000022"));
        assert_eq!(s["pid"], json!(4));
    }

    #[test]
    fn report_shape() {
        let mut r = Report::new("pipe", json!({"name": "x"}));
        r.push(Step::failed("a", "b"));
        r.complete = true;
        let v = r.to_json();
        assert_eq!(v["schema"], json!(SCHEMA));
        assert_eq!(v["steps"].as_array().map(Vec::len), Some(1));
        assert_eq!(v["complete"], json!(true));
    }
}
