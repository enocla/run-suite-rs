use crate::diff::DiffTool;
use clap::{Parser, ValueEnum};
use std::{num::NonZeroUsize, path::PathBuf, time::Duration};

#[derive(Debug, Parser)]
#[command(
    name = "runSuite",
    version,
    allow_missing_positional = true,
    about = "Run CS246 suites with cargo-test-style results"
)]
pub struct Cli {
    /// Whitespace-separated test stems, resolved relative to the suite file
    #[arg(value_name = "SUITE", required_unless_present = "tests")]
    pub suite: Option<PathBuf>,

    /// Executable path, local executable name, or command on PATH
    #[arg(value_name = "PROGRAM")]
    pub program: PathBuf,

    /// Run this test stem without a suite file; repeat to run a set
    #[arg(
        short = 't',
        long = "test",
        value_name = "STEM",
        conflicts_with = "suite"
    )]
    pub tests: Vec<String>,

    /// Also compare .err and .ret (both required)
    #[arg(short = 'e', long = "check-stderr")]
    pub check_stderr: bool,

    /// Run with Valgrind Memcheck and fail on reported memory errors
    #[arg(short = 'v', long)]
    pub valgrind: bool,

    /// Run only test stems matching any pattern; repeat to select a set
    #[arg(long, value_name = "PATTERN")]
    pub filter: Vec<String>,

    /// Match each filter against the entire test stem
    #[arg(long, requires = "filter")]
    pub exact: bool,

    /// List selected tests without running the program
    #[arg(long)]
    pub list: bool,

    /// Maximum number of tests to run in parallel; use 1 for sequential execution
    #[arg(short = 'j', long, value_name = "N", default_value = "50")]
    pub jobs: NonZeroUsize,

    /// Stop reporting at the first failed test in suite order
    #[arg(long)]
    #[arg(short = 'f')]
    pub fail_fast: bool,

    /// Show stdout and stderr even for passing tests
    #[arg(long)]
    pub nocapture: bool,

    /// Print progress dots instead of individual result lines
    #[arg(short, long)]
    pub quiet: bool,

    /// Maximum seconds per test, including Valgrind
    #[arg(long, value_name = "SECONDS", default_value = "0.5", value_parser = parse_timeout)]
    pub timeout: Duration,

    /// Maximum captured bytes per stream (stdout, stderr, and Valgrind log)
    #[arg(long, value_name = "BYTES", default_value = "1048576")]
    pub max_output_bytes: NonZeroUsize,

    /// Color mode; auto respects terminals and NO_COLOR
    #[arg(long, value_enum, default_value = "auto")]
    pub color: ColorMode,

    /// Failure diff: auto tries delta, git, diff; builtin or a quoted command overrides it
    #[arg(long, value_name = "COMMAND", default_value = "auto")]
    pub diff_tool: DiffTool,
}

impl Cli {
    pub fn fixture_path(&self, stem: &str, extension: &str) -> PathBuf {
        let directory = self
            .suite
            .as_deref()
            .and_then(|suite| suite.parent())
            .unwrap_or_else(|| std::path::Path::new(""));
        // Append instead of replacing an existing extension in the stem.
        directory.join(format!("{stem}.{extension}"))
    }
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum ColorMode {
    Auto,
    Always,
    Never,
}

fn parse_timeout(value: &str) -> Result<Duration, String> {
    let seconds: f64 = value
        .parse()
        .map_err(|_| "expected a positive number of seconds")?;
    let duration = Duration::try_from_secs_f64(seconds)
        .map_err(|_| "timeout must be finite, positive, and representable")?;
    if duration.is_zero() {
        return Err("timeout must be greater than zero".into());
    }
    Ok(duration)
}
