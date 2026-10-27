//! Python semantics the network modules depend on: exceptions that reach a
//! client as text (`type(e).__name__`, `str(e)`), `str(value)` of a JSON
//! value, and the subprocess boundary the Python crosses with
//! `subprocess.run`.
//!
//! The Python lets exceptions travel: an `nmcli` failure deep inside the Wi-Fi
//! manager ends up as a handler's `400 "failed to connect to wifi:
//! UnspecifiedException"`. [`PyError`] carries the same two strings so the
//! Rust handlers can answer the same way.

use std::io::Read;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::Value;

/// A Python exception: its class name and `str(e)`.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{message}")]
pub struct PyError {
    pub kind: &'static str,
    pub message: String,
}

pub type PyResult<T> = Result<T, PyError>;

impl PyError {
    pub fn new(kind: &'static str, message: impl Into<String>) -> PyError {
        PyError {
            kind,
            message: message.into(),
        }
    }

    #[cfg(test)]
    pub fn exception(message: impl Into<String>) -> PyError {
        PyError::new("Exception", message)
    }

    pub fn type_error(message: impl Into<String>) -> PyError {
        PyError::new("TypeError", message)
    }

    pub fn value_error(message: impl Into<String>) -> PyError {
        PyError::new("ValueError", message)
    }

    pub fn attribute_error(message: impl Into<String>) -> PyError {
        PyError::new("AttributeError", message)
    }

    pub fn key_error(key: &str) -> PyError {
        PyError::new("KeyError", format!("'{key}'"))
    }

    /// `TypeError` from `subprocess.run` when an argument is not a string.
    pub fn not_a_path(value: &Value) -> PyError {
        PyError::type_error(format!(
            "expected str, bytes or os.PathLike object, not {}",
            type_name(value)
        ))
    }
}

/// `type(value).__name__` for a value that came from JSON.
pub fn type_name(value: &Value) -> &'static str {
    match value {
        Value::Null => "NoneType",
        Value::Bool(_) => "bool",
        Value::Number(n) if n.is_f64() => "float",
        Value::Number(_) => "int",
        Value::String(_) => "str",
        Value::Array(_) => "list",
        Value::Object(_) => "dict",
    }
}

/// `str(value)` for a value that came from JSON or the config file.
pub fn py_str(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        other => py_repr(other),
    }
}

