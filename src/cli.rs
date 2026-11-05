use crate::diff::DiffTool;
use clap::{Parser, ValueEnum};
use std::{path::PathBuf, time::Duration};

#[derive(Debug, Parser)]
#[command(
    name = "runSuite",
    version,
    allow_missing_positional = true,
    about = "Run CS246 suites with cargo-test-style results"
)]
pub struct Cli {
    /// Whitespace-separated test stems, resolved from the working directory
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

    /// Stop after the first failed test
    #[arg(long)]
    #[arg(short = 'f')]
    pub fail_fast: bool,

    /// Show stdout and stderr even for passing tests
    #[arg(long)]
    pub nocapture: bool,

    /// Print progress dots instead of individual result lines
    #[arg(short, long)]
    pub quiet: bool,

    /// Maximum seconds per test, including Valgrind (default: no limit)
    #[arg(long, value_name = "SECONDS", value_parser = parse_timeout)]
    pub timeout: Option<Duration>,

    /// Color mode; auto respects terminals and NO_COLOR
    #[arg(long, value_enum, default_value = "auto")]
    pub color: ColorMode,

    /// Failure diff: auto tries delta, git, diff; builtin or a quoted command overrides it
    #[arg(long, value_name = "COMMAND", default_value = "auto")]
    pub diff_tool: DiffTool,
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
