use std::mem::ManuallyDrop;
use std::ptr;
use std::slice;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use windows::core::{Interface, GUID};
use windows::Win32::Foundation::{RPC_E_CHANGED_MODE, S_FALSE, S_OK};
use windows::Win32::Media::MediaFoundation::{
    eAVEncCommonRateControlMode_CBR, eAVEncCommonRateControlMode_LowDelayVBR,
    eAVEncH264VProfile_Main, ICodecAPI, IMFActivate, IMFMediaEvent, IMFMediaEventGenerator,
    IMFMediaType, IMFSample, IMFTransform, MEError, MFCreateMediaType, MFCreateMemoryBuffer,
    MFCreateSample, MFMediaType_Video, MFShutdown, MFStartup, MFTEnumEx, MFVideoFormat_H264,
    MFVideoFormat_NV12, CODECAPI_AVEncCommonMeanBitRate, CODECAPI_AVEncCommonRateControlMode,
    CODECAPI_AVEncMPVGOPSize, CODECAPI_AVEncVideoForceKeyFrame, METransformHaveOutput,
    METransformNeedInput, MFT_CATEGORY_VIDEO_DECODER, MFT_CATEGORY_VIDEO_ENCODER,
    MFT_ENUM_FLAG, MFT_ENUM_FLAG_ASYNCMFT, MFT_ENUM_FLAG_HARDWARE, MFT_ENUM_FLAG_SORTANDFILTER,
    MFT_ENUM_FLAG_SYNCMFT, MFT_MESSAGE_NOTIFY_BEGIN_STREAMING, MFT_MESSAGE_NOTIFY_START_OF_STREAM,
    MFT_OUTPUT_DATA_BUFFER, MFT_OUTPUT_STREAM_PROVIDES_SAMPLES, MFT_REGISTER_TYPE_INFO,
    MFSampleExtension_CleanPoint, MF_EVENT_FLAG_NO_WAIT, MF_E_NO_EVENTS_AVAILABLE,
    MF_E_TRANSFORM_NEED_MORE_INPUT, MF_E_TRANSFORM_STREAM_CHANGE, MF_LOW_LATENCY,
    MF_MT_AVG_BITRATE, MF_MT_DEFAULT_STRIDE, MF_MT_FRAME_RATE, MF_MT_FRAME_SIZE,
    MF_MT_INTERLACE_MODE, MF_MT_MAJOR_TYPE, MF_MT_MPEG2_PROFILE, MF_MT_SUBTYPE,
    MF_TRANSFORM_ASYNC, MF_TRANSFORM_ASYNC_UNLOCK, MF_VERSION,
};
use windows::Win32::System::Com::{
    CoInitializeEx, CoTaskMemFree, CoUninitialize, COINIT_MULTITHREADED,
};
use windows::Win32::System::Variant::{VARIANT, VT_UI4};

use crate::display::{DecodedFrame, EncodedFrame, VideoDecoder, VideoEncoder};
use crate::error::{Key2PcError, Result};

struct MfSessionGuard {
    com_initialized: bool,
    mf_started: bool,
}

impl MfSessionGuard {
    fn acquire() -> Result<Self> {
        let hr = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
        let com_initialized = hr == S_OK || hr == S_FALSE;
        if !com_initialized && hr != RPC_E_CHANGED_MODE {
            return Err(Key2PcError::Display(format!("CoInitializeEx failed: {hr:?}")));
        }
        unsafe { MFStartup(MF_VERSION, 0) }
            .map_err(|err| Key2PcError::Display(format!("MFStartup failed: {err}")))?;
        Ok(Self {
            com_initialized,
            mf_started: true,
        })
    }
}

impl Drop for MfSessionGuard {
    fn drop(&mut self) {
        if self.mf_started {
            let _ = unsafe { MFShutdown() };
        }
        if self.com_initialized {
            unsafe { CoUninitialize() };
        }
    }
}

