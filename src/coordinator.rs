use embassy_futures::select::{Either4, select4};
use embassy_time::{Duration, Timer};
use heapless::String;

use crate::{
    BLE_CHANNEL, SETTINGS_CHANGED, TX_CHANNEL, UI_CHANNEL,
    ble::BleMessage,
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
        if !radio.enter_rx().await {
            Timer::after(Duration::from_millis(100)).await;
            continue;
        }
        let deadline = proto.earliest_deadline();

        let incoming = radio.receive(&mut recv_buf);
        let outgoing = TX_CHANNEL.receive();
        let deadline = async {
            match deadline {
                Some(dl) => Timer::at(dl).await,
                None => core::future::pending().await,
            }
        };
        let settings = SETTINGS_CHANGED.wait();

        // Await on 4 possible states
        match select4(incoming, outgoing, deadline, settings).await {
            // ==================================
            // --- Received a packet ---
            // ==================================
            Either4::First((len, _status)) => match proto.on_receive(&recv_buf, len as usize) {
                // A new Message has been received
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
                // The message has already been received. The ACK is resent.
                RxResult::Duplicate { ack_reply } => {
                    radio.send(&ack_reply).await;
                    log::debug!("Duplicate message, re-ACKed");
                }
                // An ACK for certain message has been confirmed.
                RxResult::AckReceived { id } => {
                    log::info!("ACK received for id={}", id);
                    display.send(UIEvent::MessageConfirmed { id }).await;
                }
                // A non-registered ACK has been received
                RxResult::UnexpectedAck { id } => {
                    log::warn!("Unexpected ACK for id={}, ignoring", id);
                }
                RxResult::ParseError(e) => {
                    log::error!("Malformed frame: {:?}", e);
                }
            },

            // ==================================
            // --- Outgoing message from UART (but any source in the future) ---
            // ==================================
            Either4::Second(request) => {
                let mut text = String::<124>::new();
                let _ = text.push_str(str::from_utf8(&request.payload).unwrap());

                match text.as_str() {
                    "up" => display.send(UIEvent::NavUp).await,
                    "down" => display.send(UIEvent::NavDown).await,
                    "menu" => display.send(UIEvent::NavMenu).await,
                    _ => match proto.frame_outgoing(0, &request.payload) {
                        Some((id, wire)) => {
                            log::info!("packet: {wire:?}");
                            // Show the message immediately (pending status).
                            // Confirmed/Discarded events update it after ACK or timeout.
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
            Either4::Third(_) => {
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
            // --- Radio Settings ---
            // ==================================
            Either4::Fourth(_) => {
                if radio.apply_settings().await {
                    log::info!("Radio settings applied");
                }
            }
        }
    }
}
