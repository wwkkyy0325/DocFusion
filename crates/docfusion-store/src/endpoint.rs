//! A vision backend that speaks to an OpenAI-compatible endpoint.
//!
//! `llama-server`, Ollama, LM Studio, vLLM and the hosted providers all expose
//! the same `/v1/chat/completions` shape, so one client covers every one of
//! them. That is what lets the tool stop running models: the choice of runtime
//! stops being ours to make and becomes a configuration value.
//!
//! Lives here rather than in the core crate because it performs network I/O,
//! and core is pure.

use std::time::Duration;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use docfusion_core::backend::{BackendError, Caps, PageRequest, VisionBackend};
use docfusion_core::extract::text::markdown_blocks;
use docfusion_core::ir::{Block, EngineId};
use serde::Deserialize;
use serde_json::json;

const ENGINE_ID: &str = "openai-compatible";
const ENGINE_VERSION: &str = "1.0.0";
const PROMPT_VERSION: &str = "v1";

/// The instruction sent with every page.
///
/// Deliberately plain and short. Document-tuned models are trained against a
/// particular prompt shape and do worse when it is dressed up, and a long
/// instruction costs tokens on every page for a benefit that has not been
/// measured.
pub const DEFAULT_PROMPT: &str =
    "Transcribe this page as GitHub-flavoured Markdown. Preserve the reading \
     order, heading levels, lists and tables. Output only the Markdown.";

/// How long a single page may take.
///
/// Generous, because a small model on a CPU is slow and a timeout that fires
/// during a legitimate inference is worse than waiting. It is not unlimited:
/// an endpoint that accepts a connection and then says nothing would otherwise
/// freeze a whole batch.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(180);

/// A prompt token count below this, for a page image, means the image never
/// became visible to the model.
///
/// A page rendered at 200 DPI is well over a million pixels; even a coarse
/// vision encoder turns that into hundreds of tokens. A handful of tokens means
/// the image was dropped — which is what a GGUF model served without its
/// multimodal projector does, answering 200 with repeated garbage rather than
/// failing. Measured rather than derived, so it is a heuristic and the message
/// says so.
const MIN_IMAGE_TOKENS: u64 = 32;

#[derive(Debug, Clone)]
pub struct EndpointConfig {
    /// Everything up to `/v1`, such as `http://127.0.0.1:8080`.
    pub base_url: String,
    pub model: String,
    pub api_key: Option<String>,
    pub prompt: String,
    pub timeout: Duration,
}

impl EndpointConfig {
    pub fn new(base_url: impl Into<String>, model: impl Into<String>) -> Self {
        Self {
            base_url: base_url.into(),
            model: model.into(),
            api_key: None,
            prompt: DEFAULT_PROMPT.to_owned(),
            timeout: DEFAULT_TIMEOUT,
        }
    }

    pub fn with_api_key(mut self, key: impl Into<String>) -> Self {
        self.api_key = Some(key.into());
        self
    }

    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    pub fn with_prompt(mut self, prompt: impl Into<String>) -> Self {
        self.prompt = prompt.into();
        self
    }
}

/// What asking the endpoint what it serves turned up.
///
/// Two outcomes rather than a list, because "it does not offer a listing" and
/// "it does not answer" are different findings with different responses: one is
/// nothing to act on, the other needs looking at.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Probe {
    /// The models the endpoint says it is serving.
    Models(Vec<String>),
    /// The endpoint answered, and does not list its models.
    Unlisted(String),
}

pub struct OpenAiEndpoint {
    config: EndpointConfig,
    agent: ureq::Agent,
}

impl OpenAiEndpoint {
    pub fn new(config: EndpointConfig) -> Self {
        let agent = ureq::Agent::new_with_config(
            ureq::Agent::config_builder()
                .timeout_global(Some(config.timeout))
                .build(),
        );

        Self { config, agent }
    }

    pub fn config(&self) -> &EndpointConfig {
        &self.config
    }

