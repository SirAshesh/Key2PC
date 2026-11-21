use std::collections::VecDeque;
use std::time::{Duration, Instant};

use crate::error::{Key2PcError, Result};
use crate::protocol::packet::{
    constant_time_eq_16, PROTOCOL_MAGIC, PROTOCOL_VERSION,
};

pub const VIDEO_PACKET_HEADER_SIZE: usize = 44;
pub const MAX_VIDEO_CHUNK_PAYLOAD: usize = 1340;
pub const VIDEO_FLAG_KEYFRAME: u16 = 1 << 0;

#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VideoPacketKind {
    Chunk = 10,
    KeyframeReq = 11,
    StreamHello = 12,
    StreamStop = 13,
}

impl VideoPacketKind {
    pub fn from_u8(value: u8) -> Result<Self> {
        match value {
            10 => Ok(Self::Chunk),
            11 => Ok(Self::KeyframeReq),
            12 => Ok(Self::StreamHello),
            13 => Ok(Self::StreamStop),
            _ => Err(Key2PcError::Protocol(format!(
                "Unknown video packet kind: {value}"
            ))),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VideoPacketHeader {
    pub kind: VideoPacketKind,
    pub frame_id: u32,
    pub chunk_idx: u16,
    pub chunk_count: u16,
    pub flags: u16,
    pub width: u16,
    pub height: u16,
    pub payload_len: u16,
    pub timestamp_us: u64,
}

impl VideoPacketHeader {
    pub fn control(kind: VideoPacketKind, width: u16, height: u16, timestamp_us: u64) -> Self {
        Self {
            kind,
            frame_id: 0,
            chunk_idx: 0,
            chunk_count: 0,
            flags: 0,
            width,
            height,
            payload_len: 0,
            timestamp_us,
        }
    }

    pub fn is_keyframe(&self) -> bool {
        (self.flags & VIDEO_FLAG_KEYFRAME) != 0
    }
}

pub fn encode_video_packet(
    header: &VideoPacketHeader,
    payload: &[u8],
    key: &[u8; 32],
    out: &mut Vec<u8>,
) -> Result<()> {
    if payload.len() > MAX_VIDEO_CHUNK_PAYLOAD {
        return Err(Key2PcError::Protocol(format!(
            "Video chunk payload exceeds max: {}",
            payload.len()
        )));
    }

    out.clear();
    out.resize(VIDEO_PACKET_HEADER_SIZE + payload.len(), 0);
    out[0..2].copy_from_slice(&PROTOCOL_MAGIC);
    out[2] = PROTOCOL_VERSION;
    out[3] = header.kind as u8;
    out[4..8].copy_from_slice(&header.frame_id.to_le_bytes());
    out[8..10].copy_from_slice(&header.chunk_idx.to_le_bytes());
    out[10..12].copy_from_slice(&header.chunk_count.to_le_bytes());
    out[12..14].copy_from_slice(&header.flags.to_le_bytes());
    out[14..16].copy_from_slice(&header.width.to_le_bytes());
    out[16..18].copy_from_slice(&header.height.to_le_bytes());
    out[18..20].copy_from_slice(&(payload.len() as u16).to_le_bytes());
    out[20..28].copy_from_slice(&header.timestamp_us.to_le_bytes());
    if !payload.is_empty() {
        out[44..].copy_from_slice(payload);
    }

    let mut hasher = blake3::Hasher::new_keyed(key);
    hasher.update(&out[..28]);
    hasher.update(payload);
    let mac = hasher.finalize();
    out[28..44].copy_from_slice(&mac.as_bytes()[..16]);
    Ok(())
}

pub fn decode_video_packet<'a>(
    bytes: &'a [u8],
    key: &[u8; 32],
) -> Result<(VideoPacketHeader, &'a [u8])> {
    if bytes.len() < VIDEO_PACKET_HEADER_SIZE {
        return Err(Key2PcError::Protocol(format!(
            "Video packet too short: {}",
            bytes.len()
        )));
    }
    if bytes[0..2] != PROTOCOL_MAGIC || bytes[2] != PROTOCOL_VERSION {
        return Err(Key2PcError::Protocol("Invalid video packet header".to_string()));
    }

    let payload_len = u16::from_le_bytes(bytes[18..20].try_into().unwrap()) as usize;
    if bytes.len() != VIDEO_PACKET_HEADER_SIZE + payload_len {
        return Err(Key2PcError::Protocol("Video payload length mismatch".to_string()));
    }

    let payload = &bytes[VIDEO_PACKET_HEADER_SIZE..];
    let mut hasher = blake3::Hasher::new_keyed(key);
    hasher.update(&bytes[..28]);
    hasher.update(payload);
    let expected = hasher.finalize();
    let mut expected_16 = [0u8; 16];
    expected_16.copy_from_slice(&expected.as_bytes()[..16]);

    if !constant_time_eq_16(&expected_16, &bytes[28..44]) {
        return Err(Key2PcError::Protocol("Video packet MAC failed".to_string()));
    }

    let header = VideoPacketHeader {
        kind: VideoPacketKind::from_u8(bytes[3])?,
        frame_id: u32::from_le_bytes(bytes[4..8].try_into().unwrap()),
        chunk_idx: u16::from_le_bytes(bytes[8..10].try_into().unwrap()),
        chunk_count: u16::from_le_bytes(bytes[10..12].try_into().unwrap()),
        flags: u16::from_le_bytes(bytes[12..14].try_into().unwrap()),
        width: u16::from_le_bytes(bytes[14..16].try_into().unwrap()),
        height: u16::from_le_bytes(bytes[16..18].try_into().unwrap()),
        payload_len: payload_len as u16,
        timestamp_us: u64::from_le_bytes(bytes[20..28].try_into().unwrap()),
    };

    Ok((header, payload))
}

pub fn chunk_video_frame(
    frame_id: u32,
    width: u16,
    height: u16,
    is_keyframe: bool,
    timestamp_us: u64,
    frame_bytes: &[u8],
    key: &[u8; 32],
    mut emit: impl FnMut(&[u8]) -> Result<()>,
) -> Result<u16> {
    if frame_bytes.is_empty() {
        return Ok(0);
    }

    let chunk_count_usize = frame_bytes.len().div_ceil(MAX_VIDEO_CHUNK_PAYLOAD);
    if chunk_count_usize > usize::from(u16::MAX) {
        return Err(Key2PcError::Protocol("Video frame too large".to_string()));
    }

    let chunk_count = chunk_count_usize as u16;
    let flags = if is_keyframe { VIDEO_FLAG_KEYFRAME } else { 0 };
    let mut packet_buf = Vec::with_capacity(VIDEO_PACKET_HEADER_SIZE + MAX_VIDEO_CHUNK_PAYLOAD);

    for (idx, chunk) in frame_bytes.chunks(MAX_VIDEO_CHUNK_PAYLOAD).enumerate() {
        let header = VideoPacketHeader {
            kind: VideoPacketKind::Chunk,
            frame_id,
            chunk_idx: idx as u16,
            chunk_count,
            flags,
            width,
            height,
            payload_len: chunk.len() as u16,
            timestamp_us,
        };
        encode_video_packet(&header, chunk, key, &mut packet_buf)?;
        emit(&packet_buf)?;
    }

    Ok(chunk_count)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssembledFrame {
    pub frame_id: u32,
    pub width: u16,
    pub height: u16,
    pub is_keyframe: bool,
    pub timestamp_us: u64,
    pub data: Vec<u8>,
}

struct PartialFrame {
    frame_id: u32,
    width: u16,
    height: u16,
    is_keyframe: bool,
    timestamp_us: u64,
    chunk_count: u16,
    received_count: u16,
    chunks: Vec<Option<Vec<u8>>>,
    started_at: Instant,
}

pub struct FrameAssembler {
    in_flight: VecDeque<PartialFrame>,
    last_completed_frame: Option<u32>,
    frame_timeout: Duration,
}

impl FrameAssembler {
    pub fn new(frame_timeout: Duration) -> Self {
        Self {
            in_flight: VecDeque::with_capacity(6),
            last_completed_frame: None,
            frame_timeout,
        }
    }

    pub fn push_chunk(
        &mut self,
        header: VideoPacketHeader,
        payload: &[u8],
        now: Instant,
    ) -> (Option<AssembledFrame>, bool) {
        if header.kind != VideoPacketKind::Chunk
            || header.chunk_count == 0
            || header.chunk_idx >= header.chunk_count
        {
            return (None, false);
        }

        if let Some(last) = self.last_completed_frame {
            if !is_newer_frame(header.frame_id, last) {
                return (None, false);
            }
        }

        let dropped_old = self.prune_expired(now);

        let pos = if let Some(idx) = self
            .in_flight
            .iter()
            .position(|f| f.frame_id == header.frame_id)
        {
            idx
        } else {
            if self.in_flight.len() >= 4 {
                self.in_flight.pop_front();
            }
            self.in_flight.push_back(PartialFrame {
                frame_id: header.frame_id,
                width: header.width,
                height: header.height,
                is_keyframe: header.is_keyframe(),
                timestamp_us: header.timestamp_us,
                chunk_count: header.chunk_count,
                received_count: 0,
                chunks: vec![None; usize::from(header.chunk_count)],
                started_at: now,
            });
            self.in_flight.len() - 1
        };

        let frame = &mut self.in_flight[pos];
        if frame.chunk_count != header.chunk_count {
            return (None, dropped_old);
        }

        let slot = &mut frame.chunks[usize::from(header.chunk_idx)];
        if slot.is_none() {
            *slot = Some(payload.to_vec());
            frame.received_count += 1;
        }

        if frame.received_count == frame.chunk_count {
            let completed = self.in_flight.remove(pos).unwrap();
            let mut skipped_gap = dropped_old;
            if let Some(prev) = self.last_completed_frame {
                if completed.frame_id != prev.wrapping_add(1) && !completed.is_keyframe {
                    skipped_gap = true;
                }
            }
            self.last_completed_frame = Some(completed.frame_id);

            self.in_flight
                .retain(|pending| is_newer_frame(pending.frame_id, completed.frame_id));

            let total_len: usize = completed
                .chunks
                .iter()
                .map(|c| c.as_ref().map_or(0, Vec::len))
                .sum();
            let mut data = Vec::with_capacity(total_len);
            for chunk in completed.chunks.into_iter().flatten() {
                data.extend_from_slice(&chunk);
            }

            return (
                Some(AssembledFrame {
                    frame_id: completed.frame_id,
                    width: completed.width,
                    height: completed.height,
                    is_keyframe: completed.is_keyframe,
                    timestamp_us: completed.timestamp_us,
                    data,
                }),
                skipped_gap,
            );
        }

        (None, dropped_old)
    }

    pub fn reset(&mut self) {
        self.in_flight.clear();
        self.last_completed_frame = None;
    }

    fn prune_expired(&mut self, now: Instant) -> bool {
        let mut dropped = false;
        while let Some(front) = self.in_flight.front() {
            if now.saturating_duration_since(front.started_at) >= self.frame_timeout {
                self.in_flight.pop_front();
                dropped = true;
            } else {
                break;
            }
        }
        dropped
    }
}

fn is_newer_frame(candidate: u32, reference: u32) -> bool {
    candidate != reference && candidate.wrapping_sub(reference) < 0x8000_0000
}
