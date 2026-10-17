//! The game's Vulkan device and swapchain as the sharing needs them: its device made with the
//! external memory and semaphore extensions (added at its creation when the GPU has them), its
//! queues by family, its swapchains' images readable by a copy, its depth images too, and each
//! present handed to the sharing first. All through the game executable's own imports
//! (`vkCreateDevice`, `vkGetDeviceQueue`, `vkCreateImage`, `vkGetDeviceProcAddr`, the last giving
//! our present and swapchain creation).
//!
//! The game keeps the present and swapchain creation it was given while it started: the first
//! producer copy's, which stays loaded for the life of the game. So what the hooks see goes into a
//! record of the process's ([`Record`], named shared memory), and every copy's present calls the
//! handler the newest running copy put there: a copy attached later (a new build, a run started
//! again) shares the frames of a device it never saw made.

use crate::ffi::*;
use monaka_channel::shared::{Create, Locked as Lock, SharedBlock, SharedRecord};
use monaka_core::sync::Locked;
use monaka_hook::module::Module;
use monaka_hook::{Hooks, InFlight, Original};
use monaka_producer::{Rejection, log, log_first};
use std::ffi::{CStr, CString, c_char, c_void};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock};

static CREATE_DEVICE: Original<CreateDeviceFn> = Original::new();
static GET_DEVICE_PROC_ADDR: Original<GetDeviceProcAddrFn> = Original::new();
static GET_DEVICE_QUEUE: Original<GetDeviceQueueFn> = Original::new();
static CREATE_IMAGE: Original<CreateImageFn> = Original::new();

/// The extensions the sharing needs; each the GPU offers is added to the game's own.
const SHARING: [&CStr; 6] = [
    c"VK_KHR_external_memory",
    c"VK_KHR_external_memory_win32",
    c"VK_KHR_external_semaphore",
    c"VK_KHR_external_semaphore_win32",
    c"VK_KHR_get_memory_requirements2",
    c"VK_KHR_dedicated_allocation",
];

/// A swapchain of the game's, as the record keeps it.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct Chain {
    pub handle: VkSwapchainKHR,
    pub format: i32,
    pub width: u32,
    pub height: u32,
    pub count: u32,
    pub images: [VkImage; 8],
}

/// A depth image of the game's, as the record keeps it.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct DepthImage {
    pub image: VkImage,
    pub format: i32,
    pub width: u32,
    pub height: u32,
    pub _pad: u32,
}

/// A colour render target of the game's the width of a screen (one a copy may read), as the
/// record keeps it.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct ColorImage {
    pub image: VkImage,
    pub format: i32,
    pub width: u32,
    pub height: u32,
    pub _pad: u32,
}

const QUEUES: usize = 16;
const CHAINS: usize = 4;
const DEPTHS: usize = 128;
const COLORS: usize = 64;
/// A colour render target at least this wide is noted, and made a copy's source.
const SCREEN_WIDTH: u32 = 1024;
/// The record's layout (also in its name); a copy with another layout leaves the record alone.
const LAYOUT: u32 = 2;

/// What the hooks of every producer copy in the game have seen, and the present handler of the
/// newest one. Plain integers only (it lives in shared memory).
#[repr(C)]
#[derive(Clone, Copy)]
pub struct Record {
    pub layout: u32,
    /// The device was made with the sharing extensions (1), without them (2), not yet seen (0).
    pub device_state: u32,
    pub device: u64,
    pub physical: u64,
    pub luid: [u8; 8],
    pub luid_valid: u32,
    pub _pad: u32,
    /// The driver's own functions behind the ones the game was given.
    pub get_device_proc_addr: u64,
    pub real_present: u64,
    pub real_create_swapchain: u64,
    /// The present handler of the newest running copy (`PresentHandler`), 0: none.
    pub handler: u64,
    pub queue_count: u32,
    pub chain_count: u32,
    pub depth_count: u32,
    pub color_count: u32,
    /// The game's queues: the handle, its family.
    pub queues: [[u64; 2]; QUEUES],
    pub chains: [Chain; CHAINS],
    pub depths: [DepthImage; DEPTHS],
    pub colors: [ColorImage; COLORS],
}

