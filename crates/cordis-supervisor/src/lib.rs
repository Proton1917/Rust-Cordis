//! DOC-15 Q0、CP-1–CP-7：安全 Rust 核心中的远端生命周期与进程监管。
#![forbid(unsafe_code)]

pub mod manager;

pub use cordis_process::{Peer, PeerOptions, run_capture};
pub use cordis_up::rpc::{Connection, ConnectionOptions, Handler, Outgoing, Payload, RemoteObject};
