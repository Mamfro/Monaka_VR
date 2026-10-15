//! Leaves a trace when the game dies while a producer runs: a Rust panic (a producer aborts on
//! one, which ends the game without a word) and the first access violations anywhere in the
//! process (module and offset), before the game's own crash handling takes over. The lines go to
//! the sink the producer installs (its session log).
//!
//! The exception handler runs on the faulting thread, which may have faulted inside the heap: it
//! builds its lines on the stack ([`StackLine`]) and asks the system for module names into stack
//! buffers, so it allocates nothing itself.

use std::ffi::c_void;
use std::fmt::{self, Write};
use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};
use windows::Win32::Foundation::HMODULE;
use windows::Win32::System::Diagnostics::Debug::{AddVectoredExceptionHandler, EXCEPTION_POINTERS, RemoveVectoredExceptionHandler};
use windows::Win32::System::LibraryLoader::{
    GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS, GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT, GetModuleFileNameW, GetModuleHandleExW,
};
use windows::core::PCWSTR;

/// A line of text in a fixed buffer on the stack; what does not fit is cut (at a character).
pub(crate) struct StackLine {
    bytes: [u8; 1024],
    len: usize,
}

impl StackLine {
    pub(crate) const fn new() -> Self {
        Self { bytes: [0; 1024], len: 0 }
    }

    pub(crate) fn as_str(&self) -> &str {
        // Only whole characters are ever written in.
        std::str::from_utf8(&self.bytes[..self.len]).unwrap_or("")
    }
}

impl Write for StackLine {
    fn write_str(&mut self, text: &str) -> fmt::Result {
        let room = self.bytes.len() - self.len;
        let mut take = text.len().min(room);
        while !text.is_char_boundary(take) {
            take -= 1;
        }
        self.bytes[self.len..self.len + take].copy_from_slice(&text.as_bytes()[..take]);
        self.len += take;
        Ok(())
    }
}

/// The module holding `address`, if any.
fn module_of(address: usize) -> Option<HMODULE> {
    let mut handle = HMODULE::default();
    let flags = GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS | GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT;
    // SAFETY: with FROM_ADDRESS the name argument is an address to look up, not a string.
    unsafe { GetModuleHandleExW(flags, PCWSTR(address as *const u16), &mut handle) }.ok()?;
    Some(handle)
}

/// Writes where `address` is (`file.dll+0x1234`, or the bare address outside any module) into
/// `line`, with no allocation.
fn describe_into(line: &mut StackLine, address: usize) {
    let Some(module) = module_of(address) else {
        let _ = write!(line, "{address:#x} (no module)");
        return;
    };
    let mut path = [0u16; 260];
    // SAFETY: fills a stack buffer with the module's path.
    let length = unsafe { GetModuleFileNameW(Some(module), &mut path) } as usize;
    let path = &path[..length.min(path.len())];
    let name = path.iter().rposition(|&c| c == u16::from(b'\\')).map_or(path, |slash| &path[slash + 1..]);
    for c in char::decode_utf16(name.iter().copied()) {
        let _ = line.write_char(c.unwrap_or('?'));
    }
    let _ = write!(line, "+{:#x}", address - module.0 as usize);
}

