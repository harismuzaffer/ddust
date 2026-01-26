use bdk_redb::Store;
use bdk_wallet::bitcoin::secp256k1::{All, Secp256k1};
use bdk_wallet::bitcoin::{
    Address, Amount, EcdsaSighashType, Network, Psbt, TapSighashType, Transaction,
};
use bdk_wallet::descriptor::ExtendedDescriptor;

use bdk_bitcoind_rpc::Emitter;
use bdk_bitcoind_rpc::bitcoincore_rpc::{Auth, Client, RpcApi};
use bdk_redb::redb::{Database, TableHandle};
use bdk_wallet::KeychainKind::Internal;
use bdk_wallet::bitcoin::absolute::LockTime;
use bdk_wallet::bitcoin::psbt::PsbtParseError;
use bdk_wallet::bitcoin::script::PushBytesBuf;
use bdk_wallet::chain::{CanonicalizationParams, CheckPoint};
use bdk_wallet::{PersistedWallet, Wallet, miniscript, wallet_name_from_descriptor};
use clap::{Parser, Subcommand, ValueEnum};
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::Arc;
use tracing::{Level, debug, error, info, trace};
use tracing_subscriber::FmtSubscriber;

fn main() {
    let args = CliArgs::parse();
    let log_level = match args.verbose {
        0 => Level::INFO,
        1 => Level::DEBUG,
        _ => Level::TRACE,
    };
    // a builder for `FmtSubscriber`.
    let subscriber = FmtSubscriber::builder()
        // all spans/events with a level higher than TRACE (e.g, debug, info, warn, etc.)
        // will be written to stdout.
        .with_max_level(log_level)
        // completes the builder.
        .finish();
    tracing::subscriber::set_global_default(subscriber).expect("setting default subscriber failed");

    let secp = Secp256k1::new();
    let network: Network = args.chain.into();
    let dust_amount = Amount::from_sat(args.amount);
    let db_file = args
        .datadir
        .join(format!("ddust-{}.redb", network.to_string().to_lowercase()));
    debug!("db file: {:?}", db_file);
    let db = Database::create(db_file).expect("failed to open database");
    let db = Arc::new(db);
    let url = default_url(&network);
    let auth = default_auth(&args.datadir, &network);
    match args.command {
        Commands::Add { desc } => {
            let wallet_name = wallet_name_from_descriptor(desc.clone(), None, network, &secp)
                .expect("must be a valid descriptor");

            if let (Some(mut wallet), mut store) = load_wallet(db.clone(), network, wallet_name) {
                sync_wallet(&url, &auth, &mut wallet, &mut store);
            } else {
                let wallets = add_descriptor(&secp, db, network, desc);
                wallets.into_iter().for_each(|(mut wallet, mut store)| {
                    sync_wallet(&url, &auth, &mut wallet, &mut store);
                })
            }
        }
        Commands::List => {
            for wallet_name in wallet_names(db.clone()) {
                info!("wallet: {}", wallet_name);
                if let (Some(mut wallet), mut store) =
                    load_wallet(db.clone(), network, wallet_name.clone())
                {
                    sync_wallet(&url, &auth, &mut wallet, &mut store);
                    wallet.list_unspent().for_each(|out| {
                        if !out.is_spent && out.txout.value <= dust_amount {
                            let address = Address::from_script(&out.txout.script_pubkey, network)
                                .expect("failed to get address");
                            let value = out.txout.value.display_dynamic();
                            info!(
                                "value: {}, address: {}, outpoint: {}:{}",
                                value, address, out.outpoint.txid, out.outpoint.vout
                            );
                        }
                    });
                } else {
                    error!("could not load wallet with name {}", wallet_name);
                }
            }
        }
        Commands::Spend { address } => {
            let filter_address = Address::from_str(&address)
                .expect("failed to parse filter address")
                .require_network(network)
                .expect("invalid network");
            for wallet_name in wallet_names(db.clone()) {
                info!("wallet: {}", wallet_name);
                if let (Some(mut wallet), mut store) =
                    load_wallet(db.clone(), network, wallet_name.clone())
                {
                    sync_wallet(&url, &auth, &mut wallet, &mut store);
                    let dust = wallet
                        .list_unspent()
                        .filter_map(|out| {
                            let out_address =
                                Address::from_script(&out.txout.script_pubkey, network)
                                    .expect("failed to get address");
                            if !out.is_spent
                                && out.txout.value <= dust_amount
                                && filter_address == out_address
                            {
                                Some(out)
                            } else {
                                None
                            }
                        })
                        .collect::<Vec<_>>();
                    if !dust.is_empty() {
                        let input_amount: Amount = dust.iter().map(|out| out.txout.value).sum();
                        debug!("fees: {}", &input_amount);
                        let utxos = dust.iter().map(|out| out.outpoint).collect::<Vec<_>>();
                        debug!("utxos: {:?}", &utxos);
                        let mut tx_builder = wallet.build_tx();
                        tx_builder
                            .nlocktime(LockTime::from_height(0).expect("valid height"))
                            .fee_absolute(input_amount)
                            .manually_selected_only()
                            .add_utxos(&utxos)
                            .expect("failed to add dust outpoints");

                        // add op_return with data if single P2WPKH input so Tx is 65vb
                        if dust.len() == 1 && dust[0].txout.script_pubkey.is_p2wpkh() {
                            let data = PushBytesBuf::try_from("ash".as_bytes().to_vec()).unwrap();
                            tx_builder.add_data(&data);
                        }
                        // otherwise op_return with no data
                        else {
                            let data = PushBytesBuf::from([]);
                            tx_builder.add_data(&data);
                        }

                        // set script type to ANYONECANPAY|ALL
                        if dust[0].txout.script_pubkey.is_p2tr() {
                            tx_builder.sighash(TapSighashType::AllPlusAnyoneCanPay.into());
                        } else {
                            tx_builder.sighash(EcdsaSighashType::AllPlusAnyoneCanPay.into());
                        }

                        let psbt = tx_builder.finish().expect("failed to create psbt");
                        info!("sign and broadcast tx for psbt: {}", psbt);
                    }
                } else {
                    error!("could not load wallet with name {}", wallet_name);
                }
            }
        }
        Commands::Broadcast { psbt } => {
            let rpc_client = Client::new(&url, auth.clone()).expect("failed to create rpc client");
            let tx = psbt
                .extract_tx()
                .expect("failed to extract transaction from PSBT");
            let txid = rpc_client
                .send_raw_transaction(&tx)
                .expect("failed to broadcast transaction");
            info!("transaction broadcast with txid: {}", txid);
        }
    }
}

