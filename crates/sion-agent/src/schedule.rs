//! When a scheduled job runs: once at a time, every interval, or on a cron
//! expression. Times are Unix seconds, and calendars are UTC.
//!
//! Cron expressions have the five usual fields, minute, hour, day of the
//! month, month and day of the week, each `*`, a number, a range `a-b`, a
//! step `*/n` or `a-b/n`, or a list of these separated by commas. Months
//! and days may be named (`jan`, `mon`), Sunday is 0 or 7, and `@hourly`,
//! `@daily`, `@weekly`, `@monthly` and `@yearly` stand for the usual
//! expressions. As in Vixie cron, when both the day of the month and the
//! day of the week are restricted, a day matches if either does.

use serde::{Deserialize, Serialize};

/// When a job runs.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum When {
    /// Once, at this time.
    At(u64),
    /// Every `seconds`, the first time `seconds` after `from`.
    Every {
        /// The interval.
        seconds: u64,
        /// When the schedule was made.
        from: u64,
    },
    /// On a cron expression.
    Cron(String),
}

impl When {
    /// The first time after `after` at which the job runs, if any.
    pub fn next_after(&self, after: u64) -> Option<u64> {
        match self {
            When::At(at) => (*at > after).then_some(*at),
            When::Every { seconds, from } => {
                if *seconds == 0 {
                    return None;
                }
                if after < *from {
                    return Some(from + seconds);
                }
                let periods = (after - from) / seconds + 1;
                Some(from + periods * seconds)
            }
            When::Cron(text) => Cron::parse(text).ok()?.next_after(after),
        }
    }

    /// The schedule in words.
    pub fn describe(&self) -> String {
        match self {
            When::At(at) => format!("once, at {}", rfc3339(*at)),
            When::Every { seconds, .. } => format!("every {}", duration_text(*seconds)),
            When::Cron(text) => format!("on cron {text:?} (UTC)"),
        }
    }
}

/// A parsed cron expression.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Cron {
    minutes: Vec<bool>,
    hours: Vec<bool>,
    days: Vec<bool>,
    months: Vec<bool>,
    weekdays: Vec<bool>,
    days_restricted: bool,
    weekdays_restricted: bool,
}

const MONTHS: [&str; 12] = [
    "jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec",
];
const WEEKDAYS: [&str; 7] = ["sun", "mon", "tue", "wed", "thu", "fri", "sat"];

/// One field: which values in `min..=max` it allows, and whether it is
/// restricted (not `*`).
fn field(
    text: &str,
    min: u32,
    max: u32,
    names: &[&str],
    name_base: u32,
) -> Result<(Vec<bool>, bool), String> {
    let mut allowed = vec![false; max as usize + 1];
    let value = |part: &str| -> Result<u32, String> {
        let lower = part.to_ascii_lowercase();
        if let Some(i) = names.iter().position(|n| *n == lower) {
            return Ok(i as u32 + name_base);
        }
        part.parse::<u32>()
            .map_err(|_| format!("{part:?} is not a number in a cron field"))
    };
    for item in text.split(',') {
        let (range, step) = match item.split_once('/') {
            Some((range, step)) => (
                range,
                step.parse::<u32>()
                    .ok()
                    .filter(|s| *s > 0)
                    .ok_or_else(|| format!("{step:?} is not a step"))?,
            ),
            None => (item, 1),
        };
        let (low, high) = if range == "*" {
            (min, max)
        } else if let Some((a, b)) = range.split_once('-') {
            (value(a)?, value(b)?)
        } else {
            let v = value(range)?;
            // `a/n` runs from a to the end of the range.
            (v, if item.contains('/') { max } else { v })
        };
        if low < min || high > max || low > high {
            return Err(format!("{item:?} is outside {min} to {max}"));
        }
        let mut v = low;
        while v <= high {
            allowed[v as usize] = true;
            v += step;
        }
    }
    Ok((allowed, text != "*"))
}

