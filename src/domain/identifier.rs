//! Versioned external identity for Parties.
//! Key invariant: Identifiers are NEVER overwritten — old ones are inactivated,
//! new ones are created. This preserves the full audit trail.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::party::PartyId;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum IdentifierType {
    /// Social Security Number (US), National Insurance Number (UK), Aadhaar (IN)
    NationalId,
    PassportNumber,
    TaxIdentificationNumber,
    BusinessRegistrationNumber,
    DriversLicense,
    Email,
    Phone,
    Other(String),
}

/// A real-world identity linked to a Party.
/// Once created, NEVER modified — only versioned.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Identifier {
    pub id: Uuid,
    /// Which party this identifier belongs to
    pub party_id: PartyId,
    pub identifier_type: IdentifierType,
    /// The actual value (e.g., passport number "AB1234567")
    pub value: String,
    /// ISO 3166-1 alpha-2 issuing country
    pub issuing_country: Option<String>,
    /// When this identifier became effective
    pub effective_from: DateTime<Utc>,
    /// When this identifier expires (if applicable)
    pub effective_to: Option<DateTime<Utc>>,
    /// Current status — inactivated when replaced, never deleted
    pub status: IdentifierStatus,
    /// If this identifier replaced a previous one, point to it
    pub replaces: Option<Uuid>,
    pub verified_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum IdentifierStatus {
    Active,
    Inactive,
    Expired,
    PendingVerification,
}

impl Identifier {
    pub fn new(
        party_id: PartyId,
        identifier_type: IdentifierType,
        value: String,
        issuing_country: Option<String>,
        effective_to: Option<DateTime<Utc>>,
    ) -> Self {
        Self {
            id: Uuid::now_v7(),
            party_id,
            identifier_type,
            value,
            issuing_country,
            effective_from: Utc::now(),
            effective_to,
            status: IdentifierStatus::PendingVerification,
            replaces: None,
            verified_at: None,
        }
    }

    /// Mark this identifier as verified (e.g., after KYC check)
    pub fn verify(&mut self) {
        self.status = IdentifierStatus::Active;
        self.verified_at = Some(Utc::now());
    }

    /// Inactivate this identifier (do not delete) and return a new one to replace it
    pub fn replace_with(&mut self, new_value: String) -> Identifier {
        self.status = IdentifierStatus::Inactive;
        let mut replacement = Identifier::new(
            self.party_id,
            self.identifier_type.clone(),
            new_value,
            self.issuing_country.clone(),
            self.effective_to,
        );
        replacement.replaces = Some(self.id);
        replacement.status = IdentifierStatus::Active;
        replacement.verified_at = Some(Utc::now());
        replacement
    }
}
