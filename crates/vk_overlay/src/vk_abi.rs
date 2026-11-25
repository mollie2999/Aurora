//! Minimal Vulkan 1.0 + WSI + Khronos Loader Layer Interface v2 ABI types.
//! Zero external crate dependencies; matches the C Vulkan specification on x86_64 Linux.

#![allow(dead_code, non_camel_case_types, non_snake_case)]

use std::ffi::{c_char, c_ulong, c_void};

pub type VkResult = i32;
pub type VkBool32 = u32;
pub type VkFlags = u32;
pub type VkDeviceSize = u64;

pub type VkInstance = *mut c_void;
pub type VkPhysicalDevice = *mut c_void;
pub type VkDevice = *mut c_void;
pub type VkQueue = *mut c_void;
pub type VkCommandBuffer = *mut c_void;

pub type VkSurfaceKHR = u64;
pub type VkSwapchainKHR = u64;
pub type VkImage = u64;
pub type VkImageView = u64;
pub type VkBuffer = u64;
pub type VkDeviceMemory = u64;
pub type VkRenderPass = u64;
pub type VkFramebuffer = u64;
pub type VkPipelineLayout = u64;
pub type VkPipeline = u64;
pub type VkShaderModule = u64;
pub type VkDescriptorSetLayout = u64;
pub type VkDescriptorPool = u64;
pub type VkDescriptorSet = u64;
pub type VkSampler = u64;
pub type VkCommandPool = u64;
pub type VkSemaphore = u64;
pub type VkFence = u64;

pub type PFN_vkVoidFunction = Option<unsafe extern "C" fn()>;

pub const VK_SUCCESS: VkResult = 0;
pub const VK_SUBOPTIMAL_KHR: VkResult = 1000001003;
pub const VK_FALSE: VkBool32 = 0;
pub const VK_TRUE: VkBool32 = 1;

// Loader layer interface constants
pub const VK_STRUCTURE_TYPE_LOADER_INSTANCE_CREATE_INFO: i32 = 47;
pub const VK_STRUCTURE_TYPE_LOADER_DEVICE_CREATE_INFO: i32 = 48;
pub const VK_LAYER_LINK_INFO: u32 = 0;
pub const LAYER_NEGOTIATE_INTERFACE_STRUCT: i32 = 0;
pub const CURRENT_LOADER_LAYER_INTERFACE_VERSION: u32 = 2;

// Core / WSI structure types
pub const VK_STRUCTURE_TYPE_SUBMIT_INFO: i32 = 4;
pub const VK_STRUCTURE_TYPE_MEMORY_ALLOCATE_INFO: i32 = 5;
pub const VK_STRUCTURE_TYPE_FENCE_CREATE_INFO: i32 = 8;
pub const VK_STRUCTURE_TYPE_SEMAPHORE_CREATE_INFO: i32 = 9;
pub const VK_STRUCTURE_TYPE_BUFFER_CREATE_INFO: i32 = 12;
pub const VK_STRUCTURE_TYPE_IMAGE_CREATE_INFO: i32 = 14;
pub const VK_STRUCTURE_TYPE_IMAGE_VIEW_CREATE_INFO: i32 = 15;
pub const VK_STRUCTURE_TYPE_SHADER_MODULE_CREATE_INFO: i32 = 16;
pub const VK_STRUCTURE_TYPE_PIPELINE_SHADER_STAGE_CREATE_INFO: i32 = 18;
pub const VK_STRUCTURE_TYPE_PIPELINE_VERTEX_INPUT_STATE_CREATE_INFO: i32 = 19;
pub const VK_STRUCTURE_TYPE_PIPELINE_INPUT_ASSEMBLY_STATE_CREATE_INFO: i32 = 20;
pub const VK_STRUCTURE_TYPE_PIPELINE_VIEWPORT_STATE_CREATE_INFO: i32 = 22;
pub const VK_STRUCTURE_TYPE_PIPELINE_RASTERIZATION_STATE_CREATE_INFO: i32 = 23;
pub const VK_STRUCTURE_TYPE_PIPELINE_MULTISAMPLE_STATE_CREATE_INFO: i32 = 24;
pub const VK_STRUCTURE_TYPE_PIPELINE_COLOR_BLEND_STATE_CREATE_INFO: i32 = 26;
pub const VK_STRUCTURE_TYPE_PIPELINE_DYNAMIC_STATE_CREATE_INFO: i32 = 27;
pub const VK_STRUCTURE_TYPE_GRAPHICS_PIPELINE_CREATE_INFO: i32 = 28;
pub const VK_STRUCTURE_TYPE_PIPELINE_LAYOUT_CREATE_INFO: i32 = 30;
pub const VK_STRUCTURE_TYPE_SAMPLER_CREATE_INFO: i32 = 31;
pub const VK_STRUCTURE_TYPE_DESCRIPTOR_SET_LAYOUT_CREATE_INFO: i32 = 32;
pub const VK_STRUCTURE_TYPE_DESCRIPTOR_POOL_CREATE_INFO: i32 = 33;
pub const VK_STRUCTURE_TYPE_DESCRIPTOR_SET_ALLOCATE_INFO: i32 = 34;
pub const VK_STRUCTURE_TYPE_WRITE_DESCRIPTOR_SET: i32 = 35;
pub const VK_STRUCTURE_TYPE_FRAMEBUFFER_CREATE_INFO: i32 = 37;
pub const VK_STRUCTURE_TYPE_RENDER_PASS_CREATE_INFO: i32 = 38;
pub const VK_STRUCTURE_TYPE_COMMAND_POOL_CREATE_INFO: i32 = 39;
pub const VK_STRUCTURE_TYPE_COMMAND_BUFFER_ALLOCATE_INFO: i32 = 40;
pub const VK_STRUCTURE_TYPE_COMMAND_BUFFER_BEGIN_INFO: i32 = 42;
pub const VK_STRUCTURE_TYPE_RENDER_PASS_BEGIN_INFO: i32 = 43;
pub const VK_STRUCTURE_TYPE_IMAGE_MEMORY_BARRIER: i32 = 45;

// Formats
pub const VK_FORMAT_R8G8B8A8_UNORM: i32 = 37;
pub const VK_FORMAT_R8G8B8A8_SRGB: i32 = 43;
pub const VK_FORMAT_B8G8R8A8_UNORM: i32 = 44;
pub const VK_FORMAT_B8G8R8A8_SRGB: i32 = 50;
pub const VK_FORMAT_A2R10G10B10_UNORM_PACK32: i32 = 58;
pub const VK_FORMAT_A2B10G10R10_UNORM_PACK32: i32 = 64;
pub const VK_FORMAT_R32G32_SFLOAT: i32 = 103;
pub const VK_FORMAT_R16G16B16A16_SFLOAT: i32 = 97;

