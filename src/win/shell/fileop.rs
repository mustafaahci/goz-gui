//! The shell's file operation (IFileOperation) and a progress sink that
//! hands each item done to a closure.

use core::cell::RefCell;
use core::mem::ManuallyDrop;

use windows::Win32::Foundation::HWND as WHWND;
use windows::Win32::System::Com::{CLSCTX_ALL, CoCreateInstance};
use windows::Win32::UI::Shell::{
    FILEOPERATION_FLAGS, FileOperation as FILE_OPERATION, IFileOperation,
    IFileOperationProgressSink, IFileOperationProgressSink_Impl, IShellItem,
};
use windows::core::{HRESULT, PCWSTR, implement};
use windows_core::IUnknownImpl;
use windows_sys::Win32::Foundation::HWND;

use super::ShellItem;
use crate::win::{nul_ended, sys};

/// The shell's file operation (IFileOperation): items queued, then done at
/// once, with the shell's progress, questions and undo record. The names
/// queued are kept here until it drops (after the operation is released),
/// so the operation may read them whenever it does its work.
pub struct FileOperation {
    // Released before the names it was given are freed (fields drop in
    // order).
    op: IFileOperation,
    names: RefCell<Vec<Vec<u16>>>,
}

impl FileOperation {
    /// A new operation (CoCreateInstance, any context); this thread needs
    /// COM.
    pub fn new() -> windows::core::Result<FileOperation> {
        // SAFETY: no pointers but the class id, a constant.
        let op: IFileOperation = unsafe { CoCreateInstance(&FILE_OPERATION, None, CLSCTX_ALL) }?;
        Ok(FileOperation {
            op,
            names: RefCell::new(Vec::new()),
        })
    }

    /// A name to pass to the operation: kept with it, or null for `None`.
    fn name(&self, name: Option<&[u16]>) -> PCWSTR {
        let Some(name) = name else {
            return PCWSTR::null();
        };
        let mut names = self.names.borrow_mut();
        names.push(nul_ended(name).into_owned());
        // The buffer stays where it is when the list grows: only the list's
        // own vector moves.
        PCWSTR(names.last().map_or(core::ptr::null(), |n| n.as_ptr()))
    }

    /// The window that owns the operation's progress and questions.
    pub fn set_owner_window(&self, owner: HWND) -> windows::core::Result<()> {
        // SAFETY: a window handle, which the shell checks.
        unsafe { self.op.SetOwnerWindow(WHWND(owner)) }
    }

    /// FOF_* and FOFX_* flags.
    pub fn set_operation_flags(&self, flags: u32) -> windows::core::Result<()> {
        // SAFETY: flags, no pointers.
        unsafe { self.op.SetOperationFlags(FILEOPERATION_FLAGS(flags)) }
    }

    /// Tells `sink` of each item as it is done (the operation keeps it
    /// until it is released, or [`FileOperation::unadvise`]); the cookie
    /// that `unadvise` takes.
    pub fn advise(&self, sink: &ProgressSink) -> windows::core::Result<u32> {
        // SAFETY: a live sink, which the operation keeps a reference to.
        unsafe { self.op.Advise(&sink.0) }
    }

    /// Stops telling the sink `cookie` names.
    pub fn unadvise(&self, cookie: u32) -> windows::core::Result<()> {
        // SAFETY: a cookie, no pointers.
        unsafe { self.op.Unadvise(cookie) }
    }

    /// Queues a rename of `item` to `name` (a name, not a path).
    pub fn rename_item(&self, item: &ShellItem, name: &[u16]) -> windows::core::Result<()> {
        let n = self.name(Some(name));
        // SAFETY: a live item; a NUL-terminated name kept with the
        // operation; no sink of the item's own.
        unsafe {
            self.op
                .RenameItem(&item.0, n, None::<&IFileOperationProgressSink>)
        }
    }

    /// Queues a move of `item` into `folder`, as `name` when given (else
    /// under its own name).
    pub fn move_item(
        &self,
        item: &ShellItem,
        folder: &ShellItem,
        name: Option<&[u16]>,
    ) -> windows::core::Result<()> {
        let n = self.name(name);
        // SAFETY: live items; a name kept with the operation, or null.
        unsafe {
            self.op
                .MoveItem(&item.0, &folder.0, n, None::<&IFileOperationProgressSink>)
        }
    }

