//! Loaded modules: where they sit, their exports, and which build they are.

use crate::mem;
use monaka_core::scan::{Code, Section, section_headers};
use monaka_core::sha256::{Sha256, hex};
use std::ffi::CString;
use std::io::Read;
use std::path::PathBuf;
use windows::Win32::Foundation::{CloseHandle, HMODULE};
use windows::Win32::System::Diagnostics::ToolHelp::{CreateToolhelp32Snapshot, MODULEENTRY32W, Module32FirstW, Module32NextW, TH32CS_SNAPMODULE};
use windows::Win32::System::Memory::{MEM_COMMIT, MEMORY_BASIC_INFORMATION, PAGE_GUARD, PAGE_NOACCESS, VirtualQuery};
use windows::Win32::System::LibraryLoader::{
    GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS, GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT, GetModuleFileNameW, GetModuleHandleExW,
    GetModuleHandleW, GetProcAddress,
};
use windows::core::{HSTRING, PCSTR, PCWSTR};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Module {
    base: usize,
    size: usize,
}

impl Module {
    /// A module already loaded in this process, by file name.
    pub fn find(name: &str) -> Option<Self> {
        // SAFETY: looks up a loaded module without changing its reference count.
        let handle = unsafe { GetModuleHandleW(&HSTRING::from(name)) }.ok()?;
        Self::from_handle(handle)
    }

    /// The module whose image contains `address`.
    pub fn containing(address: usize) -> Option<Self> {
        let mut handle = HMODULE::default();
        let flags = GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS | GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT;
        // SAFETY: with FROM_ADDRESS the name argument is an address to look up, not a string.
        unsafe { GetModuleHandleExW(flags, PCWSTR(address as *const u16), &mut handle) }.ok()?;
        Self::from_handle(handle)
    }

