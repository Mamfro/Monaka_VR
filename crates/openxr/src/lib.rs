//! OpenXR for the viewer: the hand-written bindings in [`sys`] behind owning types.
//!
//! The runtime supplies `openxr_loader.dll`; nothing is linked at build time. Child objects borrow
//! their parent ([`Session`] borrows [`Instance`], [`Space`] and [`Swapchain`] borrow `Session`),
//! so the compiler enforces destruction order. Only the D3D11 graphics binding is covered.

pub mod sys;

use std::ffi::c_char;
use std::fmt;
use std::path::Path;
use windows::Win32::Foundation::{FreeLibrary, HMODULE, LUID};
use windows::Win32::Graphics::Direct3D11::{ID3D11Device, ID3D11Texture2D};
use windows::Win32::System::LibraryLoader::{GetProcAddress, LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR, LOAD_LIBRARY_SEARCH_SYSTEM32, LoadLibraryExW};
use windows::core::{HSTRING, Interface, s};

pub use sys::{Extent2Df, Fovf, Posef, Quaternionf, SystemId, Time, Vector3f, ViewConfigurationView};

#[derive(Debug, Clone)]
pub enum Error {
    /// An OpenXR call returned a failure code.
    Call { call: &'static str, result: i32 },
    /// The loader could not be loaded or lacks an entry point.
    Loader(String),
    /// The runtime is missing something the viewer needs.
    Unsupported(&'static str),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Call { call, result } => write!(f, "{call} failed: XrResult {result}"),
            Error::Loader(why) => write!(f, "OpenXR loader: {why}"),
            Error::Unsupported(what) => write!(f, "runtime does not support {what}"),
        }
    }
}

impl std::error::Error for Error {}

pub type Result<T> = std::result::Result<T, Error>;

fn check(result: sys::Result, call: &'static str) -> Result<()> {
    if result >= 0 { Ok(()) } else { Err(Error::Call { call, result }) }
}

fn text(chars: &[c_char]) -> String {
    let bytes: Vec<u8> = chars.iter().take_while(|&&c| c != 0).map(|&c| c as u8).collect();
    String::from_utf8_lossy(&bytes).into_owned()
}

fn copy_name<const N: usize>(target: &mut [c_char; N], name: &str) {
    for (slot, byte) in target.iter_mut().zip(name.bytes().take(N - 1)) {
        *slot = byte as c_char;
    }
}

/// Loads one entry point by its NUL-terminated name.
///
/// # Safety
/// `T` must be the `PFN_xr*` type matching `name`.
unsafe fn load_fn<T: Copy>(get: sys::PFN_xrGetInstanceProcAddr, instance: sys::Instance, name: &'static str) -> Result<T> {
    debug_assert!(name.ends_with('\0'));
    debug_assert_eq!(size_of::<T>(), size_of::<unsafe extern "system" fn()>());
    let mut function: sys::PFN_xrVoidFunction = None;
    // SAFETY: valid NUL-terminated name and out-pointer.
    check(unsafe { get(instance, name.as_ptr().cast(), &mut function) }, "xrGetInstanceProcAddr")?;
    let function = function.ok_or_else(|| Error::Loader(format!("missing {}", name.trim_end_matches('\0'))))?;
    // SAFETY: the caller names the matching function pointer type; all are pointer-sized.
    Ok(unsafe { std::mem::transmute_copy(&function) })
}

macro_rules! functions {
    ($($name:ident: $pfn:ident),* $(,)?) => {
        #[allow(non_snake_case)]
        struct Functions { $($name: sys::$pfn,)* }
        impl Functions {
            fn load(get: sys::PFN_xrGetInstanceProcAddr, instance: sys::Instance) -> Result<Self> {
                // SAFETY: each field's type is the PFN type of the function it is loaded by name.
                unsafe { Ok(Self { $($name: load_fn(get, instance, concat!("xr", stringify!($name), "\0"))?,)* }) }
            }
        }
    };
}

functions! {
    DestroyInstance: PFN_xrDestroyInstance,
    GetInstanceProperties: PFN_xrGetInstanceProperties,
    GetSystem: PFN_xrGetSystem,
    GetSystemProperties: PFN_xrGetSystemProperties,
    GetD3D11GraphicsRequirementsKHR: PFN_xrGetD3D11GraphicsRequirementsKHR,
    EnumerateViewConfigurationViews: PFN_xrEnumerateViewConfigurationViews,
    EnumerateEnvironmentBlendModes: PFN_xrEnumerateEnvironmentBlendModes,
    CreateSession: PFN_xrCreateSession,
    DestroySession: PFN_xrDestroySession,
    CreateReferenceSpace: PFN_xrCreateReferenceSpace,
    DestroySpace: PFN_xrDestroySpace,
    EnumerateSwapchainFormats: PFN_xrEnumerateSwapchainFormats,
    CreateSwapchain: PFN_xrCreateSwapchain,
    DestroySwapchain: PFN_xrDestroySwapchain,
    EnumerateSwapchainImages: PFN_xrEnumerateSwapchainImages,
    AcquireSwapchainImage: PFN_xrAcquireSwapchainImage,
    WaitSwapchainImage: PFN_xrWaitSwapchainImage,
    ReleaseSwapchainImage: PFN_xrReleaseSwapchainImage,
    PollEvent: PFN_xrPollEvent,
    BeginSession: PFN_xrBeginSession,
    EndSession: PFN_xrEndSession,
    RequestExitSession: PFN_xrRequestExitSession,
    WaitFrame: PFN_xrWaitFrame,
    BeginFrame: PFN_xrBeginFrame,
    EndFrame: PFN_xrEndFrame,
    LocateViews: PFN_xrLocateViews,
    StringToPath: PFN_xrStringToPath,
    CreateActionSet: PFN_xrCreateActionSet,
    DestroyActionSet: PFN_xrDestroyActionSet,
    CreateAction: PFN_xrCreateAction,
    DestroyAction: PFN_xrDestroyAction,
    SuggestInteractionProfileBindings: PFN_xrSuggestInteractionProfileBindings,
    AttachSessionActionSets: PFN_xrAttachSessionActionSets,
    CreateActionSpace: PFN_xrCreateActionSpace,
    SyncActions: PFN_xrSyncActions,
    LocateSpace: PFN_xrLocateSpace,
    GetActionStatePose: PFN_xrGetActionStatePose,
    GetActionStateBoolean: PFN_xrGetActionStateBoolean,
    GetActionStateFloat: PFN_xrGetActionStateFloat,
    GetActionStateVector2f: PFN_xrGetActionStateVector2f,
    ApplyHapticFeedback: PFN_xrApplyHapticFeedback,
}

/// The runtime's `openxr_loader.dll`.
pub struct Loader {
    module: HMODULE,
    get: sys::PFN_xrGetInstanceProcAddr,
}

impl Loader {
    /// Loads the loader from an absolute path (searching only its folder and System32 for its
    /// dependencies).
    pub fn load(path: &Path) -> Result<Self> {
        if !path.is_absolute() {
            return Err(Error::Loader("path must be absolute".into()));
        }
        // SAFETY: loads a DLL by absolute path; it is freed on drop.
        let module = unsafe { LoadLibraryExW(&HSTRING::from(path), None, LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR | LOAD_LIBRARY_SEARCH_SYSTEM32) }
            .map_err(|e| Error::Loader(format!("cannot load {}: {e}", path.display())))?;
        // SAFETY: looks up an export of the module we hold.
        let Some(get) = (unsafe { GetProcAddress(module, s!("xrGetInstanceProcAddr")) }) else {
            // SAFETY: frees the module loaded above.
            let _ = unsafe { FreeLibrary(module) };
            return Err(Error::Loader("not an OpenXR loader (no xrGetInstanceProcAddr)".into()));
        };
        // SAFETY: the export has the documented xrGetInstanceProcAddr signature.
        let get = unsafe { std::mem::transmute::<unsafe extern "system" fn() -> isize, sys::PFN_xrGetInstanceProcAddr>(get) };
        Ok(Self { module, get })
    }

