//! The model that finds structure on a page, and fetching it.
//!
//! This is the one thing the tool needs that is not inside it. `build.rs`
//! embeds the recognition weights — 6.9 MB — because they are a promise: a scan
//! is readable with no setup and with no network. What this module fetches is
//! 124 MB of layout detector, which is an improvement rather than a capability:
//! the tool reads every page it ever read without it, and finds no headings,
//! tables, headers or footers, which is what the recognition engine is.
//!
//! **Nothing here runs on its own.** A download happens because somebody asked
//! for one. There is no fetch at startup and no refresh in the background, so
//! an installation that never needs structure detection never pays 124 MB for
//! it — and one that does pays once.
//!
//! The shape of this follows the same decisions as the model downloader in the
//! Verse project: a state a caller can watch, resume rather than restart,
//! mirrors tried in order, verification before anything is trusted, and a
//! failure that is reported *and* returned. That last one is written as a
//! warning to whoever edits this next, because it is the one that fails
//! silently: a fetch that returns an error without telling the callback leaves
//! a window saying "downloading" for ever.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::RwLock;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use sha2::{Digest, Sha256};

/// The model, and everywhere it can be had.
pub struct Spec {
    /// Stable identifier, for anything that has to name it.
    pub id: &'static str,
    pub file: &'static str,
    /// The size it must be. Used to refuse a download there is no room for,
    /// before anything is written.
    pub bytes: u64,
    /// Lowercase hex, of the whole file.
    pub sha256: &'static str,
    /// Tried in order. The first that answers wins.
    ///
    /// Two because one is a single point of failure for a quarter of a gigabyte
    /// — a mirror having a bad hour should not be the end of it.
    pub mirrors: &'static [&'static str],
}

/// PaddleOCR's layout detector, large.
///
/// Large rather than medium because the medium weights have never been exported
/// to ONNX by anybody official, and the large one has. Nineteen megabytes of
/// accuracy given up to a packaging accident, which is worth knowing about.
///
/// Apache 2.0, like the rest of PaddleOCR.
pub const STRUCTURE: Spec = Spec {
    id: "pp-doclayout-plus-l",
    file: "PP-DocLayout_plus-L.onnx",
    bytes: 129_736_329,
    sha256: "77afb2caa74dd13240d087d2eced91d7fcd2caebd16006a0a66162fc8707ff0e",
    mirrors: &[
        "https://www.modelscope.cn/models/PaddlePaddle/PP-DocLayout_plus-L_onnx/resolve/master/inference.onnx",
        "https://huggingface.co/PaddlePaddle/PP-DocLayout_plus-L_onnx/resolve/main/inference.onnx",
    ],
};

/// PaddleOCR's table structure recogniser.
///
/// Reads a picture of a table and answers with the grid — the rows, the
/// columns, and a box for each cell — which is the half of a table that
/// geometry cannot do. The recognised lines are then put into the cells by
/// where the boxes are, which is what PP-Structure does and what this project
/// now has the other two thirds of: the detector and the recogniser are
/// `rapidocr`.
///
/// **7.4 MB**, which is why this one is not an argument. Its own size is what
/// settled the question the 124 MB layout model had to be measured to settle.
///
/// The mirror is first because the ONNX export is Hugging Face's alone —
/// ModelScope carries `PaddlePaddle/SLANet_plus` in the PaddlePaddle format,
/// which nothing here can load.
pub const TABLE: Spec = Spec {
    id: "slanet-plus",
    file: "SLANet_plus.onnx",
    bytes: 7_782_138,
    sha256: "7790c0c13ce064782c9d22ebeb16b4da8216f83d3ba576da962c106ef58386da",
    mirrors: &[
        "https://hf-mirror.com/PaddlePaddle/SLANet_plus_onnx/resolve/main/inference.onnx",
        "https://huggingface.co/PaddlePaddle/SLANet_plus_onnx/resolve/main/inference.onnx",
    ],
};

