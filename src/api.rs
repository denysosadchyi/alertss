//! Background poller for alert data.
//!
//! Primary source: official alerts.in.ua API (needs a token in config.json, 20 s poll).
//! Fallback: ubilling.net.ua mirror (no levels, 15 s poll).

use crate::state::{FetchResult, Level, RegionState, Snapshot};
use crate::util;
use std::collections::BTreeMap;
use std::thread;
use std::time::Duration;
use windows::Foundation::Uri;
use windows::Web::Http::{HttpClient, HttpMethod, HttpRequestMessage};
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::System::Com::{CoInitializeEx, COINIT_MULTITHREADED};
use windows::Win32::UI::WindowsAndMessaging::{IsWindow, PostMessageW};
use windows::core::HSTRING;

const OFFICIAL_URL: &str = "https://api.alerts.in.ua/v1/alerts/active.json";
const FALLBACK_URL: &str = "https://ubilling.net.ua/aerialalerts/";
const OFFICIAL_INTERVAL: u64 = 20;
const FALLBACK_INTERVAL: u64 = 15;
const SITREP_URL: &str = "https://api.alerts.in.ua/v3/alerts/active.md";
const SITREP_INTERVAL: u64 = 60;
const SITREP_BACKOFF: u64 = 300;
/// A SITREP older than this no longer counts (levels dropped, pure ubilling).
const SITREP_MAX_AGE: u64 = 180;
const KYIV: &str = "м. Київ";
const SEVASTOPOL: &str = "Севастополь";

/// The 26 region keys (exactly as the fallback source and the map spell them; no Crimea).
fn region_keys() -> impl Iterator<Item = &'static str> {
    crate::map_data::REGIONS
        .iter()
        .map(|r| r.name)
        .filter(|n| *n != crate::menu::CRIMEA)
}

/// Spawn a background thread that polls and posts `crate::state::WM_APP_DATA` to `notify_hwnd`
/// with lParam = Box::into_raw(Box<FetchResult>). `notify_hwnd` is the raw HWND value.
/// If `fake` is true, no network: synthetic levels/partial alerts cycling every 20 s.
pub fn spawn_poller(notify_hwnd: isize, fake: bool) {
    thread::spawn(move || {
        if fake {
            poller_fake(notify_hwnd);
        } else {
            poller_real(notify_hwnd);
        }
    });
}

fn poller_real(notify_hwnd: isize) {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
    }

    let start = std::time::Instant::now();
    let mut last_fetch = start - Duration::from_secs(60);
    let mut interval = FALLBACK_INTERVAL;
    // SITREP (token-less level source): last good snapshot + when, and its own schedule.
    let mut sitrep: Option<(Snapshot, std::time::Instant)> = None;
    let mut sitrep_due = start;

    loop {
        if last_fetch.elapsed() >= Duration::from_secs(interval) {
            last_fetch = std::time::Instant::now();
            // Re-read the config each cycle so a token added while running takes effect.
            let token = crate::config::Config::load().api_token.trim().to_string();
            interval = if token.is_empty() { FALLBACK_INTERVAL } else { OFFICIAL_INTERVAL };

            let mut result = None;
            if !token.is_empty() {
                result = fetch_official(&token);
            }
            let result = result.unwrap_or_else(|| {
                // Token-less path: ubilling (fast on/off) merged with the SITREP (levels).
                if token.is_empty() && std::time::Instant::now() >= sitrep_due {
                    match http_get(SITREP_URL, None)
                        .and_then(|md| parse_sitrep(&md, util::now_unix()))
                    {
                        Ok(s) => {
                            sitrep = Some((s, std::time::Instant::now()));
                            sitrep_due = std::time::Instant::now() + Duration::from_secs(SITREP_INTERVAL);
                        }
                        Err(_) => {
                            sitrep_due = std::time::Instant::now() + Duration::from_secs(SITREP_BACKOFF);
                        }
                    }
                }
                let url = format!("{}?_={}", FALLBACK_URL, util::now_unix());
                match http_get(&url, None) {
                    Ok(text) => match parse_snapshot(&text, util::now_unix()) {
                        FetchResult::Ok(ub) => {
                            let fresh = sitrep
                                .as_ref()
                                .filter(|(_, at)| token.is_empty() && at.elapsed() <= Duration::from_secs(SITREP_MAX_AGE))
                                .map(|(s, _)| s);
                            FetchResult::Ok(merge_snapshots(ub, fresh))
                        }
                        e => e,
                    },
                    Err(e) => FetchResult::Err(e),
                }
            });
            if !post_message(notify_hwnd, result) {
                break;
            }
        }
        thread::sleep(Duration::from_millis(100));
    }
}