    /// Names of the instance extensions the runtime offers.
    pub fn extensions(&self) -> Result<Vec<String>> {
        // SAFETY: the matching PFN type for this name.
        let enumerate: sys::PFN_xrEnumerateInstanceExtensionProperties =
            unsafe { load_fn(self.get, sys::NULL_HANDLE, "xrEnumerateInstanceExtensionProperties\0")? };
        let mut count = 0;
        // SAFETY: two-call idiom with a correctly sized, typed buffer.
        unsafe {
            check(enumerate(std::ptr::null(), 0, &mut count, std::ptr::null_mut()), "xrEnumerateInstanceExtensionProperties")?;
            let blank = sys::ExtensionProperties {
                ty: sys::TYPE_EXTENSION_PROPERTIES,
                next: std::ptr::null_mut(),
                extensionName: [0; sys::MAX_EXTENSION_NAME_SIZE],
                extensionVersion: 0,
            };
            let mut properties = vec![blank; count as usize];
            check(enumerate(std::ptr::null(), count, &mut count, properties.as_mut_ptr()), "xrEnumerateInstanceExtensionProperties")?;
            Ok(properties.iter().take(count as usize).map(|p| text(&p.extensionName)).collect())
        }
    }
}

impl Drop for Loader {
    fn drop(&mut self) {
        // SAFETY: frees the module we loaded; every instance using it has been destroyed first.
        let _ = unsafe { FreeLibrary(self.module) };
    }
}

/// What the viewer needs to know about the headset.
#[derive(Clone, Debug)]
pub struct SystemInfo {
    pub name: String,
    pub max_image_width: u32,
    pub max_image_height: u32,
    pub max_layers: u32,
}

/// An event from [`Instance::poll_event`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event {
    SessionState(i32),
    InstanceLossPending,
    Other,
}

/// Extensions enabled when the runtime offers them (controller profiles beyond the core ones,
/// hand tracking for the fingers, depth beside the eye images, eye tracking).
pub const OPTIONAL_EXTENSIONS: [&std::ffi::CStr; 6] = [
    sys::VALVE_FRAME_CONTROLLER_INTERACTION,
    sys::EXT_HAND_TRACKING,
    sys::EXT_HAND_JOINTS_MOTION_RANGE,
    sys::KHR_COMPOSITION_LAYER_DEPTH,
    sys::EXT_EYE_GAZE_INTERACTION,
    sys::FB_EYE_TRACKING_SOCIAL,
];

/// `XR_EXT_hand_tracking`'s entry points, when the extension is enabled.
#[allow(non_snake_case)]
struct HandFunctions {
    CreateHandTrackerEXT: sys::PFN_xrCreateHandTrackerEXT,
    DestroyHandTrackerEXT: sys::PFN_xrDestroyHandTrackerEXT,
    LocateHandJointsEXT: sys::PFN_xrLocateHandJointsEXT,
}

/// `XR_FB_eye_tracking_social`'s entry points, when the extension is enabled.
#[allow(non_snake_case)]
struct EyeTrackerFunctions {
    CreateEyeTrackerFB: sys::PFN_xrCreateEyeTrackerFB,
    DestroyEyeTrackerFB: sys::PFN_xrDestroyEyeTrackerFB,
    GetEyeGazesFB: sys::PFN_xrGetEyeGazesFB,
}

pub struct Instance {
    handle: sys::Instance,
    fns: Functions,
    hand_fns: Option<HandFunctions>,
    eye_fns: Option<EyeTrackerFunctions>,
    /// The optional extensions enabled (those the runtime offers).
    enabled: Vec<&'static std::ffi::CStr>,
    // Dropped after `Drop::drop` has destroyed the instance.
    _loader: Loader,
}

impl Instance {
    /// Creates an instance with the D3D11 binding enabled, and the optional extensions
    /// ([`OPTIONAL_EXTENSIONS`]) the runtime offers.
    pub fn create(loader: Loader, application: &str) -> Result<Self> {
        let offered = loader.extensions()?;
        let offers = |name: &std::ffi::CStr| offered.iter().any(|e| e.as_bytes() == name.to_bytes());
        if !offers(sys::KHR_D3D11_ENABLE) {
            return Err(Error::Unsupported("Direct3D 11 (XR_KHR_D3D11_enable)"));
        }
        let mut info = sys::ApplicationInfo {
            applicationName: [0; sys::MAX_APPLICATION_NAME_SIZE],
            applicationVersion: 1,
            engineName: [0; sys::MAX_ENGINE_NAME_SIZE],
            engineVersion: 1,
            apiVersion: sys::API_VERSION_1_0,
        };
        copy_name(&mut info.applicationName, application);
        copy_name(&mut info.engineName, "Monaka VR");
        let enabled: Vec<&'static std::ffi::CStr> = OPTIONAL_EXTENSIONS.into_iter().filter(|name| offers(name)).collect();
        let mut extensions = vec![sys::KHR_D3D11_ENABLE.as_ptr()];
        extensions.extend(enabled.iter().map(|name| name.as_ptr()));
        let create = sys::InstanceCreateInfo {
            ty: sys::TYPE_INSTANCE_CREATE_INFO,
            next: std::ptr::null(),
            createFlags: 0,
            applicationInfo: info,
            enabledApiLayerCount: 0,
            enabledApiLayerNames: std::ptr::null(),
            enabledExtensionCount: extensions.len() as u32,
            enabledExtensionNames: extensions.as_ptr(),
        };
        // SAFETY: the matching PFN type for this name.
        let create_instance: sys::PFN_xrCreateInstance = unsafe { load_fn(loader.get, sys::NULL_HANDLE, "xrCreateInstance\0")? };
        let mut handle = sys::NULL_HANDLE;
        // SAFETY: valid create info (pointers outlive the call) and out-pointer.
        check(unsafe { create_instance(&create, &mut handle) }, "xrCreateInstance")?;
        // SAFETY: each field's type is the PFN type of the function it is loaded by name.
        let hand_fns = enabled
            .contains(&sys::EXT_HAND_TRACKING)
            .then(|| unsafe {
                Some(HandFunctions {
                    CreateHandTrackerEXT: load_fn(loader.get, handle, "xrCreateHandTrackerEXT\0").ok()?,
                    DestroyHandTrackerEXT: load_fn(loader.get, handle, "xrDestroyHandTrackerEXT\0").ok()?,
                    LocateHandJointsEXT: load_fn(loader.get, handle, "xrLocateHandJointsEXT\0").ok()?,
                })
            })
            .flatten();
        // SAFETY: as above.
        let eye_fns = enabled
            .contains(&sys::FB_EYE_TRACKING_SOCIAL)
            .then(|| unsafe {
                Some(EyeTrackerFunctions {
                    CreateEyeTrackerFB: load_fn(loader.get, handle, "xrCreateEyeTrackerFB\0").ok()?,
                    DestroyEyeTrackerFB: load_fn(loader.get, handle, "xrDestroyEyeTrackerFB\0").ok()?,
                    GetEyeGazesFB: load_fn(loader.get, handle, "xrGetEyeGazesFB\0").ok()?,
                })
            })
            .flatten();
        match Functions::load(loader.get, handle) {
            Ok(fns) => Ok(Self { handle, fns, hand_fns, eye_fns, enabled, _loader: loader }),
            Err(e) => {
                // SAFETY: destroys the instance just created, before the loader is freed.
                if let Ok(destroy) = unsafe { load_fn::<sys::PFN_xrDestroyInstance>(loader.get, handle, "xrDestroyInstance\0") } {
                    unsafe { destroy(handle) };
                }
                Err(e)
            }
        }
    }

    pub fn runtime_name(&self) -> Result<String> {
        let mut properties = sys::InstanceProperties {
            ty: sys::TYPE_INSTANCE_PROPERTIES,
            next: std::ptr::null_mut(),
            runtimeVersion: 0,
            runtimeName: [0; sys::MAX_RUNTIME_NAME_SIZE],
        };
        // SAFETY: typed out-struct.
        check(unsafe { (self.fns.GetInstanceProperties)(self.handle, &mut properties) }, "xrGetInstanceProperties")?;
        Ok(text(&properties.runtimeName))
    }

