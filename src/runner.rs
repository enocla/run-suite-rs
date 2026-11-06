use crate::{capture::Capture, cli::Cli};
use std::{
    env,
    ffi::OsString,
    fs::{self, File},
    io,
    path::{Path, PathBuf},
    process::{Command, ExitStatus, Stdio},
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};

pub struct TestResult {
    pub name: String,
    pub elapsed: Duration,
    pub problems: Vec<Problem>,
    pub args: Vec<String>,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

pub enum Problem {
    Message(String),
    Timeout(Duration),
    Mismatch {
        stream: &'static str,
        expected: Vec<u8>,
        actual: Vec<u8>,
    },
    Valgrind(Vec<u8>),
}

impl TestResult {
    pub fn passed(&self) -> bool {
        self.problems.is_empty()
    }

    pub fn timed_out(&self) -> bool {
        self.problems
            .iter()
            .any(|problem| matches!(problem, Problem::Timeout(_)))
    }
}

pub fn load_suite(path: &Path) -> Result<Vec<String>, String> {
    let contents = fs::read_to_string(path)
        .map_err(|err| format!("cannot read suite {}: {err}", path.display()))?;
    Ok(contents
        .split_ascii_whitespace()
        .map(str::to_owned)
        .collect())
}

pub fn resolve_program(program: &Path) -> Result<PathBuf, String> {
    // A bare local name works even when '.' is absent from PATH.
    if executable(program) {
        return fs::canonicalize(program)
            .map_err(|err| format!("cannot resolve {}: {err}", program.display()));
    }
    if program.components().count() == 1 {
        for directory in env::split_paths(&env::var_os("PATH").unwrap_or_default()) {
            let candidate = directory.join(program);
            if executable(&candidate) {
                return fs::canonicalize(&candidate)
                    .map_err(|err| format!("cannot resolve {}: {err}", candidate.display()));
            }
        }
    }
    Err(format!(
        "{} does not exist or is not executable",
        program.display()
    ))
}

fn executable(path: &Path) -> bool {
    let Ok(metadata) = path.metadata() else {
        return false;
    };
    if !metadata.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        metadata.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}

fn required(path: &Path) -> Result<Vec<u8>, String> {
    fs::read(path).map_err(|err| format!("cannot read {}: {err}", path.display()))
}

fn optional(path: &Path) -> Result<Option<Vec<u8>>, String> {
    match fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(format!("cannot read {}: {err}", path.display())),
    }
}

struct Fixture {
    args: Vec<String>,
    input: Option<File>,
    stdout: Vec<u8>,
    stderr: Option<Vec<u8>>,
    exit_code: Option<i32>,
}

impl Fixture {
    fn load(stem: &str, cli: &Cli) -> Result<Self, String> {
        let stdout = required(&cli.fixture_path(stem, "out"))?;
        let args_path = cli.fixture_path(stem, "args");
        let args = match optional(&args_path)? {
            None => Vec::new(),
            Some(bytes) => {
                let text = String::from_utf8(bytes)
                    .map_err(|_| format!("{} is not UTF-8", args_path.display()))?;
                if text.contains('\0') {
                    return Err(format!("{} contains a NUL byte", args_path.display()));
                }
                shlex::split(&text).ok_or_else(|| {
                    format!(
                        "{} contains an unfinished quote or escape",
                        args_path.display()
                    )
                })?
            }
        };
        let input_path = cli.fixture_path(stem, "in");
        let input = match File::open(&input_path) {
            Ok(file) if file.metadata().map_err(|err| err.to_string())?.is_file() => Some(file),
            Ok(_) => return Err(format!("{} is not a regular file", input_path.display())),
            Err(err) if err.kind() == io::ErrorKind::NotFound => None,
            Err(err) => return Err(format!("cannot read {}: {err}", input_path.display())),
        };
        let (stderr, exit_code) = if cli.check_stderr {
            let stderr = required(&cli.fixture_path(stem, "err"))?;
            let ret_path = cli.fixture_path(stem, "ret");
            let ret = required(&ret_path)?;
            let code: i32 = std::str::from_utf8(&ret)
                .ok()
                .and_then(|text| text.trim().parse().ok())
                .filter(|code| (0..=255).contains(code))
                .ok_or_else(|| {
                    format!(
                        "{} must contain an integer from 0 to 255",
                        ret_path.display()
                    )
                })?;
            (Some(stderr), Some(code))
        } else {
            (None, None)
        };
        Ok(Self {
            args,
            input,
            stdout,
            stderr,
            exit_code,
        })
    }
}