/// Where crash lines go.
pub type Sink = fn(fmt::Arguments<'_>);

static HANDLER: AtomicUsize = AtomicUsize::new(0);
static SINK: AtomicUsize = AtomicUsize::new(0);
static LOGGED: AtomicU32 = AtomicU32::new(0);
const EXCEPTION_CONTINUE_SEARCH: i32 = 0;

fn emit(args: fmt::Arguments<'_>) {
    let sink = SINK.load(Ordering::Acquire);
    if sink != 0 {
        // SAFETY: only `install` stores here, and it stores a `Sink`.
        let sink: Sink = unsafe { std::mem::transmute::<usize, Sink>(sink) };
        sink(args);
    }
}

unsafe extern "system" fn vectored(info: *mut EXCEPTION_POINTERS) -> i32 {
    // SAFETY: the system passes a valid record for the exception being dispatched.
    let record = unsafe { &*(*info).ExceptionRecord };
    let code = record.ExceptionCode.0 as u32;
    // Access violation, stack buffer overrun / fail-fast, illegal or privileged instruction.
    if matches!(code, 0xC000_0005 | 0xC000_0409 | 0xC000_001D | 0xC000_0096) && LOGGED.fetch_add(1, Ordering::Relaxed) < 8 {
        let mut line = StackLine::new();
        let _ = write!(line, "exception {code:#x} at ");
        describe_into(&mut line, record.ExceptionAddress as usize);
        if code == 0xC000_0005 && record.NumberParameters >= 2 {
            let kind = match record.ExceptionInformation[0] {
                0 => "reading",
                1 => "writing",
                _ => "executing",
            };
            let _ = write!(line, " {kind} {:#x}", record.ExceptionInformation[1]);
        }
        let _ = write!(line, " (thread {})", crate::thread_id());
        emit(format_args!("{}", line.as_str()));
        // SAFETY: the system passes the faulting thread's context with the record.
        let stack = crate::arch::stack_pointer(unsafe { &*(*info).ContextRecord });
        let mut line = StackLine::new();
        let _ = line.write_str("  on its stack: ");
        stack_code(&mut line, stack);
        emit(format_args!("{}", line.as_str()));
    }
    EXCEPTION_CONTINUE_SEARCH
}

/// Writes the first code addresses in modules found on a stack from `rsp` up into `line` (return
/// addresses, mostly: who called into the fault, and whether a producer's detour was among them).
fn stack_code(line: &mut StackLine, rsp: usize) {
    let mut found = 0;
    for slot in 0..1024 {
        let Some(value) = crate::mem::read::<usize>(rsp + slot * size_of::<usize>()) else { break };
        if module_of(value).is_some() && after_call(value) {
            if found > 0 {
                let _ = line.write_str(" < ");
            }
            describe_into(line, value);
            found += 1;
            if found >= 16 {
                break;
            }
        }
    }
    if found == 0 {
        let _ = line.write_str("nothing in a module");
    }
}

/// Whether `address` follows a call instruction (so is a return address): `call rel32`,
/// `call [rip+disp32]`, `call reg`, `call [reg+disp8]`, `call [reg+disp32]`.
fn after_call(address: usize) -> bool {
    let mut b = [0u8; 7];
    if !crate::mem::read_bytes(address - 7, &mut b) {
        return false;
    }
    b[2] == 0xE8 || (b[1] == 0xFF && b[2] == 0x15) || (b[5] == 0xFF && b[6] & 0xF8 == 0xD0) || (b[4] == 0xFF && b[5] & 0xF8 == 0x50) || (b[1] == 0xFF && b[2] & 0xF8 == 0x90)
}

/// Starts logging panics and the first faults to `sink`.
pub fn install(sink: Sink) {
    SINK.store(sink as usize, Ordering::Release);
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        emit(format_args!("PANIC (the game is about to close): {info}"));
        previous(info);
    }));
    if HANDLER.load(Ordering::Acquire) == 0 {
        // SAFETY: registers a handler that only reads the record and logs; it stays valid while
        // this DLL is loaded (a producer is never unloaded).
        let handle = unsafe { AddVectoredExceptionHandler(0, Some(vectored)) };
        HANDLER.store(handle as usize, Ordering::Release);
    }
}

/// Stops logging faults (the panic hook stays: the DLL stays loaded and its log with it).
pub fn remove() {
    let handle = HANDLER.swap(0, Ordering::AcqRel);
    if handle != 0 {
        // SAFETY: removes the handler this module added.
        unsafe { RemoveVectoredExceptionHandler(handle as *const c_void) };
    }
}

#[cfg(test)]
mod tests;
