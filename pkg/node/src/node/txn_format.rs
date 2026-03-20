use borsh::{BorshDeserialize, BorshSerialize};
use primitives::block_height::BlockHeight;
use serde::{Deserialize, Serialize};
use wire_message::WireMessage;

use crate::utxo::UtxoProof;

#[derive(Debug, Clone, BorshSerialize, BorshDeserialize, Serialize, Deserialize)]
pub struct TxnMetadata {
    pub block_height: BlockHeight,
    pub block_time: Option<u64>,
    pub block_hash: [u8; 32],
    pub block_txn_index: u32,
}

#[derive(Debug, Clone)]
#[wire_message::wire_message]
pub enum TxnFormat {
    V1(UtxoProof, TxnMetadata),
    // TODO next version:
    // - cache the hash of the transaction in the metadata
}

impl WireMessage for TxnFormat {
    type Ctx = ();
    type Err = core::convert::Infallible;

    fn version(&self) -> u64 {
        match self {
            Self::V1(_, _) => 1,
        }
    }

    fn upgrade_once(self, _ctx: &mut Self::Ctx) -> Result<Self, wire_message::Error> {
        match self {
            Self::V1(_, _) => Err(Self::max_version_error()),
        }
    }
}

impl block_store::Transaction for TxnFormat {
    fn txn_hash(&self) -> [u8; 32] {
        match self {
            Self::V1(txn, _) => txn.hash().into_inner(),
        }
    }
}