pub fn run_tests(
    stems: &[String],
    program: &Path,
    cli: &Cli,
    mut report: impl FnMut(&TestResult),
) -> Vec<TestResult> {
    let next = AtomicUsize::new(0);
    let stopped = AtomicBool::new(false);
    let (sender, receiver) = mpsc::channel();
    thread::scope(|scope| {
        for _ in 0..cli.jobs.get().min(stems.len()) {
            let sender = sender.clone();
            let next = &next;
            let stopped = &stopped;
            scope.spawn(move || {
                while !stopped.load(Ordering::Relaxed) {
                    let index = next.fetch_add(1, Ordering::Relaxed);
                    let Some(stem) = stems.get(index) else {
                        break;
                    };
                    let result = run_test(stem, program, cli);
                    if cli.fail_fast && !result.passed() {
                        stopped.store(true, Ordering::Relaxed);
                    }
                    if sender.send((index, result)).is_err() {
                        break;
                    }
                }
            });
        }
        drop(sender);

        let mut pending: Vec<Option<TestResult>> =
            std::iter::repeat_with(|| None).take(stems.len()).collect();
        let mut results = Vec::new();
        for (index, result) in receiver {
            pending[index] = Some(result);
            // Workers claim indices in order, so every test preceding a failure
            // is already scheduled. Wait for those tests before reporting it.
            while let Some(result) = pending[results.len()].take() {
                let failed = !result.passed();
                report(&result);
                results.push(result);
                if cli.fail_fast && failed {
                    return results;
                }
                if results.len() == stems.len() {
                    break;
                }
            }
        }
        results
    })
}

fn run_test(stem: &str, program: &Path, cli: &Cli) -> TestResult {
    let start = Instant::now();
    let mut result = TestResult {
        name: stem.to_owned(),
        elapsed: Duration::ZERO,
        problems: Vec::new(),
        args: Vec::new(),
        stdout: Vec::new(),
        stderr: Vec::new(),
    };
    if let Err(err) = execute(stem, program, cli, &mut result) {
        result.problems.push(Problem::Message(err));
    }
    result.elapsed = start.elapsed();
    result
}

