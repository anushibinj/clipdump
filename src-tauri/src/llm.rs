//! "Split": ask the user's own OpenAI compatible server to break one long text
//! into the pieces a person would copy one by one (phone numbers, e-mail
//! addresses, tokens, user names, passwords, order numbers, ...).
//!
//! The model decides what counts as a piece, so nothing here knows about phone
//! numbers or e-mails. What this module does own is trust: a model can invent or
//! "fix" a value, and a copied password that is one character off is worse than
//! none. So every piece the model returns must occur character for character in
//! the original text, or it is dropped.
//!
//! Privacy: the text leaves the machine only when the user presses Split on one
//! item, only to the server they configured. Error messages never contain the
//! text or the server's answer (they reach the webview and may be logged).
//!
//! The HTTP call is behind [`HttpPost`] so the request building, the tolerant
//! parsing and the validation are tested without a network.

use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// Characters of the text sent to the model. A clipboard item can be megabytes;
/// the model's context and the user's bill are not the place for all of it.
pub const MAX_INPUT_CHARS: usize = 20_000;
/// Pieces returned at most.
pub const MAX_PARTS: usize = 100;
/// Longest label and longest value kept (characters).
const MAX_LABEL_CHARS: usize = 40;
const MAX_VALUE_CHARS: usize = 2_000;
/// How long the whole request may take.
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(60);

/// Where and how to ask. Built from the settings; `None` until Split is set up.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LlmConfig {
    pub base_url: String,
    pub model: String,
    pub api_key: String,
}

impl LlmConfig {
    pub fn from_settings(s: &crate::settings::Settings) -> Option<Self> {
        if s.llm_base_url.is_empty() || s.llm_model.is_empty() {
            return None;
        }
        Some(Self {
            base_url: s.llm_base_url.clone(),
            model: s.llm_model.clone(),
            api_key: s.llm_api_key.clone(),
        })
    }
}

/// One copyable piece: a short name for what it is and the exact text.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Part {
    pub label: String,
    pub value: String,
}

/// The one network call. Returns the response body of a 2xx answer, or a fixed
/// message that says what went wrong without quoting the server.
pub trait HttpPost {
    fn post_json(&self, url: &str, bearer: Option<&str>, body: &Value) -> Result<String, String>;
}

/// `base` is the server's API root (`http://localhost:11434/v1`); the endpoint
/// is `<base>/chat/completions`. A URL that already ends with it is kept, so
/// pasting the full endpoint works too. Only http and https are accepted.
pub fn endpoint(base: &str) -> Result<String, String> {
    let base = base.trim();
    if !(base.starts_with("http://") || base.starts_with("https://")) || base.contains(char::is_whitespace)
    {
        return Err("the server URL must start with http:// or https://".to_owned());
    }
    let trimmed = base.trim_end_matches('/');
    Ok(if trimmed.ends_with("/chat/completions") {
        trimmed.to_owned()
    } else {
        format!("{trimmed}/chat/completions")
    })
}

const SYSTEM_PROMPT: &str = "You split a pasted text into the separate items a person would want to copy one at a time. \
Items can be anything copyable: phone numbers, e-mail addresses, user names, passwords, API keys, tokens, \
IDs, order or invoice numbers, URLs, addresses, codes, amounts, dates, names, file paths. Do not assume a fixed list: \
decide from the text what the copyable items are.\n\
Reply with JSON only, no prose, in exactly this shape:\n\
{\"items\":[{\"label\":\"<2 to 4 word name of what it is>\",\"value\":\"<the item>\"}]}\n\
Rules:\n\
- \"value\" must be copied character for character from the text. Never correct, reformat, shorten or invent anything.\n\
- Leave out labels, field names and filler such as \"Password:\" or \"call me at\"; the value is only the item itself.\n\
- One entry per distinct item, in the order they appear in the text.\n\
- If the text has nothing worth copying separately, reply {\"items\":[]}.";

/// The chat completion request body. Deterministic (`temperature` 0) because the
/// same text should always split the same way.
pub fn build_request(model: &str, text: &str) -> Value {
    json!({
        "model": model,
        "temperature": 0,
        "messages": [
            { "role": "system", "content": SYSTEM_PROMPT },
            { "role": "user", "content": text },
        ],
    })
}