// Layouts
pub const VK_IMAGE_LAYOUT_UNDEFINED: i32 = 0;
pub const VK_IMAGE_LAYOUT_COLOR_ATTACHMENT_OPTIMAL: i32 = 2;
pub const VK_IMAGE_LAYOUT_SHADER_READ_ONLY_OPTIMAL: i32 = 5;
pub const VK_IMAGE_LAYOUT_TRANSFER_SRC_OPTIMAL: i32 = 6;
pub const VK_IMAGE_LAYOUT_TRANSFER_DST_OPTIMAL: i32 = 7;
pub const VK_IMAGE_LAYOUT_PRESENT_SRC_KHR: i32 = 1000001002;

// Usage / Flags
pub const VK_IMAGE_USAGE_TRANSFER_SRC_BIT: VkFlags = 0x0000_0001;
pub const VK_IMAGE_USAGE_TRANSFER_DST_BIT: VkFlags = 0x0000_0002;
pub const VK_IMAGE_USAGE_SAMPLED_BIT: VkFlags = 0x0000_0004;
pub const VK_IMAGE_USAGE_COLOR_ATTACHMENT_BIT: VkFlags = 0x0000_0010;

pub const VK_BUFFER_USAGE_TRANSFER_SRC_BIT: VkFlags = 0x0000_0001;
pub const VK_BUFFER_USAGE_TRANSFER_DST_BIT: VkFlags = 0x0000_0002;
pub const VK_BUFFER_USAGE_INDEX_BUFFER_BIT: VkFlags = 0x0000_0040;
pub const VK_BUFFER_USAGE_VERTEX_BUFFER_BIT: VkFlags = 0x0000_0080;

pub const VK_MEMORY_PROPERTY_DEVICE_LOCAL_BIT: VkFlags = 0x0000_0001;
pub const VK_MEMORY_PROPERTY_HOST_VISIBLE_BIT: VkFlags = 0x0000_0002;
pub const VK_MEMORY_PROPERTY_HOST_COHERENT_BIT: VkFlags = 0x0000_0004;

pub const VK_IMAGE_ASPECT_COLOR_BIT: VkFlags = 0x0000_0001;
pub const VK_SHADER_STAGE_VERTEX_BIT: VkFlags = 0x0000_0001;
pub const VK_SHADER_STAGE_FRAGMENT_BIT: VkFlags = 0x0000_0010;
pub const VK_DESCRIPTOR_TYPE_COMBINED_IMAGE_SAMPLER: i32 = 1;
pub const VK_DESCRIPTOR_POOL_CREATE_FREE_DESCRIPTOR_SET_BIT: VkFlags = 0x0000_0001;

pub const VK_PIPELINE_STAGE_TOP_OF_PIPE_BIT: VkFlags = 0x0000_0001;
pub const VK_PIPELINE_STAGE_FRAGMENT_SHADER_BIT: VkFlags = 0x0000_0080;
pub const VK_PIPELINE_STAGE_COLOR_ATTACHMENT_OUTPUT_BIT: VkFlags = 0x0000_0400;
pub const VK_PIPELINE_STAGE_TRANSFER_BIT: VkFlags = 0x0000_1000;

pub const VK_ACCESS_SHADER_READ_BIT: VkFlags = 0x0000_0020;
pub const VK_ACCESS_COLOR_ATTACHMENT_READ_BIT: VkFlags = 0x0000_0080;
pub const VK_ACCESS_COLOR_ATTACHMENT_WRITE_BIT: VkFlags = 0x0000_0100;
pub const VK_ACCESS_TRANSFER_READ_BIT: VkFlags = 0x0000_0800;
pub const VK_ACCESS_TRANSFER_WRITE_BIT: VkFlags = 0x0000_1000;

pub const VK_COMMAND_POOL_CREATE_RESET_COMMAND_BUFFER_BIT: VkFlags = 0x0000_0002;
pub const VK_COMMAND_BUFFER_USAGE_ONE_TIME_SUBMIT_BIT: VkFlags = 0x0000_0001;
pub const VK_FENCE_CREATE_SIGNALED_BIT: VkFlags = 0x0000_0001;
pub const VK_SUBPASS_EXTERNAL: u32 = !0u32;
pub const VK_QUEUE_FAMILY_IGNORED: u32 = !0u32;

pub const VK_ATTACHMENT_LOAD_OP_LOAD: i32 = 0;
pub const VK_ATTACHMENT_LOAD_OP_DONT_CARE: i32 = 2;
pub const VK_ATTACHMENT_STORE_OP_STORE: i32 = 0;
pub const VK_ATTACHMENT_STORE_OP_DONT_CARE: i32 = 1;

pub const VK_PIPELINE_BIND_POINT_GRAPHICS: i32 = 0;
pub const VK_SUBPASS_CONTENTS_INLINE: i32 = 0;
pub const VK_INDEX_TYPE_UINT32: i32 = 1;

pub const VK_PRIMITIVE_TOPOLOGY_TRIANGLE_LIST: i32 = 3;
pub const VK_POLYGON_MODE_FILL: i32 = 0;
pub const VK_CULL_MODE_NONE: VkFlags = 0;
pub const VK_FRONT_FACE_COUNTER_CLOCKWISE: i32 = 0;
pub const VK_SAMPLE_COUNT_1_BIT: VkFlags = 1;
pub const VK_BLEND_FACTOR_ONE: i32 = 1;
pub const VK_BLEND_FACTOR_ONE_MINUS_SRC_ALPHA: i32 = 7;
pub const VK_BLEND_OP_ADD: i32 = 0;
pub const VK_COLOR_COMPONENT_RGBA: VkFlags = 0xF;
pub const VK_DYNAMIC_STATE_VIEWPORT: i32 = 0;
pub const VK_DYNAMIC_STATE_SCISSOR: i32 = 1;
pub const VK_FILTER_NEAREST: i32 = 0;
pub const VK_FILTER_LINEAR: i32 = 1;
pub const VK_SAMPLER_ADDRESS_MODE_CLAMP_TO_EDGE: i32 = 2;

/// Extract the loader dispatch table key from any dispatchable Vulkan handle
/// (`VkInstance`, `VkPhysicalDevice`, `VkDevice`, `VkQueue`, `VkCommandBuffer`).
#[inline]
pub unsafe fn dispatch_key(handle: *mut c_void) -> usize {
    if handle.is_null() {
        0
    } else {
        unsafe { *(handle as *const usize) }
    }
}

/// Returns true if a swapchain format expects linear RGB output (sRGB or float).
#[inline]
pub fn is_linear_output_format(fmt: i32) -> bool {
    matches!(
        fmt,
        VK_FORMAT_R8G8B8A8_SRGB
            | VK_FORMAT_B8G8R8A8_SRGB
            | VK_FORMAT_R16G16B16A16_SFLOAT
    )
}

// ---- Loader Layer Structs --------------------------------------------------

#[repr(C)]
pub struct VkBaseInStructure {
    pub sType: i32,
    pub pNext: *const VkBaseInStructure,
}

#[repr(C)]
pub struct VkLayerInstanceLink {
    pub pNext: *mut VkLayerInstanceLink,
    pub pfnNextGetInstanceProcAddr: PFN_vkGetInstanceProcAddr,
    pub pfnNextGetPhysicalDeviceProcAddr: *const c_void,
}

