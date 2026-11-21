use std::mem::ManuallyDrop;
use std::ptr;
use std::slice;
use std::time::{Duration, Instant};

use windows::core::{Interface, PCWSTR};
use windows::Win32::Foundation::RECT;
use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_UNKNOWN;
use windows::Win32::Graphics::Direct3D11::{
    D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext, ID3D11Multithread, ID3D11Texture2D,
    ID3D11VideoContext, ID3D11VideoDevice, ID3D11VideoProcessor, ID3D11VideoProcessorEnumerator,
    ID3D11VideoProcessorInputView, ID3D11VideoProcessorOutputView, D3D11_BIND_RENDER_TARGET,
    D3D11_BOX, D3D11_CPU_ACCESS_READ, D3D11_CREATE_DEVICE_BGRA_SUPPORT,
    D3D11_CREATE_DEVICE_VIDEO_SUPPORT, D3D11_MAPPED_SUBRESOURCE, D3D11_MAP_READ, D3D11_SDK_VERSION,
    D3D11_TEXTURE2D_DESC, D3D11_USAGE_DEFAULT, D3D11_USAGE_STAGING,
    D3D11_VIDEO_FRAME_FORMAT_PROGRESSIVE, D3D11_VIDEO_PROCESSOR_CONTENT_DESC,
    D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC, D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC_0,
    D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC, D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC_0,
    D3D11_VIDEO_PROCESSOR_STREAM, D3D11_VIDEO_USAGE_PLAYBACK_NORMAL,
    D3D11_VPIV_DIMENSION_TEXTURE2D, D3D11_VPOV_DIMENSION_TEXTURE2D,
};
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_FORMAT_NV12, DXGI_RATIONAL, DXGI_SAMPLE_DESC,
};
use windows::Win32::Graphics::Dxgi::{
    CreateDXGIFactory1, IDXGIAdapter1, IDXGIFactory1, IDXGIOutput, IDXGIOutput1,
    IDXGIOutputDuplication, DXGI_ERROR_ACCESS_LOST, DXGI_ERROR_DEVICE_REMOVED,
    DXGI_ERROR_DEVICE_RESET, DXGI_ERROR_INVALID_CALL, DXGI_ERROR_NOT_CURRENTLY_AVAILABLE,
    DXGI_ERROR_WAIT_TIMEOUT, DXGI_OUTDUPL_FRAME_INFO, DXGI_OUTDUPL_POINTER_SHAPE_INFO,
    DXGI_OUTDUPL_POINTER_SHAPE_TYPE_COLOR, DXGI_OUTDUPL_POINTER_SHAPE_TYPE_MASKED_COLOR,
    DXGI_OUTDUPL_POINTER_SHAPE_TYPE_MONOCHROME,
};
use windows::Win32::Graphics::Gdi::{
    BitBlt, CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, EnumDisplayDevicesW,
    GdiFlush, GetDC, ReleaseDC, SelectObject, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, CAPTUREBLT,
    DIB_RGB_COLORS, DISPLAY_DEVICEW, HBITMAP, HDC, HGDIOBJ, SRCCOPY,
};
use windows::Win32::UI::HiDpi::{
    SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
};
use windows::Win32::UI::WindowsAndMessaging::{
    DrawIconEx, GetCursorInfo, GetIconInfo, CURSORINFO, CURSOR_SHOWING, DI_NORMAL, HICON, ICONINFO,
};

use crate::display::{
    compute_encode_dimensions, is_virtual_display_signature, select_best_monitor,
    utf16_z_to_string, CaptureResult, MonitorInfo, PointerShapeDesc, ScreenCapturer,
    EDD_GET_DEVICE_INTERFACE_NAME,
};
use crate::error::{Key2PcError, Result};
use crate::input::DesktopBounds;

struct SelectedOutput {
    adapter: IDXGIAdapter1,
    output: IDXGIOutput,
    info: MonitorInfo,
}

