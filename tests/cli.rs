#![cfg(unix)]

use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::Path,
    process::{Command, Output},
    time::{Duration, Instant},
};
use tempfile::TempDir;

struct Suite {
    dir: TempDir,
}

impl Suite {
    fn new(program: &str) -> Self {
        let suite = Self {
            dir: tempfile::tempdir().unwrap(),
        };
        suite.script("program", program);
        suite.write("suite", "one\n");
        suite.write("one.out", "hello\n");
        suite
    }

    fn write(&self, name: &str, contents: impl AsRef<[u8]>) {
        fs::write(self.dir.path().join(name), contents).unwrap();
    }

    fn script(&self, name: &str, contents: &str) {
        self.write(name, format!("#!/bin/sh\n{contents}\n"));
        fs::set_permissions(
            self.dir.path().join(name),
            fs::Permissions::from_mode(0o755),
        )
        .unwrap();
    }

    fn command(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_runSuite"));
        command.current_dir(self.dir.path()).env("NO_COLOR", "1");
        command
    }

    fn run(&self, flags: &[&str]) -> Output {
        self.command()
            .args(flags)
            .args(["suite", "./program"])
            .output()
            .unwrap()
    }
}

fn stdout(output: &Output) -> String {
    String::from_utf8(output.stdout.clone()).unwrap()
}

