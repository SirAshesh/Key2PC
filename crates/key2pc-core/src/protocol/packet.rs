use crate::error::{Key2PcError, Result};

pub const PACKET_SIZE: usize = 44;
pub const PROTOCOL_MAGIC: [u8; 2] = *b"K2";
pub const PROTOCOL_VERSION: u8 = 1;

pub const FLAG_KEY_UP: u32 = 1 << 0;
pub const FLAG_EXTENDED: u32 = 1 << 1;
pub const FLAG_ALT_DOWN: u32 = 1 << 2;
pub const FLAG_SESSION_START: u32 = 1 << 3;

#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PacketKind {
    KeyEvent = 1,
    Ack = 2,
    Heartbeat = 3,
    ReleaseAll = 4,
    DiscoverReq = 5,
    DiscoverResp = 6,
    Ping = 7,
    Pong = 8,
    MouseEvent = 9,
}

impl PacketKind {
    pub fn from_u8(value: u8) -> Result<Self> {
        match value {
            1 => Ok(Self::KeyEvent),
            2 => Ok(Self::Ack),
            3 => Ok(Self::Heartbeat),
            4 => Ok(Self::ReleaseAll),
            5 => Ok(Self::DiscoverReq),
            6 => Ok(Self::DiscoverResp),
            7 => Ok(Self::Ping),
            8 => Ok(Self::Pong),
            9 => Ok(Self::MouseEvent),
            _ => Err(Key2PcError::Protocol(format!(
                "Unknown packet kind: {value}"
            ))),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyAction {
    Down,
    Up,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyEvent {
    pub vk: u16,
    pub scan_code: u16,
    pub action: KeyAction,
    pub extended: bool,
    pub alt_down: bool,
}

impl KeyEvent {
    pub fn to_flags(self) -> u32 {
        let mut flags = 0u32;
        if self.action == KeyAction::Up {
            flags |= FLAG_KEY_UP;
        }
        if self.extended {
            flags |= FLAG_EXTENDED;
        }
        if self.alt_down {
            flags |= FLAG_ALT_DOWN;
        }
        flags
    }

    pub fn from_parts(vk: u16, scan_code: u16, flags: u32) -> Self {
        Self {
            vk,
            scan_code,
            action: if (flags & FLAG_KEY_UP) != 0 {
                KeyAction::Up
            } else {
                KeyAction::Down
            },
            extended: (flags & FLAG_EXTENDED) != 0,
            alt_down: (flags & FLAG_ALT_DOWN) != 0,
        }
    }
}

#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseAction {
    Move = 0,
    LeftDown = 1,
    LeftUp = 2,
    RightDown = 3,
    RightUp = 4,
    MiddleDown = 5,
    MiddleUp = 6,
    X1Down = 7,
    X1Up = 8,
    X2Down = 9,
    X2Up = 10,
    Wheel = 11,
    HWheel = 12,
}

impl MouseAction {
    pub fn from_u8(value: u8) -> Result<Self> {
        match value {
            0 => Ok(Self::Move),
            1 => Ok(Self::LeftDown),
            2 => Ok(Self::LeftUp),
            3 => Ok(Self::RightDown),
            4 => Ok(Self::RightUp),
            5 => Ok(Self::MiddleDown),
            6 => Ok(Self::MiddleUp),
            7 => Ok(Self::X1Down),
            8 => Ok(Self::X1Up),
            9 => Ok(Self::X2Down),
            10 => Ok(Self::X2Up),
            11 => Ok(Self::Wheel),
            12 => Ok(Self::HWheel),
            _ => Err(Key2PcError::Protocol(format!(
                "Unknown mouse action: {value}"
            ))),
        }
    }

    pub fn requires_reliable_ack(self) -> bool {
        !matches!(self, Self::Move)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MouseEvent {
    pub action: MouseAction,
    pub norm_x: u16,
    pub norm_y: u16,
    pub wheel_delta: i16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Packet {
    pub kind: PacketKind,
    pub seq: u64,
    pub timestamp_us: u64,
    pub vk: u16,
    pub scan_code: u16,
    pub flags: u32,
}

impl Packet {
    pub fn key_event(seq: u64, timestamp_us: u64, event: KeyEvent) -> Self {
        Self {
            kind: PacketKind::KeyEvent,
            seq,
            timestamp_us,
            vk: event.vk,
            scan_code: event.scan_code,
            flags: event.to_flags(),
        }
    }

    pub fn mouse_event(seq: u64, timestamp_us: u64, event: MouseEvent) -> Self {
        let flags = (u32::from(event.action as u8) << 16) | u32::from(event.norm_y);
        Self {
            kind: PacketKind::MouseEvent,
            seq,
            timestamp_us,
            vk: event.wheel_delta as u16,
            scan_code: event.norm_x,
            flags,
        }
    }

    pub fn ack(seq: u64, timestamp_us: u64) -> Self {
        Self {
            kind: PacketKind::Ack,
            seq,
            timestamp_us,
            vk: 0,
            scan_code: 0,
            flags: 0,
        }
    }

    pub fn heartbeat(seq: u64, timestamp_us: u64, held_count: u16) -> Self {
        Self {
            kind: PacketKind::Heartbeat,
            seq,
            timestamp_us,
            vk: held_count,
            scan_code: 0,
            flags: 0,
        }
    }

    pub fn release_all(seq: u64, timestamp_us: u64, session_start: bool) -> Self {
        Self {
            kind: PacketKind::ReleaseAll,
            seq,
            timestamp_us,
            vk: 0,
            scan_code: 0,
            flags: if session_start { FLAG_SESSION_START } else { 0 },
        }
    }

    pub fn discover_req(seq: u64, timestamp_us: u64) -> Self {
        Self {
            kind: PacketKind::DiscoverReq,
            seq,
            timestamp_us,
            vk: 0,
            scan_code: 0,
            flags: 0,
        }
    }

    pub fn discover_resp(seq: u64, timestamp_us: u64, listen_port: u16, video_port: u16) -> Self {
        Self {
            kind: PacketKind::DiscoverResp,
            seq,
            timestamp_us,
            vk: listen_port,
            scan_code: video_port,
            flags: 0,
        }
    }

    pub fn ping(seq: u64, timestamp_us: u64, session_start: bool) -> Self {
        Self {
            kind: PacketKind::Ping,
            seq,
            timestamp_us,
            vk: 0,
            scan_code: 0,
            flags: if session_start { FLAG_SESSION_START } else { 0 },
        }
    }

    pub fn pong(seq: u64, timestamp_us: u64) -> Self {
        Self {
            kind: PacketKind::Pong,
            seq,
            timestamp_us,
            vk: 0,
            scan_code: 0,
            flags: 0,
        }
    }

    pub fn as_key_event(&self) -> Option<KeyEvent> {
        if self.kind == PacketKind::KeyEvent {
            Some(KeyEvent::from_parts(self.vk, self.scan_code, self.flags))
        } else {
            None
        }
    }

    pub fn as_mouse_event(&self) -> Result<Option<MouseEvent>> {
        if self.kind != PacketKind::MouseEvent {
            return Ok(None);
        }
        let action_byte = ((self.flags >> 16) & 0xFF) as u8;
        let action = MouseAction::from_u8(action_byte)?;
        Ok(Some(MouseEvent {
            action,
            norm_x: self.scan_code,
            norm_y: (self.flags & 0xFFFF) as u16,
            wheel_delta: self.vk as i16,
        }))
    }

    pub fn is_session_start(&self) -> bool {
        (self.flags & FLAG_SESSION_START) != 0
    }

    pub fn encode(&self, key: &[u8; 32]) -> [u8; PACKET_SIZE] {
        let mut out = [0u8; PACKET_SIZE];
        out[0..2].copy_from_slice(&PROTOCOL_MAGIC);
        out[2] = PROTOCOL_VERSION;
        out[3] = self.kind as u8;
        out[4..12].copy_from_slice(&self.seq.to_le_bytes());
        out[12..20].copy_from_slice(&self.timestamp_us.to_le_bytes());
        out[20..22].copy_from_slice(&self.vk.to_le_bytes());
        out[22..24].copy_from_slice(&self.scan_code.to_le_bytes());
        out[24..28].copy_from_slice(&self.flags.to_le_bytes());

        let tag = compute_mac16(key, &out[..28]);
        out[28..44].copy_from_slice(&tag);
        out
    }

    pub fn decode(bytes: &[u8], key: &[u8; 32]) -> Result<Self> {
        if bytes.len() != PACKET_SIZE {
            return Err(Key2PcError::Protocol(format!(
                "Invalid packet length: {}",
                bytes.len()
            )));
        }
        if bytes[0..2] != PROTOCOL_MAGIC {
            return Err(Key2PcError::Protocol("Invalid magic header".to_string()));
        }
        if bytes[2] != PROTOCOL_VERSION {
            return Err(Key2PcError::Protocol(format!(
                "Unsupported protocol version: {}",
                bytes[2]
            )));
        }

        let expected = compute_mac16(key, &bytes[..28]);
        if !constant_time_eq_16(&expected, &bytes[28..44]) {
            return Err(Key2PcError::Protocol("MAC verification failed".to_string()));
        }

        let kind = PacketKind::from_u8(bytes[3])?;
        let seq = u64::from_le_bytes(bytes[4..12].try_into().unwrap());
        let timestamp_us = u64::from_le_bytes(bytes[12..20].try_into().unwrap());
        let vk = u16::from_le_bytes(bytes[20..22].try_into().unwrap());
        let scan_code = u16::from_le_bytes(bytes[22..24].try_into().unwrap());
        let flags = u32::from_le_bytes(bytes[24..28].try_into().unwrap());

        Ok(Self {
            kind,
            seq,
            timestamp_us,
            vk,
            scan_code,
            flags,
        })
    }
}

pub fn derive_auth_key(secret: &str) -> [u8; 32] {
    *blake3::hash(secret.as_bytes()).as_bytes()
}

pub(crate) fn compute_mac16(key: &[u8; 32], payload: &[u8]) -> [u8; 16] {
    let hash = blake3::keyed_hash(key, payload);
    let mut mac = [0u8; 16];
    mac.copy_from_slice(&hash.as_bytes()[..16]);
    mac
}

pub(crate) fn constant_time_eq_16(a: &[u8; 16], b: &[u8]) -> bool {
    if b.len() != 16 {
        return false;
    }
    let mut diff = 0u8;
    for i in 0..16 {
        diff |= a[i] ^ b[i];
    }
    diff == 0
}
