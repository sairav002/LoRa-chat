use embassy_futures::select::{Either4, select4};
use embassy_time::{Duration, Instant, Timer};
use heapless::{String, Vec};

use crate::{
    BLE_CHANNEL, SETTINGS_CHANGED, TX_CHANNEL, UI_CHANNEL,
    ble::BleMessage,
    config::{LBT_BACKOFF_BASE_MS, LBT_MAX_ATTEMPTS, LORA_MAX_PAYLOAD},
    display::UIEvent,
    events::TxRequest,
    protocol::{MAX_TEXT, MessagePacket, Packet, ProtocolError, encode_ack},
    radio::{RadioError, RadioTrait},
    session::{Session, TimeoutAction},
};

// -- Coordinator errors ---------------------------------------------------------

enum CoordinatorError {
    Radio(RadioError),
    Protocol(ProtocolError),
}

impl From<RadioError> for CoordinatorError {
    fn from(e: RadioError) -> Self {
        CoordinatorError::Radio(e)
    }
}

impl From<ProtocolError> for CoordinatorError {
    fn from(e: ProtocolError) -> Self {
        CoordinatorError::Protocol(e)
    }
}

// -- Coordinator ---------------------------------------------------------------

struct Coordinator<R> {
    radio: R,
    session: Session,
}

impl<R: RadioTrait> Coordinator<R> {
    fn new(radio: R) -> Self {
        Self {
            radio,
            session: Session::new(),
        }
    }

    async fn run(&mut self) -> ! {
        let mut recv_buf = [0u8; LORA_MAX_PAYLOAD];
        log::info!("Coordinator started");

        loop {
            let deadline_fut = async {
                match self.session.earliest_deadline() {
                    Some(dl) => Timer::at(dl).await,
                    None => core::future::pending().await,
                }
            };

            match select4(
                self.radio.receive(&mut recv_buf),
                TX_CHANNEL.receive(),
                deadline_fut,
                SETTINGS_CHANGED.wait(),
            )
            .await
            {
                Either4::First(result) => {
                    if let Err(_) = self.handle_recv(result, &recv_buf).await {
                        log::warn!("RX processing error, continuing");
                    }
                }
                Either4::Second(request) => self.handle_send(request).await,
                Either4::Third(_) => self.handle_timeouts().await,
                Either4::Fourth(_) => {
                    if let Err(_) = self.radio.apply_settings().await {
                        log::error!("apply_settings failed, keeping previous settings");
                    }
                }
            }
        }
    }

    async fn handle_recv(
        &mut self,
        recv_result: Result<usize, RadioError>,
        buf: &[u8],
    ) -> Result<(), CoordinatorError> {
        let len = recv_result?;

        let display = UI_CHANNEL.sender();
        let ble = BLE_CHANNEL.sender();

        match Packet::parse(&buf[..len])? {
            Packet::Message(msg) => {
                let ack = encode_ack(msg.id);
                transmit_lbt(&mut self.radio, &ack).await;

                if self.session.is_duplicate(msg.id) {
                    log::debug!("Duplicate id={}, re-ACKed", msg.id);
                    return Ok(());
                }
                self.session.record(msg.id);

                display
                    .send(UIEvent::MessageReceived {
                        id: msg.id,
                        text: msg.text.clone(),
                    })
                    .await;
                ble.send(BleMessage(msg.text)).await;
            }
            Packet::Ack(ack) => {
                if self.session.on_ack(ack.id) {
                    log::info!("ACK received id={}", ack.id);
                    display.send(UIEvent::MessageConfirmed { id: ack.id }).await;
                } else {
                    log::warn!("Unexpected ACK id={}, ignoring", ack.id);
                }
            }
            _ => log::debug!("Unhandled packet type, ignoring"),
        }

        Ok(())
    }

    async fn handle_send(&mut self, request: TxRequest) {
        let display = UI_CHANNEL.sender();

        match parse_input(&request.payload) {
            InputCommand::NavUp => display.send(UIEvent::NavUp).await,
            InputCommand::NavDown => display.send(UIEvent::NavDown).await,
            InputCommand::NavMenu => display.send(UIEvent::NavMenu).await,
            InputCommand::Message(raw) => {
                let text_str = match core::str::from_utf8(raw) {
                    Ok(s) => s,
                    Err(_) => {
                        log::warn!("Outgoing payload is not valid UTF-8, dropping");
                        return;
                    }
                };

                let text: String<MAX_TEXT> = match String::try_from(text_str) {
                    Ok(t) => t,
                    Err(_) => {
                        log::warn!("Outgoing payload too long, dropping");
                        return;
                    }
                };

                let id = self.session.next_id();
                let msg = MessagePacket::new(0, id, text.clone());

                let mut wire: Vec<u8, LORA_MAX_PAYLOAD> = Vec::new();
                let _ = wire.resize_default(LORA_MAX_PAYLOAD);
                let len = match msg.encode(&mut wire) {
                    Some(l) => l,
                    None => {
                        log::warn!("Failed to encode outgoing message, dropping");
                        return;
                    }
                };
                wire.truncate(len);

                if !self.session.track(id, wire.clone()) {
                    log::warn!("Pending ACK queue full, dropping TX request");
                    return;
                }

                log::info!("TX id={} len={}", id, len);
                display.send(UIEvent::MessageSent { id, text }).await;
                transmit_lbt(&mut self.radio, &wire).await;
            }
        }
    }

    async fn handle_timeouts(&mut self) {
        let display = UI_CHANNEL.sender();
        let actions = self.session.process_timeouts(Instant::now());

        for action in actions {
            match action {
                TimeoutAction::Retry { id, wire } => {
                    log::warn!("ACK timeout id={}, retransmitting", id);
                    transmit_lbt(&mut self.radio, &wire).await;
                }
                TimeoutAction::GiveUp { id } => {
                    log::error!("Gave up id={}", id);
                    display.send(UIEvent::MessageDiscarded { id }).await;
                }
            }
        }
    }
}

// -- Input parsing -------------------------------------------------------------

pub enum InputCommand<'a> {
    NavUp,
    NavDown,
    NavMenu,
    Message(&'a [u8]),
}

pub fn parse_input(payload: &[u8]) -> InputCommand<'_> {
    match payload {
        b"up" => InputCommand::NavUp,
        b"down" => InputCommand::NavDown,
        b"menu" => InputCommand::NavMenu,
        other => InputCommand::Message(other),
    }
}

// -- Listen Before Talk --------------------------------------------------------

/// CSMA-style transmit: run CAD, transmit if clear, otherwise back off and retry.
async fn transmit_lbt<R: RadioTrait>(radio: &mut R, payload: &[u8]) -> bool {
    for attempt in 0..LBT_MAX_ATTEMPTS {
        match radio.cad().await {
            Ok(false) => return radio.send(payload).await.is_ok(),
            Ok(true) => {
                let backoff = lbt_backoff_ms(attempt);
                log::debug!("Channel busy, backing off {} ms", backoff);
                Timer::after(Duration::from_millis(backoff)).await;
            }
            Err(_) => return false,
        }
    }
    log::warn!("LBT gave up after {} attempts", LBT_MAX_ATTEMPTS);
    false
}

fn lbt_backoff_ms(attempt: u8) -> u64 {
    let base = LBT_BACKOFF_BASE_MS << attempt.min(5);
    let jitter = (Instant::now().as_ticks() & 0x3F) as u64;
    base + jitter
}

// -- Task entry point ----------------------------------------------------------

#[embassy_executor::task]
pub async fn coordinator_task(radio: crate::radio::Radio) {
    Coordinator::new(radio).run().await;
}
