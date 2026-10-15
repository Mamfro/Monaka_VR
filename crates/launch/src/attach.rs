//! Attaching a producer DLL to a running game and calling its `MonakaStart` or `MonakaStop` (what
//! `monaka_loader` does, as a library for the launcher).
//!
//! A producer is a DLL named `monaka_NAME.dll` or `monaka_NAME-VERSION.dll`; it names the game
//! executable it belongs to in its exported `MONAKA_HOST` (see `monaka_producer::producer!`; several,
//! `;`-separated, for one engine's games), so a new game needs nothing here, and the pack contract it
//! speaks in `MONAKA_CONTRACT`; a DLL without either is refused. Before a start, any other producer
//! copy still running in the game is stopped (it answers `MonakaStatus`), so two versions never hook
//! at once. A copy is never unloaded: it stays loaded and inert after its stop.

use monaka_core::pe::{self, Image};
use monaka_core::status::{State, code};
use std::path::{Path, PathBuf};
use windows::Win32::Foundation::{CloseHandle, HANDLE, HMODULE, WAIT_ABANDONED, WAIT_OBJECT_0};
use windows::Win32::System::Diagnostics::Debug::{ReadProcessMemory, WriteProcessMemory};
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, MODULEENTRY32W, Module32FirstW, Module32NextW, TH32CS_SNAPMODULE, TH32CS_SNAPMODULE32,
};
use windows::Win32::System::LibraryLoader::{
    GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS, GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT, GetModuleFileNameW, GetModuleHandleExW,
    GetModuleHandleW, GetProcAddress,
};
use windows::Win32::System::Memory::{MEM_COMMIT, MEM_RELEASE, MEM_RESERVE, PAGE_READWRITE, VirtualAllocEx, VirtualFreeEx};
use windows::Win32::System::Threading::{
    CreateMutexW, CreateRemoteThread, GetExitCodeThread, IsWow64Process, OpenProcess, PROCESS_CREATE_THREAD, PROCESS_NAME_WIN32,
    PROCESS_QUERY_INFORMATION, PROCESS_VM_OPERATION, PROCESS_VM_READ, PROCESS_VM_WRITE, QueryFullProcessImageNameW, ReleaseMutex,
    WaitForSingleObject,
};
use windows::core::{BOOL, HSTRING, PCWSTR, PWSTR, s, w};

/// The producer name of a DLL file name (`monaka_NAME` of `monaka_NAME[-VERSION].dll`), if it is one.
fn producer_name(file_name: &str) -> Option<String> {
    let lower = file_name.to_ascii_lowercase();
    let stem = lower.strip_suffix(".dll")?;
    let (name, version) = stem.split_once('-').unwrap_or((stem, ""));
    let named = name.strip_prefix("monaka_").is_some_and(|n| !n.is_empty() && n.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'));
    let versioned = !stem.contains('-') || (!version.is_empty() && version.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'));
    (named && versioned).then(|| name.to_owned())
}

/// The game executable a producer DLL belongs to (its `MONAKA_HOST`). A DLL that does not say which
/// pack contract it speaks (`MONAKA_CONTRACT`), or speaks one this Monaka does not run (another
/// major version, or a newer minor one), is refused: its options, channels or entry points may not
/// be what this Monaka expects.
fn producer_host(file_name: &str, bytes: &[u8]) -> Result<String, Failure> {
    producer_name(file_name).ok_or("expected a producer DLL named monaka_NAME[-VERSION].dll")?;
    let packed = pe::file_export_u32(bytes, "MONAKA_CONTRACT")
        .map_err(|e| format!("{file_name}: {e}"))?
        .ok_or_else(|| format!("{file_name} is not a game pack for this Monaka (it has no MONAKA_CONTRACT export)"))?;
    let contract = monaka_core::pack::Contract::from_packed(packed);
    monaka_core::pack::CONTRACT.runs(contract).map_err(|why| format!("{file_name} cannot run: {why}"))?;
    pe::file_export_text(bytes, "MONAKA_HOST")
        .map_err(|e| format!("{file_name}: {e}"))?
        .ok_or_else(|| format!("{file_name} does not name its game (no MONAKA_HOST export)"))
}

/// What to do with another producer copy loaded in the game, from its `MonakaStatus`: stop it
/// (running), leave it (never started, stopped or refused), or attach nothing (anything else: it
/// is between states, failed its shutdown, or answers a value this Monaka does not know).
fn must_stop(name: &str, status: u32) -> Result<bool, Failure> {
    match State::from_u32(status) {
        Some(State::Running) => Ok(true),
        Some(State::New | State::Stopped | State::Rejected) => Ok(false),
        Some(state) => Err(format!("the producer {name} already in the game is {state:?}; nothing new attached (restart the game)")),
        None => Err(format!("the producer {name} already in the game reports an unknown state {status}; nothing new attached (restart the game)")),
    }
}

/// How often a module list is asked for before giving up: Toolhelp fails now and then while the
/// game is loading or unloading modules.
const SNAPSHOT_TRIES: u32 = 5;

type Failure = String;

struct Handle(HANDLE);

impl Drop for Handle {
    fn drop(&mut self) {
        // SAFETY: closes a handle this value owns.
        let _ = unsafe { CloseHandle(self.0) };
    }
}

fn wide(buffer: &[u16]) -> String {
    let end = buffer.iter().position(|&c| c == 0).unwrap_or(buffer.len());
    String::from_utf16_lossy(&buffer[..end])
}

struct RemoteModule {
    name: String,
    path: String,
    base: usize,
    size: u32,
}

fn snapshot(pid: u32) -> Result<Handle, Failure> {
    let mut tries = 0;
    loop {
        tries += 1;
        // SAFETY: a Toolhelp snapshot this function hands over to its owner.
        match unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPMODULE | TH32CS_SNAPMODULE32, pid) } {
            Ok(handle) => return Ok(Handle(handle)),
            Err(e) if tries >= SNAPSHOT_TRIES => return Err(format!("cannot enumerate modules: {e}")),
            Err(_) => std::thread::sleep(std::time::Duration::from_millis(20)),
        }
    }
}