fn pack_u64(hi: u32, lo: u32) -> u64 {
    (u64::from(hi) << 32) | u64::from(lo)
}

fn unpack_u64(val: u64) -> (u32, u32) {
    ((val >> 32) as u32, (val & 0xFFFF_FFFF) as u32)
}

fn variant_u32(val: u32) -> VARIANT {
    let mut variant = VARIANT::default();
    unsafe {
        (*variant.Anonymous.Anonymous).vt = VT_UI4;
        (*variant.Anonymous.Anonymous).Anonymous.ulVal = val;
    }
    variant
}

fn build_video_media_type(
    subtype: GUID,
    width: u32,
    height: u32,
    fps: u32,
    bitrate_bps: Option<u32>,
    h264_profile: Option<u32>,
) -> Result<IMFMediaType> {
    let media_type = unsafe { MFCreateMediaType() }
        .map_err(|err| Key2PcError::Display(format!("MFCreateMediaType failed: {err}")))?;
    unsafe {
        media_type.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
        media_type.SetGUID(&MF_MT_SUBTYPE, &subtype)?;
        media_type.SetUINT32(&MF_MT_INTERLACE_MODE, 2)?;
        media_type.SetUINT64(&MF_MT_FRAME_SIZE, pack_u64(width, height))?;
        media_type.SetUINT64(&MF_MT_FRAME_RATE, pack_u64(fps, 1))?;
        if let Some(bitrate) = bitrate_bps {
            media_type.SetUINT32(&MF_MT_AVG_BITRATE, bitrate)?;
        }
        if let Some(profile) = h264_profile {
            media_type.SetUINT32(&MF_MT_MPEG2_PROFILE, profile)?;
        }
    }
    Ok(media_type)
}

fn create_sample_from_bytes(bytes: &[u8], pts_100ns: i64, duration_100ns: i64) -> Result<IMFSample> {
    let sample = unsafe { MFCreateSample() }
        .map_err(|err| Key2PcError::Display(format!("MFCreateSample failed: {err}")))?;
    let buffer = unsafe { MFCreateMemoryBuffer(bytes.len() as u32) }
        .map_err(|err| Key2PcError::Display(format!("MFCreateMemoryBuffer failed: {err}")))?;

    let mut dst_ptr: *mut u8 = ptr::null_mut();
    unsafe { buffer.Lock(&mut dst_ptr, None, None) }
        .map_err(|err| Key2PcError::Display(format!("IMFMediaBuffer::Lock failed: {err}")))?;
    unsafe {
        ptr::copy_nonoverlapping(bytes.as_ptr(), dst_ptr, bytes.len());
        let _ = buffer.Unlock();
        buffer.SetCurrentLength(bytes.len() as u32)?;
        sample.AddBuffer(&buffer)?;
        sample.SetSampleTime(pts_100ns)?;
        sample.SetSampleDuration(duration_100ns)?;
    }
    Ok(sample)
}

fn enumerate_mft_candidates(
    category: GUID,
    flags: MFT_ENUM_FLAG,
    input_subtype: Option<GUID>,
    output_subtype: Option<GUID>,
) -> Result<Vec<IMFActivate>> {
    let in_info = input_subtype.map(|sub| MFT_REGISTER_TYPE_INFO {
        guidMajorType: MFMediaType_Video,
        guidSubtype: sub,
    });
    let out_info = output_subtype.map(|sub| MFT_REGISTER_TYPE_INFO {
        guidMajorType: MFMediaType_Video,
        guidSubtype: sub,
    });
    let mut raw_list: *mut Option<IMFActivate> = ptr::null_mut();
    let mut count = 0u32;
    unsafe {
        MFTEnumEx(
            category,
            flags,
            in_info.as_ref().map(|r| r as *const _),
            out_info.as_ref().map(|r| r as *const _),
            &mut raw_list,
            &mut count,
        )
    }
    .map_err(|err| Key2PcError::Display(format!("MFTEnumEx failed: {err}")))?;

    let mut activates = Vec::new();
    if !raw_list.is_null() && count > 0 {
        let entries = unsafe { slice::from_raw_parts_mut(raw_list, count as usize) };
        for entry in entries {
            if let Some(act) = entry.take() {
                activates.push(act);
            }
        }
        unsafe { CoTaskMemFree(Some(raw_list.cast())) };
    }
    Ok(activates)
}

