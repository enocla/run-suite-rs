use crate::report::visible_bytes;
use std::{
    fs,
    io::{self, Write},
    path::Path,
    process::{Command, Stdio},
    str::FromStr,
};

#[derive(Debug, Clone)]
pub enum DiffTool {
    Auto,
    Builtin,
    Command(Vec<String>),
}

impl FromStr for DiffTool {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "auto" => Ok(Self::Auto),
            "builtin" => Ok(Self::Builtin),
            _ => {
                let words = shlex::split(value)
                    .filter(|words| !words.is_empty() && !words[0].is_empty())
                    .ok_or("diff tool must be a command with balanced quotes and escapes")?;
                if words.iter().any(|word| word.contains('\0')) {
                    return Err("diff tool contains a NUL byte".into());
                }
                Ok(Self::Command(words))
            }
        }
    }
}

impl DiffTool {
    /// Returns false when the built-in report should be used instead.
    pub fn show(
        &self,
        stream: &str,
        expected: &[u8],
        actual: &[u8],
        color: bool,
    ) -> Result<bool, String> {
        if matches!(self, Self::Builtin) {
            return Ok(false);
        }
        let expected = display_text(expected);
        let actual = display_text(actual);
        // Escaping can make distinct byte sequences look identical.
        if expected == actual {
            return Ok(false);
        }
        let directory = tempfile::tempdir().map_err(|err| err.to_string())?;
        let expected_path = directory.path().join(format!("expected.{stream}"));
        let actual_path = directory.path().join(format!("actual.{stream}"));
        fs::write(&expected_path, expected).map_err(|err| err.to_string())?;
        fs::write(&actual_path, actual).map_err(|err| err.to_string())?;
        io::stdout().flush().map_err(|err| err.to_string())?;
        match self {
            Self::Auto => {
                // Missing tools are normal; a tool that starts but fails gets a fallback report.
                let tools: &[&str] = if color {
                    &["delta", "git", "diff"]
                } else {
                    &["git", "diff"]
                };
                for &name in tools {
                    match run_tool(&[name.to_owned()], &expected_path, &actual_path, color) {
                        Ok(()) => return Ok(true),
                        Err(err) if err.kind() == io::ErrorKind::NotFound => continue,
                        Err(err) => return Err(format!("{name}: {err}")),
                    }
                }
                Ok(false)
            }
            Self::Command(words) => {
                run_tool(words, &expected_path, &actual_path, color)
                    .map_err(|err| format!("{}: {err}", words[0]))?;
                Ok(true)
            }
            Self::Builtin => unreachable!(),
        }
    }
}

fn display_text(bytes: &[u8]) -> String {
    let mut text = String::new();
    for line in bytes.split_inclusive(|byte| *byte == b'\n') {
        let newline = line.ends_with(b"\n");
        text.push_str(&visible_bytes(if newline {
            &line[..line.len() - 1]
        } else {
            line
        }));
        if newline {
            text.push('\n');
        }
    }
    text
}

fn run_tool(words: &[String], expected: &Path, actual: &Path, color: bool) -> io::Result<()> {
    // Delta can retain ANSI from its internally generated diff even with --raw.
    // Use plain Git output for the delta preset when colors are disabled.
    let name = if words.len() == 1 && words[0] == "delta" && !color {
        "git"
    } else {
        &words[0]
    };
    let mut command = Command::new(name);
    // Bare names are convenient presets; commands with arguments are passed through.
    if words.len() == 1 {
        match name {
            "delta" => {
                command.arg("--paging=never");
                command.arg("--diff-args=--no-ext-diff --no-textconv --color=always");
            }
            "git" => {
                command.args([
                    if color { "--paginate" } else { "--no-pager" },
                    "diff",
                    "--no-index",
                    "--no-ext-diff",
                    "--no-textconv",
                    if color {
                        "--color=always"
                    } else {
                        "--color=never"
                    },
                ]);
            }
            "diff" => {
                command.arg("-u");
            }
            _ => {}
        }
    } else {
        command.args(&words[1..]);
    }
    let status = command
        .arg("--")
        .arg(expected)
        .arg(actual)
        .stdin(Stdio::null())
        .status()?;
    // Diff tools conventionally return 1 when the files differ.
    if matches!(status.code(), Some(0 | 1)) {
        Ok(())
    } else {
        Err(io::Error::other(format!("diff tool exited with {status}")))
    }
}
