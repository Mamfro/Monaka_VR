//! Hooks. An inline hook replaces the first instructions of a function with a jump to a relay
//! near it, which jumps to the detour; the replaced instructions run from a trampoline the detour
//! calls as the original. The instructions must be described exactly ([`Instruction`]) and are
//! checked byte for byte before anything is written, so a different game build is refused
//! instead of corrupted. A vtable hook swaps one method pointer in an object's vtable; an import
//! hook swaps one entry of a module's import address table.
//!
//! Relays and trampolines are never freed: a thread may still be inside one after a hook is
//! removed. Many hooks share one region of executable memory near their code ([`slot_near`]).

use crate::module::Module;
use crate::threads::Frozen;
use crate::{Error, Result, mem};
use monaka_core::sync::Locked;
use std::marker::PhantomData;
use std::ops::Range;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use windows::Win32::System::Diagnostics::Debug::FlushInstructionCache;
use windows::Win32::System::Memory::{
    MEM_COMMIT, MEM_FREE, MEM_RESERVE, MEMORY_BASIC_INFORMATION, PAGE_EXECUTE_READWRITE, PAGE_PROTECTION_FLAGS, PAGE_READWRITE,
    VirtualAlloc, VirtualProtect, VirtualQuery,
};
use windows::Win32::System::SystemInformation::{GetSystemInfo, SYSTEM_INFO};
use windows::Win32::System::Threading::GetCurrentProcess;

/// A function in a game module where a hook goes: its name (for the log and errors), its offset in
/// the module or the name it is exported by, its first instructions exactly as they must appear,
/// and its type `F` (a function pointer type). A build table declares its sites; [`Hooks::site`]
/// hooks one.
pub struct Site<F> {
    pub name: &'static str,
    pub rva: usize,
    /// Found by this exported name instead of an offset.
    pub export: Option<&'static str>,
    pub prologue: &'static [Instruction],
    function: PhantomData<F>,
}

impl<F> Clone for Site<F> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<F> Copy for Site<F> {}

impl<F> std::fmt::Debug for Site<F> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.export {
            Some(export) => write!(f, "{} ({export})", self.name),
            None => write!(f, "{} (+{:#x})", self.name, self.rva),
        }
    }
}

impl<F> Site<F> {
    /// # Safety
    /// At `rva`, in the module the site is used with, starts a function of type `F` (a function
    /// pointer type), and `prologue` is its first instructions with every rip-relative
    /// displacement marked. A build table makes this claim once for all its sites, valid for the
    /// build its fingerprint checks (the bytes are checked again before anything is written).
    pub const unsafe fn new(name: &'static str, rva: usize, prologue: &'static [Instruction]) -> Self {
        Self { name, rva, export: None, prologue, function: PhantomData }
    }

    /// A site found by the name `export` its module exports it by.
    ///
    /// # Safety
    /// As [`Site::new`], for the function `export` names.
    pub const unsafe fn exported(name: &'static str, export: &'static str, prologue: &'static [Instruction]) -> Self {
        Self { name, rva: 0, export: Some(export), prologue, function: PhantomData }
    }

    /// Where the site is in `module`.
    pub fn address(&self, module: &Module) -> Result<usize> {
        match self.export {
            Some(export) => module.export(export).ok_or_else(|| Error::Refused(format!("{}: {export} is not exported", self.name))),
            None => Ok(module.at(self.rva)),
        }
    }

    /// The site's function in `module`, to call: every instruction the site describes checked
    /// first, so the call goes to the code the build table measured.
    pub fn function(&self, module: &Module) -> Result<F>
    where
        F: Copy,
    {
        assert!(size_of::<F>() == size_of::<usize>(), "{}: a site's type is a function pointer", self.name);
        let address = self.address(module)?;
        let expected: Vec<u8> = self.prologue.iter().flat_map(|i| i.bytes.iter().copied()).collect();
        let mut actual = vec![0u8; expected.len()];
        if expected.is_empty() || !mem::read_bytes(address, &mut actual) || actual != expected {
            return Err(Error::Signature(self.name.to_owned()));
        }
        // SAFETY: the site's declaration ([`Site::new`]) is the claim that a function of type `F`
        // starts here in this module, and its code was just checked; `F` is pointer-sized.
        Ok(unsafe { std::mem::transmute_copy::<usize, F>(&address) })
    }
}

