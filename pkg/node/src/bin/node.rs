use std::sync::Arc;
use std::{pin::Pin, time::Duration};

use clap::Parser;
use eyre::Result;
use futures::Future;
use node::{
    config::{cli::CliArgs, Config},
    create_rpc_server,
};
use node::{Mode, Node, TxnStats};
use rpc::tracing::setup_tracing;

#[tokio::main]
async fn main() -> Result<()> {
    color_eyre::install().unwrap();

    let args = CliArgs::parse();

    let config = Config::from_env(args.clone()).unwrap();

    let _guard = setup_tracing(
        &[
            "node",
            "solid",
            "smirk",
            "p2p",
            "prover",
            "zk_primitives",
            "contracts",
            "block_store",
        ],
        &args.log_level,
        &args.log_format,
        config.sentry_dsn.clone(),
        config.env_name.clone(),
    )?;

    // Listen address of the server
    let rpc_laddr = config.rpc_laddr.clone();

    // Private key
    let peer_signer = config.secret_key.clone();

    let secret_key =
        web3::signing::SecretKey::from_slice(&config.secret_key.secret_key().secret_bytes()[..])
            .unwrap();
    let contracts_client =
        contracts::Client::new(&config.eth_rpc_url, config.minimum_gas_price_gwei);
    let contract =
        contracts::RollupContract::load(contracts_client, &config.rollup_contract_addr, secret_key)
            .await?;

    // Services
    let node = Node::new(peer_signer, contract.clone(), config.clone()).unwrap();
    let txn_stats = Arc::new(TxnStats::new(Arc::clone(&node.shared)));
    let server = create_rpc_server(
        &rpc_laddr,
        config.health_check_commit_interval_sec,
        Arc::clone(&node.shared),
        Arc::clone(&txn_stats),
    )?;

    let prover_task: Pin<Box<dyn Future<Output = Result<(), node::prover::Error>>>> =
        if config.mode == Mode::Prover || config.mode == Mode::MockProver {
            Box::pin(node::prover::worker::run_prover(
                &config,
                Arc::clone(&node.shared),
            ))
        } else {
            Box::pin(async { futures::future::pending().await })
        };

    tokio::select! {
        res = node.run() => {
            tracing::info!("node shutdown: {:?}", res);
        }
        res = prover_task => {
            tracing::info!("prover shutdown: {:?}", res);
        }
        res = server => {
            tracing::info!("rpc server shutdown: {:?}", res);
        }
        res = contract.worker(Duration::from_secs(30)) => {
            tracing::info!("contract worker shutdown: {:?}", res);
        }
        res = txn_stats.worker() => {
            tracing::info!("txn stats worker shutdown: {:?}", res);
        }
    }

    Ok(())
}
