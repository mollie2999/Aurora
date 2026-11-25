#![allow(non_snake_case)]
//! Linux Vulkan implicit layer (`libVkLayer_aurora_overlay.so`) for Aurora's
//! in-game overlay.
//!
//! Runs purely in Linux userspace inside the standard Vulkan loader
//! (`VK_ADD_IMPLICIT_LAYER_PATH` + `AURORA_VK_OVERLAY=1`), below Wine/Proton
//! and any Windows anti-cheat (e.g. GameGuard / EAC / BattlEye). Works on
//! anti-cheat Proton games (DXVK / vkd3d-proton) as well as native Linux Vulkan
//! games.

#[cfg(target_os = "linux")]
#[path = "../../steamclient/src/overlay/wire.rs"]
pub mod wire;

#[cfg(target_os = "linux")]
mod input;
#[cfg(target_os = "linux")]
mod net;
#[cfg(target_os = "linux")]
mod renderer;
#[cfg(target_os = "linux")]
mod spv;
#[cfg(target_os = "linux")]
mod vk_abi;

#[cfg(target_os = "linux")]
mod layer {
    use crate::input::{self, SurfaceKind};
    use crate::net;
    use crate::renderer::OverlayRenderer;
    use crate::vk_abi::*;
    use crate::wire::FromGame;
    use std::collections::HashMap;
    use std::ffi::{CStr, c_char, c_void};
    use std::panic::{AssertUnwindSafe, catch_unwind};
    use std::ptr;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::{Arc, LazyLock, Mutex};
    use std::time::Instant;

    #[derive(Clone, Copy)]
    enum SurfaceTarget {
        X11(u64),
        Wayland,
    }

    struct QueueInfo {
        family: u32,
        device_key: usize,
    }

    struct DeviceState {
        dd: DeviceDispatch,
        phys: VkPhysicalDevice,
        inst_key: usize,
        mem_props: VkPhysicalDeviceMemoryProperties,
        graphics_families: Vec<bool>,
    }
    unsafe impl Send for DeviceState {}

    struct SwapchainState {
        device: VkDevice,
        device_key: usize,
        surface: VkSurfaceKHR,
        format: i32,
        can_copy_src: bool,
        width: u32,
        height: u32,
        renderer: Option<OverlayRenderer>,
        init_failed: bool,
    }
    unsafe impl Send for SwapchainState {}

    struct FpsTracker {
        frames: u32,
        since: Instant,
    }

    static INSTANCES: LazyLock<Mutex<HashMap<usize, InstanceDispatch>>> = LazyLock::new(|| Mutex::new(HashMap::new()));
    static DEVICES: LazyLock<Mutex<HashMap<usize, DeviceState>>> = LazyLock::new(|| Mutex::new(HashMap::new()));
    static QUEUES: LazyLock<Mutex<HashMap<usize, QueueInfo>>> = LazyLock::new(|| Mutex::new(HashMap::new()));
    static SURFACES: LazyLock<Mutex<HashMap<VkSurfaceKHR, SurfaceTarget>>> = LazyLock::new(|| Mutex::new(HashMap::new()));
    static SWAPCHAINS: LazyLock<Mutex<HashMap<VkSwapchainKHR, (usize, Arc<Mutex<SwapchainState>>)>>> = LazyLock::new(|| Mutex::new(HashMap::new()));
    static FPS: LazyLock<Mutex<Option<FpsTracker>>> = LazyLock::new(|| Mutex::new(None));
    static LAST_FPS: AtomicU32 = AtomicU32::new(0);

    unsafe fn load_inst_fn<T>(gipa: PFN_vkGetInstanceProcAddr, inst: VkInstance, name: &CStr) -> Option<T> {
        let p = unsafe { gipa(inst, name.as_ptr()) }?;
        Some(unsafe { std::mem::transmute_copy(&p) })
    }

    unsafe fn load_dev_fn<T>(gdpa: PFN_vkGetDeviceProcAddr, dev: VkDevice, name: &CStr) -> Option<T> {
        let p = unsafe { gdpa(dev, name.as_ptr()) }?;
        Some(unsafe { std::mem::transmute_copy(&p) })
    }

    fn find_layer_inst_link(p_create_info: *const c_void) -> *mut VkLayerInstanceCreateInfo {
        let mut cur = p_create_info as *const VkBaseInStructure;
        while !cur.is_null() {
            unsafe {
                if (*cur).sType == VK_STRUCTURE_TYPE_LOADER_INSTANCE_CREATE_INFO {
                    let info = cur as *mut VkLayerInstanceCreateInfo;
                    if (*info).function == VK_LAYER_LINK_INFO {
                        return info;
                    }
                }
                cur = (*cur).pNext;
            }
        }
        ptr::null_mut()
    }

    fn find_layer_dev_link(p_create_info: *const VkDeviceCreateInfo) -> *mut VkLayerDeviceCreateInfo {
        let mut cur = p_create_info as *const VkBaseInStructure;
        while !cur.is_null() {
            unsafe {
                if (*cur).sType == VK_STRUCTURE_TYPE_LOADER_DEVICE_CREATE_INFO {
                    let info = cur as *mut VkLayerDeviceCreateInfo;
                    if (*info).function == VK_LAYER_LINK_INFO {
                        return info;
                    }
                }
                cur = (*cur).pNext;
            }
        }
        ptr::null_mut()
    }

