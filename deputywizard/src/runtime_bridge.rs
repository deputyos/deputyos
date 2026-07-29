//! Authenticated HTTP-to-Unix-socket bridge for the resident `deputyd`.
//!
//! The wizard's existing Token/AccountOwner middleware is the network trust
//! boundary. The privileged agent remains on its root-owned Unix socket; this
//! module exposes only typed, allow-listed protocol commands and never accepts
//! a program name, shell fragment, or arbitrary argument vector.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::Result;
use deputyd::{AgentCommand, AgentRequest, AgentResponse};

pub trait RuntimeAgent: Send + Sync {
    fn execute(&self, command: AgentCommand) -> Result<AgentResponse>;
}

#[derive(Debug, Clone)]
pub struct SocketRuntimeAgent {
    socket: PathBuf,
}

impl SocketRuntimeAgent {
    pub fn new(socket: impl Into<PathBuf>) -> Self {
        Self {
            socket: socket.into(),
        }
    }

    pub fn socket(&self) -> &Path {
        &self.socket
    }
}

impl Default for SocketRuntimeAgent {
    fn default() -> Self {
        Self::new(deputyd::DEFAULT_SOCKET)
    }
}

impl RuntimeAgent for SocketRuntimeAgent {
    fn execute(&self, command: AgentCommand) -> Result<AgentResponse> {
        static NEXT_REQUEST: AtomicU64 = AtomicU64::new(1);
        let sequence = NEXT_REQUEST.fetch_add(1, Ordering::Relaxed);
        let request = AgentRequest::new(format!("wizard-{sequence}"), command).with_actor(
            deputyd::ActorContext {
                kind: "authenticated_session".to_string(),
                id: "wizard-control".to_string(),
                source: Some("system_ui".to_string()),
            },
        );
        deputyd::request(&self.socket, &request)
    }
}
