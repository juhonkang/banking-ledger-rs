//! Banking identity — Party and Identifier models.
//! Party ≠ Account: one Party can own many Accounts, one Account can have many Parties (joint).
//! Identifiers are versioned — never overwritten, only inactivated.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Unique, immutable internal identifier for a Party.
/// Never changes, never reused. The digital fingerprint.
pub type PartyId = Uuid;

/// What kind of entity this Party represents.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum PartyType {
    Individual,
    Corporation,
    Trust,
    GovernmentAgency,
    FinancialInstitution,
    /// For entities that don't fit standard categories
    Other(String),
}

/// The abstract entity that interacts with the financial system.
/// Aggregate root in DDD terms — cluster of domain objects.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Party {
    pub id: PartyId,
    pub party_type: PartyType,
    pub legal_name: String,
    /// When this party was first registered in the system
    pub created_at: DateTime<Utc>,
    /// Soft-delete / status flag
    pub status: PartyStatus,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum PartyStatus {
    Active,
    Suspended,
    Closed,
}

impl Party {
    pub fn new(party_type: PartyType, legal_name: String) -> Self {
        Self {
            id: Uuid::now_v7(),
            party_type,
            legal_name,
            created_at: Utc::now(),
            status: PartyStatus::Active,
        }
    }
}
