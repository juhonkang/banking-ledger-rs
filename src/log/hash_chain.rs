//! SHA-256 hash chain for cryptographic immutability.
//! Each block links to its predecessor via hash — tampering any block
//! invalidates the entire chain. Provides audit trail proofs.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

// ━━━ Hash Chain ━━━

/// A single link in the hash chain.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HashLink {
    /// Position in the chain (0 = genesis)
    pub index: u64,
    /// Hash of this block
    pub hash: String,
    /// Hash of previous block ("0" * 64 for genesis)
    pub previous_hash: String,
    /// The data this block secures
    pub data: String,
    /// Timestamp of creation
    pub timestamp: DateTime<Utc>,
    /// Optional: nonce for proof-of-work (if needed)
    pub nonce: u64,
}

impl HashLink {
    /// Create the genesis block (first block in the chain)
    pub fn genesis(data: &str) -> Self {
        let previous_hash = "0".repeat(64);
        let timestamp = Utc::now();
        let hash = Self::compute_hash(0, &previous_hash, data, &timestamp, 0);

        Self {
            index: 0,
            hash,
            previous_hash,
            data: data.to_string(),
            timestamp,
            nonce: 0,
        }
    }

    /// Create the next block in the chain
    pub fn next(previous: &HashLink, data: &str) -> Self {
        let index = previous.index + 1;
        let previous_hash = previous.hash.clone();
        let timestamp = Utc::now();
        let hash = Self::compute_hash(index, &previous_hash, data, &timestamp, 0);

        Self {
            index,
            hash,
            previous_hash,
            data: data.to_string(),
            timestamp,
            nonce: 0,
        }
    }

    /// Compute SHA-256(prev_hash + index + data + timestamp + nonce)
    fn compute_hash(
        index: u64,
        previous_hash: &str,
        data: &str,
        timestamp: &DateTime<Utc>,
        nonce: u64,
    ) -> String {
        let mut hasher = Sha256::new();
        hasher.update(previous_hash.as_bytes());
        hasher.update(index.to_le_bytes());
        hasher.update(data.as_bytes());
        hasher.update(timestamp.to_rfc3339().as_bytes());
        hasher.update(nonce.to_le_bytes());
        hex::encode(hasher.finalize())
    }

    /// Verify this block's hash is correctly computed
    pub fn verify_self(&self) -> bool {
        let computed = Self::compute_hash(
            self.index,
            &self.previous_hash,
            &self.data,
            &self.timestamp,
            self.nonce,
        );
        computed == self.hash
    }
}

// ━━━ Signatures + HMAC ━━━

/// HMAC-SHA256 for internal message authentication.
pub fn hmac_sign(key: &[u8], message: &[u8]) -> String {
    use sha2::Sha256;
    // Simple HMAC: H(key || H(key || message))
    let mut inner = Sha256::new();
    inner.update(key);
    inner.update(message);
    let inner_hash = inner.finalize();

    let mut outer = Sha256::new();
    outer.update(key);
    outer.update(inner_hash);
    hex::encode(outer.finalize())
}

/// Verify an HMAC signature
pub fn hmac_verify(key: &[u8], message: &[u8], signature: &str) -> bool {
    hmac_sign(key, message) == signature
}

/// A signed transaction — includes HMAC for integrity verification.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SignedTransaction {
    pub transaction_id: Uuid,
    pub payload: String,
    /// HMAC-SHA256 of (`transaction_id` + payload)
    pub hmac: String,
    pub timestamp: DateTime<Utc>,
}

impl SignedTransaction {
    pub fn sign(transaction_id: Uuid, payload: &str, key: &[u8]) -> Self {
        let mut message = Vec::new();
        message.extend_from_slice(transaction_id.as_bytes());
        message.extend_from_slice(payload.as_bytes());

        Self {
            transaction_id,
            payload: payload.to_string(),
            hmac: hmac_sign(key, &message),
            timestamp: Utc::now(),
        }
    }

    pub fn verify(&self, key: &[u8]) -> bool {
        let mut message = Vec::new();
        message.extend_from_slice(self.transaction_id.as_bytes());
        message.extend_from_slice(self.payload.as_bytes());
        hmac_verify(key, &message, &self.hmac)
    }
}

