use std::process::{Command, ExitStatus, Stdio};
use std::thread;
use std::time::{Duration, Instant};

#[derive(Debug)]
pub struct TimedRun {
    pub status: ExitStatus,
    pub stdout: String,
    pub stderr: String,
    pub timed_out: bool,
}

pub fn run_sema_with_timeout(source: &str, timeout: Duration) -> TimedRun {
    let mut child = Command::new(env!("CARGO_BIN_EXE_sema"))
        .args(["--no-llm", "-e", source])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn sema watchdog child");

    let started = Instant::now();
    let mut timed_out = false;
    loop {
        if child
            .try_wait()
            .expect("poll sema watchdog child")
            .is_some()
        {
            break;
        }
        if started.elapsed() >= timeout {
            timed_out = true;
            child.kill().expect("kill hung sema watchdog child");
            break;
        }
        thread::sleep(Duration::from_millis(10));
    }

    let output = child
        .wait_with_output()
        .expect("collect sema watchdog output");
    TimedRun {
        status: output.status,
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        timed_out,
    }
}