/// One instruction of a hooked prologue, exactly as it must appear in the game.
#[derive(Clone, Copy, Debug)]
pub struct Instruction {
    bytes: &'static [u8],
    /// Offset of a 32-bit displacement relative to the end of the instruction, if any.
    rip_displacement: Option<usize>,
}

impl Instruction {
    /// An instruction that runs the same anywhere.
    pub const fn plain(bytes: &'static [u8]) -> Self {
        Self { bytes, rip_displacement: None }
    }

    /// An instruction with a rip-relative 32-bit displacement at byte `offset`. The displacement
    /// bytes are part of the signature; the trampoline gets a corrected copy.
    pub const fn rip_relative(bytes: &'static [u8], offset: usize) -> Self {
        Self { bytes, rip_displacement: Some(offset) }
    }
}

use crate::arch::{RELAY_SIZE, absolute_jump, relative32};
const TRAMPOLINE_OFFSET: usize = 32;
/// One hook's relay and trampoline: the relay, then at [`TRAMPOLINE_OFFSET`] the replaced
/// instructions (at most a jump's worth plus one, 20 bytes) and the jump back.
const SLOT: usize = 128;
/// The executable memory reserved at once: the allocation granularity, so nothing is left over.
const REGION: usize = 0x10000;
/// How far from its code a hook's slot may be (a rel32 jump reaches 2 GiB).
const REACH: usize = 0x7000_0000;

/// A region of executable memory and how much of it is handed out.
struct Region {
    base: usize,
    used: usize,
}

static REGIONS: Mutex<Vec<Region>> = Mutex::new(Vec::new());

/// A fresh [`SLOT`] of executable memory within a rel32 jump of `target`, from a region already
/// near it, or a new one. Never handed out again.
fn slot_near(target: usize) -> Option<usize> {
    let mut regions = REGIONS.locked();
    if let Some(region) = regions.iter_mut().find(|r| r.used + SLOT <= REGION && r.base.abs_diff(target) < REACH - REGION) {
        let slot = region.base + region.used;
        region.used += SLOT;
        return Some(slot);
    }
    let base = allocate_near(target)?;
    regions.push(Region { base, used: SLOT });
    Some(base)
}

/// Commits a [`REGION`] of executable memory within a rel32 jump of `target`.
fn allocate_near(target: usize) -> Option<usize> {
    let mut info = SYSTEM_INFO::default();
    // SAFETY: fills a local.
    unsafe { GetSystemInfo(&mut info) };
    let granularity = info.dwAllocationGranularity.max(0x10000) as usize;
    let reach = REACH;
    let origin = target & !(granularity - 1);
    for step in 1..(reach / granularity) {
        for candidate in [origin.checked_sub(step * granularity), origin.checked_add(step * granularity)].into_iter().flatten() {
            let mut region = MEMORY_BASIC_INFORMATION::default();
            // SAFETY: queries an address; no memory is touched.
            let queried = unsafe { VirtualQuery(Some(candidate as *const _), &mut region, size_of::<MEMORY_BASIC_INFORMATION>()) };
            if queried == 0 || region.State != MEM_FREE {
                continue;
            }
            // SAFETY: reserves and commits fresh pages at a free address.
            let block = unsafe { VirtualAlloc(Some(candidate as *const _), REGION, MEM_RESERVE | MEM_COMMIT, PAGE_EXECUTE_READWRITE) };
            if !block.is_null() {
                return Some(block as usize);
            }
        }
    }
    None
}