    // ---- Intercepted Vulkan functions --------------------------------------

    unsafe extern "C" fn hooked_create_instance(
        p_create_info: *const c_void,
        p_allocator: *const c_void,
        p_instance: *mut VkInstance,
    ) -> VkResult {
        let link_ci = find_layer_inst_link(p_create_info);
        if link_ci.is_null() {
            return -3; // VK_ERROR_INITIALIZATION_FAILED
        }
        let link = unsafe { (*link_ci).pLayerInfo };
        if link.is_null() {
            return -3;
        }
        let next_gipa = unsafe { (*link).pfnNextGetInstanceProcAddr };
        unsafe { (*link_ci).pLayerInfo = (*link).pNext };

        let Some(next_create): Option<PFN_vkCreateInstance> = (unsafe { load_inst_fn(next_gipa, ptr::null_mut(), c"vkCreateInstance") }) else {
            return -3;
        };
        let res = unsafe { next_create(p_create_info, p_allocator, p_instance) };
        if res != VK_SUCCESS || p_instance.is_null() {
            return res;
        }
        let inst = unsafe { *p_instance };
        let key = unsafe { dispatch_key(inst) };
        if let Some(destroy_instance) = unsafe { load_inst_fn::<PFN_vkDestroyInstance>(next_gipa, inst, c"vkDestroyInstance") } {
            let id = InstanceDispatch {
                get_instance_proc_addr: next_gipa,
                destroy_instance,
                create_xlib_surface: unsafe { load_inst_fn(next_gipa, inst, c"vkCreateXlibSurfaceKHR") },
                create_xcb_surface: unsafe { load_inst_fn(next_gipa, inst, c"vkCreateXcbSurfaceKHR") },
                create_wayland_surface: unsafe { load_inst_fn(next_gipa, inst, c"vkCreateWaylandSurfaceKHR") },
                destroy_surface: unsafe { load_inst_fn(next_gipa, inst, c"vkDestroySurfaceKHR") },
                queue_family_properties: unsafe { load_inst_fn(next_gipa, inst, c"vkGetPhysicalDeviceQueueFamilyProperties") },
                get_physical_device_memory_properties: unsafe { load_inst_fn(next_gipa, inst, c"vkGetPhysicalDeviceMemoryProperties") },
                get_physical_device_surface_capabilities: unsafe { load_inst_fn(next_gipa, inst, c"vkGetPhysicalDeviceSurfaceCapabilitiesKHR") },
            };
            INSTANCES.lock().unwrap_or_else(|e| e.into_inner()).insert(key, id);
        }
        res
    }

    unsafe extern "C" fn hooked_destroy_instance(instance: VkInstance, p_allocator: *const c_void) {
        let key = unsafe { dispatch_key(instance) };
        let (id, empty) = {
            let mut g = INSTANCES.lock().unwrap_or_else(|e| e.into_inner());
            (g.remove(&key), g.is_empty())
        };
        if empty {
            net::shared().shutdown();
        }
        if let Some(id) = id {
            unsafe { (id.destroy_instance)(instance, p_allocator) };
        }
    }

    unsafe extern "C" fn hooked_create_xlib_surface(
        instance: VkInstance,
        p_create_info: *const VkXlibSurfaceCreateInfoKHR,
        p_allocator: *const c_void,
        p_surface: *mut VkSurfaceKHR,
    ) -> VkResult {
        let key = unsafe { dispatch_key(instance) };
        let f = INSTANCES.lock().unwrap_or_else(|e| e.into_inner()).get(&key).and_then(|d| d.create_xlib_surface);
        let Some(f) = f else { return -3 };
        let res = unsafe { f(instance, p_create_info, p_allocator, p_surface) };
        if res == VK_SUCCESS && !p_create_info.is_null() && !p_surface.is_null() {
            let win = unsafe { (*p_create_info).window as u64 };
            let surf = unsafe { *p_surface };
            SURFACES.lock().unwrap_or_else(|e| e.into_inner()).insert(surf, SurfaceTarget::X11(win));
        }
        res
    }

    unsafe extern "C" fn hooked_create_xcb_surface(
        instance: VkInstance,
        p_create_info: *const VkXcbSurfaceCreateInfoKHR,
        p_allocator: *const c_void,
        p_surface: *mut VkSurfaceKHR,
    ) -> VkResult {
        let key = unsafe { dispatch_key(instance) };
        let f = INSTANCES.lock().unwrap_or_else(|e| e.into_inner()).get(&key).and_then(|d| d.create_xcb_surface);
        let Some(f) = f else { return -3 };
        let res = unsafe { f(instance, p_create_info, p_allocator, p_surface) };
        if res == VK_SUCCESS && !p_create_info.is_null() && !p_surface.is_null() {
            let win = unsafe { (*p_create_info).window as u64 };
            let surf = unsafe { *p_surface };
            SURFACES.lock().unwrap_or_else(|e| e.into_inner()).insert(surf, SurfaceTarget::X11(win));
        }
        res
    }

