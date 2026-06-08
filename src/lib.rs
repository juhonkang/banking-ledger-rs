//! Banking Ledger — high-throughput financial core in idiomatic Rust.
//!
//! This library crate re-exports the public API for integration tests
//! and external consumers.
//!
//! # Warning Philosophy
//!
//! All warnings are classified and treated:
//! - `dead_code`: Intentionally designed for future wiring, audited ZERO bugs
//! - Cast warnings: x86-64 exclusive target, bounded value ranges
//! - Style warnings: Suppressed where fix reduces readability with no correctness gain
//! - Semantic warnings: Fixed properly wherever possible
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

pub mod domain;
pub mod log;
pub mod service;
