# Tessera

A window tiling manager for Windows 11 that combines **PowerToys FancyZones** with the
**tiling of the Cosmic desktop**:

- A layout splits each monitor into **zones** (like FancyZones).
- Each zone is its own **tiling tree** (like Cosmic): several windows in one zone are tiled
  automatically, stacked (tabs), or fill the zone (monocle).
- The `single` layout (one zone per monitor) gives you pure auto-tiling.
- Each monitor **and** each virtual desktop has its own layout.

> [!WARNING]
> **This app was coded entirely by AI** (Anthropic's Claude), directed and tested by a human but
> without a line-by-line human code review. It hooks the keyboard and moves other programs'
> windows. Review the code before you rely on it, and use it at your own risk.

## Getting started

Download `tessera-<version>-x64.zip` (or `-arm64`) from the
[releases](https://github.com/Padishar24/tessera/releases), unzip it to a folder of your choice
and run `tessera.exe`. No installation or administrator rights are needed. Keep the folder where
it is once you have turned on "Start with Windows": the autostart entry points to that path.

The binaries are not signed yet, so Windows SmartScreen may warn on first start
("More info" → "Run anyway").

To build it yourself:

```bash
cargo build --release
target/release/tessera.exe
```

On first start, Tessera creates `%APPDATA%\Tessera\config.toml` and `layouts.toml`. It runs in
the system tray. From the tray menu you can open the configuration or the zone editor and turn
on "Start with Windows". On exit, every window returns to its original
position; maximized windows are maximized again.

Logs are written to `%LOCALAPPDATA%\Tessera\logs`.

## Usage

| Shortcut (default) | Effect |
|---|---|
| `Win+Alt+H/J/K/L`, `Win+Alt+Arrows` | Focus left/down/up/right, across zones and monitors |
| `Win+Alt+Shift+H/J/K/L`, `Win+Alt+Shift+Arrows` | Move the window: swap with its neighbor inside the same zone, otherwise move it into the neighboring zone (placed automatically there), or onto the neighboring monitor at the edge |
| `Win+Alt+1..9` | Move the window to zone *n* |
| `Win+Alt+^` / `Win+Alt+Shift+^` | Next / previous layout |
| `Win+Alt+F` | Toggle floating |
| `Win+Alt+S`, `Win+Alt+Tab` | Turn the whole zone into a stack (or back to auto), cycle through the stack |
| `Win+Alt+O` | Arrange the zone as columns ↔ rows |
| `Win+Alt+Plus/Minus` | Grow/shrink |
| `Win+Alt+Space` | Monocle (window fills the zone) |
| `Win+Alt+P` | Pause tiling on this monitor |
| `Win+Alt+W` | Window hints: jump to any window by typing its letters (see below) |
| `Win+Ctrl+Alt+1..4` / `+Shift` | Switch desktop / move the window there |
| `Win+Alt+E` | Zone editor |
| `Win+Alt+Shift+C` | Reload the configuration |

**With the mouse:**
- **Hold Shift while dragging:** the zones appear with a preview of where the window will
  land. Release it on the edge of a window to insert it beside that window; release it in the
  middle to add it to that window's stack.
- **Drag without Shift:** the window swaps places with the window under the cursor, or moves
  to another monitor.
- **Drag an edge between two tiles:** changes the split.
- **Click a tab above a stack:** switches the visible window. The **×** on a tab, or a
  middle-click on it, closes that window (`stack_close_button = false` under `[appearance]`
  hides the button).

Windows you maximize yourself stay maximized and return to their tile when restored.

## Configuration

- `config.toml`:
  - Gaps, colors, the key that shows the zones while dragging
  - Keyboard shortcuts (`"win+alt+h" = "focus left"`)
  - Window rules (`[[rules]]` with `exe`, `class` or `title` → `ignore`, `float` or `zone`)
  - Scratchpads (`[[scratchpads]]`, see below)
- `layouts.toml`: layouts as a template (`columns`, `rows`, `grid`, `priority-grid`), as a
  nested split (`split`), or as free rectangles in percent (`canvas`).
- Both files are **reloaded automatically** when you save them. If there is an error, the
  previous configuration stays active and a message appears.
- `state.json` remembers the layout and pause state per monitor and desktop, the mode of each
  zone, and each window's zone. After a restart, windows land back in their zones.

### Window hints

`Win+Alt+W` (action `hint`) puts a badge with one or two letters in the middle of every
visible window on all monitors. Type the letters to act on that window:

| Typed | Effect |
|---|---|
| Letters | Focus the window |
| `Shift` + letters | Swap it with the focused window (both tiled; also across zones and monitors) |
| `Ctrl` + letters | Pull it into the focused window's stack |
| `Backspace` | Take back a letter |
| `Esc` or any other key | Cancel |

While the badges are shown, Tessera captures the keyboard. The capture ends after 8 seconds
without typing at the latest (the keyboard hook ends it by itself after 10 seconds).

### Scratchpads

A scratchpad is a window that stays minimized until its shortcut summons it, centered (or at
the top edge) of the monitor you are working on, in front of everything. The same shortcut
hides it again; if it is visible but behind other windows, the shortcut brings it to the
front.

```toml
[[scratchpads]]
name = "notes"
exe = "notepad\\.exe"        # patterns like in [[rules]]
title = "scratch"
command = "notepad.exe"      # started if no window matches
args = ["%USERPROFILE%\\scratch.txt"]
key = "win+alt+n"
width = 50                   # percent of the work area
height = 60
position = "center"          # or "top"
hide_on_focus_loss = false   # hide it as soon as another window gets the focus
```

- The first window that matches becomes the scratchpad; further windows of the program are
  managed as usual. A scratchpad always floats.
- Windows that already exist when Tessera starts (or when the configuration adds the
  scratchpad) are minimized right away. A window you start yourself, or that `command` starts,
  is shown.
- Hidden scratchpads stay in the taskbar; clicking them there summons them as well. Summoning
  also brings a scratchpad over from another virtual desktop.
- Instead of `key`, any shortcut can use the action `toggle-scratchpad <n>` (the *n*-th
  `[[scratchpads]]` entry).

### Scenes

A scene is a named snapshot of the arrangement on all monitors of the current desktop: the
layout and zone modes of each monitor and which program sits in which zone. Applying it
restores the layouts and moves the tiled windows of those programs back into their zones.

- **Tray menu → Scenes → Save for this monitor setup** saves a scene named after the connected
  monitors that is applied **automatically** whenever exactly these monitors are connected
  again, e.g. when you dock or undock a laptop.
- **Tray menu → Scenes → *name*** applies a scene.
- From the command line:

  ```bash
  tesserac scene save coding            # --auto: apply with these monitors; --launch: see below
  tesserac scene apply coding
  tesserac scene list
  tesserac scene delete coding
  ```

Windows are matched by program and window class, the same title first. With `launch = true`,
applying a scene also starts programs that have no window yet (by the path saved with the
scene, not for Store apps); their windows land in their zones when they appear within a
minute. Scenes are stored in `scenes.toml` next to `config.toml`, where you can also switch
`auto` and `launch` on or off.

### File Explorer tabs

With `explorer_tabs = true` under `[general]` (off by default), `Win+E` opens a new tab in the
topmost File Explorer window on the current desktop instead of another window, restoring that
window if it is minimized. Without such a window, a new one opens as usual. A `win+e` entry
under `[keybindings]` takes precedence. The action is also available as `open-explorer`, e.g.
for another shortcut or `tesserac msg open-explorer`.

### Zone modes

Each zone has a mode that decides how its windows are arranged. New windows follow it, and it
is remembered across restarts:

| Mode | Arrangement | Set with |
|---|---|---|
| `auto` (default) | A new window splits the focused one along its longer side | `Win+Alt+S` on a stack zone |
| `stack` | All windows of the zone in one stack with tabs | `Win+Alt+S` |
| `columns` | All windows side by side | `Win+Alt+O` |
| `rows` | All windows on top of each other | `Win+Alt+O` |

The mode belongs to the zone of a layout on a given monitor and desktop. When a monitor
switches to a layout with a different number of zones, its zones start again in `auto`.

Shortcuts marked ⚠ in `config.toml` override built-in Windows shortcuts; delete the line to
keep the Windows function. `Win+L` and `Ctrl+Alt+Del` cannot be assigned.

## Zone editor

Open it with `Win+Alt+E`, from the tray ("Open zone editor"), or with `tessera editor`. The
editor lies transparently over the monitor and shows the zones at their actual size.

| Input | Effect |
|---|---|
| Click | Split the zone vertically |
| Shift+click | Split the zone horizontally |
| Right-click | Remove the zone |
| Drag a divider | Resize |
| Esc | Close the editor |

"Save & apply" writes `layouts.toml` and keeps its comments and all
other layouts. The editor then applies the layout to the monitor immediately.

## Command line

`tesserac.exe` controls the running daemon through a named pipe:

```bash
tesserac msg focus left
tesserac msg move-to-zone 2
tesserac set-layout columns-3
tesserac query windows
tesserac query state
tesserac scene list
tesserac check-config
tesserac desktop list
```

## Architecture

| Crate | Contents |
|---|---|
| `tessera-core` | Platform-independent logic: configuration, layouts, tiling tree, navigation, state, covered by unit tests (`cargo test`) |
| `tessera-win` | Win32: events, windows, monitors, positioning, keyboard hook, overlays (Direct2D), virtual desktops |
| `tessera-editor` | Zone editor (eframe/egui with glow) |
| `tessera` | Daemon `tessera.exe` and command line `tesserac.exe` |

## Releasing

Bump `version` in `Cargo.toml`, commit, then tag and push:

```bash
git tag vX.Y.Z
git push origin vX.Y.Z
```

The `Release` workflow checks that the tag matches `Cargo.toml`, runs the tests and
`cargo deny` (security advisories, licenses, sources; see `deny.toml`), and attaches
`tessera-<tag>-x64.zip` and `tessera-<tag>-arm64.zip` (with SHA-256 files and a build
provenance attestation) to a GitHub release. Each zip contains `THIRD_PARTY_LICENSES.html`
(generated by `cargo about` from `about.toml`/`about.hbs`) and a CycloneDX SBOM, `sbom.cdx.json`. Releases are immutable: a published version cannot
be changed, so fix mistakes with a new version. See [SECURITY.md](SECURITY.md) for reporting
vulnerabilities and verifying downloads.

## Limitations

- **Windows running as administrator** cannot be moved by a normally started Tessera (UIPI).
  They float instead; to manage them, Tessera itself must run as administrator.
- **Virtual desktops** rely on an undocumented Windows interface (`winvd`, tested with build
  26200). If it breaks after an update, tiling and zones keep working; only the desktop
  shortcuts report an error.
- A window with a **minimum size** can extend beyond its tile. In that case, let it float with a
  rule (`float = true`).
- The editor shows **canvas layouts** but they can only be changed in `layouts.toml`.

## License

MIT, see [LICENSE](LICENSE).
