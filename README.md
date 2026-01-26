[![Rust](https://github.com/bubb1es71/ddust/actions/workflows/rust.yaml/badge.svg)](https://github.com/bubb1es71/ddust/actions/workflows/rust.yaml)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://choosealicense.com/licenses/mit/)

## Overview

The `ddust` app is an experimental tool to find and safely dispose of “dust attack” UTXOs in on-chain, descriptor-based wallets. 

A "dust attack" is when an adversary sends dust amount UTXOs to all the anonymous on-chain addresses they are interested in de-anonymizing and hope that some of these will be unintentionally spent together with unrelated UTXOs. (see [Output linking | Bitcoin Optech](https://bitcoinops.org/en/topics/output-linking/))

The `ddust` tool creates small, low-fee, stand-alone transactions that spend dust UTXO inputs (to the same address) to an OP_RETURN output. This prevents dust UTXOs from being accidentally spent with other wallet UTXOs.

## Motivation

Most modern wallets automatically lock dust amount UTXOs so they are never spent. This solves the problem but has a cost and future risks. The cost is the bloating of the mempool with UTXOs that will never be spent. Risks are that a wallet software bug, restore from keys, or migration to a new wallet could “unlock” the dust. There is also a risk that a future wallet owner (such a with inheritance) could misunderstand the reason the dust is locked and spend it. In any case there are risks in the future of accidentally de-anonymizing the wallet.

Now that the default minimum relay fee rate (as of core 30) has been lowered to 0.1 sats/vB it is possible to spend previously unspendable dust UTXOs. This is done by creating a small transaction that uses the entire dust UTXO amount for fees and has an OP_RETURN output. The transaction only needs to be at least the minimum relay size of 65 bytes and have a fee > 0.1 sats/vb.

## Features

- **Descriptor-based**: Add Bitcoin public key descriptors to monitor addresses for dust
- **Multi-network support**: Works with mainnet, testnet, testnet4, signet, and regtest
- **Privacy-focused**: Only consolidates dust size UTXOs sent to the same wallet address
- **Persistent storage**: Uses redb for efficient wallet state management
- **Bitcoin Core integration**: Syncs directly with Bitcoin Core via RPC

## Installation

```bash
cargo install --path .
```

## Usage

```bash
ddust --help
```

```
Usage: ddust [OPTIONS] <COMMAND>

Commands:
  add        Add a public key descriptor to scan for dust UTXOs
  list       List all dust UTXOs in your wallet descriptor(s)
  spend      Create a PSBT to spend dust UTXOs for an address to an OP_RETURN, the entire amount will go to fees
  broadcast  Broadcast a PSBT after it's been signed
  help       Print this message or the help of the given subcommand(s)

Options:
  -v, --verbose...                 Increase verbosity (-v, -vv, -vvv, etc.)
  -d, --datadir <DATADIR>          Directory to store wallet data [env: DDUST_DATADIR=] [default: data]
  -c, --chain <CHAIN>              Bitcoin network [env: DDUST_CHAIN=] [default: regtest] [possible values: main, test, testnet4, signet, regtest]
  -a, --amount <AMOUNT>            Maximum UTXO amount to treat as dust (in Sats) [env: DDUST_AMOUNT=] [default: 546]
  -f, --fingerprint <FINGERPRINT>  Fingerprint of descriptor, if not provided, all descriptors are used [env: DDUST_FINGERPRINT=]
  -h, --help                       Print help
  -V, --version                    Print version
```

## Requirements

- rust 1.92+
- a local bitcoin core node
  - version 30+
  - RPC enabled
  - cookie file authentication
