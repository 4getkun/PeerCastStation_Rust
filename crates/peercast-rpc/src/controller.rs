use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use thiserror::Error;
use tokio::sync::Mutex;

use crate::client::{
    ChannelViewModel, ClientRpcError, PeerCastRpcClient, RpcTransport, StatusViewModel,
    VersionInfoViewModel,
};
use crate::protocol::rpc_code;
use crate::server::{PluginSummary, ServentSettings};

/// Controller error conditions (REQ-084, Global Rule 2).
#[derive(Error, Debug, Clone, PartialEq, Eq)]
pub enum ControllerError {
    #[error("Mutation is already in flight (re-entrancy guard triggered)")]
    DuplicatePendingMutation,

    #[error("Read request is already in progress")]
    ReadAlreadyInProgress,

    #[error(
        "Received stale response for superseded sequence (expected >= {expected}, got {received})"
    )]
    StaleResponseIgnored { expected: u64, received: u64 },

    #[error("RPC client error: {0}")]
    Client(#[from] ClientRpcError),

    #[error("Uncertain mutation response: automatic retry forbidden (REQ-084)")]
    UncertainMutationRetryForbidden,
}

/// Explicit reactive screen state machine (Global Rule 2: No 3+ boolean flags).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScreenState<T> {
    /// Screen is uninitialized / idle.
    Initial,
    /// Asynchronous data read in progress for given sequence token.
    Loading { seq: u64 },
    /// Screen data successfully loaded and available for presentation.
    Loaded(T),
    /// Data query succeeded but contains no items (e.g. empty channel list).
    Empty,
    /// General query or network error occurred.
    Error { message: String, code: Option<i32> },
    /// Client lacks permission to view or execute this screen.
    Forbidden { reason: String },
    /// Daemon or network disconnected.
    Disconnected,
}

impl<T> ScreenState<T> {
    pub fn is_loading(&self) -> bool {
        matches!(self, ScreenState::Loading { .. })
    }

    pub fn is_loaded(&self) -> bool {
        matches!(self, ScreenState::Loaded(_))
    }

    pub fn data(&self) -> Option<&T> {
        match self {
            ScreenState::Loaded(d) => Some(d),
            _ => None,
        }
    }
}

/// Explicit mutation lifecycle state machine (Global Rule 2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MutationState {
    /// No mutation in flight.
    Idle,
    /// Mutation currently executing on network.
    Pending { action: &'static str, seq: u64 },
    /// Mutation successfully applied and committed.
    Succeeded,
    /// Mutation failed or was rejected by backend.
    Failed,
}

impl MutationState {
    pub fn is_pending(&self) -> bool {
        matches!(self, MutationState::Pending { .. })
    }
}

/// Aggregated presentation data for the Index screen (REQ-081).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexScreenData {
    pub version: VersionInfoViewModel,
    pub status: StatusViewModel,
    pub plugins: Vec<PluginSummary>,
}

/// Controller managing the Index screen (`index.html`) lifecycle and state (REQ-081, REQ-084).
pub struct IndexController<T: RpcTransport> {
    client: PeerCastRpcClient<T>,
    state: Arc<Mutex<ScreenState<IndexScreenData>>>,
    seq_counter: AtomicU64,
}

impl<T: RpcTransport> IndexController<T> {
    pub fn new(client: PeerCastRpcClient<T>) -> Self {
        Self {
            client,
            state: Arc::new(Mutex::new(ScreenState::Initial)),
            seq_counter: AtomicU64::new(1),
        }
    }

    pub async fn state(&self) -> ScreenState<IndexScreenData> {
        self.state.lock().await.clone()
    }

