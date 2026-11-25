//! Drawing the overlay inside a D3D12 game (native Windows and Proton/VKD3D-Proton).
//!
//! DXGI swap chains are shared between D3D11 and D3D12, so `d3d11.rs`'s
//! `IDXGISwapChain::Present` / `Present1` / `ResizeBuffers` / `ResizeBuffers1`
//! hooks already intercept D3D12 frames and hand non-D3D11 swap chains here.
//!
//! Capture the Direct queue passed to DXGI at swapchain creation. DXGI private
//! interface data ties its reference to the lifetime of that specific chain.

use super::util::{self, Backend};
use super::{Frame, TexOp, shared};
use crate::overlay::wire::VERTEX_SIZE;
use std::collections::HashMap;
use std::ffi::c_void;
use std::mem::ManuallyDrop;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;
use windows::Win32::Foundation::{CloseHandle, HANDLE, HWND, RECT};
use windows::Win32::Graphics::Direct3D::{D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST, ID3DBlob};
use windows::Win32::Graphics::Direct3D12::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::Win32::Graphics::Dxgi::{CreateDXGIFactory1, IDXGIFactory2, IDXGISwapChain, IDXGISwapChain3};
use windows::Win32::System::Threading::{CreateEventW, INFINITE, WaitForSingleObject};
use windows::core::{HRESULT, Interface, s, w};

type Raw = *mut c_void;
const MAX_SRVS: usize = 256;
const QUEUE_KEY: windows::core::GUID = windows::core::GUID::from_u128(0xa72ab65f_51bb_460f_af69_86a63ffc7349);
static RENDERER: Mutex<Option<D3d12Renderer>> = Mutex::new(None);

macro_rules! creation_hook {
    ($hook:ident, $original:ident, $($arg:ident),+) => {
        static $original: AtomicUsize = AtomicUsize::new(0);
        unsafe extern "system" fn $hook(this: Raw, device: Raw, $($arg: Raw,)+ out: *mut Raw) -> HRESULT {
            // DXGI creation arguments between pDevice and ppSwapChain are all pointers or HWNDs.
            let real: unsafe extern "system" fn(Raw, Raw, $(creation_hook!(@type $arg),)+ *mut Raw) -> HRESULT =
                unsafe { std::mem::transmute($original.load(Ordering::Acquire)) };
            let hr = unsafe { real(this, device, $($arg,)+ out) };
            if hr.is_ok() && !out.is_null() {
                unsafe { associate(*out, device) };
            }
            hr
        }
    };
    (@type $arg:ident) => { Raw };
}
creation_hook!(create_chain, CREATE, desc);
creation_hook!(create_hwnd, HWND_CREATE, hwnd, desc, fullscreen, output);
creation_hook!(create_core, CORE_CREATE, window, desc, output);
creation_hook!(create_composition, COMPOSITION_CREATE, desc, output);

unsafe fn associate(chain: Raw, device: Raw) {
    let Some(sc) = (unsafe { IDXGISwapChain::from_raw_borrowed(&chain) }) else { return };
    let Some(dev) = (unsafe { windows::core::IUnknown::from_raw_borrowed(&device) }) else { return };
    if let Ok(q) = dev.cast::<ID3D12CommandQueue>()
        && unsafe { q.GetDesc() }.Type == D3D12_COMMAND_LIST_TYPE_DIRECT
    {
        let _ = unsafe { sc.SetPrivateDataInterface(&QUEUE_KEY, &q) };
    }
}

pub fn install_if_loaded() -> bool {
    static INSTALLED: Mutex<bool> = Mutex::new(false);
    let mut installed = INSTALLED.lock().unwrap_or_else(|e| e.into_inner());
    if *installed { return true; }
    unsafe {
        if windows::Win32::System::LibraryLoader::GetModuleHandleW(w!("dxgi.dll")).is_err() { return false; }
        let Ok(factory) = CreateDXGIFactory1::<IDXGIFactory2>() else { return false };
        let vt = *(factory.as_raw() as *mut *mut usize);
        *installed = util::patch_vtable(vt, 10, create_chain as *const () as usize, &CREATE)
            & util::patch_vtable(vt, 15, create_hwnd as *const () as usize, &HWND_CREATE)
            & util::patch_vtable(vt, 16, create_core as *const () as usize, &CORE_CREATE)
            & util::patch_vtable(vt, 24, create_composition as *const () as usize, &COMPOSITION_CREATE);
    }
    *installed
}

/// ResizeBuffers1 can assign a different present queue to each back buffer.
pub unsafe fn invalidate_queue(this: Raw) {
    if let Some(sc) = unsafe { IDXGISwapChain::from_raw_borrowed(&this) } {
        let _ = unsafe { sc.SetPrivateData(&QUEUE_KEY, 0, std::ptr::null()) };
    }
    if let Ok(mut guard) = RENDERER.lock()
        && guard.as_ref().is_some_and(|r| r.swapchain == this as usize)
    { *guard = None; }
}

/// Called from `d3d11.rs` when `ResizeBuffers` / `ResizeBuffers1` is invoked.
pub fn before_resize(this: *mut c_void) {
    let _ = std::panic::catch_unwind(|| {
        if let Ok(mut guard) = RENDERER.lock() {
            if let Some(r) = guard.as_mut().filter(|r| r.swapchain == this as usize) {
                unsafe { r.release_back_buffers() };
            }
        }
    });
}

/// Called from `d3d11.rs` when a DXGI swap chain is not D3D11.
pub fn on_present(this: *mut c_void) {
    let _ = std::panic::catch_unwind(|| unsafe { present_inner(this) });
}

