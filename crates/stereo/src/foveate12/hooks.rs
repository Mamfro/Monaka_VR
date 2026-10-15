//! The hooks: render target views sized at their creation, and on the game's command lists the
//! targets, viewports, render passes and resets that tell a scene pass, where VRS is set and cleared.

use super::*;

pub(super) type CreateViewFn = unsafe extern "system" fn(*mut c_void, *mut c_void, *const D3D12_RENDER_TARGET_VIEW_DESC, D3D12_CPU_DESCRIPTOR_HANDLE);

pub(super) static CREATE_VIEW: Original<CreateViewFn> = Original::new();

/// VRS is set on scene passes (armed, until the stop).
pub(super) static ARMED: AtomicBool = AtomicBool::new(false);
/// Lists VRS is set on now (passes on them are looked at even when not armed, to clear it).
pub(super) static ON_LISTS: AtomicU64 = AtomicU64::new(0);
/// The distance between render target views in a descriptor heap.
pub(super) static VIEW_STEP: AtomicU32 = AtomicU32::new(0);

/// The game's command-list calls foveation watches ([`crate::lists12`]).
pub(super) static SUBSCRIBER: Subscriber =
    Subscriber { name: "foveation", targets: Some(set_targets), viewports: Some(set_viewports), begin_pass: Some(begin_pass), reset: Some(reset_list), ..Subscriber::NONE };

/// Hooks the game device's `CreateRenderTargetView` (the descriptor step taken from it too), and
/// subscribes to the game's command lists: `OMSetRenderTargets`, `BeginRenderPass`,
/// `RSSetViewports` and `Reset` ([`crate::lists12`], from the game queue's executions).
///
/// # Safety
/// At install, before the hooks are enabled; `queue` is the game's live direct queue and `device`
/// its device.
pub(super) unsafe fn hook(hooks: &mut Hooks, queue: &ID3D12CommandQueue, device: &ID3D12Device) -> Result<(), Lack> {
    // SAFETY: reads a constant of the live device.
    VIEW_STEP.store(unsafe { device.GetDescriptorHandleIncrementSize(D3D12_DESCRIPTOR_HEAP_TYPE_RTV) }, Relaxed);
    let create = lists12::slot(offset_of!(ID3D12Device_Vtbl, CreateRenderTargetView));
    let place = lists12::method_place(device.as_raw(), create);
    // SAFETY: the game's live device, whose method has the detour's type (the `windows` crate's
    // vtable layout gives the slot).
    unsafe {
        hooks.method_noted(&CREATE_VIEW, "CreateRenderTargetView", device.as_raw(), create, create_view as CreateViewFn, |why| {
            log!("foveation: CreateRenderTargetView: {why}; hooking its vtable slot instead")
        })
    }
    .missing()?;
    log!("foveation: the game's device {:#x}: CreateRenderTargetView hooked at {place}", device.as_raw() as usize);
    // Last: a feature that installs has joined, and leaves at its release.
    // SAFETY: the caller's guarantee.
    unsafe { lists12::join(hooks, queue, &SUBSCRIBER) }.missing()
}

/// Render target views' sizes, by descriptor handle.
pub(super) static VIEWS: Mutex<BTreeMap<usize, (u32, u32)>> = Mutex::new(BTreeMap::new());

unsafe extern "system" fn create_view(device: *mut c_void, resource: *mut c_void, desc: *const D3D12_RENDER_TARGET_VIEW_DESC, handle: D3D12_CPU_DESCRIPTOR_HANDLE) {
    let flight = InFlight::enter();
    CALLS[C_VIEWS].fetch_add(1, Relaxed);
    // SAFETY: the game's view description (or null), for the duration of its call.
    let size = view_size(resource, unsafe { desc.as_ref() });
    log_first(1, format_args!("foveation: first CreateRenderTargetView, on device {:#x}: a view of {size:?} at {:#x}", device as usize, handle.ptr));
    {
        let mut views = VIEWS.locked();
        match size {
            Some(size) => views.insert(handle.ptr, size),
            None => views.remove(&handle.ptr),
        };
    }
    // SAFETY: the original, with the game's arguments.
    unsafe { CREATE_VIEW.get(&flight)(device, resource, desc, handle) }
}

