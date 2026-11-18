//! Rough "made for an older game" estimate from dates.
//!
//! A mod file uploaded before a major game update (new heroes, reworks,
//! "Gameplay Update" patches) often breaks; one that only missed minor
//! patches usually still works. The idea of a major-update cutoff comes from
//! Deadlock Mod Manager's "Outdated" badge; Kombini keeps a list of updates
//! instead of one date so it can say which update a mod predates.
//!
//! This is only an estimate. The content checks in `kombini-repair` are the
//! authoritative answer for the files they understand.

use serde::{Deserialize, Serialize};

/// One game update, from Steam news or the built-in list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GameUpdate {
    /// Unix seconds when the update went live (the announcement time).
    pub date: i64,
    pub title: String,
    pub major: bool,
    #[serde(default)]
    pub url: Option<String>,
}

/// How a mod's upload date relates to the game's updates.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Freshness {
    /// No usable date (local mods).
    Unknown,
    /// No known game update since the mod's file was uploaded.
    Current,
    /// Only minor updates since; `latest` is the newest of them.
    MinorBehind { count: usize, latest: GameUpdate },
    /// Uploaded before `first`, the first major update after it.
    BeforeMajor {
        first: GameUpdate,
        majors: usize,
        total: usize,
    },
}

impl Freshness {
    pub fn maybe_outdated(&self) -> bool {
        matches!(self, Self::BeforeMajor { .. })
    }
}

/// Compares `mod_date` with `updates` (any order).
pub fn assess(mod_date: Option<i64>, updates: &[GameUpdate]) -> Freshness {
    let Some(at) = mod_date.filter(|t| *t > 0) else {
        return Freshness::Unknown;
    };
    let mut after: Vec<&GameUpdate> = updates.iter().filter(|u| u.date > at).collect();
    after.sort_by_key(|u| u.date);
    let Some(latest) = after.last() else {
        return Freshness::Current;
    };
    let majors = after.iter().filter(|u| u.major).count();
    match after.iter().find(|u| u.major) {
        Some(first) => Freshness::BeforeMajor {
            first: (*first).clone(),
            majors,
            total: after.len(),
        },
        None => Freshness::MinorBehind {
            count: after.len(),
            latest: (*latest).clone(),
        },
    }
}

/// Classifies a Steam announcement: `Some(true)` major, `Some(false)` minor,
/// `None` for posts that are not game updates (spotlights, events).
pub fn classify_news(title: &str, tags: &[String]) -> Option<bool> {
    let t = title.to_lowercase();
    if t.contains("gameplay update") {
        Some(true)
    } else if t.contains("update") || tags.iter().any(|tag| tag == "patchnotes") {
        Some(false)
    } else {
        None
    }
}

/// Major updates Valve did not title "Gameplay Update" (hero releases,
/// reworks), plus the titled ones so the list works offline. Times are the
/// Steam announcement times (UTC). Extend when a new one ships.
const KNOWN_MAJOR_UPDATES: &[(i64, &str)] = &[
    (1_793_393_509, "Halloween 2026 + Gameplay Update"),
    (1_792_184_407, "Paint the Town"),
    (1_790_974_791, "Listen up, Crumbums! Your King is here."),
    (1_790_713_511, "City Never Sleeps"),
    (1_779_486_662, "Gameplay Update - 05-22-2026"),
    (1_777_592_743, "Gameplay Update - 04-30-2026"),
    (1_772_833_020, "Gameplay Update - 03-06-2026"),
    (1_769_122_593, "Old Gods, New Blood"),
    (1_755_549_740, "Six New Heroes"),
    (1_746_732_792, "Shop Rework Update"),
    (1_740_520_159, "Map Rework Update"),
    (
        1_737_139_201,
        "Holliday, Vyper, Calico, and The Magnificent Sinclair",
    ),
    (1_734_468_800, "Winter Visual Update"),
];

/// Same update if announced within this many seconds.
const SAME_UPDATE_SECS: i64 = 6 * 3600;

pub fn builtin_updates() -> Vec<GameUpdate> {
    KNOWN_MAJOR_UPDATES
        .iter()
        .map(|(date, title)| GameUpdate {
            date: *date,
            title: (*title).to_owned(),
            major: true,
            url: None,
        })
        .collect()
}

/// The built-in list plus `fetched` updates it does not already cover,
/// newest first. A built-in entry keeps its "major" mark and gains the
/// fetched post's URL.
pub fn merge_updates(fetched: &[GameUpdate]) -> Vec<GameUpdate> {
    let mut out = builtin_updates();
    for f in fetched {
        if let Some(known) = out
            .iter_mut()
            .find(|k| (k.date - f.date).abs() <= SAME_UPDATE_SECS)
        {
            if known.url.is_none() {
                known.url.clone_from(&f.url);
            }
            known.major |= f.major;
        } else {
            out.push(f.clone());
        }
    }
    out.sort_by_key(|u| std::cmp::Reverse(u.date));
    out
}

