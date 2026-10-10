//! Raw OpenXR 1.0 types and entry points: only what the viewer uses.
//!
//! Transcribed from the Khronos headers `openxr.h` / `openxr_platform.h` (OpenXR SDK 1.1.63,
//! Apache-2.0 OR MIT). Layouts follow the C definitions field for field; the tests check sizes.
#![allow(non_camel_case_types, non_snake_case, dead_code)]

use std::ffi::{c_char, c_void};

pub type Result = i32;
pub type StructureType = i32;
pub type Bool32 = u32;
pub type Version = u64;
pub type Flags64 = u64;
pub type Time = i64;
pub type Duration = i64;
pub type SystemId = u64;
pub type Path = u64;
pub type Instance = u64;
pub type Session = u64;
pub type Space = u64;
pub type Swapchain = u64;
pub type ActionSet = u64;
pub type Action = u64;

pub const NULL_HANDLE: u64 = 0;
pub const SUCCESS: Result = 0;
pub const EVENT_UNAVAILABLE: Result = 4;
pub const INFINITE_DURATION: Duration = 0x7fff_ffff_ffff_ffff;

pub const fn make_version(major: u64, minor: u64, patch: u64) -> Version {
    (major << 48) | (minor << 32) | patch
}
pub const API_VERSION_1_0: Version = make_version(1, 0, 63);

pub const KHR_D3D11_ENABLE: &std::ffi::CStr = c"XR_KHR_D3D11_enable";
/// Valve's Steam Frame controllers' interaction profile (`/interaction_profiles/valve/frame_controller`):
/// without it the runtime refuses bindings for that profile.
pub const VALVE_FRAME_CONTROLLER_INTERACTION: &std::ffi::CStr = c"XR_VALVE_frame_controller_interaction";
/// Hand tracking: 26 joints per hand (from cameras, or from controllers that sense the fingers).
pub const EXT_HAND_TRACKING: &std::ffi::CStr = c"XR_EXT_hand_tracking";
/// Hand joints with the fingers free to close through where the controller is.
pub const EXT_HAND_JOINTS_MOTION_RANGE: &std::ffi::CStr = c"XR_EXT_hand_joints_motion_range";

pub const TYPE_SYSTEM_HAND_TRACKING_PROPERTIES_EXT: StructureType = 1_000_051_000;
pub const TYPE_HAND_TRACKER_CREATE_INFO_EXT: StructureType = 1_000_051_001;
pub const TYPE_HAND_JOINTS_LOCATE_INFO_EXT: StructureType = 1_000_051_002;
pub const TYPE_HAND_JOINT_LOCATIONS_EXT: StructureType = 1_000_051_003;
pub const TYPE_HAND_JOINTS_MOTION_RANGE_INFO_EXT: StructureType = 1_000_080_000;
/// `XrHandEXT`.
pub const HAND_LEFT_EXT: i32 = 1;
pub const HAND_RIGHT_EXT: i32 = 2;
pub const HAND_JOINT_SET_DEFAULT_EXT: i32 = 0;
pub const HAND_JOINT_COUNT_EXT: u32 = 26;
/// `XrHandJointsMotionRangeEXT`: the fingers as they are, not wrapped around the controller.
pub const HAND_JOINTS_MOTION_RANGE_UNOBSTRUCTED_EXT: i32 = 1;
pub type HandTrackerEXT = u64;

#[repr(C)]
pub struct SystemHandTrackingPropertiesEXT {
    pub ty: StructureType,
    pub next: *mut c_void,
    pub supportsHandTracking: Bool32,
}

#[repr(C)]
pub struct HandTrackerCreateInfoEXT {
    pub ty: StructureType,
    pub next: *const c_void,
    pub hand: i32,
    pub handJointSet: i32,
}

#[repr(C)]
pub struct HandJointsMotionRangeInfoEXT {
    pub ty: StructureType,
    pub next: *const c_void,
    pub handJointsMotionRange: i32,
}

