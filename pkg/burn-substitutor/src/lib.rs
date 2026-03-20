use std::time::Duration;

use contracts::RollupContract;
use eyre::{Context, ContextCompat};
use primitives::{
    block_height::BlockHeight,
    pagination::{CursorChoice, CursorChoiceAfter, OpaqueCursor, OpaqueCursorChoice},
};
use reqwest::StatusCode;
use zk_circuits::data::UTXOProof;
use zk_primitives::Element;

pub struct BurnSubstitutor {
    rollup_contract: RollupContract,
    node_rpc_url: String,
    eth_txn_confirm_wait_interval: Duration,
    cursor: Option<OpaqueCursorChoice<ListTxnsPosition>>,
}

impl BurnSubstitutor {
    pub fn new(
        rollup_contract: RollupContract,
        node_rpc_url: String,
        eth_txn_confirm_wait_interval: Duration,
    ) -> Self {
        BurnSubstitutor {
            rollup_contract,
            node_rpc_url,
            eth_txn_confirm_wait_interval,
            cursor: None,
        }
    }

    pub async fn tick(&mut self) -> Result<Vec<Element>, eyre::Error> {
        if self.cursor.is_none() {
            let last_rollup = self.fetch_last_rollup_block().await?;

            self.cursor = Some(
                CursorChoice::After(CursorChoiceAfter::After(ListTxnsPosition {
                    block: last_rollup,
                    txn: u64::MAX,
                }))
                .opaque(),
            );
        }

        let (txns, cursor) = Self::fetch_transactions(
            &reqwest::Client::new(),
            &self.node_rpc_url,
            None,
            self.cursor.as_ref(),
            false,
        )
        .await
        .context("Failed to fetch transactions")?;

        let mut substituted_burns = Vec::new();
        for txn in &txns {
            let inputs = txn
                .proof
                .input_leaves
                .iter()
                .filter(|e| !e.is_zero())
                .collect::<Vec<_>>();
            let has_only_one_input = inputs.len() == 1;
            let has_no_outputs = txn.proof.output_leaves.iter().all(|e| e.is_zero());
            let has_mb = txn.proof.mb_hash != Element::ZERO && txn.proof.mb_value != Element::ZERO;
            let is_burn = has_only_one_input && has_no_outputs && has_mb;

            if is_burn {
                let nullifier = inputs[0];

                if self.rollup_contract.was_burn_substituted(nullifier).await? {
                    continue;
                }

                let txn = self
                    .rollup_contract
                    .substitute_burn(nullifier, &txn.proof.mb_value)
                    .await
                    .context("Failed to substitute burn")?;

                self.rollup_contract
                    .client
                    .wait_for_confirm(txn, self.eth_txn_confirm_wait_interval)
                    .await
                    .context("Failed to wait for burn substitution")?;

                substituted_burns.push(*nullifier);
            }
        }

        if !txns.is_empty() {
            self.cursor = cursor
                .after
                .map(|after| CursorChoice::After(after.0).opaque());
        }

        Ok(substituted_burns)
    }

    async fn fetch_last_rollup_block(&mut self) -> Result<BlockHeight, contracts::Error> {
        self.rollup_contract.block_height().await.map(BlockHeight)
    }

    async fn fetch_transactions(
        client: &reqwest::Client,
        network_base_url: &str,
        limit: Option<usize>,
        cursor: Option<&OpaqueCursorChoice<ListTxnsPosition>>,
        poll: bool,
    ) -> Result<(Vec<Transaction>, OpaqueCursor<ListTxnsPosition>), eyre::Error> {
        let req = client
            .get(format!("{network_base_url}/v0/transactions"))
            .query(&[
                ("limit", limit.map(|l| l.to_string())),
                ("order", Some("OldestToNewest".to_owned())),
                ("cursor", cursor.map(|c| c.serialize()).transpose()?),
                ("poll", Some(poll.to_string())),
            ]);

        let resp = req.send().await?;

        match resp.status() {
            StatusCode::OK => {}
            e => return Err(eyre::eyre!("Unexpected status code: {e}")),
        }

        let mut resp = resp.json::<serde_json::Value>().await?;

        let txns = serde_json::from_value::<Vec<Transaction>>(
            resp.get_mut("txns").context("Missing txns field")?.take(),
        )?;

        let cursor = resp
            .get_mut("cursor")
            .context("Missing pagination field")?
            .take();

        let cursor = serde_json::from_value(cursor).context("Failed to parse cursor")?;

        Ok((txns, cursor))
    }
}

#[derive(Debug, serde::Deserialize)]
struct Transaction {
    pub proof: UTXOProof<0>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
struct ListTxnsPosition {
    block: BlockHeight,
    txn: u64,
}