    /// Queues a copy of `item` into `folder`, as `name` when given (else
    /// under its own name).
    pub fn copy_item(
        &self,
        item: &ShellItem,
        folder: &ShellItem,
        name: Option<&[u16]>,
    ) -> windows::core::Result<()> {
        let n = self.name(name);
        // SAFETY: live items; a name kept with the operation, or null.
        unsafe {
            self.op
                .CopyItem(&item.0, &folder.0, n, None::<&IFileOperationProgressSink>)
        }
    }

    /// Queues the deletion of `item` (to the Recycle Bin, as the flags say).
    pub fn delete_item(&self, item: &ShellItem) -> windows::core::Result<()> {
        // SAFETY: a live item; no sink of its own.
        unsafe {
            self.op
                .DeleteItem(&item.0, None::<&IFileOperationProgressSink>)
        }
    }

    /// Queues a new item `name` in `folder`, with FILE_ATTRIBUTE_*
    /// `attributes` (FILE_ATTRIBUTE_DIRECTORY: a folder), from no template.
    pub fn new_item(
        &self,
        folder: &ShellItem,
        attributes: u32,
        name: &[u16],
    ) -> windows::core::Result<()> {
        let n = self.name(Some(name));
        // SAFETY: a live folder; a name kept with the operation; no
        // template.
        unsafe {
            self.op.NewItem(
                &folder.0,
                attributes,
                n,
                PCWSTR::null(),
                None::<&IFileOperationProgressSink>,
            )
        }
    }

    /// Does what is queued (its windows run their own message loops).
    pub fn perform(&self) -> windows::core::Result<()> {
        // SAFETY: no pointers; the operation stays alive through the call
        // (`&self` holds it).
        unsafe { self.op.PerformOperations() }
    }

    /// Whether any of it was stopped or skipped (by the user).
    pub fn any_aborted(&self) -> windows::core::Result<bool> {
        // SAFETY: no pointers.
        unsafe { self.op.GetAnyOperationsAborted() }.map(|b| b.as_bool())
    }
}

/// Which kind of item a file operation's progress sink reports done.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ItemOp {
    /// PostRenameItem.
    Rename,
    /// PostMoveItem.
    Move,
    /// PostCopyItem.
    Copy,
    /// PostDeleteItem.
    Delete,
    /// PostNewItem.
    New,
}

/// What a progress sink hands each item done to: the kind, the item (for a
/// new item, the folder it went in), the item it is now, and how it went.
type OnPost = Box<dyn Fn(ItemOp, Option<&ShellItem>, Option<&ShellItem>, HRESULT)>;

/// A value used only on the thread that made it. COM calls an object that
/// is agile (as `#[implement]` makes it) from whatever thread it is on, and
/// may release it from any; so each use checks the thread first, and the
/// value is dropped only on its own thread (left alone on another, where
/// its drop could touch what is not `Send`).
struct OnThread<T> {
    owner: u32,
    value: ManuallyDrop<T>,
}

impl<T> OnThread<T> {
    fn new(value: T) -> OnThread<T> {
        OnThread {
            owner: sys::thread_id(),
            value: ManuallyDrop::new(value),
        }
    }

    /// The value, on its own thread (a thread id is reused only once its
    /// thread has ended, and with it everything else of that thread that
    /// the value could share).
    fn get(&self) -> Option<&T> {
        (sys::thread_id() == self.owner).then_some(&*self.value)
    }
}

impl<T> Drop for OnThread<T> {
    fn drop(&mut self) {
        // A thread id is reused only once its thread has ended, and with it
        // everything else of that thread that the value could share.
        if sys::thread_id() == self.owner {
            // SAFETY: dropped once, here, on the thread that made it.
            unsafe { ManuallyDrop::drop(&mut self.value) };
        }
    }
}

/// A progress sink for [`FileOperation::advise`] (IFileOperationProgressSink)
/// that calls a closure for each item an operation has done.
pub struct ProgressSink(IFileOperationProgressSink);

impl ProgressSink {
    /// A sink that calls `on_post` for each item done: the kind, the item
    /// (for a new item, the folder it went in), the item it is now (`None`
    /// when there is none), and how it went. `on_post` runs only on this
    /// thread (the operation's), and is dropped here.
    pub fn new(
        on_post: impl Fn(ItemOp, Option<&ShellItem>, Option<&ShellItem>, HRESULT) + 'static,
    ) -> ProgressSink {
        ProgressSink(
            Sink {
                on_post: OnThread::new(Box::new(on_post)),
            }
            .into(),
        )
    }
}

