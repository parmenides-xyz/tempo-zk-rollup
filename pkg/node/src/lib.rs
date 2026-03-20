#![deny(clippy::disallowed_methods)]
#![feature(once_cell)] // this feature is fine beause it's since been stabilized
#![feature(bound_map)] // this feature is fine beause it's since been stabilized

mod block;
mod cache;
pub mod config;
mod constants;
mod errors;
mod mempool;
mod network;
mod network_handler;
mod node;
pub mod prover;
mod rpc;
mod sync;
mod types;
mod util;
mod utxo;

pub use crate::block::Block;
pub use crate::errors::*;
pub use crate::node::*;
pub use crate::rpc::routes::{configure_routes, State};
pub use crate::rpc::server::create_rpc_server;
pub use crate::rpc::stats::TxnStats;
pub use crate::utxo::UtxoProof;
