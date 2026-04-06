//! Transaction size calculator for Bitcoin dust disposal transactions.
//!
//! This module provides correct size calculations for different Bitcoin transaction
//! input types, accounting for script types, multisig configurations, and the
//! ddust protocol's OP_RETURN output requirements.

/// Multisig configuration (m-of-n)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MultisigConfig {
    /// Number of required signatures (m)
    pub required: usize,
    /// Total number of keys (n)
    pub total: usize,
}

impl MultisigConfig {
    pub fn new(required: usize, total: usize) -> Self {
        assert!(required > 0, "required must be > 0");
        assert!(total >= required, "total must be >= required");
        assert!(total <= 15, "standard multisig limited to 15 keys");
        Self { required, total }
    }
}

/// What's wrapped inside a P2SH script
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum P2SHInner {
    /// Bare multisig (m-of-n)
    Multisig(MultisigConfig),
    /// Nested SegWit (P2SH-P2WPKH)
    P2WPKH,
    /// Nested SegWit multisig (P2SH-P2WSH)
    P2WSH(MultisigConfig),
}

/// Input script type configuration
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputType {
    /// Legacy Pay-to-Public-Key-Hash
    P2PKH,
    /// Native SegWit Pay-to-Witness-Public-Key-Hash
    P2WPKH,
    /// Taproot (key-path spend)
    P2TR,
    /// Pay-to-Script-Hash (wrapping various script types)
    P2SH(P2SHInner),
    /// Native SegWit Pay-to-Witness-Script-Hash (multisig)
    P2WSH(MultisigConfig),
}

/// Transaction size components broken down by category
#[derive(Debug, Clone, PartialEq)]
pub struct TxSizeBreakdown {
    /// Transaction overhead (version, input/output counts, locktime)
    pub overhead_bytes: usize,
    /// Total input base size (non-witness portion)
    pub input_base_bytes: usize,
    /// Total input witness size
    pub input_witness_bytes: usize,
    /// Output size (OP_RETURN)
    pub output_bytes: usize,
    /// Base transaction size (non-witness data)
    pub base_size: usize,
    /// Total transaction size including witness
    pub total_size: usize,
    /// Transaction weight in weight units
    pub weight: usize,
    /// Virtual size in virtual bytes
    pub vsize: f64,
}

impl TxSizeBreakdown {
    /// Check if this transaction meets the 65-byte minimum base size
    pub fn meets_min_size(&self) -> bool {
        self.base_size >= 65
    }

    /// Calculate fee rate given an input amount in satoshis
    pub fn fee_rate(&self, input_sats: u64) -> f64 {
        input_sats as f64 / self.vsize
    }

    /// Calculate minimum sats needed for a target fee rate
    pub fn min_sats_for_rate(&self, target_rate: f64) -> u64 {
        (self.vsize * target_rate).ceil() as u64
    }
}

/// Calculator for dust disposal transaction sizes
#[derive(Debug, Clone)]
pub struct TxSizeCalculator {
    inputs: Vec<InputType>,
}

impl Default for TxSizeCalculator {
    fn default() -> Self {
        Self::new()
    }
}

impl TxSizeCalculator {
    /// Transaction overhead: version (4) + input count (1) + output count (1) + locktime (4)
    const OVERHEAD: usize = 10;

    /// Witness marker and flag bytes (only present in witness transactions)
    const WITNESS_HEADER: usize = 2;

    /// Empty OP_RETURN output size: amount (8) + script len (1) + OP_RETURN OP_0 (2)
    const OP_RETURN_EMPTY: usize = 11;

    /// "ash" OP_RETURN output size: amount (8) + script len (1) + OP_RETURN OP_PUSHBYTES_3 "ash" (5)
    const OP_RETURN_ASH: usize = 14;

    /// Minimum base transaction size required by Bitcoin Core
    const MIN_BASE_SIZE: usize = 65;

    /// Create a new calculator for a dust disposal transaction
    pub fn new() -> Self {
        Self { inputs: Vec::new() }
    }

