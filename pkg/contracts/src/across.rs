use crate::error::Result;
use crate::Client;
use ethereum_types::U64;
use testutil::eth::EthNode;
use web3::{
    contract::{
        tokens::{Tokenizable, Tokenize},
        Contract,
    },
    ethabi,
    signing::{Key, SecretKey, SecretKeyRef},
    transports::Http,
    types::{Address, FilterBuilder, H256, U256},
};

pub struct AcrossWithAuthorizationContract {
    client: Client,
    contract: Contract<Http>,
    signer: SecretKey,
    signer_address: Address,
    address: Address,
    /// The ethereum block height used for all contract calls.
    /// If None, the latest block is used.
    block_height: Option<U64>,
}

impl AcrossWithAuthorizationContract {
    pub fn new(
        client: Client,
        contract: Contract<Http>,
        signer: SecretKey,
        address: Address,
    ) -> Self {
        let signer_address = Key::address(&SecretKeyRef::new(&signer));

        Self {
            client,
            contract,
            signer,
            signer_address,
            address,
            block_height: None,
        }
    }

    pub fn at_height(mut self, block_height: Option<u64>) -> Self {
        self.block_height = block_height.map(|x| x.into());
        self
    }

    pub fn address(&self) -> Address {
        self.address
    }

    pub async fn load(client: Client, contract_address: &str, signer: SecretKey) -> Result<Self> {
        let contract_json = include_str!("../../../eth/artifacts/contracts/AcrossWithAuthorization.sol/AcrossWithAuthorization.json");
        let contract = client.load_contract_from_str(contract_address, contract_json)?;
        Ok(Self::new(
            client,
            contract,
            signer,
            contract_address.parse()?,
        ))
    }

    pub async fn from_eth_node(eth_node: &EthNode, signer: SecretKey) -> Result<Self> {
        let contract_addr = "TODO";

        let client = Client::from_eth_node(eth_node);
        Self::load(client, contract_addr, signer).await
    }

    pub async fn call(&self, func: &str, params: impl Tokenize + Clone) -> Result<H256> {
        self.client
            .call(
                &self.contract,
                func,
                params,
                &self.signer,
                self.signer_address,
            )
            .await
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn deposit_v3_with_authorization(
        &self,
        signature_for_receive: &[u8],
        signature_for_deposit: &[u8],
        valid_after: U256,
        valid_before: U256,
        nonce: H256,
        depositor: Address,
        recipient: Address,
        input_token: Address,
        output_token: Address,
        input_amount: U256,
        output_amount: U256,
        destination_chain_id: U256,
        exclusive_relayer: Address,
        quote_timestamp: u32,
        fill_deadline: u32,
        exclusivity_deadline: u32,
        message: Vec<u8>,
    ) -> Result<H256> {
        let r = &signature_for_receive[0..32];
        let s = &signature_for_receive[32..64];
        let v = signature_for_receive[64];
        let v = if v < 27 { v + 27 } else { v };

        let r2 = &signature_for_deposit[0..32];
        let s2 = &signature_for_deposit[32..64];
        let v2 = signature_for_deposit[64];
        let v2 = if v2 < 27 { v2 + 27 } else { v2 };

        self.call(
            "depositV3WithAuthorization",
            &[
                web3::types::U256::from(v).into_token(),
                web3::types::H256::from_slice(r).into_token(),
                web3::types::H256::from_slice(s).into_token(),
                web3::types::U256::from(v2).into_token(),
                web3::types::H256::from_slice(r2).into_token(),
                web3::types::H256::from_slice(s2).into_token(),
                valid_after.into_token(),
                valid_before.into_token(),
                nonce.into_token(),
                depositor.into_token(),
                recipient.into_token(),
                input_token.into_token(),
                output_token.into_token(),
                input_amount.into_token(),
                output_amount.into_token(),
                destination_chain_id.into_token(),
                exclusive_relayer.into_token(),
                quote_timestamp.into_token(),
                fill_deadline.into_token(),
                exclusivity_deadline.into_token(),
                message.into_token(),
            ][..],
        )
        .await
    }

    pub async fn deposit_event_txn(&self, depositor: Address, nonce: H256) -> Result<Option<H256>> {
        let event = self.contract.abi().event("Deposited")?;
        let topic_filter = event.filter(ethabi::RawTopicFilter {
            topic0: ethabi::Topic::This(depositor.into_token()),
            topic1: ethabi::Topic::This(nonce.into_token()),
            topic2: ethabi::Topic::Any,
        })?;

        let filter = FilterBuilder::default()
            .address(vec![self.address])
            .from_block(web3::types::BlockNumber::Earliest)
            .to_block(web3::types::BlockNumber::Latest)
            .topic_filter(topic_filter)
            .build();

        let logs = self.client.client().eth().logs(filter).await?;

        Ok(logs
            .into_iter()
            .filter_map(|log| log.transaction_hash)
            .next())
    }
}
