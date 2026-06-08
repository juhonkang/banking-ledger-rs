//! RBAC (Role-Based Access Control) — banking-grade authorization.
//!
//! # Design
//!
//! ```text
//! Subject ──has──▶ Role ──grants──▶ Permissions ──scoped──▶ Resource
//! ```
//!
//! # Banking Roles
//!
//! | Role | Can |
//! |------|-----|
//! | Admin | Everything — create users, change limits, kill switches |
//! | Auditor | Read-only: view all accounts, journal, audit trail, verify chains |
//! | Teller | Operational: create accounts, process transfers, view own branch |
//! | Customer | Self-service: view own accounts, initiate transfers, download statements |
//! | System | Internal: saga orchestration, auto-reconciliation, cron jobs |
//! | Compliance | GDPR: redact PII, export data, generate SAR reports |
//!
//! # Enforcement Pattern
//!
//! Uses Axum middleware + Extension Trait: `RequestExt::require(permission)`

use std::collections::{HashMap, HashSet};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

// ━━━ Core Types ━━━

/// A unique identity — person, service account, or API key.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SubjectId(pub Uuid);

/// Banking roles with predefined permission sets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    Admin,
    Auditor,
    Teller,
    Customer,
    System,
    Compliance,
}

/// Granular permissions — each maps to one or more API endpoints.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Permission {
    // Account permissions
    ReadAnyAccount,
    ReadOwnAccount,
    CreateAccount,
    UpdateAccountStatus,
    CloseAccount,

    // Transaction permissions
    InitiateTransfer,
    InitiateOwnTransfer,
    ViewAnyTransaction,
    ViewOwnTransaction,

    // Audit permissions
    ViewAuditLog,
    VerifyChainIntegrity,
    ExportAuditReport,
    ViewTrialBalance,

    // Admin permissions
    ManageUsers,
    ManageRoles,
    ConfigureLimits,
    ViewSystemMetrics,

    // Compliance / GDPR
    RedactPii,
    ExportUserData,
    GenerateSarReport,

    // Internal
    SagaOrchestrate,
    AutoReconcile,
    CronJob,
}

/// RBAC engine — maps subjects to roles, roles to permissions.
#[derive(Debug, Clone)]
pub struct RbacEngine {
    /// `subject_id` → set of roles
    role_bindings: HashMap<SubjectId, HashSet<Role>>,
    /// role → set of permissions
    role_permissions: HashMap<Role, HashSet<Permission>>,
}

