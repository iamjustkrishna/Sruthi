//! User settings, stored as JSON next to the models.

use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

use anyhow::Result;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "lowercase")]
pub enum OverlayPosition {
    /// Centered just above the taskbar.
    #[default]
    Bottom,
    /// Under the text field being typed into; bottom center when the app
    /// doesn't report one.
    #[serde(alias = "cursor")]
    Field,
}

/// Which palette the windows wear.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "lowercase")]
pub enum Theme {
    /// Match the Windows color mode, live.
    #[default]
    System,
    Light,
    Dark,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub model: Option<String>,
    pub overlay: OverlayPosition,
    /// Clean up dictations with the local LLM (when one is installed).
    pub smart: bool,
    /// Which LLM to use for Smart formatting.
    pub llm: Option<String>,
    /// Personal dictionary: names, product terms and rare words.
    pub dictionary: Vec<String>,
    /// Push-to-talk shortcut, e.g. "Ctrl+Alt+Space" (None: the default).
    pub hotkey: Option<String>,
    /// Keep the microphone open so recording starts instantly.
    pub instant_mic: bool,
    /// Microphone to record from (None: Windows' default).
    pub microphone: Option<String>,
    /// Window palette: follow the system, or force light / dark.
    pub theme: Theme,
    /// Cloud provider for Smart formatting ("gemini", "groq", "anthropic");
    /// None: the local model. API keys are kept in Credential Manager.
    pub cloud: Option<String>,
    /// Model chosen per cloud provider (missing: the provider's default).
    pub cloud_models: HashMap<String, String>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            model: None,
            overlay: OverlayPosition::default(),
            smart: true,
            llm: None,
            dictionary: Vec::new(),
            hotkey: None,
            instant_mic: true,
            microphone: None,
            cloud: None,
            cloud_models: HashMap::new(),
            theme: Theme::default(),
        }
    }
}

/// The app used to keep everything under "SrutamDictate"; the first run
/// under the new name adopts that folder whole (models, recordings,
/// settings, stats) instead of starting empty.
fn adopt_legacy_dir(root: &std::path::Path) {
    if root.exists() {
        return;
    }
    let Some(parent) = root.parent() else { return };
    let legacy = parent.join("SrutamDictate");
    if legacy.is_dir() {
        let _ = fs::rename(&legacy, root);
    }
}

pub fn app_root() -> PathBuf {
    let root = dirs::data_local_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("Sruthi");
    adopt_legacy_dir(&root);
    root
}

fn path() -> PathBuf {
    app_root().join("settings.json")
}

pub fn load() -> Settings {
    fs::read_to_string(path())
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

pub fn save(settings: &Settings) -> Result<()> {
    fs::create_dir_all(app_root())?;
    fs::write(path(), serde_json::to_string_pretty(settings)?)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn theme_defaults_to_following_the_system() {
        let settings: Settings = Default::default();
        assert_eq!(settings.theme, Theme::System);
        // Stored lowercase, so the file stays readable by hand.
        assert_eq!(
            serde_json::to_string(&settings.theme).unwrap(),
            "\"system\""
        );
    }

    #[test]
    fn adopts_the_legacy_data_folder_once() {
        let base = std::env::temp_dir().join(format!("sruthi-migrate-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(base.join("SrutamDictate").join("recordings")).unwrap();
        fs::write(base.join("SrutamDictate").join("stats.json"), "{}").unwrap();

        let root = base.join("Sruthi");
        adopt_legacy_dir(&root);
        assert!(root.join("recordings").is_dir());
        assert!(!base.join("SrutamDictate").exists());
    }

    #[test]
    fn leaves_an_existing_new_data_folder_alone() {
        let base = std::env::temp_dir().join(format!("sruthi-nomigrate-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(base.join("SrutamDictate")).unwrap();
        fs::create_dir_all(base.join("Sruthi")).unwrap();

        let root = base.join("Sruthi");
        adopt_legacy_dir(&root);
        assert!(base.join("SrutamDictate").exists());
    }

    #[test]
    fn the_chosen_theme_survives_a_roundtrip() {
        let mut settings = Settings::default();
        settings.theme = Theme::Dark;
        store_roundtrip(&mut settings, Theme::Dark);
        settings.theme = Theme::Light;
        store_roundtrip(&mut settings, Theme::Light);
        settings.theme = Theme::System;
        store_roundtrip(&mut settings, Theme::System);
    }

    fn store_roundtrip(settings: &mut Settings, expected: Theme) {
        let json = serde_json::to_string(settings).unwrap();
        let loaded: Settings = serde_json::from_str(&json).unwrap();
        assert_eq!(loaded.theme, expected);
    }
}
