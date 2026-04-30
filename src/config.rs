/// Centralized configuration.
///
/// Pin assignments target the TTGO LoRa32 V2.1 board.
/// Schematic ref: http://www.lilygo.cn/prod_view.aspx?Id=1270
///
/// Edit this file when changing wiring or radio settings.
/// No other file should contain magic numbers.

// ── Radio (SX1276 over SPI2) ─────────────────────────────────────────
pub const LORA_FREQUENCY_HZ: u32 = 868_000_000;
pub const LORA_MAX_PAYLOAD: usize = 128;
pub const LORA_PREAMBLE_SYMBOLS: u16 = 8;

// ── Channels ──────────────────────────────────────────────────────────
/// Depth of the display event queue (radio/UART → display task).
pub const DISPLAY_CHANNEL_SIZE: usize = 8;
/// Depth of the TX request queue (UART/BLE → radio task).
pub const TX_CHANNEL_SIZE: usize = 4;
/// Depth of the BLE notify queue (coordinator → BLE task).
pub const BLE_CHANNEL_SIZE: usize = 4;

// ── BLE ───────────────────────────────────────────────────────────────
/// Name advertised over BLE. Shown in phone scanners (nRF Connect, etc.).
pub const BLE_DEVICE_NAME: &str = "LilyGoLoRa";
/// Static random BLE address. Last octet must have top two bits set (0b11).
pub const BLE_ADDRESS: [u8; 6] = [0xff, 0x8f, 0x1a, 0x05, 0xe4, 0xff];
/// Max payload bytes per BLE write forwarded into TX_CHANNEL.
/// Matches the 124-byte TxRequest payload cap.
pub const BLE_MAX_PAYLOAD: usize = 124;
