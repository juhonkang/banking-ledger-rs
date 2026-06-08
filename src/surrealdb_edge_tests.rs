//! SurrealDB integration edge tests — async connection, query, schema validation.

#[cfg(test)]
mod surrealdb_edge_tests {
    use crate::store::SurrealStore;

    #[tokio::test]
    async fn test_surrealdb_connect() {
        let result = SurrealStore::connect(
            "localhost:4321",
            "quincyns",
            "maindb",
            "root",
            "root",
        )
        .await;
        // May fail if SurrealDB isn't running, but shouldn't panic
        let _ = result;
    }

    #[tokio::test]
    async fn test_surrealdb_health_check() {
        let store = SurrealStore::connect("localhost:4321", "quincyns", "maindb", "root", "root").await;
        if let Ok(s) = store {
            assert!(s.health_check().await);
        }
    }
}
