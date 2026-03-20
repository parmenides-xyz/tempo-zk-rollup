//! Minimal agent CLI for transacting on the Tempo ZK Rollup.
//!
//! Usage:
//!   agent mint <amount>   # Full flow: ZK proof → on-chain mint → submit UTXO to node
//!   agent info            # Show node status

use clap::{Parser, Subcommand};
use eyre::Result;
use zk_circuits::constants::MERKLE_TREE_DEPTH;
use zk_circuits::data::{Mint, ParameterSet, SnarkWitness, Utxo};
use zk_circuits::test::rollup::Wallet;

#[derive(Parser)]
#[command(name = "agent", about = "Tempo ZK Rollup — Agent CLI")]
struct Cli {
    #[arg(long, default_value = "https://rpc.tempo.xyz")]
    tempo_rpc: String,

    #[arg(long, default_value = "0xbFe5aafd3B85AaD2daCa84968Ae64FD534555776")]
    contract: String,

    #[arg(long, env = "TEMPO_PRIVATE_KEY")]
    key: String,

    #[arg(long, default_value = "0x20c000000000000000000000b9537d11c60e8b50")]
    fee_token: String,

    #[arg(long, default_value = "http://localhost:8080")]
    rpc: String,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Deposit into the rollup
    Mint {
        /// Amount in base units (1000000 = $1 USDC)
        amount: u64,
    },
    /// Show node status
    Info,
}

fn cast(args: &[&str]) -> Result<String> {
    let output = std::process::Command::new("cast").args(args).output()?;
    if !output.status.success() {
        let err = String::from_utf8_lossy(&output.stderr);
        eyre::bail!("cast failed: {}", err);
    }
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Command::Mint { amount } => {
            let wallet = Wallet::new();
            let note = wallet.new_note(amount);
            println!("Wallet:  {:?}", wallet.address());
            println!("Amount:  {} base units\n", amount);

            // Step 1: Generate EVM mint proof (for on-chain contract)
            println!("1. Generating ZK mint proof (on-chain)...");
            let mint = Mint::new([note.clone()]);
            let params = ParameterSet::Eight;
            let evm_proof = mint.evm_proof(params)?;
            println!("   {} bytes", evm_proof.len());

            let proof_hex = format!("0x{}", hex::encode(&evm_proof));
            let commitment = format!("0x{}", hex::encode(note.commitment().to_be_bytes()));
            let value = format!("0x{}", hex::encode(note.value().to_be_bytes()));
            let source = format!("0x{}", hex::encode(note.source().to_be_bytes()));

            // Step 2: Approve USDC
            println!("2. Approving USDC...");
            cast(&["send", &cli.fee_token,
                "approve(address,uint256)", &cli.contract,
                "0xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff",
                "--rpc-url", &cli.tempo_rpc,
                "--private-key", &cli.key,
                "--tempo.fee-token", &cli.fee_token])?;
            println!("   Done.");

            // Step 3: Call mint() on settlement contract
            println!("3. Submitting mint to Tempo L1...");
            let result = cast(&["send", &cli.contract,
                "mint(bytes,bytes32,bytes32,bytes32)",
                &proof_hex, &commitment, &value, &source,
                "--rpc-url", &cli.tempo_rpc,
                "--private-key", &cli.key,
                "--tempo.fee-token", &cli.fee_token])?;
            println!("   On-chain mint done.");

            // Step 4: Generate UTXO proof (for rollup node)
            println!("4. Generating UTXO mint proof (rollup node)...");
            let utxo: Utxo<MERKLE_TREE_DEPTH> = Utxo::new_mint(note);
            let utxo_snark = utxo.snark(zk_circuits::CircuitKind::Utxo)?;
            let witness = SnarkWitness::V1(utxo_snark.to_witness());
            println!("   UTXO proof generated.");

            // Step 5: Submit to rollup node
            println!("5. Submitting UTXO to rollup node...");
            let client = reqwest::Client::new();
            let resp = client
                .post(format!("{}/v0/transaction", cli.rpc))
                .json(&serde_json::json!({ "snark": witness }))
                .send()
                .await?;

            if resp.status().is_success() {
                let body: serde_json::Value = resp.json().await?;
                println!("   Accepted!");
                println!("   Block:    {}", body["height"]);
                println!("   Root:     {}", body["root_hash"]);
                println!("   Txn hash: {}", body["txn_hash"]);
            } else {
                let text = resp.text().await?;
                println!("   Node error: {}", text);
            }

            println!("\n{}", result.trim());
        }

        Command::Info => {
            let client = reqwest::Client::new();
            match client.get(format!("{}/v0/health", cli.rpc)).send().await {
                Ok(r) if r.status().is_success() => {
                    println!("Node: connected ({})", cli.rpc);
                    if let Ok(body) = r.json::<serde_json::Value>().await {
                        println!("{}", serde_json::to_string_pretty(&body)?);
                    }
                }
                _ => println!("Node: not reachable ({})", cli.rpc),
            }
        }
    }

    Ok(())
}