pub fn enumerate_monitors() -> Result<Vec<MonitorInfo>> {
    let _ = unsafe { SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
    Ok(collect_outputs()?
        .into_iter()
        .map(|item| item.info)
        .collect())
}

fn collect_outputs() -> Result<Vec<SelectedOutput>> {
    let factory: IDXGIFactory1 = unsafe { CreateDXGIFactory1() }
        .map_err(|err| Key2PcError::Display(format!("CreateDXGIFactory1 failed: {err}")))?;
    let mut result = Vec::new();
    let mut adapter_idx = 0u32;

    while let Ok(adapter) = unsafe { factory.EnumAdapters1(adapter_idx) } {
        adapter_idx += 1;
        let adapter_name = unsafe { adapter.GetDesc1() }
            .map(|d| utf16_z_to_string(&d.Description))
            .unwrap_or_default();
        let mut output_idx = 0u32;
        while let Ok(output) = unsafe { adapter.EnumOutputs(output_idx) } {
            output_idx += 1;
            let Ok(desc) = (unsafe { output.GetDesc() }) else {
                continue;
            };
            if !desc.AttachedToDesktop.as_bool() {
                continue;
            }
            let rect = desc.DesktopCoordinates;
            let bounds = DesktopBounds::new(
                rect.left,
                rect.top,
                rect.right - rect.left,
                rect.bottom - rect.top,
            );
            let name = utf16_z_to_string(&desc.DeviceName);
            let is_virtual = check_output_is_virtual(&name, &adapter_name);
            let is_primary = rect.left == 0 && rect.top == 0;
            let index = result.len();
            result.push(SelectedOutput {
                adapter: adapter.clone(),
                output,
                info: MonitorInfo {
                    index,
                    name,
                    bounds,
                    is_primary,
                    is_virtual,
                },
            });
        }
    }

    Ok(result)
}

fn check_output_is_virtual(device_name: &str, adapter_name: &str) -> bool {
    let wide: Vec<u16> = device_name
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    let mut mon = DISPLAY_DEVICEW {
        cb: std::mem::size_of::<DISPLAY_DEVICEW>() as u32,
        ..Default::default()
    };
    let (mon_str, mon_id) = if unsafe {
        EnumDisplayDevicesW(
            PCWSTR(wide.as_ptr()),
            0,
            &mut mon,
            EDD_GET_DEVICE_INTERFACE_NAME,
        )
    }
    .as_bool()
    {
        (
            utf16_z_to_string(&mon.DeviceString),
            utf16_z_to_string(&mon.DeviceID),
        )
    } else {
        (String::new(), String::new())
    };
    is_virtual_display_signature(device_name, &format!("{adapter_name} {mon_str}"), &mon_id)
}

struct GpuColorConverter {
    _video_device: ID3D11VideoDevice,
    video_context: ID3D11VideoContext,
    _video_enum: ID3D11VideoProcessorEnumerator,
    processor: ID3D11VideoProcessor,
    capture_tex: ID3D11Texture2D,
    input_view: ID3D11VideoProcessorInputView,
    nv12_gpu_tex: ID3D11Texture2D,
    output_view: ID3D11VideoProcessorOutputView,
    nv12_staging_tex: ID3D11Texture2D,
}

struct GdiCaptureFallback {
    screen_dc: HDC,
    mem_dc: HDC,
    bitmap: HBITMAP,
    old_obj: HGDIOBJ,
    bits_ptr: *const u8,
    src_width: usize,
    src_height: usize,
    bounds: DesktopBounds,
}

unsafe impl Send for GdiCaptureFallback {}

impl GdiCaptureFallback {
    fn new(bounds: DesktopBounds) -> Result<Self> {
        let src_w = bounds.width.max(1) as usize;
        let src_h = bounds.height.max(1) as usize;
        let screen_dc = unsafe { GetDC(None) };
        if screen_dc.is_invalid() {
            return Err(Key2PcError::Display("GDI GetDC failed".to_string()));
        }
        let mem_dc = unsafe { CreateCompatibleDC(Some(screen_dc)) };
        if mem_dc.is_invalid() {
            unsafe { ReleaseDC(None, screen_dc) };
            return Err(Key2PcError::Display(
                "GDI CreateCompatibleDC failed".to_string(),
            ));
        }

        let bmi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: src_w as i32,
                biHeight: -(src_h as i32),
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };

        let mut bits_void: *mut core::ffi::c_void = ptr::null_mut();
        let bitmap = unsafe {
            CreateDIBSection(Some(mem_dc), &bmi, DIB_RGB_COLORS, &mut bits_void, None, 0)
        }
        .map_err(|err| {
            unsafe {
                let _ = DeleteDC(mem_dc);
                ReleaseDC(None, screen_dc);
            }
            Key2PcError::Display(format!("CreateDIBSection failed: {err}"))
        })?;

        if bits_void.is_null() {
            unsafe {
                let _ = DeleteObject(HGDIOBJ(bitmap.0));
                let _ = DeleteDC(mem_dc);
                ReleaseDC(None, screen_dc);
            }
            return Err(Key2PcError::Display(
                "CreateDIBSection null bits".to_string(),
            ));
        }

        let old_obj = unsafe { SelectObject(mem_dc, HGDIOBJ(bitmap.0)) };
        Ok(Self {
            screen_dc,
            mem_dc,
            bitmap,
            old_obj,
            bits_ptr: bits_void.cast::<u8>(),
            src_width: src_w,
            src_height: src_h,
            bounds,
        })
    }

    fn capture_nv12(&mut self, dst_w: usize, dst_h: usize, out_nv12: &mut Vec<u8>) -> Result<()> {
        let ok = unsafe {
            BitBlt(
                self.mem_dc,
                0,
                0,
                self.src_width as i32,
                self.src_height as i32,
                Some(self.screen_dc),
                self.bounds.left,
                self.bounds.top,
                SRCCOPY | CAPTUREBLT,
            )
        };
        if ok.is_err() {
            return Err(Key2PcError::Display("GDI BitBlt failed".to_string()));
        }
        draw_gdi_cursor(self.mem_dc, self.bounds);
        unsafe {
            let _ = GdiFlush();
        }
        let pitch = self.src_width * 4;
        let src = unsafe { slice::from_raw_parts(self.bits_ptr, pitch * self.src_height) };
        out_nv12.resize(dst_w * dst_h * 3 / 2, 0);
        bgra_to_nv12_scaled(
            src,
            self.src_width,
            self.src_height,
            pitch,
            dst_w,
            dst_h,
            out_nv12,
        );
        Ok(())
    }
}

impl Drop for GdiCaptureFallback {
    fn drop(&mut self) {
        unsafe {
            SelectObject(self.mem_dc, self.old_obj);
            let _ = DeleteObject(HGDIOBJ(self.bitmap.0));
            let _ = DeleteDC(self.mem_dc);
            ReleaseDC(None, self.screen_dc);
        }
    }
}

#[derive(Default)]
struct PointerShapeState {
    data: Vec<u8>,
    info: DXGI_OUTDUPL_POINTER_SHAPE_INFO,
    visible: bool,
    x: i32,
    y: i32,
}

impl PointerShapeState {
    fn logical_height(&self) -> u32 {
        if self.info.Type == DXGI_OUTDUPL_POINTER_SHAPE_TYPE_MONOCHROME.0 as u32 {
            self.info.Height / 2
        } else {
            self.info.Height
        }
    }
}

pub struct DxgiScreenCapturer {
    monitor_setting: String,
    cfg_width: u32,
    cfg_height: u32,
    match_source_resolution: bool,
    device: ID3D11Device,
    context: ID3D11DeviceContext,
    output1: IDXGIOutput1,
    duplication: Option<IDXGIOutputDuplication>,
    gpu_converter: Option<GpuColorConverter>,
    cpu_bgra_staging: Option<ID3D11Texture2D>,
    gdi_fallback: Option<GdiCaptureFallback>,
    pointer: PointerShapeState,
    pointer_patch_staging: Option<ID3D11Texture2D>,
    pointer_patch_capacity: (u32, u32),
    pointer_scratch_bgra: Vec<u8>,
    cpu_bgra_work: Vec<u8>,
    src_width: u32,
    src_height: u32,
    bounds: DesktopBounds,
    target_width: u32,
    target_height: u32,
    last_bounds_check: Instant,
    last_dxgi_retry: Instant,
}

