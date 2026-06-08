//! Banking Ledger — high-throughput financial core in idiomatic Rust.
//!
//! # Architecture
//!
//! - `domain` — pure domain models (zero I/O, fully testable)
//! - `service` — business logic with concurrency control
//! - `log` — immutable append-only event store
//! - `store` — SurrealDB persistence (via Docker)
//! - `api` — Axum REST server
//!
//! # Design Philosophy
//!
//! 1. **Newtype wrapping** for type safety (Money wraps Decimal, `AccountId` wraps Uuid)
//! 2. **Lock-free where possible** — `AtomicI64` CAS for hot-path balance updates
//! 3. **Append-only immutability** — journal entries never modified, only reversed
//! 4. **Zero-cost abstractions** — no GC, no reflection, no runtime overhead
//! 5. **Compile-time guarantees** — Send/Sync proven by compiler, not tests
//!
//! # Quick Start
//!
//! ```bash
//! docker compose up -d    # SurrealDB + API
//! cargo test              # 176 tests
//! ```

#![deny(rust_2018_idioms)]
#![warn(missing_docs)]
#![warn(clippy::all)]
#![warn(clippy::pedantic)]
#![allow(clippy::module_name_repetitions)]

use std::sync::Arc;

mod api;
mod domain;
mod extensions;
mod log;
mod rbac;
mod service;
mod store;

#[cfg(test)]
mod edge_cases_test;

#[cfg(all(test, feature = "full"))]
mod exhaustive_edge_tests;

#[cfg(test)]
mod deep_correctness_tests;

#[cfg(test)]
mod audit_bug_regression;
mod boundary_probe_tests;

#[cfg(test)]
mod regression_tests;

#[tokio::main]
async fn main() {
    println!("═══════════════════════════════════════════");
    println!("  BANKING LEDGER — Rust Core");
    println!("═══════════════════════════════════════════");

    // Attempt SurrealDB connection if configured
    let surreal_url = std::env::var("SURREAL_URL").unwrap_or_default();
    let surreal_store = if !surreal_url.is_empty() {
        let user = std::env::var("SURREAL_USER").unwrap_or_else(|_| "root".into());
        let pass = std::env::var("SURREAL_PASS").unwrap_or_else(|_| "root".into());
        let ns = std::env::var("SURREAL_NS").unwrap_or_else(|_| "banking_ledger".into());
        let db = std::env::var("SURREAL_DB").unwrap_or_else(|_| "ledger".into());

        println!("  Connecting to SurrealDB: {surreal_url}");
        match store::SurrealStore::connect(&surreal_url, &ns, &db, &user, &pass).await {
            Ok(store) => {
                println!("  SurrealDB connected: {ns}/{db}");
                Some(store)
            }
            Err(e) => {
                eprintln!("  ⚠ SurrealDB unavailable: {e}");
                eprintln!("  Running in-memory mode (data lost on restart)");
                None
            }
        }
    } else {
        println!("  No SURREAL_URL set — in-memory mode");
        None
    };

    // Run the API server
    let port: u16 = std::env::var("API_PORT")
        .unwrap_or_else(|_| "3001".into())
        .parse()
        .unwrap_or(3001);

    api::serve(port, surreal_store.map(Arc::new)).await.unwrap();
}
