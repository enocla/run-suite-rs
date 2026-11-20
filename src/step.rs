use crate::{
    capture::{Capture, OutputLine, nonblocking},
    report::{safe_text, style, visible_bytes},
};
use std::{
    fs::File,
    io::{self, BufRead, BufReader, IsTerminal, Write},
    os::fd::AsRawFd,
    process::ChildStdin,
    time::{Duration, Instant},
};

pub struct Steps<'a> {
    stem: &'a str,
    input: BufReader<File>,
    pipe: Option<ChildStdin>,
    line: Vec<u8>,
    number: usize,
    written: usize,
    waiting_since: Option<Instant>,
    paused: Duration,
    color: bool,
    view: Option<LiveView<'a>>,
}

impl<'a> Steps<'a> {
    pub fn new(
        stem: &'a str,
        input: File,
        pipe: ChildStdin,
        expected: &'a [u8],
        inline: bool,
        color: bool,
    ) -> io::Result<Self> {
        nonblocking(pipe.as_raw_fd())?;
        Ok(Self {
            stem,
            input: BufReader::new(input),
            pipe: Some(pipe),
            line: Vec::new(),
            number: 0,
            written: 0,
            waiting_since: None,
            paused: Duration::ZERO,
            color,
            view: inline.then(|| LiveView::new(expected, color)),
        })
    }

    pub fn paused(&self) -> Duration {
        self.paused
            + self
                .waiting_since
                .map_or(Duration::ZERO, |since| since.elapsed())
    }

    pub fn advance(&mut self, stdout: &Capture) -> io::Result<bool> {
        let Some(pipe) = self.pipe.as_mut() else {
            return Ok(false);
        };
        if self.written == self.line.len() {
            self.line.clear();
            self.written = 0;
            if self.input.read_until(b'\n', &mut self.line)? == 0 {
                self.pipe = None;
                return Ok(false);
            }
            self.number += 1;
            let line = self.line.strip_suffix(b"\n").unwrap_or(&self.line);
            let label = format!("{}.in line {}", safe_text(self.stem), self.number);
            println!(
                "{}{}{}",
                style(self.color, &label, "1;36"),
                style(self.color, " | ", "2"),
                style(self.color, &visible_bytes(line), "1;34"),
            );
            println!(
                "Press {} to send this line.",
                style(self.color, "Enter", "1;35")
            );
            io::stdout().flush()?;
            self.waiting_since = Some(Instant::now());
        }
        if let Some(since) = self.waiting_since {
            if !enter_pressed()? {
                if let Some(view) = &mut self.view
                    && let Some(actual) = stdout.latest_line()
                {
                    view.render(actual)?;
                }
                return Ok(false);
            }
            if let Some(view) = &mut self.view {
                view.end_prompt()?;
            }
            self.paused += since.elapsed();
            self.waiting_since = None;
        }
        match pipe.write(&self.line[self.written..]) {
            Ok(count) => {
                self.written += count;
                return Ok(count != 0);
            }
            Err(err)
                if matches!(
                    err.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
                ) => {}
            Err(err) if err.kind() == io::ErrorKind::BrokenPipe => self.pipe = None,
            Err(err) => return Err(err),
        }
        Ok(false)
    }
}

pub fn inline_enabled() -> bool {
    io::stdin().is_terminal()
        && io::stdout().is_terminal()
        && std::env::var("TERM").map_or(true, |term| term != "dumb")
}

struct LiveView<'a> {
    remaining: &'a [u8],
    expected: Option<&'a [u8]>,
    previous: OutputLine,
    drawn: bool,
    color: bool,
}

impl<'a> LiveView<'a> {
    fn new(expected: &'a [u8], color: bool) -> Self {
        Self {
            remaining: expected,
            expected: None,
            previous: OutputLine::default(),
            drawn: false,
            color,
        }
    }

