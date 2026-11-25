use std::mem::ManuallyDrop;
use std::slice;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Instant;

use crossbeam_channel::{Receiver, Sender};
use windows::core::{w, Interface, BOOL, PCWSTR};
use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Direct3D::{D3D_DRIVER_TYPE_HARDWARE, D3D_DRIVER_TYPE_WARP};
use windows::Win32::Graphics::Direct3D11::{
    D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext, ID3D11RenderTargetView, ID3D11Texture2D,
    ID3D11VideoContext, ID3D11VideoDevice, ID3D11VideoProcessor, ID3D11VideoProcessorEnumerator,
    ID3D11VideoProcessorInputView, ID3D11VideoProcessorOutputView, D3D11_BIND_RENDER_TARGET,
    D3D11_CPU_ACCESS_WRITE, D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_CREATE_DEVICE_VIDEO_SUPPORT,
    D3D11_MAPPED_SUBRESOURCE, D3D11_MAP_WRITE, D3D11_SDK_VERSION, D3D11_TEXTURE2D_DESC,
    D3D11_USAGE_DEFAULT, D3D11_USAGE_STAGING, D3D11_VIDEO_FRAME_FORMAT_PROGRESSIVE,
    D3D11_VIDEO_PROCESSOR_CONTENT_DESC, D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC,
    D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC_0, D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC,
    D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC_0, D3D11_VIDEO_PROCESSOR_STREAM,
    D3D11_VIDEO_USAGE_PLAYBACK_NORMAL, D3D11_VPIV_DIMENSION_TEXTURE2D,
    D3D11_VPOV_DIMENSION_TEXTURE2D,
};
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_ALPHA_MODE_IGNORE, DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_FORMAT_NV12, DXGI_FORMAT_UNKNOWN,
    DXGI_RATIONAL, DXGI_SAMPLE_DESC,
};
use windows::Win32::Graphics::Dxgi::{
    IDXGIDevice, IDXGIFactory2, IDXGIFactory5, IDXGISwapChain1, DXGI_FEATURE_PRESENT_ALLOW_TEARING,
    DXGI_PRESENT, DXGI_PRESENT_ALLOW_TEARING, DXGI_SCALING_STRETCH, DXGI_SWAP_CHAIN_DESC1,
    DXGI_SWAP_CHAIN_FLAG, DXGI_SWAP_CHAIN_FLAG_ALLOW_TEARING, DXGI_SWAP_EFFECT_FLIP_DISCARD,
    DXGI_USAGE_RENDER_TARGET_OUTPUT,
};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, DrawTextW, EndPaint, FillRect, GetMonitorInfoW, GetStockObject, InvalidateRect,
    MonitorFromWindow, SetBkMode, SetTextColor, BLACK_BRUSH, DT_CENTER, DT_WORDBREAK, HBRUSH,
    MONITORINFO, MONITOR_DEFAULTTONEAREST, PAINTSTRUCT, TRANSPARENT,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::{
    SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    ReleaseCapture, SetCapture, VK_F11, VK_RETURN,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetClientRect, GetMessageW,
    GetWindowLongW, GetWindowRect, LoadCursorW, PostMessageW, PostQuitMessage, RegisterClassW,
    SetCursor, SetWindowLongW, SetWindowPos, SetWindowTextW, ShowWindow, TranslateMessage,
    CW_USEDEFAULT, GWL_STYLE, HTCLIENT, HWND_TOP, IDC_ARROW, MSG, SWP_FRAMECHANGED,
    SWP_NOOWNERZORDER, SWP_NOZORDER, SW_HIDE, SW_SHOW, WINDOW_STYLE, WM_APP, WM_CLOSE, WM_DESTROY,
    WM_KEYDOWN, WM_KILLFOCUS, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MBUTTONDOWN, WM_MBUTTONUP,
    WM_MOUSEHWHEEL, WM_MOUSEMOVE, WM_MOUSEWHEEL, WM_PAINT, WM_RBUTTONDOWN, WM_RBUTTONUP,
    WM_SETCURSOR, WM_SETFOCUS, WM_SIZE, WM_SYSKEYDOWN, WM_XBUTTONDOWN, WM_XBUTTONUP, WNDCLASSW,
    WS_OVERLAPPEDWINDOW, WS_POPUP, WS_VISIBLE,
};

use crate::display::DecodedFrame;
use crate::error::{Key2PcError, Result};
use crate::protocol::{MouseAction, MouseEvent};

