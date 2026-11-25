//! Frontend-neutral client API over the rex-rune core.
//!
//! This is the only crate that names rex-rune types, and none appear in its public API. When a
//! new core revision changes something, the fix belongs here and frontends don't notice.
#![forbid(unsafe_code)]

mod boxes;
pub mod demo;
mod frame;
mod host;
mod input;
mod model;
mod predict;

pub use boxes::{BoxConfig, Boxes, TINTS};
pub use frame::{CellRef, CursorShape, Frame, Underline};
pub use host::{Host, Key, KeyAction, Split, Wake};
pub use predict::set_local_echo;
pub use model::{Agent, AgentState, PaneView, Session, Size, Snapshot, Tab, WaitingFor, plain};

/// Call once before anything else.
pub fn init() {
    rex_rune::platform::init();
}

/// The local server's socket.
pub fn local_endpoint() -> String {
    rex_rune::transport::local_endpoint()
}

/// Wait for host workers to close their connections, so servers drop this client at once.
pub fn shutdown(timeout: std::time::Duration) -> bool {
    rex_rune::connection::wait_for_hosts(timeout) & input::finished(timeout)
}

/// The core's newest supported server API revision.
pub const API_REVISION: u32 = rex_rune::transport::API_REVISION;
