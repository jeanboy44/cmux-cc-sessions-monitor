//! Timezone configuration and timestamp formatting.
//! Defaults to KST (Korea Standard Time, UTC+9).

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Timezone {
    Kst,
    Utc,
    Jst,
    Est,
    Pst,
    Custom(i32), // Offset in minutes
}

impl Default for Timezone {
    fn default() -> Self {
        Timezone::Kst
    }
}

impl Timezone {
    #[allow(dead_code)]
    pub const ALL: [Timezone; 5] = [
        Timezone::Kst,
        Timezone::Utc,
        Timezone::Jst,
        Timezone::Est,
        Timezone::Pst,
    ];

    /// Parses a timezone string leniently. Defaults to KST.
    pub fn parse(s: &str) -> Self {
        let trimmed = s.trim();
        match trimmed.to_uppercase().as_str() {
            "KST" | "KOREA" | "ASIA/SEOUL" | "+09:00" | "+9" | "+0900" | "UTC+9" | "GMT+9" => Timezone::Kst,
            "UTC" | "GMT" | "Z" | "+00:00" | "+0" | "0" | "UTC+0" => Timezone::Utc,
            "JST" | "JAPAN" | "ASIA/TOKYO" => Timezone::Jst,
            "EST" | "EDT" | "AMERICA/NEW_YORK" | "-05:00" | "-5" | "-0500" | "UTC-5" => Timezone::Est,
            "PST" | "PDT" | "AMERICA/LOS_ANGELES" | "-08:00" | "-8" | "-0800" | "UTC-8" => Timezone::Pst,
            _ => {
                // Try parsing numeric offset like "+09:00" or "-05:00"
                if let Some(mins) = parse_offset_minutes(trimmed) {
                    Timezone::Custom(mins)
                } else {
                    Timezone::Kst
                }
            }
        }
    }

    /// Offset from UTC in seconds.
    pub fn offset_seconds(&self) -> i64 {
        match self {
            Timezone::Kst | Timezone::Jst => 9 * 3600,
            Timezone::Utc => 0,
            Timezone::Est => -5 * 3600,
            Timezone::Pst => -8 * 3600,
            Timezone::Custom(mins) => (*mins as i64) * 60,
        }
    }

    /// User-friendly label for display.
    pub fn label(&self) -> &'static str {
        match self {
            Timezone::Kst => "KST (UTC+9)",
            Timezone::Utc => "UTC",
            Timezone::Jst => "JST (UTC+9)",
            Timezone::Est => "EST (UTC-5)",
            Timezone::Pst => "PST (UTC-8)",
            Timezone::Custom(_) => "사용자 지정",
        }
    }

    /// Short code for config persistence.
    pub fn as_code(&self) -> &'static str {
        match self {
            Timezone::Kst => "KST",
            Timezone::Utc => "UTC",
            Timezone::Jst => "JST",
            Timezone::Est => "EST",
            Timezone::Pst => "PST",
            Timezone::Custom(_) => "Custom",
        }
    }

    /// Cycles to the next timezone in the list.
    pub fn next(&self) -> Self {
        match self {
            Timezone::Kst => Timezone::Utc,
            Timezone::Utc => Timezone::Jst,
            Timezone::Jst => Timezone::Est,
            Timezone::Est => Timezone::Pst,
            Timezone::Pst => Timezone::Kst,
            Timezone::Custom(_) => Timezone::Kst,
        }
    }

    /// Formats epoch milliseconds as `YYYY-MM-DD HH:MM:SS`.
    pub fn format_epoch_ms(&self, ms: i64) -> String {
        if ms <= 0 {
            return "-".to_string();
        }
        let total_secs = ms / 1000 + self.offset_seconds();
        let days = total_secs.div_euclid(86400);
        let rem_secs = total_secs.rem_euclid(86400);
        let (y, m, d) = days_to_ymd(days);
        let hour = rem_secs / 3600;
        let min = (rem_secs % 3600) / 60;
        let sec = rem_secs % 60;
        format!("{y:04}-{m:02}-{d:02} {hour:02}:{min:02}:{sec:02}")
    }

    /// Formats epoch milliseconds as `YYYY-MM-DD HH:MM`.
    pub fn format_epoch_ms_short(&self, ms: i64) -> String {
        if ms <= 0 {
            return "-".to_string();
        }
        let total_secs = ms / 1000 + self.offset_seconds();
        let days = total_secs.div_euclid(86400);
        let rem_secs = total_secs.rem_euclid(86400);
        let (y, m, d) = days_to_ymd(days);
        let hour = rem_secs / 3600;
        let min = (rem_secs % 3600) / 60;
        format!("{y:04}-{m:02}-{d:02} {hour:02}:{min:02}")
    }

    /// Formats epoch milliseconds as time-only `HH:MM:SS`.
    pub fn format_epoch_time(&self, ms: i64) -> String {
        if ms <= 0 {
            return "-".to_string();
        }
        let total_secs = ms / 1000 + self.offset_seconds();
        let rem_secs = total_secs.rem_euclid(86400);
        let hour = rem_secs / 3600;
        let min = (rem_secs % 3600) / 60;
        let sec = rem_secs % 60;
        format!("{hour:02}:{min:02}:{sec:02}")
    }

    /// Formats SQLite UTC datetime string (`YYYY-MM-DD HH:MM:SS` or ISO) into configured timezone.
    pub fn format_iso_utc(&self, iso: &str) -> String {
        if let Some(epoch_ms) = parse_utc_iso_to_ms(iso) {
            self.format_epoch_ms(epoch_ms)
        } else {
            iso.to_string()
        }
    }

    /// Formats SQLite UTC datetime string into `YYYY-MM-DD HH:MM`.
    pub fn format_iso_utc_short(&self, iso: &str) -> String {
        if let Some(epoch_ms) = parse_utc_iso_to_ms(iso) {
            self.format_epoch_ms_short(epoch_ms)
        } else {
            iso.chars().take(16).collect()
        }
    }
}

