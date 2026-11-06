# Publishing Tessera in the Microsoft Store

Step by step, from the developer account to the first release and later updates. The Store
listing texts are in [listing.md](listing.md); the package is built by
[`packaging/msix/build.ps1`](../msix/build.ps1) locally or by the `Release` workflow.

## 1. Developer account (once)

1. Go to <https://storedeveloper.microsoft.com> and choose **Individual developer**. Registration
   is free for individuals.
2. Sign in with a **personal** Microsoft account (not a work account).
3. Verify your identity with an ID document and a selfie.
4. Choose the **publisher display name**. It is shown under the app in the Store.

## 2. Reserve the app name

Partner Center → **Apps and games** → **New product** → **MSIX or PWA app** → reserve the name.

Tessera is reserved as **Tessera Window Manager**. The package's `<DisplayName>` in
[`packaging/msix/AppxManifest.xml`](../msix/AppxManifest.xml) must match a reserved name
exactly; change it there if you ever reserve a different one. The Start menu entry
(`DisplayName` of the first `<uap:VisualElements>`) can stay the short "Tessera".

## 3. Package identity → GitHub variables

Partner Center → your app → **Product management → Product identity** shows three values. Store
them as repository variables (they are public anyway: they end up in every package):

```bash
gh variable set MSIX_IDENTITY_NAME --body "<Package/Identity/Name>"
gh variable set MSIX_PUBLISHER --body "<Package/Identity/Publisher, starts with CN=>"
gh variable set MSIX_PUBLISHER_DISPLAY_NAME --body "<Package/Properties/PublisherDisplayName>"
```

Without them the workflow still builds a bundle, with a placeholder identity and a warning,
which Partner Center rejects.

## 4. Build the bundle

**Via a release (normal way).** Bump the version and push a tag as usual (README → Releasing).
Besides the GitHub release, the `Release` run has an artifact **`msix-store`** containing
`Tessera_X.Y.Z.0.msixbundle` (x64 + ARM64). Download it from the run page or with:

```bash
gh run download <run-id> --name msix-store
```

The bundle is unsigned on purpose: the Store signs it. It is not attached to the GitHub release
because nobody could install it from there.

**Locally (e.g. for the first test).** Needs the Windows SDK:

```powershell
cargo build --release
pwsh packaging/msix/build.ps1 -X64 target/release `
    -IdentityName "<Name>" -Publisher "CN=<...>" -PublisherDisplayName "<Name>"
```

This builds x64 only; the Store accepts that, but ARM64 users then run the x64 build emulated.

## 5. Test the package before submitting (recommended)

Packaged apps behave differently in a few places (autostart, file access, command-line
aliases). Test once on your own PC:

1. Settings → System → **For developers** → turn on **Developer Mode**.
2. Quit the zip version of Tessera (only one instance can run) and turn off its "Start with
   Windows", otherwise both start at login.
3. Build with the default (placeholder) identity and register the unpacked layout:

   ```powershell
   pwsh packaging/msix/build.ps1 -X64 target/release
   Add-AppxPackage -Register target\msix\layout-x64\AppxManifest.xml
   ```

4. Start **Tessera** from the Start menu and check:
   - [ ] Tray icon appears, shortcuts and tiling work.
   - [ ] Tray → open configuration opens `%APPDATA%\Tessera\config.toml` (the real folder, the
         same file as the zip version uses).
   - [ ] Zone editor (`Win+Alt+E`) and keyboard shortcuts window open.
   - [ ] Tray → "Start with Windows" turns on; Tessera appears under Settings → Apps → Startup;
         after signing out and in, it starts. Turning it off in the tray works too.
   - [ ] In a **new** terminal: `tesserac query state` answers. (`tessera` has no
         command-line alias in the package; that is expected.)
   - [ ] Logs appear in `%LOCALAPPDATA%\Tessera\logs`.
5. Remove the test package afterwards:

   ```powershell
   Get-AppxPackage Tessera.Local | Remove-AppxPackage
   ```

Optional: the **Windows App Certification Kit** (part of the Windows SDK, `appcert.exe`, needs
administrator rights) runs most of the Store's technical checks locally.

## 6. Screenshots

Take at least one, better four or five, PNG, 1920 × 1080 or larger, without personal data
(mails, names, customer documents) in the windows:

1. A monitor tiled into zones with several real programs (editor, browser, terminal).
2. Shift-drag: the zone overlay with the drop preview.
3. Window hints: the letter badges on the windows.
4. The zone editor over a monitor.
5. The keyboard shortcuts window.

Optional captions (up to 200 characters each), e.g. "Zones per monitor, each tiling its windows
automatically".

## 7. Create the submission

Partner Center → your app → **Start submission**.

### Pricing and availability

- Markets: all (default).
- Visibility: **Public audience**, discoverable.
- Pricing: **Free**. No free trial, no sale.
- Release: as soon as possible after certification.

### Properties

- Category: **Utilities & tools**.
- Privacy policy URL: `https://github.com/Padishar24/tessera/blob/main/PRIVACY.md`
- Website: `https://github.com/Padishar24/tessera`
- Support contact: `https://github.com/Padishar24/tessera/issues`
- System requirements: Keyboard and Mouse as minimum hardware.

