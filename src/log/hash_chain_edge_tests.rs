//! Edge case tests for HashChain: redaction, proof verification, tamper detection,
//! chain integrity under various attack vectors, parallel verification stub.

#[cfg(test)]
mod hash_chain_edge_tests {
    use uuid::Uuid;
    use chrono::Utc;

    use crate::log::hash_chain::{
        HashChain, HashLink, SignedTransaction, ChainProof,
        hmac_sign, hmac_verify, RedactError,
    };

    fn test_key() -> &'static [u8] {
        b"test-chain-key-32-bytes-long!!"
    }

    // ━━━ HashChain ━━━

    #[test]
    fn test_chain_genesis_always_present() {
        let chain = HashChain::new(test_key());
        assert_eq!(chain.len(), 1);
        assert!(!chain.is_empty());
        assert_eq!(chain.latest().unwrap().index, 0);
    }

    #[test]
    fn test_chain_append_increases_length() {
        let mut chain = HashChain::new(test_key());
        chain.append("tx1");
        chain.append("tx2");
        assert_eq!(chain.len(), 3);
    }

    #[test]
    fn test_chain_verify_empty_is_genesis_only() {
        let chain = HashChain::new(test_key());
        let (valid, tampered) = chain.verify_chain();
        assert!(valid);
        assert!(tampered.is_empty());
    }

    #[test]
    fn test_chain_tamper_data_detected() {
        let mut chain = HashChain::new(test_key());
        chain.append("tx1");
        chain.blocks[1].data = "CORRUPTED".to_string();
        let (valid, tampered) = chain.verify_chain();
        assert!(!valid);
        assert!(tampered.contains(&1));
    }

    #[test]
    fn test_chain_tamper_hash_detected() {
        let mut chain = HashChain::new(test_key());
        chain.append("tx1");
        chain.blocks[1].hash = "deadbeef".repeat(8); // 64 hex chars
        let (valid, tampered) = chain.verify_chain();
        assert!(!valid);
    }

    #[test]
    fn test_chain_tamper_previous_hash_breaks_linkage() {
        let mut chain = HashChain::new(test_key());
        chain.append("tx1");
        chain.append("tx2");
        // Break link: block 2 no longer points to block 1's hash
        chain.blocks[2].previous_hash = "0".repeat(64);
        let (valid, tampered) = chain.verify_chain();
        assert!(!valid);
        assert!(tampered.contains(&2));
    }

    #[test]
    fn test_chain_sequential_integrity() {
        let mut chain = HashChain::new(test_key());
        for i in 0..100 {
            chain.append(&format!("tx{i}"));
        }
        let (valid, _) = chain.verify_chain();
        assert!(valid);
    }

    #[test]
    fn test_chain_get_block_out_of_bounds() {
        let chain = HashChain::new(test_key());
        assert!(chain.get_block(999).is_none());
        assert!(chain.get_block(0).is_some()); // Genesis
    }

    #[test]
    fn test_chain_query_by_time_empty_range() {
        let mut chain = HashChain::new(test_key());
        chain.append("tx1");
        // Query: from future to past (empty range)
        let future = Utc::now() + chrono::Duration::hours(1);
        let past = Utc::now() - chrono::Duration::hours(1);
        let results = chain.query_by_time(future, past);
        assert!(results.is_empty());
    }

    #[test]
    fn test_chain_proof_for_block_bounds() {
        let mut chain = HashChain::new(test_key());
        chain.append("data");
        assert!(chain.proof_for_block(999).is_none());
        let genesis_proof = chain.proof_for_block(0).unwrap();
        assert!(genesis_proof.previous_block_hash.is_none()); // Genesis has no predecessor
        assert!(genesis_proof.next_block_hash.is_some());     // Has successor
    }

    #[test]
    fn test_chain_proof_verify_position() {
        let mut chain = HashChain::new(test_key());
        chain.append("data1");
        chain.append("data2");

        let proof = chain.proof_for_block(1).unwrap();
        assert!(proof.verify_position());

        // Corrupt the proof
        let mut bad_proof = proof.clone();
        bad_proof.block.data = "TAMPERED".to_string();
        assert!(!bad_proof.verify_position());
    }

    // ━━━ Redaction ━━━

    #[test]
    fn test_redact_block_succeeds() {
        let mut chain = HashChain::new(test_key());
        chain.append("sensitive data");
        chain.redact(1).unwrap();
        assert_eq!(chain.blocks[1].data, "[REDACTED]");
    }

    #[test]
    fn test_redact_cannot_touch_genesis() {
        let mut chain = HashChain::new(test_key());
        let result = chain.redact(0);
        assert!(result.is_err());
        // Check error variant
        match result {
            Err(RedactError::CannotRedactGenesis) => {},
            _ => panic!("expected CannotRedactGenesis"),
        }
    }

    #[test]
    fn test_redact_out_of_bounds() {
        let mut chain = HashChain::new(test_key());
        let result = chain.redact(999);
        assert!(result.is_err());
    }

    #[test]
    fn test_redact_keeps_chain_valid() {
        let mut chain = HashChain::new(test_key());
        chain.append("tx1");
        chain.append("tx2");
        chain.append("tx3");
        chain.redact(2).unwrap(); // Redact tx2
        let (valid, _) = chain.verify_chain();
        assert!(valid, "redacted chain should remain valid");
    }

    // ━━━ HMAC ━━━

    #[test]
    fn test_hmac_deterministic() {
        let sig1 = hmac_sign(b"key", b"message");
        let sig2 = hmac_sign(b"key", b"message");
        assert_eq!(sig1, sig2);
    }

    #[test]
    fn test_hmac_different_key_different_output() {
        let sig1 = hmac_sign(b"key1", b"message");
        let sig2 = hmac_sign(b"key2", b"message");
        assert_ne!(sig1, sig2);
    }

    #[test]
    fn test_hmac_different_message_different_output() {
        let sig1 = hmac_sign(b"key", b"msg1");
        let sig2 = hmac_sign(b"key", b"msg2");
        assert_ne!(sig1, sig2);
    }

    #[test]
    fn test_hmac_verify_works() {
        let key = b"my-secret-key";
        let msg = b"important data";
        let sig = hmac_sign(key, msg);
        assert!(hmac_verify(key, msg, &sig));
        assert!(!hmac_verify(b"wrong-key", msg, &sig));
        assert!(!hmac_verify(key, b"wrong-msg", &sig));
    }

    // ━━━ SignedTransaction ━━━

    #[test]
    fn test_signed_transaction_roundtrip() {
        let tx_id = Uuid::now_v7();
        let signed = SignedTransaction::sign(tx_id, "payload", test_key());
        assert!(signed.verify(test_key()));
    }

    #[test]
    fn test_signed_transaction_tamper_detected() {
        let tx_id = Uuid::now_v7();
        let mut signed = SignedTransaction::sign(tx_id, "payload", test_key());
        signed.payload = "TAMPERED".to_string();
        assert!(!signed.verify(test_key()));
    }

    #[test]
    fn test_signed_transaction_different_tx_id_fails() {
        let tx_id1 = Uuid::now_v7();
        let tx_id2 = Uuid::now_v7();
        let signed = SignedTransaction::sign(tx_id1, "payload", test_key());
        // Tamper: change tx_id but keep hmac
        let tampered = SignedTransaction {
            transaction_id: tx_id2,
            payload: signed.payload.clone(),
            hmac: signed.hmac.clone(),
            timestamp: signed.timestamp,
        };
        assert!(!tampered.verify(test_key()));
    }
}
