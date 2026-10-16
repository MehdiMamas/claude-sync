//! Optional Claude helper (§19): explains a collision or a CLAUDE.md change and summarises
//! incoming group changes, with the user's own Anthropic API key. The key lives only in the
//! OS keychain (account `@anthropic-key`, next to the GitHub token), is sent only to
//! `https://api.anthropic.com`, and is never logged. Every call is on demand: it costs the
//! user money, so nothing here runs by itself.
use crate::github::token_store::Store;
use crate::{secrets, Error, Result};
use async_trait::async_trait;
use serde_json::{json, Value};

pub const ACCOUNT: &str = "@anthropic-key";
pub const API_URL: &str = "https://api.anthropic.com/v1/messages";
pub const MODEL: &str = "claude-haiku-5-5";
pub const VERSION: &str = "2023-06-01";
pub const MAX_TOKENS: u32 = 600;
/// Text longer than this is cut, with a note, before it is sent.
pub const MAX_INPUT: usize = 40 * 1024;

const SYSTEM: &str = "You help a user of claude-sync, an app that shares Claude Code settings in groups. Explain plainly in a few short bullets what changed and what the user should decide. Do not repeat the text back. No preamble.";

/// What the user is asking about; it only changes the first line of the prompt.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Task {
    /// Two rules or settings from different groups clash (§7b).
    Collision,
    /// Lines others changed in the group's CLAUDE.md wait for review (§7a).
    ClaudeMdReview,
    /// Incoming changes in the group's activity feed.
    Activity,
}

impl Task {
    fn intro(self) -> &'static str {
        match self {
            Task::Collision => "These two rules or settings from different groups clash. Say how they differ and which one to keep:",
            Task::ClaudeMdReview => "Other members changed the group's CLAUDE.md. Say what the changes do and whether to accept them:",
            Task::Activity => "These are recent changes other members made to the group. Summarise them:",
        }
    }
}

#[derive(Clone)]
pub struct Reply {
    pub status: u16,
    pub body: String,
}

/// The one network call. The app uses reqwest; tests use a fake.
#[async_trait]
pub trait Http: Send + Sync {
    /// `Err` means the request never got an answer (offline, DNS, timeout).
    async fn post(
        &self,
        url: &str,
        headers: &[(&str, &str)],
        body: String,
    ) -> Result<Reply, String>;
}

pub struct ReqwestHttp;

#[async_trait]
impl Http for ReqwestHttp {
    async fn post(
        &self,
        url: &str,
        headers: &[(&str, &str)],
        body: String,
    ) -> Result<Reply, String> {
        let mut req = crate::github::http_client().post(url).body(body);
        for (k, v) in headers {
            req = req.header(*k, *v);
        }
        let res = req.send().await.map_err(|e| e.without_url().to_string())?;
        let status = res.status().as_u16();
        let body = res.text().await.map_err(|e| e.without_url().to_string())?;
        Ok(Reply { status, body })
    }
}

/// The saved key, if any.
pub fn load_key(store: &impl Store) -> Result<Option<String>> {
    Ok(store.get(ACCOUNT)?.filter(|k| !k.is_empty()))
}

/// Check and store a key. Only the shape is checked here; "Test key" asks Anthropic.
pub fn save_key(store: &impl Store, key: &str) -> Result<()> {
    let key = key.trim();
    if !key.starts_with("sk-ant-") || key.chars().any(char::is_whitespace) {
        return Err(Error::Invalid(
            "That does not look like an Anthropic API key. It starts with sk-ant-.".into(),
        ));
    }
    store.set(ACCOUNT, key)
}

pub fn remove_key(store: &impl Store) -> Result<()> {
    store.delete(ACCOUNT)
}

/// The last four characters, for "saved key ending in …".
pub fn key_hint(key: &str) -> String {
    let n = key.chars().count();
    key.chars().skip(n.saturating_sub(4)).collect()
}

/// Cut `text` to `MAX_INPUT` bytes on a char boundary, with a note.
fn cap(text: &str) -> String {
    if text.len() <= MAX_INPUT {
        return text.to_string();
    }
    let mut end = MAX_INPUT;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!(
        "{}\n[…cut: the text was longer than {} KB]",
        &text[..end],
        MAX_INPUT / 1024
    )
}

fn request_body(prompt: &str, max_tokens: u32) -> String {
    json!({
        "model": MODEL,
        "max_tokens": max_tokens,
        "system": SYSTEM,
        "messages": [{ "role": "user", "content": prompt }],
    })
    .to_string()
}

async fn send(http: &dyn Http, key: &str, body: String) -> Result<String> {
    let headers = [
        ("x-api-key", key),
        ("anthropic-version", VERSION),
        ("content-type", "application/json"),
    ];
    let reply = http.post(API_URL, &headers, body).await.map_err(|_| {
        Error::Invalid(
            "Could not reach Anthropic. Check your internet connection and try again.".into(),
        )
    })?;
    if reply.status == 200 {
        let v: Value = serde_json::from_str(&reply.body)?;
        let text: String = v["content"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|b| b["text"].as_str())
            .collect();
        return Ok(text.trim().to_string());
    }
    Err(Error::Invalid(match reply.status {
        401 | 403 => "Anthropic did not accept your API key. Check it in Settings.".to_string(),
        429 => "Anthropic says you are sending too many requests or are out of credit. Wait a bit or check your balance.".to_string(),
        529 => "Anthropic is overloaded right now. Try again in a minute.".to_string(),
        s => format!("Anthropic returned an error ({s})."),
    }))
}

