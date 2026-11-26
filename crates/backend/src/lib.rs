//! Frontend-neutral client API over the rex-rune core, and shells on this computer.
//!
//! This is the only crate that names rex-rune types, and none appear in its public API. When a
//! new core revision changes something, the fix belongs here and frontends don't notice.
#![forbid(unsafe_code)]

mod answer;
mod boxes;
pub mod demo;
mod frame;
mod host;
mod input;
mod keys;
mod local;
mod model;
mod predict;
mod setup;
mod tailnet;

pub use answer::{AnswerKey, Asker, Outcome, Question, Reply, Ticket, pause};
pub use boxes::{BoxConfig, Boxes, LOCAL, TINTS};
pub use frame::{CellRef, CursorShape, Frame, Underline};
pub use host::{Button, Host, Key, KeyAction, Mouse, Pointer, Select, Split, Wake};
pub use keys::{
    KeyCheck, KeyState, add_to_agent_program, check_key, install_key_program, key_state, keys_only_program,
    make_key_program, own_key,
};
pub use predict::set_local_echo;
pub use setup::{HostKey, Problem, Started, install_program, scan_host_key, start_preview, start_server, trust_host_key};
pub use tailnet::{TailnetMachine, tailnet};
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
