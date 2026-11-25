//! Vulkan swapchain overlay renderer for `libVkLayer_aurora_overlay.so`.
//! Draws egui meshes and texture atlases directly onto a game's swapchain image
//! inside `vkQueuePresentKHR` using semaphore chaining.

use crate::net::{Frame, TexOp};
use crate::spv::{FS_SPV, VS_SPV};
use crate::vk_abi::*;
use crate::wire::VERTEX_SIZE;
use std::collections::HashMap;
use std::ffi::c_void;
use std::ptr;
use std::sync::Arc;

/// Pixel layout of a Vulkan swapchain image for F12 screenshots.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PixelKind {
    Rgba8,
    Bgra8,
    Rgb10a2,
    Bgr10a2,
    Half,
}

pub fn vk_pixel_kind(fmt: i32) -> Option<PixelKind> {
    Some(match fmt {
        VK_FORMAT_R8G8B8A8_UNORM | VK_FORMAT_R8G8B8A8_SRGB => PixelKind::Rgba8,
        VK_FORMAT_B8G8R8A8_UNORM | VK_FORMAT_B8G8R8A8_SRGB => PixelKind::Bgra8,
        VK_FORMAT_A2B10G10R10_UNORM_PACK32 => PixelKind::Rgb10a2,
        VK_FORMAT_A2R10G10B10_UNORM_PACK32 => PixelKind::Bgr10a2,
        VK_FORMAT_R16G16B16A16_SFLOAT => PixelKind::Half,
        _ => return None,
    })
}

fn half_to_f32(h: u16) -> f32 {
    let sign = if h & 0x8000 != 0 { -1.0 } else { 1.0 };
    let exp = ((h >> 10) & 0x1F) as i32;
    let frac = (h & 0x3FF) as f32;
    sign * match exp {
        0 => frac * 2f32.powi(-24),
        31 => f32::INFINITY,
        _ => (1.0 + frac / 1024.0) * 2f32.powi(exp - 15),
    }
}

fn linear_to_srgb8(v: f32) -> u8 {
    let v = if v.is_nan() { 0.0 } else { v.clamp(0.0, 1.0) };
    let s = if v <= 0.003_130_8 { v * 12.92 } else { 1.055 * v.powf(1.0 / 2.4) - 0.055 };
    (s * 255.0 + 0.5) as u8
}

/// Convert packed swapchain pixels (`raw` must fit `width * height * bpp`) into tightly packed RGB.
pub fn vk_pixels_to_rgb(kind: PixelKind, raw: &[u8], width: u32, height: u32) -> Vec<u8> {
    let bpp = if kind == PixelKind::Half { 8 } else { 4 };
    let px = (width as usize).saturating_mul(height as usize);
    let len = px.saturating_mul(bpp).min(raw.len());
    let src = &raw[..len];
    let mut out = Vec::with_capacity(px.saturating_mul(3));
    match kind {
        PixelKind::Rgba8 => src.chunks_exact(4).for_each(|p| out.extend_from_slice(&p[..3])),
        PixelKind::Bgra8 => src.chunks_exact(4).for_each(|p| out.extend_from_slice(&[p[2], p[1], p[0]])),
        PixelKind::Rgb10a2 => src.chunks_exact(4).for_each(|p| {
            let v = u32::from_le_bytes([p[0], p[1], p[2], p[3]]);
            out.extend_from_slice(&[((v & 0x3FF) >> 2) as u8, (((v >> 10) & 0x3FF) >> 2) as u8, (((v >> 20) & 0x3FF) >> 2) as u8]);
        }),
        PixelKind::Bgr10a2 => src.chunks_exact(4).for_each(|p| {
            let v = u32::from_le_bytes([p[0], p[1], p[2], p[3]]);
            out.extend_from_slice(&[(((v >> 20) & 0x3FF) >> 2) as u8, (((v >> 10) & 0x3FF) >> 2) as u8, ((v & 0x3FF) >> 2) as u8]);
        }),
        PixelKind::Half => src.chunks_exact(8).for_each(|p| {
            for c in 0..3 {
                out.push(linear_to_srgb8(half_to_f32(u16::from_le_bytes([p[c * 2], p[c * 2 + 1]]))));
            }
        }),
    }
    out
}

struct GpuBuf {
    buf: VkBuffer,
    mem: VkDeviceMemory,
    cap: usize,
}

struct Texture {
    image: VkImage,
    mem: VkDeviceMemory,
    view: VkImageView,
    set: VkDescriptorSet,
    width: u32,
    height: u32,
    linear: bool,
}

struct FrameSlot {
    image: VkImage,
    view: VkImageView,
    fb: VkFramebuffer,
    cmd: VkCommandBuffer,
    fence: VkFence,
    fence_pending: bool,
    last_submit_gen: u64,
    done_sem: VkSemaphore,
    vb: Option<GpuBuf>,
    ib: Option<GpuBuf>,
    uploaded: Option<Arc<Frame>>,
    staging: Vec<GpuBuf>,
}

pub struct OverlayRenderer {
    pub queue_family: u32,
    device: VkDevice,
    dd: DeviceDispatch,
    mem_props: VkPhysicalDeviceMemoryProperties,
    format: i32,
    can_copy_src: bool,
    width: u32,
    height: u32,
    linear_out: bool,
    cmd_pool: VkCommandPool,
    render_pass: VkRenderPass,
    ds_layout: VkDescriptorSetLayout,
    ds_pool: VkDescriptorPool,
    sampler_linear: VkSampler,
    sampler_nearest: VkSampler,
    pipe_layout: VkPipelineLayout,
    pipeline: VkPipeline,
    slots: Vec<FrameSlot>,
    textures: HashMap<u64, Texture>,
    submit_gen: u64,
    retired_tex: Vec<(u64, Texture)>,
}

