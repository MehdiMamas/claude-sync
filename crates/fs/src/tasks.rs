//! OS task writers for group jobs (v2-Parity: scheduled jobs). The exact text or XML of a task
//! is built here; the OS tools (`schtasks`, `launchctl`, `systemctl`, `crontab`) run behind
//! [`Runner`], so tests capture the commands. [`reconcile`] re-registers what moved or is
//! missing and never registers a job twice; [`remove_all`] is for uninstall.
use crate::atomic::write_atomic;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

pub const TASK_FOLDER: &str = "\\claude-sync\\";
pub const LAUNCHD_PREFIX: &str = "dev.claude-sync.job.";
pub const SYSTEMD_PREFIX: &str = "claude-sync-job-";
pub const OS_GRACE_MINUTES: u32 = 10;
/// The most (hour, minute) times one job may have. Task Scheduler takes 48 triggers per task.
pub const MAX_TIMES: usize = 48;
const DAY_NAMES: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskSpec {
    pub id: String,
    pub title: String,
    pub times: Vec<(u8, u8)>,
    /// Cron weekdays (0 = Sunday); `None`: every day.
    pub days: Option<Vec<u8>>,
    pub program: PathBuf,
    pub args: Vec<String>,
    pub timeout_minutes: u32,
}

impl TaskSpec {
    pub fn new(
        id: &str,
        title: &str,
        cron: &str,
        program: &Path,
        timeout_minutes: u32,
    ) -> Result<Self, String> {
        let s = Schedule::parse(cron)?;
        Ok(Self {
            id: id.into(),
            title: title.into(),
            times: s.times(),
            days: s.days,
            program: program.into(),
            args: vec!["run-job".into(), id.into()],
            timeout_minutes,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Schedule {
    /// Sorted, unique.
    pub minutes: Vec<u8>,
    /// Sorted, unique.
    pub hours: Vec<u8>,
    /// Sorted cron weekdays (0 = Sunday). `None`: every day.
    pub days: Option<Vec<u8>>,
}

impl Schedule {
    pub fn parse(expr: &str) -> Result<Self, String> {
        let fields: Vec<&str> = expr.split_whitespace().collect();
        let [minute, hour, dom, month, dow] = fields.as_slice() else {
            return Err(format!(
                "cron {expr:?} needs 5 fields (minute hour day-of-month month day-of-week)"
            ));
        };
        let minutes = numbers(minute, 59, "minute")?;
        let hours = numbers(hour, 23, "hour")?;
        if *dom != "*" || *month != "*" {
            return Err(format!(
                "cron {expr:?}: day of month and month must be *; only weekly schedules are supported"
            ));
        }
        let days = weekdays(dow)?;
        let s = Self {
            minutes,
            hours,
            days,
        };
        if s.times().len() > MAX_TIMES {
            return Err(format!(
                "cron {expr:?} runs at {} times a day; at most {MAX_TIMES} are supported",
                s.times().len()
            ));
        }
        Ok(s)
    }

    /// Every (hour, minute) of a run day, in order.
    pub fn times(&self) -> Vec<(u8, u8)> {
        self.hours
            .iter()
            .flat_map(|h| self.minutes.iter().map(move |m| (*h, *m)))
            .collect()
    }

    /// `Mon and Thu at 10:17`, `Every day at 08:00 and 18:30`, `Weekdays at 09:00`.
    /// Times are the machine's local time.
    pub fn describe(&self) -> String {
        let days = match self.days.as_deref() {
            None => "Every day".to_string(),
            Some([1, 2, 3, 4, 5]) => "Weekdays".to_string(),
            Some([0, 6]) => "Weekends".to_string(),
            Some(days) => {
                // Monday first.
                let mut d = days.to_vec();
                d.sort_by_key(|x| (x + 6) % 7);
                join(
                    d.iter()
                        .map(|x| DAY_NAMES[*x as usize].to_string())
                        .collect(),
                )
            }
        };
        let times = self
            .times()
            .iter()
            .map(|(h, m)| format!("{h:02}:{m:02}"))
            .collect();
        format!("{days} at {}", join(times))
    }
}

/// `a`, `a and b`, `a, b and c`.
fn join(items: Vec<String>) -> String {
    match items.split_last() {
        None => String::new(),
        Some((last, [])) => last.clone(),
        Some((last, rest)) => format!("{} and {last}", rest.join(", ")),
    }
}

fn number(s: &str, max: u8, what: &str) -> Result<u8, String> {
    s.parse::<u8>()
        .ok()
        .filter(|n| *n <= max && !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()))
        .ok_or_else(|| format!("cron {what} {s:?} must be a number from 0 to {max}"))
}

/// A number or comma list (`7`, `0,30`). No `*`, ranges or steps: a job runs at set times.
fn numbers(field: &str, max: u8, what: &str) -> Result<Vec<u8>, String> {
    if field.contains(['*', '-', '/']) {
        return Err(format!(
            "cron {what} {field:?} must be numbers or a comma list (no *, ranges or steps)"
        ));
    }
    let mut out = field
        .split(',')
        .map(|s| number(s, max, what))
        .collect::<Result<Vec<_>, _>>()?;
    out.sort_unstable();
    out.dedup();
    Ok(out)
}

/// `*`, or a comma list of 0-6 days and ranges (`1,4`, `1-5`).
fn weekdays(field: &str) -> Result<Option<Vec<u8>>, String> {
    if field == "*" {
        return Ok(None);
    }
    let mut days = Vec::new();
    for part in field.split(',') {
        match part.split_once('-') {
            Some((a, b)) => {
                let (a, b) = (number(a, 6, "day of week")?, number(b, 6, "day of week")?);
                if a > b {
                    return Err(format!("cron day of week range {part:?} goes backwards"));
                }
                days.extend(a..=b);
            }
            None => days.push(number(part, 6, "day of week")?),
        }
    }
    days.sort_unstable();
    days.dedup();
    Ok((days.len() < 7).then_some(days))
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

const WEEKDAYS: [&str; 7] = [
    "Sunday",
    "Monday",
    "Tuesday",
    "Wednesday",
    "Thursday",
    "Friday",
    "Saturday",
];

/// Quote one Windows command-line argument (CommandLineToArgvW rules).
fn win_quote(arg: &str) -> String {
    if !arg.is_empty() && !arg.contains([' ', '\t', '"']) {
        return arg.into();
    }
    let mut out = String::from('"');
    let mut slashes = 0;
    for c in arg.chars() {
        match c {
            '\\' => slashes += 1,
            '"' => {
                out.push_str(&"\\".repeat(slashes * 2 + 1));
                out.push('"');
                slashes = 0;
                continue;
            }
            _ => {}
        }
        if c != '\\' {
            out.push_str(&"\\".repeat(slashes));
            slashes = 0;
            out.push(c);
        }
    }
    out.push_str(&"\\".repeat(slashes * 2));
    out.push('"');
    out
}

/// The Task Scheduler definition of `task` for `user` (`DOMAIN\name`, or empty for the
/// registering user). Times are local: no time zone on `StartBoundary`.
pub fn task_xml(task: &TaskSpec, user: &str) -> String {
    let mut triggers = String::new();
    for (h, m) in &task.times {
        let schedule = match &task.days {
            None => "      <ScheduleByDay>\n        <DaysInterval>1</DaysInterval>\n      </ScheduleByDay>\n".to_string(),
            Some(days) => {
                let names: String = days
                    .iter()
                    .map(|d| format!("          <{} />\n", WEEKDAYS[*d as usize]))
                    .collect();
                format!(
                    "      <ScheduleByWeek>\n        <DaysOfWeek>\n{names}        </DaysOfWeek>\n        <WeeksInterval>1</WeeksInterval>\n      </ScheduleByWeek>\n"
                )
            }
        };
        triggers.push_str(&format!(
            "    <CalendarTrigger>\n      <StartBoundary>2026-01-01T{h:02}:{m:02}:00</StartBoundary>\n      <Enabled>true</Enabled>\n{schedule}    </CalendarTrigger>\n"
        ));
    }
    let user = match user {
        "" => String::new(),
        u => format!("      <UserId>{}</UserId>\n", xml_escape(u)),
    };
    let args: Vec<String> = task.args.iter().map(|a| win_quote(a)).collect();
    format!(
        r#"<?xml version="1.0" encoding="UTF-16"?>
<Task version="1.2" xmlns="http://schemas.microsoft.com/windows/2004/02/mit/task">
  <RegistrationInfo>
    <Author>claude-sync</Author>
    <Description>{desc}</Description>
  </RegistrationInfo>
  <Triggers>
{triggers}  </Triggers>
  <Principals>
    <Principal id="Author">
{user}      <LogonType>InteractiveToken</LogonType>
      <RunLevel>LeastPrivilege</RunLevel>
    </Principal>
  </Principals>
  <Settings>
    <MultipleInstancesPolicy>IgnoreNew</MultipleInstancesPolicy>
    <DisallowStartIfOnBatteries>false</DisallowStartIfOnBatteries>
    <StopIfGoingOnBatteries>false</StopIfGoingOnBatteries>
    <AllowHardTerminate>true</AllowHardTerminate>
    <StartWhenAvailable>true</StartWhenAvailable>
    <RunOnlyIfNetworkAvailable>false</RunOnlyIfNetworkAvailable>
    <IdleSettings>
      <StopOnIdleEnd>false</StopOnIdleEnd>
      <RestartOnIdle>false</RestartOnIdle>
    </IdleSettings>
    <AllowStartOnDemand>true</AllowStartOnDemand>
    <Enabled>true</Enabled>
    <Hidden>true</Hidden>
    <RunOnlyIfIdle>false</RunOnlyIfIdle>
    <WakeToRun>false</WakeToRun>
    <ExecutionTimeLimit>PT{limit}M</ExecutionTimeLimit>
    <Priority>7</Priority>
  </Settings>
  <Actions Context="Author">
    <Exec>
      <Command>{command}</Command>
      <Arguments>{args}</Arguments>
    </Exec>
  </Actions>
</Task>
"#,
        desc = xml_escape(&format!(
            "{} (claude-sync group job {})",
            task.title, task.id
        )),
        limit = task.timeout_minutes + OS_GRACE_MINUTES,
        command = xml_escape(&win_quote(&task.program.to_string_lossy())),
        args = xml_escape(&args.join(" ")),
    )
}

/// UTF-16LE with a BOM, as the `encoding="UTF-16"` declaration says.
pub fn utf16_bytes(text: &str) -> Vec<u8> {
    let mut out = vec![0xFF, 0xFE];
    for unit in text.encode_utf16() {
        out.extend_from_slice(&unit.to_le_bytes());
    }
    out
}

/// The launchd agent of `task`: one `StartCalendarInterval` entry per weekday and time.
/// launchd runs a missed interval when the Mac wakes up.
pub fn launchd_plist(task: &TaskSpec, prefix: &str) -> String {
    let mut args = String::new();
    for a in std::iter::once(task.program.to_string_lossy().into_owned()).chain(task.args.clone()) {
        args.push_str(&format!("\t\t<string>{}</string>\n", xml_escape(&a)));
    }
    let days: Vec<Option<u8>> = match &task.days {
        None => vec![None],
        Some(d) => d.iter().copied().map(Some).collect(),
    };
    let mut intervals = String::new();
    for day in &days {
        for (h, m) in &task.times {
            intervals.push_str(&format!(
                "\t\t<dict>\n\t\t\t<key>Hour</key>\n\t\t\t<integer>{h}</integer>\n\t\t\t<key>Minute</key>\n\t\t\t<integer>{m}</integer>\n"
            ));
            if let Some(d) = day {
                intervals.push_str(&format!(
                    "\t\t\t<key>Weekday</key>\n\t\t\t<integer>{d}</integer>\n"
                ));
            }
            intervals.push_str("\t\t</dict>\n");
        }
    }
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
<plist version=\"1.0\">\n\
<dict>\n\
\t<key>Label</key>\n\
\t<string>{prefix}{id}</string>\n\
\t<key>ProgramArguments</key>\n\
\t<array>\n\
{args}\t</array>\n\
\t<key>StartCalendarInterval</key>\n\
\t<array>\n\
{intervals}\t</array>\n\
\t<key>ProcessType</key>\n\
\t<string>Background</string>\n\
</dict>\n\
</plist>\n",
        id = xml_escape(&task.id),
    )
}

const SYSTEMD_DAYS: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];

/// Quote one `ExecStart=` word: double quotes, with `\`, `"`, `%` and `$` escaped.
fn systemd_quote(arg: &str) -> String {
    let mut out = String::from('"');
    for c in arg.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '%' => out.push_str("%%"),
            '$' => out.push_str("$$"),
            '\n' => out.push_str("\\n"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// The `.service` unit of `task` (run once per timer firing).
pub fn systemd_service(task: &TaskSpec) -> String {
    let exec: Vec<String> = std::iter::once(task.program.to_string_lossy().into_owned())
        .chain(task.args.clone())
        .map(|a| systemd_quote(&a))
        .collect();
    let title: String = task.title.chars().filter(|c| !c.is_control()).collect();
    format!(
        "[Unit]\nDescription={title} (claude-sync group job {id})\n\n[Service]\nType=oneshot\nExecStart={exec}\nTimeoutStartSec={secs}\n",
        id = task.id,
        exec = exec.join(" "),
        secs = (task.timeout_minutes + OS_GRACE_MINUTES) * 60,
    )
}

/// The `.timer` unit: one `OnCalendar=` per weekday set and time. `Persistent=true` runs a
/// missed firing when the machine comes back.
pub fn systemd_timer(task: &TaskSpec) -> String {
    let days = match &task.days {
        None => String::new(),
        Some(d) => {
            let names: Vec<&str> = d.iter().map(|&n| SYSTEMD_DAYS[(n % 7) as usize]).collect();
            format!("{} ", names.join(","))
        }
    };
    let mut lines = String::new();
    for (h, m) in &task.times {
        lines.push_str(&format!("OnCalendar={days}*-*-* {h:02}:{m:02}:00\n"));
    }
    format!(
        "[Unit]\nDescription={} (claude-sync group job {})\n\n[Timer]\n{lines}Persistent=true\n\n[Install]\nWantedBy=timers.target\n",
        task.title.chars().filter(|c| !c.is_control()).collect::<String>(),
        task.id,
    )
}

/// What an OS tool answered.
pub struct Out {
    pub ok: bool,
    pub stdout: String,
    pub stderr: String,
}

/// Runs an OS tool. Tests capture the calls instead.
pub trait Runner {
    fn run(&self, program: &str, args: &[String], stdin: Option<&[u8]>) -> std::io::Result<Out>;
}

pub struct SystemRunner;

impl Runner for SystemRunner {
    fn run(&self, program: &str, args: &[String], stdin: Option<&[u8]>) -> std::io::Result<Out> {
        use std::io::Write;
        use std::process::{Command, Stdio};
        let mut c = Command::new(program);
        crate::quiet(&mut c);
        c.args(args).stdout(Stdio::piped()).stderr(Stdio::piped());
        c.stdin(if stdin.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        });
        let mut child = c.spawn()?;
        if let (Some(b), Some(mut i)) = (stdin, child.stdin.take()) {
            i.write_all(b)?;
        }
        let o = child.wait_with_output()?;
        Ok(Out {
            ok: o.status.success(),
            stdout: String::from_utf8_lossy(&o.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&o.stderr).into_owned(),
        })
    }
}

fn s(v: &[&str]) -> Vec<String> {
    v.iter().map(|x| x.to_string()).collect()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Kind {
    /// Task Scheduler folder `\claude-sync\`; `temp` holds the XML handed to `schtasks`; `user` is `DOMAIN\name`.
    Windows {
        temp: PathBuf,
        user: String,
    },
    Launchd {
        agents_dir: PathBuf,
        uid: String,
    },
    Systemd {
        units_dir: PathBuf,
    },
    /// No systemd: one crontab line per job.
    Cron,
}

pub struct Tasks<'a> {
    pub kind: Kind,
    pub run: &'a dyn Runner,
}

fn fail(what: &str, o: &Out) -> String {
    let m = [o.stderr.trim(), o.stdout.trim()]
        .into_iter()
        .find(|x| !x.is_empty())
        .unwrap_or("no output");
    format!("{what} failed: {m}")
}

fn cron_word(a: &str) -> String {
    // `%` starts stdin in a crontab line; a word with anything but plain characters is single-quoted.
    let a = a.replace('%', "\\%");
    if !a.is_empty()
        && a.chars()
            .all(|c| c.is_ascii_alphanumeric() || "/._-:=,+@\\%".contains(c))
    {
        a
    } else {
        format!("'{}'", a.replace('\'', "'\\''"))
    }
}

/// The crontab lines of a job: one per time, ending with its marker.
pub fn cron_lines(t: &TaskSpec) -> Vec<String> {
    let days = t.days.as_ref().map_or("*".to_string(), |d| {
        d.iter()
            .map(|x| x.to_string())
            .collect::<Vec<_>>()
            .join(",")
    });
    let cmd: Vec<String> = std::iter::once(t.program.to_string_lossy().into_owned())
        .chain(t.args.clone())
        .map(|a| cron_word(&a))
        .collect();
    t.times
        .iter()
        .map(|(h, m)| {
            format!(
                "{m} {h} * * {days} {} # {SYSTEMD_PREFIX}{}",
                cmd.join(" "),
                t.id
            )
        })
        .collect()
}

impl Tasks<'_> {
    fn go(&self, p: &str, a: &[&str], stdin: Option<&[u8]>) -> Result<Out, String> {
        self.run
            .run(p, &s(a), stdin)
            .map_err(|e| format!("cannot run {p}: {e}"))
    }
    fn ok(&self, what: &str, p: &str, a: &[&str]) -> Result<(), String> {
        let o = self.go(p, a, None)?;
        if o.ok {
            Ok(())
        } else {
            Err(fail(what, &o))
        }
    }

    fn crontab_read(&self) -> Result<String, String> {
        let o = self.go("crontab", &["-l"], None)?;
        if o.ok {
            Ok(o.stdout)
        } else if o.stderr.to_lowercase().contains("no crontab") {
            Ok(String::new())
        } else {
            Err(fail("crontab -l", &o)) // never turned into an empty table
        }
    }
    fn crontab_without(table: &str, id: Option<&str>) -> String {
        let end = id.map(|i| format!("# {SYSTEMD_PREFIX}{i}"));
        let mut out: String = table
            .lines()
            .filter(|l| match &end {
                Some(e) => !l.ends_with(e.as_str()),
                None => !l.contains(&format!("# {SYSTEMD_PREFIX}")),
            })
            .map(|l| format!("{l}\n"))
            .collect();
        if out.trim().is_empty() {
            out.clear();
        }
        out
    }

    pub fn register(&self, t: &TaskSpec) -> Result<(), String> {
        match &self.kind {
            Kind::Windows { temp, user } => {
                let f = temp.join(format!(
                    "claude-sync-task-{}-{}.xml",
                    std::process::id(),
                    t.id
                ));
                write_atomic(&f, &utf16_bytes(&task_xml(t, user))).map_err(|e| e.to_string())?;
                let name = format!("{TASK_FOLDER}{}", t.id);
                let r = self.ok(
                    &format!("registering {name}"),
                    "schtasks.exe",
                    &["/Create", "/F", "/TN", &name, "/XML", &f.to_string_lossy()],
                );
                let _ = std::fs::remove_file(&f);
                r
            }
            Kind::Launchd { agents_dir, uid } => {
                let file = agents_dir.join(format!("{LAUNCHD_PREFIX}{}.plist", t.id));
                write_atomic(&file, launchd_plist(t, LAUNCHD_PREFIX).as_bytes())
                    .map_err(|e| e.to_string())?;
                let domain = format!("gui/{uid}");
                // A loaded agent keeps its old definition until it is booted out.
                let _ = self.go(
                    "/bin/launchctl",
                    &["bootout", &format!("{domain}/{LAUNCHD_PREFIX}{}", t.id)],
                    None,
                );
                self.ok(
                    "launchctl bootstrap",
                    "/bin/launchctl",
                    &["bootstrap", &domain, &file.to_string_lossy()],
                )
            }
            Kind::Systemd { units_dir } => {
                for (ext, text) in [("service", systemd_service(t)), ("timer", systemd_timer(t))] {
                    write_atomic(
                        &units_dir.join(format!("{SYSTEMD_PREFIX}{}.{ext}", t.id)),
                        text.as_bytes(),
                    )
                    .map_err(|e| e.to_string())?;
                }
                let timer = format!("{SYSTEMD_PREFIX}{}.timer", t.id);
                self.ok(
                    "systemctl daemon-reload",
                    "systemctl",
                    &["--user", "daemon-reload"],
                )?;
                self.ok(
                    "systemctl enable",
                    "systemctl",
                    &["--user", "enable", "--now", &timer],
                )?;
                self.ok(
                    "systemctl restart",
                    "systemctl",
                    &["--user", "restart", &timer],
                )
            }
            Kind::Cron => {
                let mut table = Self::crontab_without(&self.crontab_read()?, Some(&t.id));
                for l in cron_lines(t) {
                    table.push_str(&l);
                    table.push('\n');
                }
                let o = self.go("crontab", &["-"], Some(table.as_bytes()))?;
                if o.ok {
                    Ok(())
                } else {
                    Err(fail("crontab", &o))
                }
            }
        }
    }

    pub fn unregister(&self, id: &str) -> Result<(), String> {
        let gone = |r: std::io::Result<()>| match r {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.to_string()),
            _ => Ok(()),
        };
        match &self.kind {
            Kind::Windows { .. } => {
                let name = format!("{TASK_FOLDER}{id}");
                if !self.go("schtasks.exe", &["/Query", "/TN", &name], None)?.ok {
                    return Ok(());
                }
                self.ok(
                    &format!("removing {name}"),
                    "schtasks.exe",
                    &["/Delete", "/F", "/TN", &name],
                )
            }
            Kind::Launchd { agents_dir, uid } => {
                let _ = self.go(
                    "/bin/launchctl",
                    &["bootout", &format!("gui/{uid}/{LAUNCHD_PREFIX}{id}")],
                    None,
                );
                gone(std::fs::remove_file(
                    agents_dir.join(format!("{LAUNCHD_PREFIX}{id}.plist")),
                ))
            }
            Kind::Systemd { units_dir } => {
                let _ = self.go(
                    "systemctl",
                    &[
                        "--user",
                        "disable",
                        "--now",
                        &format!("{SYSTEMD_PREFIX}{id}.timer"),
                    ],
                    None,
                );
                for ext in ["timer", "service"] {
                    gone(std::fs::remove_file(
                        units_dir.join(format!("{SYSTEMD_PREFIX}{id}.{ext}")),
                    ))?;
                }
                let _ = self.go("systemctl", &["--user", "daemon-reload"], None);
                Ok(())
            }
            Kind::Cron => {
                let t = self.crontab_read()?;
                let n = Self::crontab_without(&t, Some(id));
                if n == t {
                    return Ok(());
                }
                let o = self.go("crontab", &["-"], Some(n.as_bytes()))?;
                if o.ok {
                    Ok(())
                } else {
                    Err(fail("crontab", &o))
                }
            }
        }
    }

    pub fn exists(&self, id: &str) -> Result<bool, String> {
        Ok(match &self.kind {
            Kind::Windows { .. } => {
                self.go(
                    "schtasks.exe",
                    &["/Query", "/TN", &format!("{TASK_FOLDER}{id}")],
                    None,
                )?
                .ok
            }
            Kind::Launchd { agents_dir, .. } => agents_dir
                .join(format!("{LAUNCHD_PREFIX}{id}.plist"))
                .is_file(),
            Kind::Systemd { units_dir } => ["timer", "service"].iter().all(|e| {
                units_dir
                    .join(format!("{SYSTEMD_PREFIX}{id}.{e}"))
                    .is_file()
            }),
            Kind::Cron => self
                .crontab_read()?
                .lines()
                .any(|l| l.ends_with(&format!("# {SYSTEMD_PREFIX}{id}"))),
        })
    }
}

/// What `reconcile` did.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Reconciled {
    pub registered: Vec<String>,
    pub unregistered: Vec<String>,
    pub errors: Vec<String>,
}

