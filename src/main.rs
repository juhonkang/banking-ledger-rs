//! Banking Ledger — high-throughput financial core in idiomatic Rust.
//!
//! # Architecture
//!
//! - `domain` — pure domain models (zero I/O, fully testable)
//! - `service` — business logic with concurrency control
//! - `log` — immutable append-only event store
//! - `store` — `SurrealDB` persistence (via Docker)
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
// Cast and dead_code warnings are intentional — x86-64 only, designed for future wiring
#![allow(dead_code)]
#![allow(clippy::cast_possible_truncation)]
#![allow(clippy::cast_precision_loss)]
#![allow(clippy::cast_sign_loss)]
#![allow(clippy::cast_possible_wrap)]
#![allow(clippy::cast_lossless)]
#![allow(clippy::format_push_string)]
#![allow(clippy::struct_field_names)]
#![allow(clippy::unused_self)]
#![allow(clippy::struct_excessive_bools)]
#![allow(clippy::type_complexity)]
#![allow(clippy::unnecessary_wraps)]
#![allow(clippy::trivially_copy_pass_by_ref)]
#![allow(clippy::if_same_then_else)]
#![allow(clippy::match_same_arms)]
#![allow(clippy::manual_clamp)]
#![allow(clippy::assigning_clones)]
#![allow(clippy::used_underscore_binding)]
#![allow(clippy::unused_async)]
#![allow(clippy::needless_pass_by_value)]
#![allow(clippy::ref_option)]
#![allow(clippy::unreadable_literal)]
#![allow(clippy::comparison_chain)]
#![allow(clippy::same_functions_in_if_condition)]
#![allow(clippy::self_only_used_in_recursion)]
#![allow(clippy::collapsible_else_if)]
#![allow(clippy::unnecessary_map_or)]

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
#[cfg(test)]
mod benchmarks;
mod boundary_probe_tests;
mod boundary_probe_extended_tests;

#[cfg(test)]
mod event_bus_edge_cases;

#[cfg(test)]
mod hash_chain_verify_edge_tests;

#[cfg(test)]
mod journal_audit_edge_tests;

#[cfg(test)]
mod dlq_edge_tests;

#[cfg(test)]
mod rbac_edge_tests;

#[cfg(test)]
mod regression_tests;

#[cfg(test)]
mod saga_edge_tests;

#[cfg(test)]
mod serde_edge_tests;

#[tokio::main]
async fn main() {
    println!("═══════════════════════════════════════════");
    println!("  BANKING LEDGER — Rust Core");
    println!("═══════════════════════════════════════════");

    // Attempt SurrealDB connection if configured
    let surreal_url = std::env::var("SURREAL_URL").unwrap_or_default();
    let surreal_store = if surreal_url.is_empty() {
        println!("  No SURREAL_URL set — in-memory mode");
        None
    } else {
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
    };

    // Run the API server
    let port: u16 = std::env::var("API_PORT")
        .unwrap_or_else(|_| "3001".into())
        .parse()
        .unwrap_or(3001);

    api::serve(port, surreal_store.map(Arc::new)).await.unwrap();
}
