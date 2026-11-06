use crate::{
    cli::ColorMode,
    diff::DiffTool,
    runner::{Problem, TestResult},
};
use std::{
    env,
    io::{self, IsTerminal, Write},
    time::Duration,
};

pub struct Reporter {
    color: bool,
    pub quiet: bool,
    pub nocapture: bool,
    diff_tool: DiffTool,
}

impl Reporter {
    pub fn new(mode: ColorMode, quiet: bool, nocapture: bool, diff_tool: DiffTool) -> Self {
        let color = match mode {
            ColorMode::Always => true,
            ColorMode::Never => false,
            ColorMode::Auto => {
                io::stdout().is_terminal()
                    && env::var_os("NO_COLOR").is_none()
                    && env::var("TERM").map_or(true, |term| term != "dumb")
            }
        };
        Self {
            color,
            quiet,
            nocapture,
            diff_tool,
        }
    }

    fn style(&self, text: &str, code: &str) -> String {
        if self.color {
            format!("\x1b[{code}m{text}\x1b[0m")
        } else {
            text.to_owned()
        }
    }

    pub fn start(&self, count: usize) {
        println!(
            "\n{}",
            self.style(
                &format!("running {count} test{}", if count == 1 { "" } else { "s" }),
                "1;34"
            )
        );
    }

    pub fn result(&self, result: &TestResult) {
        let (word, mark, code) = if result.timed_out() {
            ("TIMEOUT", "T", "1;33")
        } else if result.passed() {
            ("ok", ".", "32")
        } else {
            ("FAILED", "F", "31;1")
        };
        if self.quiet {
            print!("{}", self.style(mark, code));
        } else {
            println!(
                "test {} ... {} {}",
                self.style(&safe_text(&result.name), "1;36"),
                self.style(word, code),
                self.style(&format!("({})", format_duration(result.elapsed)), "2")
            );
        }
        let _ = io::stdout().flush();
        if self.nocapture && result.passed() {
            if self.quiet {
                println!();
            }
            self.captured(result);
        }
    }

    pub fn failures(&self, results: &[TestResult]) {
        if self.quiet {
            println!();
        }
        for (timed_out, title, code) in [(false, "failures:", "1;31"), (true, "timeouts:", "1;33")]
        {
            let group: Vec<_> = results
                .iter()
                .filter(|result| !result.passed() && result.timed_out() == timed_out)
                .collect();
            if group.is_empty() {
                continue;
            }
            println!("\n{}", self.style(title, code));
            for result in &group {
                self.diagnostics(result);
            }
        }
        for (timed_out, title, code) in [(false, "failures:", "1;31"), (true, "timeouts:", "1;33")]
        {
            let group: Vec<_> = results
                .iter()
                .filter(|result| !result.passed() && result.timed_out() == timed_out)
                .collect();
            if group.is_empty() {
                continue;
            }
            println!("\n{}", self.style(title, code));
            for result in group {
                println!(
                    "    {}",
                    self.style(
                        &safe_text(&result.name),
                        if timed_out { code } else { "1;35" }
                    )
                );
            }
        }
    }

    fn diagnostics(&self, result: &TestResult) {
        println!(
            "\n{}",
            self.style(
                &format!("---- {} ----", safe_text(&result.name)),
                if result.timed_out() { "1;33" } else { "1;35" }
            )
        );
        if !result.args.is_empty() {
            println!("{} {:?}", self.style("arguments:", "1;36"), result.args);
        }
        for problem in &result.problems {
            match problem {
                Problem::Message(message) => {
                    println!("{}", self.style(&safe_text(message), "31"))
                }
                Problem::Timeout(duration) => println!(
                    "{}",
                    self.style(
                        &format!("timed out after {}", format_duration(*duration)),
                        "1;33"
                    )
                ),
                Problem::Mismatch {
                    stream,
                    expected,
                    actual,
                } => {
                    let offset = expected
                        .iter()
                        .zip(actual)
                        .position(|(a, b)| a != b)
                        .unwrap_or(expected.len().min(actual.len()));
                    println!("{}", self.style(&format!(
                            "{stream} differs at byte {offset} (expected {} bytes, actual {} bytes)",
                            expected.len(),
                            actual.len()
                        ), "1;33"));
                    let shown = match self.diff_tool.show(stream, expected, actual, self.color) {
                        Ok(shown) => shown,
                        Err(err) => {
                            println!(
                                "{}",
                                self.style(
                                    &format!(
                                        "diff unavailable: {}; using built-in report",
                                        safe_text(&err)
                                    ),
                                    "33"
                                )
                            );
                            false
                        }
                    };
                    if !shown {
                        self.block("expected", expected, "-", "31");
                        self.block("actual", actual, "+", "32");
                    }
                }
                Problem::Valgrind(log) => self.block("Valgrind log", log, " ", "33"),
            }
        }
        // Always show complete stderr, even if an external diff hides context.
        if !result.stdout.is_empty() && !has_mismatch(result, "stdout") {
            self.block("stdout", &result.stdout, " ", "36");
        }
        if !result.stderr.is_empty() {
            self.block("stderr", &result.stderr, " ", "33");
        }
    }

