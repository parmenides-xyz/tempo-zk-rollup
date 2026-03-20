#![warn(clippy::unwrap_used, clippy::expect_used)]
#![deny(clippy::disallowed_methods)]

mod constants;
pub mod smirk_metadata;

use crate::constants::{MERKLE_TREE_DEPTH, MERKLE_TREE_PATH_DEPTH, UTXO_AGGREGATIONS};
use borsh::{BorshDeserialize, BorshSerialize};
pub use constants::MAXIMUM_TXNS;
use constants::{UTXO_AGG_LEAVES, UTXO_AGG_NUMBER};
use contracts::RollupContract;
use ethereum_types::H256;
use primitives::sig::Signature;
use smirk::{
    hash_cache::{NoopHashCache, SimpleHashCache},
    Element, Tree,
};
use smirk_metadata::SmirkMetadata;
use std::sync::Arc;
use tracing::info;
use web3::{ethabi, types::TransactionId};
use zk_circuits::{
    aggregate_utxo::AggregateUtxo,
    chips::aggregation::snark::Snark,
    data::{AggregateAgg, Batch, Insert, MerklePath, Note, ParameterSet, SnarkWitness, Utxo},
    evm_verifier, Base, CircuitKind,
};

type Result<T, E = Error> = std::result::Result<T, E>;

