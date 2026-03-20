use std::{str::FromStr, sync::Arc};

use super::State;
use crate::{node, utxo::UtxoProof, BlockFormat};
use actix_web::web;
use base64::Engine;
use block_store::BlockListOrder;
use eyre::Context;
use futures::StreamExt;
use itertools::Itertools;
use primitives::{
    block_height::BlockHeight,
    hash::CryptoHash,
    pagination::{Cursor, CursorChoice, OpaqueCursor, OpaqueCursorChoice, Paginator},
};
use rpc::error::{HTTPError, HttpResult};
use serde::{Deserialize, Serialize};
use wire_message::WireMessage;
use zk_circuits::data::SnarkWitness;
use zk_primitives::Element;

#[derive(Deserialize)]
pub struct SubmitUtxoBody {
    snark: SnarkWitness,
}

#[derive(Serialize)]
pub struct SubmitUtxoResp {
    height: BlockHeight,
    root_hash: Element,
    txn_hash: CryptoHash,
}

#[tracing::instrument(err, skip_all)]
pub async fn submit_txn(
    state: web::Data<State>,
    web::Json(data): web::Json<SubmitUtxoBody>,
) -> HttpResult<web::Json<SubmitUtxoResp>> {
    let SnarkWitness::V1(snark) = &data.snark;

    tracing::info!(
        method = "submit_txn",
        instances = ?snark.instances,
        proof = base64::prelude::BASE64_STANDARD.encode(&snark.proof),
        "Incoming request"
    );

    let utxo = UtxoProof::from_snark_witness(data.snark);
    let utxo_hash = utxo.hash();

    let node = Arc::clone(&state.node);
    let block = tokio::spawn(async move { node.submit_transaction_and_wait(utxo).await })
        .await
        .context("tokio spawn join handle error")??;

    Ok(web::Json(SubmitUtxoResp {
        height: block.content.header.height,
        root_hash: block.content.state.root_hash,
        txn_hash: utxo_hash,
    }))
}

#[derive(Serialize)]
pub(crate) struct TxnWithInfo {
    pub(crate) proof: UtxoProof,
    pub(crate) index_in_block: u64,
    pub(crate) hash: CryptoHash,
    pub(crate) block_height: BlockHeight,
    pub(crate) time: u64,
}

#[derive(Serialize)]
pub struct ListTxnsResponse {
    txns: Vec<TxnWithInfo>,
    cursor: OpaqueCursor<ListTxnsPosition>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct ListTxnsPosition {
    block: BlockHeight,
    txn: u64,
}

#[derive(Debug, Deserialize)]
enum ListTxnOrder {
    NewestToOldest,
    OldestToNewest,
}

impl ListTxnOrder {
    fn to_block_list_order(&self) -> BlockListOrder {
        match self {
            ListTxnOrder::NewestToOldest => BlockListOrder::HighestToLowest,
            ListTxnOrder::OldestToNewest => BlockListOrder::LowestToHighest,
        }
    }