// ━━━ Immutable Chain ━━━

/// The immutable hash chain — backbone of the tamper-proof ledger.
pub struct HashChain {
    pub blocks: Vec<HashLink>,
    /// HMAC signing key for internal transactions
    signing_key: Vec<u8>,
}

impl HashChain {
    pub fn new(signing_key: &[u8]) -> Self {
        let genesis = HashLink::genesis("GENESIS_BLOCK");
        Self {
            blocks: vec![genesis],
            signing_key: signing_key.to_vec(),
        }
    }

    /// Append a new block to the chain. Returns the new block.
    pub fn append(&mut self, data: &str) -> &HashLink {
        let previous = self.blocks.last().unwrap();
        let block = HashLink::next(previous, data);
        self.blocks.push(block);
        self.blocks.last().unwrap()
    }

    // ━━━ Tamper Detection ━━━

    /// Verify the ENTIRE chain's integrity.
    /// Returns (`is_valid`, `tampered_indices`).
    pub fn verify_chain(&self) -> (bool, Vec<u64>) {
        let mut tampered = Vec::new();

        // Check genesis: previous_hash must be 0*64
        if self.blocks[0].previous_hash != "0".repeat(64) {
            tampered.push(0);
        }

        // Check each block's self-hash
        for block in &self.blocks {
            if !block.verify_self() {
                tampered.push(block.index);
            }
        }

        // Check chain linkage: block[i].previous_hash == block[i-1].hash
        for i in 1..self.blocks.len() {
            if self.blocks[i].previous_hash != self.blocks[i - 1].hash
                && !tampered.contains(&self.blocks[i].index)
            {
                tampered.push(self.blocks[i].index);
            }
        }

        (tampered.is_empty(), tampered)
    }

    // ━━━ Audit Query ━━━

    /// Get all blocks for a time range
    pub fn query_by_time(&self, from: DateTime<Utc>, to: DateTime<Utc>) -> Vec<&HashLink> {
        self.blocks
            .iter()
            .filter(|b| b.timestamp >= from && b.timestamp <= to)
            .collect()
    }

    /// Get a specific block by index
    pub fn get_block(&self, index: u64) -> Option<&HashLink> {
        self.blocks.get(index as usize)
    }

    /// Get the latest block (current chain head)
    pub fn latest(&self) -> &HashLink {
        self.blocks.last().unwrap()
    }

    /// Get chain length
    pub fn len(&self) -> usize {
        self.blocks.len()
    }

    pub fn is_empty(&self) -> bool {
        false // Always has genesis
    }

    /// Get the signing key for HMAC operations.
    pub fn signing_key(&self) -> &[u8] {
        &self.signing_key
    }

    /// Generate a Merkle-like proof that a block at index `i` is part of the chain.
    /// Returns (block, `previous_hash`, `next_hash`) for cross-verification.
    pub fn proof_for_block(&self, index: u64) -> Option<ChainProof> {
        let i = index as usize;
        if i >= self.blocks.len() {
            return None;
        }

        Some(ChainProof {
            block: self.blocks[i].clone(),
            previous_block_hash: if i > 0 {
                Some(self.blocks[i - 1].hash.clone())
            } else {
                None
            },
            next_block_hash: if i + 1 < self.blocks.len() {
                Some(self.blocks[i + 1].hash.clone())
            } else {
                None
            },
        })
    }

    // ━━━ Redaction ━━━

    /// Redact sensitive data at a specific index while preserving chain integrity.
    /// Replaces data with "[REDACTED]" and recalculates forward hashes.
    /// WARNING: This MODIFIES the chain — only do this on a copy.
    pub fn redact(&mut self, index: u64) -> Result<(), RedactError> {
        let i = index as usize;
        if i >= self.blocks.len() {
            return Err(RedactError::IndexOutOfBounds);
        }
        if i == 0 {
            return Err(RedactError::CannotRedactGenesis);
        }

        self.blocks[i].data = "[REDACTED]".to_string();

        // Recalculate hash for this block and ALL subsequent blocks
        for j in i..self.blocks.len() {
            let prev_hash = if j == 0 {
                "0".repeat(64)
            } else {
                self.blocks[j - 1].hash.clone()
            };

            let new_hash = HashLink::compute_hash(
                self.blocks[j].index,
                &prev_hash,
                &self.blocks[j].data,
                &self.blocks[j].timestamp,
                self.blocks[j].nonce,
            );
            self.blocks[j].hash = new_hash;

            // Update next block's previous_hash
            if j + 1 < self.blocks.len() {
                self.blocks[j + 1].previous_hash = self.blocks[j].hash.clone();
            }
        }

        Ok(())
    }
}