/// Writes code over memory that may be read-only, then restores its protection. Allocates
/// nothing (it runs while other threads are frozen).
///
/// # Safety
/// `address..address+bytes.len()` must be memory this process may change, and no thread may be
/// executing the bytes being replaced.
unsafe fn overwrite(address: usize, bytes: &[u8]) -> windows::core::Result<()> {
    let mut previous = PAGE_PROTECTION_FLAGS::default();
    // SAFETY: the caller guarantees the range; protection is restored right after the copy.
    unsafe {
        VirtualProtect(address as *const _, bytes.len(), PAGE_EXECUTE_READWRITE, &mut previous)?;
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), address as *mut u8, bytes.len());
        let mut ignored = PAGE_PROTECTION_FLAGS::default();
        let _ = VirtualProtect(address as *const _, bytes.len(), previous, &mut ignored);
        let _ = FlushInstructionCache(GetCurrentProcess(), Some(address as *const _), bytes.len());
    }
    Ok(())
}

struct InlineHook {
    name: String,
    target: usize,
    original: Vec<u8>,
    patch: Vec<u8>,
    enabled: bool,
}

impl InlineHook {
    /// Checks the target against an exactly described prologue, then builds the hook.
    fn described(name: &str, target: usize, prologue: &[Instruction], detour: usize) -> Result<(Self, usize)> {
        let mut length = 0;
        let mut used = 0;
        while length < 5 {
            let Some(instruction) = prologue.get(used) else {
                return Err(Error::Refused(format!("{name}: the described prologue is shorter than a jump")));
            };
            length += instruction.bytes.len();
            used += 1;
        }
        let expected: Vec<u8> = prologue[..used].iter().flat_map(|i| i.bytes.iter().copied()).collect();
        let mut actual = vec![0u8; length];
        if !mem::read_bytes(target, &mut actual) || actual != expected {
            return Err(Error::Signature(name.to_owned()));
        }
        let parts: Vec<(&[u8], Option<usize>)> = prologue[..used].iter().map(|i| (i.bytes, i.rip_displacement)).collect();
        Self::prepare(name, target, &parts, detour)
    }

    /// Decodes the target's own first instructions ([`crate::decode`]), then builds the hook.
    fn decoded(name: &str, target: usize, detour: usize) -> Result<(Self, usize)> {
        let mut bytes = [0u8; 32];
        if !mem::read_bytes(target, &mut bytes) {
            return Err(Error::Refused(format!("{name}: code unreadable")));
        }
        let decoded = crate::decode::prologue(&bytes, 5).ok_or_else(|| Error::Signature(format!("{name} (prologue not decodable)")))?;
        let mut at = 0;
        let parts: Vec<(&[u8], Option<usize>)> = decoded
            .iter()
            .map(|d| {
                let part = (&bytes[at..at + d.length], d.rip_displacement);
                at += d.length;
                part
            })
            .collect();
        Self::prepare(name, target, &parts, detour)
    }

