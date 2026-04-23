use bdk_redb::Store;
use bdk_wallet::bitcoin::secp256k1::{All, Secp256k1};
use bdk_wallet::bitcoin::{
    Address, Amount, EcdsaSighashType, Network, OutPoint, Psbt, ScriptBuf, Sequence,
    TapSighashType, Transaction, TxIn,
};
use bdk_wallet::coin_selection::DefaultCoinSelectionAlgorithm;
use bdk_wallet::descriptor::ExtendedDescriptor;

use bdk_bitcoind_rpc::Emitter;
use bdk_bitcoind_rpc::bitcoincore_rpc::{Auth, Client, RpcApi};
use bdk_redb::redb::{Database, TableHandle};
use bdk_wallet::KeychainKind::Internal;
use bdk_wallet::bitcoin::absolute::LockTime;
use bdk_wallet::bitcoin::ecdsa::Signature;
use bdk_wallet::bitcoin::psbt::Input;
use bdk_wallet::bitcoin::psbt::PsbtParseError;
use bdk_wallet::bitcoin::script::Instruction;
use bdk_wallet::bitcoin::script::PushBytesBuf;
use bdk_wallet::chain::{CanonicalizationParams, CheckPoint};
use bdk_wallet::serde::Serialize;
use bdk_wallet::{LocalOutput, PersistedWallet, Wallet, miniscript, wallet_name_from_descriptor};
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
        // will be written to stderr.
        .with_max_level(log_level)
        .with_writer(std::io::stderr)
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
    let rpc_client = Client::new(&url, auth.clone()).expect("failed to create rpc client");

    match args.command {
        Commands::Add { desc, start_height } => {
            cmd_add(&secp, &db, network, &rpc_client, desc, start_height);
        }
        Commands::List => {
            let dust = cmd_list(&db, network, &rpc_client, dust_amount);
            println!("{}", serde_json::to_string_pretty(&dust).unwrap());
        }
        Commands::Spend { address } => {
            let filter_address = Address::from_str(&address)
                .expect("failed to parse filter address")
                .require_network(network)
                .expect("invalid network");
            if let Some(psbt) = cmd_spend(&db, network, &rpc_client, dust_amount, filter_address) {
                println!("{}", psbt);
            }
        }
        Commands::Broadcast { psbt } => {
            let txid = cmd_broadcast(&rpc_client, psbt);
            println!("{}", txid);
        }
    }
}

fn cmd_add(
    secp: &Secp256k1<All>,
    db: &Arc<Database>,
    network: Network,
    rpc_client: &Client,
    desc: ExtendedDescriptor,
    start_height: u32,
) {
    let wallet_name = wallet_name_from_descriptor(desc.clone(), None, network, secp)
        .expect("must be a valid descriptor");

    if let (Some(mut wallet), mut store) = load_wallet(db.clone(), network, wallet_name) {
        sync_wallet(rpc_client, &mut wallet, &mut store);
    } else {
        let wallets = add_descriptor(secp, db.clone(), network, desc, start_height, rpc_client);
        wallets.into_iter().for_each(|(mut wallet, mut store)| {
            sync_wallet(rpc_client, &mut wallet, &mut store);
        });
    }
}

fn cmd_list(
    db: &Arc<Database>,
    network: Network,
    rpc_client: &Client,
    dust_amount: Amount,
) -> Vec<Dust> {
    let mut found_dust = Vec::new();
    for wallet_name in wallet_names(db.clone()) {
        debug!("wallet: {}", wallet_name);
        if let (Some(mut wallet), mut store) = load_wallet(db.clone(), network, wallet_name.clone())
        {
            sync_wallet(rpc_client, &mut wallet, &mut store);
            wallet.list_unspent().for_each(|out| {
                if is_dust(&out, &dust_amount) {
                    let address = Address::from_script(&out.txout.script_pubkey, network)
                        .expect("failed to get address")
                        .to_string();
                    let value = out.txout.value.to_sat() as u32;
                    found_dust.push(Dust {
                        address,
                        value,
                        outpoint: out.outpoint,
                    });
                }
            });
        } else {
            error!("could not load wallet with name {}", wallet_name);
        }
    }
    found_dust
}

fn cmd_spend(
    db: &Arc<Database>,
    network: Network,
    rpc_client: &Client,
    dust_amount: Amount,
    filter_address: Address,
) -> Option<Psbt> {
    for wallet_name in wallet_names(db.clone()) {
        debug!("wallet: {}", wallet_name);
        if let (Some(mut wallet), mut store) = load_wallet(db.clone(), network, wallet_name.clone())
        {
            sync_wallet(rpc_client, &mut wallet, &mut store);
            let dust = wallet
                .list_unspent()
                .filter_map(|out| {
                    let out_address = Address::from_script(&out.txout.script_pubkey, network)
                        .expect("failed to get address");
                    if is_dust(&out, &dust_amount) && filter_address == out_address {
                        Some(out)
                    } else {
                        None
                    }
                })
                .collect::<Vec<_>>();
            debug!("dust: {:?}", dust);
            if !dust.is_empty() {
                let mut input_amount: Amount = dust.iter().map(|out| out.txout.value).sum();
                let utxos = dust.iter().map(|out| out.outpoint).collect::<Vec<_>>();
                let unconfirmed_txs = find_unconfirmed_ddust_txs(rpc_client);
                debug!("found {} unconfirmed ddust txs", unconfirmed_txs.len());

                let mut tx_builder = wallet.build_tx();
                tx_builder
                    .nlocktime(LockTime::from_height(0).expect("valid height"))
                    .set_exact_sequence(Sequence::MAX)
                    .manually_selected_only()
                    .add_utxos(&utxos)
                    .expect("failed to add dust outpoints");

                let batchable_txs = find_batchable_txs(rpc_client, &dust, &unconfirmed_txs);
                if !batchable_txs.is_empty() {
                    debug!(
                        "batching {} of {} unconfirmed txs",
                        batchable_txs.len(),
                        unconfirmed_txs.len()
                    );
                    input_amount += add_foreign_utxos(rpc_client, &mut tx_builder, &batchable_txs);
                }

                info!("total spent to fees: {}", &input_amount);
                tx_builder.fee_absolute(input_amount);

                if !batchable_txs.is_empty() {
                    // the new tx shall use the data found in the unconfirmed txs
                    let suggested_script = &batchable_txs[0].output[0].script_pubkey;
                    let op_return = match suggested_script.as_bytes() {
                        // empty OP_RETURN no data
                        [0x6a, 0x00] => vec![],
                        // skip 0x6a (OP_RETURN) and push byte
                        [0x6a, _, rest @ ..] => rest.to_vec(),
                        _ => vec![],
                    };
                    let data = PushBytesBuf::try_from(op_return).unwrap();
                    tx_builder.add_data(&data);
                } else {
                    // add op_return with data if single witness input, so Tx is 65vb
                    if dust.len() == 1 && dust[0].txout.script_pubkey.is_witness_program() {
                        let data = PushBytesBuf::try_from("ash".as_bytes().to_vec()).unwrap();
                        tx_builder.add_data(&data);
                    } else {
                        let data = PushBytesBuf::try_from(vec![]).unwrap();
                        tx_builder.add_data(&data);
                    }
                }

                // set script type to ALL|ANYONECANPAY
                if dust[0].txout.script_pubkey.is_p2tr() {
                    tx_builder.sighash(TapSighashType::AllPlusAnyoneCanPay.into());
                } else {
                    tx_builder.sighash(EcdsaSighashType::AllPlusAnyoneCanPay.into());
                }

                let psbt = tx_builder.finish().expect("failed to create psbt");
                return Some(psbt);
            }
        } else {
            error!("could not load wallet with name {}", wallet_name);
        }
    }
    None
}

