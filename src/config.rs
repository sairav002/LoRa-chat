/// Centralized configuration.
///
/// Pin assignments target the TTGO LoRa32 V2.1 board.
/// Schematic ref: http://www.lilygo.cn/prod_view.aspx?Id=1270
///
/// Edit this file when changing wiring or radio settings.
/// No other file should contain magic numbers.

// -- Radio (SX1276 over SPI2) -----------------------------------------------
pub const LORA_FREQUENCY_HZ: u32 = 868_000_000;
pub const LORA_MAX_PAYLOAD: usize = 128;
/// Preamble length for both TX and RX.
///
/// CAD polling requires the sender's preamble to outlast one full poll cycle.
/// The coordinator computes the poll interval from the active SF/BW, keeping
/// a 50% safety margin. Increasing this allows a longer (safer) poll interval.
///
/// At SF12/BW125: 16 symbols = 524 ms preamble, supports up to ~230 ms polling.
/// At SF7/BW125:  16 symbols =  16 ms preamble — CAD polling is not viable at low SF.
pub const LORA_PREAMBLE_SYMBOLS: u16 = 16;

// -- Listen Before Talk (CSMA) ----------------------------------------------
/// Max CAD attempts before giving up on TX (channel persistently busy).
pub const LBT_MAX_ATTEMPTS: u8 = 5;
/// Base backoff between busy CAD detections (milliseconds).
/// Doubles per attempt with jitter, capped by attempt 5.
pub const LBT_BACKOFF_BASE_MS: u64 = 50;

// -- Session ----------------------------------------------------------------
/// How many outgoing frames can await an ACK simultaneously.
pub const MAX_PENDING_ACKS: usize = 5;
/// How many recently seen message IDs to remember for deduplication.
pub const DEDUP_WINDOW: usize = 16;
/// How long to wait for an ACK before retransmitting (milliseconds).
pub const ACK_TIMEOUT_MS: u64 = 5_000;
/// Maximum number of retransmit attempts before giving up.
pub const MAX_RETRIES: u8 = 2;

// -- Channels ---------------------------------------------------------------
/// Depth of the display event queue (radio/UART → display task).
pub const DISPLAY_CHANNEL_SIZE: usize = 8;
/// Depth of the TX request queue (UART/BLE → radio task).
pub const TX_CHANNEL_SIZE: usize = 4;
/// Depth of the BLE notify queue (coordinator → BLE task).
pub const BLE_CHANNEL_SIZE: usize = 4;

// -- BLE --------------------------------------------------------------------
/// Name advertised over BLE. Shown in phone scanners (nRF Connect, etc.).
pub const BLE_DEVICE_NAME: &str = "LilyGoLoRa";
/// Static random BLE address. Last octet must have top two bits set (0b11).
pub const BLE_ADDRESS: [u8; 6] = [0xff, 0x8f, 0x1a, 0x05, 0xe4, 0xff];
/// Max payload bytes per BLE write forwarded into TX_CHANNEL.
/// Matches the 124-byte TxRequest payload cap.
pub const BLE_MAX_PAYLOAD: usize = 124;