fn unlock_async_if_needed(transform: &IMFTransform) -> bool {
    let Ok(attrs) = (unsafe { transform.GetAttributes() }) else {
        return false;
    };
    let is_async = unsafe { attrs.GetUINT32(&MF_TRANSFORM_ASYNC) }.unwrap_or(0) != 0;
    if is_async {
        let _ = unsafe { attrs.SetUINT32(&MF_TRANSFORM_ASYNC_UNLOCK, 1) };
    }
    let _ = unsafe { attrs.SetUINT32(&MF_LOW_LATENCY, 1) };
    is_async
}

fn pull_output_sample(
    transform: &IMFTransform,
    provides_samples: bool,
    alloc_size: u32,
) -> std::result::Result<Option<IMFSample>, windows::core::Error> {
    let mut out_buf = MFT_OUTPUT_DATA_BUFFER::default();
    if !provides_samples {
        let sample = unsafe { MFCreateSample()? };
        let buffer = unsafe { MFCreateMemoryBuffer(alloc_size)? };
        unsafe { sample.AddBuffer(&buffer)? };
        out_buf.pSample = ManuallyDrop::new(Some(sample));
    }

    let mut status = 0u32;
    let res = unsafe { transform.ProcessOutput(0, slice::from_mut(&mut out_buf), &mut status) };
    let sample = ManuallyDrop::into_inner(out_buf.pSample);
    let _events = ManuallyDrop::into_inner(out_buf.pEvents);

    match res {
        Ok(()) => Ok(sample),
        Err(err) if err.code() == MF_E_TRANSFORM_NEED_MORE_INPUT => Ok(None),
        Err(err) => Err(err),
    }
}

fn poll_next_mft_event(
    event_gen: &IMFMediaEventGenerator,
    deadline: Instant,
) -> Result<Option<IMFMediaEvent>> {
    loop {
        match unsafe { event_gen.GetEvent(MF_EVENT_FLAG_NO_WAIT) } {
            Ok(ev) => return Ok(Some(ev)),
            Err(err) if err.code() == MF_E_NO_EVENTS_AVAILABLE => {
                if Instant::now() >= deadline {
                    return Ok(None);
                }
                thread::sleep(Duration::from_micros(500));
            }
            Err(err) => {
                return Err(Key2PcError::Display(format!("GetEvent failed: {err}")));
            }
        }
    }
}

pub struct MfH264Encoder {
    transform: IMFTransform,
    event_gen: Option<IMFMediaEventGenerator>,
    pending_inputs: u32,
    provides_samples: bool,
    output_buf_size: u32,
    width: u32,
    height: u32,
    frame_duration_100ns: i64,
    force_idr: AtomicBool,
    _guard: MfSessionGuard,
}

unsafe impl Send for MfH264Encoder {}

