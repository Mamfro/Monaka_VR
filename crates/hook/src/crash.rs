//! Leaves a trace when the game dies while a producer runs: a Rust panic (a producer aborts on
//! one, which ends the game without a word) and the first access violations anywhere in the
//! process (module and offset), before the game's own crash handling takes over. The lines go to
//! the sink the producer installs (its session log).

use crate::module::Module;
use std::ffi::c_void;
use std::fmt;
use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};
use windows::Win32::System::Diagnostics::Debug::{AddVectoredExceptionHandler, EXCEPTION_POINTERS, RemoveVectoredExceptionHandler};

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
        let target = if code == 0xC000_0005 && record.NumberParameters >= 2 {
            let kind = match record.ExceptionInformation[0] {
                0 => "reading",
                1 => "writing",
                _ => "executing",
            };
            format!(" {kind} {:#x}", record.ExceptionInformation[1])
        } else {
            String::new()
        };
        emit(format_args!("exception {code:#x} at {}{target} (thread {})", Module::describe(record.ExceptionAddress as usize), crate::thread_id()));
        // SAFETY: the system passes the faulting thread's context with the record.
        let stack = crate::arch::stack_pointer(unsafe { &*(*info).ContextRecord });
        emit(format_args!("  on its stack: {}", stack_code(stack)));
    }
    EXCEPTION_CONTINUE_SEARCH
}

/// The first code addresses in modules found on a stack from `rsp` up (return addresses, mostly:
/// who called into the fault, and whether a producer's detour was among them).
fn stack_code(rsp: usize) -> String {
    let mut found = Vec::new();
    for slot in 0..1024 {
        let Some(value) = crate::mem::read::<usize>(rsp + slot * size_of::<usize>()) else { break };
        if Module::containing(value).is_some() && after_call(value) {
            found.push(Module::describe(value));
            if found.len() >= 16 {
                break;
            }
        }
    }
    if found.is_empty() { "nothing in a module".to_string() } else { found.join(" < ") }
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
