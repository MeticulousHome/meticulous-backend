//! Commands the machine image provides (`subprocess` in the Python), behind a
//! trait so handler tests can record them instead of touching the host, as
//! the Python suite replaces the `subprocess` module.

use std::io::Write as _;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::Arc;

use parking_lot::RwLock;

/// What a finished command left behind (`subprocess.CompletedProcess`).
#[derive(Debug, Clone, Default)]
pub struct CommandOutput {
    /// The exit code; `None` when a signal ended it.
    pub status: Option<i32>,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

impl CommandOutput {
    pub fn ok() -> CommandOutput {
        CommandOutput {
            status: Some(0),
            ..CommandOutput::default()
        }
    }

    pub fn success(&self) -> bool {
        self.status == Some(0)
    }

    pub fn stdout_text(&self) -> String {
        String::from_utf8_lossy(&self.stdout).into_owned()
    }

    /// The `returncode` Python reports: negative signal numbers become -1.
    pub fn returncode(&self) -> i32 {
        self.status.unwrap_or(-1)
    }
}

/// A program [`CommandRunner::start`] left running.
pub trait Process: Send {
    /// End it and reap it.
    fn stop(&mut self);
}

pub trait CommandRunner: Send + Sync {
    /// Run `argv` to completion with `stdin` piped in and the output captured.
    /// `Err` is Python's `OSError` from `Popen`: the program could not start.
    fn run(&self, argv: &[&str], stdin: Option<&[u8]>) -> std::io::Result<CommandOutput>;

    /// `subprocess.run(argv, capture_output=True, timeout=...)`: a command
    /// still running after `timeout` is killed and answers
    /// `ErrorKind::TimedOut` (`TimeoutExpired`). A runner that records
    /// instead of running has nothing to time out and only records it.
    fn run_with_timeout(
        &self,
        argv: &[&str],
        timeout: std::time::Duration,
    ) -> std::io::Result<CommandOutput> {
        let _ = timeout;
        self.run(argv, None)
    }

    /// `subprocess.Popen(argv, stdout=DEVNULL, stderr=DEVNULL)`: start it
    /// and do not wait.
    fn spawn(&self, argv: &[&str]) -> std::io::Result<()>;

    /// Like [`CommandRunner::spawn`], keeping a handle to stop it later
    /// (playsound's non-blocking `playsound(path, block=False).stop()`).
    fn start(&self, argv: &[&str]) -> std::io::Result<Box<dyn Process>>;

    /// `shutil.which(name)`.
    fn which(&self, name: &str) -> Option<PathBuf>;
}

/// The real host: `std::process::Command` with the Python's binaries.
pub struct SystemRunner;

impl CommandRunner for SystemRunner {
    fn run(&self, argv: &[&str], stdin: Option<&[u8]>) -> std::io::Result<CommandOutput> {
        let (program, args) = argv
            .split_first()
            .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidInput, "empty argv"))?;
        let mut child = Command::new(program)
            .args(args)
            .stdin(if stdin.is_some() {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        if let (Some(input), Some(mut pipe)) = (stdin, child.stdin.take()) {
            // A program that exits without reading its input is not an error
            // for the caller; its exit status says what happened.
            let _ = pipe.write_all(input);
        }
        let output = child.wait_with_output()?;
        Ok(CommandOutput {
            status: output.status.code(),
            stdout: output.stdout,
            stderr: output.stderr,
        })
    }

    fn run_with_timeout(
        &self,
        argv: &[&str],
        timeout: std::time::Duration,
    ) -> std::io::Result<CommandOutput> {
        let argv: Vec<String> = argv.iter().map(|a| a.to_string()).collect();
        match crate::py::run_command(&argv, Some(timeout), &[]) {
            Ok(done) => Ok(CommandOutput {
                status: (done.returncode >= 0).then_some(done.returncode),
                stdout: done.stdout.into_bytes(),
                stderr: done.stderr.into_bytes(),
            }),
            Err(error) => {
                let kind = match error {
                    crate::py::RunError::NotFound(_) => std::io::ErrorKind::NotFound,
                    crate::py::RunError::Timeout { .. } => std::io::ErrorKind::TimedOut,
                    crate::py::RunError::Os(_) => std::io::ErrorKind::Other,
                };
                Err(std::io::Error::new(kind, error.to_string()))
            }
        }
    }

    fn spawn(&self, argv: &[&str]) -> std::io::Result<()> {
        let (program, args) = argv
            .split_first()
            .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidInput, "empty argv"))?;
        let mut child = Command::new(program)
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?;
        // Reap it when it ends so it does not linger as a zombie.
        std::thread::Builder::new()
            .name("reaper".into())
            .spawn(move || {
                let _ = child.wait();
            })
            .map(|_| ())
    }

    fn start(&self, argv: &[&str]) -> std::io::Result<Box<dyn Process>> {
        let (program, args) = argv
            .split_first()
            .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidInput, "empty argv"))?;
        let child = Command::new(program)
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?;
        Ok(Box::new(ChildProcess(child)))
    }

    fn which(&self, name: &str) -> Option<PathBuf> {
        let path = std::env::var_os("PATH")?;
        std::env::split_paths(&path)
            .map(|dir| dir.join(name))
            .find(|candidate| is_executable(candidate))
    }
}

/// A started child: it stays a zombie once it ends until stopped, as the
/// Python's `Popen` does until its next poll.
struct ChildProcess(std::process::Child);