fn status(output: &Output, expected: i32) {
    assert_eq!(
        output.status.code(),
        Some(expected),
        "stdout:\n{}\nstderr:\n{}",
        stdout(output),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn passes_with_named_result_timing_and_summary() {
    let suite = Suite::new("printf 'hello\\n'");
    let output = suite.run(&[]);
    status(&output, 0);
    let text = stdout(&output);
    assert!(text.contains("running 1 test\ntest one ... ok ("));
    assert!(text.contains("1 passed; 0 failed; 0 timed out; 0 filtered out; 0 skipped"));
    assert!(text.contains("finished in"));
    assert!(!text.contains('\x1b'));
}

#[test]
fn failure_reports_mismatch_and_continues_running() {
    let suite = Suite::new("printf 'hello\\n'");
    suite.write("suite", "bad one\n");
    suite.write("bad.out", "goodbye\n");
    let output = suite.run(&["--diff-tool", "builtin"]);
    status(&output, 1);
    let text = stdout(&output);
    assert!(text.contains("test bad ... FAILED"));
    assert!(text.contains("test one ... ok"));
    assert!(text.contains("stdout differs at byte 0"));
    assert!(text.contains("- goodbye"));
    assert!(text.contains("+ hello"));
    assert!(text.contains("1 passed; 1 failed"));
}

#[test]
fn arguments_support_quoting_empty_strings_and_literal_shell_characters() {
    let suite = Suite::new("printf '<%s>\\n' \"$@\"\ncat");
    suite.write(
        "one.args",
        "'two words' \"\" ';' '$(touch injected)' '*.txt'\n",
    );
    suite.write("one.in", "from stdin\n");
    suite.write(
        "one.out",
        "<two words>\n<>\n<;>\n<$(touch injected)>\n<*.txt>\nfrom stdin\n",
    );
    status(&suite.run(&[]), 0);
    assert!(!suite.dir.path().join("injected").exists());
}

#[test]
fn missing_input_is_eof() {
    let suite = Suite::new("cat\nprintf 'hello\\n'");
    status(&suite.run(&["--timeout", "1"]), 0);
}

#[test]
fn stderr_and_nonzero_exit_are_checked_with_e() {
    let suite = Suite::new("printf 'hello\\n'\nprintf 'warning\\n' >&2\nexit 7");
    suite.write("one.err", "warning\n");
    suite.write("one.ret", "7\n");
    status(&suite.run(&["-e"]), 0);
    suite.write("one.err", "different\n");
    suite.write("one.ret", "0\n");
    let output = suite.run(&["-e"]);
    status(&output, 1);
    assert!(stdout(&output).contains("stderr differs"));
    assert!(stdout(&output).contains("exit status: expected 0, actual 7"));
}

#[test]
fn nonzero_exit_without_e_fails_and_keeps_stderr_diagnostics() {
    let suite = Suite::new("printf 'hello\\n'\nprintf 'crash detail\\n' >&2\nexit 7");
    let output = suite.run(&[]);
    status(&output, 1);
    assert!(stdout(&output).contains("program exited with 7"));
    assert!(stdout(&output).contains("crash detail"));
}

#[test]
fn failed_program_shows_full_stderr_beyond_old_byte_and_line_limits() {
    let suite = Suite::new("cat diagnostics >&2\nexit 7");
    let diagnostics = format!(
        "{}\n{}last diagnostic",
        "x".repeat(5000),
        "detail\n".repeat(80)
    );
    suite.write("diagnostics", &diagnostics);
    for flags in [
        vec!["--diff-tool", "builtin"],
        vec!["-e", "--diff-tool", "diff"],
    ] {
        suite.write("one.err", "expected diagnostic\n");
        suite.write("one.ret", "0\n");
        let output = suite.run(&flags);
        status(&output, 1);
        let text = stdout(&output);
        let stderr = text
            .split("  stderr:\n")
            .nth(1)
            .unwrap()
            .split("\n  to reproduce")
            .next()
            .unwrap();
        assert!(stderr.contains(&"x".repeat(5000)));
        assert_eq!(stderr.matches("detail\n").count(), 80);
        assert!(stderr.contains("last diagnostic"));
        assert!(stderr.contains("No newline at end of output"));
        assert!(!text.contains("output truncated"));
    }
}

fn reproduction(output: &Output) -> String {
    stdout(output)
        .split("  to reproduce (")
        .nth(1)
        .expect("failure should include reproduction commands")
        .split_once("):\n")
        .unwrap()
        .1
        .lines()
        .take_while(|line| line.starts_with("    "))
        .map(|line| line.strip_prefix("    ").unwrap())
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn reproduction_preserves_arguments_input_and_compares_after_nonzero_exit() {
    let suite = Suite::new("printf '<%s>\\n' \"$@\"\ncat\nprintf 'crash detail\\n' >&2\nexit 7");
    let stem = "odd 'case";
    let args = [
        "two words",
        "",
        ";",
        "$(touch injected)",
        "*.txt",
        "a'b",
        "line\nnext\t\u{1b}",
    ];
    suite.write(&format!("{stem}.args"), shlex::try_join(args).unwrap());
    suite.write(&format!("{stem}.in"), "from stdin\n");
    suite.write(&format!("{stem}.out"), "expected\n");
    suite.write(&format!("{stem}.err"), "expected error\n");
    suite.write(&format!("{stem}.ret"), "0\n");
    fs::rename(
        suite.dir.path().join("program"),
        suite.dir.path().join("prog 'name"),
    )
    .unwrap();
    // Force the environment fallback so each shell can be tested in a sandbox.
    suite.script("ps", "exit 1");
    let mut paths = vec![suite.dir.path().to_path_buf()];
    paths.extend(std::env::split_paths(
        &std::env::var_os("PATH").unwrap_or_default(),
    ));
    let search_path = std::env::join_paths(paths).unwrap();
    for shell in ["bash", "zsh", "fish", "sh"] {
        match Command::new(shell).args(["-c", "true"]).output() {
            Ok(_) => {}
            Err(err)
                if matches!(shell, "zsh" | "fish")
                    && err.kind() == std::io::ErrorKind::NotFound =>
            {
                continue;
            }
            Err(err) => panic!("cannot start {shell}: {err}"),
        }
        for check_stderr in [false, true] {
            let mut command = suite.command();
            command
                .env("PATH", &search_path)
                .env("SHELL", format!("/bin/{shell}"));
            command.args(["./prog 'name", "-t", stem, "--diff-tool", "builtin"]);
            if check_stderr {
                command.arg("-e");
            }
            let output = command.output().unwrap();
            status(&output, 1);
            let commands = reproduction(&output);
            assert!(!commands.contains('\x1b'));
            assert!(stdout(&output).contains(&format!(
                "to reproduce ({}):",
                if shell == "sh" { "sh; via bash" } else { shell }
            )));
            let mut replay_command = Command::new(shell);
            if shell == "fish" {
                replay_command.arg("--no-config");
            }
            let replay = replay_command
                .args([if shell == "fish" { "-c" } else { "-ec" }, &commands])
                .current_dir(suite.dir.path())
                .output()
                .unwrap();
            assert!(
                replay.status.success(),
                "{shell}: {}",
                String::from_utf8_lossy(&replay.stderr)
            );
            let text = stdout(&replay);
            assert!(text.contains("exit status: 7"));
            assert!(text.contains("+<two words>\n+<>\n+<;>\n+<$(touch injected)>\n+<*.txt>\n+<a'b>\n+<line\n+next\t\u{1b}>\n+from stdin"));
            assert!(String::from_utf8_lossy(&replay.stderr).contains("crash detail"));
            if check_stderr {
                assert!(text.contains("expected exit status: 0"));
                assert!(text.contains("-expected error\n+crash detail"));
            }
            assert!(!suite.dir.path().join("injected").exists());
        }
    }
}

#[test]
fn shell_detection_prefers_active_ancestor_then_falls_back_to_shell_environment() {
    let suite = Suite::new("exit 7");
    suite.script(
        "ps",
        "if [ \"$2\" = 42 ]; then printf '1 /usr/local/bin/fish\\n'; else printf '42 cargo\\n'; fi",
    );
    let mut paths = vec![suite.dir.path().to_path_buf()];
    paths.extend(std::env::split_paths(
        &std::env::var_os("PATH").unwrap_or_default(),
    ));
    let search_path = std::env::join_paths(paths).unwrap();
    let run = |shell: Option<&str>| {
        let mut command = suite.command();
        command
            .env("PATH", &search_path)
            .args(["suite", "./program", "--diff-tool", "builtin"]);
        if let Some(shell) = shell {
            command.env("SHELL", shell);
        } else {
            command.env_remove("SHELL");
        }
        let output = command.output().unwrap();
        status(&output, 1);
        stdout(&output)
    };
    assert!(run(Some("/bin/bash")).contains("to reproduce (fish):"));
    suite.script("ps", "exit 1");
    assert!(run(Some("/bin/zsh")).contains("to reproduce (zsh):"));
    assert!(run(None).contains("to reproduce (sh; via bash):"));
}

#[test]
fn reproduction_uses_eof_when_input_is_absent_and_falls_back_to_diff() {
    let suite = Suite::new("cat\nprintf 'actual\\n'");
    suite.script("ps", "exit 1");
    let mut paths = vec![suite.dir.path().to_path_buf()];
    paths.extend(std::env::split_paths(
        &std::env::var_os("PATH").unwrap_or_default(),
    ));
    let search_path = std::env::join_paths(&paths).unwrap();
    for shell in ["bash", "fish"] {
        let Some(executable) = paths
            .iter()
            .map(|directory| directory.join(shell))
            .find(|path| path.is_file())
        else {
            assert_eq!(shell, "fish", "bash is required for reproduction tests");
            continue;
        };
        suite.script("delta", "printf 'delta replay\\n'\nexit 1");
        let output = diff_command(&suite)
            .env("SHELL", format!("/bin/{shell}"))
            .args(["--color", "never"])
            .output()
            .unwrap();
        let commands = reproduction(&output);
        assert!(commands.contains("< /dev/null"));
        let flag = if shell == "fish" { "-c" } else { "-ec" };
        let mut replay_command = Command::new(&executable);
        if shell == "fish" {
            replay_command.arg("--no-config");
        }
        let replay = replay_command
            .args([flag, &commands])
            .env("PATH", &search_path)
            .current_dir(suite.dir.path())
            .output()
            .unwrap();
        assert!(
            replay.status.success(),
            "{}",
            String::from_utf8_lossy(&replay.stderr)
        );
        assert!(
            stdout(&replay).contains("delta replay"),
            "{shell}: {}\n{}",
            stdout(&replay),
            String::from_utf8_lossy(&replay.stderr)
        );
        fs::remove_file(suite.dir.path().join("delta")).unwrap();
        let mut replay_command = Command::new(&executable);
        if shell == "fish" {
            replay_command.arg("--no-config");
        }
        let replay = replay_command
            .args([flag, &commands])
            .env("PATH", "/usr/bin:/bin")
            .current_dir(suite.dir.path())
            .output()
            .unwrap();
        assert!(
            replay.status.success(),
            "{}",
            String::from_utf8_lossy(&replay.stderr)
        );
        assert!(stdout(&replay).contains("-hello\n+actual"));
    }
}

#[test]
fn failure_colors_cover_names_diagnostics_commands_and_summary() {
    let suite = Suite::new("printf 'detail\\n' >&2\nexit 7");
    let output = suite.run(&["--color", "always", "--diff-tool", "builtin"]);
    status(&output, 1);
    let text = stdout(&output);
    for colored in [
        "\x1b[1;36mone",
        "\x1b[1;35m---- one ----",
        "\x1b[31mprogram exited with 7",
        "\x1b[33m  detail",
        "\x1b[1;36mto reproduce",
        "\x1b[1;31m1 failed",
    ] {
        assert!(text.contains(colored), "missing {colored:?}");
    }
    assert!(
        ["\x1b[32mcapture=", "\x1b[32mbegin", "\x1b[32mbash -c"]
            .iter()
            .any(|prefix| text.contains(prefix))
    );
    assert!(!stdout(&suite.run(&["--color", "never", "--diff-tool", "builtin"])).contains('\x1b'));
}

#[test]
fn signal_exit_has_a_clear_diagnostic() {
    let suite = Suite::new("kill -TERM $$");
    let output = suite.run(&[]);
    status(&output, 1);
    assert!(stdout(&output).contains("signal 15 (shell status 143)"));
}

#[test]
fn missing_fixture_fails_and_next_test_still_runs() {
    let suite = Suite::new("printf 'hello\\n'");
    suite.write("suite", "missing one\n");
    let output = suite.run(&[]);
    status(&output, 1);
    assert!(stdout(&output).contains("cannot read missing.out"));
    assert!(stdout(&output).contains("test one ... ok"));
}

#[test]
fn e_requires_err_and_ret_and_validates_exit_code() {
    let suite = Suite::new("printf 'hello\\n'");
    let output = suite.run(&["-e"]);
    status(&output, 1);
    assert!(stdout(&output).contains("cannot read one.err"));
    suite.write("one.err", "");
    let output = suite.run(&["-e"]);
    status(&output, 1);
    assert!(stdout(&output).contains("cannot read one.ret"));
    for code in ["-1", "256", "NaN", "0 1"] {
        suite.write("one.ret", code);
        let output = suite.run(&["-e"]);
        status(&output, 1);
        assert!(stdout(&output).contains("integer from 0 to 255"));
    }
}

#[test]
fn malformed_arguments_do_not_start_the_program() {
    let suite = Suite::new("touch ran\nprintf 'hello\\n'");
    suite.write("one.args", "'unfinished");
    let output = suite.run(&[]);
    status(&output, 1);
    assert!(stdout(&output).contains("unfinished quote"));
    assert!(!suite.dir.path().join("ran").exists());
}

#[test]
fn color_modes_override_auto_and_no_color() {
    let suite = Suite::new("printf 'hello\\n'");
    let always = suite.run(&["--color", "always"]);
    status(&always, 0);
    assert!(stdout(&always).contains("\x1b[32mok\x1b[0m"));
    let never = suite.run(&["--color", "never"]);
    status(&never, 0);
    assert!(!stdout(&never).contains('\x1b'));
}

#[test]
fn filters_list_and_exact_selection_report_correct_counts() {
    let suite = Suite::new("printf 'hello\\n'");
    suite.write("suite", "one one.more two\n");
    suite.write("one.more.out", "hello\n");
    let output = suite.run(&["--filter", "one"]);
    status(&output, 0);
    assert!(stdout(&output).contains("2 passed; 0 failed; 0 timed out; 1 filtered out"));
    let output = suite.run(&["--filter", "one", "--exact"]);
    status(&output, 0);
    assert!(stdout(&output).contains("1 passed; 0 failed; 0 timed out; 2 filtered out"));
    // Listing is independent of the executable and fixture availability.
    let output = suite
        .command()
        .args(["--list", "--filter", "two", "suite", "missing"])
        .output()
        .unwrap();
    status(&output, 0);
    assert!(stdout(&output).contains("two: test\n"));
    assert!(stdout(&output).contains("1 tests; 2 filtered out"));
}

#[test]
fn direct_test_runs_without_a_suite_file() {
    let suite = Suite::new("printf 'hello\\n'");
    fs::remove_file(suite.dir.path().join("suite")).unwrap();
    for args in [
        vec!["./program", "--test", "one"],
        vec!["-t", "one", "program"],
    ] {
        let output = suite.command().args(args).output().unwrap();
        status(&output, 0);
        assert!(stdout(&output).contains("test one ... ok"));
        assert!(stdout(&output).contains("1 passed; 0 failed; 0 timed out; 0 filtered out"));
    }
}

#[test]
fn direct_tests_run_only_supplied_stems_in_order() {
    let suite = Suite::new("printf 'hello\\n'");
    fs::remove_file(suite.dir.path().join("suite")).unwrap();
    suite.write("two.out", "hello\n");
    suite.write("unselected.out", "wrong\n");
    let output = suite
        .command()
        .args(["program", "-t", "two", "--test", "one", "-t", "two"])
        .output()
        .unwrap();
    status(&output, 0);
    let text = stdout(&output);
    let names: Vec<_> = text
        .lines()
        .filter(|line| line.starts_with("test "))
        .collect();
    assert_eq!(names.len(), 4); // Three result lines and the summary.
    assert!(names[0].starts_with("test two ... ok"));
    assert!(names[1].starts_with("test one ... ok"));
    assert!(names[2].starts_with("test two ... ok"));
    assert!(text.contains("3 passed; 0 failed; 0 timed out; 0 filtered out"));
    assert!(!text.contains("unselected"));
}

#[test]
fn direct_tests_use_companion_files_and_existing_options() {
    let suite = Suite::new("printf '%s\\n' \"$1\"\ncat\nprintf 'warning\\n' >&2\nexit 7");
    fs::remove_file(suite.dir.path().join("suite")).unwrap();
    fs::create_dir(suite.dir.path().join("tests")).unwrap();
    suite.write("tests/nested.case.args", "'two words'\n");
    suite.write("tests/nested.case.in", "from stdin\n");
    suite.write("tests/nested.case.out", "two words\nfrom stdin\n");
    suite.write("tests/nested.case.err", "warning\n");
    suite.write("tests/nested.case.ret", "7\n");
    let output = suite
        .command()
        .args([
            "-e",
            "--test",
            "tests/nested.case",
            "program",
            "--timeout",
            "1",
        ])
        .output()
        .unwrap();
    status(&output, 0);
    assert!(stdout(&output).contains("test tests/nested.case ... ok"));
}

#[test]
fn direct_tests_can_be_listed_or_report_missing_fixtures() {
    let suite = Suite::new("touch ran\nprintf 'hello\\n'");
    fs::remove_file(suite.dir.path().join("suite")).unwrap();
    let output = suite
        .command()
        .args(["absent", "--test", "missing", "--test", "one", "--list"])
        .output()
        .unwrap();
    status(&output, 0);
    assert_eq!(
        stdout(&output),
        "missing: test\none: test\n\n2 tests; 0 filtered out\n"
    );
    assert!(!suite.dir.path().join("ran").exists());
    let output = suite
        .command()
        .args(["program", "--test", "missing", "--test", "one"])
        .output()
        .unwrap();
    status(&output, 1);
    assert!(stdout(&output).contains("cannot read missing.out"));
    assert!(stdout(&output).contains("1 passed; 1 failed"));
}

#[test]
fn direct_tests_require_a_program_and_cannot_be_combined_with_a_suite() {
    let suite = Suite::new("touch ran\nprintf 'hello\\n'");
    for args in [
        vec!["--test", "one"],
        vec!["program"],
        vec!["suite", "program", "--test", "one"],
        vec!["program", "--test"],
    ] {
        let output = suite.command().args(args).output().unwrap();
        status(&output, 2);
    }
    assert!(!suite.dir.path().join("ran").exists());
}

#[test]
fn repeated_filters_select_a_union_without_extra_runs() {
    let suite = Suite::new("printf 'hello\\n'");
    suite.write("suite", "one one.more two unselected\n");
    suite.write("one.more.out", "hello\n");
    suite.write("two.out", "hello\n");
    let output = suite.run(&["--filter", "two", "--filter", "one", "--filter", "one.more"]);
    status(&output, 0);
    let text = stdout(&output);
    assert!(text.contains("3 passed; 0 failed; 0 timed out; 1 filtered out"));
    assert!(text.find("test one ...").unwrap() < text.find("test two ...").unwrap());
    let output = suite.run(&["--filter", "two", "--filter", "one", "--exact"]);
    status(&output, 0);
    assert!(stdout(&output).contains("2 passed; 0 failed; 0 timed out; 2 filtered out"));
    let output = suite.run(&["--filter", "absent", "--filter", "also.absent"]);
    status(&output, 0);
    assert!(stdout(&output).contains("0 passed; 0 failed; 0 timed out; 4 filtered out"));
}

#[test]
fn fail_fast_counts_unrun_tests_as_skipped() {
    let suite = Suite::new("printf 'wrong\\n'");
    suite.write("suite", "one two three\n");
    let output = suite.run(&["--fail-fast"]);
    status(&output, 1);
    assert!(stdout(&output).contains("0 passed; 1 failed; 0 timed out; 0 filtered out; 2 skipped"));
    assert!(!stdout(&output).contains("test two ..."));
}

fn check_parallel_limit(flags: &[&str], jobs: usize) {
    let suite = Suite::new(
        "touch \"started-$1\"\nwhile [ ! -f release ]; do sleep 0.01; done\nif [ \"$1\" = case0 ]; then sleep 0.1; fi\nprintf 'hello\\n'",
    );
    let stems: Vec<_> = (0..jobs + 1).map(|i| format!("case{i}")).collect();
    suite.write("suite", stems.join("\n"));
    for stem in &stems {
        suite.write(&format!("{stem}.args"), stem);
        suite.write(&format!("{stem}.out"), "hello\n");
    }
    let child = suite
        .command()
        .args(flags)
        .args(["suite", "program", "--timeout", "5"])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let started_count = || {
        fs::read_dir(suite.dir.path())
            .unwrap()
            .filter(|entry| {
                entry
                    .as_ref()
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .starts_with("started-")
            })
            .count()
    };
    let deadline = Instant::now() + Duration::from_secs(3);
    while started_count() < jobs && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
    // All active tests wait at the gate, so no worker can start another test.
    std::thread::sleep(Duration::from_millis(50));
    let active = started_count();
    suite.write("release", "");
    let output = child.wait_with_output().unwrap();
    status(&output, 0);
    assert_eq!(active, jobs);
    assert_eq!(started_count(), stems.len());
    let text = stdout(&output);
    let reported: Vec<_> = text
        .lines()
        .filter_map(|line| line.strip_prefix("test case"))
        .map(|line| line.split_once(" ... ok").unwrap().0)
        .collect();
    let expected: Vec<_> = (0..stems.len()).map(|i| i.to_string()).collect();
    assert_eq!(reported, expected);
}

#[test]
fn parallel_default_is_fifty_and_reports_in_suite_order() {
    check_parallel_limit(&[], 50);
}

#[test]
fn jobs_option_limits_parallel_execution() {
    check_parallel_limit(&["--jobs", "2"], 2);
}

#[test]
fn one_job_executes_tests_sequentially_in_suite_order() {
    let suite = Suite::new("printf '%s\\n' \"$1\" >> order\nprintf 'hello\\n'");
    suite.write("suite", "three one two one\n");
    for stem in ["one", "two", "three"] {
        suite.write(&format!("{stem}.args"), stem);
        suite.write(&format!("{stem}.out"), "hello\n");
    }
    status(&suite.run(&["-j", "1"]), 0);
    assert_eq!(
        fs::read_to_string(suite.dir.path().join("order")).unwrap(),
        "three\none\ntwo\none\n"
    );
}

#[test]
fn parallel_fail_fast_reports_the_first_failure_in_suite_order() {
    for first_fails in [false, true] {
        let suite = Suite::new(
            r#"
case "$1" in
    one|two)
        while [ ! -f third-failed ]; do sleep 0.01; done
        sleep 0.15
        printf 'hello\n'
        exit "$2"
        ;;
    three) touch third-failed; exit 7 ;;
    four) touch should-not-run; printf 'hello\n' ;;
esac
"#,
        );
        suite.write("suite", "one two three four\n");
        suite.write("one.args", if first_fails { "one 7" } else { "one 0" });
        suite.write("two.args", "two 7");
        suite.write("three.args", "three");
        suite.write("four.args", "four");
        for stem in ["two", "three", "four"] {
            suite.write(&format!("{stem}.out"), "hello\n");
        }
        let output = suite.run(&["-j", "3", "--fail-fast", "--timeout", "2"]);
        status(&output, 1);
        let text = stdout(&output);
        if first_fails {
            assert!(text.contains("test one ... FAILED"));
            assert!(!text.contains("test two ..."));
            assert!(text.contains("0 passed; 1 failed; 0 timed out; 0 filtered out; 3 skipped"));
        } else {
            assert!(text.contains("test one ... ok"));
            assert!(text.contains("test two ... FAILED"));
            assert!(text.contains("1 passed; 1 failed; 0 timed out; 0 filtered out; 2 skipped"));
        }
        assert!(!text.contains("test three ..."));
        assert!(!text.contains("test four ..."));
        assert!(!suite.dir.path().join("should-not-run").exists());
    }
}