#[repr(C)]
pub struct VkLayerInstanceCreateInfo {
    pub sType: i32,
    pub pNext: *const c_void,
    pub function: u32,
    pub pLayerInfo: *mut VkLayerInstanceLink,
}

#[repr(C)]
pub struct VkLayerDeviceLink {
    pub pNext: *mut VkLayerDeviceLink,
    pub pfnNextGetInstanceProcAddr: PFN_vkGetInstanceProcAddr,
    pub pfnNextGetDeviceProcAddr: PFN_vkGetDeviceProcAddr,
}

#[repr(C)]
pub struct VkLayerDeviceCreateInfo {
    pub sType: i32,
    pub pNext: *const c_void,
    pub function: u32,
    pub pLayerInfo: *mut VkLayerDeviceLink,
}

#[repr(C)]
pub struct VkNegotiateLayerInterface {
    pub sType: i32,
    pub pNext: *mut c_void,
    pub loaderLayerInterfaceVersion: u32,
    pub pfnGetInstanceProcAddr: Option<PFN_vkGetInstanceProcAddr>,
    pub pfnGetDeviceProcAddr: Option<PFN_vkGetDeviceProcAddr>,
    pub pfnGetPhysicalDeviceProcAddr: *const c_void,
}

// ---- WSI / Core Structs ----------------------------------------------------

#[repr(C)]
#[derive(Copy, Clone, Default)]
pub struct VkExtent2D {
    pub width: u32,
    pub height: u32,
}

#[repr(C)]
#[derive(Copy, Clone, Default)]
pub struct VkExtent3D {
    pub width: u32,
    pub height: u32,
    pub depth: u32,
}

#[repr(C)]
#[derive(Copy, Clone, Default)]
pub struct VkOffset2D {
    pub x: i32,
    pub y: i32,
}

#[repr(C)]
#[derive(Copy, Clone, Default)]
pub struct VkOffset3D {
    pub x: i32,
    pub y: i32,
    pub z: i32,
}

#[repr(C)]
#[derive(Copy, Clone, Default)]
pub struct VkRect2D {
    pub offset: VkOffset2D,
    pub extent: VkExtent2D,
}

#[repr(C)]
#[derive(Copy, Clone, Default)]
pub struct VkViewport {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub minDepth: f32,
    pub maxDepth: f32,
}

#[repr(C)]
pub struct VkXlibSurfaceCreateInfoKHR {
    pub sType: i32,
    pub pNext: *const c_void,
    pub flags: VkFlags,
    pub dpy: *mut c_void,
    pub window: c_ulong,
}

#[repr(C)]
pub struct VkXcbSurfaceCreateInfoKHR {
    pub sType: i32,
    pub pNext: *const c_void,
    pub flags: VkFlags,
    pub connection: *mut c_void,
    pub window: u32,
}

#[repr(C)]
pub struct VkWaylandSurfaceCreateInfoKHR {
    pub sType: i32,
    pub pNext: *const c_void,
    pub flags: VkFlags,
    pub display: *mut c_void,
    pub surface: *mut c_void,
}

#[repr(C)]
pub struct VkSurfaceCapabilitiesKHR {
    pub minImageCount: u32,
    pub maxImageCount: u32,
    pub currentExtent: VkExtent2D,
    pub minImageExtent: VkExtent2D,
    pub maxImageExtent: VkExtent2D,
    pub maxImageArrayLayers: u32,
    pub supportedTransforms: VkFlags,
    pub currentTransform: VkFlags,
    pub supportedCompositeAlpha: VkFlags,
    pub supportedUsageFlags: VkFlags,
}

#[repr(C)]
pub struct VkDeviceQueueCreateInfo {
    pub sType: i32,
    pub pNext: *const c_void,
    pub flags: VkFlags,
    pub queueFamilyIndex: u32,
    pub queueCount: u32,
    pub pQueuePriorities: *const f32,
}

#[repr(C)]
pub struct VkDeviceCreateInfo {
    pub sType: i32,
    pub pNext: *const c_void,
    pub flags: VkFlags,
    pub queueCreateInfoCount: u32,
    pub pQueueCreateInfos: *const VkDeviceQueueCreateInfo,
    pub enabledLayerCount: u32,
    pub ppEnabledLayerNames: *const *const c_char,
    pub enabledExtensionCount: u32,
    pub ppEnabledExtensionNames: *const *const c_char,
    pub pEnabledFeatures: *const c_void,
}

#[repr(C)]
pub struct VkDeviceQueueInfo2 {
    pub sType: i32,
    pub pNext: *const c_void,
    pub flags: VkFlags,
    pub queueFamilyIndex: u32,
    pub queueIndex: u32,
}

#[repr(C)]
#[derive(Copy, Clone)]
pub struct VkSwapchainCreateInfoKHR {
    pub sType: i32,
    pub pNext: *const c_void,
    pub flags: VkFlags,
    pub surface: VkSurfaceKHR,
    pub minImageCount: u32,
    pub imageFormat: i32,
    pub imageColorSpace: i32,
    pub imageExtent: VkExtent2D,
    pub imageArrayLayers: u32,
    pub imageUsage: VkFlags,
    pub imageSharingMode: i32,
    pub queueFamilyIndexCount: u32,
    pub pQueueFamilyIndices: *const u32,
    pub preTransform: VkFlags,
    pub compositeAlpha: VkFlags,
    pub presentMode: i32,
    pub clipped: VkBool32,
    pub oldSwapchain: VkSwapchainKHR,
}

#[repr(C)]
#[derive(Copy, Clone)]
pub struct VkPresentInfoKHR {
    pub sType: i32,
    pub pNext: *const c_void,
    pub waitSemaphoreCount: u32,
    pub pWaitSemaphores: *const VkSemaphore,
    pub swapchainCount: u32,
    pub pSwapchains: *const VkSwapchainKHR,
    pub pImageIndices: *const u32,
    pub pResults: *mut VkResult,
}

#[repr(C)]
#[derive(Copy, Clone, Default)]
pub struct VkMemoryType {
    pub propertyFlags: VkFlags,
    pub heapIndex: u32,
}

#[repr(C)]
#[derive(Copy, Clone, Default)]
pub struct VkMemoryHeap {
    pub size: VkDeviceSize,
    pub flags: VkFlags,
}

#[repr(C)]
#[derive(Copy, Clone)]
pub struct VkPhysicalDeviceMemoryProperties {
    pub memoryTypeCount: u32,
    pub memoryTypes: [VkMemoryType; 32],
    pub memoryHeapCount: u32,
    pub memoryHeaps: [VkMemoryHeap; 16],
}

impl Default for VkPhysicalDeviceMemoryProperties {
    fn default() -> Self {
        unsafe { std::mem::zeroed() }
    }
}

#[repr(C)]
#[derive(Copy, Clone, Default)]
pub struct VkMemoryRequirements {
    pub size: VkDeviceSize,
    pub alignment: VkDeviceSize,
    pub memoryTypeBits: u32,
}

