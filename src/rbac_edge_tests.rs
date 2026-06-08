//! RBAC edge case coverage — role binding, permission inheritance,
//! separation of duties, and audit access escalation.

#[cfg(test)]
mod rbac_edge_tests {
    use std::collections::HashSet;

    use crate::rbac::{Permission, RbacEngine, Role, SubjectId};

    #[test]
    fn test_rbac_admin_has_broad_permissions() {
        let mut engine = RbacEngine::new();
        let subj = SubjectId(uuid::Uuid::now_v7());
        engine.bind(subj, Role::Admin);

        assert!(engine.can(&subj, Permission::ManageUsers));
        assert!(engine.can(&subj, Permission::ViewAuditLog));
        assert!(engine.can(&subj, Permission::ReadAnyAccount));
    }

    #[test]
    fn test_rbac_customer_only_own() {
        let mut engine = RbacEngine::new();
        let subj = SubjectId(uuid::Uuid::now_v7());
        engine.bind(subj, Role::Customer);

        assert!(engine.can(&subj, Permission::ReadOwnAccount));
        assert!(engine.can(&subj, Permission::InitiateOwnTransfer));
        assert!(engine.can(&subj, Permission::ViewOwnTransaction));
        assert!(!engine.can(&subj, Permission::ManageUsers));
        assert!(!engine.can(&subj, Permission::ReadAnyAccount));
    }

    #[test]
    fn test_rbac_auditor_read_only() {
        let mut engine = RbacEngine::new();
        let subj = SubjectId(uuid::Uuid::now_v7());
        engine.bind(subj, Role::Auditor);

        assert!(engine.can(&subj, Permission::ViewAuditLog));
        assert!(engine.can(&subj, Permission::VerifyChainIntegrity));
        assert!(engine.can(&subj, Permission::ViewTrialBalance));
        assert!(!engine.can(&subj, Permission::InitiateTransfer));
        assert!(!engine.can(&subj, Permission::ManageUsers));
    }

    #[test]
    fn test_rbac_teller_operational() {
        let mut engine = RbacEngine::new();
        let subj = SubjectId(uuid::Uuid::now_v7());
        engine.bind(subj, Role::Teller);

        assert!(engine.can(&subj, Permission::InitiateTransfer));
        assert!(engine.can(&subj, Permission::ReadAnyAccount));
        assert!(!engine.can(&subj, Permission::ManageUsers));
    }

    #[test]
    fn test_rbac_unbound_subject_denied() {
        let engine = RbacEngine::new();
        let subj = SubjectId(uuid::Uuid::now_v7());
        assert!(!engine.can(&subj, Permission::ReadAnyAccount));
        assert!(!engine.can(&subj, Permission::ManageUsers));
    }

    #[test]
    fn test_rbac_subject_multiple_roles() {
        let mut engine = RbacEngine::new();
        let subj = SubjectId(uuid::Uuid::now_v7());
        engine.bind(subj, Role::Auditor);
        engine.bind(subj, Role::Teller);

        // Has auditor perms
        assert!(engine.can(&subj, Permission::ViewAuditLog));
        // Has teller perms
        assert!(engine.can(&subj, Permission::InitiateTransfer));
        // No admin perms
        assert!(!engine.can(&subj, Permission::ManageUsers));
    }

    #[test]
    fn test_rbac_unbind_removes_access() {
        let mut engine = RbacEngine::new();
        let subj = SubjectId(uuid::Uuid::now_v7());
        engine.bind(subj, Role::Auditor);
        assert!(engine.can(&subj, Permission::ViewAuditLog));

        engine.unbind(&subj, Role::Auditor);
        assert!(!engine.can(&subj, Permission::ViewAuditLog));
    }

    #[test]
    fn test_rbac_can_all_permissions() {
        let mut engine = RbacEngine::new();
        let subj = SubjectId(uuid::Uuid::now_v7());
        engine.bind(subj, Role::Admin);

        assert!(engine.can_all(&subj, &[Permission::ManageUsers, Permission::ViewAuditLog, Permission::ReadAnyAccount]));
        // Auditor-only perm should also work for admin via role hierarchy
        assert!(engine.can_all(&subj, &[Permission::ViewAuditLog, Permission::ExportAuditReport]));
    }

    #[test]
    fn test_rbac_permissions_for_subject() {
        let mut engine = RbacEngine::new();
        let subj = SubjectId(uuid::Uuid::now_v7());
        engine.bind(subj, Role::Compliance);

        let perms: HashSet<_> = engine.permissions_for(&subj);
        assert!(perms.contains(&Permission::RedactPii));
        assert!(perms.contains(&Permission::ExportUserData));
        assert!(!perms.contains(&Permission::ManageUsers));
    }

    #[test]
    fn test_rbac_audit_export_is_json() {
        let mut engine = RbacEngine::new();
        engine.bind(SubjectId(uuid::Uuid::now_v7()), Role::Admin);
        let json = engine.export_audit();
        assert!(json.is_object());
    }
}
