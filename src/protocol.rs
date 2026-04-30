use embassy_time::{Duration, Instant};
use heapless::{String, Vec};

use crate::config::LORA_MAX_PAYLOAD;

const MAX_RETRIES: u8 = 2;
const ACK_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_PENDING_ACKS: usize = 5;
const DEDUP_WINDOW: usize = 16;

// ── Wire format constants ────────────────────────────────────────────
//
//  Byte 0:        [type:4 | dest:4]
//  Byte 1:        id
//  Byte 2:        size (text length, 0..=124)
//  Bytes 3..3+n:  UTF-8 text payload
//  Byte 3+n:      checksum (XOR of bytes 0..3+n)

const HEADER_SIZE: usize = 3;
const CHECKSUM_SIZE: usize = 1;
const OVERHEAD: usize = HEADER_SIZE + CHECKSUM_SIZE;
const MAX_TEXT: usize = LORA_MAX_PAYLOAD - OVERHEAD;

const TYPE_MESSAGE: u8 = 0;
const TYPE_ACK: u8 = 1;
const TYPE_PING: u8 = 2;
const TYPE_GPS: u8 = 3;
const TYPE_RESEND: u8 = 4;

// --- Error type ---

#[derive(Debug)]
pub enum ProtocolError {
    TooShort,
    InvalidType(u8),
    InvalidUtf8,
    SizeMismatch,
    BadChecksum,
}

// --- Packet types ---

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

fn checksum(bytes: &[u8]) -> u8 {
    bytes.iter().fold(0u8, |acc, &b| acc ^ b)
}

impl Packet {
    /// Parse a raw frame from the wire.
    pub fn parse(raw: &[u8]) -> Result<Self, ProtocolError> {
        if raw.is_empty() {
            return Err(ProtocolError::TooShort);
        }

        let pkt_type = raw[0] >> 4;

        match pkt_type {
            TYPE_MESSAGE => MessagePacket::parse(raw).map(Packet::Message),
            TYPE_ACK => AckPacket::parse(raw).map(Packet::Ack),
            TYPE_PING => Ok(Packet::Ping),
            TYPE_GPS => Ok(Packet::Gps),
            TYPE_RESEND => Ok(Packet::Resend),
            other => Err(ProtocolError::InvalidType(other)),
        }
    }
}

impl MessagePacket {
    /// Parse a MessagePacket from raw bytes.
    ///
    /// Layout: [type:4|dest:4] [id] [size] [text...] [checksum]
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
        let computed = checksum(payload);

        if received_checksum != computed {
            return Err(ProtocolError::BadChecksum);
        }

        let text_str = core::str::from_utf8(payload).map_err(|_| ProtocolError::InvalidUtf8)?;
        let text = String::try_from(text_str).map_err(|_| ProtocolError::SizeMismatch)?;

        Ok(Self { dest, id, text })
    }

    /// Encode into a caller-provided buffer. Returns the number of bytes written.
    pub fn encode(&self, buf: &mut [u8]) -> Option<usize> {
        let text_bytes = self.text.as_bytes();
        let frame_len = HEADER_SIZE + text_bytes.len() + CHECKSUM_SIZE;

        if buf.len() < frame_len {
            return None;
        }

        buf[0] = (TYPE_MESSAGE << 4) | (self.dest & 0x0F);
        buf[1] = self.id;
        buf[2] = text_bytes.len() as u8;
        buf[HEADER_SIZE..HEADER_SIZE + text_bytes.len()].copy_from_slice(text_bytes);
        buf[HEADER_SIZE + text_bytes.len()] = checksum(text_bytes);

        Some(frame_len)
    }
}

impl AckPacket {
    fn parse(raw: &[u8]) -> Result<Self, ProtocolError> {
        if raw.len() < 2 {
            return Err(ProtocolError::TooShort);
        }
        Ok(Self { id: raw[1] })
    }

    pub fn encode(&self, buf: &mut [u8]) -> Option<usize> {
        if buf.len() < 2 {
            return None;
        }
        buf[0] = TYPE_ACK << 4;
        buf[1] = self.id;
        Some(2)
    }
}

pub enum RxResult {
    NewMessage {
        packet: MessagePacket,
        ack_reply: [u8; 2],
    },
    Duplicate {
        ack_reply: [u8; 2],
    },
    AckReceived {
        id: u8,
    },
    UnexpectedAck {
        id: u8,
    },
    ParseError(ProtocolError),
}

pub enum TimeoutAction {
    Retry {
        id: u8,
        wire_payload: Vec<u8, LORA_MAX_PAYLOAD>,
    },
    GiveUp {
        id: u8,
    },
}

