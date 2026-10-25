use std::fmt;
use thiserror::Error;
use tokio::sync::{mpsc, watch};

/// A monotonic generation token preventing stale events across restarts (REQ-014).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct GenerationToken(pub u64);

impl GenerationToken {
    pub const INITIAL: GenerationToken = GenerationToken(1);

    pub fn next(&self) -> Self {
        GenerationToken(self.0.saturating_add(1))
    }
}

impl fmt::Display for GenerationToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Gen({})", self.0)
    }
}

/// Exhaustive lifecycle phases for an actor worker (Global Rule 2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum WorkerPhase {
    #[default]
    Idle,
    Starting,
    Running,
    Stopping,
    Stopped,
    Failed,
}

impl WorkerPhase {
    /// Returns true if the worker is in an active phase (`Starting` or `Running`).
    #[inline]
    pub fn is_active(&self) -> bool {
        matches!(self, WorkerPhase::Starting | WorkerPhase::Running)
    }

    /// Returns true if the worker has reached a terminal phase (`Stopped` or `Failed`).
    #[inline]
    pub fn is_terminal(&self) -> bool {
        matches!(self, WorkerPhase::Stopped | WorkerPhase::Failed)
    }
}

/// Actor errors for command dispatch and lifecycle transitions.
#[derive(Error, Debug, PartialEq, Eq, Clone)]
pub enum ActorError {
    #[error("Actor command channel is closed (worker has terminated)")]
    ChannelClosed,

    #[error("Actor command queue is full (bounded buffer capacity exceeded)")]
    QueueFull,

    #[error("Stale generation token: expected {expected}, got {actual}")]
    StaleGeneration {
        expected: GenerationToken,
        actual: GenerationToken,
    },

    #[error("Actor has already terminated")]
    AlreadyTerminated,
}

/// Immutable status snapshot published by the running worker to all handles (REQ-011).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkerStatus<S> {
    pub phase: WorkerPhase,
    pub generation: GenerationToken,
    pub custom: S,
}

impl<S: Default> WorkerStatus<S> {
    pub fn new_initial(generation: GenerationToken) -> Self {
        Self {
            phase: WorkerPhase::Idle,
            generation,
            custom: S::default(),
        }
    }
}

/// Internal command envelope carrying generation token for validation.
#[derive(Debug)]
pub enum ActorEnvelope<C> {
    Custom { cmd: C, generation: GenerationToken },
    Stop { generation: GenerationToken },
}

impl<C> ActorEnvelope<C> {
    pub fn generation(&self) -> GenerationToken {
        match self {
            ActorEnvelope::Custom { generation, .. } => *generation,
            ActorEnvelope::Stop { generation } => *generation,
        }
    }
}

/// A cloneable, thread-safe command handle to a running worker (REQ-011, REQ-012).
///
/// Posting commands does NOT lock the worker or require mutable access across awaits.
pub struct WorkerHandle<C, S> {
    sender: mpsc::Sender<ActorEnvelope<C>>,
    status_rx: watch::Receiver<WorkerStatus<S>>,
    generation: GenerationToken,
}

impl<C, S: Clone> Clone for WorkerHandle<C, S> {
    fn clone(&self) -> Self {
        Self {
            sender: self.sender.clone(),
            status_rx: self.status_rx.clone(),
            generation: self.generation,
        }
    }
}

impl<C, S: Clone> WorkerHandle<C, S> {
    pub fn new(
        sender: mpsc::Sender<ActorEnvelope<C>>,
        status_rx: watch::Receiver<WorkerStatus<S>>,
        generation: GenerationToken,
    ) -> Self {
        Self {
            sender,
            status_rx,
            generation,
        }
    }

    /// Returns the target generation token of this handle.
    #[inline]
    pub fn generation(&self) -> GenerationToken {
        self.generation
    }

    /// Returns an immutable snapshot of current worker status (REQ-011).
    pub fn status(&self) -> WorkerStatus<S> {
        self.status_rx.borrow().clone()
    }

    /// Returns current worker lifecycle phase.
    pub fn phase(&self) -> WorkerPhase {
        self.status_rx.borrow().phase
    }

    /// Posts a command to the worker asynchronously under bounded capacity.
    pub async fn post(&self, cmd: C) -> Result<(), ActorError> {
        let envelope = ActorEnvelope::Custom {
            cmd,
            generation: self.generation,
        };
        self.sender
            .send(envelope)
            .await
            .map_err(|_| ActorError::ChannelClosed)
    }