type MerkleTree<C = NoopHashCache> = Tree<MERKLE_TREE_DEPTH, SmirkMetadata, C>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("failed to convert H256 to bn256::Fr")]
    ConvertH256ToBn256Fr(H256),

    // Temporary workaround, sometimes rollup transactions get stuck. Restarting the prover fixes it
    #[error("rollup transaction timed out")]
    RollupTransactionTimeout,

    #[error("from hex error")]
    FromHex(#[from] rustc_hex::FromHexError),

    #[error("web3 error")]
    Web3(#[from] web3::Error),

    #[error("ethabi error")]
    EthAbi(#[from] ethabi::Error),

    #[error("zk error: {0}")]
    Zk(#[from] zk_circuits::Error),

    #[error("TryFromSlice error")]
    TryFromSlice(#[from] std::array::TryFromSliceError),

    #[error("web3 contract error")]
    Web3Contract(#[from] web3::contract::Error),

    #[error("serde_json error")]
    SerdeJson(#[from] serde_json::Error),

    #[error("secp256k1 error")]
    Secp256k1(#[from] secp256k1::Error),

    #[error("smirk storage error")]
    Smirk(#[from] smirk::storage::Error),

    #[error("smirk collision error")]
    SmirkCollision(#[from] smirk::CollisionError),

    #[error("contract error")]
    Contract(#[from] contracts::Error),

    #[error("tokio task join error")]
    TokioTaskJoin(#[from] tokio::task::JoinError),
}

#[derive(Debug, Clone)]
pub struct Transaction {
    pub proof: SnarkWitness,
}

impl Transaction {
    pub fn new(proof: SnarkWitness) -> Self {
        Self { proof }
    }
}
#[derive(Debug, Clone, Default, BorshSerialize, BorshDeserialize)]
pub struct Proof {
    pub proof: Vec<u8>,
    pub agg_instances: Vec<Element>,
    pub old_root: Element,
    pub new_root: Element,
    pub utxo_hashes: Vec<Element>,
}

impl Proof {
    pub fn new_root(&self) -> &Element {
        &self.new_root
    }
}

#[derive(Debug, Clone, Default, BorshSerialize, BorshDeserialize)]
pub struct RollupInput {
    proof: Proof,
    height: u64,
    other_hash: [u8; 32],
    signatures: Vec<Signature>,
}

impl RollupInput {
    pub fn new(
        proof: Proof,
        height: u64,
        other_hash: [u8; 32],
        signatures: Vec<Signature>,
    ) -> Self {
        Self {
            proof,
            height,
            other_hash,
            signatures,
        }
    }

    pub fn old_root(&self) -> &Element {
        &self.proof.old_root
    }

    pub fn height(&self) -> u64 {
        self.height
    }
}

pub struct Prover {
    contract: RollupContract,
}

impl Prover {
    pub fn new(contract: RollupContract) -> Self {
        Self { contract }
    }

    #[tracing::instrument(err, skip_all, fields(height, txns_len = txns.len()))]
    pub async fn prove(
        self: &Arc<Self>,
        notes_tree: &MerkleTree<SimpleHashCache>,
        _ban_tree: &MerkleTree,
        height: u64,
        txns: [Option<Transaction>; MAXIMUM_TXNS],
    ) -> Result<Proof> {
        info!(
            "Bundling {} UTXO proof(s) and proving new root hash",
            txns.len()
        );

        let (agg, proof) = tokio::task::spawn_blocking({
            let s = Arc::clone(self);
            let mut tree = notes_tree.clone();

            move || s.generate_aggregate_proof(&mut tree, txns, height)
        })
        .await??;

        Ok(Proof {
            proof,
            agg_instances: agg
                .agg_instances()
                .iter()
                .copied()
                .map(Element::from)
                .collect(),
            old_root: Element::from(*agg.old_root()),
            new_root: Element::from(*agg.new_root()),
            utxo_hashes: agg
                .utxo_values()
                .iter()
                .copied()
                .map(Element::from)
                .collect(),
        })
    }

    #[tracing::instrument(err, skip(self), fields(height = input.height))]
    pub async fn rollup(&self, input: &RollupInput) -> Result<H256> {
        info!("Sending proof and new root to Tempo");

        let tx = self
            .contract
            .verify_block(
                &input.proof.proof,
                // These should never fail. If they fail, we will catch them in testing
                #[allow(clippy::unwrap_used)]
                input.proof.agg_instances.clone().try_into().unwrap(),
                &input.proof.old_root,
                &input.proof.new_root,
                &input.proof.utxo_hashes,
                input.other_hash,
                input.height,
                &input
                    .signatures
                    .iter()
                    .map(|s| &s.0[..])
                    .collect::<Vec<_>>(),
                1_000_000,
            )
            .await?;

        info!(
            ?tx,
            "Ethereum root rollup update sent. Waiting for receipt...",
        );

        let wait_start = std::time::Instant::now();
        while self
            .contract
            .client
            .client()
            .eth()
            .transaction_receipt(tx)
            .await?
            .is_none()
        {
            if self
                .contract
                .client
                .client()
                .eth()
                .transaction(TransactionId::Hash(tx))
                .await?
                .is_none()
            {
                // The transaction was dropped
                break;
            }

            // Wait for a maximum of 5 minutes
            if wait_start.elapsed() > std::time::Duration::from_secs(5 * 60) {
                return Err(Error::RollupTransactionTimeout);
            }

            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        }

        info!("Ethereum root rollup update confirmed");

        Ok(tx)
    }

    #[tracing::instrument(err, skip_all)]
    fn generate_aggregate_proof(
        &self,
        tree: &mut MerkleTree<SimpleHashCache>,
        txns: [Option<Transaction>; 6],
        current_block: u64,
    ) -> Result<(AggregateAgg<1>, Vec<u8>), Error> {
        let mut txns = txns.into_iter().map(|t| match t {
            Some(t) => Ok(t),
            None => Ok(Transaction {
                proof: SnarkWitness::V1(
                    Utxo::<MERKLE_TREE_DEPTH>::new_padding()
                        .snark(CircuitKind::Utxo)?
                        .to_witness(),
                ),
            }),
        });

        let mut utxo_aggregations = Vec::new();
        for _i in 0..UTXO_AGGREGATIONS {
            // Unwrap is safe because we know we have enough txns
            #[allow(clippy::unwrap_used)]
            let txns: [Transaction; UTXO_AGG_NUMBER] = (&mut txns)
                .take(3)
                .collect::<Result<Vec<_>>>()?
                .try_into()
                .unwrap();

            let utxo_aggregate = self.aggregate_utxo(tree, txns.clone(), current_block)?;
            utxo_aggregations.push(utxo_aggregate);
        }

        #[allow(clippy::unwrap_used)]
        let agg = self.aggregate_aggregate_utxo(&utxo_aggregations.try_into().unwrap())?;
        let agg_agg_agg = AggregateAgg::<1>::new([agg.snark(ParameterSet::TwentyOne)?]);
        let agg = agg_agg_agg;
        let (pk, _) = agg.keygen(ParameterSet::TwentyOne);
        let proof = evm_verifier::gen_proof(
            ParameterSet::TwentyOne,
            &pk,
            agg.clone(),
            &[&agg.public_inputs()],
        )?;

        Ok((agg, proof))
    }

    #[tracing::instrument(err, skip_all)]
    fn aggregate_aggregate_utxo(
        &self,
        aggregations: &[AggregateUtxo<UTXO_AGG_NUMBER, MERKLE_TREE_DEPTH, UTXO_AGG_LEAVES>;
             UTXO_AGGREGATIONS],
    ) -> Result<AggregateAgg<UTXO_AGGREGATIONS>, Error> {
        Ok(AggregateAgg::new(
            #[allow(clippy::unwrap_used)]
            TryInto::<[Snark; UTXO_AGGREGATIONS]>::try_into(
                aggregations
                    .iter()
                    .map(|a| a.snark(ParameterSet::TwentyOne))
                    .collect::<Result<Vec<Snark>, _>>()?,
            )
            .unwrap(),
        ))
    }

    #[tracing::instrument(err, skip_all)]
    fn aggregate_utxo(
        &self,
        tree: &mut MerkleTree<SimpleHashCache>,
        utxos: [Transaction; UTXO_AGG_NUMBER],
        current_block: u64,
    ) -> Result<AggregateUtxo<UTXO_AGG_NUMBER, MERKLE_TREE_DEPTH, UTXO_AGG_LEAVES>, Error> {
        let (_, _, _, batch) = self.gen_batch(tree, &utxos, current_block)?;
        // TODO: use pre-generated VK as this can be expensive
        let (_, utxo_vk) = Utxo::<MERKLE_TREE_DEPTH>::default().keygen(ParameterSet::Fourteen);

        Ok(AggregateUtxo::new(
            utxos.map(|u| {
                let SnarkWitness::V1(proof) = u.proof;
                proof.to_snark(&utxo_vk, ParameterSet::Fourteen)
            }),
            batch,
        ))
    }

    #[tracing::instrument(err, skip_all)]
    fn gen_batch(
        &self,
        tree: &mut MerkleTree<SimpleHashCache>,
        txns: &[Transaction; UTXO_AGG_NUMBER],
        current_block: u64,
    ) -> Result<(
        usize,
        Element,
        Element,
        Batch<UTXO_AGG_LEAVES, MERKLE_TREE_DEPTH>,
    )> {
        let (inserts, old_tree, new_tree) = {
            let old_tree = tree.root_hash();
            let padding_path = tree.path_for(Note::padding_note().commitment());

            let mut leaves = vec![];

            // Extract leaves to be inserted from proof
            for Transaction { proof } in txns {
                let instances = match &proof {
                    SnarkWitness::V1(proof) => &proof.instances[0],
                };

                let elements = instances
                    .iter()
                    .skip(3)
                    .map(|f| Element::from_base(f.to_base()))
                    .collect::<Vec<Element>>();

                // Skip the first instance, as that is the root
                leaves.extend(elements);
            }

            let mut inserts = vec![];
            for leaf in leaves {
                let path = if leaf == Element::ZERO {
                    padding_path.clone()
                } else {
                    tree.insert(
                        leaf,
                        SmirkMetadata {
                            inserted_in: current_block,
                        },
                    )?;
                    tree.path_for(leaf)
                };

                let fpath = path
                    .siblings_deepest_first()
                    .iter()
                    .cloned()
                    .take(MERKLE_TREE_PATH_DEPTH)
                    .collect::<Vec<Element>>();

                let mp = MerklePath::new(fpath);
                inserts.push(Insert::new(leaf, mp));
            }

            let new_tree = tree.root_hash();

            (inserts, old_tree, new_tree)
        };
        let inserts_len: usize = inserts.len();

        #[allow(clippy::unwrap_used)]
        let fixed_size_inserts: [Insert<MERKLE_TREE_DEPTH>; UTXO_AGG_LEAVES] =
            inserts.try_into().unwrap();

        Ok((
            inserts_len,
            old_tree,
            new_tree,
            Batch::new(fixed_size_inserts),
        ))
    }

    pub fn get_proof(&self, notes_tree: &MerkleTree, note_cm: Base) -> Result<Vec<Base>, Error> {
        let el = Element::from_base(note_cm);

        // Sibling path
        let path = notes_tree.path_for(el);

        let path = path
            .siblings_deepest_first()
            .iter()
            .copied()
            .map(Element::to_base)
            .collect();

        Ok(path)
    }

    // TODO: We don't use this yet, when we do, make the ban tree persistent
    // /// Bans a given address, adding them to the banned list
    // pub async fn ban_address(&self, address: Base) -> Result<Vec<Base>, Error> {
    //     let mut ban_tree = self.ban_tree.lock();
    //     let el = Element::from_base(address);

    //     // Insert address into banned tree
    //     ban_tree.insert(el, ())?;

    //     let path = ban_tree.path_for(el);

    //     let path = path
    //         .siblings_deepest_first()
    //         .iter()
    //         .copied()
    //         .map(Element::to_base)
    //         .collect();

    //     Ok(path)
    // }

    /// Checks if the address is in the banned list
    pub async fn compliance_check(
        &self,
        ban_tree: &MerkleTree,
        address: Base,
    ) -> Result<(bool, Vec<Base>), Error> {
        let el = Element::from_base(address);

        let is_banned = ban_tree.contains_element(&el);
        let path = ban_tree.path_for(el);

        let path = path
            .siblings_deepest_first()
            .iter()
            .copied()
            .map(Element::to_base)
            .collect();

        Ok((is_banned, path))
    }
}

// #[cfg(test)]
// mod tests {
//     use std::str::FromStr;

//     use web3::types::Address;
//     use zk_circuits::utxo::{InputNote, Note, Utxo, UtxoKind};

//     use super::*;

//     struct Env {
//         rollup_contract_addr: Address,
//         evm_secret_key: SecretKey,
//     }

//     fn get_env() -> Env {
//         Env {
//             rollup_contract_addr: Address::from_str(
//                 &std::env::var("ROLLUP_CONTRACT_ADDR")
//                     .expect("env var ROLLUP_CONTRACT_ADDR is not set"),
//             )
//             .unwrap(),
//             evm_secret_key: SecretKey::from_str(&std::env::var("PROVER_SECRET_KEY").unwrap_or(
//                 // Seems to be the default when deploying with hardhat to a local node
//                 "ac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80".to_owned(),
//             ))
//             .unwrap(),
//         }
//     }

//     fn contract(addr: Address) -> RollupContract {
//         let rpc = std::env::var("ETHEREUM_RPC").unwrap_or("http://localhost:8545".to_owned());
//         let web3 = web3::Web3::new(web3::transports::Http::new(&rpc).unwrap());
//         let contract = include_bytes!("../../../eth/artifacts/contracts/Rollup.sol/Rollup.json");
//         let contract = serde_json::from_slice::<serde_json::Value>(contract).unwrap();
//         let contract =
//             serde_json::from_value::<ethabi::Contract>(contract.get("abi").unwrap().clone())
//                 .unwrap();
//         let contract = web3::contract::Contract::new(web3.eth(), addr, contract);

//         RollupContract::new(web3, contract)
//     }

//     #[tokio::test]
//     async fn test_prover() {
//         let env = get_env();
//         let contract = contract(env.rollup_contract_addr);

//         let notes_tree: Arc<Mutex<Tree<33>>> =
//             Arc::new(Mutex::new(Tree::<MERKLE_TREE_DEPTH>::new()));

//         let ban_tree: Arc<Mutex<Tree<MERKLE_TREE_DEPTH>>> =
//             Arc::new(Mutex::new(Tree::<MERKLE_TREE_DEPTH>::new()));

//         let root_hash = notes_tree.lock().root_hash().to_base();

//         let prover = Prover::new(contract, env.evm_secret_key, notes_tree, ban_tree);

//         let inputs = [
//             InputNote::<MERKLE_TREE_DEPTH>::padding_note(),
//             InputNote::padding_note(),
//         ];
//         let outputs = [Note::padding_note(), Note::padding_note()];
//         let utxo_proof = Utxo::new(inputs, outputs, root_hash, UtxoKind::Transfer);
//         let utxo_params = read_utxo_params();

//         // Prove
//         let snark = utxo_proof.snark(&utxo_params).unwrap();

//         prover
//             .add_tx(Transaction {
//                 proof: snark.to_witness(),
//             })
//             .unwrap();

//         prover.rollup().await.unwrap();
//     }

//     lazy_static::lazy_static! {
//         // Without this, we get "nonce too low" error when running tests in parallel:
//         // "Nonce too low. Expected nonce to be 1008 but got 1007."
//         static ref TEST_NONCE_BUG_MUTEX: tokio::sync::Mutex<()> = tokio::sync::Mutex::new(());
//     }

//     #[tokio::test]
//     async fn add_prover() {
//         let _lock = TEST_NONCE_BUG_MUTEX.lock().await;

//         let env = get_env();
//         let contract = contract(env.rollup_contract_addr);

//         let tx = contract
//             .add_prover(&env.evm_secret_key, &Address::from_low_u64_be(0xfe))
//             .await
//             .unwrap();

//         while contract
//             .web3_client
//             .eth()
//             .transaction_receipt(tx)
//             .await
//             .unwrap()
//             .is_none()
//         {
//             tokio::time::sleep(std::time::Duration::from_secs(1)).await;
//         }
//     }
// }