    /// Asks the endpoint which models it is serving.
    ///
    /// Probing replaces detecting. Whether a backend is usable is a question
    /// about somebody else's program, and that program answers it better than
    /// any inspection of the machine we could write.
    pub fn probe(&self) -> Result<Probe, BackendError> {
        let mut request = self.agent.get(&self.url("/v1/models"));
        if let Some(key) = &self.config.api_key {
            request = request.header("Authorization", format!("Bearer {key}"));
        }

        let mut response = match request.call() {
            Ok(response) => response,
            // An endpoint that does not implement the listing is not broken,
            // and nothing requires it to implement one. Calling that a fault
            // would send somebody to repair something that reads pages
            // perfectly well — which is why this is separated from the
            // connection failures below rather than folded in with them.
            Err(ureq::Error::StatusCode(code)) if matches!(code, 404 | 405 | 501) => {
                return Ok(Probe::Unlisted(format!(
                    "{} answered {code} when asked to list its models",
                    self.config.base_url
                )));
            }
            Err(error) => return Err(self.describe(error, "list its models")),
        };

        let listing: ModelList = response
            .body_mut()
            .read_json()
            .map_err(|error| self.unreadable(error))?;

        Ok(Probe::Models(
            listing.data.into_iter().map(|model| model.id).collect(),
        ))
    }

    fn url(&self, path: &str) -> String {
        format!("{}{path}", self.config.base_url.trim_end_matches('/'))
    }

    /// Turns a transport error into something a person can act on.
    fn describe(&self, error: ureq::Error, doing: &str) -> BackendError {
        let base = &self.config.base_url;

        match error {
            ureq::Error::StatusCode(code) => BackendError::Failed(format!(
                "the endpoint at {base} answered {code} when asked to {doing}"
            )),
            ureq::Error::Timeout(_) => BackendError::Failed(format!(
                "the endpoint at {base} did not answer within {:?} when asked to {doing}; \
                 a model that is too large for the machine, or one that is still loading, \
                 looks exactly like this",
                self.config.timeout
            )),
            ureq::Error::HostNotFound => BackendError::Unavailable(format!(
                "{base} does not resolve; check the address in the configuration"
            )),
            other => BackendError::Unavailable(format!(
                "could not reach {base}: {other}. If nothing is listening there, see the \
                 setup guide for how to start one"
            )),
        }
    }

    fn unreadable(&self, error: ureq::Error) -> BackendError {
        BackendError::Failed(format!(
            "the endpoint at {} returned something that is not an OpenAI-compatible \
             response: {error}",
            self.config.base_url
        ))
    }
}

impl VisionBackend for OpenAiEndpoint {
    fn id(&self) -> EngineId {
        EngineId::from(ENGINE_ID)
    }

    fn version(&self) -> &str {
        ENGINE_VERSION
    }

    fn prompt_version(&self) -> &str {
        PROMPT_VERSION
    }

    /// Neither, and that is the point.
    ///
    /// A general vision model returns Markdown with no geometry and no
    /// confidence, so verification degrades to comparing text. Declaring
    /// otherwise would let the verifier align on boxes that do not exist.
    fn capabilities(&self) -> Caps {
        Caps::default()
    }

    fn parse(&self, request: &PageRequest<'_>) -> Result<Vec<Block>, BackendError> {
        let png = request
            .image
            .to_png()
            .map_err(|error| BackendError::Failed(error.to_string()))?;
        let data_url = format!("data:image/png;base64,{}", BASE64.encode(&png));

        let body = json!({
            "model": self.config.model,
            // Recognition is not a creative act, and a model that paraphrases
            // is worse than one that does not.
            "temperature": 0,
            "messages": [{
                "role": "user",
                "content": [
                    { "type": "text", "text": self.config.prompt },
                    { "type": "image_url", "image_url": { "url": data_url } },
                ],
            }],
        });

        let mut call = self
            .agent
            .post(&self.url("/v1/chat/completions"))
            .header("Content-Type", "application/json");
        if let Some(key) = &self.config.api_key {
            call = call.header("Authorization", format!("Bearer {key}"));
        }

        let mut response = call
            .send_json(&body)
            .map_err(|error| self.describe(error, "read a page"))?;

        let completion: Completion = response
            .body_mut()
            .read_json()
            .map_err(|error| self.unreadable(error))?;

        self.to_blocks(completion, request.page)
    }
}

