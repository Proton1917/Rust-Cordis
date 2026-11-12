#![forbid(unsafe_code)]

pub mod manager;
mod process;

pub use cordis_up::rpc::{Connection, ConnectionOptions, Handler, Outgoing, Payload, RemoteObject};
pub use process::{Peer, PeerOptions};
