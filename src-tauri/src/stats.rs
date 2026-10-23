//! The "Your dictation" numbers for the main window: how many words have
//! been dictated (lifetime, today, per day), speaking speed, time saved
//! against typing, streaks, the apps dictations land in, and how long the
//! pipeline takes from key release to text. Persisted as `stats.json` next
//! to the settings; never leaves this PC.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::recordings;

/// Rolling window of recent speaking speeds.
const WPMS_KEPT: usize = 20;
/// Speeds above this are echo-check artefacts, not speaking; ignore them.
const WPM_SANITY_CAP: f32 = 400.0;
/// Below this, a sample says more about the clip than the speaker.
const MIN_SAMPLE_WORDS: u32 = 3;
const MIN_SAMPLE_SECONDS: f32 = 1.0;
/// Wispr Flow's reference: what the same words would cost on a keyboard.
const TYPING_WPM: f32 = 40.0;

#[derive(Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct Day {
    pub words: u32,
    pub dictations: u32,
    pub speech_s: f32,
    /// Raw words the model heard minus what was pasted (smart formatting
    /// trims fillers and self-corrections).
    pub cleaned: u32,
    /// Words per target app (process name, e.g. "chrome.exe").
    pub apps: HashMap<String, u32>,
    /// Release-to-text wait, summed over `latency_n` dictations.
    pub latency_ms: u64,
    pub latency_n: u32,
}

#[derive(Default, Serialize, Deserialize)]
pub struct Stats {
    /// Word counts and friends per local calendar day ("2026-10-22").
    days: HashMap<String, Day>,
    /// Words per target app across all days.
    apps: HashMap<String, u32>,
    /// Recent speaking speeds, newest last.
    wpms: Vec<f32>,
    /// One-time migration: dictations made before this file existed were
    /// replayed from the recordings index exactly once.
    #[serde(default)]
    migrated: bool,
}

/// One dictation to count.
pub struct Input<'a> {
    /// What the model heard, before smart formatting.
    pub heard: &'a str,
    /// What was pasted.
    pub text: &'a str,
    /// Recording length in seconds.
    pub speech_s: f32,
    /// Release-to-text wait in milliseconds, when it was measured
    /// (replayed recordings predate the latency measurement).
    pub latency_ms: Option<u64>,
    /// Target app process name ("chrome.exe"); None when unknown (replayed
    /// recordings never captured which app got the text).
    pub app: Option<&'a str>,
    /// Local day key ("2026-10-22").
    pub day: &'a str,
}