impl MfH264Encoder {
    pub fn new(width: u32, height: u32, fps: u32, bitrate_kbps: u32) -> Result<Self> {
        let guard = MfSessionGuard::acquire()?;
        let bitrate_bps = bitrate_kbps.saturating_mul(1000);
        let mut candidates = enumerate_mft_candidates(
            MFT_CATEGORY_VIDEO_ENCODER,
            MFT_ENUM_FLAG_HARDWARE | MFT_ENUM_FLAG_ASYNCMFT | MFT_ENUM_FLAG_SORTANDFILTER,
            Some(MFVideoFormat_NV12),
            Some(MFVideoFormat_H264),
        )?;
        let sw_candidates = enumerate_mft_candidates(
            MFT_CATEGORY_VIDEO_ENCODER,
            MFT_ENUM_FLAG_SYNCMFT | MFT_ENUM_FLAG_ASYNCMFT | MFT_ENUM_FLAG_SORTANDFILTER,
            Some(MFVideoFormat_NV12),
            Some(MFVideoFormat_H264),
        )?;
        candidates.extend(sw_candidates);

        let mut last_err = String::from("No H.264 encoder MFT available");
        for activate in candidates {
            match Self::try_init_candidate(activate, width, height, fps, bitrate_bps) {
                Ok((transform, is_async, provides_samples, output_buf_size)) => {
                    let event_gen = if is_async {
                        transform.cast::<IMFMediaEventGenerator>().ok()
                    } else {
                        None
                    };
                    return Ok(Self {
                        transform,
                        event_gen,
                        pending_inputs: 0,
                        provides_samples,
                        output_buf_size,
                        width,
                        height,
                        frame_duration_100ns: (10_000_000 / i64::from(fps)).max(1),
                        force_idr: AtomicBool::new(true),
                        _guard: guard,
                    });
                }
                Err(err) => {
                    last_err = err.to_string();
                }
            }
        }

        Err(Key2PcError::Display(last_err))
    }

    fn try_init_candidate(
        activate: IMFActivate,
        width: u32,
        height: u32,
        fps: u32,
        bitrate_bps: u32,
    ) -> Result<(IMFTransform, bool, bool, u32)> {
        let transform: IMFTransform = unsafe { activate.ActivateObject() }
            .map_err(|err| Key2PcError::Display(format!("ActivateObject failed: {err}")))?;
        let is_async = unlock_async_if_needed(&transform);

        if let Ok(codec_api) = transform.cast::<ICodecAPI>() {
            let low_delay_mode = variant_u32(eAVEncCommonRateControlMode_LowDelayVBR.0 as u32);
            if unsafe { codec_api.SetValue(&CODECAPI_AVEncCommonRateControlMode, &low_delay_mode) }
                .is_err()
            {
                let cbr_mode = variant_u32(eAVEncCommonRateControlMode_CBR.0 as u32);
                let _ =
                    unsafe { codec_api.SetValue(&CODECAPI_AVEncCommonRateControlMode, &cbr_mode) };
            }
            let bitrate_var = variant_u32(bitrate_bps);
            let _ = unsafe { codec_api.SetValue(&CODECAPI_AVEncCommonMeanBitRate, &bitrate_var) };
            let gop_var = variant_u32(fps);
            let _ = unsafe { codec_api.SetValue(&CODECAPI_AVEncMPVGOPSize, &gop_var) };
        }

        let out_type = build_video_media_type(
            MFVideoFormat_H264,
            width,
            height,
            fps,
            Some(bitrate_bps),
            Some(eAVEncH264VProfile_Main.0 as u32),
        )?;
        unsafe { transform.SetOutputType(0, &out_type, 0) }
            .map_err(|err| Key2PcError::Display(format!("SetOutputType failed: {err}")))?;

        let in_type = build_video_media_type(MFVideoFormat_NV12, width, height, fps, None, None)?;
        unsafe { transform.SetInputType(0, &in_type, 0) }
            .map_err(|err| Key2PcError::Display(format!("SetInputType failed: {err}")))?;

        unsafe {
            transform.ProcessMessage(MFT_MESSAGE_NOTIFY_BEGIN_STREAMING, 0)?;
            transform.ProcessMessage(MFT_MESSAGE_NOTIFY_START_OF_STREAM, 0)?;
        }

        let out_info = unsafe { transform.GetOutputStreamInfo(0)? };
        let provides_samples = (out_info.dwFlags & MFT_OUTPUT_STREAM_PROVIDES_SAMPLES.0 as u32) != 0;
        let output_buf_size = out_info.cbSize.max(width * height * 2).max(262_144);
        Ok((transform, is_async, provides_samples, output_buf_size))
    }

