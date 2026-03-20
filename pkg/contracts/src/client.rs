use std::{future::Future, time::Duration};

use crate::Result;
use ethereum_types::{Address, H256, U64};
use testutil::eth::EthNode;
use tokio::time::interval;
use web3::{
    contract::{tokens::Tokenize, Contract, Options},
    ethabi,
    signing::SecretKey,
    transports::Http,
    types::{Transaction, U256},
    Web3,
};

#[derive(Debug, Clone)]
pub struct Client {
    client: Web3<Http>,
    minimum_gas_price: Option<U256>,
    pub use_latest_for_nonce: bool,
}

impl Client {
    pub fn new(rpc: &str, minimum_gas_price_gwei: Option<u64>) -> Client {
        let client = Web3::new(Http::new(rpc).unwrap());
        let minimum_gas_price = minimum_gas_price_gwei.map(|gwei| U256::from(gwei) * 1_000_000_000);

        Client {
            client,
            minimum_gas_price,
            use_latest_for_nonce: false,
        }
    }

    pub fn load_contract_from_str(
        &self,
        address: &str,
        contract_json: &str,
    ) -> Result<Contract<Http>> {
        let contract_json_value = serde_json::from_str::<serde_json::Value>(contract_json)?;
        // unwrap should be fine since the json is embedded at build time
        #[allow(clippy::unwrap_used)]
        let abi_value = contract_json_value.get("abi").unwrap();

        let contract_abi = serde_json::from_value::<ethabi::Contract>(abi_value.clone())?;

        Ok(Contract::new(
            self.client.eth(),
            address.parse()?,
            contract_abi,
        ))
    }

    pub fn from_eth_node(eth_node: &EthNode) -> Self {
        Self::new(&eth_node.rpc_url(), None)
    }

    pub async fn eth_balance(&self, address: Address) -> Result<U256> {
        let balance =
            retry_on_network_failure(move || self.client.eth().balance(address, None)).await?;
        Ok(balance)
    }

    pub fn client(&self) -> &Web3<Http> {
        &self.client
    }

    pub async fn fast_gas_price(&self) -> Result<U256, web3::Error> {
        let gas_price: U256 =
            retry_on_network_failure(move || self.client.eth().gas_price()).await?;
        let fast_gas_price = gas_price * 2;

        match self.minimum_gas_price {
            Some(minimum_gas_price) if fast_gas_price < minimum_gas_price => Ok(minimum_gas_price),
            _ => Ok(fast_gas_price),
        }
    }

    #[tracing::instrument(err, ret, skip(self))]
    pub async fn get_nonce(
        &self,
        address: Address,
        block: web3::types::BlockNumber,
    ) -> Result<U256, web3::Error> {
        self.client
            .eth()
            .transaction_count(address, Some(block))
            .await
    }

    #[tracing::instrument(err, ret, skip(self))]
    pub async fn nonce(&self, address: Address) -> Result<U256, web3::Error> {
        retry_on_network_failure(move || {
            self.get_nonce(
                address,
                match self.use_latest_for_nonce {
                    true => web3::types::BlockNumber::Latest,
                    false => web3::types::BlockNumber::Pending,
                },
            )
        })
        .await
    }

    pub(crate) async fn options(&self, address: Address) -> Result<Options, web3::Error> {
        let gas_price = self.fast_gas_price().await?;
        let nonce = self.nonce(address).await?;

        Ok(Options {
            gas: Some(10_000_000.into()),
            gas_price: Some(gas_price),
            nonce: Some(nonce),
            ..Default::default()
        })
    }

    pub async fn call(
        &self,
        contract: &Contract<Http>,
        func: &str,
        params: impl Tokenize + Clone,
        signer: &SecretKey,
        _signer_address: Address,
    ) -> Result<H256> {
        // Encode the function call data using the contract ABI
        let tokens = params.into_tokens();
        let function = contract.abi().function(func)
            .map_err(|e| crate::error::Error::Other(format!("ABI error: {e}")))?;
        let calldata = function.encode_input(&tokens)
            .map_err(|e| crate::error::Error::Other(format!("Encode error: {e}")))?;

        let contract_addr = format!("{:?}", contract.address());
        let calldata_hex = format!("0x{}", hex::encode(&calldata));
        let key_hex = format!("0x{}", hex::encode(signer.secret_bytes()));
        let fee_token = "0x20c000000000000000000000b9537d11c60e8b50";

        // Use Tempo's Foundry fork (cast) to send the transaction with fee token support
        let output = std::process::Command::new("cast")
            .args([
                "send", &contract_addr,
                &calldata_hex,
                "--rpc-url", "https://rpc.tempo.xyz",
                "--private-key", &key_hex,
                "--tempo.fee-token", fee_token,
                "--json",
            ])
            .output()
            .map_err(|e| crate::error::Error::Other(format!("cast failed to execute: {e}")))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(crate::error::Error::Other(format!("cast send failed: {stderr}")));
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        let json: serde_json::Value = serde_json::from_str(&stdout)
            .map_err(|e| crate::error::Error::Other(format!("cast JSON parse error: {e}, output: {stdout}")))?;

        let tx_hash_str = json["transactionHash"].as_str()
            .ok_or_else(|| crate::error::Error::Other("No transactionHash in cast output".into()))?;

        let tx_hash: H256 = tx_hash_str.parse()
            .map_err(|e| crate::error::Error::Other(format!("Invalid tx hash: {e}")))?;

        Ok(tx_hash)
    }