    /// The head-mounted display, if one is connected.
    pub fn headset(&self) -> Result<SystemId> {
        let info = sys::SystemGetInfo { ty: sys::TYPE_SYSTEM_GET_INFO, next: std::ptr::null(), formFactor: sys::FORM_FACTOR_HEAD_MOUNTED_DISPLAY };
        let mut system = 0;
        // SAFETY: typed in and out structs.
        check(unsafe { (self.fns.GetSystem)(self.handle, &info, &mut system) }, "xrGetSystem")?;
        Ok(system)
    }

    pub fn system_info(&self, system: SystemId) -> Result<SystemInfo> {
        let mut properties = sys::SystemProperties {
            ty: sys::TYPE_SYSTEM_PROPERTIES,
            next: std::ptr::null_mut(),
            systemId: 0,
            vendorId: 0,
            systemName: [0; sys::MAX_SYSTEM_NAME_SIZE],
            graphicsProperties: Default::default(),
            trackingProperties: Default::default(),
        };
        // SAFETY: typed out-struct.
        check(unsafe { (self.fns.GetSystemProperties)(self.handle, system, &mut properties) }, "xrGetSystemProperties")?;
        let graphics = properties.graphicsProperties;
        Ok(SystemInfo {
            name: text(&properties.systemName),
            max_image_width: graphics.maxSwapchainImageWidth,
            max_image_height: graphics.maxSwapchainImageHeight,
            max_layers: graphics.maxLayerCount,
        })
    }

    /// The GPU the runtime renders on and the minimum D3D feature level it needs.
    pub fn d3d11_requirements(&self, system: SystemId) -> Result<(LUID, i32)> {
        let mut requirements = sys::GraphicsRequirementsD3D11KHR {
            ty: sys::TYPE_GRAPHICS_REQUIREMENTS_D3D11_KHR,
            next: std::ptr::null_mut(),
            adapterLuid: LUID::default(),
            minFeatureLevel: 0,
        };
        // SAFETY: typed out-struct.
        check(unsafe { (self.fns.GetD3D11GraphicsRequirementsKHR)(self.handle, system, &mut requirements) }, "xrGetD3D11GraphicsRequirementsKHR")?;
        Ok((requirements.adapterLuid, requirements.minFeatureLevel))
    }

    /// Recommended and maximum eye image sizes for primary stereo.
    pub fn stereo_views(&self, system: SystemId) -> Result<Vec<ViewConfigurationView>> {
        let blank = sys::ViewConfigurationView {
            ty: sys::TYPE_VIEW_CONFIGURATION_VIEW,
            next: std::ptr::null_mut(),
            recommendedImageRectWidth: 0,
            maxImageRectWidth: 0,
            recommendedImageRectHeight: 0,
            maxImageRectHeight: 0,
            recommendedSwapchainSampleCount: 0,
            maxSwapchainSampleCount: 0,
        };
        let mut views = [blank; 2];
        let mut count = 0;
        let stereo = sys::VIEW_CONFIGURATION_TYPE_PRIMARY_STEREO;
        // SAFETY: typed buffer of the stated capacity.
        check(unsafe { (self.fns.EnumerateViewConfigurationViews)(self.handle, system, stereo, 2, &mut count, views.as_mut_ptr()) }, "xrEnumerateViewConfigurationViews")?;
        Ok(views[..count.min(2) as usize].to_vec())
    }

    pub fn supports_opaque_stereo(&self, system: SystemId) -> Result<bool> {
        let stereo = sys::VIEW_CONFIGURATION_TYPE_PRIMARY_STEREO;
        let mut count = 0;
        // SAFETY: two-call idiom with a typed buffer.
        unsafe {
            check((self.fns.EnumerateEnvironmentBlendModes)(self.handle, system, stereo, 0, &mut count, std::ptr::null_mut()), "xrEnumerateEnvironmentBlendModes")?;
            let mut modes = vec![0i32; count as usize];
            check((self.fns.EnumerateEnvironmentBlendModes)(self.handle, system, stereo, count, &mut count, modes.as_mut_ptr()), "xrEnumerateEnvironmentBlendModes")?;
            Ok(modes.contains(&sys::ENVIRONMENT_BLEND_MODE_OPAQUE))
        }
    }

    /// Whether optional extension `name` ([`OPTIONAL_EXTENSIONS`]) is enabled: the runtime offers it.
    pub fn has_extension(&self, name: &std::ffi::CStr) -> bool {
        self.enabled.contains(&name)
    }

    /// Whether projection layers may carry each eye's depth (`XR_KHR_composition_layer_depth`).
    pub fn depth_layer(&self) -> bool {
        self.has_extension(sys::KHR_COMPOSITION_LAYER_DEPTH)
    }

    /// Asks the system's properties with an extension's properties record chained on them.
    ///
    /// # Safety
    /// `chained` must point to a writable extension properties record (type set, `next` null) that
    /// outlives the call.
    unsafe fn system_properties_with(&self, system: SystemId, chained: *mut std::ffi::c_void) -> bool {
        let mut properties = sys::SystemProperties {
            ty: sys::TYPE_SYSTEM_PROPERTIES,
            next: chained,
            systemId: 0,
            vendorId: 0,
            systemName: [0; sys::MAX_SYSTEM_NAME_SIZE],
            graphicsProperties: Default::default(),
            trackingProperties: Default::default(),
        };
        // SAFETY: typed out-structs, the chained one valid per the caller.
        unsafe { (self.fns.GetSystemProperties)(self.handle, system, &mut properties) >= 0 }
    }

    /// Whether the headset system tracks hands (`XR_EXT_hand_tracking` enabled and supported).
    pub fn hand_tracking(&self, system: SystemId) -> bool {
        if self.hand_fns.is_none() {
            return false;
        }
        let mut hands = sys::SystemHandTrackingPropertiesEXT { ty: sys::TYPE_SYSTEM_HAND_TRACKING_PROPERTIES_EXT, next: std::ptr::null_mut(), supportsHandTracking: 0 };
        // SAFETY: a typed properties record that outlives the call.
        let ok = unsafe { self.system_properties_with(system, (&raw mut hands).cast()) };
        ok && hands.supportsHandTracking != 0
    }

    /// Whether the headset system gives eye gaze as a pose action (`XR_EXT_eye_gaze_interaction`
    /// enabled and supported).
    pub fn eye_gaze(&self, system: SystemId) -> bool {
        if !self.has_extension(sys::EXT_EYE_GAZE_INTERACTION) {
            return false;
        }
        let mut gaze = sys::SystemEyeGazeInteractionPropertiesEXT {
            ty: sys::TYPE_SYSTEM_EYE_GAZE_INTERACTION_PROPERTIES_EXT,
            next: std::ptr::null_mut(),
            supportsEyeGazeInteraction: 0,
        };
        // SAFETY: a typed properties record that outlives the call.
        let ok = unsafe { self.system_properties_with(system, (&raw mut gaze).cast()) };
        ok && gaze.supportsEyeGazeInteraction != 0
    }

    /// Whether the headset system tracks the eyes through Meta's eye tracker
    /// (`XR_FB_eye_tracking_social` enabled and supported).
    pub fn eye_tracking_fb(&self, system: SystemId) -> bool {
        if self.eye_fns.is_none() {
            return false;
        }
        let mut eyes = sys::SystemEyeTrackingPropertiesFB { ty: sys::TYPE_SYSTEM_EYE_TRACKING_PROPERTIES_FB, next: std::ptr::null_mut(), supportsEyeTracking: 0 };
        // SAFETY: a typed properties record that outlives the call.
        let ok = unsafe { self.system_properties_with(system, (&raw mut eyes).cast()) };
        ok && eyes.supportsEyeTracking != 0
    }