    fn render(&mut self, actual: OutputLine) -> io::Result<()> {
        if self.drawn && actual == self.previous {
            return Ok(());
        }
        for _ in self.previous.number..actual.number {
            let valid = !self.remaining.is_empty();
            let mut lines = self.remaining.splitn(2, |&byte| byte == b'\n');
            self.expected = lines.next().filter(|_| valid);
            self.remaining = lines.next().unwrap_or_default();
        }
        let mut out = io::stdout().lock();
        write!(out, "\r\x1b[2K")?;
        if actual.number == 0 {
            write!(out, "{}", style(self.color, "Waiting for stdout…", "33"))?;
        } else {
            let label = format!("stdout #{}  ", actual.number);
            let width = terminal_columns().saturating_sub(label.len() + 22) / 2;
            let expected = self.expected.map_or_else(
                || "(no line)".to_owned(),
                |line| preview_text(line, false, width),
            );
            let text = preview_text(&actual.bytes, actual.truncated, width);
            let differs = !actual.truncated && self.expected != Some(&actual.bytes);
            let expected_code = if differs { "33" } else { "36" };
            let actual_code = if actual.truncated {
                "33"
            } else if differs {
                "1;31"
            } else {
                "32"
            };
            let label = style(self.color, &label, "1;35");
            if width >= 10 {
                write!(
                    out,
                    "{label}{} {}{}{} {}",
                    style(self.color, "Expected", "1;36"),
                    style(self.color, &expected, expected_code),
                    style(self.color, " | ", "2"),
                    style(self.color, "Actual", "1;32"),
                    style(self.color, &text, actual_code),
                )?;
            } else {
                write!(out, "{label}")?;
            }
        }
        out.flush()?;
        self.previous = actual;
        self.drawn = true;
        Ok(())
    }

    fn end_prompt(&mut self) -> io::Result<()> {
        if self.drawn {
            self.drawn = false;
            let mut out = io::stdout().lock();
            writeln!(out)?;
            out.flush()?;
        }
        Ok(())
    }
}

impl Drop for LiveView<'_> {
    fn drop(&mut self) {
        let _ = self.end_prompt();
    }
}

fn preview_text(bytes: &[u8], truncated: bool, width: usize) -> String {
    let rendered = visible_bytes(&bytes[..bytes.len().min(96)]);
    let mut used = 0;
    let mut preview = String::new();
    for ch in rendered.chars() {
        // Budget two terminal cells for non-ASCII characters to avoid wrapping.
        used += if ch.is_ascii() { 1 } else { 2 };
        if used >= width {
            preview.push('…');
            return preview;
        }
        preview.push(ch);
    }
    if truncated || bytes.len() > 96 {
        preview.push('…');
    }
    preview
}

fn terminal_columns() -> usize {
    let mut size: libc::winsize = unsafe { std::mem::zeroed() };
    if unsafe { libc::ioctl(libc::STDOUT_FILENO, libc::TIOCGWINSZ, &mut size) } == 0
        && size.ws_col > 0
    {
        usize::from(size.ws_col)
    } else {
        80
    }
}

// Poll before reading so the runner can still check child exit and capture limits
// while waiting for the operator. Read one byte to leave later keystrokes queued.
fn enter_pressed() -> io::Result<bool> {
    let mut fd = libc::pollfd {
        fd: libc::STDIN_FILENO,
        events: libc::POLLIN,
        revents: 0,
    };
    match unsafe { libc::poll(&mut fd, 1, 0) } {
        0 => return Ok(false),
        -1 => return interrupted(),
        _ => {}
    }
    let mut byte = 0u8;
    match unsafe { libc::read(fd.fd, (&mut byte as *mut u8).cast(), 1) } {
        1 => Ok(byte == b'\n'),
        0 => Err(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            "stdin closed while waiting for Enter",
        )),
        _ => interrupted(),
    }
}

fn interrupted() -> io::Result<bool> {
    let err = io::Error::last_os_error();
    match err.kind() {
        io::ErrorKind::Interrupted | io::ErrorKind::WouldBlock => Ok(false),
        _ => Err(err),
    }
}
