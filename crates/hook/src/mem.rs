//! The game's memory, read and written without trusting it. Engine pointers can be stale or
//! half-written; these go through the kernel, so a bad address fails the call instead of
//! faulting the game.

use std::mem::{MaybeUninit, size_of};
use windows::Win32::System::Diagnostics::Debug::{ReadProcessMemory, WriteProcessMemory};
use windows::Win32::System::Threading::GetCurrentProcess;

/// Plain data: any bit pattern is a valid value.
///
/// # Safety
/// Implement only for integers, floats and arrays of them.
pub unsafe trait Plain: Copy + 'static {}

// SAFETY: integers and floats accept every bit pattern.
unsafe impl Plain for u8 {}
unsafe impl Plain for u16 {}
unsafe impl Plain for u32 {}
unsafe impl Plain for u64 {}
unsafe impl Plain for usize {}
unsafe impl Plain for i16 {}
unsafe impl Plain for i32 {}
unsafe impl Plain for i64 {}
unsafe impl Plain for f32 {}
// SAFETY: arrays of plain data are plain.
unsafe impl<T: Plain, const N: usize> Plain for [T; N] {}

pub fn read_bytes(address: usize, buffer: &mut [u8]) -> bool {
    if address == 0 || buffer.is_empty() {
        return false;
    }
    let mut done = 0usize;
    // SAFETY: the kernel checks the source range; the destination is our own buffer.
    let ok = unsafe {
        ReadProcessMemory(GetCurrentProcess(), address as *const _, buffer.as_mut_ptr().cast(), buffer.len(), Some(&mut done))
    };
    ok.is_ok() && done == buffer.len()
}

pub fn read<T: Plain>(address: usize) -> Option<T> {
    if address == 0 {
        return None;
    }
    let mut value = MaybeUninit::<T>::uninit();
    let mut done = 0usize;
    // SAFETY: the kernel checks the source range and fills our local.
    let ok = unsafe {
        ReadProcessMemory(GetCurrentProcess(), address as *const _, value.as_mut_ptr().cast(), size_of::<T>(), Some(&mut done))
    };
    // SAFETY: fully written plain data.
    (ok.is_ok() && done == size_of::<T>()).then(|| unsafe { value.assume_init() })
}

/// `N` floats (a position, a direction), if all are finite.
pub fn read_finite<const N: usize>(address: usize) -> Option<[f32; N]> {
    read::<[f32; N]>(address).filter(|v| v.iter().all(|c| c.is_finite()))
}

/// A NUL-terminated byte string of at most `max` bytes, as text.
pub fn read_c_string(address: usize, max: usize) -> Option<String> {
    let mut text = Vec::new();
    while text.len() < max {
        let byte = read::<u8>(address + text.len())?;
        if byte == 0 {
            return String::from_utf8(text).ok();
        }
        text.push(byte);
    }
    None
}

/// Writes plain data into the game's memory; false if the address is not writable.
pub fn write<T: Plain>(address: usize, value: T) -> bool {
    if address == 0 {
        return false;
    }
    let mut done = 0usize;
    // SAFETY: the kernel checks the destination range; the source is our own value.
    let ok = unsafe {
        WriteProcessMemory(GetCurrentProcess(), address as *const _, (&value as *const T).cast(), size_of::<T>(), Some(&mut done))
    };
    ok.is_ok() && done == size_of::<T>()
}

/// Writes a borrowed byte buffer without trusting the destination address.
pub fn write_bytes(address: usize, bytes: &[u8]) -> bool {
    if address == 0 || bytes.is_empty() { return false; }
    let mut done = 0usize;
    // SAFETY: the kernel validates the destination; the source is our own slice.
    let ok = unsafe { WriteProcessMemory(GetCurrentProcess(), address as *const _, bytes.as_ptr().cast(), bytes.len(), Some(&mut done)) };
    ok.is_ok() && done == bytes.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn byte_buffers_are_bounded_and_bad_destinations_fail() {
        let mut target = [9u8;8];
        let address = target.as_mut_ptr() as usize;
        assert!(write_bytes(address+2,&[1,2,3]));
        assert_eq!(read::<[u8;8]>(address),Some([9,9,1,2,3,9,9,9]));
        assert!(!write_bytes(address,&[]));
        assert!(!write_bytes(0,&[1]));
        assert!(!write_bytes(8,&[1,2,3]));
        std::hint::black_box(&mut target);
    }

    #[test]
    fn reads_and_writes_survive_bad_addresses() {
        let mut target = [1.0f32, 2.0, 3.0];
        let address = target.as_mut_ptr() as usize;
        assert_eq!(read::<[f32; 3]>(address), Some([1.0, 2.0, 3.0]));
        assert!(write(address + 4, 5.0f32));
        assert_eq!(read::<f32>(address + 4), Some(5.0));
        assert_eq!(read::<u64>(0), None);
        assert_eq!(read::<u64>(8), None, "unmapped low memory fails instead of faulting");
        assert!(!write(8, 1u32));
        std::hint::black_box(&mut target);
    }
}