/// `repr(value)`.
pub fn py_repr(value: &Value) -> String {
    match value {
        Value::Null => "None".into(),
        Value::Bool(true) => "True".into(),
        Value::Bool(false) => "False".into(),
        Value::Number(n) => match (n.as_i64(), n.as_u64()) {
            (Some(i), _) => i.to_string(),
            (None, Some(u)) => u.to_string(),
            _ => pyfmt::repr_f64(n.as_f64().unwrap_or(f64::NAN)),
        },
        Value::String(s) => repr_str(s),
        Value::Array(items) => format!(
            "[{}]",
            items.iter().map(py_repr).collect::<Vec<_>>().join(", ")
        ),
        Value::Object(map) => format!(
            "{{{}}}",
            map.iter()
                .map(|(k, v)| format!("{}: {}", repr_str(k), py_repr(v)))
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

/// `repr(str)`: single quotes unless the text holds one and no double quote.
pub fn repr_str(s: &str) -> String {
    let quote = if s.contains('\'') && !s.contains('"') {
        '"'
    } else {
        '\''
    };
    let mut out = String::with_capacity(s.len() + 2);
    out.push(quote);
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c == quote => {
                out.push('\\');
                out.push(c);
            }
            c if (c as u32) < 0x20 || c as u32 == 0x7f => {
                out.push_str(&format!("\\x{:02x}", c as u32));
            }
            c => out.push(c),
        }
    }
    out.push(quote);
    out
}

/// `repr(bytes)`: `b'\x01\xc0\xa8\x01*'`.
pub fn bytes_repr(bytes: &[u8]) -> String {
    let quote = if bytes.contains(&b'\'') && !bytes.contains(&b'"') {
        '"'
    } else {
        '\''
    };
    let mut out = String::from("b");
    out.push(quote);
    for &b in bytes {
        match b {
            b'\\' => out.push_str("\\\\"),
            b'\n' => out.push_str("\\n"),
            b'\r' => out.push_str("\\r"),
            b'\t' => out.push_str("\\t"),
            b if b == quote as u8 => {
                out.push('\\');
                out.push(quote);
            }
            0x20..=0x7e => out.push(b as char),
            b => out.push_str(&format!("\\x{b:02x}")),
        }
    }
    out.push(quote);
    out
}

/// Python's `key in container` for a decoded JSON body: a key lookup in a
/// dict, membership in a list, a substring test in a string, and a
/// `TypeError` for anything else.
pub fn contains(container: &Value, key: &str) -> PyResult<bool> {
    match container {
        Value::Object(map) => Ok(map.contains_key(key)),
        Value::Array(items) => Ok(items.iter().any(|i| i.as_str() == Some(key))),
        Value::String(s) => Ok(s.contains(key)),
        other => Err(PyError::type_error(format!(
            "argument of type '{}' is not iterable",
            type_name(other)
        ))),
    }
}

/// Python's `container[key]` with a string key.
pub fn subscript<'a>(container: &'a Value, key: &str) -> PyResult<&'a Value> {
    match container {
        Value::Object(map) => map.get(key).ok_or_else(|| PyError::key_error(key)),
        Value::Array(_) => Err(PyError::type_error(
            "list indices must be integers or slices, not str",
        )),
        Value::String(_) => Err(PyError::type_error(
            "string indices must be integers, not 'str'",
        )),
        other => Err(PyError::type_error(format!(
            "'{}' object is not subscriptable",
            type_name(other)
        ))),
    }
}

/// The host's commands, behind a trait so tests can replace them the way the
/// Python tests monkeypatch `subprocess`.
pub trait CommandRunner: Send + Sync {
    /// `subprocess.run(argv, capture_output=True, text=True, timeout=...)`.
    fn run(&self, argv: &[String], timeout: Option<Duration>) -> Result<Completed, RunError>;
    /// `subprocess.run(script, shell=True, ...)`.
    fn run_shell(&self, script: &str) -> Result<Completed, RunError>;
}

/// The real host.
#[derive(Debug, Default)]
pub struct SystemRunner;

impl CommandRunner for SystemRunner {
    fn run(&self, argv: &[String], timeout: Option<Duration>) -> Result<Completed, RunError> {
        run_command(argv, timeout, &[])
    }

    fn run_shell(&self, script: &str) -> Result<Completed, RunError> {
        run_shell(script)
    }
}

/// Owned argv from string literals.
pub fn argv<S: AsRef<str>>(parts: &[S]) -> Vec<String> {
    parts.iter().map(|p| p.as_ref().to_string()).collect()
}

/// What `subprocess.run(..., capture_output=True, text=True)` returns.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Completed {
    /// Negative for a child killed by a signal, like Python's `returncode`.
    pub returncode: i32,
    pub stdout: String,
    pub stderr: String,
}

/// Why `subprocess.run` raised before the child produced a result.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RunError {
    /// `FileNotFoundError: [Errno 2] No such file or directory: 'nmcli'`.
    #[error("[Errno 2] No such file or directory: '{0}'")]
    NotFound(String),
    /// `TimeoutExpired`.
    #[error("Command '{command}' timed out after {seconds} seconds")]
    Timeout { command: String, seconds: String },
    /// Any other `OSError`.
    #[error("{0}")]
    Os(String),
}

impl RunError {
    pub fn kind(&self) -> &'static str {
        match self {
            RunError::NotFound(_) => "FileNotFoundError",
            RunError::Timeout { .. } => "TimeoutExpired",
            RunError::Os(_) => "OSError",
        }
    }

    pub fn to_py(&self) -> PyError {
        PyError::new(self.kind(), self.to_string())
    }
}