const WM_APP_NEW_FRAME: u32 = WM_APP + 20;
const WM_APP_COMMAND: u32 = WM_APP + 21;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ViewerCommand {
    Show,
    Hide,
    ToggleVisible,
    ToggleFullscreen,
    SetFullscreen(bool),
    Shutdown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ViewerEvent {
    Mouse(MouseEvent),
    VisibilityChanged(bool),
    FocusChanged(bool),
    FullscreenChanged(bool),
}

pub struct ViewerOptions {
    pub initial_width: u32,
    pub initial_height: u32,
    pub start_visible: bool,
    pub start_fullscreen: bool,
    pub capture_mouse: bool,
}

pub struct DisplayViewer {
    hwnd_raw: isize,
    visible: Arc<AtomicBool>,
    fullscreen: Arc<AtomicBool>,
    latest_frame: Arc<Mutex<Option<DecodedFrame>>>,
    status_text: Arc<Mutex<String>>,
    command_tx: Sender<ViewerCommand>,
    handle: Mutex<Option<JoinHandle<()>>>,
}

impl DisplayViewer {
    pub fn start(options: ViewerOptions) -> Result<(Self, Receiver<ViewerEvent>)> {
        let (event_tx, event_rx) = crossbeam_channel::bounded(512);
        let (command_tx, command_rx) = crossbeam_channel::unbounded();
        let (ready_tx, ready_rx) = crossbeam_channel::bounded::<Result<isize>>(1);
        let latest_frame = Arc::new(Mutex::new(None));
        let frame_slot = Arc::clone(&latest_frame);
        let status_text = Arc::new(Mutex::new(
            "Waiting for PC connection on local network...\nPress F11 / Alt+Enter / Ctrl+Alt+F: Fullscreen"
                .to_string(),
        ));
        let status_slot = Arc::clone(&status_text);
        let visible = Arc::new(AtomicBool::new(options.start_visible));
        let visible_flag = Arc::clone(&visible);
        let fullscreen = Arc::new(AtomicBool::new(options.start_fullscreen));
        let fullscreen_flag = Arc::clone(&fullscreen);

        let handle = thread::Builder::new()
            .name("key2pc-viewer".to_string())
            .spawn(move || {
                let _ = unsafe {
                    SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2)
                };
                if let Err(err) = run_viewer_loop(
                    ViewerLoopInit {
                        options,
                        latest_frame: frame_slot,
                        status_text: status_slot,
                        visible: visible_flag,
                        fullscreen_flag,
                        command_rx,
                        event_tx,
                    },
                    &ready_tx,
                ) {
                    let _ = ready_tx.send(Err(err));
                }
            })
            .map_err(|err| Key2PcError::Display(format!("Failed to spawn viewer thread: {err}")))?;

        let hwnd_raw = ready_rx
            .recv()
            .map_err(|_| Key2PcError::Display("Viewer thread exited early".to_string()))??;

        Ok((
            Self {
                hwnd_raw,
                visible,
                fullscreen,
                latest_frame,
                status_text,
                command_tx,
                handle: Mutex::new(Some(handle)),
            },
            event_rx,
        ))
    }

    pub fn is_fullscreen(&self) -> bool {
        self.fullscreen.load(Ordering::Acquire)
    }

    pub fn set_status_text(&self, text: impl Into<String>) {
        let new_text = text.into();
        let changed = if let Ok(mut guard) = self.status_text.lock() {
            if *guard != new_text {
                *guard = new_text;
                true
            } else {
                false
            }
        } else {
            false
        };
        if changed {
            let hwnd = HWND(self.hwnd_raw as *mut _);
            unsafe {
                let _ = InvalidateRect(Some(hwnd), None, true);
            }
        }
    }

    pub fn push_frame(&self, frame: DecodedFrame) {
        if !self.visible.load(Ordering::Acquire) {
            return;
        }
        if let Ok(mut guard) = self.latest_frame.lock() {
            *guard = Some(frame);
        }
        let hwnd = HWND(self.hwnd_raw as *mut _);
        unsafe {
            let _ = PostMessageW(Some(hwnd), WM_APP_NEW_FRAME, WPARAM(0), LPARAM(0));
        }
    }

    pub fn send_command(&self, cmd: ViewerCommand) {
        let _ = self.command_tx.send(cmd);
        let hwnd = HWND(self.hwnd_raw as *mut _);
        unsafe {
            let _ = PostMessageW(Some(hwnd), WM_APP_COMMAND, WPARAM(0), LPARAM(0));
        }
    }

    pub fn is_visible(&self) -> bool {
        self.visible.load(Ordering::Acquire)
    }

    pub fn shutdown(&self) {
        self.send_command(ViewerCommand::Shutdown);
        if let Ok(mut guard) = self.handle.lock() {
            if let Some(handle) = guard.take() {
                let _ = handle.join();
            }
        }
    }
}

impl Drop for DisplayViewer {
    fn drop(&mut self) {
        self.shutdown();
    }
}

struct D3dRenderer {
    hwnd: HWND,
    device: ID3D11Device,
    context: ID3D11DeviceContext,
    video_device: Option<ID3D11VideoDevice>,
    video_context: Option<ID3D11VideoContext>,
    swap_chain: IDXGISwapChain1,
    allow_tearing: bool,
    backbuffer_rtv: Option<ID3D11RenderTargetView>,
    output_view: Option<ID3D11VideoProcessorOutputView>,
    video_enum: Option<ID3D11VideoProcessorEnumerator>,
    processor: Option<ID3D11VideoProcessor>,
    nv12_staging: Option<ID3D11Texture2D>,
    nv12_gpu: Option<ID3D11Texture2D>,
    input_view: Option<ID3D11VideoProcessorInputView>,
    bgra_fallback_staging: Option<ID3D11Texture2D>,
    src_width: u32,
    src_height: u32,
    win_width: u32,
    win_height: u32,
    dest_rect: RECT,
}

impl D3dRenderer {
    fn new(hwnd: HWND, win_width: u32, win_height: u32) -> Result<Self> {
        let (device, context) = create_viewer_d3d11_device()?;
        let video_device = device.cast::<ID3D11VideoDevice>().ok();
        let video_context = context.cast::<ID3D11VideoContext>().ok();

        let dxgi_device: IDXGIDevice = device
            .cast()
            .map_err(|err| Key2PcError::Display(format!("IDXGIDevice cast failed: {err}")))?;
        let adapter = unsafe { dxgi_device.GetAdapter() }
            .map_err(|err| Key2PcError::Display(format!("GetAdapter failed: {err}")))?;
        let factory: IDXGIFactory2 = unsafe { adapter.GetParent() }
            .map_err(|err| Key2PcError::Display(format!("GetParent factory failed: {err}")))?;

        let mut allow_tearing = false;
        if let Ok(factory5) = factory.cast::<IDXGIFactory5>() {
            let mut supported = BOOL(0);
            if unsafe {
                factory5.CheckFeatureSupport(
                    DXGI_FEATURE_PRESENT_ALLOW_TEARING,
                    (&raw mut supported).cast(),
                    std::mem::size_of::<BOOL>() as u32,
                )
            }
            .is_ok()
            {
                allow_tearing = supported.as_bool();
            }
        }

        let swap_flags = if allow_tearing {
            DXGI_SWAP_CHAIN_FLAG_ALLOW_TEARING.0 as u32
        } else {
            0
        };
        let swap_desc = DXGI_SWAP_CHAIN_DESC1 {
            Width: win_width.max(1),
            Height: win_height.max(1),
            Format: DXGI_FORMAT_B8G8R8A8_UNORM,
            Stereo: false.into(),
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
            BufferCount: 2,
            Scaling: DXGI_SCALING_STRETCH,
            SwapEffect: DXGI_SWAP_EFFECT_FLIP_DISCARD,
            AlphaMode: DXGI_ALPHA_MODE_IGNORE,
            Flags: swap_flags,
        };

        let swap_chain =
            unsafe { factory.CreateSwapChainForHwnd(&device, hwnd, &swap_desc, None, None) }
                .map_err(|err| {
                    Key2PcError::Display(format!("CreateSwapChainForHwnd failed: {err}"))
                })?;

        let mut renderer = Self {
            hwnd,
            device,
            context,
            video_device,
            video_context,
            swap_chain,
            allow_tearing,
            backbuffer_rtv: None,
            output_view: None,
            video_enum: None,
            processor: None,
            nv12_staging: None,
            nv12_gpu: None,
            input_view: None,
            bgra_fallback_staging: None,
            src_width: 0,
            src_height: 0,
            win_width: win_width.max(1),
            win_height: win_height.max(1),
            dest_rect: RECT {
                left: 0,
                top: 0,
                right: win_width as i32,
                bottom: win_height as i32,
            },
        };
        renderer.rebuild_backbuffer_views()?;
        Ok(renderer)
    }