struct CapturePipelineState {
    device: ID3D11Device,
    context: ID3D11DeviceContext,
    output1: IDXGIOutput1,
    duplication: Option<IDXGIOutputDuplication>,
    gpu_converter: Option<GpuColorConverter>,
    cpu_bgra_staging: Option<ID3D11Texture2D>,
    gdi_fallback: Option<GdiCaptureFallback>,
    src_width: u32,
    src_height: u32,
    target_width: u32,
    target_height: u32,
    bounds: DesktopBounds,
}

impl DxgiScreenCapturer {
    pub fn new(monitor_setting: &str, target_width: u32, target_height: u32) -> Result<Self> {
        Self::new_with_mode(monitor_setting, target_width, target_height, false)
    }

    pub fn new_with_mode(
        monitor_setting: &str,
        cfg_width: u32,
        cfg_height: u32,
        match_source_resolution: bool,
    ) -> Result<Self> {
        let state = build_capture_pipeline(
            monitor_setting,
            cfg_width,
            cfg_height,
            match_source_resolution,
            false,
        )?;
        let now = Instant::now();

        Ok(Self {
            monitor_setting: monitor_setting.to_string(),
            cfg_width,
            cfg_height,
            match_source_resolution,
            device: state.device,
            context: state.context,
            output1: state.output1,
            duplication: state.duplication,
            gpu_converter: state.gpu_converter,
            cpu_bgra_staging: state.cpu_bgra_staging,
            gdi_fallback: state.gdi_fallback,
            pointer: PointerShapeState::default(),
            pointer_patch_staging: None,
            pointer_patch_capacity: (0, 0),
            pointer_scratch_bgra: Vec::new(),
            cpu_bgra_work: Vec::new(),
            src_width: state.src_width,
            src_height: state.src_height,
            bounds: state.bounds,
            target_width: state.target_width,
            target_height: state.target_height,
            last_bounds_check: now,
            last_dxgi_retry: now,
        })
    }

    pub fn capture_gdi_snapshot(&mut self, out_nv12: &mut Vec<u8>) -> Result<()> {
        if let Some(gdi) = &mut self.gdi_fallback {
            return gdi.capture_nv12(
                self.target_width as usize,
                self.target_height as usize,
                out_nv12,
            );
        }
        let mut gdi = GdiCaptureFallback::new(self.bounds)?;
        gdi.capture_nv12(
            self.target_width as usize,
            self.target_height as usize,
            out_nv12,
        )
    }

    fn rebuild_pipeline(&mut self, prefer_gdi: bool) -> Result<()> {
        self.duplication = None;
        self.gpu_converter = None;
        self.cpu_bgra_staging = None;
        self.gdi_fallback = None;
        self.pointer_patch_staging = None;
        self.pointer_patch_capacity = (0, 0);
        let state = build_capture_pipeline(
            &self.monitor_setting,
            self.cfg_width,
            self.cfg_height,
            self.match_source_resolution,
            prefer_gdi,
        )?;
        self.device = state.device;
        self.context = state.context;
        self.output1 = state.output1;
        self.duplication = state.duplication;
        self.gpu_converter = state.gpu_converter;
        self.cpu_bgra_staging = state.cpu_bgra_staging;
        self.gdi_fallback = state.gdi_fallback;
        self.src_width = state.src_width;
        self.src_height = state.src_height;
        self.target_width = state.target_width;
        self.target_height = state.target_height;
        self.bounds = state.bounds;
        let now = Instant::now();
        self.last_bounds_check = now;
        self.last_dxgi_retry = now;
        Ok(())
    }

    fn recreate_duplication(&mut self) -> Result<()> {
        std::thread::sleep(Duration::from_millis(75));
        if self.rebuild_pipeline(false).is_ok() {
            return Ok(());
        }
        self.rebuild_pipeline(true)
    }

    fn copy_gpu_nv12(&self, conv: &GpuColorConverter, out_nv12: &mut Vec<u8>) -> Result<()> {
        let streams = [D3D11_VIDEO_PROCESSOR_STREAM {
            Enable: true.into(),
            pInputSurface: ManuallyDrop::new(Some(conv.input_view.clone())),
            ..Default::default()
        }];
        let blt_res = unsafe {
            conv.video_context
                .VideoProcessorBlt(&conv.processor, &conv.output_view, 0, &streams)
        };
        let [stream] = streams;
        let _ = ManuallyDrop::into_inner(stream.pInputSurface);
        blt_res.map_err(|err| Key2PcError::Display(format!("VideoProcessorBlt failed: {err}")))?;

        unsafe {
            self.context
                .CopyResource(&conv.nv12_staging_tex, &conv.nv12_gpu_tex);
        }

        let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
        unsafe {
            self.context.Map(
                &conv.nv12_staging_tex,
                0,
                D3D11_MAP_READ,
                0,
                Some(&mut mapped),
            )
        }
        .map_err(|err| Key2PcError::Display(format!("Map NV12 staging failed: {err}")))?;

        let w = self.target_width as usize;
        let h = self.target_height as usize;
        let pitch = mapped.RowPitch as usize;
        let required_len = w * h * 3 / 2;
        out_nv12.resize(required_len, 0);

        let src_ptr = mapped.pData.cast::<u8>();
        let total_src_bytes = pitch * (h + h / 2);
        let src_slice = unsafe { slice::from_raw_parts(src_ptr, total_src_bytes) };

        for row in 0..h {
            let src_off = row * pitch;
            let dst_off = row * w;
            out_nv12[dst_off..dst_off + w].copy_from_slice(&src_slice[src_off..src_off + w]);
        }
        let uv_src_base = h * pitch;
        let uv_dst_base = h * w;
        for row in 0..(h / 2) {
            let src_off = uv_src_base + row * pitch;
            let dst_off = uv_dst_base + row * w;
            out_nv12[dst_off..dst_off + w].copy_from_slice(&src_slice[src_off..src_off + w]);
        }

        unsafe {
            self.context.Unmap(&conv.nv12_staging_tex, 0);
        }
        Ok(())
    }