#[test]
fn quiet_and_nocapture_preserve_summary_and_output() {
    let suite = Suite::new("printf 'hello\\n'\nprintf 'warning\\n' >&2");
    let quiet = suite.run(&["-q"]);
    status(&quiet, 0);
    assert!(stdout(&quiet).contains("running 1 test\n.\n"));
    assert!(!stdout(&quiet).contains("test one ..."));
    assert!(!stdout(&quiet).contains("warning"));
    let capture = suite.run(&["--nocapture"]);
    status(&capture, 0);
    assert!(stdout(&capture).contains("one output"));
    assert!(stdout(&capture).contains("hello"));
    assert!(stdout(&capture).contains("warning"));
}

#[test]
fn timeout_kills_the_process_group_and_preserves_partial_output() {
    let suite = Suite::new("printf 'partial\\n'\n(sleep 0.8; touch survived) &\nwait");
    let started = Instant::now();
    // Allow child startup overhead while timing out well before the marker write.
    let output = suite.run(&["--timeout", "0.25"]);
    status(&output, 1);
    assert!(started.elapsed() < Duration::from_secs(2));
    assert!(stdout(&output).contains("timed out after 250ms"));
    assert!(stdout(&output).contains("test one ... TIMEOUT"));
    assert!(stdout(&output).contains("timeouts:\n    one"));
    assert!(stdout(&output).contains("test result: TIMEOUT. 0 passed; 0 failed; 1 timed out"));
    assert!(!stdout(&output).contains("failures:"));
    assert!(!stdout(&output).contains('\x1b'));
    assert!(stdout(&output).contains("partial"));
    std::thread::sleep(Duration::from_millis(900));
    assert!(!suite.dir.path().join("survived").exists());
}

