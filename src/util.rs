//! Time helpers. All instants are Unix seconds (UTC) as i64.

use std::mem;
use windows::Win32::Foundation::{FILETIME, SYSTEMTIME};
use windows::Win32::System::SystemInformation::GetLocalTime;
use windows::Win32::System::Time::{
    FileTimeToSystemTime, SystemTimeToFileTime, SystemTimeToTzSpecificLocalTime,
    TzSpecificLocalTimeToSystemTime,
};

/// Current time, Unix seconds UTC.
pub fn now_unix() -> i64 {
    match std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH) {
        Ok(d) => d.as_secs() as i64,
        Err(_) => 0,
    }
}

/// Parse "YYYY-MM-DD HH:MM:SS" given in the PC's local time zone into Unix seconds UTC.
/// Returns None for malformed input or years < 2022 (the source uses 1970-01-01 as "unknown").
pub fn parse_local(s: &str) -> Option<i64> {
    // Expected format: "YYYY-MM-DD HH:MM:SS"
    let parts: Vec<&str> = s.split_whitespace().collect();
    if parts.len() != 2 {
        return None;
    }

    let date_parts: Vec<&str> = parts[0].split('-').collect();
    if date_parts.len() != 3 {
        return None;
    }
    let year: i32 = date_parts[0].parse().ok()?;
    let month: u16 = date_parts[1].parse().ok()?;
    let day: u16 = date_parts[2].parse().ok()?;

    if year < 2022 {
        return None;
    }

    let time_parts: Vec<&str> = parts[1].split(':').collect();
    if time_parts.len() != 3 {
        return None;
    }
    let hour: u16 = time_parts[0].parse().ok()?;
    let minute: u16 = time_parts[1].parse().ok()?;
    let second: u16 = time_parts[2].parse().ok()?;

    let st = SYSTEMTIME {
        wYear: year as u16,
        wMonth: month,
        wDayOfWeek: 0,
        wDay: day,
        wHour: hour,
        wMinute: minute,
        wSecond: second,
        wMilliseconds: 0,
    };

    unsafe {
        // Convert local SYSTEMTIME to UTC SYSTEMTIME, then to FILETIME.
        let mut st_utc: SYSTEMTIME = mem::zeroed();
        if TzSpecificLocalTimeToSystemTime(None, &st, &mut st_utc).is_err() {
            return None;
        }
        let mut ft_utc: FILETIME = mem::zeroed();
        if SystemTimeToFileTime(&st_utc, &mut ft_utc).is_err() {
            return None;
        }

        // FILETIME is 100ns intervals since 1601-01-01
        // Unix epoch is 1970-01-01
        // Offset: 11644473600 seconds
        let filetime_val = ((ft_utc.dwHighDateTime as i64) << 32) | (ft_utc.dwLowDateTime as i64);
        let unix_secs = (filetime_val / 10_000_000) - 11644473600;
        Some(unix_secs)
    }
}

/// Format an instant as local "HH:MM".
pub fn local_hm(unix: i64) -> String {
    if unix < 0 {
        return "00:00".to_string();
    }

    let filetime = (unix + 11644473600) * 10_000_000;
    let ft = FILETIME {
        dwLowDateTime: (filetime & 0xffffffff) as u32,
        dwHighDateTime: ((filetime >> 32) & 0xffffffff) as u32,
    };

    unsafe {
        let mut st_utc = mem::zeroed();
        if FileTimeToSystemTime(&ft, &mut st_utc).is_err() {
            return "00:00".to_string();
        }

        let mut st_local = mem::zeroed();
        if SystemTimeToTzSpecificLocalTime(None, &st_utc, &mut st_local).is_err() {
            return "00:00".to_string();
        }

        format!("{:02}:{:02}", st_local.wHour, st_local.wMinute)
    }
}