    unsafe extern "C" fn hooked_create_wayland_surface(
        instance: VkInstance,
        p_create_info: *const VkWaylandSurfaceCreateInfoKHR,
        p_allocator: *const c_void,
        p_surface: *mut VkSurfaceKHR,
    ) -> VkResult {
        let key = unsafe { dispatch_key(instance) };
        let f = INSTANCES.lock().unwrap_or_else(|e| e.into_inner()).get(&key).and_then(|d| d.create_wayland_surface);
        let Some(f) = f else { return -3 };
        let res = unsafe { f(instance, p_create_info, p_allocator, p_surface) };
        if res == VK_SUCCESS && !p_surface.is_null() {
            let surf = unsafe { *p_surface };
            SURFACES.lock().unwrap_or_else(|e| e.into_inner()).insert(surf, SurfaceTarget::Wayland);
        }
        res
    }

    unsafe extern "C" fn hooked_destroy_surface(
        instance: VkInstance,
        surface: VkSurfaceKHR,
        p_allocator: *const c_void,
    ) {
        SURFACES.lock().unwrap_or_else(|e| e.into_inner()).remove(&surface);
        let key = unsafe { dispatch_key(instance) };
        let f = INSTANCES.lock().unwrap_or_else(|e| e.into_inner()).get(&key).and_then(|d| d.destroy_surface);
        if let Some(f) = f {
            unsafe { f(instance, surface, p_allocator) };
        }
    }

    unsafe extern "C" fn hooked_create_device(
        physical_device: VkPhysicalDevice,
        p_create_info: *const VkDeviceCreateInfo,
        p_allocator: *const c_void,
        p_device: *mut VkDevice,
    ) -> VkResult {
        let link_ci = find_layer_dev_link(p_create_info);
        if link_ci.is_null() {
            return -3;
        }
        let link = unsafe { (*link_ci).pLayerInfo };
        if link.is_null() {
            return -3;
        }
        let next_gipa = unsafe { (*link).pfnNextGetInstanceProcAddr };
        let next_gdpa = unsafe { (*link).pfnNextGetDeviceProcAddr };
        unsafe { (*link_ci).pLayerInfo = (*link).pNext };

        let Some(next_create): Option<PFN_vkCreateDevice> = (unsafe { load_inst_fn(next_gipa, ptr::null_mut(), c"vkCreateDevice") }) else {
            return -3;
        };
        let res = unsafe { next_create(physical_device, p_create_info, p_allocator, p_device) };
        if res != VK_SUCCESS || p_device.is_null() {
            return res;
        }

        let dev = unsafe { *p_device };
        let dev_key = unsafe { dispatch_key(dev) };
        let inst_key = unsafe { dispatch_key(physical_device) };

        let mut mem_props = VkPhysicalDeviceMemoryProperties::default();
        if let Some(get_mem) = INSTANCES
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&inst_key)
            .and_then(|d| d.get_physical_device_memory_properties)
        {
            unsafe { get_mem(physical_device, &mut mem_props) };
        }

        let mut graphics_families = Vec::new();
        if let Some(query) = INSTANCES.lock().unwrap_or_else(|e| e.into_inner()).get(&inst_key).and_then(|d| d.queue_family_properties) {
            let mut count = 0;
            unsafe { query(physical_device, &mut count, ptr::null_mut()) };
            let mut props = vec![VkQueueFamilyProperties::default(); count as usize];
            unsafe { query(physical_device, &mut count, props.as_mut_ptr()) };
            graphics_families.extend(props.iter().take(count as usize).map(|p| p.queueFlags & 1 != 0));
        }