    /// Tries to post a command immediately without waiting (non-blocking try-send, REQ-013).
    pub fn try_post(&self, cmd: C) -> Result<(), ActorError> {
        let envelope = ActorEnvelope::Custom {
            cmd,
            generation: self.generation,
        };
        self.sender.try_send(envelope).map_err(|err| match err {
            mpsc::error::TrySendError::Full(_) => ActorError::QueueFull,
            mpsc::error::TrySendError::Closed(_) => ActorError::ChannelClosed,
        })
    }

    /// Idempotent stop request. If the worker is already stopping or stopped, returns Ok immediately.
    pub async fn stop(&self) -> Result<(), ActorError> {
        let current_phase = self.phase();
        if current_phase.is_terminal() || current_phase == WorkerPhase::Stopping {
            return Ok(());
        }

        let envelope = ActorEnvelope::Stop {
            generation: self.generation,
        };
        match self.sender.send(envelope).await {
            Ok(()) => Ok(()),
            Err(_) => {
                // Channel closed means worker is already terminated
                Ok(())
            }
        }
    }

    /// Non-blocking stop trigger.
    pub fn stop_nowait(&self) -> Result<(), ActorError> {
        let current_phase = self.phase();
        if current_phase.is_terminal() || current_phase == WorkerPhase::Stopping {
            return Ok(());
        }

        let envelope = ActorEnvelope::Stop {
            generation: self.generation,
        };
        match self.sender.try_send(envelope) {
            Ok(()) => Ok(()),
            Err(mpsc::error::TrySendError::Closed(_)) => Ok(()),
            Err(mpsc::error::TrySendError::Full(_)) => Err(ActorError::QueueFull),
        }
    }
}

/// The exclusive worker execution context (REQ-012).
///
/// Consumes commands from the inbox channel and publishes status updates.
/// A worker runs under single exclusive ownership and does NOT hold locks across network awaits.
pub struct WorkerContext<C, S> {
    inbox: mpsc::Receiver<ActorEnvelope<C>>,
    status_tx: watch::Sender<WorkerStatus<S>>,
    generation: GenerationToken,
}

impl<C, S: Clone> WorkerContext<C, S> {
    pub fn new(
        inbox: mpsc::Receiver<ActorEnvelope<C>>,
        status_tx: watch::Sender<WorkerStatus<S>>,
        generation: GenerationToken,
    ) -> Self {
        Self {
            inbox,
            status_tx,
            generation,
        }
    }

    /// Returns current generation token.
    #[inline]
    pub fn generation(&self) -> GenerationToken {
        self.generation
    }

    /// Updates worker lifecycle phase and status snapshot.
    pub fn set_phase(&self, phase: WorkerPhase) {
        self.status_tx.send_modify(|s| s.phase = phase);
    }

    /// Updates the custom status payload.
    pub fn update_custom<F: FnOnce(&mut S)>(&self, f: F) {
        self.status_tx.send_modify(|s| f(&mut s.custom));
    }

    /// Receives the next command, automatically filtering out stale generation events (REQ-014).
    ///
    /// Returns:
    /// - `Some(ActorEnvelope::Custom)`: valid command for current generation.
    /// - `Some(ActorEnvelope::Stop)`: stop requested for current generation.
    /// - `None`: channel closed (all handles dropped).
    pub async fn recv(&mut self) -> Option<ActorEnvelope<C>> {
        loop {
            let envelope = self.inbox.recv().await?;
            if envelope.generation() != self.generation {
                // REQ-014: Stale generation events are ignored
                continue;
            }
            return Some(envelope);
        }
    }
}

/// Creates a new paired `(WorkerHandle, WorkerContext)` with a bounded channel capacity.
pub fn create_actor_pair<C, S: Clone + Default>(
    capacity: usize,
    generation: GenerationToken,
) -> (WorkerHandle<C, S>, WorkerContext<C, S>) {
    let (cmd_tx, cmd_rx) = mpsc::channel(capacity);
    let initial_status = WorkerStatus::new_initial(generation);
    let (status_tx, status_rx) = watch::channel(initial_status);

    let handle = WorkerHandle::new(cmd_tx, status_rx, generation);
    let context = WorkerContext::new(cmd_rx, status_tx, generation);

    (handle, context)
}
