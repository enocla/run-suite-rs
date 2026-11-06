# Rust runSuite

A Rust/clap runner for the existing CS246 suite format, with colored results,
failure diagnostics, timings, and a summary in the style of `cargo test`.
The original executables remain at `../runSuite` and `../1269/tools/runSuite`.

## Build and run

From this directory:

```sh
cargo build --release
./target/release/runSuite --help
```

Test fixtures are read relative to the suite file. For example, from the
coursework root:

```sh
runSuite working/a2/q3/suiteq3.txt ./working/a2/q3/a2q3-exec
```

You can also run test stems directly, without creating or specifying a suite file:

```sh
runSuite ./a2q3-exec --test test1
runSuite ./a2q3-exec --test test1 --test tests/nested.case
```

Repeat `--test` (or `-t`) for a set of tests. Supply stems without companion
extensions such as `.out`; the usual fixture files are loaded from the current
working directory. Direct tests are reported in the order supplied, including repeats.
`--test` cannot be combined with a suite file. All other options apply as usual.

Example output excerpt with `--diff-tool builtin` (headings, test names, diagnostics,
commands, and summary counts are colored on a terminal):

```text
running 3 tests
test test1 ... ok (2ms)
test test2 ... FAILED (3ms)
test test3 ... ok (2ms)

failures:

---- test2 ----
stdout differs at byte 0 (expected 4 bytes, actual 4 bytes)
  expected:
    1 | - yes
  actual:
    1 | + no!

failures:
    test2

test result: FAILED. 2 passed; 1 failed; 0 timed out; 0 filtered out; 0 skipped; finished in 7ms
```

## Suite format

The suite file contains whitespace-separated stems, for example:

```text
test1 test2
tests/nested.case
```

Relative stems are resolved from the **suite file's directory**. For example,
`tests/suite.txt` containing `one` reads `tests/one.out`, `tests/one.in`, and the
other companion files from `tests/`. Absolute stems are used as given. An
existing stem extension is preserved: `nested.case` uses `nested.case.out`.
With `--test` and no suite file, stems are relative to the current working
directory. Executable paths and paths passed as program arguments remain
relative to the current working directory.

| Companion file | Meaning |
| --- | --- |
| `STEM.out` | Required expected stdout; compared byte for byte. |
| `STEM.in` | Optional stdin file; missing means immediate EOF. |
| `STEM.args` | Optional UTF-8 arguments with shell-style quotes and escapes. |
| `STEM.err` | Required expected stderr when `-e` is enabled; compared byte for byte. |
| `STEM.ret` | Required exit status when `-e` is enabled; an integer from 0 through 255. |

The suite file must be UTF-8. Output and input may contain arbitrary bytes.
There are no suite comments: every whitespace-separated token is a test stem.

Arguments such as `'two words' "" '*.txt'` become three literal arguments,
including an empty argument. No shell executes the program invocation;
variables, command substitutions, and globs are not expanded. `shlex` recognizes
unquoted shell comments in `.args`. Unfinished quotes or escapes fail that test.
An executable can be an explicit path, a bare local name, or a command on `PATH`.

## Options

| Option | Behavior |
| --- | --- |
| `-t STEM`, `--test STEM` | Run a test stem directly without a suite; repeat for a set. |
| `-e`, `--check-stderr` | Compare stderr and exit status using `.err` and `.ret`. |
| `-v`, `--valgrind` | Run under Memcheck; show its log and fail on reported memory errors. |
| `--filter PATTERN` | Select test stems containing any supplied pattern; repeat for a set. |
| `--filter NAME --exact` | Match each supplied filter against the entire stem. |
| `--list` | List selected tests without validating or running the executable. |
| `-j N`, `--jobs N` | Run at most N tests in parallel (default: 50); use 1 for sequential execution. |
| `--fail-fast` | Report through the first failure or timeout in suite order; count later selected tests as skipped. |
| `--nocapture` | Also show captured stdout/stderr for passing tests. |
| `-q`, `--quiet` | Use progress dots, `F` for failures, and `T` for timeouts, retaining details and summaries. |
| `--timeout SECONDS` | Per-test limit, supporting fractional seconds; default: 0.5 seconds (500 ms), or 10 seconds with `-v`/`--valgrind`. |
| `--max-output-bytes BYTES` | Maximum captured bytes per stream; default: 1048576 (1 MiB). Exceeding the limit fails the test. |
| `--color auto\|always\|never` | Auto colors terminal output unless `NO_COLOR` is set or `TERM=dumb`. |
| `--diff-tool COMMAND` | Failure renderer; `auto` (default) tries delta, Git, then system diff. Use `builtin` or a quoted command to override. |

