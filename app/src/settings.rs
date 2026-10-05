use std::fs;
use std::io::Write;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::path::PathBuf;
use std::sync::{LazyLock, Mutex};

use serde::{Deserialize, Serialize};

pub const DEFAULT_TRANSCRIPTION_MODEL: &str = "microsoft/mai-transcribe-2";
pub const DEFAULT_CLEANUP_MODEL: &str = "google/gemini-3.5-flash-lite";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Output {
    Paste,
    Copy,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Sort {
    Price,
    Name,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Shortcut {
    pub key_code: u32,
    pub modifiers: u32,
    pub label: String,
}

impl Default for Shortcut {
    fn default() -> Self {
        Self {
            key_code: 49,
            modifiers: crate::hotkey::OPTION,
            label: "⌥Space".into(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Settings {
    pub transcription_model: String,
    pub cleanup_model: Option<String>,
    pub remove_computer_audio: bool,
    pub output: Output,
    pub instructions: String,
    pub shortcut: Shortcut,
    pub sort: Sort,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            transcription_model: DEFAULT_TRANSCRIPTION_MODEL.into(),
            cleanup_model: Some(DEFAULT_CLEANUP_MODEL.into()),
            remove_computer_audio: true,
            output: Output::Paste,
            instructions: String::new(),
            shortcut: Shortcut::default(),
            sort: Sort::Price,
        }
    }
}

static SETTINGS: LazyLock<Mutex<Settings>> = LazyLock::new(|| Mutex::new(load()));

pub fn support_dir() -> PathBuf {
    #[cfg(feature = "e2e")]
    if let Some(dir) = std::env::var_os("YAPR_SUPPORT_DIR") {
        return PathBuf::from(dir);
    }
    PathBuf::from(std::env::var_os("HOME").unwrap_or_default())
        .join("Library/Application Support/Yapr")
}

pub fn get() -> Settings {
    SETTINGS.lock().unwrap().clone()
}

pub fn update(change: impl FnOnce(&mut Settings)) {
    let snapshot = {
        let mut settings = SETTINGS.lock().unwrap();
        change(&mut settings);
        settings.clone()
    };
    if let Err(error) = write_file(
        "settings.json",
        &serde_json::to_vec_pretty(&snapshot).unwrap(),
    ) {
        crate::overlay::show(crate::overlay::State::Error(format!(
            "Could not save settings: {error}"
        )));
    }
}

fn load() -> Settings {
    fs::read(support_dir().join("settings.json"))
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

pub fn write_file(name: &str, bytes: &[u8]) -> Result<(), String> {
    let dir = support_dir();
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(&dir)
        .map_err(|e| e.to_string())?;
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).map_err(|e| e.to_string())?;
    let temp = dir.join(format!(".{name}.tmp"));
    let _ = fs::remove_file(&temp);
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&temp)
        .map_err(|e| e.to_string())?;
    file.write_all(bytes).map_err(|e| e.to_string())?;
    fs::rename(&temp, dir.join(name)).map_err(|e| e.to_string())
}