impl Cron {
    /// Reads a cron expression.
    pub fn parse(text: &str) -> Result<Cron, String> {
        let expanded = match text.trim() {
            "@hourly" => "0 * * * *",
            "@daily" | "@midnight" => "0 0 * * *",
            "@weekly" => "0 0 * * 0",
            "@monthly" => "0 0 1 * *",
            "@yearly" | "@annually" => "0 0 1 1 *",
            other => other,
        };
        let fields: Vec<&str> = expanded.split_whitespace().collect();
        let [minute, hour, day, month, weekday] = fields.as_slice() else {
            return Err(format!(
                "a cron expression has five fields (minute hour day month weekday), not {:?}",
                text
            ));
        };
        let (minutes, _) = field(minute, 0, 59, &[], 0)?;
        let (hours, _) = field(hour, 0, 23, &[], 0)?;
        let (days, days_restricted) = field(day, 1, 31, &[], 0)?;
        let (months, _) = field(month, 1, 12, &MONTHS, 1)?;
        let (mut weekdays, weekdays_restricted) = field(weekday, 0, 7, &WEEKDAYS, 0)?;
        if weekdays[7] {
            weekdays[0] = true;
        }
        weekdays.truncate(7);
        Ok(Cron {
            minutes,
            hours,
            days,
            months,
            weekdays,
            days_restricted,
            weekdays_restricted,
        })
    }

    fn day_matches(&self, day: u32, weekday: u32) -> bool {
        let by_day = self.days[day as usize];
        let by_weekday = self.weekdays[weekday as usize];
        match (self.days_restricted, self.weekdays_restricted) {
            (true, true) => by_day || by_weekday,
            (true, false) => by_day,
            (false, true) => by_weekday,
            (false, false) => true,
        }
    }

    /// The first minute after `after` that the expression matches, within
    /// eight years; None if there is none (such as February 30).
    pub fn next_after(&self, after: u64) -> Option<u64> {
        let mut t = (after / 60 + 1) * 60;
        let end = after.saturating_add(8 * 366 * 86_400);
        while t <= end {
            let days = t / 86_400;
            let (year, month, day) = civil(days);
            if !self.months[month as usize] {
                // To the first day of the next month.
                let (y, m) = if month == 12 {
                    (year + 1, 1)
                } else {
                    (year, month + 1)
                };
                t = days_from_civil(y, m, 1) * 86_400;
                continue;
            }
            let weekday = ((days + 4) % 7) as u32;
            if !self.day_matches(day, weekday) {
                t = (days + 1) * 86_400;
                continue;
            }
            let hour = ((t % 86_400) / 3_600) as usize;
            if !self.hours[hour] {
                t = (t / 3_600 + 1) * 3_600;
                continue;
            }
            let minute = ((t % 3_600) / 60) as usize;
            if !self.minutes[minute] {
                t += 60;
                continue;
            }
            return Some(t);
        }
        None
    }
}

/// The civil date (year, month 1-12, day 1-31) of a day since 1970-01-01.
pub fn civil(days: u64) -> (i64, u32, u32) {
    // Howard Hinnant's algorithm.
    let z = days as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// The day since 1970-01-01 of a civil date.
pub fn days_from_civil(year: i64, month: u32, day: u32) -> u64 {
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let m = month as i64;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + day as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    (era * 146_097 + doe - 719_468).max(0) as u64
}

/// A time as RFC 3339, in UTC: `2026-11-02T09:00:00Z`.
pub fn rfc3339(t: u64) -> String {
    let (year, month, day) = civil(t / 86_400);
    let s = t % 86_400;
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        s / 3_600,
        (s % 3_600) / 60,
        s % 60
    )
}

/// Reads an RFC 3339 time in UTC (`Z` or `+00:00`), with or without
/// seconds.
pub fn parse_rfc3339(text: &str) -> Result<u64, String> {
    let bad = || format!("{text:?} is not a time like 2026-11-02T09:00:00Z (UTC)");
    let rest = text
        .strip_suffix('Z')
        .or_else(|| text.strip_suffix("+00:00"))
        .ok_or_else(bad)?;
    let (date, time) = rest.split_once('T').ok_or_else(bad)?;
    let numbers = |s: &str, sep: char| -> Result<Vec<u64>, String> {
        s.split(sep)
            .map(|p| p.parse::<u64>().map_err(|_| bad()))
            .collect()
    };
    let date = numbers(date, '-')?;
    let time = numbers(time, ':')?;
    let ([year, month, day], [hour, minute, rest @ ..]) = (date.as_slice(), time.as_slice()) else {
        return Err(bad());
    };
    let second = match rest {
        [] => 0,
        [s] => *s,
        _ => return Err(bad()),
    };
    if !(1..=12).contains(month)
        || !(1..=31).contains(day)
        || *hour > 23
        || *minute > 59
        || second > 60
    {
        return Err(bad());
    }
    let days = days_from_civil(*year as i64, *month as u32, *day as u32);
    Ok(days * 86_400 + hour * 3_600 + minute * 60 + second)
}