/// The part of `text` that is sent: at most [`MAX_INPUT_CHARS`] characters.
fn clip(text: &str) -> &str {
    match text.char_indices().nth(MAX_INPUT_CHARS) {
        Some((i, _)) => &text[..i],
        None => text,
    }
}

/// Reply text of the first choice. Servers differ in details, so this only
/// insists on `choices[0].message.content` being a string.
fn reply_text(body: &str) -> Result<String, String> {
    let v: Value =
        serde_json::from_str(body).map_err(|_| "the server did not answer with JSON".to_owned())?;
    v.pointer("/choices/0/message/content")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| "the server's answer has no message (is the URL an OpenAI compatible API?)".to_owned())
}

/// The JSON object inside a model reply: models wrap it in a code fence or add a
/// sentence around it even when told not to, so take the outermost `{ ... }`.
fn json_object(reply: &str) -> Option<&str> {
    let start = reply.find('{')?;
    let end = reply.rfind('}')?;
    (end > start).then(|| &reply[start..=end])
}

#[derive(Deserialize)]
struct Items {
    items: Vec<RawPart>,
}

#[derive(Deserialize)]
struct RawPart {
    #[serde(default)]
    label: String,
    value: String,
}

/// Turn the model's reply into pieces that are safe to copy. A piece is kept only
/// when its value (trimmed) occurs in `original` exactly; duplicates, empty
/// values and anything past [`MAX_PARTS`] are dropped. Labels are trimmed and
/// shortened; a missing label becomes "Item".
pub fn parse_parts(reply: &str, original: &str) -> Result<Vec<Part>, String> {
    let json = json_object(reply).ok_or_else(|| "the model did not answer with a list".to_owned())?;
    let items: Items =
        serde_json::from_str(json).map_err(|_| "the model did not answer with a list".to_owned())?;
    let mut parts: Vec<Part> = Vec::new();
    for raw in items.items {
        let value = raw.value.trim();
        if value.is_empty()
            || value.chars().count() > MAX_VALUE_CHARS
            || !original.contains(value)
            || parts.iter().any(|p| p.value == value)
        {
            continue;
        }
        let label: String = raw.label.trim().chars().take(MAX_LABEL_CHARS).collect();
        parts.push(Part {
            label: if label.is_empty() { "Item".to_owned() } else { label },
            value: value.to_owned(),
        });
        if parts.len() == MAX_PARTS {
            break;
        }
    }
    Ok(parts)
}

/// Split `text` with the configured server.
pub fn split(cfg: &LlmConfig, text: &str, http: &dyn HttpPost) -> Result<Vec<Part>, String> {
    let url = endpoint(&cfg.base_url)?;
    let sent = clip(text);
    if sent.trim().is_empty() {
        return Ok(Vec::new());
    }
    let bearer = Some(cfg.api_key.as_str()).filter(|k| !k.is_empty());
    let body = http.post_json(&url, bearer, &build_request(&cfg.model, sent))?;
    // Validated against what was sent, not the whole text: the model never saw the rest.
    parse_parts(&reply_text(&body)?, sent)
}

/// The real HTTP client (blocking, run from a command thread).
pub struct UreqHttp;