#[test]
fn mixed_results_report_timeouts_separately_in_normal_and_quiet_modes() {
    let suite = Suite::new(
        r#"
case "$1" in
    slow) printf 'partial\n'; sleep 10 ;;
    bad) printf 'wrong\n' ;;
    *) printf 'hello\n' ;;
esac
"#,
    );
    suite.write("suite", "one bad slow\n");
    for stem in ["bad", "slow"] {
        suite.write(&format!("{stem}.args"), stem);
        suite.write(&format!("{stem}.out"), "hello\n");
    }
    for quiet in [false, true] {
        let mut flags = vec!["-j", "1", "--timeout", "0.25", "--diff-tool", "builtin"];
        if quiet {
            flags.push("-q");
        }
        let output = suite.run(&flags);
        status(&output, 1);
        let text = stdout(&output);
        if quiet {
            assert!(text.contains("running 3 tests\n.FT\n"));
        } else {
            assert!(text.contains("test one ... ok"));
            assert!(text.contains("test bad ... FAILED"));
            assert!(text.contains("test slow ... TIMEOUT"));
        }
        assert!(text.contains("failures:\n    bad\n\ntimeouts:\n    slow\n"));
        assert!(text.contains("test result: FAILED. 1 passed; 1 failed; 1 timed out"));
        assert!(text.contains("partial"));
        assert!(!text.contains('\x1b'));
    }
}