/// How long a mirror has to answer before the next one is tried.
///
/// Generous, because this is a quarter of a gigabyte and a slow connection is
/// not a broken one. The timeout is on *starting*, not on finishing: what it
/// catches is a mirror that accepts a connection and then says nothing.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);

/// How much is read at a time. Large enough that a quarter of a gigabyte is a
/// few thousand reads, small enough that a cancelled download stops promptly.
const CHUNK: usize = 64 * 1024;

/// How much progress may pass between two reports.
///
/// Without a floor the callback fires for every 64 KB, which for this file is
/// two thousand calls saying nothing.
const REPORT_EVERY: u64 = 4 * 1024 * 1024;

/// Room to leave on the disk past what is being written.
///
/// A filesystem needs space for its own bookkeeping, and a disk filled to the
/// last byte is a machine that has stopped.
const HEADROOM: u64 = 64 * 1024 * 1024;

/// What a fetch is doing, for whoever is watching.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum State {
    /// Nothing in progress.
    Idle,
    Fetching {
        /// The mirror being tried, by name.
        mirror: &'static str,
        /// Bytes of the whole file now on disk, carried over from before.
        received: u64,
        total: u64,
    },
    /// Everything is on disk; checking what arrived.
    Verifying,
    /// Complete and usable.
    Ready,
    /// Gave up. Written for a person, not for a log.
    Failed { reason: String },
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("could not create {path}: {source}")]
    Directory {
        path: PathBuf,
        source: io::Error,
    },

    #[error("not enough room for {file}: {needed} more bytes needed, {free} available")]
    NoRoom {
        file: String,
        needed: u64,
        free: u64,
    },

    #[error("could not fetch {file} from any mirror; the last said: {last}")]
    Unreachable { file: String, last: String },

    #[error(
        "{file} arrived but its digest is {actual}, not {expected}. It was deleted rather than \
         trusted: the weights are what the detector reads a page with"
    )]
    Corrupt {
        file: String,
        expected: &'static str,
        actual: String,
    },

    #[error("could not write {path}: {source}")]
    Write { path: PathBuf, source: io::Error },
}

/// What came of asking for the model.
#[derive(Debug)]
pub enum Outcome {
    /// On disk, and the digest checked.
    Ready(PathBuf),
    /// Somebody asked it to stop. Nothing is wrong; nothing was finished
    /// either, so there is nothing to report as a failure.
    Cancelled,
}

/// Where models that are not inside the binary live.
///
/// The same directory `build.rs` caches the embedded weights in, so that "where
/// does DocFusion put models" has one answer rather than two.
pub fn default_root() -> PathBuf {
    crate::dirs::user_dir().join("models")
}

/// Where the model lives once it is here.
pub fn path_in(root: &Path) -> PathBuf {
    path_of(&STRUCTURE, root)
}

/// Where any model lives once it is here.
pub fn path_of(spec: &Spec, root: &Path) -> PathBuf {
    root.join(spec.file)
}

/// Whether the named model is here and correct.
///
/// Reads the whole file, because a digest check that skips the bytes is not a
/// digest check. Called before every use rather than remembered, so a file that
/// somebody deleted, truncated or replaced is noticed then rather than read as
/// a page.
pub fn present_spec(spec: &Spec, root: &Path) -> bool {
    digest_of(&path_of(spec, root)).as_deref() == Some(spec.sha256)
}

/// The same, for the model this module is named after.
pub fn present(root: &Path) -> bool {
    present_spec(&STRUCTURE, root)
}

/// Whether the table structure model is here.
pub fn table_present(root: &Path) -> bool {
    present_spec(&TABLE, root)
}

/// A yes-or-no question about the disk, answered once and remembered until it
/// is forgotten.
///
/// The question this exists for reads 124 MB, so asking it per page would be a
/// per-page cost for something that only changes when a model arrives. Which is
/// also the reason it can be forgotten: the window fetches the model itself,
/// and a button that appeared to work but changed nothing until the next launch
/// would be a button that did not work.
///
/// Forgetting is safe between documents and not during one: what the answer
/// decides is the name a page is filed under, and that name has to stay the
/// same for every page of a document.
pub struct Cached(RwLock<Option<bool>>);