    pub fn create_session(&self, system: SystemId, device: &ID3D11Device) -> Result<Session<'_>> {
        let binding = sys::GraphicsBindingD3D11KHR { ty: sys::TYPE_GRAPHICS_BINDING_D3D11_KHR, next: std::ptr::null(), device: device.as_raw() };
        let info = sys::SessionCreateInfo {
            ty: sys::TYPE_SESSION_CREATE_INFO,
            next: (&raw const binding).cast(),
            createFlags: 0,
            systemId: system,
        };
        let mut handle = sys::NULL_HANDLE;
        // SAFETY: the binding outlives the call; the runtime takes its own device reference.
        check(unsafe { (self.fns.CreateSession)(self.handle, &info, &mut handle) }, "xrCreateSession")?;
        Ok(Session { instance: self, handle })
    }

    /// The `XrPath` for a path string such as `/user/hand/right`.
    pub fn path(&self, text: &str) -> Result<sys::Path> {
        let text = std::ffi::CString::new(text).map_err(|_| Error::Unsupported("a path with a NUL in it"))?;
        let mut path = 0;
        // SAFETY: NUL-terminated string and out-pointer.
        check(unsafe { (self.fns.StringToPath)(self.handle, text.as_ptr(), &mut path) }, "xrStringToPath")?;
        Ok(path)
    }

    pub fn action_set(&self, name: &str, localized: &str) -> Result<ActionSet<'_>> {
        let mut info = sys::ActionSetCreateInfo {
            ty: sys::TYPE_ACTION_SET_CREATE_INFO,
            next: std::ptr::null(),
            actionSetName: [0; sys::MAX_ACTION_SET_NAME_SIZE],
            localizedActionSetName: [0; sys::MAX_LOCALIZED_ACTION_SET_NAME_SIZE],
            priority: 0,
        };
        copy_name(&mut info.actionSetName, name);
        copy_name(&mut info.localizedActionSetName, localized);
        let mut handle = sys::NULL_HANDLE;
        // SAFETY: typed in and out.
        check(unsafe { (self.fns.CreateActionSet)(self.handle, &info, &mut handle) }, "xrCreateActionSet")?;
        Ok(ActionSet { instance: self, handle })
    }

    /// Suggests where `profile` (for example `/interaction_profiles/oculus/touch_controller`) binds
    /// each action. A runtime that does not know the profile refuses; callers try several.
    pub fn suggest_bindings(&self, profile: &str, bindings: &[(&Action<'_>, &str)]) -> Result<()> {
        let mut suggested = Vec::with_capacity(bindings.len());
        for (action, path) in bindings {
            suggested.push(sys::ActionSuggestedBinding { action: action.handle, binding: self.path(path)? });
        }
        let info = sys::InteractionProfileSuggestedBinding {
            ty: sys::TYPE_INTERACTION_PROFILE_SUGGESTED_BINDING,
            next: std::ptr::null(),
            interactionProfile: self.path(profile)?,
            countSuggestedBindings: suggested.len() as u32,
            suggestedBindings: suggested.as_ptr(),
        };
        // SAFETY: the binding list outlives the call.
        check(unsafe { (self.fns.SuggestInteractionProfileBindings)(self.handle, &info) }, "xrSuggestInteractionProfileBindings")
    }

    /// The next queued event, if any.
    pub fn poll_event(&self) -> Result<Option<Event>> {
        let mut buffer = sys::EventDataBuffer { ty: sys::TYPE_EVENT_DATA_BUFFER, next: std::ptr::null(), varying: [0; 4000] };
        // SAFETY: a full-size event buffer.
        let result = unsafe { (self.fns.PollEvent)(self.handle, &mut buffer) };
        if result == sys::EVENT_UNAVAILABLE {
            return Ok(None);
        }
        check(result, "xrPollEvent")?;
        Ok(Some(match buffer.ty {
            sys::TYPE_EVENT_DATA_SESSION_STATE_CHANGED => {
                // SAFETY: the runtime wrote this event type into the buffer, which is large enough.
                let changed = unsafe { &*(&raw const buffer).cast::<sys::EventDataSessionStateChanged>() };
                Event::SessionState(changed.state)
            }
            sys::TYPE_EVENT_DATA_INSTANCE_LOSS_PENDING => Event::InstanceLossPending,
            _ => Event::Other,
        }))
    }
}

impl Drop for Instance {
    fn drop(&mut self) {
        // SAFETY: destroys our instance once; every child borrowed it and is gone.
        unsafe { (self.fns.DestroyInstance)(self.handle) };
    }
}

/// Predicted timing of the frame being rendered.
#[derive(Clone, Copy, Debug)]
pub struct FrameTiming {
    pub display_time: Time,
    pub should_render: bool,
}

pub struct Session<'i> {
    instance: &'i Instance,
    handle: sys::Session,
}