    /// Refreshes the Index screen data with stale read protection (REQ-084, Global Rule 2).
    pub async fn refresh(&self) -> Result<(), ControllerError> {
        let seq = self.seq_counter.fetch_add(1, Ordering::SeqCst);

        // Transition to Loading state
        {
            let mut state = self.state.lock().await;
            *state = ScreenState::Loading { seq };
        }

        // Fetch queries concurrently
        let ver_res = self.client.get_version_info().await;
        let status_res = self.client.get_status().await;
        let plugins_res = self.client.get_plugins().await;

        let mut state = self.state.lock().await;

        // Stale read check: verify our seq is still the newest
        let current_seq = self.seq_counter.load(Ordering::SeqCst) - 1;
        if seq != current_seq {
            return Err(ControllerError::StaleResponseIgnored {
                expected: current_seq,
                received: seq,
            });
        }

        // Process query results
        match (ver_res, status_res, plugins_res) {
            (Ok(version), Ok(status), Ok(plugins)) => {
                *state = ScreenState::Loaded(IndexScreenData {
                    version,
                    status,
                    plugins,
                });
                Ok(())
            }
            (Err(ClientRpcError::Unauthorized), _, _)
            | (_, Err(ClientRpcError::Unauthorized), _)
            | (_, _, Err(ClientRpcError::Unauthorized)) => {
                *state = ScreenState::Forbidden {
                    reason: "Admin authorization required to view full index status".to_string(),
                };
                Ok(())
            }
            (Err(ClientRpcError::Transport(msg)), _, _)
            | (_, Err(ClientRpcError::Transport(msg)), _)
            | (_, _, Err(ClientRpcError::Transport(msg))) => {
                *state = ScreenState::Disconnected;
                Err(ControllerError::Client(ClientRpcError::Transport(msg)))
            }
            (Err(e), _, _) | (_, Err(e), _) | (_, _, Err(e)) => {
                *state = ScreenState::Error {
                    message: e.to_string(),
                    code: None,
                };
                Err(ControllerError::Client(e))
            }
        }
    }
}

/// Controller managing the Settings screen (`settings.html`) with mutation re-entrancy guards (REQ-081, REQ-084).
pub struct SettingsController<T: RpcTransport> {
    client: PeerCastRpcClient<T>,
    view_state: Arc<Mutex<ScreenState<ServentSettings>>>,
    mutation_state: Arc<Mutex<MutationState>>,
    seq_counter: AtomicU64,
}

impl<T: RpcTransport> SettingsController<T> {
    pub fn new(client: PeerCastRpcClient<T>) -> Self {
        Self {
            client,
            view_state: Arc::new(Mutex::new(ScreenState::Initial)),
            mutation_state: Arc::new(Mutex::new(MutationState::Idle)),
            seq_counter: AtomicU64::new(1),
        }
    }

    pub async fn view_state(&self) -> ScreenState<ServentSettings> {
        self.view_state.lock().await.clone()
    }

    pub async fn mutation_state(&self) -> MutationState {
        *self.mutation_state.lock().await
    }

    /// Loads settings from backend into `view_state` with stale read protection (REQ-084).
    pub async fn load_settings(&self) -> Result<(), ControllerError> {
        let seq = self.seq_counter.fetch_add(1, Ordering::SeqCst);
        {
            let mut vs = self.view_state.lock().await;
            *vs = ScreenState::Loading { seq };
        }

        let res = self.client.get_settings().await;

        let mut vs = self.view_state.lock().await;
        let current_seq = self.seq_counter.load(Ordering::SeqCst) - 1;
        if seq != current_seq {
            return Err(ControllerError::StaleResponseIgnored {
                expected: current_seq,
                received: seq,
            });
        }

        match res {
            Ok(settings) => {
                *vs = ScreenState::Loaded(settings);
                Ok(())
            }
            Err(ClientRpcError::Unauthorized) => {
                *vs = ScreenState::Forbidden {
                    reason: "Settings require administrative access".to_string(),
                };
                Ok(())
            }
            Err(ClientRpcError::Transport(msg)) => {
                *vs = ScreenState::Disconnected;
                Err(ControllerError::Client(ClientRpcError::Transport(msg)))
            }
            Err(e) => {
                *vs = ScreenState::Error {
                    message: e.to_string(),
                    code: None,
                };
                Err(ControllerError::Client(e))
            }
        }
    }

