# Ddust Development Journal

A running log of progress on the [ddust](https://github.com/bubb1es71/ddust) project - a Rust CLI tool for disposing of dust attack UTXOs.

---

## [17.02.2026] - Initial Research & Scoping

### Research
- [x] Studied the discussion on Delving Bitcoin: [Disposing of dust attack UTXOs](https://delvingbitcoin.org/t/disposing-of-dust-attack-utxos/2215)
- [x] Reviewed the [UTXO Set Report](https://research.mempool.space/utxo-set-report/) shared in the thread
  - ~49% of all UTXOs hold less than 1000 sats; most are taproot (likely data-embedding related)
  - Data does not specifically identify dust attack UTXOs - detection logic is still an open problem

### Findings
- [x] [`bubb1es71/ddust`](https://github.com/bubb1es71/ddust) - an early-stage Rust CLI for dedusting wallets; project has room to contribute meaningfully
- [x] The thread discusses using `SIGHASH_ANYONECANPAY` to allow multiple parties to combine their dust inputs into a single transaction, reducing fees and blockspace usage
- 
### Journal
To get started with the project, i setup a regtest bitcoin node locally and played around the cli app. I had to create a wallet, generate 101 blocks to enable spending coinbase outputs. I then created a dust utxo sending to a new address in my wallet. Finally generated a new block to clear the mempool. Once i had the setup ready, i following these steps to spend the first dust UTXO
- Add the wallet in the cli using ADD cli command
- Run LIST command to list all the dust UTXO
- Spend the dust dust UTXO using the SPEND command
    - The command returns an unsigned PSBT
- I signed the PSBT and finally broadcasted the signed PSBT
- I reviewed the transaction using `bitcoin-cli` and voila!, we have our dust removed from the wallet. The transaction output is an op_return with no data or a 3 byte string "ash" depending on the following factors:
    - if a single UTXO was spent and the input was a native segwit, then the total transaction size is ~62 bytes which is less then then minimum relay size of 65 bytes(non-witness size) since Bitcoin Core 25.0. In this case we need the 3 bytes of "ash" to ensure that the transaction size is acceptable:
        - e.g. P2TR: 10(overhead) + 41(input: 32(prev tx id) + 4(vout) + 1(script size of 0) + 4(sequence) fixed for native segwit) + 14(op_return: 10 + 3 bytes of 'ash') = 65 vbytes
    - if multiple UTXOs or a single UTXO of legacy script was spent, then the transaction is already well above 65 bytes and thus op_return data is null:
        - e.g. P2PKH: 10(overhead) + 148(input) + 10(op_return with null data) = 168 vbytes

### Contributions Proposed
- Dust attack detection logic
- Transaction combining via `SIGHASH_ANYONECANPAY` (discussed in the thread)
- Combining dust UTXOs with mempool ddust transactions

### Next Steps
- Reach out to bubb1es and explore collaboration

---

## [02.03.2026] - Collaboration, first PRs

### Collaboration
- [x] Contacted [bubb1es](https://github.com/bubb1es71) (upstream maintainer) via email; found through [his Delving Bitcoin post](https://delvingbitcoin.org/t/disposing-of-dust-attack-utxos/2215/2)
- [x] bubb1es accepted the collaboration and is willing to code review and test contributions
- [x] worked on Combining dust UTXOs with mempool ddust transactions [issue #1](https://github.com/bubb1es71/ddust/issues/1)

### Merged
- [x] [PR #4](https://github.com/bubb1es71/ddust/pull/4) - **Sidharth**: Added `SIGN` recipe to Justfile for PSBT signing, removing the need to pass multiple arguments manually to `bitcoin-cli`

### In Review
- [PR #5](https://github.com/bubb1es71/ddust/pull/5) - Combine unconfirmed ddust mempool transactions with new ddust transactions
  - Saves ~23 vbytes per input by reusing existing inputs
  - Details: [Delving Bitcoin comment](https://delvingbitcoin.org/t/disposing-of-dust-attack-utxos/2215/2?u=harris)

### In Progress
- **Sidharth**: Researching dust attack detection; will open a GitHub issue and implement
 
### Journal
To begin with implementing `combine dust UTXO with existing ddust unconfirmed transaction`, i started looking for a Rust crate that allows adding foreign UTXOs to a transaction. Fortunately bdk-wallet does that, albiet as an experimental feature. I implemented the feature by building a new transaction and as usual adding the newly found dust UTXOs to it. Then looked at the mempool, find the transactions that follow the ddust pattern:
- has a single op_return
- one or more inputs with SIGHASH_ALL|ANYONECANPAY signature type: meaning anyone can add inputs as long as the output committed to remains same
- op_return: can be empty or contains the string "ash"

The mempool UTXOs can be added to our new transaction only if the new fee rate > existing fee of mempool transactions(max of all txs) + 1 as required by RBF replacement rules

Combining UTXOs from mempool transactions allows us to save some blockchain space of around 23 bytes per input. This is significant.

### Next Steps
- bubb1es review of PR #5
- Open issue for dust attack detection (Sidharth)
- Schedule sync with bubb1es
- Work on [Issue #8](https://github.com/bubb1es71/ddust/issues/8) - Document test scenarios and vectors

---

## [08.03.2026] - Feature added: Combining dust UTXOs with mempool ddust transactions 

### Collaboration
- [x] [Code review](https://github.com/bubb1es71/ddust/issues/9) of [PR of the maintainer](https://github.com/bubb1es71/ddust/pull/10) - Broadcast fails when adding unconfirmed dust utxo

### Merged
- [x] [PR #2](https://github.com/bubb1es71/dusts/pull/2) - **Sidharth**: Fix division by zero panic in AddressStats percentage methods in another repo `dusts`
- [x] [PR #5](https://github.com/bubb1es71/ddust/pull/5) - Combining dust UTXOs with mempool ddust transactions

### In Progress
- **Sidharth**: [Issue #6](https://github.com/bubb1es71/ddust/issues/6)
- [Issue #8](https://github.com/bubb1es71/ddust/issues/8) - Document test scenarios and vectors

### Journal
Bubb1es reviewed my PR and suggested that i should have fewer commits without intermediate work like fixes to my own patches. I had to rebase, squash. We also reviewed the sizes of different scripts to ensure that we are adding the 3 bytes "ash" to the right script. The PR was merged and i moved to next task of documenting test vectors

### Next Steps
- bubb1es review of [PR #5](https://github.com/bubb1es71/ddust/pull/5) 
- Open issue for dust attack detection (Sidharth)
- Schedule sync with bubb1es

---

## [23.03.2026] - Test scenarios

### In Progress
- [Issue #8](https://github.com/bubb1es71/ddust/issues/8) - Add test scenarios
 
### Collaboration
- [x] Raised [PR #17](https://github.com/bubb1es71/ddust/pull/17): Setup TestEnv and add tests for add, list and spend scenarios. Nearly covers the entire code base
- [x] Reviewed Bubbl1es [PR #16](https://github.com/bubb1es71/dusts/pull/16) that returns simple output JSON values for dust list to enable `jq` on the output

### Journal
- We needed a way to enable testing `ddust` commands relying on `bitcoind`. Bubb1es can across this PR https://github.com/bitcoindevkit/rust-esplora-client/pull/176 that creates a small custom `TestEnv` instantiated per-test. This PR relies on `corepc` https://crates.io/crates/corepc-node for `bitcoind` node setup
- I went ahead and wrote a similar custom `TestEnv` wrapping `corepc_node::Node` with helpers for wallet creation, descriptor extraction, address generation, mining, multisig, and PSBT signing with `SIGHASH_ALL|ANYONECANPAY`. Our TestEnv didn't need `electrsd` and thus was relatively simpler
- I also added `TestContext` struct for shared test setup (regtest node with `-txindex` and `-dustrelayfee=0`). This is the minimal common setup that out tests would need
- Extracted command logic from `main()` match arms into standalone functions (`cmd_add`, `cmd_list`, `cmd_spend`, `cmd_broadcast`). This also enabled us to test our functionalities
- Added test scenarios:
  - **Add + List**: dust filtering across multiple address types (P2TR, P2WPKH, P2PKH)
  - **Add with start_height**: wallet only finds dust sent at or after the given block height
  - **Unconfirmed dust ignored**: cmd_list skips unconfirmed UTXOs
  - **Single non-witness spend**: Legacy and P2SH-SegWit produce empty OP_RETURN because the tx size is already sufficient for relaying
  - **Single witness spend**: Bech32m (P2TR) produces "ash" OP_RETURN because tx size is less than the minimum relay size
  - **Multiple UTXOs spend**: multiple inputs always produce empty OP_RETURN
  - **Multisig spend**: 2-of-2 P2SH multisig with both wallets signing
  - **Non-dust spend**: cmd_spend returns None when UTXO is above dust threshold i.e. there is no dust in the wallet
  - **Combine**: valid RBF combine preserves original OP_RETURN type (Empty and Ash), no-combine when fee rate is insufficient. The thing with combine feature is that it finds unconfirmed txs in the mempool and replaces it with a new tx with new dust UTXOs added, possible because of `SIGHASH_ALL|ANYONECANPAY`
- Updated GitHub Actions workflow to download bitcoind for CI test runs

### Next Steps
- Bubbl1es reviews the [PR #17](https://github.com/bubb1es71/ddust/pull/17) and merges it

---

### Ideas Backlog
| Feature                        | Description                                                                  | Comments                                                            |
|--------------------------------|------------------------------------------------------------------------------|---------------------------------------------------------------------|
| Staggered broadcast scheduling | Spread dust spends over time with random delays to reduce timing correlation |                                                                     |
| Dry run mode                   | Preview what would happen without broadcasting                               |                                                                     |
| Private broadcast              | Integrate `-privatebroadcast` flag (Bitcoin Core v31+)                       | bubb1es prefers that private broadcast should be left upto the user |