fn cmd_broadcast(rpc_client: &Client, psbt: Psbt) -> bdk_wallet::bitcoin::Txid {
    let tx = psbt
        .extract_tx()
        .expect("failed to extract transaction from PSBT");
    rpc_client
        .send_raw_transaction(&tx)
        .expect("failed to broadcast transaction")
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
        /// Block height to start scanning for transactions
        #[arg(short, long, default_value_t = 0)]
        start_height: u32,
    },
    /// List all dust UTXOs in your wallet descriptor(s), returns json array
    List,
    /// Spend dust UTXOs to an OP_RETURN, the entire amount goes to fees, returns PSBT
    Spend {
        /// Bitcoin address of dust to be spent
        address: String,
    },
    /// Broadcast a PSBT after it's been signed, returns txid
    Broadcast {
        #[arg(value_parser = parse_psbt)]
        psbt: Psbt,
    },
}

#[derive(Serialize)]
struct Dust {
    pub address: String,
    pub value: u32,
    pub outpoint: OutPoint,
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
    start_height: u32,
    rpc_client: &Client,
) -> Vec<(PersistedWallet<Store>, Store)> {
    if descriptor.is_multipath() {
        let single_descriptors = descriptor
            .into_single_descriptors()
            .expect("must be multipath");
        single_descriptors
            .into_iter()
            .map(|desc| create_wallet(secp, db.clone(), network, desc, start_height, rpc_client))
            .collect()
    } else {
        vec![create_wallet(
            secp,
            db.clone(),
            network,
            descriptor,
            start_height,
            rpc_client,
        )]
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
    start_height: u32,
    rpc_client: &Client,
) -> (PersistedWallet<Store>, Store) {
    let wallet_name = wallet_name_from_descriptor(single_descriptor.clone(), None, network, secp)
        .expect("must be a valid descriptor");
    let mut wallet_store = Store::new(db.clone(), wallet_name).expect("db store not created");
    let mut wallet = Wallet::create_single(single_descriptor)
        .network(network)
        .create_wallet(&mut wallet_store)
        .expect("unable to create wallet");
    if start_height > 0 {
        let genesis_hash = rpc_client
            .get_block_hash(0)
            .expect("failed to get genesis block hash");
        let start_hash = rpc_client
            .get_block_hash(start_height as u64)
            .expect("failed to get start block hash");
        let start_block = rpc_client
            .get_block(&start_hash)
            .expect("failed to get start block");
        wallet
            .apply_block_connected_to(&start_block, start_height, (0, genesis_hash).into())
            .expect("failed to apply start block");
    }
    (wallet, wallet_store)
}

fn sync_wallet(rpc_client: &Client, wallet: &mut PersistedWallet<Store>, store: &mut Store) {
    let blockchain_info = rpc_client.get_blockchain_info().unwrap();
    debug!(
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
        rpc_client,
        wallet_tip.clone(),
        emitter_height,
        expected_mempool_tx,
    );

    debug!("syncing blocks...");
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
            info!(
                "persisting blocks to height: {}, {:.2}% done",
                block.block_height(),
                percent_done
            );
            wallet.persist(store).expect("unable to persist wallet");
        }
    }

    debug!("syncing mempool...");
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

fn find_unconfirmed_ddust_txs(rpc_client: &Client) -> Vec<Transaction> {
    let tx_ids = rpc_client
        .get_raw_mempool()
        .expect("failed to get mempool transaction IDs");
    let mut unconfirmed_txs: Vec<Transaction> = vec![];
    // the batch tx shall use the first unconfirmed tx's script data
    let mut first_found_script: Option<Vec<u8>> = None;

    // find txs in the mempool that match ddust pattern
    for txid in tx_ids {
        let tx = rpc_client.get_raw_transaction(&txid, None).unwrap();
        if is_ddust_tx(&tx, &first_found_script) {
            if first_found_script.is_none() {
                let script_bytes = tx.output[0].script_pubkey.as_bytes().to_vec();
                first_found_script = Some(script_bytes);
            }

            unconfirmed_txs.push(tx);
        }
    }

    unconfirmed_txs
}