/// `subprocess.run(argv, capture_output=True, text=True, timeout=...)`.
pub fn run_command(
    argv: &[String],
    timeout: Option<Duration>,
    env: &[(&str, &str)],
) -> Result<Completed, RunError> {
    let Some((program, args)) = argv.split_first() else {
        return Err(RunError::Os("empty command".into()));
    };
    let mut command = Command::new(program);
    command
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (key, value) in env {
        command.env(key, value);
    }
    let mut child = command.spawn().map_err(|error| match error.kind() {
        std::io::ErrorKind::NotFound => RunError::NotFound(program.clone()),
        _ => RunError::Os(error.to_string()),
    })?;
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let out_reader = std::thread::spawn(move || read_all(stdout));
    let err_reader = std::thread::spawn(move || read_all(stderr));

    let started = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {}
            Err(error) => return Err(RunError::Os(error.to_string())),
        }
        if let Some(limit) = timeout
            && started.elapsed() >= limit
        {
            let _ = child.kill();
            let _ = child.wait();
            return Err(RunError::Timeout {
                command: format!(
                    "[{}]",
                    argv.iter()
                        .map(|a| repr_str(a))
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
                seconds: format_seconds(limit),
            });
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    let stdout = out_reader.join().unwrap_or_default();
    let stderr = err_reader.join().unwrap_or_default();
    Ok(Completed {
        returncode: returncode(status),
        stdout,
        stderr,
    })
}

/// `subprocess.run(script, shell=True, stdout=PIPE, stderr=PIPE, text=True)`.
pub fn run_shell(script: &str) -> Result<Completed, RunError> {
    run_command(
        &["/bin/sh".to_string(), "-c".to_string(), script.to_string()],
        None,
        &[],
    )
}

fn read_all(pipe: Option<impl Read>) -> String {
    let mut bytes = Vec::new();
    if let Some(mut pipe) = pipe {
        let _ = pipe.read_to_end(&mut bytes);
    }
    String::from_utf8_lossy(&bytes).into_owned()
}

fn returncode(status: std::process::ExitStatus) -> i32 {
    use std::os::unix::process::ExitStatusExt;
    match status.code() {
        Some(code) => code,
        None => -status.signal().unwrap_or(0),
    }
}

fn format_seconds(d: Duration) -> String {
    let secs = d.as_secs_f64();
    if secs.fract() == 0.0 {
        format!("{}", secs as u64)
    } else {
        pyfmt::repr_f64(secs)
    }
}

/// `CalledProcessError.__str__`.
pub fn called_process_error(command: &str, returncode: i32) -> String {
    if returncode < 0 {
        format!(
            "Command '{command}' died with {}.",
            signal_name(-returncode)
        )
    } else {
        format!("Command '{command}' returned non-zero exit status {returncode}.")
    }
}

fn signal_name(signal: i32) -> String {
    let name = match signal {
        1 => "SIGHUP",
        2 => "SIGINT",
        6 => "SIGABRT",
        9 => "SIGKILL",
        11 => "SIGSEGV",
        13 => "SIGPIPE",
        15 => "SIGTERM",
        _ => return format!("unknown signal {signal}"),
    };
    format!("<Signals.{name}: {signal}>")
}

/// `shutil.which(name)`.
pub fn which(name: &str) -> Option<std::path::PathBuf> {
    use std::os::unix::fs::PermissionsExt;
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(name))
        .find(|candidate| {
            std::fs::metadata(candidate)
                .map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
                .unwrap_or(false)
        })
}

/// Python's `round()`: halves go to the even neighbour.
pub fn round_half_even(x: f64) -> i64 {
    let r = x.round();
    if (x - x.trunc()).abs() == 0.5 {
        (2.0 * (x / 2.0).round()) as i64
    } else {
        r as i64
    }
}