impl OverlayRenderer {
    pub unsafe fn new(
        device: VkDevice,
        dd: DeviceDispatch,
        mem_props: VkPhysicalDeviceMemoryProperties,
        queue_family: u32,
        swapchain: VkSwapchainKHR,
        format: i32,
        can_copy_src: bool,
        width: u32,
        height: u32,
    ) -> Option<Self> {
        let get_images = dd.get_swapchain_images?;
        let mut count = 0u32;
        if unsafe { get_images(device, swapchain, &mut count, ptr::null_mut()) } != VK_SUCCESS || count == 0 {
            return None;
        }
        let mut images = vec![0u64; count as usize];
        if unsafe { get_images(device, swapchain, &mut count, images.as_mut_ptr()) } != VK_SUCCESS {
            return None;
        }
        images.truncate(count as usize);

        // Command pool
        let cp_ci = VkCommandPoolCreateInfo {
            sType: VK_STRUCTURE_TYPE_COMMAND_POOL_CREATE_INFO,
            pNext: ptr::null(),
            flags: VK_COMMAND_POOL_CREATE_RESET_COMMAND_BUFFER_BIT,
            queueFamilyIndex: queue_family,
        };
        let mut cmd_pool = 0u64;
        if unsafe { (dd.create_command_pool)(device, &cp_ci, ptr::null(), &mut cmd_pool) } != VK_SUCCESS {
            return None;
        }

        // Render pass: LOAD existing game frame in PRESENT_SRC_KHR, blend overlay, leave in PRESENT_SRC_KHR.
        let att = VkAttachmentDescription {
            flags: 0,
            format,
            samples: VK_SAMPLE_COUNT_1_BIT,
            loadOp: VK_ATTACHMENT_LOAD_OP_LOAD,
            storeOp: VK_ATTACHMENT_STORE_OP_STORE,
            stencilLoadOp: VK_ATTACHMENT_LOAD_OP_DONT_CARE,
            stencilStoreOp: VK_ATTACHMENT_STORE_OP_DONT_CARE,
            initialLayout: VK_IMAGE_LAYOUT_PRESENT_SRC_KHR,
            finalLayout: VK_IMAGE_LAYOUT_PRESENT_SRC_KHR,
        };
        let att_ref = VkAttachmentReference {
            attachment: 0,
            layout: VK_IMAGE_LAYOUT_COLOR_ATTACHMENT_OPTIMAL,
        };
        let subpass = VkSubpassDescription {
            flags: 0,
            pipelineBindPoint: VK_PIPELINE_BIND_POINT_GRAPHICS,
            inputAttachmentCount: 0,
            pInputAttachments: ptr::null(),
            colorAttachmentCount: 1,
            pColorAttachments: &att_ref,
            pResolveAttachments: ptr::null(),
            pDepthStencilAttachment: ptr::null(),
            preserveAttachmentCount: 0,
            pPreserveAttachments: ptr::null(),
        };
        let dep = VkSubpassDependency {
            srcSubpass: VK_SUBPASS_EXTERNAL,
            dstSubpass: 0,
            srcStageMask: VK_PIPELINE_STAGE_COLOR_ATTACHMENT_OUTPUT_BIT,
            dstStageMask: VK_PIPELINE_STAGE_COLOR_ATTACHMENT_OUTPUT_BIT,
            srcAccessMask: 0,
            dstAccessMask: VK_ACCESS_COLOR_ATTACHMENT_READ_BIT | VK_ACCESS_COLOR_ATTACHMENT_WRITE_BIT,
            dependencyFlags: 0,
        };
        let rp_ci = VkRenderPassCreateInfo {
            sType: VK_STRUCTURE_TYPE_RENDER_PASS_CREATE_INFO,
            pNext: ptr::null(),
            flags: 0,
            attachmentCount: 1,
            pAttachments: &att,
            subpassCount: 1,
            pSubpasses: &subpass,
            dependencyCount: 1,
            pDependencies: &dep,
        };
        let mut render_pass = 0u64;
        if unsafe { (dd.create_render_pass)(device, &rp_ci, ptr::null(), &mut render_pass) } != VK_SUCCESS {
            unsafe { (dd.destroy_command_pool)(device, cmd_pool, ptr::null()) };
            return None;
        }

        // Descriptor set layout (binding 0 = combined image sampler in fragment shader)
        let binding = VkDescriptorSetLayoutBinding {
            binding: 0,
            descriptorType: VK_DESCRIPTOR_TYPE_COMBINED_IMAGE_SAMPLER,
            descriptorCount: 1,
            stageFlags: VK_SHADER_STAGE_FRAGMENT_BIT,
            pImmutableSamplers: ptr::null(),
        };
        let dsl_ci = VkDescriptorSetLayoutCreateInfo {
            sType: VK_STRUCTURE_TYPE_DESCRIPTOR_SET_LAYOUT_CREATE_INFO,
            pNext: ptr::null(),
            flags: 0,
            bindingCount: 1,
            pBindings: &binding,
        };
        let mut ds_layout = 0u64;
        if unsafe { (dd.create_descriptor_set_layout)(device, &dsl_ci, ptr::null(), &mut ds_layout) } != VK_SUCCESS {
            unsafe {
                (dd.destroy_render_pass)(device, render_pass, ptr::null());
                (dd.destroy_command_pool)(device, cmd_pool, ptr::null());
            }
            return None;
        }

        // Descriptor pool
        let pool_size = VkDescriptorPoolSize {
            ty: VK_DESCRIPTOR_TYPE_COMBINED_IMAGE_SAMPLER,
            descriptorCount: 512,
        };
        let dp_ci = VkDescriptorPoolCreateInfo {
            sType: VK_STRUCTURE_TYPE_DESCRIPTOR_POOL_CREATE_INFO,
            pNext: ptr::null(),
            flags: VK_DESCRIPTOR_POOL_CREATE_FREE_DESCRIPTOR_SET_BIT,
            maxSets: 512,
            poolSizeCount: 1,
            pPoolSizes: &pool_size,
        };
        let mut ds_pool = 0u64;
        if unsafe { (dd.create_descriptor_pool)(device, &dp_ci, ptr::null(), &mut ds_pool) } != VK_SUCCESS {
            unsafe {
                (dd.destroy_descriptor_set_layout)(device, ds_layout, ptr::null());
                (dd.destroy_render_pass)(device, render_pass, ptr::null());
                (dd.destroy_command_pool)(device, cmd_pool, ptr::null());
            }
            return None;
        }

        // Samplers
        let make_sampler = |filter: i32| -> Option<VkSampler> {
            let ci = VkSamplerCreateInfo {
                sType: VK_STRUCTURE_TYPE_SAMPLER_CREATE_INFO,
                pNext: ptr::null(),
                flags: 0,
                magFilter: filter,
                minFilter: filter,
                mipmapMode: filter,
                addressModeU: VK_SAMPLER_ADDRESS_MODE_CLAMP_TO_EDGE,
                addressModeV: VK_SAMPLER_ADDRESS_MODE_CLAMP_TO_EDGE,
                addressModeW: VK_SAMPLER_ADDRESS_MODE_CLAMP_TO_EDGE,
                mipLodBias: 0.0,
                anisotropyEnable: VK_FALSE,
                maxAnisotropy: 1.0,
                compareEnable: VK_FALSE,
                compareOp: 0,
                minLod: 0.0,
                maxLod: 0.0,
                borderColor: 0,
                unnormalizedCoordinates: VK_FALSE,
            };
            let mut s = 0u64;
            if unsafe { (dd.create_sampler)(device, &ci, ptr::null(), &mut s) } == VK_SUCCESS {
                Some(s)
            } else {
                None
            }
        };
        let Some(sampler_linear) = make_sampler(VK_FILTER_LINEAR) else {
            unsafe {
                (dd.destroy_descriptor_pool)(device, ds_pool, ptr::null());
                (dd.destroy_descriptor_set_layout)(device, ds_layout, ptr::null());
                (dd.destroy_render_pass)(device, render_pass, ptr::null());
                (dd.destroy_command_pool)(device, cmd_pool, ptr::null());
            }
            return None;
        };
        let Some(sampler_nearest) = make_sampler(VK_FILTER_NEAREST) else {
            unsafe {
                (dd.destroy_sampler)(device, sampler_linear, ptr::null());
                (dd.destroy_descriptor_pool)(device, ds_pool, ptr::null());
                (dd.destroy_descriptor_set_layout)(device, ds_layout, ptr::null());
                (dd.destroy_render_pass)(device, render_pass, ptr::null());
                (dd.destroy_command_pool)(device, cmd_pool, ptr::null());
            }
            return None;
        };

        // Pipeline layout: 12-byte push constant range (vec2 screen + float linear_out)
        let pc_range = VkPushConstantRange {
            stageFlags: VK_SHADER_STAGE_VERTEX_BIT | VK_SHADER_STAGE_FRAGMENT_BIT,
            offset: 0,
            size: 12,
        };
        let pl_ci = VkPipelineLayoutCreateInfo {
            sType: VK_STRUCTURE_TYPE_PIPELINE_LAYOUT_CREATE_INFO,
            pNext: ptr::null(),
            flags: 0,
            setLayoutCount: 1,
            pSetLayouts: &ds_layout,
            pushConstantRangeCount: 1,
            pPushConstantRanges: &pc_range,
        };
        let mut pipe_layout = 0u64;
        if unsafe { (dd.create_pipeline_layout)(device, &pl_ci, ptr::null(), &mut pipe_layout) } != VK_SUCCESS {
            unsafe {
                (dd.destroy_sampler)(device, sampler_nearest, ptr::null());
                (dd.destroy_sampler)(device, sampler_linear, ptr::null());
                (dd.destroy_descriptor_pool)(device, ds_pool, ptr::null());
                (dd.destroy_descriptor_set_layout)(device, ds_layout, ptr::null());
                (dd.destroy_render_pass)(device, render_pass, ptr::null());
                (dd.destroy_command_pool)(device, cmd_pool, ptr::null());
            }
            return None;
        }

        // Shader modules
        let vs_ci = VkShaderModuleCreateInfo {
            sType: VK_STRUCTURE_TYPE_SHADER_MODULE_CREATE_INFO,
            pNext: ptr::null(),
            flags: 0,
            codeSize: VS_SPV.len() * 4,
            pCode: VS_SPV.as_ptr(),
        };
        let fs_ci = VkShaderModuleCreateInfo {
            sType: VK_STRUCTURE_TYPE_SHADER_MODULE_CREATE_INFO,
            pNext: ptr::null(),
            flags: 0,
            codeSize: FS_SPV.len() * 4,
            pCode: FS_SPV.as_ptr(),
        };
        let mut vs_mod = 0u64;
        let mut fs_mod = 0u64;
        let ok_vs = unsafe { (dd.create_shader_module)(device, &vs_ci, ptr::null(), &mut vs_mod) } == VK_SUCCESS;
        let ok_fs = ok_vs && unsafe { (dd.create_shader_module)(device, &fs_ci, ptr::null(), &mut fs_mod) } == VK_SUCCESS;
        if !ok_fs {
            unsafe {
                if vs_mod != 0 { (dd.destroy_shader_module)(device, vs_mod, ptr::null()); }
                (dd.destroy_pipeline_layout)(device, pipe_layout, ptr::null());
                (dd.destroy_sampler)(device, sampler_nearest, ptr::null());
                (dd.destroy_sampler)(device, sampler_linear, ptr::null());
                (dd.destroy_descriptor_pool)(device, ds_pool, ptr::null());
                (dd.destroy_descriptor_set_layout)(device, ds_layout, ptr::null());
                (dd.destroy_render_pass)(device, render_pass, ptr::null());
                (dd.destroy_command_pool)(device, cmd_pool, ptr::null());
            }
            return None;
        }

        let stages = [
            VkPipelineShaderStageCreateInfo {
                sType: VK_STRUCTURE_TYPE_PIPELINE_SHADER_STAGE_CREATE_INFO,
                pNext: ptr::null(),
                flags: 0,
                stage: VK_SHADER_STAGE_VERTEX_BIT,
                module: vs_mod,
                pName: c"main".as_ptr(),
                pSpecializationInfo: ptr::null(),
            },
            VkPipelineShaderStageCreateInfo {
                sType: VK_STRUCTURE_TYPE_PIPELINE_SHADER_STAGE_CREATE_INFO,
                pNext: ptr::null(),
                flags: 0,
                stage: VK_SHADER_STAGE_FRAGMENT_BIT,
                module: fs_mod,
                pName: c"main".as_ptr(),
                pSpecializationInfo: ptr::null(),
            },
        ];
        let vbind = VkVertexInputBindingDescription {
            binding: 0,
            stride: VERTEX_SIZE as u32,
            inputRate: 0,
        };
        let vattrs = [
            VkVertexInputAttributeDescription { location: 0, binding: 0, format: VK_FORMAT_R32G32_SFLOAT, offset: 0 },
            VkVertexInputAttributeDescription { location: 1, binding: 0, format: VK_FORMAT_R32G32_SFLOAT, offset: 8 },
            VkVertexInputAttributeDescription { location: 2, binding: 0, format: VK_FORMAT_R8G8B8A8_UNORM, offset: 16 },
        ];
        let vi = VkPipelineVertexInputStateCreateInfo {
            sType: VK_STRUCTURE_TYPE_PIPELINE_VERTEX_INPUT_STATE_CREATE_INFO,
            pNext: ptr::null(),
            flags: 0,
            vertexBindingDescriptionCount: 1,
            pVertexBindingDescriptions: &vbind,
            vertexAttributeDescriptionCount: vattrs.len() as u32,
            pVertexAttributeDescriptions: vattrs.as_ptr(),
        };
        let ia = VkPipelineInputAssemblyStateCreateInfo {
            sType: VK_STRUCTURE_TYPE_PIPELINE_INPUT_ASSEMBLY_STATE_CREATE_INFO,
            pNext: ptr::null(),
            flags: 0,
            topology: VK_PRIMITIVE_TOPOLOGY_TRIANGLE_LIST,
            primitiveRestartEnable: VK_FALSE,
        };
        let vp_state = VkPipelineViewportStateCreateInfo {
            sType: VK_STRUCTURE_TYPE_PIPELINE_VIEWPORT_STATE_CREATE_INFO,
            pNext: ptr::null(),
            flags: 0,
            viewportCount: 1,
            pViewports: ptr::null(),
            scissorCount: 1,
            pScissors: ptr::null(),
        };
        let rs = VkPipelineRasterizationStateCreateInfo {
            sType: VK_STRUCTURE_TYPE_PIPELINE_RASTERIZATION_STATE_CREATE_INFO,
            pNext: ptr::null(),
            flags: 0,
            depthClampEnable: VK_FALSE,
            rasterizerDiscardEnable: VK_FALSE,
            polygonMode: VK_POLYGON_MODE_FILL,
            cullMode: VK_CULL_MODE_NONE,
            frontFace: VK_FRONT_FACE_COUNTER_CLOCKWISE,
            depthBiasEnable: VK_FALSE,
            depthBiasConstantFactor: 0.0,
            depthBiasClamp: 0.0,
            depthBiasSlopeFactor: 0.0,
            lineWidth: 1.0,
        };
        let ms = VkPipelineMultisampleStateCreateInfo {
            sType: VK_STRUCTURE_TYPE_PIPELINE_MULTISAMPLE_STATE_CREATE_INFO,
            pNext: ptr::null(),
            flags: 0,
            rasterizationSamples: VK_SAMPLE_COUNT_1_BIT,
            sampleShadingEnable: VK_FALSE,
            minSampleShading: 0.0,
            pSampleMask: ptr::null(),
            alphaToCoverageEnable: VK_FALSE,
            alphaToOneEnable: VK_FALSE,
        };
        // Premultiplied alpha blending (egui outputs premultiplied sRGBA).
        let blend_att = VkPipelineColorBlendAttachmentState {
            blendEnable: VK_TRUE,
            srcColorBlendFactor: VK_BLEND_FACTOR_ONE,
            dstColorBlendFactor: VK_BLEND_FACTOR_ONE_MINUS_SRC_ALPHA,
            colorBlendOp: VK_BLEND_OP_ADD,
            srcAlphaBlendFactor: VK_BLEND_FACTOR_ONE,
            dstAlphaBlendFactor: VK_BLEND_FACTOR_ONE_MINUS_SRC_ALPHA,
            alphaBlendOp: VK_BLEND_OP_ADD,
            colorWriteMask: VK_COLOR_COMPONENT_RGBA,
        };
        let cb = VkPipelineColorBlendStateCreateInfo {
            sType: VK_STRUCTURE_TYPE_PIPELINE_COLOR_BLEND_STATE_CREATE_INFO,
            pNext: ptr::null(),
            flags: 0,
            logicOpEnable: VK_FALSE,
            logicOp: 0,
            attachmentCount: 1,
            pAttachments: &blend_att,
            blendConstants: [0.0; 4],
        };
        let dyn_states = [VK_DYNAMIC_STATE_VIEWPORT, VK_DYNAMIC_STATE_SCISSOR];
        let ds = VkPipelineDynamicStateCreateInfo {
            sType: VK_STRUCTURE_TYPE_PIPELINE_DYNAMIC_STATE_CREATE_INFO,
            pNext: ptr::null(),
            flags: 0,
            dynamicStateCount: dyn_states.len() as u32,
            pDynamicStates: dyn_states.as_ptr(),
        };
        let gp_ci = VkGraphicsPipelineCreateInfo {
            sType: VK_STRUCTURE_TYPE_GRAPHICS_PIPELINE_CREATE_INFO,
            pNext: ptr::null(),
            flags: 0,
            stageCount: stages.len() as u32,
            pStages: stages.as_ptr(),
            pVertexInputState: &vi,
            pInputAssemblyState: &ia,
            pTessellationState: ptr::null(),
            pViewportState: &vp_state,
            pRasterizationState: &rs,
            pMultisampleState: &ms,
            pDepthStencilState: ptr::null(),
            pColorBlendState: &cb,
            pDynamicState: &ds,
            layout: pipe_layout,
            renderPass: render_pass,
            subpass: 0,
            basePipelineHandle: 0,
            basePipelineIndex: -1,
        };
        let mut pipeline = 0u64;
        let pipe_ok = unsafe { (dd.create_graphics_pipelines)(device, 0, 1, &gp_ci, ptr::null(), &mut pipeline) } == VK_SUCCESS;
        unsafe {
            (dd.destroy_shader_module)(device, fs_mod, ptr::null());
            (dd.destroy_shader_module)(device, vs_mod, ptr::null());
        }
        if !pipe_ok {
            unsafe {
                (dd.destroy_pipeline_layout)(device, pipe_layout, ptr::null());
                (dd.destroy_sampler)(device, sampler_nearest, ptr::null());
                (dd.destroy_sampler)(device, sampler_linear, ptr::null());
                (dd.destroy_descriptor_pool)(device, ds_pool, ptr::null());
                (dd.destroy_descriptor_set_layout)(device, ds_layout, ptr::null());
                (dd.destroy_render_pass)(device, render_pass, ptr::null());
                (dd.destroy_command_pool)(device, cmd_pool, ptr::null());
            }
            return None;
        }

        // Per-swapchain-image slots
        let mut cmds = vec![ptr::null_mut(); images.len()];
        let alloc_ci = VkCommandBufferAllocateInfo {
            sType: VK_STRUCTURE_TYPE_COMMAND_BUFFER_ALLOCATE_INFO,
            pNext: ptr::null(),
            commandPool: cmd_pool,
            level: 0,
            commandBufferCount: images.len() as u32,
        };
        if unsafe { (dd.allocate_command_buffers)(device, &alloc_ci, cmds.as_mut_ptr()) } != VK_SUCCESS {
            let mut r = Self {
                device, dd, mem_props, queue_family, format, can_copy_src, width, height, linear_out: is_linear_output_format(format),
                cmd_pool, render_pass, ds_layout, ds_pool, sampler_linear, sampler_nearest,
                pipe_layout, pipeline, slots: Vec::new(), textures: HashMap::new(),
                submit_gen: 0, retired_tex: Vec::new(),
            };
            r.destroy();
            return None;
        }

        let mut slots = Vec::with_capacity(images.len());
        let sem_ci = VkSemaphoreCreateInfo { sType: VK_STRUCTURE_TYPE_SEMAPHORE_CREATE_INFO, pNext: ptr::null(), flags: 0 };
        let fence_ci = VkFenceCreateInfo { sType: VK_STRUCTURE_TYPE_FENCE_CREATE_INFO, pNext: ptr::null(), flags: 0 };
        for (&img, &cmd) in images.iter().zip(cmds.iter()) {
            let iv_ci = VkImageViewCreateInfo {
                sType: VK_STRUCTURE_TYPE_IMAGE_VIEW_CREATE_INFO,
                pNext: ptr::null(),
                flags: 0,
                image: img,
                viewType: 1, // 2D
                format,
                components: VkComponentMapping::default(),
                subresourceRange: VkImageSubresourceRange {
                    aspectMask: VK_IMAGE_ASPECT_COLOR_BIT,
                    baseMipLevel: 0,
                    levelCount: 1,
                    baseArrayLayer: 0,
                    layerCount: 1,
                },
            };
            let mut view = 0u64;
            let mut fb = 0u64;
            let mut fence = 0u64;
            let mut done_sem = 0u64;
            let ok = unsafe {
                (dd.create_image_view)(device, &iv_ci, ptr::null(), &mut view) == VK_SUCCESS
                    && {
                        let fb_ci = VkFramebufferCreateInfo {
                            sType: VK_STRUCTURE_TYPE_FRAMEBUFFER_CREATE_INFO,
                            pNext: ptr::null(),
                            flags: 0,
                            renderPass: render_pass,
                            attachmentCount: 1,
                            pAttachments: &view,
                            width,
                            height,
                            layers: 1,
                        };
                        (dd.create_framebuffer)(device, &fb_ci, ptr::null(), &mut fb) == VK_SUCCESS
                    }
                    && (dd.create_fence)(device, &fence_ci, ptr::null(), &mut fence) == VK_SUCCESS
                    && (dd.create_semaphore)(device, &sem_ci, ptr::null(), &mut done_sem) == VK_SUCCESS
            };
            if !ok {
                unsafe {
                    if done_sem != 0 { (dd.destroy_semaphore)(device, done_sem, ptr::null()); }
                    if fence != 0 { (dd.destroy_fence)(device, fence, ptr::null()); }
                    if fb != 0 { (dd.destroy_framebuffer)(device, fb, ptr::null()); }
                    if view != 0 { (dd.destroy_image_view)(device, view, ptr::null()); }
                }
                let mut r = Self {
                    device, dd, mem_props, queue_family, format, can_copy_src, width, height, linear_out: is_linear_output_format(format),
                    cmd_pool, render_pass, ds_layout, ds_pool, sampler_linear, sampler_nearest,
                    pipe_layout, pipeline, slots, textures: HashMap::new(),
                    submit_gen: 0, retired_tex: Vec::new(),
                };
                r.destroy();
                return None;
            }
            slots.push(FrameSlot {
                image: img,
                view,
                fb,
                cmd,
                fence,
                fence_pending: false,
                last_submit_gen: 0,
                done_sem,
                vb: None,
                ib: None,
                uploaded: None,
                staging: Vec::new(),
            });
        }

        Some(Self {
            queue_family,
            device,
            dd,
            mem_props,
            format,
            can_copy_src,
            width,
            height,
            linear_out: is_linear_output_format(format),
            cmd_pool,
            render_pass,
            ds_layout,
            ds_pool,
            sampler_linear,
            sampler_nearest,
            pipe_layout,
            pipeline,
            slots,
            textures: HashMap::new(),
            submit_gen: 0,
            retired_tex: Vec::new(),
        })
    }

