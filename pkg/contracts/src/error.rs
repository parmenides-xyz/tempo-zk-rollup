use ethereum_types::H256;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("unknown transaction: {0}")]
    UnknownTransaction(H256),

    #[error("web3 error")]
    Web3(#[from] web3::Error),

    #[error("web3 contract error")]
    Web3Contract(#[from] web3::contract::Error),

    #[error("web3 ethabi error")]
    Web3Ethabi(#[from] web3::ethabi::Error),

    #[error("serde_json error")]
    SerdeJson(#[from] serde_json::Error),

    #[error("from hex error")]
    FromHex(#[from] rustc_hex::FromHexError),

    #[error("tokio task join error")]
    TokioJoin(#[from] tokio::task::JoinError),

    #[error("{0}")]
    Other(String),
}

pub type Result<T, E = Error> = std::result::Result<T, E>;