        if let Some(dd) = unsafe { load_device_dispatch(next_gdpa, dev) } {
            DEVICES.lock().unwrap_or_else(|e| e.into_inner()).insert(
                dev_key,
                DeviceState {
                    dd,
                    phys: physical_device,
                    inst_key,
                    mem_props,
                    graphics_families,
                },
            );
        }
        res
    }

    unsafe fn load_device_dispatch(gdpa: PFN_vkGetDeviceProcAddr, dev: VkDevice) -> Option<DeviceDispatch> {
        unsafe {
            Some(DeviceDispatch {
                get_device_proc_addr: gdpa,
                destroy_device: load_dev_fn(gdpa, dev, c"vkDestroyDevice")?,
                get_device_queue: load_dev_fn(gdpa, dev, c"vkGetDeviceQueue")?,
                get_device_queue2: load_dev_fn(gdpa, dev, c"vkGetDeviceQueue2"),
                create_swapchain: load_dev_fn(gdpa, dev, c"vkCreateSwapchainKHR"),
                destroy_swapchain: load_dev_fn(gdpa, dev, c"vkDestroySwapchainKHR"),
                get_swapchain_images: load_dev_fn(gdpa, dev, c"vkGetSwapchainImagesKHR"),
                queue_present: load_dev_fn(gdpa, dev, c"vkQueuePresentKHR"),
                queue_submit: load_dev_fn(gdpa, dev, c"vkQueueSubmit")?,
                device_wait_idle: load_dev_fn(gdpa, dev, c"vkDeviceWaitIdle")?,

                create_command_pool: load_dev_fn(gdpa, dev, c"vkCreateCommandPool")?,
                destroy_command_pool: load_dev_fn(gdpa, dev, c"vkDestroyCommandPool")?,
                allocate_command_buffers: load_dev_fn(gdpa, dev, c"vkAllocateCommandBuffers")?,
                begin_command_buffer: load_dev_fn(gdpa, dev, c"vkBeginCommandBuffer")?,
                end_command_buffer: load_dev_fn(gdpa, dev, c"vkEndCommandBuffer")?,
                reset_command_buffer: load_dev_fn(gdpa, dev, c"vkResetCommandBuffer")?,

                create_semaphore: load_dev_fn(gdpa, dev, c"vkCreateSemaphore")?,
                destroy_semaphore: load_dev_fn(gdpa, dev, c"vkDestroySemaphore")?,
                create_fence: load_dev_fn(gdpa, dev, c"vkCreateFence")?,
                destroy_fence: load_dev_fn(gdpa, dev, c"vkDestroyFence")?,
                wait_for_fences: load_dev_fn(gdpa, dev, c"vkWaitForFences")?,
                reset_fences: load_dev_fn(gdpa, dev, c"vkResetFences")?,

                create_image: load_dev_fn(gdpa, dev, c"vkCreateImage")?,
                destroy_image: load_dev_fn(gdpa, dev, c"vkDestroyImage")?,
                get_image_memory_requirements: load_dev_fn(gdpa, dev, c"vkGetImageMemoryRequirements")?,
                bind_image_memory: load_dev_fn(gdpa, dev, c"vkBindImageMemory")?,
                create_image_view: load_dev_fn(gdpa, dev, c"vkCreateImageView")?,
                destroy_image_view: load_dev_fn(gdpa, dev, c"vkDestroyImageView")?,

                create_buffer: load_dev_fn(gdpa, dev, c"vkCreateBuffer")?,
                destroy_buffer: load_dev_fn(gdpa, dev, c"vkDestroyBuffer")?,
                get_buffer_memory_requirements: load_dev_fn(gdpa, dev, c"vkGetBufferMemoryRequirements")?,
                bind_buffer_memory: load_dev_fn(gdpa, dev, c"vkBindBufferMemory")?,
                allocate_memory: load_dev_fn(gdpa, dev, c"vkAllocateMemory")?,
                free_memory: load_dev_fn(gdpa, dev, c"vkFreeMemory")?,
                map_memory: load_dev_fn(gdpa, dev, c"vkMapMemory")?,
                unmap_memory: load_dev_fn(gdpa, dev, c"vkUnmapMemory")?,

                create_shader_module: load_dev_fn(gdpa, dev, c"vkCreateShaderModule")?,
                destroy_shader_module: load_dev_fn(gdpa, dev, c"vkDestroyShaderModule")?,
                create_render_pass: load_dev_fn(gdpa, dev, c"vkCreateRenderPass")?,
                destroy_render_pass: load_dev_fn(gdpa, dev, c"vkDestroyRenderPass")?,
                create_framebuffer: load_dev_fn(gdpa, dev, c"vkCreateFramebuffer")?,
                destroy_framebuffer: load_dev_fn(gdpa, dev, c"vkDestroyFramebuffer")?,
                create_descriptor_set_layout: load_dev_fn(gdpa, dev, c"vkCreateDescriptorSetLayout")?,
                destroy_descriptor_set_layout: load_dev_fn(gdpa, dev, c"vkDestroyDescriptorSetLayout")?,
                create_descriptor_pool: load_dev_fn(gdpa, dev, c"vkCreateDescriptorPool")?,
                destroy_descriptor_pool: load_dev_fn(gdpa, dev, c"vkDestroyDescriptorPool")?,
                allocate_descriptor_sets: load_dev_fn(gdpa, dev, c"vkAllocateDescriptorSets")?,
                free_descriptor_sets: load_dev_fn(gdpa, dev, c"vkFreeDescriptorSets")?,
                update_descriptor_sets: load_dev_fn(gdpa, dev, c"vkUpdateDescriptorSets")?,
                create_sampler: load_dev_fn(gdpa, dev, c"vkCreateSampler")?,
                destroy_sampler: load_dev_fn(gdpa, dev, c"vkDestroySampler")?,
                create_pipeline_layout: load_dev_fn(gdpa, dev, c"vkCreatePipelineLayout")?,
                destroy_pipeline_layout: load_dev_fn(gdpa, dev, c"vkDestroyPipelineLayout")?,
                create_graphics_pipelines: load_dev_fn(gdpa, dev, c"vkCreateGraphicsPipelines")?,
                destroy_pipeline: load_dev_fn(gdpa, dev, c"vkDestroyPipeline")?,

                cmd_pipeline_barrier: load_dev_fn(gdpa, dev, c"vkCmdPipelineBarrier")?,
                cmd_copy_buffer_to_image: load_dev_fn(gdpa, dev, c"vkCmdCopyBufferToImage")?,
                cmd_copy_image_to_buffer: load_dev_fn(gdpa, dev, c"vkCmdCopyImageToBuffer"),
                cmd_begin_render_pass: load_dev_fn(gdpa, dev, c"vkCmdBeginRenderPass")?,
                cmd_end_render_pass: load_dev_fn(gdpa, dev, c"vkCmdEndRenderPass")?,
                cmd_bind_pipeline: load_dev_fn(gdpa, dev, c"vkCmdBindPipeline")?,
                cmd_set_viewport: load_dev_fn(gdpa, dev, c"vkCmdSetViewport")?,
                cmd_set_scissor: load_dev_fn(gdpa, dev, c"vkCmdSetScissor")?,
                cmd_push_constants: load_dev_fn(gdpa, dev, c"vkCmdPushConstants")?,
                cmd_bind_vertex_buffers: load_dev_fn(gdpa, dev, c"vkCmdBindVertexBuffers")?,
                cmd_bind_index_buffer: load_dev_fn(gdpa, dev, c"vkCmdBindIndexBuffer")?,
                cmd_bind_descriptor_sets: load_dev_fn(gdpa, dev, c"vkCmdBindDescriptorSets")?,
                cmd_draw_indexed: load_dev_fn(gdpa, dev, c"vkCmdDrawIndexed")?,
            })
        }
    }

    unsafe extern "C" fn hooked_destroy_device(device: VkDevice, p_allocator: *const c_void) {
        let key = unsafe { dispatch_key(device) };
        // Remove matching swapchains under the map lock, then destroy their renderers outside it.
        let removed: Vec<Arc<Mutex<SwapchainState>>> = {
            let mut sws = SWAPCHAINS.lock().unwrap_or_else(|e| e.into_inner());
            let keys: Vec<VkSwapchainKHR> = sws
                .iter()
                .filter_map(|(&k, (dev_key, _))| (*dev_key == key).then_some(k))
                .collect();
            keys.into_iter().filter_map(|k| sws.remove(&k).map(|(_, entry)| entry)).collect()
        };
        for entry in removed {
            let mut s = entry.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(mut r) = s.renderer.take() {
                r.destroy();
            }
        }
        QUEUES.lock().unwrap_or_else(|e| e.into_inner()).retain(|_, info| info.device_key != key);
        let state = DEVICES.lock().unwrap_or_else(|e| e.into_inner()).remove(&key);
        if let Some(state) = state {
            unsafe { (state.dd.destroy_device)(device, p_allocator) };
        }
    }

    unsafe extern "C" fn hooked_get_device_queue(
        device: VkDevice,
        queue_family_index: u32,
        queue_index: u32,
        p_queue: *mut VkQueue,
    ) {
        let key = unsafe { dispatch_key(device) };
        let f = DEVICES.lock().unwrap_or_else(|e| e.into_inner()).get(&key).map(|s| s.dd.get_device_queue);
        if let Some(f) = f {
            unsafe { f(device, queue_family_index, queue_index, p_queue) };
            if !p_queue.is_null() {
                let q = unsafe { *p_queue } as usize;
                QUEUES.lock().unwrap_or_else(|e| e.into_inner()).insert(q, QueueInfo { family: queue_family_index, device_key: key });
            }
        }
    }

    unsafe extern "C" fn hooked_get_device_queue2(
        device: VkDevice,
        p_queue_info: *const VkDeviceQueueInfo2,
        p_queue: *mut VkQueue,
    ) {
        let key = unsafe { dispatch_key(device) };
        let f = DEVICES.lock().unwrap_or_else(|e| e.into_inner()).get(&key).and_then(|s| s.dd.get_device_queue2);
        if let Some(f) = f {
            unsafe { f(device, p_queue_info, p_queue) };
            if !p_queue.is_null() && !p_queue_info.is_null() {
                let q = unsafe { *p_queue } as usize;
                let fam = unsafe { (*p_queue_info).queueFamilyIndex };
                QUEUES.lock().unwrap_or_else(|e| e.into_inner()).insert(q, QueueInfo { family: fam, device_key: key });
            }
        }
    }

    unsafe extern "C" fn hooked_create_swapchain(
        device: VkDevice,
        p_create_info: *const VkSwapchainCreateInfoKHR,
        p_allocator: *const c_void,
        p_swapchain: *mut VkSwapchainKHR,
    ) -> VkResult {
        let dev_key = unsafe { dispatch_key(device) };
        let (create_fn, phys, inst_key) = {
            let g = DEVICES.lock().unwrap_or_else(|e| e.into_inner());
            let Some(st) = g.get(&dev_key) else { return -3 };
            (st.dd.create_swapchain, st.phys, st.inst_key)
        };
        let Some(create_fn) = create_fn else { return -3 };
        if p_create_info.is_null() {
            return unsafe { create_fn(device, p_create_info, p_allocator, p_swapchain) };
        }

        let mut ci = unsafe { *p_create_info };
        // Ensure COLOR_ATTACHMENT_BIT (drawing overlay) and TRANSFER_SRC_BIT (F12 screenshot copy)
        // are enabled when the surface supports them.
        let need_bits = VK_IMAGE_USAGE_COLOR_ATTACHMENT_BIT | VK_IMAGE_USAGE_TRANSFER_SRC_BIT;
        if (ci.imageUsage & need_bits) != need_bits {
            let caps_fn = INSTANCES
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .get(&inst_key)
                .and_then(|d| d.get_physical_device_surface_capabilities);
            if let Some(caps_fn) = caps_fn {
                let mut caps: VkSurfaceCapabilitiesKHR = unsafe { std::mem::zeroed() };
                if unsafe { caps_fn(phys, ci.surface, &mut caps) } == VK_SUCCESS {
                    ci.imageUsage |= caps.supportedUsageFlags & need_bits;
                }
            }
        }

        let res = unsafe { create_fn(device, &ci, p_allocator, p_swapchain) };
        if res == VK_SUCCESS && !p_swapchain.is_null() {
            let sw = unsafe { *p_swapchain };
            SWAPCHAINS.lock().unwrap_or_else(|e| e.into_inner()).insert(
                sw,
                (
                    dev_key,
                    Arc::new(Mutex::new(SwapchainState {
                        device,
                        device_key: dev_key,
                        surface: ci.surface,
                        format: ci.imageFormat,
                        can_copy_src: (ci.imageUsage & VK_IMAGE_USAGE_TRANSFER_SRC_BIT) != 0,
                        width: ci.imageExtent.width,
                        height: ci.imageExtent.height,
                        renderer: None,
                        init_failed: false,
                    })),
                ),
            );
        }
        res
    }

    unsafe extern "C" fn hooked_destroy_swapchain(
        device: VkDevice,
        swapchain: VkSwapchainKHR,
        p_allocator: *const c_void,
    ) {
        let removed = SWAPCHAINS.lock().unwrap_or_else(|e| e.into_inner()).remove(&swapchain);
        if let Some((_, entry)) = removed {
            let mut s = entry.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(mut r) = s.renderer.take() {
                r.destroy();
            }
        }
        let dev_key = unsafe { dispatch_key(device) };
        let f = DEVICES.lock().unwrap_or_else(|e| e.into_inner()).get(&dev_key).and_then(|s| s.dd.destroy_swapchain);
        if let Some(f) = f {
            unsafe { f(device, swapchain, p_allocator) };
        }
    }

    unsafe extern "C" fn hooked_queue_present(
        queue: VkQueue,
        p_present_info: *const VkPresentInfoKHR,
    ) -> VkResult {
        let dev_key = unsafe { dispatch_key(queue) };
        let present_fn = DEVICES.lock().unwrap_or_else(|e| e.into_inner()).get(&dev_key).and_then(|s| s.dd.queue_present);
        let Some(present_fn) = present_fn else { return -3 };

        if !net::is_enabled() || p_present_info.is_null() {
            return unsafe { present_fn(queue, p_present_info) };
        }

        let modified = catch_unwind(AssertUnwindSafe(|| unsafe {
            let pi = *p_present_info;
            if pi.swapchainCount == 0 || pi.pSwapchains.is_null() || pi.pImageIndices.is_null() {
                return None;
            }
            let sw_handle = *pi.pSwapchains;
            let img_idx = *pi.pImageIndices;

            let wait_sems: &[VkSemaphore] = if pi.waitSemaphoreCount > 0 && !pi.pWaitSemaphores.is_null() {
                std::slice::from_raw_parts(pi.pWaitSemaphores, pi.waitSemaphoreCount as usize)
            } else {
                &[]
            };

            let queue_family = QUEUES
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .get(&(queue as usize))
                .filter(|i| i.device_key == dev_key)
                .map(|i| i.family)?;

            let sw_entry = SWAPCHAINS
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .get(&sw_handle)
                .map(|(_, entry)| entry.clone())?;
            let mut sw = sw_entry.lock().unwrap_or_else(|e| e.into_inner());

            if sw.device_key != dev_key || sw.renderer.as_ref().is_some_and(|r| r.queue_family != queue_family)
                || !DEVICES.lock().unwrap_or_else(|e| e.into_inner()).get(&dev_key)?
                    .graphics_families.get(queue_family as usize).copied().unwrap_or(false)
            { return None; }

            // Ignore tiny helper/dummy swapchains (e.g. launcher splash windows).
            if sw.width < 128 || sw.height < 128 {
                return None;
            }

            if let Some(&target) = SURFACES.lock().unwrap_or_else(|e| e.into_inner()).get(&sw.surface) {
                input::set_surface(match target {
                    SurfaceTarget::X11(win) => SurfaceKind::X11(win),
                    SurfaceTarget::Wayland => SurfaceKind::Wayland,
                });
            }
            input::ensure_started();
            net::shared().update_size(sw.width, sw.height);

            // Emit FPS once a second for Aurora's FPS counter overlay.
            if let Ok(mut g) = FPS.lock() {
                let st = g.get_or_insert_with(|| FpsTracker { frames: 0, since: Instant::now() });
                st.frames += 1;
                let elapsed = st.since.elapsed().as_secs_f32();
                if elapsed >= 1.0 {
                    let fps = (st.frames as f32 / elapsed).round() as u32;
                    LAST_FPS.store(fps, Ordering::Relaxed);
                    net::shared().send(FromGame::Fps(fps));
                    st.frames = 0;
                    st.since = Instant::now();
                }
            }

            let take_shot = net::shared().take_screenshot_request();
            let frame = net::shared().current_frame();
            let mut tex_ops = net::shared().drain_tex_ops();
            if frame.is_none() && tex_ops.is_empty() && !take_shot {
                return None;
            }

            if sw.renderer.is_none() && !sw.init_failed {
                let dev_info = {
                    let g = DEVICES.lock().unwrap_or_else(|e| e.into_inner());
                    g.get(&sw.device_key).map(|d| (d.dd.clone(), d.mem_props))
                };
                if let Some((dd, mem_props)) = dev_info {
                    sw.renderer = OverlayRenderer::new(sw.device, dd, mem_props, queue_family, sw_handle, sw.format, sw.can_copy_src, sw.width, sw.height);
                    if sw.renderer.is_some() {
                        // Seed the freshly created swapchain renderer with every live
                        // texture (especially egui's font atlas id=0) before applying deltas.
                        let mut seeded = net::shared().snapshot_tex_ops();
                        seeded.append(&mut tex_ops);
                        tex_ops = seeded;
                    } else {
                        sw.init_failed = true;
                    }
                }
            }

            let Some(renderer) = sw.renderer.as_mut() else {
                if take_shot {
                    net::shared().send(FromGame::Screenshot(false));
                }
                return None;
            };
            let (done_sem, shot_res) = renderer.process_present(queue, img_idx, wait_sems, frame.as_ref(), tex_ops, take_shot);
            match shot_res {
                Some(Ok((w, h, rgb))) => net::shared().send_screenshot_rgb(w, h, rgb),
                Some(Err(())) => net::shared().send(FromGame::Screenshot(false)),
                None => {}
            }
            done_sem.map(|sem| (pi, sem))
        }))
        .ok()
        .flatten();

        if let Some((mut pi, done_sem)) = modified {
            pi.waitSemaphoreCount = 1;
            pi.pWaitSemaphores = &done_sem;
            unsafe { present_fn(queue, &pi) }
        } else {
            unsafe { present_fn(queue, p_present_info) }
        }
    }

    // ---- Khronos Loader Layer Interface v2 Entry Points --------------------

    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn vkGetInstanceProcAddr(
        instance: VkInstance,
        p_name: *const c_char,
    ) -> PFN_vkVoidFunction {
        if p_name.is_null() {
            return None;
        }
        let name = unsafe { CStr::from_ptr(p_name) }.to_bytes();
        let intercepted: Option<*const ()> = match name {
            b"vkGetInstanceProcAddr" => Some(vkGetInstanceProcAddr as *const ()),
            b"vkCreateInstance" => Some(hooked_create_instance as *const ()),
            b"vkDestroyInstance" => Some(hooked_destroy_instance as *const ()),
            b"vkCreateXlibSurfaceKHR" => Some(hooked_create_xlib_surface as *const ()),
            b"vkCreateXcbSurfaceKHR" => Some(hooked_create_xcb_surface as *const ()),
            b"vkCreateWaylandSurfaceKHR" => Some(hooked_create_wayland_surface as *const ()),
            b"vkDestroySurfaceKHR" => Some(hooked_destroy_surface as *const ()),
            b"vkCreateDevice" => Some(hooked_create_device as *const ()),
            b"vkGetDeviceProcAddr" => Some(vkGetDeviceProcAddr as *const ()),
            b"vkDestroyDevice" => Some(hooked_destroy_device as *const ()),
            b"vkGetDeviceQueue" => Some(hooked_get_device_queue as *const ()),
            b"vkGetDeviceQueue2" => Some(hooked_get_device_queue2 as *const ()),
            b"vkCreateSwapchainKHR" => Some(hooked_create_swapchain as *const ()),
            b"vkDestroySwapchainKHR" => Some(hooked_destroy_swapchain as *const ()),
            b"vkQueuePresentKHR" => Some(hooked_queue_present as *const ()),
            _ => None,
        };
        if let Some(p) = intercepted {
            return Some(unsafe { std::mem::transmute::<*const (), unsafe extern "C" fn()>(p) });
        }
        if instance.is_null() {
            return None;
        }
        let key = unsafe { dispatch_key(instance) };
        let next = INSTANCES.lock().unwrap_or_else(|e| e.into_inner()).get(&key).map(|d| d.get_instance_proc_addr);
        next.and_then(|f| unsafe { f(instance, p_name) })
    }

    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn vkGetDeviceProcAddr(
        device: VkDevice,
        p_name: *const c_char,
    ) -> PFN_vkVoidFunction {
        if p_name.is_null() {
            return None;
        }
        let name = unsafe { CStr::from_ptr(p_name) }.to_bytes();
        let intercepted: Option<*const ()> = match name {
            b"vkGetDeviceProcAddr" => Some(vkGetDeviceProcAddr as *const ()),
            b"vkDestroyDevice" => Some(hooked_destroy_device as *const ()),
            b"vkGetDeviceQueue" => Some(hooked_get_device_queue as *const ()),
            b"vkGetDeviceQueue2" => Some(hooked_get_device_queue2 as *const ()),
            b"vkCreateSwapchainKHR" => Some(hooked_create_swapchain as *const ()),
            b"vkDestroySwapchainKHR" => Some(hooked_destroy_swapchain as *const ()),
            b"vkQueuePresentKHR" => Some(hooked_queue_present as *const ()),
            _ => None,
        };
        if let Some(p) = intercepted {
            return Some(unsafe { std::mem::transmute::<*const (), unsafe extern "C" fn()>(p) });
        }
        if device.is_null() {
            return None;
        }
        let key = unsafe { dispatch_key(device) };
        let next = DEVICES.lock().unwrap_or_else(|e| e.into_inner()).get(&key).map(|s| s.dd.get_device_proc_addr);
        next.and_then(|f| unsafe { f(device, p_name) })
    }

    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn vkNegotiateLoaderLayerInterfaceVersion(
        p_version_struct: *mut VkNegotiateLayerInterface,
    ) -> VkResult {
        if p_version_struct.is_null() {
            return -1;
        }
        let v = unsafe { &mut *p_version_struct };
        if v.sType != LAYER_NEGOTIATE_INTERFACE_STRUCT {
            return -1;
        }
        if v.loaderLayerInterfaceVersion > CURRENT_LOADER_LAYER_INTERFACE_VERSION {
            v.loaderLayerInterfaceVersion = CURRENT_LOADER_LAYER_INTERFACE_VERSION;
        }
        v.pfnGetInstanceProcAddr = Some(vkGetInstanceProcAddr);
        v.pfnGetDeviceProcAddr = Some(vkGetDeviceProcAddr);
        v.pfnGetPhysicalDeviceProcAddr = ptr::null();
        VK_SUCCESS
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;

    #[test]
    fn spirv_magic_valid() {
        assert_eq!(spv::VS_SPV[0], 0x0723_0203);
        assert_eq!(spv::FS_SPV[0], 0x0723_0203);
    }

    #[test]
    fn keysym_mapping_covers_nav_and_alnum() {
        assert_eq!(input::keysym_to_vk(0xff1b), Some(0x1B)); // Esc
        assert_eq!(input::keysym_to_vk(0xff09), Some(0x09)); // Tab
        assert_eq!(input::keysym_to_vk(0xff0d), Some(0x0D)); // Enter
        assert_eq!(input::keysym_to_vk(b'a' as u64), Some(b'A' as u32));
        assert_eq!(input::keysym_to_vk(b'5' as u64), Some(b'5' as u32));
        for vk in [0x09, 0x1B, 0x20, 0x70, 0x7B, b'A' as u32, b'Z' as u32, b'0' as u32, 0xC0] {
            let sym = input::vk_to_keysym(vk).expect("vk_to_keysym");
            assert_eq!(input::keysym_to_vk(sym), Some(vk));
        }
    }

    #[test]
    fn converts_vulkan_swapchain_pixels_to_rgb() {
        use renderer::{PixelKind, vk_pixel_kind, vk_pixels_to_rgb};
        assert_eq!(vk_pixel_kind(vk_abi::VK_FORMAT_B8G8R8A8_SRGB), Some(PixelKind::Bgra8));
        assert_eq!(vk_pixel_kind(vk_abi::VK_FORMAT_R8G8B8A8_UNORM), Some(PixelKind::Rgba8));
        assert_eq!(vk_pixel_kind(vk_abi::VK_FORMAT_A2B10G10R10_UNORM_PACK32), Some(PixelKind::Rgb10a2));
        assert_eq!(vk_pixel_kind(vk_abi::VK_FORMAT_A2R10G10B10_UNORM_PACK32), Some(PixelKind::Bgr10a2));
        assert_eq!(vk_pixel_kind(vk_abi::VK_FORMAT_R16G16B16A16_SFLOAT), Some(PixelKind::Half));

        assert_eq!(vk_pixels_to_rgb(PixelKind::Rgba8, &[10, 20, 30, 255], 1, 1), vec![10, 20, 30]);
        assert_eq!(vk_pixels_to_rgb(PixelKind::Bgra8, &[10, 20, 30, 255], 1, 1), vec![30, 20, 10]);
        assert_eq!(vk_pixels_to_rgb(PixelKind::Rgb10a2, &(1023u32 | (512 << 20)).to_le_bytes(), 1, 1), vec![255, 0, 128]);
        assert_eq!(vk_pixels_to_rgb(PixelKind::Bgr10a2, &(1023u32 | (512 << 20)).to_le_bytes(), 1, 1), vec![128, 0, 255]);
        assert_eq!(vk_pixels_to_rgb(PixelKind::Half, &[0x00, 0x3C, 0x00, 0x00, 0x00, 0x38, 0x00, 0x3C], 1, 1), vec![255, 0, 188]);
    }

    #[test]
    fn hotkey_env_parsing_and_proc_interception() {
        assert_eq!(net::parse_hotkey_env(Some("2:120"), (1, 9)), (2, 120));
        assert_eq!(net::parse_hotkey_env(Some("bad"), (1, 9)), (1, 9));
        assert_eq!(net::parse_hotkey_env(Some("1:0"), (1, 9)), (1, 9));

        unsafe {
            assert!(layer::vkGetInstanceProcAddr(std::ptr::null_mut(), c"vkQueuePresentKHR".as_ptr()).is_some());
            assert!(layer::vkGetInstanceProcAddr(std::ptr::null_mut(), c"vkCreateSwapchainKHR".as_ptr()).is_some());
            assert!(layer::vkGetInstanceProcAddr(std::ptr::null_mut(), c"vkCreateWaylandSurfaceKHR".as_ptr()).is_some());
            assert!(layer::vkGetDeviceProcAddr(std::ptr::null_mut(), c"vkQueuePresentKHR".as_ptr()).is_some());
        }
    }

    #[test]
    fn negotiate_interface_v2() {
        let mut info = vk_abi::VkNegotiateLayerInterface {
            sType: vk_abi::LAYER_NEGOTIATE_INTERFACE_STRUCT,
            pNext: std::ptr::null_mut(),
            loaderLayerInterfaceVersion: 2,
            pfnGetInstanceProcAddr: None,
            pfnGetDeviceProcAddr: None,
            pfnGetPhysicalDeviceProcAddr: std::ptr::null(),
        };
        let res = unsafe { layer::vkNegotiateLoaderLayerInterfaceVersion(&mut info) };
        assert_eq!(res, vk_abi::VK_SUCCESS);
        assert!(info.pfnGetInstanceProcAddr.is_some());
        assert!(info.pfnGetDeviceProcAddr.is_some());
    }
}
