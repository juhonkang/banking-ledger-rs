//! Identity Service — manages Party and Identifier lifecycle.
//! Key invariant: Identifiers are versioned, never overwritten.
//! `PartyId` is immutable. One Party can own multiple accounts.

use std::collections::HashMap;
use std::sync::RwLock;

use crate::domain::identifier::{Identifier, IdentifierStatus, IdentifierType};
use crate::domain::party::{Party, PartyId, PartyStatus, PartyType};

pub struct IdentityService {
    parties: RwLock<HashMap<PartyId, Party>>,
    identifiers: RwLock<HashMap<uuid::Uuid, Identifier>>,
    // Index: party_id → list of identifier ids
    party_identifiers: RwLock<HashMap<PartyId, Vec<uuid::Uuid>>>,
}

impl IdentityService {
    pub fn new() -> Self {
        Self {
            parties: RwLock::new(HashMap::new()),
            identifiers: RwLock::new(HashMap::new()),
            party_identifiers: RwLock::new(HashMap::new()),
        }
    }

    /// Create a new Party and return its immutable ID
    pub fn create_party(&self, party_type: PartyType, legal_name: &str) -> Party {
        let party = Party::new(party_type, legal_name.to_string());
        self.parties
            .write()
            .unwrap()
            .insert(party.id, party.clone());
        party
    }

    /// Add an identifier to a Party. Validates that Party exists.
    pub fn add_identifier(
        &self,
        party_id: PartyId,
        identifier_type: IdentifierType,
        value: &str,
        issuing_country: Option<&str>,
    ) -> Result<Identifier, IdentityError> {
        // Validate party exists
        let parties = self.parties.read().unwrap();
        if !parties.contains_key(&party_id) {
            return Err(IdentityError::PartyNotFound(party_id));
        }
        // Ensure party is active
        if let Some(party) = parties.get(&party_id) {
            if party.status != PartyStatus::Active {
                return Err(IdentityError::PartyNotActive(party_id));
            }
        }
        drop(parties);

        let identifier = Identifier::new(
            party_id,
            identifier_type,
            value.to_string(),
            issuing_country.map(std::string::ToString::to_string),
            None,
        );

        self.identifiers
            .write()
            .unwrap()
            .insert(identifier.id, identifier.clone());
        self.party_identifiers
            .write()
            .unwrap()
            .entry(party_id)
            .or_default()
            .push(identifier.id);

        Ok(identifier)
    }

    /// Replace an identifier (e.g., expired passport → new passport).
    /// Old identifier is inactivated, NEVER deleted. New one is created.
    pub fn replace_identifier(
        &self,
        old_identifier_id: uuid::Uuid,
        new_value: &str,
    ) -> Result<Identifier, IdentityError> {
        let mut identifiers = self.identifiers.write().unwrap();

        let old = identifiers
            .get_mut(&old_identifier_id)
            .ok_or(IdentityError::IdentifierNotFound(old_identifier_id))?;

        let replacement = old.replace_with(new_value.to_string());
        let replacement_id = replacement.id;
        let party_id = replacement.party_id;

        identifiers.insert(replacement_id, replacement.clone());
        drop(identifiers);

        self.party_identifiers
            .write()
            .unwrap()
            .entry(party_id)
            .or_default()
            .push(replacement_id);

        Ok(replacement)
    }

    /// Get all active identifiers for a party
    pub fn get_active_identifiers(&self, party_id: PartyId) -> Vec<Identifier> {
        let pi = self.party_identifiers.read().unwrap();
        let identifiers = self.identifiers.read().unwrap();

        pi.get(&party_id)
            .map(|ids| {
                ids.iter()
                    .filter_map(|id| identifiers.get(id))
                    .filter(|i| i.status == IdentifierStatus::Active)
                    .cloned()
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Get a party by ID
    pub fn get_party(&self, party_id: PartyId) -> Option<Party> {
        self.parties.read().unwrap().get(&party_id).cloned()
    }

    /// Get all parties
    pub fn all_parties(&self) -> Vec<Party> {
        self.parties.read().unwrap().values().cloned().collect()
    }

    /// Iterate all parties (avoids allocation)
    pub fn for_each_party(&self, mut f: impl FnMut(&Party)) {
        for party in self.parties.read().unwrap().values() {
            f(party);
        }
    }
}

impl Default for IdentityService {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, thiserror::Error)]
pub enum IdentityError {
    #[error("Party not found: {0}")]
    PartyNotFound(PartyId),
    #[error("Party not active: {0}")]
    PartyNotActive(PartyId),
    #[error("Identifier not found: {0}")]
    IdentifierNotFound(uuid::Uuid),
}
