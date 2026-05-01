use embassy_futures::select::{Either4, select4};
use embassy_time::{Duration, Timer};
use heapless::String;

use crate::{
    BLE_CHANNEL, SETTINGS_CHANGED, TX_CHANNEL, UI_CHANNEL,
    ble::BleMessage,
    config::LORA_CAD_SLEEP_MS,
    display::UIEvent,
    events::TxRequest,
    protocol::{Protocol, RxResult, TimeoutAction},
    radio::RadioTrait,
};

// ── Input parsing ──────────────────────────────────────────────────────

/// Classify a raw input payload before it reaches the coordinator.
///
/// Navigation commands come from UART/BLE as plain text but are UI concerns,
/// not radio/protocol concerns. Separating them here keeps the coordinator
/// arms focused on a single job each.
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

// ── Coordinator arms ───────────────────────────────────────────────────

/// Handle one outgoing input request: dispatch nav events or frame and transmit
/// a message.
async fn handle_outgoing<R: RadioTrait>(
    request: TxRequest,
    proto: &mut Protocol,
    radio: &mut R,
) {
    let display = UI_CHANNEL.sender();

    match parse_input(&request.payload) {
        InputCommand::NavUp => display.send(UIEvent::NavUp).await,
        InputCommand::NavDown => display.send(UIEvent::NavDown).await,
        InputCommand::NavMenu => display.send(UIEvent::NavMenu).await,
        InputCommand::Message(raw) => {
            let text = match core::str::from_utf8(raw) {
                Ok(s) => {
                    let mut t = String::<124>::new();
                    let _ = t.push_str(s);
                    t
                }
                Err(_) => {
                    log::warn!("Outgoing payload is not valid UTF-8, dropping");
                    return;
                }
            };

            match proto.frame_outgoing(0, raw) {
                Some((id, wire)) => {
                    log::info!("packet: {wire:?}");
                    display.send(UIEvent::MessageSent { id, text }).await;
                    radio.send(&wire).await;
                }
                None => log::warn!("Pending ACK queue full, dropping TX request"),
            }
        }
    }
}

/// Run one CAD poll. If activity is detected, receive one packet, run it
/// through the protocol state machine, and dispatch results.
async fn handle_cad<R: RadioTrait>(proto: &mut Protocol, radio: &mut R, buf: &mut [u8]) {
    if !radio.cad().await {
        return;
    }

    if !radio.enter_rx().await {
        return;
    }

    let (len, ok) = radio.receive(buf).await;
    if !ok {
        return;
    }

    let display = UI_CHANNEL.sender();
    let ble_channel = BLE_CHANNEL.sender();

    match proto.on_receive(buf, len as usize) {
        RxResult::NewMessage { packet, ack_reply } => {
            radio.send(&ack_reply).await;
            display
                .send(UIEvent::MessageReceived {
                    id: packet.id,
                    text: packet.text.clone(),
                })
                .await;
            ble_channel.send(BleMessage(packet.text)).await;
        }
        RxResult::Duplicate { ack_reply } => {
            radio.send(&ack_reply).await;
            log::debug!("Duplicate message, re-ACKed");
        }
        RxResult::AckReceived { id } => {
            log::info!("ACK received for id={}", id);
            display.send(UIEvent::MessageConfirmed { id }).await;
        }
        RxResult::UnexpectedAck { id } => {
            log::warn!("Unexpected ACK for id={}, ignoring", id);
        }
        RxResult::ParseError(e) => {
            log::error!("Malformed frame: {:?}", e);
        }
    }
}

/// Process any expired ACK deadlines and retransmit or give up.
async fn handle_timeouts<R: RadioTrait>(proto: &mut Protocol, radio: &mut R) {
    let display = UI_CHANNEL.sender();
    let actions = proto.process_timeouts(embassy_time::Instant::now());

    for action in actions {
        match action {
            TimeoutAction::Retry { id, wire_payload } => {
                log::warn!("ACK timeout for id={}, retransmitting", id);
                radio.send(&wire_payload).await;
            }
            TimeoutAction::GiveUp { id } => {
                log::error!("Gave up on id={}", id);
                display.send(UIEvent::MessageDiscarded { id }).await;
            }
        }
    }
}

// ── Task entry point ───────────────────────────────────────────────────

#[embassy_executor::task]
pub async fn coordinator_task(mut radio: crate::radio::Radio) {
    run_coordinator(&mut radio).await;
}

/// Core coordinator loop, generic over any `RadioTrait` implementation.
///
/// Separated from the `#[embassy_executor::task]` entry point so it can be
/// driven in tests with a mock radio.
pub async fn run_coordinator<R: RadioTrait>(radio: &mut R) {
    let mut proto = Protocol::new();
    let mut recv_buf = [0u8; 255];

    log::info!("Coordinator started");

    loop {
        let deadline = proto.earliest_deadline();

        let outgoing = TX_CHANNEL.receive();
        let deadline_fut = async {
            match deadline {
                Some(dl) => Timer::at(dl).await,
                None => core::future::pending().await,
            }
        };
        let settings = SETTINGS_CHANGED.wait();
        let cad_window = Timer::after(Duration::from_millis(LORA_CAD_SLEEP_MS));

        match select4(outgoing, deadline_fut, settings, cad_window).await {
            Either4::First(request) => handle_outgoing(request, &mut proto, radio).await,
            Either4::Second(_) => handle_timeouts(&mut proto, radio).await,
            Either4::Third(_) => {
                if radio.apply_settings().await {
                    log::info!("Radio settings applied");
                }
            }
            Either4::Fourth(_) => handle_cad(&mut proto, radio, &mut recv_buf).await,
        }
    }
}
