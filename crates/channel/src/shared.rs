//! Named kernel objects: handles, mutexes, and shared-memory records guarded by a mutex.

use crate::{Error, Result};
use monaka_core::console::ConsoleBlock;
use monaka_core::debug_draw::DebugBlock;
use monaka_core::page::PageBlock;
use monaka_core::panels::PanelsBlock;
use monaka_core::protocol::{FenceHeader, FingerBlock, HandBlock, PoseBlock, ReticleBlock, Stamp, Versioned};
use std::marker::PhantomData;
use std::ops::{Deref, DerefMut};
use std::ptr::NonNull;
use windows::Win32::Foundation::{
    CloseHandle, ERROR_ALREADY_EXISTS, GetLastError, HANDLE, INVALID_HANDLE_VALUE, WAIT_ABANDONED, WAIT_OBJECT_0,
};
use windows::Win32::System::Memory::{
    CreateFileMappingW, FILE_MAP_ALL_ACCESS, FILE_MAP_READ, MEMORY_MAPPED_VIEW_ADDRESS, MapViewOfFile, OpenFileMappingW,
    PAGE_READWRITE, UnmapViewOfFile,
};
use windows::Win32::System::Threading::{
    CreateMutexW, MUTEX_MODIFY_STATE, OpenMutexW, ReleaseMutex, SYNCHRONIZATION_SYNCHRONIZE, WaitForSingleObject,
};
use windows::core::HSTRING;

/// Plain data that may live in shared memory.
///
/// # Safety
/// Implementors must be `#[repr(C)]` and made only of integers, floats and arrays or structs of
/// them, so every bit pattern another process writes is a valid value.
pub unsafe trait SharedRecord: Copy {}

// SAFETY: repr(C) records of integers and floats only.
unsafe impl SharedRecord for PoseBlock {}
unsafe impl SharedRecord for HandBlock {}
unsafe impl SharedRecord for FingerBlock {}
unsafe impl SharedRecord for ReticleBlock {}
unsafe impl SharedRecord for DebugBlock {}
unsafe impl SharedRecord for PageBlock {}
unsafe impl SharedRecord for ConsoleBlock {}
unsafe impl SharedRecord for PanelsBlock {}

unsafe impl SharedRecord for FenceHeader {}
unsafe impl SharedRecord for Stamp {}

impl<T: SharedRecord + Versioned> SharedBlock<T> {
    /// The block `name` (guarded by `lock`), made when it is not there yet and started when blank,
    /// so either side may create it; refused when it holds another layout. Waits up to `wait_ms`
    /// for the lock.
    pub fn open_versioned(name: &str, lock: &str, wait_ms: u32) -> Result<Self> {
        let mut block = Self::create(name, lock, Create::OrOpen)?;
        {
            let mut record = block.lock(wait_ms).tolerant().ok_or(Error::Busy)?;
            if record.is_blank() {
                record.start();
            }
            if !record.is_compatible() {
                let stamp = record.stamp();
                return Err(crate::incompatible_block(T::WHAT, (stamp.magic, stamp.version), T::MAGIC, T::VERSION));
            }
        }
        Ok(block)
    }
}

/// A kernel handle closed on drop.
#[derive(Debug)]
pub struct OwnedHandle(HANDLE);

// SAFETY: kernel handles are process-wide values, usable from any thread.
unsafe impl Send for OwnedHandle {}

impl OwnedHandle {
    /// Takes ownership of `handle`; a null or invalid handle is refused.
    pub fn new(handle: HANDLE) -> Option<Self> {
        (!handle.is_invalid() && handle != INVALID_HANDLE_VALUE).then_some(Self(handle))
    }

    pub fn raw(&self) -> HANDLE {
        self.0
    }
}

impl Drop for OwnedHandle {
    fn drop(&mut self) {
        // SAFETY: we own the handle and close it once.
        let _ = unsafe { CloseHandle(self.0) };
    }
}