impl Session<'_> {
    fn fns(&self) -> &Functions {
        &self.instance.fns
    }

    /// A reference space (`sys::REFERENCE_SPACE_TYPE_*`) with an identity offset.
    pub fn reference_space(&self, kind: i32) -> Result<Space<'_>> {
        let info = sys::ReferenceSpaceCreateInfo {
            ty: sys::TYPE_REFERENCE_SPACE_CREATE_INFO,
            next: std::ptr::null(),
            referenceSpaceType: kind,
            poseInReferenceSpace: Posef::IDENTITY,
        };
        let mut handle = sys::NULL_HANDLE;
        // SAFETY: typed in and out.
        check(unsafe { (self.fns().CreateReferenceSpace)(self.handle, &info, &mut handle) }, "xrCreateReferenceSpace")?;
        Ok(Space { instance: self.instance, handle })
    }

    /// Image formats the runtime accepts (`DXGI_FORMAT` values), in its order of preference.
    pub fn swapchain_formats(&self) -> Result<Vec<i64>> {
        let mut count = 0;
        // SAFETY: two-call idiom with a typed buffer.
        unsafe {
            check((self.fns().EnumerateSwapchainFormats)(self.handle, 0, &mut count, std::ptr::null_mut()), "xrEnumerateSwapchainFormats")?;
            let mut formats = vec![0i64; count as usize];
            check((self.fns().EnumerateSwapchainFormats)(self.handle, count, &mut count, formats.as_mut_ptr()), "xrEnumerateSwapchainFormats")?;
            formats.truncate(count as usize);
            Ok(formats)
        }
    }

    /// A swapchain filled by copies (transfer destination) and sampled by the compositor.
    pub fn create_swapchain(&self, format: i64, width: u32, height: u32) -> Result<Swapchain<'_>> {
        self.swapchain(format, width, height, sys::SWAPCHAIN_USAGE_TRANSFER_DST_BIT | sys::SWAPCHAIN_USAGE_SAMPLED_BIT)
    }

    /// A depth swapchain (a depth format such as `DXGI_FORMAT_D32_FLOAT`) filled by copies, for
    /// [`DepthView`]s.
    pub fn create_depth_swapchain(&self, format: i64, width: u32, height: u32) -> Result<Swapchain<'_>> {
        self.swapchain(format, width, height, sys::SWAPCHAIN_USAGE_TRANSFER_DST_BIT | sys::SWAPCHAIN_USAGE_DEPTH_STENCIL_ATTACHMENT_BIT)
    }

    fn swapchain(&self, format: i64, width: u32, height: u32, usage: sys::Flags64) -> Result<Swapchain<'_>> {
        let info = sys::SwapchainCreateInfo {
            ty: sys::TYPE_SWAPCHAIN_CREATE_INFO,
            next: std::ptr::null(),
            createFlags: 0,
            usageFlags: usage,
            format,
            sampleCount: 1,
            width,
            height,
            faceCount: 1,
            arraySize: 1,
            mipCount: 1,
        };
        let mut handle = sys::NULL_HANDLE;
        // SAFETY: typed in and out.
        check(unsafe { (self.fns().CreateSwapchain)(self.handle, &info, &mut handle) }, "xrCreateSwapchain")?;
        let mut swapchain = Swapchain { instance: self.instance, handle, images: Vec::new(), width, height };
        let mut count = 0;
        // SAFETY: two-call idiom; D3D11 swapchain images are XrSwapchainImageD3D11KHR records. The
        // runtime owns the textures, so each is AddRef'd as it is wrapped.
        unsafe {
            check((self.fns().EnumerateSwapchainImages)(handle, 0, &mut count, std::ptr::null_mut()), "xrEnumerateSwapchainImages")?;
            let blank = sys::SwapchainImageD3D11KHR { ty: sys::TYPE_SWAPCHAIN_IMAGE_D3D11_KHR, next: std::ptr::null_mut(), texture: std::ptr::null_mut() };
            let mut images = vec![blank; count as usize];
            check((self.fns().EnumerateSwapchainImages)(handle, count, &mut count, images.as_mut_ptr().cast()), "xrEnumerateSwapchainImages")?;
            for image in images.iter().take(count as usize) {
                let texture = ID3D11Texture2D::from_raw_borrowed(&image.texture).cloned().ok_or(Error::Unsupported("null swapchain image"))?;
                swapchain.images.push(texture);
            }
        }
        Ok(swapchain)
    }

    pub fn begin(&self) -> Result<()> {
        let info = sys::SessionBeginInfo {
            ty: sys::TYPE_SESSION_BEGIN_INFO,
            next: std::ptr::null(),
            primaryViewConfigurationType: sys::VIEW_CONFIGURATION_TYPE_PRIMARY_STEREO,
        };
        // SAFETY: typed info.
        check(unsafe { (self.fns().BeginSession)(self.handle, &info) }, "xrBeginSession")
    }

    pub fn end(&self) -> Result<()> {
        // SAFETY: session handle we own.
        check(unsafe { (self.fns().EndSession)(self.handle) }, "xrEndSession")
    }

    pub fn request_exit(&self) -> Result<()> {
        // SAFETY: session handle we own.
        check(unsafe { (self.fns().RequestExitSession)(self.handle) }, "xrRequestExitSession")
    }

    /// Blocks until the runtime wants the next frame.
    pub fn wait_frame(&self) -> Result<FrameTiming> {
        let info = sys::BaseInfo { ty: sys::TYPE_FRAME_WAIT_INFO, next: std::ptr::null() };
        let mut state = sys::FrameState { ty: sys::TYPE_FRAME_STATE, next: std::ptr::null_mut(), predictedDisplayTime: 0, predictedDisplayPeriod: 0, shouldRender: 0 };
        // SAFETY: typed in and out.
        check(unsafe { (self.fns().WaitFrame)(self.handle, &info, &mut state) }, "xrWaitFrame")?;
        Ok(FrameTiming { display_time: state.predictedDisplayTime, should_render: state.shouldRender != 0 })
    }

    pub fn begin_frame(&self) -> Result<()> {
        let info = sys::BaseInfo { ty: sys::TYPE_FRAME_BEGIN_INFO, next: std::ptr::null() };
        // SAFETY: typed info.
        check(unsafe { (self.fns().BeginFrame)(self.handle, &info) }, "xrBeginFrame")
    }

    /// Submits the frame's layers (none when the frame should not render).
    pub fn end_frame(&self, display_time: Time, layers: &[Layer<'_>]) -> Result<()> {
        struct RawProjection {
            layer: sys::CompositionLayerProjection,
            views: [sys::CompositionLayerProjectionView; 2],
            depths: [sys::CompositionLayerDepthInfoKHR; 2],
        }
        let depth_layer = self.instance.depth_layer();
        // Boxed so the pointers handed to the runtime stay put while the lists grow.
        let mut projections: Vec<Box<RawProjection>> = Vec::new();
        let mut quads: Vec<Box<sys::CompositionLayerQuad>> = Vec::new();
        let mut headers: Vec<*const sys::CompositionLayerBaseHeader> = Vec::with_capacity(layers.len());
        for layer in layers {
            match layer {
                Layer::Projection { space, views } => {
                    let view = |v: &ProjectionView<'_>| sys::CompositionLayerProjectionView {
                        ty: sys::TYPE_COMPOSITION_LAYER_PROJECTION_VIEW,
                        next: std::ptr::null(),
                        pose: v.pose,
                        fov: v.fov,
                        subImage: v.swapchain.full_image(),
                    };
                    let depth = |v: &ProjectionView<'_>| {
                        let (image, near, far) = match &v.depth {
                            Some(d) => (d.swapchain.sub_image(d.rect), d.near_z, d.far_z),
                            None => (v.swapchain.full_image(), 0.0, 0.0),
                        };
                        sys::CompositionLayerDepthInfoKHR {
                            ty: sys::TYPE_COMPOSITION_LAYER_DEPTH_INFO_KHR,
                            next: std::ptr::null(),
                            subImage: image,
                            minDepth: 0.0,
                            maxDepth: 1.0,
                            nearZ: near,
                            farZ: far,
                        }
                    };
                    let mut raw = Box::new(RawProjection {
                        layer: sys::CompositionLayerProjection {
                            ty: sys::TYPE_COMPOSITION_LAYER_PROJECTION,
                            next: std::ptr::null(),
                            layerFlags: 0,
                            space: space.handle,
                            viewCount: 2,
                            views: std::ptr::null(),
                        },
                        views: [view(&views[0]), view(&views[1])],
                        depths: [depth(&views[0]), depth(&views[1])],
                    });
                    // Each eye's depth goes on its view only when the extension is enabled.
                    for (eye, view) in views.iter().enumerate() {
                        if depth_layer && view.depth.is_some() {
                            raw.views[eye].next = (&raw const raw.depths[eye]).cast();
                        }
                    }
                    raw.layer.views = raw.views.as_ptr();
                    headers.push((&raw const raw.layer).cast());
                    projections.push(raw);
                }
                Layer::Quad { space, eye, swapchain, pose, size } => {
                    let raw = Box::new(sys::CompositionLayerQuad {
                        ty: sys::TYPE_COMPOSITION_LAYER_QUAD,
                        next: std::ptr::null(),
                        layerFlags: 0,
                        space: space.handle,
                        eyeVisibility: if *eye == 0 { sys::EYE_VISIBILITY_LEFT } else { sys::EYE_VISIBILITY_RIGHT },
                        subImage: swapchain.full_image(),
                        pose: *pose,
                        size: *size,
                    });
                    headers.push((&raw const *raw).cast());
                    quads.push(raw);
                }
                Layer::Overlay { space, swapchain, pose, size } => {
                    let raw = Box::new(sys::CompositionLayerQuad {
                        ty: sys::TYPE_COMPOSITION_LAYER_QUAD,
                        next: std::ptr::null(),
                        layerFlags: sys::COMPOSITION_LAYER_BLEND_TEXTURE_SOURCE_ALPHA_BIT,
                        space: space.handle,
                        eyeVisibility: sys::EYE_VISIBILITY_BOTH,
                        subImage: swapchain.full_image(),
                        pose: *pose,
                        size: *size,
                    });
                    headers.push((&raw const *raw).cast());
                    quads.push(raw);
                }
            }
        }
        let info = sys::FrameEndInfo {
            ty: sys::TYPE_FRAME_END_INFO,
            next: std::ptr::null(),
            displayTime: display_time,
            environmentBlendMode: sys::ENVIRONMENT_BLEND_MODE_OPAQUE,
            layerCount: headers.len() as u32,
            layers: headers.as_ptr(),
        };
        // SAFETY: every layer record and the header list live until after the call.
        check(unsafe { (self.fns().EndFrame)(self.handle, &info) }, "xrEndFrame")
    }

    /// Both eye views at `time` in `space`, if orientation and position are tracked.
    pub fn locate_views(&self, space: &Space<'_>, time: Time) -> Result<Option<[sys::View; 2]>> {
        let info = sys::ViewLocateInfo {
            ty: sys::TYPE_VIEW_LOCATE_INFO,
            next: std::ptr::null(),
            viewConfigurationType: sys::VIEW_CONFIGURATION_TYPE_PRIMARY_STEREO,
            displayTime: time,
            space: space.handle,
        };
        let mut state = sys::ViewState { ty: sys::TYPE_VIEW_STATE, next: std::ptr::null_mut(), viewStateFlags: 0 };
        let blank = sys::View { ty: sys::TYPE_VIEW, next: std::ptr::null_mut(), pose: Posef::IDENTITY, fov: Fovf::default() };
        let mut views = [blank; 2];
        let mut count = 0;
        // SAFETY: typed buffers of the stated capacity.
        check(unsafe { (self.fns().LocateViews)(self.handle, &info, &mut state, 2, &mut count, views.as_mut_ptr()) }, "xrLocateViews")?;
        let tracked = sys::VIEW_STATE_ORIENTATION_VALID_BIT | sys::VIEW_STATE_POSITION_VALID_BIT;
        Ok((count == 2 && state.viewStateFlags & tracked == tracked).then_some(views))
    }

    /// Makes `sets` the session's action sets (once, before the first [`Session::sync`]).
    pub fn attach(&self, sets: &[&ActionSet<'_>]) -> Result<()> {
        let handles: Vec<sys::ActionSet> = sets.iter().map(|s| s.handle).collect();
        let info = sys::SessionActionSetsAttachInfo {
            ty: sys::TYPE_SESSION_ACTION_SETS_ATTACH_INFO,
            next: std::ptr::null(),
            countActionSets: handles.len() as u32,
            actionSets: handles.as_ptr(),
        };
        // SAFETY: the handle list outlives the call.
        check(unsafe { (self.fns().AttachSessionActionSets)(self.handle, &info) }, "xrAttachSessionActionSets")
    }

    /// A space that follows pose action `action` for `subaction` (a hand path).
    pub fn action_space(&self, action: &Action<'_>, subaction: sys::Path) -> Result<Space<'_>> {
        let info = sys::ActionSpaceCreateInfo {
            ty: sys::TYPE_ACTION_SPACE_CREATE_INFO,
            next: std::ptr::null(),
            action: action.handle,
            subactionPath: subaction,
            poseInActionSpace: Posef::IDENTITY,
        };
        let mut handle = sys::NULL_HANDLE;
        // SAFETY: typed in and out.
        check(unsafe { (self.fns().CreateActionSpace)(self.handle, &info, &mut handle) }, "xrCreateActionSpace")?;
        Ok(Space { instance: self.instance, handle })
    }

    /// Reads the controllers into `set`'s actions. Succeeds without updating them while the
    /// session is not focused.
    pub fn sync(&self, set: &ActionSet<'_>) -> Result<()> {
        let active = [sys::ActiveActionSet { actionSet: set.handle, subactionPath: sys::NULL_HANDLE }];
        let info = sys::ActionsSyncInfo { ty: sys::TYPE_ACTIONS_SYNC_INFO, next: std::ptr::null(), countActiveActionSets: 1, activeActionSets: active.as_ptr() };
        // SAFETY: the active set list outlives the call.
        check(unsafe { (self.fns().SyncActions)(self.handle, &info) }, "xrSyncActions")
    }

    /// Buzzes the controller of `subaction` through vibration action `action`: `amplitude` 0 to 1
    /// for `seconds`, at the runtime's own frequency.
    pub fn vibrate(&self, action: &Action<'_>, subaction: sys::Path, amplitude: f32, seconds: f32) -> Result<()> {
        let info = sys::HapticActionInfo { ty: sys::TYPE_HAPTIC_ACTION_INFO, next: std::ptr::null(), action: action.handle, subactionPath: subaction };
        let vibration = sys::HapticVibration {
            ty: sys::TYPE_HAPTIC_VIBRATION,
            next: std::ptr::null(),
            duration: (seconds * 1e9) as sys::Duration,
            frequency: sys::FREQUENCY_UNSPECIFIED,
            amplitude: amplitude.clamp(0.0, 1.0),
        };
        // SAFETY: typed in; the vibration is the haptic header the call takes.
        check(unsafe { (self.fns().ApplyHapticFeedback)(self.handle, &info, &vibration) }, "xrApplyHapticFeedback")
    }

    /// Whether pose action `action` is bound to a live source for `subaction`.
    pub fn pose_active(&self, action: &Action<'_>, subaction: sys::Path) -> Result<bool> {
        let info = sys::ActionStateGetInfo { ty: sys::TYPE_ACTION_STATE_GET_INFO, next: std::ptr::null(), action: action.handle, subactionPath: subaction };
        let mut state = sys::ActionStatePose { ty: sys::TYPE_ACTION_STATE_POSE, next: std::ptr::null_mut(), isActive: 0 };
        // SAFETY: typed in and out.
        check(unsafe { (self.fns().GetActionStatePose)(self.handle, &info, &mut state) }, "xrGetActionStatePose")?;
        Ok(state.isActive != 0)
    }

    /// A button action's state (any of its bound sources), false when unbound or inactive.
    pub fn boolean(&self, action: &Action<'_>) -> Result<bool> {
        let info = sys::ActionStateGetInfo { ty: sys::TYPE_ACTION_STATE_GET_INFO, next: std::ptr::null(), action: action.handle, subactionPath: sys::NULL_HANDLE };
        let mut state = sys::ActionStateBoolean { ty: sys::TYPE_ACTION_STATE_BOOLEAN, next: std::ptr::null_mut(), currentState: 0, changedSinceLastSync: 0, lastChangeTime: 0, isActive: 0 };
        // SAFETY: typed in and out.
        check(unsafe { (self.fns().GetActionStateBoolean)(self.handle, &info, &mut state) }, "xrGetActionStateBoolean")?;
        Ok(state.isActive != 0 && state.currentState != 0)
    }

    /// An analog action's value (a trigger, 0..1), 0 when unbound or inactive.
    pub fn float(&self, action: &Action<'_>) -> Result<f32> {
        let info = sys::ActionStateGetInfo { ty: sys::TYPE_ACTION_STATE_GET_INFO, next: std::ptr::null(), action: action.handle, subactionPath: sys::NULL_HANDLE };
        let mut state = sys::ActionStateFloat { ty: sys::TYPE_ACTION_STATE_FLOAT, next: std::ptr::null_mut(), currentState: 0.0, changedSinceLastSync: 0, lastChangeTime: 0, isActive: 0 };
        // SAFETY: typed in and out.
        check(unsafe { (self.fns().GetActionStateFloat)(self.handle, &info, &mut state) }, "xrGetActionStateFloat")?;
        Ok(if state.isActive != 0 { state.currentState } else { 0.0 })
    }

    /// A two-axis action's value (a stick, -1..1, up positive), centred when unbound or inactive.
    pub fn vector2(&self, action: &Action<'_>) -> Result<sys::Vector2f> {
        let info = sys::ActionStateGetInfo { ty: sys::TYPE_ACTION_STATE_GET_INFO, next: std::ptr::null(), action: action.handle, subactionPath: sys::NULL_HANDLE };
        let mut state = sys::ActionStateVector2f {
            ty: sys::TYPE_ACTION_STATE_VECTOR2F,
            next: std::ptr::null_mut(),
            currentState: sys::Vector2f::default(),
            changedSinceLastSync: 0,
            lastChangeTime: 0,
            isActive: 0,
        };
        // SAFETY: typed in and out.
        check(unsafe { (self.fns().GetActionStateVector2f)(self.handle, &info, &mut state) }, "xrGetActionStateVector2f")?;
        Ok(if state.isActive != 0 { state.currentState } else { sys::Vector2f::default() })
    }

    /// Where `space` is in `base` at `time`, if both orientation and position are known.
    pub fn locate(&self, space: &Space<'_>, base: &Space<'_>, time: Time) -> Result<Option<Posef>> {
        let mut location = sys::SpaceLocation { ty: sys::TYPE_SPACE_LOCATION, next: std::ptr::null_mut(), locationFlags: 0, pose: Posef::IDENTITY };
        // SAFETY: typed out-struct.
        check(unsafe { (self.fns().LocateSpace)(space.handle, base.handle, time, &mut location) }, "xrLocateSpace")?;
        let valid = sys::SPACE_LOCATION_ORIENTATION_VALID_BIT | sys::SPACE_LOCATION_POSITION_VALID_BIT;
        Ok((location.locationFlags & valid == valid).then_some(location.pose))
    }

    /// Where gaze action space `space` (`XR_EXT_eye_gaze_interaction`) is in `base` at `time`, and
    /// when the runtime sampled it (0 if it does not say), while the eyes are tracked. The position
    /// is zero when only the direction is known.
    pub fn locate_gaze(&self, space: &Space<'_>, base: &Space<'_>, time: Time) -> Result<Option<(Posef, Time)>> {
        let mut sample = sys::EyeGazeSampleTimeEXT { ty: sys::TYPE_EYE_GAZE_SAMPLE_TIME_EXT, next: std::ptr::null_mut(), time: 0 };
        let chained = if self.instance.has_extension(sys::EXT_EYE_GAZE_INTERACTION) { (&raw mut sample).cast() } else { std::ptr::null_mut() };
        let mut location = sys::SpaceLocation { ty: sys::TYPE_SPACE_LOCATION, next: chained, locationFlags: 0, pose: Posef::IDENTITY };
        // SAFETY: typed out-structs, the chained one outliving the call.
        check(unsafe { (self.fns().LocateSpace)(space.handle, base.handle, time, &mut location) }, "xrLocateSpace")?;
        let looking = sys::SPACE_LOCATION_ORIENTATION_VALID_BIT | sys::SPACE_LOCATION_ORIENTATION_TRACKED_BIT;
        if location.locationFlags & looking != looking {
            return Ok(None);
        }
        let mut pose = location.pose;
        if location.locationFlags & sys::SPACE_LOCATION_POSITION_VALID_BIT == 0 {
            pose.position = Vector3f::default();
        }
        Ok(Some((pose, sample.time)))
    }

    /// Meta's eye tracker (`XR_FB_eye_tracking_social`); fails without it.
    pub fn eye_tracker(&self) -> Result<EyeTracker<'_>> {
        let fns = self.instance.eye_fns.as_ref().ok_or(Error::Unsupported("eye tracking (XR_FB_eye_tracking_social)"))?;
        let info = sys::EyeTrackerCreateInfoFB { ty: sys::TYPE_EYE_TRACKER_CREATE_INFO_FB, next: std::ptr::null() };
        let mut handle = 0;
        // SAFETY: typed in and out.
        check(unsafe { (fns.CreateEyeTrackerFB)(self.handle, &info, &mut handle) }, "xrCreateEyeTrackerFB")?;
        Ok(EyeTracker { instance: self.instance, handle })
    }
}

