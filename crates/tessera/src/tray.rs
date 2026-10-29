//! Tray icon with menu.

use tray_icon::menu::{CheckMenuItem, Menu, MenuId, MenuItem, PredefinedMenuItem, Submenu};
use tray_icon::{Icon, TrayIcon, TrayIconBuilder};

pub const OPEN_CONFIG: &str = "open-config";
pub const RELOAD_CONFIG: &str = "reload-config";
pub const OPEN_EDITOR: &str = "open-editor";
pub const OPEN_LOGS: &str = "open-logs";
pub const AUTOSTART: &str = "autostart";
pub const SAVE_SCENE: &str = "save-scene";
/// Prefix of the menu items that apply a scene, followed by its name.
pub const SCENE_PREFIX: &str = "scene:";
pub const QUIT: &str = "quit";

/// Tray icon and the menu entries the daemon keeps up to date.
pub struct Tray {
    _icon: TrayIcon,
    /// Its check mark is maintained by the daemon.
    pub autostart: CheckMenuItem,
    scenes: Submenu,
}

pub fn create(autostart: bool) -> anyhow::Result<Tray> {
    let autostart = CheckMenuItem::with_id(
        MenuId::new(AUTOSTART),
        "Start with Windows",
        true,
        autostart,
        None,
    );
    let scenes = Submenu::new("Scenes", true);
    let menu = Menu::new();
    menu.append_items(&[
        &MenuItem::new(concat!("Tessera ", env!("CARGO_PKG_VERSION")), false, None),
        &PredefinedMenuItem::separator(),
        &scenes,
        &PredefinedMenuItem::separator(),
        &MenuItem::with_id(MenuId::new(OPEN_CONFIG), "Open configuration", true, None),
        &MenuItem::with_id(
            MenuId::new(RELOAD_CONFIG),
            "Reload configuration",
            true,
            None,
        ),
        &MenuItem::with_id(MenuId::new(OPEN_EDITOR), "Open zone editor", true, None),
        &MenuItem::with_id(MenuId::new(OPEN_LOGS), "Open log folder", true, None),
        &PredefinedMenuItem::separator(),
        &autostart,
        &PredefinedMenuItem::separator(),
        &MenuItem::with_id(MenuId::new(QUIT), "Exit", true, None),
    ])?;

    let icon = TrayIconBuilder::new()
        .with_tooltip("Tessera")
        .with_icon(icon()?)
        .with_menu(Box::new(menu))
        .with_menu_on_left_click(true)
        .build()?;
    let tray = Tray {
        _icon: icon,
        autostart,
        scenes,
    };
    tray.set_scenes(&[]);
    Ok(tray)
}

impl Tray {
    /// Rebuilds the "Scenes" submenu: one entry per scene (name, whether it
    /// is applied automatically), then the entry that saves the current
    /// arrangement for this monitor setup.
    pub fn set_scenes(&self, scenes: &[(String, bool)]) {
        while self.scenes.remove_at(0).is_some() {}
        let result = (|| -> tray_icon::menu::Result<()> {
            if scenes.is_empty() {
                self.scenes
                    .append(&MenuItem::new("No scenes saved yet", false, None))?;
            }
            for (name, auto) in scenes {
                let text = if *auto {
                    format!("{name} (automatic)")
                } else {
                    name.clone()
                };
                // `&` would mark a mnemonic in the native menu.
                let text = text.replace('&', "&&");
                self.scenes.append(&MenuItem::with_id(
                    MenuId::new(format!("{SCENE_PREFIX}{name}")),
                    text,
                    true,
                    None,
                ))?;
            }
            self.scenes.append(&PredefinedMenuItem::separator())?;
            self.scenes.append(&MenuItem::with_id(
                MenuId::new(SAVE_SCENE),
                "Save for this monitor setup",
                true,
                None,
            ))
        })();
        if let Err(e) = result {
            tracing::warn!("Scene menu could not be updated: {e}");
        }
    }
}

/// Creates the icon at runtime: one tall tile on the left, two on the right.
fn icon() -> anyhow::Result<Icon> {
    const SIZE: u32 = 32;
    const ACCENT: [u8; 4] = [0x62, 0xa0, 0xea, 0xff];
    const LIGHT: [u8; 4] = [0xc8, 0xdc, 0xf5, 0xff];
    let tiles = [
        (2..15, 2..30, ACCENT),
        (17..30, 2..15, LIGHT),
        (17..30, 17..30, LIGHT),
    ];

    let mut rgba = Vec::with_capacity((SIZE * SIZE * 4) as usize);
    for y in 0..SIZE {
        for x in 0..SIZE {
            let pixel = tiles
                .iter()
                .find(|(xs, ys, _)| xs.contains(&x) && ys.contains(&y))
                .map_or([0; 4], |(_, _, color)| *color);
            rgba.extend_from_slice(&pixel);
        }
    }
    Ok(Icon::from_rgba(rgba, SIZE, SIZE)?)
}
