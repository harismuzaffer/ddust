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

### Contributions Proposed
- Dust attack detection logic
- Transaction combining via `SIGHASH_ANYONECANPAY` (discussed in the thread)
- Combining dust UTXOs with mempool ddust transactions

### Next Steps
- Reach out to bubb1es and explore collaboration

---

## [02.03.2026] - Collaboration Established, First PRs

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

### Next Steps
- bubb1es review of PR #5
- Open issue for dust attack detection (Sidharth)
- Schedule sync with bubb1es

### Ideas Backlog
| Feature                        | Description                                                                  | Comments                                                            |
|--------------------------------|------------------------------------------------------------------------------|---------------------------------------------------------------------|
| Staggered broadcast scheduling | Spread dust spends over time with random delays to reduce timing correlation |                                                                     |
| Dry run mode                   | Preview what would happen without broadcasting                               |                                                                     |
| Private broadcast              | Integrate `-privatebroadcast` flag (Bitcoin Core v31+)                       | bubb1es prefers that private broadcast should be left upto the user |