    fn apply_force_keyframe(&self) {
        if let Ok(codec_api) = self.transform.cast::<ICodecAPI>() {
            let val = variant_u32(1);
            let _ = unsafe { codec_api.SetValue(&CODECAPI_AVEncVideoForceKeyFrame, &val) };
        }
    }

    fn extract_encoded_packet(&self, sample: &IMFSample) -> Result<EncodedFrame> {
        let buffer = unsafe { sample.ConvertToContiguousBuffer() }
            .map_err(|err| Key2PcError::Display(format!("ConvertToContiguousBuffer failed: {err}")))?;
        let mut ptr: *mut u8 = ptr::null_mut();
        let mut len = 0u32;
        unsafe { buffer.Lock(&mut ptr, None, Some(&mut len)) }
            .map_err(|err| Key2PcError::Display(format!("Buffer lock failed: {err}")))?;
        let data = unsafe { slice::from_raw_parts(ptr, len as usize) }.to_vec();
        let _ = unsafe { buffer.Unlock() };

        let clean_point = unsafe { sample.GetUINT32(&MFSampleExtension_CleanPoint) }.unwrap_or(0) != 0;
        let has_idr = clean_point || contains_h264_idr(&data);

        Ok(EncodedFrame {
            data,
            is_keyframe: has_idr,
            width: self.width,
            height: self.height,
        })
    }
}

impl VideoEncoder for MfH264Encoder {
    fn encode_nv12(&mut self, nv12_data: &[u8], pts_100ns: i64) -> Result<Vec<EncodedFrame>> {
        if self.force_idr.swap(false, Ordering::AcqRel) {
            self.apply_force_keyframe();
        }
        let sample = create_sample_from_bytes(nv12_data, pts_100ns, self.frame_duration_100ns)?;
        let alloc_size = self.output_buf_size;
        let mut out_frames = Vec::new();

        if let Some(event_gen) = self.event_gen.clone() {
            let mut input_sent = false;
            if self.pending_inputs > 0 {
                self.pending_inputs -= 1;
                unsafe { self.transform.ProcessInput(0, &sample, 0) }
                    .map_err(|err| Key2PcError::Display(format!("ProcessInput failed: {err}")))?;
                input_sent = true;
            }
            let deadline = Instant::now() + Duration::from_millis(120);
            while let Some(event) = poll_next_mft_event(&event_gen, deadline)? {
                let kind = unsafe { event.GetType() }
                    .map_err(|err| Key2PcError::Display(format!("GetType failed: {err}")))?;
                if kind == MEError.0 as u32 {
                    return Err(Key2PcError::Display("Encoder reported MEError".to_string()));
                }
                if kind == METransformNeedInput.0 as u32 {
                    if !input_sent {
                        unsafe { self.transform.ProcessInput(0, &sample, 0) }.map_err(|err| {
                            Key2PcError::Display(format!("ProcessInput failed: {err}"))
                        })?;
                        input_sent = true;
                    } else {
                        self.pending_inputs += 1;
                        break;
                    }
                } else if kind == METransformHaveOutput.0 as u32 {
                    if let Some(out_sample) =
                        pull_output_sample(&self.transform, self.provides_samples, alloc_size)
                            .map_err(|err| {
                                Key2PcError::Display(format!("ProcessOutput failed: {err}"))
                            })?
                    {
                        out_frames.push(self.extract_encoded_packet(&out_sample)?);
                        break;
                    }
                }
            }
        } else {
            unsafe { self.transform.ProcessInput(0, &sample, 0) }
                .map_err(|err| Key2PcError::Display(format!("ProcessInput failed: {err}")))?;
            loop {
                match pull_output_sample(&self.transform, self.provides_samples, alloc_size) {
                    Ok(Some(out_sample)) => {
                        out_frames.push(self.extract_encoded_packet(&out_sample)?);
                    }
                    Ok(None) => break,
                    Err(err) => {
                        return Err(Key2PcError::Display(format!("ProcessOutput failed: {err}")));
                    }
                }
            }
        }

        Ok(out_frames)
    }

