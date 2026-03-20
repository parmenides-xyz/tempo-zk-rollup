use std::{sync::Arc, time::Duration};

use ethereum_types::U64;
use smirk::Element;
use tracing::{error, info, instrument};

use crate::{
    network::NetworkEvent,
    utxo::{validate_txn, UtxoProof},
    Block, Error, NodeShared, Result,
};

impl NodeShared {
    pub async fn submit_transaction_and_wait(&self, utxo: UtxoProof) -> Result<Arc<Block>> {
        let mut started_waiting_at_eth_block = None;
        loop {
            match self.validate_transaction(&utxo).await {
                Ok(_) => break,
                Err(err @ Error::MintIsNotInTheContract { key: _ })
                | Err(err @ Error::BurnIsNotInTheContract { key: _ }) => {
                    let current_eth_block = self
                        .rollup_contract
                        .client
                        .client()
                        .eth()
                        .block_number()
                        .await
                        .map_err(Error::FailedToGetEthBlockNumber)?
                        .as_u64();
                    let started_waiting_at_eth_block =
                        *started_waiting_at_eth_block.get_or_insert(current_eth_block);

                    let waited_too_long_for_confirmation = current_eth_block
                        - started_waiting_at_eth_block
                        > self.config.safe_eth_height_offset;

                    // TODO: we could wait a little extra time and accept mints/burns
                    // that are not even valid at `latest` height yet,
                    // because they are still in eth mempool
                    if self.config.safe_eth_height_offset == 0 || waited_too_long_for_confirmation {
                        return Err(err);
                    }
                }
                Err(err) => return Err(err),
            }

            tokio::time::sleep(Duration::from_secs(6)).await;
        }

        self.send_all(NetworkEvent::Transaction(utxo.clone())).await;

        let changes = utxo.leaves();
        self.mempool.add_wait(utxo.hash(), utxo, changes).await
    }

    pub(super) async fn validate_transaction(&self, utxo: &UtxoProof) -> Result<()> {
        let is_mint_or_burn = utxo.mb_hash != Element::ZERO && utxo.mb_value != Element::ZERO;
        if is_mint_or_burn {
            let eth_block = self
                .rollup_contract
                .client
                .client()
                .eth()
                .block_number()
                .await
                .map_err(Error::FailedToGetEthBlockNumber)?;
            let safe_eth_height =
                match eth_block.overflowing_sub(U64::from(self.config.safe_eth_height_offset)) {
                    (safe_eth_height, false) => safe_eth_height,
                    // This can happen if we are running with a local hardhat node
                    (_, true) => U64::from(0),
                };
            let rollup_contract_at_safe_height = self
                .rollup_contract
                .clone()
                .at_height(Some(safe_eth_height.as_u64()));

            match (utxo.input_leaves, utxo.output_leaves) {
                // mint
                ([Element::ZERO, Element::ZERO], [key, Element::ZERO]) => {
                    if rollup_contract_at_safe_height
                        .get_mint(&key)
                        .await?
                        .is_none()
                    {
                        return Err(Error::MintIsNotInTheContract { key });
                    }
                }
                // burn
                ([key, Element::ZERO], [Element::ZERO, Element::ZERO]) => {
                    match rollup_contract_at_safe_height.has_burn(&key).await? {
                        false => return Err(Error::BurnIsNotInTheContract { key }),
                        true => {}
                    }
                }

                _ => {
                    return Err(Error::InvalidMintOrBurnLeaves);
                }
            }
        }

        validate_txn(
            self.config.mode,
            utxo,
            self.height(),
            &self.block_store,
            &self.notes_tree.read(),
        )
    }

    #[instrument(skip(self))]
    pub async fn receive_transaction(&self, txn: UtxoProof) -> Result<()> {
        info!("Received transaction");

        if let Err(err) = self.validate_transaction(&txn).await {
            error!(
                ?err,
                "Failed to validate transaction received from another node"
            );
            return Ok(());
        }

        let changes = txn.leaves();
        self.mempool.add(txn.hash(), txn, changes);

        Ok(())
    }
}
