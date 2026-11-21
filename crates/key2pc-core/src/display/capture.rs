use std::slice;

use windows::core::Interface;
use windows::Win32::Foundation::RECT;
use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_UNKNOWN;
use windows::Win32::Graphics::Direct3D11::{
    D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext, ID3D11Multithread, ID3D11Texture2D,
    ID3D11VideoContext, ID3D11VideoDevice, ID3D11VideoProcessor, ID3D11VideoProcessorEnumerator,
    ID3D11VideoProcessorInputView, ID3D11VideoProcessorOutputView, D3D11_BIND_RENDER_TARGET,
    D3D11_BIND_SHADER_RESOURCE, D3D11_CPU_ACCESS_READ, D3D11_CREATE_DEVICE_BGRA_SUPPORT,
    D3D11_CREATE_DEVICE_VIDEO_SUPPORT, D3D11_MAP_READ, D3D11_MAPPED_SUBRESOURCE,
    D3D11_SDK_VERSION, D3D11_TEXTURE2D_DESC, D3D11_USAGE_DEFAULT, D3D11_USAGE_STAGING,
    D3D11_VIDEO_FRAME_FORMAT_PROGRESSIVE, D3D11_VIDEO_PROCESSOR_CONTENT_DESC,
    D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC, D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC_0,
    D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC, D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC_0,
    D3D11_VIDEO_PROCESSOR_STREAM, D3D11_VIDEO_USAGE_PLAYBACK_NORMAL, D3D11_VPIV_DIMENSION_TEXTURE2D,
    D3D11_VPOV_DIMENSION_TEXTURE2D,
};
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_FORMAT_NV12, DXGI_RATIONAL, DXGI_SAMPLE_DESC,
};
use windows::Win32::Graphics::Dxgi::{
    CreateDXGIFactory1, IDXGIAdapter1, IDXGIFactory1, IDXGIOutput, IDXGIOutput1,
    IDXGIOutputDuplication, DXGI_ERROR_ACCESS_LOST, DXGI_ERROR_WAIT_TIMEOUT, DXGI_OUTDUPL_FRAME_INFO,
};

use crate::display::{select_monitor_index, CaptureResult, MonitorInfo, ScreenCapturer};
use crate::error::{Key2PcError, Result};
use crate::input::DesktopBounds;

struct SelectedOutput {
    adapter: IDXGIAdapter1,
    output: IDXGIOutput,
    info: MonitorInfo,
}

pub fn enumerate_monitors() -> Result<Vec<MonitorInfo>> {
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
            let name_len = desc
                .DeviceName
                .iter()
                .position(|&c| c == 0)
                .unwrap_or(desc.DeviceName.len());
            let name = String::from_utf16_lossy(&desc.DeviceName[..name_len]);
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
                },
            });
        }
    }

    Ok(result)
}

pub struct DxgiScreenCapturer {
    device: ID3D11Device,
    context: ID3D11DeviceContext,
    video_device: ID3D11VideoDevice,
    video_context: ID3D11VideoContext,
    video_enum: ID3D11VideoProcessorEnumerator,
    processor: ID3D11VideoProcessor,
    output1: IDXGIOutput1,
    duplication: IDXGIOutputDuplication,
    capture_tex: ID3D11Texture2D,
    input_view: ID3D11VideoProcessorInputView,
    nv12_gpu_tex: ID3D11Texture2D,
    output_view: ID3D11VideoProcessorOutputView,
    nv12_staging_tex: ID3D11Texture2D,
    bounds: DesktopBounds,
    target_width: u32,
    target_height: u32,
}

