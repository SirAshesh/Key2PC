mod packet;
mod sequence;
mod video;

pub use packet::{
    derive_auth_key, KeyAction, KeyEvent, MouseAction, MouseEvent, Packet, PacketKind,
    FLAG_ALT_DOWN, FLAG_EXTENDED, FLAG_KEY_UP, FLAG_SESSION_START, PACKET_SIZE, PROTOCOL_MAGIC,
    PROTOCOL_VERSION,
};
pub use sequence::SequenceWindow;
pub use video::{
    chunk_video_frame, decode_video_packet, encode_video_packet, AssembledFrame, FrameAssembler,
    VideoPacketHeader, VideoPacketKind, MAX_VIDEO_CHUNK_PAYLOAD, VIDEO_FLAG_KEYFRAME,
    VIDEO_PACKET_HEADER_SIZE,
};