    fn find_memory_type(&self, type_bits: u32, req_flags: VkFlags) -> Option<u32> {
        let count = (self.mem_props.memoryTypeCount as usize).min(32);
        for i in 0..count {
            if (type_bits & (1u32 << i)) != 0 && (self.mem_props.memoryTypes[i].propertyFlags & req_flags) == req_flags {
                return Some(i as u32);
            }
        }
        None
    }

    unsafe fn create_host_buf(&self, size: usize, usage: VkFlags) -> Option<GpuBuf> {
        let alloc_size = size.max(256) as VkDeviceSize;
        let ci = VkBufferCreateInfo {
            sType: VK_STRUCTURE_TYPE_BUFFER_CREATE_INFO,
            pNext: ptr::null(),
            flags: 0,
            size: alloc_size,
            usage,
            sharingMode: 0,
            queueFamilyIndexCount: 0,
            pQueueFamilyIndices: ptr::null(),
        };
        let mut buf = 0u64;
        if unsafe { (self.dd.create_buffer)(self.device, &ci, ptr::null(), &mut buf) } != VK_SUCCESS {
            return None;
        }
        let mut req = VkMemoryRequirements::default();
        unsafe { (self.dd.get_buffer_memory_requirements)(self.device, buf, &mut req) };
        let Some(mem_ty) = self.find_memory_type(req.memoryTypeBits, VK_MEMORY_PROPERTY_HOST_VISIBLE_BIT | VK_MEMORY_PROPERTY_HOST_COHERENT_BIT) else {
            unsafe { (self.dd.destroy_buffer)(self.device, buf, ptr::null()) };
            return None;
        };
        let ai = VkMemoryAllocateInfo {
            sType: VK_STRUCTURE_TYPE_MEMORY_ALLOCATE_INFO,
            pNext: ptr::null(),
            allocationSize: req.size,
            memoryTypeIndex: mem_ty,
        };
        let mut mem = 0u64;
        if unsafe { (self.dd.allocate_memory)(self.device, &ai, ptr::null(), &mut mem) } != VK_SUCCESS {
            unsafe { (self.dd.destroy_buffer)(self.device, buf, ptr::null()) };
            return None;
        }
        if unsafe { (self.dd.bind_buffer_memory)(self.device, buf, mem, 0) } != VK_SUCCESS {
            unsafe {
                (self.dd.free_memory)(self.device, mem, ptr::null());
                (self.dd.destroy_buffer)(self.device, buf, ptr::null());
            }
            return None;
        }
        Some(GpuBuf { buf, mem, cap: alloc_size as usize })
    }

