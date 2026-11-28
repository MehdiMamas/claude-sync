//! `schedules.json`: `{"jobs": [...]}` with the limits of Parity row 73. The file is a
//! `Doc<Value>` so every key of every job stays; [`validate`] reads the jobs that pass.
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const PERMISSION_MODES: [&str; 4] = ["default", "acceptEdits", "plan", "bypassPermissions"];
pub const MAX_PROMPT: usize = 4096;
pub const MAX_JOBS: usize = 20;
pub const DEFAULT_TIMEOUT: u32 = 60;
pub const MAX_TIMEOUT: u32 = 240;
pub const ALLOWED_ARGS: [&str; 3] = ["--chrome", "--model", "--effort"];
pub const EFFORTS: [&str; 5] = ["low", "medium", "high", "xhigh", "max"];
/// Task Scheduler takes 48 triggers per task.
pub const MAX_TIMES: usize = 48;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JobEntry {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    pub cron: String,
    pub prompt: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permission_mode: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_minutes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub report: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub args: Option<Vec<String>>,
}

/// A job (or the whole file) that was skipped, and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Problem {
    pub id: Option<String>,
    pub message: String,
}

/// `[a-z0-9][a-z0-9-]{0,40}`
pub fn valid_id(id: &str) -> bool {
    let b = id.as_bytes();
    !b.is_empty()
        && b.len() <= 41
        && (b[0].is_ascii_lowercase() || b[0].is_ascii_digit())
        && b.iter()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'-')
}

/// `--chrome`, `--model <id>` and `--effort <level>`, each at most once.
pub fn valid_args(args: &[String]) -> Result<(), String> {
    let mut seen = Vec::new();
    let mut it = args.iter();
    while let Some(flag) = it.next() {
        if !ALLOWED_ARGS.contains(&flag.as_str()) {
            return Err(format!(
                "args: {flag:?} is not allowed; only --chrome, --model <id> and --effort <level>"
            ));
        }
        if seen.contains(&flag) {
            return Err(format!("args: {flag} is given twice"));
        }
        seen.push(flag);
        match flag.as_str() {
            "--model" => {
                let v = it.next().map(String::as_str).unwrap_or("");
                let ok = (1..=64).contains(&v.len())
                    && v.as_bytes()[0].is_ascii_alphanumeric()
                    && v.bytes()
                        .all(|c| c.is_ascii_alphanumeric() || b"-._[]".contains(&c));
                if !ok {
                    return Err(format!("args: --model needs a model id, not {v:?}"));
                }
            }
            "--effort" => {
                let v = it.next().map(String::as_str).unwrap_or("");
                if !EFFORTS.contains(&v) {
                    return Err(format!(
                        "args: --effort {v:?} must be one of {}",
                        EFFORTS.join(", ")
                    ));
                }
            }
            _ => {}
        }
    }
    Ok(())
}

/// `~` or `~/a/b`: a folder in the home folder, no `..`.
pub fn valid_home_path(raw: &str) -> Result<(), String> {
    let rest = match raw {
        "~" => "",
        r => r.strip_prefix("~/").ok_or_else(|| {
            format!("{raw:?} must be ~ or start with ~/ (a folder in your home folder)")
        })?,
    };
    for part in rest.split('/').filter(|p| !p.is_empty()) {
        if part == "." || part == ".." || part.contains(['\\', ':']) || part.contains('\0') {
            return Err(format!("{raw:?} may not contain {part:?}"));
        }
    }
    Ok(())
}

fn number(s: &str, max: u8, what: &str) -> Result<u8, String> {
    s.parse::<u8>()
        .ok()
        .filter(|n| *n <= max && s.bytes().all(|b| b.is_ascii_digit()))
        .ok_or_else(|| format!("cron {what} {s:?} must be a number from 0 to {max}"))
}

fn count(field: &str, max: u8, what: &str) -> Result<usize, String> {
    if field.contains(['*', '-', '/']) {
        return Err(format!(
            "cron {what} {field:?} must be numbers or a comma list (no *, ranges or steps)"
        ));
    }
    let mut n = field
        .split(',')
        .map(|s| number(s, max, what))
        .collect::<Result<Vec<_>, _>>()?;
    n.sort_unstable();
    n.dedup();
    Ok(n.len())
}

