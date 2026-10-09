//! Where the tool is set up, when nothing says otherwise.
//!
//! Read here rather than in the command line because the window reads it too,
//! and one file with two readers needs one definition. The settings are the
//! same whatever is asking: they describe the tool, not the interface.

use std::env;
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::dirs;

/// The settings a run can inherit from a file.
///
/// Only what would otherwise be retyped on every invocation, and deliberately
/// not everything: `--json` and `--dry-run` describe what *this run* should do
/// rather than how the tool is set up, and a file that quietly made every run a
/// dry run would be a trap rather than a convenience.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub root: Option<PathBuf>,
    pub endpoint: Option<String>,
    pub model: Option<String>,
    /// A secret. The file it lives in is per user and outside any repository,
    /// because a key beside the documents is a key that gets committed.
    pub api_key: Option<String>,
    /// Whether to let the model think before it answers.
    ///
    /// Absent means no, and no means the request asks for thinking to be turned
    /// off — the provider's default is on, and it dominates the output tokens.
    /// Set it to `true` for a provider that rejects a field it does not know,
    /// since the request is what carries the setting.
    pub reasoning: Option<bool>,
    /// One of `off`, `doubt`, `fields` or `always`.
    pub verify: Option<String>,
    pub max_verify_pages: Option<u32>,
    /// How many pages may be read at once. Each costs about 250 MB.
    pub max_ocr_pipelines: Option<usize>,
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("{} cannot be read: {source}", path.display())]
    Unreadable {
        path: PathBuf,
        source: std::io::Error,
    },

    #[error("{} is not valid:\n{message}", path.display())]
    Malformed { path: PathBuf, message: String },

    #[error("{} could not be written: {source}", path.display())]
    Unwritable {
        path: PathBuf,
        source: std::io::Error,
    },
}

impl Config {
    /// Reads the configuration file, if there is one.
    ///
    /// A file that is not there is not a failure — the tool works with no
    /// configuration at all, which is the point of shipping the recognition
    /// engine — but one that is there and cannot be understood is: a misspelt
    /// key or a value of the wrong type, silently ignored, means running
    /// against the wrong endpoint with nothing to say why.
    pub fn load() -> Result<Self, ConfigError> {
        Self::from_path(&path())
    }

    pub fn from_path(path: &Path) -> Result<Self, ConfigError> {
        let text = match fs::read_to_string(path) {
            Ok(text) => text,
            Err(error) if error.kind() == ErrorKind::NotFound => return Ok(Self::default()),
            Err(source) => {
                return Err(ConfigError::Unreadable {
                    path: path.to_owned(),
                    source,
                });
            }
        };

        toml::from_str(&text).map_err(|error| ConfigError::Malformed {
            path: path.to_owned(),
            message: error.to_string(),
        })
    }

    /// Writes the model settings into the file, leaving everything else in it
    /// as it was.
    ///
    /// Edited rather than rewritten, because most of that file is the comments
    /// explaining the settings in it. A window that deleted them on save would
    /// be one nobody dares use twice, and the loss would be silent.
    ///
    /// An empty `api_key` means the key stays as it is rather than being
    /// removed. A key is never read back into a window, so there is nothing a
    /// form could show — which makes an empty box mean "I did not touch this",
    /// not "delete it". Turning the model off is done by clearing the endpoint,
    /// after which the key does nothing.
    ///
    /// Empty strings are removed rather than written as empty ones: every
    /// reader of this file treats a missing setting as "not configured", and
    /// `endpoint = ""` is not the same thing to a URL parser as no endpoint.
    pub fn save_model(
        path: &Path,
        endpoint: &str,
        model: &str,
        api_key: &str,
        reasoning: bool,
    ) -> Result<(), ConfigError> {
        let text = match fs::read_to_string(path) {
            Ok(text) => text,
            Err(error) if error.kind() == ErrorKind::NotFound => String::new(),
            Err(source) => {
                return Err(ConfigError::Unreadable {
                    path: path.to_owned(),
                    source,
                });
            }
        };

        let mut document = text
            .parse::<toml_edit::DocumentMut>()
            .map_err(|error| ConfigError::Malformed {
                path: path.to_owned(),
                message: error.to_string(),
            })?;

        set(&mut document, "endpoint", endpoint);
        set(&mut document, "model", model);
        if !api_key.trim().is_empty() {
            set(&mut document, "api_key", api_key);
        }

        // Written either way, because a flag has no blank value that could mean
        // "leave it alone": not writing it would leave the opposite of what was
        // just chosen in a file somebody had set it in before.
        place(&mut document, "reasoning", toml_edit::value(reasoning));

        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|source| ConfigError::Unwritable {
                path: path.to_owned(),
                source,
            })?;
        }

        crate::atomic::write_atomic(path, document.to_string().as_bytes()).map_err(|source| {
            ConfigError::Unwritable {
                path: path.to_owned(),
                source,
            }
        })
    }
}

