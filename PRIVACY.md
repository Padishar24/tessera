# Privacy Policy

*Last updated: 2026-11-06*

This policy covers Tessera, the window tiling manager for Windows, in every form it is
distributed: the Microsoft Store app and the zip releases on GitHub.

## Summary

Tessera collects no personal data and sends nothing anywhere. It has no network code, no
telemetry, no analytics, no crash reporting and no accounts. Everything it reads or writes stays
on your computer.

## What Tessera processes on your computer

To do its job, Tessera works with the following information. None of it leaves your computer.

- **Keyboard input.** A keyboard hook checks each key press against the shortcuts in your
  configuration. Key presses are neither stored nor logged; at the debug log level, the logs
  name the Tessera actions your shortcuts trigger (e.g. "focus left"). While you record a new shortcut in
  the shortcuts window, only that one key combination is captured and written to your
  configuration.
- **Windows of other programs.** To arrange windows, Tessera reads their position, size, window
  title, window class and program name, and moves and resizes them.
- **Files Tessera writes:**
  - `%APPDATA%\Tessera`: your configuration, layouts, saved scenes and the state across restarts.
    These can contain program names that you or Tessera assigned to zones.
  - `%LOCALAPPDATA%\Tessera\logs`: log files, kept for 7 days. Depending on the log level, they
    can contain program names and window titles.

You can view, edit or delete these files at any time. Uninstalling Tessera does not delete them,
so a reinstall keeps your configuration. Delete both folders to remove all traces.

## Third parties

Tessera shares no information with third parties. If you install it from the Microsoft Store,
Microsoft handles the download and updates under its own privacy statement
(<https://privacy.microsoft.com/privacystatement>). The developer receives only the anonymous,
aggregated install and crash statistics that Partner Center shows every Store publisher.

## Changes

Changes to this policy are published in this file; its history is visible in the repository.

## Contact

Questions about this policy: open an issue at <https://github.com/Padishar24/tessera/issues>.