#[repr(C)]
pub struct VkMemoryAllocateInfo {
    pub sType: i32,
    pub pNext: *const c_void,
    pub allocationSize: VkDeviceSize,
    pub memoryTypeIndex: u32,
}

#[repr(C)]
pub struct VkBufferCreateInfo {
    pub sType: i32,
    pub pNext: *const c_void,
    pub flags: VkFlags,
    pub size: VkDeviceSize,
    pub usage: VkFlags,
    pub sharingMode: i32,
    pub queueFamilyIndexCount: u32,
    pub pQueueFamilyIndices: *const u32,
}

#[repr(C)]
pub struct VkImageCreateInfo {
    pub sType: i32,
    pub pNext: *const c_void,
    pub flags: VkFlags,
    pub imageType: i32,
    pub format: i32,
    pub extent: VkExtent3D,
    pub mipLevels: u32,
    pub arrayLayers: u32,
    pub samples: VkFlags,
    pub tiling: i32,
    pub usage: VkFlags,
    pub sharingMode: i32,
    pub queueFamilyIndexCount: u32,
    pub pQueueFamilyIndices: *const u32,
    pub initialLayout: i32,
}

#[repr(C)]
#[derive(Copy, Clone)]
pub struct VkImageSubresourceRange {
    pub aspectMask: VkFlags,
    pub baseMipLevel: u32,
    pub levelCount: u32,
    pub baseArrayLayer: u32,
    pub layerCount: u32,
}

#[repr(C)]
#[derive(Copy, Clone)]
pub struct VkImageSubresourceLayers {
    pub aspectMask: VkFlags,
    pub mipLevel: u32,
    pub baseArrayLayer: u32,
    pub layerCount: u32,
}

#[repr(C)]
pub struct VkBufferImageCopy {
    pub bufferOffset: VkDeviceSize,
    pub bufferRowLength: u32,
    pub bufferImageHeight: u32,
    pub imageSubresource: VkImageSubresourceLayers,
    pub imageOffset: VkOffset3D,
    pub imageExtent: VkExtent3D,
}

#[repr(C)]
pub struct VkImageMemoryBarrier {
    pub sType: i32,
    pub pNext: *const c_void,
    pub srcAccessMask: VkFlags,
    pub dstAccessMask: VkFlags,
    pub oldLayout: i32,
    pub newLayout: i32,
    pub srcQueueFamilyIndex: u32,
    pub dstQueueFamilyIndex: u32,
    pub image: VkImage,
    pub subresourceRange: VkImageSubresourceRange,
}

#[repr(C)]
#[derive(Copy, Clone, Default)]
pub struct VkComponentMapping {
    pub r: i32,
    pub g: i32,
    pub b: i32,
    pub a: i32,
}

#[repr(C)]
pub struct VkImageViewCreateInfo {
    pub sType: i32,
    pub pNext: *const c_void,
    pub flags: VkFlags,
    pub image: VkImage,
    pub viewType: i32,
    pub format: i32,
    pub components: VkComponentMapping,
    pub subresourceRange: VkImageSubresourceRange,
}

#[repr(C)]
pub struct VkShaderModuleCreateInfo {
    pub sType: i32,
    pub pNext: *const c_void,
    pub flags: VkFlags,
    pub codeSize: usize,
    pub pCode: *const u32,
}

#[repr(C)]
pub struct VkAttachmentDescription {
    pub flags: VkFlags,
    pub format: i32,
    pub samples: VkFlags,
    pub loadOp: i32,
    pub storeOp: i32,
    pub stencilLoadOp: i32,
    pub stencilStoreOp: i32,
    pub initialLayout: i32,
    pub finalLayout: i32,
}

#[repr(C)]
pub struct VkAttachmentReference {
    pub attachment: u32,
    pub layout: i32,
}

#[repr(C)]
pub struct VkSubpassDescription {
    pub flags: VkFlags,
    pub pipelineBindPoint: i32,
    pub inputAttachmentCount: u32,
    pub pInputAttachments: *const VkAttachmentReference,
    pub colorAttachmentCount: u32,
    pub pColorAttachments: *const VkAttachmentReference,
    pub pResolveAttachments: *const VkAttachmentReference,
    pub pDepthStencilAttachment: *const VkAttachmentReference,
    pub preserveAttachmentCount: u32,
    pub pPreserveAttachments: *const u32,
}

#[repr(C)]
pub struct VkSubpassDependency {
    pub srcSubpass: u32,
    pub dstSubpass: u32,
    pub srcStageMask: VkFlags,
    pub dstStageMask: VkFlags,
    pub srcAccessMask: VkFlags,
    pub dstAccessMask: VkFlags,
    pub dependencyFlags: VkFlags,
}

#[repr(C)]
pub struct VkRenderPassCreateInfo {
    pub sType: i32,
    pub pNext: *const c_void,
    pub flags: VkFlags,
    pub attachmentCount: u32,
    pub pAttachments: *const VkAttachmentDescription,
    pub subpassCount: u32,
    pub pSubpasses: *const VkSubpassDescription,
    pub dependencyCount: u32,
    pub pDependencies: *const VkSubpassDependency,
}

#[repr(C)]
pub struct VkFramebufferCreateInfo {
    pub sType: i32,
    pub pNext: *const c_void,
    pub flags: VkFlags,
    pub renderPass: VkRenderPass,
    pub attachmentCount: u32,
    pub pAttachments: *const VkImageView,
    pub width: u32,
    pub height: u32,
    pub layers: u32,
}

#[repr(C)]
pub struct VkDescriptorSetLayoutBinding {
    pub binding: u32,
    pub descriptorType: i32,
    pub descriptorCount: u32,
    pub stageFlags: VkFlags,
    pub pImmutableSamplers: *const VkSampler,
}

#[repr(C)]
pub struct VkDescriptorSetLayoutCreateInfo {
    pub sType: i32,
    pub pNext: *const c_void,
    pub flags: VkFlags,
    pub bindingCount: u32,
    pub pBindings: *const VkDescriptorSetLayoutBinding,
}

#[repr(C)]
pub struct VkDescriptorPoolSize {
    pub ty: i32,
    pub descriptorCount: u32,
}

#[repr(C)]
pub struct VkDescriptorPoolCreateInfo {
    pub sType: i32,
    pub pNext: *const c_void,
    pub flags: VkFlags,
    pub maxSets: u32,
    pub poolSizeCount: u32,
    pub pPoolSizes: *const VkDescriptorPoolSize,
}

#[repr(C)]
pub struct VkDescriptorSetAllocateInfo {
    pub sType: i32,
    pub pNext: *const c_void,
    pub descriptorPool: VkDescriptorPool,
    pub descriptorSetCount: u32,
    pub pSetLayouts: *const VkDescriptorSetLayout,
}

#[repr(C)]
pub struct VkDescriptorImageInfo {
    pub sampler: VkSampler,
    pub imageView: VkImageView,
    pub imageLayout: i32,
}

