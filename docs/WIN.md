# The Win32 boundary (`src/win`)

goz-gui is `#![deny(unsafe_code)]`. Every `unsafe` block is in `src/win`, a layer of safe functions over `windows-sys` and the `windows` crate's COM interfaces. The rest of the crate calls Windows only through it. This page is for anyone who adds a call to Windows: what the layer promises, and how to extend it.

## What "safe" means here

A safe function in `win` must be sound for **every** argument safe code can pass. The layer's trust model (also at the top of `src/win/mod.rs`):

- **The system checks its handles.** user32, gdi32, comctl32 and shell32 report a stale or wrong `HWND`, `HDC`, `HMENU`, GDI object or image list as an error; they never follow it as a pointer. Handles are plain `Copy` values, and any value is a valid argument.
- **A message means what its documentation says.** A message below `WM_USER` means the same to every window class, so a wrapper for one may be sent to any window. A message from `WM_USER` up is defined by its control class (`LVM_FIRST` and `DTM_FIRST` are both `0x1000`), so its wrapper checks the window's class first (`of_class`) and does nothing for a window of another class.
- **What the system delivers is what it documents.** A procedure's `lParam` for `WM_NOTIFY`, `WM_DRAWITEM` and the like points at the structure the message defines. Only a [`Msg`](../src/win/msg.rs) the system delivered hands those out, through typed accessors (`m.notify()`, `n.get::<NMLVDISPINFOW>()`, `m.draw_item()`); a `Msg` made with `Msg::new` hands out nothing.
- **goz-gui's private messages (`WM_APP` up to `0xC000`) carry no pointers.** A value that has to travel with one goes as a [parcel](../src/win/parcel.rs): the message carries a ticket, and the window claims the value by ticket and by type.
- **In-process COM servers keep their interfaces' contracts.** The calls here load third-party code into the process: shell extensions (context menu, icon and thumbnail handlers), WIC's codecs, namespace extensions. Like the system, they write no further than the buffers they are given, and a server's `DllCanUnloadNow` says it can go only when nothing of it is in use or running. Code in the process could break any of Rust's promises anyway; this is the narrow part of that trust the layer relies on, written down because it is about code that is not the system's. The second half is relied on by one call alone, `com::Apartment::end_unloading_unused` (CoFreeUnusedLibrariesEx), which goz-gui makes only as the preview's thread ends, its objects released; never on the UI thread, where a context menu handler is known to leave a thread behind (see `shell::ContextMenu` in `src/shell.rs`).

So the rules for a wrapper:

1. **No raw pointer in a safe signature**, in or out. Take `&str`, `&[u16]`, `&RECT`, `&mut` structs or owned values; build the pointer inside.
2. **A structure with pointer fields goes in by value**, and the wrapper sets those fields itself (`lv::insert_item(lv, item, Some("text"))` sets `pszText`; the caller's is ignored).
3. **A buffer the system writes to is the wrapper's own**, sized from what it passes, and copied out.
4. **No generic `send(hwnd, msg, w, l)`**: a message is sent through a function that types its parameters (`lv::item_rect`, `edit::set_sel`). `win::send_plain` takes only the system messages whose parameters are all integers or handles; `win::send_app` and `win::post_app` only goz-gui's own.
5. **What the system frees is owned**: a kernel handle, a theme handle, a hook, a find handle, an accelerator table, an item ID list, a COM object is a type whose `Drop` releases it (`sync::Event`, `process::Process`, `uxtheme::Theme`, `hook::Hook`, `file::Find`, `queue::AccelTable`, `dib::Section`, `shell::Pidl`, `shell::ShellItem`, `shell::DataObject`, `preview::Handler`); no safe function closes an arbitrary handle.
6. **Memory the system lends is copied out within the call** (a DIB section's bits, a clipboard global, a shell string): no Rust reference into it outlives the call that got it. A `dib::Canvas`'s or `dib::Section`'s pixels go in and out by copy (`pixels`, `set_pixels`, through GDI, which finds the bitmap by its handle each time), never through a pointer into the section; `clipboard::Clipboard::data` copies a block while the clipboard is open, `shell::DataObject::get` while it holds the medium.
7. **Every `unsafe` block has a `// SAFETY:` comment** saying why its preconditions hold (`clippy::undocumented_unsafe_blocks` is on in `win`).

## Procedures are closures

goz-gui gives Windows only the trampolines in `win`; each hands the message to a Rust closure as a `&Msg`.

- **A window**: register its class with `win::class::Class { .. }.register()`, make it with `win::class::create(&Create { .. }, move |m| ...)`. The closure gets every message from `WM_NCCREATE` to `WM_NCDESTROY` and is dropped after. `m.def()` is `DefWindowProcW`.
- **A modal dialog**: build a `Template`, then `t.modal(owner, &|m| proc(&state, m))`. The state lives on the caller's stack; the dialog cannot outlive the call. Answer as a dialog procedure does (nonzero: handled); a `WM_NOTIFY` answer goes in with `dlg::set_result`. `dialog::common(m, &look)` does what every themed dialog shares.
- **A child dialog** (a page): `t.create_child(parent, move |m| ...)`; the closure owns its state (an `Rc`) and is dropped with the dialog.
- **A subclass**: declare a `static KEY: subclass::Key = subclass::Key::new("what it is");`, then `subclass::set(hwnd, &KEY, move |m| ...)`. `m.def()` passes the message on. One subclass per key and window; setting it again replaces it; `subclass::remove(hwnd, &KEY)` takes it off; it goes with the window.
- **A hook**: `hook::set(Kind::Cbt, |event| ...)`, dropped to unhook. Events are copied out (`Event::Create { hwnd, style }`).
- **A COM object of goz-gui's** (a file operation's progress sink, a preview handler's frame and stream, a window's accessible object) is a private type configured with closures or a Rust value (`shell::ProgressSink::new(|..| ...)`, `preview::Frame::new(context, translate)`, `a11y::AccessibleObject::new(hwnd, elements)`); only a wrapper of its interface is handed out.

A closure that is running is kept alive even if it destroys its own window or removes its own subclass, so it never frees what it is using. Outside `class`'s trampoline, which keeps a window's closure there, nothing in goz-gui reads or writes `GWLP_USERDATA`: state lives in the closure.

A message handed on to another of goz-gui's functions goes as the `&Msg` itself (`App::strip_message(m)`, `Page::message(app, page, m)`, `Strip::message(app, m)`, `PaneContent::handle(app, pane, m)`, `ContextMenu::message(m)`), not as its parts: only the `Msg` the system delivered vouches for what its `lParam` points at, and the parts alone vouch for nothing. The thread's message loop takes messages with `queue::get`, which answers `Got::Message` for each and `Got::Quit(code)` at WM_QUIT.

## Where things are

One module per area of the API; `wnd`'s functions are re-exported at `win::` itself (`win::set_focus`, `win::text`).

- `mod.rs`: the trust model, `send_raw`, class checks (`class_is`, `class_name`, `of_class`), UTF-16 helpers (`wz`, `wz_units`, `until_nul`, `units_as_bytes`), words of a parameter.
- `msg`: `Msg` (a delivered message, its typed accessors, `def`, `paint_next_into`), `Notify`, `View`.
- `class`, `dlg`, `subclass`, `hook`: the trampolines (windows, dialogs, subclasses, hooks), each handing a closure a `Msg`; `dlg` also has the message box and the colour picker.
- `queue`: the thread's message queue (`get`, `peek`, `dispatch`, `msg_wait`), accelerator tables (`AccelTable`, owned, its entries copied out).
- `parcel`: values that travel with goz-gui's private messages.
- `dropfiles`: files dropped on a window, read while the drop is delivered.
- `wnd`: windows as handles: place, text, state, parent, focus, timers, hotkeys, moves together, goz-gui's own messages, DWM attributes.
- `sys`: keyboard and mouse state, the current message's time, metrics, DPI, monitors, caret, sounds, the last error and its text, the system folder and environment.
- `gdi`: device contexts, GDI objects, text, drawing, stretching, fonts; `dib`: 32-bit DIB sections whose pixels are copied in and out (`Canvas`, drawn on in a DC of its own; `Section`, a bitmap of its own), and any bitmap's pixels.
- `uxtheme`: theme data, a control's visual style, the undocumented dark mode.
- `menu`: menus made, filled, read, changed and shown; `menubar`: the menu bar's undocumented drawing messages.
- `scroll`: scroll bars and the wheel.
- Controls, each checking the class before a message from `WM_USER` up: `button`, `combo`, `datetime` (the date picker), `edit`, `header`, `hotkey`, `listbox`, `lv` (list view), `tab`, `tip` (tooltip), `trackbar`, `tv` (tree view); `imagelist` for image lists, owned or the shell's.
- `icon`: owned icons, the program's and the system's, the shell's file and stock icons.
- `shell`: the shell. In `mod.rs`: shell items and their icon handlers, item images (`ImageFactory`), file information, settings, associations, known folders, ShellExecute, recent documents, Properties, the tray icon. `pidl`: item ID lists (`Pidl`), the shell's folders (`Folder`), opening a folder with items selected. `data`: data objects (`DataObject`: read and written by copy, the OLE clipboard's, dragged out). `menu`: the Explorer context menu (`ShellMenu`). `dialog`: the common item dialogs. `fileop`: file operations and their progress sink. `link`: shortcuts, property stores, the jump list.
- `preview`: preview handlers (`Handler`), the frame they pass keys to (`Frame`) and the read-only stream they read a file through (`Stream`).
- `wic`: the Windows Imaging Component: decoders, frames, scaling and conversion, pixels copied out.
- `a11y`: accessibility (MSAA) for what a window paints itself: an accessible object that adds painted parts to the system's standard one.
- `file`: attributes, times, names, listings (`Find`), owned file handles, drives, `file:` URLs.
- `clipboard`: the clipboard, opened by a guard; its sequence number and registered formats.
- `process`: processes and threads, named mutexes, a named pipe read and written overlapped, services.
- `reg`: the registry, its keys named as text or in UTF-16.
- `resource`: resources of a program or library loaded as data.
- `locale`: normalization, comparisons, case, code pages (the ANSI one decoded), the user's date and time formats; `time`: the clock and time conversions.
- `com`: COM apartments, and OLE.
- `sync`: kernel events.

## Adding a wrapper

Put it in the module of its API (`lv.rs` for list-view messages, `gdi.rs` for GDI, `com.rs` for COM apartments and the like), with a `///` line saying what it does, and a test where it can be tested without a desktop (`trampolines_tests.rs` makes message-only windows and dialogs that never show). Add it when something uses it: the layer has no dead code (`win` allows none), so a wrapper nothing calls is deleted, not kept for later. The one exception is a wrapper that serves only as a test's oracle (Windows' own answer that goz-gui's is checked against: `locale::format_byte_size`, `shell::command_line_to_argv`), which is `#[cfg(test)]`.