impl HttpPost for UreqHttp {
    fn post_json(&self, url: &str, bearer: Option<&str>, body: &Value) -> Result<String, String> {
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .timeout_global(Some(REQUEST_TIMEOUT))
            // A 4xx or 5xx is an answer to explain, not a transport error.
            .http_status_as_error(false)
            .build()
            .into();
        let mut req = agent.post(url);
        if let Some(key) = bearer {
            req = req.header("Authorization", &format!("Bearer {key}"));
        }
        let mut resp = req
            .send_json(body)
            .map_err(|_| "could not reach the LLM server (is it running, is the URL right?)".to_owned())?;
        let status = resp.status().as_u16();
        if !(200..300).contains(&status) {
            return Err(match status {
                401 | 403 => "the LLM server refused the API key".to_owned(),
                404 => "the LLM server has no such endpoint or model (check the URL and the model name)".to_owned(),
                429 => "the LLM server says too many requests, try again in a moment".to_owned(),
                _ => format!("the LLM server answered with an error ({status})"),
            });
        }
        resp.body_mut()
            .read_to_string()
            .map_err(|_| "the LLM server's answer could not be read".to_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    struct Fake {
        reply: Result<String, String>,
        seen: RefCell<Vec<(String, Option<String>, Value)>>,
    }

    impl Fake {
        fn answering(content: &str) -> Self {
            let body = json!({ "choices": [{ "message": { "content": content } }] }).to_string();
            Self { reply: Ok(body), seen: RefCell::default() }
        }
    }

    impl HttpPost for Fake {
        fn post_json(&self, url: &str, bearer: Option<&str>, body: &Value) -> Result<String, String> {
            self.seen
                .borrow_mut()
                .push((url.to_owned(), bearer.map(str::to_owned), body.clone()));
            self.reply.clone()
        }
    }

    fn cfg() -> LlmConfig {
        LlmConfig { base_url: "http://localhost:11434/v1/".into(), model: "m".into(), api_key: String::new() }
    }

    const TEXT: &str = "Call Ann on +1 555 0100 or mail ann@example.com. user: ann_k pw: Tr0ub4dor&3";

    #[test]
    fn the_endpoint_is_the_base_plus_chat_completions() {
        assert_eq!(endpoint("http://h:1/v1").unwrap(), "http://h:1/v1/chat/completions");
        assert_eq!(endpoint(" https://api.openai.com/v1/ ").unwrap(), "https://api.openai.com/v1/chat/completions");
        assert_eq!(
            endpoint("https://h/v1/chat/completions").unwrap(),
            "https://h/v1/chat/completions"
        );
        assert!(endpoint("ftp://h").is_err());
        assert!(endpoint("localhost:11434").is_err());
        assert!(endpoint("http://a b").is_err());
        assert!(endpoint("").is_err());
    }

    #[test]
    fn the_config_needs_a_url_and_a_model() {
        let mut s = crate::settings::Settings::default();
        assert!(LlmConfig::from_settings(&s).is_none());
        s.llm_base_url = "http://h/v1".into();
        assert!(LlmConfig::from_settings(&s).is_none(), "no model yet");
        s.llm_model = "m".into();
        assert!(LlmConfig::from_settings(&s).is_some());
    }

    #[test]
    fn the_request_carries_the_model_the_text_and_no_randomness() {
        let r = build_request("gpt-x", "hello");
        assert_eq!(r["model"], "gpt-x");
        assert_eq!(r["temperature"], 0);
        assert_eq!(r["messages"][0]["role"], "system");
        assert_eq!(r["messages"][1], json!({ "role": "user", "content": "hello" }));
    }

    #[test]
    fn exact_values_are_kept_in_order_with_their_labels() {
        let reply = r#"{"items":[
            {"label":"Phone","value":"+1 555 0100"},
            {"label":"Email","value":"ann@example.com"},
            {"label":"Username","value":"ann_k"},
            {"label":"Password","value":"Tr0ub4dor&3"}]}"#;
        let parts = parse_parts(reply, TEXT).unwrap();
        let got: Vec<_> = parts.iter().map(|p| (p.label.as_str(), p.value.as_str())).collect();
        assert_eq!(
            got,
            [("Phone", "+1 555 0100"), ("Email", "ann@example.com"), ("Username", "ann_k"), ("Password", "Tr0ub4dor&3")]
        );
    }