impl RbacEngine {
    /// Create a new RBAC engine with all banking roles pre-configured.
    pub fn new() -> Self {
        let mut role_permissions = HashMap::new();

        // Admin: everything
        role_permissions.insert(Role::Admin, {
            let mut perms = HashSet::new();
            perms.insert(Permission::ReadAnyAccount);
            perms.insert(Permission::CreateAccount);
            perms.insert(Permission::UpdateAccountStatus);
            perms.insert(Permission::CloseAccount);
            perms.insert(Permission::InitiateTransfer);
            perms.insert(Permission::ViewAnyTransaction);
            perms.insert(Permission::ViewAuditLog);
            perms.insert(Permission::VerifyChainIntegrity);
            perms.insert(Permission::ExportAuditReport);
            perms.insert(Permission::ViewTrialBalance);
            perms.insert(Permission::ManageUsers);
            perms.insert(Permission::ManageRoles);
            perms.insert(Permission::ConfigureLimits);
            perms.insert(Permission::ViewSystemMetrics);
            perms.insert(Permission::RedactPii);
            perms.insert(Permission::ExportUserData);
            perms.insert(Permission::GenerateSarReport);
            perms
        });

        // Auditor: read-only, everything
        role_permissions.insert(Role::Auditor, {
            let mut perms = HashSet::new();
            perms.insert(Permission::ReadAnyAccount);
            perms.insert(Permission::ViewAnyTransaction);
            perms.insert(Permission::ViewAuditLog);
            perms.insert(Permission::VerifyChainIntegrity);
            perms.insert(Permission::ExportAuditReport);
            perms.insert(Permission::ViewTrialBalance);
            perms
        });

        // Teller: operational
        role_permissions.insert(Role::Teller, {
            let mut perms = HashSet::new();
            perms.insert(Permission::ReadAnyAccount);
            perms.insert(Permission::CreateAccount);
            perms.insert(Permission::InitiateTransfer);
            perms.insert(Permission::ViewAnyTransaction);
            perms.insert(Permission::ViewAuditLog);
            perms.insert(Permission::VerifyChainIntegrity);
            perms.insert(Permission::ViewTrialBalance);
            perms
        });

        // Customer: self-service only
        role_permissions.insert(Role::Customer, {
            let mut perms = HashSet::new();
            perms.insert(Permission::ReadOwnAccount);
            perms.insert(Permission::InitiateOwnTransfer);
            perms.insert(Permission::ViewOwnTransaction);
            perms
        });

        // System: internal operations
        role_permissions.insert(Role::System, {
            let mut perms = HashSet::new();
            perms.insert(Permission::ReadAnyAccount);
            perms.insert(Permission::InitiateTransfer);
            perms.insert(Permission::SagaOrchestrate);
            perms.insert(Permission::AutoReconcile);
            perms.insert(Permission::CronJob);
            perms
        });

        // Compliance: GDPR/data protection
        role_permissions.insert(Role::Compliance, {
            let mut perms = HashSet::new();
            perms.insert(Permission::ReadAnyAccount);
            perms.insert(Permission::ViewAnyTransaction);
            perms.insert(Permission::ViewAuditLog);
            perms.insert(Permission::VerifyChainIntegrity);
            perms.insert(Permission::ExportAuditReport);
            perms.insert(Permission::RedactPii);
            perms.insert(Permission::ExportUserData);
            perms.insert(Permission::GenerateSarReport);
            perms
        });

        Self {
            role_bindings: HashMap::new(),
            role_permissions,
        }
    }

    /// Bind a subject to a role.
    pub fn bind(&mut self, subject: SubjectId, role: Role) {
        self.role_bindings
            .entry(subject)
            .or_default()
            .insert(role);
    }

    /// Remove a role from a subject.
    pub fn unbind(&mut self, subject: &SubjectId, role: Role) {
        if let Some(roles) = self.role_bindings.get_mut(subject) {
            roles.remove(&role);
        }
    }

    /// Get all roles for a subject.
    pub fn roles_for(&self, subject: &SubjectId) -> HashSet<Role> {
        self.role_bindings
            .get(subject)
            .cloned()
            .unwrap_or_default()
    }

    /// Check if a subject has a specific permission.
    pub fn can(&self, subject: &SubjectId, permission: Permission) -> bool {
        let roles = self.roles_for(subject);
        roles.iter().any(|role| {
            self.role_permissions
                .get(role)
                .is_some_and(|perms| perms.contains(&permission))
        })
    }

    /// Check if a subject has ALL specified permissions.
    pub fn can_all(&self, subject: &SubjectId, permissions: &[Permission]) -> bool {
        permissions.iter().all(|p| self.can(subject, *p))
    }

    /// Check if a subject has ANY of the specified permissions.
    pub fn can_any(&self, subject: &SubjectId, permissions: &[Permission]) -> bool {
        permissions.iter().any(|p| self.can(subject, *p))
    }

    /// Get all permissions for a subject.
    pub fn permissions_for(&self, subject: &SubjectId) -> HashSet<Permission> {
        let roles = self.roles_for(subject);
        let mut all = HashSet::new();
        for role in &roles {
            if let Some(perms) = self.role_permissions.get(role) {
                all.extend(perms);
            }
        }
        all
    }

    /// List all subjects with a given role.
    pub fn subjects_with_role(&self, role: Role) -> Vec<SubjectId> {
        self.role_bindings
            .iter()
            .filter(|(_, roles)| roles.contains(&role))
            .map(|(subj, _)| *subj)
            .collect()
    }