/// What the dictation contributed, for the pill and result box.
#[derive(Clone, Serialize)]
pub struct Sample {
    pub words: u32,
    /// This dictation's speaking speed (None: too short to be meaningful).
    pub wpm: Option<f32>,
    /// The rolling recent average after adding this one.
    pub avg_wpm: Option<f32>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct WeekDay {
    pub date: String,
    pub words: u32,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct AppWords {
    pub name: String,
    pub words: u32,
}

/// The snapshot the main window renders.
#[derive(Clone, Serialize, Default)]
pub struct Usage {
    pub words: u32,
    pub words_today: u32,
    pub dictations: u32,
    pub wpm: f32,
    pub saved_min: f32,
    pub streak: u32,
    pub longest_streak: u32,
    pub week: Vec<WeekDay>,
    pub top_apps: Vec<AppWords>,
    pub avg_latency_ms: u32,
    pub cleaned: u32,
}

impl Stats {
    /// Counts one finished dictation. A dictation with no words at all is
    /// not counted (there was nothing to track), matching Wispr Flow.
    pub fn record(&mut self, input: Input) -> Sample {
        let words = count_words(input.text);
        if words == 0 {
            return Sample {
                words: 0,
                wpm: None,
                avg_wpm: self.avg_wpm(),
            };
        }

        // Speaking speed only from dictations big enough to say something.
        let wpm = if input.speech_s >= MIN_SAMPLE_SECONDS && words >= MIN_SAMPLE_WORDS {
            Some((words as f32 / input.speech_s * 60.0).min(WPM_SANITY_CAP))
        } else {
            None
        };
        if let Some(speed) = wpm {
            self.wpms.push(speed);
            if self.wpms.len() > WPMS_KEPT {
                self.wpms.remove(0);
            }
        }

        let day = self.days.entry(input.day.to_string()).or_default();
        day.words += words;
        day.dictations += 1;
        day.speech_s += input.speech_s;
        day.cleaned += count_words(input.heard).saturating_sub(words);
        if let Some(app) = input.app {
            *day.apps.entry(app.to_string()).or_default() += words;
            *self.apps.entry(app.to_string()).or_default() += words;
        }
        if let Some(latency_ms) = input.latency_ms {
            day.latency_ms += latency_ms;
            day.latency_n += 1;
        }

        Sample {
            words,
            wpm,
            avg_wpm: self.avg_wpm(),
        }
    }

    /// Replays the recordings index into the stats exactly once, so the card
    /// is populated from dictations made before the stats existed. App names
    /// and paste latency weren't captured back then, so those two columns
    /// only fill going forward.
    pub fn backfill_history(&mut self, entries: Vec<recordings::Entry>) {
        if self.migrated {
            return;
        }
        let mut ordered = entries;
        ordered.sort_by_key(|entry| entry.time_ms);
        // Oldest first, so the rolling speed window ends on the newest.
        for entry in ordered {
            self.record(Input {
                heard: &entry.heard,
                text: &entry.text,
                speech_s: entry.seconds,
                latency_ms: None,
                app: None,
                day: &local_day_key(entry.time_ms),
            });
        }
        self.migrated = true;
    }

    fn avg_wpm(&self) -> Option<f32> {
        if self.wpms.is_empty() {
            return None;
        }
        Some(self.wpms.iter().sum::<f32>() / self.wpms.len() as f32)
    }

    fn day(&self, key: &str) -> Day {
        self.days.get(key).cloned().unwrap_or_default()
    }

    /// Current streak: today, or yesterday when today hasn't had words yet
    /// (a day without dictation doesn't retroactively break the streak).
    pub fn streak_from(&self, today: &str) -> u32 {
        let mut streak = 0;
        let mut day = today.to_string();
        if self.day(&day).dictations == 0 {
            let Some(previous) = prev_day_key(&day) else {
                return 0;
            };
            day = previous;
        }
        while self.day(&day).dictations > 0 {
            streak += 1;
            let Some(previous) = prev_day_key(&day) else {
                break;
            };
            day = previous;
        }
        streak
    }

    fn longest_streak(&self) -> u32 {
        let mut keys: Vec<String> = self
            .days
            .iter()
            .filter(|(_, d)| d.dictations > 0)
            .map(|(k, _)| k.clone())
            .collect();
        keys.sort();
        let mut longest = 0;
        let mut chain = 0;
        let mut previous: Option<String> = None;
        for key in keys {
            if previous
                .as_deref()
                .is_some_and(|p| next_day_key(p).as_deref() == Some(key.as_str()))
            {
                chain += 1;
            } else {
                chain = 1;
            }
            longest = longest.max(chain);
            previous = Some(key);
        }
        longest
    }

    /// The last seven days, oldest first.
    pub fn week(&self, today: &str) -> Vec<WeekDay> {
        let mut keys = vec![today.to_string()];
        while keys.len() < 7 {
            let Some(previous) = prev_day_key(keys.last().expect("starts with today")) else {
                break;
            };
            keys.push(previous);
        }
        keys.reverse();
        keys.into_iter()
            .map(|date| WeekDay {
                words: self.day(&date).words,
                date,
            })
            .collect()
    }

    /// Everything the settings screen shows, in one shot.
    pub fn usage(&self, today: &str) -> Usage {
        let (mut words, mut dictations, mut cleaned) = (0, 0, 0);
        for day in self.days.values() {
            words += day.words;
            dictations += day.dictations;
            cleaned += day.cleaned;
        }
        Usage {
            words,
            words_today: self.day(today).words,
            dictations,
            wpm: self.avg_wpm().unwrap_or(0.0),
            saved_min: words as f32 / TYPING_WPM,
            streak: self.streak_from(today),
            longest_streak: self.longest_streak(),
            week: self.week(today),
            top_apps: {
                let mut apps: Vec<AppWords> = self
                    .apps
                    .iter()
                    .map(|(name, &words)| AppWords {
                        name: name.clone(),
                        words,
                    })
                    .collect();
                apps.sort_by(|a, b| b.words.cmp(&a.words));
                apps.truncate(3);
                apps
            },
            avg_latency_ms: {
                let (mut ms, mut n) = (0, 0);
                for day in self.days.values() {
                    ms += day.latency_ms;
                    n += day.latency_n;
                }
                if n > 0 { (ms / u64::from(n)) as u32 } else { 0 }
            },
            cleaned,
        }
    }

    pub fn reset(&mut self) {
        *self = Self::default();
    }
}

pub fn count_words(text: &str) -> u32 {
    text.split_whitespace().count() as u32
}

fn path() -> PathBuf {
    crate::settings::app_root().join("stats.json")
}

/// Today as a local-day key ("2026-10-22").
#[cfg(windows)]
pub fn today_key() -> String {
    // SAFETY: GetLocalTime only fills the struct it's given.
    let mut t = unsafe { std::mem::zeroed() };
    unsafe { windows_sys::Win32::System::SystemInformation::GetLocalTime(&mut t) };
    format!("{:04}-{:02}-{:02}", t.wYear, t.wMonth, t.wDay)
}

#[cfg(not(windows))]
pub fn today_key() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let (year, month, day) = civil_from_days((secs / 86_400) as i64);
    format!("{year:04}-{month:02}-{day:02}")
}

/// The local calendar day a timestamp falls in ("2026-10-22"); used to
/// replay recordings made before the stats started labelling their days.
fn local_day_key(time_ms: u64) -> String {
    day_key_for_offset(time_ms, utc_offset_ms())
}

fn day_key_for_offset(time_ms: u64, offset_ms: i64) -> String {
    let (year, month, day) = civil_from_days((time_ms as i64 + offset_ms).div_euclid(86_400_000));
    format!("{year:04}-{month:02}-{day:02}")
}

/// How far local time is ahead of UTC, in milliseconds.
#[cfg(windows)]
fn utc_offset_ms() -> i64 {
    // SAFETY: GetTimeZoneInformation only fills the struct it's given.
    let mut tzi = unsafe { std::mem::zeroed() };
    // SAFETY: the call only reads the pointer it is passed.
    let state = unsafe { windows_sys::Win32::System::Time::GetTimeZoneInformation(&mut tzi) };
    // Per the docs, UTC = local time + bias, so local stays ahead of UTC by
    // the negated bias. State 2 is daylight saving time, when the daylight
    // bias applies; otherwise the standard bias is the active adjustment.
    let bias = if state == 2 {
        tzi.Bias + tzi.DaylightBias
    } else {
        tzi.Bias + tzi.StandardBias
    };
    -i64::from(bias) * 60_000
}

#[cfg(not(windows))]
fn utc_offset_ms() -> i64 {
    0
}

pub fn load() -> Stats {
    load_from(&path())
}

pub fn load_from(file: &Path) -> Stats {
    fs::read_to_string(file)
        .ok()
        .and_then(|s| from_json(&s))
        .unwrap_or_default()
}

fn from_json(text: &str) -> Option<Stats> {
    serde_json::from_str(text).ok()
}

impl Stats {
    /// Writes the stats file. Mutating calls (record, reset) leave saving
    /// to the caller so tests never touch the real file.
    pub fn save(&self) {
        store(&path(), self);
    }
}

/// Writes the file; a failed save must never take dictation down, so
/// failures only log.
pub fn store(file: &Path, stats: &Stats) {
    if let Err(e) = save_stats(file, stats) {
        log::warn!("Couldn't save the dictation stats: {e}");
    }
}

fn save_stats(file: &Path, stats: &Stats) -> Result<()> {
    fs::create_dir_all(file.parent().unwrap_or(&std::env::temp_dir()))?;
    fs::write(file, serde_json::to_string_pretty(stats)?)?;
    Ok(())
}

/// The day before the given key ("2026-10-22" -> "2026-10-21").
pub fn prev_day_key(key: &str) -> Option<String> {
    let (year, month, day) = parse_key(key)?;
    if day > 1 {
        Some(format!(
            "{year:04}-{month:02}-{day:02}",
            year = year,
            month = month,
            day = day - 1
        ))
    } else if month > 1 {
        let previous = month - 1;
        let last = days_in_month(year, previous);
        Some(format!("{year:04}-{previous:02}-{last:02}"))
    } else if year > 1 {
        Some(format!("{:04}-12-31", year - 1))
    } else {
        None
    }
}

fn next_day_key(key: &str) -> Option<String> {
    let (year, month, day) = parse_key(key)?;
    if day < days_in_month(year, month) {
        let next = day + 1;
        Some(format!("{year:04}-{month:02}-{next:02}"))
    } else if month < 12 {
        let next = month + 1;
        Some(format!("{year:04}-{next:02}-01"))
    } else {
        Some(format!("{:04}-01-01", year + 1))
    }
}

fn parse_key(key: &str) -> Option<(u32, u32, u32)> {
    let mut parts = key.split('-').map(|p| p.parse::<u32>().ok());
    match (parts.next(), parts.next(), parts.next(), parts.next()) {
        (Some(Some(y)), Some(Some(m)), Some(Some(d)), None)
            if (1..=12).contains(&m) && (1..=31).contains(&d) =>
        {
            Some((y, m, d))
        }
        _ => None,
    }
}

fn days_in_month(year: u32, month: u32) -> u32 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        2 if (year % 4 == 0 && year % 100 != 0) || year % 400 == 0 => 29,
        2 => 28,
        _ => 30,
    }
}

