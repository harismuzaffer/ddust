```
  BIP: ?
  Layer: Applications
  Title: Dust UTXO Disposal Protocol
  Authors: bubb1es <bubb1es71@proton.me>
           harris <imtux2@proton.me>
  Status: Draft
  Type: Specification
  Assigned: ?
  License: CC0-1.0
  Discussion: 2026-01-25: https://delvingbitcoin.org/t/disposing-of-dust-attack-utxos/2215
  Version: 0.1.0
```

## Abstract

This BIP specifies a standardized protocol for safely disposing of dust UTXOs by spending them to an OP_RETURN output with the entire value going to transaction fees. The protocol enables wallet software to remove unwanted small-value UTXOs, particularly those received in dust attacks, without creating new address linkages or degrading user privacy. The specification includes transaction format requirements, signature conventions enabling third-party batching, and validation rules for compliant implementations.

## Motivation

### The Dust Attack Problem

Dust attacks are a well-documented privacy threat where attackers send small amounts of bitcoin to numerous addresses. When wallet software later consolidates these dust UTXOs with non-dust UTXOs the attacker can analyze the blockchain to link previously unassociated addresses, potentially deanonymizing users.

The common solution to this issue is to "lock" dust UTXOs and never spend them, but this creates its own problems:

1. **UTXO Set Bloat**: Unspent dust permanently occupies space in the UTXO set that all full nodes must maintain
2. **Wallet Clutter**: Accumulated dust degrades wallet usability and complicates coin selection
3. **Accidental Consolidation**: Users may inadvertently spend dust during legitimate transactions, achieving the attacker's goal
4. **Lock Fragility**: Wallet software that "locks" dust UTXOs to prevent spending provides only temporary protection; wallet migrations, restores from seed phrases, software bugs, or inheritance scenarios can inadvertently unlock dust, exposing users to the original attack

### Why OP_RETURN Disposal

Spending dust to an OP_RETURN output with the entire value going to fees provides several benefits:

1. **No New UTXOs**: OP_RETURN outputs are provably unspendable and not stored in the UTXO set
2. **No Address Linking**: Without a change output, there is no new address to link
3. **Permanent Removal**: The dust UTXOs are removed from the users wallet entirely
4. **Miner Compensation**: OP_RETURN outputs are small, providing higher transaction fees rates
5. **No Cost to Victims**: Dust attack UTXO values are used to pay for their own disposal

### Why Standardization

A standardized protocol enables:

1. **Wallet Anonymity**: transactions with a standard format cannot be used to fingerprint the wallet software a user is running
2. **Third-Party Batching**: multiple dust disposals can be combined into single transactions, reducing overall block space consumption
3. **Best Practice Codification**: Ensures implementations follow privacy-preserving best practices
4. **Easy Identification**: Chain analysis tools can use disposal transactions to help trace the sources of dust attacks

## Specification

### Transaction Format

A compliant dust disposal transaction MUST satisfy all the following requirements:

#### Overall

1. The transaction MUST signal RBF replaceability (nSequence < 0xFFFFFFFE)
2. The ntimelock MUST be set to block height 0
3. The fee rate MUST be at least 0.1 sat/vB

#### Outputs

1. The transaction MUST have exactly one output
2. The single output MUST be an OP_RETURN output
3. The OP_RETURN data MUST be either:
   - Empty: `0x6a 0x00` (OP_RETURN OP_0), or
   - The ASCII string "ash": `0x6a 0x03 0x61 0x73 0x68` (OP_RETURN OP_PUSHBYTES_3 "ash")

The "ash" marker MUST be used when padding is needed to meet the 65 vB minimum standard transaction size with a single witness input. Implementations SHOULD prefer empty OP_RETURN data when the transaction already meets minimum size requirements.

#### Inputs

1. All inputs MUST use the signature hash type `SIGHASH_ALL | SIGHASH_ANYONECANPAY` (0x81)
2. For Taproot (P2TR) inputs using key-path spending, implementations MUST explicitly append the signature hash type byte `SIGHASH_ALL | SIGHASH_ANYONECANPAY` (0x81) to enable ANYONECANPAY semantics, as the default sighash for Taproot (SIGHASH_DEFAULT, which omits the byte) does not include ANYONECANPAY.
3. All inputs must be confirmed in the blockchain at least one block deep

#### Fees

1. The entire input value MUST go to fees (output value is zero for OP_RETURN)
2. The transaction fee rate MUST be at least 0.1 sat/vB to meet minimum relay requirements (Bitcoin Core 30.0+)
3. The transaction fee rate MAY be higher based on the available dust UTXO amounts and transaction size

### Transaction Size

1. The transaction size MUST be at least 65 virtual bytes to meet Bitcoin Core's minimum relay size
2. If the transaction would otherwise be smaller than 65 vB, the "ash" OP_RETURN marker MUST be used to add the necessary bytes

### Address Consolidation Rules

To preserve users privacy implementations:

- MUST NOT consolidate dust UTXOs that were sent to different addresses
- SHOULD consolidate dust UTXOs for dust sent to the same address
- MUST NOT broadcast dust disposal transactions at the same time for dust sent to different addresses

