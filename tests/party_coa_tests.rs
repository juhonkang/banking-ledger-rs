//! Integration tests for Party + COA endpoints.
//! Run with: cargo test --test party_coa_tests -- --ignored
//! Requires server on :3001

#[cfg(test)]
mod tests {
    use std::process::Command;

    const BASE: &str = "http://127.0.0.1:3001";

    fn curl(path: &str) -> String {
        let output = Command::new("curl")
            .args(["-s", &format!("{BASE}{path}")])
            .output()
            .expect("curl failed");
        String::from_utf8_lossy(&output.stdout).to_string()
    }

    fn curl_post(path: &str, body: &str) -> String {
        let output = Command::new("curl")
            .args([
                "-s", "-X", "POST",
                &format!("{BASE}{path}"),
                "-H", "Content-Type: application/json",
                "-d", body,
            ])
            .output()
            .expect("curl failed");
        String::from_utf8_lossy(&output.stdout).to_string()
    }

    #[test]
    #[ignore = "requires running server on :3001"]
    fn test_health() {
        let resp = curl("/health");
        assert!(resp.contains("healthy"));
    }

    #[test]
    #[ignore = "requires running server on :3001"]
    fn test_coa_summary() {
        let resp = curl("/coa");
        assert!(resp.contains("Asset"));
        assert!(resp.contains("Liability"));
        assert!(resp.contains("Equity"));
        assert!(resp.contains("Revenue"));
        assert!(resp.contains("Expense"));
        assert!(resp.contains("normal_balance"));
    }

    #[test]
    #[ignore = "requires running server on :3001"]
    fn test_create_and_get_party() {
        let create = curl_post("/parties", r#"{"party_type": "individual", "legal_name": "Alice Tester"}"#);
        assert!(create.contains("Active"), "create party: {create}");

        let id: String = if let Some(start) = create.find("\"id\":\"") {
            let rest = &create[start + 6..];
            rest[..rest.find('"').unwrap_or(36)].to_string()
        } else {
            panic!("No id in response: {create}")
        };

        let get = curl(&format!("/parties/{id}"));
        assert!(get.contains("Alice Tester"), "get party: {get}");

        let list = curl("/parties");
        assert!(list.contains("Alice Tester"), "list parties: {list}");
    }
}
