//! Shortcuts, property stores and the taskbar's jump list.

use windows::Win32::Foundation::{HWND as WHWND, PROPERTYKEY};
use windows::Win32::System::Com::StructuredStorage::{PROPVARIANT, PropVariantClear};
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, CoCreateInstance, CoTaskMemAlloc, IPersistFile, STGM_READ,
};
use windows::Win32::System::Variant::VT_LPWSTR;
use windows::Win32::UI::Shell::Common::{IObjectArray, IObjectCollection};
use windows::Win32::UI::Shell::PropertiesSystem::{IPropertyStore, SHGetPropertyStoreForWindow};
use windows::Win32::UI::Shell::{
    DestinationList, EnumerableObjectCollection, ICustomDestinationList, IShellLinkW,
    ShellLink as SHELL_LINK,
};
use windows::core::{Interface, PCWSTR, PWSTR, Result};
use windows_sys::Win32::Foundation::HWND;

use super::Pidl;
use crate::win::{nul_ended, until_nul, wz};

/// A string PROPVARIANT (VT_LPWSTR) holding a copy of `text`, for
/// `PropVariantClear` to free.
fn string_variant(text: &str) -> PROPVARIANT {
    let wide = wz(text);
    let mut pv = PROPVARIANT::default();
    // SAFETY: task memory of the string's size, filled with it and
    // given to the value, which `PropVariantClear` frees; the union is
    // written as the VT_LPWSTR value it is then tagged as.
    unsafe {
        let mem = CoTaskMemAlloc(wide.len() * 2).cast::<u16>();
        if mem.is_null() {
            return pv;
        }
        core::ptr::copy_nonoverlapping(wide.as_ptr(), mem, wide.len());
        let inner = &mut *pv.Anonymous.Anonymous;
        inner.vt = VT_LPWSTR;
        inner.Anonymous.pwszVal = PWSTR(mem);
    }
    pv
}

/// A property store: a window's (its taskbar identity) or a shortcut's.
pub struct PropertyStore(IPropertyStore);

impl PropertyStore {
    /// The window's.
    pub fn for_window(hwnd: HWND) -> Result<PropertyStore> {
        // SAFETY: a window handle.
        unsafe { SHGetPropertyStoreForWindow(WHWND(hwnd)) }.map(PropertyStore)
    }

    /// Sets property `key` to the string `text`.
    pub fn set_string(&self, key: &PROPERTYKEY, text: &str) -> Result<()> {
        let mut pv = string_variant(text);
        // SAFETY: the store this holds; the key and value outlive the
        // call (the store keeps its own copy).
        let r = unsafe { self.0.SetValue(key, &pv) };
        // SAFETY: a value made by `string_variant`, cleared once.
        unsafe {
            let _ = PropVariantClear(&mut pv);
        }
        r
    }

    /// Saves what was set.
    pub fn commit(&self) -> Result<()> {
        // SAFETY: the store this holds.
        unsafe { self.0.Commit() }
    }
}

/// A shortcut (IShellLinkW), in memory until saved.
pub struct ShellLink(IShellLinkW);

impl ShellLink {
    /// A new shortcut, of nothing yet; fails without COM on the thread.
    pub fn new() -> Result<ShellLink> {
        // SAFETY: the system's class, in this process.
        unsafe { CoCreateInstance(&SHELL_LINK, None, CLSCTX_INPROC_SERVER) }.map(ShellLink)
    }

    /// What it starts.
    pub fn set_path(&self, path: &str) -> Result<()> {
        let path = wz(path);
        // SAFETY: the shortcut this holds; a NUL-terminated string read
        // during the call.
        unsafe { self.0.SetPath(PCWSTR(path.as_ptr())) }
    }

    /// Reads the shortcut file at `path` (IPersistFile::Load, read only).
    pub fn load(&self, path: &[u16]) -> Result<()> {
        let file = self.0.cast::<IPersistFile>()?;
        let path = nul_ended(path);
        // SAFETY: the shortcut's interface; a NUL-terminated path read
        // during the call.
        unsafe { file.Load(PCWSTR(path.as_ptr()), STGM_READ) }
    }

    /// What it points to, as an absolute ID list (GetIDList); `None` when
    /// it has none.
    pub fn id_list(&self) -> Option<Pidl> {
        // SAFETY: the shortcut this holds.
        let list = unsafe { self.0.GetIDList() }.ok()?;
        // SAFETY: the list GetIDList allocated for the caller (or null).
        unsafe { Pidl::own(list.cast()) }
    }

    /// The command line it passes.
    pub fn set_arguments(&self, args: &str) -> Result<()> {
        let args = wz(args);
        // SAFETY: as in `set_path`.
        unsafe { self.0.SetArguments(PCWSTR(args.as_ptr())) }
    }

    /// The command line it passes (the first 1023 units).
    pub fn arguments(&self) -> Result<String> {
        let mut buf = [0u16; 1024];
        // SAFETY: the shortcut this holds; a buffer with its length (the
        // slice).
        unsafe { self.0.GetArguments(&mut buf) }?;
        Ok(String::from_utf16_lossy(until_nul(&buf)))
    }

    /// The folder it starts in.
    pub fn set_working_directory(&self, dir: &str) -> Result<()> {
        let dir = wz(dir);
        // SAFETY: as in `set_path`.
        unsafe { self.0.SetWorkingDirectory(PCWSTR(dir.as_ptr())) }
    }