/// Explain or summarise `text`. Refuses, without sending, when the text holds a secret (§9).
pub async fn explain(http: &dyn Http, key: &str, task: Task, text: &str) -> Result<String> {
    if let Some(pattern) = secrets::first_hit(text) {
        return Err(Error::Invalid(format!(
            "This text looks like it contains a secret ({pattern}), so it was not sent to Claude. Remove the secret first."
        )));
    }
    let prompt = format!("{}\n\n{}", task.intro(), cap(text));
    send(http, key, request_body(&prompt, MAX_TOKENS)).await
}

/// Ask Anthropic whether the key works (one tiny request).
pub async fn test_key(http: &dyn Http, key: &str) -> Result<()> {
    send(http, key, request_body("Reply with: ok", 5))
        .await
        .map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::collections::HashMap;
    use std::sync::Mutex;

    #[derive(Default)]
    struct MemStore(RefCell<HashMap<String, String>>);
    impl Store for MemStore {
        fn get(&self, a: &str) -> Result<Option<String>> {
            Ok(self.0.borrow().get(a).cloned())
        }
        fn set(&self, a: &str, s: &str) -> Result<()> {
            self.0.borrow_mut().insert(a.into(), s.into());
            Ok(())
        }
        fn delete(&self, a: &str) -> Result<()> {
            self.0.borrow_mut().remove(a);
            Ok(())
        }
    }

    type Seen = Vec<(String, Vec<(String, String)>, String)>;
    struct Fake {
        reply: Option<(u16, String)>,
        seen: Mutex<Seen>,
    }
    impl Fake {
        fn new(status: u16, body: &str) -> Self {
            Fake {
                reply: Some((status, body.into())),
                seen: Mutex::default(),
            }
        }
        fn offline() -> Self {
            Fake {
                reply: None,
                seen: Mutex::default(),
            }
        }
    }
    #[async_trait]
    impl Http for Fake {
        async fn post(
            &self,
            url: &str,
            headers: &[(&str, &str)],
            body: String,
        ) -> Result<Reply, String> {
            let h = headers
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect();
            self.seen.lock().unwrap().push((url.into(), h, body));
            self.reply
                .clone()
                .map(|(status, body)| Reply { status, body })
                .ok_or("offline".into())
        }
    }

    const KEY: &str = "sk-ant-api03-abcdefghijWXYZ";
    const OK: &str = r#"{"content":[{"type":"text","text":"- one\n- two"}]}"#;

    fn run<T>(f: impl std::future::Future<Output = T>) -> T {
        tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap()
            .block_on(f)
    }

    #[test]
    fn key_is_stored_and_removed() {
        let s = MemStore::default();
        assert_eq!(load_key(&s).unwrap(), None);
        save_key(&s, &format!("  {KEY}\n")).unwrap();
        assert_eq!(load_key(&s).unwrap().as_deref(), Some(KEY));
        assert_eq!(key_hint(KEY), "WXYZ");
        remove_key(&s).unwrap();
        assert_eq!(load_key(&s).unwrap(), None);
    }

    #[test]
    fn odd_keys_are_refused() {
        let s = MemStore::default();
        assert!(save_key(&s, "ghp_nope").is_err());
        assert!(save_key(&s, "sk-ant-a b").is_err());
        assert_eq!(load_key(&s).unwrap(), None);
    }

    #[test]
    fn request_has_the_right_shape_and_no_key_in_body() {
        let f = Fake::new(200, OK);
        let out = run(explain(&f, KEY, Task::Collision, "use tabs\nuse spaces")).unwrap();
        assert_eq!(out, "- one\n- two");
        let seen = f.seen.lock().unwrap();
        let (url, headers, body) = &seen[0];
        assert_eq!(url, "https://api.anthropic.com/v1/messages");
        assert!(headers.contains(&("x-api-key".into(), KEY.into())));
        assert!(headers.contains(&("anthropic-version".into(), "2023-06-01".into())));
        assert!(!body.contains(KEY));
        let v: Value = serde_json::from_str(body).unwrap();
        assert_eq!(v["model"], "claude-haiku-5-5");
        assert_eq!(v["max_tokens"], 600);
        assert!(v["messages"][0]["content"]
            .as_str()
            .unwrap()
            .contains("use spaces"));
    }

    #[test]
    fn text_with_a_secret_is_not_sent() {
        let f = Fake::new(200, OK);
        let text = format!("token: ghp_{}", "a".repeat(36));
        let err = run(explain(&f, KEY, Task::Activity, &text))
            .unwrap_err()
            .to_string();
        assert!(err.contains("not sent"), "{err}");
        assert!(!err.contains("ghp_aaa"));
        assert!(f.seen.lock().unwrap().is_empty());
    }

    #[test]
    fn long_text_is_cut_with_a_note() {
        let f = Fake::new(200, OK);
        run(explain(&f, KEY, Task::Activity, &"é".repeat(MAX_INPUT))).unwrap();
        let body = f.seen.lock().unwrap()[0].2.clone();
        assert!(body.contains("cut"));
        assert!(body.len() < MAX_INPUT + 2048);
    }

    #[test]
    fn errors_are_plain_words() {
        let msg = |f: Fake| {
            run(explain(&f, KEY, Task::Activity, "x"))
                .unwrap_err()
                .to_string()
        };
        assert!(msg(Fake::new(401, "{}")).contains("did not accept your API key"));
        assert!(msg(Fake::new(429, "{}")).contains("credit"));
        assert!(msg(Fake::new(529, "{}")).contains("overloaded"));
        assert!(msg(Fake::offline()).contains("internet"));
        assert!(!msg(Fake::new(401, "{}")).contains(KEY));
    }

    #[test]
    fn test_key_maps_401() {
        assert!(run(test_key(&Fake::new(200, OK), KEY)).is_ok());
        assert!(run(test_key(&Fake::new(401, "{}"), KEY)).is_err());
    }
}
