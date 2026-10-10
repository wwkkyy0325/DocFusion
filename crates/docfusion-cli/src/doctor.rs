//! What this installation can read, and what is missing to read more.
//!
//! The question "can this machine run a model" is answered by asking the
//! endpoint rather than by inspecting the machine. GPU vendor, VRAM, whether a
//! daemon is running and which models it holds are all questions about somebody
//! else's program, and that program answers them better than any inspection we
//! could write. What was an environment-detection subsystem is one request and
//! a message.
//!
//! Two things no endpoint can report are deliberately not checked. Free disk
//! space guards against a risk that markdown output — kilobytes per document —
//! does not pose, and the WebView2 runtime is for a GUI that does not exist
//! yet. Each belongs with the thing that needs it rather than here.
//!
//! The models *are* reported here, in one line, because "can this machine read
//! my scan" is the question this command exists to answer and the answer is
//! about a download.

use docfusion_store::endpoint::Probe;
use serde_json::json;

use crate::output::Reporter;
use crate::{Cli, CliError, FAILED, OK, endpoint, engine_ids};

/// What the tool can be relied on to read, given what could be confirmed.
///
/// A configured endpoint that could not be confirmed reports `ocr-only`, not
/// `vision`. The endpoint may well work — the listing is not required of it —
/// but "vision" would be a promise the run just failed to verify, and the
/// problem line says so either way.
const OCR_ONLY: &str = "ocr-only";
const VISION: &str = "vision";
const TEXT_ONLY: &str = "text-only";

/// What the set is for, said in the report rather than assumed.
const READS: &str = "the words on a page, and the shape of it";

/// The line about the set, fetching it first when asked to.
///
/// One line for one set, which is what it is now. It used to be two lines and
/// a third of the models, because the recognition weights were in the binary
/// and only the other two were fetched; a report that split them was describing
/// where the files came from rather than what the machine can do.
///
/// The one thing in this report that can be acted on here rather than
/// described: a quarter of a gigabyte is a decision, and `--fetch` is how
/// somebody makes it.
fn set_line(root: &std::path::Path, fetch: bool) -> Result<String, CliError> {
    use docfusion_store::models::{Outcome, SET, State};

    let total = docfusion_store::models::set_bytes();

    // Rounded up, like the window rounds it: this is a warning about a
    // download, and the one answer worse than an overestimate is "0 MB".
    let megabytes = |bytes: u64| bytes.div_ceil(1024 * 1024);

    if !fetch {
        let absent = docfusion_store::models::absent(SET, root);

        if absent.is_empty() {
            return Ok(format!(
                "models          present, {} MB — {READS}",
                megabytes(total)
            ));
        }

        let missing = docfusion_store::models::still_to_fetch(&absent, root);

        // Whole or in part, because a resumed download is a real state and
        // "not downloaded" would be wrong about the part that is here.
        let how = if missing == total {
            format!("not downloaded, {} MB", megabytes(total))
        } else {
            format!(
                "{} MB of {} MB not downloaded",
                megabytes(missing),
                megabytes(total)
            )
        };

        return Ok(format!(
            "models          {how}; `docfusion doctor --fetch` gets the set"
        ));
    }

    // One bar for the whole set rather than one per file: six files that each
    // ran 0 to 100 would look like six downloads, and the answer to "how long
    // is this going to take" is the set.
    //
    // Progress on stderr, redrawn in place. It reports every four megabytes,
    // which is about thirty lines for the layout model — thirty appended lines
    // would bury everything else the command said.
    let cancelled = std::sync::atomic::AtomicBool::new(false);
    let mut landed = 0;
    let mut stopped = false;

    for spec in SET {
        if docfusion_store::models::present_spec(spec, root) {
            landed += spec.bytes;
            continue;
        }

        let base = landed;
        let outcome = docfusion_store::models::ensure_of(
            spec,
            root,
            &mut |state| match state {
                State::Fetching { received, .. } => {
                    let at = base.saturating_add(*received).min(total);
                    eprint!(
                        "\r  fetching {} MB… {}%    ",
                        megabytes(total),
                        at * 100 / total.max(1)
                    );
                }
                State::Verifying => eprint!("\r  checking the digests…  "),
                _ => {}
            },
            &cancelled,
        );

        match outcome {
            Ok(Outcome::Ready(_)) => landed += spec.bytes,
            Ok(Outcome::Cancelled) => {
                stopped = true;
                break;
            }
            Err(error) => {
                eprintln!();
                return Err(CliError::Usage(error.to_string()));
            }
        }
    }

    eprintln!();

    if stopped {
        return Ok("models          stopped, and nothing is wrong".to_owned());
    }

    Ok(format!(
        "models          present, {} MB — {READS}",
        megabytes(total)
    ))
}