unsafe fn present_inner(this: *mut c_void) {
    let active = util::active_backend();
    if active != Backend::None && active != Backend::D3d12 {
        return;
    }
    install_if_loaded();
    let Some(sc) = (unsafe { IDXGISwapChain::from_raw_borrowed(&this) }) else { return };
    let Ok(sc3) = sc.cast::<IDXGISwapChain3>() else { return };
    let Ok(mut guard) = RENDERER.lock() else { return };

    if let Some(r) = guard.as_ref() {
        if r.swapchain != this as usize {
            if r.last_present.elapsed().as_secs_f32() < 1.0 {
                return;
            }
            crate::log!("overlay: D3D12 following a new swap chain");
            *guard = None;
        }
    }

    if guard.is_none() {
        let Ok(device): windows::core::Result<ID3D12Device> = (unsafe { sc.GetDevice() }) else {
            return;
        };
        let mut raw = std::ptr::null_mut::<c_void>();
        let mut len = size_of::<Raw>() as u32;
        if unsafe { sc.GetPrivateData(&QUEUE_KEY, &mut len, (&raw mut raw).cast()) }.is_err() || raw.is_null() {
            return;
        }
        // GetPrivateData returns an owned IUnknown reference for interface data.
        let unknown = unsafe { windows::core::IUnknown::from_raw(raw) };
        let Ok(queue) = unknown.cast::<ID3D12CommandQueue>() else { return };
        match unsafe { D3d12Renderer::new(&sc3, this as usize, device, queue) } {
            Ok(r) => {
                if !util::claim_backend(Backend::D3d12) {
                    return;
                }
                crate::log!("overlay: drawing into the game's D3D12 swap chain ({}x{})", r.size.0, r.size.1);
                super::input::attach(r.hwnd);
                super::hooked(r.size);
                *guard = Some(r);
            }
            Err(e) => {
                crate::log!("overlay: D3D12 init failed: {e}");
                return;
            }
        }
    }

    let r = guard.as_mut().expect("just set");
    r.last_present = Instant::now();
    if let Ok(desc) = unsafe { sc.GetDesc() } {
        let size = (desc.BufferDesc.Width, desc.BufferDesc.Height);
        if size != r.size {
            unsafe { r.release_back_buffers() };
            r.size = size;
            super::resized(size);
        }
        if desc.OutputWindow != r.hwnd {
            r.hwnd = desc.OutputWindow;
            super::input::attach(r.hwnd);
        }
    }
    let (ops, frame) = shared::take_for_present();
    let has_draw = frame.as_ref().is_some_and(|f| !f.meshes.is_empty());
    if ops.is_empty() && !has_draw {
        return;
    }
    if let Err(e) = unsafe { r.render(&sc3, ops, frame.as_deref()) } {
        crate::log!("overlay: D3D12 draw failed: {e}");
    }
}

/// D3D12 HLSL with two static samplers (`s0` linear, `s1` nearest) selected by
/// root constant `use_linear`, avoiding a separate sampler descriptor heap.
const D3D12_SHADER: &str = r#"
cbuffer C : register(b0) { float2 screen; float linear_out; float use_linear; };
struct VI { float2 pos : POSITION; float2 uv : TEXCOORD0; float4 col : COLOR0; };
struct PI { float4 pos : SV_POSITION; float2 uv : TEXCOORD0; float4 col : COLOR0; };
PI vs(VI i) {
    PI o;
    o.pos = float4(2.0 * i.pos.x / screen.x - 1.0, 1.0 - 2.0 * i.pos.y / screen.y, 0.0, 1.0);
    o.uv = i.uv;
    o.col = i.col;
    return o;
}
Texture2D t0 : register(t0);
SamplerState s_lin : register(s0);
SamplerState s_near : register(s1);
float3 to_linear(float3 c) { return c <= 0.04045 ? c / 12.92 : pow((c + 0.055) / 1.055, 2.4); }
float4 ps(PI i) : SV_Target {
    float4 tex = use_linear > 0.5 ? t0.Sample(s_lin, i.uv) : t0.Sample(s_near, i.uv);
    float4 c = i.col * tex;
    if (linear_out > 0.5) c.rgb = to_linear(c.rgb);
    return c;
}
"#;

struct D3d12Tex {
    resource: ID3D12Resource,
    srv_slot: usize,
    size: [u32; 2],
    linear: bool,
}

struct FrameSlot {
    allocator: ID3D12CommandAllocator,
    back_buffer: Option<ID3D12Resource>,
    rtv: D3D12_CPU_DESCRIPTOR_HANDLE,
    vb: Option<(ID3D12Resource, *mut u8, usize)>,
    ib: Option<(ID3D12Resource, *mut u8, usize)>,
    /// Staging buffers and retired textures kept alive until `fence_value` completes.
    garbage: Vec<ID3D12Resource>,
    fence_value: u64,
}

struct D3d12Renderer {
    swapchain: usize,
    hwnd: HWND,
    device: ID3D12Device,
    queue: ID3D12CommandQueue,
    root_sig: ID3D12RootSignature,
    pso: ID3D12PipelineState,
    pso_format: DXGI_FORMAT,
    rtv_heap: ID3D12DescriptorHeap,
    rtv_stride: usize,
    srv_heap: ID3D12DescriptorHeap,
    srv_stride: usize,
    free_srvs: Vec<usize>,
    cmd_list: ID3D12GraphicsCommandList,
    fence: ID3D12Fence,
    fence_event: HANDLE,
    next_fence: u64,
    frames: Vec<FrameSlot>,
    textures: HashMap<u64, D3d12Tex>,
    /// Resources retired from `textures`, freed once `fence >= retire_fence`.
    retired_resources: Vec<(u64, ID3D12Resource)>,
    /// SRV heap slots retired from `textures`, reused only once all prior
    /// in-flight frames (`fence >= retire_fence`) have completed on the GPU.
    retired_srvs: Vec<(u64, usize)>,
    linear_out: bool,
    size: (u32, u32),
    last_present: Instant,
}

