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
//! space guards against a risk that markdown output — kilobytes per document,
//! against a 6.9 MB one-time extraction — does not pose, and the WebView2
//! runtime is for a GUI that does not exist yet. Each belongs with the thing
//! that needs it rather than here.

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

pub fn run(cli: &Cli, reporter: &Reporter) -> Result<u8, CliError> {
    let root = cli.root();
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
                serves = models;
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

    let tier = match &endpoint {
        Some(_) if problems.is_empty() => VISION,
        _ => OCR_ONLY,
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
            "Documents with a text layer, and every scan, are read by the recognition \
             engine bundled in the binary. That needs no endpoint and no setup."
                .to_owned(),
        );
        lines.push(match &endpoint {
            Some(_) => format!(
                "The endpoint comes from {}. Fix it there, or override it for one run \
                 with --endpoint and --model.",
                crate::config::path().display()
            ),
            None => format!(
                "To read pages with a vision model as well, put an endpoint in {}. \
                 See the setup guide.",
                crate::config::path().display()
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
            "config": crate::config::path().display().to_string(),
        }),
        lines.join("\n"),
    );

    // A backend that was configured and could not be confirmed is worth a
    // non-zero exit: the point of asking is to find out, and an answer that
    // always says "fine" is not an answer. No endpoint at all is a supported
    // configuration rather than a problem — the bundled engine still reads
    // everything it can.
    Ok(if problems.is_empty() { OK } else { FAILED })
}