### Batching Dust Disposal Transactions via RBF

Multiple unconfirmed dust disposal transactions created by unrelated entities MAY be batched into a single replacement transaction using Replace-By-Fee (RBF). This is enabled by the SIGHASH_ANYONECANPAY signature type.

#### Batching Requirements

1. The replacement transaction MUST include all inputs from all replaced transactions
2. The replacement transaction MUST signal RBF replaceability (nSequence < 0xFFFFFFFE)
3. The ntimelock MUST be set to block height 0
4. The combined fee rate MUST exceed the highest fee rate among replaced transactions by at least 0.1 sat/vB
5. The replacement MUST pay a higher absolute fee than the sum of all replaced transactions fees

#### Third-Party Batching

A third-party service batching dust disposal transactions could compromise their user's privacy by collecting related network and timing metadata. The best practice for these services is:

1. The service MUST NOT collect pre-signed inputs directly from wallet users
2. The service SHOULD collect pre-signed inputs from the public bitcoin network mempool
3. The service MAY add their own UTXO inputs to improve the batch transaction's fee rate as long as all the requirements of this specification are still followed

This mempool-based approach preserves user privacy while enabling efficient batching:

- Users broadcast their individual dust disposal transactions to the network
- Batching services monitor the mempool for compliant dust disposal transactions
- Services can combine unconfirmed transactions via RBF without knowing user identities

### Dust Threshold

Implementations SHOULD allow users to configure their own dust threshold based on:

1. Current and anticipated fee rates
2. Input script type (different types have different spending costs)
3. Varying amounts that may be used by dust attack initiators

A UTXO is generally considered dust if its value is less than the cost to spend it at a reasonable fee rate, but any small UTXO value could be used in a dust attack.

### Validation Function

A transaction is a valid dust disposal transaction if and only if it meets all the following criteria:

1. **Single Output Requirement**: The transaction must contain exactly one output. Transactions with zero outputs or multiple outputs are not valid dust disposal transactions.
2. **OP_RETURN Output Type**: The single output must be an OP_RETURN output, which creates a provably unspendable output that is not stored in the UTXO set.
3. **OP_RETURN Data Constraint**: The data payload in the OP_RETURN output must be one of two options:
   - Empty (no data), or
   - The three-byte ASCII string "ash" (hexadecimal bytes: 0x61, 0x73, 0x68)
   - Any other data payload makes the transaction non-compliant.
4. **Signature Hash Type**: Every input in the transaction must be signed using the signature hash type `SIGHASH_ALL | SIGHASH_ANYONECANPAY`, which has the hexadecimal value 0x81.

A transaction that fails to meet any of these four requirements is not a valid dust disposal transaction according to this specification.

### Security Considerations

#### Transaction Signing

1. **Key Security**: Signing dust disposal transactions require signing with the wallet's private keys. This could be a risk for cold storage wallets where the key or keys needed to sign are not easily accessible.
2. **Transaction Correctness**: Transaction signers must carefully review and verify that only dust UTXOs are spent and no other inputs are signed.

#### Privacy Preservation

1. **Network surveillance**: Internet service providers and other internet monitors may be able to determine the nodes that initially broadcast a dust disposal transaction, TOR or other privacy preserving overlay networks should be used
2. **Timing Analysis**: Users should be aware that the timing of dust disposal transactions is publicly observable. Dust disposal transactions should not be broadcast at the same time or on a predictable schedule
3. **Amount Analysis**: The specific dust amounts selected for dust disposal if outside the norm may be used to fingerprint the wallet creating the disposal transactions

## Rationale

### Why Empty or "ash" OP_RETURN Data?

1. **Minimal Size**: Empty data (2 bytes: OP_RETURN OP_0) minimizes the transaction size
2. **Standardization**: Consistent transaction construction eliminates wallet fingerprinting
3. **Padding Option**: The "ash" string (5 bytes: OP_RETURN OP_PUSHBYTES_3 "ash") provides a standardized way to meet the minimum transaction size; e.g., for a single P2TR dust input
4. **Semantic Meaning**: "ash" metaphorically represents the result of "burning" the dust

### Why Per-Address Transactions?

Consolidating dust from multiple addresses for the same wallet creates the same privacy harm that dust attacks attempt to achieve. By requiring wallet software to create separate transactions per address (by default), the protocol ensures dust disposal doesn't harm privacy.

### Why 65 vB Minimum?

Bitcoin Core enforces a minimum transaction size of 65 virtual bytes as a policy rule to prevent certain attack vectors. Compliant transactions must meet this threshold to be relayed by standard nodes.

### Why 0.1 sat/vB Minimum Fee Rate?

Bitcoin Core 30.0 reduced the minimum relay fee rate to 0.1 sat/vB (1 sat/kvB). This allows dust UTXOs to be disposed of economically even when their value is very small. Implementations targeting earlier node versions may need higher minimum fee rates.

### Why SIGHASH_ALL|ANYONECANPAY?