    fn copy_cpu_bgra_to_nv12(
        &mut self,
        staging: &ID3D11Texture2D,
        out_nv12: &mut Vec<u8>,
    ) -> Result<()> {
        let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
        unsafe {
            self.context
                .Map(staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))
        }
        .map_err(|err| Key2PcError::Display(format!("Map BGRA staging failed: {err}")))?;

        let src_w = self.src_width as usize;
        let src_h = self.src_height as usize;
        let dst_w = self.target_width as usize;
        let dst_h = self.target_height as usize;
        let pitch = mapped.RowPitch as usize;
        let mapped_slice =
            unsafe { slice::from_raw_parts(mapped.pData.cast::<u8>(), pitch * src_h) };

        let bgra_src: &[u8] = if self.pointer.visible && !self.pointer.data.is_empty() {
            let tight_pitch = src_w * 4;
            self.cpu_bgra_work.resize(tight_pitch * src_h, 0);
            for row in 0..src_h {
                let s_off = row * pitch;
                let d_off = row * tight_pitch;
                self.cpu_bgra_work[d_off..d_off + tight_pitch]
                    .copy_from_slice(&mapped_slice[s_off..s_off + tight_pitch]);
            }
            let shape_desc = PointerShapeDesc {
                pointer_type: self.pointer.info.Type,
                width: self.pointer.info.Width,
                height: self.pointer.info.Height,
                pitch: self.pointer.info.Pitch,
            };
            composite_dxgi_pointer_over_bgra(
                &mut self.cpu_bgra_work,
                src_w,
                src_h,
                tight_pitch,
                0,
                0,
                self.pointer.x,
                self.pointer.y,
                &shape_desc,
                &self.pointer.data,
            );
            &self.cpu_bgra_work
        } else {
            mapped_slice
        };
        let used_pitch = if self.pointer.visible && !self.pointer.data.is_empty() {
            src_w * 4
        } else {
            pitch
        };

        out_nv12.resize(dst_w * dst_h * 3 / 2, 0);
        bgra_to_nv12_scaled(bgra_src, src_w, src_h, used_pitch, dst_w, dst_h, out_nv12);

        unsafe {
            self.context.Unmap(staging, 0);
        }
        Ok(())
    }

    fn overlay_dxgi_pointer_on_gpu(
        &mut self,
        desktop_tex: &ID3D11Texture2D,
        capture_tex: &ID3D11Texture2D,
    ) {
        if !self.pointer.visible || self.pointer.data.is_empty() {
            return;
        }
        let ptr_w = self.pointer.info.Width as i32;
        let ptr_h = self.pointer.logical_height() as i32;
        if ptr_w <= 0 || ptr_h <= 0 {
            return;
        }

        let x0 = self.pointer.x.max(0);
        let y0 = self.pointer.y.max(0);
        let x1 = (self.pointer.x + ptr_w).min(self.src_width as i32);
        let y1 = (self.pointer.y + ptr_h).min(self.src_height as i32);
        if x0 >= x1 || y0 >= y1 {
            return;
        }

        let cw = (x1 - x0) as u32;
        let ch = (y1 - y0) as u32;
        if self.pointer_patch_staging.is_none()
            || self.pointer_patch_capacity.0 < cw
            || self.pointer_patch_capacity.1 < ch
        {
            let alloc_w = cw.max(128);
            let alloc_h = ch.max(128);
            let Ok(tex) = create_texture(
                &self.device,
                alloc_w,
                alloc_h,
                DXGI_FORMAT_B8G8R8A8_UNORM,
                D3D11_USAGE_STAGING,
                0,
                D3D11_CPU_ACCESS_READ.0 as u32,
            ) else {
                return;
            };
            self.pointer_patch_staging = Some(tex);
            self.pointer_patch_capacity = (alloc_w, alloc_h);
        }

        let Some(patch_tex) = &self.pointer_patch_staging else {
            return;
        };
        let src_box = D3D11_BOX {
            left: x0 as u32,
            top: y0 as u32,
            front: 0,
            right: x1 as u32,
            bottom: y1 as u32,
            back: 1,
        };
        unsafe {
            self.context.CopySubresourceRegion(
                patch_tex,
                0,
                0,
                0,
                0,
                desktop_tex,
                0,
                Some(&src_box),
            );
        }

        let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
        if unsafe {
            self.context
                .Map(patch_tex, 0, D3D11_MAP_READ, 0, Some(&mut mapped))
        }
        .is_err()
        {
            return;
        }

        let tight_pitch = (cw as usize) * 4;
        self.pointer_scratch_bgra
            .resize(tight_pitch * (ch as usize), 0);
        let map_pitch = mapped.RowPitch as usize;
        let map_slice =
            unsafe { slice::from_raw_parts(mapped.pData.cast::<u8>(), map_pitch * (ch as usize)) };
        for row in 0..(ch as usize) {
            let s_off = row * map_pitch;
            let d_off = row * tight_pitch;
            self.pointer_scratch_bgra[d_off..d_off + tight_pitch]
                .copy_from_slice(&map_slice[s_off..s_off + tight_pitch]);
        }
        unsafe {
            self.context.Unmap(patch_tex, 0);
        }

        let shape_desc = PointerShapeDesc {
            pointer_type: self.pointer.info.Type,
            width: self.pointer.info.Width,
            height: self.pointer.info.Height,
            pitch: self.pointer.info.Pitch,
        };
        composite_dxgi_pointer_over_bgra(
            &mut self.pointer_scratch_bgra,
            cw as usize,
            ch as usize,
            tight_pitch,
            x0,
            y0,
            self.pointer.x,
            self.pointer.y,
            &shape_desc,
            &self.pointer.data,
        );

        unsafe {
            self.context.UpdateSubresource(
                capture_tex,
                0,
                Some(&src_box),
                self.pointer_scratch_bgra.as_ptr().cast(),
                tight_pitch as u32,
                0,
            );
        }
    }
}