/// A simple tool that finds and spends dust UTXOs in a privacy-preserving way
#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
pub struct CliArgs {
    /// Increase verbosity (-v, -vv, -vvv, etc.)
    #[arg(short, long, action = clap::ArgAction::Count)]
    verbose: u8,
    /// Directory to store wallet data
    #[arg(short, long="datadir", default_value = "data", env = "DDUST_DATADIR", value_parser = clap::value_parser!(PathBuf))]
    datadir: PathBuf,
    /// Bitcoin network
    #[arg(short, long="chain", default_value = "regtest", env = "DDUST_CHAIN", value_parser = clap::value_parser!(Chain))]
    chain: Chain,
    /// Maximum UTXO amount to treat as dust (in Sats)
    #[arg(short, long = "amount", default_value = "546", env = "DDUST_AMOUNT")]
    amount: u64,
    /// Fingerprint of descriptor, if not provided, all descriptors are used
    #[arg(short, long = "fingerprint", env = "DDUST_FINGERPRINT")]
    fingerprint: Option<String>,
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Add a public key descriptor to scan for dust UTXOs
    Add {
        /// Descriptor to add
        #[arg(value_parser = parse_descriptor)]
        desc: ExtendedDescriptor,
    },
    /// List all dust UTXOs in your wallet descriptor(s)
    List,
    /// Create a PSBT to spend dust UTXOs for an address to an OP_RETURN, the entire amount will go to fees
    Spend {
        /// Bitcoin address of dust to be spent
        address: String,
    },
    /// Broadcast a PSBT after it's been signed
    Broadcast {
        #[arg(value_parser = parse_psbt)]
        psbt: Psbt,
    },
}

#[derive(Debug, Clone, Copy, ValueEnum)]
#[value(rename_all = "lowercase")]
enum Chain {
    Main,
    Test,
    Testnet4,
    Signet,
    Regtest,
}

impl From<Chain> for Network {
    fn from(chain: Chain) -> Network {
        match chain {
            Chain::Main => Network::Bitcoin,
            Chain::Test => Network::Testnet,
            Chain::Testnet4 => Network::Testnet4,
            Chain::Signet => Network::Signet,
            Chain::Regtest => Network::Regtest,
        }
    }
}

fn parse_descriptor(s: &str) -> Result<ExtendedDescriptor, miniscript::Error> {
    let secp = Secp256k1::new();
    ExtendedDescriptor::parse_descriptor(&secp, s).map(|(desc, _)| desc)
}

fn parse_psbt(s: &str) -> Result<Psbt, PsbtParseError> {
    Psbt::from_str(s)
}

fn default_rpc_port(network: &Network) -> u16 {
    match network {
        Network::Bitcoin => 8332,
        Network::Testnet => 18332,
        Network::Testnet4 => 48332,
        Network::Signet => 38332,
        Network::Regtest => 18443,
    }
}

fn default_url(network: &Network) -> String {
    let port = default_rpc_port(network);
    format!("http://127.0.0.1:{}", port)
}

