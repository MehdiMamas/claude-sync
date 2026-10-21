//! The OS scheduler a job is registered with (§17): Task Scheduler on Windows, a launchd
//! agent on macOS, a systemd user timer on Linux. Behind [`Scheduler`] so the rest is tested with a fake; normal tests never
//! register real tasks.
use super::cron::Schedule;
use crate::claude_cli;
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// The Task Scheduler folder every job lives in.
pub const TASK_FOLDER: &str = r"\claude-sync\";
/// launchd labels are `dev.claude-sync.job.<id>`.
pub const LAUNCHD_PREFIX: &str = "dev.claude-sync.job.";
/// Debug builds of the app use these, so a dev run never replaces or removes the tasks of
/// the installed app.
pub const TASK_FOLDER_DEV: &str = r"\claude-sync-dev\";
pub const LAUNCHD_PREFIX_DEV: &str = "dev.claude-sync.dev-job.";
/// systemd user units are `claude-sync-job-<id>.service` and `.timer`.
pub const SYSTEMD_PREFIX: &str = "claude-sync-job-";
pub const SYSTEMD_PREFIX_DEV: &str = "claude-sync-dev-job-";
/// The task may run this much longer than the job's own timeout before the OS stops it.
pub const OS_GRACE_MINUTES: u32 = 10;

/// What a job registers: run `program args` on `schedule`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
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
        schedule: &Schedule,
        program: &Path,
        args: Vec<String>,
        timeout_minutes: u32,
    ) -> Self {
        Self {
            id: id.into(),
            title: title.into(),
            times: schedule.times(),
            days: schedule.days.clone(),
            program: program.into(),
            args,
            timeout_minutes,
        }
    }

    /// Changes whenever the registered task would.
    pub fn hash(&self) -> String {
        crate::apply::sha256_hex(&serde_json::to_vec(self).expect("serializes"))
    }
}

pub trait Scheduler: Send + Sync {
    /// `Err(reason)` when this OS has no scheduler support yet.
    fn supported(&self) -> Result<(), String> {
        Ok(())
    }
    /// Create or replace the task.
    fn register(&self, task: &TaskSpec) -> crate::Result<()>;
    /// Remove the task; a missing task is fine.
    fn unregister(&self, id: &str) -> crate::Result<()>;
    fn exists(&self, id: &str) -> crate::Result<bool>;
}

/// The scheduler of this OS; `dev` (debug builds) uses its own folder and labels.
pub fn system(home: &Path, dev: bool) -> Box<dyn Scheduler> {
    if cfg!(windows) {
        Box::new(TaskScheduler::new(if dev {
            TASK_FOLDER_DEV
        } else {
            TASK_FOLDER
        }))
    } else if cfg!(target_os = "macos") {
        Box::new(Launchd {
            agents_dir: home.join("Library").join("LaunchAgents"),
            prefix: if dev {
                LAUNCHD_PREFIX_DEV
            } else {
                LAUNCHD_PREFIX
            },
        })
    } else if cfg!(target_os = "linux") {
        Box::new(SystemdUser {
            units_dir: systemd_units_dir(home),
            prefix: if dev {
                SYSTEMD_PREFIX_DEV
            } else {
                SYSTEMD_PREFIX
            },
        })
    } else {
        Box::new(Unsupported)
    }
}

/// `$XDG_CONFIG_HOME/systemd/user`, else `~/.config/systemd/user`.
fn systemd_units_dir(home: &Path) -> PathBuf {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(|| home.join(".config"));
    base.join("systemd").join("user")
}

pub struct Unsupported;

impl Unsupported {
    const WHY: &'static str = "Scheduled jobs are not supported on this OS yet";
}

impl Scheduler for Unsupported {
    fn supported(&self) -> Result<(), String> {
        Err(Self::WHY.into())
    }
    fn register(&self, _: &TaskSpec) -> crate::Result<()> {
        Err(crate::Error::Invalid(Self::WHY.into()))
    }
    fn unregister(&self, _: &str) -> crate::Result<()> {
        Ok(())
    }
    fn exists(&self, _: &str) -> crate::Result<bool> {
        Ok(false)
    }
}