#[repr(C)]
pub struct VkWriteDescriptorSet {
    pub sType: i32,
    pub pNext: *const c_void,
    pub dstSet: VkDescriptorSet,
    pub dstBinding: u32,
    pub dstArrayElement: u32,
    pub descriptorCount: u32,
    pub descriptorType: i32,
    pub pImageInfo: *const VkDescriptorImageInfo,
    pub pBufferInfo: *const c_void,
    pub pTexelBufferView: *const c_void,
}

#[repr(C)]
pub struct VkSamplerCreateInfo {
    pub sType: i32,
    pub pNext: *const c_void,
    pub flags: VkFlags,
    pub magFilter: i32,
    pub minFilter: i32,
    pub mipmapMode: i32,
    pub addressModeU: i32,
    pub addressModeV: i32,
    pub addressModeW: i32,
    pub mipLodBias: f32,
    pub anisotropyEnable: VkBool32,
    pub maxAnisotropy: f32,
    pub compareEnable: VkBool32,
    pub compareOp: i32,
    pub minLod: f32,
    pub maxLod: f32,
    pub borderColor: i32,
    pub unnormalizedCoordinates: VkBool32,
}

#[repr(C)]
pub struct VkPushConstantRange {
    pub stageFlags: VkFlags,
    pub offset: u32,
    pub size: u32,
}

#[repr(C)]
pub struct VkPipelineLayoutCreateInfo {
    pub sType: i32,
    pub pNext: *const c_void,
    pub flags: VkFlags,
    pub setLayoutCount: u32,
    pub pSetLayouts: *const VkDescriptorSetLayout,
    pub pushConstantRangeCount: u32,
    pub pPushConstantRanges: *const VkPushConstantRange,
}

#[repr(C)]
pub struct VkPipelineShaderStageCreateInfo {
    pub sType: i32,
    pub pNext: *const c_void,
    pub flags: VkFlags,
    pub stage: VkFlags,
    pub module: VkShaderModule,
    pub pName: *const c_char,
    pub pSpecializationInfo: *const c_void,
}

#[repr(C)]
pub struct VkVertexInputBindingDescription {
    pub binding: u32,
    pub stride: u32,
    pub inputRate: i32,
}

#[repr(C)]
pub struct VkVertexInputAttributeDescription {
    pub location: u32,
    pub binding: u32,
    pub format: i32,
    pub offset: u32,
}

#[repr(C)]
pub struct VkPipelineVertexInputStateCreateInfo {
    pub sType: i32,
    pub pNext: *const c_void,
    pub flags: VkFlags,
    pub vertexBindingDescriptionCount: u32,
    pub pVertexBindingDescriptions: *const VkVertexInputBindingDescription,
    pub vertexAttributeDescriptionCount: u32,
    pub pVertexAttributeDescriptions: *const VkVertexInputAttributeDescription,
}

#[repr(C)]
pub struct VkPipelineInputAssemblyStateCreateInfo {
    pub sType: i32,
    pub pNext: *const c_void,
    pub flags: VkFlags,
    pub topology: i32,
    pub primitiveRestartEnable: VkBool32,
}

#[repr(C)]
pub struct VkPipelineViewportStateCreateInfo {
    pub sType: i32,
    pub pNext: *const c_void,
    pub flags: VkFlags,
    pub viewportCount: u32,
    pub pViewports: *const VkViewport,
    pub scissorCount: u32,
    pub pScissors: *const VkRect2D,
}

#[repr(C)]
pub struct VkPipelineRasterizationStateCreateInfo {
    pub sType: i32,
    pub pNext: *const c_void,
    pub flags: VkFlags,
    pub depthClampEnable: VkBool32,
    pub rasterizerDiscardEnable: VkBool32,
    pub polygonMode: i32,
    pub cullMode: VkFlags,
    pub frontFace: i32,
    pub depthBiasEnable: VkBool32,
    pub depthBiasConstantFactor: f32,
    pub depthBiasClamp: f32,
    pub depthBiasSlopeFactor: f32,
    pub lineWidth: f32,
}

#[repr(C)]
pub struct VkPipelineMultisampleStateCreateInfo {
    pub sType: i32,
    pub pNext: *const c_void,
    pub flags: VkFlags,
    pub rasterizationSamples: VkFlags,
    pub sampleShadingEnable: VkBool32,
    pub minSampleShading: f32,
    pub pSampleMask: *const u32,
    pub alphaToCoverageEnable: VkBool32,
    pub alphaToOneEnable: VkBool32,
}

#[repr(C)]
pub struct VkPipelineColorBlendAttachmentState {
    pub blendEnable: VkBool32,
    pub srcColorBlendFactor: i32,
    pub dstColorBlendFactor: i32,
    pub colorBlendOp: i32,
    pub srcAlphaBlendFactor: i32,
    pub dstAlphaBlendFactor: i32,
    pub alphaBlendOp: i32,
    pub colorWriteMask: VkFlags,
}

#[repr(C)]
pub struct VkPipelineColorBlendStateCreateInfo {
    pub sType: i32,
    pub pNext: *const c_void,
    pub flags: VkFlags,
    pub logicOpEnable: VkBool32,
    pub logicOp: i32,
    pub attachmentCount: u32,
    pub pAttachments: *const VkPipelineColorBlendAttachmentState,
    pub blendConstants: [f32; 4],
}

#[repr(C)]
pub struct VkPipelineDynamicStateCreateInfo {
    pub sType: i32,
    pub pNext: *const c_void,
    pub flags: VkFlags,
    pub dynamicStateCount: u32,
    pub pDynamicStates: *const i32,
}

#[repr(C)]
pub struct VkGraphicsPipelineCreateInfo {
    pub sType: i32,
    pub pNext: *const c_void,
    pub flags: VkFlags,
    pub stageCount: u32,
    pub pStages: *const VkPipelineShaderStageCreateInfo,
    pub pVertexInputState: *const VkPipelineVertexInputStateCreateInfo,
    pub pInputAssemblyState: *const VkPipelineInputAssemblyStateCreateInfo,
    pub pTessellationState: *const c_void,
    pub pViewportState: *const VkPipelineViewportStateCreateInfo,
    pub pRasterizationState: *const VkPipelineRasterizationStateCreateInfo,
    pub pMultisampleState: *const VkPipelineMultisampleStateCreateInfo,
    pub pDepthStencilState: *const c_void,
    pub pColorBlendState: *const VkPipelineColorBlendStateCreateInfo,
    pub pDynamicState: *const VkPipelineDynamicStateCreateInfo,
    pub layout: VkPipelineLayout,
    pub renderPass: VkRenderPass,
    pub subpass: u32,
    pub basePipelineHandle: VkPipeline,
    pub basePipelineIndex: i32,
}

#[repr(C)]
pub struct VkCommandPoolCreateInfo {
    pub sType: i32,
    pub pNext: *const c_void,
    pub flags: VkFlags,
    pub queueFamilyIndex: u32,
}