/// `have`: id to the spec last registered (kept by the caller in the store). A job is
/// (re-)registered when it is new, its spec changed (the exe moved, a time changed) or `verify`
/// finds the OS task gone; a job that is already right is never touched, so it never runs
/// twice. Jobs no longer wanted are removed. Returns the new `have`.
pub fn reconcile(
    t: &Tasks,
    want: &[TaskSpec],
    have: &BTreeMap<String, TaskSpec>,
    verify: bool,
) -> (BTreeMap<String, TaskSpec>, Reconciled) {
    let mut r = Reconciled::default();
    let mut now = have.clone();
    for id in have
        .keys()
        .filter(|i| !want.iter().any(|w| &w.id == *i))
        .cloned()
        .collect::<Vec<_>>()
    {
        match t.unregister(&id) {
            Ok(()) => {
                now.remove(&id);
                r.unregistered.push(id);
            }
            Err(e) => r.errors.push(format!("{id}: {e}")),
        }
    }
    for w in want {
        let same = have.get(&w.id) == Some(w);
        if same && !(verify && !t.exists(&w.id).unwrap_or(false)) {
            continue;
        }
        match t.register(w) {
            Ok(()) => {
                now.insert(w.id.clone(), w.clone());
                r.registered.push(w.id.clone());
            }
            Err(e) => r.errors.push(format!("{}: {e}", w.id)),
        }
    }
    (now, r)
}

