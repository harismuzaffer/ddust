use std::str::FromStr;

use bdk_bitcoind_rpc::bitcoincore_rpc::{Auth, Client};
use bdk_wallet::bitcoin::secp256k1::Secp256k1;
use bdk_wallet::bitcoin::{Address, Amount, Psbt, PublicKey, Txid};
use bdk_wallet::descriptor::ExtendedDescriptor;
use bdk_wallet::serde_json::{self, json};
use corepc_node::AddressType;

pub struct TestEnv {
    pub node: corepc_node::Node,
}

#[allow(dead_code)]
impl TestEnv {
    /// Instantiate a [`TestEnv`] with default configuration.
    pub fn new() -> Self {
        TestEnv::new_with_conf(corepc_node::Conf::default())
    }

    /// Instantiate a [`TestEnv`] with a custom [`corepc_node::Conf`].
    pub fn new_with_conf(conf: corepc_node::Conf) -> Self {
        let bitcoind_exe = std::env::var("BITCOIND_EXE")
            .ok()
            .or_else(|| corepc_node::downloaded_exe_path().ok())
            .expect(
                "you need to provide an env var BITCOIND_EXE or specify a bitcoind version feature",
            );
        let node = corepc_node::Node::with_conf(bitcoind_exe, &conf).unwrap();
        let env = Self { node };

        env.node.client.create_wallet("mining").unwrap();
        let mining_address = env
            .wallet_client("mining")
            .get_new_address(None, None)
            .unwrap()
            .address()
            .unwrap()
            .assume_checked();
        env.node
            .client
            .generate_to_address(101, &mining_address)
            .unwrap();
        env
    }

    /// Create a corepc [`Client`](corepc_node::Client) - wallet specific.
    fn wallet_client(&self, wallet_name: &str) -> corepc_node::Client {
        let url = self.node.rpc_url_with_wallet(wallet_name);
        let auth =
            corepc_client::client_sync::Auth::CookieFile(self.node.params.cookie_file.clone());
        corepc_node::Client::new_with_auth(&url, auth).unwrap()
    }

    /// Create a [`bdk_bitcoind_rpc::bitcoincore_rpc::Client`] connected to this node.
    pub fn rpc_client(&self) -> Client {
        let url = self.node.rpc_url();
        let auth = Auth::CookieFile(self.node.params.cookie_file.clone());
        Client::new(&url, auth).unwrap()
    }

    /// Create a [`bdk_bitcoind_rpc::bitcoincore_rpc::Client`] specific to the given wallet
    pub fn rpc_client_wallet(&self, wallet_name: &str) -> Client {
        let url = self.node.rpc_url_with_wallet(wallet_name);
        let auth = Auth::CookieFile(self.node.params.cookie_file.clone());
        Client::new(&url, auth).unwrap()
    }

    /// Create a Bitcoin Core wallet.
    pub fn create_wallet(&self, name: &str) {
        self.node.client.create_wallet(name).unwrap();
    }

    /// Get the external descriptor of the given address type from a wallet.
    pub fn get_descriptor(
        &self,
        wallet_name: &str,
        address_type: &AddressType,
    ) -> ExtendedDescriptor {
        let prefix = match address_type {
            AddressType::Legacy => "pkh(",
            AddressType::P2shSegwit => "sh(wpkh(",
            AddressType::Bech32 => "wpkh(",
            AddressType::Bech32m => "tr(",
        };
        let result = self.wallet_client(wallet_name).list_descriptors().unwrap();
        let desc_str = result
            .descriptors
            .into_iter()
            .find(|d| d.active && d.internal == Some(false) && d.descriptor.starts_with(prefix))
            .expect("no matching external descriptor found")
            .descriptor;
        let secp = Secp256k1::new();
        ExtendedDescriptor::parse_descriptor(&secp, &desc_str)
            .map(|(desc, _)| desc)
            .expect("failed to parse descriptor")
    }

    /// Get a new address of the given type from the given wallet
    pub fn new_address(&self, wallet_name: &str, address_type: &AddressType) -> Address {
        self.wallet_client(wallet_name)
            .get_new_address(None, Some(address_type.clone()))
            .unwrap()
            .address()
            .unwrap()
            .assume_checked()
    }

    /// Send `amount` to `address` and return the txid.
    pub fn send_to_address(&self, address: &Address, amount: Amount) -> Txid {
        self.wallet_client("mining")
            .send_to_address(address, amount)
            .unwrap()
            .txid()
            .unwrap()
    }

    /// Mine `count` blocks.
    pub fn mine_blocks(&self, count: usize) {
        let mining_address = self
            .wallet_client("mining")
            .get_new_address(None, None)
            .unwrap()
            .address()
            .unwrap()
            .assume_checked();
        self.node
            .client
            .generate_to_address(count, &mining_address)
            .unwrap();
    }

    /// Create a `required`-of-N multisig address from the given wallets.
    /// Returns the multisig address and its descriptor.
    pub fn create_multisig(
        &self,
        wallet_names: &[&str],
        required: usize,
    ) -> (Address, ExtendedDescriptor) {
        let pubkeys: Vec<PublicKey> = wallet_names
            .iter()
            .map(|name| {
                let addr = self.new_address(name, &AddressType::Bech32);
                let pubkey_hex = self
                    .wallet_client(name)
                    .get_address_info(&addr)
                    .unwrap()
                    .pubkey
                    .unwrap();
                PublicKey::from_str(pubkey_hex.as_str()).unwrap()
            })
            .collect();

        let result = self
            .node
            .client
            .create_multisig(required as u32, pubkeys)
            .unwrap();

        let secp = Secp256k1::new();
        let desc = ExtendedDescriptor::parse_descriptor(&secp, &result.descriptor)
            .map(|(d, _)| d)
            .unwrap();

        (
            Address::from_str(&result.address).unwrap().assume_checked(),
            desc,
        )
    }

    /// Process and sign a PSBT using the given wallet.
    pub fn wallet_process_psbt(&self, wallet_name: &str, psbt: &Psbt) -> Psbt {
        let result = self
            .wallet_client(wallet_name)
            .call::<serde_json::Value>(
                "walletprocesspsbt",
                &[
                    json!(psbt.to_string()),
                    json!(true),
                    json!("ALL|ANYONECANPAY"),
                ],
            )
            .unwrap();
        let psbt_str = result["psbt"].as_str().unwrap();
        Psbt::from_str(psbt_str).unwrap()
    }
}
