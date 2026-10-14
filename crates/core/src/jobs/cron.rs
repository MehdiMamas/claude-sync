//! The cron subset a job may use (§16): exact minutes and hours (numbers or comma lists),
//! `*` for day of month and month, and `*` or 0-6 lists and ranges for the day of week.
//! That is what both Task Scheduler and launchd can express without surprises.

/// The most (hour, minute) times one job may have. Task Scheduler takes 48 triggers per task.
pub const MAX_TIMES: usize = 48;

const DAY_NAMES: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_describes() {
        let s = Schedule::parse("17 10 * * 1,4").unwrap();
        assert_eq!(s.times(), [(10, 17)]);
        assert_eq!(s.days, Some(vec![1, 4]));
        assert_eq!(s.describe(), "Mon and Thu at 10:17");
        let cases = [
            ("0 9 * * 1-5", "Weekdays at 09:00"),
            (
                "30,0 18,8 * * *",
                "Every day at 08:00, 08:30, 18:00 and 18:30",
            ),
            ("5 7 * * 0-6", "Every day at 07:05"),
            ("0 12 * * 6,0", "Weekends at 12:00"),
            ("0 12 * * 0,3,5", "Wed, Fri and Sun at 12:00"),
            ("  1   2 * * 3 ", "Wed at 02:01"),
        ];
        for (expr, want) in cases {
            assert_eq!(Schedule::parse(expr).unwrap().describe(), want, "{expr}");
        }
    }

    #[test]
    fn rejects_what_cannot_be_mapped() {
        for bad in [
            "",
            "17 10 * *",
            "17 10 * * 1 x",
            "* 10 * * *",
            "17 * * * *",
            "*/5 10 * * *",
            "0-30 10 * * *",
            "60 10 * * *",
            "0 24 * * *",
            "0 10 1 * *",
            "0 10 * 1 *",
            "0 10 * * 7",
            "0 10 * * mon",
            "0 10 * * 5-1",
            "0 10 * * */2",
            "0 10 * * 1,",
            "+1 10 * * *",
            "0,1,2,3,4,5,6,7,8,9,10,11,12,13,14,15,16,17,18,19,20,21,22,23,24 0,1 * * *",
        ] {
            assert!(Schedule::parse(bad).is_err(), "{bad:?}");
        }
        assert!(Schedule::parse(
            "0,30 0,1,2,3,4,5,6,7,8,9,10,11,12,13,14,15,16,17,18,19,20,21,22,23 * * *"
        )
        .is_ok());
    }
}