/// The size of a view of `resource` (a 2D texture) as `desc` describes it (its mip level).
pub(super) fn view_size(resource: *mut c_void, desc: Option<&D3D12_RENDER_TARGET_VIEW_DESC>) -> Option<(u32, u32)> {
    // SAFETY: the resource the game's call names, live for the call; only its description is read.
    let texture = unsafe { ID3D12Resource::from_raw_borrowed(&resource) }?;
    // SAFETY: reads a descriptor.
    let d = unsafe { texture.GetDesc() };
    if d.Dimension != D3D12_RESOURCE_DIMENSION_TEXTURE2D {
        return None;
    }
    let mip = match desc {
        // SAFETY: both views of these dimensions start with their mip level.
        Some(desc) if desc.ViewDimension == D3D12_RTV_DIMENSION_TEXTURE2D || desc.ViewDimension == D3D12_RTV_DIMENSION_TEXTURE2DARRAY => unsafe { desc.Anonymous.Texture2D.MipSlice },
        _ => 0,
    };
    Some((((d.Width >> mip) as u32).max(1), (d.Height >> mip).max(1)))
}

/// The render targets a pass binds.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(super) enum Targets {
    /// None (a depth-only pass), or not bound yet on this list.
    #[default]
    None,
    /// All of one size.
    Sized((u32, u32)),
    /// Of different sizes.
    Mixed,
    /// One whose view was made before the hooks went in: the viewport tells the size.
    Unknown,
}

/// The targets of `count` views (`handle` gives the descriptor of the `i`th).
pub(super) fn targets_of(count: u32, handle: impl Fn(usize) -> Option<usize>) -> Targets {
    let views = VIEWS.locked();
    let mut size = None;
    for i in 0..count.min(8) as usize {
        match handle(i).and_then(|h| views.get(&h)) {
            None => return Targets::Unknown,
            Some(s) if size.is_some_and(|known| known != *s) => return Targets::Mixed,
            Some(s) => size = Some(*s),
        }
    }
    size.map_or(Targets::None, Targets::Sized)
}

/// The targets an `OMSetRenderTargets` binds.
pub(super) fn bound_targets(count: u32, handles: *const D3D12_CPU_DESCRIPTOR_HANDLE, single: BOOL) -> Targets {
    if handles.is_null() {
        return Targets::None;
    }
    let step = VIEW_STEP.load(Relaxed) as usize;
    // A single handle stands for a range of views in one heap.
    targets_of(count, |i| if single.as_bool() { mem::read::<usize>(handles as usize).map(|h| h + i * step) } else { mem::read::<usize>(handles as usize + i * 8) })
}

/// # Safety
/// [`crate::lists12`]'s: inside the game's call on its list, being recorded on this thread.
unsafe fn set_targets(list: *mut c_void, count: u32, handles: *const D3D12_CPU_DESCRIPTOR_HANDLE, single: BOOL, _: *const D3D12_CPU_DESCRIPTOR_HANDLE) {
    CALLS[C_TARGETS].fetch_add(1, Relaxed);
    log_first(1, format_args!("foveation: first OMSetRenderTargets, on list {:#x}: targets {:?} (scene {:?})", list as usize, bound_targets(count, handles, single), scene()));
    if engaged() {
        let targets = bound_targets(count, handles, single);
        // SAFETY: the game's list, being recorded on this thread, outside a render pass.
        unsafe { pass(list, true, |p| p.targets = targets) };
    }
}

/// The targets a `BeginRenderPass` binds.
pub(super) fn pass_targets(count: u32, targets: *const D3D12_RENDER_PASS_RENDER_TARGET_DESC) -> Targets {
    let stride = size_of::<D3D12_RENDER_PASS_RENDER_TARGET_DESC>();
    // Each target's description starts with its view's handle.
    if targets.is_null() { Targets::None } else { targets_of(count, |i| mem::read::<usize>(targets as usize + i * stride)) }
}

