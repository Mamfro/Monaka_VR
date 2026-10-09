//! Freezing the process's other threads while code is patched, and checking none of them is
//! stopped inside the bytes being replaced.

use std::ops::Range;
use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::System::Diagnostics::Debug::{CONTEXT, GetThreadContext};
use windows::Win32::System::Diagnostics::ToolHelp::{CreateToolhelp32Snapshot, TH32CS_SNAPTHREAD, THREADENTRY32, Thread32First, Thread32Next};
use windows::Win32::System::Threading::{
    GetCurrentProcessId, GetCurrentThreadId, OpenThread, ResumeThread, SuspendThread, THREAD_GET_CONTEXT, THREAD_QUERY_INFORMATION,
    THREAD_SUSPEND_RESUME,
};

pub struct Frozen {
    threads: Vec<HANDLE>,
}

impl Frozen {
    /// Suspends every other thread of this process (threads that cannot be opened are skipped).
    /// Nothing is allocated once the first thread is suspended: a suspended thread may hold the
    /// heap lock.
    pub fn others() -> Self {
        let mut ids = Vec::with_capacity(256);
        // SAFETY: plain Toolhelp enumeration of this process's threads.
        unsafe {
            let (process, me) = (GetCurrentProcessId(), GetCurrentThreadId());
            if let Ok(snapshot) = CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0) {
                let mut entry = THREADENTRY32 { dwSize: size_of::<THREADENTRY32>() as u32, ..Default::default() };
                let mut more = Thread32First(snapshot, &mut entry).is_ok();
                while more {
                    if entry.th32OwnerProcessID == process && entry.th32ThreadID != me {
                        ids.push(entry.th32ThreadID);
                    }
                    more = Thread32Next(snapshot, &mut entry).is_ok();
                }
                let _ = CloseHandle(snapshot);
            }
        }
        let mut threads = Vec::with_capacity(ids.len());
        for id in ids {
            let access = THREAD_SUSPEND_RESUME | THREAD_GET_CONTEXT | THREAD_QUERY_INFORMATION;
            // SAFETY: each opened handle is suspended once here and resumed and closed once in
            // Drop; `threads` has room for every id, so pushing never allocates.
            unsafe {
                if let Ok(thread) = OpenThread(access, false, id) {
                    if SuspendThread(thread) != u32::MAX {
                        threads.push(thread);
                    } else {
                        let _ = CloseHandle(thread);
                    }
                }
            }
        }
        Self { threads }
    }

    /// Whether any frozen thread would resume inside one of `ranges`.
    pub fn any_inside(&self, ranges: &[Range<usize>]) -> bool {
        self.threads.iter().any(|&thread| {
            let mut context = CONTEXT { ContextFlags: crate::arch::CONTROL, ..Default::default() };
            // SAFETY: reads a suspended thread's registers into an aligned local.
            let ok = unsafe { GetThreadContext(thread, &mut context) }.is_ok();
            let ip = crate::arch::instruction_pointer(&context);
            // An unreadable context is treated as inside: patching is retried, not risked.
            !ok || ranges.iter().any(|range| range.contains(&ip))
        })
    }
}

impl Drop for Frozen {
    fn drop(&mut self) {
        for &thread in &self.threads {
            // SAFETY: resumes and closes each handle this value suspended and owns.
            unsafe {
                ResumeThread(thread);
                let _ = CloseHandle(thread);
            }
        }
    }
}
