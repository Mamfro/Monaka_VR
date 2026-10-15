//! One hook point for a D3D12 game's command lists, shared by the features that watch or add to
//! them. The game queue's `ExecuteCommandLists` is hooked ([`join`]), and the first list it
//! executes is hooked once, in that list's own code: those of `OMSetRenderTargets`,
//! `RSSetViewports`, `Reset`, `BeginRenderPass`, `DrawInstanced`, `DrawIndexedInstanced` and
//! `BeginEvent` its subscribers watch. Every list of
//! its class shares that code, so this reaches the lists the game records on, also when they are
//! another library's proxies (Streamline's, when the game's device was made through
//! `sl.interposer.dll`) whose code a list of our own would not share.
//!
//! A feature subscribes with a [`Subscriber`]: plain functions, each called inside the game's
//! call, before the game's own (a draw subscriber may make the game's draw itself). One set of
//! hooks serves every subscriber, so two features never patch the same code. The list hooks come
//! out when the last subscriber leaves ([`leave`]); the queue hook goes with the run's hooks.

use monaka_core::sync::Locked;
use monaka_hook::module::Module;
use monaka_hook::{Hooks, InFlight, Original, Placement, mem};
use monaka_producer::log;
use std::ffi::c_void;
use std::mem::offset_of;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicPtr, Ordering::*};
use windows::Win32::Graphics::Direct3D12::*;
use windows::core::{BOOL, HRESULT, Interface};

/// The game queue executes `count` lists, at `lists` (called on every execution, the lists hooked
/// or not: [`hooked`]).
pub type Execute = fn(count: u32, lists: *const *mut c_void);
/// `OMSetRenderTargets`, with the game's arguments.
pub type SetTargets = unsafe fn(list: *mut c_void, count: u32, targets: *const D3D12_CPU_DESCRIPTOR_HANDLE, single: BOOL, depth: *const D3D12_CPU_DESCRIPTOR_HANDLE);
/// `RSSetViewports`, with the game's arguments.
pub type SetViewports = unsafe fn(list: *mut c_void, count: u32, viewports: *const D3D12_VIEWPORT);
/// `BeginRenderPass`: its render targets (each description starts with its view's handle).
pub type BeginPass = unsafe fn(list: *mut c_void, count: u32, targets: *const D3D12_RENDER_PASS_RENDER_TARGET_DESC);
/// `Reset`: the list starts afresh.
pub type Reset = unsafe fn(list: *mut c_void);
/// `DrawInstanced` or `DrawIndexedInstanced` of `count` vertices (or indices). `draw` makes the
/// game's draw; true when the subscriber made it itself (or kept it out on purpose): the later
/// subscribers do not see it, and the game's draw is not made again.
pub type Draw = unsafe fn(list: *mut c_void, count: u32, draw: &dyn Fn()) -> bool;
/// `BeginEvent`.
pub type Event = unsafe fn(list: *mut c_void);

/// What a feature does with the game's command lists: a function for each call it watches. The
/// list functions are called inside the game's call on its list, being recorded on that thread
/// (their safety condition): they may record on it.
#[derive(Clone, Copy)]
pub struct Subscriber {
    /// For the log.
    pub name: &'static str,
    pub execute: Option<Execute>,
    pub targets: Option<SetTargets>,
    pub viewports: Option<SetViewports>,
    pub begin_pass: Option<BeginPass>,
    pub reset: Option<Reset>,
    pub draw: Option<Draw>,
    pub event: Option<Event>,
}

impl Subscriber {
    /// Watches nothing: a subscriber states only what it does (struct update syntax).
    pub const NONE: Subscriber = Subscriber { name: "", execute: None, targets: None, viewports: None, begin_pass: None, reset: None, draw: None, event: None };
}

/// The subscribers, in the order they joined (null: a free place).
const PLACES: usize = 4;
static SUBSCRIBED: [AtomicPtr<Subscriber>; PLACES] = [const { AtomicPtr::new(std::ptr::null_mut()) }; PLACES];

