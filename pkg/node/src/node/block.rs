use either::Either;
use prover::smirk_metadata::SmirkMetadata;
use smirk::{Batch, Element};
use tracing::instrument;

use crate::{
    block::{Block, BlockState},
    types::BlockHeight,
    Error, NodeShared, PersistentMerkleTree, Result,
};

impl NodeShared {
    #[instrument(skip_all)]
    pub(super) fn validate_block(&self, block: &Block) -> Result<()> {
        if self
            .config
            .bad_blocks
            .contains(&block.content.header.height)
        {
            return Ok(());
        }

        let validator = self.get_leader_for_block_height(block.content.header.height);

        let signed_by = block
            .signature
            .verify(&block.hash())
            .ok_or(Error::InvalidSignature)?;

        if signed_by != validator {
            return Err(Error::InvalidSignature);
        }

        block
            .content
            .validate(self.config.mode, &self.block_store, &self.notes_tree.read())?;

        Ok(())
    }

    #[instrument(skip_all)]
    pub(crate) fn apply_block_to_tree(
        notes_tree: &mut PersistentMerkleTree,
        state: &BlockState,
        current_height: BlockHeight,
        ignore_collisions: bool,
    ) -> Result<()> {
        let leaves = state
            .txns
            .iter()
            .flat_map(|txn| txn.leaves())
            .filter(|e| *e != Element::ZERO);

        for leaf in leaves.clone() {
            if !ignore_collisions && notes_tree.tree().contains_element(&leaf) {
                panic!("Double-spend detected. This should never happen, this should have been caught before commit");
            }
        }

        let metadata = SmirkMetadata::inserted_in(current_height.0);
        let leaves_with_height = leaves.map(|e| (e, metadata.clone()));
        let leaves_with_height_maybe_ignoring_collisions = match ignore_collisions {
            false => Either::Left(leaves_with_height),
            true => Either::Right(
                // If we're ignoring collisions, we need to filter out the leaves that are already in the tree,
                // otherwise the insert_batch would fail.
                leaves_with_height.filter(|(leaf, _)| !notes_tree.tree().contains_element(leaf)),
            ),
        };
        let batch = Batch::from_entries(leaves_with_height_maybe_ignoring_collisions)?;

        notes_tree.insert_batch(batch)?;
        Ok(())
    }
}