impl ScreenCapturer for DxgiScreenCapturer {
    fn bounds(&self) -> DesktopBounds {
        self.bounds
    }

    fn target_size(&self) -> (u32, u32) {
        (self.target_width, self.target_height)
    }

    fn capture_nv12(&mut self, timeout_ms: u32, out_nv12: &mut Vec<u8>) -> Result<CaptureResult> {
        if self.last_bounds_check.elapsed() >= Duration::from_millis(750) {
            self.last_bounds_check = Instant::now();
            if let Ok(current_bounds) = query_active_monitor_bounds(&self.monitor_setting) {
                if current_bounds != self.bounds {
                    self.recreate_duplication()?;
                    return Ok(CaptureResult::Reinitialized);
                }
            }
        }

        if self.duplication.is_none() && self.last_dxgi_retry.elapsed() >= Duration::from_secs(2) {
            self.last_dxgi_retry = Instant::now();
            if let Ok(state) = build_capture_pipeline(
                &self.monitor_setting,
                self.cfg_width,
                self.cfg_height,
                self.match_source_resolution,
                false,
            ) {
                if state.duplication.is_some() {
                    self.device = state.device;
                    self.context = state.context;
                    self.output1 = state.output1;
                    self.duplication = state.duplication;
                    self.gpu_converter = state.gpu_converter;
                    self.cpu_bgra_staging = state.cpu_bgra_staging;
                    self.gdi_fallback = None;
                    self.pointer_patch_staging = None;
                    self.pointer_patch_capacity = (0, 0);
                    self.src_width = state.src_width;
                    self.src_height = state.src_height;
                    self.target_width = state.target_width;
                    self.target_height = state.target_height;
                    self.bounds = state.bounds;
                    return Ok(CaptureResult::Reinitialized);
                }
            }
        }

        if let Some(duplication) = self.duplication.clone() {
            let mut frame_info = DXGI_OUTDUPL_FRAME_INFO::default();
            let mut resource = None;
            let acquire_res =
                unsafe { duplication.AcquireNextFrame(timeout_ms, &mut frame_info, &mut resource) };

            if let Err(err) = acquire_res {
                let code = err.code();
                if code == DXGI_ERROR_WAIT_TIMEOUT {
                    return Ok(CaptureResult::Timeout);
                }
                if code == DXGI_ERROR_ACCESS_LOST
                    || code == DXGI_ERROR_INVALID_CALL
                    || code == DXGI_ERROR_DEVICE_RESET
                    || code == DXGI_ERROR_DEVICE_REMOVED
                    || code == DXGI_ERROR_NOT_CURRENTLY_AVAILABLE
                {
                    self.recreate_duplication()?;
                    return Ok(CaptureResult::Reinitialized);
                }
                return Err(Key2PcError::Display(format!(
                    "AcquireNextFrame failed: {err}"
                )));
            }

            if frame_info.LastMouseUpdateTime != 0 {
                self.pointer.visible = frame_info.PointerPosition.Visible.as_bool();
                self.pointer.x = frame_info.PointerPosition.Position.x;
                self.pointer.y = frame_info.PointerPosition.Position.y;
                if frame_info.PointerShapeBufferSize > 0 {
                    let needed = frame_info.PointerShapeBufferSize as usize;
                    if self.pointer.data.len() < needed {
                        self.pointer.data.resize(needed, 0);
                    }
                    let mut req = 0u32;
                    let mut shape_info = DXGI_OUTDUPL_POINTER_SHAPE_INFO::default();
                    if unsafe {
                        duplication.GetFramePointerShape(
                            needed as u32,
                            self.pointer.data.as_mut_ptr().cast(),
                            &mut req,
                            &mut shape_info,
                        )
                    }
                    .is_ok()
                    {
                        self.pointer.data.truncate(req as usize);
                        self.pointer.info = shape_info;
                    }
                }
            }

            let desktop_res = resource.ok_or_else(|| {
                let _ = unsafe { duplication.ReleaseFrame() };
                Key2PcError::Display("Empty desktop resource".to_string())
            })?;

            let Ok(desktop_tex) = desktop_res.cast::<ID3D11Texture2D>() else {
                let _ = unsafe { duplication.ReleaseFrame() };
                return Err(Key2PcError::Display(
                    "Frame texture cast failed".to_string(),
                ));
            };

            let mut tex_desc = D3D11_TEXTURE2D_DESC::default();
            unsafe { desktop_tex.GetDesc(&mut tex_desc) };
            if tex_desc.Width != self.src_width || tex_desc.Height != self.src_height {
                let _ = unsafe { duplication.ReleaseFrame() };
                self.recreate_duplication()?;
                return Ok(CaptureResult::Reinitialized);
            }

            let capture_tex_opt = self
                .gpu_converter
                .as_ref()
                .map(|conv| conv.capture_tex.clone());
            if let Some(capture_tex) = capture_tex_opt {
                unsafe {
                    self.context.CopyResource(&capture_tex, &desktop_tex);
                }
                self.overlay_dxgi_pointer_on_gpu(&desktop_tex, &capture_tex);
            } else if let Some(staging) = &self.cpu_bgra_staging {
                unsafe {
                    self.context.CopyResource(staging, &desktop_tex);
                }
            }
            let _ = unsafe { duplication.ReleaseFrame() };

            if let Some(conv) = &self.gpu_converter {
                self.copy_gpu_nv12(conv, out_nv12)?;
            } else if let Some(staging) = self.cpu_bgra_staging.clone() {
                self.copy_cpu_bgra_to_nv12(&staging, out_nv12)?;
            }
            return Ok(CaptureResult::Captured);
        }

        if let Some(gdi) = &mut self.gdi_fallback {
            gdi.capture_nv12(
                self.target_width as usize,
                self.target_height as usize,
                out_nv12,
            )?;
            return Ok(CaptureResult::Captured);
        }
        Err(Key2PcError::Display(
            "No screen capture backend available".to_string(),
        ))
    }
}