// SAFETY: repr(C), integers and arrays of integers only.
unsafe impl SharedRecord for Record {}

/// What a copy's present handler is called with: the game's queue, its swapchain and image index.
pub type PresentHandler = unsafe extern "system" fn(VkQueue, VkSwapchainKHR, u32);

/// This copy's view of the process's record.
static RECORD: Mutex<Option<SharedBlock<Record>>> = Mutex::new(None);

/// The driver's present and swapchain creation behind this copy's own (set where this copy hands
/// its own out, so they are there before the game can call them). The game keeps the functions of
/// the copy that gave them, so each copy's are the ones its own detours need.
static REAL_PRESENT: AtomicUsize = AtomicUsize::new(0);
static REAL_CREATE_SWAPCHAIN: AtomicUsize = AtomicUsize::new(0);
/// The present handler last read from the record: used when the record's lock is busy, so a present
/// never waits on it.
static LAST_HANDLER: AtomicU64 = AtomicU64::new(0);

/// How long a hook waits for the record's lock: briefly on rare calls (device, image and swapchain
/// creation), never on a present.
const RECORD_WAIT_MS: u32 = 200;

/// Runs `work` on the record under its lock (none: no record, or another layout).
fn with_record<R>(work: impl FnOnce(&mut Record) -> R) -> Option<R> {
    with_record_within(RECORD_WAIT_MS, work)
}

/// [`with_record`], waiting at most `wait_ms` for the lock (0: not at all, this copy's own lock
/// included).
fn with_record_within<R>(wait_ms: u32, work: impl FnOnce(&mut Record) -> R) -> Option<R> {
    let mut guard = if wait_ms == 0 { RECORD.try_lock().ok()? } else { RECORD.locked() };
    let block = guard.as_mut()?;
    let mut record = match block.lock(wait_ms) {
        Lock::Held(record) | Lock::Abandoned(record) => record,
        Lock::Busy => return None,
    };
    if record.layout == 0 {
        // SAFETY: an all-zero record is a valid empty one.
        *record = unsafe { std::mem::zeroed() };
        record.layout = LAYOUT;
    }
    (record.layout == LAYOUT).then(|| work(&mut record))
}

/// The game's device, once it was made with the sharing extensions (by this copy's hooks or an
/// earlier one's).
#[derive(Clone, Copy)]
pub struct Device {
    pub device: VkDevice,
    pub physical: VkPhysicalDevice,
    pub luid: Option<[u8; 8]>,
    pub memory: &'static VkPhysicalDeviceMemoryProperties,
}

/// The memory types of the device's GPU, read once by this copy.
static MEMORY: OnceLock<VkPhysicalDeviceMemoryProperties> = OnceLock::new();

/// The game's device, once it was made with the sharing extensions.
pub fn device() -> Option<Device> {
    let (device, physical, luid) = with_record(|r| (r.device_state == 1).then(|| (r.device as usize, r.physical as usize, (r.luid_valid != 0).then_some(r.luid))))??;
    let memory = MEMORY.get_or_init(|| memory_properties(physical));
    Some(Device { device, physical, luid, memory })
}

/// A device function by name, from the device's own table.
pub fn function(name: &CStr) -> Option<usize> {
    let (device, lookup) = with_record(|r| (r.device_state == 1 && r.get_device_proc_addr != 0).then_some((r.device as usize, r.get_device_proc_addr as usize)))??;
    // SAFETY: the driver's own lookup (the record keeps it) on the game's live device.
    let address = unsafe { std::mem::transmute::<usize, GetDeviceProcAddrFn>(lookup)(device, name.as_ptr()) };
    (address != 0).then_some(address)
}

/// The family of the game's queue `queue`.
pub fn family_of(queue: VkQueue) -> Option<u32> {
    with_record(|r| r.queues[..r.queue_count as usize].iter().find(|q| q[0] == queue as u64).map(|q| q[1] as u32))?
}

/// The game's swapchain `handle`.
pub fn swapchain(handle: VkSwapchainKHR) -> Option<Chain> {
    with_record(|r| r.chains[..r.chain_count as usize].iter().find(|c| c.handle == handle).copied())?
}

