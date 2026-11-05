use std::{
    io::{self, Read},
    sync::{
        Arc,
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
}

impl Capture {
    pub fn start<R>(mut source: R, limit: usize) -> io::Result<Self>
    where
        R: Source + 'static,
    {
        source.nonblocking()?;

        let stopped = Arc::new(AtomicBool::new(false));
        let exceeded = Arc::new(AtomicBool::new(false));
        let stop = stopped.clone();
        let overflow = exceeded.clone();
        let reader = thread::Builder::new().spawn(move || {
            let mut bytes = Vec::new();
            let mut buffer = [0; 8192];
            loop {
                match source.read(&mut buffer) {
                    Ok(0) => return Ok(bytes),
                    Ok(count) => {
                        let retained = count.min(limit - bytes.len());
                        bytes.extend_from_slice(&buffer[..retained]);
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
        })
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
        let fd = self.as_raw_fd();
        let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
        if flags == -1 || unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } == -1
        {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }
}

#[cfg(not(unix))]
impl<T: Read + Send> Source for T {
    fn nonblocking(&self) -> io::Result<()> {
        Ok(())
    }
}
