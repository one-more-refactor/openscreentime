//! Background loops that outlive their own bugs.
//!
//! The server runs a handful of forever-loops next to the HTTP stack (offline
//! sweep and retention, phone alerts, the Telegram poller, the command janitor,
//! system health). A bare `tokio::spawn` that panics dies silently and the
//! server keeps answering `/health` while, say, devices never flip offline
//! again. Every such loop goes through [`spawn`]: a panic is logged and the
//! task is started again a few seconds later.

use std::future::Future;
use std::time::Duration;

/// How long to wait before restarting a task that panicked, so a task that
/// panics on every run can't spin the CPU.
const RESTART_DELAY: Duration = Duration::from_secs(5);

/// Spawn `make()` and restart it whenever it panics. A task that returns
/// normally is done (one-shot tasks such as OIDC discovery end that way).
pub fn spawn<F, Fut>(name: &'static str, make: F)
where
    F: Fn() -> Fut + Send + Sync + 'static,
    Fut: Future<Output = ()> + Send + 'static,
{
    tokio::spawn(async move {
        loop {
            match tokio::spawn(make()).await {
                Ok(()) => {
                    tracing::debug!(task = name, "background task finished");
                    return;
                }
                Err(e) if e.is_panic() => {
                    let panic = e.into_panic();
                    let msg = panic
                        .downcast_ref::<&str>()
                        .map(|s| s.to_string())
                        .or_else(|| panic.downcast_ref::<String>().cloned())
                        .unwrap_or_else(|| "non-string panic".into());
                    tracing::error!(task = name, panic = %msg, "background task panicked; restarting");
                }
                // Cancelled: the runtime is shutting down.
                Err(_) => return,
            }
            tokio::time::sleep(RESTART_DELAY).await;
        }
    });
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::Arc;

    #[tokio::test(start_paused = true)]
    async fn a_panicking_task_is_restarted() {
        let runs = Arc::new(AtomicU32::new(0));
        let r = runs.clone();
        super::spawn("test", move || {
            let r = r.clone();
            async move {
                if r.fetch_add(1, Ordering::SeqCst) < 2 {
                    panic!("boom");
                }
            }
        });
        // Two panics, two restart delays, then the third run returns normally.
        for _ in 0..50 {
            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        }
        assert_eq!(runs.load(Ordering::SeqCst), 3);
    }
}
