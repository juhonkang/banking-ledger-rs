//! Integration tests — full API flows at the service level.
//! These test the actual types from the crate without spinning up an HTTP server.
//!
//! Run with: cargo test --test integration_extended

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use banking_ledger::domain::account::{
    Account, AccountId, AccountStatus, AccountType, DebitError,
};
use banking_ledger::domain::identifier::{IdentifierStatus, IdentifierType};
use banking_ledger::domain::party::{PartyStatus, PartyType};
use banking_ledger::log::hash_chain::HashChain;
use banking_ledger::service::account_service::AccountService;
use banking_ledger::service::identity_service::IdentityService;
use banking_ledger::service::ledger_service::LedgerService;

// ━━━ Test 1: Create Account → Debit → Credit → Verify Balance ━━━

#[test]
fn test_create_debit_credit_balance_flow() {
    let svc = AccountService::new();

    // Create account with initial balance of $1,000.00 (100,000 cents)
    let acc = svc.create_account(AccountType::Asset, "USD", 100_000, None);
    let id = acc.id;

    assert_eq!(svc.get_balance_cents(id).unwrap(), 100_000);
    assert_eq!(
        svc.get_account(id).unwrap().available_balance_cents(),
        100_000
    );

    // Debit $300.00 → balance should be $700.00
    let new_bal = svc.perform_debit(id, 30_000).unwrap();
    assert_eq!(new_bal, 70_000);
    assert_eq!(svc.get_balance_cents(id).unwrap(), 70_000);
    assert_eq!(
        svc.get_account(id).unwrap().available_balance_cents(),
        70_000
    );

    // Credit $450.00 → balance should be $1,150.00
    let new_bal = svc.perform_credit(id, 45_000).unwrap();
    assert_eq!(new_bal, 115_000);
    assert_eq!(svc.get_balance_cents(id).unwrap(), 115_000);
    assert_eq!(
        svc.get_account(id).unwrap().available_balance_cents(),
        115_000
    );

    // Verify account status still Open
    assert_eq!(svc.get_account(id).unwrap().status(), AccountStatus::Open);
}

// ━━━ Test 2: Create Two Accounts → Transfer → Verify Both Balances ━━━

#[test]
fn test_transfer_between_accounts() {
    // Create two accounts directly and share via HashMap (same pattern as production)
    let acc1 = Account::new(AccountType::Asset, "USD", 500_000, None);
    let acc2 = Account::new(AccountType::Liability, "USD", 100_000, None);
    let id1 = acc1.id;
    let id2 = acc2.id;

    let accounts: Arc<RwLock<HashMap<AccountId, Account>>> =
        Arc::new(RwLock::new(HashMap::new()));
    {
        let mut map = accounts.write().unwrap();
        map.insert(id1, acc1);
        map.insert(id2, acc2);
    }

    let ledger = LedgerService::new(accounts.clone());

    // Transfer $200.00 from acc1 to acc2
    let txn_id = ledger.begin_transaction("TRANSFER-001");
    let result = ledger.record_transfer(txn_id, id1, id2, 200_000, "Transfer from savings to checking");
    assert!(result.is_ok(), "transfer should succeed: {:?}", result.err());

    // Verify balances after transfer
    {
        let map = accounts.read().unwrap();
        let a1 = map.get(&id1).expect("acc1 should exist");
        let a2 = map.get(&id2).expect("acc2 should exist");
        assert_eq!(a1.balance_cents(), 300_000, "acc1: 500k - 200k = 300k");
        assert_eq!(a2.balance_cents(), 300_000, "acc2: 100k + 200k = 300k");
    }

    // Verify journal entry was recorded
    let entries = ledger.get_all_entries();
    assert_eq!(entries.len(), 1);
    let entry = &entries[0];
    assert!(entry.verify_balance(), "journal entry must be balanced");
    assert_eq!(entry.legs.len(), 2);
    assert_eq!(entry.description, "Transfer from savings to checking");

    // Verify transaction was committed
    let txn = ledger.get_transaction(txn_id).expect("transaction should exist");
    assert!(
        matches!(
            txn.status,
            banking_ledger::domain::journal::TransactionStatus::Committed
        ),
        "transaction should be Committed, got {:?}",
        txn.status
    );
}

// ━━━ Test 3: Place Hold → Debit Blocked → Release Hold → Debit Allowed ━━━