    fn newest_to_oldest() -> Self {
        ListTxnOrder::NewestToOldest
    }
}

#[derive(Debug, Deserialize)]
pub struct ListTxnsQuery {
    limit: Option<usize>,
    cursor: Option<OpaqueCursorChoice<ListTxnsPosition>>,
    #[serde(default = "ListTxnOrder::newest_to_oldest")]
    order: ListTxnOrder,
    #[serde(default = "bool::default")]
    poll: bool,
}

#[tracing::instrument(err, skip_all)]
pub async fn list_txns(
    state: web::Data<State>,
    path: web::Path<()>,
    web::Query(query): web::Query<ListTxnsQuery>,
) -> HttpResult<web::Json<ListTxnsResponse>> {
    tracing::info!(method = "list_txns", ?path, ?query, "Incoming request");

    let block_fetcher =
        |cursor: &Option<CursorChoice<BlockHeight>>, order: BlockListOrder, limit: usize| {
            state
                .node
                .fetch_blocks_non_empty_paginated(cursor, order, limit)
        };

    let max_height = state.node.max_height();

    let (cursor, transactions) = list_txns_inner(block_fetcher, &query, max_height)?;

    let (cursor, transactions) = if transactions.is_empty() && query.poll {
        let towards_newer_height = match (&query.order, query.cursor.as_deref()) {
            (ListTxnOrder::NewestToOldest, Some(CursorChoice::Before(before))) => {
                Some(before.inner().block.next())
            }
            (ListTxnOrder::OldestToNewest, Some(CursorChoice::After(after))) => {
                Some(after.inner().block.next())
            }
            _ => None,
        };

        match towards_newer_height {
            None => {
                // There is no new block to wait for,
                // so we just sleep in case the client retries immediately.
                tokio::time::sleep(std::time::Duration::from_secs(25)).await;
                (cursor, transactions)
            }
            Some(height) => {
                let commit_stream = state.node.commit_stream(Some(height)).await;
                let mut non_empty_block_stream = Box::pin(commit_stream.filter(|r| {
                    let error_or_block_has_commits = r
                        .as_ref()
                        .map_or(true, |commit| !commit.content.state.txns.is_empty());

                    async move { error_or_block_has_commits }
                }));

                tokio::select! {
                    _ = tokio::time::sleep(std::time::Duration::from_secs(50)) => {
                        (cursor, transactions)
                    }
                    _ = non_empty_block_stream.next() => {
                        list_txns_inner(
                            block_fetcher,
                            &query,
                            max_height,
                        )?
                    }
                }
            }
        }
    } else {
        (cursor, transactions)
    };

    Ok(web::Json(ListTxnsResponse {
        cursor: cursor.into_opaque(),
        txns: transactions,
    }))
}

#[allow(clippy::type_complexity)]
fn list_txns_inner<I: Iterator<Item = Result<BlockFormat, node::Error>>>(
    block_fetcher: impl FnOnce(
        &Option<CursorChoice<BlockHeight>>,
        BlockListOrder,
        usize,
    ) -> Result<I, node::Error>,
    query: &ListTxnsQuery,
    max_height: BlockHeight,
) -> Result<(Cursor<ListTxnsPosition>, Vec<TxnWithInfo>), HTTPError> {
    let txn_limit = query.limit.unwrap_or(10).min(100);

    let blocks = block_fetcher(
        &query
            .cursor
            .as_ref()
            .map(|pag| pag.map_pos(|pos| pos.block)),
        query.order.to_block_list_order(),
        // Because we filter later in the code, we need to fetch an extra block
        txn_limit + 1,
    )?;

    let transactions = blocks
        .map(|r| {
            r.map(|r| {
                let (block, metadata) = match r.upgrade(&mut ()).unwrap() {
                    node::BlockFormat::V1(_) => unreachable!("already upgraded"),
                    node::BlockFormat::V2(block, metadata) => (block, metadata),
                };

                block
                    .content
                    .state
                    .txns
                    .into_iter()
                    .enumerate()
                    .map(move |(i, txn)| TxnWithInfo {
                        hash: txn.hash(),
                        proof: txn,
                        index_in_block: i as u64,
                        block_height: block.content.header.height,
                        time: metadata.timestamp_unix_s.unwrap_or(
                            node::NodeShared::estimate_block_time(
                                block.content.header.height,
                                max_height,
                            ),
                        ),
                    })
            })
        })
        .flatten_ok();

    // These are transactions that were returned on the previous page,
    // since the last returned block could have had
    // more (in total at the time) transactions than the limit.
    let txns_to_skip = query
        .cursor
        .clone()
        .map(|cursor| cursor.into_inner())
        .map(|cursor| (cursor.inner().block, 0)..=(cursor.inner().block, cursor.inner().txn));

    let transactions = transactions
        .filter(|r| {
            if let Some(txns_to_skip) = &txns_to_skip {
                let Ok(txn) = r else {
                    return true;
                };

                !txns_to_skip.contains(&(txn.block_height, txn.index_in_block))
            } else {
                true
            }
        })
        .take(txn_limit);

    let (cursor, transactions) = Paginator::new(transactions, |r| {
        r.as_ref().ok().map(|txn| ListTxnsPosition {
            block: txn.block_height,
            txn: txn.index_in_block,
        })
    })
    .collect::<Result<Vec<_>, _>>();

    let transactions = transactions?;

    Ok((
        Cursor {
            // See the comment on txns_to_skip as to why this needs to be inclusive
            before: cursor.before.map(|b| b.inclusive()),
            after: cursor.after.map(|a| a.inclusive()),
        },
        transactions,
    ))
}

#[derive(Serialize)]
pub struct GetTxnResponse {
    txn: TxnWithInfo,
}

#[tracing::instrument(err, skip_all)]
pub async fn get_txn(
    state: web::Data<State>,
    path: web::Path<(String,)>,
) -> HttpResult<web::Json<GetTxnResponse>> {
    tracing::info!(method = "get_txn", ?path, "Incoming request");

    let (txn_hash,) = path.into_inner();
    let txn_hash =
        CryptoHash::from_str(&txn_hash).map_err(|err| crate::Error::FailedToParseHash {
            hash: txn_hash,
            source: err,
        })?;

    let (txn, metadata) = state
        .node
        .get_txn(txn_hash.into_inner())?
        .ok_or(crate::Error::TxnNotFound { txn: txn_hash })?;

    let time = metadata.block_time.unwrap_or_else(|| {
        node::NodeShared::estimate_block_time(metadata.block_height, state.node.max_height())
    });

    Ok(web::Json(GetTxnResponse {
        txn: TxnWithInfo {
            proof: txn,
            index_in_block: metadata.block_txn_index as u64,
            hash: txn_hash,
            block_height: metadata.block_height,
            time,
        },
    }))
}

#[cfg(test)]
mod tests {
    use primitives::pagination::Opaque;