impl OpenAiEndpoint {
    fn to_blocks(&self, completion: Completion, page: u32) -> Result<Vec<Block>, BackendError> {
        let Completion { choices, usage } = completion;

        let text = choices
            .into_iter()
            .next()
            .and_then(|choice| choice.message.content)
            .unwrap_or_default();
        let markdown = text.trim();

        if markdown.is_empty() {
            return Err(BackendError::Failed(format!(
                "the endpoint at {} returned no text for page {page}",
                self.config.base_url
            )));
        }

        // Only checked when the endpoint reports usage: a missing field is not
        // evidence of a problem, and failing on it would break endpoints that
        // simply do not report tokens.
        if let Some(tokens) = usage.and_then(|usage| usage.prompt_tokens)
            && tokens < MIN_IMAGE_TOKENS
        {
            return Err(BackendError::Failed(format!(
                "the endpoint reported only {tokens} prompt tokens for a page image, so the \
                 image never reached the model. A GGUF model served without its multimodal \
                 projector does this, and answers with repeated nonsense rather than an error. \
                 This is a heuristic, but the count is far below what a page can produce."
            )));
        }

        Ok(markdown_blocks(strip_envelope(markdown), &self.id()))
    }
}

/// Removes the packaging a chat model sometimes wraps its answer in.
///
/// Asked to output only Markdown, a model may answer "Here is the transcription
/// of the page in GitHub-flavoured Markdown." and put the page inside a fenced
/// block anyway. The fence is punctuation around the answer, not part of it:
/// left in place, the whole page becomes one verbatim code block and every
/// heading, list and table the prompt asked for is lost.
///
/// Deliberately narrow. It unwraps only when the whole answer reduces to a
/// single fenced block — possibly introduced by one sentence — because that is
/// the shape being defended against. A page that genuinely contains a code
/// listing has prose around it, and prose means no unwrapping.
fn strip_envelope(markdown: &str) -> &str {
    let text = markdown.trim();

    if let Some(inner) = single_fenced_block(text) {
        return inner;
    }

    if let Some((_, rest)) = text.split_once("\n\n")
        && let Some(inner) = single_fenced_block(rest.trim())
    {
        return inner;
    }

    text
}

/// The contents of `text`, when `text` is exactly one fenced block.
fn single_fenced_block(text: &str) -> Option<&str> {
    let first_line = text.find('\n')?;
    let opening = &text[..first_line];

    let fence = opening.chars().next()?;
    if fence != '`' && fence != '~' {
        return None;
    }

    let length = opening.chars().take_while(|character| *character == fence).count();
    if length < 3 {
        return None;
    }
    // An info string may follow the fence, but not another run of it: a fence
    // containing its own delimiter is not an opening fence.
    if opening[length..].contains(fence) {
        return None;
    }

    let body = &text[first_line + 1..];
    let closing = format!("\n{}", fence.to_string().repeat(length));
    let end = body.rfind(&closing)?;

    if !body[end + closing.len()..].trim().is_empty() {
        return None;
    }

    Some(body[..end].trim_matches('\n'))
}

#[derive(Debug, Deserialize)]
struct Completion {
    #[serde(default)]
    choices: Vec<Choice>,
    #[serde(default)]
    usage: Option<Usage>,
}

#[derive(Debug, Deserialize)]
struct Choice {
    message: Message,
}

#[derive(Debug, Deserialize)]
struct Message {
    #[serde(default)]
    content: Option<String>,
}

