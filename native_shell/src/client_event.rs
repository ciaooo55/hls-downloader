//! Lightweight event envelope for IPC-only clients.
//!
//! The resident Core owns `CoreRuntime`. Native presenters only need the
//! serialized event envelope carried by the v7 IPC protocol, so keep that DTO
//! available without compiling the full runtime implementation.

use crate::contract::CoreEvent;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EventEnvelope {
    pub sequence: u64,
    pub event: CoreEvent,
}