/// Merge the fast ubilling snapshot with a fresh SITREP snapshot (if any).
/// alert = ubilling || sitrep; where the SITREP has the region alerting its
/// level/partial/areas/changed win; ubilling-only alerts are Red (level unknown).
fn merge_snapshots(mut ub: Snapshot, sitrep: Option<&Snapshot>) -> Snapshot {
    let Some(sr) = sitrep else {
        return ub; // pure ubilling, has_levels = false
    };
    for (name, st) in ub.regions.iter_mut() {
        match sr.regions.get(name) {
            Some(s) if s.alert => *st = s.clone(),
            _ if st.alert => {
                st.level = Level::Red;
                st.partial = false;
                st.areas.clear();
            }
            _ => {}
        }
    }
    ub.has_levels = true;
    ub
}

/// Some(Ok) on success; None on any failure (caller falls back).
fn fetch_official(token: &str) -> Option<FetchResult> {
    let text = http_get(OFFICIAL_URL, Some(token)).ok()?;
    match parse_official(&text, util::now_unix()) {
        Ok(s) => Some(FetchResult::Ok(s)),
        Err(_) => None,
    }
}

/// Blocking HTTP GET via WinRT HttpClient. Non-2xx statuses are errors.
fn http_get(url: &str, bearer: Option<&str>) -> Result<String, String> {
    let uri = Uri::CreateUri(&HSTRING::from(url)).map_err(|_| "Invalid URL".to_string())?;
    let client = HttpClient::new().map_err(|_| "Failed to create client".to_string())?;
    let req = HttpRequestMessage::Create(&HttpMethod::Get().map_err(|e| e.to_string())?, &uri)
        .map_err(|e| e.to_string())?;
    if let Some(t) = bearer {
        let headers = req.Headers().map_err(|e| e.to_string())?;
        headers
            .TryAppendWithoutValidation(
                &HSTRING::from("Authorization"),
                &HSTRING::from(format!("Bearer {}", t)),
            )
            .map_err(|e| e.to_string())?;
    }
    let resp = client
        .SendRequestAsync(&req)
        .map_err(|e| e.to_string())?
        .join()
        .map_err(|e| e.to_string())?;
    if !resp.IsSuccessStatusCode().map_err(|e| e.to_string())? {
        let code = resp.StatusCode().map(|c| c.0).unwrap_or(0);
        return Err(format!("HTTP {}", code));
    }
    let text = resp
        .Content()
        .map_err(|e| e.to_string())?
        .ReadAsStringAsync()
        .map_err(|e| e.to_string())?
        .join()
        .map_err(|e| e.to_string())?;
    Ok(text.to_string())
}

fn poller_fake(notify_hwnd: isize) {
    let keys: Vec<&str> = region_keys().collect();
    loop {
        let unix_now = util::now_unix();
        let slot = unix_now / 20;
        let changed = slot * 20;

        let mut regions = BTreeMap::new();
        for (i, name) in keys.iter().enumerate() {
            // Phases: 0 none, 1 yellow partial, 2 red whole, 3 yellow whole.
            let st = match (slot + i as i64) % 4 {
                1 => RegionState {
                    alert: true,
                    changed: Some(changed),
                    level: Level::Yellow,
                    partial: true,
                    areas: vec!["Тестовий район".to_string()],
                },
                2 => RegionState { alert: true, changed: Some(changed), level: Level::Red, ..Default::default() },
                3 => RegionState { alert: true, changed: Some(changed), level: Level::Yellow, ..Default::default() },
                _ => RegionState { changed: Some(changed), ..Default::default() },
            };
            regions.insert(name.to_string(), st);
        }

        let snapshot = Snapshot { regions, fetched_at: unix_now, has_levels: true };
        if !post_message(notify_hwnd, FetchResult::Ok(snapshot)) {
            break;
        }
        thread::sleep(Duration::from_secs(2));
    }
}

// ---------------------------------------------------------------- official API parsing