    /// Where `address` is, for logs: `file.dll+0x1234`, or the bare address outside any module.
    pub fn describe(address: usize) -> String {
        match Self::containing(address) {
            Some(module) => {
                let name = module.path().and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned())).unwrap_or_else(|| "?".into());
                format!("{name}+{:#x}", address - module.base())
            }
            None => format!("{address:#x} (no module)"),
        }
    }

    /// Every module loaded in this process now.
    pub fn loaded() -> Vec<Self> {
        let mut out = Vec::new();
        // SAFETY: a snapshot of our own process's modules, walked and closed here.
        unsafe {
            let Ok(snapshot) = CreateToolhelp32Snapshot(TH32CS_SNAPMODULE, 0) else { return out };
            let mut entry = MODULEENTRY32W { dwSize: size_of::<MODULEENTRY32W>() as u32, ..Default::default() };
            let mut more = Module32FirstW(snapshot, &mut entry).is_ok();
            while more {
                if let Some(module) = Self::from_handle(entry.hModule) {
                    out.push(module);
                }
                more = Module32NextW(snapshot, &mut entry).is_ok();
            }
            let _ = CloseHandle(snapshot);
        }
        out
    }

    /// The names of the DLLs this module imports from (its import table).
    pub fn imported_dlls(&self) -> Vec<String> {
        let mut out = Vec::new();
        let base = self.base;
        let Some(nt) = mem::read::<u32>(base + 0x3c).map(|n| base + n as usize) else { return out };
        let optional = nt + 24;
        let directories = optional + crate::arch::DIRECTORIES;
        if mem::read::<u32>(directories - 4).is_none_or(|n| n < 2) {
            return out;
        }
        let Some(imports) = mem::read::<u32>(directories + 8).filter(|&i| i != 0) else { return out };
        for descriptor in (0..4096).map(|i| base + imports as usize + 20 * i) {
            let (Some(library), Some(thunks)) = (mem::read::<u32>(descriptor + 12), mem::read::<u32>(descriptor + 16)) else { break };
            if library == 0 && thunks == 0 {
                break;
            }
            if let Some(name) = mem::read_c_string(base + library as usize, 260) {
                out.push(name);
            }
        }
        out
    }

    fn from_handle(handle: HMODULE) -> Option<Self> {
        let base = handle.0 as usize;
        let nt = mem::read::<u32>(base + 0x3c)? as usize;
        let size = mem::read::<u32>(base + nt + 24 + 56)? as usize;
        (base != 0 && size != 0).then_some(Self { base, size })
    }

    pub fn base(&self) -> usize {
        self.base
    }

    pub fn size(&self) -> usize {
        self.size
    }

    /// The address of an RVA inside this module.
    pub fn at(&self, rva: usize) -> usize {
        self.base + rva
    }

    pub fn contains(&self, address: usize) -> bool {
        address >= self.base && address - self.base < self.size
    }

    /// The address of export number `ordinal` (an export without a name, or looked up as callers
    /// do: SDL takes XInput's `XInputGetStateEx` as #100).
    pub fn export_ordinal(&self, ordinal: u16) -> Option<usize> {
        // SAFETY: an ordinal lookup is a "name" pointer whose high bits are zero.
        let function = unsafe { GetProcAddress(HMODULE(self.base as *mut _), PCSTR(ordinal as usize as *const u8)) }?;
        Some(function as usize)
    }

    pub fn export(&self, name: &str) -> Option<usize> {
        let name = CString::new(name).ok()?;
        // SAFETY: looks up a name in a loaded module's export table.
        let function = unsafe { GetProcAddress(HMODULE(self.base as *mut _), PCSTR(name.as_ptr().cast())) }?;
        Some(function as usize)
    }

    /// The import address table entry through which this module calls `function` of `dll`
    /// (matched without regard to case; imports by ordinal are not considered).
    pub fn import_slot(&self, dll: &str, function: &str) -> Option<usize> {
        self.find_import(dll, |entry| entry >> ORDINAL_FLAG == 0 && mem::read_c_string(self.base + (entry & 0x7fff_ffff) + 2, 512).is_some_and(|n| n == function))
    }

    /// The import-table slot through which this module calls `dll`'s export number `ordinal`
    /// (imported by number, not by name; XInput's are).
    pub fn import_ordinal_slot(&self, dll: &str, ordinal: u16) -> Option<usize> {
        self.find_import(dll, |entry| entry >> ORDINAL_FLAG == 1 && entry & 0xffff == ordinal as usize)
    }

    /// The slot of the first import from `dll` whose lookup entry `matches` (high bit: by
    /// ordinal, low 16 bits the ordinal; otherwise the RVA of a hint and the name).
    fn find_import(&self, dll: &str, matches: impl Fn(usize) -> bool) -> Option<usize> {
        let base = self.base;
        let nt = base + mem::read::<u32>(base + 0x3c)? as usize;
        let directories = nt + 24 + crate::arch::DIRECTORIES;
        if mem::read::<u32>(directories - 4)? < 2 {
            return None;
        }
        let imports = mem::read::<u32>(directories + 8)? as usize;
        if imports == 0 {
            return None;
        }
        for descriptor in (0..4096).map(|i| base + imports + 20 * i) {
            let names = mem::read::<u32>(descriptor)? as usize;
            let library = mem::read::<u32>(descriptor + 12)? as usize;
            let thunks = mem::read::<u32>(descriptor + 16)? as usize;
            if library == 0 && thunks == 0 {
                return None;
            }
            if !mem::read_c_string(base + library, 260).is_some_and(|n| n.eq_ignore_ascii_case(dll)) {
                continue;
            }
            let lookup = if names != 0 { names } else { thunks };
            // Entries are pointer-sized (8 bytes in a 64-bit image, 4 in a 32-bit one).
            let width = size_of::<usize>();
            for i in 0..65536 {
                let entry = mem::read::<usize>(base + lookup + width * i)?;
                if entry == 0 {
                    break;
                }
                if matches(entry) {
                    return Some(base + thunks + width * i);
                }
            }
        }
        None
    }

    /// This module's sections as loaded, for scanning its code ([`monaka_core::scan`]). Every
    /// section is checked committed and readable first; `None` if one is not.
    ///
    /// # Safety
    /// The module must stay loaded while the result is used (a game's own executable does), and
    /// its sections must not be unmapped or made unreadable meanwhile.
    pub unsafe fn code(&self) -> Option<Code<'static>> {
        let mut headers = vec![0u8; 0x1000];
        if !mem::read_bytes(self.base, &mut headers) {
            return None;
        }
        let (table, _, exceptions) = section_headers(&headers).ok()?;
        let mut sections = Vec::with_capacity(table.len());
        for header in table.into_iter().filter(|h| h.virtual_size > 0) {
            let start = self.base + header.rva as usize;
            let length = header.virtual_size as usize;
            if !self.contains(start) || !self.contains(start + length - 1) || !readable(start, length) {
                return None;
            }
            // SAFETY: the range lies inside the loaded image and was just checked committed and
            // readable; the caller keeps the module loaded while the slice lives.
            let bytes = unsafe { std::slice::from_raw_parts(start as *const u8, length) };
            sections.push(Section { name: header.name, rva: header.rva, bytes, executable: header.executable });
        }
        Code::new(self.base as u64, sections, exceptions).ok()
    }

    /// This module's code ([`Module::code`]), read once and kept for the process's life: for a
    /// module that stays loaded (the game's executable, the system's DLLs), as research tools ask
    /// of it again and again.
    pub fn kept_code(&self) -> Option<&'static Code<'static>> {
        static KEPT: std::sync::Mutex<Vec<(usize, &'static Code<'static>)>> = std::sync::Mutex::new(Vec::new());
        let mut kept = KEPT.lock().ok()?;
        if let Some((_, code)) = kept.iter().find(|(base, _)| *base == self.base) {
            return Some(code);
        }
        // SAFETY: kept only for modules the caller knows stay loaded (see above).
        let code: &'static Code<'static> = Box::leak(Box::new(unsafe { self.code() }?));
        kept.push((self.base, code));
        Some(code)
    }

    /// Whether the code at `rva` is exactly `expected`.
    pub fn bytes_match(&self, rva: usize, expected: &[u8]) -> bool {
        let mut actual = vec![0u8; expected.len()];
        mem::read_bytes(self.at(rva), &mut actual) && actual == expected
    }

    pub fn path(&self) -> Option<PathBuf> {
        let mut buffer = vec![0u16; 32768];
        // SAFETY: fills our buffer with the module's path.
        let length = unsafe { GetModuleFileNameW(Some(HMODULE(self.base as *mut _)), &mut buffer) } as usize;
        (length > 0 && length < buffer.len()).then(|| PathBuf::from(String::from_utf16_lossy(&buffer[..length])))
    }

    /// Upper-case hex SHA-256 of the module's file on disk.
    pub fn file_sha256(&self) -> Option<String> {
        let mut file = std::fs::File::open(self.path()?).ok()?;
        let mut hash = Sha256::default();
        let mut buffer = vec![0u8; 1 << 20];
        loop {
            let read = file.read(&mut buffer).ok()?;
            if read == 0 {
                break;
            }
            hash.update(&buffer[..read]);
        }
        Some(hex(&hash.finish()))
    }
}