/// Reads a duration: a number and a unit, `s`, `m`, `h` or `d` (`90s`,
/// `15m`, `2h`, `1d`).
pub fn parse_duration(text: &str) -> Result<u64, String> {
    let text = text.trim();
    let split = text
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(text.len());
    let (number, unit) = text.split_at(split);
    let n: u64 = number
        .parse()
        .map_err(|_| format!("{text:?} is not a duration like 15m"))?;
    let unit = match unit {
        "s" => 1,
        "m" => 60,
        "h" => 3_600,
        "d" => 86_400,
        _ => return Err(format!("{text:?}: the unit is s, m, h or d")),
    };
    match n.checked_mul(unit) {
        Some(0) | None => Err(format!("{text:?} is not a duration of at least a second")),
        Some(seconds) => Ok(seconds),
    }
}

/// A duration in words: `15m`, `2h`, `1d`, `90s`.
pub fn duration_text(seconds: u64) -> String {
    if seconds % 86_400 == 0 {
        format!("{}d", seconds / 86_400)
    } else if seconds % 3_600 == 0 {
        format!("{}h", seconds / 3_600)
    } else if seconds % 60 == 0 {
        format!("{}m", seconds / 60)
    } else {
        format!("{seconds}s")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(text: &str) -> u64 {
        parse_rfc3339(text).unwrap()
    }

    #[test]
    fn dates_convert_both_ways() {
        assert_eq!(civil(0), (1970, 1, 1));
        assert_eq!(days_from_civil(2026, 10, 29), 20_755);
        assert_eq!(civil(20_755), (2026, 10, 29));
        assert_eq!(civil(days_from_civil(2028, 2, 29)), (2028, 2, 29));
        assert_eq!(rfc3339(t("2026-11-02T09:05:07Z")), "2026-11-02T09:05:07Z");
        assert_eq!(t("2026-11-02T09:05+00:00"), t("2026-11-02T09:05:00Z"));
        assert!(parse_rfc3339("2026-11-02 09:05").is_err());
        assert!(parse_rfc3339("2026-13-02T09:05Z").is_err());
    }

    #[test]
    fn cron_finds_the_next_matching_minute() {
        // Thursday 29 October 2026, 10:17:30.
        let now = t("2026-10-29T10:17:30Z");
        let next = |expr: &str| rfc3339(Cron::parse(expr).unwrap().next_after(now).unwrap());
        assert_eq!(next("* * * * *"), "2026-10-29T10:18:00Z");
        assert_eq!(next("*/15 * * * *"), "2026-10-29T10:30:00Z");
        assert_eq!(next("0 9 * * *"), "2026-10-30T09:00:00Z");
        assert_eq!(next("0 9 * * mon-fri"), "2026-10-30T09:00:00Z");
        assert_eq!(next("0 9 * * sat,sun"), "2026-10-31T09:00:00Z");
        assert_eq!(next("30 8 1 * *"), "2026-11-01T08:30:00Z");
        assert_eq!(next("@monthly"), "2026-11-01T00:00:00Z");
        assert_eq!(next("@yearly"), "2027-01-01T00:00:00Z");
        assert_eq!(next("0 0 29 2 *"), "2028-02-29T00:00:00Z");
        assert_eq!(next("0 12 * * 7"), "2026-11-01T12:00:00Z");
        // Both day fields restricted: either matches (the 13th, or a Friday).
        assert_eq!(next("0 0 13 * fri"), "2026-10-30T00:00:00Z");
        assert_eq!(Cron::parse("0 0 30 2 *").unwrap().next_after(now), None);
        for bad in [
            "* * * *",
            "60 * * * *",
            "* * 0 * *",
            "*/0 * * * *",
            "5-1 * * * *",
            "x * * * *",
        ] {
            assert!(Cron::parse(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn intervals_and_single_runs() {
        let every = When::Every {
            seconds: 900,
            from: 1_000,
        };
        assert_eq!(every.next_after(0), Some(1_900));
        assert_eq!(every.next_after(1_900), Some(2_800));
        assert_eq!(every.next_after(2_000), Some(2_800));
        assert_eq!(When::At(5_000).next_after(4_999), Some(5_000));
        assert_eq!(When::At(5_000).next_after(5_000), None);
        assert_eq!(parse_duration("15m").unwrap(), 900);
        assert_eq!(parse_duration("2h").unwrap(), 7_200);
        assert!(parse_duration("0s").is_err());
        assert!(parse_duration("15 minutes").is_err());
        assert_eq!(duration_text(900), "15m");
        assert_eq!(
            When::Cron("0 9 * * mon-fri".into()).describe(),
            "on cron \"0 9 * * mon-fri\" (UTC)"
        );
    }
}