/// Both eyes' gaze (`XR_FB_eye_tracking_social`).
pub struct EyeTracker<'s> {
    instance: &'s Instance,
    handle: sys::EyeTrackerFB,
}

impl EyeTracker<'_> {
    /// Each eye's gaze pose in `base` at `time` (left, right; `None` for an eye not tracked) and
    /// when the runtime sampled them.
    pub fn gazes(&self, base: &Space<'_>, time: Time) -> Result<([Option<Posef>; 2], Time)> {
        let Some(fns) = self.instance.eye_fns.as_ref() else { return Ok(([None; 2], 0)) };
        let info = sys::EyeGazesInfoFB { ty: sys::TYPE_EYE_GAZES_INFO_FB, next: std::ptr::null(), baseSpace: base.handle, time };
        let mut gazes = sys::EyeGazesFB { ty: sys::TYPE_EYE_GAZES_FB, next: std::ptr::null_mut(), gaze: [sys::EyeGazeFB::default(); sys::EYE_POSITION_COUNT_FB], time: 0 };
        // SAFETY: typed in and out.
        check(unsafe { (fns.GetEyeGazesFB)(self.handle, &info, &mut gazes) }, "xrGetEyeGazesFB")?;
        Ok((gazes.gaze.map(|g| (g.isValid != 0).then_some(g.gazePose)), gazes.time))
    }
}

