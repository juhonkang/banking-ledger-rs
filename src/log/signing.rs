//! Digital signatures for financial transactions (ed25519).
//! Every transaction hash is signed, every signature is verified.
//! Any tampering is detected through SHA-256 + Ed25519.

use chrono::{DateTime, Utc};
use ed25519_dalek::{Signer, SigningKey, Verifier, VerifyingKey, Signature};
use rust_decimal::Decimal;
use sha2::{Sha256, Digest};
use uuid::Uuid;

// ━━━ Transaction Model ━━━

/// Status of a signed transaction in the verification pipeline.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TxStatus {
    Created,
    Signed,
    Verified,
    Invalid,
    Finalized,
}

/// An immutable financial transaction ready for cryptographic signing.
#[derive(Debug, Clone)]
pub struct Transaction {
    pub sender: String,
    pub recipient: String,
    pub amount: Decimal,
    pub timestamp: DateTime<Utc>,
    pub correlation_id: Uuid,
    pub hash: Vec<u8>,
    pub signature: Option<Vec<u8>>,
}

impl Transaction {
    /// Create a new unsigned transaction.
    pub fn new(sender: &str, recipient: &str, amount: Decimal) -> Self {
        let ts = Utc::now();
        let cid = Uuid::now_v7();
        let mut tx = Self {
            sender: sender.to_string(),
            recipient: recipient.to_string(),
            amount,
            timestamp: ts,
            correlation_id: cid,
            hash: vec![],
            signature: None,
        };
        tx.hash = tx.compute_hash();
        tx
    }

    /// Deterministic canonical byte representation for hashing.
    /// Order: sender | recipient | amount | timestamp | correlation_id
    pub fn canonical_bytes(&self) -> Vec<u8> {
        let mut buf = Vec::new();
        buf.extend_from_slice(self.sender.as_bytes());
        buf.push(b'|');
        buf.extend_from_slice(self.recipient.as_bytes());
        buf.push(b'|');
        buf.extend_from_slice(self.amount.to_string().as_bytes());
        buf.push(b'|');
        buf.extend_from_slice(self.timestamp.to_rfc3339().as_bytes());
        buf.push(b'|');
        buf.extend_from_slice(self.correlation_id.to_string().as_bytes());
        buf
    }

    /// SHA-256 hash of canonical bytes.
    pub fn compute_hash(&self) -> Vec<u8> {
        let mut hasher = Sha256::new();
        hasher.update(self.canonical_bytes());
        hasher.finalize().to_vec()
    }
}

// ━━━ Signed Transaction ━━━

/// A transaction with attached cryptographic signature and verifier's public key.
#[derive(Debug, Clone)]
pub struct SignedTransaction {
    pub transaction: Transaction,
    pub signature: Vec<u8>,
    pub public_key: Vec<u8>,
}

// ━━━ Signing Module ━━━

/// Generates keypairs and signs transactions with Ed25519.
pub struct SigningModule;

impl SigningModule {
    /// Generate a new Ed25519 keypair using OS randomness.
    pub fn generate_keypair() -> (SigningKey, VerifyingKey) {
        let mut csprng = rand::rngs::OsRng;
        let signing_key = SigningKey::generate(&mut csprng);
        let verifying_key = signing_key.verifying_key();
        (signing_key, verifying_key)
    }

    /// Sign a transaction's hash with Ed25519.
    /// Returns the signature bytes.
    pub fn sign(tx: &Transaction, signing_key: &SigningKey) -> Result<Vec<u8>, SigningError> {
        if tx.hash.is_empty() {
            return Err(SigningError::MissingHash);
        }
        let signature: Signature = signing_key.sign(&tx.hash);
        Ok(signature.to_bytes().to_vec())
    }

    /// Sign and return a SignedTransaction.
    pub fn sign_transaction(
        tx: &Transaction,
        signing_key: &SigningKey,
        verifying_key: &VerifyingKey,
    ) -> Result<SignedTransaction, SigningError> {
        let signature = Self::sign(tx, signing_key)?;
        Ok(SignedTransaction {
            transaction: tx.clone(),
            signature,
            public_key: verifying_key.as_bytes().to_vec(),
        })
    }
}

