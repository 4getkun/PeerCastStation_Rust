use std::future::Future;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{broadcast, Mutex};
use tokio::time::Instant;

/// Explicit application lifecycle states (Global Rule 2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LifecyclePhase {
    Initializing,
    Running,
    ShuttingDown,
    Stopped,
}

/// Reason triggering application shutdown.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShutdownReason {
    /// Received OS interrupt or termination signal (SIGINT, SIGTERM, Windows Ctrl+C/Break/Close).
    OsSignal(String),
    /// Administrative RPC stop request.
    AdminRpc,
    /// Inter-process kill request from second instance (`--kill`).
    IpcKill,
    /// Internal fatal error.
    InternalError(String),
}

/// Guard object for an active asynchronous task.
///
/// Automatically decrements the active task counter when dropped (REQ-014).
pub struct TaskGuard {
    counter: Arc<AtomicUsize>,
}

impl Drop for TaskGuard {
    fn drop(&mut self) {
        self.counter.fetch_sub(1, Ordering::SeqCst);
    }
}

/// Tracks spawned asynchronous tasks to guarantee join policies and clean drain on shutdown (REQ-014).
#[derive(Debug, Clone)]
pub struct TaskTracker {
    active_count: Arc<AtomicUsize>,
}

impl Default for TaskTracker {
    fn default() -> Self {
        Self::new()
    }
}

impl TaskTracker {
    pub fn new() -> Self {
        Self {
            active_count: Arc::new(AtomicUsize::new(0)),
        }
    }

    /// Number of currently running tasks tracked.
    pub fn active_tasks(&self) -> usize {
        self.active_count.load(Ordering::SeqCst)
    }

    /// Creates an RAII guard incrementing the active task counter.
    pub fn track(&self) -> TaskGuard {
        self.active_count.fetch_add(1, Ordering::SeqCst);
        TaskGuard {
            counter: Arc::clone(&self.active_count),
        }
    }

    /// Spawns a Tokio task with automatic active tracking and RAII lifecycle management (REQ-014).
    pub fn spawn<F, T>(&self, future: F) -> tokio::task::JoinHandle<T>
    where
        F: Future<Output = T> + Send + 'static,
        T: Send + 'static,
    {
        let guard = self.track();
        tokio::spawn(async move {
            let _guard = guard;
            future.await
        })
    }

    /// Waits monotonically until all active tasks have terminated or the timeout expires.
    ///
    /// Stop/block guard: Tasks must NOT survive shutdown (REQ-014, REQ-067).
    pub async fn wait_for_drain(&self, timeout: Duration) -> Result<(), usize> {
        let deadline = Instant::now() + timeout;
        while self.active_tasks() > 0 {
            if Instant::now() >= deadline {
                return Err(self.active_tasks());
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        Ok(())
    }
}

/// Central application lifecycle coordinator with re-entrancy protection (REQ-014, Global Rule 2).
pub struct AppLifecycleManager {
    phase: Arc<Mutex<LifecyclePhase>>,
    shutdown_tx: broadcast::Sender<ShutdownReason>,
    task_tracker: TaskTracker,
}

impl Default for AppLifecycleManager {
    fn default() -> Self {
        Self::new()
    }
}

impl AppLifecycleManager {
    pub fn new() -> Self {
        let (shutdown_tx, _) = broadcast::channel(16);
        Self {
            phase: Arc::new(Mutex::new(LifecyclePhase::Initializing)),
            shutdown_tx,
            task_tracker: TaskTracker::new(),
        }
    }

    /// Current lifecycle phase.
    pub async fn phase(&self) -> LifecyclePhase {
        *self.phase.lock().await
    }

    /// Task tracker for spawning monitored tasks.
    pub fn task_tracker(&self) -> &TaskTracker {
        &self.task_tracker
    }

    /// Subscribes to the shutdown broadcast channel.
    pub fn subscribe_shutdown(&self) -> broadcast::Receiver<ShutdownReason> {
        self.shutdown_tx.subscribe()
    }

    /// Marks the application as initialized and running.
    pub async fn mark_running(&self) -> bool {
        let mut p = self.phase.lock().await;
        if *p == LifecyclePhase::Initializing {
            *p = LifecyclePhase::Running;
            true
        } else {
            false
        }
    }

    /// Requests application shutdown with strict early-return re-entrancy guard (Global Rule 2).
    ///
    /// Returns `true` if this invocation initiated the shutdown transition; `false` if
    /// shutdown was already in progress or completed.
    pub async fn request_shutdown(&self, reason: ShutdownReason) -> bool {
        let mut p = self.phase.lock().await;
        // Early return re-entrancy guard
        if *p == LifecyclePhase::ShuttingDown || *p == LifecyclePhase::Stopped {
            return false;
        }

        *p = LifecyclePhase::ShuttingDown;
        let _ = self.shutdown_tx.send(reason);
        true
    }

    /// Performs graceful drain of active tasks and finalizes transition to `Stopped`.
    pub async fn complete_shutdown(&self, timeout: Duration) -> Result<(), usize> {
        let drain_res = self.task_tracker.wait_for_drain(timeout).await;
        let mut p = self.phase.lock().await;
        *p = LifecyclePhase::Stopped;
        drain_res
    }
}