#[test]
fn timeout_colors_and_fail_fast_remain_distinct_from_failures() {
    let suite = Suite::new("printf 'partial\\n'\nsleep 10");
    suite.write("suite", "one two\n");
    suite.write("two.out", "hello\n");
    for quiet in [false, true] {
        let mut flags = vec![
            "-j",
            "1",
            "--timeout",
            "0.25",
            "--fail-fast",
            "--color",
            "always",
        ];
        if quiet {
            flags.push("-q");
        }
        let output = suite.run(&flags);
        status(&output, 1);
        let text = stdout(&output);
        for colored in [
            "\x1b[1;33mTIMEOUT\x1b[0m",
            "\x1b[1;33mtimeouts:\x1b[0m",
            "\x1b[1;33m---- one ----\x1b[0m",
            "\x1b[1;33mtimed out after 250ms\x1b[0m",
            "\x1b[1;33mone\x1b[0m",
            "\x1b[1;33m1 timed out\x1b[0m",
        ] {
            assert!(text.contains(colored), "missing {colored:?}");
        }
        if quiet {
            assert!(text.contains("\x1b[1;33mT\x1b[0m\n"));
        }
        assert!(text.contains("0 passed"));
        assert!(text.contains("0 failed"));
        assert!(text.contains("1 skipped"));
        assert!(!text.contains("FAILED"));
        assert!(!text.contains("failures:"));
        assert!(!text.contains("two"));
    }
}

#[test]
fn valgrind_timeout_keeps_its_log_without_becoming_a_memory_failure() {
    let suite = Suite::new("printf 'partial\\n'\nsleep 10");
    suite.write("one.err", "");
    suite.write("one.ret", "0\n");
    mock_valgrind(&suite, "==42== Memcheck startup");
    let output = valgrind_command(&suite)
        .args(["--timeout", "0.25"])
        .output()
        .unwrap();
    status(&output, 1);
    let text = stdout(&output);
    assert!(text.contains("test one ... TIMEOUT"));
    assert!(text.contains("0 passed; 0 failed; 1 timed out"));
    assert!(text.contains("Valgrind log:"));
    assert!(text.contains("==42== Memcheck startup"));
    assert!(text.contains("partial"));
    assert!(!text.contains("did not produce an error summary"));
    assert!(!text.contains("failures:"));
}

#[test]
fn large_stdout_and_stderr_do_not_deadlock() {
    let suite = Suite::new("head -c 200000 /dev/zero\nhead -c 200000 /dev/zero >&2");
    suite.write("one.out", vec![0; 200000]);
    suite.write("one.err", vec![0; 200000]);
    suite.write("one.ret", "0\n");
    status(&suite.run(&["-e", "--timeout", "2"]), 0);
}

#[test]
fn byte_exact_diagnostics_show_missing_newline_and_invalid_bytes() {
    let suite = Suite::new("printf 'hello'");
    let output = suite.run(&["--diff-tool", "builtin"]);
    status(&output, 1);
    assert!(stdout(&output).contains("stdout differs at byte 5"));
    assert!(stdout(&output).contains("No newline at end of output"));
    suite.write("one.out", [0xff, 0x1b, b'\n']);
    let output = suite.run(&["--diff-tool", "builtin"]);
    status(&output, 1);
    assert!(stdout(&output).contains("\\xff\\u{1b}"));
    assert!(!stdout(&output).contains('\x1b'));
}