// Accessed only under RENDERER's mutex.
unsafe impl Send for D3d12Renderer {}

impl Drop for D3d12Renderer {
    fn drop(&mut self) {
        unsafe {
            self.wait_idle();
            if !self.fence_event.is_invalid() {
                let _ = CloseHandle(self.fence_event);
            }
        }
    }
}

fn concrete_rtv_format(f: DXGI_FORMAT) -> (DXGI_FORMAT, bool) {
    match f {
        DXGI_FORMAT_R8G8B8A8_TYPELESS => (DXGI_FORMAT_R8G8B8A8_UNORM, false),
        DXGI_FORMAT_B8G8R8A8_TYPELESS => (DXGI_FORMAT_B8G8R8A8_UNORM, false),
        DXGI_FORMAT_R10G10B10A2_TYPELESS => (DXGI_FORMAT_R10G10B10A2_UNORM, false),
        DXGI_FORMAT_R16G16B16A16_TYPELESS => (DXGI_FORMAT_R16G16B16A16_FLOAT, true),
        DXGI_FORMAT_R8G8B8A8_UNORM_SRGB | DXGI_FORMAT_B8G8R8A8_UNORM_SRGB | DXGI_FORMAT_R16G16B16A16_FLOAT => (f, true),
        _ => (f, false),
    }
}

fn transition_barrier(res: &ID3D12Resource, before: D3D12_RESOURCE_STATES, after: D3D12_RESOURCE_STATES) -> D3D12_RESOURCE_BARRIER {
    D3D12_RESOURCE_BARRIER {
        Type: D3D12_RESOURCE_BARRIER_TYPE_TRANSITION,
        Flags: D3D12_RESOURCE_BARRIER_FLAG_NONE,
        Anonymous: D3D12_RESOURCE_BARRIER_0 {
            Transition: ManuallyDrop::new(D3D12_RESOURCE_TRANSITION_BARRIER {
                pResource: ManuallyDrop::new(Some(res.clone())),
                Subresource: D3D12_RESOURCE_BARRIER_ALL_SUBRESOURCES,
                StateBefore: before,
                StateAfter: after,
            }),
        },
    }
}

/// Drop the ManuallyDrop-wrapped resource inside a transition barrier.
unsafe fn drop_barrier(mut b: D3D12_RESOURCE_BARRIER) {
    unsafe {
        let t: &mut D3D12_RESOURCE_TRANSITION_BARRIER = &mut b.Anonymous.Transition;
        ManuallyDrop::drop(&mut t.pResource);
        ManuallyDrop::drop(&mut b.Anonymous.Transition);
    }
}

impl D3d12Renderer {
    unsafe fn new(sc3: &IDXGISwapChain3, raw: usize, device: ID3D12Device, queue: ID3D12CommandQueue) -> windows::core::Result<Self> {
        unsafe {
            let desc = sc3.GetDesc()?;
            let buffer_count = desc.BufferCount.max(1) as usize;
            let (rtv_format, linear_out) = concrete_rtv_format(desc.BufferDesc.Format);

            let root_sig = Self::create_root_signature(&device)?;
            let pso = Self::create_pso(&device, &root_sig, rtv_format)?;

            let rtv_heap: ID3D12DescriptorHeap = device.CreateDescriptorHeap(&D3D12_DESCRIPTOR_HEAP_DESC {
                Type: D3D12_DESCRIPTOR_HEAP_TYPE_RTV,
                NumDescriptors: buffer_count as u32,
                Flags: D3D12_DESCRIPTOR_HEAP_FLAG_NONE,
                NodeMask: 0,
            })?;
            let rtv_stride = device.GetDescriptorHandleIncrementSize(D3D12_DESCRIPTOR_HEAP_TYPE_RTV) as usize;

            let srv_heap: ID3D12DescriptorHeap = device.CreateDescriptorHeap(&D3D12_DESCRIPTOR_HEAP_DESC {
                Type: D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV,
                NumDescriptors: MAX_SRVS as u32,
                Flags: D3D12_DESCRIPTOR_HEAP_FLAG_SHADER_VISIBLE,
                NodeMask: 0,
            })?;
            let srv_stride = device.GetDescriptorHandleIncrementSize(D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV) as usize;

            let rtv_start = rtv_heap.GetCPUDescriptorHandleForHeapStart();
            let mut frames = Vec::with_capacity(buffer_count);
            for i in 0..buffer_count {
                let allocator: ID3D12CommandAllocator = device.CreateCommandAllocator(D3D12_COMMAND_LIST_TYPE_DIRECT)?;
                let rtv = D3D12_CPU_DESCRIPTOR_HANDLE { ptr: rtv_start.ptr + i * rtv_stride };
                frames.push(FrameSlot { allocator, back_buffer: None, rtv, vb: None, ib: None, garbage: Vec::new(), fence_value: 0 });
            }

            let cmd_list: ID3D12GraphicsCommandList =
                device.CreateCommandList(0, D3D12_COMMAND_LIST_TYPE_DIRECT, &frames[0].allocator, Some(&pso))?;
            cmd_list.Close()?;

            let fence: ID3D12Fence = device.CreateFence(0, D3D12_FENCE_FLAG_NONE)?;
            let fence_event = CreateEventW(None, false, false, None)?;

            let mut r = Self {
                swapchain: raw,
                hwnd: desc.OutputWindow,
                device,
                queue,
                root_sig,
                pso,
                pso_format: rtv_format,
                rtv_heap,
                rtv_stride,
                srv_heap,
                srv_stride,
                free_srvs: (0..MAX_SRVS).rev().collect(),
                cmd_list,
                fence,
                fence_event,
                next_fence: 1,
                frames,
                textures: HashMap::new(),
                retired_resources: Vec::new(),
                retired_srvs: Vec::new(),
                linear_out,
                size: (desc.BufferDesc.Width, desc.BufferDesc.Height),
                last_present: Instant::now(),
            };
            r.ensure_back_buffers(sc3)?;
            Ok(r)
        }
    }

