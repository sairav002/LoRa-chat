/// Radio hardware layer — LoRa configuration, transmit, and receive.
///
/// Owns the SX1276 driver and exposes a minimal async API via `RadioTrait`.
/// Tests can supply a mock implementation against the same trait.
use embassy_time::Delay;
use embedded_hal_bus::spi::ExclusiveDevice;
use esp_hal::{
    gpio::{Input, Output},
    spi::master::Spi,
};
use lora_phy::iv::GenericSx127xInterfaceVariant;
use lora_phy::sx127x::{Sx127x, Sx1276};
use lora_phy::{LoRa, RxMode, mod_params::*, sx127x};

use crate::config::LORA_PREAMBLE_SYMBOLS;

pub type RadioSpi = ExclusiveDevice<Spi<'static, esp_hal::Async>, Output<'static>, Delay>;
pub type RadioIv = GenericSx127xInterfaceVariant<Output<'static>, Input<'static>>;
type RadioDriver = Sx127x<RadioSpi, RadioIv, Sx1276>;

// -- Errors -----------------------------------------------------------------

/// Errors surfaced from radio operations.
///
/// Kept coarse on purpose: the coordinator only needs to decide whether to
/// retry, log, or give up. Detailed driver errors are logged inside the impl.
#[derive(Debug)]
pub enum RadioError {
    /// Configuration step failed (modulation/packet params, settings).
    Config,
    /// Transmit operation failed.
    Tx,
    /// Receive operation failed.
    Rx,
    /// CAD operation failed.
    Cad,
}

// -- Trait ------------------------------------------------------------------

/// Abstraction over the LoRa radio hardware.
///
/// All coordinator logic is written against this trait so that tests can
/// inject a mock without touching real hardware.
pub trait RadioTrait {
    /// Run one Channel Activity Detection cycle.
    /// Returns `true` if preamble activity was detected.
    /// Leaves the radio in standby on completion.
    async fn cad(&mut self) -> Result<bool, RadioError>;

    /// Enter continuous RX and block until one frame is received.
    /// Cancel-safe: dropping the future leaves the radio in continuous RX;
    /// the next `prepare_for_*` call transitions cleanly through standby.
    /// Returns the number of bytes received.
    async fn receive(&mut self, buf: &mut [u8]) -> Result<usize, RadioError>;

    /// Transmit a payload. Leaves the radio in standby on completion.
    async fn send(&mut self, payload: &[u8]) -> Result<(), RadioError>;

    /// Rebuild modulation params from the current `RADIO_SETTINGS` mutex.
    async fn apply_settings(&mut self) -> Result<(), RadioError>;
}

// -- Concrete hardware implementation ---------------------------------------

pub struct Radio {
    lora: LoRa<RadioDriver, Delay>,
    mdltn_params: ModulationParams,
    rx_params: PacketParams,
    tx_params: PacketParams,
    tx_power_dbm: i32,
}

impl Radio {
    pub async fn new(spi_device: RadioSpi, iv: RadioIv) -> Option<Self> {
        let lora_config = sx127x::Config {
            chip: Sx1276,
            tcxo_used: false,
            rx_boost: true,
            tx_boost: true,
        };
        let sx127x = Sx127x::new(spi_device, iv, lora_config);

        let mut lora = match LoRa::new(sx127x, true, Delay).await {
            Ok(l) => l,
            Err(e) => {
                log::error!("LoRa init failed: {:?}", e);
                return None;
            }
        };

        let (sf, bw, cr, freq, tx_power_dbm) = {
            let s = crate::RADIO_SETTINGS.lock().await;
            (
                s.spreading_factor,
                s.bandwidth,
                s.coding_rate,
                s.frequency_hz,
                s.tx_power_dbm,
            )
        };

        let mdltn_params = lora
            .create_modulation_params(sf, bw, cr, freq)
            .map_err(|e| log::error!("Failed to create modulation params: {:?}", e))
            .ok()?;

        let rx_params = lora
            .create_rx_packet_params(
                LORA_PREAMBLE_SYMBOLS,
                false,
                crate::config::LORA_MAX_PAYLOAD as u8,
                true,
                false,
                &mdltn_params,
            )
            .map_err(|e| log::error!("Failed to create RX params: {:?}", e))
            .ok()?;

        let tx_params = lora
            .create_tx_packet_params(LORA_PREAMBLE_SYMBOLS, false, true, false, &mdltn_params)
            .map_err(|e| log::error!("Failed to create TX params: {:?}", e))
            .ok()?;

        Some(Self {
            lora,
            mdltn_params,
            rx_params,
            tx_params,
            tx_power_dbm,
        })
    }
}

impl RadioTrait for Radio {
    async fn receive(&mut self, buf: &mut [u8]) -> Result<usize, RadioError> {
        self.lora
            .prepare_for_rx(RxMode::Continuous, &self.mdltn_params, &self.rx_params)
            .await
            .map_err(|e| {
                log::error!("prepare_for_rx failed: {:?}", e);
                RadioError::Config
            })?;
        self.lora
            .rx(&self.rx_params, buf)
            .await
            .map(|(len, _status)| len as usize)
            .map_err(|e| {
                log::error!("rx failed: {:?}", e);
                RadioError::Rx
            })
    }

    async fn cad(&mut self) -> Result<bool, RadioError> {
        self.lora
            .prepare_for_cad(&self.mdltn_params)
            .await
            .map_err(|e| {
                log::error!("prepare_for_cad failed: {:?}", e);
                RadioError::Cad
            })?;
        self.lora.cad(&self.mdltn_params).await.map_err(|e| {
            log::error!("cad failed: {:?}", e);
            RadioError::Cad
        })
    }

    async fn send(&mut self, payload: &[u8]) -> Result<(), RadioError> {
        self.lora
            .prepare_for_tx(
                &self.mdltn_params,
                &mut self.tx_params,
                self.tx_power_dbm,
                payload,
            )
            .await
            .map_err(|e| {
                log::error!("prepare_for_tx failed: {:?}", e);
                RadioError::Tx
            })?;
        self.lora.tx().await.map_err(|e| {
            log::error!("tx failed: {:?}", e);
            RadioError::Tx
        })
    }

    async fn apply_settings(&mut self) -> Result<(), RadioError> {
        let (sf, bw, cr, freq, tx_power_dbm) = {
            let s = crate::RADIO_SETTINGS.lock().await;
            (
                s.spreading_factor,
                s.bandwidth,
                s.coding_rate,
                s.frequency_hz,
                s.tx_power_dbm,
            )
        };

        let mdltn_params = self.lora.create_modulation_params(sf, bw, cr, freq).map_err(|e| {
            log::error!("apply_settings: modulation params failed: {:?}", e);
            RadioError::Config
        })?;

        let rx_params = self
            .lora
            .create_rx_packet_params(
                LORA_PREAMBLE_SYMBOLS,
                false,
                crate::config::LORA_MAX_PAYLOAD as u8,
                true,
                false,
                &mdltn_params,
            )
            .map_err(|e| {
                log::error!("apply_settings: RX params failed: {:?}", e);
                RadioError::Config
            })?;

        let tx_params = self
            .lora
            .create_tx_packet_params(LORA_PREAMBLE_SYMBOLS, false, true, false, &mdltn_params)
            .map_err(|e| {
                log::error!("apply_settings: TX params failed: {:?}", e);
                RadioError::Config
            })?;

        self.mdltn_params = mdltn_params;
        self.rx_params = rx_params;
        self.tx_params = tx_params;
        self.tx_power_dbm = tx_power_dbm;
        Ok(())
    }
}
