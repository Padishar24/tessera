//! Watches `config.toml` and `layouts.toml` for changes.

use std::ffi::OsString;
use std::path::Path;

use notify::{EventKind, RecommendedWatcher, RecursiveMode, Watcher};

use crate::paths;

/// Runs as long as the value lives.
pub struct ConfigWatcher {
    _watcher: RecommendedWatcher,
}

/// Calls `on_change` (on a foreign thread) when one of the two files next
/// to `config` is written or replaced. Editors often save several times or
/// via rename – the caller should debounce.
pub fn watch(
    config: &Path,
    on_change: impl Fn() + Send + 'static,
) -> anyhow::Result<ConfigWatcher> {
    let dir = config
        .parent()
        .ok_or_else(|| anyhow::anyhow!("{} has no parent folder", config.display()))?;
    let names: Vec<OsString> = [config.to_path_buf(), paths::layouts_file_for(config)]
        .iter()
        .filter_map(|p| p.file_name().map(ToOwned::to_owned))
        .collect();

    let mut watcher = notify::recommended_watcher(move |result: notify::Result<notify::Event>| {
        let Ok(event) = result else {
            return;
        };
        let relevant = matches!(event.kind, EventKind::Create(_) | EventKind::Modify(_))
            && event
                .paths
                .iter()
                .any(|p| p.file_name().is_some_and(|n| names.iter().any(|w| w == n)));
        if relevant {
            on_change();
        }
    })?;
    watcher.watch(dir, RecursiveMode::NonRecursive)?;
    Ok(ConfigWatcher { _watcher: watcher })
}
