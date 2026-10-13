//! Finding things in a running game: what class an object is, which of its fields hold a value
//! the game is known to be using, and who called a hooked function.

use crate::mem;
use windows::Win32::System::Diagnostics::Debug::RtlCaptureStackBackTrace;

/// The MSVC RTTI name of a polymorphic C++ object (for example `.?AVPlayerDI_PH@@`), read through
/// its vtable's complete object locator.
pub fn class_name(object: usize) -> Option<String> {
    let vtable = mem::read::<usize>(object)?;
    let locator = mem::read::<usize>(vtable.checked_sub(8)?)?;
    let base = locator.checked_sub(mem::read::<u32>(locator + 0x14)? as usize)?;
    let descriptor = base + mem::read::<u32>(locator + 0x0c)? as usize;
    let mut name = [0u8; 160];
    // The name may end before the buffer does; read what is readable, a chunk at a time.
    let mut length = 0;
    while length < name.len() {
        let chunk = 16.min(name.len() - length);
        if !mem::read_bytes(descriptor + 0x10 + length, &mut name[length..length + chunk]) {
            break;
        }
        length += chunk;
        if name[..length].contains(&0) {
            break;
        }
    }
    let end = name[..length].iter().position(|&b| b == 0)?;
    let text = std::str::from_utf8(&name[..end]).ok()?;
    text.starts_with(".?A").then(|| text.to_owned())
}

/// The start of the complete C++ object that `object` (possibly a base-class subobject in its
/// middle) belongs to, from the RTTI locator's offset.
pub fn complete_object(object: usize) -> Option<usize> {
    let vtable = mem::read::<usize>(object)?;
    let locator = mem::read::<usize>(vtable.checked_sub(8)?)?;
    object.checked_sub(mem::read::<u32>(locator + 4)? as usize)
}

/// Every 4-byte-aligned float in `object..object+length` that `matches` labels, as
/// `(offset, value, label)`. Unreadable stretches are skipped.
pub fn find_floats(object: usize, length: usize, mut matches: impl FnMut(f32) -> Option<&'static str>) -> Vec<(usize, f32, &'static str)> {
    let mut found = Vec::new();
    let mut page = [0u8; 256];
    let mut offset = 0;
    while offset < length {
        let size = page.len().min(length - offset);
        if mem::read_bytes(object + offset, &mut page[..size]) {
            for (i, bytes) in page[..size].chunks_exact(4).enumerate() {
                let value = f32::from_le_bytes(bytes.try_into().expect("4 bytes"));
                if value.is_finite()
                    && let Some(label) = matches(value)
                {
                    found.push((offset + i * 4, value, label));
                }
            }
        }
        offset += size;
    }
    found
}


/// Labels a float that equals `angle` (radians) as radians or degrees, either sign, or as the
/// sine of it (a direction's component).
pub fn angle_matcher(angle: f32) -> impl FnMut(f32) -> Option<&'static str> {
    let degrees = angle.to_degrees();
    let sine = angle.sin();
    move |v| {
        if (v - angle).abs() < 0.002 {
            Some("rad")
        } else if (v + angle).abs() < 0.002 {
            Some("-rad")
        } else if (v - degrees).abs() < 0.1 {
            Some("deg")
        } else if (v + degrees).abs() < 0.1 {
            Some("-deg")
        } else if (v - sine).abs() < 0.002 {
            Some("sin")
        } else if (v + sine).abs() < 0.002 {
            Some("-sin")
        } else {
            None
        }
    }
}

/// Where the game called the hooked function in progress from: the first return address on the
/// stack outside this module (a detour is entered by a jump, so the frame above our own code is
/// the game's call site). Call it from inside a detour; 0 if the stack could not be walked.
#[inline(never)]
pub fn caller() -> usize {
    let Some(own) = crate::module::Module::containing(caller as fn() -> usize as usize) else { return 0 };
    let mut frames = [std::ptr::null_mut(); 16];
    // SAFETY: fills our buffer with return addresses.
    let captured = unsafe { RtlCaptureStackBackTrace(0, &mut frames, None) } as usize;
    frames[..captured.min(frames.len())].iter().map(|&f| f as usize).find(|&f| !own.contains(f)).unwrap_or(0)
}

/// The return addresses on the stack outside this module, nearest first (at most `most`): the
/// game's call chain into the hooked function in progress. Call it from inside a detour.
#[inline(never)]
pub fn callers(most: usize) -> Vec<usize> {
    let Some(own) = crate::module::Module::containing(callers as fn(usize) -> Vec<usize> as usize) else { return Vec::new() };
    let mut frames = [std::ptr::null_mut(); 32];
    // SAFETY: fills our buffer with return addresses.
    let captured = unsafe { RtlCaptureStackBackTrace(0, &mut frames, None) } as usize;
    frames[..captured.min(frames.len())].iter().map(|&f| f as usize).filter(|&f| !own.contains(f)).take(most).collect()
}