/// Escape element text (never used in attributes, so quotes stay as they are).
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

/// `schtasks.exe` under one Task Scheduler folder (`\claude-sync\`; tests use their own).
pub struct TaskScheduler {
    folder: String,
}

impl TaskScheduler {
    pub fn new(folder: &str) -> Self {
        Self {
            folder: folder.into(),
        }
    }

    pub fn task_name(&self, id: &str) -> String {
        format!("{}{id}", self.folder)
    }

    fn schtasks(&self, args: &[&str]) -> crate::Result<claude_cli::Output> {
        Ok(claude_cli::run(
            Path::new("schtasks.exe"),
            args,
            Duration::from_secs(60),
        )?)
    }

    fn user() -> String {
        match (std::env::var("USERDOMAIN"), std::env::var("USERNAME")) {
            (Ok(d), Ok(u)) if !d.is_empty() && !u.is_empty() => format!("{d}\\{u}"),
            (_, Ok(u)) => u,
            _ => String::new(),
        }
    }
}

fn failed(what: &str, out: &claude_cli::Output) -> crate::Error {
    let msg = [out.stderr.trim(), out.stdout.trim()]
        .into_iter()
        .find(|s| !s.is_empty())
        .unwrap_or("no output");
    crate::Error::Invalid(format!("{what} failed: {msg}"))
}

impl Scheduler for TaskScheduler {
    fn register(&self, task: &TaskSpec) -> crate::Result<()> {
        let file = std::env::temp_dir().join(format!(
            "claude-sync-task-{}-{}.xml",
            std::process::id(),
            task.id
        ));
        std::fs::write(&file, utf16_bytes(&task_xml(task, &Self::user())))?;
        let name = self.task_name(&task.id);
        let out = self.schtasks(&[
            "/Create",
            "/F",
            "/TN",
            &name,
            "/XML",
            &file.to_string_lossy(),
        ]);
        let _ = std::fs::remove_file(&file);
        let out = out?;
        if !out.ok() {
            return Err(failed(&format!("registering {name}"), &out));
        }
        Ok(())
    }

    fn unregister(&self, id: &str) -> crate::Result<()> {
        if !self.exists(id)? {
            return Ok(());
        }
        let name = self.task_name(id);
        let out = self.schtasks(&["/Delete", "/F", "/TN", &name])?;
        if !out.ok() {
            return Err(failed(&format!("removing {name}"), &out));
        }
        Ok(())
    }

    fn exists(&self, id: &str) -> crate::Result<bool> {
        Ok(self.schtasks(&["/Query", "/TN", &self.task_name(id)])?.ok())
    }
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

/// launchd agents in `~/Library/LaunchAgents`, loaded with `launchctl bootstrap gui/<uid>`.
pub struct Launchd {
    pub agents_dir: PathBuf,
    /// [`LAUNCHD_PREFIX`], or [`LAUNCHD_PREFIX_DEV`].
    pub prefix: &'static str,
}

impl Launchd {
    fn plist(&self, id: &str) -> PathBuf {
        self.agents_dir.join(format!("{}{id}.plist", self.prefix))
    }

    fn domain() -> crate::Result<String> {
        let out = claude_cli::run(Path::new("/usr/bin/id"), &["-u"], Duration::from_secs(10))?;
        let uid = out.stdout.trim();
        if !out.ok() || uid.is_empty() {
            return Err(failed("id -u", &out));
        }
        Ok(format!("gui/{uid}"))
    }

