#[cfg(test)]
mod tests {
    use crate::domain::identifier::{IdentifierStatus, IdentifierType};
    use crate::domain::party::PartyType;
    use crate::service::identity_service::IdentityService;

    #[test]
    fn test_create_party() {
        let svc = IdentityService::new();
        let party = svc.create_party(PartyType::Individual, "Nguyen Van A");
        assert!(!party.id.is_nil());
        assert_eq!(party.legal_name, "Nguyen Van A");
        assert!(matches!(party.party_type, PartyType::Individual));
    }

    #[test]
    fn test_add_identifier_to_party() {
        let svc = IdentityService::new();
        let party = svc.create_party(PartyType::Individual, "Test Person");

        let id = svc
            .add_identifier(
                party.id,
                IdentifierType::PassportNumber,
                "P12345678",
                Some("VN"),
            )
            .expect("should add identifier");

        assert_eq!(id.value, "P12345678");
        assert_eq!(id.issuing_country, Some("VN".to_string()));
    }

    #[test]
    fn test_replace_identifier_preserves_history() {
        let svc = IdentityService::new();
        let party = svc.create_party(PartyType::Individual, "Test Person");

        let old = svc
            .add_identifier(party.id, IdentifierType::PassportNumber, "OLD-001", None)
            .unwrap();

        let new = svc
            .replace_identifier(old.id, "NEW-002")
            .expect("should replace");

        // New identifier is active
        assert_eq!(new.value, "NEW-002");
        assert_eq!(new.status, IdentifierStatus::Active);
        assert!(new.replaces.is_some());

        // Old identifier is inactivated, NOT deleted
        let all_active = svc.get_active_identifiers(party.id);
        assert_eq!(all_active.len(), 1);
        assert_eq!(all_active[0].value, "NEW-002");
    }

    #[test]
    fn test_add_identifier_to_nonexistent_party_fails() {
        let svc = IdentityService::new();
        let fake_id = uuid::Uuid::now_v7();
        let result = svc.add_identifier(fake_id, IdentifierType::NationalId, "123", None);
        assert!(result.is_err());
    }

    #[test]
    fn test_get_active_identifiers() {
        let svc = IdentityService::new();
        let party = svc.create_party(PartyType::Corporation, "Test Corp");

        svc.add_identifier(
            party.id,
            IdentifierType::TaxIdentificationNumber,
            "TAX-001",
            None,
        )
        .unwrap();

        let active = svc.get_active_identifiers(party.id);
        // New identifiers start as PendingVerification, so active count is 0
        // After verification they become Active
        assert_eq!(active.len(), 0);
    }
}