impl Cached {
    pub const fn new() -> Self {
        Self(RwLock::new(None))
    }

    /// The remembered answer, looking it up if this is the first question or
    /// the first since the last [`Cached::forget`].
    pub fn get(&self, look: impl FnOnce() -> bool) -> bool {
        // Taken for writing even to read, so that two threads asking at once
        // read the model once between them rather than twice.
        let mut known = self.0.write().unwrap_or_else(|p| p.into_inner());

        *known.get_or_insert_with(look)
    }

    /// Throws the answer away, so the next question looks at the disk again.
    pub fn forget(&self) {
        let mut known = self.0.write().unwrap_or_else(|p| p.into_inner());
        *known = None;
    }
}

impl Default for Cached {
    fn default() -> Self {
        Self::new()
    }
}

/// Fetches the model if it is not already here.
///
/// `on_state` is called as it goes, and called before *anything* is written, so
/// a caller drawing progress has something to draw from the first moment.
pub fn ensure(
    root: &Path,
    on_state: &mut dyn FnMut(&State),
    cancelled: &AtomicBool,
) -> Result<Outcome, Error> {
    ensure_of(&STRUCTURE, root, on_state, cancelled)
}

/// The same, for a spec supplied rather than the one this program ships.
///
/// Separate so that the tests can point the same code at a server they control
/// on a port that is free on this machine, and so that a caller with more than
/// one model can say which. A download path exercised against the real mirror
/// is exercised once, by hand, on the day somebody remembers.
pub fn ensure_of(
    spec: &Spec,
    root: &Path,
    on_state: &mut dyn FnMut(&State),
    cancelled: &AtomicBool,
) -> Result<Outcome, Error> {
    let path = root.join(spec.file);

    if digest_of(&path).as_deref() == Some(spec.sha256) {
        on_state(&State::Ready);
        return Ok(Outcome::Ready(path));
    }

    fs::create_dir_all(root).map_err(|source| Error::Directory {
        path: root.to_path_buf(),
        source,
    })?;

    // What survived an earlier attempt. Resumed rather than restarted: a
    // quarter of a gigabyte over a connection that blipped is worth keeping.
    let already = fs::metadata(&path).map(|meta| meta.len()).unwrap_or(0);

    if let Some(free) = free_space(root) {
        let needed = spec.bytes.saturating_sub(already);
        if needed.saturating_add(HEADROOM) > free {
            return Err(gave_up(
                on_state,
                Error::NoRoom {
                    file: spec.file.to_owned(),
                    needed: needed.saturating_add(HEADROOM),
                    free,
                },
            ));
        }
    }

    on_state(&State::Idle);

    let mut last = String::from("no mirror was tried");

    for mirror in spec.mirrors {
        match fetch(spec, mirror, &path, already, on_state, cancelled) {
            Ok(()) => {
                on_state(&State::Verifying);

                return match digest_of(&path) {
                    Some(actual) if actual == spec.sha256 => {
                        on_state(&State::Ready);
                        Ok(Outcome::Ready(path))
                    }
                    Some(actual) => {
                        // Deleted rather than kept: a file that is the wrong
                        // bytes is worse than no file, because the next run
                        // would try to resume from it.
                        let _ = fs::remove_file(&path);

                        Err(gave_up(
                            on_state,
                            Error::Corrupt {
                                file: spec.file.to_owned(),
                                expected: spec.sha256,
                                actual,
                            },
                        ))
                    }
                    None => Err(gave_up(
                        on_state,
                        Error::Write {
                            path: path.clone(),
                            source: io::Error::other("the file could not be read back"),
                        },
                    )),
                };
            }
            Err(Attempt::Cancelled) => {
                on_state(&State::Idle);
                return Ok(Outcome::Cancelled);
            }
            Err(Attempt::Failed(reason)) => last = reason,
        }
    }

    Err(gave_up(
        on_state,
        Error::Unreachable {
            file: spec.file.to_owned(),
            last,
        },
    ))
}