fn query_active_monitor_bounds(monitor_setting: &str) -> Result<DesktopBounds> {
    let outputs = collect_outputs()?;
    if outputs.is_empty() {
        return Err(Key2PcError::Display(
            "No attached desktop monitors found".to_string(),
        ));
    }
    let infos: Vec<MonitorInfo> = outputs.iter().map(|o| o.info.clone()).collect();
    let idx = select_best_monitor(monitor_setting, &infos);
    Ok(infos[idx].bounds)
}

fn build_capture_pipeline(
    monitor_setting: &str,
    cfg_w: u32,
    cfg_h: u32,
    match_source_resolution: bool,
    prefer_gdi: bool,
) -> Result<CapturePipelineState> {
    let _ = unsafe { SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
    let outputs = collect_outputs()?;
    if outputs.is_empty() {
        return Err(Key2PcError::Display(
            "No attached desktop monitors found".to_string(),
        ));
    }

    let infos: Vec<MonitorInfo> = outputs.iter().map(|o| o.info.clone()).collect();
    let idx = select_best_monitor(monitor_setting, &infos);
    let selected = outputs.into_iter().nth(idx).unwrap();
    let bounds = selected.info.bounds;
    let src_w = bounds.width as u32;
    let src_h = bounds.height as u32;
    let (dst_w, dst_h) =
        compute_encode_dimensions(src_w, src_h, cfg_w, cfg_h, match_source_resolution);

    let (device, context) = create_adapter_device(&selected.adapter)?;
    if let Ok(mt) = device.cast::<ID3D11Multithread>() {
        let _ = unsafe { mt.SetMultithreadProtected(true) };
    }

    let output1: IDXGIOutput1 = selected
        .output
        .cast()
        .map_err(|err| Key2PcError::Display(format!("IDXGIOutput1 cast failed: {err}")))?;

    let duplication = if prefer_gdi {
        None
    } else {
        try_duplicate_with_brief_retry(&output1, &device)
    };
    let (gpu_converter, cpu_bgra_staging, gdi_fallback) = if duplication.is_some() {
        let conv = try_create_gpu_converter(&device, &context, src_w, src_h, dst_w, dst_h).ok();
        let staging = if conv.is_none() {
            Some(create_texture(
                &device,
                src_w,
                src_h,
                DXGI_FORMAT_B8G8R8A8_UNORM,
                D3D11_USAGE_STAGING,
                0,
                D3D11_CPU_ACCESS_READ.0 as u32,
            )?)
        } else {
            None
        };
        (conv, staging, None)
    } else {
        (None, None, Some(GdiCaptureFallback::new(bounds)?))
    };

    Ok(CapturePipelineState {
        device,
        context,
        output1,
        duplication,
        gpu_converter,
        cpu_bgra_staging,
        gdi_fallback,
        src_width: src_w,
        src_height: src_h,
        target_width: dst_w,
        target_height: dst_h,
        bounds,
    })
}

fn draw_gdi_cursor(mem_dc: HDC, bounds: DesktopBounds) {
    let mut ci = CURSORINFO {
        cbSize: std::mem::size_of::<CURSORINFO>() as u32,
        ..Default::default()
    };
    if unsafe { GetCursorInfo(&mut ci) }.is_err() {
        return;
    }
    if (ci.flags.0 & CURSOR_SHOWING.0) == 0 || ci.hCursor.is_invalid() {
        return;
    }
    let mut ii = ICONINFO::default();
    let mut hx = 0i32;
    let mut hy = 0i32;
    let icon = HICON(ci.hCursor.0);
    if unsafe { GetIconInfo(icon, &mut ii) }.is_ok() {
        hx = ii.xHotspot as i32;
        hy = ii.yHotspot as i32;
        if !ii.hbmMask.is_invalid() {
            unsafe {
                let _ = DeleteObject(HGDIOBJ(ii.hbmMask.0));
            }
        }
        if !ii.hbmColor.is_invalid() {
            unsafe {
                let _ = DeleteObject(HGDIOBJ(ii.hbmColor.0));
            }
        }
    }
    let rel_x = ci.ptScreenPos.x - bounds.left - hx;
    let rel_y = ci.ptScreenPos.y - bounds.top - hy;
    if rel_x + 128 < 0 || rel_y + 128 < 0 || rel_x >= bounds.width || rel_y >= bounds.height {
        return;
    }
    unsafe {
        let _ = DrawIconEx(mem_dc, rel_x, rel_y, icon, 0, 0, 0, None, DI_NORMAL);
    }
}

#[allow(clippy::too_many_arguments)]
pub fn composite_dxgi_pointer_over_bgra(
    dst_bgra: &mut [u8],
    dst_w: usize,
    dst_h: usize,
    dst_pitch: usize,
    region_x: i32,
    region_y: i32,
    ptr_x: i32,
    ptr_y: i32,
    shape: &PointerShapeDesc,
    shape_data: &[u8],
) {
    let shape_w = shape.width as i32;
    let logical_h = if shape.pointer_type == DXGI_OUTDUPL_POINTER_SHAPE_TYPE_MONOCHROME.0 as u32 {
        (shape.height / 2) as i32
    } else {
        shape.height as i32
    };
    let shape_pitch = shape.pitch as usize;
    if shape_w <= 0 || logical_h <= 0 || shape_pitch == 0 {
        return;
    }

    for sy in 0..logical_h {
        let dy = ptr_y + sy - region_y;
        if dy < 0 || (dy as usize) >= dst_h {
            continue;
        }
        let dst_row_off = (dy as usize) * dst_pitch;
        for sx in 0..shape_w {
            let dx = ptr_x + sx - region_x;
            if dx < 0 || (dx as usize) >= dst_w {
                continue;
            }
            let dp = dst_row_off + (dx as usize) * 4;
            if dp + 3 >= dst_bgra.len() {
                continue;
            }

            if shape.pointer_type == DXGI_OUTDUPL_POINTER_SHAPE_TYPE_MONOCHROME.0 as u32 {
                let and_off = (sy as usize) * shape_pitch + (sx as usize) / 8;
                let xor_off = ((sy + logical_h) as usize) * shape_pitch + (sx as usize) / 8;
                if xor_off >= shape_data.len() {
                    continue;
                }
                let mask = 0x80u8 >> ((sx as usize) & 7);
                let and_set = (shape_data[and_off] & mask) != 0;
                let xor_set = (shape_data[xor_off] & mask) != 0;
                match (and_set, xor_set) {
                    (false, false) => {
                        dst_bgra[dp] = 0;
                        dst_bgra[dp + 1] = 0;
                        dst_bgra[dp + 2] = 0;
                        dst_bgra[dp + 3] = 255;
                    }
                    (false, true) => {
                        dst_bgra[dp] = 255;
                        dst_bgra[dp + 1] = 255;
                        dst_bgra[dp + 2] = 255;
                        dst_bgra[dp + 3] = 255;
                    }
                    (true, true) => {
                        dst_bgra[dp] ^= 0xFF;
                        dst_bgra[dp + 1] ^= 0xFF;
                        dst_bgra[dp + 2] ^= 0xFF;
                    }
                    (true, false) => {}
                }
            } else if shape.pointer_type == DXGI_OUTDUPL_POINTER_SHAPE_TYPE_COLOR.0 as u32 {
                let sp = (sy as usize) * shape_pitch + (sx as usize) * 4;
                if sp + 3 >= shape_data.len() {
                    continue;
                }
                let a = u16::from(shape_data[sp + 3]);
                if a == 0 {
                    continue;
                }
                let inv_a = 255 - a;
                for c in 0..3 {
                    let s_val = u16::from(shape_data[sp + c]);
                    let d_val = u16::from(dst_bgra[dp + c]);
                    dst_bgra[dp + c] = (s_val + ((d_val * inv_a + 127) / 255)).min(255) as u8;
                }
                dst_bgra[dp + 3] = 255;
            } else if shape.pointer_type == DXGI_OUTDUPL_POINTER_SHAPE_TYPE_MASKED_COLOR.0 as u32 {
                let sp = (sy as usize) * shape_pitch + (sx as usize) * 4;
                if sp + 3 >= shape_data.len() {
                    continue;
                }
                if shape_data[sp + 3] != 0 {
                    dst_bgra[dp] ^= shape_data[sp];
                    dst_bgra[dp + 1] ^= shape_data[sp + 1];
                    dst_bgra[dp + 2] ^= shape_data[sp + 2];
                } else {
                    dst_bgra[dp] = shape_data[sp];
                    dst_bgra[dp + 1] = shape_data[sp + 1];
                    dst_bgra[dp + 2] = shape_data[sp + 2];
                    dst_bgra[dp + 3] = 255;
                }
            }
        }
    }
}

fn try_duplicate_with_brief_retry(
    output1: &IDXGIOutput1,
    device: &ID3D11Device,
) -> Option<IDXGIOutputDuplication> {
    for attempt in 0..3 {
        match unsafe { output1.DuplicateOutput(device) } {
            Ok(dup) => return Some(dup),
            Err(err) => {
                let code = err.code();
                if (code == DXGI_ERROR_NOT_CURRENTLY_AVAILABLE || code == DXGI_ERROR_ACCESS_LOST)
                    && attempt < 2
                {
                    std::thread::sleep(Duration::from_millis(60));
                    continue;
                }
                return None;
            }
        }
    }
    None
}

fn create_adapter_device(adapter: &IDXGIAdapter1) -> Result<(ID3D11Device, ID3D11DeviceContext)> {
    let attempts = [
        D3D11_CREATE_DEVICE_BGRA_SUPPORT | D3D11_CREATE_DEVICE_VIDEO_SUPPORT,
        D3D11_CREATE_DEVICE_BGRA_SUPPORT,
    ];
    let mut last_err = String::from("D3D11CreateDevice failed");
    for flags in attempts {
        let mut device: Option<ID3D11Device> = None;
        let mut context: Option<ID3D11DeviceContext> = None;
        match unsafe {
            D3D11CreateDevice(
                adapter,
                D3D_DRIVER_TYPE_UNKNOWN,
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
        "D3D11CreateDevice failed: {last_err}"
    )))
}

fn try_create_gpu_converter(
    device: &ID3D11Device,
    context: &ID3D11DeviceContext,
    src_w: u32,
    src_h: u32,
    dst_w: u32,
    dst_h: u32,
) -> Result<GpuColorConverter> {
    let video_device: ID3D11VideoDevice = device.cast()?;
    let video_context: ID3D11VideoContext = context.cast()?;

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
        OutputWidth: dst_w,
        OutputHeight: dst_h,
        Usage: D3D11_VIDEO_USAGE_PLAYBACK_NORMAL,
    };

    let video_enum = unsafe { video_device.CreateVideoProcessorEnumerator(&content_desc) }?;
    let processor = unsafe { video_device.CreateVideoProcessor(&video_enum, 0) }?;

    let src_rect = RECT {
        left: 0,
        top: 0,
        right: src_w as i32,
        bottom: src_h as i32,
    };
    let dst_rect = RECT {
        left: 0,
        top: 0,
        right: dst_w as i32,
        bottom: dst_h as i32,
    };
    unsafe {
        video_context.VideoProcessorSetStreamSourceRect(&processor, 0, true, Some(&src_rect));
        video_context.VideoProcessorSetStreamDestRect(&processor, 0, true, Some(&dst_rect));
        video_context.VideoProcessorSetOutputTargetRect(&processor, true, Some(&dst_rect));
    }

    let (capture_tex, input_view) =
        create_input_tex_and_view(device, &video_device, &video_enum, src_w, src_h)?;

    let nv12_gpu_tex = create_texture(
        device,
        dst_w,
        dst_h,
        DXGI_FORMAT_NV12,
        D3D11_USAGE_DEFAULT,
        D3D11_BIND_RENDER_TARGET.0 as u32,
        0,
    )?;
    let output_view_desc = D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC {
        ViewDimension: D3D11_VPOV_DIMENSION_TEXTURE2D,
        Anonymous: D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC_0 {
            Texture2D: Default::default(),
        },
    };
    let mut output_view: Option<ID3D11VideoProcessorOutputView> = None;
    unsafe {
        video_device.CreateVideoProcessorOutputView(
            &nv12_gpu_tex,
            &video_enum,
            &output_view_desc,
            Some(&mut output_view),
        )
    }?;

    let nv12_staging_tex = create_texture(
        device,
        dst_w,
        dst_h,
        DXGI_FORMAT_NV12,
        D3D11_USAGE_STAGING,
        0,
        D3D11_CPU_ACCESS_READ.0 as u32,
    )?;

    Ok(GpuColorConverter {
        _video_device: video_device,
        video_context,
        _video_enum: video_enum,
        processor,
        capture_tex,
        input_view,
        nv12_gpu_tex,
        output_view: output_view.unwrap(),
        nv12_staging_tex,
    })
}

