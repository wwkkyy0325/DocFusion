//! Where DocFusion keeps the files it owns on this machine.

use std::path::{Path, PathBuf};

/// The per-user directory for everything the tool stores outside a knowledge
/// base: the extracted model weights, and the configuration file.
///
/// Per user rather than per knowledge base, because both are the same for every
/// knowledge base. Extracting the weights per document store would multiply
/// 6.9 MB by however many there are, and a configuration file beside the
/// documents is a configuration file somebody eventually commits.
///
/// The variants below are the three places this can reasonably go. The last is
/// a fallback rather than a preference: somewhere writable is better than
/// failing to start, and anything that cares which it got is welcome to say so.
/// Where the knowledge base goes when nobody has said.
///
/// One place, shared by the window and the command line, because a personal
/// knowledge base is one library — two tools pointing at two directories
/// because they were started differently is how a library becomes two half
/// libraries.
///
/// Under the user's documents rather than the tool's own directory: this is
/// Markdown somebody may want to open, sync or back up, and it is data rather
/// than application state. It is also absolute, which matters because a desktop
/// application started from a shortcut has no working directory a relative path
/// could be resolved against.
pub fn default_knowledge_base() -> PathBuf {
    if let Ok(profile) = std::env::var("USERPROFILE") {
        return Path::new(&profile).join("Documents").join("DocFusion");
    }
    if let Ok(home) = std::env::var("HOME") {
        return Path::new(&home).join("DocFusion");
    }
    user_dir().join("knowledge")
}

pub fn user_dir() -> PathBuf {
    if let Ok(explicit) = std::env::var("DOCFUSION_HOME") {
        return PathBuf::from(explicit);
    }
    if let Ok(local) = std::env::var("LOCALAPPDATA") {
        return Path::new(&local).join("DocFusion");
    }
    if let Ok(home) = std::env::var("HOME") {
        return Path::new(&home).join(".cache").join("docfusion");
    }
    std::env::temp_dir().join("docfusion")
}