/// Reports a failure and answers it.
///
/// Both, every time, by construction. A version of this that returned without
/// reporting left the window saying "downloading" for ever, and the only way to
/// find out otherwise was a return value the window does not read.
fn gave_up(on_state: &mut dyn FnMut(&State), error: Error) -> Error {
    on_state(&State::Failed {
        reason: error.to_string(),
    });

    error
}

/// One attempt at one mirror.
enum Attempt {
    Failed(String),
    Cancelled,
}

fn fetch(
    spec: &Spec,
    mirror: &'static str,
    path: &Path,
    already: u64,
    on_state: &mut dyn FnMut(&State),
    cancelled: &AtomicBool,
) -> Result<(), Attempt> {
    let agent = ureq::Agent::new_with_config(
        ureq::Agent::config_builder()
            .timeout_connect(Some(CONNECT_TIMEOUT))
            .build(),
    );

    let mut request = agent.get(mirror);

    if already > 0 {
        request = request.header("Range", format!("bytes={already}-"));
    }

    let mut response = request
        .call()
        .map_err(|error| Attempt::Failed(format!("{mirror}: {error}")))?;

    // A server that ignored the `Range` answered 200 with the whole file, and
    // what is on disk is the beginning of it — so this starts again rather than
    // appending the whole file to its own first half.
    let resuming = already > 0 && response.status().as_u16() == 206;
    let received_before = if resuming { already } else { 0 };

    if !resuming {
        let _ = fs::remove_file(path);
    }

    let mut file = OpenOptions::new()
        .create(true)
        .append(resuming)
        .write(true)
        .truncate(!resuming)
        .open(path)
        .map_err(|error| Attempt::Failed(format!("{}: {error}", path.display())))?;

    let mut reader = response.body_mut().as_reader();
    let mut buffer = vec![0u8; CHUNK];
    let mut received = received_before;
    let mut reported = received_before;

    on_state(&State::Fetching {
        mirror,
        received,
        total: spec.bytes,
    });

    loop {
        if cancelled.load(Ordering::Relaxed) {
            return Err(Attempt::Cancelled);
        }

        let read = match reader.read(&mut buffer) {
            Ok(0) => break,
            Ok(read) => read,
            Err(error) => {
                // What was written stays: this is what the next attempt
                // resumes from. Removing it would make every interruption cost
                // everything before it.
                return Err(Attempt::Failed(format!("{mirror}: {error}")));
            }
        };

        file.write_all(&buffer[..read])
            .map_err(|error| Attempt::Failed(format!("{}: {error}", path.display())))?;

        received += read as u64;

        if received.saturating_sub(reported) >= REPORT_EVERY {
            reported = received;
            on_state(&State::Fetching {
                mirror,
                received,
                total: spec.bytes,
            });
        }
    }

    file.flush()
        .map_err(|error| Attempt::Failed(format!("{}: {error}", path.display())))?;

    on_state(&State::Fetching {
        mirror,
        received,
        total: spec.bytes,
    });

    Ok(())
}

/// The SHA-256 of a whole file, lowercase hex, or `None` if it cannot be read.
fn digest_of(path: &Path) -> Option<String> {
    let mut file = File::open(path).ok()?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; CHUNK];

    loop {
        match file.read(&mut buffer) {
            Ok(0) => break,
            Ok(read) => hasher.update(&buffer[..read]),
            Err(_) => return None,
        }
    }

    Some(
        hasher
            .finalize()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect(),
    )
}

/// Room left on the filesystem holding `path`, or `None` when the platform will
/// not say.
///
/// An unanswerable question must not refuse a download. This is a courtesy to
/// somebody about to wait for a quarter of a gigabyte, not a gate.
#[cfg(windows)]
fn free_space(path: &Path) -> Option<u64> {
    use std::os::windows::ffi::OsStrExt;

    // SAFETY: a plain call taking a null-terminated path and three out-pointers
    // that this scope owns and that outlive it.
    unsafe extern "system" {
        fn GetDiskFreeSpaceExW(
            directory: *const u16,
            available_to_caller: *mut u64,
            total: *mut u64,
            free: *mut u64,
        ) -> i32;
    }

    let wide: Vec<u16> = path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();

    let (mut for_us, mut total, mut free) = (0u64, 0u64, 0u64);

    let ok = unsafe { GetDiskFreeSpaceExW(wide.as_ptr(), &mut for_us, &mut total, &mut free) };

    (ok != 0).then_some(for_us)
}