    /// Builds the relay and trampoline for instructions known to be the target's first ones;
    /// writes nothing at the target.
    fn prepare(name: &str, target: usize, parts: &[(&[u8], Option<usize>)], detour: usize) -> Result<(Self, usize)> {
        let actual: Vec<u8> = parts.iter().flat_map(|(bytes, _)| bytes.iter().copied()).collect();
        let length = actual.len();
        let block = slot_near(target).ok_or_else(|| Error::NoNearMemory(name.to_owned()))?;

        let mut trampoline = Vec::with_capacity(length + RELAY_SIZE);
        let mut source = target;
        for &(bytes, rip_displacement) in parts {
            let start = trampoline.len();
            trampoline.extend_from_slice(bytes);
            if let Some(offset) = rip_displacement {
                let at = offset..offset + 4;
                let displacement = i32::from_le_bytes(bytes[at.clone()].try_into().expect("4 bytes")) as isize;
                let absolute = (source + bytes.len()).wrapping_add_signed(displacement);
                let moved = relative32(block + TRAMPOLINE_OFFSET + start + bytes.len(), absolute).ok_or_else(|| Error::NoNearMemory(name.to_owned()))?;
                trampoline[start + at.start..start + at.end].copy_from_slice(&moved.to_le_bytes());
            }
            source += bytes.len();
        }
        trampoline.extend_from_slice(&absolute_jump(target + length));

        let mut patch = vec![0x90u8; length];
        let relative = relative32(target + 5, block).ok_or_else(|| Error::NoNearMemory(name.to_owned()))?;
        patch[0] = 0xe9;
        patch[1..5].copy_from_slice(&relative.to_le_bytes());

        assert!(TRAMPOLINE_OFFSET + trampoline.len() <= SLOT, "{name}: the trampoline is longer than a slot");
        // SAFETY: the block is a fresh RWX slot of SLOT bytes that nothing else uses.
        unsafe {
            std::ptr::copy_nonoverlapping(absolute_jump(detour).as_ptr(), block as *mut u8, RELAY_SIZE);
            std::ptr::copy_nonoverlapping(trampoline.as_ptr(), (block + TRAMPOLINE_OFFSET) as *mut u8, trampoline.len());
            let _ = FlushInstructionCache(GetCurrentProcess(), Some(block as *const _), SLOT);
        }
        let hook = Self { name: name.to_owned(), target, original: actual, patch, enabled: false };
        Ok((hook, block + TRAMPOLINE_OFFSET))
    }

    /// Bytes a thread must not be stopped inside while this hook is written or removed.
    fn unsafe_range(&self) -> Range<usize> {
        self.target + 1..self.target + self.original.len()
    }

    /// # Safety
    /// Other threads must be frozen and outside [`Self::unsafe_range`].
    unsafe fn set(&mut self, enabled: bool) -> windows::core::Result<()> {
        if self.enabled != enabled {
            let bytes = if enabled { &self.patch } else { &self.original };
            // SAFETY: the target is the verified prologue in a loaded module; the caller froze
            // the other threads outside it.
            unsafe { overwrite(self.target, bytes)? };
            self.enabled = enabled;
        }
        Ok(())
    }
}

struct VtableHook {
    name: String,
    slot: usize,
    original: usize,
    detour: usize,
    enabled: bool,
}

impl VtableHook {
    fn set(&mut self, enabled: bool) -> Result<()> {
        if self.enabled == enabled {
            return Ok(());
        }
        let current = mem::read::<usize>(self.slot).ok_or_else(|| Error::Refused(format!("{}: slot unreadable", self.name)))?;
        let (from, to) = if enabled { (self.original, self.detour) } else { (self.detour, self.original) };
        if current != from {
            // Someone else replaced the pointer since: putting ours back or removing theirs would
            // break their chain, so the slot is left alone.
            let place = crate::module::Module::describe;
            return Err(Error::Refused(format!(
                "{}: slot changed by someone else (now {}, was {}, ours {})",
                self.name,
                place(current),
                place(self.original),
                place(self.detour)
            )));
        }
        let mut previous = PAGE_PROTECTION_FLAGS::default();
        // SAFETY: the slot is an aligned function pointer in a loaded module (a vtable or an
        // import table); it is swapped with one atomic store, so a concurrent call sees either.
        unsafe {
            VirtualProtect(self.slot as *const _, 8, PAGE_READWRITE, &mut previous)
                .map_err(|e| Error::Os(format!("{}: unprotect slot", self.name), e))?;
            (*(self.slot as *const AtomicUsize)).store(to, Ordering::SeqCst);
            let mut ignored = PAGE_PROTECTION_FLAGS::default();
            let _ = VirtualProtect(self.slot as *const _, 8, previous, &mut ignored);
        }
        self.enabled = enabled;
        Ok(())
    }
}