// --- Protocol state machine ---

struct PendingAck {
    wire_payload: Vec<u8, LORA_MAX_PAYLOAD>,
    id: u8,
    retries_left: u8,
    deadline: Instant,
}

pub struct Protocol {
    next_seq: u8,
    pending: Vec<PendingAck, MAX_PENDING_ACKS>,
    seen_ids: Vec<u8, DEDUP_WINDOW>,
}

fn encode_ack(id: u8) -> [u8; 2] {
    let mut buf = [0u8; 2];
    AckPacket { id }.encode(&mut buf);
    buf
}

impl Protocol {
    pub fn new() -> Self {
        Self {
            next_seq: 0,
            pending: Vec::new(),
            seen_ids: Vec::new(),
        }
    }

    pub fn on_receive(&mut self, buf: &[u8], len: usize) -> RxResult {
        let raw = match buf.get(..len) {
            Some(r) => r,
            None => return RxResult::ParseError(ProtocolError::TooShort),
        };

        match Packet::parse(raw) {
            Ok(Packet::Message(msg)) => {
                let ack_reply = encode_ack(msg.id);

                if self.is_duplicate(msg.id) {
                    RxResult::Duplicate { ack_reply }
                } else {
                    self.record_seen(msg.id);
                    RxResult::NewMessage {
                        packet: msg,
                        ack_reply,
                    }
                }
            }
            Ok(Packet::Ack(ack)) => {
                if self.remove_pending(ack.id) {
                    RxResult::AckReceived { id: ack.id }
                } else {
                    RxResult::UnexpectedAck { id: ack.id }
                }
            }
            Ok(_) => {
                // TODO: handle Ping, Gps, Resend
                RxResult::ParseError(ProtocolError::InvalidType(0))
            }
            Err(e) => RxResult::ParseError(e),
        }
    }

    pub fn frame_outgoing(
        &mut self,
        dest: u8,
        raw_text: &[u8],
    ) -> Option<(u8, Vec<u8, LORA_MAX_PAYLOAD>)> {
        if self.pending.is_full() {
            return None;
        }

        let text_str = core::str::from_utf8(raw_text).ok()?;
        let text = String::try_from(text_str).ok()?;

        let id = self.next_seq;
        self.next_seq = self.next_seq.wrapping_add(1);

        let msg = MessagePacket { dest, id, text };
        let mut wire: Vec<u8, LORA_MAX_PAYLOAD> = Vec::new();
        wire.resize_default(LORA_MAX_PAYLOAD).ok();
        let len = msg.encode(&mut wire)?;
        wire.truncate(len);

        self.pending
            .push(PendingAck {
                wire_payload: wire.clone(),
                id,
                retries_left: MAX_RETRIES,
                deadline: Instant::now() + ACK_TIMEOUT,
            })
            .ok();

        Some((id, wire))
    }

    // --- Timeout path ---
    pub fn earliest_deadline(&self) -> Option<Instant> {
        self.pending.iter().map(|p| p.deadline).min()
    }

    pub fn process_timeouts(&mut self, now: Instant) -> Vec<TimeoutAction, MAX_PENDING_ACKS> {
        let mut actions: Vec<TimeoutAction, MAX_PENDING_ACKS> = Vec::new();

        let mut i = self.pending.len();
        while i > 0 {
            i -= 1;

            if self.pending[i].deadline > now {
                continue;
            }

            if self.pending[i].retries_left > 0 {
                self.pending[i].retries_left -= 1;
                let attempt = (MAX_RETRIES - self.pending[i].retries_left) as u32;
                self.pending[i].deadline = now + ACK_TIMEOUT * attempt;

                actions
                    .push(TimeoutAction::Retry {
                        id: self.pending[i].id,
                        wire_payload: self.pending[i].wire_payload.clone(),
                    })
                    .ok();
            } else {
                let id = self.pending[i].id;
                self.pending.remove(i);
                actions.push(TimeoutAction::GiveUp { id }).ok();
            }
        }

        actions
    }

    fn is_duplicate(&self, id: u8) -> bool {
        self.seen_ids.contains(&id)
    }

    fn record_seen(&mut self, id: u8) {
        if self.seen_ids.is_full() {
            self.seen_ids.remove(0);
        }
        self.seen_ids.push(id).ok();
    }

    fn remove_pending(&mut self, id: u8) -> bool {
        if let Some(pos) = self.pending.iter().position(|p| p.id == id) {
            self.pending.remove(pos);
            true
        } else {
            false
        }
    }
}
