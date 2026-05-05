use heapless::String;

use crate::config::LORA_MAX_PAYLOAD;

// ── Wire format ──────────────────────────────────────────────────────────────
//
//  Byte 0:        [type:4 | dest:4]
//  Byte 1:        id
//  Byte 2:        size  (text length, 0..=MAX_TEXT)
//  Bytes 3..3+n:  UTF-8 text payload
//  Byte 3+n:      checksum (XOR of bytes 0..3+n-1, i.e. the header + payload)

const HEADER_SIZE: usize = 3;
const CHECKSUM_SIZE: usize = 1;
const OVERHEAD: usize = HEADER_SIZE + CHECKSUM_SIZE;
pub const MAX_TEXT: usize = LORA_MAX_PAYLOAD - OVERHEAD;

const TYPE_MESSAGE: u8 = 0;
const TYPE_ACK: u8 = 1;
const TYPE_PING: u8 = 2;
const TYPE_GPS: u8 = 3;
const TYPE_RESEND: u8 = 4;

// ── Error ────────────────────────────────────────────────────────────────────

#[derive(Debug, PartialEq)]
pub enum ProtocolError {
    TooShort,
    InvalidType(u8),
    InvalidUtf8,
    SizeMismatch,
    BadChecksum,
}

// ── Packet types ─────────────────────────────────────────────────────────────

pub struct MessagePacket {
    pub dest: u8,
    pub id: u8,
    pub text: String<MAX_TEXT>,
}

pub struct AckPacket {
    pub id: u8,
}

pub enum Packet {
    Message(MessagePacket),
    Ack(AckPacket),
    Ping,
    Gps,
    Resend,
}

// ── Checksum ─────────────────────────────────────────────────────────────────

fn checksum(bytes: &[u8]) -> u8 {
    bytes.iter().fold(0u8, |acc, &b| acc ^ b)
}

// ── Packet: parse ─────────────────────────────────────────────────────────────

impl Packet {
    pub fn parse(raw: &[u8]) -> Result<Self, ProtocolError> {
        if raw.is_empty() {
            return Err(ProtocolError::TooShort);
        }

        match raw[0] >> 4 {
            TYPE_MESSAGE => MessagePacket::parse(raw).map(Packet::Message),
            TYPE_ACK => AckPacket::parse(raw).map(Packet::Ack),
            TYPE_PING => Ok(Packet::Ping),
            TYPE_GPS => Ok(Packet::Gps),
            TYPE_RESEND => Ok(Packet::Resend),
            other => Err(ProtocolError::InvalidType(other)),
        }
    }
}

// ── MessagePacket ─────────────────────────────────────────────────────────────

impl MessagePacket {
    pub fn new(dest: u8, id: u8, text: String<MAX_TEXT>) -> Self {
        Self { dest, id, text }
    }

    fn parse(raw: &[u8]) -> Result<Self, ProtocolError> {
        if raw.len() < OVERHEAD {
            return Err(ProtocolError::TooShort);
        }

        let dest = raw[0] & 0x0F;
        let id = raw[1];
        let size = raw[2] as usize;

        let expected_len = HEADER_SIZE + size + CHECKSUM_SIZE;
        if raw.len() < expected_len {
            return Err(ProtocolError::SizeMismatch);
        }

        let payload = &raw[HEADER_SIZE..HEADER_SIZE + size];
        let received_checksum = raw[HEADER_SIZE + size];

        // Checksum covers header bytes + payload bytes.
        let computed = checksum(&raw[..HEADER_SIZE + size]);
        if received_checksum != computed {
            return Err(ProtocolError::BadChecksum);
        }

        let text_str = core::str::from_utf8(payload).map_err(|_| ProtocolError::InvalidUtf8)?;
        let text = String::try_from(text_str).map_err(|_| ProtocolError::SizeMismatch)?;

        Ok(Self { dest, id, text })
    }

    /// Encode into `buf`. Returns the number of bytes written, or `None` if
    /// `buf` is too small or the text exceeds `MAX_TEXT`.
    pub fn encode(&self, buf: &mut [u8]) -> Option<usize> {
        let text_bytes = self.text.as_bytes();
        let frame_len = HEADER_SIZE + text_bytes.len() + CHECKSUM_SIZE;

        if buf.len() < frame_len || text_bytes.len() > MAX_TEXT {
            return None;
        }

        buf[0] = (TYPE_MESSAGE << 4) | (self.dest & 0x0F);
        buf[1] = self.id;
        buf[2] = text_bytes.len() as u8;
        buf[HEADER_SIZE..HEADER_SIZE + text_bytes.len()].copy_from_slice(text_bytes);

        // Checksum covers header + payload (same range as parse).
        buf[HEADER_SIZE + text_bytes.len()] = checksum(&buf[..HEADER_SIZE + text_bytes.len()]);

        Some(frame_len)
    }
}

// ── AckPacket ─────────────────────────────────────────────────────────────────

impl AckPacket {
    pub fn new(id: u8) -> Self {
        Self { id }
    }

    fn parse(raw: &[u8]) -> Result<Self, ProtocolError> {
        if raw.len() < 2 {
            return Err(ProtocolError::TooShort);
        }
        Ok(Self { id: raw[1] })
    }

    /// Encode into `buf`. Returns the number of bytes written, or `None` if
    /// `buf` is too small.
    pub fn encode(&self, buf: &mut [u8]) -> Option<usize> {
        if buf.len() < 2 {
            return None;
        }
        buf[0] = TYPE_ACK << 4;
        buf[1] = self.id;
        Some(2)
    }
}

// ── Convenience helpers ───────────────────────────────────────────────────────

/// Encode an ACK for `id` into a fixed 2-byte array. Always succeeds.
pub fn encode_ack(id: u8) -> [u8; 2] {
    let mut buf = [0u8; 2];
    AckPacket::new(id).encode(&mut buf);
    buf
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn make_text(s: &str) -> String<MAX_TEXT> {
        String::try_from(s).unwrap()
    }

    #[test]
    fn message_roundtrip() {
        let msg = MessagePacket::new(3, 42, make_text("hello"));
        let mut buf = [0u8; LORA_MAX_PAYLOAD];
        let len = msg.encode(&mut buf).unwrap();

        match Packet::parse(&buf[..len]).unwrap() {
            Packet::Message(m) => {
                assert_eq!(m.dest, 3);
                assert_eq!(m.id, 42);
                assert_eq!(m.text.as_str(), "hello");
            }
            _ => panic!("expected Message"),
        }
    }

    #[test]
    fn ack_roundtrip() {
        let ack = encode_ack(7);
        match Packet::parse(&ack).unwrap() {
            Packet::Ack(a) => assert_eq!(a.id, 7),
            _ => panic!("expected Ack"),
        }
    }

    #[test]
    fn bad_checksum_rejected() {
        let msg = MessagePacket::new(0, 1, make_text("test"));
        let mut buf = [0u8; LORA_MAX_PAYLOAD];
        let len = msg.encode(&mut buf).unwrap();
        buf[len - 1] ^= 0xFF; // corrupt checksum
        assert_eq!(Packet::parse(&buf[..len]), Err(ProtocolError::BadChecksum));
    }

    #[test]
    fn too_short_rejected() {
        assert_eq!(Packet::parse(&[]), Err(ProtocolError::TooShort));
        assert_eq!(
            Packet::parse(&[TYPE_MESSAGE << 4, 0]),
            Err(ProtocolError::TooShort)
        );
    }

    #[test]
    fn unknown_type_rejected() {
        assert_eq!(Packet::parse(&[0xF0]), Err(ProtocolError::InvalidType(0xF)));
    }
}