    fn force_keyframe(&mut self) {
        self.force_idr.store(true, Ordering::Release);
    }
}

pub struct MfH264Decoder {
    transform: IMFTransform,
    event_gen: Option<IMFMediaEventGenerator>,
    pending_inputs: u32,
    provides_samples: bool,
    output_buf_size: u32,
    width: u32,
    height: u32,
    coded_height: u32,
    stride: usize,
    _guard: MfSessionGuard,
}

unsafe impl Send for MfH264Decoder {}

impl MfH264Decoder {
    pub fn new(width: u32, height: u32) -> Result<Self> {
        let guard = MfSessionGuard::acquire()?;
        let mut candidates = enumerate_mft_candidates(
            MFT_CATEGORY_VIDEO_DECODER,
            MFT_ENUM_FLAG_SYNCMFT | MFT_ENUM_FLAG_ASYNCMFT | MFT_ENUM_FLAG_SORTANDFILTER,
            Some(MFVideoFormat_H264),
            Some(MFVideoFormat_NV12),
        )?;
        let hw_candidates = enumerate_mft_candidates(
            MFT_CATEGORY_VIDEO_DECODER,
            MFT_ENUM_FLAG_HARDWARE | MFT_ENUM_FLAG_ASYNCMFT | MFT_ENUM_FLAG_SORTANDFILTER,
            Some(MFVideoFormat_H264),
            Some(MFVideoFormat_NV12),
        )?;
        candidates.extend(hw_candidates);

        let mut last_err = String::from("No H.264 decoder MFT available");
        for activate in candidates {
            match Self::try_init_candidate(activate, width, height) {
                Ok((transform, event_gen, provides_samples, output_buf_size)) => {
                    let mut dec = Self {
                        transform,
                        event_gen,
                        pending_inputs: 0,
                        provides_samples,
                        output_buf_size,
                        width,
                        height,
                        coded_height: height,
                        stride: width as usize,
                        _guard: guard,
                    };
                    dec.refresh_output_layout();
                    return Ok(dec);
                }
                Err(err) => {
                    last_err = err.to_string();
                }
            }
        }

        Err(Key2PcError::Display(last_err))
    }

    fn try_init_candidate(
        activate: IMFActivate,
        width: u32,
        height: u32,
    ) -> Result<(IMFTransform, Option<IMFMediaEventGenerator>, bool, u32)> {
        let transform: IMFTransform = unsafe { activate.ActivateObject() }
            .map_err(|err| Key2PcError::Display(format!("Decoder ActivateObject failed: {err}")))?;
        let is_async = unlock_async_if_needed(&transform);

        let in_type = build_video_media_type(MFVideoFormat_H264, width, height, 60, None, None)?;
        unsafe { transform.SetInputType(0, &in_type, 0) }
            .map_err(|err| Key2PcError::Display(format!("Decoder SetInputType failed: {err}")))?;

        Self::select_nv12_output_type(&transform)?;

        unsafe {
            transform.ProcessMessage(MFT_MESSAGE_NOTIFY_BEGIN_STREAMING, 0)?;
            transform.ProcessMessage(MFT_MESSAGE_NOTIFY_START_OF_STREAM, 0)?;
        }

        let out_info = unsafe { transform.GetOutputStreamInfo(0)? };
        let provides_samples = (out_info.dwFlags & MFT_OUTPUT_STREAM_PROVIDES_SAMPLES.0 as u32) != 0;
        let output_buf_size = out_info
            .cbSize
            .max(width.max(1920) * height.max(1088) * 2)
            .max(4_194_304);
        let event_gen = if is_async {
            transform.cast::<IMFMediaEventGenerator>().ok()
        } else {
            None
        };
        Ok((transform, event_gen, provides_samples, output_buf_size))
    }