#[test]
fn test_hold_blocks_debit_release_allows_debit() {
    let svc = AccountService::new();

    // Create account with $1,000.00
    let acc = svc.create_account(AccountType::Asset, "USD", 100_000, None);
    let id = acc.id;

    // Place hold of $600.00 — available drops to $400.00
    svc.place_hold(id, 60_000)
        .expect("place_hold should succeed");
    // Balance unchanged, available reduced
    assert_eq!(svc.get_balance_cents(id).unwrap(), 100_000);
    assert_eq!(
        svc.get_account(id).unwrap().available_balance_cents(),
        40_000
    );

    // Try to debit $500.00 — should fail (available = $400.00 < $500.00)
    let result = svc.perform_debit(id, 50_000);
    assert!(
        result.is_err(),
        "debit should be blocked by hold; available=40_000, requested=50_000"
    );
    assert_eq!(
        result.unwrap_err(),
        DebitError::InsufficientFunds {
            available: 40_000,
            requested: 50_000,
        }
    );

    // Balance still $1,000.00 (debit was rejected)
    assert_eq!(svc.get_balance_cents(id).unwrap(), 100_000);

    // Release hold of $600.00 — available restored to $1,000.00
    svc.release_hold(id, 60_000)
        .expect("release_hold should succeed");
    assert_eq!(
        svc.get_account(id).unwrap().available_balance_cents(),
        100_000
    );

    // Now debit $500.00 — should succeed (available = $1,000.00)
    let new_bal = svc
        .perform_debit(id, 50_000)
        .expect("debit should succeed after hold release");
    assert_eq!(new_bal, 50_000);
    assert_eq!(svc.get_balance_cents(id).unwrap(), 50_000);
    assert_eq!(
        svc.get_account(id).unwrap().available_balance_cents(),
        50_000
    );
}

// ━━━ Test 4: Hash Chain Append 100 Blocks → Verify Chain Integrity ━━━

#[test]
fn test_hash_chain_100_blocks_integrity() {
    let key = b"test-chain-key-32-bytes-!!end";
    let mut chain = HashChain::new(key);

    // Genesis block exists
    assert_eq!(chain.len(), 1);
    assert_eq!(chain.blocks[0].index, 0);
    assert_eq!(
        chain.blocks[0].data,
        "GENESIS_BLOCK",
        "genesis block should contain GENESIS_BLOCK"
    );

    // Append 100 blocks
    for i in 0..100 {
        let block = chain.append(&format!("block-data-{}", i));
        assert_eq!(block.index, i + 1, "block index should be sequential");
        assert!(!block.hash.is_empty(), "block must have a non-empty hash");
    }

    // Verify chain length (genesis + 100 blocks)
    assert_eq!(chain.len(), 101);

    // Verify full chain integrity
    let (valid, tampered) = chain.verify_chain();
    assert!(valid, "chain should be valid after 100 appends");
    assert!(tampered.is_empty(), "no blocks should be tampered: {tampered:?}");

    // Verify individual blocks at boundaries
    let first_block = chain.get_block(1).expect("block 1 should exist");
    assert!(first_block.verify_self(), "block 1 should self-verify");

    let last_block = chain.get_block(100).expect("block 100 should exist");
    assert!(last_block.verify_self(), "last block should self-verify");

    // Verify chain proof for a middle block
    let proof = chain.proof_for_block(50).expect("proof for block 50 should exist");
    assert!(proof.verify_position(), "proof for block 50 should verify position");

    // Tamper detection: modify block 33 → verify_chain should catch it
    let original_data = chain.blocks[33].data.clone();
    chain.blocks[33].data = "TAMPERED_DATA".to_string();
    // Do NOT recalculate hash — this simulates real tampering

    let (still_valid, tampered_indices) = chain.verify_chain();
    assert!(!still_valid, "chain should be INVALID after tampering");
    assert!(
        tampered_indices.contains(&33),
        "block 33 should be flagged as tampered: {tampered_indices:?}"
    );

    // Restore original data (but hash is now wrong)
    chain.blocks[33].data = original_data;
    let (final_valid, _) = chain.verify_chain();
    // After restoring data but not recalculating hash, the chain should still
    // be verified as valid (data matches original hash before tamper, and
    // block 34's previous_hash still points to block 33's original hash).
    // Block 33's self-hash was NOT recomputed when we tampered — we only
    // changed data. So block 33's computed hash (from data + timestamp + ...)
    // equals its stored hash (original). The original data is restored,
    // so verify_self passes.
    // HOWEVER: block 34's previous_hash still equals block 33's original hash,
    // which is now different from block 33's ACTUAL hash (since we changed data
    // earlier). Wait, no — we restored the data. So block 33's data is back
    // to original, and its computed hash = stored hash = original hash.
    // Block 34's previous_hash = original hash. So the chain IS valid again.
    assert!(final_valid, "chain should be valid after restoring original data");
}