#[test]
fn paths_with_spaces_and_stems_with_extensions_work() {
    let suite = Suite::new("printf 'hello\\n'");
    fs::rename(
        suite.dir.path().join("program"),
        suite.dir.path().join("program with spaces"),
    )
    .unwrap();
    suite.write("suite file", "one.more\n");
    suite.write("one.more.out", "hello\n");
    let output = suite
        .command()
        .args(["suite file", "./program with spaces"])
        .output()
        .unwrap();
    status(&output, 0);
}

fn diff_command(suite: &Suite) -> Command {
    let mut command = suite.command();
    let mut paths = vec![suite.dir.path().to_path_buf()];
    paths.extend(["/usr/bin".into(), "/bin".into()]);
    command.env("PATH", std::env::join_paths(paths).unwrap());
    command.args(["program", "--test", "one"]);
    command
}

#[test]
fn default_diff_prefers_delta_and_keeps_the_failure_status() {
    let suite = Suite::new("printf 'hello\\n'");
    suite.write("one.out", "goodbye\n");
    suite.script(
        "delta",
        "printf '%s\\n' \"$@\" > diff.args\nprintf 'delta rendered\\n'\nexit 1",
    );
    suite.script("git", "touch git-ran\nexit 2");
    suite.script("diff", "touch diff-ran\nexit 2");
    let output = diff_command(&suite)
        .args(["--color", "always"])
        .output()
        .unwrap();
    status(&output, 1);
    let text = stdout(&output);
    assert!(text.contains("delta rendered"));
    assert!(text.find("stdout differs").unwrap() < text.find("delta rendered").unwrap());
    assert!(text.find("delta rendered").unwrap() < text.find("test result:").unwrap());
    assert!(!text.contains("  expected:"));
    let args = fs::read_to_string(suite.dir.path().join("diff.args")).unwrap();
    assert!(args.contains("--paging=never\n"));
    assert!(!args.contains("--raw\n"));
    assert!(!suite.dir.path().join("git-ran").exists());
    assert!(!suite.dir.path().join("diff-ran").exists());
    // Temporary paths no longer exist after rendering.
    for path in args.lines().filter(|line| line.starts_with('/')) {
        assert!(!Path::new(path).exists());
    }
}

#[test]
fn default_diff_uses_git_when_delta_is_unavailable() {
    let suite = Suite::new("printf 'hello\\n'");
    suite.write("one.out", "goodbye\n");
    suite.script(
        "git",
        "printf '%s\\n' \"$@\" > diff.args\nprintf 'git rendered\\n'\nexit 1",
    );
    let output = diff_command(&suite)
        .args(["--color", "always"])
        .output()
        .unwrap();
    status(&output, 1);
    assert!(stdout(&output).contains("git rendered"));
    let args = fs::read_to_string(suite.dir.path().join("diff.args")).unwrap();
    assert!(args.starts_with(
        "--paginate\ndiff\n--no-index\n--no-ext-diff\n--no-textconv\n--color=always\n--\n"
    ));
}

#[test]
fn system_and_git_diffs_show_real_hunks_and_escape_test_output() {
    let suite = Suite::new("printf 'context\\nnew\\nlast\\n'");
    suite.write("one.out", "context\nold\nlast\n");
    for tool in ["diff", "git"] {
        let output = diff_command(&suite)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .args(["--diff-tool", tool])
            .output()
            .unwrap();
        status(&output, 1);
        let text = stdout(&output);
        assert!(text.contains("@@ -1,3 +1,3 @@"));
        assert!(text.contains(" context\n-old\n+new\n last\n"));
        assert!(!text.contains("  expected:"));
    }
    suite.write("one.out", [0xff, 0x1b, b'\n']);
    let output = diff_command(&suite)
        .args(["--diff-tool", "diff"])
        .output()
        .unwrap();
    status(&output, 1);
    assert!(stdout(&output).contains("-\\xff\\u{1b}"));
    assert!(!stdout(&output).contains('\x1b'));
}

#[test]
fn custom_diff_command_receives_literal_arguments_and_both_streams() {
    let suite = Suite::new("printf 'actual\\n'\nprintf 'actual error\\n' >&2");
    suite.write("one.out", "expected\n");
    suite.write("one.err", "expected error\n");
    suite.write("one.ret", "0\n");
    suite.script(
        "diff tool",
        r#"
printf '%s\n' "$@" >> diff.args
shift 4
printf 'expected file: '; cat "$1"
printf 'actual file: '; cat "$2"
exit 1
"#,
    );
    let tool = format!(
        "'{}' 'two words' ';' '$(touch injected)'",
        suite.dir.path().join("diff tool").display()
    );
    let output = diff_command(&suite)
        .args(["-e", "--diff-tool", &tool])
        .output()
        .unwrap();
    status(&output, 1);
    let text = stdout(&output);
    assert!(text.contains("expected file: expected\nactual file: actual\n"));
    assert!(text.contains("expected file: expected error\nactual file: actual error\n"));
    let args = fs::read_to_string(suite.dir.path().join("diff.args")).unwrap();
    assert!(args.contains("two words\n;\n$(touch injected)\n--\n"));
    assert!(!suite.dir.path().join("injected").exists());
}

#[test]
fn unavailable_or_failing_diff_tools_fall_back_to_builtin() {
    let suite = Suite::new("printf 'actual\\n'");
    suite.write("one.out", "expected\n");
    suite.script("broken-diff", "exit 2");
    for tool in ["nonexistent-diff-tool", "broken-diff"] {
        let output = diff_command(&suite)
            .args(["--diff-tool", tool])
            .output()
            .unwrap();
        status(&output, 1);
        let text = stdout(&output);
        assert!(text.contains("diff unavailable:"));
        assert!(text.contains("- expected"));
        assert!(text.contains("+ actual"));
    }
    for tool in ["", "'unfinished", "''"] {
        status(
            &diff_command(&suite)
                .args(["--diff-tool", tool])
                .output()
                .unwrap(),
            2,
        );
    }
}

#[test]
fn absolute_suite_path_resolves_fixtures_relative_to_suite() {
    let suite = Suite::new("printf 'hello\\n'");
    let suites = tempfile::tempdir().unwrap();
    fs::write(suites.path().join("suite"), "one\n").unwrap();
    fs::write(suites.path().join("one.out"), "hello\n").unwrap();
    suite.write("one.out", "wrong directory\n");
    let output = suite
        .command()
        .arg(suites.path().join("suite"))
        .arg("program")
        .output()
        .unwrap();
    status(&output, 0);
    assert!(stdout(&output).contains("test one ... ok"));
}

