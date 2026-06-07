//! Banking Ledger — high-throughput financial core in idiomatic Rust.
//!
//! # Architecture
//!
//! - `domain` — pure domain models (zero I/O, fully testable)
//! - `service` — business logic with concurrency control
//! - `log` — immutable append-only event store
//! - `store` — optional `SurrealDB` persistence
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
//! cargo run          # Start API server on :3001
//! cargo test         # 118 tests
//! python3 test_api.py # 19 integration tests
//! ```

#![deny(rust_2018_idioms)]
#![warn(missing_docs)]
#![warn(clippy::all)]
#![warn(clippy::pedantic)]
#![allow(clippy::module_name_repetitions)]

mod api;
mod domain;
mod extensions;
mod log;
mod service;
mod store;

#[cfg(test)]
mod edge_cases_test;

#[cfg(all(test, feature = "full"))]
mod exhaustive_edge_tests;

#[cfg(test)]
mod deep_correctness_tests;

#[tokio::main]
async fn main() {
    println!("═══════════════════════════════════════════");
    println!("  BANKING LEDGER — Rust Core");
    println!("  API server starting...");
    println!("═══════════════════════════════════════════\n");

    // Run the API server
    api::serve(3001).await.unwrap();
}