/// Outcome of trying to take a mutex.
pub enum Locked<G> {
    Held(G),
    /// The previous owner died while holding it; we hold it now.
    Abandoned(G),
    Busy,
}

impl<G> Locked<G> {
    /// Treats an abandoned lock as held: the records it guards are written whole, so a dead owner
    /// cannot leave one half-written.
    pub fn tolerant(self) -> Option<G> {
        match self {
            Locked::Held(g) | Locked::Abandoned(g) => Some(g),
            Locked::Busy => None,
        }
    }

    /// Treats an abandoned lock as a broken channel and releases it at once.
    pub fn strict(self) -> Result<G> {
        match self {
            Locked::Held(g) => Ok(g),
            Locked::Abandoned(_) => Err(Error::Unhealthy),
            Locked::Busy => Err(Error::Busy),
        }
    }
}

/// A named Win32 mutex.
#[derive(Debug)]
pub struct NamedMutex(OwnedHandle);

impl NamedMutex {
    /// Creates the mutex or opens an existing one; the flag says whether it already existed.
    pub fn create(name: &str) -> Result<(Self, bool)> {
        // SAFETY: plain Win32 call with a valid name.
        let handle = unsafe { CreateMutexW(None, false, &HSTRING::from(name)) }?;
        // SAFETY: reads the calling thread's last-error value, set by the call above.
        let existed = unsafe { GetLastError() } == ERROR_ALREADY_EXISTS;
        Ok((Self(OwnedHandle(handle)), existed))
    }

    pub fn open(name: &str) -> Result<Self> {
        // SAFETY: plain Win32 call with a valid name.
        let handle = unsafe { OpenMutexW(SYNCHRONIZATION_SYNCHRONIZE | MUTEX_MODIFY_STATE, false, &HSTRING::from(name)) }?;
        Ok(Self(OwnedHandle(handle)))
    }

    /// Waits at most `timeout_ms`. Win32 mutexes are re-entrant per thread, so `&mut self` keeps one
    /// guard per handle at a time.
    pub fn lock(&mut self, timeout_ms: u32) -> Locked<MutexGuard<'_>> {
        // SAFETY: waiting on a mutex handle we own.
        let wait = unsafe { WaitForSingleObject(self.0.raw(), timeout_ms) };
        let guard = || MutexGuard { mutex: self, _thread_bound: PhantomData };
        if wait == WAIT_OBJECT_0 {
            Locked::Held(guard())
        } else if wait == WAIT_ABANDONED {
            Locked::Abandoned(guard())
        } else {
            Locked::Busy
        }
    }
}

/// Holds a [`NamedMutex`] until dropped. A Win32 mutex must be released by the thread that took
/// it, so the guard is not `Send`.
pub struct MutexGuard<'a> {
    mutex: &'a NamedMutex,
    _thread_bound: PhantomData<*const ()>,
}

impl Drop for MutexGuard<'_> {
    fn drop(&mut self) {
        // SAFETY: this thread owns the mutex while the guard exists.
        let _ = unsafe { ReleaseMutex(self.mutex.0.raw()) };
    }
}

/// How to come by the shared memory and its mutex.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Create {
    /// Create both; fail if either already exists (a producer claiming a fresh channel name).
    Exclusive,
    /// Create both or open what exists (either side may arrive first).
    OrOpen,
}

/// One `T` in named shared memory, guarded by a named mutex. All access goes through [`lock`],
/// so the other process never writes while a reference into the memory is alive.
///
/// [`lock`]: SharedBlock::lock
pub struct SharedBlock<T: SharedRecord> {
    view: NonNull<T>,
    writable: bool,
    mutex: NamedMutex,
    _mapping: OwnedHandle,
}

// SAFETY: the memory is only reached through the mutex; the block can move between threads.
unsafe impl<T: SharedRecord> Send for SharedBlock<T> {}

