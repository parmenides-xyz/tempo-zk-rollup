# ZK Rollup on Tempo

Reference implementation for first ZK rollup on [Tempo](https://tempo.xyz)—enables private UTXO-based stablecoin transfers that settle on Tempo L1.

## Deployed (Tempo Mainnet)

| Contract | Address |
|----------|---------|
| Rollup | [`0xbFe5aafd3B85AaD2daCa84968Ae64FD534555776`](https://explore.tempo.xyz/address/0xbFe5aafd3B85AaD2daCa84968Ae64FD534555776) |

## Quick Start

```bash
git clone https://github.com/danielyim/tempo-zk-rollup.git
cd tempo-zk-rollup
git lfs pull
forge install
cargo build --release --bin node --bin agent
```

### Run

```bash
# Terminal 1: Validator
cargo run --release --bin node -- \
  --eth-rpc-url=https://rpc.tempo.xyz \
  --rollup-contract-addr=0xbFe5aafd3B85AaD2daCa84968Ae64FD534555776 \
  --secret-key=$KEY --p2p-laddr=/ip4/127.0.0.1/tcp/5000

# Terminal 2: Prover
cargo run --release --bin node -- \
  --eth-rpc-url=https://rpc.tempo.xyz \
  --rollup-contract-addr=0xbFe5aafd3B85AaD2daCa84968Ae64FD534555776 \
  --secret-key=$KEY --mode=prover \
  --db-path=~/.tempo-rollup/prover/db \
  --smirk-path=~/.tempo-rollup/prover/smirk \
  --rpc-laddr=0.0.0.0:8081 \
  --p2p-laddr=/ip4/127.0.0.1/tcp/5001 \
  --p2p-dial=/ip4/127.0.0.1/tcp/5000

# Terminal 3: Mint $0.10
TEMPO_PRIVATE_KEY=$KEY cargo run --release --bin agent -- mint 100000
```