/// Adds pre-signed inputs from unconfirmed ddust transactions as foreign UTXOs
/// to the given tx_builder. Returns the total amount added.
fn add_foreign_utxos(
    rpc_client: &Client,
    tx_builder: &mut bdk_wallet::TxBuilder<'_, DefaultCoinSelectionAlgorithm>,
    unconfirmed_txs: &[Transaction],
) -> Amount {
    let mut added_amount = Amount::ZERO;
    for tx in unconfirmed_txs {
        for input in &tx.input {
            let f_outpoint = input.previous_output;
            let f_input_prev_tx = rpc_client
                .get_raw_transaction(&f_outpoint.txid, None)
                .unwrap();
            let f_prev_txout = f_input_prev_tx.output[f_outpoint.vout as usize].clone();

            added_amount += f_prev_txout.value;

            let mut f_psbt_input = Input::default();
            // p2tr sighash algorithm commits to all input amounts, thus
            // non_witness_utxo is not needed to verify the input value
            if f_prev_txout.script_pubkey.is_p2tr() {
                f_psbt_input.witness_utxo = Some(f_prev_txout);
            } else {
                f_psbt_input.non_witness_utxo = Some(f_input_prev_tx.clone());
            }
            if !input.witness.is_empty() {
                f_psbt_input.final_script_witness = Some(input.witness.clone());
            }
            if !input.script_sig.is_empty() {
                f_psbt_input.final_script_sig = Some(input.script_sig.clone());
            }
            tx_builder
                .add_foreign_utxo_with_sequence(
                    f_outpoint,
                    f_psbt_input,
                    input.segwit_weight(),
                    input.sequence,
                )
                .unwrap_or_else(|_| {
                    panic!("failed to add the foreign UTXO. Outpoint: {}", f_outpoint)
                });
        }
    }
    added_amount
}

/// ddust pattern:
/// has a single op_return
/// one or more inputs with ALL|ANYONECANPAY signature type
/// op_return: can be empty or contains the string "ash"
fn is_ddust_tx(tx: &Transaction, want_script: &Option<Vec<u8>>) -> bool {
    // Must have exactly one output
    if tx.output.len() != 1 {
        return false;
    }

    // Must be OP_RETURN
    let script = &tx.output[0].script_pubkey;
    if !script.is_op_return() {
        return false;
    }

    // Must be empty OP_RETURN or "ash"
    let script_bytes = script.as_bytes();
    let is_dust_disposal = if let Some(existing_script) = want_script {
        script_bytes == existing_script.as_slice()
    } else {
        script_bytes == [0x6a, 0x00] || script_bytes == [0x6a, 0x03, 0x61, 0x73, 0x68]
    };

    if !is_dust_disposal {
        return false;
    }

    // All inputs must be ALL|ANYONECANPAY
    for input in &tx.input {
        if !input.witness.is_empty() {
            // If a segwit input check the witness sighash byte
            let sig = input.witness.nth(0).unwrap();
            match sig.len() {
                // Taproot with explicit sighash
                65 => {
                    if sig[64] != TapSighashType::AllPlusAnyoneCanPay as u8 {
                        return false;
                    }
                }
                // ECDSA (P2WPKH/P2WSH)
                71..=73 => {
                    if *sig.last().unwrap() != EcdsaSighashType::AllPlusAnyoneCanPay as u8 {
                        return false;
                    }
                }
                // Taproot default sighash (64 bytes) or unknown
                _ => return false,
            }
        }
        // If a legacy, input check the script sig sighash byte
        else if input.script_sig.is_p2pkh() || input.script_sig.is_p2sh() {
            for instruction in input.script_sig.instructions() {
                if let Ok(Instruction::PushBytes(data)) = instruction
                    && let Ok(sig) = Signature::from_slice(data.as_bytes())
                    && sig.sighash_type != EcdsaSighashType::AllPlusAnyoneCanPay
                {
                    return false;
                }
            }
        }
    }
    true
}

fn get_input_vsize(input: &TxIn) -> f64 {
    let weight = input.base_size() * 3 + input.total_size();
    weight as f64 / 4.0
}

fn estimate_input_vsize(script_pubkey: &ScriptBuf) -> f64 {
    if script_pubkey.is_p2tr() {
        57.75
    } else if script_pubkey.is_p2wpkh() {
        68.0
    } else if script_pubkey.is_p2wsh() {
        // 2-of-3 multisig estimate
        105.0
    } else if script_pubkey.is_p2pkh() {
        148.0
    } else if script_pubkey.is_p2sh() {
        // Could be P2SH-P2WPKH (~364 WU)
        // Could be P2SH-P2WSH (~478 WU for 2-of-3)
        // Could be bare P2SH multisig (~1188 WU for 2-of-3)
        // Can't tell from scriptPubKey alone, use worst case
        297.0
    } else {
        panic!("Unsupported input encountered");
    }
}

/// Returns true if `LocalOutput` is not spent, under the dust amount threshold, and is confirmed.
fn is_dust(out: &LocalOutput, dust_amount: &Amount) -> bool {
    !out.is_spent && out.txout.value <= *dust_amount && out.chain_position.is_confirmed()
}