/// Parses an offset like "+09:00" or "-05:00" into total minutes.
fn parse_offset_minutes(s: &str) -> Option<i32> {
    let clean = s.trim_start_matches("UTC").trim_start_matches("GMT").trim();
    if clean.is_empty() {
        return None;
    }
    let sign = if clean.starts_with('-') { -1 } else { 1 };
    let num_part = clean.trim_start_matches('+').trim_start_matches('-');
    let parts: Vec<&str> = num_part.split(':').collect();
    let hours: i32 = parts.get(0)?.parse().ok()?;
    let mins: i32 = parts.get(1).and_then(|s| s.parse().ok()).unwrap_or(0);
    Some(sign * (hours * 60 + mins))
}

/// Parses a UTC ISO string (`YYYY-MM-DD HH:MM:SS` or `YYYY-MM-DDTHH:MM:SS.sssZ`) to epoch milliseconds.
fn parse_utc_iso_to_ms(s: &str) -> Option<i64> {
    let clean = s.replace('T', " ").trim_end_matches('Z').trim().to_string();
    let parts: Vec<&str> = clean.split(' ').collect();
    if parts.len() < 2 {
        return None;
    }
    let date_parts: Vec<i64> = parts[0].split('-').filter_map(|p| p.parse().ok()).collect();
    if date_parts.len() != 3 {
        return None;
    }
    let (y, m, d) = (date_parts[0], date_parts[1], date_parts[2]);

    let time_sub: Vec<&str> = parts[1].split('.').collect();
    let time_parts: Vec<i64> = time_sub[0].split(':').filter_map(|p| p.parse().ok()).collect();
    if time_parts.len() < 2 {
        return None;
    }
    let hour = time_parts[0];
    let min = time_parts[1];
    let sec = time_parts.get(2).copied().unwrap_or(0);
    let ms = if time_sub.len() > 1 {
        let frac = time_sub[1];
        let padded = format!("{:<03}", &frac[..frac.len().min(3)]);
        padded.parse::<i64>().unwrap_or(0)
    } else {
        0
    };

    let days = ymd_to_days(y, m as u32, d as u32);
    let total_secs = days * 86400 + hour * 3600 + min * 60 + sec;
    Some(total_secs * 1000 + ms)
}

/// Converts (year, month, day) to days since 1970-01-01.
fn ymd_to_days(y: i64, m: u32, d: u32) -> i64 {
    let mut total_days = (y - 1970) * 365 + (y - 1969) / 4;
    let days_in_month = [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    let is_leap = y % 4 == 0 && (y % 100 != 0 || y % 400 == 0);
    for (idx, &dm) in days_in_month.iter().enumerate() {
        if (idx as u32 + 1) < m {
            total_days += dm;
            if idx == 1 && is_leap {
                total_days += 1;
            }
        }
    }
    total_days += (d as i64) - 1;
    total_days
}

/// Howard Hinnant's fast O(1) algorithm: days since 1970-01-01 to (year, month, day).
pub fn days_to_ymd(days: i64) -> (i32, u32, u32) {
    let z = days + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = (z - era * 146097) as u32;
    let yoe = (doe - doe / 1024 + doe / 1461 - doe / 142459) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (y as i32, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_timezone_parsing() {
        assert_eq!(Timezone::parse("KST"), Timezone::Kst);
        assert_eq!(Timezone::parse("kst"), Timezone::Kst);
        assert_eq!(Timezone::parse("Asia/Seoul"), Timezone::Kst);
        assert_eq!(Timezone::parse("UTC"), Timezone::Utc);
        assert_eq!(Timezone::parse("JST"), Timezone::Jst);
        assert_eq!(Timezone::parse("EST"), Timezone::Est);
        assert_eq!(Timezone::parse("PST"), Timezone::Pst);
        assert_eq!(Timezone::parse("unknown"), Timezone::Kst); // Default
    }

    #[test]
    fn test_kst_formatting() {
        let tz = Timezone::Kst;
        // 2026-11-06 02:30:00 UTC -> 2026-11-06 11:30:00 KST
        let formatted = tz.format_iso_utc("2026-11-06 02:30:00");
        assert_eq!(formatted, "2026-11-06 11:30:00");

        let formatted_short = tz.format_iso_utc_short("2026-11-06 02:30:00");
        assert_eq!(formatted_short, "2026-11-06 11:30");
    }

    #[test]
    fn test_utc_formatting() {
        let tz = Timezone::Utc;
        let formatted = tz.format_iso_utc("2026-11-06 02:30:00");
        assert_eq!(formatted, "2026-11-06 02:30:00");
    }

    #[test]
    fn test_days_to_ymd_boundaries() {
        assert_eq!(days_to_ymd(0), (1970, 1, 1));
        assert_eq!(days_to_ymd(20763), (2026, 11, 6));
    }
}