    unsafe fn create_root_signature(device: &ID3D12Device) -> windows::core::Result<ID3D12RootSignature> {
        use windows::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};
        type SerializeFn = unsafe extern "system" fn(
            *const D3D12_ROOT_SIGNATURE_DESC,
            D3D_ROOT_SIGNATURE_VERSION,
            *mut *mut c_void,
            *mut *mut c_void,
        ) -> HRESULT;
        unsafe {
            let module = GetModuleHandleW(w!("d3d12.dll"))?;
            let Some(p) = GetProcAddress(module, s!("D3D12SerializeRootSignature")) else {
                return Err(windows::core::Error::from_thread());
            };
            let serialize: SerializeFn = std::mem::transmute(p);

            let range = D3D12_DESCRIPTOR_RANGE {
                RangeType: D3D12_DESCRIPTOR_RANGE_TYPE_SRV,
                NumDescriptors: 1,
                BaseShaderRegister: 0,
                RegisterSpace: 0,
                OffsetInDescriptorsFromTableStart: D3D12_DESCRIPTOR_RANGE_OFFSET_APPEND,
            };
            let params = [
                D3D12_ROOT_PARAMETER {
                    ParameterType: D3D12_ROOT_PARAMETER_TYPE_32BIT_CONSTANTS,
                    Anonymous: D3D12_ROOT_PARAMETER_0 {
                        Constants: D3D12_ROOT_CONSTANTS { ShaderRegister: 0, RegisterSpace: 0, Num32BitValues: 4 },
                    },
                    ShaderVisibility: D3D12_SHADER_VISIBILITY_ALL,
                },
                D3D12_ROOT_PARAMETER {
                    ParameterType: D3D12_ROOT_PARAMETER_TYPE_DESCRIPTOR_TABLE,
                    Anonymous: D3D12_ROOT_PARAMETER_0 {
                        DescriptorTable: D3D12_ROOT_DESCRIPTOR_TABLE { NumDescriptorRanges: 1, pDescriptorRanges: &range },
                    },
                    ShaderVisibility: D3D12_SHADER_VISIBILITY_PIXEL,
                },
            ];
            let mk_sampler = |filter, reg| D3D12_STATIC_SAMPLER_DESC {
                Filter: filter,
                AddressU: D3D12_TEXTURE_ADDRESS_MODE_CLAMP,
                AddressV: D3D12_TEXTURE_ADDRESS_MODE_CLAMP,
                AddressW: D3D12_TEXTURE_ADDRESS_MODE_CLAMP,
                MipLODBias: 0.0,
                MaxAnisotropy: 0,
                ComparisonFunc: D3D12_COMPARISON_FUNC_ALWAYS,
                BorderColor: D3D12_STATIC_BORDER_COLOR_TRANSPARENT_BLACK,
                MinLOD: 0.0,
                MaxLOD: f32::MAX,
                ShaderRegister: reg,
                RegisterSpace: 0,
                ShaderVisibility: D3D12_SHADER_VISIBILITY_PIXEL,
            };
            let samplers = [
                mk_sampler(D3D12_FILTER_MIN_MAG_MIP_LINEAR, 0),
                mk_sampler(D3D12_FILTER_MIN_MAG_MIP_POINT, 1),
            ];
            let desc = D3D12_ROOT_SIGNATURE_DESC {
                NumParameters: params.len() as u32,
                pParameters: params.as_ptr(),
                NumStaticSamplers: samplers.len() as u32,
                pStaticSamplers: samplers.as_ptr(),
                Flags: D3D12_ROOT_SIGNATURE_FLAG_ALLOW_INPUT_ASSEMBLER_INPUT_LAYOUT,
            };
            let (mut blob, mut err) = (std::ptr::null_mut(), std::ptr::null_mut());
            let hr = serialize(&desc, D3D_ROOT_SIGNATURE_VERSION_1, &mut blob, &mut err);
            if !err.is_null() {
                let _ = ID3DBlob::from_raw(err);
            }
            hr.ok()?;
            let blob = ID3DBlob::from_raw(blob);
            let bytes = std::slice::from_raw_parts(blob.GetBufferPointer() as *const u8, blob.GetBufferSize());
            device.CreateRootSignature(0, bytes)
        }
    }

    unsafe fn create_pso(device: &ID3D12Device, root_sig: &ID3D12RootSignature, rtv_format: DXGI_FORMAT) -> windows::core::Result<ID3D12PipelineState> {
        unsafe {
            let vs = util::compile_hlsl(D3D12_SHADER, s!("vs"), s!("vs_5_0"))?;
            let ps = util::compile_hlsl(D3D12_SHADER, s!("ps"), s!("ps_5_0"))?;
            let elements = [
                D3D12_INPUT_ELEMENT_DESC {
                    SemanticName: s!("POSITION"),
                    SemanticIndex: 0,
                    Format: DXGI_FORMAT_R32G32_FLOAT,
                    InputSlot: 0,
                    AlignedByteOffset: 0,
                    InputSlotClass: D3D12_INPUT_CLASSIFICATION_PER_VERTEX_DATA,
                    InstanceDataStepRate: 0,
                },
                D3D12_INPUT_ELEMENT_DESC {
                    SemanticName: s!("TEXCOORD"),
                    SemanticIndex: 0,
                    Format: DXGI_FORMAT_R32G32_FLOAT,
                    InputSlot: 0,
                    AlignedByteOffset: 8,
                    InputSlotClass: D3D12_INPUT_CLASSIFICATION_PER_VERTEX_DATA,
                    InstanceDataStepRate: 0,
                },
                D3D12_INPUT_ELEMENT_DESC {
                    SemanticName: s!("COLOR"),
                    SemanticIndex: 0,
                    Format: DXGI_FORMAT_R8G8B8A8_UNORM,
                    InputSlot: 0,
                    AlignedByteOffset: 16,
                    InputSlotClass: D3D12_INPUT_CLASSIFICATION_PER_VERTEX_DATA,
                    InstanceDataStepRate: 0,
                },
            ];
            let mut blend = D3D12_BLEND_DESC::default();
            blend.RenderTarget[0] = D3D12_RENDER_TARGET_BLEND_DESC {
                BlendEnable: true.into(),
                LogicOpEnable: false.into(),
                SrcBlend: D3D12_BLEND_ONE,
                DestBlend: D3D12_BLEND_INV_SRC_ALPHA,
                BlendOp: D3D12_BLEND_OP_ADD,
                SrcBlendAlpha: D3D12_BLEND_INV_DEST_ALPHA,
                DestBlendAlpha: D3D12_BLEND_ONE,
                BlendOpAlpha: D3D12_BLEND_OP_ADD,
                LogicOp: D3D12_LOGIC_OP_NOOP,
                RenderTargetWriteMask: D3D12_COLOR_WRITE_ENABLE_ALL.0 as u8,
            };
            let raster = D3D12_RASTERIZER_DESC {
                FillMode: D3D12_FILL_MODE_SOLID,
                CullMode: D3D12_CULL_MODE_NONE,
                FrontCounterClockwise: false.into(),
                DepthBias: 0,
                DepthBiasClamp: 0.0,
                SlopeScaledDepthBias: 0.0,
                DepthClipEnable: true.into(),
                MultisampleEnable: false.into(),
                AntialiasedLineEnable: false.into(),
                ForcedSampleCount: 0,
                ConservativeRaster: D3D12_CONSERVATIVE_RASTERIZATION_MODE_OFF,
            };
            let depth = D3D12_DEPTH_STENCIL_DESC {
                DepthEnable: false.into(),
                DepthWriteMask: D3D12_DEPTH_WRITE_MASK_ZERO,
                DepthFunc: D3D12_COMPARISON_FUNC_ALWAYS,
                StencilEnable: false.into(),
                ..Default::default()
            };
            let mut rtv_formats = [DXGI_FORMAT_UNKNOWN; 8];
            rtv_formats[0] = rtv_format;
            let mut desc = D3D12_GRAPHICS_PIPELINE_STATE_DESC {
                pRootSignature: ManuallyDrop::new(Some(root_sig.clone())),
                VS: D3D12_SHADER_BYTECODE { pShaderBytecode: vs.as_ptr().cast(), BytecodeLength: vs.len() },
                PS: D3D12_SHADER_BYTECODE { pShaderBytecode: ps.as_ptr().cast(), BytecodeLength: ps.len() },
                BlendState: blend,
                SampleMask: u32::MAX,
                RasterizerState: raster,
                DepthStencilState: depth,
                InputLayout: D3D12_INPUT_LAYOUT_DESC { pInputElementDescs: elements.as_ptr(), NumElements: elements.len() as u32 },
                PrimitiveTopologyType: D3D12_PRIMITIVE_TOPOLOGY_TYPE_TRIANGLE,
                NumRenderTargets: 1,
                RTVFormats: rtv_formats,
                SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
                ..Default::default()
            };
            let res = device.CreateGraphicsPipelineState(&desc);
            ManuallyDrop::drop(&mut desc.pRootSignature);
            res
        }
    }

    unsafe fn wait_fence(&self, value: u64) {
        if value == 0 {
            return;
        }
        unsafe {
            if self.fence.GetCompletedValue() < value {
                if self.fence.SetEventOnCompletion(value, self.fence_event).is_ok() {
                    WaitForSingleObject(self.fence_event, INFINITE);
                } else {
                    while self.fence.GetCompletedValue() < value {
                        std::thread::yield_now();
                    }
                }
            }
        }
    }

    unsafe fn wait_idle(&mut self) {
        unsafe {
            let val = self.next_fence;
            self.next_fence += 1;
            if self.queue.Signal(&self.fence, val).is_ok() {
                self.wait_fence(val);
            }
        }
    }

    unsafe fn release_back_buffers(&mut self) {
        unsafe {
            self.wait_idle();
            for f in &mut self.frames {
                f.back_buffer = None;
                f.garbage.clear();
            }
        }
    }

    unsafe fn ensure_back_buffers(&mut self, sc3: &IDXGISwapChain3) -> windows::core::Result<()> {
        if self.frames.iter().all(|f| f.back_buffer.is_some()) {
            return Ok(());
        }
        unsafe {
            let desc = sc3.GetDesc()?;
            let count = desc.BufferCount.max(1) as usize;
            let (rtv_format, linear_out) = concrete_rtv_format(desc.BufferDesc.Format);
            if rtv_format != self.pso_format {
                self.pso = Self::create_pso(&self.device, &self.root_sig, rtv_format)?;
                self.pso_format = rtv_format;
            }
            self.linear_out = linear_out;
            self.size = (desc.BufferDesc.Width, desc.BufferDesc.Height);
            if count > self.frames.len() {
                self.rtv_heap = self.device.CreateDescriptorHeap(&D3D12_DESCRIPTOR_HEAP_DESC {
                    Type: D3D12_DESCRIPTOR_HEAP_TYPE_RTV,
                    NumDescriptors: count as u32,
                    Flags: D3D12_DESCRIPTOR_HEAP_FLAG_NONE,
                    NodeMask: 0,
                })?;
                let start = self.rtv_heap.GetCPUDescriptorHandleForHeapStart();
                for (i, f) in self.frames.iter_mut().enumerate() {
                    f.rtv = D3D12_CPU_DESCRIPTOR_HANDLE { ptr: start.ptr + i * self.rtv_stride };
                }
                for i in self.frames.len()..count {
                    let allocator: ID3D12CommandAllocator = self.device.CreateCommandAllocator(D3D12_COMMAND_LIST_TYPE_DIRECT)?;
                    let rtv = D3D12_CPU_DESCRIPTOR_HANDLE { ptr: start.ptr + i * self.rtv_stride };
                    self.frames.push(FrameSlot { allocator, back_buffer: None, rtv, vb: None, ib: None, garbage: Vec::new(), fence_value: 0 });
                }
            }
            let rtv_desc = D3D12_RENDER_TARGET_VIEW_DESC {
                Format: rtv_format,
                ViewDimension: D3D12_RTV_DIMENSION_TEXTURE2D,
                ..Default::default()
            };
            for (i, f) in self.frames.iter_mut().take(count).enumerate() {
                let bb: ID3D12Resource = sc3.GetBuffer(i as u32)?;
                self.device.CreateRenderTargetView(&bb, Some(&rtv_desc), f.rtv);
                f.back_buffer = Some(bb);
            }
            Ok(())
        }
    }

    unsafe fn create_upload_buffer(&self, bytes: usize) -> windows::core::Result<(ID3D12Resource, *mut u8)> {
        unsafe {
            let heap = D3D12_HEAP_PROPERTIES { Type: D3D12_HEAP_TYPE_UPLOAD, ..Default::default() };
            let desc = D3D12_RESOURCE_DESC {
                Dimension: D3D12_RESOURCE_DIMENSION_BUFFER,
                Alignment: 0,
                Width: bytes as u64,
                Height: 1,
                DepthOrArraySize: 1,
                MipLevels: 1,
                Format: DXGI_FORMAT_UNKNOWN,
                SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
                Layout: D3D12_TEXTURE_LAYOUT_ROW_MAJOR,
                Flags: D3D12_RESOURCE_FLAG_NONE,
            };
            let mut res: Option<ID3D12Resource> = None;
            self.device.CreateCommittedResource(&heap, D3D12_HEAP_FLAG_NONE, &desc, D3D12_RESOURCE_STATE_GENERIC_READ, None, &mut res)?;
            let res = res.ok_or_else(|| windows::core::Error::from_hresult(windows::Win32::Foundation::E_FAIL))?;
            let mut ptr = std::ptr::null_mut();
            let empty_read = D3D12_RANGE { Begin: 0, End: 0 };
            res.Map(0, Some(&empty_read), Some(&mut ptr))?;
            Ok((res, ptr as *mut u8))
        }
    }

    unsafe fn ensure_upload_slot(
        device_ref: &Self,
        slot: &mut Option<(ID3D12Resource, *mut u8, usize)>,
        garbage: &mut Vec<ID3D12Resource>,
        bytes: usize,
    ) -> windows::core::Result<(u64, *mut u8)> {
        if let Some((res, ptr, _cap)) = slot.as_ref().filter(|(_, _, cap)| *cap >= bytes) {
            return Ok((unsafe { res.GetGPUVirtualAddress() }, *ptr));
        }
        let Some(cap) = util::checked_grow_capacity(bytes, 64 * 1024) else {
            return Err(windows::core::Error::from(windows::Win32::Foundation::E_INVALIDARG));
        };
        let (res, ptr) = unsafe { device_ref.create_upload_buffer(cap)? };
        if let Some((old, _, _)) = slot.take() {
            garbage.push(old);
        }
        let gpu = unsafe { res.GetGPUVirtualAddress() };
        *slot = Some((res, ptr, cap));
        Ok((gpu, ptr))
    }

    unsafe fn reclaim_retired(&mut self) {
        let completed = unsafe { self.fence.GetCompletedValue() };
        self.retired_resources.retain(|(f, _)| *f > completed);
        let mut i = 0;
        while i < self.retired_srvs.len() {
            if self.retired_srvs[i].0 <= completed {
                let (_, slot) = self.retired_srvs.swap_remove(i);
                self.free_srvs.push(slot);
            } else {
                i += 1;
            }
        }
    }

    fn retire_fence(&self) -> u64 {
        self.frames.iter().map(|f| f.fence_value).max().unwrap_or(0).max(self.next_fence)
    }

    unsafe fn apply_tex_op(&mut self, op: TexOp, garbage: &mut Vec<ID3D12Resource>) {
        match op {
            TexOp::Free(id) => {
                if let Some(tex) = self.textures.remove(&id) {
                    let rf = self.retire_fence();
                    self.retired_resources.push((rf, tex.resource));
                    self.retired_srvs.push((rf, tex.srv_slot));
                }
            }
            TexOp::Set(t) => unsafe {
                let [w, h] = t.size;
                let Some((row_bytes, _)) = util::validate_tex_rgba(t.size, t.rgba.len()) else {
                    return;
                };
                let align = D3D12_TEXTURE_DATA_PITCH_ALIGNMENT as usize;
                let Some(row_pitch) = row_bytes.checked_add(align - 1).map(|v| v & !(align - 1)) else {
                    return;
                };
                if u32::try_from(row_pitch).is_err() {
                    return;
                }
                let Some(upload_size) = row_pitch.checked_mul(h as usize) else {
                    return;
                };
                if let Some([px, py]) = t.pos {
                    let Some(tex) = self.textures.get(&t.id) else { return };
                    if !util::validate_tex_subrect([px, py], t.size, tex.size) {
                        return;
                    }
                }
                let Ok((staging, ptr)) = self.create_upload_buffer(upload_size) else { return };
                for row in 0..h as usize {
                    std::ptr::copy_nonoverlapping(t.rgba.as_ptr().add(row * row_bytes), ptr.add(row * row_pitch), row_bytes);
                }
                staging.Unmap(0, None);

                let (dst_res, dst_x, dst_y, was_srv) = match t.pos {
                    Some([px, py]) => {
                        let Some(tex) = self.textures.get_mut(&t.id) else { return };
                        tex.linear = t.linear;
                        (tex.resource.clone(), px, py, true)
                    }
                    None => {
                        // Allocate a fresh SRV slot so we never overwrite a descriptor
                        // that another in-flight command list is still reading.
                        let Some(slot) = self.free_srvs.pop() else { return };
                        let heap = D3D12_HEAP_PROPERTIES { Type: D3D12_HEAP_TYPE_DEFAULT, ..Default::default() };
                        let desc = D3D12_RESOURCE_DESC {
                            Dimension: D3D12_RESOURCE_DIMENSION_TEXTURE2D,
                            Alignment: 0,
                            Width: w as u64,
                            Height: h,
                            DepthOrArraySize: 1,
                            MipLevels: 1,
                            Format: DXGI_FORMAT_R8G8B8A8_UNORM,
                            SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
                            Layout: D3D12_TEXTURE_LAYOUT_UNKNOWN,
                            Flags: D3D12_RESOURCE_FLAG_NONE,
                        };
                        let mut res: Option<ID3D12Resource> = None;
                        if self.device.CreateCommittedResource(&heap, D3D12_HEAP_FLAG_NONE, &desc, D3D12_RESOURCE_STATE_COPY_DEST, None, &mut res).is_err() {
                            self.free_srvs.push(slot);
                            return;
                        }
                        let Some(res) = res else {
                            self.free_srvs.push(slot);
                            return;
                        };
                        let srv_desc = D3D12_SHADER_RESOURCE_VIEW_DESC {
                            Format: DXGI_FORMAT_R8G8B8A8_UNORM,
                            ViewDimension: D3D12_SRV_DIMENSION_TEXTURE2D,
                            Shader4ComponentMapping: D3D12_DEFAULT_SHADER_4_COMPONENT_MAPPING,
                            Anonymous: D3D12_SHADER_RESOURCE_VIEW_DESC_0 {
                                Texture2D: D3D12_TEX2D_SRV { MostDetailedMip: 0, MipLevels: 1, PlaneSlice: 0, ResourceMinLODClamp: 0.0 },
                            },
                        };
                        let cpu = D3D12_CPU_DESCRIPTOR_HANDLE { ptr: self.srv_heap.GetCPUDescriptorHandleForHeapStart().ptr + slot * self.srv_stride };
                        self.device.CreateShaderResourceView(&res, Some(&srv_desc), cpu);
                        if let Some(old) = self.textures.insert(t.id, D3d12Tex { resource: res.clone(), srv_slot: slot, size: t.size, linear: t.linear }) {
                            let rf = self.retire_fence();
                            self.retired_resources.push((rf, old.resource));
                            self.retired_srvs.push((rf, old.srv_slot));
                        }
                        (res, 0, 0, false)
                    }
                };

                if was_srv {
                    let b = transition_barrier(&dst_res, D3D12_RESOURCE_STATE_PIXEL_SHADER_RESOURCE, D3D12_RESOURCE_STATE_COPY_DEST);
                    self.cmd_list.ResourceBarrier(std::slice::from_ref(&b));
                    drop_barrier(b);
                }

                let mut dst_loc = D3D12_TEXTURE_COPY_LOCATION {
                    pResource: ManuallyDrop::new(Some(dst_res.clone())),
                    Type: D3D12_TEXTURE_COPY_TYPE_SUBRESOURCE_INDEX,
                    Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 { SubresourceIndex: 0 },
                };
                let mut src_loc = D3D12_TEXTURE_COPY_LOCATION {
                    pResource: ManuallyDrop::new(Some(staging.clone())),
                    Type: D3D12_TEXTURE_COPY_TYPE_PLACED_FOOTPRINT,
                    Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 {
                        PlacedFootprint: D3D12_PLACED_SUBRESOURCE_FOOTPRINT {
                            Offset: 0,
                            Footprint: D3D12_SUBRESOURCE_FOOTPRINT {
                                Format: DXGI_FORMAT_R8G8B8A8_UNORM,
                                Width: w,
                                Height: h,
                                Depth: 1,
                                RowPitch: row_pitch as u32,
                            },
                        },
                    },
                };
                self.cmd_list.CopyTextureRegion(&dst_loc, dst_x, dst_y, 0, &src_loc, None);
                ManuallyDrop::drop(&mut dst_loc.pResource);
                ManuallyDrop::drop(&mut src_loc.pResource);

                let b = transition_barrier(&dst_res, D3D12_RESOURCE_STATE_COPY_DEST, D3D12_RESOURCE_STATE_PIXEL_SHADER_RESOURCE);
                self.cmd_list.ResourceBarrier(std::slice::from_ref(&b));
                drop_barrier(b);

                garbage.push(staging);
            },
        }
    }

    unsafe fn render(&mut self, sc3: &IDXGISwapChain3, ops: Vec<TexOp>, frame: Option<&Frame>) -> windows::core::Result<()> {
        unsafe {
            self.ensure_back_buffers(sc3)?;
            let idx = (sc3.GetCurrentBackBufferIndex() as usize) % self.frames.len();
            let fence_val = self.frames[idx].fence_value;
            self.wait_fence(fence_val);
            self.reclaim_retired();
            self.frames[idx].garbage.clear();
            self.frames[idx].allocator.Reset()?;
            self.cmd_list.Reset(&self.frames[idx].allocator, Some(&self.pso))?;

            let mut garbage = std::mem::take(&mut self.frames[idx].garbage);
            for op in ops {
                self.apply_tex_op(op, &mut garbage);
            }

            if let Some(frame) = frame.filter(|f| !f.meshes.is_empty())
                && let Some((vbytes, ibytes)) = util::validate_mesh_totals(&frame.meshes, VERTEX_SIZE)
                && vbytes > 0
                && ibytes > 0
            {
                let mut vb_slot = self.frames[idx].vb.take();
                let mut ib_slot = self.frames[idx].ib.take();
                let vb_res = Self::ensure_upload_slot(self, &mut vb_slot, &mut garbage, vbytes);
                let ib_res = Self::ensure_upload_slot(self, &mut ib_slot, &mut garbage, ibytes);
                self.frames[idx].vb = vb_slot;
                self.frames[idx].ib = ib_slot;
                // If upload buffer growth failed after texture uploads were recorded,
                // skip drawing meshes instead of abandoning the command list so
                // recorded texture barriers still execute cleanly.
                if let (Ok((vb_gpu, vb_ptr)), Ok((ib_gpu, ib_ptr))) = (vb_res, ib_res)
                    && let Some(bb) = self.frames[idx].back_buffer.clone()
                {
                    let mut v_at = vb_ptr;
                    let mut i_at = ib_ptr;
                    for m in &frame.meshes {
                        std::ptr::copy_nonoverlapping(m.vertices.as_ptr(), v_at, m.vertices.len());
                        v_at = v_at.add(m.vertices.len());
                        let ib_len = m.indices.len() * 4;
                        std::ptr::copy_nonoverlapping(m.indices.as_ptr() as *const u8, i_at, ib_len);
                        i_at = i_at.add(ib_len);
                    }
                    let rtv = self.frames[idx].rtv;

                    let b_in = transition_barrier(&bb, D3D12_RESOURCE_STATE_PRESENT, D3D12_RESOURCE_STATE_RENDER_TARGET);
                    self.cmd_list.ResourceBarrier(std::slice::from_ref(&b_in));
                    drop_barrier(b_in);

                    let ppp = frame.pixels_per_point.max(0.1);
                    let (w, h) = (self.size.0 as f32, self.size.1 as f32);
                    self.cmd_list.OMSetRenderTargets(1, Some(&rtv), false, None);
                    self.cmd_list.RSSetViewports(&[D3D12_VIEWPORT {
                        TopLeftX: 0.0,
                        TopLeftY: 0.0,
                        Width: w,
                        Height: h,
                        MinDepth: 0.0,
                        MaxDepth: 1.0,
                    }]);
                    self.cmd_list.SetPipelineState(&self.pso);
                    self.cmd_list.SetGraphicsRootSignature(&self.root_sig);
                    self.cmd_list.SetDescriptorHeaps(&[Some(self.srv_heap.clone())]);
                    self.cmd_list.IASetPrimitiveTopology(D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
                    let vbv = D3D12_VERTEX_BUFFER_VIEW {
                        BufferLocation: vb_gpu,
                        SizeInBytes: vbytes as u32,
                        StrideInBytes: VERTEX_SIZE as u32,
                    };
                    let ibv = D3D12_INDEX_BUFFER_VIEW {
                        BufferLocation: ib_gpu,
                        SizeInBytes: ibytes as u32,
                        Format: DXGI_FORMAT_R32_UINT,
                    };
                    self.cmd_list.IASetVertexBuffers(0, Some(&[vbv]));
                    self.cmd_list.IASetIndexBuffer(Some(&ibv));

                    let srv_gpu_start = self.srv_heap.GetGPUDescriptorHandleForHeapStart().ptr;
                    let (mut first_index, mut base_vertex) = (0u32, 0i32);
                    for m in &frame.meshes {
                        let count = m.indices.len() as u32;
                        let verts = (m.vertices.len() / VERTEX_SIZE) as i32;
                        if let Some([left, top, right, bottom]) = util::clip_rect_pixels(m.clip, ppp, self.size.0, self.size.1) {
                            if let Some(tex) = self.textures.get(&m.texture).filter(|_| count > 0) {
                                let constants = [
                                    w / ppp,
                                    h / ppp,
                                    if self.linear_out { 1.0 } else { 0.0 },
                                    if tex.linear { 1.0 } else { 0.0 },
                                ];
                                self.cmd_list.RSSetScissorRects(&[RECT { left, top, right, bottom }]);
                                self.cmd_list.SetGraphicsRoot32BitConstants(0, 4, constants.as_ptr().cast(), 0);
                                let gpu_handle = D3D12_GPU_DESCRIPTOR_HANDLE { ptr: srv_gpu_start + (tex.srv_slot * self.srv_stride) as u64 };
                                self.cmd_list.SetGraphicsRootDescriptorTable(1, gpu_handle);
                                self.cmd_list.DrawIndexedInstanced(count, 1, first_index, base_vertex, 0);
                            }
                        }
                        first_index += count;
                        base_vertex += verts;
                    }

                    let b_out = transition_barrier(&bb, D3D12_RESOURCE_STATE_RENDER_TARGET, D3D12_RESOURCE_STATE_PRESENT);
                    self.cmd_list.ResourceBarrier(std::slice::from_ref(&b_out));
                    drop_barrier(b_out);
                }
            }

            self.frames[idx].garbage = garbage;
            self.cmd_list.Close()?;

            let list: ID3D12CommandList = self.cmd_list.cast()?;
            self.queue.ExecuteCommandLists(&[Some(list)]);

            let sig = self.next_fence;
            self.next_fence += 1;
            self.queue.Signal(&self.fence, sig)?;
            self.frames[idx].fence_value = sig;
            Ok(())
        }
    }
}
