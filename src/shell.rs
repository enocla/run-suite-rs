use std::{env, path::Path, process::Command};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shell {
    Bash,
    Zsh,
    Fish,
    Posix,
}

impl Shell {
    pub fn detect() -> Self {
        #[cfg(unix)]
        {
            // SHELL names the login shell, which can differ from the active shell.
            let mut pid = unsafe { libc::getppid() }.to_string();
            for _ in 0..16 {
                let Ok(output) = Command::new("ps")
                    .args(["-p", &pid, "-o", "ppid=", "-o", "comm="])
                    .output()
                else {
                    break;
                };
                if !output.status.success() {
                    break;
                }
                let text = String::from_utf8_lossy(&output.stdout);
                let text = text.trim();
                let Some((parent, command)) = text.split_once(char::is_whitespace) else {
                    break;
                };
                if let Some(shell) = Self::from_command(command.trim()) {
                    return shell;
                }
                if parent == "0" || parent == pid {
                    break;
                }
                pid = parent.to_owned();
            }
        }
        env::var("SHELL")
            .ok()
            .and_then(|shell| Self::from_command(&shell))
            .unwrap_or(Self::Posix)
    }

    fn from_command(command: &str) -> Option<Self> {
        match Path::new(command)
            .file_name()?
            .to_str()?
            .trim_start_matches('-')
        {
            "bash" => Some(Self::Bash),
            "zsh" => Some(Self::Zsh),
            "fish" => Some(Self::Fish),
            "sh" | "dash" | "ash" | "ksh" => Some(Self::Posix),
            _ => None,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Bash => "bash",
            Self::Zsh => "zsh",
            Self::Fish => "fish",
            Self::Posix => "sh; via bash",
        }
    }

    pub fn quote(self, word: &str) -> String {
        if self == Self::Fish {
            if word.chars().any(char::is_control) {
                return word.bytes().map(|byte| format!("\\X{byte:02x}")).collect();
            }
            return format!("'{}'", word.replace('\\', "\\\\").replace('\'', "\\'"));
        }
        if word.chars().any(char::is_control) {
            return format!(
                "$'{}'",
                word.bytes()
                    .map(|byte| format!("\\{byte:03o}"))
                    .collect::<String>()
            );
        }
        shlex::try_quote(word)
            .expect("fixture arguments cannot contain NUL")
            .into_owned()
    }

    pub fn path(self, path: &Path) -> String {
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStrExt;
            if path.to_str().is_none() {
                let bytes = path.as_os_str().as_bytes();
                return if self == Self::Fish {
                    bytes.iter().map(|byte| format!("\\X{byte:02x}")).collect()
                } else {
                    format!(
                        "$'{}'",
                        bytes
                            .iter()
                            .map(|byte| format!("\\{byte:03o}"))
                            .collect::<String>()
                    )
                };
            }
        }
        self.quote(&path.to_string_lossy())
    }

    pub fn wrap(self, lines: Vec<String>) -> Vec<String> {
        if self != Self::Posix {
            return lines;
        }
        // Older /bin/sh implementations lack ANSI-C quoting. Explicitly invoke
        // bash so controls and arbitrary path bytes keep their exact values.
        let script = lines.join("\n").replace('\'', "'\"'\"'");
        format!("bash -c '{script}'")
            .lines()
            .map(str::to_owned)
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::Shell;

    #[test]
    fn recognizes_shell_paths_and_login_process_names() {
        assert_eq!(
            Shell::from_command("/usr/local/bin/fish"),
            Some(Shell::Fish)
        );
        assert_eq!(Shell::from_command("-zsh"), Some(Shell::Zsh));
        assert_eq!(Shell::from_command("/bin/bash"), Some(Shell::Bash));
        assert_eq!(Shell::from_command("dash"), Some(Shell::Posix));
        assert_eq!(Shell::from_command("cargo"), None);
    }
}
