//! Wire contracts shared by Core and all IPC-only clients.
#![cfg_attr(not(test), warn(clippy::undocumented_unsafe_blocks))]

pub mod contract;
pub mod crawl_contract;
pub mod ipc;
pub use contract::*;
pub use crawl_contract::*;
pub use ipc::*;