/// # Safety
/// As [`set_targets`].
unsafe fn begin_pass(list: *mut c_void, count: u32, targets: *const D3D12_RENDER_PASS_RENDER_TARGET_DESC) {
    CALLS[C_PASSES].fetch_add(1, Relaxed);
    log_first(1, format_args!("foveation: first BeginRenderPass, on list {:#x}: targets {:?} (scene {:?})", list as usize, pass_targets(count, targets), scene()));
    if engaged() {
        let bound = pass_targets(count, targets);
        // SAFETY: the game's list, being recorded on this thread; the pass has not begun yet.
        unsafe { pass(list, true, |p| p.targets = bound) };
    }
}

/// The first viewport: left, top, width, height (then its depth range).
pub(super) fn first_viewport(count: u32, viewports: *const D3D12_VIEWPORT) -> Option<[f32; 4]> {
    if count > 0 && !viewports.is_null() { mem::read::<[f32; 4]>(viewports as usize) } else { None }
}

/// # Safety
/// As [`set_targets`].
unsafe fn set_viewports(list: *mut c_void, count: u32, viewports: *const D3D12_VIEWPORT) {
    CALLS[C_VIEWPORTS].fetch_add(1, Relaxed);
    log_first(1, format_args!("foveation: first RSSetViewports, on list {:#x}: {count} viewports, the first {:?} (scene {:?})", list as usize, first_viewport(count, viewports), scene()));
    if engaged() {
        let first = first_viewport(count, viewports);
        if let Some([_, _, w, h]) = first {
            note_viewport((w.round() as u32, h.round() as u32));
        }
        let size = first.filter(|&[x, y, w, h]| x == 0.0 && y == 0.0 && w >= 1.0 && h >= 1.0).map(|[_, _, w, h]| (w.round() as u32, h.round() as u32));
        // SAFETY: the game's list, being recorded on this thread (perhaps inside a render pass, so
        // nothing is copied here).
        unsafe { pass(list, false, |p| p.viewport = size) };
    }
}

/// # Safety
/// As [`set_targets`].
unsafe fn reset_list(list: *mut c_void) {
    CALLS[C_RESETS].fetch_add(1, Relaxed);
    log_first(1, format_args!("foveation: first Reset, of list {:#x} (scene {:?})", list as usize, scene()));
    if let Some(gone) = LISTS.locked().remove(&(list as usize))
        && gone.bound.is_some()
    {
        ON_LISTS.fetch_sub(1, Relaxed);
    }
}

pub(super) fn engaged() -> bool {
    ARMED.load(Relaxed) || ON_LISTS.load(Relaxed) > 0
}

/// What a list has bound since its last reset.
#[derive(Clone, Copy, Debug, Default)]
pub(super) struct Pass {
    pub(super) targets: Targets,
    pub(super) viewport: Option<(u32, u32)>,
    /// The image set on it (its eye and the images' generation), if any.
    pub(super) bound: Option<(usize, u64)>,
}

impl Pass {
    /// The size its pass draws at: its targets', or its viewport's for targets of unknown size.
    pub(super) fn size(&self) -> Option<((u32, u32), bool)> {
        match self.targets {
            Targets::Sized(size) => Some((size, true)),
            Targets::Unknown => self.viewport.map(|size| (size, false)),
            Targets::None | Targets::Mixed => None,
        }
    }
}

/// The image's rate in place of the pass's own: the draw's rate passed through, then overridden.
pub(super) const COMBINERS: [D3D12_SHADING_RATE_COMBINER; 2] = [D3D12_SHADING_RATE_COMBINER_PASSTHROUGH, D3D12_SHADING_RATE_COMBINER_OVERRIDE];

pub(super) static LISTS: Mutex<BTreeMap<usize, Pass>> = Mutex::new(BTreeMap::new());
/// More lists than this tracked at once: the table starts afresh (lists the game let go of).
pub(super) const LISTS_KEPT: usize = 4096;