    /// Add an input of the specified type
    pub fn add_input(mut self, input_type: InputType) -> Self {
        self.inputs.push(input_type);
        self
    }

    /// Add multiple inputs of the same type
    pub fn add_inputs(mut self, input_type: InputType, count: usize) -> Self {
        for _ in 0..count {
            self.inputs.push(input_type);
        }
        self
    }

    /// Calculate the final transaction size breakdown.
    /// Automatically determines whether to use empty or "ash" OP_RETURN
    /// based on minimum transaction size requirements.
    pub fn calculate(&self) -> TxSizeBreakdown {
        assert!(!self.inputs.is_empty(), "must have at least one input");

        let input_base: usize = self.inputs.iter().map(Self::input_base_size).sum();
        let input_witness: usize = self.inputs.iter().map(Self::input_witness_size).sum();
        let has_witness = input_witness > 0;

        // Check if we need "ash" padding to meet minimum size
        // Only single witness inputs need padding
        let base_with_empty = Self::OVERHEAD + input_base + Self::OP_RETURN_EMPTY;
        let needs_ash =
            self.inputs.len() == 1 && has_witness && base_with_empty < Self::MIN_BASE_SIZE;

        let output_bytes = if needs_ash {
            Self::OP_RETURN_ASH
        } else {
            Self::OP_RETURN_EMPTY
        };

        self.build_breakdown(input_base, input_witness, output_bytes, has_witness)
    }

    /// Calculate with a specific OP_RETURN type (for testing edge cases)
    pub fn calculate_with_op_return(&self, use_ash: bool) -> TxSizeBreakdown {
        assert!(!self.inputs.is_empty(), "must have at least one input");

        let input_base: usize = self.inputs.iter().map(Self::input_base_size).sum();
        let input_witness: usize = self.inputs.iter().map(Self::input_witness_size).sum();
        let has_witness = input_witness > 0;

        let output_bytes = if use_ash {
            Self::OP_RETURN_ASH
        } else {
            Self::OP_RETURN_EMPTY
        };

        self.build_breakdown(input_base, input_witness, output_bytes, has_witness)
    }

    /// Get the vsize contribution of just the inputs (useful for batching calculations)
    pub fn input_vsize(&self) -> f64 {
        let input_base: usize = self.inputs.iter().map(Self::input_base_size).sum();
        let input_witness: usize = self.inputs.iter().map(Self::input_witness_size).sum();

        // Input weight = base * 4 + witness
        let input_weight = input_base * 4 + input_witness;
        input_weight as f64 / 4.0
    }

    /// Get vsize for a single input type
    pub fn single_input_vsize(input_type: InputType) -> f64 {
        let base = Self::input_base_size(&input_type);
        let witness = Self::input_witness_size(&input_type);
        let weight = base * 4 + witness;
        weight as f64 / 4.0
    }

    fn build_breakdown(
        &self,
        input_base: usize,
        input_witness: usize,
        output_bytes: usize,
        has_witness: bool,
    ) -> TxSizeBreakdown {
        let base_size = Self::OVERHEAD + input_base + output_bytes;
        let witness_overhead = if has_witness { Self::WITNESS_HEADER } else { 0 };
        let total_size = base_size + witness_overhead + input_witness;

        // Weight calculation:
        // - Base data (non-witness) counts as 4 weight units per byte
        // - Witness data counts as 1 weight unit per byte
        let weight = base_size * 4 + witness_overhead + input_witness;
        let vsize = weight as f64 / 4.0;

        TxSizeBreakdown {
            overhead_bytes: Self::OVERHEAD,
            input_base_bytes: input_base,
            input_witness_bytes: input_witness,
            output_bytes,
            base_size,
            total_size,
            weight,
            vsize,
        }
    }