// ━━━ Signature Verifier ━━━

/// Verifies Ed25519 signatures on transactions.
pub struct SignatureVerifier;

impl SignatureVerifier {
    /// Verify a transaction's signature against its hash and public key.
    pub fn verify(
        tx: &Transaction,
        public_key_bytes: &[u8],
        signature_bytes: &[u8],
    ) -> Result<bool, SigningError> {
        if tx.hash.is_empty() {
            return Err(SigningError::MissingHash);
        }

        let verifying_key = VerifyingKey::from_bytes(
            &public_key_bytes
                .try_into()
                .map_err(|_| SigningError::InvalidPublicKey)?
        ).map_err(|_| SigningError::InvalidPublicKey)?;

        let signature = Signature::from_bytes(
            signature_bytes
                .try_into()
                .map_err(|_| SigningError::InvalidSignature)?
        );

        Ok(verifying_key.verify(&tx.hash, &signature).is_ok())
    }

    /// Detect tampering: re-hash and verify signature.
    /// Returns false if signature is invalid OR hash doesn't match recomputed hash.
    pub fn verify_tamper_detection(
        tx: &Transaction,
        public_key_bytes: &[u8],
        signature_bytes: &[u8],
    ) -> bool {
        // Re-hash canonical bytes
        let recomputed = tx.compute_hash();
        if recomputed != tx.hash {
            return false; // Tampered — hash changed
        }
        Self::verify(tx, public_key_bytes, signature_bytes).unwrap_or(false)
    }
}

// ━━━ Errors ━━━

#[derive(Debug, thiserror::Error)]
pub enum SigningError {
    #[error("Transaction has no hash — call compute_hash() first")]
    MissingHash,
    #[error("Invalid public key bytes")]
    InvalidPublicKey,
    #[error("Invalid signature bytes")]
    InvalidSignature,
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal_macros::dec;

    #[test]
    fn test_sign_and_verify() {
        let tx = Transaction::new("alice", "bob", dec!(100.00));
        let (signing_key, verifying_key) = SigningModule::generate_keypair();

        let signature = SigningModule::sign(&tx, &signing_key).unwrap();
        let result = SignatureVerifier::verify(
            &tx,
            verifying_key.as_bytes(),
            &signature,
        )
        .unwrap();

        assert!(result);
    }

    #[test]
    fn test_tamper_detection() {
        let tx = Transaction::new("alice", "bob", dec!(100.00));
        let (signing_key, verifying_key) = SigningModule::generate_keypair();
        let signature = SigningModule::sign(&tx, &signing_key).unwrap();

        // Tamper with the amount
        let mut tampered = tx.clone();
        tampered.amount = dec!(999999.99);
        // Attacker forgot to recompute hash — old hash still stored
        // tampered.hash ets same as original

        let result = SignatureVerifier::verify_tamper_detection(
            &tampered,
            verifying_key.as_bytes(),
            &signature,
        );
        assert!(!result, "Tampered transaction must be detected");
    }

    #[test]
    fn test_canonical_bytes_deterministic() {
        let tx1 = Transaction::new("alice", "bob", dec!(42.50));
        let tx2 = Transaction {
            sender: "alice".into(),
            recipient: "bob".into(),
            amount: dec!(42.50),
            timestamp: tx1.timestamp,
            correlation_id: tx1.correlation_id,
            hash: vec![],
            signature: None,
        };

        assert_eq!(tx1.canonical_bytes(), tx2.canonical_bytes());
        assert_eq!(tx1.compute_hash(), tx2.compute_hash());
    }

    #[test]
    fn test_wrong_key_fails() {
        let tx = Transaction::new("alice", "bob", dec!(100.00));
        let (signing_key, _) = SigningModule::generate_keypair();
        let (_, other_verifying_key) = SigningModule::generate_keypair();
        let signature = SigningModule::sign(&tx, &signing_key).unwrap();

        let result = SignatureVerifier::verify(
            &tx,
            other_verifying_key.as_bytes(),
            &signature,
        )
        .unwrap();

        assert!(!result, "Wrong public key must fail verification");
    }
}