fn default_auth(data_dir: &Path, network: &Network) -> Auth {
    //Auth::UserPass("user".to_string(), "password".to_string())
    match network {
        Network::Bitcoin => Auth::CookieFile(data_dir.to_path_buf().join(".cookie")),
        Network::Testnet => {
            Auth::CookieFile(data_dir.to_path_buf().join("testnet").join(".cookie"))
        }
        Network::Testnet4 => {
            Auth::CookieFile(data_dir.to_path_buf().join("testnet4").join(".cookie"))
        }
        Network::Signet => Auth::CookieFile(data_dir.to_path_buf().join("signet").join(".cookie")),
        Network::Regtest => {
            Auth::CookieFile(data_dir.to_path_buf().join("regtest").join(".cookie"))
        }
    }
}

fn add_descriptor(
    secp: &Secp256k1<All>,
    db: Arc<Database>,
    network: Network,
    descriptor: ExtendedDescriptor,
) -> Vec<(PersistedWallet<Store>, Store)> {
    if descriptor.is_multipath() {
        let single_descriptors = descriptor
            .into_single_descriptors()
            .expect("must be multipath");
        single_descriptors
            .into_iter()
            .map(|desc| create_wallet(secp, db.clone(), network, desc))
            .collect()
    } else {
        vec![create_wallet(secp, db.clone(), network, descriptor)]
    }
}

fn load_wallet(
    db: Arc<Database>,
    network: Network,
    wallet_name: String,
) -> (Option<PersistedWallet<Store>>, Store) {
    let mut wallet_store = Store::new(db.clone(), wallet_name).expect("db store not created");
    let wallet = Wallet::load()
        .descriptor(Internal, None::<ExtendedDescriptor>)
        .check_network(network)
        .load_wallet(&mut wallet_store)
        .expect("unable to load wallet");
    (wallet, wallet_store)
}

fn create_wallet(
    secp: &Secp256k1<All>,
    db: Arc<Database>,
    network: Network,
    single_descriptor: ExtendedDescriptor,
) -> (PersistedWallet<Store>, Store) {
    let wallet_name = wallet_name_from_descriptor(single_descriptor.clone(), None, network, secp)
        .expect("must be a valid descriptor");
    let mut wallet_store = Store::new(db.clone(), wallet_name).expect("db store not created");
    let wallet = Wallet::create_single(single_descriptor)
        .network(network)
        .create_wallet(&mut wallet_store)
        .expect("unable to create wallet");
    (wallet, wallet_store)
}

fn sync_wallet(url: &str, auth: &Auth, wallet: &mut PersistedWallet<Store>, store: &mut Store) {
    let rpc_client = Client::new(url, auth.clone()).expect("failed to create rpc client");
    let blockchain_info = rpc_client.get_blockchain_info().unwrap();
    info!(
        "connected to bitcoin core rpc, chain: {}",
        blockchain_info.chain
    );
    debug!(
        "latest block: {} at height {}",
        blockchain_info.best_block_hash, blockchain_info.blocks,
    );

    let wallet_tip: CheckPoint = wallet.latest_checkpoint();
    debug!(
        "current wallet tip is: {} at height {}",
        &wallet_tip.hash(),
        &wallet_tip.height()
    );

    // reload the last 200 blocks in case of a reorg
    let emitter_height = wallet_tip.height().saturating_sub(200);
    let expected_mempool_tx = wallet
        .tx_graph()
        .list_canonical_txs(
            wallet.local_chain(),
            wallet.local_chain().tip().block_id(),
            CanonicalizationParams::default(),
        )
        .filter(|tx| tx.chain_position.is_unconfirmed());
    let mut emitter = Emitter::new(
        &rpc_client,
        wallet_tip.clone(),
        emitter_height,
        expected_mempool_tx,
    );

    info!("syncing blocks...");
    while let Some(block) = emitter.next_block().unwrap() {
        wallet
            .apply_block_connected_to(&block.block, block.block_height(), block.connected_to())
            .unwrap();
        let percent_done =
            f64::from(block.block_height()) / f64::from(blockchain_info.headers as u32) * 100f64;
        trace!(
            "applied blocks to height: {}, {:.2}% done",
            block.block_height(),
            percent_done
        );
        if block.block_height() % 10_000 == 0 {
            debug!(
                "persisting blocks to height: {}, {:.2}% done",
                block.block_height(),
                percent_done
            );
            wallet.persist(store).expect("unable to persist wallet");
        }
    }

    info!("syncing mempool...");
    let mempool_emissions: Vec<(Arc<Transaction>, u64)> = emitter.mempool().unwrap().update;
    wallet.apply_unconfirmed_txs(mempool_emissions);
    wallet.persist(store).expect("unable to persist wallet");
}

fn wallet_names(db: Arc<Database>) -> Vec<String> {
    let read_tx = db.begin_read().expect("failed to begin read");
    let tables = read_tx.list_tables().expect("failed to list tables");
    tables
        .filter_map(|table| {
            let name = table.name().to_string();
            name.strip_suffix("_keychain").map(|name| name.to_string())
        })
        .collect::<Vec<String>>()
}