    #[test]
    fn a_value_the_text_does_not_contain_is_dropped() {
        // The model "fixed" the password and invented a number.
        let reply = r#"{"items":[
            {"label":"Password","value":"Tr0ub4dor&4"},
            {"label":"Phone","value":"+1 555 0199"},
            {"label":"Email","value":"ann@example.com"}]}"#;
        let parts = parse_parts(reply, TEXT).unwrap();
        assert_eq!(parts.len(), 1);
        assert_eq!(parts[0].value, "ann@example.com");
    }

    #[test]
    fn fences_prose_and_whitespace_around_the_json_are_tolerated() {
        let reply = "Sure! Here you go:\n```json\n{\"items\":[{\"label\":\" Email \",\"value\":\"  ann@example.com \"}]}\n```\nAnything else?";
        let parts = parse_parts(reply, TEXT).unwrap();
        assert_eq!(parts, vec![Part { label: "Email".into(), value: "ann@example.com".into() }]);
    }

    #[test]
    fn duplicates_empty_values_and_missing_labels_are_handled() {
        let reply = r#"{"items":[
            {"value":"ann_k"},
            {"label":"User","value":"ann_k"},
            {"label":"Empty","value":"   "}]}"#;
        let parts = parse_parts(reply, TEXT).unwrap();
        assert_eq!(parts, vec![Part { label: "Item".into(), value: "ann_k".into() }]);
    }

    #[test]
    fn no_items_is_an_answer_not_an_error() {
        assert!(parse_parts(r#"{"items":[]}"#, TEXT).unwrap().is_empty());
    }

    #[test]
    fn a_reply_that_is_not_a_list_is_an_error_without_the_reply_in_it() {
        for bad in ["I cannot help with that", "{\"items\": 3}", "{}", ""] {
            let e = parse_parts(bad, TEXT).unwrap_err();
            assert!(!e.contains("cannot"), "the reply must not leak into the message");
        }
    }

    #[test]
    fn parts_are_capped() {
        let text: String = (0..150).map(|n| format!("id{n} ")).collect();
        let items: Vec<Value> = (0..150).map(|n| json!({ "label": "Id", "value": format!("id{n}") })).collect();
        let reply = json!({ "items": items }).to_string();
        assert_eq!(parse_parts(&reply, &text).unwrap().len(), MAX_PARTS);
    }

    #[test]
    fn long_text_is_clipped_on_a_character_boundary() {
        let text = "é".repeat(MAX_INPUT_CHARS + 5);
        assert_eq!(clip(&text).chars().count(), MAX_INPUT_CHARS);
        assert_eq!(clip("short"), "short");
    }

    #[test]
    fn split_posts_to_the_endpoint_with_the_key_and_validates_the_answer() {
        let http = Fake::answering(r#"{"items":[{"label":"Email","value":"ann@example.com"},{"label":"X","value":"nope"}]}"#);
        let mut c = cfg();
        c.api_key = "sk-test".into();
        let parts = split(&c, TEXT, &http).unwrap();
        assert_eq!(parts.len(), 1);
        let seen = http.seen.borrow();
        assert_eq!(seen[0].0, "http://localhost:11434/v1/chat/completions");
        assert_eq!(seen[0].1.as_deref(), Some("sk-test"));
        assert_eq!(seen[0].2["messages"][1]["content"], TEXT);
    }

    #[test]
    fn no_key_means_no_authorization_header() {
        let http = Fake::answering(r#"{"items":[]}"#);
        split(&cfg(), TEXT, &http).unwrap();
        assert_eq!(http.seen.borrow()[0].1, None);
    }

    #[test]
    fn blank_text_never_reaches_the_server() {
        let http = Fake::answering(r#"{"items":[]}"#);
        assert!(split(&cfg(), " \n ", &http).unwrap().is_empty());
        assert!(http.seen.borrow().is_empty());
    }

    #[test]
    fn transport_errors_and_odd_bodies_come_back_as_plain_messages() {
        let down = Fake { reply: Err("could not reach the LLM server".into()), seen: RefCell::default() };
        assert!(split(&cfg(), TEXT, &down).unwrap_err().contains("could not reach"));
        let html = Fake { reply: Ok("<html>".into()), seen: RefCell::default() };
        assert!(split(&cfg(), TEXT, &html).unwrap_err().contains("JSON"));
        let empty = Fake { reply: Ok("{\"choices\":[]}".into()), seen: RefCell::default() };
        assert!(split(&cfg(), TEXT, &empty).unwrap_err().contains("OpenAI compatible"));
        let mut bad = cfg();
        bad.base_url = "nope".into();
        assert!(split(&bad, TEXT, &down).unwrap_err().contains("http"));
    }
}
