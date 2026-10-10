//! The models this tool reads a page with, and fetching them.
//!
//! **One set.** Six files, 144 MB, listed below and nowhere else, and none of
//! them inside the binary. That last part is the trade this module made: the
//! recognition weights used to be compiled in by `build.rs`, which made a fresh
//! installation read a scan with no network at all — at the price of a second
//! directory models live in, a second set of digests to keep in step with
//! upstream, and a settings tab that could only describe half of what it was
//! talking about. An installation is now either complete or it has nothing, and
//! the file that says what "complete" means is this one.
//!
//! What was given up is worth stating plainly, because four comments in this
//! tree used to promise it: a fresh install reads no image and no scanned PDF
//! until 144 MB has arrived, and the command line is no different from the
//! window in that respect.
//!
//! The two halves of the set are not equal. The recognition weights are 7 MB
//! and are not optional — without them a page of text that is a picture cannot
//! be read at all. The layout detector is 124 MB and is an improvement rather
//! than a capability: the tool reads every page it ever read without it, and
//! finds no headings, tables, headers or footers. They arrive together because
//! one set is easier to describe, to verify and to test than two — not because
//! they are alike.
//!
//! **Nothing here runs on its own.** A download happens because somebody asked
//! for one. There is no fetch at startup and no refresh in the background.
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