fn execute(stem: &str, program: &Path, cli: &Cli, result: &mut TestResult) -> Result<(), String> {
    let fixture = Fixture::load(stem, cli)?;
    result.args.clone_from(&fixture.args);
    let temporary =
        tempfile::tempdir().map_err(|err| format!("cannot create temporary directory: {err}"))?;
    let memory_path = temporary.path().join("valgrind.log");
    let limit = cli.max_output_bytes.get();
    #[cfg(unix)]
    let mut memory_capture = if cli.valgrind {
        Some(
            Capture::start(
                crate::capture::log_pipe(&memory_path).map_err(|err| err.to_string())?,
                limit,
            )
            .map_err(|err| err.to_string())?,
        )
    } else {
        None
    };
    #[cfg(unix)]
    let watchdog = crate::watchdog::Watchdog::start(temporary.path())
        .map_err(|err| format!("cannot start process watchdog: {err}"))?;
    let mut command = if cli.valgrind {
        let mut command = Command::new("valgrind");
        let mut log_file = OsString::from("--log-file=");
        log_file.push(&memory_path);
        command
            .args([
                "--tool=memcheck",
                "--leak-check=full",
                "--show-leak-kinds=all",
            ])
            .arg(log_file)
            .arg(program);
        // Read ERROR SUMMARY instead of overriding the target's exit code.
        command
    } else {
        Command::new(program)
    };
    command
        .args(&fixture.args)
        .stdin(fixture.input.map_or_else(Stdio::null, Stdio::from))
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(watchdog.process_group());
    }
    let child = command.spawn().map_err(|err| {
        if cli.valgrind && err.kind() == io::ErrorKind::NotFound {
            "cannot start Valgrind: install valgrind or omit -v".into()
        } else {
            format!("cannot start {}: {err}", program.display())
        }
    })?;
    let mut child = ReapChild(child);
    let mut stdout = Capture::start(child.0.stdout.take().unwrap(), limit)
        .map_err(|err| format!("cannot capture stdout: {err}"))?;
    let mut stderr = Capture::start(child.0.stderr.take().unwrap(), limit)
        .map_err(|err| format!("cannot capture stderr: {err}"))?;
    let start = Instant::now();
    let (status, timed_out) = loop {
        let mut exceeded = stdout.exceeded() || stderr.exceeded();
        #[cfg(unix)]
        {
            exceeded |= memory_capture.as_ref().is_some_and(Capture::exceeded);
        }
        if exceeded {
            stop(&mut child.0);
            break (child.0.wait().map_err(|err| err.to_string())?, false);
        }
        match child.0.try_wait() {
            Ok(Some(status)) => break (status, false),
            Ok(None) => {}
            Err(err) => {
                stop(&mut child.0);
                return Err(format!("cannot wait for program: {err}"));
            }
        }
        if start.elapsed() >= cli.timeout {
            stop(&mut child.0);
            let status = child
                .0
                .wait()
                .map_err(|err| format!("cannot reap timed-out program: {err}"))?;
            break (status, true);
        }
        thread::sleep(Duration::from_millis(5));
    };
    // Always kill remaining descendants, including after a successful parent
    // exit, before draining the final buffered output.
    #[cfg(unix)]
    drop(watchdog);
    result.stdout = stdout
        .finish()
        .map_err(|err| format!("cannot read stdout: {err}"))?;
    result.stderr = stderr
        .finish()
        .map_err(|err| format!("cannot read stderr: {err}"))?;
    #[cfg(unix)]
    let log = memory_capture
        .as_mut()
        .map(Capture::finish)
        .transpose()
        .map_err(|err| format!("cannot read Valgrind log: {err}"))?
        .unwrap_or_default();
    #[cfg(not(unix))]
    let log = optional(&memory_path)?.unwrap_or_default();
    let mut limited = false;
    let mut streams = vec![("stdout", stdout.exceeded()), ("stderr", stderr.exceeded())];
    #[cfg(unix)]
    streams.push((
        "Valgrind log",
        memory_capture.as_ref().is_some_and(Capture::exceeded),
    ));
    for (stream, exceeded) in streams {
        if exceeded {
            limited = true;
            result.problems.push(Problem::Message(format!(
                "{stream} exceeded output limit of {limit} bytes (capture truncated)"
            )));
        }
    }
    if timed_out {
        result.problems.push(Problem::Timeout(cli.timeout));
    } else if !limited {
        compare(
            "stdout",
            fixture.stdout,
            &result.stdout,
            &mut result.problems,
        );
        if let Some(expected) = fixture.stderr {
            compare("stderr", expected, &result.stderr, &mut result.problems);
        }
        if let Some(expected) = fixture.exit_code {
            if shell_code(status) != expected {
                result.problems.push(Problem::Message(format!(
                    "exit status: expected {expected}, actual {}",
                    status_text(status)
                )));
            }
        } else if !status.success() {
            result.problems.push(Problem::Message(format!(
                "program exited with {}",
                status_text(status)
            )));
        }
    }
    if cli.valgrind && !timed_out && !limited {
        match valgrind_errors(&log) {
            Some(0) => {}
            Some(count) => {
                result.problems.push(Problem::Message(format!(
                    "Valgrind reported {count} memory error(s)"
                )));
                result.problems.push(Problem::Valgrind(log));
            }
            None => {
                result.problems.push(Problem::Message(
                    "Valgrind did not produce an error summary".into(),
                ));
                result.problems.push(Problem::Valgrind(log));
            }
        }
    } else if cli.valgrind && !log.is_empty() {
        result.problems.push(Problem::Valgrind(log));
    }
    Ok(())
}

fn compare(stream: &'static str, expected: Vec<u8>, actual: &[u8], problems: &mut Vec<Problem>) {
    if expected != actual {
        problems.push(Problem::Mismatch {
            stream,
            expected,
            actual: actual.to_vec(),
        });
    }
}

fn valgrind_errors(log: &[u8]) -> Option<u64> {
    String::from_utf8_lossy(log).lines().rev().find_map(|line| {
        let (_, suffix) = line.split_once("ERROR SUMMARY:")?;
        suffix
            .split_whitespace()
            .next()?
            .replace(',', "")
            .parse()
            .ok()
    })
}

// Rust's Child does not reap or stop a process on drop. Cover every error path.
struct ReapChild(std::process::Child);

impl Drop for ReapChild {
    fn drop(&mut self) {
        stop(&mut self.0);
    }
}

fn stop(child: &mut std::process::Child) {
    let _ = child.kill();
    let _ = child.wait();
}

fn shell_code(status: ExitStatus) -> i32 {
    if let Some(code) = status.code() {
        return code;
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        128 + status.signal().unwrap_or(0)
    }
    #[cfg(not(unix))]
    {
        1
    }
}

fn status_text(status: ExitStatus) -> String {
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if let Some(signal) = status.signal() {
            return format!("signal {signal} (shell status {})", shell_code(status));
        }
    }
    shell_code(status).to_string()
}