pub fn run(cli: &Cli, reporter: &Reporter) -> Result<u8, CliError> {
    let root = cli.root();
    let model_root = docfusion_store::models::default_root();
    let engines = engine_ids(cli)?;
    let endpoint = endpoint(cli)?;

    let mut problems: Vec<String> = Vec::new();
    let mut notes: Vec<String> = Vec::new();
    let mut serves: Vec<String> = Vec::new();
    let mut listed = false;

    if let Some(endpoint) = &endpoint {
        let wanted = &endpoint.config().model;

        match endpoint.probe() {
            Ok(Probe::Models(models)) => {
                // Asked before anything else, because it is the failure that
                // costs a document to discover: a model that says it cannot be
                // shown an image will answer every scan with fluent text about
                // nothing, and answer it confidently.
                if let Some(model) = models.iter().find(|model| model.id == *wanted)
                    && !model.reads_images()
                {
                    problems.push(format!(
                        "`{wanted}` does not accept images, so it cannot read a scanned page"
                    ));
                }

                serves = models.into_iter().map(|model| model.id).collect();
                listed = serves.iter().any(|served| served == wanted);
                if !listed {
                    problems.push(format!(
                        "`{wanted}` is not in the endpoint's list of models; it serves {}",
                        if serves.is_empty() {
                            "nothing".to_owned()
                        } else {
                            serves.join(", ")
                        }
                    ));
                }
            }
            // Nothing to act on. The endpoint answered; it simply does not offer
            // a listing, which nothing requires of it. Counted as confirmed,
            // because the alternative is crying wolf at a working setup.
            Ok(Probe::Unlisted(note)) => {
                listed = true;
                notes.push(note);
            }
            // It did not answer at all, which is the case worth failing on.
            Err(error) => problems.push(error.to_string()),
        }
    }

    // The models first, because `--fetch` changes what every question below
    // answers — and asking before the fetch and printing after it is how a
    // report comes out saying "present, 138 MB" on one line and "text-only" on
    // the next.
    let models_line = set_line(&model_root, cli.fetch)?;
    docfusion_store::forget_models();

    // Three answers rather than two, because there are three things a machine
    // can be now. `ocr-only` used to describe every installation, since the
    // recognition weights were in the binary; it is a claim about a download
    // today, and one an unpatched machine does not get to make.
    let tier = match (
        endpoint.is_some() && problems.is_empty(),
        docfusion_store::ocr::available(),
    ) {
        (true, _) => VISION,
        (false, true) => OCR_ONLY,
        (false, false) => TEXT_ONLY,
    };

    let memory = docfusion_store::memory::total();
    let readers = crate::readers(cli);

    let mut lines = vec![
        format!(
            "knowledge base  {}{}",
            root.display(),
            if root.is_dir() { "" } else { "  (not created yet)" }
        ),
        format!("engines         {}", engines.join(", ")),
        format!(
            "memory          {}",
            match memory {
                Some(bytes) => format!(
                    "{:.1} GB, so {readers} page{} at once",
                    bytes as f64 / (1024.0 * 1024.0 * 1024.0),
                    if readers == 1 { "" } else { "s" }
                ),
                None => format!("unknown, so {readers} page(s) at once"),
            }
        ),
    ];

    match &endpoint {
        Some(endpoint) => {
            lines.push(format!("endpoint        {}", endpoint.config().base_url));
            if !serves.is_empty() {
                lines.push(format!("  serves        {}", serves.join(", ")));
            }
            lines.push(format!(
                "  selected      {}{}",
                endpoint.config().model,
                if listed { "  (listed)" } else { "" }
            ));
        }
        None => lines.push("endpoint        none configured".to_owned()),
    }

    // Reported whether or not it is here, because "it read your scan and found
    // no headings" and "it read your scan and finds headings" are different
    // answers, and only one of them is worth looking for a bug in.
    lines.push(models_line);

    lines.push(format!("capability      {tier}"));

    for note in &notes {
        lines.push(format!("note            {note}"));
    }
    for problem in &problems {
        lines.push(format!("problem         {problem}"));
    }

    // What to do about it, which is the half of a health report usually left
    // out, and the half that decides whether it was worth running.
    if endpoint.is_none() || !problems.is_empty() {
        lines.push(String::new());
        lines.push(
            "Documents with a text layer are read with nothing configured and nothing \
             downloaded. Every scan is read by the recognition engine on this machine, \
             which needs no endpoint — and needs the models above."
                .to_owned(),
        );
        lines.push(match &endpoint {
            Some(_) => format!(
                "The endpoint comes from {}. Fix it there, or override it for one run \
                 with --endpoint and --model.",
                docfusion_store::config::path().display()
            ),
            None => format!(
                "To read pages with a vision model as well, put an endpoint in {}. \
                 See the setup guide.",
                docfusion_store::config::path().display()
            ),
        });
    }

    reporter.record(
        json!({
            "command": "doctor",
            "root": root.display().to_string(),
            "root_exists": root.is_dir(),
            "engines": engines,
            "memory_bytes": memory,
            "readers": readers,
            "endpoint": endpoint.as_ref().map(|e| e.config().base_url.clone()),
            "model": endpoint.as_ref().map(|e| e.config().model.clone()),
            "serves": serves,
            "model_listed": listed,
            "tier": tier,
            "notes": notes,
            "problems": problems,
            "config": docfusion_store::config::path().display().to_string(),
        }),
        lines.join("\n"),
    );

    // A backend that was configured and could not be confirmed is worth a
    // non-zero exit: the point of asking is to find out, and an answer that
    // always says "fine" is not an answer. No endpoint at all is a supported
    // configuration rather than a problem, and so is a machine whose models
    // have not been fetched yet — the models line says so, and a report that
    // failed on it would make the first run of a fresh install an error.
    Ok(if problems.is_empty() { OK } else { FAILED })
}