/// The format and size of the game's depth image `image` (one made since the first copy's hooks
/// went in).
pub fn depth_image(image: VkImage) -> Option<(i32, (u32, u32))> {
    with_record(|r| r.depths[..r.depth_count as usize].iter().find(|d| d.image == image).map(|d| (d.format, (d.width, d.height))))?
}

/// The format and size of the game's screen-wide colour render target `image` (one made since the
/// first copy's hooks went in).
pub fn color_image(image: VkImage) -> Option<(i32, (u32, u32))> {
    with_record(|r| r.colors[..r.color_count as usize].iter().find(|c| c.image == image).map(|c| (c.format, (c.width, c.height))))?
}

/// Whether the hooks have seen the game make its device (this copy's or an earlier one's): not
/// when it was made before any copy joined, or is still to be made.
pub fn device_seen() -> bool {
    with_record(|r| r.device_state != 0).unwrap_or(false)
}

/// Whether a record of an earlier layout (an older producer build's, under its own name) is in this
/// process: the game was started under that build, whose hooks keep their notes there, which this
/// copy cannot read.
pub fn older_record() -> bool {
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Memory::{FILE_MAP_READ, OpenFileMappingW};
    use windows::core::PCWSTR;
    let pid = std::process::id();
    // Layout 1 had no layout in its name.
    let names = std::iter::once(format!("Local\\monaka-vulkan-{pid}")).chain((2..LAYOUT).map(|layout| format!("Local\\monaka-vulkan-{pid}-{layout}")));
    names.into_iter().any(|name| {
        let wide: Vec<u16> = name.encode_utf16().chain(Some(0)).collect();
        // SAFETY: opens a named mapping by a NUL-terminated name, and closes it at once.
        unsafe {
            match OpenFileMappingW(FILE_MAP_READ.0, false, PCWSTR(wide.as_ptr())) {
                Ok(handle) => {
                    let _ = CloseHandle(handle);
                    true
                }
                Err(_) => false,
            }
        }
    })
}

/// Why nothing can be shared yet, for the summary.
pub fn state() -> String {
    with_record(|r| match r.device_state {
        1 => format!("device with sharing; {} swapchains, {} depth images, {} screen-wide colour images", r.chain_count, r.depth_count, r.color_count),
        2 => "the game's device has no sharing extensions".to_owned(),
        _ => "no device seen since the first hooks went in (made before them?)".to_owned(),
    })
    .unwrap_or_else(|| "no record".to_owned())
}

/// This copy's present handler made the one every copy's present calls.
pub fn take_presents(handler: PresentHandler) {
    with_record(|r| r.handler = handler as usize as u64);
}

/// This copy's present handler taken back (when it is still the one).
pub fn give_back_presents(handler: PresentHandler) {
    with_record(|r| {
        if r.handler == handler as usize as u64 {
            r.handler = 0;
        }
    });
}

/// Opens the process's record and hooks the game executable's Vulkan imports.
///
/// # Safety
/// Called once at start, before the hooks are enabled; `module` imports these from vulkan-1.dll.
pub unsafe fn install(hooks: &mut Hooks, module: &Module) -> Result<(), Rejection> {
    let pid = std::process::id();
    let block = SharedBlock::<Record>::create(&format!("Local\\monaka-vulkan-{pid}-{LAYOUT}"), &format!("Local\\monaka-vulkan-{pid}-{LAYOUT}-lock"), Create::OrOpen)
        .map_err(|e| Rejection::not_ready(format!("the Vulkan record: {e}")))?;
    *RECORD.locked() = Some(block);
    if with_record(|r| r.device_state).is_some_and(|seen| seen != 0) {
        log!("Vulkan: joined after the game made its device ({})", state());
    }
    // SAFETY: the imports have these types (the Vulkan API's).
    unsafe {
        hooks.import(&CREATE_DEVICE, "vkCreateDevice", module, "vulkan-1.dll", "vkCreateDevice", create_device as CreateDeviceFn)?;
        hooks.import(&GET_DEVICE_PROC_ADDR, "vkGetDeviceProcAddr", module, "vulkan-1.dll", "vkGetDeviceProcAddr", get_device_proc_addr as GetDeviceProcAddrFn)?;
        hooks.import(&GET_DEVICE_QUEUE, "vkGetDeviceQueue", module, "vulkan-1.dll", "vkGetDeviceQueue", get_device_queue as GetDeviceQueueFn)?;
        hooks.import(&CREATE_IMAGE, "vkCreateImage", module, "vulkan-1.dll", "vkCreateImage", create_image as CreateImageFn)?;
    }
    Ok(())
}

