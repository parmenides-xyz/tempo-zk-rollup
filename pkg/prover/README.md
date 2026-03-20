# Prover

Prover is responsible for generating aggregation proofs and submitting them to Ethereum.

## Testing the prover

To test the prover, start a local Ethereum node with `cd eth && npm run node` and deploy the contracts:

```
cd eth
npm run deploy -- --network localhost
```

Copy the `Rollup` contract address and set it as an environment variable `ROLLUP_CONTRACT_ADDR`:

```
export ROLLUP_CONTRACT_ADDR=0x2279b7a0a67db372996a5fab50d91eaa73d2ebe6
```

Then run the tests:

```
cargo test
```


