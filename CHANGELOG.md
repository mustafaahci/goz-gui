# Changelog

All notable changes to goz-gui are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.1.0] - 2026-11-11

The first release: a native Windows front end for [goz](https://github.com/mustafaahci/goz), the instant NTFS file search engine, with the window, menus, keys and options of Everything 1.5.

### Requirements

- 64-bit Windows 10 or 11.
- gozd, goz's search service, installed and running. goz-gui works with gozd v0.6.0; [gozd v0.7.0 or newer](https://github.com/mustafaahci/goz/releases/latest) is needed for the newest features (see the last item below). goz-gui asks gozd what it supports and does without what it does not.

### Added

- A single exe in Rust on plain Win32 and the common controls: no UI framework, no web view, no runtime to install.
- Talks to gozd over its named pipe with goz's own protocol crates, after checking that the pipe belongs to the elevated service.
- Search as you type in goz's query syntax, in a virtual results list that stays fast and small with millions of matches; match case and match path, filters, sorting, search history, Advanced Search and a find bar.
- Live results: new, changed and deleted files show up without searching again.
- Tabs and multiple windows; details and thumbnail views; zoom; Name, Path, Size, Extension, Type and Date Modified columns.
- Folders, Filters, Bookmarks and Preview panes; bookmarks, run history and a taskbar jump list.
- Work on the results: open, Explorer's context menu, copy, cut and paste, delete, rename in place, Advanced Rename, Copy and Move to Folder, New Folder, Set File Time and Create Shortcut, with undo and redo.
- File lists: open, edit and create Everything file lists (`.efu`, CSV or plain text); export results as EFU, CSV or text.
- A background mode with a tray icon and global hotkeys, one instance per data folder, Everything's command-line switches and `goz:` links.
- Options on Everything's pages, including Start menu, desktop, Explorer and start-with-Windows integration for the current user, keyboard shortcuts, fonts and colours.
- Follows the system's light or dark theme and per-monitor DPI, and tells screen readers about the parts it draws itself.
- `content:` searches file contents, read by goz-gui under the user's own account.
- Settings in `%APPDATA%\goz-gui`, or beside the exe for portable use (a `goz-gui.ini` there with `app_data=0`).
- With gozd v0.7.0 or newer: live results from gozd's change notifications instead of polling; Date Created, Date Accessed, Attributes and Date Recently Changed columns and sorting by them; folder sizes; Options → Indexes; and Everything 1.5's search syntax (OR, NOT, groups, modifiers, macros), with `content:` terms anywhere in it.

[Unreleased]: https://github.com/mustafaahci/goz-gui/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/mustafaahci/goz-gui/releases/tag/v0.1.0