#[repr(C)]
pub struct VkCommandBufferAllocateInfo {
    pub sType: i32,
    pub pNext: *const c_void,
    pub commandPool: VkCommandPool,
    pub level: i32,
    pub commandBufferCount: u32,
}

#[repr(C)]
pub struct VkCommandBufferBeginInfo {
    pub sType: i32,
    pub pNext: *const c_void,
    pub flags: VkFlags,
    pub pInheritanceInfo: *const c_void,
}

#[repr(C)]
pub struct VkRenderPassBeginInfo {
    pub sType: i32,
    pub pNext: *const c_void,
    pub renderPass: VkRenderPass,
    pub framebuffer: VkFramebuffer,
    pub renderArea: VkRect2D,
    pub clearValueCount: u32,
    pub pClearValues: *const c_void,
}

#[repr(C)]
pub struct VkSemaphoreCreateInfo {
    pub sType: i32,
    pub pNext: *const c_void,
    pub flags: VkFlags,
}

#[repr(C)]
pub struct VkFenceCreateInfo {
    pub sType: i32,
    pub pNext: *const c_void,
    pub flags: VkFlags,
}

#[repr(C)]
pub struct VkSubmitInfo {
    pub sType: i32,
    pub pNext: *const c_void,
    pub waitSemaphoreCount: u32,
    pub pWaitSemaphores: *const VkSemaphore,
    pub pWaitDstStageMask: *const VkFlags,
    pub commandBufferCount: u32,
    pub pCommandBuffers: *const VkCommandBuffer,
    pub signalSemaphoreCount: u32,
    pub pSignalSemaphores: *const VkSemaphore,
}

// ---- Function Pointer Types ------------------------------------------------

pub type PFN_vkGetInstanceProcAddr = unsafe extern "C" fn(VkInstance, *const c_char) -> PFN_vkVoidFunction;
pub type PFN_vkGetDeviceProcAddr = unsafe extern "C" fn(VkDevice, *const c_char) -> PFN_vkVoidFunction;
pub type PFN_vkCreateInstance = unsafe extern "C" fn(*const c_void, *const c_void, *mut VkInstance) -> VkResult;
pub type PFN_vkDestroyInstance = unsafe extern "C" fn(VkInstance, *const c_void);
pub type PFN_vkCreateXlibSurfaceKHR = unsafe extern "C" fn(VkInstance, *const VkXlibSurfaceCreateInfoKHR, *const c_void, *mut VkSurfaceKHR) -> VkResult;
pub type PFN_vkCreateXcbSurfaceKHR = unsafe extern "C" fn(VkInstance, *const VkXcbSurfaceCreateInfoKHR, *const c_void, *mut VkSurfaceKHR) -> VkResult;
pub type PFN_vkCreateWaylandSurfaceKHR = unsafe extern "C" fn(VkInstance, *const VkWaylandSurfaceCreateInfoKHR, *const c_void, *mut VkSurfaceKHR) -> VkResult;
pub type PFN_vkDestroySurfaceKHR = unsafe extern "C" fn(VkInstance, VkSurfaceKHR, *const c_void);
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct VkQueueFamilyProperties {
    pub queueFlags: u32,
    pub queueCount: u32,
    pub timestampValidBits: u32,
    pub minImageTransferGranularity: VkExtent3D,
}
pub type PFN_vkGetPhysicalDeviceQueueFamilyProperties = unsafe extern "C" fn(VkPhysicalDevice, *mut u32, *mut VkQueueFamilyProperties);
pub type PFN_vkGetPhysicalDeviceMemoryProperties = unsafe extern "C" fn(VkPhysicalDevice, *mut VkPhysicalDeviceMemoryProperties);
pub type PFN_vkGetPhysicalDeviceSurfaceCapabilitiesKHR = unsafe extern "C" fn(VkPhysicalDevice, VkSurfaceKHR, *mut VkSurfaceCapabilitiesKHR) -> VkResult;
pub type PFN_vkCreateDevice = unsafe extern "C" fn(VkPhysicalDevice, *const VkDeviceCreateInfo, *const c_void, *mut VkDevice) -> VkResult;
pub type PFN_vkDestroyDevice = unsafe extern "C" fn(VkDevice, *const c_void);
pub type PFN_vkGetDeviceQueue = unsafe extern "C" fn(VkDevice, u32, u32, *mut VkQueue);
pub type PFN_vkGetDeviceQueue2 = unsafe extern "C" fn(VkDevice, *const VkDeviceQueueInfo2, *mut VkQueue);
pub type PFN_vkCreateSwapchainKHR = unsafe extern "C" fn(VkDevice, *const VkSwapchainCreateInfoKHR, *const c_void, *mut VkSwapchainKHR) -> VkResult;
pub type PFN_vkDestroySwapchainKHR = unsafe extern "C" fn(VkDevice, VkSwapchainKHR, *const c_void);
pub type PFN_vkGetSwapchainImagesKHR = unsafe extern "C" fn(VkDevice, VkSwapchainKHR, *mut u32, *mut VkImage) -> VkResult;
pub type PFN_vkQueuePresentKHR = unsafe extern "C" fn(VkQueue, *const VkPresentInfoKHR) -> VkResult;
pub type PFN_vkQueueSubmit = unsafe extern "C" fn(VkQueue, u32, *const VkSubmitInfo, VkFence) -> VkResult;
pub type PFN_vkDeviceWaitIdle = unsafe extern "C" fn(VkDevice) -> VkResult;

pub type PFN_vkCreateCommandPool = unsafe extern "C" fn(VkDevice, *const VkCommandPoolCreateInfo, *const c_void, *mut VkCommandPool) -> VkResult;
pub type PFN_vkDestroyCommandPool = unsafe extern "C" fn(VkDevice, VkCommandPool, *const c_void);
pub type PFN_vkAllocateCommandBuffers = unsafe extern "C" fn(VkDevice, *const VkCommandBufferAllocateInfo, *mut VkCommandBuffer) -> VkResult;
pub type PFN_vkBeginCommandBuffer = unsafe extern "C" fn(VkCommandBuffer, *const VkCommandBufferBeginInfo) -> VkResult;
pub type PFN_vkEndCommandBuffer = unsafe extern "C" fn(VkCommandBuffer) -> VkResult;
pub type PFN_vkResetCommandBuffer = unsafe extern "C" fn(VkCommandBuffer, VkFlags) -> VkResult;

pub type PFN_vkCreateSemaphore = unsafe extern "C" fn(VkDevice, *const VkSemaphoreCreateInfo, *const c_void, *mut VkSemaphore) -> VkResult;
pub type PFN_vkDestroySemaphore = unsafe extern "C" fn(VkDevice, VkSemaphore, *const c_void);
pub type PFN_vkCreateFence = unsafe extern "C" fn(VkDevice, *const VkFenceCreateInfo, *const c_void, *mut VkFence) -> VkResult;
pub type PFN_vkDestroyFence = unsafe extern "C" fn(VkDevice, VkFence, *const c_void);
pub type PFN_vkWaitForFences = unsafe extern "C" fn(VkDevice, u32, *const VkFence, VkBool32, u64) -> VkResult;
pub type PFN_vkResetFences = unsafe extern "C" fn(VkDevice, u32, *const VkFence) -> VkResult;