    pub async fn query<R, A, B, P>(
        &self,
        contract: &Contract<Http>,
        func: &str,
        params: P,
        from: A,
        options: Options,
        block: B,
    ) -> Result<R, web3::contract::Error>
    where
        R: web3::contract::tokens::Detokenize,
        A: Into<Option<Address>> + Clone,
        B: Into<Option<web3::types::BlockId>> + Clone,
        P: Tokenize + Clone,
    {
        let result =
            retry_on_network_failure(move || contract.query(func, params, from, options, block))
                .await?;

        Ok(result)
    }

    /// Wait for a transaction to be confirmed and returns the block number.
    ///
    /// Times out if a transaction has been unknown (not in mempool) for 60 seconds.
    #[tracing::instrument(err, skip(self))]
    pub async fn wait_for_confirm(&self, txn_hash: H256, interval_period: Duration) -> Result<U64> {
        let unknown_timeout = std::time::Instant::now() + Duration::from_secs(60);

        let mut interval = interval(interval_period);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

        loop {
            interval.tick().await;

            let tx = retry_on_network_failure(move || {
                self.client
                    .eth()
                    .transaction(web3::types::TransactionId::Hash(txn_hash))
            })
            .await?;

            match tx {
                None => {
                    // Transaction doesn't exist / is unknown
                    if std::time::Instant::now() > unknown_timeout {
                        return Err(crate::Error::UnknownTransaction(txn_hash));
                    }
                }
                Some(Transaction {
                    block_number: None, ..
                }) => {
                    // Transaction is pending
                }
                Some(Transaction {
                    block_number: Some(block_number),
                    ..
                }) => {
                    // Transaction is confirmed
                    return Ok(block_number);
                }
            }
        }
    }
}

trait IsNetworkFailure {
    fn is_network_failure(&self) -> bool;
}

impl IsNetworkFailure for web3::error::Error {
    fn is_network_failure(&self) -> bool {
        matches!(self, web3::error::Error::Transport(_))
    }
}

impl IsNetworkFailure for web3::contract::Error {
    fn is_network_failure(&self) -> bool {
        matches!(
            self,
            web3::contract::Error::Api(web3::error::Error::Transport(_))
        )
    }
}

/// Retries 4 times for a maximum of 16s.
async fn retry_on_network_failure<T, E: IsNetworkFailure, Fut: Future<Output = Result<T, E>>>(
    f: impl FnOnce() -> Fut + Clone,
) -> Result<T, E> {
    const DELAYS: &[Duration] = &[
        Duration::from_secs(1),
        Duration::from_secs(5),
        Duration::from_secs(10),
    ];

    for (i, delay) in DELAYS
        .iter()
        .chain(std::iter::once(&Duration::ZERO))
        .enumerate()
    {
        let res = (f.clone())().await;

        if res.as_ref().is_err_and(|err| err.is_network_failure()) {
            let was_last_try = i == DELAYS.len();
            if was_last_try {
                return res;
            }

            tokio::time::sleep(*delay).await;
        } else {
            return res;
        }
    }

    unreachable!()
}

#[cfg(test)]
mod tests {
    use std::sync::{atomic::AtomicU16, Arc};

    use web3::error::Error;
    use web3::error::TransportError;

    #[tokio::test]
    async fn test_retry_on_network_failure() {
        let gen_result = |succeed_at_call_count| async move {
            let call_count = Arc::new(AtomicU16::new(0));

            super::retry_on_network_failure(move || {
                let call_count = Arc::clone(&call_count);
                async move {
                    let call_count =
                        call_count.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
                    if call_count == succeed_at_call_count {
                        Ok(())
                    } else {
                        Err(Error::Transport(TransportError::Code(call_count)))
                    }
                }
            })
            .await
        };

        {
            // Never succeed
            let start = std::time::Instant::now();
            let result = gen_result(u16::MAX).await;
            let elapsed = start.elapsed();

            assert!(
                matches!(&result, Err(Error::Transport(TransportError::Code(4)))),
                "{result:?}"
            );
            assert!(elapsed >= std::time::Duration::from_secs(16), "{elapsed:?}");
        }

        {
            // Succeed first try
            let start = std::time::Instant::now();
            let result = gen_result(1).await;
            let elapsed = start.elapsed();

            assert!(result.is_ok(), "{result:?}");
            assert!(elapsed < std::time::Duration::from_millis(1), "{elapsed:?}");
        }
    }
}