/// Parse "YYYY-MM-DDTHH:MM:SS[.fff]Z" (UTC) into Unix seconds.
fn parse_iso_utc(s: &str) -> Option<i64> {
    let b = s.as_bytes();
    if b.len() < 19
        || b[4] != b'-'
        || b[7] != b'-'
        || (b[10] != b'T' && b[10] != b' ')
        || b[13] != b':'
        || b[16] != b':'
    {
        return None;
    }
    let num = |a: usize, z: usize| s.get(a..z)?.parse::<i64>().ok();
    let (y, mo, d) = (num(0, 4)?, num(5, 7)?, num(8, 10)?);
    let (h, mi, se) = (num(11, 13)?, num(14, 16)?, num(17, 19)?);
    if !(1..=12).contains(&mo) || !(1..=31).contains(&d) || h > 23 || mi > 59 || se > 60 {
        return None;
    }
    Some(civil_to_unix(y, mo, d, h, mi, se))
}

fn normalize_key(name: &str) -> &str {
    match name.trim() {
        "м. Севастополь" | "Севастополь" => SEVASTOPOL,
        "Київ" | "м. Київ" => KYIV,
        other => other,
    }
}

/// Parse the response of `GET /v1/alerts/active.json` into a full 26-region snapshot.
pub fn parse_official(json: &str, now: i64) -> Result<Snapshot, String> {
    let v: serde_json::Value = serde_json::from_str(json).map_err(|e| format!("JSON: {e}"))?;
    let alerts = v
        .get("alerts")
        .and_then(|a| a.as_array())
        .ok_or_else(|| "Missing 'alerts' array".to_string())?;

    let mut regions: BTreeMap<String, RegionState> =
        region_keys().map(|k| (k.to_string(), RegionState::default())).collect();
    // Per-region sub-area titles and "has whole-region alert" flag.
    let mut areas: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut whole: BTreeMap<String, bool> = BTreeMap::new();

    for a in alerts {
        let s = |k: &str| a.get(k).and_then(|x| x.as_str()).unwrap_or("");
        if s("alert_type") != "air_raid" {
            continue;
        }
        if a.get("finished_at").map_or(false, |f| f.is_string()) {
            continue;
        }
        let title = s("location_title");
        let ltype = s("location_type");
        let title_key = normalize_key(title);

        let (key, is_whole) = if title_key == KYIV || title_key == SEVASTOPOL || ltype == "oblast" {
            (title_key, true)
        } else {
            (normalize_key(s("location_oblast")), false)
        };
        let Some(st) = regions.get_mut(key) else { continue };

        let level = match s("alert_level") {
            "yellow" => Level::Yellow,
            _ => Level::Red, // "red" or missing: official rule, no level = red
        };
        st.alert = true;
        st.level = st.level.max(level);
        if let Some(t) = a.get("started_at").and_then(|x| x.as_str()).and_then(parse_iso_utc) {
            st.changed = Some(st.changed.map_or(t, |c| c.min(t)));
        }
        if is_whole {
            whole.insert(key.to_string(), true);
        } else if !title.is_empty() {
            areas.entry(key.to_string()).or_default().push(title.to_string());
        }
    }

    for (key, st) in regions.iter_mut() {
        if st.alert && !whole.contains_key(key) {
            st.partial = true;
            let mut list = areas.remove(key).unwrap_or_default();
            list.sort();
            list.dedup();
            st.areas = list;
        }
    }

    Ok(Snapshot { regions, fetched_at: now, has_levels: true })
}

// ---------------------------------------------------------------- SITREP (token-less levels)