/// One model file, and everywhere it can be had.
pub struct Spec {
    /// Stable identifier, for anything that has to name it.
    pub id: &'static str,
    /// Where it goes under the root, as a path rather than a name.
    ///
    /// Four of the six sit in a directory of their own, because `rapidocr`
    /// looks for the recogniser's weights in one directory named after the
    /// model set and there is no way to tell it otherwise.
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

/// The four the recogniser reads a page with: where the lines are, which way up
/// they are, what they say, and the characters they may say it with.
///
/// They were embedded in the binary until this change and are now fetched like
/// everything else, which is why their digests are pinned twice — here and in
/// `rapidocr-core` — and held together by a test. The crate cannot import this
/// module and this module cannot ask the crate at compile time, so the two
/// lists are compared at test time instead; an upstream update then fails loudly
/// rather than leaving a downloader fetching weights the pipeline no longer
/// expects. The filenames, URLs and digests below are the crate's, copied.
///
/// They land in a directory named for the model set, because that is where the
/// crate looks for them. One mirror each, where the two large models below get
/// two apiece: these are 7 MB in total, so a mirror having a bad hour costs a
/// retry and not a wait, and inventing a second URL for them would be inventing
/// a URL.
///
/// Apache 2.0, like the rest of PaddleOCR.
pub const DETECTION: Spec = Spec {
    id: "pp-ocrv6-det-tiny",
    file: "ppocrv6-tiny/PP-OCRv6_det_tiny.onnx",
    bytes: 1_829_618,
    sha256: "f42c0fbd294d95eac1a550e131b277dac97462c8025fa4b6c3cec1b7894bd3d5",
    mirrors: &[
        "https://www.modelscope.cn/models/RapidAI/RapidOCR/resolve/v3.9.0/onnx/PP-OCRv6/det/PP-OCRv6_det_tiny.onnx",
    ],
};

/// What the lines the detector found actually say.
///
/// Tiny rather than small or medium. Both were measured on the corpus and
/// neither was kept: small read nine cases better and four worse, medium read
/// four of nine worse than tiny and took twice as long. See the changelog entry
/// for the run, because "we could not measure a difference worth 139 MB" is a
/// decision somebody will otherwise make again.
pub const RECOGNITION: Spec = Spec {
    id: "pp-ocrv6-rec-tiny",
    file: "ppocrv6-tiny/PP-OCRv6_rec_tiny.onnx",
    bytes: 4_489_813,
    sha256: "e16e242de5937ad92609223f19bc2aff3727ee40b095f996907c24749bad251b",
    mirrors: &[
        "https://www.modelscope.cn/models/RapidAI/RapidOCR/resolve/v3.9.0/onnx/PP-OCRv6/rec/PP-OCRv6_rec_tiny.onnx",
    ],
};

/// Which way up a line is, before anything tries to read it.
///
/// Shared across every model set upstream ships, and required by all of them:
/// the pipeline classifies direction before it recognises. So this one is not a
/// choice and has no alternatives to weigh.
pub const CLASSIFIER: Spec = Spec {
    id: "ppocr-mobile-cls",
    file: "ppocrv6-tiny/ch_ppocr_mobile_v2.0_cls_mobile.onnx",
    bytes: 585_532,
    sha256: "e47acedf663230f8863ff1ab0e64dd2d82b838fceb5957146dab185a89d6215c",
    mirrors: &[
        "https://www.modelscope.cn/models/RapidAI/RapidOCR/resolve/v3.9.0/onnx/PP-OCRv4/cls/ch_ppocr_mobile_v2.0_cls_mobile.onnx",
    ],
};

/// The characters the recogniser may choose from, one per line.
///
/// Twenty-seven kilobytes, and without it the recogniser answers with indices
/// into a list nobody has. Pinned to `master` rather than a tag because this is
/// where upstream keeps the tiny dictionary — there is no release of it.
pub const DICTIONARY: Spec = Spec {
    id: "ppocrv6-tiny-dict",
    file: "ppocrv6-tiny/ppocrv6_tiny_dict.txt",
    bytes: 27_156,
    sha256: "c5cbe34ef40c29c4df07ed012bf96569cb69a2d2a01a07027e9f13cb832bd9cd",
    mirrors: &[
        "https://www.modelscope.cn/models/RapidAI/RapidOCR/resolve/master/paddle/PP-OCRv6/rec/PP-OCRv6_rec_tiny/ppocrv6_tiny_dict.txt",
    ],
};

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

/// The set: everything this tool reads a page with, in the order it arrives.
///
/// This is the list. Nothing else in the tree decides which models there are —
/// a download walks it, `doctor` counts it, and the window draws it as one
/// thing, because one set is the point of the change this list came from.
///
/// Ordered smallest first, which is the order to fetch in rather than the order
/// to think about them in. A mirror that is going to fail should fail on the
/// 27 KB file rather than after 124 MB has been spent on its neighbour.
///
/// The four recognition files must be here for a page of text that is a picture
/// to be read at all. `STRUCTURE` and `TABLE` are improvements on that, and the
/// engine is built without them when they are absent — see `ocr::local`.
pub const SET: &[&Spec] = &[
    &DICTIONARY,
    &CLASSIFIER,
    &DETECTION,
    &RECOGNITION,
    &TABLE,
    &STRUCTURE,
];

/// The four that have to be here for a scan to be read at all.
pub const RECOGNITION_SET: &[&Spec] = &[&DICTIONARY, &CLASSIFIER, &DETECTION, &RECOGNITION];

/// The directory those four sit in.
///
/// Two things have to agree on this and neither can be computed from the other:
/// the paths above, which is where the downloader writes, and the name
/// `rapidocr` is asked for the model set by, which is where the pipeline reads.
/// A test holds them together.
pub const RECOGNITION_DIR: &str = "ppocrv6-tiny";

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

/// Where every model in the set goes.
///
/// One directory, and the only answer to "where does DocFusion put models"
/// there is. It used to be two — this one for the layout and table models, and
/// `DOCFUSION_MODEL_CACHE` for the weights `build.rs` unpacked — which is the
/// kind of thing that is true until somebody moves a file.
///
/// `DOCFUSION_MODEL_CACHE` still wins when it is set, and now it means the whole
/// set rather than the half `build.rs` happened to fetch. Anybody who set it
/// gets one directory answering to it instead of two answering to different
/// things.
pub fn default_root() -> PathBuf {
    match std::env::var("DOCFUSION_MODEL_CACHE") {
        Ok(explicit) if !explicit.is_empty() => PathBuf::from(explicit),
        _ => crate::dirs::user_dir().join("models"),
    }
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

/// Whether every one of `specs` is here, which is the whole question and never
/// a fraction of it.
///
/// Costs the digest of everything named, so a caller that asks on a schedule
/// wants [`Cached`] and one that asks once wants this.
pub fn present_all(specs: &[&Spec], root: &Path) -> bool {
    specs.iter().all(|spec| present_spec(spec, root))
}

/// Which of `specs` are not here.
///
/// The specs rather than their names, because the two callers want different
/// things from the same answer: one names them in a sentence somebody reads
/// when a page will not read, and the other hands them to [`still_to_fetch`],
/// which is only correct about files that are missing.
pub fn absent<'a>(specs: &[&'a Spec], root: &Path) -> Vec<&'a Spec> {
    specs
        .iter()
        .copied()
        .filter(|spec| !present_spec(spec, root))
        .collect()
}

