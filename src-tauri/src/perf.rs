// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Sruthi contributors

//! Honest wait-time estimates from this PC's own dictations.
//!
//! Each successful run stores milliseconds spent per audio second. The
//! median of the newest runs is used, so a one-off slow run (a Windows
//! update, a background build) cannot poison the estimate. The file is
//! local-only, like timings.log.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::settings;

/// Enough history to smooth a slow run without hiding a real regression.
const SAMPLES: usize = 12;
const FILE: &str = "perf.json";

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct Entry {
    /// Milliseconds of post-stop wait per audio second.
    samples: Vec<f32>,
    pub backend: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct Estimate {
    /// Milliseconds of wait per audio second.
    pub ms_per_s: f32,
    pub runs: u32,
    pub backend: Option<String>,
}

#[derive(Default, Serialize, Deserialize)]
struct Store {
    models: HashMap<String, Entry>,
}

fn path() -> PathBuf {
    settings::app_root().join(FILE)
}

fn parse(text: &str) -> Store {
    serde_json::from_str(text).unwrap_or_default()
}

fn load() -> Store {
    std::fs::read_to_string(path())
        .map(|text| parse(&text))
        .unwrap_or_default()
}

fn save(store: &Store) {
    if let Ok(text) = serde_json::to_string_pretty(store) {
        let _ = std::fs::create_dir_all(settings::app_root());
        // Write beside the real file and rename, so a crash cannot leave a
        // half-written estimate history.
        let tmp = path().with_extension("json.tmp");
        if std::fs::write(&tmp, text).is_ok() {
            let _ = std::fs::rename(&tmp, path());
        }
    }
}

/// The measured estimate for `model`, if this PC has run it successfully.
pub fn estimate(model: &str) -> Option<Estimate> {
    let store = load();
    estimate_in(&store, model)
}

/// Measured estimates for every model with history, loaded once for the
/// model list.
pub fn all() -> HashMap<String, Estimate> {
    let store = load();
    store
        .models
        .iter()
        .filter_map(|(id, _)| estimate_in(&store, id).map(|e| (id.clone(), e)))
        .collect()
}

fn estimate_in(store: &Store, model: &str) -> Option<Estimate> {
    let entry = store.models.get(model)?;
    let ms_per_s = median(&entry.samples)?;
    Some(Estimate {
        ms_per_s,
        runs: entry.samples.len() as u32,
        backend: entry.backend.clone(),
    })
}

/// Records a successful, non-aborted run. Very short taps are not useful:
/// their fixed overhead would make every estimate look worse.
pub fn record(model: &str, audio_s: f32, waited: Duration, backend: &str) {
    if audio_s < 1.0 || waited.is_zero() {
        return;
    }
    let ms_per_s = waited.as_secs_f32() * 1000.0 / audio_s;
    if !ms_per_s.is_finite() {
        return;
    }
    let mut store = load();
    let entry = store.models.entry(model.to_owned()).or_default();
    push_sample(entry, ms_per_s);
    entry.backend = Some(backend.to_owned());
    save(&store);
}

/// Adds a sample and keeps only the newest `SAMPLES` values.
fn push_sample(entry: &mut Entry, ms_per_s: f32) {
    entry.samples.push(ms_per_s.clamp(1.0, 120_000.0));
    if entry.samples.len() > SAMPLES {
        let excess = entry.samples.len() - SAMPLES;
        entry.samples.drain(..excess);
    }
}

/// Median of a non-empty sample list; `None` for an empty one.
pub fn median(samples: &[f32]) -> Option<f32> {
    if samples.is_empty() {
        return None;
    }
    let mut sorted = samples.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let middle = sorted.len() / 2;
    Some(if sorted.len() % 2 == 0 {
        (sorted[middle - 1] + sorted[middle]) / 2.0
    } else {
        sorted[middle]
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn median_ignores_one_slow_run() {
        let samples = [800.0, 1000.0, 12_000.0];
        assert_eq!(median(&samples), Some(1000.0));
        assert_eq!(median(&[10.0, 20.0]), Some(15.0));
        assert_eq!(median(&[]), None);
    }

    #[test]
    fn only_the_newest_twelve_runs_are_kept() {
        let mut entry = Entry::default();
        for i in 0..20 {
            push_sample(&mut entry, i as f32);
        }
        assert_eq!(entry.samples.len(), SAMPLES);
        assert_eq!(entry.samples.first(), Some(&8.0));
        assert_eq!(entry.samples.last(), Some(&19.0));
    }

    #[test]
    fn a_corrupt_file_reads_as_empty() {
        let store = parse("{ this is not json");
        assert!(store.models.is_empty());
    }
}