    fn select_nv12_output_type(transform: &IMFTransform) -> Result<()> {
        let mut idx = 0u32;
        while let Ok(candidate) = unsafe { transform.GetOutputAvailableType(0, idx) } {
            idx += 1;
            if let Ok(subtype) = unsafe { candidate.GetGUID(&MF_MT_SUBTYPE) } {
                if subtype == MFVideoFormat_NV12 {
                    unsafe { transform.SetOutputType(0, &candidate, 0) }.map_err(|err| {
                        Key2PcError::Display(format!("Decoder SetOutputType failed: {err}"))
                    })?;
                    return Ok(());
                }
            }
        }
        Err(Key2PcError::Display(
            "Decoder does not expose NV12 output".to_string(),
        ))
    }

    fn refresh_output_layout(&mut self) {
        let Ok(out_type) = (unsafe { self.transform.GetOutputCurrentType(0) }) else {
            return;
        };
        if let Ok(packed_size) = unsafe { out_type.GetUINT64(&MF_MT_FRAME_SIZE) } {
            let (cw, ch) = unpack_u64(packed_size);
            if cw > 0 && ch > 0 {
                if self.width == 0 {
                    self.width = cw;
                }
                if self.height == 0 {
                    self.height = ch;
                }
                self.coded_height = ch;
                self.stride = cw as usize;
            }
        }
        if let Ok(raw_stride) = unsafe { out_type.GetUINT32(&MF_MT_DEFAULT_STRIDE) } {
            let signed_stride = (raw_stride as i32).unsigned_abs() as usize;
            if signed_stride >= self.width as usize {
                self.stride = signed_stride;
            }
        }
        if let Ok(info) = unsafe { self.transform.GetOutputStreamInfo(0) } {
            self.output_buf_size = self.output_buf_size.max(info.cbSize);
        }
    }

    fn extract_decoded_nv12(&self, sample: &IMFSample) -> Result<DecodedFrame> {
        let buffer = unsafe { sample.ConvertToContiguousBuffer() }
            .map_err(|err| Key2PcError::Display(format!("Decoder buffer failed: {err}")))?;
        let w = self.width as usize;
        let h = self.height as usize;
        let ch = self.coded_height.max(self.height) as usize;
        let mut out_nv12 = vec![0u8; w * h * 3 / 2];

        let mut ptr: *mut u8 = ptr::null_mut();
        let mut len = 0u32;
        unsafe { buffer.Lock(&mut ptr, None, Some(&mut len)) }
            .map_err(|err| Key2PcError::Display(format!("Decoder lock failed: {err}")))?;
        let src = unsafe { slice::from_raw_parts(ptr, len as usize) };
        let pitch = self.stride.max(w);
        let inferred_ch = if pitch > 0 && (len as usize) >= pitch * (ch + ch / 2) {
            ch
        } else {
            h
        };
        copy_nv12_planes(src, pitch, inferred_ch, w, h, &mut out_nv12);
        let _ = unsafe { buffer.Unlock() };

        Ok(DecodedFrame {
            nv12_data: out_nv12,
            width: self.width,
            height: self.height,
        })
    }

    fn drain_sync_outputs(&mut self, alloc_size: u32) -> Result<Vec<DecodedFrame>> {
        let mut frames = Vec::new();
        loop {
            match pull_output_sample(&self.transform, self.provides_samples, alloc_size) {
                Ok(Some(out_sample)) => {
                    frames.push(self.extract_decoded_nv12(&out_sample)?);
                }
                Ok(None) => break,
                Err(err) if err.code() == MF_E_TRANSFORM_STREAM_CHANGE => {
                    Self::select_nv12_output_type(&self.transform)?;
                    self.refresh_output_layout();
                }
                Err(err) => {
                    return Err(Key2PcError::Display(format!(
                        "Decoder ProcessOutput failed: {err}"
                    )));
                }
            }
        }
        Ok(frames)
    }
}

