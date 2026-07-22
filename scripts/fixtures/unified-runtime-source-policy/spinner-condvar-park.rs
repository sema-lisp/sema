// Fixture: the sanctioned B6 spinner shape — the render loop parks on its condvar
// (`wait_timeout_while`), so `stop` wakes it immediately and teardown can join it.
// No `thread::sleep` frame loop, so this PASSES the spinner-park policy.
fn spawn_spinner_condvar(stop: std::sync::Arc<(std::sync::Mutex<bool>, std::sync::Condvar)>) {
    std::thread::spawn(move || {
        let mut frame_idx = 0usize;
        loop {
            let (lock, cvar) = &*stop;
            let stopped = lock.lock().unwrap();
            if *stopped {
                break;
            }
            let _ = frame_idx;
            frame_idx += 1;
            let (stopped, _timed_out) = cvar
                .wait_timeout_while(stopped, std::time::Duration::from_millis(80), |s| !*s)
                .unwrap();
            if *stopped {
                break;
            }
        }
    });
}
