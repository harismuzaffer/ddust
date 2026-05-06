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
- **Mempool batching**: Automatically detects and batch unconfirmed ddust transactions via RBF

## Installation

```bash
cargo install --path .
```

## Usage

```bash
ddust --help
```

```
A simple tool that finds and spends dust UTXOs in a privacy-preserving way

Usage: ddust [OPTIONS] <COMMAND>

Commands:
  desc       List public key descriptors that will be scanned for dust UTXOs
  add        Add a public key descriptor to scan for dust UTXOs
  list       List all dust UTXOs in your wallet descriptor(s), returns json array
  spend      Spend dust UTXOs to an OP_RETURN, the entire amount goes to fees, returns PSBT
  broadcast  Broadcast a PSBT after it's been signed, returns txid
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

## Address public-key exposure

By default, `list` and `spend` skip dust UTXOs at any address that also holds an unspent non-dust UTXO. Disposing of the dust reveals the public key for that address, which exposes the unspent non-dust UTXOs to a hypothetical future long-term quantum attack.

Pass `--unsafe` to either command to override this check and operate on the dust anyway:

```bash
ddust list --unsafe
ddust spend <address> --unsafe
```

## Mempool Batching

When running `spend`, ddust scans the mempool for existing unconfirmed ddust transactions identified by a single OP_RETURN output and inputs signed with sighash `ALL|ANYONECANPAY`. If matching transactions are found, ddust checks whether batching them with the new dust inputs would satisfy RBF replacement rules: the combined fee rate must exceed the highest existing ddust transaction fee rate by at least 0.1 sat/vB.

If the check passes, ddust builds a single replacement transaction that:

1. Includes the new dust UTXOs as inputs
2. Adds the inputs from all matching unconfirmed ddust transactions as foreign UTXOs
3. Uses the OP_RETURN script from the first unconfirmed transaction (preserving its data)
4. Sets the total fee to the sum of all input amounts plus the fees from the replaced transactions

This RBF-based approach consolidates multiple pending dust disposals into one confirmed transaction, saving around 23 bytes of blockchain space per input, reducing on-chain footprint and ensuring the replacement fee rate is sufficient for acceptance by the network.

## Testing

The test suite requires a bitcoind binary to run integration tests against a regtest network.

### Setting up bitcoind

Before running tests, set the `BITCOIND_EXE` environment variable to point to your `bitcoind` binary:
```bash
export BITCOIND_EXE=/path/to/bitcoind
```

If you don't have `bitcoind` installed, download it from [bitcoincore.org](https://bitcoincore.org/en/download/):
```bash
# Example for Linux
wget https://bitcoincore.org/bin/bitcoin-core-31.0/bitcoin-31.0-x86_64-linux-gnu.tar.gz
tar xzf bitcoin-31.0-x86_64-linux-gnu.tar.gz
export BITCOIND_EXE=$PWD/bitcoin-31.0/bin/bitcoind
```

### Running tests

```bash
just test
```

## Requirements

- rust 1.92+
- a local tor proxy daemon
- a local bitcoin core node
  - version 31+
  - RPC enabled (e.g. "-rpcallowip=0.0.0.0/0 -rpcbind=0.0.0.0")
  - privatebroadcast enabled (e.g. "-privatebroadcast")
  - TOR proxy enabled (e.g. "-proxy=127.0.0.1:9050")
  - cookie file authentication