impl VideoDecoder for MfH264Decoder {
    fn decode_annex_b(&mut self, data: &[u8], pts_100ns: i64) -> Result<Vec<DecodedFrame>> {
        let sample = create_sample_from_bytes(data, pts_100ns, 166_666)?;
        let alloc_size = self.output_buf_size;

        if let Some(event_gen) = self.event_gen.clone() {
            let mut frames = Vec::new();
            let mut input_sent = false;
            if self.pending_inputs > 0 {
                self.pending_inputs -= 1;
                unsafe { self.transform.ProcessInput(0, &sample, 0) }.map_err(|err| {
                    Key2PcError::Display(format!("Decoder ProcessInput failed: {err}"))
                })?;
                input_sent = true;
            }
            let deadline = Instant::now() + Duration::from_millis(120);
            while let Some(event) = poll_next_mft_event(&event_gen, deadline)? {
                let kind = unsafe { event.GetType() }
                    .map_err(|err| Key2PcError::Display(format!("Decoder GetType failed: {err}")))?;
                if kind == MEError.0 as u32 {
                    return Err(Key2PcError::Display("Decoder reported MEError".to_string()));
                }
                if kind == METransformNeedInput.0 as u32 {
                    if !input_sent {
                        unsafe { self.transform.ProcessInput(0, &sample, 0) }.map_err(|err| {
                            Key2PcError::Display(format!("Decoder ProcessInput failed: {err}"))
                        })?;
                        input_sent = true;
                    } else {
                        self.pending_inputs += 1;
                        break;
                    }
                } else if kind == METransformHaveOutput.0 as u32 {
                    match pull_output_sample(&self.transform, self.provides_samples, alloc_size) {
                        Ok(Some(out_sample)) => {
                            frames.push(self.extract_decoded_nv12(&out_sample)?);
                            break;
                        }
                        Ok(None) => break,
                        Err(err) if err.code() == MF_E_TRANSFORM_STREAM_CHANGE => {
                            Self::select_nv12_output_type(&self.transform)?;
                            self.refresh_output_layout();
                        }
                        Err(err) => {
                            return Err(Key2PcError::Display(format!(
                                "Decoder ProcessOutput failed: {err}"
                            )));
                        }
                    }
                }
            }
            return Ok(frames);
        }

        unsafe { self.transform.ProcessInput(0, &sample, 0) }
            .map_err(|err| Key2PcError::Display(format!("Decoder ProcessInput failed: {err}")))?;
        self.drain_sync_outputs(alloc_size)
    }
}

fn copy_nv12_planes(
    src: &[u8],
    pitch: usize,
    coded_h: usize,
    w: usize,
    h: usize,
    dst: &mut [u8],
) {
    for row in 0..h {
        let src_off = row * pitch;
        let dst_off = row * w;
        if src_off + w <= src.len() && dst_off + w <= dst.len() {
            dst[dst_off..dst_off + w].copy_from_slice(&src[src_off..src_off + w]);
        }
    }
    let uv_src_base = coded_h * pitch;
    let uv_dst_base = h * w;
    for row in 0..(h / 2) {
        let src_off = uv_src_base + row * pitch;
        let dst_off = uv_dst_base + row * w;
        if src_off + w <= src.len() && dst_off + w <= dst.len() {
            dst[dst_off..dst_off + w].copy_from_slice(&src[src_off..src_off + w]);
        }
    }
}

fn contains_h264_idr(data: &[u8]) -> bool {
    let mut i = 0usize;
    while i + 4 < data.len() {
        if data[i] == 0 && data[i + 1] == 0 {
            let nal_pos = if data[i + 2] == 1 {
                Some(i + 3)
            } else if data[i + 2] == 0 && data[i + 3] == 1 && i + 4 < data.len() {
                Some(i + 4)
            } else {
                None
            };
            if let Some(pos) = nal_pos {
                let nal_type = data[pos] & 0x1F;
                if nal_type == 5 {
                    return true;
                }
                i = pos;
                continue;
            }
        }
        i += 1;
    }
    false
}
