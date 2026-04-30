/// UART task — reads newline-delimited text from a serial port and
/// submits raw text as TX requests to the radio task.
///
/// The radio task is responsible for assigning sequence IDs and
/// framing the wire payload.
///
/// Lines longer than 128 bytes are truncated.
use esp_hal::uart::Uart;
use heapless::Vec;

use crate::{TX_CHANNEL, events::TxRequest};

const MAX_LINE: usize = 120;

#[embassy_executor::task]
pub async fn uart_task(uart: Uart<'static, esp_hal::Async>) {
    let sender = TX_CHANNEL.sender();

    let mut read_buf = [0u8; 64];
    let mut line_buf = [0u8; MAX_LINE];
    let mut line_pos: usize = 0;

    let (mut uart_rx, _uart_tx) = uart.split();

    log::info!("UART task started");

    loop {
        match uart_rx.read_async(&mut read_buf).await {
            Ok(n) => {
                for &b in &read_buf[..n] {
                    if b == b'\n' || b == b'\r' {
                        if line_pos == 0 {
                            continue;
                        }

                        let mut payload: Vec<u8, 124> = Vec::new();
                        let _ = payload.extend_from_slice(&line_buf[..line_pos]);

                        sender.send(TxRequest { payload }).await;
                        line_pos = 0;
                    } else if line_pos < line_buf.len() {
                        line_buf[line_pos] = b;
                        line_pos += 1;
                    }
                }
            }
            Err(e) => {
                log::error!("UART read error: {:?}", e);
                line_pos = 0;
            }
        }
    }
}