fn weekdays(field: &str) -> Result<(), String> {
    if field == "*" {
        return Ok(());
    }
    for part in field.split(',') {
        match part.split_once('-') {
            Some((a, b)) => {
                if number(a, 6, "day of week")? > number(b, 6, "day of week")? {
                    return Err(format!("cron day of week range {part:?} goes backwards"));
                }
            }
            None => {
                number(part, 6, "day of week")?;
            }
        }
    }
    Ok(())
}

/// The cron subset a job may use: exact minutes and hours (numbers or comma lists), `*` for
/// day of month and month, `*` or 0-6 lists and ranges for the day of week.
pub fn valid_cron(expr: &str) -> Result<(), String> {
    let fields: Vec<&str> = expr.split_whitespace().collect();
    let [minute, hour, dom, month, dow] = fields.as_slice() else {
        return Err(format!(
            "cron {expr:?} needs 5 fields (minute hour day-of-month month day-of-week)"
        ));
    };
    let times = count(minute, 59, "minute")? * count(hour, 23, "hour")?;
    if *dom != "*" || *month != "*" {
        return Err(format!(
            "cron {expr:?}: day of month and month must be *; only weekly schedules are supported"
        ));
    }
    weekdays(dow)?;
    if times > MAX_TIMES {
        return Err(format!(
            "cron {expr:?} runs at {times} times a day; at most {MAX_TIMES} are supported"
        ));
    }
    Ok(())
}

fn check(j: &JobEntry) -> Result<(), String> {
    if !valid_id(&j.id) {
        return Err(format!(
            "id {:?} must be lowercase letters, digits and dashes (at most 41, starting with a letter or digit)",
            j.id
        ));
    }
    valid_cron(&j.cron)?;
    if j.prompt.trim().is_empty() {
        return Err("prompt is empty".into());
    }
    if j.prompt.len() > MAX_PROMPT {
        return Err(format!(
            "prompt is {} bytes; at most {MAX_PROMPT}",
            j.prompt.len()
        ));
    }
    if j.prompt.contains('\0') {
        return Err("prompt contains a NUL character".into());
    }
    if j.prompt.starts_with('-') {
        return Err("prompt may not start with - (it would be read as an option)".into());
    }
    if let Some(m) = &j.permission_mode {
        if !PERMISSION_MODES.contains(&m.as_str()) {
            return Err(format!(
                "permissionMode {m:?} must be one of {}",
                PERMISSION_MODES.join(", ")
            ));
        }
    }
    if let Some(t) = j.timeout_minutes {
        if !(1..=u64::from(MAX_TIMEOUT)).contains(&t) {
            return Err(format!(
                "timeoutMinutes {t} must be from 1 to {MAX_TIMEOUT}"
            ));
        }
    }
    if let Some(c) = &j.cwd {
        valid_home_path(c).map_err(|e| format!("cwd {e}"))?;
    }
    if let Some(r) = &j.report {
        valid_home_path(r).map_err(|e| format!("report {e}"))?;
        if r == "~" {
            return Err("report must be a file, not ~".into());
        }
    }
    valid_args(j.args.as_deref().unwrap_or_default())
}

/// The valid jobs of one `schedules.json` value, and what was skipped. A bad job never takes
/// the others down with it.
pub fn validate(file: &Value) -> (Vec<JobEntry>, Vec<Problem>) {
    let whole = |message: String| (Vec::new(), vec![Problem { id: None, message }]);
    let Some(obj) = file.as_object() else {
        return whole("schedules.json is not a JSON object".into());
    };
    let entries = match obj.get("jobs") {
        Some(Value::Array(a)) => a.as_slice(),
        None => &[],
        Some(_) => return whole("schedules.json: \"jobs\" is not a list".into()),
    };
    let (mut jobs, mut problems) = (Vec::<JobEntry>::new(), Vec::new());
    for (i, entry) in entries.iter().enumerate() {
        let id = entry.get("id").and_then(Value::as_str).map(String::from);
        let result = if i >= MAX_JOBS {
            Err(format!("more than {MAX_JOBS} jobs; the rest are skipped"))
        } else {
            serde_json::from_value::<JobEntry>(entry.clone())
                .map_err(|e| e.to_string())
                .and_then(|j| check(&j).map(|()| j))
        };
        match result {
            Ok(j) if jobs.iter().any(|o| o.id == j.id) => problems.push(Problem {
                id,
                message: "the id is used twice in this file".into(),
            }),
            Ok(j) => jobs.push(j),
            Err(message) => problems.push(Problem { id, message }),
        }
    }
    (jobs, problems)
}