#[cfg(not(windows))]
fn free_space(_path: &Path) -> Option<u64> {
    // Nothing here asks the question, and the answer is allowed to be "nobody
    // said". See above: the check must never be the reason a download is
    // refused.
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader};
    use std::net::{TcpListener, TcpStream};
    use std::sync::{Arc, Mutex};
    use std::thread;

    /// Big enough that a resume is a resume, small enough to be quick.
    const SIZE: usize = 300 * 1024;

    /// The whole reason the layout and table models can arrive while a window
    /// is open. A `OnceLock` gave this answer once and could not take it back,
    /// which made a download mid-session a download that changed nothing.
    #[test]
    fn the_answer_is_looked_up_once_and_again_after_forgetting() {
        let cell = Cached::new();

        assert!(cell.get(|| true));

        // Asking again must not look again: this closure is what looking costs,
        // and the answer above is the one that is still on record.
        assert!(cell.get(|| panic!("the disk was read twice for one answer")));

        // And after forgetting it must look again. The answer is inverted
        // rather than the closure counted, so a remembered `true` fails here
        // instead of passing on the strength of the first line.
        cell.forget();

        assert!(!cell.get(|| false), "forgetting did not make it look again");
    }

    /// Forgetting a question nobody has asked is not a way to get an answer
    /// without looking.
    #[test]
    fn forgetting_before_asking_still_looks() {
        let cell = Cached::new();
        cell.forget();

        assert!(!cell.get(|| false));
    }


    fn body() -> Vec<u8> {
        (0..SIZE).map(|i| ((i * 7) % 251) as u8).collect()
    }

    fn digest(bytes: &[u8]) -> String {
        Sha256::digest(bytes)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect()
    }

    /// Anything with a lifetime the tests can name for long enough. Leaked on
    /// purpose: a test that leaks a few bytes is a test that leaks a few bytes.
    fn leaked(text: String) -> &'static str {
        Box::leak(text.into_boxed_str())
    }

    /// What the server was asked for, so a test can tell a resume from a
    /// restart.
    type Asked = Arc<Mutex<Vec<Option<u64>>>>;

    /// A server that serves `bytes`, honouring `Range` only when told to.
    ///
    /// The "only when told to" is the case worth having: a server that ignores
    /// the header answers 200 with the whole file, and a client that appends
    /// that to what it already has ends up with a file of twice the length that
    /// is wrong in the middle.
    fn serving(bytes: Vec<u8>, honour_range: bool, asked: Asked) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let address = format!("http://{}", listener.local_addr().expect("address"));

        thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { break };
                let from = start_of(&mut stream);
                asked.lock().unwrap().push(from);

                let (status, rest) = match from {
                    Some(from) if honour_range => (
                        format!(
                            "206 Partial Content\r\nContent-Range: bytes {}-{}/{}",
                            from,
                            bytes.len() - 1,
                            bytes.len()
                        ),
                        &bytes[(from as usize).min(bytes.len())..],
                    ),
                    _ => ("200 OK".to_owned(), &bytes[..]),
                };

                let mut response = format!(
                    "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    rest.len()
                )
                .into_bytes();

                response.extend_from_slice(rest);
                let _ = stream.write_all(&response);
            }
        });

        address
    }

    /// The first byte asked for, or `None` when the whole file was asked for.
    fn start_of(stream: &mut TcpStream) -> Option<u64> {
        let mut reader = BufReader::new(stream.try_clone().ok()?);
        let mut from = None;

        loop {
            let mut line = String::new();
            if reader.read_line(&mut line).ok()? == 0 || line == "\r\n" {
                break;
            }
            if let Some(value) = line.to_ascii_lowercase().strip_prefix("range:") {
                from = value
                    .trim()
                    .strip_prefix("bytes=")?
                    .strip_suffix('-')?
                    .parse()
                    .ok();
            }
        }

        from
    }

    fn spec_for(bytes: &[u8], mirrors: Vec<&'static str>) -> Spec {
        Spec {
            id: "test",
            file: leaked("test.onnx".to_owned()),
            bytes: bytes.len() as u64,
            sha256: leaked(digest(bytes)),
            mirrors: Box::leak(mirrors.into_boxed_slice()),
        }
    }

    /// Runs a fetch and hands back both what it answered and everything it said
    /// on the way, because the two are checked together and by different tests.
    fn run(spec: &Spec, root: &Path, cancelled: &AtomicBool) -> (Result<Outcome, Error>, Vec<State>) {
        let mut states = Vec::new();
        let outcome = ensure_of(spec, root, &mut |state| states.push(state.clone()), cancelled);

        (outcome, states)
    }

    /// Nothing here is fetched twice. Pointed at a port with nothing behind it,
    /// so a fetch would not merely be wasteful, it would fail.
    #[test]
    fn a_file_that_is_already_here_is_not_fetched_again() {
        let dir = tempfile::tempdir().unwrap();
        let bytes = body();
        fs::write(dir.path().join("test.onnx"), &bytes).unwrap();

        let spec = spec_for(&bytes, vec!["http://127.0.0.1:1"]);
        let (outcome, states) = run(&spec, dir.path(), &AtomicBool::new(false));

        assert!(matches!(outcome, Ok(Outcome::Ready(_))), "{outcome:?}");
        assert_eq!(states, vec![State::Ready]);
    }

    #[test]
    fn a_download_lands_and_is_checked() {
        let dir = tempfile::tempdir().unwrap();
        let bytes = body();
        let asked: Asked = Default::default();
        let address = serving(bytes.clone(), true, asked);

        let spec = spec_for(&bytes, vec![leaked(address)]);
        let (outcome, states) = run(&spec, dir.path(), &AtomicBool::new(false));

        assert!(matches!(outcome, Ok(Outcome::Ready(_))), "{outcome:?}");
        assert_eq!(fs::read(dir.path().join("test.onnx")).unwrap(), bytes);

        // Starting, then asked for, then checked, then usable — in that order.
        // A caller drawing this has to hear about the checking rather than be
        // left to guess whether a long silence is work or a hang.
        let at = |want: fn(&State) -> bool| {
            states
                .iter()
                .position(want)
                .unwrap_or_else(|| panic!("never reported: {states:?}"))
        };

        assert_eq!(states.first(), Some(&State::Idle));
        assert!(at(|s| matches!(s, State::Fetching { .. })) < at(|s| *s == State::Verifying));
        assert!(at(|s| *s == State::Verifying) < at(|s| *s == State::Ready));
        assert_eq!(states.last(), Some(&State::Ready));
    }

    /// A quarter of a gigabyte over a connection that blipped. Starting again
    /// costs everything already paid for.
    #[test]
    fn an_interrupted_download_resumes_rather_than_starting_again() {
        let dir = tempfile::tempdir().unwrap();
        let bytes = body();
        let asked: Asked = Default::default();
        let address = serving(bytes.clone(), true, asked.clone());

        // As if an earlier attempt got this far.
        let so_far = SIZE / 3;
        fs::write(dir.path().join("test.onnx"), &bytes[..so_far]).unwrap();

        let spec = spec_for(&bytes, vec![leaked(address)]);
        let (outcome, _) = run(&spec, dir.path(), &AtomicBool::new(false));

        assert!(matches!(outcome, Ok(Outcome::Ready(_))), "{outcome:?}");
        assert_eq!(fs::read(dir.path().join("test.onnx")).unwrap(), bytes);

        // And it asked to carry on from there rather than from the beginning.
        assert_eq!(asked.lock().unwrap().as_slice(), &[Some(so_far as u64)]);
    }

    /// The half-written file is the beginning of the file, so a server that
    /// ignores `Range` and answers 200 must start it again.
    #[test]
    fn a_server_that_ignores_the_range_gets_a_file_started_over() {
        let dir = tempfile::tempdir().unwrap();
        let bytes = body();
        let asked: Asked = Default::default();
        let address = serving(bytes.clone(), false, asked);

        fs::write(dir.path().join("test.onnx"), &bytes[..SIZE / 3]).unwrap();

        let spec = spec_for(&bytes, vec![leaked(address)]);
        let (outcome, _) = run(&spec, dir.path(), &AtomicBool::new(false));

        assert!(matches!(outcome, Ok(Outcome::Ready(_))), "{outcome:?}");
        assert_eq!(fs::read(dir.path().join("test.onnx")).unwrap(), bytes);
    }

    /// A file of the wrong bytes is worse than no file, because the next run
    /// would resume from it.
    #[test]
    fn a_wrong_digest_is_deleted_rather_than_kept() {
        let dir = tempfile::tempdir().unwrap();
        let wanted = body();
        let served: Vec<u8> = wanted.iter().map(|byte| byte.wrapping_add(1)).collect();
        let asked: Asked = Default::default();
        let address = serving(served, true, asked);

        // The spec expects `wanted`; the mirror serves something else.
        let spec = spec_for(&wanted, vec![leaked(address)]);
        let (outcome, states) = run(&spec, dir.path(), &AtomicBool::new(false));

        assert!(matches!(outcome, Err(Error::Corrupt { .. })), "{outcome:?}");
        assert!(!dir.path().join("test.onnx").exists(), "the file was kept");
        assert!(matches!(states.last(), Some(State::Failed { .. })));
    }

    #[test]
    fn the_next_mirror_is_tried_when_the_first_does_not_answer() {
        let dir = tempfile::tempdir().unwrap();
        let bytes = body();
        let asked: Asked = Default::default();
        let address = serving(bytes.clone(), true, asked);

        let spec = spec_for(&bytes, vec!["http://127.0.0.1:1", leaked(address)]);
        let (outcome, _) = run(&spec, dir.path(), &AtomicBool::new(false));

        assert!(matches!(outcome, Ok(Outcome::Ready(_))), "{outcome:?}");
    }

    #[test]
    fn none_of_the_mirrors_answering_says_so() {
        let dir = tempfile::tempdir().unwrap();
        let spec = spec_for(&body(), vec!["http://127.0.0.1:1"]);

        let (outcome, _) = run(&spec, dir.path(), &AtomicBool::new(false));

        assert!(
            matches!(outcome, Err(Error::Unreachable { .. })),
            "{outcome:?}"
        );
    }

    #[test]
    fn a_cancelled_download_is_not_a_failure() {
        let dir = tempfile::tempdir().unwrap();
        let bytes = body();
        let asked: Asked = Default::default();
        let address = serving(bytes.clone(), true, asked);

        let spec = spec_for(&bytes, vec![leaked(address)]);
        let cancelled = AtomicBool::new(true);
        let (outcome, states) = run(&spec, dir.path(), &cancelled);

        assert!(matches!(outcome, Ok(Outcome::Cancelled)), "{outcome:?}");
        assert_eq!(states.last(), Some(&State::Idle));
    }

    /// The one the Verse downloader calls out as having been a real bug: a
    /// fetch that returns an error without telling the callback leaves a window
    /// saying "downloading" for ever, because the return value is the one thing
    /// the window does not read.
    #[test]
    fn every_failure_is_reported_as_well_as_returned() {
        let dir = tempfile::tempdir().unwrap();
        let spec = spec_for(&body(), vec!["http://127.0.0.1:1"]);

        let (outcome, states) = run(&spec, dir.path(), &AtomicBool::new(false));

        assert!(outcome.is_err());
        assert!(
            matches!(states.last(), Some(State::Failed { .. })),
            "the failure never reached the callback: {states:?}"
        );
    }
}