/// vulkan-1.dll's export `name`.
fn loader_export(name: &CStr) -> Option<usize> {
    use windows::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};
    // SAFETY: lookups in the loaded loader.
    unsafe {
        let loader = GetModuleHandleW(windows::core::w!("vulkan-1.dll")).ok()?;
        GetProcAddress(loader, windows::core::PCSTR(name.as_ptr().cast())).map(|f| f as usize)
    }
}

/// The extensions of `physical` among those the sharing needs.
fn offered(physical: VkPhysicalDevice) -> Vec<&'static CStr> {
    let Some(enumerate) = loader_export(c"vkEnumerateDeviceExtensionProperties") else { return Vec::new() };
    // SAFETY: the loader's export of this name has this type.
    let enumerate: EnumerateDeviceExtensionPropertiesFn = unsafe { std::mem::transmute(enumerate) };
    let mut count = 0u32;
    // SAFETY: the two-call enumeration on the game's physical device.
    let names: Vec<CString> = unsafe {
        if enumerate(physical, std::ptr::null(), &mut count, std::ptr::null_mut()) != VK_SUCCESS {
            return Vec::new();
        }
        let mut all: Vec<VkExtensionProperties> = (0..count).map(|_| std::mem::zeroed()).collect();
        if enumerate(physical, std::ptr::null(), &mut count, all.as_mut_ptr()) != VK_SUCCESS {
            return Vec::new();
        }
        all.iter().take(count as usize).map(|e| CStr::from_ptr(e.extensionName.as_ptr()).to_owned()).collect()
    };
    SHARING.into_iter().filter(|wanted| names.iter().any(|n| n.as_c_str() == *wanted)).collect()
}

/// The memory types of `physical`.
fn memory_properties(physical: VkPhysicalDevice) -> VkPhysicalDeviceMemoryProperties {
    // SAFETY: zeroed plain data for the query to fill.
    let mut memory: VkPhysicalDeviceMemoryProperties = unsafe { std::mem::zeroed() };
    if let Some(f) = loader_export(c"vkGetPhysicalDeviceMemoryProperties") {
        // SAFETY: the loader's export of this name has this type.
        unsafe { std::mem::transmute::<usize, GetPhysicalDeviceMemoryPropertiesFn>(f)(physical, &mut memory) };
    }
    memory
}

/// The LUID of `physical`'s GPU.
fn luid(physical: VkPhysicalDevice) -> Option<[u8; 8]> {
    let f = loader_export(c"vkGetPhysicalDeviceProperties2")?;
    let mut id = VkPhysicalDeviceIDProperties {
        sType: STRUCTURE_PHYSICAL_DEVICE_ID_PROPERTIES,
        pNext: std::ptr::null_mut(),
        deviceUUID: [0; 16],
        driverUUID: [0; 16],
        deviceLUID: [0; 8],
        deviceNodeMask: 0,
        deviceLUIDValid: 0,
    };
    // VkPhysicalDeviceProperties2: its type, its chain, then the (large) properties: room to spare.
    let mut properties = vec![0u64; 512];
    properties[0] = STRUCTURE_PHYSICAL_DEVICE_PROPERTIES_2 as u64;
    properties[1] = &mut id as *mut _ as u64;
    // SAFETY: the loader's export of this name has this type; the buffer outsizes the struct.
    unsafe { std::mem::transmute::<usize, GetPhysicalDeviceProperties2Fn>(f)(physical, properties.as_mut_ptr().cast()) };
    (id.deviceLUIDValid != 0).then_some(id.deviceLUID)
}

