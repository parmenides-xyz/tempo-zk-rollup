use block_store::BlockListOrder;
use block_store::BlockStore;

use block_store::StoreList;
use zk_circuits::{constants::MERKLE_TREE_DEPTH, data::SnarkWitness, CircuitKind};
use zk_primitives::Element;

use crate::Mode;
use crate::{
    constants::RECENT_ROOT_COUNT, types::BlockHeight, BlockFormat, Error, PersistentMerkleTree,
    Result,
};

pub type UtxoProof = zk_circuits::data::UTXOProof<MERKLE_TREE_DEPTH>;

/// Validate a UTXO txn, we check the following:
/// - The proof is valid
/// - The recent root is recent enough
/// - The input notes are not already spent (not in tree)
/// - The output notes do not already exist (not in tree)
pub fn validate_txn(
    mode: Mode,
    utxo: &UtxoProof,
    height: BlockHeight,
    block_store: &BlockStore<BlockFormat>,
    notes_tree: &PersistentMerkleTree,
) -> Result<()> {
    let SnarkWitness::V1(witness) = utxo.to_snark_witness();

    if !witness.verify(CircuitKind::Utxo) {
        return Err(Error::InvalidProof);
    }

    // No need to check recent roots if recent_root is zero
    // TODO: are we defo this is secure?
    if utxo.recent_root != Element::ZERO {
        let next_block = height.next();
        let range = BlockHeight(next_block.saturating_sub(RECENT_ROOT_COUNT))..next_block;

        // TODO: this should be finding the last 64 DIFFERENT hashes, not just the last 64 blocks or we should increase
        // the number of recent roots
        let recent_roots = block_store
            .list(range, BlockListOrder::LowestToHighest)
            .into_iterator()
            .map(|r| {
                let block = r?.1.into_block();
                Ok::<_, Error>(block)
            })
            .map(|b| Ok::<_, Error>(b?.content.state.root_hash))
            .collect::<Result<Vec<_>>>()?;

        if !recent_roots.iter().any(|r| *r == utxo.recent_root) && !mode.is_prover() {
            return Err(Error::UtxoRootIsNotRecentEnough {
                utxo_recent_root: utxo.recent_root,
                recent_roots,
                txn_hash: utxo.hash(),
            });
        }
    }

    // Check if any of the txn inserts are already in the tree
    let tree = notes_tree.tree();

    for leaf in utxo.input_leaves {
        if leaf >= Element::MODULUS {
            return Err(Error::InvalidElementSize { element: leaf });
        }

        if leaf != Element::ZERO && tree.contains_element(&leaf) {
            return Err(Error::NoteAlreadySpent {
                spent_note: leaf,
                failing_txn_hash: utxo.hash(),
            });
        }
    }

    for leaf in utxo.output_leaves {
        if leaf >= Element::MODULUS {
            return Err(Error::InvalidElementSize { element: leaf });
        }

        if leaf != Element::ZERO && tree.contains_element(&leaf) {
            return Err(Error::OutputNoteExists { output_note: leaf });
        }
    }

    Ok(())
}
