//! Core types and pipeline for DocFusion.
//!
//! This crate is the pure heart of the project: the intermediate
//! representation, extraction engines, verification, and Markdown rendering.
//! It performs no file I/O — engines receive bytes and return values — so it
//! stays trivially testable and both the CLI and the GUI are thin shells over
//! it.

pub mod bench;
pub mod extract;
pub mod image;
pub mod ir;
pub mod mime;
pub mod render;