/// Sets a top-level key to a string, or removes it when the value is blank.
fn set(document: &mut toml_edit::DocumentMut, key: &str, text: &str) {
    let text = text.trim();

    if text.is_empty() {
        document.remove(key);
        return;
    }

    place(document, key, toml_edit::value(text));
}

/// Puts a value at a top-level key, keeping the trivia that was around whatever
/// was there before.
///
/// Assigning a value replaces the whitespace and the comments attached to it as
/// well, so without this a note somebody wrote on that line — `# the old one` —
/// would go with it, silently, which is the whole thing editing rather than
/// rewriting was meant to avoid.
fn place(document: &mut toml_edit::DocumentMut, key: &str, mut item: toml_edit::Item) {
    let existing = document
        .get(key)
        .and_then(toml_edit::Item::as_value)
        .map(|value| (value.decor().prefix().cloned(), value.decor().suffix().cloned()));

    if let (Some((prefix, suffix)), Some(value)) = (existing, item.as_value_mut()) {
        if let Some(prefix) = prefix {
            value.decor_mut().set_prefix(prefix);
        }
        if let Some(suffix) = suffix {
            value.decor_mut().set_suffix(suffix);
        }
    }

    document[key] = item;
}

/// Where the configuration file lives.
///
/// Per user, and outside any repository. `DOCFUSION_CONFIG` overrides it, which
/// is what a test or a scripted run wants and what nothing else should need.
pub fn path() -> PathBuf {
    match env::var_os("DOCFUSION_CONFIG") {
        Some(explicit) => PathBuf::from(explicit),
        None => dirs::user_dir().join("config.toml"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn write(contents: &str) -> tempfile::TempDir {
        let directory = tempfile::tempdir().expect("temp dir");
        let mut file = fs::File::create(directory.path().join("config.toml")).expect("create");
        file.write_all(contents.as_bytes()).expect("write");
        directory
    }

    fn load(contents: &str) -> Result<Config, ConfigError> {
        let directory = write(contents);
        Config::from_path(&directory.path().join("config.toml"))
    }

    #[test]
    fn a_missing_file_is_not_a_failure() {
        let config = Config::from_path(Path::new("no-such-directory/config.toml"))
            .expect("a tool with no configuration still runs");

        assert!(config.endpoint.is_none());
        assert!(config.api_key.is_none());
    }

    #[test]
    fn the_settings_are_read() {
        let config = load(
            r#"
            endpoint = "https://api.deepseek.com"
            model = "deepseek-flash"
            api_key = "sk-example"
            verify = "fields"
            max_verify_pages = 7
            max_ocr_pipelines = 2
            "#,
        )
        .expect("valid");

        assert_eq!(config.endpoint.as_deref(), Some("https://api.deepseek.com"));
        assert_eq!(config.model.as_deref(), Some("deepseek-flash"));
        assert_eq!(config.api_key.as_deref(), Some("sk-example"));
        assert_eq!(config.verify.as_deref(), Some("fields"));
        assert_eq!(config.max_verify_pages, Some(7));
        assert_eq!(config.max_ocr_pipelines, Some(2));
    }

    /// The failure that has to be loud. A misspelt key silently ignored means
    /// running against the wrong endpoint with nothing to say why, which is
    /// exactly the kind of quiet wrong answer the tool exists to avoid.
    #[test]
    fn a_misspelt_setting_is_an_error_rather_than_a_shrug() {
        let error = load("api_ky = \"sk-example\"").expect_err("should not be ignored");

        let message = error.to_string();
        assert!(message.contains("config.toml"), "{message}");
        assert!(message.contains("api_ky"), "{message}");
    }

    #[test]
    fn a_value_of_the_wrong_type_is_an_error() {
        let error = load("max_verify_pages = \"lots\"").expect_err("should not be ignored");
        assert!(error.to_string().contains("max_verify_pages"), "{error}");
    }

    #[test]
    fn an_empty_file_leaves_every_setting_to_its_default() {
        let config = load("").expect("valid");

        assert!(config.root.is_none());
        assert!(config.endpoint.is_none());
    }

    /// Comments and blank lines are the point of a config file a person edits.
    #[test]
    fn comments_are_allowed() {
        let config = load(
            "# what the endpoint is for\n\
             model = \"deepseek-flash\"  # trailing\n\
             \n",
        )
        .expect("valid");

        assert_eq!(config.model.as_deref(), Some("deepseek-flash"));
    }

    /// The reason saving edits rather than rewrites. A config file a person
    /// keeps is mostly the comments explaining it, and a save that deleted them
    /// would delete them silently — the worst way to lose something.
    ///
    /// The shape here is the shape the file actually has: a block of comments
    /// standing over each setting, a note written at the end of a line, a
    /// setting that is not ours to touch, and one that is commented out.
    #[test]
    fn saving_keeps_the_comments_and_everything_else_in_the_file() {
        let directory = write(
            "# DocFusion settings.\n\
             \n\
             # Which endpoint to read scanned pages with.\n\
             endpoint = \"http://old\"   # the one at work\n\
             \n\
             # Which model that endpoint should serve.\n\
             model = \"old-model\"\n\
             \n\
             # The key for that endpoint, if it needs one.\n\
             api_key = \"sk-old\"\n\
             \n\
             # ── Everything below is optional ─────────────\n\
             \n\
             # Default: doubt\n\
             # verify = \"doubt\"\n\
             verify = \"always\"\n",
        );
        let path = directory.path().join("config.toml");

        Config::save_model(&path, "https://api.deepseek.com", "deepseek-flash", "sk-new", false)
            .expect("saved");

        let text = fs::read_to_string(&path).expect("read back");
        for kept in [
            "# DocFusion settings.",
            "# Which endpoint to read scanned pages with.",
            "# the one at work",
            "# Which model that endpoint should serve.",
            "# The key for that endpoint, if it needs one.",
            "# ── Everything below is optional ─────────────",
            "# Default: doubt",
            "# verify = \"doubt\"",
            "verify = \"always\"",
        ] {
            assert!(text.contains(kept), "{kept} was lost:\n{text}");
        }

        let config = Config::from_path(&path).expect("valid");
        assert_eq!(config.endpoint.as_deref(), Some("https://api.deepseek.com"));
        assert_eq!(config.model.as_deref(), Some("deepseek-flash"));
        assert_eq!(config.api_key.as_deref(), Some("sk-new"));
        assert_eq!(config.verify.as_deref(), Some("always"));
    }

    /// A key goes one way — typed in, written out. Nothing reads it back, so an
    /// empty box means the person did not touch it, not that they want it gone.
    #[test]
    fn an_empty_key_leaves_the_key_alone() {
        let directory = write("api_key = \"sk-existing\"\n");
        let path = directory.path().join("config.toml");

        Config::save_model(&path, "http://localhost:8080", "local", "", false).expect("saved");

        let config = Config::from_path(&path).expect("valid");
        assert_eq!(config.api_key.as_deref(), Some("sk-existing"));
        assert_eq!(config.endpoint.as_deref(), Some("http://localhost:8080"));
    }

    /// Clearing the endpoint is how the model is switched off. Removing it
    /// rather than emptying it, because `endpoint = ""` is not what "no
    /// endpoint" looks like to the code that reads this.
    #[test]
    fn an_empty_endpoint_is_removed_rather_than_emptied() {
        let directory = write("endpoint = \"http://old\"\nmodel = \"old\"\n");
        let path = directory.path().join("config.toml");

        Config::save_model(&path, "  ", "", "", false).expect("saved");

        let config = Config::from_path(&path).expect("valid");
        assert!(config.endpoint.is_none());
        assert!(config.model.is_none());
    }

    /// Saving is how the file comes to exist on a machine that has never run
    /// the tool, so it makes the directory on the way.
    #[test]
    fn saving_creates_the_file_and_the_directory_it_goes_in() {
        let directory = tempfile::tempdir().expect("temp dir");
        let path = directory.path().join("nested").join("config.toml");

        Config::save_model(&path, "https://api.deepseek.com", "deepseek-flash", "sk-1", false)
            .expect("saved");

        let config = Config::from_path(&path).expect("valid");
        assert_eq!(config.model.as_deref(), Some("deepseek-flash"));
    }
}