/// Selects unconfirmed ddust transactions that can be batched into a single RBF-compliant replacement.
/// `dust_utxos` are extra inputs added by the batcher.
///
/// Replacement fee:
/// new_fee = sum(dust_utxos) + sum(replaced_fees)
///
/// A transaction is included only if:
///
/// * Total fee pays at least the absolute fee of all replaced txs. This is gauranteed since each tx contributes > 0 sats
/// * Replacement tx pays for its own bandwidth i.e. Added fee > 0.1 × replacement_vsize
/// * Replacement feerate exceeds all included txs (processed in ascending order)
/// * Total replaced txs stay within the mempool eviction limit (100)
fn find_batchable_txs(
    rpc_client: &Client,
    dust_utxos: &[LocalOutput],
    unconfirmed_txs: &[Transaction],
) -> Vec<Transaction> {
    if unconfirmed_txs.is_empty() || dust_utxos.is_empty() {
        return vec![];
    }
    let output_script = &unconfirmed_txs[0].output[0].script_pubkey;

    let mut batchable: Vec<Transaction> = vec![];

    // replacement tx vsize
    // overhead
    let mut tx_vsize = 10.5;
    tx_vsize += dust_utxos
        .iter()
        .map(|utxo| estimate_input_vsize(&utxo.txout.script_pubkey))
        .sum::<f64>();

    tx_vsize += match output_script.as_bytes() {
        // empty OP_RETURN no data, size = 11
        [0x6a, 0x00] => 11.0,
        // contains 3 bytes 'ash', size = 14
        _ => 14.0,
    };

    let tx_input_amount: Amount = dust_utxos.iter().map(|out| out.txout.value).sum();
    let dust_amount_sats = tx_input_amount.to_sat() as f64;

    // collect fee, rate and input vsize for each unconfirmed tx
    let mut tx_info: Vec<(Transaction, Amount, f64, f64)> = unconfirmed_txs
        .iter()
        .map(|tx| {
            let entry = rpc_client.get_mempool_entry(&tx.compute_txid()).unwrap();
            let fee = entry.fees.base;
            let vsize = entry.vsize as f64;
            let rate = fee.to_sat() as f64 / vsize;
            let input_vsize: f64 = tx.input.iter().map(get_input_vsize).sum();
            (tx.clone(), fee, rate, input_vsize)
        })
        .collect();

    // sort ascending by fee rate so the rate check against the current iteration's rate
    // covers all previously accepted txs (their rates are <= current).
    tx_info.sort_by(|a, b| a.2.partial_cmp(&b.2).unwrap());

    // mempool eviction cap
    const MAX_REPLACED: usize = 100;

    let mut combined_vsize: f64 = tx_vsize;
    let mut combined_amount: Amount = tx_input_amount;

    for (tx, fee, rate, input_vsize) in tx_info {
        if batchable.len() >= MAX_REPLACED {
            debug!("batchable: hit eviction cap of {} txs", MAX_REPLACED);
            break;
        }
        let new_amount = combined_amount + fee;
        let new_vsize = combined_vsize + input_vsize;
        let new_rate = new_amount.to_sat() as f64 / new_vsize;

        // additional fee (dust_amount_sats) must cover incremental relay over new vsize.
        let bandwidth_ok = dust_amount_sats >= 0.1 * new_vsize;
        // replacement rate must exceed every replaced tx's rate.
        let rate_ok = new_rate > rate;

        if bandwidth_ok && rate_ok {
            debug!(
                "batchable: adding tx (rate {:.3}, combined rate {:.3}, combined vsize {:.1})",
                rate, new_rate, new_vsize
            );
            combined_amount = new_amount;
            combined_vsize = new_vsize;
            batchable.push(tx);
        } else {
            // sorted ascending by rate, so subsequent txs have rate >= this one (rate check
            // gets harder) and add at least some vsize (bandwidth check gets harder).
            debug!(
                "batchable: stopping at rate {:.3} (bandwidth_ok={}, rate_ok={}, combined rate {:.3}, combined vsize {:.1})",
                rate, bandwidth_ok, rate_ok, new_rate, new_vsize
            );
            break;
        }
    }

    batchable
}

#[cfg(test)]
mod test_calc;

#[cfg(test)]
mod test_env;

#[cfg(test)]
mod tests {
    use crate::test_calc::{InputType, TxSizeCalculator};

    use super::*;
    use corepc_node::AddressType;
    use test_env::TestEnv;

    enum OpReturn {
        Empty,
        Ash,
    }

    impl OpReturn {
        fn as_script(&self) -> ScriptBuf {
            let data = match self {
                OpReturn::Empty => PushBytesBuf::new(),
                OpReturn::Ash => PushBytesBuf::try_from(b"ash".to_vec()).unwrap(),
            };
            ScriptBuf::new_op_return(data)
        }
    }

    struct TestContext {
        env: TestEnv,
        db: Arc<Database>,
        rpc_client: Client,
        secp: Secp256k1<All>,
        network: Network,
        wallet1_name: String,
        wallet2_name: String,
    }

    impl TestContext {
        fn new() -> Self {
            let mut conf = corepc_node::Conf::default();
            conf.args.push("-txindex");
            // allows sending small amount of sats
            conf.args.push("-dustrelayfee=0");
            let env = TestEnv::new_with_conf(conf);
            let network = Network::Regtest;
            let db_file = env
                .node
                .workdir()
                .join(format!("ddust-{}.redb", network.to_string().to_lowercase()));
            let db = Arc::new(Database::create(db_file).expect("failed to open database"));
            let rpc_client = env.rpc_client();
            let secp = Secp256k1::new();
            let wallet1_name = "wallet_1".to_string();
            env.create_wallet(&wallet1_name);
            let wallet2_name = "wallet_2".to_string();
            env.create_wallet(&wallet2_name);
            Self {
                env,
                db,
                rpc_client,
                secp,
                network,
                wallet1_name,
                wallet2_name,
            }
        }
    }

    /// Add descriptors for multiple address types, send dust and non-dust UTXOs,
    /// verify cmd_list only returns UTXOs at or below the dust threshold.
    #[test]
    fn test_cmd_add_list() {
        let ctx = TestContext::new();

        let desc = ctx
            .env
            .get_descriptor(&ctx.wallet1_name, &AddressType::Bech32m);
        cmd_add(&ctx.secp, &ctx.db, ctx.network, &ctx.rpc_client, desc, 0);
        let desc = ctx
            .env
            .get_descriptor(&ctx.wallet1_name, &AddressType::Bech32);
        cmd_add(&ctx.secp, &ctx.db, ctx.network, &ctx.rpc_client, desc, 0);
        let desc = ctx
            .env
            .get_descriptor(&ctx.wallet1_name, &AddressType::Legacy);
        cmd_add(&ctx.secp, &ctx.db, ctx.network, &ctx.rpc_client, desc, 0);
        let dust_sats = Amount::from_sat(555);

        let addr1 = ctx
            .env
            .new_address(&ctx.wallet1_name, &AddressType::Bech32m);
        // dust UTXO 1
        ctx.env.send_to_address(&addr1, Amount::from_sat(400));
        // non dust UTXO
        ctx.env.send_to_address(&addr1, Amount::from_sat(700));
        // dust UTXO 2
        ctx.env.send_to_address(&addr1, Amount::from_sat(401));
        let addr2 = ctx.env.new_address(&ctx.wallet1_name, &AddressType::Bech32);
        // dust UTXO 3
        ctx.env.send_to_address(&addr2, Amount::from_sat(500));
        let addr3 = ctx.env.new_address(&ctx.wallet1_name, &AddressType::Legacy);
        // dust UTXO 4
        ctx.env.send_to_address(&addr3, Amount::from_sat(546));
        // non-dust UTXO
        ctx.env.send_to_address(&addr3, Amount::from_sat(600));

        ctx.env.mine_blocks(1);
        let dust = cmd_list(&ctx.db, ctx.network, &ctx.rpc_client, dust_sats);
        assert_eq!(dust.len(), 4);
    }