/// Which of the set is missing, given the three answers a caller already has.
///
/// The three booleans are the remembered ones — `structure::available`,
/// `table::available`, `ocr::available` — rather than fresh digests, because
/// this is asked on every redraw and answering it honestly would read 144 MB a
/// frame. Which of them covers what is the one place that mapping is written
/// down.
pub fn absent_from(recognition: bool, structure: bool, tables: bool) -> Vec<&'static Spec> {
    RECOGNITION_SET
        .iter()
        .copied()
        .filter(|_| !recognition)
        .chain(std::iter::once(&TABLE).filter(|_| !tables))
        .chain(std::iter::once(&STRUCTURE).filter(|_| !structure))
        .collect()
}

/// How much of these — already known to be missing — would still have to
/// arrive.
///
/// Answered from the file's length rather than from its digest, because this
/// feeds a sentence about megabytes and the digest of the layout model is
/// 124 MB of hashing. A file that is already the full length but failed its
/// digest counts as wholly still to come, because it is: the whole of it is
/// replaced rather than resumed, which is why the caller passing a file it
/// knows is present would get a wrong answer.
pub fn still_to_fetch(specs: &[&Spec], root: &Path) -> u64 {
    specs
        .iter()
        .map(|spec| {
            let have = fs::metadata(path_of(spec, root))
                .map(|meta| meta.len())
                .unwrap_or(0);

            if have >= spec.bytes {
                spec.bytes
            } else {
                spec.bytes - have
            }
        })
        .sum()
}

