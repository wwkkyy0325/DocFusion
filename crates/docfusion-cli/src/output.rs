//! The CLI's output contract.
//!
//! stdout carries results and nothing else: NDJSON under `--json`, human lines
//! otherwise. Progress, warnings and errors always go to stderr. A caller
//! parsing stdout should never have to defend against a log line, which is the
//! single most common way a command line tool breaks an agent that drives it.

use serde_json::Value;

/// Exit codes, as published by `docfusion schema`.
pub const OK: u8 = 0;
/// A general failure.
pub const FAILED: u8 = 1;
/// Some inputs succeeded and some did not.
pub const PARTIAL: u8 = 2;
/// No engine can read this input — it needs a backend that is not configured.
pub const NO_BACKEND: u8 = 3;

pub struct Reporter {
    json: bool,
}

impl Reporter {
    pub fn new(json: bool) -> Self {
        Self { json }
    }

    /// Writes one result.
    pub fn record(&self, value: Value, human: impl std::fmt::Display) {
        if self.json {
            println!("{value}");
        } else {
            println!("{human}");
        }
    }

    /// Writes a non-result message. Always stderr, whatever the mode.
    pub fn note(&self, message: impl std::fmt::Display) {
        eprintln!("{message}");
    }
}