    /// Add a descriptor with start_height > 0 and verify that dust sent
    /// before that height is not found by cmd_list.
    #[test]
    fn test_cmd_add_start_height() {
        let ctx = TestContext::new();
        let dust_sats = Amount::from_sat(555);
        let start_height = 103;

        let addr = ctx
            .env
            .new_address(&ctx.wallet1_name, &AddressType::Bech32m);

        // block 102. send dust before start_height
        ctx.env.send_to_address(&addr, Amount::from_sat(400));
        ctx.env.mine_blocks(1);

        // block 103. send dusts after start_height
        ctx.env.send_to_address(&addr, Amount::from_sat(401));
        ctx.env.mine_blocks(1);
        // block 104
        ctx.env.send_to_address(&addr, Amount::from_sat(402));
        ctx.env.mine_blocks(1);

        // add descriptor with start_height=103, should skip block 102
        let desc = ctx
            .env
            .get_descriptor(&ctx.wallet1_name, &AddressType::Bech32m);
        cmd_add(
            &ctx.secp,
            &ctx.db,
            ctx.network,
            &ctx.rpc_client,
            desc,
            start_height,
        );

        let dust = cmd_list(&ctx.db, ctx.network, &ctx.rpc_client, dust_sats);
        assert_eq!(
            dust.len(),
            2,
            "should only find dust sent at or after start_height"
        );
    }

    /// Send one confirmed and one unconfirmed dust UTXO, verify cmd_list
    /// only returns the confirmed one.
    #[test]
    fn test_cmd_list_unconfirmed_dust() {
        let ctx = TestContext::new();
        let dust_sats = Amount::from_sat(555);

        let addr = ctx
            .env
            .new_address(&ctx.wallet1_name, &AddressType::Bech32m);

        // send dust and confirm the tx
        ctx.env.send_to_address(&addr, Amount::from_sat(400));
        ctx.env.mine_blocks(1);

        // send dust but do not confirm the tx
        ctx.env.send_to_address(&addr, Amount::from_sat(401));

        let desc = ctx
            .env
            .get_descriptor(&ctx.wallet1_name, &AddressType::Bech32m);
        cmd_add(&ctx.secp, &ctx.db, ctx.network, &ctx.rpc_client, desc, 0);

        let dust = cmd_list(&ctx.db, ctx.network, &ctx.rpc_client, dust_sats);
        assert_eq!(dust.len(), 1, "should only find confirmed dust utxos");
    }

    fn broadcast_and_assert(
        ctx: &TestContext,
        psbt: Psbt,
        expected_inputs: usize,
        expected_op_return: OpReturn,
    ) {
        let txid = cmd_broadcast(&ctx.rpc_client, psbt);
        let tx = ctx
            .env
            .node
            .client
            .get_raw_transaction(txid)
            .unwrap()
            .transaction()
            .unwrap();
        assert!(is_ddust_tx(&tx, &None));
        assert_eq!(tx.input.len(), expected_inputs);
        assert_eq!(tx.output.len(), 1);
        assert_eq!(tx.output[0].script_pubkey, expected_op_return.as_script());
    }

    fn run_spend_test(
        addr_type: &AddressType,
        dust_sats: u64,
        utxo_count: usize,
        expected_op_return: OpReturn,
    ) {
        let ctx = TestContext::new();

        let desc = ctx.env.get_descriptor(&ctx.wallet1_name, addr_type);
        cmd_add(&ctx.secp, &ctx.db, ctx.network, &ctx.rpc_client, desc, 0);

        let addr = ctx.env.new_address(&ctx.wallet1_name, addr_type);
        let send_amt = match addr_type {
            AddressType::Legacy | AddressType::P2shSegwit => 555,
            _ => 400,
        };

        for _ in 0..utxo_count {
            ctx.env.send_to_address(&addr, Amount::from_sat(send_amt));
        }
        ctx.env.mine_blocks(1);

        let result = cmd_spend(
            &ctx.db,
            ctx.network,
            &ctx.rpc_client,
            Amount::from_sat(dust_sats),
            addr,
        );
        assert!(result.is_some(), "expected a psbt to be created");

        let psbt = result.unwrap();
        let signed = ctx.env.wallet_process_psbt(&ctx.wallet1_name, &psbt);
        broadcast_and_assert(&ctx, signed, utxo_count, expected_op_return);
    }

    /// Spending a single non-witness (Legacy, P2SH-SegWit) dust UTXO produces an empty OP_RETURN.
    #[test]
    fn test_spend_single_non_witness() {
        run_spend_test(&AddressType::Legacy, 600, 1, OpReturn::Empty);
        run_spend_test(&AddressType::P2shSegwit, 600, 1, OpReturn::Empty);
    }

    /// Spending a single witness (Bech32m/P2TR) dust UTXO produces an "ash" OP_RETURN.
    #[test]
    fn test_spend_single_witness() {
        run_spend_test(&AddressType::Bech32m, 546, 1, OpReturn::Ash);
    }

    /// Spending multiple dust UTXOs always produces an empty OP_RETURN regardless of script type.
    #[test]
    fn test_spend_multiple_utxos() {
        // multiple UTXOs always produce empty OP_RETURN regardless of script type or sig count
        run_spend_test(&AddressType::Legacy, 600, 3, OpReturn::Empty);
        run_spend_test(&AddressType::Bech32m, 546, 3, OpReturn::Empty);
    }

    /// cmd_spend returns None when the address has no dust UTXOs (amount above threshold).
    #[test]
    fn test_non_dust_spend() {
        let ctx = TestContext::new();
        let dust_sats = Amount::from_sat(600);

        let desc = ctx
            .env
            .get_descriptor(&ctx.wallet1_name, &AddressType::Bech32);
        cmd_add(&ctx.secp, &ctx.db, ctx.network, &ctx.rpc_client, desc, 0);

        let addr = ctx
            .env
            .new_address(&ctx.wallet1_name, &AddressType::Bech32m);
        // non dust UTXO created
        ctx.env.send_to_address(&addr, Amount::from_sat(1500));
        ctx.env.mine_blocks(1);

        let result = cmd_spend(&ctx.db, ctx.network, &ctx.rpc_client, dust_sats, addr);
        assert!(result.is_none(), "expected no Psbt created");
    }