/// Unix seconds for a civil date-time given as UTC (Howard Hinnant's days-from-civil).
fn civil_to_unix(y: i64, mo: i64, d: i64, h: i64, mi: i64, se: i64) -> i64 {
    let y2 = if mo <= 2 { y - 1 } else { y };
    let era = y2.div_euclid(400);
    let yoe = y2 - era * 400;
    let mp = (mo + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    (era * 146097 + doe - 719468) * 86400 + h * 3600 + mi * 60 + se
}

/// Kyiv-local reference taken from the report header:
/// `LOCAL TIME  Friday, 09 October 2026, 11:20 (Europe/Kyiv, 2026-10-09T11:20:59+03:00)`.
struct KyivClock {
    y: i64,
    mo: i64,
    d: i64,
    /// Offset from UTC in seconds.
    off: i64,
}

impl KyivClock {
    fn from_report(md: &str) -> Option<KyivClock> {
        let line = md.lines().find(|l| l.starts_with("LOCAL TIME"))?;
        let iso = line.rsplit(", ").next()?.trim_end_matches(')').trim();
        // 2026-10-09T11:20:59+03:00
        let b = iso.as_bytes();
        if iso.len() < 25 {
            return None;
        }
        let num = |a: usize, z: usize| iso.get(a..z)?.parse::<i64>().ok();
        let sign = match b[19] {
            b'+' => 1,
            b'-' => -1,
            _ => return None,
        };
        let off = sign * (num(20, 22)? * 3600 + num(23, 25)? * 60);
        Some(KyivClock { y: num(0, 4)?, mo: num(5, 7)?, d: num(8, 10)?, off })
    }

    /// Local wall time on the given civil date -> Unix seconds.
    fn at(&self, y: i64, mo: i64, d: i64, h: i64, mi: i64) -> i64 {
        civil_to_unix(y, mo, d, h, mi, 0) - self.off
    }

    /// Parse the text after "since ": "09:01", "Oct 8, 20:44" or "Oct 8 2024, 12:31".
    fn parse_since(&self, rest: &str, now: i64) -> Option<i64> {
        let toks: Vec<&str> = rest
            .split(|c: char| c == ' ' || c == ',')
            .filter(|t| !t.is_empty())
            .take(4)
            .collect();
        let hm = |t: &str| -> Option<(i64, i64)> {
            let t = t.trim_end_matches('.');
            let (h, m) = t.split_once(':')?;
            Some((h.parse().ok()?, m.parse().ok()?))
        };
        let first = toks.first()?;
        if first.as_bytes().first()?.is_ascii_digit() {
            let (h, m) = hm(first)?;
            let mut t = self.at(self.y, self.mo, self.d, h, m);
            if t > now + 120 {
                t -= 86400;
            }
            return Some(t);
        }
        const MONTHS: [&str; 12] =
            ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
        let mon = first.get(..3)?;
        let mo = MONTHS.iter().position(|m| *m == mon)? as i64 + 1;
        let d: i64 = toks.get(1)?.parse().ok()?;
        let (year, time_tok) = match toks.get(2)?.parse::<i64>() {
            Ok(y) if y > 1999 => (Some(y), *toks.get(3)?),
            _ => (None, *toks.get(2)?),
        };
        let (h, m) = hm(time_tok)?;
        let mut y = year.unwrap_or(self.y);
        let mut t = self.at(y, mo, d, h, m);
        if year.is_none() && t > now + 86400 {
            y -= 1; // "Dec 31" read in early January
            t = self.at(y, mo, d, h, m);
        }
        Some(t)
    }
}

/// Text inside the last "(...)" of `s` (the Ukrainian name of "Eng (Укр)").
fn last_parens(s: &str) -> Option<&str> {
    let end = s.rfind(')')?;
    let start = s[..end].rfind('(')?;
    Some(s[start + 1..end].trim())
}

/// Split on ", " outside parentheses/brackets.
fn split_top(s: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let (mut depth, mut start) = (0i32, 0usize);
    let b = s.as_bytes();
    for i in 0..b.len() {
        match b[i] {
            b'(' | b'[' => depth += 1,
            b')' | b']' => depth -= 1,
            b',' if depth == 0 && b.get(i + 1) == Some(&b' ') => {
                out.push(&s[start..i]);
                start = i + 2;
            }
            _ => {}
        }
    }
    out.push(&s[start..]);
    out
}

fn level_from(word: &str) -> Level {
    if word.trim_start().starts_with("yellow") { Level::Yellow } else { Level::Red }
}

/// One entry of an areas list ("Synelnykivskyi (Синельниківський) [hromada]") -> display name
/// and whether it denotes the whole oblast.
fn format_area(item: &str, oblast_uk: &str) -> (String, bool) {
    let mut s = item.trim().trim_end_matches('.').trim();
    let mut hromada = false;
    if s.ends_with(']') {
        if let Some(i) = s.rfind('[') {
            hromada = s[i..].contains("hromada");
            s = s[..i].trim_end();
        }
    }
    let uk = last_parens(s).unwrap_or(s).trim();
    let whole = uk == oblast_uk;
    let name = if uk.starts_with("м. ") || uk.ends_with("район") || uk.ends_with("громада") || whole {
        uk.to_string()
    } else if hromada {
        format!("{uk} громада")
    } else if uk.ends_with("ий") || uk.ends_with("ій") {
        format!("{uk} р-н")
    } else {
        format!("{uk} громада")
    };
    (name, whole)
}

/// Parse the list part of "N areas affected: ..." into (level, areas, whole-oblast flag).
fn parse_area_list(list: &str, oblast_uk: &str) -> (Level, Vec<String>, bool) {
    let list = list.trim().trim_end_matches('.').trim();
    let mut level = Level::None;
    let mut areas = Vec::new();
    let mut whole = false;
    let mut add = |chunk: &str, lvl: Level| {
        level = level.max(lvl);
        for item in split_top(chunk) {
            if item.trim().is_empty() {
                continue;
            }
            let (name, w) = format_area(item, oblast_uk);
            whole |= w;
            areas.push(name);
        }
    };

    // Same level for all: "... [red (missile / mass-strike level)]".
    if list.ends_with(']') {
        if let Some(i) = list.rfind('[') {
            let tag = &list[i + 1..list.len() - 1];
            if tag.starts_with("red") || tag.starts_with("yellow") {
                add(&list[..i], level_from(tag));
                return (level, areas, whole);
            }
        }
    }
    // Mixed: "red (missile / mass-strike level): A (А), B (Б) [hromada]; yellow (drone level): C (В)".
    for piece in list.split("; ") {
        let p = piece.trim();
        if p.starts_with("red") || p.starts_with("yellow") {
            if let Some(i) = p.find("): ") {
                add(&p[i + 3..], level_from(p));
                continue;
            }
        }
        add(p, Level::Red); // no level info: treated as red
    }
    (level, areas, whole)
}

/// Parse one oblast paragraph of section 3 and fold it into `regions`.
fn apply_paragraph(para: &str, clock: &KyivClock, now: i64, regions: &mut BTreeMap<String, RegionState>) {
    // "**Dnipropetrovska oblast (Дніпропетровська область)** — ..."
    let Some(rest) = para.strip_prefix("**") else { return };
    let Some((head, body)) = rest.split_once("**") else { return };

    if head.starts_with("Nominal alerts") {
        // Standing nominal alerts for occupied territory: Luhansk counts as a red, whole alert.
        for (key, st) in regions.iter_mut() {
            if body.contains(&format!("({key})")) {
                st.alert = true;
                st.level = Level::Red;
                st.partial = false;
                st.areas.clear();
            }
        }
        return;
    }

    let Some(uk) = last_parens(head) else { return };
    let key = normalize_key(uk);
    let Some(st) = regions.get_mut(key) else { return };
    let body = body.trim_start_matches(|c: char| c == ' ' || c == '—' || c == '-');

    let changed = body
        .find("since ")
        .and_then(|i| clock.parse_since(&body[i + 6..], now));

    let (level, areas, whole) = if let Some(i) = body.find(" affected: ") {
        let mut list = &body[i + 11..];
        if let Some(j) = list.find(". The most recent") {
            list = &list[..j];
        }
        let (l, a, w) = parse_area_list(list, uk);
        let w = w || body.contains("entire oblast") || body.contains("whole oblast");
        (if l == Level::None { Level::Red } else { l }, a, w)
    } else if let Some(i) = body.find("Alert level: ") {
        (level_from(&body[i + 13..]), Vec::new(), true)
    } else {
        // No area list and no level: whole region, red by the official rule.
        (Level::Red, Vec::new(), true)
    };

    st.alert = true;
    st.level = level;
    st.partial = !whole;
    st.areas = if whole { Vec::new() } else { areas };
    st.changed = changed;
}

/// Parse the public SITREP Markdown (`GET /v3/alerts/active.md`) into a 26-region snapshot.
pub fn parse_sitrep(md: &str, now: i64) -> Result<Snapshot, String> {
    let clock = KyivClock::from_report(md).ok_or_else(|| "SITREP: no LOCAL TIME".to_string())?;
    let start = md
        .find("## 3. CURRENT WARNING STATUS")
        .ok_or_else(|| "SITREP: no section 3".to_string())?;
    let section = &md[start + 3..];
    let section = section.find("\n## ").map_or(section, |e| &section[..e]);

    let mut regions: BTreeMap<String, RegionState> =
        region_keys().map(|k| (k.to_string(), RegionState::default())).collect();
    for para in section.split("\n\n").map(str::trim).filter(|p| p.starts_with("**")) {
        apply_paragraph(para, &clock, now, &mut regions);
    }
    Ok(Snapshot { regions, fetched_at: now, has_levels: true })
}

// ---------------------------------------------------------------- fallback parsing

fn parse_snapshot(json_str: &str, now: i64) -> FetchResult {
    let json_value: serde_json::Value = match serde_json::from_str(json_str) {
        Ok(v) => v,
        Err(_) => return FetchResult::Err("JSON parse error".to_string()),
    };

    let states = match json_value.get("states") {
        Some(s) => s,
        None => return FetchResult::Err("Missing 'states' key".to_string()),
    };

    let mut regions = BTreeMap::new();

    if let Some(obj) = states.as_object() {
        for (region_name, region_data) in obj {
            let alert = region_data
                .get("alertnow")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);

            let changed = region_data
                .get("changed")
                .and_then(|v| v.as_str())
                .and_then(|s| util::parse_local(s));

            regions.insert(
                region_name.clone(),
                RegionState {
                    alert,
                    changed,
                    // No level data in this source: an alert without a level counts as red.
                    level: if alert { Level::Red } else { Level::None },
                    ..Default::default()
                },
            );
        }
    }

    if regions.is_empty() {
        return FetchResult::Err("No regions found".to_string());
    }

    FetchResult::Ok(Snapshot { regions, fetched_at: now, has_levels: false })
}

