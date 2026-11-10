Kasandra 1.2 adds a terminal.

## New in 1.2

**SSH terminal in a tab.** Open one from a connection's menu (**SSH Terminal**) to land on
its SSH host, or use **Terminal** in the toolbar to reach any `user@host` or open a shell
on this computer. It is a real terminal, so vim, htop and tmux work, and it uses your
system `ssh`, so `~/.ssh/config`, the agent and known hosts behave as usual. A saved SSH
password is typed in for you at the login prompt. Sessions stay open when you switch tabs.

## Also

The project now lives at github.com/mdenizay/kasandra. The old address redirects, so
updates keep arriving whichever one your copy asks.

## Install

| Platform | File |
| --- | --- |
| macOS (Apple Silicon and Intel) | `Kasandra_1.2.0_universal.dmg`, or `brew install --cask mdenizay/tap/kasandra` |
| Windows | `Kasandra_1.2.0_x64-setup.exe` |
| Linux | `.AppImage`, `.deb` or `.rpm` |

macOS is signed with a Developer ID and notarised, so it opens with no warning. The
Windows installer is unsigned — SmartScreen will ask; choose **More info → Run anyway**.

If you already have Kasandra or MieSQL, it will offer this update at launch.
