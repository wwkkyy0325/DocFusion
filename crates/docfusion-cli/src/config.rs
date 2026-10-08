//! Where the tool is set up, when no flag says otherwise.

use std::env;
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use docfusion_store::dirs;
use serde::Deserialize;

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
    /// One of `off`, `doubt`, `fields` or `always`.
    pub verify: Option<String>,
    pub max_verify_pages: Option<u32>,
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
            "#,
        )
        .expect("valid");

        assert_eq!(config.endpoint.as_deref(), Some("https://api.deepseek.com"));
        assert_eq!(config.model.as_deref(), Some("deepseek-flash"));
        assert_eq!(config.api_key.as_deref(), Some("sk-example"));
        assert_eq!(config.verify.as_deref(), Some("fields"));
        assert_eq!(config.max_verify_pages, Some(7));
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
}