    unsafe fn destroy_buf(&self, b: GpuBuf) {
        unsafe {
            (self.dd.destroy_buffer)(self.device, b.buf, ptr::null());
            (self.dd.free_memory)(self.device, b.mem, ptr::null());
        }
    }

    unsafe fn destroy_tex(&self, t: Texture) {
        unsafe {
            let _ = (self.dd.free_descriptor_sets)(self.device, self.ds_pool, 1, &t.set);
            (self.dd.destroy_image_view)(self.device, t.view, ptr::null());
            (self.dd.destroy_image)(self.device, t.image, ptr::null());
            (self.dd.free_memory)(self.device, t.mem, ptr::null());
        }
    }

    unsafe fn create_texture(&self, w: u32, h: u32, linear: bool) -> Option<Texture> {
        let img_ci = VkImageCreateInfo {
            sType: VK_STRUCTURE_TYPE_IMAGE_CREATE_INFO,
            pNext: ptr::null(),
            flags: 0,
            imageType: 1, // 2D
            format: VK_FORMAT_R8G8B8A8_UNORM,
            extent: VkExtent3D { width: w, height: h, depth: 1 },
            mipLevels: 1,
            arrayLayers: 1,
            samples: VK_SAMPLE_COUNT_1_BIT,
            tiling: 0, // OPTIMAL
            usage: VK_IMAGE_USAGE_TRANSFER_DST_BIT | VK_IMAGE_USAGE_SAMPLED_BIT,
            sharingMode: 0,
            queueFamilyIndexCount: 0,
            pQueueFamilyIndices: ptr::null(),
            initialLayout: VK_IMAGE_LAYOUT_UNDEFINED,
        };
        let mut image = 0u64;
        if unsafe { (self.dd.create_image)(self.device, &img_ci, ptr::null(), &mut image) } != VK_SUCCESS {
            return None;
        }
        let mut req = VkMemoryRequirements::default();
        unsafe { (self.dd.get_image_memory_requirements)(self.device, image, &mut req) };
        let mem_ty = self
            .find_memory_type(req.memoryTypeBits, VK_MEMORY_PROPERTY_DEVICE_LOCAL_BIT)
            .or_else(|| self.find_memory_type(req.memoryTypeBits, 0));
        let Some(mem_ty) = mem_ty else {
            unsafe { (self.dd.destroy_image)(self.device, image, ptr::null()) };
            return None;
        };
        let ai = VkMemoryAllocateInfo {
            sType: VK_STRUCTURE_TYPE_MEMORY_ALLOCATE_INFO,
            pNext: ptr::null(),
            allocationSize: req.size,
            memoryTypeIndex: mem_ty,
        };
        let mut mem = 0u64;
        if unsafe { (self.dd.allocate_memory)(self.device, &ai, ptr::null(), &mut mem) } != VK_SUCCESS {
            unsafe { (self.dd.destroy_image)(self.device, image, ptr::null()) };
            return None;
        }
        if unsafe { (self.dd.bind_image_memory)(self.device, image, mem, 0) } != VK_SUCCESS {
            unsafe {
                (self.dd.free_memory)(self.device, mem, ptr::null());
                (self.dd.destroy_image)(self.device, image, ptr::null());
            }
            return None;
        }
        let iv_ci = VkImageViewCreateInfo {
            sType: VK_STRUCTURE_TYPE_IMAGE_VIEW_CREATE_INFO,
            pNext: ptr::null(),
            flags: 0,
            image,
            viewType: 1,
            format: VK_FORMAT_R8G8B8A8_UNORM,
            components: VkComponentMapping::default(),
            subresourceRange: VkImageSubresourceRange {
                aspectMask: VK_IMAGE_ASPECT_COLOR_BIT,
                baseMipLevel: 0,
                levelCount: 1,
                baseArrayLayer: 0,
                layerCount: 1,
            },
        };
        let mut view = 0u64;
        if unsafe { (self.dd.create_image_view)(self.device, &iv_ci, ptr::null(), &mut view) } != VK_SUCCESS {
            unsafe {
                (self.dd.free_memory)(self.device, mem, ptr::null());
                (self.dd.destroy_image)(self.device, image, ptr::null());
            }
            return None;
        }
        let ds_ai = VkDescriptorSetAllocateInfo {
            sType: VK_STRUCTURE_TYPE_DESCRIPTOR_SET_ALLOCATE_INFO,
            pNext: ptr::null(),
            descriptorPool: self.ds_pool,
            descriptorSetCount: 1,
            pSetLayouts: &self.ds_layout,
        };
        let mut set = 0u64;
        if unsafe { (self.dd.allocate_descriptor_sets)(self.device, &ds_ai, &mut set) } != VK_SUCCESS {
            unsafe {
                (self.dd.destroy_image_view)(self.device, view, ptr::null());
                (self.dd.free_memory)(self.device, mem, ptr::null());
                (self.dd.destroy_image)(self.device, image, ptr::null());
            }
            return None;
        }
        let info = VkDescriptorImageInfo {
            sampler: if linear { self.sampler_linear } else { self.sampler_nearest },
            imageView: view,
            imageLayout: VK_IMAGE_LAYOUT_SHADER_READ_ONLY_OPTIMAL,
        };
        let write = VkWriteDescriptorSet {
            sType: VK_STRUCTURE_TYPE_WRITE_DESCRIPTOR_SET,
            pNext: ptr::null(),
            dstSet: set,
            dstBinding: 0,
            dstArrayElement: 0,
            descriptorCount: 1,
            descriptorType: VK_DESCRIPTOR_TYPE_COMBINED_IMAGE_SAMPLER,
            pImageInfo: &info,
            pBufferInfo: ptr::null(),
            pTexelBufferView: ptr::null(),
        };
        unsafe { (self.dd.update_descriptor_sets)(self.device, 1, &write, 0, ptr::null()) };
        Some(Texture { image, mem, view, set, width: w, height: h, linear })
    }