Tests run in parallel with up to 50 concurrent tests by default, including
repeated stems. Results are reported in suite order; details retain suite order
within the failures and timeouts sections.
Use `--jobs 1` for tests that share files or otherwise need sequential execution.
With `--fail-fast`, any completed failure or timeout stops new tests from being
scheduled. Earlier tests finish so the reported failure or timeout is the first
in suite order, even if a later test finished sooner. Already running tests
finish; results after the first failure or timeout in suite order are omitted
and counted as skipped.
Passing output is hidden by default. Failure reports escape terminal control
characters and invalid UTF-8. Within the output limit, comparison uses the
complete, original bytes. Built-in output blocks show the full output and mark
missing final newlines. Built-in
expected and actual blocks include right-aligned line numbers starting at 1.
Every failed
test shows its captured stderr, including when stderr also has a diff. Output-limit
failures retain only the first `--max-output-bytes` bytes per stream and identify
which stream was truncated.

## Failure diffs

Mismatches automatically use `delta` when it is available on `PATH`, with paging
disabled so results and the summary stay in the terminal. Delta reads your usual
configuration. When color is disabled (including redirected output), automatic
selection skips delta and uses a plain Git or system diff. The bare `delta`
preset also uses plain Git output when color is disabled.

If delta is unavailable, the runner tries `git diff --no-index`, which uses your
configured Git pager on a terminal when color is enabled, then system `diff -u`.
No Git repository is needed. If no tool is installed, or the chosen tool fails,
the built-in expected and actual blocks are used. Rendering never changes the
test's pass/fail status.

Choose a tool explicitly, or pass a quoted command with arguments:

```sh
runSuite ./program --test test1 --diff-tool delta
runSuite ./program --test test1 --diff-tool git
runSuite ./program --test test1 --diff-tool diff
runSuite ./program --test test1 --diff-tool 'delta --paging=never --side-by-side'
runSuite ./program --test test1 --diff-tool builtin
```

Custom commands receive `-- EXPECTED_FILE ACTUAL_FILE`, with expected first.
Arguments support shell-style quoting; the runner does not execute a shell or
interpret pipes. Temporary files contain escaped display text, preserving line
breaks and missing final newlines. They are removed after the tool finishes.
External diffs use complete display text; their output limits, paging, and color
are controlled by the tool. Bare delta and Git presets follow `--color`.

## Selecting tests from a suite

Run one test or a chosen set without editing the suite file:

```sh
runSuite suiteq3.txt ./a2q3-exec --filter test1 --exact
runSuite suiteq3.txt ./a2q3-exec --filter test1 --filter test3 --exact
```

Omit `--exact` to select groups by substring, such as `--filter insert --filter
remove`. A test matching multiple filters runs once per occurrence in the suite.
With no matching tests, the runner reports zero tests and all suite entries as
filtered out. Add `--list` to preview a selection without running it.

Exit codes: **0** for all selected tests passing, **1** for any test failure,
**2** for command-line or suite/program configuration errors. Without `-e`, a
nonzero program exit is a failure. With `-e`, the specified nonzero exit may
pass; Unix signals use shell status `128 + signal` for comparison.

Stdout and stderr are drained concurrently through pipes into bounded memory;
they are never spooled to temporary files. Each stream is limited to 1 MiB by
default, even when stderr comparison is disabled. On Unix the separate Valgrind
log also uses a bounded pipe, so it cannot fill temporary storage. Increase the
limit with `--max-output-bytes BYTES` when larger output is expected. A test that
exceeds a capture limit fails and retains the captured prefix for diagnosis.

