//! Resources of programs and libraries, read without running them.

use windows_sys::Win32::Foundation::{FreeLibrary, HMODULE};
use windows_sys::Win32::System::LibraryLoader::{
    EnumResourceNamesW, FindResourceW, LOAD_LIBRARY_AS_DATAFILE, LOAD_LIBRARY_AS_IMAGE_RESOURCE,
    LoadLibraryExW, LoadResource, LockResource, SizeofResource,
};

use windows_sys::Win32::System::Memory::{
    MEM_COMMIT, MEMORY_BASIC_INFORMATION, PAGE_EXECUTE_READ, PAGE_EXECUTE_READWRITE,
    PAGE_EXECUTE_WRITECOPY, PAGE_READONLY, PAGE_READWRITE, PAGE_WRITECOPY, VirtualQuery,
};

use super::nul_ended;

/// A program or library loaded as data (nothing in it runs), to copy its
/// resources; freed when this drops.
pub struct DataFile(HMODULE);

impl DataFile {
    /// Loads `path` as data; `None` if it could not.
    pub fn load(path: &[u16]) -> Option<DataFile> {
        let path = nul_ended(path);
        // SAFETY: a NUL-terminated path, loaded as a data file and image
        // resource: mapped to be read, none of its code run.
        let module = unsafe {
            LoadLibraryExW(
                path.as_ptr(),
                core::ptr::null_mut(),
                LOAD_LIBRARY_AS_DATAFILE | LOAD_LIBRARY_AS_IMAGE_RESOURCE,
            )
        };
        (!module.is_null()).then_some(DataFile(module))
    }

    /// A copy of resource `id` of the numbered type `kind` (RT_*).
    pub fn by_id(&self, kind: u16, id: u16) -> Option<Vec<u8>> {
        // SAFETY: the file this holds, loaded; numbered names
        // (MAKEINTRESOURCE: below 0x10000, never read as strings).
        unsafe { copy(self.0, int_name(id), int_name(kind)) }
    }

    /// A copy of the `n`th resource (from 0) of the numbered type `kind`,
    /// in the order the file lists them.
    pub fn nth(&self, kind: u16, n: usize) -> Option<Vec<u8>> {
        struct Search {
            want: usize,
            seen: usize,
            found: Option<Vec<u8>>,
        }
        /// One resource of the type: copied when it is the one wanted.
        unsafe extern "system" fn each(
            module: HMODULE,
            kind: *const u16,
            name: *const u16,
            param: isize,
        ) -> i32 {
            // SAFETY: `param` is the `Search` below, alive across the
            // enumeration and reached only here.
            let search = unsafe { &mut *(param as *mut Search) };
            if search.seen == search.want {
                // SAFETY: the module being enumerated, and the type and
                // name the enumeration gives, valid during this call.
                search.found = unsafe { copy(module, name, kind) };
                return 0;
            }
            search.seen += 1;
            1
        }
        let mut search = Search {
            want: n,
            seen: 0,
            found: None,
        };
        // SAFETY: the file this holds; a numbered type; the callback
        // above with the `Search` it expects, alive across the call.
        unsafe {
            EnumResourceNamesW(
                self.0,
                int_name(kind),
                Some(each),
                (&raw mut search) as isize,
            )
        };
        search.found
    }
}

impl Drop for DataFile {
    fn drop(&mut self) {
        // SAFETY: no pointers; the module this loaded, freed once.
        unsafe { FreeLibrary(self.0) };
    }
}

/// A numbered resource name or type (MAKEINTRESOURCE).
fn int_name(id: u16) -> *const u16 {
    core::ptr::without_provenance(usize::from(id))
}

/// Whether the `size` bytes at `data` are in one run of committed,
/// readable pages (a file's resource directory says where its data is and
/// how long; a file made to lie is not read past what is mapped).
fn readable(data: *const core::ffi::c_void, size: usize) -> bool {
    let mut mbi = MEMORY_BASIC_INFORMATION::default();
    // SAFETY: an out-struct and its size; the address is only looked up.
    let n = unsafe { VirtualQuery(data, &mut mbi, size_of::<MEMORY_BASIC_INFORMATION>()) };
    if n == 0 || mbi.State != MEM_COMMIT {
        return false;
    }
    let read = PAGE_READONLY
        | PAGE_READWRITE
        | PAGE_WRITECOPY
        | PAGE_EXECUTE_READ
        | PAGE_EXECUTE_READWRITE
        | PAGE_EXECUTE_WRITECOPY;
    let end = (mbi.BaseAddress as usize).saturating_add(mbi.RegionSize);
    mbi.Protect & read != 0 && (data as usize).checked_add(size).is_some_and(|e| e <= end)
}

/// A copy of resource `name` of type `kind` in `module`.
///
/// # Safety
/// `module` is loaded; `name` and `kind` are resource names (numbers
/// below 0x10000, or NUL-terminated strings).
unsafe fn copy(module: HMODULE, name: *const u16, kind: *const u16) -> Option<Vec<u8>> {
    // SAFETY: as the caller promises; the module stays loaded across the
    // calls.
    let (data, size) = unsafe {
        let found = FindResourceW(module, name, kind);
        if found.is_null() {
            return None;
        }
        let size = SizeofResource(module, found) as usize;
        (LockResource(LoadResource(module, found)), size)
    };
    if data.is_null() || size == 0 || !readable(data, size) {
        return None;
    }
    // SAFETY: `size` bytes of the loaded module's mapping, readable
    // (checked above), copied while it stays loaded.
    Some(unsafe { core::slice::from_raw_parts(data.cast::<u8>(), size) }.to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_library_s_icon_groups_are_read_as_data() {
        const RT_GROUP_ICON: u16 = 14;
        let path: Vec<u16> = r"C:\Windows\System32\shell32.dll".encode_utf16().collect();
        let dll = DataFile::load(&path).unwrap();
        // An icon directory: reserved 0, type 1.
        let first = dll.nth(RT_GROUP_ICON, 0).unwrap();
        assert_eq!(&first[..4], &[0, 0, 1, 0]);
        assert!(dll.nth(RT_GROUP_ICON, 1_000_000).is_none());
        assert!(dll.by_id(RT_GROUP_ICON, 0xfff0).is_none());
    }
}
