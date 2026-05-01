use embassy_futures::select::{Either4, select4};
use embassy_time::{Duration, Timer};
use heapless::String;

use crate::{
    BLE_CHANNEL, SETTINGS_CHANGED, TX_CHANNEL, UI_CHANNEL,
    ble::BleMessage,
    config::LORA_CAD_SLEEP_MS,
    display::UIEvent,
    protocol::{Protocol, RxResult, TimeoutAction},
    radio::Radio,
};

#[embassy_executor::task]
pub async fn coordinator_task(mut radio: Radio) {
    let mut proto = Protocol::new();
    let mut recv_buf = [0u8; 255];
    let display = UI_CHANNEL.sender();
    let ble_channel = BLE_CHANNEL.sender();

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
        // CAD sleep: how long to wait between channel activity checks.
        let cad_window = Timer::after(Duration::from_millis(LORA_CAD_SLEEP_MS));

        match select4(outgoing, deadline_fut, settings, cad_window).await {
            // ==================================
            // --- Outgoing message ---
            // ==================================
            Either4::First(request) => {
                let mut text = String::<124>::new();
                let _ = text.push_str(str::from_utf8(&request.payload).unwrap());

                match text.as_str() {
                    "up" => display.send(UIEvent::NavUp).await,
                    "down" => display.send(UIEvent::NavDown).await,
                    "menu" => display.send(UIEvent::NavMenu).await,
                    _ => match proto.frame_outgoing(0, &request.payload) {
                        Some((id, wire)) => {
                            log::info!("packet: {wire:?}");
                            display.send(UIEvent::MessageSent { id, text }).await;
                            radio.send(&wire).await;
                        }
                        None => {
                            log::warn!("Pending ACK queue full, dropping TX request");
                        }
                    },
                }
            }

            // ==================================
            // --- ACK Timeout ---
            // ==================================
            Either4::Second(_) => {
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

            // ==================================
            // --- Radio Settings Changed ---
            // ==================================
            Either4::Third(_) => {
                if radio.apply_settings().await {
                    log::info!("Radio settings applied");
                }
            }

            // ==================================
            // --- CAD window: poll for channel activity ---
            // ==================================
            Either4::Fourth(_) => {
                if radio.cad().await {
                    // Preamble detected — switch to full RX to receive the packet.
                    if radio.enter_rx().await {
                        let (len, _status) = radio.receive(&mut recv_buf).await;
                        match proto.on_receive(&recv_buf, len as usize) {
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
                }
            }
        }
    }
}