    pub unsafe fn process_present(
        &mut self,
        queue: VkQueue,
        image_index: u32,
        wait_sems: &[VkSemaphore],
        frame: Option<&Arc<Frame>>,
        tex_ops: Vec<TexOp>,
        take_shot: bool,
    ) -> (Option<VkSemaphore>, Option<Result<(u32, u32, Vec<u8>), ()>>) {
        let idx = image_index as usize;
        if idx >= self.slots.len() {
            return (None, take_shot.then_some(Err(())));
        }
        let has_meshes = frame.is_some_and(|f| !f.meshes.is_empty());
        if !has_meshes && tex_ops.is_empty() && !take_shot {
            return (None, None);
        }

        let fence = self.slots[idx].fence;
        let cmd = self.slots[idx].cmd;
        let fb = self.slots[idx].fb;
        let sw_image = self.slots[idx].image;
        let done_sem = self.slots[idx].done_sem;

        // Wait & reset only if a previous submit actually used this slot's fence.
        if self.slots[idx].fence_pending {
            if unsafe { (self.dd.wait_for_fences)(self.device, 1, &fence, VK_TRUE, 2_000_000_000) } != VK_SUCCESS {
                return (None, take_shot.then_some(Err(())));
            }
            if unsafe { (self.dd.reset_fences)(self.device, 1, &fence) } != VK_SUCCESS {
                return (None, take_shot.then_some(Err(())));
            }
            self.slots[idx].fence_pending = false;
        }

        // Reclaim staging buffers from this slot's completed frame and retired
        // textures once all pending slots referencing them have finished.
        let old_staging = std::mem::take(&mut self.slots[idx].staging);
        for b in old_staging {
            unsafe { self.destroy_buf(b) };
        }
        let min_pending = self
            .slots
            .iter()
            .filter(|s| s.fence_pending)
            .map(|s| s.last_submit_gen)
            .min_by(|a, b| (a.wrapping_sub(*b) as i64).cmp(&0));
        let mut ri = 0;
        while ri < self.retired_tex.len() {
            let safe = match min_pending {
                None => true,
                Some(min_gen) => (min_gen.wrapping_sub(self.retired_tex[ri].0) as i64) > 0,
            };
            if safe {
                let (_, t) = self.retired_tex.swap_remove(ri);
                unsafe { self.destroy_tex(t) };
            } else {
                ri += 1;
            }
        }

        if unsafe { (self.dd.reset_command_buffer)(cmd, 0) } != VK_SUCCESS {
            return (None, take_shot.then_some(Err(())));
        }
        let bi = VkCommandBufferBeginInfo {
            sType: VK_STRUCTURE_TYPE_COMMAND_BUFFER_BEGIN_INFO,
            pNext: ptr::null(),
            flags: VK_COMMAND_BUFFER_USAGE_ONE_TIME_SUBMIT_BIT,
            pInheritanceInfo: ptr::null(),
        };
        if unsafe { (self.dd.begin_command_buffer)(cmd, &bi) } != VK_SUCCESS {
            return (None, take_shot.then_some(Err(())));
        }

        // 0. Capture swapchain image BEFORE drawing the overlay if requested.
        let mut capture: Option<(GpuBuf, PixelKind)> = None;
        let mut shot_early_err = false;
        if take_shot {
            if let (true, Some(copy_fn), Some(kind)) = (self.can_copy_src, self.dd.cmd_copy_image_to_buffer, vk_pixel_kind(self.format)) {
                let bpp = if kind == PixelKind::Half { 8usize } else { 4usize };
                let byte_len = (self.width as usize).saturating_mul(self.height as usize).saturating_mul(bpp);
                if byte_len > 0 {
                    if let Some(buf) = unsafe { self.create_host_buf(byte_len, VK_BUFFER_USAGE_TRANSFER_DST_BIT) } {
                        let subrange = VkImageSubresourceRange {
                            aspectMask: VK_IMAGE_ASPECT_COLOR_BIT,
                            baseMipLevel: 0,
                            levelCount: 1,
                            baseArrayLayer: 0,
                            layerCount: 1,
                        };
                        let to_src = VkImageMemoryBarrier {
                            sType: VK_STRUCTURE_TYPE_IMAGE_MEMORY_BARRIER,
                            pNext: ptr::null(),
                            srcAccessMask: 0,
                            dstAccessMask: VK_ACCESS_TRANSFER_READ_BIT,
                            oldLayout: VK_IMAGE_LAYOUT_PRESENT_SRC_KHR,
                            newLayout: VK_IMAGE_LAYOUT_TRANSFER_SRC_OPTIMAL,
                            srcQueueFamilyIndex: VK_QUEUE_FAMILY_IGNORED,
                            dstQueueFamilyIndex: VK_QUEUE_FAMILY_IGNORED,
                            image: sw_image,
                            subresourceRange: subrange,
                        };
                        unsafe {
                            (self.dd.cmd_pipeline_barrier)(
                                cmd,
                                VK_PIPELINE_STAGE_TOP_OF_PIPE_BIT,
                                VK_PIPELINE_STAGE_TRANSFER_BIT,
                                0,
                                0,
                                ptr::null(),
                                0,
                                ptr::null(),
                                1,
                                &to_src,
                            );
                        }
                        let region = VkBufferImageCopy {
                            bufferOffset: 0,
                            bufferRowLength: 0,
                            bufferImageHeight: 0,
                            imageSubresource: VkImageSubresourceLayers {
                                aspectMask: VK_IMAGE_ASPECT_COLOR_BIT,
                                mipLevel: 0,
                                baseArrayLayer: 0,
                                layerCount: 1,
                            },
                            imageOffset: VkOffset3D { x: 0, y: 0, z: 0 },
                            imageExtent: VkExtent3D { width: self.width, height: self.height, depth: 1 },
                        };
                        unsafe { copy_fn(cmd, sw_image, VK_IMAGE_LAYOUT_TRANSFER_SRC_OPTIMAL, buf.buf, 1, &region) };
                        let to_present = VkImageMemoryBarrier {
                            sType: VK_STRUCTURE_TYPE_IMAGE_MEMORY_BARRIER,
                            pNext: ptr::null(),
                            srcAccessMask: VK_ACCESS_TRANSFER_READ_BIT,
                            dstAccessMask: VK_ACCESS_COLOR_ATTACHMENT_READ_BIT | VK_ACCESS_COLOR_ATTACHMENT_WRITE_BIT,
                            oldLayout: VK_IMAGE_LAYOUT_TRANSFER_SRC_OPTIMAL,
                            newLayout: VK_IMAGE_LAYOUT_PRESENT_SRC_KHR,
                            srcQueueFamilyIndex: VK_QUEUE_FAMILY_IGNORED,
                            dstQueueFamilyIndex: VK_QUEUE_FAMILY_IGNORED,
                            image: sw_image,
                            subresourceRange: subrange,
                        };
                        unsafe {
                            (self.dd.cmd_pipeline_barrier)(
                                cmd,
                                VK_PIPELINE_STAGE_TRANSFER_BIT,
                                VK_PIPELINE_STAGE_COLOR_ATTACHMENT_OUTPUT_BIT,
                                0,
                                0,
                                ptr::null(),
                                0,
                                ptr::null(),
                                1,
                                &to_present,
                            );
                        }
                        capture = Some((buf, kind));
                    } else {
                        shot_early_err = true;
                    }
                } else {
                    shot_early_err = true;
                }
            } else {
                shot_early_err = true;
            }
        }

        // If capture failed and nothing else needs rendering, abort cleanly without submitting.
        if capture.is_none() && !has_meshes && tex_ops.is_empty() {
            let _ = unsafe { (self.dd.end_command_buffer)(cmd) };
            return (None, take_shot.then_some(Err(())));
        }

        // 1. Apply texture updates / frees
        let mut new_staging = Vec::new();
        let retire_gen = self.submit_gen.wrapping_add(1);
        for op in tex_ops {
            match op {
                TexOp::ClearAll => {
                    for (_, t) in self.textures.drain() {
                        self.retired_tex.push((retire_gen, t));
                    }
                }
                TexOp::Free(id) => {
                    if let Some(t) = self.textures.remove(&id) {
                        self.retired_tex.push((retire_gen, t));
                    }
                }
                TexOp::Set(ts) => {
                    let Some(expected) = (ts.size[0] as usize)
                        .checked_mul(ts.size[1] as usize)
                        .and_then(|px| px.checked_mul(4))
                    else {
                        continue;
                    };
                    if ts.size[0] == 0 || ts.size[1] == 0 || ts.rgba.len() != expected {
                        continue;
                    }
                    // Validate sub-rectangle bounds and prepare staging buffer
                    // BEFORE creating or replacing `self.textures` so a staging
                    // allocation/map failure never leaves an UNDEFINED texture.
                    if let Some([px, py]) = ts.pos {
                        let Some(existing) = self.textures.get(&ts.id) else { continue };
                        if !px.checked_add(ts.size[0]).is_some_and(|r| r <= existing.width)
                            || !py.checked_add(ts.size[1]).is_some_and(|b| b <= existing.height)
                        {
                            continue;
                        }
                    }
                    let Some(stg) = (unsafe { self.create_host_buf(ts.rgba.len(), VK_BUFFER_USAGE_TRANSFER_SRC_BIT) }) else {
                        continue;
                    };
                    let mut mapped: *mut c_void = ptr::null_mut();
                    if unsafe { (self.dd.map_memory)(self.device, stg.mem, 0, ts.rgba.len() as VkDeviceSize, 0, &mut mapped) } != VK_SUCCESS {
                        unsafe { self.destroy_buf(stg) };
                        continue;
                    }
                    unsafe {
                        ptr::copy_nonoverlapping(ts.rgba.as_ptr(), mapped.cast::<u8>(), ts.rgba.len());
                        (self.dd.unmap_memory)(self.device, stg.mem);
                    }

                    let (old_layout, target_img) = match ts.pos {
                        Some(_) => (VK_IMAGE_LAYOUT_SHADER_READ_ONLY_OPTIMAL, self.textures[&ts.id].image),
                        None => {
                            let reuse = self.textures.get(&ts.id).is_some_and(|e| {
                                e.width == ts.size[0] && e.height == ts.size[1] && e.linear == ts.linear
                            });
                            if reuse {
                                (VK_IMAGE_LAYOUT_SHADER_READ_ONLY_OPTIMAL, self.textures[&ts.id].image)
                            } else {
                                let Some(tex) = (unsafe { self.create_texture(ts.size[0], ts.size[1], ts.linear) }) else {
                                    unsafe { self.destroy_buf(stg) };
                                    continue;
                                };
                                let img = tex.image;
                                if let Some(old) = self.textures.insert(ts.id, tex) {
                                    self.retired_tex.push((retire_gen, old));
                                }
                                (VK_IMAGE_LAYOUT_UNDEFINED, img)
                            }
                        }
                    };

                    let subrange = VkImageSubresourceRange {
                        aspectMask: VK_IMAGE_ASPECT_COLOR_BIT,
                        baseMipLevel: 0,
                        levelCount: 1,
                        baseArrayLayer: 0,
                        layerCount: 1,
                    };
                    let to_dst = VkImageMemoryBarrier {
                        sType: VK_STRUCTURE_TYPE_IMAGE_MEMORY_BARRIER,
                        pNext: ptr::null(),
                        srcAccessMask: if old_layout == VK_IMAGE_LAYOUT_SHADER_READ_ONLY_OPTIMAL {
                            VK_ACCESS_SHADER_READ_BIT
                        } else {
                            0
                        },
                        dstAccessMask: VK_ACCESS_TRANSFER_WRITE_BIT,
                        oldLayout: old_layout,
                        newLayout: VK_IMAGE_LAYOUT_TRANSFER_DST_OPTIMAL,
                        srcQueueFamilyIndex: VK_QUEUE_FAMILY_IGNORED,
                        dstQueueFamilyIndex: VK_QUEUE_FAMILY_IGNORED,
                        image: target_img,
                        subresourceRange: subrange,
                    };
                    let src_stage = if old_layout == VK_IMAGE_LAYOUT_SHADER_READ_ONLY_OPTIMAL {
                        VK_PIPELINE_STAGE_FRAGMENT_SHADER_BIT
                    } else {
                        VK_PIPELINE_STAGE_TOP_OF_PIPE_BIT
                    };
                    unsafe {
                        (self.dd.cmd_pipeline_barrier)(
                            cmd,
                            src_stage,
                            VK_PIPELINE_STAGE_TRANSFER_BIT,
                            0,
                            0,
                            ptr::null(),
                            0,
                            ptr::null(),
                            1,
                            &to_dst,
                        );
                    }
                    let [ox, oy] = ts.pos.unwrap_or([0, 0]);
                    let region = VkBufferImageCopy {
                        bufferOffset: 0,
                        bufferRowLength: 0,
                        bufferImageHeight: 0,
                        imageSubresource: VkImageSubresourceLayers {
                            aspectMask: VK_IMAGE_ASPECT_COLOR_BIT,
                            mipLevel: 0,
                            baseArrayLayer: 0,
                            layerCount: 1,
                        },
                        imageOffset: VkOffset3D { x: ox as i32, y: oy as i32, z: 0 },
                        imageExtent: VkExtent3D { width: ts.size[0], height: ts.size[1], depth: 1 },
                    };
                    unsafe {
                        (self.dd.cmd_copy_buffer_to_image)(
                            cmd,
                            stg.buf,
                            target_img,
                            VK_IMAGE_LAYOUT_TRANSFER_DST_OPTIMAL,
                            1,
                            &region,
                        );
                    }
                    let to_read = VkImageMemoryBarrier {
                        sType: VK_STRUCTURE_TYPE_IMAGE_MEMORY_BARRIER,
                        pNext: ptr::null(),
                        srcAccessMask: VK_ACCESS_TRANSFER_WRITE_BIT,
                        dstAccessMask: VK_ACCESS_SHADER_READ_BIT,
                        oldLayout: VK_IMAGE_LAYOUT_TRANSFER_DST_OPTIMAL,
                        newLayout: VK_IMAGE_LAYOUT_SHADER_READ_ONLY_OPTIMAL,
                        srcQueueFamilyIndex: VK_QUEUE_FAMILY_IGNORED,
                        dstQueueFamilyIndex: VK_QUEUE_FAMILY_IGNORED,
                        image: target_img,
                        subresourceRange: subrange,
                    };
                    unsafe {
                        (self.dd.cmd_pipeline_barrier)(
                            cmd,
                            VK_PIPELINE_STAGE_TRANSFER_BIT,
                            VK_PIPELINE_STAGE_FRAGMENT_SHADER_BIT,
                            0,
                            0,
                            ptr::null(),
                            0,
                            ptr::null(),
                            1,
                            &to_read,
                        );
                    }
                    new_staging.push(stg);
                }
            }
        }
        self.slots[idx].staging = new_staging;

        // 2. Upload (only when frame changed) & draw meshes
        if let Some(frame) = frame {
            let mesh_totals = (|| -> Option<(usize, usize)> {
                let mut tv = 0usize;
                let mut ti = 0usize;
                let mut total_verts = 0usize;
                let mut total_indices = 0usize;
                for m in &frame.meshes {
                    if m.vertices.len() % VERTEX_SIZE != 0 {
                        return None;
                    }
                    let vcount = m.vertices.len() / VERTEX_SIZE;
                    let vcount_u32 = u32::try_from(vcount).ok()?;
                    if m.indices.iter().any(|&idx| idx >= vcount_u32) {
                        return None;
                    }
                    let ib = m.indices.len().checked_mul(4)?;
                    tv = tv.checked_add(m.vertices.len())?;
                    ti = ti.checked_add(ib)?;
                    total_verts = total_verts.checked_add(vcount)?;
                    total_indices = total_indices.checked_add(m.indices.len())?;
                }
                i32::try_from(total_verts).ok()?;
                u32::try_from(total_indices).ok()?;
                Some((tv, ti))
            })();
            if let Some((total_vbytes, total_ibytes)) = mesh_totals
                && total_vbytes > 0
                && total_ibytes > 0
            {
                let mut vb_opt = self.slots[idx].vb.take();
                let mut ib_opt = self.slots[idx].ib.take();

                let mut grew = false;
                if vb_opt.as_ref().is_none_or(|b| b.cap < total_vbytes) {
                    if let Some(cap) = total_vbytes.checked_next_power_of_two() {
                        if let Some(old) = vb_opt.take() {
                            unsafe { self.destroy_buf(old) };
                        }
                        vb_opt = unsafe { self.create_host_buf(cap, VK_BUFFER_USAGE_VERTEX_BUFFER_BIT) };
                        grew = true;
                    } else {
                        vb_opt = None;
                    }
                }
                if ib_opt.as_ref().is_none_or(|b| b.cap < total_ibytes) {
                    if let Some(cap) = total_ibytes.checked_next_power_of_two() {
                        if let Some(old) = ib_opt.take() {
                            unsafe { self.destroy_buf(old) };
                        }
                        ib_opt = unsafe { self.create_host_buf(cap, VK_BUFFER_USAGE_INDEX_BUFFER_BIT) };
                        grew = true;
                    } else {
                        ib_opt = None;
                    }
                }

                if let (Some(vb), Some(ib)) = (vb_opt.as_ref(), ib_opt.as_ref()) {
                    let same = !grew && self.slots[idx].uploaded.as_ref().is_some_and(|u| Arc::ptr_eq(u, frame));
                    let mut ready = same;
                    if !same {
                        let mut vmap: *mut c_void = ptr::null_mut();
                        let mut imap: *mut c_void = ptr::null_mut();
                        let mapped_ok = unsafe {
                            (self.dd.map_memory)(self.device, vb.mem, 0, total_vbytes as VkDeviceSize, 0, &mut vmap) == VK_SUCCESS
                                && (self.dd.map_memory)(self.device, ib.mem, 0, total_ibytes as VkDeviceSize, 0, &mut imap) == VK_SUCCESS
                        };
                        if mapped_ok {
                            let mut voff = 0usize;
                            let mut ioff = 0usize;
                            for m in &frame.meshes {
                                if !m.vertices.is_empty() {
                                    unsafe { ptr::copy_nonoverlapping(m.vertices.as_ptr(), vmap.cast::<u8>().add(voff), m.vertices.len()) };
                                    voff += m.vertices.len();
                                }
                                if !m.indices.is_empty() {
                                    let bytes = m.indices.len() * 4;
                                    unsafe { ptr::copy_nonoverlapping(m.indices.as_ptr().cast::<u8>(), imap.cast::<u8>().add(ioff), bytes) };
                                    ioff += bytes;
                                }
                            }
                            unsafe {
                                (self.dd.unmap_memory)(self.device, vb.mem);
                                (self.dd.unmap_memory)(self.device, ib.mem);
                            }
                            self.slots[idx].uploaded = Some(Arc::clone(frame));
                            ready = true;
                        } else {
                            if !vmap.is_null() {
                                unsafe { (self.dd.unmap_memory)(self.device, vb.mem) };
                            }
                            self.slots[idx].uploaded = None;
                        }
                    }

                    if ready {
                        let rp_bi = VkRenderPassBeginInfo {
                            sType: VK_STRUCTURE_TYPE_RENDER_PASS_BEGIN_INFO,
                            pNext: ptr::null(),
                            renderPass: self.render_pass,
                            framebuffer: fb,
                            renderArea: VkRect2D {
                                offset: VkOffset2D { x: 0, y: 0 },
                                extent: VkExtent2D { width: self.width, height: self.height },
                            },
                            clearValueCount: 0,
                            pClearValues: ptr::null(),
                        };
                        let vp = VkViewport {
                            x: 0.0,
                            y: 0.0,
                            width: self.width as f32,
                            height: self.height as f32,
                            minDepth: 0.0,
                            maxDepth: 1.0,
                        };
                        let ppp = frame.pixels_per_point.max(0.1);
                        let pc = [
                            self.width as f32 / ppp,
                            self.height as f32 / ppp,
                            if self.linear_out { 1.0f32 } else { 0.0f32 },
                        ];
                        let zero_off: VkDeviceSize = 0;

                        unsafe {
                            (self.dd.cmd_begin_render_pass)(cmd, &rp_bi, VK_SUBPASS_CONTENTS_INLINE);
                            (self.dd.cmd_bind_pipeline)(cmd, VK_PIPELINE_BIND_POINT_GRAPHICS, self.pipeline);
                            (self.dd.cmd_set_viewport)(cmd, 0, 1, &vp);
                            (self.dd.cmd_push_constants)(
                                cmd,
                                self.pipe_layout,
                                VK_SHADER_STAGE_VERTEX_BIT | VK_SHADER_STAGE_FRAGMENT_BIT,
                                0,
                                12,
                                pc.as_ptr().cast(),
                            );
                            (self.dd.cmd_bind_vertex_buffers)(cmd, 0, 1, &vb.buf, &zero_off);
                            (self.dd.cmd_bind_index_buffer)(cmd, ib.buf, 0, VK_INDEX_TYPE_UINT32);
                        }

                        let mut vtx_base = 0i32;
                        let mut idx_base = 0u32;
                        for m in &frame.meshes {
                            let vcount = (m.vertices.len() / VERTEX_SIZE) as i32;
                            let icount = m.indices.len() as u32;
                            if vcount > 0 && icount > 0 {
                                if let Some(tex) = self.textures.get(&m.texture) {
                                    let min_x = (m.clip[0] * ppp).floor().clamp(0.0, self.width as f32) as i32;
                                    let min_y = (m.clip[1] * ppp).floor().clamp(0.0, self.height as f32) as i32;
                                    let max_x = (m.clip[2] * ppp).ceil().clamp(0.0, self.width as f32) as i32;
                                    let max_y = (m.clip[3] * ppp).ceil().clamp(0.0, self.height as f32) as i32;
                                    if max_x > min_x && max_y > min_y {
                                        let sc = VkRect2D {
                                            offset: VkOffset2D { x: min_x, y: min_y },
                                            extent: VkExtent2D { width: (max_x - min_x) as u32, height: (max_y - min_y) as u32 },
                                        };
                                        unsafe {
                                            (self.dd.cmd_set_scissor)(cmd, 0, 1, &sc);
                                            (self.dd.cmd_bind_descriptor_sets)(
                                                cmd,
                                                VK_PIPELINE_BIND_POINT_GRAPHICS,
                                                self.pipe_layout,
                                                0,
                                                1,
                                                &tex.set,
                                                0,
                                                ptr::null(),
                                            );
                                            (self.dd.cmd_draw_indexed)(cmd, icount, 1, idx_base, vtx_base, 0);
                                        }
                                    }
                                }
                            }
                            vtx_base += vcount;
                            idx_base += icount;
                        }
                        unsafe { (self.dd.cmd_end_render_pass)(cmd) };
                    }
                }
                self.slots[idx].vb = vb_opt;
                self.slots[idx].ib = ib_opt;
            }
        }

        if unsafe { (self.dd.end_command_buffer)(cmd) } != VK_SUCCESS {
            if let Some((buf, _)) = capture {
                unsafe { self.destroy_buf(buf) };
            }
            return (None, take_shot.then_some(Err(())));
        }

        const STAGE_MASK: VkFlags = VK_PIPELINE_STAGE_TRANSFER_BIT | VK_PIPELINE_STAGE_COLOR_ATTACHMENT_OUTPUT_BIT;
        let wait_stages_stack = [STAGE_MASK; 8];
        let wait_stages_heap: Vec<VkFlags>;
        let wait_stages_ptr = if wait_sems.is_empty() {
            ptr::null()
        } else if wait_sems.len() <= wait_stages_stack.len() {
            wait_stages_stack.as_ptr()
        } else {
            wait_stages_heap = vec![STAGE_MASK; wait_sems.len()];
            wait_stages_heap.as_ptr()
        };
        let submit = VkSubmitInfo {
            sType: VK_STRUCTURE_TYPE_SUBMIT_INFO,
            pNext: ptr::null(),
            waitSemaphoreCount: wait_sems.len() as u32,
            pWaitSemaphores: if wait_sems.is_empty() { ptr::null() } else { wait_sems.as_ptr() },
            pWaitDstStageMask: wait_stages_ptr,
            commandBufferCount: 1,
            pCommandBuffers: &cmd,
            signalSemaphoreCount: 1,
            pSignalSemaphores: &done_sem,
        };
        if unsafe { (self.dd.queue_submit)(queue, 1, &submit, fence) } != VK_SUCCESS {
            if let Some((buf, _)) = capture {
                unsafe { self.destroy_buf(buf) };
            }
            return (None, take_shot.then_some(Err(())));
        }
        self.submit_gen = self.submit_gen.wrapping_add(1);
        self.slots[idx].last_submit_gen = self.submit_gen;
        self.slots[idx].fence_pending = true;

        let shot_res = if let Some((buf, kind)) = capture {
            let mut rgb_res = Err(());
            if unsafe { (self.dd.wait_for_fences)(self.device, 1, &fence, VK_TRUE, 2_000_000_000) } == VK_SUCCESS {
                let mut mapped: *mut c_void = ptr::null_mut();
                if unsafe { (self.dd.map_memory)(self.device, buf.mem, 0, buf.cap as VkDeviceSize, 0, &mut mapped) } == VK_SUCCESS {
                    let slice = unsafe { std::slice::from_raw_parts(mapped.cast::<u8>(), buf.cap) };
                    let rgb = vk_pixels_to_rgb(kind, slice, self.width, self.height);
                    unsafe { (self.dd.unmap_memory)(self.device, buf.mem) };
                    rgb_res = Ok((self.width, self.height, rgb));
                }
                unsafe { self.destroy_buf(buf) };
            } else {
                // Fence timed out or failed: GPU copy may still be in flight,
                // so keep `buf` alive until this slot's fence signals.
                self.slots[idx].staging.push(buf);
            }
            Some(rgb_res)
        } else if shot_early_err {
            Some(Err(()))
        } else {
            None
        };

        (Some(done_sem), shot_res)
    }

