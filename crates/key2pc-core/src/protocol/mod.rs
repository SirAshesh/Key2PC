mod packet;
mod sequence;

pub use packet::{
    derive_auth_key, KeyAction, KeyEvent, Packet, PacketKind, FLAG_ALT_DOWN, FLAG_EXTENDED,
    FLAG_KEY_UP, FLAG_SESSION_START, PACKET_SIZE, PROTOCOL_MAGIC, PROTOCOL_VERSION,
};
pub use sequence::SequenceWindow;