#[test]
fn relative_suite_path_resolves_all_companion_files_and_preserves_test_names() {
    let suite = Suite::new("cat \"$1\"\ncat\nprintf 'warning\\n' >&2\nexit 7");
    fs::create_dir_all(suite.dir.path().join("suite files/nested")).unwrap();
    suite.write("suite files/suite", "nested/one.case missing\n");
    suite.write("suite files/nested/one.case.args", "'program input'\n");
    suite.write("suite files/nested/one.case.in", "stdin\n");
    suite.write("suite files/nested/one.case.out", "argument input\nstdin\n");
    suite.write("suite files/nested/one.case.err", "warning\n");
    suite.write("suite files/nested/one.case.ret", "7\n");
    suite.write("program input", "argument input\n");
    let output = suite
        .command()
        .args([
            "suite files/suite",
            "./program",
            "-e",
            "--filter",
            "nested/one.case",
            "--exact",
        ])
        .output()
        .unwrap();
    status(&output, 0);
    assert!(stdout(&output).contains("test nested/one.case ... ok"));
    assert!(stdout(&output).contains("1 passed; 0 failed; 0 timed out; 1 filtered out"));
    let listed = suite
        .command()
        .args(["suite files/suite", "absent", "--list"])
        .output()
        .unwrap();
    status(&listed, 0);
    assert_eq!(
        stdout(&listed),
        "nested/one.case: test\nmissing: test\n\n2 tests; 0 filtered out\n"
    );
}

#[test]
fn missing_suite_relative_fixture_does_not_fall_back_to_working_directory() {
    let suite = Suite::new("touch ran\nprintf 'hello\\n'");
    fs::create_dir(suite.dir.path().join("tests")).unwrap();
    suite.write("tests/suite", "one\n");
    let output = suite
        .command()
        .args(["tests/suite", "program"])
        .output()
        .unwrap();
    status(&output, 1);
    assert!(stdout(&output).contains("cannot read tests/one.out"));
    assert!(!suite.dir.path().join("ran").exists());
}

#[test]
fn absolute_stems_are_preserved_in_a_nested_suite() {
    let suite = Suite::new("printf 'hello\\n'");
    fs::create_dir(suite.dir.path().join("tests")).unwrap();
    suite.write(
        "tests/suite",
        format!("{}\n", suite.dir.path().join("one").display()),
    );
    let output = suite
        .command()
        .args(["tests/suite", "program"])
        .output()
        .unwrap();
    status(&output, 0);
}

#[test]
fn empty_suite_has_a_zero_test_summary() {
    let suite = Suite::new("printf 'hello\\n'");
    suite.write("suite", " \n\t");
    let output = suite.run(&[]);
    status(&output, 0);
    assert!(stdout(&output).contains("running 0 tests"));
    assert!(stdout(&output).contains("0 passed; 0 failed"));
}

#[test]
fn configuration_and_clap_errors_use_exit_two() {
    let suite = Suite::new("printf 'hello\\n'");
    for args in [
        vec![],
        vec!["--unknown"],
        vec!["suite", "absent"],
        vec!["absent", "program"],
    ] {
        let output = suite.command().args(args).output().unwrap();
        status(&output, 2);
    }
    for timeout in ["0", "-1", "NaN", "inf", "abc", "1e100"] {
        status(&suite.run(&["--timeout", timeout]), 2);
    }
    for jobs in ["0", "-1", "abc"] {
        status(&suite.run(&["--jobs", jobs]), 2);
    }
    for limit in ["0", "-1", "abc", "18446744073709551616"] {
        status(&suite.run(&["--max-output-bytes", limit]), 2);
    }
    status(&suite.command().arg("--help").output().unwrap(), 0);
}

fn mock_valgrind(suite: &Suite, summary: &str) {
    suite.script(
        "valgrind",
        &format!(
            r#"
while [ "$#" -gt 0 ]; do
    case "$1" in
        --log-file=*) log="${{1#--log-file=}}"; shift ;;
        --*) shift ;;
        *) break ;;
    esac
done
printf '%s\n' '{summary}' > "$log"
exec "$@"
"#
        ),
    );
}

fn valgrind_command(suite: &Suite) -> Command {
    let mut command = suite.command();
    let mut paths = vec![suite.dir.path().to_path_buf()];
    paths.extend(std::env::split_paths(
        &std::env::var_os("PATH").unwrap_or_default(),
    ));
    command.env("PATH", std::env::join_paths(paths).unwrap());
    command.args(["-v", "-e", "suite", "./program"]);
    command
}

#[test]
fn valgrind_memory_errors_fail_without_overriding_target_exit_status() {
    let suite = Suite::new("printf 'hello\\n'\nprintf 'target stderr\\n' >&2\nexit 7");
    suite.write("one.err", "target stderr\n");
    suite.write("one.ret", "7\n");
    mock_valgrind(&suite, "==42== ERROR SUMMARY: 0 errors from 0 contexts");
    status(&valgrind_command(&suite).output().unwrap(), 0);
    mock_valgrind(&suite, "==42== ERROR SUMMARY: 1,234 errors from 1 context");
    let output = valgrind_command(&suite).output().unwrap();
    status(&output, 1);
    assert!(stdout(&output).contains("Valgrind reported 1234 memory error(s)"));
    assert!(stdout(&output).contains("Valgrind log"));
    assert!(!stdout(&output).contains("\nexit status:"));
}

#[test]
fn valgrind_missing_or_incomplete_log_is_reported() {
    let suite = Suite::new("printf 'hello\\n'");
    suite.write("one.err", "");
    suite.write("one.ret", "0\n");
    mock_valgrind(&suite, "valgrind: startup error");
    let output = valgrind_command(&suite).output().unwrap();
    status(&output, 1);
    assert!(stdout(&output).contains("did not produce an error summary"));
    let output = suite
        .command()
        .env("PATH", Path::new("/nonexistent"))
        .args(["-v", "suite", "./program"])
        .output()
        .unwrap();
    status(&output, 1);
    assert!(stdout(&output).contains("install valgrind or omit -v"));
}