    pub fn destroy(&mut self) {
        unsafe {
            // Wait only on this swapchain's own pending fences rather than calling
            // `vkDeviceWaitIdle`, which requires host synchronization across every
            // queue on the device and would race with concurrent swapchain presents.
            let pending_fences: Vec<VkFence> = self
                .slots
                .iter()
                .filter(|s| s.fence_pending)
                .map(|s| s.fence)
                .collect();
            if !pending_fences.is_empty() {
                let _ = (self.dd.wait_for_fences)(
                    self.device,
                    pending_fences.len() as u32,
                    pending_fences.as_ptr(),
                    VK_TRUE,
                    u64::MAX,
                );
            }
            let slots = std::mem::take(&mut self.slots);
            for slot in slots {
                for b in slot.staging {
                    self.destroy_buf(b);
                }
                if let Some(b) = slot.vb {
                    self.destroy_buf(b);
                }
                if let Some(b) = slot.ib {
                    self.destroy_buf(b);
                }
                (self.dd.destroy_semaphore)(self.device, slot.done_sem, ptr::null());
                (self.dd.destroy_fence)(self.device, slot.fence, ptr::null());
                (self.dd.destroy_framebuffer)(self.device, slot.fb, ptr::null());
                (self.dd.destroy_image_view)(self.device, slot.view, ptr::null());
            }
            let textures = std::mem::take(&mut self.textures);
            for (_, t) in textures {
                self.destroy_tex(t);
            }
            let retired = std::mem::take(&mut self.retired_tex);
            for (_, t) in retired {
                self.destroy_tex(t);
            }
            if self.pipeline != 0 {
                (self.dd.destroy_pipeline)(self.device, self.pipeline, ptr::null());
            }
            if self.pipe_layout != 0 {
                (self.dd.destroy_pipeline_layout)(self.device, self.pipe_layout, ptr::null());
            }
            if self.sampler_nearest != 0 {
                (self.dd.destroy_sampler)(self.device, self.sampler_nearest, ptr::null());
            }
            if self.sampler_linear != 0 {
                (self.dd.destroy_sampler)(self.device, self.sampler_linear, ptr::null());
            }
            if self.ds_pool != 0 {
                (self.dd.destroy_descriptor_pool)(self.device, self.ds_pool, ptr::null());
            }
            if self.ds_layout != 0 {
                (self.dd.destroy_descriptor_set_layout)(self.device, self.ds_layout, ptr::null());
            }
            if self.render_pass != 0 {
                (self.dd.destroy_render_pass)(self.device, self.render_pass, ptr::null());
            }
            if self.cmd_pool != 0 {
                (self.dd.destroy_command_pool)(self.device, self.cmd_pool, ptr::null());
            }
        }
    }
}
