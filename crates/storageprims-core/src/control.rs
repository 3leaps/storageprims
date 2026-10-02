//! Cooperative cancellation and one absolute operation deadline.
//!
//! Cancellation does not establish rollback, and dropping a future cannot return
//! an outcome. Providers must preserve any commit evidence already observed.

use std::future::Future;
use std::pin::Pin;
use std::time::Instant;

use tokio::sync::watch;

/// A typed, sendable future without erasing operation-specific failure evidence.
pub type FutureResult<'a, T, E> = Pin<Box<dyn Future<Output = Result<T, E>> + Send + 'a>>;

/// A shared monotonic cancellation latch. Clones observe the same cancellation.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct Cancellation {
    sender: watch::Sender<bool>,
}

impl Cancellation {
    pub fn new() -> Self {
        let (sender, _) = watch::channel(false);
        Self { sender }
    }

    /// Request cancellation; repeated requests never clear the latch.
    pub fn cancel(&self) {
        self.sender.send_replace(true);
    }

    pub fn is_cancelled(&self) -> bool {
        *self.sender.borrow()
    }

    /// Wait for cancellation without losing a request racing registration.
    pub fn cancelled(&self) -> Pin<Box<dyn Future<Output = ()> + Send + '_>> {
        Box::pin(async move {
            let mut receiver = self.sender.subscribe();
            loop {
                if *receiver.borrow_and_update() {
                    return;
                }
                // This future borrows the sender, so the channel cannot close.
                if receiver.changed().await.is_err() {
                    return;
                }
            }
        })
    }
}

impl Default for Cancellation {
    fn default() -> Self {
        Self::new()
    }
}

/// Control shared across all steps of an operation, never a per-step timer.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct OperationControl {
    deadline: Instant,
    cancellation: Cancellation,
}

impl OperationControl {
    pub fn new(deadline: Instant, cancellation: Cancellation) -> Self {
        Self {
            deadline,
            cancellation,
        }
    }

    pub fn deadline(&self) -> Instant {
        self.deadline
    }

    pub fn cancellation(&self) -> &Cancellation {
        &self.cancellation
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[tokio::test]
    async fn cancellation_before_registration_is_observed() {
        let latch = Cancellation::new();
        assert!(!latch.is_cancelled());
        let other = latch.clone();
        other.cancel();
        other.cancel();
        assert!(latch.is_cancelled());
        tokio::time::timeout(Duration::from_secs(1), latch.cancelled())
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn concurrent_waiters_and_registration_races_do_not_lose_cancellation() {
        for _ in 0..64 {
            let latch = Cancellation::new();
            let other = latch.clone();
            let cancel = tokio::spawn(async move {
                tokio::task::yield_now().await;
                other.cancel();
            });
            tokio::time::timeout(Duration::from_secs(1), async {
                tokio::join!(latch.cancelled(), latch.cancelled());
            })
            .await
            .unwrap();
            cancel.await.unwrap();
            assert!(latch.is_cancelled());
        }
    }

    #[test]
    fn parallel_registration_and_cancellation_do_not_lose_notifications() {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .unwrap();
        for _ in 0..64 {
            let latch = Cancellation::new();
            let other = latch.clone();
            let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
            let cancel_barrier = barrier.clone();
            let cancel = std::thread::spawn(move || {
                cancel_barrier.wait();
                other.cancel();
            });
            runtime.block_on(async {
                barrier.wait();
                tokio::time::timeout(Duration::from_secs(1), async {
                    tokio::join!(latch.cancelled(), latch.cancelled());
                })
                .await
                .unwrap();
            });
            cancel.join().unwrap();
            assert!(latch.is_cancelled());
        }
    }

    #[test]
    fn control_clone_preserves_absolute_deadline_and_shared_latch() {
        let deadline = Instant::now();
        let control = OperationControl::new(deadline, Cancellation::default());
        let cloned = control.clone();
        cloned.cancellation().cancel();
        assert_eq!(control.deadline(), deadline);
        assert_eq!(cloned.deadline(), deadline);
        assert!(control.cancellation().is_cancelled());
    }
}
