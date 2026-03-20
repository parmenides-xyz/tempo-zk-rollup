use super::protocol::PolyProtocol;
use borsh::{BorshDeserialize, BorshSerialize};
use libp2p::{
    request_response,
    swarm::{behaviour::toggle::Toggle, keep_alive, NetworkBehaviour},
};

#[derive(NetworkBehaviour)]
pub struct Behaviour<NetworkEvent>
where
    NetworkEvent: Clone + Sync + Send + BorshSerialize + BorshDeserialize + 'static,
{
    pub rr: request_response::Behaviour<PolyProtocol<NetworkEvent>>,
    pub keep_alive: keep_alive::Behaviour,
    pub whitelist: Toggle<whitelist_ips::Behaviour>,
}