/// Where [`Hooks::method`] put its hook.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Placement {
    /// In the method's code.
    Code,
    /// In the vtable slot, because the code's prologue did not decode (why).
    Vtable(String),
}

/// Method indices of the COM interfaces the producers hook (vtable slot numbers).
pub mod slot {
    /// `IDXGISwapChain::Present`.
    pub const SWAPCHAIN_PRESENT: usize = 8;
    /// `ID3D11DeviceContext::DrawIndexed`.
    pub const CONTEXT_DRAW_INDEXED: usize = 12;
    /// `ID3D11DeviceContext::Draw`.
    pub const CONTEXT_DRAW: usize = 13;
    /// `ID3D11DeviceContext::DrawIndexedInstanced`.
    pub const CONTEXT_DRAW_INDEXED_INSTANCED: usize = 20;
    /// `ID3D11DeviceContext::DrawInstanced`.
    pub const CONTEXT_DRAW_INSTANCED: usize = 21;
    /// `ID3D11DeviceContext::OMSetRenderTargets`.
    pub const CONTEXT_OM_SET_RENDER_TARGETS: usize = 33;
}

/// A set of hooks prepared one by one and switched on or off together.
#[derive(Default)]
pub struct Hooks {
    inline: Vec<InlineHook>,
    vtable: Vec<VtableHook>,
}

fn function_address<F: Copy>(function: F) -> usize {
    assert_eq!(size_of::<F>(), size_of::<usize>(), "hooks take function pointers only");
    // SAFETY: F is a pointer-sized function pointer type.
    unsafe { std::mem::transmute_copy(&function) }
}

impl Hooks {
    /// Refuses a second inline hook on `target`: both would be built from the same original bytes,
    /// and the one enabled last would silently replace the other's jump, so one detour never ran.
    fn refuse_twice(&self, name: &str, target: usize) -> Result<()> {
        match self.inline.iter().find(|hook| hook.target == target) {
            Some(first) => Err(Error::Refused(format!("{name}: {target:#x} is already hooked by {}", first.name))),
            None => Ok(()),
        }
    }

    /// Prepares an inline hook at `site` in `module` ([`Hooks::inline`]): safe, the claim that
    /// `site` is a function of type `F` with that prologue being the site's own ([`Site::new`]).
    pub fn site<F: Copy>(&mut self, original: &Original<F>, module: &Module, site: &Site<F>, detour: F) -> Result<()> {
        let target = site.address(module)?;
        // SAFETY: the site's declaration is the claim that a function of type F with this prologue
        // starts there in this module; the prologue is checked before anything is written.
        unsafe { self.inline(original, site.name, target, site.prologue, detour) }
    }

    /// Prepares an inline hook at `target` and points `original` at the trampoline the detour
    /// calls. Nothing changes in the game until [`Hooks::enable`].
    ///
    /// # Safety
    /// `target` must be the start of a function of type `F` in a loaded module, and `prologue`
    /// its first instructions with every rip-relative displacement marked. `F` must be a
    /// function pointer type.
    pub unsafe fn inline<F: Copy>(
        &mut self,
        original: &Original<F>,
        name: &str,
        target: usize,
        prologue: &[Instruction],
        detour: F,
    ) -> Result<()> {
        self.refuse_twice(name, target)?;
        let (hook, trampoline) = InlineHook::described(name, target, prologue, function_address(detour))?;
        self.inline.push(hook);
        // SAFETY: the trampoline runs the target's own instructions, so it has type F.
        unsafe { original.set(trampoline) };
        Ok(())
    }

