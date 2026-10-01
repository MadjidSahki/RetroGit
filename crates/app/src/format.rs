//! Small, dependency-free formatting helpers.

/// `YYYY-MM-DD HH:MM` in UTC for a Unix timestamp.
pub fn format_epoch(secs: i64) -> String {
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let (y, m, d) = civil_from_days(days);
    format!(
        "{y:04}-{m:02}-{d:02} {:02}:{:02}",
        rem / 3600,
        (rem % 3600) / 60
    )
}

/// Howard Hinnant's days-to-civil algorithm.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// Unix time of an ISO-8601 UTC timestamp such as `2026-10-01T08:38:17Z` (GitHub's format).
pub fn parse_iso8601(text: &str) -> Option<i64> {
    let t = text.trim().strip_suffix('Z')?;
    let (date, time) = t.split_once('T')?;
    let mut d = date.splitn(3, '-').map(|p| p.parse::<i64>().ok());
    let (y, m, day) = (d.next()??, d.next()??, d.next()??);
    let mut h = time
        .split('.')
        .next()?
        .splitn(3, ':')
        .map(|p| p.parse::<i64>().ok());
    let (hh, mm, ss) = (h.next()??, h.next()??, h.next()??);
    if !(1..=12).contains(&m) || !(1..=31).contains(&day) || hh > 23 || mm > 59 || ss > 60 {
        return None;
    }
    // Days from civil (inverse of `civil_from_days`).
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    Some(days * 86_400 + hh * 3600 + mm * 60 + ss)
}

/// Human-readable byte count: `512 B`, `1.5 KB`, `12.3 MB`.
pub fn format_bytes(bytes: usize) -> String {
    const KB: f64 = 1024.0;
    let b = bytes as f64;
    if b < KB {
        format!("{bytes} B")
    } else if b < KB * KB {
        format!("{:.1} KB", b / KB)
    } else if b < KB * KB * KB {
        format!("{:.1} MB", b / (KB * KB))
    } else {
        format!("{:.2} GB", b / (KB * KB * KB))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn epoch_formatting() {
        assert_eq!(format_epoch(0), "1970-01-01 00:00");
        assert_eq!(format_epoch(1_700_000_000), "2023-11-14 22:13");
        assert_eq!(format_epoch(951_782_400), "2000-02-29 00:00");
        assert_eq!(format_epoch(-86_400), "1969-12-31 00:00");
    }

    #[test]
    fn iso_timestamps() {
        assert_eq!(parse_iso8601("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(parse_iso8601("2023-11-14T22:13:20Z"), Some(1_700_000_000));
        assert_eq!(parse_iso8601("2000-02-29T00:00:00.123Z"), Some(951_782_400));
        assert_eq!(parse_iso8601("2026-10-01"), None);
        assert_eq!(parse_iso8601("2026-13-01T00:00:00Z"), None);
        assert_eq!(parse_iso8601(""), None);
    }

    #[test]
    fn byte_formatting() {
        assert_eq!(format_bytes(512), "512 B");
        assert_eq!(format_bytes(1536), "1.5 KB");
        assert_eq!(format_bytes(5 * 1024 * 1024), "5.0 MB");
    }
}
