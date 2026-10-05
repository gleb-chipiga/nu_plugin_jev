//! Carries invocation cancellation across asynchronous requests and retry waits.

use tokio::sync::watch;

/// Cancels one invocation and every clone of its signal through a remembered watch value.
/// Sender ownership is also a liveness guard: dropping all senders ends asynchronous waits.
#[derive(Clone)]
pub(crate) struct CancelHandle(watch::Sender<bool>);

/// Observes cancellation without polling or blocking a Tokio worker.
#[derive(Clone)]
pub(crate) struct CancelSignal(watch::Receiver<bool>);

impl CancelHandle {
    /// Creates a fresh cancellation pair owned by one invocation.
    pub(crate) fn new() -> (Self, CancelSignal) {
        let (sender, receiver) = watch::channel(false);
        (Self(sender), CancelSignal(receiver))
    }

    /// Notifies all subscribed requests and retry waits to stop.
    pub(crate) fn cancel(&self) {
        // A stored flag lets late subscribers observe cancellation without relying on one wakeup.
        // send_replace is synchronous, so the SDK signal handler need not enter an async runtime.
        self.0.send_replace(true);
    }
}

impl CancelSignal {
    /// Waits until cancellation is requested or all senders are dropped.
    pub(crate) async fn cancelled(&mut self) {
        if *self.0.borrow() {
            return;
        }
        while self.0.changed().await.is_ok() {
            if *self.0.borrow() {
                return;
            }
        }
    }

    /// Reports whether cancellation has already been requested.
    pub(crate) fn is_cancelled(&self) -> bool {
        *self.0.borrow()
    }
}

#[cfg(test)]
mod tests {
    use super::CancelHandle;

    /// Wakes an already waiting observer and remembers cancellation for later ones.
    #[test]
    fn cancellation_is_reusable() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            let (handle, mut signal) = CancelHandle::new();
            let mut other = signal.clone();
            let waiter = tokio::spawn(async move { other.cancelled().await });
            handle.cancel();
            waiter.await.unwrap();
            signal.cancelled().await;
            assert!(signal.is_cancelled());
        });
    }
}