/// A `subprocess` stand-in: records `(command line, shell)` like the Python
/// tests' `FakeSubprocess` and answers from a closure.
#[cfg(test)]
pub mod fake {
    use super::*;

    type Respond = Box<dyn Fn(&str, bool) -> Result<Completed, RunError> + Send + Sync>;

    pub struct RecordingRunner {
        pub calls: parking_lot::Mutex<Vec<(String, bool)>>,
        respond: Respond,
    }

    impl RecordingRunner {
        pub fn new(
            respond: impl Fn(&str, bool) -> Result<Completed, RunError> + Send + Sync + 'static,
        ) -> std::sync::Arc<RecordingRunner> {
            std::sync::Arc::new(RecordingRunner {
                calls: parking_lot::Mutex::new(Vec::new()),
                respond: Box::new(respond),
            })
        }

        /// Every command succeeds silently.
        pub fn ok() -> std::sync::Arc<RecordingRunner> {
            RecordingRunner::new(|_, _| Ok(Completed::default()))
        }

        pub fn calls(&self) -> Vec<(String, bool)> {
            self.calls.lock().clone()
        }
    }

    impl CommandRunner for RecordingRunner {
        fn run(&self, argv: &[String], _timeout: Option<Duration>) -> Result<Completed, RunError> {
            let line = argv.join(" ");
            self.calls.lock().push((line.clone(), false));
            (self.respond)(&line, false)
        }

        fn run_shell(&self, script: &str) -> Result<Completed, RunError> {
            self.calls.lock().push((script.to_string(), true));
            (self.respond)(script, true)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn str_and_repr_like_python() {
        assert_eq!(py_str(&json!(null)), "None");
        assert_eq!(py_str(&json!(true)), "True");
        assert_eq!(py_str(&json!(1234)), "1234");
        assert_eq!(py_str(&json!(1.5)), "1.5");
        assert_eq!(py_str(&json!("x")), "x");
        assert_eq!(py_repr(&json!({"a": [1, "b"]})), "{'a': [1, 'b']}");
        assert_eq!(repr_str("it's"), "\"it's\"");
    }

    #[test]
    fn membership_and_subscript_like_python() {
        assert_eq!(contains(&json!({"mode": 1}), "mode"), Ok(true));
        assert_eq!(contains(&json!(["mode"]), "mode"), Ok(true));
        assert_eq!(contains(&json!("a mode"), "mode"), Ok(true));
        assert_eq!(
            contains(&json!(5), "mode").unwrap_err().message,
            "argument of type 'int' is not iterable"
        );
        assert_eq!(
            subscript(&json!(null), "ssid").unwrap_err().message,
            "'NoneType' object is not subscriptable"
        );
        assert_eq!(subscript(&json!({}), "ssid").unwrap_err().message, "'ssid'");
    }

    #[test]
    fn rounding_like_python() {
        assert_eq!(round_half_even(0.5), 0);
        assert_eq!(round_half_even(1.5), 2);
        assert_eq!(round_half_even(2.5), 2);
        assert_eq!(round_half_even(-0.5), 0);
        assert_eq!(round_half_even(2.6), 3);
    }

    #[test]
    fn subprocess_errors_read_like_python() {
        let missing = run_command(&["definitely-not-here-xyz".to_string()], None, &[]);
        assert_eq!(
            missing.unwrap_err().to_string(),
            "[Errno 2] No such file or directory: 'definitely-not-here-xyz'"
        );
        let ok = run_shell("echo out; echo err >&2; exit 3").expect("sh runs");
        assert_eq!(ok.returncode, 3);
        assert_eq!(ok.stdout, "out\n");
        assert_eq!(ok.stderr, "err\n");
        assert_eq!(
            called_process_error("timedatectl set-timezone X", 1),
            "Command 'timedatectl set-timezone X' returned non-zero exit status 1."
        );
    }
}