    fn captured(&self, result: &TestResult) {
        println!("\n---- {} output ----", safe_text(&result.name));
        self.block("stdout", &result.stdout, " ", "36");
        self.block("stderr", &result.stderr, " ", "33");
    }

    fn block(&self, title: &str, bytes: &[u8], prefix: &str, color: &str) {
        println!("  {}", self.style(&format!("{title}:"), "1;36"));
        if bytes.is_empty() {
            println!("    (empty)");
            return;
        }
        let numbered = matches!(prefix, "-" | "+");
        let width = if numbered {
            bytes
                .split_inclusive(|byte| *byte == b'\n')
                .count()
                .to_string()
                .len()
        } else {
            0
        };
        for (index, line) in bytes.split_inclusive(|byte| *byte == b'\n').enumerate() {
            let line = line.strip_suffix(b"\n").unwrap_or(line);
            let text = self.style(&format!("{prefix} {}", visible_bytes(line)), color);
            if numbered {
                println!(
                    "    {} {text}",
                    self.style(&format!("{:>width$} |", index + 1), "2")
                );
            } else {
                println!("    {text}");
            }
        }
        if !bytes.ends_with(b"\n") {
            println!("    {}", self.style("\\ No newline at end of output", "33"));
        }
    }

    pub fn summary(
        &self,
        results: &[TestResult],
        filtered: usize,
        skipped: usize,
        elapsed: Duration,
    ) {
        let passed = results.iter().filter(|result| result.passed()).count();
        let timed_out = results.iter().filter(|result| result.timed_out()).count();
        let failed = results.len() - passed - timed_out;
        let (label, code) = if failed > 0 {
            ("FAILED", "31;1")
        } else if timed_out > 0 {
            ("TIMEOUT", "1;33")
        } else {
            ("ok", "32;1")
        };
        println!(
            "\n{} {}. {}; {}; {}; {}; {}; {}\n",
            self.style("test result:", "1;34"),
            self.style(label, code),
            self.style(&format!("{passed} passed"), "32"),
            self.style(
                &format!("{failed} failed"),
                if failed == 0 { "2" } else { "1;31" }
            ),
            self.style(
                &format!("{timed_out} timed out"),
                if timed_out == 0 { "2" } else { "1;33" }
            ),
            self.style(&format!("{filtered} filtered out"), "33"),
            self.style(&format!("{skipped} skipped"), "35"),
            self.style(&format!("finished in {}", format_duration(elapsed)), "2")
        );
    }
}

pub(crate) fn format_duration(duration: Duration) -> String {
    if duration < Duration::from_secs(1) {
        format!("{}ms", duration.as_millis())
    } else {
        format!("{:.3}s", duration.as_secs_f64())
    }
}

fn has_mismatch(result: &TestResult, stream: &str) -> bool {
    result.problems.iter().any(|problem| {
        matches!(problem,
        Problem::Mismatch { stream: actual, .. } if *actual == stream)
    })
}

pub fn safe_text(text: &str) -> String {
    text.chars()
        .flat_map(|character| {
            if character.is_control() {
                character.escape_default().collect::<Vec<_>>()
            } else {
                vec![character]
            }
        })
        .collect()
}

// Escape terminal controls and invalid UTF-8, while keeping valid Unicode readable.
pub(crate) fn visible_bytes(bytes: &[u8]) -> String {
    let mut remaining = bytes;
    let mut text = String::new();
    while !remaining.is_empty() {
        match std::str::from_utf8(remaining) {
            Ok(valid) => {
                text.push_str(&safe_text(valid));
                break;
            }
            Err(err) => {
                let valid_length = err.valid_up_to();
                text.push_str(&safe_text(
                    std::str::from_utf8(&remaining[..valid_length]).unwrap(),
                ));
                let invalid_length = err.error_len().unwrap_or(remaining.len() - valid_length);
                for byte in &remaining[valid_length..valid_length + invalid_length] {
                    text.push_str(&format!("\\x{byte:02x}"));
                }
                remaining = &remaining[valid_length + invalid_length..];
            }
        }
    }
    text
}