    /// Like [`Hooks::inline`] for code whose bytes are not known in advance (a system DLL that
    /// differs between Windows builds): its first instructions are decoded, and a prologue the
    /// decoder does not fully understand is refused ([`Error::Signature`]).
    ///
    /// # Safety
    /// `target` must be the start of a function of type `F`, a function pointer type.
    pub unsafe fn inline_decoded<F: Copy>(&mut self, original: &Original<F>, name: &str, target: usize, detour: F) -> Result<()> {
        self.refuse_twice(name, target)?;
        let (hook, trampoline) = InlineHook::decoded(name, target, function_address(detour))?;
        self.inline.push(hook);
        // SAFETY: the trampoline runs the target's own instructions, so it has type F.
        unsafe { original.set(trampoline) };
        Ok(())
    }

    /// Like [`Hooks::inline_decoded`] for a function another program may have hooked first (the
    /// Steam overlay hooks XInput in every game): when it starts with their jump (`e9` rel32, or
    /// `ff 25 00000000` and an absolute address), that jump moves into our trampoline, so their
    /// hook still runs after ours. Code that is neither is decoded, and refused with its first
    /// bytes in the error when it does not decode.
    ///
    /// # Safety
    /// `target` must be the start of a function of type `F`, a function pointer type.
    pub unsafe fn inline_chained<F: Copy>(&mut self, original: &Original<F>, name: &str, target: usize, detour: F) -> Result<()> {
        let mut first = [0u8; 16];
        if !mem::read_bytes(target, &mut first) {
            return Err(Error::Refused(format!("{name}: code unreadable")));
        }
        let leak = |bytes: &[u8]| -> &'static [u8] { Box::leak(bytes.to_vec().into_boxed_slice()) };
        let jump = match first {
            [0xe9, ..] => Some(Instruction::rip_relative(leak(&first[..5]), 1)),
            // An absolute jump and its address run the same anywhere, as one piece.
            _ => crate::arch::absolute_jump_at(&first).map(|length| Instruction::plain(leak(&first[..length]))),
        };
        // SAFETY: the caller's guarantee; a jump prologue is exactly the bytes just read.
        unsafe {
            match jump {
                Some(jump) => self.inline(original, name, target, &[jump], detour),
                None => self.inline_decoded(original, name, target, detour).map_err(|e| match e {
                    Error::Signature(why) => Error::Signature(format!("{why}; code {first:02x?}")),
                    other => other,
                }),
            }
        }
    }

    /// Hooks method `index` of a COM-style object in its code, falling back to the object's
    /// vtable slot when the prologue does not decode. Direct3D rewrites some of the D3D11
    /// immediate context's vtable entries while a game runs (the draws), which silently undoes a
    /// vtable hook, so the code is preferred. Returns where the hook went.
    ///
    /// # Safety
    /// `object` must be a live COM-style object whose method `index` has type `F`, a function
    /// pointer type.
    pub unsafe fn method<F: Copy>(
        &mut self,
        original: &Original<F>,
        name: &str,
        object: *mut core::ffi::c_void,
        index: usize,
        detour: F,
    ) -> Result<Placement> {
        let target = mem::read::<usize>(object as usize)
            .and_then(|table| mem::read::<usize>(table + index * size_of::<usize>()))
            .ok_or_else(|| Error::Refused(format!("{name}: vtable unreadable")))?;
        // SAFETY: `target` is the object's own method of type F (the caller's guarantee).
        match unsafe { self.inline_decoded(original, name, target, detour) } {
            Ok(()) => Ok(Placement::Code),
            Err(Error::Signature(why)) => {
                // SAFETY: as above.
                unsafe { self.vtable(original, name, object, index, detour) }?;
                Ok(Placement::Vtable(why))
            }
            Err(e) => Err(e),
        }
    }

    /// [`Hooks::method`] with its vtable fallback reported through `note` (a producer's log line),
    /// for callers that only need to know.
    ///
    /// # Safety
    /// As [`Hooks::method`].
    pub unsafe fn method_noted<F: Copy>(&mut self, original: &Original<F>, name: &str, object: *mut core::ffi::c_void, index: usize, detour: F, note: impl FnOnce(&str)) -> Result<()> {
        // SAFETY: the caller's guarantee.
        if let Placement::Vtable(why) = unsafe { self.method(original, name, object, index, detour) }? {
            note(&why);
        }
        Ok(())
    }