### Age ratings

Fill in the IARC questionnaire: category **Utility, Productivity, Communication or Other**
(the exact label may differ slightly), then answer **No** to every question. Tessera has no
violence, no user-to-user communication, no sharing of location or personal data, no purchases
and no web browser. Result: rated for all ages (e.g. PEGI 3, ESRB Everyone).

### Packages

Upload `Tessera_X.Y.Z.0.msixbundle`. Partner Center reads the architectures (x64, ARM64) and
the device family (Windows Desktop) from it. Leave the device family selection at Desktop.

### Store listings

Add the language **English (United States)** and fill it in from [listing.md](listing.md),
including the screenshots and, optionally, the images in [`images/`](images/).

### Submission options → restricted capabilities

Because the manifest declares restricted capabilities, Partner Center asks why. Paste:

> **runFullTrust:** Tessera is a classic Win32 desktop application (window manager) packaged as
> MSIX. It arranges the windows of other programs with the Win32 window APIs, registers a
> low-level keyboard hook for its global shortcuts and runs in the notification area. It needs
> full trust like every packaged desktop app.
>
> **unvirtualizedResources:** Tessera is configured by editing plain-text files
> (`%APPDATA%\Tessera\config.toml`, `layouts.toml`). Users open and edit them in their own
> editor, and the app's tray menu opens them with the system's file association. With
> file-system virtualization these writes would go to a private package folder that other
> programs cannot see, so the user could not find or edit the configuration. The same folder is
> used by the portable (zip) version of Tessera, so users switching between them keep their
> settings, and the configuration survives a reinstall. Only file-system write virtualization
> is disabled; Tessera writes nowhere else.

### Submission options → notes for certification

> Tessera is a tiling window manager. After starting it from the Start menu it runs in the
> notification area (tray icon); there is no main window. No account, login or internet
> connection is needed, and it does not require administrator rights.
>
> To test: open two or three windows (e.g. Notepad, File Explorer, Settings); Tessera tiles
> them side by side automatically. Win+Alt+Arrow keys move the focus, Win+Alt+Shift+Arrow keys
> move windows, Win+Alt+E opens the zone editor (Esc closes it). The tray menu (right-click the
> icon) opens the configuration file, the zone editor, the keyboard shortcuts window and the
> "Start with Windows" option. "Exit" in the tray menu restores all windows to their original
> positions.
>
> The app is open source (MIT): https://github.com/Padishar24/tessera

Then **Submit for certification**. It usually takes one to three business days; Partner Center
shows the progress and, if rejected, the reason.

## 8. If certification fails

- **Name or listing issues:** fix the text and resubmit; no new package needed.
- **`unvirtualizedResources` rejected:** remove the line
  `<desktop6:FileSystemWriteVirtualization>…` and the capability `unvirtualizedResources` from
  the manifest. The configuration then lives in a private package folder
  (`%LOCALAPPDATA%\Packages\<package>\LocalCache\Roaming\Tessera`), and Tessera's code must open
  that folder instead of `%APPDATA%\Tessera` when it runs packaged. This needs a code change
  first; do not just drop the capability.
- **Crashes or "app doesn't start":** look at `%LOCALAPPDATA%\Tessera\logs` on a test install
  (step 5).

## 9. Updates

For every new version:

1. Tag the release as usual; download the `msix-store` artifact of the run.
2. Partner Center → your app → **Update** (creates a new submission from the last one).
3. **Packages:** remove the old bundle, upload the new one. Its version must be higher than the
   published one, which the version bump in `Cargo.toml` ensures.
4. **Store listings:** update "What's new in this version".
5. Submit. Users get the update automatically.

## 10. After publication

- The Store ID (e.g. `9N…`) is under **Product identity**. The Store page is
  `https://apps.microsoft.com/detail/<Store ID>`; add it to the README.
- winget can install it too: `winget install --source msstore <Store ID>`.
- Partner Center → **Insights** shows installs and crash reports from the Store version.