    /// Calculate input base size (non-witness portion)
    fn input_base_size(input_type: &InputType) -> usize {
        match input_type {
            // P2PKH: outpoint (36) + scriptSig length (1) + scriptSig (~107) + sequence (4)
            // scriptSig: sig (~72) + pubkey (33) + push opcodes (2)
            InputType::P2PKH => 148,

            // P2WPKH: outpoint (36) + empty scriptSig (1) + sequence (4)
            InputType::P2WPKH => 41,

            // P2TR: outpoint (36) + empty scriptSig (1) + sequence (4)
            InputType::P2TR => 41,

            // P2SH variants
            InputType::P2SH(inner) => match inner {
                P2SHInner::Multisig(cfg) => Self::p2sh_multisig_base_size(cfg),
                // P2SH-P2WPKH: outpoint (36) + scriptSig length (1) + redeemScript (23) + sequence (4)
                P2SHInner::P2WPKH => 64,
                // P2SH-P2WSH: outpoint (36) + scriptSig length (1) + redeemScript (35) + sequence (4)
                P2SHInner::P2WSH(_) => 76,
            },

            // P2WSH: outpoint (36) + empty scriptSig (1) + sequence (4)
            InputType::P2WSH(_) => 41,
        }
    }

    /// Calculate input witness size
    fn input_witness_size(input_type: &InputType) -> usize {
        match input_type {
            // P2PKH: no witness data
            InputType::P2PKH => 0,

            // P2WPKH witness: item count (1) + sig length (1) + sig (~72) + pubkey length (1) + pubkey (33)
            InputType::P2WPKH => 108,

            // P2TR witness (key-path): item count (1) + sig length (1) + schnorr sig (64) + sighash type (1)
            // Note: NONE|ANYONECANPAY requires explicit sighash byte
            InputType::P2TR => 67,

            InputType::P2SH(inner) => match inner {
                // Bare P2SH multisig: no witness
                P2SHInner::Multisig(_) => 0,
                // P2SH-P2WPKH: same witness as P2WPKH
                P2SHInner::P2WPKH => 108,
                // P2SH-P2WSH: multisig witness
                P2SHInner::P2WSH(cfg) => Self::p2wsh_witness_size(cfg),
            },

            // P2WSH: multisig witness
            InputType::P2WSH(cfg) => Self::p2wsh_witness_size(cfg),
        }
    }

    /// Calculate P2SH bare multisig input base size
    fn p2sh_multisig_base_size(cfg: &MultisigConfig) -> usize {
        // outpoint: 36 bytes (txid 32 + vout 4)
        // sequence: 4 bytes
        // scriptSig contains:
        //   - OP_0 (CHECKMULTISIG bug workaround): 1 byte
        //   - m signatures: m * (push opcode 1 + sig ~72) = m * 73
        //   - redeemScript push: 1 byte (OP_PUSHDATA1) + 1 byte (length) for scripts > 75 bytes
        //   - redeemScript: OP_m (1) + n * (push 1 + pubkey 33) + OP_n (1) + OP_CHECKMULTISIG (1)
        //                 = 3 + n * 34

        let redeem_script_size = 3 + cfg.total * 34;
        let redeem_script_push = if redeem_script_size > 75 { 2 } else { 1 };

        // scriptSig = OP_0 + m * (push + sig) + redeemScript push + redeemScript
        let scriptsig_size = 1 + cfg.required * 73 + redeem_script_push + redeem_script_size;
        let scriptsig_len_varint = Self::varint_size(scriptsig_size);

        36 + scriptsig_len_varint + scriptsig_size + 4
    }

    /// Calculate P2WSH witness size for multisig
    fn p2wsh_witness_size(cfg: &MultisigConfig) -> usize {
        // Witness stack:
        //   - item count: varint (1 byte for small counts)
        //   - OP_0 dummy for CHECKMULTISIG bug: length (1) + empty (0) = 1 byte
        //   - m signatures: m * (length varint 1 + sig ~72) = m * 73
        //   - witness script: length varint + script
        //     script = OP_m (1) + n * (push 1 + pubkey 33) + OP_n (1) + OP_CHECKMULTISIG (1)
        //            = 3 + n * 34

        let witness_script_size = 3 + cfg.total * 34;
        let witness_script_len_varint = Self::varint_size(witness_script_size);

        // item count (1) + OP_0 dummy (1) + m signatures + witness script
        1 + 1 + cfg.required * 73 + witness_script_len_varint + witness_script_size
    }

