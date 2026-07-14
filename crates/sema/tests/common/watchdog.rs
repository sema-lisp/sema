use std::io::{self, Read};
use std::process::{Command, ExitStatus, Stdio};
use std::thread;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

#[cfg(unix)]
use std::os::fd::AsRawFd;
#[cfg(unix)]
use std::os::unix::process::CommandExt;
#[cfg(any(unix, windows))]
use std::sync::atomic::{AtomicBool, Ordering};
#[cfg(any(unix, windows))]
use std::sync::Arc;

const DIAGNOSTIC_CAPTURE_LIMIT: usize = 64 * 1024;

#[derive(Debug)]
pub struct TimedRun {
    pub status: ExitStatus,
    pub stdout: String,
    pub stderr: String,
    pub timed_out: bool,
}

struct BoundedDrain {
    handle: JoinHandle<io::Result<Vec<u8>>>,
    #[cfg(any(unix, windows))]
    stop: Arc<AtomicBool>,
}

impl BoundedDrain {
    fn finish(self) -> Vec<u8> {
        #[cfg(any(unix, windows))]
        self.stop.store(true, Ordering::Release);
        self.handle
            .join()
            .expect("join watchdog diagnostic drain")
            .expect("drain watchdog diagnostic pipe")
    }
}

#[cfg(unix)]
fn drain_bounded<R>(reader: R) -> BoundedDrain
where
    R: AsRawFd + Read + Send + 'static,
{
    set_nonblocking(&reader).expect("configure watchdog diagnostic pipe as nonblocking");
    spawn_cancellable_drain(reader, |error| error.kind() == io::ErrorKind::WouldBlock)
}

#[cfg(windows)]
fn drain_bounded<R>(reader: R) -> BoundedDrain
where
    R: std::os::windows::io::AsRawHandle + Read + Send + 'static,
{
    set_nonblocking(&reader).expect("configure watchdog diagnostic pipe as nonblocking");
    spawn_cancellable_drain(reader, |error| {
        error.kind() == io::ErrorKind::WouldBlock
            || error.raw_os_error() == Some(windows_sys::Win32::Foundation::ERROR_NO_DATA as i32)
    })
}

#[cfg(any(unix, windows))]
fn spawn_cancellable_drain<R>(mut reader: R, is_would_block: fn(&io::Error) -> bool) -> BoundedDrain
where
    R: Read + Send + 'static,
{
    let stop = Arc::new(AtomicBool::new(false));
    let drain_stop = Arc::clone(&stop);
    let handle = thread::spawn(move || {
        let mut captured = Vec::with_capacity(DIAGNOSTIC_CAPTURE_LIMIT);
        let mut buffer = [0_u8; 8192];
        let mut final_drain_remaining = DIAGNOSTIC_CAPTURE_LIMIT;
        loop {
            let stopping = drain_stop.load(Ordering::Acquire);
            match reader.read(&mut buffer) {
                Ok(0) => break,
                Ok(read) => {
                    let remaining = DIAGNOSTIC_CAPTURE_LIMIT.saturating_sub(captured.len());
                    captured.extend_from_slice(&buffer[..read.min(remaining)]);
                    if stopping {
                        final_drain_remaining = final_drain_remaining.saturating_sub(read);
                        if final_drain_remaining == 0 {
                            break;
                        }
                    }
                }
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(error) if is_would_block(&error) => {
                    if stopping {
                        break;
                    }
                    thread::sleep(Duration::from_millis(2));
                }
                Err(error) => return Err(error),
            }
        }
        Ok(captured)
    });
    BoundedDrain { handle, stop }
}

#[cfg(unix)]
fn set_nonblocking(reader: &impl AsRawFd) -> io::Result<()> {
    let fd = reader.as_raw_fd();
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags == -1 {
        return Err(io::Error::last_os_error());
    }
    if unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } == -1 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(windows)]
fn set_nonblocking(reader: &impl std::os::windows::io::AsRawHandle) -> io::Result<()> {
    use windows_sys::Win32::System::Pipes::{SetNamedPipeHandleState, PIPE_NOWAIT};

    let mut mode = PIPE_NOWAIT;
    let result = unsafe {
        SetNamedPipeHandleState(
            reader.as_raw_handle(),
            &mut mode,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        )
    };
    if result == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(not(any(unix, windows)))]
