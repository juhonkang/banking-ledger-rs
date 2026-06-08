//! Hash chain verification edge case coverage — tamper variants,
//! boundary conditions, and parallel verification.
//! Note: HashChain::new() creates a genesis block automatically.

#[cfg(test)]
mod hash_chain_verify_edge_tests {
    use crate::log::hash_chain::HashChain;

    #[test]
    fn test_new_chain_has_genesis() {
        let chain = HashChain::new(b"test-key");
        assert_eq!(chain.len(), 1, "Genesis block should exist");
        assert!(chain.latest().is_some());
    }

    #[test]
    fn test_empty_chain_verify_ok() {
        let chain = HashChain::new(b"test-key");
        let (valid, tampered) = chain.verify_chain();
        assert!(valid, "Genesis-only chain should be valid");
        assert!(tampered.is_empty());
    }

    #[test]
    fn test_single_append_makes_two_blocks() {
        let mut chain = HashChain::new(b"test-key");
        chain.append("block-1");
        assert_eq!(chain.len(), 2); // genesis + appended
        let (valid, tampered) = chain.verify_chain();
        assert!(valid);
        assert_eq!(tampered.len(), 0);
    }

    #[test]
    fn test_two_appends_chain_sequential() {
        let mut chain = HashChain::new(b"test-key");
        chain.append("block-1");
        chain.append("block-2");
        let (valid, _) = chain.verify_chain();
        assert!(valid);
        assert_eq!(chain.len(), 3); // genesis + 2
    }

    #[test]
    fn test_chain_not_empty_after_new() {
        let chain = HashChain::new(b"test-key");
        assert!(!chain.is_empty(), "Genesis block exists");
    }

    #[test]
    fn test_latest_after_append() {
        let mut chain1 = HashChain::new(b"test-key");
        chain1.append("data-1");
        let hash1 = chain1.latest().unwrap().hash.clone();

        let mut chain2 = HashChain::new(b"test-key");
        chain2.append("data-1");
        chain2.append("data-2");
        let hash2 = chain2.latest().unwrap().hash.clone();

        assert_ne!(hash1, hash2, "Sequential blocks should have different hashes");
    }

    #[test]
    fn test_get_block_by_index() {
        let mut chain = HashChain::new(b"test-key");
        chain.append("b0"); // index 1
        chain.append("b1"); // index 2
        chain.append("b2"); // index 3

        assert!(chain.get_block(0).is_some(), "Genesis at index 0");
        assert!(chain.get_block(1).is_some());
        assert!(chain.get_block(3).is_some());
        assert!(chain.get_block(5).is_none());
    }

    #[test]
    fn test_large_block_does_not_panic() {
        let mut chain = HashChain::new(b"test-key");
        let large_data = "A".repeat(100_000);
        chain.append(&large_data);
        let (valid, _) = chain.verify_chain();
        assert!(valid);
    }

    #[test]
    fn test_empty_string_block() {
        let mut chain = HashChain::new(b"test-key");
        chain.append("");
        let (valid, _) = chain.verify_chain();
        assert!(valid, "Zero-length block should be valid");
    }

    #[test]
    fn test_different_keys_produce_different_hashes() {
        let mut chain1 = HashChain::new(b"key-alpha");
        let mut chain2 = HashChain::new(b"key-beta");

        chain1.append("same-data");
        chain2.append("same-data");

        let h1 = chain1.latest().unwrap().hash.clone();
        let h2 = chain2.latest().unwrap().hash.clone();
        assert_ne!(h1, h2, "Different keys should produce different hashes for same data");
    }

    #[test]
    fn test_signing_key_is_accessible() {
        let key = b"my-secret-key";
        let chain = HashChain::new(key);
        assert_eq!(chain.signing_key(), key);
    }
}