fn subscribers() -> impl Iterator<Item = &'static Subscriber> {
    SUBSCRIBED.iter().filter_map(|place| {
        let at = place.load(Acquire);
        // SAFETY: only `&'static Subscriber`s are stored ([`join`]).
        unsafe { at.cast_const().as_ref() }
    })
}

fn pointer(subscriber: &'static Subscriber) -> *mut Subscriber {
    (subscriber as *const Subscriber).cast_mut()
}

// --- The hooks -----------------------------------------------------------------------------------

type ExecuteFn = unsafe extern "system" fn(*mut c_void, u32, *const *mut c_void);
type SetTargetsFn = unsafe extern "system" fn(*mut c_void, u32, *const D3D12_CPU_DESCRIPTOR_HANDLE, BOOL, *const D3D12_CPU_DESCRIPTOR_HANDLE);
type ViewportsFn = unsafe extern "system" fn(*mut c_void, u32, *const D3D12_VIEWPORT);
type ResetFn = unsafe extern "system" fn(*mut c_void, *mut c_void, *mut c_void) -> HRESULT;
type BeginPassFn = unsafe extern "system" fn(*mut c_void, u32, *const D3D12_RENDER_PASS_RENDER_TARGET_DESC, *const D3D12_RENDER_PASS_DEPTH_STENCIL_DESC, D3D12_RENDER_PASS_FLAGS);
type DrawInstancedFn = unsafe extern "system" fn(*mut c_void, u32, u32, u32, u32);
type DrawIndexedInstancedFn = unsafe extern "system" fn(*mut c_void, u32, u32, u32, i32, u32);
type EventFn = unsafe extern "system" fn(*mut c_void, u32, *const c_void, u32);

static EXECUTE: Original<ExecuteFn> = Original::new();
static SET_TARGETS: Original<SetTargetsFn> = Original::new();
static VIEWPORTS: Original<ViewportsFn> = Original::new();
static RESET: Original<ResetFn> = Original::new();
static BEGIN_PASS: Original<BeginPassFn> = Original::new();
static DRAW_INSTANCED: Original<DrawInstancedFn> = Original::new();
static DRAW_INDEXED_INSTANCED: Original<DrawIndexedInstancedFn> = Original::new();
static BEGIN_EVENT: Original<EventFn> = Original::new();

/// The queue hook is in this run's hooks.
static QUEUE_HOOKED: AtomicBool = AtomicBool::new(false);
/// The first executed list has been hooked (or tried), and the list hooks are in.
static TRIED: AtomicBool = AtomicBool::new(false);
static HOOKED: AtomicBool = AtomicBool::new(false);
/// The list hooks, installed from the first list the queue executes.
static LIST_HOOKS: Mutex<Option<Hooks>> = Mutex::new(None);

/// A method's vtable slot, from its offset in the `windows` crate's vtable layout.
pub const fn slot(offset: usize) -> usize {
    offset / size_of::<usize>()
}

/// Where method `index` of COM object `object` has its code, for the log (`file.dll+0x1234`).
pub fn method_place(object: *mut c_void, index: usize) -> String {
    match mem::read::<usize>(object as usize).and_then(|table| mem::read::<usize>(table + index * size_of::<usize>())) {
        Some(code) => Module::describe(code),
        None => "an unreadable vtable".to_owned(),
    }
}