#[repr(C)]
pub struct HandJointsLocateInfoEXT {
    pub ty: StructureType,
    pub next: *const c_void,
    pub baseSpace: Space,
    pub time: Time,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct HandJointLocationEXT {
    pub locationFlags: Flags64,
    pub pose: Posef,
    pub radius: f32,
}

#[repr(C)]
pub struct HandJointLocationsEXT {
    pub ty: StructureType,
    pub next: *mut c_void,
    pub isActive: Bool32,
    pub jointCount: u32,
    pub jointLocations: *mut HandJointLocationEXT,
}

pub type PFN_xrCreateHandTrackerEXT = unsafe extern "system" fn(Session, *const HandTrackerCreateInfoEXT, *mut HandTrackerEXT) -> Result;
pub type PFN_xrDestroyHandTrackerEXT = unsafe extern "system" fn(HandTrackerEXT) -> Result;
pub type PFN_xrLocateHandJointsEXT = unsafe extern "system" fn(HandTrackerEXT, *const HandJointsLocateInfoEXT, *mut HandJointLocationsEXT) -> Result;

// XrStructureType
pub const TYPE_EXTENSION_PROPERTIES: StructureType = 2;
pub const TYPE_INSTANCE_CREATE_INFO: StructureType = 3;
pub const TYPE_SYSTEM_GET_INFO: StructureType = 4;
pub const TYPE_SYSTEM_PROPERTIES: StructureType = 5;
pub const TYPE_VIEW_LOCATE_INFO: StructureType = 6;
pub const TYPE_VIEW: StructureType = 7;
pub const TYPE_SESSION_CREATE_INFO: StructureType = 8;
pub const TYPE_SWAPCHAIN_CREATE_INFO: StructureType = 9;
pub const TYPE_SESSION_BEGIN_INFO: StructureType = 10;
pub const TYPE_VIEW_STATE: StructureType = 11;
pub const TYPE_FRAME_END_INFO: StructureType = 12;
pub const TYPE_HAPTIC_VIBRATION: StructureType = 13;
pub const TYPE_EVENT_DATA_BUFFER: StructureType = 16;
pub const TYPE_EVENT_DATA_INSTANCE_LOSS_PENDING: StructureType = 17;
pub const TYPE_EVENT_DATA_SESSION_STATE_CHANGED: StructureType = 18;
pub const TYPE_ACTION_STATE_BOOLEAN: StructureType = 23;
pub const TYPE_ACTION_STATE_FLOAT: StructureType = 24;
pub const TYPE_ACTION_STATE_VECTOR2F: StructureType = 25;
pub const TYPE_ACTION_STATE_POSE: StructureType = 27;
pub const TYPE_ACTION_SET_CREATE_INFO: StructureType = 28;
pub const TYPE_ACTION_CREATE_INFO: StructureType = 29;
pub const TYPE_ACTION_SPACE_CREATE_INFO: StructureType = 38;
pub const TYPE_SPACE_LOCATION: StructureType = 42;
pub const TYPE_INTERACTION_PROFILE_SUGGESTED_BINDING: StructureType = 51;
pub const TYPE_ACTION_STATE_GET_INFO: StructureType = 58;
pub const TYPE_HAPTIC_ACTION_INFO: StructureType = 59;
pub const TYPE_SESSION_ACTION_SETS_ATTACH_INFO: StructureType = 60;
pub const TYPE_ACTIONS_SYNC_INFO: StructureType = 61;
pub const TYPE_INSTANCE_PROPERTIES: StructureType = 32;
pub const TYPE_FRAME_WAIT_INFO: StructureType = 33;
pub const TYPE_COMPOSITION_LAYER_PROJECTION: StructureType = 35;
pub const TYPE_COMPOSITION_LAYER_QUAD: StructureType = 36;
pub const TYPE_REFERENCE_SPACE_CREATE_INFO: StructureType = 37;
pub const TYPE_VIEW_CONFIGURATION_VIEW: StructureType = 41;
pub const TYPE_FRAME_STATE: StructureType = 44;
pub const TYPE_FRAME_BEGIN_INFO: StructureType = 46;
pub const TYPE_COMPOSITION_LAYER_PROJECTION_VIEW: StructureType = 48;
pub const TYPE_SWAPCHAIN_IMAGE_ACQUIRE_INFO: StructureType = 55;
pub const TYPE_SWAPCHAIN_IMAGE_WAIT_INFO: StructureType = 56;
pub const TYPE_SWAPCHAIN_IMAGE_RELEASE_INFO: StructureType = 57;
pub const TYPE_GRAPHICS_BINDING_D3D11_KHR: StructureType = 1_000_027_000;
pub const TYPE_SWAPCHAIN_IMAGE_D3D11_KHR: StructureType = 1_000_027_001;
pub const TYPE_GRAPHICS_REQUIREMENTS_D3D11_KHR: StructureType = 1_000_027_002;

pub const FORM_FACTOR_HEAD_MOUNTED_DISPLAY: i32 = 1;
pub const VIEW_CONFIGURATION_TYPE_PRIMARY_STEREO: i32 = 2;
pub const ENVIRONMENT_BLEND_MODE_OPAQUE: i32 = 1;
pub const REFERENCE_SPACE_TYPE_VIEW: i32 = 1;
pub const REFERENCE_SPACE_TYPE_LOCAL: i32 = 2;
pub const EYE_VISIBILITY_BOTH: i32 = 0;
pub const EYE_VISIBILITY_LEFT: i32 = 1;
pub const COMPOSITION_LAYER_BLEND_TEXTURE_SOURCE_ALPHA_BIT: Flags64 = 0x2;
pub const EYE_VISIBILITY_RIGHT: i32 = 2;

pub const SESSION_STATE_READY: i32 = 2;
pub const SESSION_STATE_STOPPING: i32 = 6;
pub const SESSION_STATE_LOSS_PENDING: i32 = 7;
pub const SESSION_STATE_EXITING: i32 = 8;

pub const VIEW_STATE_ORIENTATION_VALID_BIT: Flags64 = 0x1;
pub const VIEW_STATE_POSITION_VALID_BIT: Flags64 = 0x2;
pub const SPACE_LOCATION_ORIENTATION_VALID_BIT: Flags64 = 0x1;
pub const SPACE_LOCATION_POSITION_VALID_BIT: Flags64 = 0x2;
pub const SPACE_LOCATION_ORIENTATION_TRACKED_BIT: Flags64 = 0x4;
pub const SPACE_LOCATION_POSITION_TRACKED_BIT: Flags64 = 0x8;
pub const ACTION_TYPE_BOOLEAN_INPUT: i32 = 1;
pub const ACTION_TYPE_FLOAT_INPUT: i32 = 2;
pub const ACTION_TYPE_VECTOR2F_INPUT: i32 = 3;
pub const ACTION_TYPE_POSE_INPUT: i32 = 4;
pub const ACTION_TYPE_VIBRATION_OUTPUT: i32 = 100;
/// `XR_FREQUENCY_UNSPECIFIED`: the runtime's own vibration frequency.
pub const FREQUENCY_UNSPECIFIED: f32 = 0.0;
pub const SWAPCHAIN_USAGE_TRANSFER_DST_BIT: Flags64 = 0x10;
pub const SWAPCHAIN_USAGE_SAMPLED_BIT: Flags64 = 0x20;

pub const MAX_EXTENSION_NAME_SIZE: usize = 128;
pub const MAX_SYSTEM_NAME_SIZE: usize = 256;
pub const MAX_APPLICATION_NAME_SIZE: usize = 128;
pub const MAX_ENGINE_NAME_SIZE: usize = 128;
pub const MAX_RUNTIME_NAME_SIZE: usize = 128;
pub const MAX_ACTION_SET_NAME_SIZE: usize = 64;
pub const MAX_LOCALIZED_ACTION_SET_NAME_SIZE: usize = 128;
pub const MAX_ACTION_NAME_SIZE: usize = 64;
pub const MAX_LOCALIZED_ACTION_NAME_SIZE: usize = 128;

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Quaternionf {
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub w: f32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Vector3f {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Posef {
    pub orientation: Quaternionf,
    pub position: Vector3f,
}

impl Posef {
    pub const IDENTITY: Posef = Posef { orientation: Quaternionf { x: 0.0, y: 0.0, z: 0.0, w: 1.0 }, position: Vector3f { x: 0.0, y: 0.0, z: 0.0 } };
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Fovf {
    pub angleLeft: f32,
    pub angleRight: f32,
    pub angleUp: f32,
    pub angleDown: f32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Offset2Di {
    pub x: i32,
    pub y: i32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Extent2Di {
    pub width: i32,
    pub height: i32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Extent2Df {
    pub width: f32,
    pub height: f32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Rect2Di {
    pub offset: Offset2Di,
    pub extent: Extent2Di,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct ApplicationInfo {
    pub applicationName: [c_char; MAX_APPLICATION_NAME_SIZE],
    pub applicationVersion: u32,
    pub engineName: [c_char; MAX_ENGINE_NAME_SIZE],
    pub engineVersion: u32,
    pub apiVersion: Version,
}

#[repr(C)]
pub struct InstanceCreateInfo {
    pub ty: StructureType,
    pub next: *const c_void,
    pub createFlags: Flags64,
    pub applicationInfo: ApplicationInfo,
    pub enabledApiLayerCount: u32,
    pub enabledApiLayerNames: *const *const c_char,
    pub enabledExtensionCount: u32,
    pub enabledExtensionNames: *const *const c_char,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct ExtensionProperties {
    pub ty: StructureType,
    pub next: *mut c_void,
    pub extensionName: [c_char; MAX_EXTENSION_NAME_SIZE],
    pub extensionVersion: u32,
}

#[repr(C)]
pub struct InstanceProperties {
    pub ty: StructureType,
    pub next: *mut c_void,
    pub runtimeVersion: Version,
    pub runtimeName: [c_char; MAX_RUNTIME_NAME_SIZE],
}

#[repr(C)]
pub struct SystemGetInfo {
    pub ty: StructureType,
    pub next: *const c_void,
    pub formFactor: i32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct SystemGraphicsProperties {
    pub maxSwapchainImageHeight: u32,
    pub maxSwapchainImageWidth: u32,
    pub maxLayerCount: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct SystemTrackingProperties {
    pub orientationTracking: Bool32,
    pub positionTracking: Bool32,
}

#[repr(C)]
pub struct SystemProperties {
    pub ty: StructureType,
    pub next: *mut c_void,
    pub systemId: SystemId,
    pub vendorId: u32,
    pub systemName: [c_char; MAX_SYSTEM_NAME_SIZE],
    pub graphicsProperties: SystemGraphicsProperties,
    pub trackingProperties: SystemTrackingProperties,
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct ViewConfigurationView {
    pub ty: StructureType,
    pub next: *mut c_void,
    pub recommendedImageRectWidth: u32,
    pub maxImageRectWidth: u32,
    pub recommendedImageRectHeight: u32,
    pub maxImageRectHeight: u32,
    pub recommendedSwapchainSampleCount: u32,
    pub maxSwapchainSampleCount: u32,
}

#[repr(C)]
pub struct GraphicsRequirementsD3D11KHR {
    pub ty: StructureType,
    pub next: *mut c_void,
    pub adapterLuid: windows::Win32::Foundation::LUID,
    pub minFeatureLevel: i32,
}

#[repr(C)]
pub struct GraphicsBindingD3D11KHR {
    pub ty: StructureType,
    pub next: *const c_void,
    /// `ID3D11Device*`
    pub device: *mut c_void,
}

#[repr(C)]
pub struct SessionCreateInfo {
    pub ty: StructureType,
    pub next: *const c_void,
    pub createFlags: Flags64,
    pub systemId: SystemId,
}

#[repr(C)]
pub struct ReferenceSpaceCreateInfo {
    pub ty: StructureType,
    pub next: *const c_void,
    pub referenceSpaceType: i32,
    pub poseInReferenceSpace: Posef,
}

#[repr(C)]
pub struct SwapchainCreateInfo {
    pub ty: StructureType,
    pub next: *const c_void,
    pub createFlags: Flags64,
    pub usageFlags: Flags64,
    pub format: i64,
    pub sampleCount: u32,
    pub width: u32,
    pub height: u32,
    pub faceCount: u32,
    pub arraySize: u32,
    pub mipCount: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct SwapchainImageD3D11KHR {
    pub ty: StructureType,
    pub next: *mut c_void,
    /// `ID3D11Texture2D*`, owned by the runtime.
    pub texture: *mut c_void,
}

/// Header-only stand-in for `XrSwapchainImageBaseHeader*` parameters.
#[repr(C)]
pub struct SwapchainImageBaseHeader {
    pub ty: StructureType,
    pub next: *mut c_void,
}

/// The acquire, release and frame begin/wait infos are just a type and a next pointer.
#[repr(C)]
pub struct BaseInfo {
    pub ty: StructureType,
    pub next: *const c_void,
}

#[repr(C)]
pub struct SwapchainImageWaitInfo {
    pub ty: StructureType,
    pub next: *const c_void,
    pub timeout: Duration,
}

#[repr(C)]
pub struct EventDataBuffer {
    pub ty: StructureType,
    pub next: *const c_void,
    pub varying: [u8; 4000],
}

#[repr(C)]
pub struct EventDataSessionStateChanged {
    pub ty: StructureType,
    pub next: *const c_void,
    pub session: Session,
    pub state: i32,
    pub time: Time,
}

#[repr(C)]
pub struct SessionBeginInfo {
    pub ty: StructureType,
    pub next: *const c_void,
    pub primaryViewConfigurationType: i32,
}

#[repr(C)]
pub struct FrameState {
    pub ty: StructureType,
    pub next: *mut c_void,
    pub predictedDisplayTime: Time,
    pub predictedDisplayPeriod: Duration,
    pub shouldRender: Bool32,
}

#[repr(C)]
pub struct ViewLocateInfo {
    pub ty: StructureType,
    pub next: *const c_void,
    pub viewConfigurationType: i32,
    pub displayTime: Time,
    pub space: Space,
}

#[repr(C)]
pub struct ViewState {
    pub ty: StructureType,
    pub next: *mut c_void,
    pub viewStateFlags: Flags64,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct View {
    pub ty: StructureType,
    pub next: *mut c_void,
    pub pose: Posef,
    pub fov: Fovf,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct SwapchainSubImage {
    pub swapchain: Swapchain,
    pub imageRect: Rect2Di,
    pub imageArrayIndex: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct CompositionLayerProjectionView {
    pub ty: StructureType,
    pub next: *const c_void,
    pub pose: Posef,
    pub fov: Fovf,
    pub subImage: SwapchainSubImage,
}

#[repr(C)]
pub struct CompositionLayerProjection {
    pub ty: StructureType,
    pub next: *const c_void,
    pub layerFlags: Flags64,
    pub space: Space,
    pub viewCount: u32,
    pub views: *const CompositionLayerProjectionView,
}

#[repr(C)]
pub struct CompositionLayerQuad {
    pub ty: StructureType,
    pub next: *const c_void,
    pub layerFlags: Flags64,
    pub space: Space,
    pub eyeVisibility: i32,
    pub subImage: SwapchainSubImage,
    pub pose: Posef,
    pub size: Extent2Df,
}

/// Stand-in for `XrCompositionLayerBaseHeader` (only ever pointed to).
#[repr(C)]
pub struct CompositionLayerBaseHeader {
    pub ty: StructureType,
    pub next: *const c_void,
    pub layerFlags: Flags64,
    pub space: Space,
}

#[repr(C)]
pub struct FrameEndInfo {
    pub ty: StructureType,
    pub next: *const c_void,
    pub displayTime: Time,
    pub environmentBlendMode: i32,
    pub layerCount: u32,
    pub layers: *const *const CompositionLayerBaseHeader,
}

#[repr(C)]
pub struct ActionSetCreateInfo {
    pub ty: StructureType,
    pub next: *const c_void,
    pub actionSetName: [c_char; MAX_ACTION_SET_NAME_SIZE],
    pub localizedActionSetName: [c_char; MAX_LOCALIZED_ACTION_SET_NAME_SIZE],
    pub priority: u32,
}

#[repr(C)]
pub struct ActionCreateInfo {
    pub ty: StructureType,
    pub next: *const c_void,
    pub actionName: [c_char; MAX_ACTION_NAME_SIZE],
    pub actionType: i32,
    pub countSubactionPaths: u32,
    pub subactionPaths: *const Path,
    pub localizedActionName: [c_char; MAX_LOCALIZED_ACTION_NAME_SIZE],
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct ActionSuggestedBinding {
    pub action: Action,
    pub binding: Path,
}

#[repr(C)]
pub struct InteractionProfileSuggestedBinding {
    pub ty: StructureType,
    pub next: *const c_void,
    pub interactionProfile: Path,
    pub countSuggestedBindings: u32,
    pub suggestedBindings: *const ActionSuggestedBinding,
}

#[repr(C)]
pub struct SessionActionSetsAttachInfo {
    pub ty: StructureType,
    pub next: *const c_void,
    pub countActionSets: u32,
    pub actionSets: *const ActionSet,
}

#[repr(C)]
pub struct ActionSpaceCreateInfo {
    pub ty: StructureType,
    pub next: *const c_void,
    pub action: Action,
    pub subactionPath: Path,
    pub poseInActionSpace: Posef,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct ActiveActionSet {
    pub actionSet: ActionSet,
    pub subactionPath: Path,
}

#[repr(C)]
pub struct ActionsSyncInfo {
    pub ty: StructureType,
    pub next: *const c_void,
    pub countActiveActionSets: u32,
    pub activeActionSets: *const ActiveActionSet,
}

#[repr(C)]
pub struct SpaceLocation {
    pub ty: StructureType,
    pub next: *mut c_void,
    pub locationFlags: Flags64,
    pub pose: Posef,
}

#[repr(C)]
pub struct HapticActionInfo {
    pub ty: StructureType,
    pub next: *const c_void,
    pub action: Action,
    pub subactionPath: Path,
}

/// `XrHapticVibration`, passed where the API takes an `XrHapticBaseHeader`.
#[repr(C)]
pub struct HapticVibration {
    pub ty: StructureType,
    pub next: *const c_void,
    /// Nanoseconds.
    pub duration: Duration,
    pub frequency: f32,
    pub amplitude: f32,
}

#[repr(C)]
pub struct ActionStateGetInfo {
    pub ty: StructureType,
    pub next: *const c_void,
    pub action: Action,
    pub subactionPath: Path,
}

#[repr(C)]
pub struct ActionStateBoolean {
    pub ty: StructureType,
    pub next: *mut c_void,
    pub currentState: Bool32,
    pub changedSinceLastSync: Bool32,
    pub lastChangeTime: Time,
    pub isActive: Bool32,
}

#[repr(C)]
pub struct ActionStateFloat {
    pub ty: StructureType,
    pub next: *mut c_void,
    pub currentState: f32,
    pub changedSinceLastSync: Bool32,
    pub lastChangeTime: Time,
    pub isActive: Bool32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Vector2f {
    pub x: f32,
    pub y: f32,
}

#[repr(C)]
pub struct ActionStateVector2f {
    pub ty: StructureType,
    pub next: *mut c_void,
    pub currentState: Vector2f,
    pub changedSinceLastSync: Bool32,
    pub lastChangeTime: Time,
    pub isActive: Bool32,
}

#[repr(C)]
pub struct ActionStatePose {
    pub ty: StructureType,
    pub next: *mut c_void,
    pub isActive: Bool32,
}

pub type PFN_xrVoidFunction = Option<unsafe extern "system" fn()>;
pub type PFN_xrGetInstanceProcAddr = unsafe extern "system" fn(Instance, *const c_char, *mut PFN_xrVoidFunction) -> Result;
pub type PFN_xrEnumerateInstanceExtensionProperties =
    unsafe extern "system" fn(*const c_char, u32, *mut u32, *mut ExtensionProperties) -> Result;
pub type PFN_xrCreateInstance = unsafe extern "system" fn(*const InstanceCreateInfo, *mut Instance) -> Result;
pub type PFN_xrDestroyInstance = unsafe extern "system" fn(Instance) -> Result;
pub type PFN_xrGetInstanceProperties = unsafe extern "system" fn(Instance, *mut InstanceProperties) -> Result;
pub type PFN_xrGetSystem = unsafe extern "system" fn(Instance, *const SystemGetInfo, *mut SystemId) -> Result;
pub type PFN_xrGetSystemProperties = unsafe extern "system" fn(Instance, SystemId, *mut SystemProperties) -> Result;
pub type PFN_xrGetD3D11GraphicsRequirementsKHR = unsafe extern "system" fn(Instance, SystemId, *mut GraphicsRequirementsD3D11KHR) -> Result;
pub type PFN_xrEnumerateViewConfigurationViews =
    unsafe extern "system" fn(Instance, SystemId, i32, u32, *mut u32, *mut ViewConfigurationView) -> Result;
pub type PFN_xrEnumerateEnvironmentBlendModes = unsafe extern "system" fn(Instance, SystemId, i32, u32, *mut u32, *mut i32) -> Result;
pub type PFN_xrCreateSession = unsafe extern "system" fn(Instance, *const SessionCreateInfo, *mut Session) -> Result;
pub type PFN_xrDestroySession = unsafe extern "system" fn(Session) -> Result;
pub type PFN_xrCreateReferenceSpace = unsafe extern "system" fn(Session, *const ReferenceSpaceCreateInfo, *mut Space) -> Result;
pub type PFN_xrDestroySpace = unsafe extern "system" fn(Space) -> Result;
pub type PFN_xrEnumerateSwapchainFormats = unsafe extern "system" fn(Session, u32, *mut u32, *mut i64) -> Result;
pub type PFN_xrCreateSwapchain = unsafe extern "system" fn(Session, *const SwapchainCreateInfo, *mut Swapchain) -> Result;
pub type PFN_xrDestroySwapchain = unsafe extern "system" fn(Swapchain) -> Result;
pub type PFN_xrEnumerateSwapchainImages = unsafe extern "system" fn(Swapchain, u32, *mut u32, *mut SwapchainImageBaseHeader) -> Result;
pub type PFN_xrAcquireSwapchainImage = unsafe extern "system" fn(Swapchain, *const BaseInfo, *mut u32) -> Result;
pub type PFN_xrWaitSwapchainImage = unsafe extern "system" fn(Swapchain, *const SwapchainImageWaitInfo) -> Result;
pub type PFN_xrReleaseSwapchainImage = unsafe extern "system" fn(Swapchain, *const BaseInfo) -> Result;
pub type PFN_xrPollEvent = unsafe extern "system" fn(Instance, *mut EventDataBuffer) -> Result;
pub type PFN_xrBeginSession = unsafe extern "system" fn(Session, *const SessionBeginInfo) -> Result;
pub type PFN_xrEndSession = unsafe extern "system" fn(Session) -> Result;
pub type PFN_xrRequestExitSession = unsafe extern "system" fn(Session) -> Result;
pub type PFN_xrWaitFrame = unsafe extern "system" fn(Session, *const BaseInfo, *mut FrameState) -> Result;
pub type PFN_xrBeginFrame = unsafe extern "system" fn(Session, *const BaseInfo) -> Result;
pub type PFN_xrEndFrame = unsafe extern "system" fn(Session, *const FrameEndInfo) -> Result;
pub type PFN_xrLocateViews = unsafe extern "system" fn(Session, *const ViewLocateInfo, *mut ViewState, u32, *mut u32, *mut View) -> Result;
pub type PFN_xrStringToPath = unsafe extern "system" fn(Instance, *const c_char, *mut Path) -> Result;
pub type PFN_xrCreateActionSet = unsafe extern "system" fn(Instance, *const ActionSetCreateInfo, *mut ActionSet) -> Result;
pub type PFN_xrDestroyActionSet = unsafe extern "system" fn(ActionSet) -> Result;
pub type PFN_xrCreateAction = unsafe extern "system" fn(ActionSet, *const ActionCreateInfo, *mut Action) -> Result;
pub type PFN_xrDestroyAction = unsafe extern "system" fn(Action) -> Result;
pub type PFN_xrSuggestInteractionProfileBindings = unsafe extern "system" fn(Instance, *const InteractionProfileSuggestedBinding) -> Result;
pub type PFN_xrAttachSessionActionSets = unsafe extern "system" fn(Session, *const SessionActionSetsAttachInfo) -> Result;
pub type PFN_xrCreateActionSpace = unsafe extern "system" fn(Session, *const ActionSpaceCreateInfo, *mut Space) -> Result;
pub type PFN_xrSyncActions = unsafe extern "system" fn(Session, *const ActionsSyncInfo) -> Result;
pub type PFN_xrLocateSpace = unsafe extern "system" fn(Space, Space, Time, *mut SpaceLocation) -> Result;
pub type PFN_xrGetActionStatePose = unsafe extern "system" fn(Session, *const ActionStateGetInfo, *mut ActionStatePose) -> Result;
pub type PFN_xrGetActionStateBoolean = unsafe extern "system" fn(Session, *const ActionStateGetInfo, *mut ActionStateBoolean) -> Result;
pub type PFN_xrGetActionStateFloat = unsafe extern "system" fn(Session, *const ActionStateGetInfo, *mut ActionStateFloat) -> Result;
pub type PFN_xrGetActionStateVector2f = unsafe extern "system" fn(Session, *const ActionStateGetInfo, *mut ActionStateVector2f) -> Result;
pub type PFN_xrApplyHapticFeedback = unsafe extern "system" fn(Session, *const HapticActionInfo, *const HapticVibration) -> Result;

#[cfg(test)]
mod tests {
    use super::*;
    use core::mem::{offset_of, size_of};

    // The C headers' sizes and offsets with MSVC x64 (sizeof/offsetof).
    #[test]
    fn layouts_match_c() {
        assert_eq!(size_of::<Posef>(), 28);
        assert_eq!(size_of::<ApplicationInfo>(), 272);
        assert_eq!(size_of::<InstanceCreateInfo>(), 328);
        assert_eq!(size_of::<SystemProperties>(), 304);
        assert_eq!(size_of::<SwapchainCreateInfo>(), 64);
        assert_eq!(size_of::<EventDataBuffer>(), 4016);
        assert_eq!(size_of::<View>(), 64);
        assert_eq!(size_of::<CompositionLayerProjectionView>(), 96);
        assert_eq!(size_of::<CompositionLayerProjection>(), 48);
        assert_eq!(size_of::<CompositionLayerQuad>(), 112);
        assert_eq!(offset_of!(CompositionLayerQuad, subImage), 40);
        assert_eq!(size_of::<FrameEndInfo>(), 40);
        assert_eq!(size_of::<GraphicsRequirementsD3D11KHR>(), 32);
        assert_eq!(size_of::<ReferenceSpaceCreateInfo>(), 48);
        assert_eq!(size_of::<ViewConfigurationView>(), 40);
        assert_eq!(size_of::<EventDataSessionStateChanged>(), 40);
        assert_eq!(size_of::<FrameState>(), 40);
        assert_eq!(size_of::<ViewLocateInfo>(), 40);
        assert_eq!(size_of::<SwapchainSubImage>(), 32);
        assert_eq!(size_of::<ExtensionProperties>(), 152);
        assert_eq!(size_of::<InstanceProperties>(), 152);
        assert_eq!(size_of::<SwapchainImageD3D11KHR>(), 24);
        assert_eq!(size_of::<GraphicsBindingD3D11KHR>(), 24);
        assert_eq!(size_of::<SessionCreateInfo>(), 32);
        assert_eq!(size_of::<SwapchainImageWaitInfo>(), 24);
        assert_eq!(size_of::<ViewState>(), 24);
        assert_eq!(size_of::<SessionBeginInfo>(), 24);
        assert_eq!(size_of::<ActionSetCreateInfo>(), 216);
        assert_eq!(offset_of!(ActionSetCreateInfo, priority), 208);
        assert_eq!(size_of::<ActionCreateInfo>(), 224);
        assert_eq!(offset_of!(ActionCreateInfo, actionType), 80);
        assert_eq!(offset_of!(ActionCreateInfo, subactionPaths), 88);
        assert_eq!(offset_of!(ActionCreateInfo, localizedActionName), 96);
        assert_eq!(size_of::<ActionSuggestedBinding>(), 16);
        assert_eq!(size_of::<InteractionProfileSuggestedBinding>(), 40);
        assert_eq!(offset_of!(InteractionProfileSuggestedBinding, suggestedBindings), 32);
        assert_eq!(size_of::<SessionActionSetsAttachInfo>(), 32);
        assert_eq!(size_of::<ActionSpaceCreateInfo>(), 64);
        assert_eq!(offset_of!(ActionSpaceCreateInfo, poseInActionSpace), 32);
        assert_eq!(size_of::<ActionsSyncInfo>(), 32);
        assert_eq!(size_of::<SpaceLocation>(), 56);
        assert_eq!(offset_of!(SpaceLocation, pose), 24);
        assert_eq!(size_of::<ActionStateGetInfo>(), 32);
        assert_eq!(size_of::<ActionStateBoolean>(), 40);
        assert_eq!(offset_of!(ActionStateBoolean, currentState), 16);
        assert_eq!(offset_of!(ActionStateBoolean, lastChangeTime), 24);
        assert_eq!(offset_of!(ActionStateBoolean, isActive), 32);
        assert_eq!(size_of::<ActionStateFloat>(), 40);
        assert_eq!(offset_of!(ActionStateFloat, isActive), 32);
        assert_eq!(size_of::<ActionStateVector2f>(), 48);
        assert_eq!(offset_of!(ActionStateVector2f, changedSinceLastSync), 24);
        assert_eq!(offset_of!(ActionStateVector2f, lastChangeTime), 32);
        assert_eq!(offset_of!(ActionStateVector2f, isActive), 40);
        assert_eq!(size_of::<ActionStatePose>(), 24);
        assert_eq!(API_VERSION_1_0, (1 << 48) | 63);
        // XR_EXT_hand_tracking (openxr.h, 64-bit).
        assert_eq!(size_of::<SystemHandTrackingPropertiesEXT>(), 24);
        assert_eq!(size_of::<HandTrackerCreateInfoEXT>(), 24);
        assert_eq!(size_of::<HandJointsMotionRangeInfoEXT>(), 24);
        assert_eq!(size_of::<HandJointsLocateInfoEXT>(), 32);
        assert_eq!(size_of::<HandJointLocationEXT>(), 40);
        assert_eq!(offset_of!(HandJointLocationEXT, pose), 8);
        assert_eq!(size_of::<HandJointLocationsEXT>(), 32);
        assert_eq!(offset_of!(HandJointLocationsEXT, jointLocations), 24);
    }
}