/// The COM object behind a [`ProgressSink`]. A method that runs the
/// closure holds a reference to the object meanwhile: the closure may let
/// the last other one go.
#[implement(IFileOperationProgressSink)]
struct Sink {
    on_post: OnThread<OnPost>,
}

impl Sink {
    fn post(&self, op: ItemOp, item: ItemRef<'_>, new: ItemRef<'_>, hr: HRESULT) {
        if let Some(f) = self.on_post.get() {
            // The items as the shell lends them, each with a reference of
            // its own for the call.
            let item = item.as_ref().cloned().map(ShellItem);
            let new = new.as_ref().cloned().map(ShellItem);
            f(op, item.as_ref(), new.as_ref(), hr);
        }
    }
}

type R = windows::core::Result<()>;
type ItemRef<'a> = windows::core::Ref<'a, IShellItem>;

#[allow(non_snake_case, reason = "the interface's method names")]
impl IFileOperationProgressSink_Impl for Sink_Impl {
    fn StartOperations(&self) -> R {
        Ok(())
    }
    fn FinishOperations(&self, _hr: HRESULT) -> R {
        Ok(())
    }
    fn PreRenameItem(&self, _f: u32, _item: ItemRef<'_>, _name: &PCWSTR) -> R {
        Ok(())
    }
    fn PostRenameItem(
        &self,
        _f: u32,
        item: ItemRef<'_>,
        _name: &PCWSTR,
        hr: HRESULT,
        new: ItemRef<'_>,
    ) -> R {
        let _hold = self.to_object();
        self.post(ItemOp::Rename, item, new, hr);
        Ok(())
    }
    fn PreMoveItem(&self, _f: u32, _item: ItemRef<'_>, _to: ItemRef<'_>, _name: &PCWSTR) -> R {
        Ok(())
    }
    fn PostMoveItem(
        &self,
        _f: u32,
        item: ItemRef<'_>,
        _to: ItemRef<'_>,
        _name: &PCWSTR,
        hr: HRESULT,
        new: ItemRef<'_>,
    ) -> R {
        let _hold = self.to_object();
        self.post(ItemOp::Move, item, new, hr);
        Ok(())
    }
    fn PreCopyItem(&self, _f: u32, _item: ItemRef<'_>, _to: ItemRef<'_>, _name: &PCWSTR) -> R {
        Ok(())
    }
    fn PostCopyItem(
        &self,
        _f: u32,
        item: ItemRef<'_>,
        _to: ItemRef<'_>,
        _name: &PCWSTR,
        hr: HRESULT,
        new: ItemRef<'_>,
    ) -> R {
        let _hold = self.to_object();
        self.post(ItemOp::Copy, item, new, hr);
        Ok(())
    }
    fn PreDeleteItem(&self, _f: u32, _item: ItemRef<'_>) -> R {
        Ok(())
    }
    fn PostDeleteItem(&self, _f: u32, item: ItemRef<'_>, hr: HRESULT, new: ItemRef<'_>) -> R {
        let _hold = self.to_object();
        self.post(ItemOp::Delete, item, new, hr);
        Ok(())
    }
    fn PreNewItem(&self, _f: u32, _folder: ItemRef<'_>, _name: &PCWSTR) -> R {
        Ok(())
    }
    fn PostNewItem(
        &self,
        _f: u32,
        folder: ItemRef<'_>,
        _name: &PCWSTR,
        _template: &PCWSTR,
        _attrs: u32,
        hr: HRESULT,
        new: ItemRef<'_>,
    ) -> R {
        let _hold = self.to_object();
        self.post(ItemOp::New, folder, new, hr);
        Ok(())
    }
    fn UpdateProgress(&self, _total: u32, _done: u32) -> R {
        Ok(())
    }
    fn ResetTimer(&self) -> R {
        Ok(())
    }
    fn PauseTimer(&self) -> R {
        Ok(())
    }
    fn ResumeTimer(&self) -> R {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_value_on_its_thread_is_had_there_only() {
        let v = std::sync::Arc::new(OnThread::new(5));
        assert_eq!(v.get(), Some(&5));
        let other = std::sync::Arc::clone(&v);
        let seen = std::thread::spawn(move || other.get().copied())
            .join()
            .unwrap();
        assert_eq!(seen, None);
    }
}