fn modules(pid: u32) -> Result<Vec<RemoteModule>, Failure> {
    let snapshot = snapshot(pid)?;
    // SAFETY: Toolhelp enumeration into a local entry.
    unsafe {
        let mut entry = MODULEENTRY32W { dwSize: size_of::<MODULEENTRY32W>() as u32, ..Default::default() };
        let mut found = Vec::new();
        let mut more = Module32FirstW(snapshot.0, &mut entry).is_ok();
        while more {
            found.push(RemoteModule {
                name: wide(&entry.szModule),
                path: wide(&entry.szExePath),
                base: entry.modBaseAddr as usize,
                size: entry.modBaseSize,
            });
            more = Module32NextW(snapshot.0, &mut entry).is_ok();
        }
        Ok(found)
    }
}

fn find_module(pid: u32, name: &str) -> Result<Option<RemoteModule>, Failure> {
    Ok(modules(pid)?.into_iter().find(|m| m.name.eq_ignore_ascii_case(name)))
}

/// Whether process `pid` has come far enough to be attached: its kernel32 is mapped (a process
/// just made holds only its executable and ntdll, and its module list may not be readable yet).
pub fn ready(pid: u32) -> bool {
    matches!(find_module(pid, "kernel32.dll"), Ok(Some(_)))
}

/// A module mapped in the game, read by RVA.
struct RemoteImage<'a> {
    process: &'a Handle,
    base: usize,
    size: u32,
}

impl Image for RemoteImage<'_> {
    fn read(&self, rva: u32, buffer: &mut [u8]) -> bool {
        if rva as u64 + buffer.len() as u64 > self.size as u64 {
            return false;
        }
        let mut done = 0usize;
        // SAFETY: reads another process's memory into our buffer; the kernel checks the range.
        let ok = unsafe {
            ReadProcessMemory(self.process.0, (self.base + rva as usize) as *const _, buffer.as_mut_ptr().cast(), buffer.len(), Some(&mut done))
        };
        ok.is_ok() && done == buffer.len()
    }
}

fn loaded_export(process: &Handle, module: &RemoteModule, name: &str) -> Result<Option<u32>, Failure> {
    pe::mapped_export(&RemoteImage { process, base: module.base, size: module.size }, module.size, name)
        .map_err(|e| format!("{}: {e}", module.name))
}

/// Runs `function(argument)` on a new thread in the game and returns its exit code.
fn call(process: &Handle, function: usize, argument: usize) -> Result<u32, Failure> {
    // SAFETY: `function` is an exported thread-start-compatible function of a module in the game.
    unsafe {
        let start: unsafe extern "system" fn(*mut core::ffi::c_void) -> u32 = std::mem::transmute(function);
        let thread = Handle(
            CreateRemoteThread(process.0, None, 0, Some(start), Some(argument as *const _), 0, None)
                .map_err(|e| format!("CreateRemoteThread failed: {e}"))?,
        );
        if WaitForSingleObject(thread.0, 45000) != WAIT_OBJECT_0 {
            return Err("remote operation unfinished; do not retry or unload the DLL".into());
        }
        let mut code = 0u32;
        GetExitCodeThread(thread.0, &mut code).map_err(|e| format!("cannot read the remote result: {e}"))?;
        Ok(code)
    }
}