/// A pass on `list` changed what it binds (`change`): VRS set when it draws at the scene's size,
/// cleared when it was set and the pass does not. `may_copy`: an image may be brought up to date
/// on the list here.
///
/// # Safety
/// `list` is the game's live command list, being recorded on this thread.
unsafe fn pass(list: *mut c_void, may_copy: bool, change: impl FnOnce(&mut Pass)) {
    let scene = scene();
    let (size, bound) = {
        let mut lists = LISTS.locked();
        if lists.len() >= LISTS_KEPT && !lists.contains_key(&(list as usize)) {
            let on = lists.values().filter(|p| p.bound.is_some()).count() as u64;
            ON_LISTS.fetch_sub(on.min(ON_LISTS.load(Relaxed)), Relaxed);
            lists.clear();
        }
        let entry = lists.entry(list as usize).or_default();
        change(entry);
        (entry.size(), entry.bound)
    };
    let scene_pass = match (size, scene) {
        (Some((size, by_target)), Some(scene)) if size == scene => {
            TALLY.passes[by_target as usize].fetch_add(1, Relaxed);
            ARMED.load(Relaxed)
        }
        _ => false,
    };
    if !scene_pass && bound.is_none() {
        return;
    }
    // SAFETY: the game's live list (the caller's guarantee).
    let recording = unsafe { Recording::new(list) };
    let chosen = if scene_pass { image_for(&recording, scene.unwrap_or_default(), may_copy) } else { None };
    let now = chosen.as_ref().map(|(_, b)| *b);
    if now == bound {
        return;
    }
    let Some(list5) = recording.list() else { return };
    // SAFETY: state calls on the list being recorded for the game's; the image lives as long as the
    // run's state keeps it (past the GPU's use).
    unsafe {
        match &chosen {
            Some((image, _)) => {
                list5.RSSetShadingRate(D3D12_SHADING_RATE_1X1, Some(COMBINERS.as_ptr()));
                list5.RSSetShadingRateImage(image);
            }
            None => {
                list5.RSSetShadingRateImage(None::<&ID3D12Resource>);
                list5.RSSetShadingRate(D3D12_SHADING_RATE_1X1, None);
            }
        }
    }
    match (bound.is_some(), now.is_some()) {
        (false, true) => ON_LISTS.fetch_add(1, Relaxed),
        (true, false) => ON_LISTS.fetch_sub(1, Relaxed),
        _ => 0,
    };
    if let Some(entry) = LISTS.locked().get_mut(&(list as usize)) {
        entry.bound = now;
    }
}

/// The list VRS and the image writes are recorded on, found when first needed: the native list
/// behind Streamline's proxy when the game records on one (`slGetNativeInterface`), else the
/// game's list itself. A list without VRS is logged once.
pub(super) struct Recording {
    pub(super) game: *mut c_void,
    pub(super) list: OnceCell<Option<ID3D12GraphicsCommandList5>>,
}

impl Recording {
    /// # Safety
    /// `game` is the game's live command list, being recorded on this thread, for this value's life.
    pub(super) unsafe fn new(game: *mut c_void) -> Self {
        Self { game, list: OnceCell::new() }
    }

    pub(super) fn list(&self) -> Option<&ID3D12GraphicsCommandList5> {
        self.list.get_or_init(|| self.find()).as_ref()
    }

    pub(super) fn find(&self) -> Option<ID3D12GraphicsCommandList5> {
        let game = self.game as usize;
        // SAFETY: the game's live list ([`Recording::new`]'s guarantee).
        let native = unsafe { imports::native_list(self.game) };
        let cast = match &native {
            Some(native) => {
                log_first(1, format_args!("foveation: VRS recorded on the native list {:#x} behind the game's {game:#x}", native.as_raw() as usize));
                native.cast::<ID3D12GraphicsCommandList5>()
            }
            None => {
                log_first(1, format_args!("foveation: VRS recorded on the game's list {game:#x} itself (no native list behind it)"));
                // SAFETY: as above, borrowed for this call.
                let list = unsafe { ID3D12GraphicsCommandList::from_raw_borrowed(&self.game) }?;
                list.cast::<ID3D12GraphicsCommandList5>()
            }
        };
        match cast {
            Ok(list) => Some(list),
            Err(e) => {
                log_first(1, format_args!("foveation: no VRS on the game's list {game:#x}: no ID3D12GraphicsCommandList5 ({e})"));
                None
            }
        }
    }
}