    /// Calculate VarInt size for a given value
    fn varint_size(value: usize) -> usize {
        if value < 0xFD {
            1
        } else if value <= 0xFFFF {
            3
        } else if value <= 0xFFFFFFFF {
            5
        } else {
            9
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ==================== Basic Input Type Tests ====================

    #[test]
    fn test_single_p2pkh_input() {
        let size = TxSizeCalculator::new()
            .add_input(InputType::P2PKH)
            .calculate();

        // From BIP spec table: P2PKH single input
        // Overhead: 10, Input: 148, OP_RETURN: 11 (empty)
        // Base size: 169, Weight: 676, vSize: 169
        assert_eq!(size.overhead_bytes, 10);
        assert_eq!(size.input_base_bytes, 148);
        assert_eq!(size.input_witness_bytes, 0);
        assert_eq!(size.output_bytes, 11); // empty OP_RETURN (no padding needed)
        assert_eq!(size.base_size, 169);
        assert_eq!(size.total_size, 169);
        assert_eq!(size.weight, 676);
        assert_eq!(size.vsize, 169.0);
        assert!(size.meets_min_size());
    }

    #[test]
    fn test_single_p2wpkh_input() {
        let size = TxSizeCalculator::new()
            .add_input(InputType::P2WPKH)
            .calculate();

        // From BIP spec table: P2WPKH single input
        // Overhead: 10, Input base: 41, OP_RETURN: 14 (ash for padding)
        // Base size: 65, Witness: 108, Total: 175, Weight: 370, vSize: 92.5
        assert_eq!(size.overhead_bytes, 10);
        assert_eq!(size.input_base_bytes, 41);
        assert_eq!(size.input_witness_bytes, 108);
        assert_eq!(size.output_bytes, 14); // "ash" OP_RETURN (needs padding)
        assert_eq!(size.base_size, 65);
        // total = base + witness_header (2) + witness = 65 + 2 + 108 = 175
        assert_eq!(size.total_size, 175);
        // weight = base * 4 + witness_header + witness = 65*4 + 2 + 108 = 370
        assert_eq!(size.weight, 370);
        assert_eq!(size.vsize, 92.5);
        assert!(size.meets_min_size());
    }

    #[test]
    fn test_single_p2tr_input() {
        let size = TxSizeCalculator::new()
            .add_input(InputType::P2TR)
            .calculate();

        // From BIP spec table: P2TR single input
        // Overhead: 10, Input base: 41, OP_RETURN: 14 (ash for padding)
        // Base size: 65, Witness: 67, Weight: 329, vSize: 82.25
        assert_eq!(size.overhead_bytes, 10);
        assert_eq!(size.input_base_bytes, 41);
        assert_eq!(size.input_witness_bytes, 67);
        assert_eq!(size.output_bytes, 14); // "ash" OP_RETURN (needs padding)
        assert_eq!(size.base_size, 65);
        // total = 65 + 2 + 67 = 134
        assert_eq!(size.total_size, 134);
        // weight = 65*4 + 2 + 67 = 329
        assert_eq!(size.weight, 329);
        assert_eq!(size.vsize, 82.25);
        assert!(size.meets_min_size());
    }

    // ==================== Multisig Tests ====================

    #[test]
    fn test_p2sh_2of2_multisig() {
        let size = TxSizeCalculator::new()
            .add_input(InputType::P2SH(P2SHInner::Multisig(MultisigConfig::new(
                2, 2,
            ))))
            .calculate();

        // 2-of-2 P2SH multisig:
        // redeemScript = OP_2 + 2*(push + pubkey) + OP_2 + OP_CHECKMULTISIG = 3 + 2*34 = 71 bytes
        // scriptSig = OP_0 + 2*(push + sig) + push + redeemScript = 1 + 2*73 + 1 + 71 = 219
        // input = 36 + 1 + 219 + 4 = 260
        assert_eq!(size.input_base_bytes, 260);
        assert_eq!(size.input_witness_bytes, 0);
        assert_eq!(size.output_bytes, 11); // empty (base size > 65)
        assert!(size.meets_min_size());
    }

    #[test]
    fn test_p2sh_2of3_multisig() {
        let size = TxSizeCalculator::new()
            .add_input(InputType::P2SH(P2SHInner::Multisig(MultisigConfig::new(
                2, 3,
            ))))
            .calculate();

        // 2-of-3 P2SH multisig:
        // redeemScript = 3 + 3*34 = 105 bytes (> 75, needs OP_PUSHDATA1)
        // scriptSig = OP_0 + 2*73 + 2 + 105 = 254 bytes
        // scriptSig length varint = 3 bytes (since 254 >= 253)
        // input = 36 + 3 + 254 + 4 = 297
        assert_eq!(size.input_base_bytes, 297);
        assert_eq!(size.input_witness_bytes, 0);
        assert!(size.meets_min_size());
    }

    #[test]
    fn test_p2wsh_2of3_multisig() {
        let size = TxSizeCalculator::new()
            .add_input(InputType::P2WSH(MultisigConfig::new(2, 3)))
            .calculate();

        // P2WSH 2-of-3:
        // input base: 41
        // witness script = 3 + 3*34 = 105 bytes
        // witness = 1 (count) + 1 (OP_0) + 2*73 (sigs) + 1 (len) + 105 = 254
        assert_eq!(size.input_base_bytes, 41);
        assert_eq!(size.input_witness_bytes, 254);
        assert_eq!(size.output_bytes, 14); // "ash" (single witness input)
        assert!(size.meets_min_size());

        // weight = 65*4 + 2 + 254 = 516
        assert_eq!(size.weight, 516);
        assert_eq!(size.vsize, 129.0);
    }

    #[test]
    fn test_p2sh_p2wpkh() {
        let size = TxSizeCalculator::new()
            .add_input(InputType::P2SH(P2SHInner::P2WPKH))
            .calculate();

        // P2SH-P2WPKH (nested SegWit):
        // input base: 64 (36 + 1 + 23 + 4)
        // witness: 108 (same as P2WPKH)
        assert_eq!(size.input_base_bytes, 64);
        assert_eq!(size.input_witness_bytes, 108);
        assert_eq!(size.output_bytes, 11); // empty (base = 10+64+11 = 85 >= 65)
        assert!(size.meets_min_size());

        // weight = 85*4 + 2 + 108 = 450
        assert_eq!(size.weight, 450);
        assert_eq!(size.vsize, 112.5);
    }

    #[test]
    fn test_p2sh_p2wsh_2of3() {
        let size = TxSizeCalculator::new()
            .add_input(InputType::P2SH(P2SHInner::P2WSH(MultisigConfig::new(2, 3))))
            .calculate();

        // P2SH-P2WSH 2-of-3:
        // input base: 76 (36 + 1 + 35 + 4)
        // witness = 1 + 1 + 2*73 + 1 + 105 = 254
        assert_eq!(size.input_base_bytes, 76);
        assert_eq!(size.input_witness_bytes, 254);
        assert_eq!(size.output_bytes, 11); // empty (base = 10+76+11 = 97 >= 65)
        assert!(size.meets_min_size());
    }

    // ==================== Multiple Input Tests ====================

    #[test]
    fn test_multiple_p2tr_inputs_uses_empty() {
        let size = TxSizeCalculator::new()
            .add_input(InputType::P2TR)
            .add_input(InputType::P2TR)
            .calculate();

        // Multiple inputs always use empty OP_RETURN
        assert_eq!(size.output_bytes, 11);
        assert_eq!(size.input_base_bytes, 82); // 41 * 2
        assert_eq!(size.input_witness_bytes, 134); // 67 * 2
    }

    #[test]
    fn test_mixed_inputs() {
        let size = TxSizeCalculator::new()
            .add_input(InputType::P2PKH)
            .add_input(InputType::P2TR)
            .calculate();

        assert_eq!(size.input_base_bytes, 189); // 148 + 41
        assert_eq!(size.input_witness_bytes, 67); // 0 + 67
        assert_eq!(size.output_bytes, 11); // empty (multiple inputs)
    }

    #[test]
    fn test_add_inputs_helper() {
        let size = TxSizeCalculator::new()
            .add_inputs(InputType::P2TR, 3)
            .calculate();

        assert_eq!(size.input_base_bytes, 123); // 41 * 3
        assert_eq!(size.input_witness_bytes, 201); // 67 * 3
    }

    // ==================== OP_RETURN Selection Tests ====================

    #[test]
    fn test_ash_padding_for_small_witness_tx() {
        // Single P2TR needs "ash" to meet 65-byte minimum
        let size = TxSizeCalculator::new()
            .add_input(InputType::P2TR)
            .calculate();

        assert_eq!(size.output_bytes, 14); // "ash"
        assert_eq!(size.base_size, 65); // exactly at minimum
    }

    #[test]
    fn test_empty_for_large_enough_tx() {
        // P2PKH is large enough without padding
        let size = TxSizeCalculator::new()
            .add_input(InputType::P2PKH)
            .calculate();

        assert_eq!(size.output_bytes, 11); // empty
        assert!(size.base_size >= 65);
    }

    #[test]
    fn test_force_ash_op_return() {
        let size = TxSizeCalculator::new()
            .add_input(InputType::P2PKH)
            .calculate_with_op_return(true);

        assert_eq!(size.output_bytes, 14); // forced "ash"
    }

    #[test]
    fn test_force_empty_op_return() {
        let size = TxSizeCalculator::new()
            .add_input(InputType::P2TR)
            .calculate_with_op_return(false);

        assert_eq!(size.output_bytes, 11); // forced empty
        assert!(!size.meets_min_size()); // would fail min size check
    }

    // ==================== Fee Rate Tests ====================

    #[test]
    fn test_fee_rate_calculation() {
        let size = TxSizeCalculator::new()
            .add_input(InputType::P2TR)
            .calculate();

        // 400 sats / 82.25 vB ≈ 4.86 sat/vB
        let rate = size.fee_rate(400);
        assert!((rate - 4.86).abs() < 0.1);
    }

    #[test]
    fn test_min_sats_for_rate() {
        let size = TxSizeCalculator::new()
            .add_input(InputType::P2TR)
            .calculate();

        // For 1 sat/vB with vsize 82.25, need ceil(82.25) = 83 sats
        let min_sats = size.min_sats_for_rate(1.0);
        assert_eq!(min_sats, 83);
    }

    #[test]
    fn test_fee_rates_from_bip_table() {
        // Verify fee rates match BIP specification table
        // Input Amount: 300 sats

        let p2pkh = TxSizeCalculator::new()
            .add_input(InputType::P2PKH)
            .calculate();
        assert!((p2pkh.fee_rate(300) - 1.78).abs() < 0.01);

        let p2wpkh = TxSizeCalculator::new()
            .add_input(InputType::P2WPKH)
            .calculate();
        assert!((p2wpkh.fee_rate(300) - 3.24).abs() < 0.01);

        let p2tr = TxSizeCalculator::new()
            .add_input(InputType::P2TR)
            .calculate();
        assert!((p2tr.fee_rate(300) - 3.65).abs() < 0.01);
    }

    // ==================== Single Input vSize Tests ====================

    #[test]
    fn test_single_input_vsize() {
        // These match the estimates in estimate_input_vsize() in main.rs
        assert_eq!(TxSizeCalculator::single_input_vsize(InputType::P2TR), 57.75);
        assert_eq!(
            TxSizeCalculator::single_input_vsize(InputType::P2WPKH),
            68.0
        );
        assert_eq!(
            TxSizeCalculator::single_input_vsize(InputType::P2PKH),
            148.0
        );

        // P2WSH 2-of-3: (41*4 + 254) / 4 = 104.5
        let p2wsh_vsize =
            TxSizeCalculator::single_input_vsize(InputType::P2WSH(MultisigConfig::new(2, 3)));
        assert_eq!(p2wsh_vsize, 104.5);
    }

    #[test]
    fn test_input_vsize_method() {
        let calc = TxSizeCalculator::new()
            .add_input(InputType::P2PKH)
            .add_input(InputType::P2TR);

        // Combined input vsize
        let vsize = calc.input_vsize();
        // P2PKH: 148, P2TR: 57.75, total: 205.75
        assert_eq!(vsize, 205.75);
    }

    // ==================== BIP Spec Table Verification ====================

    #[test]
    fn test_matches_bip_spec_table() {
        // Verify against the table in docs/bip-dust-disposal.md

        // P2PKH
        let p2pkh = TxSizeCalculator::new()
            .add_input(InputType::P2PKH)
            .calculate();
        assert_eq!(p2pkh.base_size, 169);
        assert_eq!(p2pkh.weight, 676);
        assert_eq!(p2pkh.vsize, 169.0);

        // P2WPKH
        let p2wpkh = TxSizeCalculator::new()
            .add_input(InputType::P2WPKH)
            .calculate();
        assert_eq!(p2wpkh.base_size, 65);
        assert_eq!(p2wpkh.weight, 370);
        assert_eq!(p2wpkh.vsize, 92.5);

        // P2TR
        let p2tr = TxSizeCalculator::new()
            .add_input(InputType::P2TR)
            .calculate();
        assert_eq!(p2tr.base_size, 65);
        assert_eq!(p2tr.weight, 329);
        assert_eq!(p2tr.vsize, 82.25);
    }

    // ==================== Edge Cases ====================

    #[test]
    #[should_panic(expected = "must have at least one input")]
    fn test_empty_calculator_panics() {
        TxSizeCalculator::new().calculate();
    }

    #[test]
    #[should_panic(expected = "required must be > 0")]
    fn test_invalid_multisig_zero_required() {
        MultisigConfig::new(0, 2);
    }

    #[test]
    #[should_panic(expected = "total must be >= required")]
    fn test_invalid_multisig_required_gt_total() {
        MultisigConfig::new(3, 2);
    }

    #[test]
    #[should_panic(expected = "standard multisig limited to 15 keys")]
    fn test_invalid_multisig_too_many_keys() {
        MultisigConfig::new(8, 16);
    }

    #[test]
    fn test_large_multisig() {
        // 11-of-15 is the largest standard multisig
        let size = TxSizeCalculator::new()
            .add_input(InputType::P2WSH(MultisigConfig::new(11, 15)))
            .calculate();

        // Should calculate without panic
        assert!(size.vsize > 0.0);
        assert!(size.meets_min_size());
    }

    // ==================== Batching Helper Tests ====================

    #[test]
    fn test_batching_size_calculation() {
        // Simulate batching: Legacy (555 sats) + Bech32m
        // First tx: overhead + P2PKH + empty OP_RETURN
        let first_tx = TxSizeCalculator::new()
            .add_input(InputType::P2PKH)
            .calculate();

        assert_eq!(first_tx.vsize, 169.0); // 10.5 rounded to 10 for overhead

        // For batching, we need to calculate the combined size
        let batched = TxSizeCalculator::new()
            .add_input(InputType::P2PKH)
            .add_input(InputType::P2TR)
            .calculate();

        // Batched tx always uses empty OP_RETURN
        assert_eq!(batched.output_bytes, 11);
    }
}