/// What the whole set weighs, which is what a person deciding whether to press
/// the button is asking.
pub fn set_bytes() -> u64 {
    SET.iter().map(|spec| spec.bytes).sum()
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

    // The file's own directory rather than the root: one of the six sits a
    // level down, and `create_dir_all` on the root would leave nowhere to put
    // it and a failure that names the wrong directory.
    let here = path.parent().unwrap_or(root);
    fs::create_dir_all(here).map_err(|source| Error::Directory {
        path: here.to_path_buf(),
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

    /// The same but for a model that lives a level down, which four of the six
    /// do because `rapidocr` looks for them in a directory of its own.
    ///
    /// Worth its own case rather than a line in the test above: the directory
    /// has to be created, and the version of this that created the *root*
    /// instead would pass every other test in this module and fail on the first
    /// real download.
    #[test]
    fn a_model_in_a_directory_of_its_own_lands_there() {
        let dir = tempfile::tempdir().unwrap();
        let bytes = body();
        let asked: Asked = Default::default();
        let address = serving(bytes.clone(), true, asked);

        let mut spec = spec_for(&bytes, vec![leaked(address)]);
        spec.file = "ppocrv6-tiny/test.onnx";

        let (outcome, _) = run(&spec, dir.path(), &AtomicBool::new(false));
        let landed = dir.path().join("ppocrv6-tiny").join("test.onnx");

        assert!(matches!(outcome, Ok(Outcome::Ready(_))), "{outcome:?}");
        assert_eq!(fs::read(&landed).unwrap(), bytes);
        assert!(present_spec(&spec, dir.path()));
    }

    /// The set is the whole answer to "what does this tool read a page with".
    ///
    /// Held against the constants rather than against a number typed here, so
    /// that adding a model fails until somebody has looked at what it does to
    /// the total the window puts on the button.
    #[test]
    fn the_set_is_the_models_and_nothing_else() {
        assert_eq!(
            SET.len(),
            RECOGNITION_SET.len() + 2,
            "the set is the four a page cannot be read without, plus layout and tables"
        );

        for spec in RECOGNITION_SET {
            assert!(
                SET.iter().any(|other| other.id == spec.id),
                "{} is in no set",
                spec.file
            );
        }

        // No file twice, which would download it twice and put two digest
        // checks on the same path.
        let mut ids: Vec<&str> = SET.iter().map(|spec| spec.id).collect();
        ids.sort_unstable();
        let count = ids.len();
        ids.dedup();
        assert_eq!(ids.len(), count, "two models share an id");

        assert_eq!(
            set_bytes(),
            SET.iter().map(|spec| spec.bytes).sum::<u64>()
        );

        // The one number the window shows before anything has been fetched.
        // Spelled out rather than compared to itself: a changed total means a
        // changed download, and that is a thing to have to look at.
        assert_eq!(set_bytes(), 144_450_586);
    }

    /// Every one of the four is found by the crate where the catalogue puts it.
    ///
    /// The path and the model-set name are two facts that cannot be computed
    /// from each other — one is written by hand in the table above and the other
    /// is what the pipeline is asked for by name — so this is the only place
    /// they meet.
    #[test]
    fn the_recognition_files_are_where_the_pipeline_looks() {
        let root = Path::new("/models");
        let model_set = rapidocr_core::model::model_set_by_name(RECOGNITION_DIR).expect("model set");
        let cache = rapidocr_core::model::ModelCache::new(root.join(RECOGNITION_DIR));

        for asset in model_set.assets() {
            let ours = RECOGNITION_SET
                .iter()
                .find(|spec| spec.file.ends_with(asset.filename))
                .unwrap_or_else(|| panic!("{} is not in the catalogue", asset.filename));

            assert_eq!(
                path_of(ours, root),
                cache.asset_path(asset),
                "{} is downloaded somewhere the pipeline does not look",
                asset.filename
            );
        }
    }

    /// A missing file is named, not counted.
    #[test]
    fn what_is_not_here_is_named() {
        let dir = tempfile::tempdir().unwrap();
        let here = spec_for(&body(), vec!["http://127.0.0.1:1"]);
        fs::write(dir.path().join("test.onnx"), body()).unwrap();

        let mut away = spec_for(&body(), vec!["http://127.0.0.1:1"]);
        away.file = "away.onnx";

        assert!(absent(&[&here], dir.path()).is_empty());

        let missing = absent(&[&here, &away], dir.path());
        assert_eq!(missing.len(), 1);
        assert_eq!(missing[0].file, "away.onnx");

        assert!(present_all(&[&here], dir.path()));
        assert!(!present_all(&[&here, &away], dir.path()));

        // And the bytes still to come are the whole of the one that is missing,
        // because a file that is not there at all has nothing to resume from.
        assert_eq!(still_to_fetch(&missing, dir.path()), away.bytes);
    }

    /// A download that was stopped is resumed rather than restarted, so the
    /// window would be wrong to promise the whole file again.
    #[test]
    fn a_partial_download_is_only_what_is_left() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(path_of(&TABLE, dir.path()), [0u8; 4096]).unwrap();

        assert_eq!(still_to_fetch(&[&TABLE], dir.path()), TABLE.bytes - 4096);
    }

    /// The length check would otherwise read this as a finished download and
    /// say there is nothing to fetch. The fetch does not agree: it checks the
    /// digest, throws this away and starts again.
    #[test]
    fn a_wrong_file_of_the_right_length_is_still_all_to_come() {
        let dir = tempfile::tempdir().unwrap();
        let path = path_of(&TABLE, dir.path());

        std::fs::File::create(&path)
            .unwrap()
            .set_len(TABLE.bytes)
            .unwrap();

        assert_eq!(still_to_fetch(&[&TABLE], dir.path()), TABLE.bytes);
    }

    /// The three remembered answers, turned back into a list of files.
    ///
    /// Asked every redraw, so it is arithmetic over booleans rather than
    /// digests — which is worth a test precisely because it cannot notice a
    /// file appearing or disappearing on its own.
    #[test]
    fn what_is_missing_follows_from_the_three_answers() {
        let all = absent_from(true, true, true);
        assert_eq!(all.len(), 0, "nothing should be missing when all three are");

        // Everything, which is a fresh installation and the case the window
        // draws a size for.
        let none = absent_from(false, false, false);
        assert_eq!(none.len(), SET.len());
        assert_eq!(
            none.iter().map(|spec| spec.bytes).sum::<u64>(),
            set_bytes()
        );

        // One half without the other: the four read a page, the other two read
        // its shape, and either can be here alone.
        let no_structure = absent_from(true, false, true);
        assert_eq!(no_structure.len(), 1);
        assert_eq!(no_structure[0].id, STRUCTURE.id);

        let no_recognition = absent_from(false, true, true);
        assert_eq!(no_recognition.len(), RECOGNITION_SET.len());
    }
}