    fn resize(&mut self, width: u32, height: u32) -> Result<()> {
        if width == 0 || height == 0 {
            return Ok(());
        }
        self.win_width = width;
        self.win_height = height;
        self.backbuffer_rtv = None;
        self.output_view = None;
        self.bgra_fallback_staging = None;

        let flags = if self.allow_tearing {
            DXGI_SWAP_CHAIN_FLAG_ALLOW_TEARING
        } else {
            DXGI_SWAP_CHAIN_FLAG(0)
        };
        unsafe {
            self.swap_chain
                .ResizeBuffers(0, width, height, DXGI_FORMAT_UNKNOWN, flags)
        }
        .map_err(|err| Key2PcError::Display(format!("ResizeBuffers failed: {err}")))?;

        self.rebuild_backbuffer_views()?;
        if self.src_width > 0 && self.src_height > 0 {
            self.rebuild_processor(self.src_width, self.src_height)?;
        }
        Ok(())
    }

    fn render_nv12(&mut self, frame: &DecodedFrame) -> Result<()> {
        if frame.width == 0 || frame.height == 0 {
            return Ok(());
        }
        if frame.width != self.src_width || frame.height != self.src_height {
            self.rebuild_nv12_resources(frame.width, frame.height)?;
            self.rebuild_processor(frame.width, frame.height)?;
        }

        if let (Some(video_context), Some(processor), Some(out_view), Some(in_view)) = (
            &self.video_context,
            &self.processor,
            &self.output_view,
            &self.input_view,
        ) {
            let staging = self.nv12_staging.as_ref().unwrap();
            let gpu_tex = self.nv12_gpu.as_ref().unwrap();
            let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
            unsafe {
                self.context
                    .Map(staging, 0, D3D11_MAP_WRITE, 0, Some(&mut mapped))
            }
            .map_err(|err| Key2PcError::Display(format!("Viewer Map staging failed: {err}")))?;

            let w = frame.width as usize;
            let h = frame.height as usize;
            let pitch = mapped.RowPitch as usize;
            let dst = unsafe {
                slice::from_raw_parts_mut(mapped.pData.cast::<u8>(), pitch * (h + h / 2))
            };

            for row in 0..h {
                let src_off = row * w;
                let dst_off = row * pitch;
                if src_off + w <= frame.nv12_data.len() {
                    dst[dst_off..dst_off + w]
                        .copy_from_slice(&frame.nv12_data[src_off..src_off + w]);
                }
            }
            let uv_src_base = h * w;
            let uv_dst_base = h * pitch;
            for row in 0..(h / 2) {
                let src_off = uv_src_base + row * w;
                let dst_off = uv_dst_base + row * pitch;
                if src_off + w <= frame.nv12_data.len() {
                    dst[dst_off..dst_off + w]
                        .copy_from_slice(&frame.nv12_data[src_off..src_off + w]);
                }
            }

            unsafe {
                self.context.Unmap(staging, 0);
                self.context.CopyResource(gpu_tex, staging);
            }

            if let Some(rtv) = &self.backbuffer_rtv {
                let black = [0.0f32, 0.0, 0.0, 1.0];
                unsafe {
                    self.context.ClearRenderTargetView(rtv, &black);
                }
            }

            let streams = [D3D11_VIDEO_PROCESSOR_STREAM {
                Enable: true.into(),
                pInputSurface: ManuallyDrop::new(Some(in_view.clone())),
                ..Default::default()
            }];
            let blt_res =
                unsafe { video_context.VideoProcessorBlt(processor, out_view, 0, &streams) };
            let [stream] = streams;
            let _ = ManuallyDrop::into_inner(stream.pInputSurface);
            if blt_res.is_err() {
                self.input_view = None;
                self.processor = None;
                self.video_enum = None;
                self.src_width = 0;
                self.src_height = 0;
                self.render_nv12_software_fallback(frame)?;
            }
        } else {
            self.render_nv12_software_fallback(frame)?;
        }

        let present_flags = if self.allow_tearing {
            DXGI_PRESENT_ALLOW_TEARING
        } else {
            DXGI_PRESENT(0)
        };
        if unsafe { self.swap_chain.Present(0, present_flags) }.is_err() {
            if self.allow_tearing {
                self.allow_tearing = false;
                unsafe { self.swap_chain.Present(0, DXGI_PRESENT(0)) }
                    .ok()
                    .map_err(|err| Key2PcError::Display(format!("Present failed: {err}")))?;
            } else {
                return Err(Key2PcError::Display("SwapChain Present failed".to_string()));
            }
        }
        let _ = self.hwnd;
        Ok(())
    }

