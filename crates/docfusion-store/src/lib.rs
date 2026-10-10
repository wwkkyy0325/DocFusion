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
pub mod layout;
pub mod manifest;
pub mod memory;
pub mod ocr;
pub mod pipeline;
pub mod reconcile;
pub mod structure;
pub mod table;