/// `vkCreateDevice`: the game's device, with the sharing extensions it lacks added.
unsafe extern "system" fn create_device(physical: VkPhysicalDevice, info: *const VkDeviceCreateInfo, allocator: *const c_void, out: *mut VkDevice) -> VkResult {
    let flight = InFlight::enter();
    // SAFETY: the game's own create info, read as the API lays it out.
    let (asked, count) = unsafe { ((*info).ppEnabledExtensionNames, (*info).enabledExtensionCount as usize) };
    let mut names: Vec<*const c_char> = (0..count).map(|i| unsafe { *asked.add(i) }).collect();
    let present: Vec<&CStr> = names.iter().map(|&n| unsafe { CStr::from_ptr(n) }).collect();
    let added: Vec<&CStr> = offered(physical).into_iter().filter(|e| !present.contains(e)).collect();
    names.extend(added.iter().map(|e| e.as_ptr()));
    // SAFETY: a copy of the game's create info with the longer extension list.
    let mut ours: VkDeviceCreateInfo = unsafe { std::ptr::read(info) };
    ours.enabledExtensionCount = names.len() as u32;
    ours.ppEnabledExtensionNames = names.as_ptr();
    // SAFETY: the original, with the game's arguments and our list (alive across the call).
    let result = unsafe { CREATE_DEVICE.get(&flight)(physical, &ours, allocator, out) };
    if result != VK_SUCCESS || out.is_null() {
        log!("vkCreateDevice failed ({result})");
        return result;
    }
    // SAFETY: the device the call just made.
    let device = unsafe { *out };
    let all_there = SHARING.iter().all(|e| present.contains(e) || added.contains(e));
    let luid = luid(physical);
    with_record(|r| {
        r.device = device as u64;
        r.physical = physical as u64;
        r.luid = luid.unwrap_or_default();
        r.luid_valid = u32::from(luid.is_some());
        r.get_device_proc_addr = GET_DEVICE_PROC_ADDR.get(&flight) as usize as u64;
        r.device_state = if all_there { 1 } else { 2 };
    });
    if all_there {
        log!("Vulkan device {device:#x}: sharing extensions added ({})", added.iter().map(|e| e.to_string_lossy()).collect::<Vec<_>>().join(", "));
    } else {
        log!("Vulkan device {device:#x}: the GPU lacks some sharing extensions; nothing can be shared");
    }
    result
}

/// `vkGetDeviceQueue`: the queue, its family noted.
unsafe extern "system" fn get_device_queue(device: VkDevice, family: u32, index: u32, out: *mut VkQueue) {
    let flight = InFlight::enter();
    // SAFETY: the original, with the game's arguments.
    unsafe { GET_DEVICE_QUEUE.get(&flight)(device, family, index, out) };
    if out.is_null() {
        return;
    }
    // SAFETY: the queue the call wrote.
    let queue = unsafe { *out } as u64;
    with_record(|r| {
        let n = r.queue_count as usize;
        if !r.queues[..n].iter().any(|q| q[0] == queue) && n < QUEUES {
            r.queues[n] = [queue, u64::from(family)];
            r.queue_count += 1;
        }
    });
}