/// Days since 1970-01-01 to a calendar date (Howard Hinnant's algorithm).
fn civil_from_days(z: i64) -> (u32, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    ((if m <= 2 { y + 1 } else { y }) as u32, m as u32, d as u32)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(
        stats: &mut Stats,
        text: &str,
        heard: &str,
        speech_s: f32,
        app: &str,
        day: &str,
        latency_ms: u64,
    ) -> Sample {
        stats.record(Input {
            heard,
            text,
            speech_s,
            app: Some(app),
            day,
            latency_ms: Some(latency_ms),
        })
    }

    #[test]
    fn a_verified_dictation_lands_in_the_day_and_speed_window() {
        let mut stats = Stats::default();
        let sample = record(
            &mut stats,
            "the launch is slipping to monday",
            "um the the launch is slipping to monday",
            4.0,
            "outlook.exe",
            "2026-10-22",
            900,
        );
        assert_eq!(sample.words, 6);
        assert_eq!(sample.wpm, Some(90.0));
        assert_eq!(sample.avg_wpm, Some(90.0));
        let usage = stats.usage("2026-10-22");
        assert_eq!(usage.words, 6);
        assert_eq!(usage.words_today, 6);
        assert_eq!(usage.dictations, 1);
        assert_eq!(usage.cleaned, 2);
        assert_eq!(usage.streak, 1);
        assert_eq!(usage.avg_latency_ms, 900);
        assert_eq!(
            usage.top_apps,
            vec![AppWords {
                name: "outlook.exe".into(),
                words: 6
            }]
        );
    }

    #[test]
    fn short_blips_count_words_but_not_speed() {
        let mut stats = Stats::default();
        let sample = record(
            &mut stats,
            "hello there",
            "hello there",
            0.7,
            "code.exe",
            "2026-10-22",
            400,
        );
        assert_eq!(sample.words, 2);
        assert_eq!(sample.wpm, None);
        assert_eq!(sample.avg_wpm, None);
        // The day still counts it.
        assert_eq!(stats.usage("2026-10-22").dictations, 1);
    }

    #[test]
    fn empty_results_are_not_counted() {
        let mut stats = Stats::default();
        let sample = record(&mut stats, "", "", 5.0, "chrome.exe", "2026-10-22", 800);
        assert_eq!(sample.words, 0);
        assert_eq!(stats.usage("2026-10-22").dictations, 0);
        assert_eq!(stats.usage("2026-10-22").words, 0);
    }

    #[test]
    fn the_speed_window_keeps_the_twenty_latest() {
        let mut stats = Stats::default();
        let text = vec!["word"; 10].join(" ");
        // Ten brisk samples (150 wpm), then twenty rushed ones (capped 400).
        for i in 0..30 {
            let speech_s = if i < 10 { 4.0 } else { 1.0 };
            let sample = record(
                &mut stats,
                &text,
                "",
                speech_s,
                "code.exe",
                "2026-10-22",
                500,
            );
            if i == 0 {
                assert_eq!(sample.wpm, Some(150.0));
                assert_eq!(sample.avg_wpm, Some(150.0));
            }
        }
        // Thirty qualifying samples pushed, only the newest twenty kept.
        assert_eq!(stats.wpms.len(), WPMS_KEPT);
        assert!((stats.usage("2026-10-22").wpm - 400.0).abs() < 0.01);
    }

    #[test]
    fn streaks_walk_back_from_today_or_yesterday() {
        let mut stats = Stats::default();
        record(
            &mut stats,
            "one two three",
            "",
            3.0,
            "code.exe",
            "2026-10-22",
            500,
        );
        record(
            &mut stats,
            "one two three",
            "",
            3.0,
            "code.exe",
            "2026-10-21",
            500,
        );
        assert_eq!(stats.usage("2026-10-22").streak, 2);
        // A day without dictation keeps yesterday's streak visible.
        assert_eq!(stats.usage("2026-10-23").streak, 2);
        // Two silent days break it.
        assert_eq!(stats.usage("2026-10-24").streak, 0);
    }

    #[test]
    fn streaks_and_weeks_cross_months_and_leap_februaries() {
        let mut stats = Stats::default();
        record(
            &mut stats,
            "one two three",
            "",
            3.0,
            "code.exe",
            "2026-10-01",
            500,
        );
        record(
            &mut stats,
            "one two three",
            "",
            3.0,
            "code.exe",
            "2026-10-01",
            500,
        );
        record(
            &mut stats,
            "one two three",
            "",
            3.0,
            "code.exe",
            "2026-09-30",
            500,
        );
        let usage = stats.usage("2026-10-01");
        assert_eq!(usage.streak, 2);
        assert_eq!(usage.week[0].date, "2026-09-25");
        assert_eq!(usage.week[6].words, 6);
        assert_eq!(next_day_key("2026-10-31").as_deref(), Some("2026-11-01"));
        assert_eq!(next_day_key("2026-12-31").as_deref(), Some("2027-01-01"));
        assert_eq!(prev_day_key("2026-01-01").as_deref(), Some("2025-12-31"));
        // 2028 is a leap year.
        assert_eq!(prev_day_key("2028-03-01").as_deref(), Some("2028-02-29"));
        assert_eq!(next_day_key("2028-02-29").as_deref(), Some("2028-03-01"));
    }

    #[test]
    fn longest_streak_counts_the_best_run() {
        let mut stats = Stats::default();
        for day in ["2026-10-20", "2026-10-21", "2026-10-22"] {
            record(&mut stats, "one two three", "", 3.0, "code.exe", day, 400);
        }
        for day in ["2026-09-10", "2026-09-11", "2026-09-12", "2026-09-13"] {
            record(&mut stats, "one two three", "", 3.0, "code.exe", day, 400);
        }
        let usage = stats.usage("2026-10-22");
        assert_eq!(usage.streak, 3);
        assert_eq!(usage.longest_streak, 4);
    }

    #[test]
    fn top_apps_rank_by_words() {
        let mut stats = Stats::default();
        record(
            &mut stats,
            "one two three four",
            "",
            4.0,
            "chrome.exe",
            "2026-10-22",
            500,
        );
        record(
            &mut stats,
            "one two",
            "",
            2.0,
            "outlook.exe",
            "2026-10-22",
            500,
        );
        record(
            &mut stats,
            "one two three",
            "",
            3.0,
            "chrome.exe",
            "2026-10-22",
            500,
        );
        let top = stats.usage("2026-10-22").top_apps;
        assert_eq!(top.len(), 2);
        assert_eq!(top[0].name, "chrome.exe");
        assert_eq!(top[0].words, 7);
        assert_eq!(top[1].words, 2);
    }

    #[test]
    fn stats_survive_a_save_load_roundtrip() {
        let file = std::env::temp_dir().join("sruthi-stats-roundtrip-test.json");
        let _ = std::fs::remove_file(&file);
        let mut stats = Stats::default();
        record(
            &mut stats,
            "one two three four five",
            "",
            5.0,
            "code.exe",
            "2026-10-22",
            600,
        );
        store(&file, &stats);
        let loaded = load_from(&file);
        assert_eq!(loaded.usage("2026-10-22").words, 5);
        assert_eq!(loaded.usage("2026-10-22").avg_latency_ms, 600);
        std::fs::remove_file(&file).ok();
    }

    #[test]
    fn a_broken_stats_file_starts_empty() {
        let file = std::env::temp_dir().join("sruthi-stats-broken-test.json");
        std::fs::write(&file, "not json at all").unwrap();
        assert_eq!(load_from(&file).usage("2026-10-22").dictations, 0);
        std::fs::remove_file(&file).ok();
    }

    /// Two recordings from 1970-01-01/1969-12-31 territory; a "today" day key
    /// can never match, so day-dependent asserts stay deterministic.
    fn historical_entries() -> Vec<recordings::Entry> {
        vec![
            recordings::Entry {
                id: "a".into(),
                time_ms: 1_000,
                seconds: 5.0,
                heard: "um the launch slips".into(),
                text: "The launch slips.".into(),
            },
            recordings::Entry {
                id: "b".into(),
                time_ms: 2_000,
                seconds: 4.0,
                heard: "hello there".into(),
                text: "Hello there.".into(),
            },
        ]
    }

    #[test]
    fn recordings_made_before_the_feature_fill_the_card() {
        let mut stats = Stats::default();
        stats.backfill_history(historical_entries());

        let usage = stats.usage("2026-10-22");
        // "The launch slips." (3) + "Hello there." (2).
        assert_eq!(usage.words, 5);
        assert_eq!(usage.dictations, 2);
        // "um" trimmed from the first recording.
        assert_eq!(usage.cleaned, 1);
        // Speed: the first qualifies (3 words / 5 s = 36 wpm), the second
        // doesn't (2 words). The averages feed the pill pace from history.
        assert!((usage.wpm - 36.0).abs() < 0.01);
        // The recordings don't land today, and app names / paste latency
        // weren't captured back then — those columns start from now on.
        assert_eq!(usage.words_today, 0);
        assert_eq!(usage.streak, 0);
        assert!(usage.top_apps.is_empty());
        assert_eq!(usage.avg_latency_ms, 0);
    }

    #[test]
    fn the_backfill_rolls_in_exactly_once() {
        let mut stats = Stats::default();
        stats.backfill_history(historical_entries());
        // A second startup replays nothing: the words already counted.
        stats.backfill_history(historical_entries());
        assert_eq!(stats.usage("2026-10-22").words, 5);

        // The one-shot flag survives restarts so later launches stay no-ops.
        let file = std::env::temp_dir().join("sruthi-stats-backfill-test.json");
        let _ = std::fs::remove_file(&file);
        store(&file, &stats);
        assert!(load_from(&file).migrated);
        std::fs::remove_file(&file).ok();
    }

    #[test]
    fn timestamps_map_to_local_days() {
        // UTC instant 0 is still Jan 1 in India (+5:30) but Dec 31 at UTC-5.
        assert_eq!(day_key_for_offset(0, 19_800_000), "1970-01-01");
        assert_eq!(day_key_for_offset(0, -18_000_000), "1969-12-31");
        // 23:40 UTC in India is already tomorrow.
        assert_eq!(day_key_for_offset(85_200_000, 19_800_000), "1970-01-02");
        assert_eq!(civil_from_days(0), (1970, 1, 1));
    }
}
