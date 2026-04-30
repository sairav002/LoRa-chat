use heapless::{String, Vec};

/// Events sent to the display task
pub enum DisplayEvent {
    /// A new message arrived from a remote node.
    MessageReceived { id: u8, text: String<124> },
    /// Our outgoing message was transmitted (awaiting ACK).
    MessageSent { id: u8, text: String<124> },
    /// A previously sent message was acknowledged by the remote.
    MessageConfirmed { id: u8 },
    /// A previously sent message was given up on after retries.
    MessageDiscarded { id: u8 },
}

/// Raw bytes from the UART, to be framed by the protocol layer.
pub struct TxRequest {
    pub payload: Vec<u8, 124>,
}