/// Hooks the game queue's `ExecuteCommandLists` into `hooks` (once a run, for the first
/// subscriber), then subscribes `subscriber`. A feature joins last in its install, so a feature
/// that installs has joined, and leaves at its release ([`leave`]).
///
/// # Safety
/// `queue` must be the game's live direct queue (a Streamline proxy or the real one), at install,
/// before the hooks are enabled.
pub unsafe fn join(hooks: &mut Hooks, queue: &ID3D12CommandQueue, subscriber: &'static Subscriber) -> monaka_hook::Result<()> {
    if !QUEUE_HOOKED.load(Acquire) {
        let index = slot(offset_of!(ID3D12CommandQueue_Vtbl, ExecuteCommandLists));
        let place = method_place(queue.as_raw(), index);
        // SAFETY: the caller's guarantee; the detour has the method's type.
        unsafe {
            hooks.method_noted(&EXECUTE, "ExecuteCommandLists", queue.as_raw(), index, execute as ExecuteFn, |why| {
                log!("command lists: ExecuteCommandLists: {why}; hooking its vtable slot instead")
            })
        }?;
        QUEUE_HOOKED.store(true, Release);
        log!("command lists: the game queue {:#x}: ExecuteCommandLists hooked at {place}", queue.as_raw() as usize);
    }
    if subscribers().any(|s| std::ptr::eq(s, subscriber)) {
        return Ok(());
    }
    for place in &SUBSCRIBED {
        if place.compare_exchange(std::ptr::null_mut(), pointer(subscriber), AcqRel, Acquire).is_ok() {
            log!("command lists: {} subscribed", subscriber.name);
            return Ok(());
        }
    }
    Err(monaka_hook::Error::Refused(format!("command lists: no place for {} ({PLACES} subscribers already)", subscriber.name)))
}

/// `subscriber` leaves (at its feature's give back or release; nothing when it has left already):
/// the last one takes the list hooks out. Waits up to `idle_ms` for detours still running, so its
/// functions are done when this returns.
pub fn leave(subscriber: &'static Subscriber, idle_ms: u64) {
    let left = SUBSCRIBED.iter().any(|place| place.compare_exchange(pointer(subscriber), std::ptr::null_mut(), AcqRel, Acquire).is_ok());
    if !left {
        return;
    }
    if subscribers().next().is_none() {
        // The queue hook goes with the run's hooks; the next run hooks it again.
        QUEUE_HOOKED.store(false, Release);
        HOOKED.store(false, Release);
        if let Some(mut hooks) = LIST_HOOKS.locked().take()
            && let Err(e) = hooks.disable()
        {
            log!("command lists: list hooks left in place: {e}");
        }
        TRIED.store(false, Release);
    }
    InFlight::wait_idle(idle_ms);
}

/// Whether the game's command lists are hooked.
pub fn hooked() -> bool {
    HOOKED.load(Acquire)
}

/// Binds render targets on the game's list unseen by the subscribers: a feature's own draw, the
/// game's binding put back after. False (nothing bound) while `OMSetRenderTargets` is not hooked.
///
/// # Safety
/// `list` is the game's live list, being recorded on this thread; the arguments are
/// `OMSetRenderTargets`'.
pub unsafe fn bind_targets(list: *mut c_void, count: u32, targets: *const D3D12_CPU_DESCRIPTOR_HANDLE, single: BOOL, depth: *const D3D12_CPU_DESCRIPTOR_HANDLE) -> bool {
    let flight = monaka_hook::InFlight::enter();
    if !hooked() || !SET_TARGETS.is_set() {
        return false;
    }
    // SAFETY: the original, with the caller's arguments.
    unsafe { SET_TARGETS.get(&flight)(list, count, targets, single, depth) };
    true
}

/// Hooks the first list the game queue executes (every list of its class shares its code), then
/// calls the subscribers.
unsafe extern "system" fn execute(queue: *mut c_void, count: u32, lists: *const *mut c_void) {
    let flight = InFlight::enter();
    if !TRIED.load(Relaxed) && count > 0 && !lists.is_null() && subscribers().next().is_some() {
        let list = mem::read::<usize>(lists as usize).unwrap_or(0);
        if list != 0 && !TRIED.swap(true, Relaxed) {
            // SAFETY: a live command list the game is executing, whose methods have these types.
            if let Err(why) = unsafe { hook_list(list as *mut c_void) } {
                log!("command lists: not hooked: {why}");
            }
        }
    }
    for subscriber in subscribers() {
        if let Some(executed) = subscriber.execute {
            executed(count, lists);
        }
    }
    // SAFETY: forwards the game's own call.
    unsafe { EXECUTE.get(&flight)(queue, count, lists) }
}