pub type PFN_vkCreateImage = unsafe extern "C" fn(VkDevice, *const VkImageCreateInfo, *const c_void, *mut VkImage) -> VkResult;
pub type PFN_vkDestroyImage = unsafe extern "C" fn(VkDevice, VkImage, *const c_void);
pub type PFN_vkGetImageMemoryRequirements = unsafe extern "C" fn(VkDevice, VkImage, *mut VkMemoryRequirements);
pub type PFN_vkBindImageMemory = unsafe extern "C" fn(VkDevice, VkImage, VkDeviceMemory, VkDeviceSize) -> VkResult;
pub type PFN_vkCreateImageView = unsafe extern "C" fn(VkDevice, *const VkImageViewCreateInfo, *const c_void, *mut VkImageView) -> VkResult;
pub type PFN_vkDestroyImageView = unsafe extern "C" fn(VkDevice, VkImageView, *const c_void);

pub type PFN_vkCreateBuffer = unsafe extern "C" fn(VkDevice, *const VkBufferCreateInfo, *const c_void, *mut VkBuffer) -> VkResult;
pub type PFN_vkDestroyBuffer = unsafe extern "C" fn(VkDevice, VkBuffer, *const c_void);
pub type PFN_vkGetBufferMemoryRequirements = unsafe extern "C" fn(VkDevice, VkBuffer, *mut VkMemoryRequirements);
pub type PFN_vkBindBufferMemory = unsafe extern "C" fn(VkDevice, VkBuffer, VkDeviceMemory, VkDeviceSize) -> VkResult;
pub type PFN_vkAllocateMemory = unsafe extern "C" fn(VkDevice, *const VkMemoryAllocateInfo, *const c_void, *mut VkDeviceMemory) -> VkResult;
pub type PFN_vkFreeMemory = unsafe extern "C" fn(VkDevice, VkDeviceMemory, *const c_void);
pub type PFN_vkMapMemory = unsafe extern "C" fn(VkDevice, VkDeviceMemory, VkDeviceSize, VkDeviceSize, VkFlags, *mut *mut c_void) -> VkResult;
pub type PFN_vkUnmapMemory = unsafe extern "C" fn(VkDevice, VkDeviceMemory);

pub type PFN_vkCreateShaderModule = unsafe extern "C" fn(VkDevice, *const VkShaderModuleCreateInfo, *const c_void, *mut VkShaderModule) -> VkResult;
pub type PFN_vkDestroyShaderModule = unsafe extern "C" fn(VkDevice, VkShaderModule, *const c_void);
pub type PFN_vkCreateRenderPass = unsafe extern "C" fn(VkDevice, *const VkRenderPassCreateInfo, *const c_void, *mut VkRenderPass) -> VkResult;
pub type PFN_vkDestroyRenderPass = unsafe extern "C" fn(VkDevice, VkRenderPass, *const c_void);
pub type PFN_vkCreateFramebuffer = unsafe extern "C" fn(VkDevice, *const VkFramebufferCreateInfo, *const c_void, *mut VkFramebuffer) -> VkResult;
pub type PFN_vkDestroyFramebuffer = unsafe extern "C" fn(VkDevice, VkFramebuffer, *const c_void);
pub type PFN_vkCreateDescriptorSetLayout = unsafe extern "C" fn(VkDevice, *const VkDescriptorSetLayoutCreateInfo, *const c_void, *mut VkDescriptorSetLayout) -> VkResult;
pub type PFN_vkDestroyDescriptorSetLayout = unsafe extern "C" fn(VkDevice, VkDescriptorSetLayout, *const c_void);
pub type PFN_vkCreateDescriptorPool = unsafe extern "C" fn(VkDevice, *const VkDescriptorPoolCreateInfo, *const c_void, *mut VkDescriptorPool) -> VkResult;
pub type PFN_vkDestroyDescriptorPool = unsafe extern "C" fn(VkDevice, VkDescriptorPool, *const c_void);
pub type PFN_vkAllocateDescriptorSets = unsafe extern "C" fn(VkDevice, *const VkDescriptorSetAllocateInfo, *mut VkDescriptorSet) -> VkResult;
pub type PFN_vkFreeDescriptorSets = unsafe extern "C" fn(VkDevice, VkDescriptorPool, u32, *const VkDescriptorSet) -> VkResult;
pub type PFN_vkUpdateDescriptorSets = unsafe extern "C" fn(VkDevice, u32, *const VkWriteDescriptorSet, u32, *const c_void);
pub type PFN_vkCreateSampler = unsafe extern "C" fn(VkDevice, *const VkSamplerCreateInfo, *const c_void, *mut VkSampler) -> VkResult;
pub type PFN_vkDestroySampler = unsafe extern "C" fn(VkDevice, VkSampler, *const c_void);
pub type PFN_vkCreatePipelineLayout = unsafe extern "C" fn(VkDevice, *const VkPipelineLayoutCreateInfo, *const c_void, *mut VkPipelineLayout) -> VkResult;
pub type PFN_vkDestroyPipelineLayout = unsafe extern "C" fn(VkDevice, VkPipelineLayout, *const c_void);
pub type PFN_vkCreateGraphicsPipelines = unsafe extern "C" fn(VkDevice, u64, u32, *const VkGraphicsPipelineCreateInfo, *const c_void, *mut VkPipeline) -> VkResult;
pub type PFN_vkDestroyPipeline = unsafe extern "C" fn(VkDevice, VkPipeline, *const c_void);

pub type PFN_vkCmdPipelineBarrier = unsafe extern "C" fn(VkCommandBuffer, VkFlags, VkFlags, VkFlags, u32, *const c_void, u32, *const c_void, u32, *const VkImageMemoryBarrier);
pub type PFN_vkCmdCopyBufferToImage = unsafe extern "C" fn(VkCommandBuffer, VkBuffer, VkImage, i32, u32, *const VkBufferImageCopy);
pub type PFN_vkCmdCopyImageToBuffer = unsafe extern "C" fn(VkCommandBuffer, VkImage, i32, VkBuffer, u32, *const VkBufferImageCopy);
pub type PFN_vkCmdBeginRenderPass = unsafe extern "C" fn(VkCommandBuffer, *const VkRenderPassBeginInfo, i32);
pub type PFN_vkCmdEndRenderPass = unsafe extern "C" fn(VkCommandBuffer);
pub type PFN_vkCmdBindPipeline = unsafe extern "C" fn(VkCommandBuffer, i32, VkPipeline);
pub type PFN_vkCmdSetViewport = unsafe extern "C" fn(VkCommandBuffer, u32, u32, *const VkViewport);
pub type PFN_vkCmdSetScissor = unsafe extern "C" fn(VkCommandBuffer, u32, u32, *const VkRect2D);
pub type PFN_vkCmdPushConstants = unsafe extern "C" fn(VkCommandBuffer, VkPipelineLayout, VkFlags, u32, u32, *const c_void);
pub type PFN_vkCmdBindVertexBuffers = unsafe extern "C" fn(VkCommandBuffer, u32, u32, *const VkBuffer, *const VkDeviceSize);
pub type PFN_vkCmdBindIndexBuffer = unsafe extern "C" fn(VkCommandBuffer, VkBuffer, VkDeviceSize, i32);
pub type PFN_vkCmdBindDescriptorSets = unsafe extern "C" fn(VkCommandBuffer, i32, VkPipelineLayout, u32, u32, *const VkDescriptorSet, u32, *const u32);
pub type PFN_vkCmdDrawIndexed = unsafe extern "C" fn(VkCommandBuffer, u32, u32, u32, i32, u32);

