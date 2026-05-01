/// Radio hardware layer — LoRa configuration, transmit, and receive.
///
/// This module owns the SPI/LoRa driver and exposes a minimal API via
/// `RadioTrait`. The concrete `Radio` type implements that trait against real
/// hardware; tests can supply a mock implementation instead.
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

// ── Trait ─────────────────────────────────────────────────────────────

/// Abstraction over the LoRa radio hardware.
///
/// All coordinator logic is written against this trait so that tests can
/// inject a mock without touching real hardware.
pub trait RadioTrait {
    /// Run one Channel Activity Detection cycle.
    /// Returns `true` if preamble activity was detected.
    async fn cad(&mut self) -> bool;

    /// Put the radio into continuous receive mode.
    /// Returns `false` on hardware error.
    async fn enter_rx(&mut self) -> bool;

    /// Wait for one incoming frame. Must call `enter_rx()` first.
    /// Returns `(length, success)`.
    async fn receive(&mut self, buf: &mut [u8]) -> (u8, bool);

    /// Transmit a payload.
    /// Returns `false` on hardware error.
    async fn send(&mut self, payload: &[u8]) -> bool;

    /// Rebuild modulation params from the current `RADIO_SETTINGS` mutex.
    /// Returns `false` on hardware error.
    async fn apply_settings(&mut self) -> bool;
}

// ── Concrete hardware implementation ──────────────────────────────────

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
    async fn cad(&mut self) -> bool {
        if let Err(e) = self.lora.prepare_for_cad(&self.mdltn_params).await {
            log::error!("CAD prepare failed: {:?}", e);
            return false;
        }
        match self.lora.cad(&self.mdltn_params).await {
            Ok(detected) => detected,
            Err(e) => {
                log::error!("CAD failed: {:?}", e);
                false
            }
        }
    }

    async fn enter_rx(&mut self) -> bool {
        if let Err(e) = self
            .lora
            .prepare_for_rx(RxMode::Continuous, &self.mdltn_params, &self.rx_params)
            .await
        {
            log::error!("Failed to enter RX mode: {:?}", e);
            return false;
        }
        true
    }

    async fn receive(&mut self, buf: &mut [u8]) -> (u8, bool) {
        match self.lora.rx(&self.rx_params, buf).await {
            Ok((len, _status)) => {
                log::debug!("PACKET");
                (len, true)
            }
            Err(e) => {
                log::error!("RX failed: {:?}", e);
                (0, false)
            }
        }
    }

    async fn send(&mut self, payload: &[u8]) -> bool {
        if let Err(e) = self
            .lora
            .prepare_for_tx(
                &self.mdltn_params,
                &mut self.tx_params,
                self.tx_power_dbm,
                payload,
            )
            .await
        {
            log::error!("prepare_for_tx failed: {:?}", e);
            return false;
        }

        if let Err(e) = self.lora.tx().await {
            log::error!("tx failed: {:?}", e);
            return false;
        }

        true
    }

    async fn apply_settings(&mut self) -> bool {
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

        let mdltn_params = match self.lora.create_modulation_params(sf, bw, cr, freq) {
            Ok(p) => p,
            Err(e) => {
                log::error!("apply_settings: modulation params failed: {:?}", e);
                return false;
            }
        };

        let rx_params = match self.lora.create_rx_packet_params(
            LORA_PREAMBLE_SYMBOLS,
            false,
            crate::config::LORA_MAX_PAYLOAD as u8,
            true,
            false,
            &mdltn_params,
        ) {
            Ok(p) => p,
            Err(e) => {
                log::error!("apply_settings: RX params failed: {:?}", e);
                return false;
            }
        };

        let tx_params = match self.lora.create_tx_packet_params(
            LORA_PREAMBLE_SYMBOLS,
            false,
            true,
            false,
            &mdltn_params,
        ) {
            Ok(p) => p,
            Err(e) => {
                log::error!("apply_settings: TX params failed: {:?}", e);
                return false;
            }
        };

        self.mdltn_params = mdltn_params;
        self.rx_params = rx_params;
        self.tx_params = tx_params;
        self.tx_power_dbm = tx_power_dbm;
        true
    }
}