Tests time out after 500 ms by default, or 10 seconds with `-v`/`--valgrind`,
including time spent in Valgrind. An explicit `--timeout SECONDS` overrides
either default. Timeouts appear as yellow `TIMEOUT`
results (`T` in quiet mode), with a separate `timeouts:` list and `timed out`
summary count. They are excluded from the `failed` count but still cause exit
status 1 and trigger `--fail-fast`. If only timeouts prevent success, the overall
result is yellow `TIMEOUT`; other failures make it red `FAILED`. Timed-out tests
retain partial output.
On Unix, each test has a separate watchdog process that kills its process group
on timeout, output overflow, normal program exit, or runner termination, including
`SIGKILL`. The watchdog also removes temporary capture resources if the runner
cannot run cleanup. Descendants that deliberately leave the process group (for
example with `setsid`) are outside this cleanup guarantee. On other platforms,
timeouts and output overflow stop the immediate child; the Unix watchdog and
bounded Valgrind log are unavailable.

Valgrind must be installed and available on `PATH` for `-v`. Its log is separate
from target stderr. The runner reads `ERROR SUMMARY` and preserves the target's
exit code rather than assigning a special error exit code. Memcheck's default
leak-error policy applies; all leak kinds are displayed. Missing or incomplete
logs fail the test. See the [Valgrind manual](https://valgrind.org/docs/manual/manual-core.html)
and [Memcheck manual](https://valgrind.org/docs/manual/mc-manual.html).
CLI parsing uses [clap's derive API](https://docs.rs/clap/latest/clap/_derive/).

## Analysis of the original binary

Both supplied binaries are identical (SHA-256
`d5e438b7562ae894ee007bed776802390ed59431523ba2179c216e2132a05d53`).
No source for the runner is included. Its retained C++ symbols, `.rodata`, and
disassembly expose `validateArguments`, `runTests`, `runTest`, `getTempFile`,
and `isSameFile`. Small controlled executions confirmed the following behavior:

1. Invocation is `runSuite [-v] [-e] suiteFile programName` with separate flags
   before the two positional arguments. It checks suite readability and program
   executability, then reads stems using C++ whitespace extraction.
2. For each stem, `.out` is required. Missing/unreadable `.out` stops the suite.
   Existing unreadable `.args` or `.in` produces a diagnostic and skips that test.
3. `runTest` builds a shell command. Optional `.args` is passed through
   `$(cat STEM.args)`: the shell performs word splitting and filename expansion;
   quotes inside that file do not group arguments. Optional `.in` redirects stdin;
   without it, the child inherits the runner's stdin.
4. Stdout goes to a `mktemp` file and is compared with `.out` using `diff`.
   Passing tests print nothing. Failures print the stem, arguments, input, and
   expected/actual output for each differing checked stream.
5. `-e` redirects stderr and saves `$?` as a newline-terminated decimal `.ret`
   temporary file. It compares these with `STEM.err` and `STEM.ret` using `diff`.
   Missing expectations become noisy `diff`/`cat` errors. Without `-e`, target
   stderr is inherited and nonzero exit status is ignored.
6. `-v` prepends `valgrind --log-fd=3 --error-exitcode=410 --leak-check=full
   --show-leak-kinds=all` and redirects descriptor 3 to `STEM.mem`. That log is
   never inspected; it is left in the fixture directory. A memory error can
   affect the checked status with `-e`, but does not itself fail a stdout-only test.
7. Temporary capture files are removed. Output mismatches **do not change the
   runner's exit code**: a suite with failed comparisons still returns 0.

The Rust runner keeps the fixture format and core comparisons while making
failures visible in the exit code and summary. Deliberate behavior changes are
quote-aware literal arguments, EOF for missing input, numeric `.ret` comparison,
captured stderr, continued execution after invalid fixtures, and checked Valgrind
logs. Clap supplies help, long flags, combined short flags, and flexible ordering.

## Verification

```sh
cargo test
cargo clippy --all-targets -- -D warnings
cargo fmt --check
```

The integration tests cover suite-relative fixtures, parallel execution limits,
ordered results and fail-fast behavior, real child processes, colored/plain reports,
quoting and literal shell characters, input, byte-exact output, stderr/status,
signals, malformed fixtures, filtering, listing, fail-fast, quiet/nocapture,
default and custom timeouts, descendant cleanup after successful exit and runner
SIGKILL, exact output-limit boundaries, endless stdout/stderr, bounded Valgrind
logs, large output capture, and Valgrind integration.
Valgrind integration tests use a controlled executable substitute. A manual
smoke test with real Valgrind also verified capture through the log pipe.

Both the original and Rust runner also passed the 25-test suite in
`working/a2/q3/suiteq3.txt` against `a2q3-exec`.