#[test]
fn default_timeout_stops_a_spinlock_and_keeps_partial_output() {
    let suite = Suite::new("printf 'partial\\n'\nwhile :; do :; done");
    let started = Instant::now();
    let output = suite.run(&[]);
    status(&output, 1);
    assert!(stdout(&output).contains("timed out after 500ms"));
    assert!(stdout(&output).contains("partial"));
    assert!(started.elapsed() < Duration::from_secs(3));
}

#[test]
fn explicit_timeout_can_extend_the_default() {
    let suite = Suite::new("sleep 0.65\nprintf 'hello\\n'");
    status(&suite.run(&["--timeout", "2"]), 0);
}

#[test]
fn output_limit_allows_exactly_the_limit_and_rejects_the_next_byte() {
    for stream in ["stdout", "stderr"] {
        let redirect = if stream == "stdout" { "" } else { ">&2" };
        let suite = Suite::new(&format!("printf '12345678' {redirect}"));
        suite.write("one.out", if stream == "stdout" { "12345678" } else { "" });
        suite.write("one.err", if stream == "stderr" { "12345678" } else { "" });
        suite.write("one.ret", "0\n");
        status(&suite.run(&["-e", "--max-output-bytes", "8"]), 0);
        suite.script("program", &format!("printf '123456789' {redirect}"));
        let output = suite.run(&["-e", "--max-output-bytes", "8"]);
        status(&output, 1);
        assert!(stdout(&output).contains(&format!("{stream} exceeded output limit of 8 bytes")));
        assert!(stdout(&output).contains("12345678"));
        assert!(!stdout(&output).contains("123456789"));
    }
}

#[test]
fn endless_stdout_and_stderr_fail_promptly_with_bounded_capture() {
    for redirect in ["", ">&2"] {
        let suite = Suite::new(&format!(
            "(sleep 0.8; touch survived) &\nwhile :; do printf 'xxxxxxxxxxxxxxxx' {redirect}; done"
        ));
        let temporary = tempfile::tempdir().unwrap();
        let started = Instant::now();
        let output = suite
            .command()
            .env("TMPDIR", temporary.path())
            .args([
                "suite",
                "program",
                "--max-output-bytes",
                "4096",
                "--timeout",
                "5",
            ])
            .output()
            .unwrap();
        status(&output, 1);
        assert!(started.elapsed() < Duration::from_secs(3));
        assert!(stdout(&output).contains("exceeded output limit of 4096 bytes"));
        assert!(!stdout(&output).contains("timed out after"));
        assert!(stdout(&output).contains("0 timed out"));
        assert!(output.stdout.len() < 6000);
        assert_eq!(fs::read_dir(temporary.path()).unwrap().count(), 0);
        std::thread::sleep(Duration::from_millis(900));
        assert!(!suite.dir.path().join("survived").exists());
    }
}

#[test]
fn default_output_limit_is_one_mebibyte() {
    let suite = Suite::new("head -c 1048577 /dev/zero | tr '\\000' x");
    let output = suite.run(&[]);
    status(&output, 1);
    assert!(stdout(&output).contains("stdout exceeded output limit of 1048576 bytes"));
}

#[test]
fn successful_program_exit_still_cleans_up_descendants() {
    let suite = Suite::new("(sleep 0.8; touch survived) &\nprintf 'hello\\n'");
    status(&suite.run(&[]), 0);
    std::thread::sleep(Duration::from_millis(900));
    assert!(!suite.dir.path().join("survived").exists());
}

#[test]
fn sigkill_of_runner_kills_children_and_removes_capture_resources() {
    use std::process::Stdio;
    let suite = Suite::new(
        "trap '' TERM\n(sleep 1.2; touch survived-$1) &\necho $! > descendant-$1.pid\necho $$ > parent-$1.pid\nwait",
    );
    suite.write("suite", "one two three\n");
    for stem in ["one", "two", "three"] {
        suite.write(&format!("{stem}.args"), stem);
        suite.write(&format!("{stem}.out"), "");
    }
    let temporary = tempfile::tempdir().unwrap();
    let mut runner = suite
        .command()
        .env("TMPDIR", temporary.path())
        .args(["suite", "program", "--timeout", "10"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let started = Instant::now();
    while ["one", "two", "three"]
        .iter()
        .any(|stem| !suite.dir.path().join(format!("parent-{stem}.pid")).exists())
    {
        if started.elapsed() > Duration::from_secs(3) {
            let _ = runner.kill();
            let _ = runner.wait();
            panic!("test program did not start");
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    runner.kill().unwrap(); // SIGKILL: the runner cannot execute cleanup code.
    runner.wait().unwrap();
    let started = Instant::now();
    while fs::read_dir(temporary.path()).unwrap().count() != 0 {
        assert!(
            started.elapsed() < Duration::from_secs(3),
            "watchdog did not clean up"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
    std::thread::sleep(Duration::from_millis(1400));
    for stem in ["one", "two", "three"] {
        assert!(!suite.dir.path().join(format!("survived-{stem}")).exists());
    }
    #[cfg(target_os = "linux")]
    for name in [
        "parent-one.pid",
        "descendant-one.pid",
        "parent-two.pid",
        "descendant-two.pid",
        "parent-three.pid",
        "descendant-three.pid",
    ] {
        let pid = fs::read_to_string(suite.dir.path().join(name)).unwrap();
        if let Ok(stat) = fs::read_to_string(format!("/proc/{}/stat", pid.trim())) {
            // A container's PID 1 may leave reparented zombies; they cannot run.
            assert!(
                stat.rsplit_once(") ").unwrap().1.starts_with('Z'),
                "process still alive: {stat}"
            );
        }
    }
}

#[test]
fn valgrind_log_is_bounded_without_spooling_to_disk() {
    let suite = Suite::new("printf 'hello\\n'");
    suite.write("one.err", "");
    suite.write("one.ret", "0\n");
    mock_valgrind(&suite, "x".repeat(8192).as_str());
    let output = valgrind_command(&suite)
        .args(["--max-output-bytes", "4096"])
        .output()
        .unwrap();
    status(&output, 1);
    assert!(stdout(&output).contains("Valgrind log exceeded output limit of 4096 bytes"));
    assert!(!stdout(&output).contains("did not produce an error summary"));
}

#[test]
fn signaled_program_exit_still_cleans_up_descendants() {
    let suite = Suite::new("(sleep 0.8; touch survived) &\nkill -KILL $$");
    let output = suite.run(&[]);
    status(&output, 1);
    assert!(stdout(&output).contains("signal 9"));
    std::thread::sleep(Duration::from_millis(900));
    assert!(!suite.dir.path().join("survived").exists());
}
