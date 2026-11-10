Kasandra 1.1 — the SQL client formerly known as MieSQL. Same app, new name: it updates in
place and keeps your connections, saved passwords, history and settings.

## New in 1.1

**Query tabs remember their database.** A new query tab starts in the database you last
opened in the sidebar, and a picker in the toolbar switches it. The database is selected
before every run, so scripts no longer need their own `USE`.

**Close result tabs.** A script with many statements no longer leaves a row of results you
cannot get rid of: close them one by one, all at once, or with a middle click.

**Middle click closes tabs**, in the editor tab bar too.

## Fixed

- **Windows opened a terminal window behind the app** on every start, and another for each
  SSH tunnel. Both are gone. `Kasandra.exe --doctor` still prints its report to the
  console it was started from.
- Right-clicking empty space showed the webview's own Back / Refresh / Save as menu.

## Install

| Platform | File |
| --- | --- |
| macOS (Apple Silicon and Intel) | `Kasandra_1.1.0_universal.dmg`, or `brew install --cask mdenizay/tap/kasandra` |
| Windows | `Kasandra_1.1.0_x64-setup.exe` |
| Linux | `.AppImage`, `.deb` or `.rpm` |

macOS is signed with a Developer ID and notarised, so it opens with no warning. The
Windows installer is unsigned — SmartScreen will ask; choose **More info → Run anyway**.
It installs for the current user and needs no administrator rights.

If you already have MieSQL, it will offer this update at launch.