#[derive(Debug, Deserialize)]
struct Usage {
    #[serde(default)]
    prompt_tokens: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct ModelList {
    #[serde(default)]
    data: Vec<Model>,
}

#[derive(Debug, Deserialize)]
struct Model {
    id: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use docfusion_core::image::PageImage;
    use docfusion_core::ir::BlockKind;
    use std::io::{BufRead, BufReader, Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::thread;

    /// A real HTTP server on an ephemeral port.
    ///
    /// Real rather than a mocked transport: the client is the thing under test,
    /// and mocking below it would test the mock.
    fn serving(body: &str) -> String {
        let body = body.to_owned();
        spawn(move |mut stream| {
            let _ = read_request(&mut stream);
            let _ = stream.write_all(http_response(&body).as_bytes());
        })
    }

    /// Accepts connections and then says nothing, which is what a model that is
    /// still loading looks like from the outside.
    fn silent() -> String {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let address = format!("http://{}", listener.local_addr().expect("address"));

        thread::spawn(move || {
            let mut held = Vec::new();
            for stream in listener.incoming() {
                let Ok(stream) = stream else { break };
                held.push(stream);
            }
        });

        address
    }

    fn spawn(handler: impl Fn(TcpStream) + Send + 'static) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let address = format!("http://{}", listener.local_addr().expect("address"));

        thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(stream) = stream else { break };
                handler(stream);
            }
        });