fn drain_bounded<R>(mut reader: R) -> BoundedDrain
where
    R: Read + Send + 'static,
{
    let handle = thread::spawn(move || {
        let mut captured = Vec::with_capacity(DIAGNOSTIC_CAPTURE_LIMIT);
        let mut buffer = [0_u8; 8192];
        loop {
            let read = reader.read(&mut buffer)?;
            if read == 0 {
                break;
            }
            let remaining = DIAGNOSTIC_CAPTURE_LIMIT.saturating_sub(captured.len());
            captured.extend_from_slice(&buffer[..read.min(remaining)]);
        }
        Ok(captured)
    });
    BoundedDrain { handle }
}

fn bounded_diagnostic(bytes: Vec<u8>) -> String {
    let mut diagnostic = String::from_utf8_lossy(&bytes).into_owned();
    if diagnostic.len() > DIAGNOSTIC_CAPTURE_LIMIT {
        let mut end = DIAGNOSTIC_CAPTURE_LIMIT;
        while !diagnostic.is_char_boundary(end) {
            end -= 1;
        }
        diagnostic.truncate(end);
    }
    diagnostic
}

#[cfg(unix)]
fn configure_process_group(command: &mut Command) {
    command.process_group(0);
}

#[cfg(not(unix))]
fn configure_process_group(_command: &mut Command) {}

#[cfg(unix)]
fn terminate_process_group(process_group: u32) {
    let result = unsafe { libc::kill(-(process_group as i32), libc::SIGKILL) };
    if result == -1 {
        let error = io::Error::last_os_error();
        if error.raw_os_error() != Some(libc::ESRCH) {
            panic!("terminate watchdog process group {process_group}: {error}");
        }
    }
}

fn run_with_timeout(command: &mut Command, timeout: Duration) -> TimedRun {
    configure_process_group(command);
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn sema watchdog child");

    let stdout = child.stdout.take().expect("take sema watchdog stdout");
    let stderr = child.stderr.take().expect("take sema watchdog stderr");
    let stdout_drain = drain_bounded(stdout);
    let stderr_drain = drain_bounded(stderr);

    let started = Instant::now();
    let mut timed_out = false;
    let status = loop {
        if let Some(status) = child.try_wait().expect("poll sema watchdog child") {
            break status;
        }
        if started.elapsed() >= timeout {
            timed_out = true;
            #[cfg(unix)]
            {
                terminate_process_group(child.id());
                break child.wait().expect("wait for killed watchdog child");
            }
            #[cfg(not(unix))]
            if let Err(kill_error) = child.kill() {
                if let Some(status) = child
                    .try_wait()
                    .expect("poll sema watchdog child after failed kill")
                {
                    break status;
                }
                panic!("kill hung sema watchdog child: {kill_error}");
            }
            #[cfg(not(unix))]
            break child.wait().expect("wait for killed sema watchdog child");
        }
        thread::sleep(Duration::from_millis(10));
    };

    // Best-effort Unix process-group cleanup terminates descendants that did not
    // escape into another session. It cannot close writers held by a `setsid`
    // descendant; cancellable nonblocking drains guarantee bounded joins even
    // when such an escaped writer remains open.
    #[cfg(unix)]
    terminate_process_group(child.id());

    let stdout = stdout_drain.finish();
    let stderr = stderr_drain.finish();
    TimedRun {
        status,
        stdout: bounded_diagnostic(stdout),
        stderr: bounded_diagnostic(stderr),
        timed_out,
    }
}

pub fn run_sema_with_timeout(source: &str, timeout: Duration) -> TimedRun {
    run_with_timeout(
        Command::new(env!("CARGO_BIN_EXE_sema")).args(["--no-llm", "-e", source]),
        timeout,
    )
}

#[cfg(any(unix, windows))]
pub fn run_command_with_timeout(program: &str, args: &[&str], timeout: Duration) -> TimedRun {
    run_with_timeout(Command::new(program).args(args), timeout)
}