    /// Spend a 2-of-2 P2SH multisig dust UTXO, produces OpReturn::Empty
    #[test]
    fn test_spend_multisig() {
        let ctx = TestContext::new();

        let (addr, desc) = ctx.env.create_multisig(
            &[&ctx.wallet1_name, &ctx.wallet2_name],
            2,
            &AddressType::Legacy,
        );

        cmd_add(&ctx.secp, &ctx.db, ctx.network, &ctx.rpc_client, desc, 0);
        ctx.env.send_to_address(&addr, Amount::from_sat(555));
        ctx.env.mine_blocks(1);

        let result = cmd_spend(
            &ctx.db,
            ctx.network,
            &ctx.rpc_client,
            Amount::from_sat(600),
            addr,
        );
        assert!(result.is_some(), "expected a psbt to be created");
        let psbt = result.unwrap();

        // 2-of-2: both wallets must sign
        let partially_signed = ctx.env.wallet_process_psbt(&ctx.wallet1_name, &psbt);
        let fully_signed = ctx
            .env
            .wallet_process_psbt(&ctx.wallet2_name, &partially_signed);
        broadcast_and_assert(&ctx, fully_signed, 1, OpReturn::Empty);
    }

    /// Minimum sats the batcher's dust must be worth for the replacement to satisfy RBF.
    /// The replacement must:
    ///  - have a higher fee rate than the replaced tx
    ///  - pay for its own bandwidth (incremental relay fee: 0.1 sat/vB * replacement vsize)
    fn min_sats_for_batching(
        orig_tx_fee: Amount,
        orig_tx_input_types: &[InputType],
        replacement_tx_input_type: InputType,
    ) -> Amount {
        let mut orig_tx = TxSizeCalculator::new();
        for input_type in orig_tx_input_types {
            orig_tx = orig_tx.add_input(*input_type);
        }
        let orig_tx_vsize = orig_tx.calculate().vsize.ceil();
        let replacement_tx = orig_tx.add_input(replacement_tx_input_type);
        let replacement_tx_vsize = replacement_tx.calculate().vsize.ceil();
        let fee_rate = orig_tx_fee.to_sat() as f64 / orig_tx_vsize;
        // requires atleast `sats` at the fee rate of the original tx
        let rate_min_sats = (fee_rate * replacement_tx_vsize) as u64 - orig_tx_fee.to_sat();
        // requires `sats` that pay the replacement_vsize at the relay rate
        let bandwidth_min_sats = (0.1 * replacement_tx_vsize) as u64;
        Amount::from_sat(bandwidth_min_sats.max(rate_min_sats))
    }
    fn setup_ctx() -> TestContext {
        let ctx = TestContext::new();

        let desc = ctx
            .env
            .get_descriptor(&ctx.wallet1_name, &AddressType::Legacy);
        cmd_add(&ctx.secp, &ctx.db, ctx.network, &ctx.rpc_client, desc, 0);
        let desc = ctx
            .env
            .get_descriptor(&ctx.wallet1_name, &AddressType::Bech32);
        cmd_add(&ctx.secp, &ctx.db, ctx.network, &ctx.rpc_client, desc, 0);
        let desc = ctx
            .env
            .get_descriptor(&ctx.wallet1_name, &AddressType::Bech32m);
        cmd_add(&ctx.secp, &ctx.db, ctx.network, &ctx.rpc_client, desc, 0);
        let desc = ctx
            .env
            .get_descriptor(&ctx.wallet2_name, &AddressType::Legacy);
        cmd_add(&ctx.secp, &ctx.db, ctx.network, &ctx.rpc_client, desc, 0);
        let desc = ctx
            .env
            .get_descriptor(&ctx.wallet2_name, &AddressType::Bech32m);
        cmd_add(&ctx.secp, &ctx.db, ctx.network, &ctx.rpc_client, desc, 0);
        let desc = ctx
            .env
            .get_descriptor(&ctx.wallet2_name, &AddressType::Bech32);
        cmd_add(&ctx.secp, &ctx.db, ctx.network, &ctx.rpc_client, desc, 0);

        ctx
    }

    /// OpReturn::Empty batch (Legacy + Bech32m)
    #[test]
    fn test_batch_legacy_bech32() {
        let ctx = setup_ctx();

        // Case: Expect OpReturn::Empty
        let addr1 = ctx.env.new_address(&ctx.wallet1_name, &AddressType::Legacy);
        let amt1 = Amount::from_sat(555);
        ctx.env.send_to_address(&addr1, amt1);
        let addr2 = ctx
            .env
            .new_address(&ctx.wallet2_name, &AddressType::Bech32m);
        let min_sats = min_sats_for_batching(amt1, &[InputType::P2PKH], InputType::P2WPKH);
        ctx.env.send_to_address(&addr2, min_sats + Amount::ONE_SAT);
        ctx.env.mine_blocks(1);

        // first tx
        let dust_sats = Amount::from_sat(600);
        let psbt = cmd_spend(&ctx.db, ctx.network, &ctx.rpc_client, dust_sats, addr1).unwrap();
        let signed = ctx.env.wallet_process_psbt(&ctx.wallet1_name, &psbt);
        broadcast_and_assert(&ctx, signed, 1, OpReturn::Empty);

        // spend addr2 and expect batch of the mempool ddust tx
        let psbt_batched =
            cmd_spend(&ctx.db, ctx.network, &ctx.rpc_client, dust_sats, addr2).unwrap();
        let signed = ctx
            .env
            .wallet_process_psbt(&ctx.wallet2_name, &psbt_batched);
        // the original tx output of OpReturn::Empty is preserved
        broadcast_and_assert(&ctx, signed.clone(), 2, OpReturn::Empty);
    }

    /// OpReturn::Ash batch (Bech32m + Legacy)
    #[test]
    fn test_batch_bech32_legacy() {
        let ctx = setup_ctx();

        // Case: Expect OpReturn::Ash
        let addr1 = ctx.env.new_address(&ctx.wallet1_name, &AddressType::Bech32);
        let amt1 = Amount::from_sat(555);
        ctx.env.send_to_address(&addr1, amt1);
        let addr2 = ctx.env.new_address(&ctx.wallet2_name, &AddressType::Legacy);
        let min_sats = min_sats_for_batching(amt1, &[InputType::P2WPKH], InputType::P2PKH);
        ctx.env.send_to_address(&addr2, min_sats);
        ctx.env.mine_blocks(1);

        // first tx
        let dust_sats = Amount::from_sat(1000);
        let psbt = cmd_spend(&ctx.db, ctx.network, &ctx.rpc_client, dust_sats, addr1).unwrap();
        let signed = ctx.env.wallet_process_psbt(&ctx.wallet1_name, &psbt);
        broadcast_and_assert(&ctx, signed, 1, OpReturn::Ash);

        // spend addr2 and expect batch of the mempool ddust tx
        let psbt_batched =
            cmd_spend(&ctx.db, ctx.network, &ctx.rpc_client, dust_sats, addr2).unwrap();
        let signed = ctx
            .env
            .wallet_process_psbt(&ctx.wallet2_name, &psbt_batched);
        // the original tx output of OpReturn::Ash is preserved
        broadcast_and_assert(&ctx, signed.clone(), 2, OpReturn::Ash);
    }

