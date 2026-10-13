//! settings.shared.json -> ~/.claude/settings.json with ownership (§8, issue #17).
//! `merge` is pure; the apply layer reads and writes the files.
use serde_json::Value;
use std::collections::BTreeMap;

#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Owned {
    /// JSON pointer -> value the app set.
    pub values: BTreeMap<String, Value>,
    /// JSON pointer of an array -> items the app added.
    pub array_items: BTreeMap<String, Vec<Value>>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Merged {
    pub settings: Value,
    pub owned: Owned,
    /// Shared keys not applied, with the reason (§8 "never shared").
    pub skipped: Vec<String>,
    pub changed: bool,
}

/// `local` and `shared` must be JSON objects. `prev` is what this group owned after the last merge.
pub fn merge(_local: &Value, _shared: &Value, _prev: &Owned) -> crate::Result<Merged> {
    todo!()
}