/// Uninstall: every task of the given ids, then (cron) every marked line.
pub fn remove_all(t: &Tasks, ids: &[String]) -> Vec<String> {
    let mut errs: Vec<String> = ids
        .iter()
        .filter_map(|i| t.unregister(i).err().map(|e| format!("{i}: {e}")))
        .collect();
    if t.kind == Kind::Cron {
        match t.crontab_read() {
            Ok(tb) => {
                let n = Tasks::crontab_without(&tb, None);
                if n != tb {
                    if let Err(e) = t.go("crontab", &["-"], Some(n.as_bytes())) {
                        errs.push(e);
                    }
                }
            }
            Err(e) => errs.push(e),
        }
    }
    errs
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    #[derive(Default)]
    struct Cap(RefCell<Vec<String>>, RefCell<String>);
    impl Runner for Cap {
        fn run(&self, p: &str, a: &[String], stdin: Option<&[u8]>) -> std::io::Result<Out> {
            self.0.borrow_mut().push(format!("{p} {}", a.join(" ")));
            if let Some(b) = stdin {
                *self.1.borrow_mut() = String::from_utf8_lossy(b).into_owned();
            }
            let miss = p == "crontab" && a == ["-l"];
            Ok(Out {
                ok: !miss,
                stdout: String::new(),
                stderr: if miss {
                    "no crontab for u".into()
                } else {
                    String::new()
                },
            })
        }
    }

    fn spec(id: &str) -> TaskSpec {
        TaskSpec::new(id, "Nightly", "17 10 * * 1,4", Path::new("/opt/cs"), 20).unwrap()
    }

    #[test]
    fn parses_and_describes() {
        let s = Schedule::parse("17 10 * * 1,4").unwrap();
        assert_eq!(s.describe(), "Mon and Thu at 10:17");
        assert!(Schedule::parse("* * * * *").is_err());
        assert!(Schedule::parse("0 12 1 * *").is_err());
    }

    #[test]
    fn texts_hold_the_exact_fields() {
        let t = spec("a1");
        let x = task_xml(&t, "PC\\me");
        assert!(
            x.contains("<Monday />") && x.contains("2026-01-01T10:17:00") && x.contains("PT30M")
        );
        assert_eq!(utf16_bytes("a")[..2], [0xFF, 0xFE]);
        let p = launchd_plist(&t, LAUNCHD_PREFIX);
        assert!(p.contains("dev.claude-sync.job.a1") && p.contains("<key>Weekday</key>"));
        assert!(systemd_timer(&t).contains("OnCalendar=Mon,Thu *-*-* 10:17:00"));
        assert!(systemd_service(&t).contains("TimeoutStartSec=1800"));
        assert_eq!(
            cron_lines(&t),
            ["17 10 * * 1,4 /opt/cs run-job a1 # claude-sync-job-a1"]
        );
    }

    #[test]
    fn reconcile_registers_once_moves_and_removes() {
        let cap = Cap::default();
        let dir = tempfile::tempdir().unwrap();
        let t = Tasks {
            kind: Kind::Launchd {
                agents_dir: dir.path().into(),
                uid: "501".into(),
            },
            run: &cap,
        };
        let (have, r) = reconcile(&t, &[spec("a1")], &BTreeMap::new(), true);
        assert_eq!(r.registered, ["a1"]);
        assert!(cap
            .0
            .borrow()
            .iter()
            .any(|c| c.starts_with("/bin/launchctl bootstrap gui/501")));
        // unchanged and present: nothing runs
        cap.0.borrow_mut().clear();
        let (have, r) = reconcile(&t, &[spec("a1")], &have, true);
        assert_eq!(r, Reconciled::default());
        assert!(cap.0.borrow().is_empty());
        // the exe moved: registered again, once
        let mut moved = spec("a1");
        moved.program = "/new/cs".into();
        let (have, r) = reconcile(&t, &[moved], &have, false);
        assert_eq!(r.registered, ["a1"]);
        // gone from the want list: removed, plist deleted
        let (have, r) = reconcile(&t, &[], &have, false);
        assert_eq!(r.unregistered, ["a1"]);
        assert!(have.is_empty() && !dir.path().join("dev.claude-sync.job.a1.plist").exists());
    }

    #[test]
    fn cron_keeps_foreign_lines_and_remove_all_takes_only_ours() {
        let cap = Cap::default();
        let t = Tasks {
            kind: Kind::Cron,
            run: &cap,
        };
        t.register(&spec("a1")).unwrap();
        assert!(cap.1.borrow().ends_with("# claude-sync-job-a1\n"));
        assert_eq!(
            Tasks::crontab_without("0 1 * * * mine\n17 10 * * 1 x # claude-sync-job-a1\n", None),
            "0 1 * * * mine\n"
        );
    }
}