    /// Submits settings updates with strict re-entrancy and blind-retry prevention (REQ-084, Global Rule 2).
    pub async fn update_settings(
        &self,
        new_settings: ServentSettings,
    ) -> Result<(), ControllerError> {
        let seq = self.seq_counter.fetch_add(1, Ordering::SeqCst);

        // Re-entrancy guard: early-return if mutation is already in flight
        {
            let mut ms = self.mutation_state.lock().await;
            if ms.is_pending() {
                return Err(ControllerError::DuplicatePendingMutation);
            }
            *ms = MutationState::Pending {
                action: "setSettings",
                seq,
            };
        }

        let res = self.client.set_settings(&new_settings).await;

        let mut ms = self.mutation_state.lock().await;
        match res {
            Ok(true) => {
                *ms = MutationState::Succeeded;
                // Update local loaded view state immediately
                let mut vs = self.view_state.lock().await;
                *vs = ScreenState::Loaded(new_settings);
                Ok(())
            }
            Ok(false) => {
                *ms = MutationState::Failed;
                Err(ControllerError::Client(ClientRpcError::Rpc {
                    code: rpc_code::INVALID_PARAMS,
                    message: "Server rejected settings update".to_string(),
                    data: None,
                }))
            }
            Err(ClientRpcError::Transport(_msg)) => {
                *ms = MutationState::Failed;
                // Stop/block: never blindly retry non-idempotent mutations after transport failure (REQ-084)
                Err(ControllerError::UncertainMutationRetryForbidden)
            }
            Err(e) => {
                *ms = MutationState::Failed;
                Err(ControllerError::Client(e))
            }
        }
    }
}

/// Controller managing the Channels screen (`channels.html`) with empty state detection (REQ-081).
pub struct ChannelsController<T: RpcTransport> {
    client: PeerCastRpcClient<T>,
    state: Arc<Mutex<ScreenState<Vec<ChannelViewModel>>>>,
    seq_counter: AtomicU64,
}

impl<T: RpcTransport> ChannelsController<T> {
    pub fn new(client: PeerCastRpcClient<T>) -> Self {
        Self {
            client,
            state: Arc::new(Mutex::new(ScreenState::Initial)),
            seq_counter: AtomicU64::new(1),
        }
    }

    pub async fn state(&self) -> ScreenState<Vec<ChannelViewModel>> {
        self.state.lock().await.clone()
    }

    /// Loads active channels and cleanly differentiates Loaded versus Empty states with stale read protection (REQ-081, REQ-084).
    pub async fn load_channels(&self) -> Result<(), ControllerError> {
        let seq = self.seq_counter.fetch_add(1, Ordering::SeqCst);
        {
            let mut s = self.state.lock().await;
            *s = ScreenState::Loading { seq };
        }

        let res = self.client.get_channels().await;

        let mut s = self.state.lock().await;
        let current_seq = self.seq_counter.load(Ordering::SeqCst) - 1;
        if seq != current_seq {
            return Err(ControllerError::StaleResponseIgnored {
                expected: current_seq,
                received: seq,
            });
        }

        match res {
            Ok(channels) => {
                if channels.is_empty() {
                    *s = ScreenState::Empty;
                } else {
                    *s = ScreenState::Loaded(channels);
                }
                Ok(())
            }
            Err(ClientRpcError::Unauthorized) => {
                *s = ScreenState::Forbidden {
                    reason: "Channels view forbidden".to_string(),
                };
                Ok(())
            }
            Err(ClientRpcError::Transport(msg)) => {
                *s = ScreenState::Disconnected;
                Err(ControllerError::Client(ClientRpcError::Transport(msg)))
            }
            Err(e) => {
                *s = ScreenState::Error {
                    message: e.to_string(),
                    code: None,
                };
                Err(ControllerError::Client(e))
            }
        }
    }
}
