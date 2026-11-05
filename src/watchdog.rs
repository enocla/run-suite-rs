//! A separate process owns each test's process group. Closing its stdin kills
//! the group, even when the runner dies without running destructors.
use std::{
    env, fs,
    io::{self, Read},
    os::unix::process::CommandExt,
    path::Path,
    process::{Child, Command, Stdio},
};

const FLAG: &str = "--__run-suite-watchdog";

pub fn enter_if_requested() {
    let mut args = env::args_os().skip(1);
    if args.next().as_deref() != Some(std::ffi::OsStr::new(FLAG)) {
        return;
    }
    let directory = args.next().expect("watchdog requires a capture directory");
    let mut buffer = [0; 64];
    loop {
        match io::stdin().read(&mut buffer) {
            Ok(0) => break,
            Ok(_) => {}
            Err(err) if err.kind() == io::ErrorKind::Interrupted => continue,
            Err(_) => break,
        }
    }
    // Unlink capture resources before killing our own group. Open files/pipes
    // are released when the group dies, including after a runner SIGKILL.
    let _ = fs::remove_dir_all(directory);
    unsafe {
        libc::kill(0, libc::SIGKILL);
    }
    std::process::exit(1);
}

pub struct Watchdog(Child);

impl Watchdog {
    pub fn start(directory: &Path) -> io::Result<Self> {
        let child = Command::new(env::current_exe()?)
            .arg(FLAG)
            .arg(directory)
            .process_group(0)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?;
        Ok(Self(child))
    }

    pub fn process_group(&self) -> i32 {
        self.0.id() as i32
    }
}

impl Drop for Watchdog {
    fn drop(&mut self) {
        // ChildStdin is close-on-exec: test children and other watchdogs cannot
        // keep this lifeline open. The group exists before any test can join it;
        // if the runner dies during spawn, joining a dead group fails safely.
        drop(self.0.stdin.take());
        let _ = self.0.wait();
    }
}
