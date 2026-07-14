use std::io::{self, Read};
use std::process::{Command, ExitStatus, Stdio};
use std::thread;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

#[cfg(unix)]
use std::os::unix::process::CommandExt;

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
}

impl BoundedDrain {
    fn finish(self) -> Vec<u8> {
        self.handle
            .join()
            .expect("join watchdog diagnostic drain")
            .expect("drain watchdog diagnostic pipe")
    }
}

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

    // A direct child can exit after spawning a descendant that inherited its
    // stdout/stderr pipes. Unix process-group cleanup closes those writers so
    // both bounded drain threads can be joined instead of leaked.
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

#[cfg(unix)]
pub fn run_command_with_timeout(program: &str, args: &[&str], timeout: Duration) -> TimedRun {
    run_with_timeout(Command::new(program).args(args), timeout)
}