    fn render_nv12_software_fallback(&mut self, frame: &DecodedFrame) -> Result<()> {
        if self.bgra_fallback_staging.is_none() {
            let desc = D3D11_TEXTURE2D_DESC {
                Width: self.win_width.max(1),
                Height: self.win_height.max(1),
                MipLevels: 1,
                ArraySize: 1,
                Format: DXGI_FORMAT_B8G8R8A8_UNORM,
                SampleDesc: DXGI_SAMPLE_DESC {
                    Count: 1,
                    Quality: 0,
                },
                Usage: D3D11_USAGE_STAGING,
                BindFlags: 0,
                CPUAccessFlags: D3D11_CPU_ACCESS_WRITE.0 as u32,
                MiscFlags: 0,
            };
            let mut tex: Option<ID3D11Texture2D> = None;
            unsafe { self.device.CreateTexture2D(&desc, None, Some(&mut tex)) }
                .map_err(|err| Key2PcError::Display(format!("Fallback texture failed: {err}")))?;
            self.bgra_fallback_staging = tex;
        }

        let staging = self.bgra_fallback_staging.as_ref().unwrap();
        let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
        unsafe {
            self.context
                .Map(staging, 0, D3D11_MAP_WRITE, 0, Some(&mut mapped))
        }
        .map_err(|err| Key2PcError::Display(format!("Fallback Map failed: {err}")))?;

        let dst_w = self.win_width as usize;
        let dst_h = self.win_height as usize;
        let pitch = mapped.RowPitch as usize;
        let dst = unsafe { slice::from_raw_parts_mut(mapped.pData.cast::<u8>(), pitch * dst_h) };
        nv12_to_bgra_scaled(
            &frame.nv12_data,
            frame.width as usize,
            frame.height as usize,
            dst,
            dst_w,
            dst_h,
            pitch,
            self.dest_rect,
        );

        unsafe {
            self.context.Unmap(staging, 0);
        }
        if let Ok(backbuffer) = unsafe { self.swap_chain.GetBuffer::<ID3D11Texture2D>(0) } {
            unsafe {
                self.context.CopyResource(&backbuffer, staging);
            }
        }
        Ok(())
    }

    fn rebuild_backbuffer_views(&mut self) -> Result<()> {
        let backbuffer: ID3D11Texture2D = unsafe { self.swap_chain.GetBuffer(0) }
            .map_err(|err| Key2PcError::Display(format!("SwapChain GetBuffer failed: {err}")))?;
        let mut rtv: Option<ID3D11RenderTargetView> = None;
        unsafe {
            self.device
                .CreateRenderTargetView(&backbuffer, None, Some(&mut rtv))
        }
        .map_err(|err| Key2PcError::Display(format!("CreateRenderTargetView failed: {err}")))?;
        self.backbuffer_rtv = rtv;

        if let (Some(video_device), Some(video_enum)) = (&self.video_device, &self.video_enum) {
            let desc = D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC {
                ViewDimension: D3D11_VPOV_DIMENSION_TEXTURE2D,
                Anonymous: D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC_0 {
                    Texture2D: Default::default(),
                },
            };
            let mut out_view: Option<ID3D11VideoProcessorOutputView> = None;
            unsafe {
                video_device.CreateVideoProcessorOutputView(
                    &backbuffer,
                    video_enum,
                    &desc,
                    Some(&mut out_view),
                )
            }
            .map_err(|err| {
                Key2PcError::Display(format!(
                    "Viewer CreateVideoProcessorOutputView failed: {err}"
                ))
            })?;
            self.output_view = out_view;
        }
        Ok(())
    }

    fn rebuild_nv12_resources(&mut self, width: u32, height: u32) -> Result<()> {
        self.input_view = None;
        self.nv12_gpu = None;
        self.nv12_staging = None;
        self.src_width = width;
        self.src_height = height;

        if self.video_device.is_none() {
            return Ok(());
        }

        let staging_desc = D3D11_TEXTURE2D_DESC {
            Width: width,
            Height: height,
            MipLevels: 1,
            ArraySize: 1,
            Format: DXGI_FORMAT_NV12,
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            Usage: D3D11_USAGE_STAGING,
            BindFlags: 0,
            CPUAccessFlags: D3D11_CPU_ACCESS_WRITE.0 as u32,
            MiscFlags: 0,
        };
        let mut staging: Option<ID3D11Texture2D> = None;
        unsafe {
            self.device
                .CreateTexture2D(&staging_desc, None, Some(&mut staging))
        }
        .map_err(|err| Key2PcError::Display(format!("Create NV12 staging failed: {err}")))?;

        let gpu_desc = D3D11_TEXTURE2D_DESC {
            Usage: D3D11_USAGE_DEFAULT,
            BindFlags: 0,
            CPUAccessFlags: 0,
            ..staging_desc
        };
        let mut gpu: Option<ID3D11Texture2D> = None;
        if unsafe { self.device.CreateTexture2D(&gpu_desc, None, Some(&mut gpu)) }.is_err() {
            self.video_device = None;
            self.video_context = None;
            return Ok(());
        }

        self.nv12_staging = staging;
        self.nv12_gpu = gpu;
        Ok(())
    }

    fn rebuild_processor(&mut self, src_w: u32, src_h: u32) -> Result<()> {
        self.input_view = None;
        self.output_view = None;
        self.processor = None;
        self.video_enum = None;
        self.dest_rect = RECT {
            left: 0,
            top: 0,
            right: self.win_width as i32,
            bottom: self.win_height as i32,
        };

        let (Some(video_device), Some(video_context)) =
            (self.video_device.clone(), self.video_context.clone())
        else {
            return Ok(());
        };

        let content_desc = D3D11_VIDEO_PROCESSOR_CONTENT_DESC {
            InputFrameFormat: D3D11_VIDEO_FRAME_FORMAT_PROGRESSIVE,
            InputFrameRate: DXGI_RATIONAL {
                Numerator: 60,
                Denominator: 1,
            },
            InputWidth: src_w,
            InputHeight: src_h,
            OutputFrameRate: DXGI_RATIONAL {
                Numerator: 60,
                Denominator: 1,
            },
            OutputWidth: self.win_width.max(1),
            OutputHeight: self.win_height.max(1),
            Usage: D3D11_VIDEO_USAGE_PLAYBACK_NORMAL,
        };

        let Ok(video_enum) =
            (unsafe { video_device.CreateVideoProcessorEnumerator(&content_desc) })
        else {
            self.disable_gpu_video_processor();
            return Ok(());
        };
        let Ok(processor) = (unsafe { video_device.CreateVideoProcessor(&video_enum, 0) }) else {
            self.disable_gpu_video_processor();
            return Ok(());
        };

        let src_rect = RECT {
            left: 0,
            top: 0,
            right: src_w as i32,
            bottom: src_h as i32,
        };
        let full_target = RECT {
            left: 0,
            top: 0,
            right: self.win_width as i32,
            bottom: self.win_height as i32,
        };
        unsafe {
            video_context.VideoProcessorSetStreamSourceRect(&processor, 0, true, Some(&src_rect));
            video_context.VideoProcessorSetStreamDestRect(
                &processor,
                0,
                true,
                Some(&self.dest_rect),
            );
            video_context.VideoProcessorSetOutputTargetRect(&processor, true, Some(&full_target));
        }

        if let Some(gpu_tex) = &self.nv12_gpu {
            if let Some(view) = try_create_viewer_input_view(
                &self.device,
                &video_device,
                &video_enum,
                gpu_tex,
                src_w,
                src_h,
            ) {
                match view {
                    InputViewCreation::Reused(in_view) => self.input_view = Some(in_view),
                    InputViewCreation::Replaced(new_tex, in_view) => {
                        self.nv12_gpu = Some(new_tex);
                        self.input_view = Some(in_view);
                    }
                }
            } else {
                self.disable_gpu_video_processor();
                return Ok(());
            }
        }

        self.video_enum = Some(video_enum);
        self.processor = Some(processor);
        if self.rebuild_backbuffer_views().is_err() {
            self.disable_gpu_video_processor();
        }
        Ok(())
    }

