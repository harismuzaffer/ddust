use bdk_redb::Store;
use bdk_wallet::bitcoin::secp256k1::{All, Secp256k1};
use bdk_wallet::bitcoin::{
    Address, Amount, EcdsaSighashType, Network, Psbt, ScriptBuf, TapSighashType, Transaction, TxIn,
};
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
    info!("db file: {:?}", db_file);
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
            for (wallet_name, out) in dust {
                let address = Address::from_script(&out.txout.script_pubkey, network)
                    .expect("failed to get address");
                info!(
                    "wallet: {}, value: {}, address: {}, outpoint: {}:{}",
                    wallet_name,
                    out.txout.value.display_dynamic(),
                    address,
                    out.outpoint.txid,
                    out.outpoint.vout
                );
            }
        }
        Commands::Spend { address } => {
            let filter_address = Address::from_str(&address)
                .expect("failed to parse filter address")
                .require_network(network)
                .expect("invalid network");
            for psbt in cmd_spend(&db, network, &rpc_client, dust_amount, filter_address) {
                info!("sign and broadcast tx for psbt: {}", psbt);
            }
        }
        Commands::Broadcast { psbt } => {
            let txid = cmd_broadcast(&rpc_client, psbt);
            info!("transaction broadcast with txid: {}", txid);
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
) -> Vec<(String, LocalOutput)> {
    let mut dust_outputs = vec![];
    for wallet_name in wallet_names(db.clone()) {
        info!("wallet: {}", wallet_name);
        if let (Some(mut wallet), mut store) = load_wallet(db.clone(), network, wallet_name.clone())
        {
            sync_wallet(rpc_client, &mut wallet, &mut store);
            wallet.list_unspent().for_each(|out| {
                if is_dust(&out, &dust_amount) {
                    dust_outputs.push((wallet_name.clone(), out));
                }
            });
        } else {
            error!("could not load wallet with name {}", wallet_name);
        }
    }
    dust_outputs
}