    /// OpReturn::Ash batch (Bech32m + Bech32m)
    #[test]
    fn test_batch_bech32_bech32() {
        let ctx = setup_ctx();

        // Case: The first Bech32m spend outputs an OpReturn:Ash, the second Bech32m spend
        // keeps op_return:Ash from the replaced tx
        let addr1 = ctx
            .env
            .new_address(&ctx.wallet1_name, &AddressType::Bech32m);
        let amt1 = Amount::from_sat(400);
        ctx.env.send_to_address(&addr1, amt1);
        let addr2 = ctx
            .env
            .new_address(&ctx.wallet2_name, &AddressType::Bech32m);
        let min_sats = min_sats_for_batching(amt1, &[InputType::P2TR], InputType::P2TR);
        ctx.env.send_to_address(&addr2, min_sats + Amount::ONE_SAT);
        ctx.env.mine_blocks(1);

        // first tx: spend addr1
        let dust_sats = Amount::from_sat(600);
        let psbt = cmd_spend(&ctx.db, ctx.network, &ctx.rpc_client, dust_sats, addr1).unwrap();
        let signed = ctx.env.wallet_process_psbt(&ctx.wallet1_name, &psbt);
        broadcast_and_assert(&ctx, signed, 1, OpReturn::Ash);

        // spend addr2 and expect batch of the mempool ddust tx
        let psbt_batched =
            cmd_spend(&ctx.db, ctx.network, &ctx.rpc_client, dust_sats, addr2).unwrap();
        let signed = ctx
            .env
            .wallet_process_psbt(&ctx.wallet2_name, &psbt_batched);
        // the original tx output of OpReturn::Ash is preserved
        broadcast_and_assert(&ctx, signed, 2, OpReturn::Ash);
    }

    /// 3. No batching when fee rate is insufficient for RBF
    #[test]
    fn test_no_batch_insufficient_rate() {
        let ctx = setup_ctx();

        // Case: Expect no batching
        let addr1 = ctx
            .env
            .new_address(&ctx.wallet1_name, &AddressType::Bech32m);
        let amt1 = Amount::from_sat(400);
        ctx.env.send_to_address(&addr1, amt1);
        let addr2 = ctx
            .env
            .new_address(&ctx.wallet2_name, &AddressType::Bech32m);
        let min_sats = min_sats_for_batching(amt1, &[InputType::P2WPKH], InputType::P2TR);
        // send less than min_sats to prevent a valid RBF
        dbg!(min_sats);
        ctx.env.send_to_address(&addr2, min_sats - Amount::ONE_SAT);
        ctx.env.mine_blocks(1);

        let dust_sats = Amount::from_sat(1000);
        let psbt = cmd_spend(&ctx.db, ctx.network, &ctx.rpc_client, dust_sats, addr1).unwrap();
        let signed = ctx.env.wallet_process_psbt(&ctx.wallet1_name, &psbt);
        broadcast_and_assert(&ctx, signed, 1, OpReturn::Ash);

        // spend addr2 and expect this tx doesn't replace the original tx because the new fee rate
        // is not enough to replace the mempool tx
        let psbt_batched =
            cmd_spend(&ctx.db, ctx.network, &ctx.rpc_client, dust_sats, addr2).unwrap();
        let signed = ctx
            .env
            .wallet_process_psbt(&ctx.wallet2_name, &psbt_batched);
        broadcast_and_assert(&ctx, signed, 1, OpReturn::Ash);
    }

