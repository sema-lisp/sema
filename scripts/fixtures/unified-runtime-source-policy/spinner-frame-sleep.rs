// Fixture: the LEGACY spinner shape — a bare `thread::sleep` frame loop. Its
// sleeping render thread can only be stopped after a full interval and never on
// teardown, so this must FAIL the spinner-park policy (SPINNER_FRAME_SLEEP).
fn spawn_spinner_legacy(stop: std::sync::Arc<std::sync::atomic::AtomicBool>) {
    std::thread::spawn(move || {
        let mut frame_idx = 0usize;
        loop {
            if stop.load(std::sync::atomic::Ordering::Relaxed) {
                break;
            }
            let _ = frame_idx;
            frame_idx += 1;
            std::thread::sleep(std::time::Duration::from_millis(80));
        }
    });
}
