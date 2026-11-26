//! Typed readers and writers for every format v1 and v2 share (v2-RepoFormats). Bytes in, typed
//! value out, and back, with no file access.
//!
//! [`Doc`] keeps the original bytes next to the typed value. Unchanged: the original bytes come
//! back as they were. Changed: only the changed values are patched into the parsed original
//! (key order and unknown keys stay) and written in the file's own indent, line ends and
//! trailing newline.
//! shortcut: a rewrite after a change re-renders the whole JSON text, so odd escapes (`é`)
//! and number spellings elsewhere in the file are normalised; upgrade if a format needs more.
use serde::{de::DeserializeOwned, Serialize};
use serde_json::{ser::PrettyFormatter, Serializer, Value};

pub mod group;
pub mod pack;
pub mod personal;
pub mod schedules;
pub mod text;

#[derive(Debug, thiserror::Error)]
pub enum FormatError {
    #[error("not valid JSON for this format: {0}")]
    Json(#[from] serde_json::Error),
    #[error("{file} has version {found}, newer than this app reads ({max}); update the app")]
    TooNew {
        file: &'static str,
        found: u64,
        max: u32,
    },
    #[error("{0}")]
    Invalid(String),
}

/// How a JSON file is laid out, so a rewrite looks like the file did.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Style {
    /// `None`: compact.
    indent: Option<Vec<u8>>,
    crlf: bool,
    final_newline: bool,
}

impl Style {
    /// What v1 writes: two spaces, LF, final newline.
    fn v1() -> Style {
        Style {
            indent: Some(b"  ".to_vec()),
            crlf: false,
            final_newline: true,
        }
    }

    fn detect(raw: &[u8]) -> Self {
        let indent = raw
            .split(|b| *b == b'\n')
            .skip(1)
            .map(|l| l.iter().take_while(|b| matches!(**b, b' ' | b'\t')).count())
            .zip(raw.split(|b| *b == b'\n').skip(1))
            .find(|(n, l)| *n > 0 && l.len() > *n && l[*n] != b'\r')
            .map(|(n, l)| l[..n].to_vec());
        Style {
            indent,
            crlf: raw.windows(2).any(|w| w == b"\r\n"),
            final_newline: raw.ends_with(b"\n"),
        }
    }

    fn render(&self, v: &Value) -> Result<Vec<u8>, FormatError> {
        let mut out = Vec::new();
        match &self.indent {
            None => out = serde_json::to_vec(v)?,
            Some(indent) => {
                let mut ser =
                    Serializer::with_formatter(&mut out, PrettyFormatter::with_indent(indent));
                v.serialize(&mut ser)?;
            }
        }
        if self.crlf {
            out = out.into_iter().fold(Vec::new(), |mut a, b| {
                if b == b'\n' {
                    a.push(b'\r');
                }
                a.push(b);
                a
            });
        }
        if self.final_newline {
            out.extend_from_slice(if self.crlf { b"\r\n" } else { b"\n" });
        }
        Ok(out)
    }
}

/// A JSON file as a typed value that writes back the bytes it read.
#[derive(Debug, Clone)]
pub struct Doc<T> {
    /// Edit this; [`Doc::to_bytes`] writes the difference.
    pub value: T,
    /// Parsed original bytes, unknown keys included; `None` for a new file.
    original: Option<(Vec<u8>, Value, Value)>,
    style: Style,
}

impl<T: Serialize + DeserializeOwned> Doc<T> {
    pub fn parse(bytes: &[u8]) -> Result<Self, FormatError> {
        let full: Value = serde_json::from_slice(bytes)?;
        let value: T = serde_json::from_value(full.clone())?;
        let typed = serde_json::to_value(&value)?;
        Ok(Doc {
            value,
            original: Some((bytes.to_vec(), full, typed)),
            style: Style::detect(bytes),
        })
    }

    /// A new file in the v1 style: two spaces, LF, final newline.
    pub fn new(value: T) -> Self {
        Doc {
            value,
            original: None,
            style: Style::v1(),
        }
    }

    pub fn to_bytes(&self) -> Result<Vec<u8>, FormatError> {
        let now = serde_json::to_value(&self.value)?;
        let Some((raw, full, typed)) = &self.original else {
            return self.style.render(&now);
        };
        if *typed == now {
            return Ok(raw.clone());
        }
        let mut out = full.clone();
        patch(typed, &now, &mut out);
        self.style.render(&out)
    }
}

/// Apply what changed from `old` to `new` onto `target` (a superset of `old`), leaving the rest.
fn patch(old: &Value, new: &Value, target: &mut Value) {
    match (old, new, &mut *target) {
        (Value::Object(o), Value::Object(n), Value::Object(t)) => {
            for k in o.keys().filter(|k| !n.contains_key(*k)) {
                t.shift_remove(k);
            }
            for (k, nv) in n {
                match (o.get(k), t.get_mut(k)) {
                    (Some(ov), _) if ov == nv => {}
                    (Some(ov), Some(tv)) => patch(ov, nv, tv),
                    _ => {
                        t.insert(k.clone(), nv.clone());
                    }
                }
            }
        }
        (Value::Array(o), Value::Array(n), Value::Array(t)) if o.len() == t.len() => {
            t.truncate(n.len());
            for (i, nv) in n.iter().enumerate() {
                match (o.get(i), t.get_mut(i)) {
                    (Some(ov), Some(tv)) => patch(ov, nv, tv),
                    _ => t.push(nv.clone()),
                }
            }
        }
        _ => *target = new.clone(),
    }
}