    /// 4. picks batchable unconfirmed ddust txs
    ///
    /// Setup: steps 1 creates the first ddust tx, step 2 creates another and batches the previous tx
    /// Steps 3 and 4 sends just under `min_sats_for_batching` (against the
    /// lowest-rate existing tx), so each step is forced to stand alone. Step 5 sends a
    /// large enough P2TR dust and batches one unconfirmed tx, leaving other txs unbatched
    ///
    /// Input types (each at its own address): P2TR, P2TR, P2PKH, P2WPKH, P2TR (batcher).
    #[test]
    fn test_batch_pick_batchable() {
        let ctx = setup_ctx();

        // addresses
        let addr1 = ctx
            .env
            .new_address(&ctx.wallet1_name, &AddressType::Bech32m);
        let addr2 = ctx
            .env
            .new_address(&ctx.wallet2_name, &AddressType::Bech32m);
        let addr3 = ctx.env.new_address(&ctx.wallet1_name, &AddressType::Legacy);
        let addr4 = ctx.env.new_address(&ctx.wallet1_name, &AddressType::Bech32);
        let addr5 = ctx
            .env
            .new_address(&ctx.wallet2_name, &AddressType::Bech32m);

        // step 1: any valid starting dust (no mempool ddust yet).
        let amt1 = Amount::from_sat(300);
        ctx.env.send_to_address(&addr1, amt1);

        // step 2 (new P2TR input): > min_sats_for_batching(tx1) batches the previous p2tr input.
        let min_sats_p2tr_p2tr = min_sats_for_batching(amt1, &[InputType::P2TR], InputType::P2TR);
        let amt_batch_p2tr_p2tr = min_sats_p2tr_p2tr + Amount::ONE_SAT;
        ctx.env.send_to_address(&addr2, amt_batch_p2tr_p2tr);

        // step 3 (new P2PKH input): doesn't batch
        let min_sats_batchedp2tr_p2pkh = min_sats_for_batching(
            amt1 + amt_batch_p2tr_p2tr,
            &[InputType::P2TR, InputType::P2TR],
            InputType::P2PKH,
        );
        let amt_no_batch_2p2tr_p2pkh = min_sats_batchedp2tr_p2pkh - Amount::ONE_SAT;
        ctx.env.send_to_address(&addr3, amt_no_batch_2p2tr_p2pkh);

        // step 4 (new P2WPKH input): doesn't batch
        let min_sats_p2wpkh = min_sats_for_batching(
            amt_no_batch_2p2tr_p2pkh,
            &[InputType::P2PKH],
            InputType::P2WPKH,
        );
        let amt_p2wpkh_no_batch = min_sats_p2wpkh - Amount::ONE_SAT;
        ctx.env.send_to_address(&addr4, amt_p2wpkh_no_batch);

        // step 5 (new P2TR input): just enough to batch the P2WPKH input
        let amt_p2tr_batch = amt_p2wpkh_no_batch + Amount::ONE_SAT;
        ctx.env.send_to_address(&addr5, amt_p2tr_batch);

        ctx.env.mine_blocks(1);

        let dust_sats = Amount::from_sat(2500);

        // 1. spend addr1 - standalone (no mempool ddust yet)
        let psbt = cmd_spend(&ctx.db, ctx.network, &ctx.rpc_client, dust_sats, addr1).unwrap();
        let signed = ctx.env.wallet_process_psbt(&ctx.wallet1_name, &psbt);
        broadcast_and_assert(&ctx, signed, 1, OpReturn::Ash);

        // 2. spend addr2 - batches input from previous tx
        let psbt = cmd_spend(&ctx.db, ctx.network, &ctx.rpc_client, dust_sats, addr2).unwrap();
        let signed = ctx.env.wallet_process_psbt(&ctx.wallet2_name, &psbt);
        broadcast_and_assert(&ctx, signed, 2, OpReturn::Ash);

        // 3. spend addr3 - standalone (P2PKH amount below the lowest batch threshold)
        let psbt = cmd_spend(&ctx.db, ctx.network, &ctx.rpc_client, dust_sats, addr3).unwrap();
        let signed = ctx.env.wallet_process_psbt(&ctx.wallet1_name, &psbt);
        broadcast_and_assert(&ctx, signed, 1, OpReturn::Empty);

        // 4. spend addr4 - standalone (P2WPKH amount below the lowest batch threshold)
        let psbt = cmd_spend(&ctx.db, ctx.network, &ctx.rpc_client, dust_sats, addr4).unwrap();
        let signed = ctx.env.wallet_process_psbt(&ctx.wallet1_name, &psbt);
        broadcast_and_assert(&ctx, signed, 1, OpReturn::Ash);

        // 5. spend addr5 - batches 1 unconfirmed tx into a single 2-input replacement.
        let psbt = cmd_spend(&ctx.db, ctx.network, &ctx.rpc_client, dust_sats, addr5).unwrap();
        let signed = ctx.env.wallet_process_psbt(&ctx.wallet2_name, &psbt);
        broadcast_and_assert(&ctx, signed, 2, OpReturn::Ash);
    }

    /// 5. test BIP 125 mempool eviction limit that limits how many unconfirmed txs you can
    ///    replace. The limit currently set to 100
    #[test]
    fn test_batch_mempool_eviction_limit() {
        let ctx = setup_ctx();

        let mut addressses = vec![];
        // create > 100 dust UTXOs
        for i in 0..120 {
            let addr = ctx
                .env
                .new_address(&ctx.wallet1_name, &AddressType::Bech32m);
            ctx.env.send_to_address(&addr, Amount::from_sat(300));
            addressses.push(addr);
            // mine periodically to prevent chain of transactions
            if i % 20 == 0 {
                ctx.env.mine_blocks(1);
            }
        }
        // a large UTXO will be used to batch all the unconfirmed ddust txs(upto 100)
        let addr_batcher1 = ctx
            .env
            .new_address(&ctx.wallet1_name, &AddressType::Bech32m);
        ctx.env
            .send_to_address(&addr_batcher1, Amount::from_sat(1000));
        // another large UTXO will be used to batch all remaining 20 unconfirmed ddust txs
        let addr_batcher2 = ctx
            .env
            .new_address(&ctx.wallet1_name, &AddressType::Bech32m);
        ctx.env
            .send_to_address(&addr_batcher2, Amount::from_sat(1000));

        ctx.env.mine_blocks(1);

        let dust_sats = Amount::from_sat(1000);

        // spend each dust UTXO as a standalone ddust tx, mining a block after each
        // to confirm it (preventing batching)
        for addr in addressses {
            let psbt = cmd_spend(&ctx.db, ctx.network, &ctx.rpc_client, dust_sats, addr).unwrap();
            let signed = ctx.env.wallet_process_psbt(&ctx.wallet1_name, &psbt);
            broadcast_and_assert(&ctx, signed, 1, OpReturn::Ash);
            ctx.env.mine_blocks(1);
        }

        // invalidate blocks to put all 120 ddust txs back into the mempool
        let count: serde_json::Value = ctx.env.node.client.call("getblockcount", &[]).unwrap();
        let target_height = count.as_u64().unwrap() - 120;
        // repeatedly invalidate the current tip until target_height is reached
        loop {
            let tip = ctx.env.node.client.call("getbestblockhash", &[]).unwrap();
            let height: u64 = ctx.env.node.client.call("getblockcount", &[]).unwrap();

            if height <= target_height {
                break;
            }

            ctx.env.node.client.invalidate_block(tip).unwrap();
        }

        // final ddust tx1: batches up to 100 mempool txs
        let psbt = cmd_spend(
            &ctx.db,
            ctx.network,
            &ctx.rpc_client,
            dust_sats,
            addr_batcher1,
        )
        .unwrap();
        let signed = ctx.env.wallet_process_psbt(&ctx.wallet1_name, &psbt);
        // 101 = one self + 100 from mempool
        broadcast_and_assert(&ctx, signed, 101, OpReturn::Ash);

        // final ddust tx2: batches remaining 20 mempool txs plus ddust tx1
        let psbt = cmd_spend(
            &ctx.db,
            ctx.network,
            &ctx.rpc_client,
            dust_sats,
            addr_batcher2,
        )
        .unwrap();
        let signed = ctx.env.wallet_process_psbt(&ctx.wallet1_name, &psbt);
        broadcast_and_assert(&ctx, signed, 122, OpReturn::Ash);
    }
}
