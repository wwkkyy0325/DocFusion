//! Storage for DocFusion.
//!
//! Everything that touches the filesystem or a database lives here, keeping
//! `docfusion-core` free of I/O so it stays trivially testable.
//!
//! The Markdown file tree is the external contract. [`manifest`] is a sidecar
//! index over it that remembers what has been processed; it can always be
//! rebuilt from the tree, so it is never the only copy of anything.

pub mod atomic;
pub mod hash;
pub mod manifest;