    fn disable_gpu_video_processor(&mut self) {
        self.video_device = None;
        self.video_context = None;
        self.video_enum = None;
        self.processor = None;
        self.input_view = None;
        self.output_view = None;
    }
}

enum InputViewCreation {
    Reused(ID3D11VideoProcessorInputView),
    Replaced(ID3D11Texture2D, ID3D11VideoProcessorInputView),
}

fn try_create_viewer_input_view(
    device: &ID3D11Device,
    video_device: &ID3D11VideoDevice,
    video_enum: &ID3D11VideoProcessorEnumerator,
    existing_tex: &ID3D11Texture2D,
    width: u32,
    height: u32,
) -> Option<InputViewCreation> {
    let desc = D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC {
        FourCC: 0,
        ViewDimension: D3D11_VPIV_DIMENSION_TEXTURE2D,
        Anonymous: D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC_0 {
            Texture2D: Default::default(),
        },
    };
    let mut view: Option<ID3D11VideoProcessorInputView> = None;
    if unsafe {
        video_device.CreateVideoProcessorInputView(existing_tex, video_enum, &desc, Some(&mut view))
    }
    .is_ok()
    {
        return view.map(InputViewCreation::Reused);
    }

    let rt_desc = D3D11_TEXTURE2D_DESC {
        Width: width,
        Height: height,
        MipLevels: 1,
        ArraySize: 1,
        Format: DXGI_FORMAT_NV12,
        SampleDesc: DXGI_SAMPLE_DESC {
            Count: 1,
            Quality: 0,
        },
        Usage: D3D11_USAGE_DEFAULT,
        BindFlags: D3D11_BIND_RENDER_TARGET.0 as u32,
        CPUAccessFlags: 0,
        MiscFlags: 0,
    };
    let mut rt_tex: Option<ID3D11Texture2D> = None;
    if unsafe { device.CreateTexture2D(&rt_desc, None, Some(&mut rt_tex)) }.is_err() {
        return None;
    }
    let rt_tex = rt_tex?;
    let mut rt_view: Option<ID3D11VideoProcessorInputView> = None;
    if unsafe {
        video_device.CreateVideoProcessorInputView(&rt_tex, video_enum, &desc, Some(&mut rt_view))
    }
    .is_ok()
    {
        return rt_view.map(|v| InputViewCreation::Replaced(rt_tex, v));
    }
    None
}

fn create_viewer_d3d11_device() -> Result<(ID3D11Device, ID3D11DeviceContext)> {
    let attempts = [
        (
            D3D_DRIVER_TYPE_HARDWARE,
            D3D11_CREATE_DEVICE_BGRA_SUPPORT | D3D11_CREATE_DEVICE_VIDEO_SUPPORT,
        ),
        (D3D_DRIVER_TYPE_HARDWARE, D3D11_CREATE_DEVICE_BGRA_SUPPORT),
        (D3D_DRIVER_TYPE_WARP, D3D11_CREATE_DEVICE_BGRA_SUPPORT),
    ];

    let mut last_err = String::from("D3D11CreateDevice failed");
    for (driver, flags) in attempts {
        let mut device: Option<ID3D11Device> = None;
        let mut context: Option<ID3D11DeviceContext> = None;
        match unsafe {
            D3D11CreateDevice(
                None,
                driver,
                Default::default(),
                flags,
                None,
                D3D11_SDK_VERSION,
                Some(&mut device),
                None,
                Some(&mut context),
            )
        } {
            Ok(()) => {
                if let (Some(d), Some(c)) = (device, context) {
                    return Ok((d, c));
                }
            }
            Err(err) => last_err = err.to_string(),
        }
    }
    Err(Key2PcError::Display(format!(
        "Viewer D3D11CreateDevice failed: {last_err}"
    )))
}

#[allow(clippy::too_many_arguments)]
fn nv12_to_bgra_scaled(
    nv12: &[u8],
    src_w: usize,
    src_h: usize,
    dst: &mut [u8],
    dst_w: usize,
    dst_h: usize,
    dst_pitch: usize,
    dest_rect: RECT,
) {
    dst.fill(0);
    if src_w == 0 || src_h == 0 || nv12.len() < src_w * src_h * 3 / 2 {
        return;
    }
    let rx0 = dest_rect.left.max(0) as usize;
    let ry0 = dest_rect.top.max(0) as usize;
    let rx1 = (dest_rect.right.max(0) as usize).min(dst_w);
    let ry1 = (dest_rect.bottom.max(0) as usize).min(dst_h);
    let rw = rx1.saturating_sub(rx0).max(1);
    let rh = ry1.saturating_sub(ry0).max(1);
    let uv_base = src_w * src_h;

    for dy in ry0..ry1 {
        let sy = ((dy - ry0) * src_h) / rh;
        let y_row = sy * src_w;
        let uv_row = uv_base + (sy / 2) * src_w;
        let row_out = &mut dst[dy * dst_pitch..dy * dst_pitch + dst_w * 4];
        for dx in rx0..rx1 {
            let sx = ((dx - rx0) * src_w) / rw;
            let y_val = i32::from(nv12[y_row + sx]) - 16;
            let uv_idx = uv_row + (sx & !1);
            let u_val = i32::from(nv12[uv_idx]) - 128;
            let v_val = i32::from(nv12[uv_idx + 1]) - 128;

            let c = y_val.max(0) * 298;
            let r = ((c + 409 * v_val + 128) >> 8).clamp(0, 255) as u8;
            let g = ((c - 100 * u_val - 208 * v_val + 128) >> 8).clamp(0, 255) as u8;
            let b = ((c + 516 * u_val + 128) >> 8).clamp(0, 255) as u8;

            let px = dx * 4;
            row_out[px] = b;
            row_out[px + 1] = g;
            row_out[px + 2] = r;
            row_out[px + 3] = 255;
        }
    }
}

