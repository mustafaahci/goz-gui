# The Win32 boundary (`src/win`)

goz-gui is `#![deny(unsafe_code)]`. Every `unsafe` block is in `src/win`, a layer of safe functions over `windows-sys` and the `windows` crate's COM interfaces. The rest of the crate calls Windows only through it. This page is for anyone who adds a call to Windows: what the layer promises, and how to extend it.

## What "safe" means here

A safe function in `win` must be sound for **every** argument safe code can pass. The layer's trust model (also at the top of `src/win/mod.rs`):

- **The system checks its handles.** user32, gdi32, comctl32 and shell32 report a stale or wrong `HWND`, `HDC`, `HMENU`, GDI object or image list as an error; they never follow it as a pointer. Handles are plain `Copy` values, and any value is a valid argument.
- **A message means what its documentation says.** A message below `WM_USER` means the same to every window class, so a wrapper for one may be sent to any window. A message from `WM_USER` up is defined by its control class (`LVM_FIRST` and `DTM_FIRST` are both `0x1000`), so its wrapper checks the window's class first (`of_class`) and does nothing for a window of another class.
- **What the system delivers is what it documents.** A procedure's `lParam` for `WM_NOTIFY`, `WM_DRAWITEM` and the like points at the structure the message defines. Only a [`Msg`](../src/win/msg.rs) the system delivered hands those out, through typed accessors (`m.notify()`, `n.get::<NMLVDISPINFOW>()`, `m.draw_item()`); a `Msg` made with `Msg::new` hands out nothing.
- **goz-gui's private messages (`WM_APP` up to `0xC000`) carry no pointers.** A value that has to travel with one goes as a [parcel](../src/win/parcel.rs): the message carries a ticket, and the window claims the value by ticket and by type.

So the rules for a wrapper:

1. **No raw pointer in a safe signature**, in or out. Take `&str`, `&[u16]`, `&RECT`, `&mut` structs or owned values; build the pointer inside.
2. **A structure with pointer fields goes in by value**, and the wrapper sets those fields itself (`lv::set_item(lv, item, Some("text"))` sets `pszText`; the caller's is ignored).
3. **A buffer the system writes to is the wrapper's own**, sized from what it passes, and copied out.
4. **No generic `send(hwnd, msg, w, l)`**: a message is sent through a function that types its parameters (`lv::item_rect`, `edit::set_sel`). `win::send_plain` takes only the system messages whose parameters are all integers or handles; `win::send_app` and `win::post_app` only goz-gui's own.
5. **What the system frees is owned**: a kernel handle, a theme handle, a hook, a find handle, an item ID list, a COM object is a type whose `Drop` releases it (`sync::Event`, `process::Process`, `uxtheme::Theme`, `hook::Hook`, `file::Find`, `shell::Pidl`, `shell::ShellItem`); no safe function closes an arbitrary handle.
6. **Memory the system lends is copied out within the call** (a DIB section's bits, a clipboard global, a shell string): no Rust reference into it outlives the call that got it. A `dib::Canvas`'s pixels go in and out by copy (`pixels`, `set_pixels`), never through a pointer into the section; `clipboard::Clipboard::data` copies a block while the clipboard is open.
7. **Every `unsafe` block has a `// SAFETY:` comment** saying why its preconditions hold (`clippy::undocumented_unsafe_blocks` is on in `win`).

## Procedures are closures

goz-gui gives Windows only the trampolines in `win`; each hands the message to a Rust closure as a `&Msg`.

- **A window**: register its class with `win::class::Class { .. }.register()`, make it with `win::class::create(&Create { .. }, move |m| ...)`. The closure gets every message from `WM_NCCREATE` to `WM_NCDESTROY` and is dropped after. `m.def()` is `DefWindowProcW`.
- **A modal dialog**: build a `Template`, then `t.modal(owner, &|m| proc(&state, m))`. The state lives on the caller's stack; the dialog cannot outlive the call. Answer as a dialog procedure does (nonzero: handled); a `WM_NOTIFY` answer goes in with `dlg::set_result`. `dialog::common(m, &look)` does what every themed dialog shares.
- **A child dialog** (a page): `t.create_child(parent, move |m| ...)`; the closure owns its state (an `Rc`) and is dropped with the dialog.
- **A subclass**: declare a `static KEY: subclass::Key = subclass::Key::new("what it is");`, then `subclass::set(hwnd, &KEY, move |m| ...)`. `m.def()` passes the message on. One subclass per key and window; setting it again replaces it; `subclass::remove(hwnd, &KEY)` takes it off; it goes with the window.
- **A hook**: `hook::set(Kind::Cbt, |event| ...)`, dropped to unhook. Events are copied out (`Event::Create { hwnd, style, .. }`).

A closure that is running is kept alive even if it destroys its own window or removes its own subclass, so it never frees what it is using. Nothing in goz-gui reads or writes `GWLP_USERDATA`: state lives in the closure.

A message handed on to another of goz-gui's functions goes as the `&Msg` itself (`App::strip_message(m)`, `Page::message(app, page, m)`, `Strip::message(app, m)`), not as its parts: only the `Msg` the system delivered vouches for what its `lParam` points at, and the parts alone vouch for nothing. The thread's message loop takes messages with `queue::get`, which answers `Got::Message` for each and `Got::Quit(code)` at WM_QUIT.

## Where things are

One module per area of the API; `wnd`'s functions are re-exported at `win::` itself (`win::set_focus`, `win::text`).

- `mod.rs`: the trust model, `send_raw`, class checks (`class_is`, `of_class`), UTF-16 helpers (`wz`, `wz_units`, `until_nul`, `units_as_bytes`), words of a parameter.
- `msg`: `Msg` (a delivered message, its typed accessors, `def`, `forward`, `paint_next_into`), `Notify`, `View`.
- `class`, `dlg`, `subclass`, `hook`: the trampolines (windows, dialogs, subclasses, hooks), each handing a closure a `Msg`; `dlg` also has the message box and the colour picker.
- `queue`: the thread's message queue (`get`, `peek`, `dispatch`, `msg_wait`), accelerator tables.
- `parcel`: values that travel with goz-gui's private messages.
- `dropfiles`: files dropped on a window, read while the drop is delivered.
- `wnd`: windows as handles: place, text, state, focus, timers, hotkeys, moves together, goz-gui's own messages, DWM attributes.
- `sys`: keyboard and mouse state, metrics, DPI, monitors, caret, sounds, the system folder and environment.
- `gdi`: device contexts, GDI objects, text, drawing, fonts; `dib`: a canvas of pixels copied in and out.
- `uxtheme`: theme data, a control's visual style, the undocumented dark mode.
- `menu`: menus made, filled, read, changed and shown; `menubar`: the menu bar's undocumented drawing messages.
- `scroll`: scroll bars and the wheel.
- Controls, each checking the class before a message from `WM_USER` up: `button`, `combo`, `datetime` (the date picker), `edit`, `header`, `hotkey`, `listbox`, `lv` (list view), `tab`, `tip` (tooltip), `trackbar`, `tv` (tree view); `imagelist` for image lists, owned or the shell's.
- `icon`: owned icons, the program's and the system's, the shell's file and stock icons.
- `shell`: shell items and item ID lists, the common item dialogs, file operations and their progress sink, shortcuts, property stores, the jump list, known folders, ShellExecute, file information, the tray icon.
- `file`: attributes, times, names, listings (`Find`), owned file handles, drives, `file:` URLs.
- `clipboard`: the clipboard, opened by a guard.
- `process`: processes and threads, named mutexes, a named pipe read and written overlapped, services.
- `reg`: the registry.
- `resource`: resources of a program or library loaded as data.
- `locale`: normalization, comparisons, case, code pages, the user's date, time and size formats; `time`: the clock and time conversions.
- `com`: COM apartments.
- `sync`: kernel events.
- `ext`: wrappers a batch of the move still adds (batch 7: the shell's context menus, the preview pane, thumbnails, accessibility), folded into the modules above once it is done.

## Adding a wrapper

Put it in the module of its API (`lv.rs` for list-view messages, `gdi.rs` for GDI, `com.rs` for COM apartments and the like), with a `///` line saying what it does, and a test where it can be tested without a desktop (`trampolines_tests.rs` makes message-only windows and dialogs that never show).