impl<T: SharedRecord> SharedBlock<T> {
    pub fn create(name: &str, lock_name: &str, mode: Create) -> Result<Self> {
        let size = size_of::<T>() as u32;
        // SAFETY: page-file-backed mapping of size_of::<T>() bytes; the name is valid.
        let mapping = unsafe { CreateFileMappingW(INVALID_HANDLE_VALUE, None, PAGE_READWRITE, 0, size, &HSTRING::from(name)) }?;
        // SAFETY: reads the last-error value set by the call above.
        let existed = unsafe { GetLastError() } == ERROR_ALREADY_EXISTS;
        let mapping = OwnedHandle(mapping);
        if existed && mode == Create::Exclusive {
            return Err(Error::AlreadyExists(name.to_owned()));
        }
        let (mutex, lock_existed) = NamedMutex::create(lock_name)?;
        if lock_existed && mode == Create::Exclusive {
            return Err(Error::AlreadyExists(lock_name.to_owned()));
        }
        Self::map(mapping, mutex, true)
    }

    pub fn open(name: &str, lock_name: &str, writable: bool) -> Result<Self> {
        let access = if writable { FILE_MAP_ALL_ACCESS } else { FILE_MAP_READ };
        // SAFETY: plain Win32 call with a valid name.
        let mapping = unsafe { OpenFileMappingW(access.0, false, &HSTRING::from(name)) }?;
        let mapping = OwnedHandle(mapping);
        Self::map(mapping, NamedMutex::open(lock_name)?, writable)
    }

    fn map(mapping: OwnedHandle, mutex: NamedMutex, writable: bool) -> Result<Self> {
        let access = if writable { FILE_MAP_ALL_ACCESS } else { FILE_MAP_READ };
        // SAFETY: maps size_of::<T>() bytes; fails if the existing mapping is smaller.
        let view = unsafe { MapViewOfFile(mapping.raw(), access, 0, 0, size_of::<T>()) };
        let view = NonNull::new(view.Value.cast::<T>()).ok_or_else(windows::core::Error::from_thread)?;
        Ok(Self { view, writable, mutex, _mapping: mapping })
    }

    pub fn lock(&mut self, timeout_ms: u32) -> Locked<BlockGuard<'_, T>> {
        let view = self.view;
        let writable = self.writable;
        match self.mutex.lock(timeout_ms) {
            Locked::Held(lock) => Locked::Held(BlockGuard { view, writable, _lock: lock }),
            Locked::Abandoned(lock) => Locked::Abandoned(BlockGuard { view, writable, _lock: lock }),
            Locked::Busy => Locked::Busy,
        }
    }
}

impl<T: SharedRecord> Drop for SharedBlock<T> {
    fn drop(&mut self) {
        // SAFETY: unmaps the view we mapped; no guard can outlive the block.
        let _ = unsafe { UnmapViewOfFile(MEMORY_MAPPED_VIEW_ADDRESS { Value: self.view.as_ptr().cast() }) };
    }
}

/// Access to a [`SharedBlock`]'s record while its mutex is held.
pub struct BlockGuard<'a, T: SharedRecord> {
    view: NonNull<T>,
    writable: bool,
    _lock: MutexGuard<'a>,
}

impl<T: SharedRecord> Deref for BlockGuard<'_, T> {
    type Target = T;
    fn deref(&self) -> &T {
        // SAFETY: the view is mapped for the block's lifetime and the mutex keeps the other side
        // from writing; any bit pattern is a valid T (SharedRecord).
        unsafe { self.view.as_ref() }
    }
}

impl<T: SharedRecord> DerefMut for BlockGuard<'_, T> {
    fn deref_mut(&mut self) -> &mut T {
        assert!(self.writable, "shared block was opened read-only");
        // SAFETY: as for deref, and the view is mapped writable.
        unsafe { self.view.as_mut() }
    }
}
