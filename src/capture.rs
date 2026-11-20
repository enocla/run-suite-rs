use std::{
    io::{self, Read},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread::{self, JoinHandle},
    time::Duration,
};

/// Drain one stream independently, retaining at most `limit` bytes in memory.
/// On Unix nonblocking reads let us stop even if an escaped descendant holds a
/// pipe open. No test stdout/stderr is ever spooled to a temporary file.
pub struct Capture {
    stopped: Arc<AtomicBool>,
    exceeded: Arc<AtomicBool>,
    reader: Option<JoinHandle<io::Result<Vec<u8>>>>,
    #[cfg(unix)]
    preview: Option<Arc<Mutex<OutputLine>>>,
}

#[derive(Clone, Default, PartialEq, Eq)]
pub struct OutputLine {
    pub number: usize,
    pub bytes: Vec<u8>,
    pub truncated: bool,
}

impl Capture {
    pub fn start<R>(source: R, limit: usize) -> io::Result<Self>
    where
        R: Source + 'static,
    {
        Self::start_with_preview(source, limit, false)
    }

    pub fn start_with_preview<R>(mut source: R, limit: usize, preview: bool) -> io::Result<Self>
    where
        R: Source + 'static,
    {
        source.nonblocking()?;

        let stopped = Arc::new(AtomicBool::new(false));
        let exceeded = Arc::new(AtomicBool::new(false));
        let stop = stopped.clone();
        let overflow = exceeded.clone();
        let preview = preview.then(|| Arc::new(Mutex::new(OutputLine::default())));
        let live = preview.clone();
        let reader = thread::Builder::new().spawn(move || {
            let mut bytes = Vec::new();
            let mut line_number = 1;
            let mut line_start = 0;
            let mut buffer = [0; 8192];
            loop {
                match source.read(&mut buffer) {
                    Ok(0) => return Ok(bytes),
                    Ok(count) => {
                        let retained = count.min(limit - bytes.len());
                        let chunk = &buffer[..retained];
                        let start = bytes.len();
                        bytes.extend_from_slice(chunk);
                        if let Some(preview) = &live
                            && !chunk.is_empty()
                        {
                            let mut last = (line_number, line_start, bytes.len());
                            for (index, _) in chunk.iter().enumerate().filter(|(_, b)| **b == b'\n')
                            {
                                let end = start + index;
                                last = (line_number, line_start, end);
                                line_number += 1;
                                line_start = end + 1;
                            }
                            if line_start < bytes.len() {
                                last = (line_number, line_start, bytes.len());
                            }
                            let (number, begin, end) = last;
                            let line = &bytes[begin..end];
                            let mut preview = preview.lock().unwrap();
                            preview.number = number;
                            preview.truncated = line.len() > 96;
                            preview.bytes.clear();
                            preview.bytes.extend_from_slice(&line[..line.len().min(96)]);
                        }
                        if retained < count {
                            overflow.store(true, Ordering::Relaxed);
                            return Ok(bytes);
                        }
                    }
                    Err(err) if err.kind() == io::ErrorKind::Interrupted => continue,
                    Err(err) if err.kind() == io::ErrorKind::WouldBlock => {
                        if stop.load(Ordering::Relaxed) {
                            return Ok(bytes);
                        }
                        thread::sleep(Duration::from_millis(1));
                    }
                    Err(err) => return Err(err),
                }
            }
        })?;
        Ok(Self {
            stopped,
            exceeded,
            reader: Some(reader),
            #[cfg(unix)]
            preview,
        })
    }

    #[cfg(unix)]
    pub fn latest_line(&self) -> Option<OutputLine> {
        self.preview
            .as_ref()
            .map(|line| line.lock().unwrap().clone())
    }

    pub fn exceeded(&self) -> bool {
        self.exceeded.load(Ordering::Relaxed)
    }

    pub fn finish(&mut self) -> io::Result<Vec<u8>> {
        self.stopped.store(true, Ordering::Relaxed);
        self.reader
            .take()
            .unwrap()
            .join()
            .map_err(|_| io::Error::other("output capture thread panicked"))?
    }
}

impl Drop for Capture {
    fn drop(&mut self) {
        self.stopped.store(true, Ordering::Relaxed);
    }
}

#[cfg(unix)]
pub fn log_pipe(path: &std::path::Path) -> io::Result<std::fs::File> {
    use std::{
        ffi::CString,
        os::unix::{ffi::OsStrExt, fs::OpenOptionsExt},
    };
    let name = CString::new(path.as_os_str().as_bytes())?;
    if unsafe { libc::mkfifo(name.as_ptr(), 0o600) } != 0 {
        return Err(io::Error::last_os_error());
    }
    // Holding both ends keeps startup nonblocking and prevents an early EOF
    // before Valgrind opens its log. finish() drains remaining bytes on stop.
    std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .custom_flags(libc::O_NONBLOCK)
        .open(path)
}

// Unix pipes must be nonblocking so a descendant that changes process groups
// cannot hold the runner open after a test finishes.
pub trait Source: Read + Send {
    fn nonblocking(&self) -> io::Result<()>;
}

#[cfg(unix)]
impl<T: Read + Send + std::os::fd::AsRawFd> Source for T {
    fn nonblocking(&self) -> io::Result<()> {
        nonblocking(self.as_raw_fd())
    }
}

#[cfg(unix)]
pub fn nonblocking(fd: std::os::fd::RawFd) -> io::Result<()> {
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags == -1 || unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } == -1 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(not(unix))]
impl<T: Read + Send> Source for T {
    fn nonblocking(&self) -> io::Result<()> {
        Ok(())
    }
}
