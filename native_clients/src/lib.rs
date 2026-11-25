//! Native Messaging, command-line and updater entry points.
#![cfg_attr(not(test), warn(clippy::undocumented_unsafe_blocks))]

pub use hls_client::*;
// Core's unit harness compiles the Host regressions with its private fixtures.
#[cfg(not(test))]
mod native_host;
pub use hls_client::update_install::{run_update_helper, run_update_helper_with_verifier};
#[cfg(not(test))]
pub use native_host::run as run_native_host;