impl Drop for EyeTracker<'_> {
    fn drop(&mut self) {
        if let Some(fns) = self.instance.eye_fns.as_ref() {
            // SAFETY: destroys our tracker once.
            unsafe { (fns.DestroyEyeTrackerFB)(self.handle) };
        }
    }
}

impl Session<'_> {
    /// A tracker for hand `hand` (0 left, 1 right); fails without hand tracking.
    pub fn hand_tracker(&self, hand: usize) -> Result<HandTracker<'_>> {
        let fns = self.instance.hand_fns.as_ref().ok_or(Error::Unsupported("hand tracking (XR_EXT_hand_tracking)"))?;
        let info = sys::HandTrackerCreateInfoEXT {
            ty: sys::TYPE_HAND_TRACKER_CREATE_INFO_EXT,
            next: std::ptr::null(),
            hand: if hand == 0 { sys::HAND_LEFT_EXT } else { sys::HAND_RIGHT_EXT },
            handJointSet: sys::HAND_JOINT_SET_DEFAULT_EXT,
        };
        let mut handle = 0;
        // SAFETY: typed in and out.
        check(unsafe { (fns.CreateHandTrackerEXT)(self.handle, &info, &mut handle) }, "xrCreateHandTrackerEXT")?;
        Ok(HandTracker { instance: self.instance, handle })
    }
}

/// One hand's joints (`XR_EXT_hand_tracking`).
pub struct HandTracker<'s> {
    instance: &'s Instance,
    handle: sys::HandTrackerEXT,
}

impl HandTracker<'_> {
    /// The hand's 26 joint orientations in `base` at `time` (x, y, z, w; `XrHandJointEXT` order),
    /// with the fingers free to close past the controller where the runtime allows it; `None`
    /// while the hand is not tracked or any joint's orientation is unknown.
    pub fn orientations(&self, base: &Space<'_>, time: Time) -> Result<Option<[[f32; 4]; 26]>> {
        let Some(fns) = self.instance.hand_fns.as_ref() else { return Ok(None) };
        let range = sys::HandJointsMotionRangeInfoEXT {
            ty: sys::TYPE_HAND_JOINTS_MOTION_RANGE_INFO_EXT,
            next: std::ptr::null(),
            handJointsMotionRange: sys::HAND_JOINTS_MOTION_RANGE_UNOBSTRUCTED_EXT,
        };
        let info = sys::HandJointsLocateInfoEXT {
            ty: sys::TYPE_HAND_JOINTS_LOCATE_INFO_EXT,
            next: if self.instance.has_extension(sys::EXT_HAND_JOINTS_MOTION_RANGE) { (&raw const range).cast() } else { std::ptr::null() },
            baseSpace: base.handle,
            time,
        };
        let mut joints = [sys::HandJointLocationEXT::default(); 26];
        let mut locations = sys::HandJointLocationsEXT {
            ty: sys::TYPE_HAND_JOINT_LOCATIONS_EXT,
            next: std::ptr::null_mut(),
            isActive: 0,
            jointCount: sys::HAND_JOINT_COUNT_EXT,
            jointLocations: joints.as_mut_ptr(),
        };
        // SAFETY: typed in and out; the joint array has the count given and outlives the call.
        check(unsafe { (fns.LocateHandJointsEXT)(self.handle, &info, &mut locations) }, "xrLocateHandJointsEXT")?;
        if locations.isActive == 0 || !joints.iter().all(|j| j.locationFlags & sys::SPACE_LOCATION_ORIENTATION_VALID_BIT != 0) {
            return Ok(None);
        }
        Ok(Some(joints.map(|j| [j.pose.orientation.x, j.pose.orientation.y, j.pose.orientation.z, j.pose.orientation.w])))
    }
}

impl Drop for HandTracker<'_> {
    fn drop(&mut self) {
        if let Some(fns) = self.instance.hand_fns.as_ref() {
            // SAFETY: destroys our tracker once.
            unsafe { (fns.DestroyHandTrackerEXT)(self.handle) };
        }
    }
}

impl Drop for Session<'_> {
    fn drop(&mut self) {
        // SAFETY: destroys our session once; its spaces and swapchains borrowed it and are gone.
        unsafe { (self.fns().DestroySession)(self.handle) };
    }
}

pub struct Space<'s> {
    instance: &'s Instance,
    handle: sys::Space,
}