        address
    }

    fn http_response(body: &str) -> String {
        format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n\
             Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )
    }

    /// Reads a request far enough that the client can finish sending it.
    fn read_request(stream: &mut TcpStream) -> std::io::Result<()> {
        let mut reader = BufReader::new(stream.try_clone()?);
        let mut content_length = 0usize;

        loop {
            let mut line = String::new();
            if reader.read_line(&mut line)? == 0 {
                return Ok(());
            }
            if let Some(value) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                content_length = value.trim().parse().unwrap_or(0);
            }
            if line == "\r\n" {
                break;
            }
        }

        let mut body = vec![0u8; content_length];
        reader.read_exact(&mut body)?;
        Ok(())
    }

    fn page() -> PageImage {
        PageImage {
            width: 2,
            height: 2,
            pixels: vec![255u8; 2 * 2 * 4],
        }
    }

    fn endpoint(address: &str) -> OpenAiEndpoint {
        OpenAiEndpoint::new(EndpointConfig::new(address, "test-model"))
    }

    fn parse(address: &str) -> Result<Vec<Block>, BackendError> {
        endpoint(address).parse(&PageRequest {
            image: &page(),
            page: 0,
        })
    }

    fn completion(content: &str) -> String {
        json!({
            "choices": [{ "message": { "content": content } }],
            "usage": { "prompt_tokens": 900 }
        })
        .to_string()
    }

    #[test]
    fn parses_a_completion_into_blocks() {
        let address = serving(&completion("# Title\n\nBody text."));

        let blocks = parse(&address).expect("parse");

        assert_eq!(blocks[0].kind, BlockKind::Heading { level: 1 });
        assert_eq!(blocks[0].text, "Title");
        assert_eq!(blocks[1].text, "Body text.");
    }

    /// A vision model answers in Markdown, and it has to land on the same block
    /// structure the other engines produce, or the rest of the pipeline would
    /// have to care which engine read a page.
    #[test]
    fn the_blocks_are_attributed_to_this_backend() {
        let address = serving(&completion("text"));

        let blocks = parse(&address).expect("parse");

        assert_eq!(blocks[0].engine.as_str(), ENGINE_ID);
    }

    /// A GGUF model served without its multimodal projector does not fail: the
    /// image never becomes vision tokens and the model returns nonsense with a
    /// 200 status.
    #[test]
    fn an_implausibly_low_token_count_is_treated_as_the_projector_failure() {
        let body = json!({
            "choices": [{ "message": { "content": "the the the the the" } }],
            "usage": { "prompt_tokens": 4 }
        })
        .to_string();
        let address = serving(&body);

        let error = parse(&address).expect_err("should refuse");

        assert!(error.to_string().contains("projector"), "{error}");
    }

    /// An endpoint that simply does not report usage is not evidence of a
    /// problem, and failing on a missing field would break working setups.
    #[test]
    fn a_missing_token_count_is_not_treated_as_a_failure() {
        let body = json!({ "choices": [{ "message": { "content": "fine" } }] }).to_string();
        let address = serving(&body);

        assert!(parse(&address).is_ok());
    }

    #[test]
    fn an_empty_completion_is_an_error() {
        let address = serving(&completion("   \n  "));

        let error = parse(&address).expect_err("should refuse");

        assert!(error.to_string().contains("returned no text"), "{error}");
    }

    #[test]
    fn a_response_with_no_choices_is_an_error() {
        let address = serving(r#"{"choices":[]}"#);

        assert!(parse(&address).is_err());
    }

    /// The failure the design names: an endpoint that accepts a connection and
    /// then says nothing must not freeze a batch.
    #[test]
    fn an_endpoint_that_never_answers_times_out() {
        let address = silent();
        let endpoint = OpenAiEndpoint::new(
            EndpointConfig::new(address, "test-model")
                .with_timeout(Duration::from_millis(300)),
        );

        let error = endpoint
            .parse(&PageRequest {
                image: &page(),
                page: 0,
            })
            .expect_err("should time out");

        assert!(error.to_string().contains("did not answer"), "{error}");
    }

    #[test]
    fn an_endpoint_that_is_not_listening_names_the_setup_guide() {
        // Bound and dropped, so the port is closed.
        let address = {
            let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
            format!("http://{}", listener.local_addr().expect("address"))
        };

        let error = parse(&address).expect_err("should refuse");

        assert!(matches!(error, BackendError::Unavailable(_)), "{error}");
        assert!(error.to_string().contains("setup guide"), "{error}");
    }

    #[test]
    fn a_rejected_request_reports_its_status() {
        let address = spawn(|mut stream| {
            let _ = read_request(&mut stream);
            let body = r#"{"error":"model not found"}"#;
            let _ = stream.write_all(
                format!(
                    "HTTP/1.1 404 Not Found\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                )
                .as_bytes(),
            );
        });

        let error = parse(&address).expect_err("should refuse");

        assert!(error.to_string().contains("404"), "{error}");
    }

    #[test]
    fn probing_lists_the_models_the_endpoint_serves() {
        let address = serving(r#"{"data":[{"id":"paddleocr-vl"},{"id":"glm-ocr"}]}"#);

        let probe = endpoint(&address).probe().expect("probe");

        assert_eq!(
            probe,
            Probe::Models(vec!["paddleocr-vl".to_owned(), "glm-ocr".to_owned()])
        );
    }

    /// Nothing requires an endpoint to implement the listing, and a server that
    /// reads pages perfectly well without one must not be reported as broken.
    #[test]
    fn an_endpoint_without_a_listing_is_not_a_fault() {
        let address = spawn(|mut stream| {
            let _ = read_request(&mut stream);
            let body = "not found";
            let _ = stream.write_all(
                format!(
                    "HTTP/1.1 404 Not Found\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                )
                .as_bytes(),
            );
        });

        assert!(matches!(
            endpoint(&address).probe().expect("probe"),
            Probe::Unlisted(_)
        ));
    }

    /// An endpoint that does not answer is the case worth failing on. A closed
    /// port rather than a stalled server, because the timeout is a hundred and
    /// eighty seconds and a test suite that waits for it is a test suite people
    /// stop running.
    #[test]
    fn an_endpoint_that_does_not_answer_is_a_failure() {
        let error = endpoint("http://127.0.0.1:1")
            .probe()
            .expect_err("nothing is listening there");

        assert!(
            error.to_string().contains("127.0.0.1:1"),
            "the message should name the endpoint: {error}"
        );
    }

    /// Editing a prompt changes the output while everything else stays the
    /// same, so the version has to reach the cache key.
    #[test]
    fn the_stamp_carries_the_prompt_version() {
        let stamp = endpoint("http://127.0.0.1:1").stamp();

        assert_eq!(stamp.id.as_str(), ENGINE_ID);
        assert_eq!(stamp.prompt_version.as_deref(), Some(PROMPT_VERSION));
    }

    /// A general vision model reports neither geometry nor confidence, and
    /// claiming otherwise would let the verifier align on boxes that do not
    /// exist.
    #[test]
    fn capabilities_claim_nothing() {
        let caps = endpoint("http://127.0.0.1:1").capabilities();

        assert!(!caps.bounding_boxes);
        assert!(!caps.confidence);
    }

    // ── The envelope a model wraps its answer in ─────────────────────────────
    //
    // Observed, not imagined: given a page and asked for Markdown, DeepSeek
    // answered "Here is the transcription of the page in GitHub-flavoured
    // Markdown." and put the page inside a fence. Left wrapped, the whole page
    // became one verbatim code block and every heading and table was lost.

    #[test]
    fn a_page_wrapped_in_a_fence_is_unwrapped() {
        let answer = "```markdown\n# Title\n\nBody.\n```";

        assert_eq!(strip_envelope(answer), "# Title\n\nBody.");
    }

    #[test]
    fn a_sentence_introducing_the_fence_goes_with_it() {
        let answer = "Here is the transcription of the page in GitHub-flavoured Markdown.\n\n\
                      ```\n# Title\n\nBody.\n```";

        assert_eq!(strip_envelope(answer), "# Title\n\nBody.");
    }

    #[test]
    fn tilde_fences_are_fences_too() {
        assert_eq!(strip_envelope("~~~\n# Title\n~~~"), "# Title");
    }

    /// The narrowness is the point. A page that really does contain a code
    /// listing has prose around it, and unwrapping would eat the page.
    #[test]
    fn a_code_block_inside_prose_is_left_alone() {
        let answer = "# Title\n\nSome prose.\n\n```\nlet x = 1;\n```\n\nMore prose.";

        assert_eq!(strip_envelope(answer), answer);
    }

    #[test]
    fn an_unclosed_fence_is_left_alone() {
        let answer = "```\n# Title\n\nBody.";

        assert_eq!(strip_envelope(answer), answer);
    }

    #[test]
    fn text_after_the_closing_fence_stops_the_unwrap() {
        let answer = "```\n# Title\n```\n\nAnd here is what I made of it.";

        assert_eq!(strip_envelope(answer), answer);
    }

    #[test]
    fn a_fence_longer_than_the_one_it_contains_survives() {
        // The inner ``` must not be mistaken for the end of the outer fence.
        let answer = "````\n```\ninner\n```\n````";

        assert_eq!(strip_envelope(answer), "```\ninner\n```");
    }

    #[test]
    fn plain_markdown_is_untouched() {
        let answer = "# Title\n\nBody.";

        assert_eq!(strip_envelope(answer), answer);
    }

    /// The reason the unwrapping exists, stated as the outcome rather than the
    /// mechanism: what the model read comes back as structure. Wrapped, the
    /// whole page would be one verbatim block with no heading in it at all.
    #[test]
    fn an_introduced_fenced_answer_becomes_structure() {
        let answer = "Here is the transcription of the page in GitHub-flavoured Markdown.\n\n\
                      ```markdown\n# Results and discussion\n\nA paragraph.\n```";

        let blocks = markdown_blocks(strip_envelope(answer), &EngineId::from("openai-compatible"));

        assert_eq!(blocks[0].kind, BlockKind::Heading { level: 1 });
        assert_eq!(blocks[0].text, "Results and discussion");
    }

    #[test]
    fn a_trailing_slash_in_the_base_url_does_not_double_up() {
        let endpoint = endpoint("http://127.0.0.1:1/");

        assert_eq!(
            endpoint.url("/v1/chat/completions"),
            "http://127.0.0.1:1/v1/chat/completions"
        );
    }
}
