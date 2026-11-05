mod cli;
mod diff;
mod report;
mod runner;
mod shell;

use clap::Parser;
use cli::Cli;
use report::Reporter;
use std::{process::ExitCode, time::Instant};

fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(&cli) {
        Ok(passed) => ExitCode::from(if passed { 0 } else { 1 }),
        Err(err) => {
            eprintln!("error: {}", report::safe_text(&err));
            ExitCode::from(2)
        }
    }
}

fn run(cli: &Cli) -> Result<bool, String> {
    let stems = match &cli.suite {
        Some(suite) => runner::load_suite(suite)?,
        None => cli.tests.clone(),
    };
    let total = stems.len();
    let selected: Vec<_> = stems
        .into_iter()
        .filter(|stem| {
            cli.filter.is_empty()
                || cli.filter.iter().any(|pattern| {
                    if cli.exact {
                        stem == pattern
                    } else {
                        stem.contains(pattern)
                    }
                })
        })
        .collect();
    let filtered = total - selected.len();
    if cli.list {
        for stem in &selected {
            println!("{}: test", report::safe_text(stem));
        }
        println!("\n{} tests; {filtered} filtered out", selected.len());
        return Ok(true);
    }
    let program = runner::resolve_program(&cli.program)?;
    let reporter = Reporter::new(cli.color, cli.quiet, cli.nocapture, cli.diff_tool.clone());
    reporter.start(selected.len());
    let start = Instant::now();
    let mut results = Vec::new();
    for stem in &selected {
        let result = runner::run_test(stem, &program, cli);
        let failed = !result.passed();
        reporter.result(&result);
        results.push(result);
        if failed && cli.fail_fast {
            break;
        }
    }
    reporter.failures(&results, &program, cli);
    reporter.summary(
        &results,
        filtered,
        selected.len() - results.len(),
        start.elapsed(),
    );
    Ok(results.iter().all(runner::TestResult::passed))
}
