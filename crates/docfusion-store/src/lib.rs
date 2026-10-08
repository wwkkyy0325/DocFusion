//! Storage for DocFusion.
//!
//! Everything that touches the filesystem or a database lives here, keeping
//! `docfusion-core` free of I/O so it stays trivially testable.

pub mod atomic;