fn create_input_tex_and_view(
    device: &ID3D11Device,
    video_device: &ID3D11VideoDevice,
    video_enum: &ID3D11VideoProcessorEnumerator,
    width: u32,
    height: u32,
) -> Result<(ID3D11Texture2D, ID3D11VideoProcessorInputView)> {
    let bind_candidates = [0u32, D3D11_BIND_RENDER_TARGET.0 as u32];
    let mut last_err = None;

    for bind_flags in bind_candidates {
        let tex = match create_texture(
            device,
            width,
            height,
            DXGI_FORMAT_B8G8R8A8_UNORM,
            D3D11_USAGE_DEFAULT,
            bind_flags,
            0,
        ) {
            Ok(t) => t,
            Err(err) => {
                last_err = Some(err);
                continue;
            }
        };
        let desc = D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC {
            FourCC: 0,
            ViewDimension: D3D11_VPIV_DIMENSION_TEXTURE2D,
            Anonymous: D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC_0 {
                Texture2D: Default::default(),
            },
        };
        let mut view: Option<ID3D11VideoProcessorInputView> = None;
        match unsafe {
            video_device.CreateVideoProcessorInputView(&tex, video_enum, &desc, Some(&mut view))
        } {
            Ok(()) => return Ok((tex, view.unwrap())),
            Err(err) => last_err = Some(err.into()),
        }
    }

    Err(last_err.unwrap_or_else(|| {
        Key2PcError::Display("CreateVideoProcessorInputView failed".to_string())
    }))
}

