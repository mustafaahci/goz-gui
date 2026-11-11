# Testing

How goz-gui is tested, and the hooks that keep a build under test away from your own files, settings and desktop. See also [Building](BUILDING.md) and the [README](../README.md).

## Automated checks

CI runs these on every push to `main` and every pull request ([`.github/workflows/ci.yml`](../.github/workflows/ci.yml)); run them before you send a change:

```
cargo fmt --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
```

`cargo test` runs the unit tests (settings, INI, columns, placement, file store, …); they write only to temporary folders. The shell test builds the Explorer menu and the data object of system files in process and only reads them.

## Test hooks

Whenever you run a build you are testing, point at least `GOZ_GUI_DATA_DIR` and `GOZ_GUI_REG_ROOT` at test locations, so it never reads or writes your real `%APPDATA%\goz-gui` or registry keys.

These environment variables keep a test run away from the user's own files and desktop:

| Variable | Effect |
| --- | --- |
| `GOZ_GUI_DATA_DIR` | The data folder to use instead of `%APPDATA%\goz-gui` (or the portable folder). Give every test run a fresh folder. |
| `GOZ_GUI_DRY_RUN=1` | Open, Open Path, Run as administrator, Properties, the Help links and the goz service commands do nothing but append one line each to `dry-run.log` in the data folder: the verb (`open`, `open-path`, `run-as`, `properties`, `url`, `service-start`, `service-stop`, `service-restart`) and the paths, tab-separated. A dry-run open still counts in the run history. |
| `GOZ_GUI_NEW_WINDOW_HIDDEN=1` | File → New Window (and the tray's new window) starts its goz-gui hidden (it inherits the variable, and the test data folder), so nothing shows on the desktop. |
| `GOZ_GUI_MENU_DUMP=1` | The context menus (row, background, header; the search box's and the filter bar's list; the tab strip's menu and its list of tabs; the status bar's and the Folders, Filters and Bookmarks panes'; Advanced Rename's format menus and its Presets menu) are built exactly as for the screen, the Explorer items included, but instead of being shown they are appended to `menu-dump.log` in the data folder, as if every submenu were opened: one line per item (its label with `\t` before a shortcut, `id=`, then the state in brackets: `default`, `checked`, `radio`, `greyed`, `owner-drawn WxH` and `icon WxH` as the shell measured them; the shell's `verb=`), indented two spaces per submenu, between a `== row menu at x,y ==` line and a `== picked … ==` line. |
| `GOZ_GUI_MENU_PICK=<label or verb>` | With `GOZ_GUI_MENU_DUMP`, the first item whose label (without `&` and the shortcut, any case) or else shell verb matches is then run, as if clicked: `Copy Full Path`, `Set Run Count...`, `properties`, `copy`. Properties and Run as administrator from the Explorer menu take goz-gui's own path (dry run included). |
| `GOZ_GUI_FILEOP_SILENT=1` | File operations (Delete, Shift+Delete, Paste, Rename, and the file tools' with their Undo and Redo) ask nothing and show no progress or error windows (`FOF_NOCONFIRMATION`, `FOF_SILENT`, `FOF_NOERRORUI`, `FOF_NOCONFIRMMKDIR`; the Explorer menu's verbs run with `CMIC_MASK_FLAG_NO_UI`). Use it only on scratch files. |
| `GOZ_GUI_REG_ROOT=<key>` | Start with Windows, the folder item, the `goz:` URL protocol and the `.efu` association go under `HKCU\<key>` instead of the real `Run` and `Classes` keys. |
| `GOZ_GUI_SHORTCUT_DIR=<dir>` | Options → General's Start menu and Desktop shortcuts go to `<dir>\Programs` and `<dir>\Desktop` instead of the user's own. |
| `GOZ_GUI_EXPORT_TO=<path>` | File → Export writes there without asking, in the format its extension names (.efu, .csv, .txt). |
| `GOZ_GUI_PICK_FOLDER=<folder>` | Edit → Copy to Folder... and Move to Folder..., Advanced Copy/Move's Browse..., Import and Export Settings, the File List Editor's Add Folder... and the Edit File List Slot dialog's Add Folder... use that folder without asking. |
| `GOZ_GUI_EDITOR_FILE=<path>` | The File List Editor's Open..., Save As..., Import... and Export... use that file without asking (Export: the type its extension names, `.tsv` for TSV; the selection only when rows are selected), and its "Save changes?" question is answered No; so do the Edit File List Slot dialog's Import... and Export... (Export: CSV, EFU or TXT by the extension), and the file Ctrl+click or F2 picks for an `include-file-list:` or `open-filelist:` term. (Add Files... and a drop on the editor have no hook: a test adds folders.) |
| `GOZ_GUI_PASTE_TEXT=<names>` | The File List Editor's and the Edit File List Slot dialog's Paste take these names (`\|` between them, a name a line as the clipboard's text would hold them) instead of the clipboard's, so a test never touches the user's clipboard. |
| `GOZ_GUI_SETTINGS_ACTION=export\|import\|reset` | Tools → Import and Export Settings... does that without its wizard and answers its questions yes (the folder from `GOZ_GUI_PICK_FOLDER`); an export's result goes to the status bar. |
| `GOZ_GUI_DESKTOP_DIR=<folder>` | Create Shortcut puts a shortcut it cannot make beside its item there instead of on the Desktop (after the question). |
| `GOZ_GUI_PREVIEW_LOG=1` | Each preview made is appended to `preview.log` in the data folder: what it showed (`picture WxH of WxH`, `text N`, `icon WxH`, `handler {CLSID} by Stream/File/Item`, `nothing`, `failed ...`), in how many ms, and the file; also a host window parked or destroyed and a thread that ended after it was given up on. |
| `GOZ_GUI_PREVIEW_STALL_MS=<ms>` | A preview handler's DoPreview is made to take that long (the preview's thread keeps answering calls meanwhile, as in a real hang): past 3 s the pane says the handler is not responding and goes on with a new thread. |
| `GOZ_GUI_GOZ_EXE=<path>` | The goz.exe Options → Indexes runs as administrator to change gozd's settings (instead of the one beside the goz service's gozd.exe, or on the PATH). With `GOZ_GUI_DRY_RUN` it is not run: `dry-run.log` gets a `config-set` line with the command, and the status bar says the settings were not changed. |
| `GOZ_GUI_SYNTAX_LOG=1` | Each time a new connection's probe tells what search syntax gozd reads, a line `boolean_syntax` and `1` (OR, NOT and groups) or `0` (an older gozd), tab-separated, is appended to `syntax.log` in the data folder. |
| `GOZ_GUI_JUMP_LIST_LOG=1` | Each time the jump list is made, its entries (category, title, arguments, tab-separated) and `committed` or `failed` are appended to `jump-list.log` in the data folder; the test instance's jump list is deleted again as it exits. Without it, a goz-gui started with `GOZ_GUI_DATA_DIR` makes no jump list. |
| `GOZ_GUI_PICK_COLOR=#RRGGBB` | The colour pickers (Fonts and Colors, Color Filters) pick that colour without showing Windows' colour dialog. |
| `GOZ_GUI_TEST_PANES=<letters>` | Empty panes and rows for trying the docks without any feature in them: each `L` adds a pane to the left dock, each `R` one to the right (up to four, open at start, kept in no settings), `T` a tab strip row and `F` a find bar row (a click counts, a right-click makes the row twice as tall). `LLRTF` has two left panes, one right, and both rows. |
| `GOZ_GUI_TRACE=<file>` | A timeline of the window's thread is written to `<file>` as goz-gui ends, one point a line, tab-separated: the system's precise time (`GetSystemTimePreciseAsFileTime`, a FILETIME a probe in another process reads alike), the kind (`g` a message taken from the queue, a modal dialog's loop's too; `c` a window made; `m` a mark: `dialog run`, `dialog init`, `dialog shown`, `dialog ended`; `s`/`r` below), the message and its name, the window's class (a mark's name; also the marks `watch on`, `watch changed` and `watch off: polling` for gozd's watch), the window, wParam, and the thread's CPU cycles (against the time: whether a step worked or waited). A window started from it (File → New Window) inherits it: give such runs their own file. |
| `GOZ_GUI_TRACE_SENDS=1` | With `GOZ_GUI_TRACE`, also the sends that make, show and paint windows (`WM_CREATE`, `WM_SHOWWINDOW`, `WM_PAINT`, `WM_WINDOWPOSCHANGED`, ...) and their returns (`s`, `r`). Hooking every send slows each one down, so timings taken with it are not a dialog's real ones. |

## Timing a dialog

To time a dialog, wait for it without polling the window list: post the command and take the process's `EVENT_OBJECT_SHOW` (an out-of-context `SetWinEventHook`). A probe that enumerates every top-level window in a tight loop holds the window manager's lock against the dialog it times, whose thread then waits for it (`WrResource`) at every window it makes or moves: the bookmark editor took 62–112 ms that way, against 20–30 ms with the event.
