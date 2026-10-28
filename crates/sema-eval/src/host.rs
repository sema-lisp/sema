//! Shared thread and cancellation primitives for native interpreter hosts.
use sema_core::runtime::RootId;
use sema_vm::runtime::RuntimeCommandHandle;
use std::sync::{Arc, Mutex};

/// A token that cancels the host's current root without moving interpreter state.
#[derive(Clone)]
pub struct CancelToken {
    command: RuntimeCommandHandle,
    running_root: Arc<Mutex<Option<RootId>>>,
}
impl CancelToken {
    pub fn new(command: RuntimeCommandHandle, running_root: Arc<Mutex<Option<RootId>>>) -> Self {
        Self {
            command,
            running_root,
        }
    }
    pub fn cancel_running(&self) -> bool {
        let root = *self.running_root.lock().unwrap_or_else(|e| e.into_inner());
        root.is_some_and(|root| self.command.cancel_root(root))
    }
}

/// Run a host on a stack large enough for the compiler's bounded recursion.
pub fn spawn_host_thread<T: Send + 'static>(
    name: &str,
    run: impl FnOnce() -> T + Send + 'static,
) -> std::io::Result<std::thread::JoinHandle<T>> {
    std::thread::Builder::new()
        .name(name.into())
        .stack_size(16 * 1024 * 1024)
        .spawn(run)
}