/// # Safety
/// `list` must be a live `ID3D12GraphicsCommandList`.
unsafe fn hook_list(list: *mut c_void) -> Result<(), String> {
    let mut hooks = Hooks::default();
    let mut hooked = Vec::new();
    // Only the methods a subscriber watches (they all joined at their install, before the hooks
    // went in).
    let wanted = |watches: fn(&Subscriber) -> bool| subscribers().any(watches);
    // SAFETY: the caller's guarantee; each detour has its method's type (the `windows` crate's
    // vtable layouts give the slots).
    unsafe {
        let (h, done) = (&mut hooks, &mut hooked);
        if wanted(|s| s.targets.is_some()) {
            let at = slot(offset_of!(ID3D12GraphicsCommandList_Vtbl, OMSetRenderTargets));
            method(h, done, &SET_TARGETS, "OMSetRenderTargets", list, at, set_targets as SetTargetsFn);
        }
        if wanted(|s| s.viewports.is_some()) {
            method(h, done, &VIEWPORTS, "RSSetViewports", list, slot(offset_of!(ID3D12GraphicsCommandList_Vtbl, RSSetViewports)), set_viewports as ViewportsFn);
        }
        if wanted(|s| s.reset.is_some()) {
            method(h, done, &RESET, "Reset", list, slot(offset_of!(ID3D12GraphicsCommandList_Vtbl, Reset)), reset as ResetFn);
        }
        if wanted(|s| s.draw.is_some()) {
            method(h, done, &DRAW_INSTANCED, "DrawInstanced", list, slot(offset_of!(ID3D12GraphicsCommandList_Vtbl, DrawInstanced)), draw_instanced as DrawInstancedFn);
            let indexed = slot(offset_of!(ID3D12GraphicsCommandList_Vtbl, DrawIndexedInstanced));
            method(h, done, &DRAW_INDEXED_INSTANCED, "DrawIndexedInstanced", list, indexed, draw_indexed_instanced as DrawIndexedInstancedFn);
        }
        if wanted(|s| s.event.is_some()) {
            method(h, done, &BEGIN_EVENT, "BeginEvent", list, slot(offset_of!(ID3D12GraphicsCommandList_Vtbl, BeginEvent)), begin_event as EventFn);
        }
        // BeginRenderPass is on the list's ID3D12GraphicsCommandList4.
        let list4 = if wanted(|s| s.begin_pass.is_some()) { ID3D12GraphicsCommandList::from_raw_borrowed(&list).map(|l| l.cast::<ID3D12GraphicsCommandList4>()) } else { None };
        match list4 {
            Some(Ok(list4)) => {
                let at = slot(offset_of!(ID3D12GraphicsCommandList4_Vtbl, BeginRenderPass));
                method(h, done, &BEGIN_PASS, "BeginRenderPass", list4.as_raw(), at, begin_pass as BeginPassFn);
            }
            Some(Err(e)) => log!("command lists: BeginRenderPass not hooked: the list has no ID3D12GraphicsCommandList4 ({e})"),
            None => {}
        }
    }
    if hooked.is_empty() {
        return Err("no method hooked".to_owned());
    }
    hooks.enable().map_err(|e| e.to_string())?;
    *LIST_HOOKS.locked() = Some(hooks);
    HOOKED.store(true, Release);
    log!("command lists: hooked from the game's list {:#x} (its class at {}): {}", list as usize, method_place(list, 0), hooked.join(", "));
    Ok(())
}

/// Hooks one method of a list into `hooks`, noting where its code is for the log (`hooked`), or
/// logging why not.
///
/// # Safety
/// As [`Hooks::method`].
unsafe fn method<F: Copy>(hooks: &mut Hooks, hooked: &mut Vec<String>, original: &Original<F>, name: &str, object: *mut c_void, index: usize, detour: F) {
    let place = method_place(object, index);
    // SAFETY: the caller's guarantee.
    match unsafe { hooks.method(original, name, object, index, detour) } {
        Ok(Placement::Code) => hooked.push(format!("{name} at {place}")),
        Ok(Placement::Vtable(why)) => hooked.push(format!("{name} in its vtable slot, the code at {place} ({why})")),
        Err(e) => log!("command lists: {name} not hooked: {e}"),
    }
}

