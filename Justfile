set quiet := true
wallet := "test"
type := "bech32" # valid address types are: legacy, p2sh-segwit, bech32, bech32m
datadir := "data"
chain := "regtest" # valid networks are: main, test, testnet4, signet, regtest
logdir := if chain == "main" {
          "."
        } else if chain == "test" {
          "testnet"
        } else if chain == "testnet4" {
          "testnet4"
        } else if chain == "signet" {
          "signet"
        } else if chain == "regtest" {
          "regtest"
        } else {
          error("invalid chain: " + chain)
        }

# list of recipes
default:
  just --list
  echo "\nDefault variables:"
  just --evaluate

# format the project code
fmt:
    cargo fmt

# lint the project
clippy: fmt
    cargo clippy --tests

# build the project
build: clippy
    cargo build --tests

# test the project
test:
    cargo test --tests

# run the project
run *command: fmt
    cargo run -- -d {{datadir}} -c {{chain}} {{command}}

# clean the project target directory
clean:
    cargo clean

# start bitcoind in default data directory
[group('rpc')]
start:
    if [ ! -d "{{datadir}}" ]; then \
        mkdir -p "{{datadir}}"; \
    fi
    bitcoind -datadir={{datadir}} -chain={{chain}} -txindex -server -fallbackfee=0.0002 -blockfilterindex=1 -peerblockfilters=1 -rpcallowip=0.0.0.0/0 -rpcbind=0.0.0.0 -daemon

# stop bitcoind
[group('rpc')]
stop:
    -bitcoin-cli -datadir={{datadir}} -chain={{chain}} stop

# tail bitcoind debug.log
[group('rpc')]
debug:
    tail {{datadir}}/{{logdir}}/debug.log

# stop bitcoind and delete all data
[group('rpc')]
reset: stop
    #!/usr/bin/env bash
    set -euo pipefail
    echo "This will remove all bitcoind {{chain}} data!"
    read -p "Are you sure? (y/n) " response
    if [[ ! "$response" =~ ^[Yy]$ ]]; then
        echo "Aborted."
    else
      rm -rf {{datadir}}/{{chain}}
    fi

# create a new wallet
[group('rpc')]
create:
    bitcoin-cli -datadir={{datadir}} -chain={{chain}} createwallet {{wallet}}

# load a wallet
[group('rpc')]
load:
    bitcoin-cli -datadir={{datadir}} -chain={{chain}} loadwallet {{wallet}}

# unload a wallet
[group('rpc')]
unload:
    bitcoin-cli -datadir={{datadir}} -chain={{chain}} unloadwallet {{wallet}}

# get wallet address
[group('rpc')]
address:
    bitcoin-cli -datadir={{datadir}} -chain={{chain}} -rpcwallet={{wallet}} getnewaddress "" {{type}}

# generate n new blocks to given address
[group('rpc')]
generate n address:
    bitcoin-cli -datadir={{datadir}} -chain={{chain}} generatetoaddress {{n}} {{address}}

# get wallet balance
[group('rpc')]
balance:
    bitcoin-cli -datadir={{datadir}} -chain={{chain}} -rpcwallet={{wallet}} getbalance

# get wallet transaction details
[group('rpc')]
tx txid:
    bitcoin-cli -datadir={{datadir}} -chain={{chain}} -rpcwallet={{wallet}} gettransaction {{txid}}

# list wallet transactions
[group('rpc')]
txs:
    bitcoin-cli -datadir={{datadir}} -chain={{chain}} -rpcwallet={{wallet}} listtransactions

# list wallet utxos
[group('rpc')]
utxos:
    bitcoin-cli -datadir={{datadir}} -chain={{chain}} -rpcwallet={{wallet}} listunspent

# decode a psbt
[group('rpc')]
decode psbt:
    bitcoin-cli -datadir={{datadir}} -chain={{chain}} -rpcwallet={{wallet}} decodepsbt {{psbt}}

# send n btc to address from wallet
[group('rpc')]
send n address:
    bitcoin-cli -named -datadir={{datadir}} -chain={{chain}} -rpcwallet={{wallet}} sendtoaddress address={{address}} amount={{n}}

# list wallet descriptors info, private = (true | false)
[group('rpc')]
descriptors private:
    bitcoin-cli -datadir={{datadir}} -chain={{chain}} -rpcwallet={{wallet}} listdescriptors {{private}}

# sign a PSBT with the wallet
[group('rpc')]
sign psbt:
    bitcoin-cli -datadir={{datadir}} -chain={{chain}} -rpcwallet={{wallet}} walletprocesspsbt '{{psbt}}' true "ALL|ANYONECANPAY"
# run any bitcoin-cli rpc command
[group('rpc')]
rpc *command:
    bitcoin-cli -datadir={{datadir}} -chain={{chain}} -rpcwallet={{wallet}} {{command}}