pub fn latest_major(updates: &[GameUpdate]) -> Option<&GameUpdate> {
    updates.iter().filter(|u| u.major).max_by_key(|u| u.date)
}

const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

/// Days since 1970-01-01 for a proleptic Gregorian date (H. Hinnant).
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    (y, m, d)
}

/// "Nov 9, 2026" (UTC).
pub fn format_date(ts: i64) -> String {
    let (y, m, d) = civil_from_days(ts.div_euclid(86_400));
    let month = MONTHS[usize::try_from(m - 1).unwrap_or(0).min(11)];
    format!("{month} {d}, {y}")
}

/// Parses steam.inf's `VersionDate` ("Nov 09 2026") and optional
/// `VersionTime` ("10:12:11"). Valve's build clock is Pacific time; it is
/// treated as UTC-8, which is close enough for a day-level comparison.
pub fn parse_version_date(date: &str, time: Option<&str>) -> Option<i64> {
    let mut parts = date.split_whitespace();
    let month = parts.next()?.get(..3)?;
    let m = MONTHS.iter().position(|n| n.eq_ignore_ascii_case(month))?;
    let d: i64 = parts.next()?.parse().ok()?;
    let y: i64 = parts.next()?.parse().ok()?;
    if !(1..=31).contains(&d) {
        return None;
    }
    let mut secs = 0;
    if let Some(t) = time {
        let mut hms = t.split(':').map(|v| v.trim().parse::<i64>().ok());
        let h = hms.next().flatten().unwrap_or(0);
        let mi = hms.next().flatten().unwrap_or(0);
        let s = hms.next().flatten().unwrap_or(0);
        secs = h * 3600 + mi * 60 + s;
    }
    let m = i64::try_from(m).ok()? + 1;
    Some(days_from_civil(y, m, d) * 86_400 + secs + 8 * 3600)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn up(date: i64, title: &str, major: bool) -> GameUpdate {
        GameUpdate {
            date,
            title: title.into(),
            major,
            url: None,
        }
    }

    #[test]
    fn assess_levels() {
        let ups = [
            up(100, "minor a", false),
            up(200, "big", true),
            up(300, "minor b", false),
        ];
        assert_eq!(assess(None, &ups), Freshness::Unknown);
        assert_eq!(assess(Some(300), &ups), Freshness::Current);
        assert_eq!(
            assess(Some(250), &ups),
            Freshness::MinorBehind {
                count: 1,
                latest: up(300, "minor b", false)
            }
        );
        assert_eq!(
            assess(Some(50), &ups),
            Freshness::BeforeMajor {
                first: up(200, "big", true),
                majors: 1,
                total: 3
            }
        );
        // Uploaded the same day but before the patch went live.
        assert!(assess(Some(199), &ups).maybe_outdated());
    }

    #[test]
    fn classifies_news_titles() {
        let none: Vec<String> = Vec::new();
        assert_eq!(
            classify_news("Halloween 2026 + Gameplay Update", &none),
            Some(true)
        );
        assert_eq!(
            classify_news("Minor Update - 11-04-2026", &none),
            Some(false)
        );
        assert_eq!(classify_news("11-21-2025 Update", &none), Some(false));
        assert_eq!(
            classify_news("Fairfax Industries Employee Spotlight", &none),
            None
        );
        assert_eq!(
            classify_news("Patch", &["patchnotes".to_owned()]),
            Some(false)
        );
    }

    #[test]
    fn merge_keeps_builtin_major_and_adds_minors() {
        let fetched = [
            GameUpdate {
                url: Some("https://x/paint".into()),
                ..up(1_792_184_407, "Paint the Town", false)
            },
            up(1_793_908_000, "Minor Update - 11-04-2026", false),
        ];
        let merged = merge_updates(&fetched);
        assert_eq!(merged[0].title, "Minor Update - 11-04-2026");
        let paint = merged.iter().find(|u| u.title == "Paint the Town").unwrap();
        assert!(paint.major);
        assert_eq!(paint.url.as_deref(), Some("https://x/paint"));
        assert_eq!(merged.len(), builtin_updates().len() + 1);
        assert_eq!(
            latest_major(&merged).unwrap().title,
            "Halloween 2026 + Gameplay Update"
        );
    }

    #[test]
    fn dates_round_trip() {
        assert_eq!(format_date(0), "Jan 1, 1970");
        assert_eq!(format_date(1_793_393_509), "Oct 30, 2026");
        let t = parse_version_date("Nov 09 2026", Some("10:12:11")).unwrap();
        assert_eq!(format_date(t), "Nov 9, 2026");
        assert_eq!(t % 86_400, 18 * 3600 + 12 * 60 + 11);
        assert_eq!(
            parse_version_date("Feb 29 2028", None)
                .map(format_date)
                .as_deref(),
            Some("Feb 29, 2028")
        );
        assert!(parse_version_date("garbage", None).is_none());
    }
}
