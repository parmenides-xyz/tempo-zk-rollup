use crate::Mode;
use clap::Parser;
use libp2p::multiaddr::Multiaddr;
use primitives::peer::PeerIdSigner;
use rpc::tracing::{LogFormat, LogLevel};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Parser, Serialize, Deserialize)]
#[clap(name = "Tempo ZK Rollup")]
#[command(author, version, about = "Tempo ZK Rollup", long_about = None)]
#[command(propagate_version = true)]
pub struct CliArgs {
    #[clap(short, long, default_value = "tempo-rollup.toml")]
    pub config_path: PathBuf,

    #[clap(short, long, env = "TEMPO_SECRET_KEY_PATH")]
    pub secret_key_path: Option<PathBuf>,

    #[arg(long, env = "POLY_MODE")]
    pub mode: Option<Mode>,

    /// RPC listen address
    // TODO: we should take this from figment
    #[arg(long, env = "POLY_RPC_LADDR")]
    pub rpc_laddr: Option<String>,

    /// P2P listen address
    #[arg(long)]
    pub p2p_laddr: Option<Multiaddr>,

    /// Peers to dial
    #[arg(long, value_delimiter = ',')]
    pub p2p_dial: Option<Vec<Multiaddr>>,

    /// Peers to dial
    #[arg(long, env = "POLY_SECRET_KEY")]
    pub secret_key: Option<PeerIdSigner>,

    /// Log level
    #[arg(value_enum, long, env = "POLY_LOG_LEVEL", default_value = "INFO")]
    pub log_level: LogLevel,

    /// Log format
    #[arg(value_enum, long, env = "POLY_LOG_FORMAT", default_value = "PRETTY")]
    pub log_format: LogFormat,

    /// Data path
    #[arg(long, env = "POLY_DB_PATH")]
    pub db_path: Option<PathBuf>,

    /// Smirk path
    #[arg(long, env = "POLY_SMIRK_PATH")]
    pub smirk_path: Option<PathBuf>,

    /// Tempo L1 RPC URL
    #[arg(long, env = "TEMPO_RPC_URL")]
    pub eth_rpc_url: Option<String>,

    /// Rollup settlement contract address on Tempo
    #[arg(long, env = "TEMPO_ROLLUP_CONTRACT")]
    pub rollup_contract_addr: Option<String>,

    /// Sync chunk size
    #[arg(long, env = "POLY_SYNC_CHUNK_SIZE")]
    pub sync_chunk_size: Option<u64>,
}