The ANYONECANPAY flag allows additional inputs to be added to the dust disposal transaction after signing. This provides several benefits:

1. **Batching**: unrelated dust disposal transactions can be found in the mempool and batched together via RBF
2. **User privacy**: transactions shared via the public mempool do not reveal user identity metadata
3. **Fee Bumping**: additional inputs can be added by unrelated third parties to increase the fee rate

### Why nlocktime block height 0

1. **User privacy**: using the same nlocktime for all dust disposal transactions obscures when it was created
2. **Fee sniping**: the value of disposal transactions should be small enough that fee sniping is not a concern

## Backwards Compatibility

This BIP introduces no changes to the Bitcoin consensus rules or peer-to-peer protocol. All transactions conforming to this specification are valid under existing consensus rules and can be relayed by nodes supporting:

- OP_RETURN outputs (standard since Bitcoin Core 0.9.0)
- SIGHASH_ANYONECANPAY (original Bitcoin feature)
- 0.1 sat/vB minimum relay fee (Bitcoin Core 30.0+)

Nodes running Bitcoin Core versions prior to 30.0 do not relay transactions with fee rates below 1 sat/vB which could slow the relaying of disposal transactions with lower fee rates.

## Reference Implementation

A reference implementation is available at: https://github.com/bubb1es71/ddust

The implementation provides:
- Command-line tool for creating dust disposal transactions
- Automatic dust detection based on configurable thresholds
- Transaction batching via RBF
- Support for P2PKH, P2SH, P2WPKH, P2WSH, and P2TR input descriptors
- Integration with Bitcoin Core (version 30.0+) via RPC for syncing and broadcasting transactions

## Test Cases

The below test cases can be used to verify a wallet disposes of dust UTXOs according to the specification in this BIP. 

### List dust

1. Add descriptors for multiple address types, send dust and non-dust UTXOs, verify that listing dust only returns UTXOs at or below the configured dust threshold (e.g. 1000 sats).
2. Send confirmed and unconfirmed dust UTXO to the wallet, verify listing dust only returns the confirmed dust UTXOs.

### Spending dust

All valid dust disposal transactions should be verified to be accepted into the bitcoind (version 30+) mempool.

1. Spending a single witness (Bech32m/P2TR) dust UTXO must produce a dust disposal transaction with a single "ash" OP_RETURN output.
2. Spending multiple dust UTXOs always produces a single empty OP_RETURN output regardless of script type.
3. Spending a single 2-of-2 P2SH multisig dust UTXO produces a single empty OP_RETURN output.
4. All dust UTXOs sent to the same address are disposed of in the same transaction. 
5. Dust disposal transactions only include confirmed dust UTXOs.

#### Example dust disposal transaction sizes

|                   | P2PKH | P2SH (2-3) | P2WPKH | P2WSH (2-3) | P2TR  |
|-------------------|-------|------------|--------|-------------|-------|
| Overhead (b)      | 10    | 10         | 10     | 10          | 10    | 
| Input (b)         | 148   | 295        | 41     | 41          | 41    |
| OP_RETURN (b)     | 11    | 11         | 14     | 14          | 14    |
| Base size (b)     | 169   | 316        | 65     | 65          | 65    |
| Witness data (b)  | 0     | 0          | 108    | 255         | 67    |
| Size (b)          | 169   | 316        | 173    | 320         | 132   |
| Weight (wu)       | 676   | 1264       | 370    | 517         | 329   |
| Virtual Size (vb) | 169   | 316        | 92.5   | 129.25      | 82.25 | 

#### Example dust disposal transaction fee rates (sats/vb)

| Input Amount | P2PKH   | P2SH (2-3) | P2WPKH | P2WSH (2-3) | P2TR  |
|--------------|---------|------------|--------|-------------|-------|
| 294          | 1.74    | 0.93       | 3.18   | 2.27        | 3.57  |
| 300          | 1.78    | 0.95       | 3.24   | 2.32        | 3.65  |
| 325          | 1.92    | 1.03       | 3.51   | 2.51        | 3.95  |
| 330          | 1.95    | 1.04       | 3.57   | 2.55        | 4.01  |

### Batching dust disposal txs via RBF

1. Adding a Bech32m dust input to an unconfirmed disposal transaction with a legacy dust input keeps the original single empty OP_RETURN output.
2. Adding a Bech32m dust input to an unconfirmed disposal transaction with a Bech32m dust input keeps the original single "ash" OP_RETURN output.
3. A new dust input is always added to an unconfirmed disposal transaction with one or more inputs as long as the new fee rate is sufficient for RBF.
4. New dust inputs are added to a new, unbatched dust disposal transaction when adding them to an unconfirmed disposal transaction has an insufficient fee rate for RBF.

## Related work

* "dust-b-gone": https://github.com/petertodd/dust-b-gone
* "dusts":  https://github.com/bubb1es71/dusts

## Changelog

* **0.1.0** (2026-03-22):
    * Initial draft of the BIP.

## Copyright

This document is licensed under the Creative Commons CC0 1.0 Universal license.