/// `vkCreateImage`: a depth image, or a colour render target the width of a screen, also a copy's
/// source, and noted with its format and size.
unsafe extern "system" fn create_image(device: VkDevice, info: *const VkImageCreateInfo, allocator: *const c_void, out: *mut VkImage) -> VkResult {
    let flight = InFlight::enter();
    // SAFETY: a copy of the game's create info.
    let mut ours: VkImageCreateInfo = unsafe { std::ptr::read(info) };
    let depth = ours.usage & IMAGE_USAGE_DEPTH_STENCIL_ATTACHMENT != 0;
    let screen = ours.usage & IMAGE_USAGE_COLOR_ATTACHMENT != 0 && ours.extent[0] >= SCREEN_WIDTH;
    if depth || screen {
        ours.usage |= IMAGE_USAGE_TRANSFER_SRC;
    }
    // SAFETY: the original, with the game's arguments and our usage.
    let result = unsafe { CREATE_IMAGE.get(&flight)(device, &ours, allocator, out) };
    if depth && result == VK_SUCCESS && !out.is_null() {
        // SAFETY: the image the call just made.
        let image = unsafe { *out };
        let entry = DepthImage { image, format: ours.format, width: ours.extent[0], height: ours.extent[1], _pad: 0 };
        with_record(|r| {
            // The newest kept: the oldest goes when the list is full.
            if r.depth_count as usize == DEPTHS {
                r.depths.copy_within(1.., 0);
                r.depth_count -= 1;
            }
            r.depths[r.depth_count as usize] = entry;
            r.depth_count += 1;
        });
        log_first(16, format_args!("Vulkan depth image {image:#x}: {}x{} format {}", ours.extent[0], ours.extent[1], ours.format));
    }
    if screen && result == VK_SUCCESS && !out.is_null() {
        // SAFETY: the image the call just made.
        let image = unsafe { *out };
        let entry = ColorImage { image, format: ours.format, width: ours.extent[0], height: ours.extent[1], _pad: 0 };
        with_record(|r| {
            // The newest kept: the oldest goes when the list is full.
            if r.color_count as usize == COLORS {
                r.colors.copy_within(1.., 0);
                r.color_count -= 1;
            }
            r.colors[r.color_count as usize] = entry;
            r.color_count += 1;
        });
    }
    result
}

/// `vkGetDeviceProcAddr`: our present and swapchain creation in place of the driver's.
unsafe extern "system" fn get_device_proc_addr(device: VkDevice, name: *const c_char) -> usize {
    let flight = InFlight::enter();
    // SAFETY: the original, with the game's arguments.
    let real = unsafe { GET_DEVICE_PROC_ADDR.get(&flight)(device, name) };
    if real == 0 || name.is_null() {
        return real;
    }
    // SAFETY: the game's name, a C string.
    let name = unsafe { CStr::from_ptr(name) };
    if name == c"vkQueuePresentKHR" {
        REAL_PRESENT.store(real, Ordering::Release);
        with_record(|r| r.real_present = real as u64);
        return queue_present as QueuePresentFn as usize;
    }
    if name == c"vkCreateSwapchainKHR" {
        REAL_CREATE_SWAPCHAIN.store(real, Ordering::Release);
        with_record(|r| r.real_create_swapchain = real as u64);
        return create_swapchain as CreateSwapchainFn as usize;
    }
    real
}

/// The driver's function behind one of ours: the one this copy saw, else the loader's export
/// (which dispatches to the driver), so a detour always has a real function to forward to.
fn real_or_loader(real: &AtomicUsize, name: &CStr) -> Option<usize> {
    Some(real.load(Ordering::Acquire)).filter(|&f| f != 0).or_else(|| loader_export(name))
}

/// `vkCreateSwapchainKHR`: its images also a copy's source; the images noted.
unsafe extern "system" fn create_swapchain(device: VkDevice, info: *const VkSwapchainCreateInfoKHR, allocator: *const c_void, out: *mut VkSwapchainKHR) -> VkResult {
    let _flight = InFlight::enter();
    // VK_ERROR_INITIALIZATION_FAILED only when not even the loader has the function.
    let Some(real) = real_or_loader(&REAL_CREATE_SWAPCHAIN, c"vkCreateSwapchainKHR") else { return -3 };
    // SAFETY: the driver's swapchain creation (or the loader's, which dispatches to it).
    let real: CreateSwapchainFn = unsafe { std::mem::transmute(real) };
    // SAFETY: a copy of the game's create info.
    let mut ours: VkSwapchainCreateInfoKHR = unsafe { std::ptr::read(info) };
    ours.imageUsage |= IMAGE_USAGE_TRANSFER_SRC;
    // SAFETY: the original, with the game's arguments and our usage.
    let result = unsafe { real(device, &ours, allocator, out) };
    if result != VK_SUCCESS || out.is_null() {
        log!("vkCreateSwapchainKHR failed ({result})");
        return result;
    }
    // SAFETY: the swapchain the call just made.
    let handle = unsafe { *out };
    let images = swapchain_images(device, handle);
    log!("Vulkan swapchain {handle:#x}: {}x{} format {}, {} images", ours.imageExtent.width, ours.imageExtent.height, ours.imageFormat, images.len());
    let mut chain = Chain { handle, format: ours.imageFormat, width: ours.imageExtent.width, height: ours.imageExtent.height, count: images.len().min(8) as u32, images: [0; 8] };
    chain.images[..chain.count as usize].copy_from_slice(&images[..chain.count as usize]);
    let old = ours.oldSwapchain;
    with_record(|r| {
        let mut kept: Vec<Chain> = r.chains[..r.chain_count as usize].iter().copied().filter(|c| c.handle != old).collect();
        if kept.len() == CHAINS {
            kept.remove(0);
        }
        kept.push(chain);
        r.chain_count = kept.len() as u32;
        r.chains[..kept.len()].copy_from_slice(&kept);
    });
    result
}

