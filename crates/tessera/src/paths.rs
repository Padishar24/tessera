//! Locations of configuration and logs; loading the configuration.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use anyhow::Context;
use tessera_core::config::{DEFAULT_CONFIG, LoadedConfig};
use tessera_core::layout::DEFAULT_LAYOUTS;

const APP_DIR: &str = "Tessera";
/// Folder under the former project name.
const LEGACY_APP_DIR: &str = "TilingMgr";

/// `%APPDATA%\Tessera`
pub fn config_dir() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join(APP_DIR)
}

pub fn config_file() -> PathBuf {
    config_dir().join("config.toml")
}

/// `%LOCALAPPDATA%\Tessera\logs`
pub fn log_dir() -> PathBuf {
    dirs::data_local_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join(APP_DIR)
        .join("logs")
}

/// `layouts.toml` lives next to `config.toml`.
pub fn layouts_file_for(config: &Path) -> PathBuf {
    config.with_file_name("layouts.toml")
}

/// `state.json` (state across restarts) lives next to `config.toml`.
pub fn state_file_for(config: &Path) -> PathBuf {
    config.with_file_name("state.json")
}

/// `scenes.toml` (saved scenes) lives next to `config.toml`.
pub fn scenes_file_for(config: &Path) -> PathBuf {
    config.with_file_name("scenes.toml")
}

/// Creates the default files on first start; returns the path of `config.toml`.
/// A configuration under the former name (`%APPDATA%\TilingMgr`) is
/// taken over first.
pub fn ensure_config() -> io::Result<PathBuf> {
    migrate_legacy_config_dir()?;
    let path = config_file();
    let layouts = layouts_file_for(&path);
    for (file, content) in [(&path, DEFAULT_CONFIG), (&layouts, DEFAULT_LAYOUTS)] {
        if !file.exists() {
            fs::create_dir_all(config_dir())?;
            fs::write(file, content)?;
            tracing::info!("Default file created: {}", file.display());
        }
    }
    Ok(path)
}

/// Renames `%APPDATA%\TilingMgr` to `%APPDATA%\Tessera` as long as the
/// new folder does not exist yet (so configuration, layouts and state are
/// preserved).
fn migrate_legacy_config_dir() -> io::Result<()> {
    let Some(base) = dirs::config_dir() else {
        return Ok(());
    };
    let (legacy, current) = (base.join(LEGACY_APP_DIR), base.join(APP_DIR));
    if legacy.is_dir() && !current.exists() {
        fs::rename(&legacy, &current)?;
        tracing::info!(
            "Configuration taken over from {} to {}",
            legacy.display(),
            current.display()
        );
    }
    Ok(())
}

/// Loads `config.toml` and the adjacent `layouts.toml`. If `layouts.toml`
/// is missing, the built-in layouts apply.
pub fn load_config(path: &Path) -> anyhow::Result<LoadedConfig> {
    let text = fs::read_to_string(path).with_context(|| path.display().to_string())?;
    let layouts_path = layouts_file_for(path);
    let layouts = if layouts_path.exists() {
        fs::read_to_string(&layouts_path).with_context(|| layouts_path.display().to_string())?
    } else {
        DEFAULT_LAYOUTS.to_owned()
    };
    Ok(LoadedConfig::parse_with_layouts(&text, &layouts)?)
}
