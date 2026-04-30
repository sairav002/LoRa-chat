use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, mutex::Mutex, signal::Signal};
use lora_phy::mod_params::{Bandwidth, CodingRate, SpreadingFactor};

use crate::config::LORA_FREQUENCY_HZ;

pub struct RadioSettings {
    pub spreading_factor: SpreadingFactor,
    pub bandwidth: Bandwidth,
    pub coding_rate: CodingRate,
    pub frequency_hz: u32,
    pub tx_power_dbm: i32,
}

impl RadioSettings {
    const fn default() -> Self {
        Self {
            spreading_factor: SpreadingFactor::_12,
            bandwidth: Bandwidth::_125KHz,
            coding_rate: CodingRate::_4_5,
            frequency_hz: LORA_FREQUENCY_HZ,
            tx_power_dbm: 22,
        }
    }
}

pub static RADIO_SETTINGS: Mutex<CriticalSectionRawMutex, RadioSettings> =
    Mutex::new(RadioSettings::default());

/// Signalled (with `()`) whenever the menu commits a settings change.
/// The coordinator waits on this and calls `radio.apply_settings()`.
pub static SETTINGS_CHANGED: Signal<CriticalSectionRawMutex, ()> = Signal::new();