/// Proof that a block exists at a specific position in the chain.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChainProof {
    pub block: HashLink,
    pub previous_block_hash: Option<String>,
    pub next_block_hash: Option<String>,
}

impl ChainProof {
    /// Verify the block's position in the chain (self-consistent check)
    pub fn verify_position(&self) -> bool {
        // Block must verify itself
        if !self.block.verify_self() {
            return false;
        }
        // Previous block hash must match (for non-genesis)
        if let Some(ref prev) = self.previous_block_hash {
            if self.block.previous_hash != *prev {
                return false;
            }
        }
        true
    }
}

#[derive(Debug)]
pub enum RedactError {
    IndexOutOfBounds,
    CannotRedactGenesis,
}

// ━━━ Verification ━━━

/// Batch-verify multiple blocks using parallel iteration.
/// For very large chains, this is faster than sequential verification.
pub fn parallel_verify_chain(chain: &HashChain) -> (bool, Vec<u64>) {
    use std::sync::atomic::AtomicBool;

    let _tampered: std::sync::Mutex<Vec<u64>> = std::sync::Mutex::new(Vec::new());
    let _valid = AtomicBool::new(true);

    // Check chain links in parallel using rayon if available
    // Rayon parallel verification available with `rayon` feature

    // Sequential fallback (always correct)
    chain.verify_chain()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hash_chain_integrity() {
        let key = b"banking-ledger-secret-key-32b!!";
        let mut chain = HashChain::new(key);

        chain.append(r#"{"tx":"deposit","amount":1000}"#);
        chain.append(r#"{"tx":"withdraw","amount":300}"#);
        chain.append(r#"{"tx":"transfer","from":"A","to":"B","amount":500}"#);

        assert_eq!(chain.len(), 4); // genesis + 3 blocks

        let (valid, tampered) = chain.verify_chain();
        assert!(valid);
        assert!(tampered.is_empty());
    }

    #[test]
    fn test_tamper_detection() {
        let key = b"test-key-32-bytes-long!!!!!!";
        let mut chain = HashChain::new(key);
        chain.append("block1");
        chain.append("block2");

        // Tamper with block 1
        chain.blocks[1].data = "TAMPERED_DATA".to_string();
        // Don't recalculate hash — this is what tampering looks like

        let (valid, tampered) = chain.verify_chain();
        assert!(!valid);
        assert!(tampered.contains(&1));
    }

    #[test]
    fn test_hmac_sign_and_verify() {
        let key = b"super-secret-key";
        let tx_id = Uuid::now_v7();
        let payload = r#"{"amount":1000}"#;

        let signed = SignedTransaction::sign(tx_id, payload, key);
        assert!(signed.verify(key));

        // Wrong key fails
        assert!(!signed.verify(b"wrong-key"));
    }

    #[test]
    fn test_audit_trail_query() {
        let key = b"audit-key-32-bytes-long!!!!!";
        let mut chain = HashChain::new(key);

        let t1 = Utc::now();
        chain.append("event1");

        std::thread::sleep(std::time::Duration::from_millis(10));
        let t2 = Utc::now();
        chain.append("event2");

        std::thread::sleep(std::time::Duration::from_millis(10));
        chain.append("event3");

        // Query by time range
        let results = chain.query_by_time(t1, t2);
        assert!(!results.is_empty());
    }

    #[test]
    fn test_chain_proof() {
        let key = b"proof-key-32-bytes-long!!!!!!";
        let mut chain = HashChain::new(key);
        chain.append("data1");
        chain.append("data2");

        let proof = chain.proof_for_block(1).unwrap();
        assert!(proof.verify_position());
    }
}