impl DxgiScreenCapturer {
    pub fn new(monitor_setting: &str, target_width: u32, target_height: u32) -> Result<Self> {
        let outputs = collect_outputs()?;
        if outputs.is_empty() {
            return Err(Key2PcError::Display(
                "No attached desktop monitors found".to_string(),
            ));
        }

        let idx = select_monitor_index(monitor_setting, outputs.len());
        let selected = outputs.into_iter().nth(idx).unwrap();
        let bounds = selected.info.bounds;
        let src_w = bounds.width as u32;
        let src_h = bounds.height as u32;
        let dst_w = target_width & !1;
        let dst_h = target_height & !1;

        let mut device: Option<ID3D11Device> = None;
        let mut context: Option<ID3D11DeviceContext> = None;
        unsafe {
            D3D11CreateDevice(
                &selected.adapter,
                D3D_DRIVER_TYPE_UNKNOWN,
                Default::default(),
                D3D11_CREATE_DEVICE_BGRA_SUPPORT | D3D11_CREATE_DEVICE_VIDEO_SUPPORT,
                None,
                D3D11_SDK_VERSION,
                Some(&mut device),
                None,
                Some(&mut context),
            )
        }
        .map_err(|err| Key2PcError::Display(format!("D3D11CreateDevice failed: {err}")))?;

        let device = device.unwrap();
        let context = context.unwrap();
        if let Ok(mt) = device.cast::<ID3D11Multithread>() {
            let _ = unsafe { mt.SetMultithreadProtected(true) };
        }

        let video_device: ID3D11VideoDevice = device
            .cast()
            .map_err(|err| Key2PcError::Display(format!("ID3D11VideoDevice unavailable: {err}")))?;
        let video_context: ID3D11VideoContext = context
            .cast()
            .map_err(|err| Key2PcError::Display(format!("ID3D11VideoContext unavailable: {err}")))?;

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

        let video_enum = unsafe { video_device.CreateVideoProcessorEnumerator(&content_desc) }
            .map_err(|err| {
                Key2PcError::Display(format!("CreateVideoProcessorEnumerator failed: {err}"))
            })?;
        let processor = unsafe { video_device.CreateVideoProcessor(&video_enum, 0) }
            .map_err(|err| Key2PcError::Display(format!("CreateVideoProcessor failed: {err}")))?;

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

        let capture_tex = create_texture(
            &device,
            src_w,
            src_h,
            DXGI_FORMAT_B8G8R8A8_UNORM,
            D3D11_USAGE_DEFAULT,
            D3D11_BIND_SHADER_RESOURCE.0 as u32,
            0,
        )?;
        let input_view_desc = D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC {
            FourCC: 0,
            ViewDimension: D3D11_VPIV_DIMENSION_TEXTURE2D,
            Anonymous: D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC_0 {
                Texture2D: Default::default(),
            },
        };
        let mut input_view: Option<ID3D11VideoProcessorInputView> = None;
        unsafe {
            video_device.CreateVideoProcessorInputView(
                &capture_tex,
                &video_enum,
                &input_view_desc,
                Some(&mut input_view),
            )
        }
        .map_err(|err| Key2PcError::Display(format!("CreateVideoProcessorInputView failed: {err}")))?;

        let nv12_gpu_tex = create_texture(
            &device,
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
        }
        .map_err(|err| {
            Key2PcError::Display(format!("CreateVideoProcessorOutputView failed: {err}"))
        })?;

        let nv12_staging_tex = create_texture(
            &device,
            dst_w,
            dst_h,
            DXGI_FORMAT_NV12,
            D3D11_USAGE_STAGING,
            0,
            D3D11_CPU_ACCESS_READ.0 as u32,
        )?;

        let output1: IDXGIOutput1 = selected
            .output
            .cast()
            .map_err(|err| Key2PcError::Display(format!("IDXGIOutput1 cast failed: {err}")))?;
        let duplication = unsafe { output1.DuplicateOutput(&device) }
            .map_err(|err| Key2PcError::Display(format!("DuplicateOutput failed: {err}")))?;

        Ok(Self {
            device,
            context,
            video_device,
            video_context,
            video_enum,
            processor,
            output1,
            duplication,
            capture_tex,
            input_view: input_view.unwrap(),
            nv12_gpu_tex,
            output_view: output_view.unwrap(),
            nv12_staging_tex,
            bounds,
            target_width: dst_w,
            target_height: dst_h,
        })
    }

    fn recreate_duplication(&mut self) -> Result<()> {
        self.duplication = unsafe { self.output1.DuplicateOutput(&self.device) }.map_err(|err| {
            Key2PcError::Display(format!("DuplicateOutput reinit failed: {err}"))
        })?;
        let _ = (&self.video_device, &self.video_enum);
        Ok(())
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
        let mut frame_info = DXGI_OUTDUPL_FRAME_INFO::default();
        let mut resource = None;
        let acquire_res = unsafe {
            self.duplication
                .AcquireNextFrame(timeout_ms, &mut frame_info, &mut resource)
        };

        if let Err(err) = acquire_res {
            if err.code() == DXGI_ERROR_WAIT_TIMEOUT {
                return Ok(CaptureResult::Timeout);
            }
            if err.code() == DXGI_ERROR_ACCESS_LOST {
                self.recreate_duplication()?;
                return Ok(CaptureResult::Reinitialized);
            }
            return Err(Key2PcError::Display(format!("AcquireNextFrame failed: {err}")));
        }

        let desktop_res = resource.ok_or_else(|| {
            let _ = unsafe { self.duplication.ReleaseFrame() };
            Key2PcError::Display("Empty desktop resource".to_string())
        })?;

        let copy_res = desktop_res.cast::<ID3D11Texture2D>().map(|desktop_tex| unsafe {
            self.context.CopyResource(&self.capture_tex, &desktop_tex);
        });
        let _ = unsafe { self.duplication.ReleaseFrame() };
        copy_res.map_err(|err| Key2PcError::Display(format!("Frame texture cast failed: {err}")))?;

        let stream = D3D11_VIDEO_PROCESSOR_STREAM {
            Enable: true.into(),
            pInputSurface: std::mem::ManuallyDrop::new(Some(self.input_view.clone())),
            ..Default::default()
        };
        let blt_res = unsafe {
            self.video_context
                .VideoProcessorBlt(&self.processor, &self.output_view, 0, &[stream])
        };
        blt_res.map_err(|err| Key2PcError::Display(format!("VideoProcessorBlt failed: {err}")))?;

        unsafe {
            self.context
                .CopyResource(&self.nv12_staging_tex, &self.nv12_gpu_tex);
        }

        let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
        unsafe {
            self.context
                .Map(&self.nv12_staging_tex, 0, D3D11_MAP_READ, 0, Some(&mut mapped))
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
            self.context.Unmap(&self.nv12_staging_tex, 0);
        }
        Ok(CaptureResult::Captured)
    }
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
