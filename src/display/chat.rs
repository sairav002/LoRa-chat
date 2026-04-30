use embedded_graphics::{
    Drawable,
    mono_font::{MonoTextStyle, ascii::FONT_6X10},
    pixelcolor::BinaryColor,
    prelude::Point,
    text::Text,
};
use heapless::{String, Vec};

use crate::display::Screen;

const MAX_MSGS: usize = 5;

#[derive(Clone, Copy, PartialEq)]
pub enum DeliveryStatus {
    Pending,
    Confirmed,
    Failed,
}

enum ChatEntry {
    Incoming {
        id: u8,
        text: String<124>,
    },
    Outgoing {
        id: u8,
        text: String<124>,
        status: DeliveryStatus,
    },
}

impl ChatEntry {
    fn id(&self) -> u8 {
        match self {
            ChatEntry::Incoming { id, .. } | ChatEntry::Outgoing { id, .. } => *id,
        }
    }
}

pub struct ChatScreen {
    entries: Vec<ChatEntry, MAX_MSGS>,
}

impl Screen for ChatScreen {
    fn render(&mut self, display: &mut super::OledDisplay) {
        let style = MonoTextStyle::new(&FONT_6X10, BinaryColor::On);
        let mut y = 12;

        for entry in &self.entries {
            let mut line: String<132> = String::new();

            match entry {
                ChatEntry::Incoming { text, .. } => {
                    line.push_str("< ").ok();
                    line.push_str(text.as_str()).ok();
                }
                ChatEntry::Outgoing { text, status, .. } => {
                    line.push_str("> ").ok();
                    line.push_str(text.as_str()).ok();
                    match status {
                        DeliveryStatus::Pending => line.push_str(" ~").ok(),
                        DeliveryStatus::Confirmed => line.push_str(" +").ok(),
                        DeliveryStatus::Failed => line.push_str(" x").ok(),
                    };
                }
            }

            let _ = Text::new(line.as_str(), Point::new(0, y), style).draw(display);
            y += 12;
        }
    }

    fn on_event(&mut self, event: &super::UIEvent) -> Option<super::ScreenTransition> {
        match event {
            super::UIEvent::NavMenu => {
                Some(super::ScreenTransition::GoTo(super::ScreenId::Menu))
            }
            super::UIEvent::MessageReceived { id, text } => {
                self.push_incoming(*id, text.clone());
                None
            }
            super::UIEvent::MessageSent { id, text } => {
                self.push_outgoing(*id, text.clone());
                None
            }
            super::UIEvent::MessageConfirmed { id } => {
                self.update_status(*id, DeliveryStatus::Confirmed);
                None
            }
            super::UIEvent::MessageDiscarded { id } => {
                self.update_status(*id, DeliveryStatus::Failed);
                None
            }
            _ => None,
        }
    }
}

impl ChatScreen {
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
        }
    }

    pub fn push_incoming(&mut self, id: u8, text: String<124>) {
        self.evict_oldest_if_full();
        self.entries.push(ChatEntry::Incoming { id, text }).ok();
    }

    pub fn push_outgoing(&mut self, id: u8, text: String<124>) {
        self.evict_oldest_if_full();
        self.entries
            .push(ChatEntry::Outgoing {
                id,
                text,
                status: DeliveryStatus::Pending,
            })
            .ok();
    }

    pub fn update_status(&mut self, id: u8, status: DeliveryStatus) {
        if let Some(entry) = self.entries.iter_mut().find(|e| e.id() == id) {
            if let ChatEntry::Outgoing { status: s, .. } = entry {
                *s = status;
            }
        }
    }

    fn evict_oldest_if_full(&mut self) {
        if self.entries.is_full() {
            self.entries.remove(0);
        }
    }
}
