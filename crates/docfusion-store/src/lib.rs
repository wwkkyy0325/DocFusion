//! Everything that touches the outside world.
//!
//! The split between this crate and `docfusion-core` is effects, not domain:
//! core is pure — bytes in, values out — and this crate is where the filesystem
//! and the database are read and written, including [`pipeline`], which
//! sequences the two.
//!
//! The Markdown file tree is the external contract. [`manifest`] is a sidecar
//! index over it that remembers what has been processed; it can always be
//! rebuilt from the tree, so it is never the only copy of anything.

pub mod atomic;
pub mod config;
pub mod dirs;
pub mod engines;
pub mod endpoint;
pub mod hash;
pub mod manifest;
pub mod memory;
pub mod models;
pub mod ocr;
pub mod pipeline;
pub mod reconcile;
pub mod structure;
pub mod table;

/// Throws away every remembered answer about which models are on disk.
///
/// The three modules that ask — [`ocr`], [`structure`] and [`table`] — each
/// remember their answer, because the question costs a digest of everything it
/// covers and it is asked before every document. The thing that changes the
/// answer is one thing: a download. So this is one call rather than three, and
/// it lives above all three rather than inside any of them, so that a fourth
/// model cannot be added without somebody noticing this list.
///
/// Forgetting one and not the others is the failure worth naming: a window
/// still offering to fetch what it has just been given, or a page read as plain
/// text because the layout model it now has was asked about a moment too early.
pub fn forget_models() {
    ocr::forget();
    structure::forget();
    table::forget();
}