    use crate::{Block, BlockFormat};

    use super::*;

    #[test]
    fn list_txns_pagination() {
        let tempdir = tempdir::TempDir::new("list_txns").unwrap();

        let store = block_store::BlockStore::<BlockFormat>::create_or_load(tempdir.path()).unwrap();

        let new_block = |height: u64, txns: Vec<UtxoProof>| {
            let mut block = Block::default();
            block.content.header.height = BlockHeight(height);
            block.content.state.txns = txns;
            block
        };

        let new_proof = |recent_root: Element| UtxoProof {
            recent_root,
            ..UtxoProof::default()
        };

        let blocks = [
            new_block(1, vec![]),
            new_block(2, vec![new_proof(Element::new(1))]),
            new_block(
                3,
                vec![new_proof(Element::new(2)), new_proof(Element::new(3))],
            ),
            new_block(4, vec![new_proof(Element::new(4))]),
        ];

        let max_height = blocks.last().unwrap().content.header.height;

        for block in &blocks {
            store.set(&BlockFormat::V1(block.clone())).unwrap();
        }

        let block_fetcher =
            |cursor: &Option<CursorChoice<BlockHeight>>, order: BlockListOrder, limit: usize| {
                Ok(store
                    .list_paginated(cursor, order, limit)?
                    .map(|r| r.map(|(_, block)| block).map_err(node::Error::from)))
            };

        let (_pagination, txns) = list_txns_inner(
            block_fetcher,
            &ListTxnsQuery {
                limit: Some(10),
                cursor: None,
                order: ListTxnOrder::NewestToOldest,
                poll: false,
            },
            max_height,
        )
        .unwrap();
        assert_eq!(txns.len(), 4);

        // Newest to oldest
        {
            let (cursor, txns) = list_txns_inner(
                block_fetcher,
                &ListTxnsQuery {
                    limit: Some(1),
                    cursor: None,
                    order: ListTxnOrder::NewestToOldest,
                    poll: false,
                },
                max_height,
            )
            .unwrap();
            assert_eq!(txns.len(), 1);
            assert_eq!(txns[0].block_height, BlockHeight(4));

            let (cursor, txns) = list_txns_inner(
                block_fetcher,
                &ListTxnsQuery {
                    limit: Some(1),
                    cursor: Some(Opaque(CursorChoice::After(cursor.after.unwrap()))),
                    order: ListTxnOrder::NewestToOldest,
                    poll: false,
                },
                max_height,
            )
            .unwrap();
            assert_eq!(txns.len(), 1);
            assert_eq!(txns[0].block_height, BlockHeight(3));
            assert_eq!(txns[0].proof.recent_root, Element::new(2));

            let (cursor, txns) = list_txns_inner(
                block_fetcher,
                &ListTxnsQuery {
                    limit: Some(1),
                    cursor: Some(Opaque(CursorChoice::After(cursor.after.unwrap()))),
                    order: ListTxnOrder::NewestToOldest,
                    poll: false,
                },
                max_height,
            )
            .unwrap();
            assert_eq!(txns.len(), 1);
            assert_eq!(txns[0].block_height, BlockHeight(3));
            assert_eq!(txns[0].proof.recent_root, Element::new(3));

            let (cursor, txns) = list_txns_inner(
                block_fetcher,
                &ListTxnsQuery {
                    limit: Some(1),
                    cursor: Some(Opaque(CursorChoice::After(cursor.after.unwrap()))),
                    order: ListTxnOrder::NewestToOldest,
                    poll: false,
                },
                max_height,
            )
            .unwrap();
            assert_eq!(txns.len(), 1);
            assert_eq!(txns[0].block_height, BlockHeight(2));
            assert_eq!(txns[0].proof.recent_root, Element::new(1));

            let (cursor, txns) = list_txns_inner(
                block_fetcher,
                &ListTxnsQuery {
                    limit: Some(1),
                    cursor: Some(Opaque(CursorChoice::After(cursor.after.unwrap()))),
                    order: ListTxnOrder::NewestToOldest,
                    poll: false,
                },
                max_height,
            )
            .unwrap();
            assert_eq!(txns.len(), 0);
            assert_eq!(cursor.before, None);
            assert_eq!(cursor.after, None);
        };

        // Oldest to newest
        {
            let (cursor, txns) = list_txns_inner(
                block_fetcher,
                &ListTxnsQuery {
                    limit: Some(1),
                    cursor: None,
                    order: ListTxnOrder::OldestToNewest,
                    poll: false,
                },
                max_height,
            )
            .unwrap();
            assert_eq!(txns.len(), 1);
            assert_eq!(txns[0].block_height, BlockHeight(2));
            assert_eq!(txns[0].proof.recent_root, Element::new(1));

            let (cursor, txns) = list_txns_inner(
                block_fetcher,
                &ListTxnsQuery {
                    limit: Some(1),
                    cursor: Some(Opaque(CursorChoice::After(cursor.after.unwrap()))),
                    order: ListTxnOrder::OldestToNewest,
                    poll: false,
                },
                max_height,
            )
            .unwrap();
            assert_eq!(txns.len(), 1);
            assert_eq!(txns[0].block_height, BlockHeight(3));
            assert_eq!(txns[0].proof.recent_root, Element::new(2));

            let (cursor, txns) = list_txns_inner(
                block_fetcher,
                &ListTxnsQuery {
                    limit: Some(1),
                    cursor: Some(Opaque(CursorChoice::After(cursor.after.unwrap()))),
                    order: ListTxnOrder::OldestToNewest,
                    poll: false,
                },
                max_height,
            )
            .unwrap();
            assert_eq!(txns.len(), 1);
            assert_eq!(txns[0].block_height, BlockHeight(3));
            assert_eq!(txns[0].proof.recent_root, Element::new(3));

            let (cursor, txns) = list_txns_inner(
                block_fetcher,
                &ListTxnsQuery {
                    limit: Some(1),
                    cursor: Some(Opaque(CursorChoice::After(cursor.after.unwrap()))),
                    order: ListTxnOrder::OldestToNewest,
                    poll: false,
                },
                max_height,
            )
            .unwrap();
            assert_eq!(txns.len(), 1);
            assert_eq!(txns[0].block_height, BlockHeight(4));
            assert_eq!(txns[0].proof.recent_root, Element::new(4));

            let (cursor, txns) = list_txns_inner(
                block_fetcher,
                &ListTxnsQuery {
                    limit: Some(1),
                    cursor: Some(Opaque(CursorChoice::After(cursor.after.unwrap()))),
                    order: ListTxnOrder::OldestToNewest,
                    poll: false,
                },
                max_height,
            )
            .unwrap();
            assert_eq!(txns.len(), 0);
            assert_eq!(cursor.before, None);
            assert_eq!(cursor.after, None);
        };
    }
}