struct ViewerWindowState {
    renderer: D3dRenderer,
    latest_frame: Arc<Mutex<Option<DecodedFrame>>>,
    last_rendered: Option<DecodedFrame>,
    status_text: Arc<Mutex<String>>,
    has_frame: bool,
    visible: Arc<AtomicBool>,
    fullscreen_flag: Arc<AtomicBool>,
    command_rx: Receiver<ViewerCommand>,
    event_tx: Sender<ViewerEvent>,
    capture_mouse: bool,
    focused: bool,
    fullscreen: bool,
    saved_windowed_rect: RECT,
    saved_style: i32,
    fps_counter: u32,
    fps_window_start: Instant,
    last_resolution: (u32, u32),
}

impl ViewerWindowState {
    fn set_fullscreen(&mut self, hwnd: HWND, enable: bool) {
        if enable && !self.visible.load(Ordering::Acquire) {
            self.set_visible(hwnd, true);
        }
        if self.fullscreen == enable {
            return;
        }
        let style = unsafe { GetWindowLongW(hwnd, GWL_STYLE) };
        if enable {
            self.saved_style = style;
            let _ = unsafe { GetWindowRect(hwnd, &mut self.saved_windowed_rect) };
            let monitor = unsafe { MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST) };
            let mut mi = MONITORINFO {
                cbSize: std::mem::size_of::<MONITORINFO>() as u32,
                ..Default::default()
            };
            if unsafe { GetMonitorInfoW(monitor, &mut mi) }.as_bool() {
                let r = mi.rcMonitor;
                let new_style = (style & !(WS_OVERLAPPEDWINDOW.0 as i32)) | (WS_POPUP.0 as i32);
                unsafe {
                    SetWindowLongW(hwnd, GWL_STYLE, new_style);
                    let _ = SetWindowPos(
                        hwnd,
                        Some(HWND_TOP),
                        r.left,
                        r.top,
                        r.right - r.left,
                        r.bottom - r.top,
                        SWP_NOOWNERZORDER | SWP_FRAMECHANGED,
                    );
                }
                self.fullscreen = true;
            }
        } else {
            let r = self.saved_windowed_rect;
            let restore_style = self.saved_style | (WS_OVERLAPPEDWINDOW.0 as i32);
            unsafe {
                SetWindowLongW(hwnd, GWL_STYLE, restore_style);
                let _ = SetWindowPos(
                    hwnd,
                    None,
                    r.left,
                    r.top,
                    r.right - r.left,
                    r.bottom - r.top,
                    SWP_NOZORDER | SWP_NOOWNERZORDER | SWP_FRAMECHANGED,
                );
            }
            self.fullscreen = false;
        }
        self.fullscreen_flag
            .store(self.fullscreen, Ordering::Release);
        let _ = self
            .event_tx
            .try_send(ViewerEvent::FullscreenChanged(self.fullscreen));
    }

    fn toggle_fullscreen(&mut self, hwnd: HWND) {
        let next = !self.fullscreen;
        self.set_fullscreen(hwnd, next);
    }

    fn set_visible(&mut self, hwnd: HWND, show: bool) {
        self.visible.store(show, Ordering::Release);
        unsafe {
            let _ = ShowWindow(hwnd, if show { SW_SHOW } else { SW_HIDE });
        }
        let _ = self.event_tx.try_send(ViewerEvent::VisibilityChanged(show));
        if !show {
            let _ = self.event_tx.try_send(ViewerEvent::FocusChanged(false));
        }
    }

    fn emit_mouse(&self, action: MouseAction, lparam: LPARAM, wheel_delta: i16) {
        if !self.capture_mouse {
            return;
        }
        let x = (lparam.0 & 0xFFFF) as i16 as i32;
        let y = ((lparam.0 >> 16) & 0xFFFF) as i16 as i32;
        let Some((norm_x, norm_y)) = client_to_normalized(x, y, self.renderer.dest_rect) else {
            return;
        };
        let _ = self.event_tx.try_send(ViewerEvent::Mouse(MouseEvent {
            action,
            norm_x,
            norm_y,
            wheel_delta,
        }));
    }

    fn update_fps_title(&mut self, hwnd: HWND, w: u32, h: u32) {
        self.fps_counter += 1;
        self.last_resolution = (w, h);
        let elapsed = self.fps_window_start.elapsed();
        if elapsed.as_secs_f32() >= 1.0 {
            let fps = (self.fps_counter as f32 / elapsed.as_secs_f32()).round() as u32;
            self.fps_counter = 0;
            self.fps_window_start = Instant::now();
            if !self.fullscreen {
                let title = format!(
                    "Key2PC Second Screen - {}x{} @ {} fps | F11 Fullscreen",
                    self.last_resolution.0, self.last_resolution.1, fps
                );
                let wide: Vec<u16> = title.encode_utf16().chain(std::iter::once(0)).collect();
                unsafe {
                    let _ = SetWindowTextW(hwnd, PCWSTR(wide.as_ptr()));
                }
            }
        }
    }
}

struct ViewerLoopInit {
    options: ViewerOptions,
    latest_frame: Arc<Mutex<Option<DecodedFrame>>>,
    status_text: Arc<Mutex<String>>,
    visible: Arc<AtomicBool>,
    fullscreen_flag: Arc<AtomicBool>,
    command_rx: Receiver<ViewerCommand>,
    event_tx: Sender<ViewerEvent>,
}