/// The caller of a hooked function, cheaply, for hot paths where [`caller`]'s stack walk is too
/// slow. A hook entered through a `caller_shim!` stub runs two instructions first: they copy the
/// return address at `[rsp]` into `r9` and jump to the Rust detour, which takes it as its fourth
/// argument. Only functions with at most three arguments can be shimmed (their `r9` carries
/// nothing); the detour pads the arguments it lacks. The stub adds no stack frame (it ends in a
/// jump), so stack walks and unwinding see the game's frames unchanged. (A thread-local slot would
/// serve any function, but DL2 has used up the TEB's 64 directly reachable TLS slots.)
///
/// `caller_shim!(stub => detour)` defines `stub`, an entry for `detour(a, b, c, caller)`; hook the
/// target with the stub cast to the target's function type ([`as_detour`]). 64-bit games only: it
/// relies on the x64 calling convention.
#[macro_export]
macro_rules! caller_shim {
    ($stub:ident => $detour:path) => {
        #[unsafe(naked)]
        pub unsafe extern "system" fn $stub() {
            core::arch::naked_asm!("mov r9, [rsp]", "jmp {detour}", detour = sym $detour)
        }
    };
}

/// A [`caller_shim!`] stub as a hook detour of type `F` (the target's function type).
///
/// # Safety
/// `F` must be a function pointer type of at most three integer or pointer arguments, and the
/// stub's detour must take those arguments (padded to three) followed by the caller.
pub unsafe fn as_detour<F: Copy>(stub: unsafe extern "system" fn()) -> F {
    debug_assert_eq!(size_of::<F>(), size_of::<usize>());
    // SAFETY: both are pointer-sized function pointers (the caller's guarantee for F).
    unsafe { std::mem::transmute_copy(&stub) }
}

// x86-64 machine code; the 32-bit tests are in `arch/x86.rs`.
#[cfg(all(test, target_arch = "x86_64"))]
mod tests {
    use super::*;
    use windows::Win32::System::Memory::{MEM_COMMIT, MEM_RESERVE, PAGE_EXECUTE_READWRITE, VirtualAlloc};

    unsafe extern "system" fn shimmed_detour(a: usize, b: usize, _: usize, caller: usize) -> usize {
        assert_eq!((a, b), (7, 9));
        caller
    }

    crate::caller_shim!(shim_stub => shimmed_detour);

    /// Calls through the stub from a small function of its own, returning what the detour saw.
    #[inline(never)]
    fn call_through_shim() -> usize {
        // SAFETY: the stub forwards to `shimmed_detour`, which takes two arguments and the caller.
        let shimmed: unsafe extern "system" fn(usize, usize) -> usize = unsafe { as_detour(shim_stub) };
        unsafe { shimmed(std::hint::black_box(7), std::hint::black_box(9)) }
    }

    #[test]
    fn the_shimmed_detour_sees_its_call_site() {
        let caller = call_through_shim();
        let here = call_through_shim as fn() -> usize as usize;
        assert!(caller > here && caller < here + 256, "caller {caller:#x} is inside the calling function ({here:#x})");
    }

    #[test]
    fn floats_matching_an_angle() {
        let block: [f32; 6] = [1.0, -40.75, 0.7112, 9.0, (-0.7112f32).sin(), 40.75];
        let found = find_floats(block.as_ptr() as usize, size_of_val(&block), angle_matcher(-0.7112));
        let labels: Vec<_> = found.iter().map(|&(offset, _, label)| (offset, label)).collect();
        assert_eq!(labels, vec![(4, "deg"), (8, "-rad"), (16, "sin"), (20, "-deg")]);
        assert!(find_floats(8, 64, angle_matcher(0.5)).is_empty(), "unreadable memory is skipped");
    }

    #[test]
    fn rust_objects_have_no_rtti() {
        let plain: [usize; 4] = [0; 4];
        assert_eq!(class_name(plain.as_ptr() as usize), None);
    }

    static SEEN: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    unsafe extern "system" fn detour() {
        SEEN.store(caller(), std::sync::atomic::Ordering::SeqCst);
    }

    #[test]
    fn caller_is_the_call_site() {
        // sub rsp,0x28; mov rax,imm64; call rax; add rsp,0x28; ret
        let mut code = vec![0x48, 0x83, 0xec, 0x28, 0x48, 0xb8];
        code.extend_from_slice(&(detour as unsafe extern "system" fn() as usize as u64).to_le_bytes());
        code.extend_from_slice(&[0xff, 0xd0, 0x48, 0x83, 0xc4, 0x28, 0xc3]);
        // SAFETY: a fresh RWX page receives the test's machine code, then runs it.
        unsafe {
            let page = VirtualAlloc(None, 4096, MEM_RESERVE | MEM_COMMIT, PAGE_EXECUTE_READWRITE) as usize;
            std::ptr::copy_nonoverlapping(code.as_ptr(), page as *mut u8, code.len());
            let stub: unsafe extern "system" fn() = std::mem::transmute(page);
            stub();
            let seen = SEEN.load(std::sync::atomic::Ordering::SeqCst);
            assert_eq!(seen, page + 16, "the address after `call rax` (page {page:#x}, seen {seen:#x})");
        }
    }
}
