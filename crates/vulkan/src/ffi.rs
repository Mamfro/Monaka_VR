//! The few Vulkan declarations the sharing uses, as the Vulkan headers lay them out (x64).

#![allow(non_camel_case_types, non_snake_case, dead_code)]

use std::ffi::{c_char, c_void};

pub type VkResult = i32;
pub const VK_SUCCESS: VkResult = 0;
pub const VK_NOT_READY: VkResult = 1;

/// Dispatchable handles are pointers, the others 64-bit values.
pub type VkInstance = usize;
pub type VkPhysicalDevice = usize;
pub type VkDevice = usize;
pub type VkQueue = usize;
pub type VkCommandBuffer = usize;
pub type VkBuffer = u64;
pub type VkImage = u64;
pub type VkDeviceMemory = u64;
pub type VkCommandPool = u64;
pub type VkSemaphore = u64;
pub type VkFence = u64;
pub type VkSwapchainKHR = u64;
pub type VkSurfaceKHR = u64;

pub const STRUCTURE_SUBMIT_INFO: u32 = 4;
pub const STRUCTURE_MEMORY_ALLOCATE_INFO: u32 = 5;
pub const STRUCTURE_FENCE_CREATE_INFO: u32 = 8;
pub const STRUCTURE_SEMAPHORE_CREATE_INFO: u32 = 9;
pub const STRUCTURE_BUFFER_CREATE_INFO: u32 = 12;
pub const STRUCTURE_COMMAND_POOL_CREATE_INFO: u32 = 39;
pub const STRUCTURE_COMMAND_BUFFER_ALLOCATE_INFO: u32 = 40;
pub const STRUCTURE_COMMAND_BUFFER_BEGIN_INFO: u32 = 42;
pub const STRUCTURE_IMAGE_MEMORY_BARRIER: u32 = 45;
pub const STRUCTURE_PHYSICAL_DEVICE_PROPERTIES_2: u32 = 1_000_059_001;
pub const STRUCTURE_PHYSICAL_DEVICE_ID_PROPERTIES: u32 = 1_000_071_004;
pub const STRUCTURE_EXTERNAL_MEMORY_BUFFER_CREATE_INFO: u32 = 1_000_072_001;
pub const STRUCTURE_IMPORT_MEMORY_WIN32_HANDLE_INFO_KHR: u32 = 1_000_073_000;
pub const STRUCTURE_MEMORY_WIN32_HANDLE_PROPERTIES_KHR: u32 = 1_000_073_002;
pub const STRUCTURE_IMPORT_SEMAPHORE_WIN32_HANDLE_INFO_KHR: u32 = 1_000_078_000;
pub const STRUCTURE_D3D12_FENCE_SUBMIT_INFO_KHR: u32 = 1_000_078_002;
pub const STRUCTURE_MEMORY_DEDICATED_ALLOCATE_INFO: u32 = 1_000_127_001;

pub const EXTERNAL_MEMORY_HANDLE_TYPE_D3D12_RESOURCE: u32 = 0x40;
pub const EXTERNAL_SEMAPHORE_HANDLE_TYPE_D3D12_FENCE: u32 = 0x8;

pub const IMAGE_USAGE_TRANSFER_SRC: u32 = 0x1;
pub const BUFFER_USAGE_TRANSFER_DST: u32 = 0x2;
pub const MEMORY_PROPERTY_DEVICE_LOCAL: u32 = 0x1;
pub const COMMAND_POOL_CREATE_RESET_COMMAND_BUFFER: u32 = 0x2;
pub const COMMAND_BUFFER_USAGE_ONE_TIME_SUBMIT: u32 = 0x1;
pub const FENCE_CREATE_SIGNALED: u32 = 0x1;

pub const IMAGE_LAYOUT_TRANSFER_SRC_OPTIMAL: i32 = 6;
pub const IMAGE_LAYOUT_PRESENT_SRC_KHR: i32 = 1_000_001_002;
pub const ACCESS_TRANSFER_READ: u32 = 0x800;
pub const ACCESS_MEMORY_READ: u32 = 0x8000;
pub const ACCESS_MEMORY_WRITE: u32 = 0x10000;
pub const PIPELINE_STAGE_TOP_OF_PIPE: u32 = 0x1;
pub const PIPELINE_STAGE_TRANSFER: u32 = 0x1000;
pub const PIPELINE_STAGE_BOTTOM_OF_PIPE: u32 = 0x2000;
pub const PIPELINE_STAGE_ALL_COMMANDS: u32 = 0x10000;
pub const IMAGE_ASPECT_COLOR: u32 = 0x1;
pub const IMAGE_ASPECT_DEPTH: u32 = 0x2;
pub const IMAGE_ASPECT_STENCIL: u32 = 0x4;
pub const IMAGE_USAGE_COLOR_ATTACHMENT: u32 = 0x10;
pub const IMAGE_USAGE_DEPTH_STENCIL_ATTACHMENT: u32 = 0x20;
pub const QUEUE_FAMILY_IGNORED: u32 = !0;