/// Stops every running producer copy in the game before a new one starts.
fn stop_others(process: &Handle, pid: u32, notes: &mut Vec<String>) -> Result<(), Failure> {
    for module in modules(pid)? {
        if producer_name(&module.name).is_none() {
            continue;
        }
        let status = loaded_export(process, &module, "MonakaStatus")?;
        let stop = loaded_export(process, &module, "MonakaStop")?;
        let (status, stop) = match (status, stop) {
            (None, None) => continue, // named like one, but not a producer
            (Some(status), Some(stop)) => (status, stop),
            _ => {
                return Err(format!(
                    "the producer {} already in the game cannot be stopped safely (it does not answer MonakaStatus and MonakaStop); nothing new attached (restart the game)",
                    module.name
                ));
            }
        };
        if !must_stop(&module.name, call(process, module.base + status as usize, 0)?)? {
            continue;
        }
        let result = call(process, module.base + stop as usize, 0)?;
        if result != code::SUCCESS {
            return Err(format!("could not stop the previous producer {} (code {result}); nothing new attached", module.name));
        }
        notes.push(format!("Previous producer inactive: {}", module.name));
    }
    Ok(())
}

/// A canonical path without the `\\?\` prefix, so it compares with the paths Toolhelp reports.
fn canonical(path: &Path) -> Result<PathBuf, Failure> {
    let full = std::fs::canonicalize(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let text = full.to_string_lossy();
    Ok(PathBuf::from(text.strip_prefix(r"\\?\").unwrap_or(&text)))
}

/// The game-side address of `LoadLibraryW` (kernel32 sits at a per-boot address, but compute it
/// from the game's own module list rather than assume). A 32-bit game (`wow64`) has its own
/// 32-bit kernel32, whose export is read from the game's memory.
fn remote_load_library(process: &Handle, pid: u32, wow64: bool) -> Result<usize, Failure> {
    if wow64 {
        let kernel = find_module(pid, "kernel32.dll")?.ok_or("the game has no kernel32.dll")?;
        let rva = loaded_export(process, &kernel, "LoadLibraryW")?.ok_or("the game's kernel32 has no LoadLibraryW")?;
        return Ok(kernel.base + rva as usize);
    }
    // SAFETY: local module lookups.
    unsafe {
        let kernel = GetModuleHandleW(w!("kernel32.dll")).map_err(|e| format!("kernel32: {e}"))?;
        let load = GetProcAddress(kernel, s!("LoadLibraryW")).ok_or("LoadLibraryW missing")? as usize;
        let mut owner = HMODULE::default();
        let flags = GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS | GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT;
        GetModuleHandleExW(flags, PCWSTR(load as *const u16), &mut owner).map_err(|e| format!("LoadLibraryW owner: {e}"))?;
        let mut path = vec![0u16; 32768];
        let length = GetModuleFileNameW(Some(owner), &mut path) as usize;
        let owner_name = Path::new(&String::from_utf16_lossy(&path[..length])).file_name().map(|n| n.to_string_lossy().into_owned());
        let owner_name = owner_name.ok_or("LoadLibraryW owner has no name")?;
        let remote = find_module(pid, &owner_name)?.ok_or("the loader's kernel module is missing from the game")?;
        Ok(remote.base + (load - owner.0 as usize))
    }
}

fn inject(process: &Handle, pid: u32, path: &Path, wow64: bool) -> Result<(), Failure> {
    let text: Vec<u16> = path.as_os_str().to_string_lossy().encode_utf16().chain(Some(0)).collect();
    let bytes = text.len() * 2;
    let load = remote_load_library(process, pid, wow64)?;
    // SAFETY: allocates, fills and frees a buffer in the game for LoadLibraryW's argument.
    unsafe {
        let remote = VirtualAllocEx(process.0, None, bytes, MEM_COMMIT | MEM_RESERVE, PAGE_READWRITE);
        if remote.is_null() {
            return Err("cannot allocate the loader argument".into());
        }
        let mut written = 0usize;
        if WriteProcessMemory(process.0, remote, text.as_ptr().cast(), bytes, Some(&mut written)).is_err() || written != bytes {
            let _ = VirtualFreeEx(process.0, remote, 0, MEM_RELEASE);
            return Err("cannot write the loader argument".into());
        }
        // On a timeout the argument stays allocated: the remote thread may still read it.
        call(process, load, remote as usize)?;
        let _ = VirtualFreeEx(process.0, remote, 0, MEM_RELEASE);
    }
    Ok(())
}

fn run(start: bool, pid: u32, dll: &Path, notes: &mut Vec<String>) -> Result<u32, Failure> {
    let path = canonical(dll)?;
    let file_name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    producer_name(&file_name).ok_or("expected a producer DLL named monaka_NAME[-VERSION].dll")?;
    let bytes = std::fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    let host = producer_host(&file_name, &bytes)?;
    let entry = if start { "MonakaStart" } else { "MonakaStop" };
    let rva = pe::file_export(&bytes, entry).map_err(|e| e.to_string())?.ok_or(format!("{file_name} has no {entry}"))?;

    let access = PROCESS_CREATE_THREAD | PROCESS_QUERY_INFORMATION | PROCESS_VM_OPERATION | PROCESS_VM_WRITE | PROCESS_VM_READ;
    // SAFETY: opens the game with the rights the loader uses.
    let process = Handle(unsafe { OpenProcess(access, false, pid) }.map_err(|e| format!("cannot open the game process: {e}"))?);
    let mut image = vec![0u16; 32768];
    let mut length = image.len() as u32;
    // SAFETY: fills our buffer with the game's executable path.
    unsafe { QueryFullProcessImageNameW(process.0, PROCESS_NAME_WIN32, PWSTR(image.as_mut_ptr()), &mut length) }
        .map_err(|e| format!("cannot read the game's path: {e}"))?;
    let executable = String::from_utf16_lossy(&image[..length as usize]);
    let executable = Path::new(&executable).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    if !pe::host_listed(&host, &executable) {
        return Err(format!("{file_name} is for {host}, but process {pid} is {executable}"));
    }
    let mut wow64 = BOOL(0);
    // SAFETY: fills a local.
    unsafe { IsWow64Process(process.0, &mut wow64) }.map_err(|e| format!("cannot tell the game's bitness: {e}"))?;
    // A 32-bit game (running under WOW64) takes a 32-bit producer, a 64-bit game a 64-bit one.
    let wow64 = wow64.as_bool();
    if pe::file_is_x86(&bytes).map_err(|e| e.to_string())? != wow64 {
        let bits = |x86: bool| if x86 { "32-bit" } else { "64-bit" };
        return Err(format!("{file_name} is {}, but the game is a {} process", bits(!wow64), bits(wow64)));
    }

    // One loader operation per game at a time.
    // SAFETY: a named mutex owned by this function and released on every path below.
    let lock = Handle(
        unsafe { CreateMutexW(None, false, &HSTRING::from(format!(r"Local\monaka-loader-{pid}"))) }
            .map_err(|e| format!("cannot create the loader lock: {e}"))?,
    );
    // SAFETY: waits on our own mutex handle.
    let wait = unsafe { WaitForSingleObject(lock.0, 5000) };
    if wait != WAIT_OBJECT_0 && wait != WAIT_ABANDONED {
        return Err("another loader operation is in progress".into());
    }
    struct Release<'a>(&'a Handle);
    impl Drop for Release<'_> {
        fn drop(&mut self) {
            // SAFETY: releases the mutex this thread acquired above.
            let _ = unsafe { ReleaseMutex(self.0.0) };
        }
    }
    let _release = Release(&lock);

    if start {
        stop_others(&process, pid, notes)?;
    }
    let mut module = find_module(pid, &file_name)?;
    if let Some(existing) = &module
        && !existing.path.eq_ignore_ascii_case(&path.to_string_lossy())
    {
        return Err(format!("a different {file_name} is already loaded from {}", existing.path));
    }
    if module.is_none() {
        if !start {
            return Err(format!("{file_name} is not loaded"));
        }
        inject(&process, pid, &path, wow64)?;
        module = find_module(pid, &file_name)?;
    }
    // LoadLibraryW's thread exit code holds only the low half of a 64-bit module handle, so the
    // module list is what tells whether the load worked.
    let module = module.ok_or_else(|| format!("the game could not load {file_name}: a DLL it needs may be missing, or the game refused it"))?;
    call(&process, module.base + rva as usize, 0)
}

/// What an attach operation returned: the producer's own result (0 is success) and what was done
/// on the way (older copies stopped).
pub struct Outcome {
    pub code: u32,
    pub notes: Vec<String>,
}

/// Attaches `dll` to the game `pid` (loading it if needed, after stopping any other producer copy)
/// and calls its `MonakaStart`. Blocks for up to about 45 seconds per remote call (a producer
/// start hashes the game DLL, which takes 20 seconds when the file is not cached).
pub fn start(pid: u32, dll: &Path) -> Result<Outcome, String> {
    let mut notes = Vec::new();
    run(true, pid, dll, &mut notes).map(|code| Outcome { code, notes })
}

/// Calls `MonakaStop` of `dll`, already loaded in the game `pid`.
pub fn stop(pid: u32, dll: &Path) -> Result<Outcome, String> {
    let mut notes = Vec::new();
    run(false, pid, dll, &mut notes).map(|code| Outcome { code, notes })
}

#[cfg(test)]
mod tests;