    /// Export the full RBAC state for audit purposes.
    pub fn export_audit(&self) -> serde_json::Value {
        let bindings: Vec<_> = self
            .role_bindings
            .iter()
            .map(|(subj, roles)| {
                serde_json::json!({
                    "subject": subj.0.to_string(),
                    "roles": roles.iter().map(|r| format!("{r:?}")).collect::<Vec<_>>(),
                    "effective_permissions": self
                        .permissions_for(subj)
                        .iter()
                        .map(|p| format!("{p:?}"))
                        .collect::<Vec<_>>(),
                })
            })
            .collect();

        serde_json::json!({
            "total_subjects": self.role_bindings.len(),
            "bindings": bindings,
        })
    }
}

impl Default for RbacEngine {
    fn default() -> Self {
        Self::new()
    }
}

// ━━━ Request-scoped Subject ━━━

/// Extracts the subject from an incoming request.
/// In production, this comes from JWT claims, API key, or mTLS cert.
/// For now, we use the `X-Subject-Id` header.
pub fn extract_subject(
    headers: &axum::http::HeaderMap,
) -> Option<SubjectId> {
    headers
        .get("x-subject-id")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| Uuid::parse_str(s).ok())
        .map(SubjectId)
}

/// Error when authorization fails.
#[derive(Debug)]
pub struct AuthzError {
    pub required: Permission,
    pub reason: String,
}

impl std::fmt::Display for AuthzError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Permission denied: requires {:?} — {}",
            self.required, self.reason
        )
    }
}

// ━━━ RBAC Extension Trait ━━━

/// Extension trait for `RbacEngine` — adds banking-specific helpers.
pub trait RbacExt {
    /// Grant a customer access to their own accounts.
    fn onboard_customer(&mut self, customer_id: Uuid) -> SubjectId;

    /// Promote a teller to auditor (temporary, for investigations).
    fn grant_temporary_audit(&mut self, subject: &SubjectId, duration_seconds: u64);

    /// Check segregation of duties: no one person can both create AND approve a transfer.
    fn check_sod(&self, requester: &SubjectId, approver: &SubjectId) -> Result<(), AuthzError>;

    /// Generate a summary of who-can-do-what.
    fn permission_matrix(&self) -> String;
}

impl RbacExt for RbacEngine {
    fn onboard_customer(&mut self, customer_id: Uuid) -> SubjectId {
        let subject = SubjectId(customer_id);
        self.bind(subject, Role::Customer);
        subject
    }

    fn grant_temporary_audit(&mut self, subject: &SubjectId, duration_seconds: u64) {
        self.bind(*subject, Role::Auditor);
        // Store expiration for scheduled revocation
        let expires_at = std::time::Instant::now() + std::time::Duration::from_secs(duration_seconds);
        // NOTE: production would persist this and use a cron/timer for revocation.
        // For now, callers should invoke `RbacEngine::revoke_temporary_audit` manually.
        let _ = expires_at; // bound for future timer integration
    }

    fn check_sod(
        &self,
        requester: &SubjectId,
        approver: &SubjectId,
    ) -> Result<(), AuthzError> {
        if requester == approver {
            return Err(AuthzError {
                required: Permission::InitiateTransfer,
                reason: "Segregation of duties: requester cannot be approver".into(),
            });
        }
        Ok(())
    }