    fn launchctl(args: &[&str]) -> crate::Result<claude_cli::Output> {
        Ok(claude_cli::run(
            Path::new("/bin/launchctl"),
            args,
            Duration::from_secs(60),
        )?)
    }
}

impl Scheduler for Launchd {
    fn register(&self, task: &TaskSpec) -> crate::Result<()> {
        let domain = Self::domain()?;
        let file = self.plist(&task.id);
        crate::state::write_atomic(&file, launchd_plist(task, self.prefix).as_bytes())?;
        // A loaded agent keeps its old definition until it is booted out.
        let _ = Self::launchctl(&["bootout", &format!("{domain}/{}{}", self.prefix, task.id)]);
        let out = Self::launchctl(&["bootstrap", &domain, &file.to_string_lossy()])?;
        if !out.ok() {
            return Err(failed("launchctl bootstrap", &out));
        }
        Ok(())
    }

    fn unregister(&self, id: &str) -> crate::Result<()> {
        let domain = Self::domain()?;
        let _ = Self::launchctl(&["bootout", &format!("{domain}/{}{id}", self.prefix)]);
        match std::fs::remove_file(self.plist(id)) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.into()),
            _ => Ok(()),
        }
    }

    fn exists(&self, id: &str) -> crate::Result<bool> {
        Ok(self.plist(id).is_file())
    }
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

/// systemd user units in `~/.config/systemd/user`, enabled with `systemctl --user`.
pub struct SystemdUser {
    pub units_dir: PathBuf,
    /// [`SYSTEMD_PREFIX`], or [`SYSTEMD_PREFIX_DEV`].
    pub prefix: &'static str,
}

impl SystemdUser {
    fn unit(&self, id: &str, ext: &str) -> PathBuf {
        self.units_dir.join(format!("{}{id}.{ext}", self.prefix))
    }

    fn systemctl(args: &[&str]) -> crate::Result<claude_cli::Output> {
        let mut full = vec!["--user"];
        full.extend_from_slice(args);
        Ok(claude_cli::run(
            Path::new("systemctl"),
            &full,
            Duration::from_secs(60),
        )?)
    }
}

impl Scheduler for SystemdUser {
    fn supported(&self) -> Result<(), String> {
        match Self::systemctl(&["--version"]) {
            Ok(o) if o.ok() => Ok(()),
            _ => Err("Scheduled jobs on Linux need systemd (systemctl --user)".into()),
        }
    }

    fn register(&self, task: &TaskSpec) -> crate::Result<()> {
        crate::state::write_atomic(
            &self.unit(&task.id, "service"),
            systemd_service(task).as_bytes(),
        )?;
        crate::state::write_atomic(
            &self.unit(&task.id, "timer"),
            systemd_timer(task).as_bytes(),
        )?;
        let out = Self::systemctl(&["daemon-reload"])?;
        if !out.ok() {
            return Err(failed("systemctl daemon-reload", &out));
        }
        let timer = format!("{}{}.timer", self.prefix, task.id);
        let out = Self::systemctl(&["enable", "--now", &timer])?;
        if !out.ok() {
            return Err(failed("systemctl enable", &out));
        }
        // A running timer keeps its old schedule until it is restarted.
        let out = Self::systemctl(&["restart", &timer])?;
        if !out.ok() {
            return Err(failed("systemctl restart", &out));
        }
        Ok(())
    }

    fn unregister(&self, id: &str) -> crate::Result<()> {
        let _ = Self::systemctl(&["disable", "--now", &format!("{}{id}.timer", self.prefix)]);
        for ext in ["timer", "service"] {
            match std::fs::remove_file(self.unit(id, ext)) {
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(e.into()),
                _ => {}
            }
        }
        let _ = Self::systemctl(&["daemon-reload"]);
        Ok(())
    }

