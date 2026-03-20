use crate::network::{NetworkEvent, SnapshotAccept, SnapshotOffer, SnapshotRequest};
use crate::node::NodeShared;
use eyre::Context;
use libp2p::PeerId;
use p2p2::Network;
use std::sync::Arc;
use tokio::task::JoinHandle;

pub fn network_handler(
    network: Arc<Network<NetworkEvent>>,
    node: Arc<NodeShared>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            let Some((network_peer_id, event)) = network.next().await else { continue };
            tracing::debug!(network_peer_id = ?network_peer_id, event = ?event, "network event");

            if let Err(e) = handle_event(&node, network_peer_id, event).await {
                tracing::error!(error = ?e, "network error");
            }
        }
    })
}

async fn handle_event(
    node: &NodeShared,
    peer: PeerId,
    event: NetworkEvent,
) -> color_eyre::Result<()> {
    use NetworkEvent as NE;

    match event {
        NE::Approval(approval) => node
            .receive_accept(&approval)
            .await
            .context("Accept failed")?,

        NE::Block(block) => {
            node.receive_proposal(block)
                .context("Failed to process block")?;
            node.ticker.tick();
        }

        NE::Transaction(txn) => node
            .receive_transaction(txn)
            .await
            .context("Transaction failed")?,

        NE::SnapshotRequest(SnapshotRequest {
            snapshot_id,
            from_height,
            to_height,
            kind,
        }) => node
            .receive_snapshot_request(peer, snapshot_id, from_height, to_height, kind)
            .await
            .context("Snapshot request failed")?,

        NE::SnapshotOffer(SnapshotOffer { snapshot_id }) => node
            .receive_snapshot_offer(peer, snapshot_id)
            .context("Snapshot offer failed")?,

        NE::SnapshotChunk(sc) => node
            .receive_snapshot_chunk(peer, sc)
            .context("Snapshot chunk failed")?,

        NE::SnapshotAccept(SnapshotAccept {
            snapshot_id,
            from_height,
            to_height,
            kind,
        }) => node
            .receive_snapshot_accept(peer, snapshot_id, from_height, to_height, kind)
            .await
            .context("Snapshot accept failed")?,
    }

    Ok(())
}