fn swapchain_images(device: VkDevice, swapchain: VkSwapchainKHR) -> Vec<VkImage> {
    let Some(lookup) = with_record(|r| r.get_device_proc_addr).filter(|&f| f != 0) else { return Vec::new() };
    // SAFETY: the driver's own lookup on the game's device.
    let get = unsafe { std::mem::transmute::<usize, GetDeviceProcAddrFn>(lookup as usize)(device, c"vkGetSwapchainImagesKHR".as_ptr()) };
    if get == 0 {
        return Vec::new();
    }
    // SAFETY: the driver's function of this name has this type.
    let get: GetSwapchainImagesFn = unsafe { std::mem::transmute(get) };
    let mut count = 0u32;
    // SAFETY: the two-call enumeration on the game's swapchain.
    unsafe {
        if get(device, swapchain, &mut count, std::ptr::null_mut()) != VK_SUCCESS {
            return Vec::new();
        }
        let mut images = vec![0u64; count as usize];
        if get(device, swapchain, &mut count, images.as_mut_ptr()) != VK_SUCCESS {
            return Vec::new();
        }
        images.truncate(count as usize);
        images
    }
}

/// `vkQueuePresentKHR` (the game keeps the first copy's): the frame handed to the newest running
/// copy's handler first (on the game's queue, its thread). It never waits on the record: with the
/// lock busy, the handler last read stands in.
unsafe extern "system" fn queue_present(queue: VkQueue, info: *const VkPresentInfoKHR) -> VkResult {
    let _flight = InFlight::enter();
    // VK_ERROR_INITIALIZATION_FAILED only when not even the loader has the function.
    let Some(real) = real_or_loader(&REAL_PRESENT, c"vkQueuePresentKHR") else { return -3 };
    let handler = match with_record_within(0, |r| r.handler) {
        Some(handler) => {
            LAST_HANDLER.store(handler, Ordering::Relaxed);
            handler
        }
        None => LAST_HANDLER.load(Ordering::Relaxed),
    };
    // SAFETY: the game's present info, read as the API lays it out.
    if handler != 0
        && let Some((swapchain, index)) = unsafe { first_image(info) }
    {
        // SAFETY: the handler a running copy registered (its DLL stays loaded), with the present's
        // own queue, swapchain and image.
        unsafe { std::mem::transmute::<usize, PresentHandler>(handler as usize)(queue, swapchain, index) };
    }
    // SAFETY: the driver's present (or the loader's, which dispatches to it), with the game's arguments.
    unsafe { std::mem::transmute::<usize, QueuePresentFn>(real)(queue, info) }
}

/// The present's first swapchain and image index.
///
/// # Safety
/// `info` is a live `VkPresentInfoKHR`.
unsafe fn first_image(info: *const VkPresentInfoKHR) -> Option<(VkSwapchainKHR, u32)> {
    // SAFETY: the caller's guarantee.
    unsafe {
        let info = info.as_ref()?;
        (info.swapchainCount > 0 && !info.pSwapchains.is_null() && !info.pImageIndices.is_null()).then(|| (*info.pSwapchains, *info.pImageIndices))
    }
}

#[cfg(test)]
mod tests;