/// Instance-level dispatch table (next layer / ICD).
#[derive(Clone)]
pub struct InstanceDispatch {
    pub get_instance_proc_addr: PFN_vkGetInstanceProcAddr,
    pub destroy_instance: PFN_vkDestroyInstance,
    pub create_xlib_surface: Option<PFN_vkCreateXlibSurfaceKHR>,
    pub create_xcb_surface: Option<PFN_vkCreateXcbSurfaceKHR>,
    pub create_wayland_surface: Option<PFN_vkCreateWaylandSurfaceKHR>,
    pub destroy_surface: Option<PFN_vkDestroySurfaceKHR>,
    pub queue_family_properties: Option<PFN_vkGetPhysicalDeviceQueueFamilyProperties>,
    pub get_physical_device_memory_properties: Option<PFN_vkGetPhysicalDeviceMemoryProperties>,
    pub get_physical_device_surface_capabilities: Option<PFN_vkGetPhysicalDeviceSurfaceCapabilitiesKHR>,
}

/// Device-level dispatch table (next layer / ICD).
#[derive(Clone)]
pub struct DeviceDispatch {
    pub get_device_proc_addr: PFN_vkGetDeviceProcAddr,
    pub destroy_device: PFN_vkDestroyDevice,
    pub get_device_queue: PFN_vkGetDeviceQueue,
    pub get_device_queue2: Option<PFN_vkGetDeviceQueue2>,
    pub create_swapchain: Option<PFN_vkCreateSwapchainKHR>,
    pub destroy_swapchain: Option<PFN_vkDestroySwapchainKHR>,
    pub get_swapchain_images: Option<PFN_vkGetSwapchainImagesKHR>,
    pub queue_present: Option<PFN_vkQueuePresentKHR>,
    pub queue_submit: PFN_vkQueueSubmit,
    pub device_wait_idle: PFN_vkDeviceWaitIdle,

    pub create_command_pool: PFN_vkCreateCommandPool,
    pub destroy_command_pool: PFN_vkDestroyCommandPool,
    pub allocate_command_buffers: PFN_vkAllocateCommandBuffers,
    pub begin_command_buffer: PFN_vkBeginCommandBuffer,
    pub end_command_buffer: PFN_vkEndCommandBuffer,
    pub reset_command_buffer: PFN_vkResetCommandBuffer,

    pub create_semaphore: PFN_vkCreateSemaphore,
    pub destroy_semaphore: PFN_vkDestroySemaphore,
    pub create_fence: PFN_vkCreateFence,
    pub destroy_fence: PFN_vkDestroyFence,
    pub wait_for_fences: PFN_vkWaitForFences,
    pub reset_fences: PFN_vkResetFences,

    pub create_image: PFN_vkCreateImage,
    pub destroy_image: PFN_vkDestroyImage,
    pub get_image_memory_requirements: PFN_vkGetImageMemoryRequirements,
    pub bind_image_memory: PFN_vkBindImageMemory,
    pub create_image_view: PFN_vkCreateImageView,
    pub destroy_image_view: PFN_vkDestroyImageView,

    pub create_buffer: PFN_vkCreateBuffer,
    pub destroy_buffer: PFN_vkDestroyBuffer,
    pub get_buffer_memory_requirements: PFN_vkGetBufferMemoryRequirements,
    pub bind_buffer_memory: PFN_vkBindBufferMemory,
    pub allocate_memory: PFN_vkAllocateMemory,
    pub free_memory: PFN_vkFreeMemory,
    pub map_memory: PFN_vkMapMemory,
    pub unmap_memory: PFN_vkUnmapMemory,

    pub create_shader_module: PFN_vkCreateShaderModule,
    pub destroy_shader_module: PFN_vkDestroyShaderModule,
    pub create_render_pass: PFN_vkCreateRenderPass,
    pub destroy_render_pass: PFN_vkDestroyRenderPass,
    pub create_framebuffer: PFN_vkCreateFramebuffer,
    pub destroy_framebuffer: PFN_vkDestroyFramebuffer,
    pub create_descriptor_set_layout: PFN_vkCreateDescriptorSetLayout,
    pub destroy_descriptor_set_layout: PFN_vkDestroyDescriptorSetLayout,
    pub create_descriptor_pool: PFN_vkCreateDescriptorPool,
    pub destroy_descriptor_pool: PFN_vkDestroyDescriptorPool,
    pub allocate_descriptor_sets: PFN_vkAllocateDescriptorSets,
    pub free_descriptor_sets: PFN_vkFreeDescriptorSets,
    pub update_descriptor_sets: PFN_vkUpdateDescriptorSets,
    pub create_sampler: PFN_vkCreateSampler,
    pub destroy_sampler: PFN_vkDestroySampler,
    pub create_pipeline_layout: PFN_vkCreatePipelineLayout,
    pub destroy_pipeline_layout: PFN_vkDestroyPipelineLayout,
    pub create_graphics_pipelines: PFN_vkCreateGraphicsPipelines,
    pub destroy_pipeline: PFN_vkDestroyPipeline,

    pub cmd_pipeline_barrier: PFN_vkCmdPipelineBarrier,
    pub cmd_copy_buffer_to_image: PFN_vkCmdCopyBufferToImage,
    pub cmd_copy_image_to_buffer: Option<PFN_vkCmdCopyImageToBuffer>,
    pub cmd_begin_render_pass: PFN_vkCmdBeginRenderPass,
    pub cmd_end_render_pass: PFN_vkCmdEndRenderPass,
    pub cmd_bind_pipeline: PFN_vkCmdBindPipeline,
    pub cmd_set_viewport: PFN_vkCmdSetViewport,
    pub cmd_set_scissor: PFN_vkCmdSetScissor,
    pub cmd_push_constants: PFN_vkCmdPushConstants,
    pub cmd_bind_vertex_buffers: PFN_vkCmdBindVertexBuffers,
    pub cmd_bind_index_buffer: PFN_vkCmdBindIndexBuffer,
    pub cmd_bind_descriptor_sets: PFN_vkCmdBindDescriptorSets,
    pub cmd_draw_indexed: PFN_vkCmdDrawIndexed,
}
