use embedded_graphics::{
    Drawable,
    mono_font::{MonoTextStyle, ascii::FONT_6X10},
    pixelcolor::BinaryColor,
    prelude::Point,
    text::Text,
};
use lora_phy::mod_params::{Bandwidth, CodingRate, SpreadingFactor};

use crate::display::{Screen, ScreenTransition, UIEvent};
use crate::settings::{RADIO_SETTINGS, SETTINGS_CHANGED};

// ── Editable fields ──────────────────────────────────────────────────────────

#[derive(Clone, Copy)]
enum Field {
    SpreadingFactor,
    TxPower,
    Bandwidth,
    CodingRate,
}

const FIELDS: &[Field] = &[
    Field::SpreadingFactor,
    Field::TxPower,
    Field::Bandwidth,
    Field::CodingRate,
];

// ── Menu state ───────────────────────────────────────────────────────────────

pub struct MenuScreen {
    cursor: usize,
    // Local draft — written to RADIO_SETTINGS only on NavSelect ("Save & Back")
    sf: SpreadingFactor,
    tx_dbm: i32,
    bw: Bandwidth,
    cr: CodingRate,
}

impl MenuScreen {
    pub fn new() -> Self {
        // Snapshot current settings for the draft. try_lock succeeds here
        // because no async task holds the mutex during a sync screen init.
        let (sf, bw, cr, tx_dbm) = RADIO_SETTINGS
            .try_lock()
            .map(|s| (s.spreading_factor, s.bandwidth, s.coding_rate, s.tx_power_dbm))
            .unwrap_or_else(|_| (
                SpreadingFactor::_12,
                Bandwidth::_125KHz,
                CodingRate::_4_5,
                22,
            ));

        Self { cursor: 0, sf, tx_dbm, bw, cr }
    }

    fn commit(&self) {
        if let Ok(mut s) = RADIO_SETTINGS.try_lock() {
            s.spreading_factor = self.sf;
            s.bandwidth = self.bw;
            s.coding_rate = self.cr;
            s.tx_power_dbm = self.tx_dbm;
        }
        SETTINGS_CHANGED.signal(());
    }

    fn increment(&mut self) {
        match FIELDS[self.cursor] {
            Field::SpreadingFactor => self.sf = sf_next(self.sf),
            Field::TxPower => self.tx_dbm = (self.tx_dbm + 1).min(22),
            Field::Bandwidth => self.bw = bw_next(self.bw),
            Field::CodingRate => self.cr = cr_next(self.cr),
        }
    }

    fn _decrement(&mut self) {
        match FIELDS[self.cursor] {
            Field::SpreadingFactor => self.sf = _sf_prev(self.sf),
            Field::TxPower => self.tx_dbm = (self.tx_dbm - 1).max(2),
            Field::Bandwidth => self.bw = _bw_prev(self.bw),
            Field::CodingRate => self.cr = _cr_prev(self.cr),
        }
    }
}

impl Screen for MenuScreen {
    fn render(&mut self, display: &mut super::OledDisplay) {
        let style = MonoTextStyle::new(&FONT_6X10, BinaryColor::On);

        let _ = Text::new("Radio Settings", Point::new(4, 10), style).draw(display);

        let rows: [(&str, [u8; 12]); 4] = [
            ("SF ", sf_label(self.sf)),
            ("dBm", dbm_label(self.tx_dbm)),
            ("BW ", bw_label(self.bw)),
            ("CR ", cr_label(self.cr)),
        ];

        for (i, (key, val)) in rows.iter().enumerate() {
            let y = 22 + (i as i32) * 11;
            let prefix = if i == self.cursor { ">" } else { " " };

            let mut line: heapless::String<32> = heapless::String::new();
            line.push_str(prefix).ok();
            line.push_str(key).ok();
            line.push_str(": ").ok();
            line.push_str(core::str::from_utf8(val).unwrap_or("?")).ok();

            let _ = Text::new(line.as_str(), Point::new(0, y), style).draw(display);
        }
    }

    fn on_event(&mut self, event: &UIEvent) -> Option<ScreenTransition> {
        match event {
            UIEvent::NavUp => {
                if self.cursor > 0 {
                    self.cursor -= 1;
                }
                None
            }
            UIEvent::NavDown => {
                if self.cursor + 1 < FIELDS.len() {
                    self.cursor += 1;
                }
                None
            }
            // Left/Right (or NavBack/NavMenu reused for now) adjust the value.
            UIEvent::NavSelect => {
                self.increment();
                None
            }
            UIEvent::NavBack => {
                // Discard draft, return to chat without saving.
                Some(ScreenTransition::Back)
            }
            UIEvent::NavMenu => {
                // Second press of the menu button = save & exit.
                self.commit();
                Some(ScreenTransition::Back)
            }
            _ => None,
        }
    }
}

// ── Cycle helpers ────────────────────────────────────────────────────────────

fn sf_next(sf: SpreadingFactor) -> SpreadingFactor {
    match sf {
        SpreadingFactor::_5 => SpreadingFactor::_6,
        SpreadingFactor::_6 => SpreadingFactor::_7,
        SpreadingFactor::_7 => SpreadingFactor::_8,
        SpreadingFactor::_8 => SpreadingFactor::_9,
        SpreadingFactor::_9 => SpreadingFactor::_10,
        SpreadingFactor::_10 => SpreadingFactor::_11,
        SpreadingFactor::_11 => SpreadingFactor::_12,
        SpreadingFactor::_12 => SpreadingFactor::_5,
    }
}