/// True if `unix` falls on today's local date.
pub fn is_today(unix: i64) -> bool {
    if unix < 0 {
        return false;
    }

    let filetime = (unix + 11644473600) * 10_000_000;
    let ft = FILETIME {
        dwLowDateTime: (filetime & 0xffffffff) as u32,
        dwHighDateTime: ((filetime >> 32) & 0xffffffff) as u32,
    };

    unsafe {
        let mut st_utc = mem::zeroed();
        if FileTimeToSystemTime(&ft, &mut st_utc).is_err() {
            return false;
        }

        let mut st_local = mem::zeroed();
        if SystemTimeToTzSpecificLocalTime(None, &st_utc, &mut st_local).is_err() {
            return false;
        }

        let today = GetLocalTime();

        st_local.wYear == today.wYear
            && st_local.wMonth == today.wMonth
            && st_local.wDay == today.wDay
    }
}

/// Compact duration for the taskbar: <24h "{h}:{mm:02}" (e.g. "0:23", "5:07"), else "{d} д".
pub fn fmt_short(secs: i64) -> String {
    if secs < 0 {
        return "0:00".to_string();
    }

    if secs < 86400 {
        // <24h, format as h:mm
        format!("{}:{:02}", secs / 3600, (secs % 3600) / 60)
    } else {
        // >=24h, format as "d д"
        let days = secs / 86400;
        format!("{} д", days)
    }
}

/// Long Ukrainian duration for the flyout: "23 хв", "1 год 5 хв", "2 д 3 год".
pub fn fmt_long(secs: i64) -> String {
    if secs < 0 {
        return "0 хв".to_string();
    }

    if secs < 3600 {
        // <1h, format as "X хв"
        let min = secs / 60;
        if min == 0 {
            "<1 хв".to_string()
        } else {
            format!("{} хв", min)
        }
    } else if secs < 86400 {
        // <24h, format as "X год Y хв"
        let hours = secs / 3600;
        let min = (secs % 3600) / 60;
        if min == 0 {
            format!("{} год", hours)
        } else {
            format!("{} год {} хв", hours, min)
        }
    } else {
        // >=24h, format as "X д Y год"
        let days = secs / 86400;
        let hours = (secs % 86400) / 3600;
        if hours == 0 {
            format!("{} д", days)
        } else {
            format!("{} д {} год", days, hours)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fmt_short() {
        assert_eq!(fmt_short(0), "0:00");
        assert_eq!(fmt_short(23 * 60), "0:23");
        assert_eq!(fmt_short(23), "0:00");
        assert_eq!(fmt_short(3600), "1:00"); // 1 hour
        assert_eq!(fmt_short(5 * 3600 + 7 * 60), "5:07");
        assert_eq!(fmt_short(86400 * 2), "2 д");
        assert_eq!(fmt_short(-1), "0:00");
    }

    #[test]
    fn test_local_roundtrip() {
        // parse_local (local -> UTC) and local_hm (UTC -> local) must be inverses.
        let t = parse_local("2026-10-09 09:03:00").unwrap();
        assert_eq!(local_hm(t), "09:03");
        assert_eq!(local_hm(now_unix()), {
            let lt = unsafe { GetLocalTime() };
            format!("{:02}:{:02}", lt.wHour, lt.wMinute)
        });
    }

    #[test]
    fn test_fmt_long() {
        assert_eq!(fmt_long(0), "<1 хв");
        assert_eq!(fmt_long(30), "<1 хв");
        assert_eq!(fmt_long(60), "1 хв");
        assert_eq!(fmt_long(3600), "1 год");
        assert_eq!(fmt_long(3600 + 5 * 60), "1 год 5 хв");
        assert_eq!(fmt_long(86400 * 2), "2 д");
        assert_eq!(fmt_long(86400 * 2 + 3 * 3600), "2 д 3 год");
        assert_eq!(fmt_long(-1), "0 хв");
    }

    #[test]
    fn test_parse_local_invalid() {
        assert_eq!(parse_local("1970-01-01 03:00:00"), None);
        assert_eq!(parse_local("garbage"), None);
        assert_eq!(parse_local(""), None);
        assert_eq!(parse_local("2021-12-31 12:00:00"), None); // year < 2022
    }
}