/// Whether every page of `[start, start + length)` is committed, readable and not a guard page.
fn readable(start: usize, length: usize) -> bool {
    let end = start + length;
    let mut at = start;
    while at < end {
        let mut region = MEMORY_BASIC_INFORMATION::default();
        // SAFETY: queries an address; no memory is touched.
        if unsafe { VirtualQuery(Some(at as *const _), &mut region, size_of::<MEMORY_BASIC_INFORMATION>()) } == 0 {
            return false;
        }
        let protect = region.Protect.0;
        if region.State != MEM_COMMIT || protect & PAGE_NOACCESS.0 != 0 || protect & PAGE_GUARD.0 != 0 {
            return false;
        }
        at = region.BaseAddress as usize + region.RegionSize;
    }
    true
}

/// The bit of an import lookup entry that says it imports by ordinal: the top one.
const ORDINAL_FLAG: u32 = usize::BITS - 1;

/// Whether `address` looks like a live COM-style object: its vtable and its first `methods`
/// slots point into loaded modules. Checked before calling into a pointer read from game memory.
pub fn plausible_object(address: usize, methods: usize) -> bool {
    let Some(table) = mem::read::<usize>(address) else { return false };
    Module::containing(table).is_some()
        && (0..methods).all(|i| mem::read::<usize>(table + size_of::<usize>() * i).is_some_and(|method| Module::containing(method).is_some()))
}

/// An object in game memory as it was seen: its address and its vtable then (none for one without a
/// plausible vtable), so a value saved from it is written back only while it is still that object
/// and not another at the same address.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Seen {
    pub address: usize,
    vtable: Option<usize>,
}

impl Seen {
    /// The object at `address` as it is now.
    pub fn now(address: usize) -> Self {
        Self { address, vtable: plausible_object(address, 1).then(|| mem::read::<usize>(address)).flatten() }
    }

    /// Whether it is still the object seen: its vtable unchanged (one without a vtable is taken to be).
    pub fn still_there(&self) -> bool {
        self.vtable.is_none_or(|v| mem::read::<usize>(self.address) == Some(v))
    }
}

/// File name of the process's executable.
pub fn host_name() -> String {
    let mut buffer = vec![0u16; 32768];
    // SAFETY: fills our buffer with the executable's path.
    let length = unsafe { GetModuleFileNameW(None, &mut buffer) } as usize;
    let path = PathBuf::from(String::from_utf16_lossy(&buffer[..length.min(buffer.len())]));
    path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
}

#[cfg(test)]
mod tests;