fn post_message(notify_hwnd: isize, result: FetchResult) -> bool {
    unsafe {
        let hwnd = HWND(notify_hwnd as *mut _);
        if !IsWindow(Some(hwnd)).as_bool() {
            return false;
        }

        let boxed = Box::new(result);
        let ptr = Box::into_raw(boxed) as isize;
        let res = PostMessageW(
            Some(hwnd),
            crate::state::WM_APP_DATA,
            WPARAM(0),
            LPARAM(ptr),
        );

        if res.is_err() {
            // Reclaim the box to avoid a leak
            let _ = Box::from_raw(ptr as *mut FetchResult);
        }

        res.is_ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_snapshot_valid() {
        let json = r#"{"source":"test","cachedat":"2026-10-09 09:54:51","states":{"Вінницька область":{"alertnow":false,"changed":"1970-01-01 03:00:00"},"м. Київ":{"alertnow":true,"changed":"2026-10-09 09:03:00"}}}"#;
        match parse_snapshot(json, util::now_unix()) {
            FetchResult::Ok(snap) => {
                assert!(!snap.has_levels);
                assert!(snap.regions.contains_key("Вінницька область"));
                assert!(snap.regions["м. Київ"].alert);
                assert_eq!(snap.regions["м. Київ"].level, Level::Red);
            }
            FetchResult::Err(e) => panic!("Unexpected error: {}", e),
        }
    }

    #[test]
    fn test_parse_snapshot_missing_states() {
        let json = r#"{"source":"test","cachedat":"2026-10-09 09:54:51"}"#;
        assert!(matches!(parse_snapshot(json, util::now_unix()), FetchResult::Err(_)));
    }

    const SAMPLE: &str = r#"{"alerts":[
      {"id":1,"location_title":"Луганська область","location_type":"oblast","started_at":"2022-04-04T16:45:39.000Z","finished_at":null,"updated_at":"2022-04-08T08:04:26.316Z","alert_type":"air_raid","location_uid":"16","location_oblast":"Луганська область","location_oblast_uid":"16","alert_level":"yellow"},
      {"id":2,"location_title":"Харківський район","location_type":"raion","started_at":"2026-10-09T10:00:00.000Z","finished_at":null,"alert_type":"air_raid","location_oblast":"Харківська область","alert_level":"yellow"},
      {"id":3,"location_title":"Куп'янська міська громада","location_type":"hromada","started_at":"2026-10-09T09:30:00.000Z","finished_at":null,"alert_type":"air_raid","location_oblast":"Харківська область","alert_level":"red"},
      {"id":4,"location_title":"Харківський район","location_type":"raion","started_at":"2026-10-09T10:05:00.000Z","finished_at":null,"alert_type":"air_raid","location_oblast":"Харківська область"},
      {"id":5,"location_title":"м. Київ","location_type":"city","started_at":"2026-10-09T08:00:00Z","finished_at":null,"alert_type":"air_raid","location_oblast":"м. Київ","alert_level":"yellow"},
      {"id":6,"location_title":"Одеська область","location_type":"oblast","started_at":"2026-10-09T08:00:00Z","finished_at":null,"alert_type":"artillery_shelling","location_oblast":"Одеська область"},
      {"id":7,"location_title":"Автономна Республіка Крим","location_type":"oblast","started_at":"2022-12-10T22:22:00Z","finished_at":null,"alert_type":"air_raid","location_oblast":"Автономна Республіка Крим"}
    ]}"#;

    #[test]
    fn iso_parse() {
        assert_eq!(parse_iso_utc("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(parse_iso_utc("2022-04-04T16:45:39.000Z"), Some(1649090739));
        assert_eq!(parse_iso_utc("garbage"), None);
    }

    #[test]
    fn official_aggregation() {
        let s = parse_official(SAMPLE, 1).unwrap();
        assert!(s.has_levels);
        assert_eq!(s.regions.len(), 26);
        assert!(!s.regions.contains_key("Автономна Республіка Крим"));
        let lug = &s.regions["Луганська область"];
        assert!(lug.alert && !lug.partial && lug.level == Level::Yellow);
        let kh = &s.regions["Харківська область"];
        assert!(kh.alert && kh.partial);
        assert_eq!(kh.level, Level::Red);
        assert_eq!(kh.areas, vec!["Куп'янська міська громада".to_string(), "Харківський район".to_string()]);
        assert_eq!(kh.changed, parse_iso_utc("2026-10-09T09:30:00Z"));
        let kyiv = &s.regions["м. Київ"];
        assert!(kyiv.alert && !kyiv.partial && kyiv.level == Level::Yellow);
        let od = &s.regions["Одеська область"];
        assert!(!od.alert && od.level == Level::None);
        assert!(!s.regions["Вінницька область"].alert);
    }

    #[test]
    fn official_empty_and_bad() {
        let s = parse_official(r#"{"alerts":[]}"#, 1).unwrap();
        assert_eq!(s.regions.len(), 26);
        assert!(s.regions.values().all(|r| !r.alert));
        assert!(parse_official("{}", 1).is_err());
        assert!(parse_official("nope", 1).is_err());
    }

    // Real report captured 2026-10-09 11:20 Kyiv time.
    const SITREP_REAL: &str = include_str!("../tests/fixtures/sitrep1.md");

    fn real_now() -> i64 {
        parse_iso_utc("2026-10-09T08:20:59Z").unwrap()
    }

    #[test]
    fn sitrep_real_report() {
        let s = parse_sitrep(SITREP_REAL, real_now()).unwrap();
        assert!(s.has_levels);
        assert_eq!(s.regions.len(), 26);

        let kyiv = &s.regions["м. Київ"];
        assert!(kyiv.alert && !kyiv.partial && kyiv.level == Level::Yellow);
        assert!(kyiv.areas.is_empty());
        assert_eq!(kyiv.changed, parse_iso_utc("2026-10-09T06:01:00Z")); // 09:01 Kyiv (+03:00)

        let dn = &s.regions["Дніпропетровська область"];
        assert!(dn.alert && dn.partial && dn.level == Level::Red);
        assert_eq!(dn.areas.len(), 11);
        assert!(dn.areas.contains(&"Синельниківський р-н".to_string()));
        assert!(dn.areas.contains(&"м. Марганець".to_string()));
        assert!(dn.areas.contains(&"Марганецька громада".to_string()));
        assert_eq!(dn.changed, parse_iso_utc("2026-10-08T17:44:00Z")); // "Oct 8, 20:44"

        let my = &s.regions["Миколаївська область"];
        assert!(my.partial && my.level == Level::Red && my.areas == vec!["Миколаївський р-н".to_string()]);

        let kh = &s.regions["Харківська область"];
        assert!(kh.partial && kh.level == Level::Yellow && kh.areas.len() == 5);
        assert!(kh.areas.contains(&"Куп'янський р-н".to_string()));
        assert!(kh.areas.contains(&"Харківська громада".to_string()));

        // Nominal alert in occupied Luhansk: red, whole.
        let lu = &s.regions["Луганська область"];
        assert!(lu.alert && !lu.partial && lu.level == Level::Red);

        // Not listed: no alert (Odesa only has explosion reports).
        assert!(!s.regions["Одеська область"].alert);
        assert!(!s.regions["Вінницька область"].alert);
        assert_eq!(s.regions.values().filter(|r| r.alert).count(), 15);
    }

    #[test]
    fn sitrep_mixed_and_whole_variants() {
        // Synthetic: mixed levels, whole-oblast via "Alert level:", and via the oblast name in the list.
        let md = "LOCAL TIME     Friday, 09 October 2026, 11:20 (Europe/Kyiv, 2026-10-09T11:20:59+03:00)\n\
\n## 3. CURRENT WARNING STATUS\n\n\
**Odeska oblast (Одеська область)** — air raid alert in effect since 10:00, ongoing for 1 h. 3 areas affected: red (missile / mass-strike level): Odeskyi (Одеський), Izmailska (Ізмаїльська) [hromada]; yellow (drone level): Bilhorod-Dnistrovskyi (Білгород-Дністровський).\n\n\
**Lvivska oblast (Львівська область)** — air raid alert in effect since Oct 9, 10:30, ongoing for 50 min. Alert level: yellow (drone level).\n\n\
**Volynska oblast (Волинська область)** — air raid alert in effect since 10:30. 2 areas affected: Volynska oblast (Волинська область), Lutskyi (Луцький) [yellow (drone level)].\n\n\
## 4. THREAT ACTIVITY\n\n**Vinnytska oblast (Вінницька область)** — ignored, outside section 3.\n";
        let s = parse_sitrep(md, real_now()).unwrap();
        let od = &s.regions["Одеська область"];
        assert!(od.partial && od.level == Level::Red);
        assert_eq!(
            od.areas,
            vec![
                "Одеський р-н".to_string(),
                "Ізмаїльська громада".to_string(),
                "Білгород-Дністровський р-н".to_string()
            ]
        );
        let lv = &s.regions["Львівська область"];
        assert!(lv.alert && !lv.partial && lv.level == Level::Yellow);
        assert_eq!(lv.changed, parse_iso_utc("2026-10-09T07:30:00Z"));
        let vo = &s.regions["Волинська область"];
        assert!(vo.alert && !vo.partial && vo.level == Level::Yellow);
        assert!(!s.regions["Вінницька область"].alert);
    }

    #[test]
    fn sitrep_errors() {
        assert!(parse_sitrep("garbage", 0).is_err());
        let no_sec = "LOCAL TIME  x (Europe/Kyiv, 2026-10-09T11:20:59+03:00)\n";
        assert!(parse_sitrep(no_sec, 0).is_err());
    }

    #[test]
    fn merge_rules() {
        let st = |alert, level| RegionState { alert, level, ..Default::default() };
        let mk = |v: Vec<(&str, RegionState)>, lv| Snapshot {
            regions: v.into_iter().map(|(k, s)| (k.to_string(), s)).collect(),
            fetched_at: 0,
            has_levels: lv,
        };
        let ub = mk(
            vec![
                ("a", st(true, Level::Red)),
                ("b", st(true, Level::Red)),
                ("c", st(false, Level::None)),
                ("d", st(false, Level::None)),
            ],
            false,
        );
        let mut sa = st(true, Level::Yellow);
        sa.partial = true;
        sa.areas = vec!["X р-н".to_string()];
        let sr = mk(vec![("a", sa), ("b", st(false, Level::None)), ("c", st(true, Level::Yellow))], true);

        // No fresh SITREP: untouched ubilling.
        assert!(!merge_snapshots(ub.clone(), None).has_levels);

        let m = merge_snapshots(ub, Some(&sr));
        assert!(m.has_levels);
        assert!(m.regions["a"].alert && m.regions["a"].level == Level::Yellow && m.regions["a"].partial);
        // ubilling-only alert: red, not partial.
        assert!(m.regions["b"].alert && m.regions["b"].level == Level::Red && !m.regions["b"].partial);
        // sitrep-only alert still counts.
        assert!(m.regions["c"].alert && m.regions["c"].level == Level::Yellow);
        assert!(!m.regions["d"].alert);
    }
}