fn cmd_spend(
    db: &Arc<Database>,
    network: Network,
    rpc_client: &Client,
    dust_amount: Amount,
    filter_address: Address,
) -> Vec<Psbt> {
    let mut psbts = vec![];
    for wallet_name in wallet_names(db.clone()) {
        info!("wallet: {}", wallet_name);
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
                info!("found {} unconfirmed ddust txs", unconfirmed_txs.len());

                let mut tx_builder = wallet.build_tx();
                tx_builder
                    .nlocktime(LockTime::from_height(0).expect("valid height"))
                    .manually_selected_only()
                    .add_utxos(&utxos)
                    .expect("failed to add dust outpoints");

                if !unconfirmed_txs.is_empty()
                    && should_combine(
                        rpc_client,
                        input_amount,
                        &unconfirmed_txs,
                        &dust,
                        &unconfirmed_txs[0].output[0].script_pubkey,
                    )
                {
                    info!("unconfirmed trs can be combined");
                    for tx in &unconfirmed_txs {
                        for input in &tx.input {
                            let f_outpoint = input.previous_output;
                            let f_input_prev_tx = rpc_client
                                .get_raw_transaction(&f_outpoint.txid, None)
                                .unwrap();
                            let f_prev_txout =
                                f_input_prev_tx.output[f_outpoint.vout as usize].clone();

                            input_amount += f_prev_txout.value;

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
                                    panic!(
                                        "failed to add the foreign UTXO. Outpoint: {}",
                                        f_outpoint
                                    )
                                });
                        }
                    }
                }

                info!("total spent to fees: {}", &input_amount);
                tx_builder.fee_absolute(input_amount);

                if !unconfirmed_txs.is_empty() {
                    // the new tx shall use the data found in the unconfirmed txs
                    let suggested_script = &unconfirmed_txs[0].output[0].script_pubkey;
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

                // set script type to ANYONECANPAY|ALL
                if dust[0].txout.script_pubkey.is_p2tr() {
                    tx_builder.sighash(TapSighashType::AllPlusAnyoneCanPay.into());
                } else {
                    tx_builder.sighash(EcdsaSighashType::AllPlusAnyoneCanPay.into());
                }

                psbts.push(tx_builder.finish().expect("failed to create psbt"));
            }
        } else {
            error!("could not load wallet with name {}", wallet_name);
        }
    }
    psbts
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
    info!(
        "connected to bitcoin core rpc, chain: {}",
        blockchain_info.chain
    );
    info!(
        "latest block: {} at height {}",
        blockchain_info.best_block_hash, blockchain_info.blocks,
    );

    let wallet_tip: CheckPoint = wallet.latest_checkpoint();
    info!(
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
            info!(
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

fn find_unconfirmed_ddust_txs(rpc_client: &Client) -> Vec<Transaction> {
    let tx_ids = rpc_client
        .get_raw_mempool()
        .expect("failed to get mempool transaction IDs");
    let mut unconfirmed_txs: Vec<Transaction> = vec![];
    // the combined tx shall use the first unconfirmed tx's script data
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

/// ddust pattern:
/// has a single op_return
/// one or more inputs with SIGHASH_ALL|ANYONECANPAY signature type
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

    // All inputs must be ANYONECANPAY|ALL
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
        // If a legacy input check the script sig sighash byte
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
        57.5
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

/// Checks if combining dust inputs with existing ddust transactions in the mempool
/// produces a fee rate at least 1 sat/vB higher than the highest existing fee rate,
/// as required by RBF replacement rules.
fn should_combine(
    rpc_client: &Client,
    this_amount: Amount,
    unconfirmed_txs: &Vec<Transaction>,
    dust_utxos: &[LocalOutput],
    output_script: &ScriptBuf,
) -> bool {
    // this tx fee rate > max foreign tx fee rate
    // this tx fee rate = fee / vsize
    // -> total dust amt / vsize
    // vsize = overhead + one op_return output + (new dust utxos + foreign utxos)
    let mut max_fee_rate: f64 = 0.0;
    let mut tx_vsize: f64 = 0.0;
    let mut input_amount: Amount = this_amount;

    // overhead size
    tx_vsize += 10.5;
    // size of dust inputs to be spent
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

    for tx in unconfirmed_txs {
        let entry = rpc_client.get_mempool_entry(&tx.compute_txid()).unwrap();
        let fee = entry.fees.base;
        input_amount += fee;
        let fee_sats = fee.to_sat();
        let vsize = entry.vsize;
        let rate = fee_sats as f64 / vsize as f64;
        if rate > max_fee_rate {
            max_fee_rate = rate;
        }

        for input in &tx.input {
            // foreign utxo input size
            tx_vsize += get_input_vsize(input);
        }
    }

    let tx_fee_rate = input_amount.to_sat() as f64 / tx_vsize;
    debug!(
        "tx_fee_rate: {}, max_fee_rate: {}, combine? {}",
        tx_fee_rate,
        max_fee_rate,
        tx_fee_rate > max_fee_rate + 0.1
    );
    tx_fee_rate > max_fee_rate + 0.1
}

#[cfg(test)]
mod test_env;

#[cfg(test)]
mod tests {
    use super::*;
    use corepc_node::AddressType;
    use test_env::TestEnv;

    struct TestContext {
        env: TestEnv,
        db: Arc<Database>,
        rpc_client: Client,
        secp: Secp256k1<All>,
        network: Network,
        wallet1_name: String,
    }

    impl TestContext {
        fn new() -> Self {
            let env = TestEnv::new();
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
            Self {
                env,
                db,
                rpc_client,
                secp,
                network,
                wallet1_name,
            }
        }
    }

    fn run_spend_test(addr_type: &AddressType, dust_sats: u64, expected_op_return: &[u8]) {
        let ctx = TestContext::new();

        let desc = ctx.env.get_descriptor(&ctx.wallet1_name, addr_type);
        cmd_add(&ctx.secp, &ctx.db, ctx.network, &ctx.rpc_client, desc, 0);

        let addr = ctx.env.new_address(&ctx.wallet1_name, addr_type);
        ctx.env.send_to_address(&addr, Amount::from_sat(dust_sats));
        ctx.env.mine_blocks(1);

        let psbts = cmd_spend(
            &ctx.db,
            ctx.network,
            &ctx.rpc_client,
            Amount::from_sat(dust_sats),
            addr,
        );
        assert!(!psbts.is_empty(), "expected a psbt to be created");

        let signed = ctx.env.wallet_process_psbt(&ctx.wallet1_name, &psbts[0]);
        let txid = cmd_broadcast(&ctx.rpc_client, signed);
        let tx = ctx
            .env
            .node
            .client
            .get_raw_transaction(txid)
            .unwrap()
            .transaction()
            .unwrap();

        assert!(is_ddust_tx(&tx, &None));
        assert_eq!(tx.output.len(), 1);
        assert_eq!(tx.output[0].script_pubkey.as_bytes(), expected_op_return);
    }

    #[test]
    fn test_spend_legacy() {
        run_spend_test(&AddressType::Legacy, 546, &[0x6a, 0x00]);
    }

    #[test]
    fn test_spend_single() {
        run_spend_test(&AddressType::Bech32m, 400, &[0x6a, 0x03, 0x61, 0x73, 0x68]);
    }
}
