//! Atomic file replacement.
//!
//! Outputs are written to a temporary file in the destination directory, flushed
//! to disk, then moved into place. A reader therefore sees either the previous
//! contents or the new contents, never a half-written file — which matters
//! because agents read the knowledge tree while a pipeline run is in progress.

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// Distinguishes temporary files created within this process.
static COUNTER: AtomicU64 = AtomicU64::new(0);

/// Writes `contents` to `path`, replacing any existing file in one step.
///
/// The parent directory must already exist. On failure the temporary file is
/// removed, so a failed write leaves no debris behind.
pub fn write_atomic(path: &Path, contents: &[u8]) -> io::Result<()> {
    let temp = temp_path(path);
    match write_then_replace(&temp, path, contents) {
        Ok(()) => Ok(()),
        Err(err) => {
            let _ = fs::remove_file(&temp);
            Err(err)
        }
    }
}

fn write_then_replace(temp: &Path, destination: &Path, contents: &[u8]) -> io::Result<()> {
    let mut file = fs::File::create(temp)?;
    file.write_all(contents)?;
    file.sync_all()?;
    // Windows refuses to move a file that is still open, so close it first.
    drop(file);

    replace(temp, destination)
}

/// Builds the temporary path, kept in the destination directory.
///
/// It must share the destination's filesystem for the replace to be atomic, and
/// the leading dot keeps it out of the way of globbing tools.
fn temp_path(path: &Path) -> PathBuf {
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "output".to_owned());
    let sequence = COUNTER.fetch_add(1, Ordering::Relaxed);

    path.with_file_name(format!(
        ".{name}.{}.{sequence}.tmp",
        std::process::id()
    ))
}

/// Replaces `destination` with `source`, overwriting it if present.
///
/// `std::fs::rename` cannot be used here: on Windows it fails with
/// `ERROR_ALREADY_EXISTS` when the destination exists, while on Unix it
/// silently overwrites. Reprocessing any document that already has output
/// would therefore fail on the project's primary platform.
///
/// `MoveFileExW` with `MOVEFILE_REPLACE_EXISTING` handles both the create and
/// overwrite cases in a single atomic step.
#[cfg(windows)]
fn replace(source: &Path, destination: &Path) -> io::Result<()> {
    use std::os::windows::ffi::OsStrExt;

    const MOVEFILE_REPLACE_EXISTING: u32 = 0x1;
    const MOVEFILE_WRITE_THROUGH: u32 = 0x8;

    unsafe extern "system" {
        fn MoveFileExW(existing: *const u16, new: *const u16, flags: u32) -> i32;
    }

    let wide = |path: &Path| -> Vec<u16> {
        path.as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect()
    };

    let source = wide(source);
    let destination = wide(destination);

    // SAFETY: both buffers are null-terminated UTF-16 owned by this scope and
    // outlive the call.
    let ok = unsafe {
        MoveFileExW(
            source.as_ptr(),
            destination.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    };

    if ok == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(not(windows))]
fn replace(source: &Path, destination: &Path) -> io::Result<()> {
    fs::rename(source, destination)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read(path: &Path) -> String {
        fs::read_to_string(path).expect("read back")
    }

    fn entries(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(dir)
            .expect("read dir")
            .map(|entry| entry.expect("entry").file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    #[test]
    fn creates_a_new_file() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("out.md");

        write_atomic(&target, b"# Title\n").unwrap();

        assert_eq!(read(&target), "# Title\n");
    }

    /// Regression test for the Windows landmine described in `replace`.
    /// This must pass on Windows, not merely on Unix CI.
    #[test]
    fn overwrites_an_existing_file() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("out.md");

        write_atomic(&target, b"first").unwrap();
        write_atomic(&target, b"second, longer").unwrap();

        assert_eq!(read(&target), "second, longer");
    }

    #[test]
    fn overwrites_an_existing_empty_file() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("out.md");
        fs::write(&target, b"").unwrap();

        write_atomic(&target, b"content").unwrap();

        assert_eq!(read(&target), "content");
    }

    #[test]
    fn leaves_no_temporary_files_behind() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("out.md");

        write_atomic(&target, b"one").unwrap();
        write_atomic(&target, b"two").unwrap();

        assert_eq!(entries(dir.path()), vec!["out.md".to_owned()]);
    }

    #[test]
    fn failed_write_leaves_no_debris() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("missing-dir").join("out.md");

        assert!(write_atomic(&target, b"content").is_err());

        assert!(entries(dir.path()).is_empty(), "temp file was not cleaned up");
    }

    #[test]
    fn temporary_names_do_not_collide() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("out.md");

        let first = temp_path(&target);
        let second = temp_path(&target);

        assert_ne!(first, second);
        assert_eq!(first.parent(), target.parent());
    }
}