    #[allow(clippy::format_push_string)]
    fn permission_matrix(&self) -> String {
        let roles = [
            Role::Admin,
            Role::Auditor,
            Role::Teller,
            Role::Customer,
            Role::System,
            Role::Compliance,
        ];
        let mut matrix = String::from("Role             | ReadAcc | CreateAcc | Transfer | ViewTxn | Audit | Redact\n");
        matrix.push_str(&"-".repeat(80));
        matrix.push('\n');

        for role in &roles {
            let perms = self.role_permissions.get(role);
            let name = format!("{role:?}");
            let read = if perms.is_some_and(|p| p.contains(&Permission::ReadAnyAccount)) { "✓" } else { "-" };
            let create = if perms.is_some_and(|p| p.contains(&Permission::CreateAccount)) { "✓" } else { "-" };
            let transfer = if perms.is_some_and(|p| p.contains(&Permission::InitiateTransfer)) { "✓" } else { "-" };
            let view = if perms.is_some_and(|p| p.contains(&Permission::ViewAnyTransaction)) { "✓" } else { "-" };
            let audit = if perms.is_some_and(|p| p.contains(&Permission::ViewAuditLog)) { "✓" } else { "-" };
            let redact = if perms.is_some_and(|p| p.contains(&Permission::RedactPii)) { "✓" } else { "-" };

            matrix.push_str(&format!(
                "{name:<16} | {read:>7} | {create:>9} | {transfer:>8} | {view:>7} | {audit:>5} | {redact:>6}\n"
            ));
        }

        matrix
    }
}

// ━━━ Direct RbacEngine Methods (not part of RbacExt trait) ━━━