pub const FORMAT_R8G8B8A8_UNORM: i32 = 37;
pub const FORMAT_R8G8B8A8_SRGB: i32 = 43;
pub const FORMAT_B8G8R8A8_UNORM: i32 = 44;
pub const FORMAT_B8G8R8A8_SRGB: i32 = 50;
pub const FORMAT_A2B10G10R10_UNORM_PACK32: i32 = 64;
pub const FORMAT_R16G16B16A16_SFLOAT: i32 = 97;
pub const FORMAT_D24_UNORM_S8_UINT: i32 = 129;
pub const FORMAT_D32_SFLOAT: i32 = 126;
pub const FORMAT_D32_SFLOAT_S8_UINT: i32 = 130;

#[repr(C)]
pub struct VkDeviceCreateInfo {
    pub sType: u32,
    pub pNext: *const c_void,
    pub flags: u32,
    pub queueCreateInfoCount: u32,
    pub pQueueCreateInfos: *const c_void,
    pub enabledLayerCount: u32,
    pub ppEnabledLayerNames: *const *const c_char,
    pub enabledExtensionCount: u32,
    pub ppEnabledExtensionNames: *const *const c_char,
    pub pEnabledFeatures: *const c_void,
}

#[repr(C)]
pub struct VkExtensionProperties {
    pub extensionName: [c_char; 256],
    pub specVersion: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct VkExtent2D {
    pub width: u32,
    pub height: u32,
}

#[repr(C)]
pub struct VkSwapchainCreateInfoKHR {
    pub sType: u32,
    pub pNext: *const c_void,
    pub flags: u32,
    pub surface: VkSurfaceKHR,
    pub minImageCount: u32,
    pub imageFormat: i32,
    pub imageColorSpace: i32,
    pub imageExtent: VkExtent2D,
    pub imageArrayLayers: u32,
    pub imageUsage: u32,
    pub imageSharingMode: i32,
    pub queueFamilyIndexCount: u32,
    pub pQueueFamilyIndices: *const u32,
    pub preTransform: u32,
    pub compositeAlpha: u32,
    pub presentMode: i32,
    pub clipped: u32,
    pub oldSwapchain: VkSwapchainKHR,
}

#[repr(C)]
pub struct VkImageCreateInfo {
    pub sType: u32,
    pub pNext: *const c_void,
    pub flags: u32,
    pub imageType: i32,
    pub format: i32,
    pub extent: [u32; 3],
    pub mipLevels: u32,
    pub arrayLayers: u32,
    pub samples: u32,
    pub tiling: i32,
    pub usage: u32,
    pub sharingMode: i32,
    pub queueFamilyIndexCount: u32,
    pub pQueueFamilyIndices: *const u32,
    pub initialLayout: i32,
}

#[repr(C)]
pub struct VkPresentInfoKHR {
    pub sType: u32,
    pub pNext: *const c_void,
    pub waitSemaphoreCount: u32,
    pub pWaitSemaphores: *const VkSemaphore,
    pub swapchainCount: u32,
    pub pSwapchains: *const VkSwapchainKHR,
    pub pImageIndices: *const u32,
    pub pResults: *mut VkResult,
}

#[repr(C)]
pub struct VkBufferCreateInfo {
    pub sType: u32,
    pub pNext: *const c_void,
    pub flags: u32,
    pub size: u64,
    pub usage: u32,
    pub sharingMode: i32,
    pub queueFamilyIndexCount: u32,
    pub pQueueFamilyIndices: *const u32,
}

#[repr(C)]
pub struct VkExternalMemoryBufferCreateInfo {
    pub sType: u32,
    pub pNext: *const c_void,
    pub handleTypes: u32,
}

#[repr(C)]
#[derive(Default)]
pub struct VkMemoryRequirements {
    pub size: u64,
    pub alignment: u64,
    pub memoryTypeBits: u32,
}

#[repr(C)]
pub struct VkMemoryAllocateInfo {
    pub sType: u32,
    pub pNext: *const c_void,
    pub allocationSize: u64,
    pub memoryTypeIndex: u32,
}

#[repr(C)]
pub struct VkImportMemoryWin32HandleInfoKHR {
    pub sType: u32,
    pub pNext: *const c_void,
    pub handleType: u32,
    pub handle: *mut c_void,
    pub name: *const u16,
}

#[repr(C)]
pub struct VkMemoryDedicatedAllocateInfo {
    pub sType: u32,
    pub pNext: *const c_void,
    pub image: VkImage,
    pub buffer: VkBuffer,
}

#[repr(C)]
pub struct VkMemoryWin32HandlePropertiesKHR {
    pub sType: u32,
    pub pNext: *mut c_void,
    pub memoryTypeBits: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct VkMemoryType {
    pub propertyFlags: u32,
    pub heapIndex: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct VkMemoryHeap {
    pub size: u64,
    pub flags: u32,
}

#[repr(C)]
pub struct VkPhysicalDeviceMemoryProperties {
    pub memoryTypeCount: u32,
    pub memoryTypes: [VkMemoryType; 32],
    pub memoryHeapCount: u32,
    pub memoryHeaps: [VkMemoryHeap; 16],
}

#[repr(C)]
pub struct VkPhysicalDeviceIDProperties {
    pub sType: u32,
    pub pNext: *mut c_void,
    pub deviceUUID: [u8; 16],
    pub driverUUID: [u8; 16],
    pub deviceLUID: [u8; 8],
    pub deviceNodeMask: u32,
    pub deviceLUIDValid: u32,
}

#[repr(C)]
pub struct VkCommandPoolCreateInfo {
    pub sType: u32,
    pub pNext: *const c_void,
    pub flags: u32,
    pub queueFamilyIndex: u32,
}

#[repr(C)]
pub struct VkCommandBufferAllocateInfo {
    pub sType: u32,
    pub pNext: *const c_void,
    pub commandPool: VkCommandPool,
    pub level: i32,
    pub commandBufferCount: u32,
}

#[repr(C)]
pub struct VkCommandBufferBeginInfo {
    pub sType: u32,
    pub pNext: *const c_void,
    pub flags: u32,
    pub pInheritanceInfo: *const c_void,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct VkImageSubresourceRange {
    pub aspectMask: u32,
    pub baseMipLevel: u32,
    pub levelCount: u32,
    pub baseArrayLayer: u32,
    pub layerCount: u32,
}

#[repr(C)]
pub struct VkImageMemoryBarrier {
    pub sType: u32,
    pub pNext: *const c_void,
    pub srcAccessMask: u32,
    pub dstAccessMask: u32,
    pub oldLayout: i32,
    pub newLayout: i32,
    pub srcQueueFamilyIndex: u32,
    pub dstQueueFamilyIndex: u32,
    pub image: VkImage,
    pub subresourceRange: VkImageSubresourceRange,
}

#[repr(C)]
pub struct VkImageSubresourceLayers {
    pub aspectMask: u32,
    pub mipLevel: u32,
    pub baseArrayLayer: u32,
    pub layerCount: u32,
}

#[repr(C)]
pub struct VkBufferImageCopy {
    pub bufferOffset: u64,
    pub bufferRowLength: u32,
    pub bufferImageHeight: u32,
    pub imageSubresource: VkImageSubresourceLayers,
    pub imageOffset: [i32; 3],
    pub imageExtent: [u32; 3],
}

#[repr(C)]
pub struct VkSubmitInfo {
    pub sType: u32,
    pub pNext: *const c_void,
    pub waitSemaphoreCount: u32,
    pub pWaitSemaphores: *const VkSemaphore,
    pub pWaitDstStageMask: *const u32,
    pub commandBufferCount: u32,
    pub pCommandBuffers: *const VkCommandBuffer,
    pub signalSemaphoreCount: u32,
    pub pSignalSemaphores: *const VkSemaphore,
}

#[repr(C)]
pub struct VkD3D12FenceSubmitInfoKHR {
    pub sType: u32,
    pub pNext: *const c_void,
    pub waitSemaphoreValuesCount: u32,
    pub pWaitSemaphoreValues: *const u64,
    pub signalSemaphoreValuesCount: u32,
    pub pSignalSemaphoreValues: *const u64,
}

#[repr(C)]
pub struct VkSemaphoreCreateInfo {
    pub sType: u32,
    pub pNext: *const c_void,
    pub flags: u32,
}

#[repr(C)]
pub struct VkFenceCreateInfo {
    pub sType: u32,
    pub pNext: *const c_void,
    pub flags: u32,
}

#[repr(C)]
pub struct VkImportSemaphoreWin32HandleInfoKHR {
    pub sType: u32,
    pub pNext: *const c_void,
    pub semaphore: VkSemaphore,
    pub flags: u32,
    pub handleType: u32,
    pub handle: *mut c_void,
    pub name: *const u16,
}

// The game's own entry points (hooked).
pub type CreateDeviceFn = unsafe extern "system" fn(VkPhysicalDevice, *const VkDeviceCreateInfo, *const c_void, *mut VkDevice) -> VkResult;
pub type GetDeviceProcAddrFn = unsafe extern "system" fn(VkDevice, *const c_char) -> usize;
pub type GetDeviceQueueFn = unsafe extern "system" fn(VkDevice, u32, u32, *mut VkQueue);
pub type QueuePresentFn = unsafe extern "system" fn(VkQueue, *const VkPresentInfoKHR) -> VkResult;
pub type CreateImageFn = unsafe extern "system" fn(VkDevice, *const VkImageCreateInfo, *const c_void, *mut VkImage) -> VkResult;
pub type CreateSwapchainFn = unsafe extern "system" fn(VkDevice, *const VkSwapchainCreateInfoKHR, *const c_void, *mut VkSwapchainKHR) -> VkResult;

// Instance level, from vulkan-1.dll's exports.
pub type EnumerateDeviceExtensionPropertiesFn = unsafe extern "system" fn(VkPhysicalDevice, *const c_char, *mut u32, *mut VkExtensionProperties) -> VkResult;
pub type GetPhysicalDeviceMemoryPropertiesFn = unsafe extern "system" fn(VkPhysicalDevice, *mut VkPhysicalDeviceMemoryProperties);
pub type GetPhysicalDeviceProperties2Fn = unsafe extern "system" fn(VkPhysicalDevice, *mut c_void);

// Device level, from the device's own table.
pub type GetSwapchainImagesFn = unsafe extern "system" fn(VkDevice, VkSwapchainKHR, *mut u32, *mut VkImage) -> VkResult;
pub type CreateBufferFn = unsafe extern "system" fn(VkDevice, *const VkBufferCreateInfo, *const c_void, *mut VkBuffer) -> VkResult;
pub type GetBufferMemoryRequirementsFn = unsafe extern "system" fn(VkDevice, VkBuffer, *mut VkMemoryRequirements);
pub type GetMemoryWin32HandlePropertiesFn = unsafe extern "system" fn(VkDevice, u32, *mut c_void, *mut VkMemoryWin32HandlePropertiesKHR) -> VkResult;
pub type AllocateMemoryFn = unsafe extern "system" fn(VkDevice, *const VkMemoryAllocateInfo, *const c_void, *mut VkDeviceMemory) -> VkResult;
pub type BindBufferMemoryFn = unsafe extern "system" fn(VkDevice, VkBuffer, VkDeviceMemory, u64) -> VkResult;
pub type CreateCommandPoolFn = unsafe extern "system" fn(VkDevice, *const VkCommandPoolCreateInfo, *const c_void, *mut VkCommandPool) -> VkResult;
pub type AllocateCommandBuffersFn = unsafe extern "system" fn(VkDevice, *const VkCommandBufferAllocateInfo, *mut VkCommandBuffer) -> VkResult;
pub type BeginCommandBufferFn = unsafe extern "system" fn(VkCommandBuffer, *const VkCommandBufferBeginInfo) -> VkResult;
pub type EndCommandBufferFn = unsafe extern "system" fn(VkCommandBuffer) -> VkResult;
pub type ResetCommandBufferFn = unsafe extern "system" fn(VkCommandBuffer, u32) -> VkResult;
pub type CmdPipelineBarrierFn =
    unsafe extern "system" fn(VkCommandBuffer, u32, u32, u32, u32, *const c_void, u32, *const c_void, u32, *const VkImageMemoryBarrier);
pub type CmdCopyImageToBufferFn = unsafe extern "system" fn(VkCommandBuffer, VkImage, i32, VkBuffer, u32, *const VkBufferImageCopy);
pub type QueueSubmitFn = unsafe extern "system" fn(VkQueue, u32, *const VkSubmitInfo, VkFence) -> VkResult;
pub type CreateSemaphoreFn = unsafe extern "system" fn(VkDevice, *const VkSemaphoreCreateInfo, *const c_void, *mut VkSemaphore) -> VkResult;
pub type ImportSemaphoreWin32HandleFn = unsafe extern "system" fn(VkDevice, *const VkImportSemaphoreWin32HandleInfoKHR) -> VkResult;
pub type CreateFenceFn = unsafe extern "system" fn(VkDevice, *const VkFenceCreateInfo, *const c_void, *mut VkFence) -> VkResult;
pub type GetFenceStatusFn = unsafe extern "system" fn(VkDevice, VkFence) -> VkResult;
pub type ResetFencesFn = unsafe extern "system" fn(VkDevice, u32, *const VkFence) -> VkResult;

#[cfg(test)]
mod tests;