fn _sf_prev(sf: SpreadingFactor) -> SpreadingFactor {
    match sf {
        SpreadingFactor::_5 => SpreadingFactor::_12,
        SpreadingFactor::_6 => SpreadingFactor::_5,
        SpreadingFactor::_7 => SpreadingFactor::_6,
        SpreadingFactor::_8 => SpreadingFactor::_7,
        SpreadingFactor::_9 => SpreadingFactor::_8,
        SpreadingFactor::_10 => SpreadingFactor::_9,
        SpreadingFactor::_11 => SpreadingFactor::_10,
        SpreadingFactor::_12 => SpreadingFactor::_11,
    }
}

fn bw_next(bw: Bandwidth) -> Bandwidth {
    match bw {
        Bandwidth::_7KHz => Bandwidth::_10KHz,
        Bandwidth::_10KHz => Bandwidth::_15KHz,
        Bandwidth::_15KHz => Bandwidth::_20KHz,
        Bandwidth::_20KHz => Bandwidth::_31KHz,
        Bandwidth::_31KHz => Bandwidth::_41KHz,
        Bandwidth::_41KHz => Bandwidth::_62KHz,
        Bandwidth::_62KHz => Bandwidth::_125KHz,
        Bandwidth::_125KHz => Bandwidth::_250KHz,
        Bandwidth::_250KHz => Bandwidth::_500KHz,
        Bandwidth::_500KHz => Bandwidth::_7KHz,
    }
}

fn _bw_prev(bw: Bandwidth) -> Bandwidth {
    match bw {
        Bandwidth::_7KHz => Bandwidth::_500KHz,
        Bandwidth::_10KHz => Bandwidth::_7KHz,
        Bandwidth::_15KHz => Bandwidth::_10KHz,
        Bandwidth::_20KHz => Bandwidth::_15KHz,
        Bandwidth::_31KHz => Bandwidth::_20KHz,
        Bandwidth::_41KHz => Bandwidth::_31KHz,
        Bandwidth::_62KHz => Bandwidth::_41KHz,
        Bandwidth::_125KHz => Bandwidth::_62KHz,
        Bandwidth::_250KHz => Bandwidth::_125KHz,
        Bandwidth::_500KHz => Bandwidth::_250KHz,
    }
}

fn cr_next(cr: CodingRate) -> CodingRate {
    match cr {
        CodingRate::_4_5 => CodingRate::_4_6,
        CodingRate::_4_6 => CodingRate::_4_7,
        CodingRate::_4_7 => CodingRate::_4_8,
        CodingRate::_4_8 => CodingRate::_4_5,
    }
}

fn _cr_prev(cr: CodingRate) -> CodingRate {
    match cr {
        CodingRate::_4_5 => CodingRate::_4_8,
        CodingRate::_4_6 => CodingRate::_4_5,
        CodingRate::_4_7 => CodingRate::_4_6,
        CodingRate::_4_8 => CodingRate::_4_7,
    }
}

// ── Label renderers (no alloc, fixed-width byte arrays) ──────────────────────

fn sf_label(sf: SpreadingFactor) -> [u8; 12] {
    let mut buf = *b"SF??        ";
    let n = match sf {
        SpreadingFactor::_5 => b"SF5 ",
        SpreadingFactor::_6 => b"SF6 ",
        SpreadingFactor::_7 => b"SF7 ",
        SpreadingFactor::_8 => b"SF8 ",
        SpreadingFactor::_9 => b"SF9 ",
        SpreadingFactor::_10 => b"SF10",
        SpreadingFactor::_11 => b"SF11",
        SpreadingFactor::_12 => b"SF12",
    };
    buf[..4].copy_from_slice(n);
    buf
}

fn dbm_label(dbm: i32) -> [u8; 12] {
    let mut buf = *b"            ";
    // Write decimal manually — no format! in no_std without alloc
    let abs = dbm.unsigned_abs();
    let hundreds = (abs / 100) as u8;
    let tens = ((abs / 10) % 10) as u8;
    let units = (abs % 10) as u8;
    let mut i = 0usize;
    if dbm < 0 { buf[i] = b'-'; i += 1; }
    if hundreds > 0 { buf[i] = b'0' + hundreds; i += 1; }
    if hundreds > 0 || tens > 0 { buf[i] = b'0' + tens; i += 1; }
    buf[i] = b'0' + units; i += 1;
    buf[i] = b'd'; i += 1;
    buf[i] = b'B'; i += 1;
    buf[i] = b'm';
    buf
}

fn bw_label(bw: Bandwidth) -> [u8; 12] {
    let mut buf = *b"            ";
    let s: &[u8] = match bw {
        Bandwidth::_7KHz => b"7.8kHz",
        Bandwidth::_10KHz => b"10.4kHz",
        Bandwidth::_15KHz => b"15.6kHz",
        Bandwidth::_20KHz => b"20.8kHz",
        Bandwidth::_31KHz => b"31.25kHz",
        Bandwidth::_41KHz => b"41.7kHz",
        Bandwidth::_62KHz => b"62.5kHz",
        Bandwidth::_125KHz => b"125kHz",
        Bandwidth::_250KHz => b"250kHz",
        Bandwidth::_500KHz => b"500kHz",
    };
    let len = s.len().min(12);
    buf[..len].copy_from_slice(&s[..len]);
    buf
}

fn cr_label(cr: CodingRate) -> [u8; 12] {
    let mut buf = *b"            ";
    let s: &[u8] = match cr {
        CodingRate::_4_5 => b"4/5",
        CodingRate::_4_6 => b"4/6",
        CodingRate::_4_7 => b"4/7",
        CodingRate::_4_8 => b"4/8",
    };
    buf[..s.len()].copy_from_slice(s);
    buf
}