    fn exists(&self, id: &str) -> crate::Result<bool> {
        Ok(self.unit(id, "timer").is_file() && self.unit(id, "service").is_file())
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    fn spec(cron: &str) -> TaskSpec {
        TaskSpec::new(
            "setup-scout",
            "Claude <setup> refresh",
            &Schedule::parse(cron).unwrap(),
            Path::new(r"C:\Users\A B\AppData\Local\claude-sync\claude-sync.exe"),
            vec!["run-job".into(), "acme/team".into(), "setup-scout".into()],
            60,
        )
    }

    #[test]
    fn systemd_units() {
        let t = TaskSpec::new(
            "setup-scout",
            "Refresh",
            &Schedule::parse("17 10 * * 1,4").unwrap(),
            Path::new("/opt/claude sync/claude-sync"),
            vec!["run-job".into(), "acme/team".into(), "a\"b%".into()],
            60,
        );
        assert_eq!(
            systemd_service(&t),
            "[Unit]\nDescription=Refresh (claude-sync group job setup-scout)\n\n[Service]\nType=oneshot\nExecStart=\"/opt/claude sync/claude-sync\" \"run-job\" \"acme/team\" \"a\\\"b%%\"\nTimeoutStartSec=4200\n"
        );
        assert!(systemd_timer(&t).contains("OnCalendar=Mon,Thu *-*-* 10:17:00\n"));
        let daily = TaskSpec { days: None, ..t };
        assert!(systemd_timer(&daily).contains("OnCalendar=*-*-* 10:17:00\n"));
    }

    #[test]
    fn windows_task_xml() {
        let want = r#"<?xml version="1.0" encoding="UTF-16"?>
<Task version="1.2" xmlns="http://schemas.microsoft.com/windows/2004/02/mit/task">
  <RegistrationInfo>
    <Author>claude-sync</Author>
    <Description>Claude &lt;setup&gt; refresh (claude-sync group job setup-scout)</Description>
  </RegistrationInfo>
  <Triggers>
    <CalendarTrigger>
      <StartBoundary>2026-01-01T10:17:00</StartBoundary>
      <Enabled>true</Enabled>
      <ScheduleByWeek>
        <DaysOfWeek>
          <Monday />
          <Thursday />
        </DaysOfWeek>
        <WeeksInterval>1</WeeksInterval>
      </ScheduleByWeek>
    </CalendarTrigger>
  </Triggers>
  <Principals>
    <Principal id="Author">
      <UserId>PC\a</UserId>
      <LogonType>InteractiveToken</LogonType>
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
    <ExecutionTimeLimit>PT70M</ExecutionTimeLimit>
    <Priority>7</Priority>
  </Settings>
  <Actions Context="Author">
    <Exec>
      <Command>"C:\Users\A B\AppData\Local\claude-sync\claude-sync.exe"</Command>
      <Arguments>run-job acme/team setup-scout</Arguments>
    </Exec>
  </Actions>
</Task>
"#;
        assert_eq!(task_xml(&spec("17 10 * * 1,4"), r"PC\a"), want);
    }

    #[test]
    fn windows_daily_triggers_one_per_time() {
        let xml = task_xml(&spec("0,30 8 * * *"), "");
        assert_eq!(xml.matches("<CalendarTrigger>").count(), 2);
        assert!(xml.contains("<StartBoundary>2026-01-01T08:30:00</StartBoundary>"));
        assert!(xml.contains(
            "      <ScheduleByDay>\n        <DaysInterval>1</DaysInterval>\n      </ScheduleByDay>\n"
        ));
        assert!(!xml.contains("<UserId>"));
        assert!(!xml.contains("ScheduleByWeek"));
    }

    #[test]
    fn utf16_has_a_bom() {
        assert_eq!(utf16_bytes("<a"), [0xFF, 0xFE, b'<', 0, b'a', 0]);
    }

    #[test]
    fn windows_quoting() {
        assert_eq!(win_quote("plain"), "plain");
        assert_eq!(win_quote("a b"), "\"a b\"");
        assert_eq!(win_quote(""), "\"\"");
        assert_eq!(win_quote(r#"say "hi""#), r#""say \"hi\"""#);
        assert_eq!(win_quote(r"C:\dir with space\"), r#""C:\dir with space\\""#);
    }

    #[test]
    fn launchd_agent() {
        let mut task = spec("17 10 * * 1,4");
        task.program = PathBuf::from("/Applications/claude-sync.app/Contents/MacOS/claude-sync");
        let want = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>
<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">
<plist version=\"1.0\">
<dict>
\t<key>Label</key>
\t<string>dev.claude-sync.job.setup-scout</string>
\t<key>ProgramArguments</key>
\t<array>
\t\t<string>/Applications/claude-sync.app/Contents/MacOS/claude-sync</string>
\t\t<string>run-job</string>
\t\t<string>acme/team</string>
\t\t<string>setup-scout</string>
\t</array>
\t<key>StartCalendarInterval</key>
\t<array>
\t\t<dict>
\t\t\t<key>Hour</key>
\t\t\t<integer>10</integer>
\t\t\t<key>Minute</key>
\t\t\t<integer>17</integer>
\t\t\t<key>Weekday</key>
\t\t\t<integer>1</integer>
\t\t</dict>
\t\t<dict>
\t\t\t<key>Hour</key>
\t\t\t<integer>10</integer>
\t\t\t<key>Minute</key>
\t\t\t<integer>17</integer>
\t\t\t<key>Weekday</key>
\t\t\t<integer>4</integer>
\t\t</dict>
\t</array>
\t<key>ProcessType</key>
\t<string>Background</string>
</dict>
</plist>
";
        assert_eq!(launchd_plist(&task, LAUNCHD_PREFIX), want);
        let daily = launchd_plist(&spec("0 8,20 * * *"), LAUNCHD_PREFIX);
        assert_eq!(daily.matches("<dict>").count(), 3);
        assert!(!daily.contains("Weekday"));
    }

    #[test]
    fn spec_hash_follows_the_task() {
        let a = spec("17 10 * * 1,4");
        let mut b = a.clone();
        assert_eq!(a.hash(), b.hash());
        b.timeout_minutes = 61;
        assert_ne!(a.hash(), b.hash());
    }

    /// Registers a real task under a throwaway folder, never `\claude-sync\`.
    /// Run with `cargo test -p claude-sync-core -- --ignored task_scheduler_round_trip`.
    #[cfg(windows)]
    #[test]
    #[ignore = "registers a real Windows scheduled task"]
    fn task_scheduler_round_trip() {
        use std::time::{SystemTime, UNIX_EPOCH};
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .subsec_nanos();
        let folder = format!(
            r"\claude-sync-test-{:08x}{:04x}\",
            nanos,
            std::process::id() & 0xffff
        );
        assert!(!folder.starts_with(TASK_FOLDER) && !folder.starts_with(TASK_FOLDER_DEV));
        let s = TaskScheduler::new(&folder);
        let mut task = spec("17 10 * * 1,4");
        task.id = "round-trip".into();
        task.program = PathBuf::from(r"C:\Windows\System32\cmd.exe");
        task.args = vec!["/c".into(), "exit".into()];
        let cleanup = || {
            let _ = s.unregister("round-trip");
            let ps = format!(
                "$s=New-Object -ComObject Schedule.Service;$s.Connect();$s.GetFolder('\\').DeleteFolder('{}',0)",
                folder.trim_matches('\\')
            );
            let _ = claude_cli::run(
                Path::new("powershell.exe"),
                &["-NoProfile", "-Command", &ps],
                Duration::from_secs(60),
            );
        };
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            s.register(&task).unwrap();
            assert!(s.exists("round-trip").unwrap());
            // Registering again replaces it.
            s.register(&task).unwrap();
            let q = claude_cli::run(
                Path::new("schtasks.exe"),
                &["/Query", "/TN", &s.task_name("round-trip"), "/XML"],
                Duration::from_secs(60),
            )
            .unwrap();
            assert!(q.ok(), "{}", q.stderr);
            assert!(q.stdout.contains("<Hidden>true</Hidden>"), "{}", q.stdout);
            assert!(q.stdout.contains("<Monday />"), "{}", q.stdout);
            s.unregister("round-trip").unwrap();
            assert!(!s.exists("round-trip").unwrap());
            s.unregister("round-trip").unwrap();
        }));
        cleanup();
        result.unwrap();
    }
}