// ━━━ Test 5: Party + Identifier Lifecycle ━━━

#[test]
fn test_party_identifier_lifecycle() -> Result<(), Box<dyn std::error::Error>> {
    let svc = IdentityService::new();

    // ━━ Create Party ━━
    let party = svc.create_party(PartyType::Individual, "Alice Johnson");
    assert_eq!(party.legal_name, "Alice Johnson");
    assert_eq!(party.status, PartyStatus::Active);
    assert!(!party.id.is_nil());

    // ━━ Add Identifier ━━
    let ident1 = svc.add_identifier(
        party.id,
        IdentifierType::PassportNumber,
        "AB123456",
        Some("US"),
    )?;
    assert_eq!(ident1.identifier_type, IdentifierType::PassportNumber);
    assert_eq!(ident1.value, "AB123456");
    assert_eq!(ident1.issuing_country.as_deref(), Some("US"));
    assert_eq!(ident1.status, IdentifierStatus::PendingVerification);

    // Initially no active identifiers (status is PendingVerification)
    let active = svc.get_active_identifiers(party.id);
    assert_eq!(active.len(), 0, "no identifiers should be active yet");

    // ━━ Replace Identifier (simulates passport renewal) ━━
    // replace_identifier inactivates old, creates new Active one
    let ident2 = svc.replace_identifier(ident1.id, "CD789012")?;
    assert_eq!(ident2.value, "CD789012");
    assert_eq!(ident2.status, IdentifierStatus::Active);
    assert_eq!(ident2.identifier_type, IdentifierType::PassportNumber);
    assert_eq!(ident2.replaces, Some(ident1.id));

    // Now there should be 1 active identifier
    let active = svc.get_active_identifiers(party.id);
    assert_eq!(active.len(), 1);
    assert_eq!(active[0].value, "CD789012");
    assert_eq!(active[0].status, IdentifierStatus::Active);

    // ━━ Add second identifier type (Email) ━━
    let ident3 = svc.add_identifier(
        party.id,
        IdentifierType::Email,
        "alice@example.com",
        None,
    )?;
    assert_eq!(ident3.identifier_type, IdentifierType::Email);
    assert_eq!(ident3.value, "alice@example.com");

    // Still only 1 active (email is PendingVerification)
    let active = svc.get_active_identifiers(party.id);
    assert_eq!(active.len(), 1);

    // ━━ Verify email → now 2 active ━━
    let ident4 = svc.replace_identifier(ident3.id, "alice@newdomain.com")?;
    assert_eq!(ident4.status, IdentifierStatus::Active);

    let active = svc.get_active_identifiers(party.id);
    assert_eq!(active.len(), 2, "should have 2 active identifiers after verification");

    // ━━ Retrieve Party ━━
    let retrieved = svc.get_party(party.id).expect("party should exist");
    assert_eq!(retrieved.legal_name, "Alice Johnson");
    assert_eq!(retrieved.party_type, PartyType::Individual);

    // ━━ List all parties ━━
    let all = svc.all_parties();
    assert_eq!(all.len(), 1);
    assert_eq!(all[0].id, party.id);

    // ━━ Add second party to verify multi-party support ━━
    let party2 = svc.create_party(PartyType::Corporation, "Acme Corp");
    assert_eq!(party2.party_type, PartyType::Corporation);
    assert_eq!(svc.all_parties().len(), 2);

    // Each party has their own identifiers
    let p1_active = svc.get_active_identifiers(party.id);
    let p2_active = svc.get_active_identifiers(party2.id);
    assert_eq!(p1_active.len(), 2);
    assert_eq!(p2_active.len(), 0, "new party should have no identifiers");

    Ok(())
}