impl Drop for Space<'_> {
    fn drop(&mut self) {
        // SAFETY: destroys our space once.
        unsafe { (self.instance.fns.DestroySpace)(self.handle) };
    }
}

/// A group of input actions, attached to a session with [`Session::attach`].
pub struct ActionSet<'i> {
    instance: &'i Instance,
    handle: sys::ActionSet,
}

impl ActionSet<'_> {
    /// A pose input (a controller's aim or grip pose) for the given hand paths.
    pub fn pose_action(&self, name: &str, localized: &str, subactions: &[sys::Path]) -> Result<Action<'_>> {
        self.action(name, localized, sys::ACTION_TYPE_POSE_INPUT, subactions)
    }

    /// A button input (any hand).
    /// A controller vibration, one per hand in `subactions` (bound to `/user/hand/*/output/haptic`).
    pub fn vibration_action(&self, name: &str, localized: &str, subactions: &[sys::Path]) -> Result<Action<'_>> {
        self.action(name, localized, sys::ACTION_TYPE_VIBRATION_OUTPUT, subactions)
    }

    pub fn boolean_action(&self, name: &str, localized: &str) -> Result<Action<'_>> {
        self.action(name, localized, sys::ACTION_TYPE_BOOLEAN_INPUT, &[])
    }

    /// An analog input such as a trigger (any hand).
    pub fn float_action(&self, name: &str, localized: &str) -> Result<Action<'_>> {
        self.action(name, localized, sys::ACTION_TYPE_FLOAT_INPUT, &[])
    }

    /// A two-axis input such as a thumbstick (any hand).
    pub fn vector2_action(&self, name: &str, localized: &str) -> Result<Action<'_>> {
        self.action(name, localized, sys::ACTION_TYPE_VECTOR2F_INPUT, &[])
    }

    fn action(&self, name: &str, localized: &str, kind: i32, subactions: &[sys::Path]) -> Result<Action<'_>> {
        let mut info = sys::ActionCreateInfo {
            ty: sys::TYPE_ACTION_CREATE_INFO,
            next: std::ptr::null(),
            actionName: [0; sys::MAX_ACTION_NAME_SIZE],
            actionType: kind,
            countSubactionPaths: subactions.len() as u32,
            subactionPaths: subactions.as_ptr(),
            localizedActionName: [0; sys::MAX_LOCALIZED_ACTION_NAME_SIZE],
        };
        copy_name(&mut info.actionName, name);
        copy_name(&mut info.localizedActionName, localized);
        let mut handle = sys::NULL_HANDLE;
        // SAFETY: typed in and out; the subaction list outlives the call.
        check(unsafe { (self.instance.fns.CreateAction)(self.handle, &info, &mut handle) }, "xrCreateAction")?;
        Ok(Action { instance: self.instance, handle })
    }
}

impl Drop for ActionSet<'_> {
    fn drop(&mut self) {
        // SAFETY: destroys our action set once; its actions borrowed it and are gone.
        unsafe { (self.instance.fns.DestroyActionSet)(self.handle) };
    }
}

pub struct Action<'s> {
    instance: &'s Instance,
    handle: sys::Action,
}

impl Drop for Action<'_> {
    fn drop(&mut self) {
        // SAFETY: destroys our action once.
        unsafe { (self.instance.fns.DestroyAction)(self.handle) };
    }
}

pub struct Swapchain<'s> {
    instance: &'s Instance,
    handle: sys::Swapchain,
    images: Vec<ID3D11Texture2D>,
    width: u32,
    height: u32,
}

impl Swapchain<'_> {
    fn full_image(&self) -> sys::SwapchainSubImage {
        sys::SwapchainSubImage {
            swapchain: self.handle,
            imageRect: sys::Rect2Di {
                offset: sys::Offset2Di { x: 0, y: 0 },
                extent: sys::Extent2Di { width: self.width as i32, height: self.height as i32 },
            },
            imageArrayIndex: 0,
        }
    }

    /// The part `rect` (left, top, width, height) of the image.
    fn sub_image(&self, rect: [u32; 4]) -> sys::SwapchainSubImage {
        let [x, y, width, height] = rect.map(|v| v as i32);
        sys::SwapchainSubImage {
            swapchain: self.handle,
            imageRect: sys::Rect2Di { offset: sys::Offset2Di { x, y }, extent: sys::Extent2Di { width, height } },
            imageArrayIndex: 0,
        }
    }

    /// Acquires the next image, waits for it, lets `fill` write it, and releases it.
    pub fn fill(&self, fill: impl FnOnce(&ID3D11Texture2D)) -> Result<()> {
        let fns = &self.instance.fns;
        let acquire = sys::BaseInfo { ty: sys::TYPE_SWAPCHAIN_IMAGE_ACQUIRE_INFO, next: std::ptr::null() };
        let wait = sys::SwapchainImageWaitInfo { ty: sys::TYPE_SWAPCHAIN_IMAGE_WAIT_INFO, next: std::ptr::null(), timeout: sys::INFINITE_DURATION };
        let release = sys::BaseInfo { ty: sys::TYPE_SWAPCHAIN_IMAGE_RELEASE_INFO, next: std::ptr::null() };
        let mut index = 0;
        // SAFETY: typed infos on a swapchain we own; the index is checked before use.
        unsafe {
            check((fns.AcquireSwapchainImage)(self.handle, &acquire, &mut index), "xrAcquireSwapchainImage")?;
            check((fns.WaitSwapchainImage)(self.handle, &wait), "xrWaitSwapchainImage")?;
        }
        let image = self.images.get(index as usize).ok_or(Error::Unsupported("swapchain image index out of range"))?;
        fill(image);
        // SAFETY: releases the image acquired above.
        check(unsafe { (fns.ReleaseSwapchainImage)(self.handle, &release) }, "xrReleaseSwapchainImage")
    }
}

impl Drop for Swapchain<'_> {
    fn drop(&mut self) {
        self.images.clear();
        // SAFETY: destroys our swapchain once, after dropping our image references.
        unsafe { (self.instance.fns.DestroySwapchain)(self.handle) };
    }
}

/// One eye of a projection layer.
pub struct ProjectionView<'a> {
    pub pose: Posef,
    pub fov: Fovf,
    pub swapchain: &'a Swapchain<'a>,
    /// The eye's depth, for the runtime's reprojection; left out unless the runtime takes depth
    /// ([`Instance::depth_layer`]).
    pub depth: Option<DepthView<'a>>,
}

/// The depth beside one eye's image (`XR_KHR_composition_layer_depth`), over a depth range of 0
/// to 1.
pub struct DepthView<'a> {
    /// A depth swapchain ([`Session::create_depth_swapchain`]).
    pub swapchain: &'a Swapchain<'a>,
    /// The part of its image that lines up with the eye image: left, top, width, height.
    pub rect: [u32; 4],
    /// The distances in metres at depth 0 (`nearZ`) and at depth 1 (`farZ`): reversed depth has
    /// the larger first, and either may be infinite.
    pub near_z: f32,
    pub far_z: f32,
}

/// A composition layer for [`Session::end_frame`].
pub enum Layer<'a> {
    /// Eye images with the poses they were rendered from; the runtime reprojects them.
    Projection { space: &'a Space<'a>, views: [ProjectionView<'a>; 2] },
    /// A flat quad shown to one eye (0 left, 1 right).
    Quad { space: &'a Space<'a>, eye: usize, swapchain: &'a Swapchain<'a>, pose: Posef, size: Extent2Df },
    /// A quad seen by both eyes, blended over the layers below by its alpha (premultiplied).
    Overlay { space: &'a Space<'a>, swapchain: &'a Swapchain<'a>, pose: Posef, size: Extent2Df },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_truncated_and_terminated() {
        let mut field = [0 as c_char; 4];
        copy_name(&mut field, "abcdef");
        assert_eq!(text(&field), "abc");
        assert_eq!(field[3], 0);
    }

    #[test]
    fn loader_requires_an_absolute_path() {
        assert!(matches!(Loader::load(Path::new("openxr_loader.dll")), Err(Error::Loader(_))));
    }
}
