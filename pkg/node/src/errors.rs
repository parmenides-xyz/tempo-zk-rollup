use std::num::ParseIntError;

use libp2p::PeerId;
use primitives::{block_height::BlockHeight, hash::CryptoHash};
use tracing::error;
use zk_primitives::Element;

use crate::sync;

pub type Result<T, E = Error> = std::result::Result<T, E>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("invalid snapshot chunk, peer mismatch - accepted {accepted}, got {got}")]
    SnapshotChunkPeerMismatch {
        accepted: Box<PeerId>,
        got: Box<PeerId>,
    },

    #[error("invalid proof")]
    InvalidProof,

    #[error("note already spent: 0x{spent_note:x}")]
    NoteAlreadySpent {
        spent_note: Element,
        failing_txn_hash: CryptoHash,
    },

    #[error(
        "leaf 0x{inserted_leaf} was already inserted in the same block in transaction 0x{txn_hash}"
    )]
    LeafAlreadyInsertedInTheSameBlock {
        inserted_leaf: Element,
        txn_hash: CryptoHash,
        failing_txn_hash: CryptoHash,
    },

    #[error("output note already exists: 0x{output_note:x}")]
    OutputNoteExists { output_note: Element },

    #[error("invalid element, size exceeds modulus")]
    InvalidElementSize { element: Element },

    #[error(
        "UTXO root is not recent enough: 0x{utxo_recent_root:x}, expected one of: {recent_roots:?}"
    )]
    UtxoRootIsNotRecentEnough {
        utxo_recent_root: Element,
        recent_roots: Vec<Element>,
        txn_hash: CryptoHash,
    },

    #[error("element is not in the tree")]
    ElementNotInTree { element: Element },

    #[error("element is not in any transaction of block {block_height}")]
    ElementNotInTxn {
        element: Element,
        block_height: BlockHeight,
    },

    #[error("block height {block} not found")]
    BlockNotFound { block: BlockHeight },

    #[error("block hash {block} not found")]
    BlockHashNotFound { block: CryptoHash },

    #[error("mint leaf is not in the contract")]
    MintIsNotInTheContract { key: Element },

    #[error("burn leaf is not in the contract")]
    BurnIsNotInTheContract { key: Element },

    #[error("burn 'to' address cannot be zero")]
    BurnToAddressCannotBeZero,

    #[error("invalid mint or burn leaves")]
    InvalidMintOrBurnLeaves,

    #[error("invalid mint or burn leaves")]
    InvalidSignature,

    #[error("invalid transaction '{txn}'")]
    InvalidTransaction { txn: CryptoHash },

    #[error("invalid block root, got: {got}, expected: {expected}")]
    InvalidBlockRoot { got: Element, expected: Element },

    #[error("failed to find transaction {txn}")]
    TxnNotFound { txn: CryptoHash },

    #[error("invalid element: {element}")]
    FailedToParseElement {
        element: String,
        #[source]
        source: ParseIntError,
    },

    #[error("invalid hash: {hash}")]
    FailedToParseHash {
        hash: String,
        #[source]
        source: rustc_hex::FromHexError,
    },

    #[error("failed to get eth block number")]
    FailedToGetEthBlockNumber(#[source] web3::Error),

    #[error("Invalid accept")]
    DoomslugError(#[from] doomslug::Error),

    #[error("sync error: {0}")]
    Sync(#[from] sync::Error),

    #[error("network error: {0}")]
    Network(#[from] p2p2::Error),

    #[error("block store error: {0}")]
    BlockStore(#[from] block_store::Error),

    #[error("smirk error: {0}")]
    Smirk(#[from] smirk::storage::Error),

    #[error("contracts error: {0}")]
    Contracts(#[from] contracts::Error),

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("smirk collision error: {0}")]
    Collision(#[from] smirk::CollisionError),
}
