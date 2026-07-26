//! Cooperative cancellation for in-flight calls.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// A shared "stop waiting" flag.
///
/// [`crate::BridgeClient::call`] checks it on every poll, so a cancellation is
/// observed within one backoff interval rather than at the deadline. Cloning
/// shares the flag; the MCP server hands one clone to the request thread and
/// keeps another to trip on shutdown.
///
/// Cancelling does **not** abort work already running inside REAPER — the
/// bridge has no way to hear about it. It stops the client waiting, cleans up
/// an unclaimed command file, and returns
/// [`IPC_CANCELLED`][crate::error::codes::IPC_CANCELLED].
#[derive(Clone, Debug, Default)]
pub struct CancelFlag(Arc<AtomicBool>);

impl CancelFlag {
    /// A fresh, uncancelled flag.
    pub fn new() -> CancelFlag {
        CancelFlag(Arc::new(AtomicBool::new(false)))
    }

    /// Requests cancellation. Idempotent, and safe from any thread.
    pub fn cancel(&self) {
        self.0.store(true, Ordering::SeqCst);
    }

    /// True once [`CancelFlag::cancel`] has been called on this flag or any
    /// clone of it.
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }

    /// Clears the flag so it can be reused for another call.
    pub fn reset(&self) {
        self.0.store(false, Ordering::SeqCst);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_new_flag_is_not_cancelled() {
        assert!(!CancelFlag::new().is_cancelled());
        assert!(!CancelFlag::default().is_cancelled());
    }

    #[test]
    fn cancelling_is_visible_and_idempotent() {
        let f = CancelFlag::new();
        f.cancel();
        assert!(f.is_cancelled());
        f.cancel();
        assert!(f.is_cancelled());
    }

    #[test]
    fn clones_share_one_flag() {
        let a = CancelFlag::new();
        let b = a.clone();
        b.cancel();
        assert!(a.is_cancelled());
    }

    #[test]
    fn reset_clears_the_flag_for_reuse() {
        let f = CancelFlag::new();
        f.cancel();
        f.reset();
        assert!(!f.is_cancelled());
    }

    #[test]
    fn cancelling_from_another_thread_is_observed() {
        let f = CancelFlag::new();
        let g = f.clone();
        let h = std::thread::spawn(move || g.cancel());
        h.join().expect("join");
        assert!(f.is_cancelled());
    }
}
