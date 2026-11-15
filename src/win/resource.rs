//! Resources of programs and libraries, read without running them.

use windows_sys::Win32::Foundation::{FreeLibrary, HMODULE};
use windows_sys::Win32::System::LibraryLoader::{
    EnumResourceNamesW, FindResourceW, LOAD_LIBRARY_AS_DATAFILE, LOAD_LIBRARY_AS_IMAGE_RESOURCE,
    LoadLibraryExW, LoadResource, LockResource, SizeofResource,
};

use windows_sys::Win32::System::Memory::{
    MEM_COMMIT, MEM_IMAGE, MEM_MAPPED, MEMORY_BASIC_INFORMATION, PAGE_EXECUTE_READ, PAGE_GUARD,
    PAGE_READONLY, VirtualQuery,
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

/// The region of pages `at` is in, if it is committed, of a mapped file
/// (an image or a data file, never memory of the process's own), and
/// read-only with no guard page: memory nothing in the process writes, and
/// that a read does not change.
fn read_only_mapping(at: *const core::ffi::c_void) -> Option<MEMORY_BASIC_INFORMATION> {
    let mut mbi = MEMORY_BASIC_INFORMATION::default();
    // SAFETY: an out-struct and its size; the address is only looked up.
    let n = unsafe { VirtualQuery(at, &mut mbi, size_of::<MEMORY_BASIC_INFORMATION>()) };
    let access = mbi.Protect & 0xff;
    (n != 0
        && mbi.State == MEM_COMMIT
        && (mbi.Type == MEM_IMAGE || mbi.Type == MEM_MAPPED)
        && mbi.Protect & PAGE_GUARD == 0
        && (access == PAGE_READONLY || access == PAGE_EXECUTE_READ))
        .then_some(mbi)
}

/// Whether the `size` bytes at `data` are a resource's, read where a file's
/// resource directory says: in one run of read-only pages (see
/// [`read_only_mapping`]) of `module`'s own mapping, or of a resource file
/// the loader found for it (its language's .mui, a .mun: a mapping that is
/// itself a PE file, MZ). A file made to lie is so read neither past what
/// is mapped of it, nor in memory the process writes or another module's
/// pages it has nothing to do with.
fn readable(module: HMODULE, data: *const core::ffi::c_void, size: usize) -> bool {
    let Some(region) = read_only_mapping(data) else {
        return false;
    };
    let end = (region.BaseAddress as usize).saturating_add(region.RegionSize);
    if (data as usize).checked_add(size).is_none_or(|e| e > end) {
        return false;
    }
    // A module loaded as a data file and image resource is its mapping's
    // address with its low bits marking how it was loaded (a mapping
    // starts on a 64 KB boundary).
    let base = (module as usize) & !0xffff;
    let alloc = region.AllocationBase as usize;
    if alloc == base {
        return true;
    }
    let head = region.AllocationBase.cast_const();
    read_only_mapping(head).is_some_and(|first| {
        let room = (first.BaseAddress as usize).saturating_add(first.RegionSize);
        // SAFETY: two bytes of a committed, read-only mapping's first page
        // (checked just above to hold them), read as they are.
        alloc.saturating_add(2) <= room && unsafe { *head.cast::<[u8; 2]>() } == *b"MZ"
    })
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
    if data.is_null() || size == 0 || !readable(module, data, size) {
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
        // A program not loaded in this process otherwise: its own mapping.
        let path: Vec<u16> = r"C:\Windows\explorer.exe".encode_utf16().collect();
        let exe = DataFile::load(&path).unwrap();
        assert_eq!(&exe.nth(RT_GROUP_ICON, 0).unwrap()[..4], &[0, 0, 1, 0]);
        // Memory that is not the file's own mapping is never read as its
        // resource, readable or not.
        let elsewhere = [0u8; 16];
        assert!(!readable(dll.0, elsewhere.as_ptr().cast(), 16));
        assert!(!readable(dll.0, core::ptr::null(), 1));
    }
}