unsafe extern "system" fn set_targets(list: *mut c_void, count: u32, targets: *const D3D12_CPU_DESCRIPTOR_HANDLE, single: BOOL, depth: *const D3D12_CPU_DESCRIPTOR_HANDLE) {
    let flight = InFlight::enter();
    for subscriber in subscribers() {
        if let Some(watch) = subscriber.targets {
            // SAFETY: the game's call on its list, with its arguments.
            unsafe { watch(list, count, targets, single, depth) };
        }
    }
    // SAFETY: forwards the game's own call.
    unsafe { SET_TARGETS.get(&flight)(list, count, targets, single, depth) }
}

unsafe extern "system" fn set_viewports(list: *mut c_void, count: u32, viewports: *const D3D12_VIEWPORT) {
    let flight = InFlight::enter();
    for subscriber in subscribers() {
        if let Some(watch) = subscriber.viewports {
            // SAFETY: the game's call on its list, with its arguments.
            unsafe { watch(list, count, viewports) };
        }
    }
    // SAFETY: forwards the game's own call.
    unsafe { VIEWPORTS.get(&flight)(list, count, viewports) }
}

unsafe extern "system" fn reset(list: *mut c_void, allocator: *mut c_void, pipeline: *mut c_void) -> HRESULT {
    let flight = InFlight::enter();
    for subscriber in subscribers() {
        if let Some(watch) = subscriber.reset {
            // SAFETY: the game's call on its list.
            unsafe { watch(list) };
        }
    }
    // SAFETY: forwards the game's own call.
    unsafe { RESET.get(&flight)(list, allocator, pipeline) }
}

unsafe extern "system" fn begin_pass(
    list: *mut c_void,
    count: u32,
    targets: *const D3D12_RENDER_PASS_RENDER_TARGET_DESC,
    depth: *const D3D12_RENDER_PASS_DEPTH_STENCIL_DESC,
    flags: D3D12_RENDER_PASS_FLAGS,
) {
    let flight = InFlight::enter();
    for subscriber in subscribers() {
        if let Some(watch) = subscriber.begin_pass {
            // SAFETY: the game's call on its list, with its arguments; the pass has not begun yet.
            unsafe { watch(list, count, targets) };
        }
    }
    // SAFETY: forwards the game's own call.
    unsafe { BEGIN_PASS.get(&flight)(list, count, targets, depth, flags) }
}

/// A draw: the subscribers in turn, the game's own made by one of them or after them.
///
/// # Safety
/// `list` is the game's live list being recorded, on its own thread; `draw` is the game's call.
unsafe fn drawn(list: *mut c_void, count: u32, draw: &dyn Fn()) {
    for subscriber in subscribers() {
        // SAFETY: the caller's.
        if let Some(watch) = subscriber.draw
            && unsafe { watch(list, count, draw) }
        {
            return;
        }
    }
    draw();
}

unsafe extern "system" fn draw_instanced(list: *mut c_void, vertices: u32, instances: u32, first_vertex: u32, first_instance: u32) {
    let flight = InFlight::enter();
    // SAFETY: the game's own call, on its list.
    unsafe { drawn(list, vertices, &|| DRAW_INSTANCED.get(&flight)(list, vertices, instances, first_vertex, first_instance)) }
}

unsafe extern "system" fn draw_indexed_instanced(list: *mut c_void, indices: u32, instances: u32, first_index: u32, base_vertex: i32, first_instance: u32) {
    let flight = InFlight::enter();
    // SAFETY: as above.
    unsafe { drawn(list, indices, &|| DRAW_INDEXED_INSTANCED.get(&flight)(list, indices, instances, first_index, base_vertex, first_instance)) }
}

unsafe extern "system" fn begin_event(list: *mut c_void, metadata: u32, data: *const c_void, size: u32) {
    let flight = InFlight::enter();
    for subscriber in subscribers() {
        if let Some(watch) = subscriber.event {
            // SAFETY: the game's call on its list.
            unsafe { watch(list) };
        }
    }
    // SAFETY: forwards the game's own call.
    unsafe { BEGIN_EVENT.get(&flight)(list, metadata, data, size) }
}

#[cfg(test)]
mod tests;