impl Process for ChildProcess {
    fn stop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn is_executable(path: &std::path::Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

/// The runner every system service uses; tests swap it for a recorder.
pub struct Host {
    runner: RwLock<Arc<dyn CommandRunner>>,
}

impl Default for Host {
    fn default() -> Host {
        Host {
            runner: RwLock::new(Arc::new(SystemRunner)),
        }
    }
}

impl Host {
    fn runner(&self) -> Arc<dyn CommandRunner> {
        self.runner.read().clone()
    }

    pub fn run(&self, argv: &[&str]) -> std::io::Result<CommandOutput> {
        self.runner().run(argv, None)
    }

    pub fn run_with_input(&self, argv: &[&str], stdin: &[u8]) -> std::io::Result<CommandOutput> {
        self.runner().run(argv, Some(stdin))
    }

    pub fn run_with_timeout(
        &self,
        argv: &[&str],
        timeout: std::time::Duration,
    ) -> std::io::Result<CommandOutput> {
        self.runner().run_with_timeout(argv, timeout)
    }

    /// `subprocess.run(command, shell=True, capture_output=True)`.
    pub fn run_shell(&self, command: &str) -> std::io::Result<CommandOutput> {
        self.runner().run(&["/bin/sh", "-c", command], None)
    }

    pub fn spawn(&self, argv: &[&str]) -> std::io::Result<()> {
        self.runner().spawn(argv)
    }

    pub fn start(&self, argv: &[&str]) -> std::io::Result<Box<dyn Process>> {
        self.runner().start(argv)
    }

    pub fn which(&self, name: &str) -> Option<PathBuf> {
        self.runner().which(name)
    }

    #[cfg(test)]
    pub fn replace(&self, runner: Arc<dyn CommandRunner>) {
        *self.runner.write() = runner;
    }
}

/// `CalledProcessError`'s text, for logs.
pub fn describe_failure(argv: &[&str], output: &CommandOutput) -> String {
    format!(
        "Command '{:?}' returned non-zero exit status {}.",
        argv,
        output.returncode()
    )
}

#[cfg(test)]
pub mod fake {
    use std::collections::HashMap;
    use std::path::PathBuf;
    use std::sync::Arc;

    use parking_lot::Mutex;

    use super::{CommandOutput, CommandRunner, Process};

    /// A recorded call: the argv and what was piped in.
    pub type Call = (Vec<String>, Option<Vec<u8>>);
    type Answer = Result<CommandOutput, std::io::ErrorKind>;

    /// Records every command and answers from a table keyed by program name;
    /// unknown programs succeed with no output, like the boot harness stubs.
    #[derive(Default)]
    pub struct FakeRunner {
        pub calls: Mutex<Vec<Call>>,
        pub spawned: Mutex<Vec<Vec<String>>>,
        /// The argv of every [`CommandRunner::start`], in order.
        pub started: Mutex<Vec<Vec<String>>>,
        /// The argv of every started program that was stopped, in order.
        pub stopped: Arc<Mutex<Vec<Vec<String>>>>,
        pub answers: Mutex<HashMap<String, Answer>>,
        pub on_path: Mutex<Vec<String>>,
    }

    struct FakeProcess {
        argv: Vec<String>,
        stopped: Arc<Mutex<Vec<Vec<String>>>>,
    }

    impl Process for FakeProcess {
        fn stop(&mut self) {
            self.stopped.lock().push(self.argv.clone());
        }
    }

    impl FakeRunner {
        pub fn answer(&self, program: &str, output: CommandOutput) {
            self.answers.lock().insert(program.to_string(), Ok(output));
        }

        pub fn exit_with(&self, program: &str, status: i32) {
            self.answer(
                program,
                CommandOutput {
                    status: Some(status),
                    ..CommandOutput::default()
                },
            );
        }

        pub fn missing(&self, program: &str) {
            self.answers
                .lock()
                .insert(program.to_string(), Err(std::io::ErrorKind::NotFound));
        }

        pub fn argvs(&self) -> Vec<Vec<String>> {
            self.calls.lock().iter().map(|(a, _)| a.clone()).collect()
        }
    }

    impl CommandRunner for FakeRunner {
        fn run(&self, argv: &[&str], stdin: Option<&[u8]>) -> std::io::Result<CommandOutput> {
            self.calls.lock().push((
                argv.iter().map(|s| s.to_string()).collect(),
                stdin.map(<[u8]>::to_vec),
            ));
            let program = argv.first().copied().unwrap_or("");
            match self.answers.lock().get(program) {
                Some(Ok(output)) => Ok(output.clone()),
                Some(Err(kind)) => Err(std::io::Error::new(*kind, "fake")),
                None => Ok(CommandOutput::ok()),
            }
        }

        fn spawn(&self, argv: &[&str]) -> std::io::Result<()> {
            self.spawned
                .lock()
                .push(argv.iter().map(|s| s.to_string()).collect());
            Ok(())
        }

        /// Fails with the program's [`FakeRunner::missing`] answer, if any.
        fn start(&self, argv: &[&str]) -> std::io::Result<Box<dyn Process>> {
            let argv: Vec<String> = argv.iter().map(|s| s.to_string()).collect();
            let program = argv.first().cloned().unwrap_or_default();
            if let Some(Err(kind)) = self.answers.lock().get(&program) {
                return Err(std::io::Error::new(*kind, "fake"));
            }
            self.started.lock().push(argv.clone());
            Ok(Box::new(FakeProcess {
                argv,
                stopped: self.stopped.clone(),
            }))
        }

        fn which(&self, name: &str) -> Option<PathBuf> {
            self.on_path
                .lock()
                .iter()
                .any(|n| n == name)
                .then(|| PathBuf::from("/usr/bin").join(name))
        }
    }
}
