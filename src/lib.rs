pub mod config;
pub mod graphics;
pub mod input;
pub mod layout;
pub mod port_owner;
pub mod remote;
pub mod serial;
pub mod session;
pub mod sync;
pub mod terminal;
pub mod ztransfer;

/// Locks a mutex without panicking when another thread panicked while holding
/// it. A session's worker panicking on odd input must not take the UI, and
/// every other session with it, down on the next frame; the data behind these
/// locks (screens, counters, status) stays usable after such a panic.
pub trait LockExt<T> {
    fn locked(&self) -> std::sync::MutexGuard<'_, T>;
}

impl<T> LockExt<T> for std::sync::Mutex<T> {
    fn locked(&self) -> std::sync::MutexGuard<'_, T> {
        self.lock().unwrap_or_else(|e| e.into_inner())
    }
}