    /// Prepares a hook of method `index` of `object`'s vtable and points `original` at the
    /// method it replaces.
    ///
    /// # Safety
    /// `object` must be a live COM-style object whose method `index` has type `F`, a function
    /// pointer type.
    pub unsafe fn vtable<F: Copy>(
        &mut self,
        original: &Original<F>,
        name: &str,
        object: *mut core::ffi::c_void,
        index: usize,
        detour: F,
    ) -> Result<()> {
        let unreadable = || Error::Refused(format!("{name}: vtable unreadable"));
        let table = mem::read::<usize>(object as usize).ok_or_else(unreadable)?;
        let slot = table + index * size_of::<usize>();
        let method = mem::read::<usize>(slot).ok_or_else(unreadable)?;
        if self.vtable.iter().any(|hook| hook.slot == slot) {
            return Err(Error::Refused(format!("{name}: vtable slot already hooked")));
        }
        self.vtable.push(VtableHook { name: name.to_owned(), slot, original: method, detour: function_address(detour), enabled: false });
        // SAFETY: the slot held the object's own method of type F.
        unsafe { original.set(method) };
        Ok(())
    }

    /// Prepares a hook of `module`'s import of `function` from `dll`: the module's own import
    /// address table entry is swapped, so only its calls are redirected and no code is patched.
    /// The robust way to hook a system function whose code differs between Windows builds (an
    /// OpenGL game's `SwapBuffers`, for example).
    ///
    /// # Safety
    /// The imported function must have type `F`, a function pointer type.
    pub unsafe fn import<F: Copy>(
        &mut self,
        original: &Original<F>,
        name: &str,
        module: &crate::module::Module,
        dll: &str,
        function: &str,
        detour: F,
    ) -> Result<()> {
        let slot = module.import_slot(dll, function).ok_or_else(|| Error::Refused(format!("{name}: {dll}!{function} is not imported")))?;
        // SAFETY: the caller's guarantee.
        unsafe { self.import_at(original, name, slot, detour) }
    }

    /// Like [`Hooks::import`] for an export the module imports by number.
    ///
    /// # Safety
    /// As [`Hooks::import`]: export `ordinal` of `dll` must have type `F`.
    pub unsafe fn import_ordinal<F: Copy>(
        &mut self,
        original: &Original<F>,
        name: &str,
        module: &crate::module::Module,
        dll: &str,
        ordinal: u16,
        detour: F,
    ) -> Result<()> {
        let slot = module.import_ordinal_slot(dll, ordinal).ok_or_else(|| Error::Refused(format!("{name}: {dll}!#{ordinal} is not imported")))?;
        // SAFETY: the caller's guarantee.
        unsafe { self.import_at(original, name, slot, detour) }
    }

    /// Prepares a hook of a function pointer the game keeps in its data at `slot` (one it looked up
    /// at run time and calls through): the pointer is swapped, as an import slot is, and no code is
    /// patched. Refused while the slot is still empty.
    ///
    /// # Safety
    /// `slot` must hold a function of type `F`, called through the slot, for as long as the hook is in.
    pub unsafe fn pointer<F: Copy>(&mut self, original: &Original<F>, name: &str, slot: usize, detour: F) -> Result<()> {
        if mem::read::<usize>(slot).is_none_or(|current| current == 0) {
            return Err(Error::Refused(format!("{name}: no function in the slot yet")));
        }
        // SAFETY: the caller's guarantee.
        unsafe { self.import_at(original, name, slot, detour) }
    }