fn run_viewer_loop(init: ViewerLoopInit, ready_tx: &Sender<Result<isize>>) -> Result<()> {
    let ViewerLoopInit {
        options,
        latest_frame,
        status_text,
        visible,
        fullscreen_flag,
        command_rx,
        event_tx,
    } = init;
    let class_name = w!("Key2PCSecondScreenClass");
    let hinstance = unsafe { GetModuleHandleW(None) }
        .map_err(|err| Key2PcError::Display(format!("GetModuleHandleW failed: {err}")))?;
    let cursor = unsafe { LoadCursorW(None, IDC_ARROW) }.unwrap_or_default();
    let black_brush = HBRUSH(unsafe { GetStockObject(BLACK_BRUSH) }.0);

    let wc = WNDCLASSW {
        lpfnWndProc: Some(viewer_wnd_proc),
        hInstance: hinstance.into(),
        hCursor: cursor,
        hbrBackground: black_brush,
        lpszClassName: class_name,
        ..Default::default()
    };
    unsafe { RegisterClassW(&wc) };

    let mut style = WINDOW_STYLE(WS_OVERLAPPEDWINDOW.0);
    if options.start_visible {
        style |= WS_VISIBLE;
    }

    let hwnd = unsafe {
        CreateWindowExW(
            Default::default(),
            class_name,
            w!("Key2PC Second Screen | F11 Fullscreen"),
            style,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            options.initial_width.min(1280) as i32,
            options.initial_height.min(720) as i32,
            None,
            None,
            Some(hinstance.into()),
            None,
        )
    }
    .map_err(|err| Key2PcError::Display(format!("CreateWindowExW failed: {err}")))?;

    let mut client_rect = RECT::default();
    let _ = unsafe { GetClientRect(hwnd, &mut client_rect) };
    let win_w = (client_rect.right - client_rect.left).max(1) as u32;
    let win_h = (client_rect.bottom - client_rect.top).max(1) as u32;
    let renderer = D3dRenderer::new(hwnd, win_w, win_h)?;

    let mut state = Box::new(ViewerWindowState {
        renderer,
        latest_frame,
        last_rendered: None,
        status_text,
        has_frame: false,
        visible,
        fullscreen_flag,
        command_rx,
        event_tx,
        capture_mouse: options.capture_mouse,
        focused: options.start_visible,
        fullscreen: false,
        saved_windowed_rect: RECT {
            left: 100,
            top: 100,
            right: 1380,
            bottom: 820,
        },
        saved_style: WS_OVERLAPPEDWINDOW.0 as i32,
        fps_counter: 0,
        fps_window_start: Instant::now(),
        last_resolution: (options.initial_width, options.initial_height),
    });

    if options.start_fullscreen {
        state.set_fullscreen(hwnd, true);
    }

    let state_ptr = Box::into_raw(state);
    unsafe {
        windows::Win32::UI::WindowsAndMessaging::SetWindowLongPtrW(
            hwnd,
            windows::Win32::UI::WindowsAndMessaging::GWLP_USERDATA,
            state_ptr as isize,
        );
    }

    let _ = ready_tx.send(Ok(hwnd.0 as isize));

    let mut msg = MSG::default();
    while unsafe { GetMessageW(&mut msg, None, 0, 0) }.as_bool() {
        unsafe {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }

    let boxed = unsafe { Box::from_raw(state_ptr) };
    drop(boxed);
    Ok(())
}

unsafe extern "system" fn viewer_wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    let ptr = unsafe {
        windows::Win32::UI::WindowsAndMessaging::GetWindowLongPtrW(
            hwnd,
            windows::Win32::UI::WindowsAndMessaging::GWLP_USERDATA,
        )
    } as *mut ViewerWindowState;

    if ptr.is_null() {
        return unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) };
    }
    let state = unsafe { &mut *ptr };

    match msg {
        WM_APP_NEW_FRAME => {
            let frame_opt = state.latest_frame.lock().ok().and_then(|mut g| g.take());
            if let Some(frame) = frame_opt {
                let (w, h) = (frame.width, frame.height);
                if state.renderer.render_nv12(&frame).is_ok() {
                    state.has_frame = true;
                    state.update_fps_title(hwnd, w, h);
                    state.last_rendered = Some(frame);
                } else {
                    let mut rect = RECT::default();
                    let _ = unsafe { GetClientRect(hwnd, &mut rect) };
                    let rw = (rect.right - rect.left).max(1) as u32;
                    let rh = (rect.bottom - rect.top).max(1) as u32;
                    if let Ok(fresh) = D3dRenderer::new(hwnd, rw, rh) {
                        state.renderer = fresh;
                        if state.renderer.render_nv12(&frame).is_ok() {
                            state.has_frame = true;
                            state.update_fps_title(hwnd, w, h);
                            state.last_rendered = Some(frame);
                        }
                    }
                }
            }
            LRESULT(0)
        }
        0x007E => {
            if state.fullscreen {
                let monitor = unsafe { MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST) };
                let mut mi = MONITORINFO {
                    cbSize: std::mem::size_of::<MONITORINFO>() as u32,
                    ..Default::default()
                };
                if unsafe { GetMonitorInfoW(monitor, &mut mi) }.as_bool() {
                    let r = mi.rcMonitor;
                    unsafe {
                        let _ = SetWindowPos(
                            hwnd,
                            Some(HWND_TOP),
                            r.left,
                            r.top,
                            r.right - r.left,
                            r.bottom - r.top,
                            SWP_NOOWNERZORDER | SWP_FRAMECHANGED,
                        );
                    }
                }
            }
            let _ = unsafe { InvalidateRect(Some(hwnd), None, false) };
            LRESULT(0)
        }
        WM_PAINT => {
            if !state.has_frame {
                let mut ps = PAINTSTRUCT::default();
                let hdc = unsafe { BeginPaint(hwnd, &mut ps) };
                let mut rect = RECT::default();
                let _ = unsafe { GetClientRect(hwnd, &mut rect) };
                let brush = HBRUSH(unsafe { GetStockObject(BLACK_BRUSH) }.0);
                unsafe {
                    FillRect(hdc, &rect, brush);
                    SetBkMode(hdc, TRANSPARENT);
                    SetTextColor(hdc, COLORREF(0x00E5_E7EB));
                }
                let text = state
                    .status_text
                    .lock()
                    .map(|g| g.clone())
                    .unwrap_or_default();
                let mut wide: Vec<u16> = text.encode_utf16().collect();
                if !wide.is_empty() {
                    let h = (rect.bottom - rect.top).max(1);
                    rect.top = (h / 2 - 32).max(0);
                    unsafe {
                        DrawTextW(hdc, &mut wide, &mut rect, DT_CENTER | DT_WORDBREAK);
                    }
                }
                let _ = unsafe { EndPaint(hwnd, &ps) };
                return LRESULT(0);
            }
            unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
        }
        WM_APP_COMMAND => {
            while let Ok(cmd) = state.command_rx.try_recv() {
                match cmd {
                    ViewerCommand::Show => state.set_visible(hwnd, true),
                    ViewerCommand::Hide => state.set_visible(hwnd, false),
                    ViewerCommand::ToggleVisible => {
                        let next = !state.visible.load(Ordering::Acquire);
                        state.set_visible(hwnd, next);
                    }
                    ViewerCommand::ToggleFullscreen => state.toggle_fullscreen(hwnd),
                    ViewerCommand::SetFullscreen(enable) => state.set_fullscreen(hwnd, enable),
                    ViewerCommand::Shutdown => unsafe {
                        let _ = DestroyWindow(hwnd);
                    },
                }
            }
            LRESULT(0)
        }
        WM_SIZE => {
            let w = (lparam.0 & 0xFFFF) as u32;
            let h = ((lparam.0 >> 16) & 0xFFFF) as u32;
            let _ = state.renderer.resize(w, h);
            if let Some(frame) = &state.last_rendered {
                let _ = state.renderer.render_nv12(frame);
            }
            LRESULT(0)
        }
        WM_SETCURSOR => {
            let hit = (lparam.0 & 0xFFFF) as u32;
            if hit == HTCLIENT && state.has_frame && state.capture_mouse && state.focused {
                unsafe {
                    SetCursor(None);
                }
                return LRESULT(1);
            }
            unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
        }
        WM_KEYDOWN | WM_SYSKEYDOWN => {
            let is_alt = (lparam.0 & (1 << 29)) != 0;
            let is_f11 = wparam.0 == usize::from(VK_F11.0);
            let is_alt_enter = wparam.0 == usize::from(VK_RETURN.0) && is_alt;
            if is_f11 || is_alt_enter {
                state.toggle_fullscreen(hwnd);
                return LRESULT(0);
            }
            unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
        }
        WM_SETFOCUS => {
            state.focused = true;
            let _ = state.event_tx.try_send(ViewerEvent::FocusChanged(true));
            LRESULT(0)
        }
        WM_KILLFOCUS => {
            state.focused = false;
            let _ = state.event_tx.try_send(ViewerEvent::FocusChanged(false));
            LRESULT(0)
        }
        WM_MOUSEMOVE => {
            state.emit_mouse(MouseAction::Move, lparam, 0);
            LRESULT(0)
        }
        WM_LBUTTONDOWN => {
            unsafe { SetCapture(hwnd) };
            state.emit_mouse(MouseAction::LeftDown, lparam, 0);
            LRESULT(0)
        }
        WM_LBUTTONUP => {
            let _ = unsafe { ReleaseCapture() };
            state.emit_mouse(MouseAction::LeftUp, lparam, 0);
            LRESULT(0)
        }
        WM_RBUTTONDOWN => {
            unsafe { SetCapture(hwnd) };
            state.emit_mouse(MouseAction::RightDown, lparam, 0);
            LRESULT(0)
        }
        WM_RBUTTONUP => {
            let _ = unsafe { ReleaseCapture() };
            state.emit_mouse(MouseAction::RightUp, lparam, 0);
            LRESULT(0)
        }
        WM_MBUTTONDOWN => {
            unsafe { SetCapture(hwnd) };
            state.emit_mouse(MouseAction::MiddleDown, lparam, 0);
            LRESULT(0)
        }
        WM_MBUTTONUP => {
            let _ = unsafe { ReleaseCapture() };
            state.emit_mouse(MouseAction::MiddleUp, lparam, 0);
            LRESULT(0)
        }
        WM_XBUTTONDOWN => {
            unsafe { SetCapture(hwnd) };
            let btn = (wparam.0 >> 16) & 0xFFFF;
            let action = if btn == 1 {
                MouseAction::X1Down
            } else {
                MouseAction::X2Down
            };
            state.emit_mouse(action, lparam, 0);
            LRESULT(1)
        }
        WM_XBUTTONUP => {
            let _ = unsafe { ReleaseCapture() };
            let btn = (wparam.0 >> 16) & 0xFFFF;
            let action = if btn == 1 {
                MouseAction::X1Up
            } else {
                MouseAction::X2Up
            };
            state.emit_mouse(action, lparam, 0);
            LRESULT(1)
        }
        WM_MOUSEWHEEL => {
            let delta = ((wparam.0 >> 16) & 0xFFFF) as u16 as i16;
            state.emit_mouse(MouseAction::Wheel, lparam, delta);
            LRESULT(0)
        }
        WM_MOUSEHWHEEL => {
            let delta = ((wparam.0 >> 16) & 0xFFFF) as u16 as i16;
            state.emit_mouse(MouseAction::HWheel, lparam, delta);
            LRESULT(0)
        }
        WM_CLOSE => {
            state.set_visible(hwnd, false);
            LRESULT(0)
        }
        WM_DESTROY => {
            unsafe { PostQuitMessage(0) };
            LRESULT(0)
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

fn client_to_normalized(x: i32, y: i32, rect: RECT) -> Option<(u16, u16)> {
    let w = (rect.right - rect.left).max(1);
    let h = (rect.bottom - rect.top).max(1);
    if w <= 0 || h <= 0 {
        return None;
    }
    let rx = (x - rect.left).clamp(0, w - 1);
    let ry = (y - rect.top).clamp(0, h - 1);
    let norm_x = ((i64::from(rx) * 65535 + i64::from((w - 1).max(1)) / 2)
        / i64::from((w - 1).max(1)))
    .clamp(0, 65535) as u16;
    let norm_y = ((i64::from(ry) * 65535 + i64::from((h - 1).max(1)) / 2)
        / i64::from((h - 1).max(1)))
    .clamp(0, 65535) as u16;
    Some((norm_x, norm_y))
}
