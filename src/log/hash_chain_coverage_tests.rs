//! Targeted coverage tests for HashChain: tamper, empty, redact, proof, HMAC.
//! Each test exercises a specific verification path with explicit assertions.

#[cfg(test)]
mod hash_chain_coverage_tests {
    use uuid::Uuid;

    use crate::log::hash_chain::{
        HashChain, SignedTransaction, ChainProof,
        hmac_sign, hmac_verify, RedactError,
    };

    fn coverage_key() -> &'static [u8] {
        b"coverage-key-32-bytes-long!!\0"
    }

    // ━━━ 1) Tamper detection on block N ━━━

    /// Tamper with data at a specific block index N and verify it is detected.
    /// Tests that verify_chain() correctly flags the exact tampered index.
    #[test]
    fn test_tamper_detection_block_n_data() {
        let mut chain = HashChain::new(coverage_key());
        chain.append("tx-A");
        chain.append("tx-B");
        chain.append("tx-C");
        chain.append("tx-D");

        // Tamper with block 2 (index 2 — the 3rd block after genesis)
        chain.blocks[2].data = "COMPROMISED_TX".to_string();

        let (valid, tampered) = chain.verify_chain();
        assert!(!valid, "chain must be invalid after tampering");
        assert!(tampered.contains(&2), "tampered indices must include index 2");
        assert_eq!(tampered.len(), 1, "only one index should be tampered");
    }

    /// Tamper with a block's hash directly and verify detection.
    #[test]
    fn test_tamper_detection_block_n_hash() {
        let mut chain = HashChain::new(coverage_key());
        chain.append("tx-1");
        chain.append("tx-2");
        chain.append("tx-3");

        // Directly corrupt block 3's hash
        chain.blocks[3].hash = "a".repeat(64);

        let (valid, tampered) = chain.verify_chain();
        assert!(!valid, "chain must be invalid after hash corruption");
        assert!(
            tampered.contains(&3),
            "tampered indices must include block 3; got {:?}",
            tampered
        );
    }

    /// Tamper with previous_hash linkage — block N no longer points to N-1's hash.
    #[test]
    fn test_tamper_detection_broken_linkage_at_n() {
        let mut chain = HashChain::new(coverage_key());
        chain.append("block-1");
        chain.append("block-2");
        chain.append("block-3");
        chain.append("block-4");

        // Break the link: block 3's previous_hash no longer matches block 2's hash
        chain.blocks[3].previous_hash = "0".repeat(64);

        let (valid, tampered) = chain.verify_chain();
        assert!(!valid, "broken linkage must invalidate chain");
        assert!(
            tampered.contains(&3),
            "block 3 must be in tampered list due to broken previous_hash"
        );
    }

    // ━━━ 2) Empty chain verify ━━━

    /// A freshly created chain contains only genesis and must verify as valid.
    #[test]
    fn test_empty_chain_verifies() {
        let chain = HashChain::new(coverage_key());

        assert_eq!(chain.len(), 1, "empty chain has genesis only");
        assert!(!chain.is_empty(), "chain is never empty — genesis always present");

        let (valid, tampered) = chain.verify_chain();
        assert!(valid, "fresh genesis-only chain must verify");
        assert!(tampered.is_empty(), "no tampered blocks in fresh chain");
    }

    /// Genesis block must have previous_hash of 64 zeros; verify this invariant.
    #[test]
    fn test_empty_chain_genesis_previous_hash_is_zeroes() {
        let chain = HashChain::new(coverage_key());
        let genesis = chain.get_block(0).expect("genesis must exist");

        assert_eq!(genesis.index, 0);
        assert_eq!(genesis.previous_hash, "0".repeat(64));
        assert_eq!(genesis.data, "GENESIS_BLOCK");
    }

    /// Verify a chain where genesis itself has been corrupted.
    #[test]
    fn test_empty_chain_genesis_tampered() {
        let mut chain = HashChain::new(coverage_key());
        // Corrupt genesis data
        chain.blocks[0].data = "EVIL_GENESIS".to_string();
        // But also corrupt its hash to match (simulating a sophisticated attack)
        // Actually, just corrupt data — the hash won't match now
        let (_valid, tampered) = chain.verify_chain();
        assert!(!_valid, "corrupted genesis must fail verification");
        assert!(tampered.contains(&0), "genesis must be flagged as tampered");
    }

    // ━━━ 3) Redact and verify ━━━

    /// Redact a block and confirm the chain remains intact after rehashing.
    #[test]
    fn test_redact_single_block_keeps_chain_valid() {
        let mut chain = HashChain::new(coverage_key());
        chain.append("sensitive-1");
        chain.append("sensitive-2");
        chain.append("public-3");
        chain.append("public-4");

        // Redact block 1 (first appended block after genesis)
        chain.redact(1).expect("redact index 1 must succeed");

        assert_eq!(
            chain.blocks[1].data, "[REDACTED]",
            "block 1 data must be replaced with [REDACTED]"
        );

        let (valid, _tampered) = chain.verify_chain();
        assert!(
            valid,
            "redacted chain must pass full verification after forward rehash"
        );
    }

    /// Redact a middle block and verify forward hashes were recalculated correctly.
    #[test]
    fn test_redact_middle_block_forward_rehash() {
        let mut chain = HashChain::new(coverage_key());
        chain.append("tx-alpha");
        chain.append("tx-beta");
        chain.append("tx-gamma");
        chain.append("tx-delta");
        chain.append("tx-epsilon");

        // Capture original hashes before redaction
        let original_block_3_hash = chain.blocks[3].hash.clone();
        let original_block_4_hash = chain.blocks[4].hash.clone();
        let original_block_5_hash = chain.blocks[5].hash.clone();

        // Redact block 2 (index 2)
        chain.redact(2).expect("redact index 2 must succeed");

        // Block 1 (before redacted) should be unchanged
        assert_eq!(chain.blocks[1].data, "tx-alpha");

        // Block 2 data is redacted, hash changed
        assert_eq!(chain.blocks[2].data, "[REDACTED]");
        assert_ne!(
            chain.blocks[2].hash, original_block_3_hash,
            "redacted block's hash must change"
        );

        // All subsequent blocks must have new hashes
        assert_ne!(chain.blocks[3].hash, original_block_3_hash, "forward hash must change");
        assert_ne!(chain.blocks[4].hash, original_block_4_hash, "forward hash must change");
        assert_ne!(chain.blocks[5].hash, original_block_5_hash, "forward hash must change");

        // Chain must remain valid
        let (valid, _) = chain.verify_chain();
        assert!(valid, "chain after forward rehash must be fully valid");
    }

    /// Redaction of genesis must be rejected.
    #[test]
    fn test_redact_genesis_rejected() {
        let mut chain = HashChain::new(coverage_key());
        chain.append("tx-1");

        let result = chain.redact(0);
        assert!(result.is_err(), "redacting genesis must fail");

        match result {
            Err(RedactError::CannotRedactGenesis) => {} // expected
            other => panic!("expected CannotRedactGenesis, got {:?}", other),
        }
    }

    /// Redaction of out-of-bounds index must be rejected.
    #[test]
    fn test_redact_out_of_bounds_rejected() {
        let mut chain = HashChain::new(coverage_key());
        chain.append("tx-1");

        let result = chain.redact(5); // only indices 0 and 1 exist
        assert!(result.is_err(), "redacting beyond chain length must fail");

        match result {
            Err(RedactError::IndexOutOfBounds) => {} // expected
            other => panic!("expected IndexOutOfBounds, got {:?}", other),
        }
    }

    // ━━━ 4) ChainProof position verification ━━━

    /// Generate a proof for a block and verify its position in the chain.
    #[test]
    fn test_chain_proof_position_verification_valid() {
        let mut chain = HashChain::new(coverage_key());
        chain.append("data-A");
        chain.append("data-B");
        chain.append("data-C");

        // Get proof for block 2 (middle block)
        let proof: ChainProof = chain
            .proof_for_block(2)
            .expect("proof for block 2 must exist");

        assert!(proof.verify_position(), "valid proof must verify position");

        // Check proof fields
        assert_eq!(proof.block.index, 2);
        assert!(proof.previous_block_hash.is_some(), "middle block has predecessor");
        assert!(proof.next_block_hash.is_some(), "middle block has successor");
        assert_eq!(
            proof.previous_block_hash.as_ref().unwrap(),
            &chain.blocks[1].hash,
            "previous_block_hash must match block 1's hash"
        );
        assert_eq!(
            proof.next_block_hash.as_ref().unwrap(),
            &chain.blocks[3].hash,
            "next_block_hash must match block 3's hash"
        );
    }

    /// Verify proof for genesis (no predecessor hash).
    #[test]
    fn test_chain_proof_genesis_position() {
        let mut chain = HashChain::new(coverage_key());
        chain.append("data-1");

        let proof: ChainProof = chain
            .proof_for_block(0)
            .expect("proof for genesis must exist");

        assert!(proof.verify_position(), "genesis proof must verify");
        assert!(
            proof.previous_block_hash.is_none(),
            "genesis has no predecessor"
        );
        assert!(
            proof.next_block_hash.is_some(),
            "genesis has a successor if chain length > 1"
        );
    }

    /// Verify proof for the last block (no next_block_hash).
    #[test]
    fn test_chain_proof_last_block_position() {
        let mut chain = HashChain::new(coverage_key());
        chain.append("data-1");
        chain.append("data-2");

        let proof: ChainProof = chain
            .proof_for_block(2)
            .expect("proof for last block must exist");

        assert!(proof.verify_position(), "last block proof must verify");
        assert!(
            proof.previous_block_hash.is_some(),
            "last block has predecessor"
        );
        assert!(
            proof.next_block_hash.is_none(),
            "last block has no successor"
        );
    }

    /// Proof for out-of-bounds index returns None.
    #[test]
    fn test_chain_proof_out_of_bounds_none() {
        let chain = HashChain::new(coverage_key());
        assert!(
            chain.proof_for_block(42).is_none(),
            "proof for non-existent index must be None"
        );
    }

    /// A tampered proof (corrupted block data) fails position verification.
    #[test]
    fn test_chain_proof_tampered_rejected() {
        let mut chain = HashChain::new(coverage_key());
        chain.append("data-1");
        chain.append("data-2");

        let proof: ChainProof = chain
            .proof_for_block(1)
            .expect("proof must exist");

        // Clone and corrupt the proof's block data
        let mut bad_proof: ChainProof = proof.clone();
        bad_proof.block.data = "INJECTED_MALICIOUS_DATA".to_string();

        assert!(
            !bad_proof.verify_position(),
            "tampered proof must fail position verification"
        );
    }

    /// A proof with mismatched previous_block_hash fails verification.
    #[test]
    fn test_chain_proof_bad_previous_hash_rejected() {
        let mut chain = HashChain::new(coverage_key());
        chain.append("data-1");
        chain.append("data-2");

        let proof: ChainProof = chain
            .proof_for_block(1)
            .expect("proof must exist");

        let mut bad_proof: ChainProof = proof.clone();
        bad_proof.previous_block_hash = Some("f".repeat(64));

        assert!(
            !bad_proof.verify_position(),
            "proof with wrong previous_block_hash must fail"
        );
    }

    // ━━━ 5) HMAC mismatch detection ━━━

    /// HMAC signed with correct key verifies; wrong key fails.
    #[test]
    fn test_hmac_mismatch_wrong_key() {
        let correct_key = b"alice-secret-key-for-hmac!!!";
        let wrong_key = b"bob-evil-key-for-hmac!!!!!";
        let msg = b"transfer 5000 USD to account 42";

        let signature = hmac_sign(correct_key, msg);

        // Correct key verifies
        assert!(
            hmac_verify(correct_key, msg, &signature),
            "correct key must verify the HMAC"
        );

        // Wrong key must fail
        assert!(
            !hmac_verify(wrong_key, msg, &signature),
            "wrong key must NOT verify the HMAC"
        );
    }

    /// HMAC signed on correct message verifies; tampered message fails.
    #[test]
    fn test_hmac_mismatch_tampered_message() {
        let key = b"integrity-key-32-bytes-long!";
        let original_msg = b"txn: deposit 1000";
        let tampered_msg = b"txn: deposit 9999"; // attacker changed amount

        let signature = hmac_sign(key, original_msg);

        assert!(
            hmac_verify(key, original_msg, &signature),
            "original message must verify"
        );

        assert!(
            !hmac_verify(key, tampered_msg, &signature),
            "tampered message must fail HMAC verification"
        );
    }

    /// Full SignedTransaction round-trip with wrong key fails.
    #[test]
    fn test_signed_transaction_hmac_mismatch() {
        let real_key = b"real-signing-key-32-bytes!!!";
        let fake_key = b"fake-signing-key-32-bytes!!!";

        let tx_id = Uuid::now_v7();
        let signed = SignedTransaction::sign(tx_id, "transfer 1000", real_key);

        // Real key verifies
        assert!(
            signed.verify(real_key),
            "real signing key must verify transaction"
        );

        // Fake key must fail
        assert!(
            !signed.verify(fake_key),
            "fake key must not verify signed transaction"
        );
    }

    /// SignedTransaction with tampered payload fails HMAC verification.
    #[test]
    fn test_signed_transaction_tampered_payload_mismatch() {
        let key = b"tamper-key-32-bytes-long!!!!!";
        let tx_id = Uuid::now_v7();

        let mut signed = SignedTransaction::sign(tx_id, "deposit 500", key);

        // Tamper the payload
        signed.payload = "withdraw 999999".to_string();

        assert!(
            !signed.verify(key),
            "tampered payload must fail HMAC verification"
        );
    }

    /// HMAC is deterministic: same inputs always produce same output.
    #[test]
    fn test_hmac_deterministic_output() {
        let key = b"deterministic-key-for-test";
        let msg = b"deterministic message";

        let sig1 = hmac_sign(key, msg);
        let sig2 = hmac_sign(key, msg);
        let sig3 = hmac_sign(key, msg);

        assert_eq!(sig1, sig2, "HMAC must be deterministic");
        assert_eq!(sig2, sig3, "HMAC must be deterministic");
    }

    /// HMAC with longer-than-block-size keys works correctly.
    #[test]
    fn test_hmac_long_key() {
        // Key longer than 64 bytes (SHA-256 block size)
        let long_key = b"this-is-a-very-long-key-that-exceeds-the-sha256-block-size-of-64-bytes-for-testing-hmac-key-hashing";
        let msg = b"message";
        let sig = hmac_sign(long_key, msg);

        assert!(hmac_verify(long_key, msg, &sig), "long key HMAC must verify");
        assert!(
            !hmac_verify(b"short-key", msg, &sig),
            "different key must fail"
        );
    }

    // ━━━ Integration: Tamper then Redact on chain with HMAC ━━━

    /// Tamper a block, confirm detection, then redact and verify chain heals.
    #[test]
    fn test_tamper_then_redact() {
        let mut chain = HashChain::new(coverage_key());
        chain.append("private-note-1");
        chain.append("private-note-2");
        chain.append("public-note");

        // Step 1: tamper block 1
        chain.blocks[1].data = "LEAKED_SECRET".to_string();
        let (valid_after_tamper, tampered) = chain.verify_chain();
        assert!(!valid_after_tamper, "chain must be invalid after tamper");
        assert!(tampered.contains(&1));

        // Step 2: redact the tampered block — this recalculates the hash
        chain.redact(1).expect("redact must succeed on tampered block");
        assert_eq!(chain.blocks[1].data, "[REDACTED]");

        // Step 3: chain must be valid again after forward rehash
        let (valid_after_redact, tampered_after_redact) = chain.verify_chain();
        assert!(valid_after_redact, "chain must be valid after redaction heals it");
        assert!(
            tampered_after_redact.is_empty(),
            "no tampered blocks after redaction"
        );
    }
}
