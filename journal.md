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

## [28.03.2026] - Test PR merged, BIP draft

### Merged
- [x] [PR #17](https://github.com/bubb1es71/ddust/pull/17) - Setup TestEnv and add tests for add, list and spend scenarios

### In Review
- [PR #20](https://github.com/bubb1es71/ddust/pull/20) - BIP draft for dust disposal protocol

### Collaboration
- [x] Reviewed bubb1es' [BIP draft](https://github.com/bubb1es71/ddust/pull/20) for the dust disposal protocol. Provided feedback on transaction size tables (incorrect values for P2PKH, missing P2WPKH and P2TR columns), and corrected the fee rate table
- [x] bubb1es shared the BIP update on Delving Bitcoin: [post #20](https://delvingbitcoin.org/t/disposing-of-dust-attack-utxos/2215/20)

### Journal
The test PR just merged,  paved the way for drafting a BIP. Having comprehensive test scenarios and a well-tested reference implementation gave us the confidence to formalize the protocol. Bubb1es wrote the initial BIP draft covering transaction format, signature conventions (`SIGHASH_ALL|ANYONECANPAY`), batching via RBF, and privacy considerations. I reviewed it and suggest few changes in the transaction size table and the fee rate table. A BIP would be a major step forward for the project - it turns a CLI tool into a standardized protocol that any wallet can implement. The BIP draft is open to any possible fixes, enhancement or feedback on [Delving Bitcoin](https://delvingbitcoin.org/t/disposing-of-dust-attack-utxos/2215/20)

As part of the Test Env setup, i also updated the docs for "Mempool batching"

### Next Steps
- Address BIP review feedback and iterate on [PR #20](https://github.com/bubb1es71/ddust/pull/20)
 
## [08.04.2026] - Transition to `NONE|ANYONECANPAY`, third-party batching (now obsolete)

### In Review and In Progress
- [PR #28](https://github.com/bubb1es71/ddust/pull/28) - Bubb1es working on [Issue #27](https://github.com/bubb1es71/ddust/issues/27) - using sighash `NONE|ANYONECANPAY` for ddust txs
- Starting working on
  - [Issue #26](https://github.com/bubb1es71/ddust/issues/26) - Broadcast transactions using `privatebroadcast`
  - [Issue #23](https://github.com/bubb1es71/ddust/issues/23) - Add more batching integration tests

### Collaboration
- [x] I worked on [Issue #24](https://github.com/bubb1es71/ddust/issues/24) - Add feature to batch unconfirmed ddust txs without adding any new inputs. Unfortunately this could not go into the code base because we found that BIP 125 rule #4 cannot be satisfied. More details under Journal section. Code [here](https://github.com/harismuzaffer/ddust/tree/feat/batch-without-input)
- [x] [PR #28](https://github.com/bubb1es71/ddust/pull/28) is currently in review but still in draft state. I am reviewing the PR and got involved in the discussion of transitioning ddust txs sighash from `SIGHASH_ALL|ANYONECANPAY` to `NONE|ANYONECANPAY`
- [x] Also check the thread starting from [Delving Bitcoin](https://delvingbitcoin.org/t/disposing-of-dust-attack-utxos/2215/20) and onwards

### Journal
Based on the discussion on [Delving Bitcoin](https://delvingbitcoin.org/t/disposing-of-dust-attack-utxos/2215/20), it was proposed to use sighash `NONE|ANYONECANPAY` instead of `SIGHASH_ALL|ANYONECANPAY` to let batched txs change the OP_RETURN data and drop the "ash" OP_RETURN padding, saving a few more bytes of block space. This transition doesn't add any spam vector. An attacker can't steal the sats and send them to their own address because RBF prevents it - the new tx would have a lower fee rate and lower absolute fee since the original tx paid the entire amount as fee. The attacker would have to add as many sats to the tx as they want to send to their address, hence the net profit would always be zero.

Another thing I was excited about was adding the feature that allows external aggregators to batch ddust txs without contributing any inputs. However, at the time of testing, I found that the replacement tx was rejected due to an "insufficient fee" error. I went back to read BIP 125 and it states that one of the RBF rules requires the replacement tx to pay for its own bandwidth at or above the rate set by the node's minimum relay fee setting - i.e., the replacement tx must pay both a higher fee rate (BTC/vbyte) and a higher absolute fee (total BTC). This is not possible without adding new inputs to the ddust tx because to satisfy "higher absolute fee" you need extra sats to cover that. Since ddust txs have no change output and the only output is an `OP_RETURN`, the only way to satisfy this rule is to add new inputs. Thus we had to abandon this feature.

I am also looking into the new feature of allowing broadcasting ddust txs using `privatebroadcast`. Note that `privatebroadcast` is available from [core 31.0](https://github.com/bitcoin-core/bitcoin-devwiki/wiki/31.0-Release-Notes-Draft#p2p-and-network-changes) and onwards, check [bitcoin/bitcoin#29415](https://github.com/bitcoin/bitcoin/pull/29415). `privatebroadcast` uses a new short-lived Tor or I2P connections to broadcast each transaction without linking it to the IP address of the sending node

### Next Steps
- Collaborate with Bubb1es on the new github [issues](https://github.com/bubb1es71/ddust/issues/)

---

## [16.04.2026] - RBF-compliant batching, revert to `ALL|ANYONECANPAY`

### Merged
- [x] [PR #28](https://github.com/bubb1es71/ddust/pull/28) - Bubb1es' sighash `NONE|ANYONECANPAY` transition (reviewed and merged)
- [x] Revert of PR #28 - reverted back to `ALL|ANYONECANPAY` based on Murch's feedback on the [bitcoindev mailing list](https://groups.google.com/g/bitcoindev/c/pr1z3_j8vTc/m/DqMYltO_AAAJ). The concern: `NONE|ANYONECANPAY` lets third parties scrape signed inputs from the mempool and use them to subsidize their own transactions for free. At current fee rates (~0.12 s/vB), each P2TR dust input is profitable to steal up to ~5.72 s/vB. This creates a replacement-war incentive and wastes relay bandwidth. `ALL|ANYONECANPAY` prevents this by locking the output to the OP_RETURN, so stolen inputs can only be spent to the same burn output

### In Progress
- [x] [Issue #30](https://github.com/bubb1es71/ddust/issues/30) - Fix batching to follow RBF rules. [PR](https://github.com/bubb1es71/ddust/pull/32) is ready for review.

### Journal
As part of the [PR](https://github.com/bubb1es71/ddust/pull/32), I refactored batching eligibility logic - instead of batching all-or-nothing, we can now batch a subset of unconfirmed ddust txs. The function sorts mempool txs ascending by fee rate and greedily includes each if the replacement satisfies all BIP 125 RBF rules: absolute fee >= sum of replaced fees (inherently satisfied since each input contributes > 0 sats), additional fee covers bandwidth (0.1 sat/vB * replacement vsize), replacement rate exceeds every replaced tx's rate, and no more than 100 evictions. The ascending sort + break-on-first-failure approach is deterministic - all compliant implementations produce the same batch for the same mempool state, preventing implementation fingerprinting.

Also refactored test fn `min_sats_for_batching` to correctly model both the rate and bandwidth RBF constraints, and updated batch tests to derive dust amounts from it.

Updated the BIP spec batching section to spell out the RBF rules and the deterministic sort order.

### Next Steps
- Work on open [issues](https://github.com/bubb1es71/ddust/issues/) in the ddust repo

---

## [29.04.2026] - Low-R signature grinding, BIP submitted to bitcoin/bips

### Merged
- [x] [PR #32](https://github.com/bubb1es71/ddust/pull/32) - [Issue #30](https://github.com/bubb1es71/ddust/issues/30) Fix batching to follow RBF rules
- [x] [PR #35](https://github.com/bubb1es71/ddust/pull/35) - [Issue #23](https://github.com/bubb1es71/ddust/issues/23) Assume low-R grinded ECDSA signatures (71 bytes) and update size calcs, fee-rate estimates, BIP tables; fix batching logic to account for the 1-byte empty witness-stack counter each legacy input incurs in a segwit tx; fix `min_sats_for_batching` to preserve the orig's OP_RETURN; refactor `find_batchable_txs`; add more batching combinations and multi-input replacement test scenarios

### In Review
- [Bitcoin BIPs PR #2150](https://github.com/bitcoin/bips/pull/2150) - The ddust BIP proposal raised on the upstream `bitcoin/bips` repo. Currently under review.

### Journal
PR #35 (Issue #23) was the bigger piece of work this cycle. ECDSA signatures in Bitcoin can vary in length depending on the random nonce `k` chosen during signing - the resulting `r` and `s` values can each be 32 or 33 bytes in DER encoding, leading to total signature sizes of 71-73 bytes. Bitcoin Core has been doing low-R grinding by default since 0.17, which retries the signing nonce until the high bit of `r` is 0, guaranteeing `r` is always 32 bytes. Combined with mandatory low-S enforcement (BIP146, since 2015), this makes signatures almost always 71 bytes (with the appended sighash byte). Since `ddust` only produces transactions through wallets that grind low-R, I updated all our size calculations to assume 71-byte sigs everywhere - the `TxSizeCalculator`, the BIP transaction-size and fee-rate tables, etc.

While doing that, I also discovered two related size-accounting bugs in `find_batchable_txs`:
- The replacement transaction is a segwit tx the moment any input is a witness program. Segwit serialization adds two costs over a pure-legacy tx: 0.5 vb for the per-tx marker+flag (paid once), and 0.25 vb per legacy input (the empty witness-stack counter byte). The earlier code only accounted for the marker+flag; legacy inputs in a segwit tx were being undercounted by 0.25 vb each, which in mixed cases produced replacements that fell below the BIP125 fee-rate threshold and got rejected by mempool.
- fixed the utils fns for test - `min_sats_for_batching` and `calculate()` to adjust the input sizes when a legacy input is encountered in a segwit tx

I also took the opportunity to refactor `find_batchable_txs` for readability. Logic unchanged.

Test side: extracted the common scaffolding from the batch tests into `run_batch_test`, which takes the original tx's input shape (a list of `(AddressType, InputType, Amount)`) and the replacement's input shape (a list of `(AddressType, InputType)`). Added all 9 single-input × single-batcher combinations across (Bech32m, Bech32, Legacy), then 6 multi-input scenarios on the original side. Each test now declares only the parameters that actually vary - addr1 type, input type, count, amount, addr2 type, input type, expected OP_RETURN.

On the BIP side: we raised PR #2150 in `bitcoin/bips`. Currently in review. The active discussion is whether to keep two OP_RETURN variants (empty for non-witness, "ash" for single-witness-input padding) or standardize on always-"ash". The reviewer's argument for always-"ash" is compelling: the 3-byte tax breaks even at a 1-in-7 merge rate (since each successful batch saves ~21 bytes by eliminating one tx's overhead), and it makes every ddust tx mergeable with every other instead of partitioning the mempool into two non-cross-mergeable pools. We're leaning toward accepting that simplification.


### Next Steps
- Address review feedback on [Bitcoin BIPs PR #2150](https://github.com/bitcoin/bips/pull/2150) - decide on always-"ash" vs two-variant OP_RETURN
- Continue working through open [issues](https://github.com/bubb1es71/ddust/issues/) in the ddust repo

---

### Ideas Backlog
| Feature                        | Description                                                                  | Comments                                             |
|--------------------------------|------------------------------------------------------------------------------|------------------------------------------------------|
| Staggered broadcast scheduling | Spread dust spends over time with random delays to reduce timing correlation |                                                      |
| Dry run mode                   | Preview what would happen without broadcasting                               |                                                      |
| Private broadcast              | Integrate `-privatebroadcast` flag (Bitcoin Core v31+)                       | [PR #20](https://github.com/bubb1es71/ddust/pull/20) |
