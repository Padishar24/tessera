//! Locations of configuration and logs; loading the configuration.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use anyhow::Context;
use tessera_core::config::{DEFAULT_CONFIG, LoadedConfig};
use tessera_core::layout::DEFAULT_LAYOUTS;
use tessera_win::package;

const APP_DIR: &str = "Tessera";
/// Folder under the former project name.
const LEGACY_APP_DIR: &str = "TilingMgr";

/// Base folders for the configuration (`%APPDATA%`) and the logs
/// (`%LOCALAPPDATA%`).
///
/// The Store version (MSIX) runs with file-system write virtualization:
/// Windows redirects its writes under `%APPDATA%` and `%LOCALAPPDATA%` to
/// `%LOCALAPPDATA%\Packages\<family>\LocalCache\Roaming` and `...\Local`,
/// where other programs do not look. An editor opened from the tray runs
/// outside the package, so Tessera uses these real folders directly.
fn base_dirs() -> (PathBuf, PathBuf) {
    let roaming = dirs::config_dir().unwrap_or_else(std::env::temp_dir);
    let local = dirs::data_local_dir().unwrap_or_else(std::env::temp_dir);
    match package::family_name() {
        Some(family) => {
            let cache = local.join("Packages").join(family).join("LocalCache");
            (cache.join("Roaming"), cache.join("Local"))
        }
        None => (roaming, local),
    }
}

/// `%APPDATA%\Tessera`; in the Store version the package's own folder
/// (see [`base_dirs`]).
pub fn config_dir() -> PathBuf {
    base_dirs().0.join(APP_DIR)
}

pub fn config_file() -> PathBuf {
    config_dir().join("config.toml")
}

/// `%LOCALAPPDATA%\Tessera\logs`; in the Store version under the
/// package's own folder (see [`base_dirs`]).
pub fn log_dir() -> PathBuf {
    base_dirs().1.join(APP_DIR).join("logs")
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
/// An existing configuration is taken over first: in the zip version one
/// under the former name (`%APPDATA%\TilingMgr`), in the Store version the
/// zip version's (`%APPDATA%\Tessera`).
pub fn ensure_config() -> io::Result<PathBuf> {
    if package::is_packaged() {
        copy_unpackaged_config()?;
    } else {
        migrate_legacy_config_dir()?;
    }
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

/// Store version, first start: copies the files of the zip version's
/// configuration folder (or the one under the former name) into the
/// package's folder. Copies, so the zip version keeps working unchanged.
fn copy_unpackaged_config() -> io::Result<()> {
    let target = config_dir();
    if target.join("config.toml").exists() {
        return Ok(());
    }
    let Some(base) = dirs::config_dir() else {
        return Ok(());
    };
    let Some(source) = [APP_DIR, LEGACY_APP_DIR]
        .into_iter()
        .map(|name| base.join(name))
        .find(|dir| dir.join("config.toml").is_file())
    else {
        return Ok(());
    };
    copy_files(&source, &target)?;
    tracing::info!(
        "Configuration copied from {} to {}",
        source.display(),
        target.display()
    );
    Ok(())
}

/// Copies the files (not the subfolders) of `from` into `to`.
fn copy_files(from: &Path, to: &Path) -> io::Result<()> {
    fs::create_dir_all(to)?;
    for entry in fs::read_dir(from)? {
        let entry = entry?;
        if entry.file_type()?.is_file() {
            fs::copy(entry.path(), to.join(entry.file_name()))?;
        }
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