    /// Prepares a hook of import slot `slot`, which holds a function of type `F`.
    unsafe fn import_at<F: Copy>(&mut self, original: &Original<F>, name: &str, slot: usize, detour: F) -> Result<()> {
        let current = mem::read::<usize>(slot).ok_or_else(|| Error::Refused(format!("{name}: import slot unreadable")))?;
        if self.vtable.iter().any(|hook| hook.slot == slot) {
            return Err(Error::Refused(format!("{name}: import slot already hooked")));
        }
        self.vtable.push(VtableHook { name: name.to_owned(), slot, original: current, detour: function_address(detour), enabled: false });
        // SAFETY: the slot held the imported function, of type F.
        unsafe { original.set(current) };
        Ok(())
    }

    pub fn enable(&mut self) -> Result<()> {
        self.switch(true)
    }

    /// Removes every hook. Hooks whose slot someone else has since replaced are left in place and
    /// reported in the error.
    pub fn disable(&mut self) -> Result<()> {
        self.switch(false)
    }

    fn switch(&mut self, enabled: bool) -> Result<()> {
        let ranges: Vec<_> = self.inline.iter().map(InlineHook::unsafe_range).collect();
        let mut attempt = 0;
        // Threads are frozen only to patch code: a game's anti-tamper can take suspended threads
        // for a debugger (Deathloop's Denuvo closes the game after a freeze). Slot swaps below are
        // single atomic stores and need no freeze.
        while !self.inline.is_empty() {
            let frozen = Frozen::others();
            if !frozen.any_inside(&ranges) {
                let mut failed = None;
                for (index, hook) in self.inline.iter_mut().enumerate() {
                    // SAFETY: every other thread is frozen outside the patched bytes.
                    if let Err(e) = unsafe { hook.set(enabled) } {
                        failed = Some((index, e));
                        break;
                    }
                }
                drop(frozen);
                if let Some((index, e)) = failed {
                    return Err(Error::Os(format!("{}: write", self.inline[index].name), e));
                }
                break;
            }
            drop(frozen);
            attempt += 1;
            if attempt == 200 {
                return Err(Error::Busy("inline hooks".to_owned()));
            }
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        let mut refused = Vec::new();
        for hook in &mut self.vtable {
            if let Err(e) = hook.set(enabled) {
                refused.push(e.to_string());
            }
        }
        if refused.is_empty() { Ok(()) } else { Err(Error::Refused(refused.join("; "))) }
    }
}

/// The original of a hooked function, as the detour calls it: set once before the hook is
/// enabled, read on every call.
pub struct Original<F> {
    address: AtomicUsize,
    _function: PhantomData<F>,
}

// SAFETY: holds only an address.
unsafe impl<F> Sync for Original<F> {}

impl<F: Copy> Original<F> {
    pub const fn new() -> Self {
        Self { address: AtomicUsize::new(0), _function: PhantomData }
    }

    /// # Safety
    /// `address` must be a function of type `F` (a function pointer type) for as long as the
    /// hook can run.
    pub unsafe fn set(&self, address: usize) {
        assert_eq!(size_of::<F>(), size_of::<usize>(), "Original holds function pointers only");
        self.address.store(address, Ordering::Release);
    }

    pub fn is_set(&self) -> bool {
        self.address.load(Ordering::Acquire) != 0
    }

    /// The original function, for a caller holding `_flight` (a detour holds it for its whole run,
    /// so a producer stops only when none is running). Detours run only after [`Original::set`],
    /// so it is always set there.
    pub fn get(&self, _flight: &crate::InFlight) -> F {
        let address = self.address.load(Ordering::Acquire);
        debug_assert_ne!(address, 0, "original read before it was set");
        // SAFETY: `set` stored a function of type F, and F is pointer-sized.
        unsafe { std::mem::transmute_copy(&address) }
    }
}

impl<F: Copy> Default for Original<F> {
    fn default() -> Self {
        Self::new()
    }
}

// x86-64 machine code; the 32-bit tests are in `arch/x86.rs`.
#[cfg(all(test, target_arch = "x86_64"))]
mod tests;
