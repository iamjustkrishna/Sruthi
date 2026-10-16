//! User settings, stored as JSON next to the models.

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

#[derive(Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Settings {
    pub model: Option<String>,
    pub overlay: OverlayPosition,
}

pub fn app_root() -> PathBuf {
    dirs::data_local_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("SrutamDictate")
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
