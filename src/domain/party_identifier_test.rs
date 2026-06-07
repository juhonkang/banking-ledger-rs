//! Comprehensive tests for Party + Identifier domain models.
//! Zero tests → 15+ tests covering: lifecycle, state machines, edge cases, invariants.

#[cfg(test)]
mod party_tests {
    use crate::domain::party::{Party, PartyStatus, PartyType};

    #[test]
    fn test_new_party_defaults_to_active() {
        let p = Party::new(PartyType::Individual, "Alice".into());
        assert_eq!(p.status, PartyStatus::Active);
        assert_eq!(p.party_type, PartyType::Individual);
        assert_eq!(p.legal_name, "Alice");
    }

    #[test]
    fn test_party_id_is_unique() {
        let p1 = Party::new(PartyType::Corporation, "Acme".into());
        let p2 = Party::new(PartyType::Corporation, "Acme".into());
        assert_ne!(p1.id, p2.id, "UUID v7 should produce unique IDs");
    }

    #[test]
    fn test_party_id_is_v7_time_ordered() {
        let p1 = Party::new(PartyType::Individual, "A".into());
        std::thread::sleep(std::time::Duration::from_millis(2));
        let p2 = Party::new(PartyType::Individual, "B".into());
        // UUID v7: timestamp is in first 48 bits → newer UUID > older UUID
        assert!(p2.id.as_u128() > p1.id.as_u128(),
            "UUID v7 should be monotonically increasing");
    }

    #[test]
    fn test_all_party_types_constructible() {
        for (pt, name) in [
            (PartyType::Individual, "Ind"),
            (PartyType::Corporation, "Corp"),
            (PartyType::Trust, "Trust"),
            (PartyType::GovernmentAgency, "Gov"),
            (PartyType::FinancialInstitution, "Fin"),
            (PartyType::Other("DAO".into()), "DAO"),
        ] {
            let p = Party::new(pt, name.into());
            assert_eq!(p.status, PartyStatus::Active);
        }
    }

    #[test]
    fn test_party_status_enum_values() {
        // Verify all statuses are distinct
        assert_ne!(PartyStatus::Active, PartyStatus::Suspended);
        assert_ne!(PartyStatus::Active, PartyStatus::Closed);
        assert_ne!(PartyStatus::Suspended, PartyStatus::Closed);
    }

    #[test]
    fn test_party_legal_name_preserves_unicode() {
        let p = Party::new(PartyType::Individual, "Nguyễn Văn A".into());
        assert_eq!(p.legal_name, "Nguyễn Văn A");
    }

    #[test]
    fn test_party_empty_legal_name() {
        let p = Party::new(PartyType::Corporation, "".into());
        assert_eq!(p.legal_name, "");
        // Empty name is a business-logic concern, not a structural invariant
    }
}

#[cfg(test)]
mod identifier_tests {
    use chrono::Utc;
    use uuid::Uuid;

    use crate::domain::identifier::{Identifier, IdentifierStatus, IdentifierType};

    fn sample_party_id() -> Uuid {
        Uuid::now_v7()
    }

    #[test]
    fn test_new_identifier_pending_verification() {
        let id = Identifier::new(
            sample_party_id(),
            IdentifierType::PassportNumber,
            "AB1234567".into(),
            Some("US".into()),
            None,
        );
        assert_eq!(id.status, IdentifierStatus::PendingVerification);
        assert_eq!(id.value, "AB1234567");
        assert_eq!(id.issuing_country, Some("US".into()));
        assert!(id.verified_at.is_none());
    }

    #[test]
    fn test_verify_changes_status() {
        let mut id = Identifier::new(
            sample_party_id(),
            IdentifierType::NationalId,
            "123-45-6789".into(),
            None,
            None,
        );
        id.verify();
        assert_eq!(id.status, IdentifierStatus::Active);
        assert!(id.verified_at.is_some());
    }

    #[test]
    fn test_replace_with_inactivates_old() {
        let mut old = Identifier::new(
            sample_party_id(),
            IdentifierType::PassportNumber,
            "OLD123".into(),
            Some("US".into()),
            None,
        );
        old.verify();
        let old_id = old.id;

        let replacement = old.replace_with("NEW456".into());

        // Old identifier should be inactive
        assert_eq!(old.status, IdentifierStatus::Inactive);
        // Replacement should be active
        assert_eq!(replacement.status, IdentifierStatus::Active);
        // Replacement should point back
        assert_eq!(replacement.replaces, Some(old_id));
        // Replacement should be auto-verified
        assert!(replacement.verified_at.is_some());
    }

    #[test]
    fn test_replace_with_preserves_party_id() {
        let party_id = sample_party_id();
        let mut old = Identifier::new(
            party_id,
            IdentifierType::Email,
            "old@example.com".into(),
            None,
            None,
        );
        let replacement = old.replace_with("new@example.com".into());
        assert_eq!(replacement.party_id, party_id);
    }

    #[test]
    fn test_replace_with_preserves_type() {
        let mut old = Identifier::new(
            sample_party_id(),
            IdentifierType::Phone,
            "+84-123-4567".into(),
            Some("VN".into()),
            None,
        );
        let replacement = old.replace_with("+84-987-6543".into());
        assert_eq!(replacement.identifier_type, IdentifierType::Phone);
    }

    #[test]
    fn test_replace_with_carries_country() {
        let mut old = Identifier::new(
            sample_party_id(),
            IdentifierType::NationalId,
            "12345".into(),
            Some("DE".into()),
            None,
        );
        let replacement = old.replace_with("67890".into());
        assert_eq!(replacement.issuing_country, Some("DE".into()));
    }

    // DateTime<Utc> is Copy, so effective_to is COPIED (not moved) — no bug here
    #[test]
    fn test_effective_to_preserved_on_replace() {
        let expiry = Utc::now() + chrono::Duration::days(365);
        let mut old = Identifier::new(
            sample_party_id(),
            IdentifierType::PassportNumber,
            "P123".into(),
            Some("UK".into()),
            Some(expiry),
        );

        let replacement = old.replace_with("P456".into());

        // Old identifier keeps its expiry (DateTime<Utc> is Copy)
        assert_eq!(old.effective_to, Some(expiry));
        // Replacement inherits the same expiry
        assert_eq!(replacement.effective_to, Some(expiry));
    }

    #[test]
    fn test_all_identifier_types() {
        let types = [
            IdentifierType::NationalId,
            IdentifierType::PassportNumber,
            IdentifierType::TaxIdentificationNumber,
            IdentifierType::BusinessRegistrationNumber,
            IdentifierType::DriversLicense,
            IdentifierType::Email,
            IdentifierType::Phone,
            IdentifierType::Other("Custom".into()),
        ];
        for t in &types {
            let id = Identifier::new(sample_party_id(), t.clone(), "val".into(), None, None);
            assert_eq!(id.identifier_type, *t);
        }
    }
}