    /// Its tip.
    pub fn set_description(&self, text: &str) -> Result<()> {
        let text = wz(text);
        // SAFETY: as in `set_path`.
        unsafe { self.0.SetDescription(PCWSTR(text.as_ptr())) }
    }

    /// Its icon: icon `index` of `file`.
    pub fn set_icon_location(&self, file: &str, index: i32) -> Result<()> {
        let file = wz(file);
        // SAFETY: as in `set_path`.
        unsafe { self.0.SetIconLocation(PCWSTR(file.as_ptr()), index) }
    }

    /// Its property store (System.Title and the like).
    pub fn property_store(&self) -> Result<PropertyStore> {
        self.0.cast::<IPropertyStore>().map(PropertyStore)
    }

    /// Writes it to the file `path` (IPersistFile).
    pub fn save(&self, path: &str) -> Result<()> {
        let file = self.0.cast::<IPersistFile>()?;
        let path = wz(path);
        // SAFETY: the shortcut's interface; a NUL-terminated path read
        // during the call.
        unsafe { file.Save(PCWSTR(path.as_ptr()), true) }
    }
}

/// Objects collected for a jump list's category.
pub struct ObjectCollection(IObjectCollection);

impl ObjectCollection {
    /// A new, empty collection; fails without COM on the thread.
    pub fn new() -> Result<ObjectCollection> {
        // SAFETY: the system's class, in this process.
        unsafe { CoCreateInstance(&EnumerableObjectCollection, None, CLSCTX_INPROC_SERVER) }
            .map(ObjectCollection)
    }

    /// Adds `link` (the collection keeps a reference).
    pub fn add(&self, link: &ShellLink) -> Result<()> {
        // SAFETY: the collection this holds; an object it keeps a
        // reference to.
        unsafe { self.0.AddObject(&link.0) }
    }
}

/// The items the user took off a jump list ([`JumpList::begin_list`]).
pub struct ObjectArray(IObjectArray);

impl ObjectArray {
    /// How many items it has.
    pub fn count(&self) -> Result<u32> {
        // SAFETY: the array this holds.
        unsafe { self.0.GetCount() }
    }

    /// Item `i`, if it is a shortcut.
    pub fn link_at(&self, i: u32) -> Result<ShellLink> {
        // SAFETY: the array this holds; a number.
        unsafe { self.0.GetAt::<IShellLinkW>(i) }.map(ShellLink)
    }
}

/// The taskbar's jump list of an AppUserModelID (ICustomDestinationList).
pub struct JumpList(ICustomDestinationList);

impl JumpList {
    /// The jump list object; fails without COM on the thread.
    pub fn new() -> Result<JumpList> {
        // SAFETY: the system's class, in this process.
        unsafe { CoCreateInstance(&DestinationList, None, CLSCTX_INPROC_SERVER) }.map(JumpList)
    }

    /// The AppUserModelID whose list it is.
    pub fn set_app_id(&self, id: &str) -> Result<()> {
        let id = wz(id);
        // SAFETY: the list this holds; a NUL-terminated string read
        // during the call.
        unsafe { self.0.SetAppID(PCWSTR(id.as_ptr())) }
    }

    /// Starts making the list again: the items the user took off it.
    pub fn begin_list(&self) -> Result<ObjectArray> {
        let mut slots = 0u32;
        // SAFETY: the list this holds; an out-value.
        unsafe { self.0.BeginList::<IObjectArray>(&mut slots) }.map(ObjectArray)
    }

    /// Adds category `name` with the objects of `items`.
    pub fn append_category(&self, name: &str, items: &ObjectCollection) -> Result<()> {
        let array = items.0.cast::<IObjectArray>()?;
        let name = wz(name);
        // SAFETY: the list this holds; a NUL-terminated name read during
        // the call; an array it keeps a reference to.
        unsafe { self.0.AppendCategory(PCWSTR(name.as_ptr()), &array) }
    }

    /// Puts the list made since `begin_list` on the taskbar.
    pub fn commit_list(&self) -> Result<()> {
        // SAFETY: the list this holds.
        unsafe { self.0.CommitList() }
    }

    /// Deletes the list of AppUserModelID `id`.
    pub fn delete_list(&self, id: &str) -> Result<()> {
        let id = wz(id);
        // SAFETY: as in `set_app_id`.
        unsafe { self.0.DeleteList(PCWSTR(id.as_ptr())) }
    }
}

/// Saves a shortcut to `target` at `path`, starting in `work_dir` when
/// given (the paths as UTF-16: a file name need not be valid Unicode).
pub fn save_shortcut(target: &[u16], work_dir: Option<&[u16]>, path: &[u16]) -> Result<()> {
    let target = nul_ended(target);
    let work = work_dir.map(nul_ended);
    let path = nul_ended(path);
    let link = ShellLink::new()?;
    // SAFETY: the shortcut made above; every string is NUL-terminated and
    // outlives the call that takes it.
    unsafe {
        link.0.SetPath(PCWSTR(target.as_ptr()))?;
        if let Some(w) = &work {
            link.0.SetWorkingDirectory(PCWSTR(w.as_ptr()))?;
        }
        link.0
            .cast::<IPersistFile>()?
            .Save(PCWSTR(path.as_ptr()), true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_shortcut_keeps_its_arguments() {
        let _com = crate::win::com::Apartment::sta().unwrap();
        let link = ShellLink::new().unwrap();
        link.set_arguments("-s \"a b\"").unwrap();
        assert_eq!(link.arguments().unwrap(), "-s \"a b\"");
    }
}