impl RbacEngine {
    /// Revoke a temporary audit grant.
    pub fn revoke_temporary_audit(&mut self, subject: &SubjectId) {
        self.unbind(subject, Role::Auditor);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_engine() -> RbacEngine {
        let mut e = RbacEngine::new();
        let admin = SubjectId(Uuid::now_v7());
        let auditor = SubjectId(Uuid::now_v7());
        let teller = SubjectId(Uuid::now_v7());
        let customer = SubjectId(Uuid::now_v7());

        e.bind(admin, Role::Admin);
        e.bind(auditor, Role::Auditor);
        e.bind(teller, Role::Teller);
        e.bind(customer, Role::Customer);

        e
    }

    #[test]
    fn test_admin_can_everything() {
        let e = test_engine();
        let subjects = e.subjects_with_role(Role::Admin);
        assert!(e.can(&subjects[0], Permission::RedactPii));
        assert!(e.can(&subjects[0], Permission::ManageUsers));
        assert!(e.can(&subjects[0], Permission::InitiateTransfer));
    }

    #[test]
    fn test_auditor_read_only() {
        let e = test_engine();
        let subjects = e.subjects_with_role(Role::Auditor);
        assert!(e.can(&subjects[0], Permission::ViewAuditLog));
        assert!(e.can(&subjects[0], Permission::VerifyChainIntegrity));
        assert!(!e.can(&subjects[0], Permission::InitiateTransfer));
        assert!(!e.can(&subjects[0], Permission::CreateAccount));
    }

    #[test]
    fn test_customer_self_service_only() {
        let e = test_engine();
        let subjects = e.subjects_with_role(Role::Customer);
        assert!(e.can(&subjects[0], Permission::ReadOwnAccount));
        assert!(e.can(&subjects[0], Permission::InitiateOwnTransfer));
        assert!(!e.can(&subjects[0], Permission::ReadAnyAccount));
        assert!(!e.can(&subjects[0], Permission::ViewAuditLog));
    }

    #[test]
    fn test_segregation_of_duties() {
        let e = test_engine();
        let subjects = e.subjects_with_role(Role::Teller);
        let teller = &subjects[0];
        assert!(e.check_sod(teller, teller).is_err());
        // Different tellers should be OK
        let other_teller = SubjectId(Uuid::now_v7());
        assert!(e.check_sod(teller, &other_teller).is_ok());
    }

    #[test]
    fn test_can_all_and_any() {
        let e = test_engine();
        let subjects = e.subjects_with_role(Role::Admin);
        let admin = &subjects[0];

        let audit_perms = [
            Permission::ViewAuditLog,
            Permission::VerifyChainIntegrity,
            Permission::ExportAuditReport,
        ];
        assert!(e.can_all(admin, &audit_perms));

        let customer_perms = [
            Permission::ReadOwnAccount,
            Permission::InitiateOwnTransfer,
        ];
        assert!(!e.can_all(admin, &customer_perms)); // Admin has "Any" not "Own"
        // But admin should have the superset permissions
        let admin_perms = [
            Permission::ReadAnyAccount,
            Permission::InitiateTransfer,
        ];
        assert!(e.can_all(admin, &admin_perms));
    }

    #[test]
    fn test_subject_extraction() {
        let mut headers = axum::http::HeaderMap::new();
        let id = Uuid::now_v7();
        headers.insert("x-subject-id", id.to_string().parse().unwrap());
        let subject = extract_subject(&headers);
        assert_eq!(subject, Some(SubjectId(id)));
    }

    #[test]
    fn test_permission_matrix() {
        let e = RbacEngine::new();
        let matrix = e.permission_matrix();
        assert!(matrix.contains("Admin"));
        assert!(matrix.contains("Customer"));
        assert!(matrix.contains("✓"));
    }

    // ━━━ RBAC Enforcement Tests — verify all roles and permissions ━━━

    #[test]
    fn test_teller_operational_access() {
        let e = test_engine();
        let subjects = e.subjects_with_role(Role::Teller);
        let teller = &subjects[0];
        // Teller can do operational tasks
        assert!(e.can(teller, Permission::ReadAnyAccount));
        assert!(e.can(teller, Permission::CreateAccount));
        assert!(e.can(teller, Permission::InitiateTransfer));
        assert!(e.can(teller, Permission::ViewAnyTransaction));
        assert!(e.can(teller, Permission::ViewAuditLog));
        assert!(e.can(teller, Permission::VerifyChainIntegrity));
        assert!(e.can(teller, Permission::ViewTrialBalance));
        // Teller CANNOT do admin/compliance tasks
        assert!(!e.can(teller, Permission::ManageUsers));
        assert!(!e.can(teller, Permission::ManageRoles));
        assert!(!e.can(teller, Permission::RedactPii));
        assert!(!e.can(teller, Permission::ExportUserData));
        assert!(!e.can(teller, Permission::GenerateSarReport));
    }

    #[test]
    fn test_system_internal_ops() {
        let mut e = RbacEngine::new();
        let system = SubjectId(Uuid::now_v7());
        e.bind(system, Role::System);
        // System has internal permissions
        assert!(e.can(&system, Permission::SagaOrchestrate));
        assert!(e.can(&system, Permission::AutoReconcile));
        assert!(e.can(&system, Permission::CronJob));
        assert!(e.can(&system, Permission::ReadAnyAccount));
        assert!(e.can(&system, Permission::InitiateTransfer));
        // System CANNOT do admin/compliance
        assert!(!e.can(&system, Permission::ManageUsers));
        assert!(!e.can(&system, Permission::RedactPii));
    }

    #[test]
    fn test_compliance_gdpr_access() {
        let mut e = RbacEngine::new();
        let compliance = SubjectId(Uuid::now_v7());
        e.bind(compliance, Role::Compliance);
        // Compliance has GDPR powers
        assert!(e.can(&compliance, Permission::RedactPii));
        assert!(e.can(&compliance, Permission::ExportUserData));
        assert!(e.can(&compliance, Permission::GenerateSarReport));
        assert!(e.can(&compliance, Permission::ReadAnyAccount));
        assert!(e.can(&compliance, Permission::ViewAuditLog));
        assert!(e.can(&compliance, Permission::VerifyChainIntegrity));
        // Compliance CANNOT do operations
        assert!(!e.can(&compliance, Permission::CreateAccount));
        assert!(!e.can(&compliance, Permission::InitiateTransfer));
        assert!(!e.can(&compliance, Permission::ManageUsers));
    }

    #[test]
    fn test_unbound_subject_denied() {
        let e = test_engine();
        let stranger = SubjectId(Uuid::now_v7());
        // No roles assigned — should have zero permissions
        assert!(!e.can(&stranger, Permission::ReadAnyAccount));
        assert!(!e.can(&stranger, Permission::ReadOwnAccount));
        assert!(!e.can(&stranger, Permission::ViewAuditLog));
        assert!(e.roles_for(&stranger).is_empty());
        assert!(e.permissions_for(&stranger).is_empty());
    }

    #[test]
    fn test_multiple_roles_union() {
        let mut e = RbacEngine::new();
        let hybrid = SubjectId(Uuid::now_v7());
        // Give both Teller and Auditor roles
        e.bind(hybrid, Role::Teller);
        e.bind(hybrid, Role::Auditor);
        // Should have union of both roles' permissions
        assert!(e.can(&hybrid, Permission::CreateAccount));  // from Teller
        assert!(e.can(&hybrid, Permission::InitiateTransfer)); // from Teller
        assert!(e.can(&hybrid, Permission::ExportAuditReport)); // from Auditor
        assert!(e.can(&hybrid, Permission::VerifyChainIntegrity)); // both
        // Should NOT have Admin-only permissions
        assert!(!e.can(&hybrid, Permission::ManageUsers));
        assert!(!e.can(&hybrid, Permission::ManageRoles));
    }

    #[test]
    fn test_role_unbind_removes_permissions() {
        let mut e = RbacEngine::new();
        let subject = SubjectId(Uuid::now_v7());
        e.bind(subject, Role::Admin);
        assert!(e.can(&subject, Permission::ManageUsers));
        // Unbind Admin → should lose permissions
        e.unbind(&subject, Role::Admin);
        assert!(!e.can(&subject, Permission::ManageUsers));
        assert!(e.roles_for(&subject).is_empty());
    }

    #[test]
    fn test_permission_coverage_all_roles() {
        let e = RbacEngine::new();
        // Verify every permission is granted by at least one role
        let all_permissions = [
            Permission::ReadAnyAccount,
            Permission::ReadOwnAccount,
            Permission::CreateAccount,
            Permission::UpdateAccountStatus,
            Permission::CloseAccount,
            Permission::InitiateTransfer,
            Permission::InitiateOwnTransfer,
            Permission::ViewAnyTransaction,
            Permission::ViewOwnTransaction,
            Permission::ViewAuditLog,
            Permission::VerifyChainIntegrity,
            Permission::ExportAuditReport,
            Permission::ViewTrialBalance,
            Permission::ManageUsers,
            Permission::ManageRoles,
            Permission::ConfigureLimits,
            Permission::ViewSystemMetrics,
            Permission::RedactPii,
            Permission::ExportUserData,
            Permission::GenerateSarReport,
            Permission::SagaOrchestrate,
            Permission::AutoReconcile,
            Permission::CronJob,
        ];
        let all_roles = [
            Role::Admin, Role::Auditor, Role::Teller,
            Role::Customer, Role::System, Role::Compliance,
        ];
        for perm in &all_permissions {
            let covered = all_roles.iter().any(|role| {
                e.role_permissions.get(role)
                    .map(|perms| perms.contains(perm))
                    .unwrap_or(false)
            });
            assert!(covered, "Permission {:?} is not granted to any role!", perm);
        }
    }

    #[test]
    fn test_subject_extraction_invalid_header() {
        let mut headers = axum::http::HeaderMap::new();
        // Invalid UUID
        headers.insert("x-subject-id", "not-a-uuid".parse().unwrap());
        assert_eq!(extract_subject(&headers), None);
    }

    #[test]
    fn test_subject_extraction_missing_header() {
        let headers = axum::http::HeaderMap::new();
        assert_eq!(extract_subject(&headers), None);
    }

    #[test]
    fn test_can_any_fallback() {
        let e = test_engine();
        let subjects = e.subjects_with_role(Role::Customer);
        let customer = &subjects[0];
        // Customer should match ANY of: ReadOwnAccount, InitiateOwnTransfer
        assert!(e.can_any(customer, &[Permission::ReadOwnAccount, Permission::ManageUsers]));
        // But should NOT match when no permissions overlap
        assert!(!e.can_any(customer, &[Permission::ManageUsers, Permission::RedactPii]));
    }
}