fn create_texture(
    device: &ID3D11Device,
    width: u32,
    height: u32,
    format: windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT,
    usage: windows::Win32::Graphics::Direct3D11::D3D11_USAGE,
    bind_flags: u32,
    cpu_access_flags: u32,
) -> Result<ID3D11Texture2D> {
    let desc = D3D11_TEXTURE2D_DESC {
        Width: width,
        Height: height,
        MipLevels: 1,
        ArraySize: 1,
        Format: format,
        SampleDesc: DXGI_SAMPLE_DESC {
            Count: 1,
            Quality: 0,
        },
        Usage: usage,
        BindFlags: bind_flags,
        CPUAccessFlags: cpu_access_flags,
        MiscFlags: 0,
    };
    let mut tex: Option<ID3D11Texture2D> = None;
    unsafe { device.CreateTexture2D(&desc, None, Some(&mut tex)) }
        .map_err(|err| Key2PcError::Display(format!("CreateTexture2D failed: {err}")))?;
    Ok(tex.unwrap())
}

fn bgra_to_nv12_scaled(
    src: &[u8],
    src_w: usize,
    src_h: usize,
    src_pitch: usize,
    dst_w: usize,
    dst_h: usize,
    out_nv12: &mut [u8],
) {
    let uv_base = dst_w * dst_h;
    let (y_plane, uv_plane) = out_nv12.split_at_mut(uv_base);
    for dy in 0..dst_h {
        let sy = (dy * src_h) / dst_h;
        let src_row = &src[sy * src_pitch..sy * src_pitch + src_w * 4];
        let y_row = &mut y_plane[dy * dst_w..(dy + 1) * dst_w];
        let compute_uv = (dy & 1) == 0;
        let uv_row_off = (dy / 2) * dst_w;

        for dx in 0..dst_w {
            let sx = (dx * src_w) / dst_w;
            let px = sx * 4;
            let b = i32::from(src_row[px]);
            let g = i32::from(src_row[px + 1]);
            let r = i32::from(src_row[px + 2]);

            let y = ((66 * r + 129 * g + 25 * b + 128) >> 8) + 16;
            y_row[dx] = y.clamp(16, 235) as u8;

            if compute_uv && (dx & 1) == 0 {
                let u = ((-38 * r - 74 * g + 112 * b + 128) >> 8) + 128;
                let v = ((112 * r - 94 * g - 18 * b + 128) >> 8) + 128;
                uv_plane[uv_row_off + dx] = u.clamp(16, 240) as u8;
                uv_plane[uv_row_off + dx + 1] = v.clamp(16, 240) as u8;
            }
        }
    }
}
