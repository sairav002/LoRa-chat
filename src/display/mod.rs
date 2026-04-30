use embassy_time::{Duration, Timer};
use embedded_graphics::{pixelcolor::BinaryColor, prelude::DrawTarget};
use esp_hal::i2c::master::I2c;
use heapless::String;
use ssd1306::{Ssd1306, mode::DisplayConfig, prelude::I2CInterface, size::DisplaySize128x64};

use chat::ChatScreen;
use init::InitScreen;
use menu::MenuScreen;

mod chat;
mod init;
mod menu;

const SPLASH_DURATION: Duration = Duration::from_secs(5);

pub type OledDisplay = Ssd1306<
    I2CInterface<I2c<'static, esp_hal::Blocking>>,
    DisplaySize128x64,
    ssd1306::mode::BufferedGraphicsMode<DisplaySize128x64>,
>;

pub trait Screen {
    fn render(&mut self, display: &mut OledDisplay);
    fn on_event(&mut self, event: &UIEvent) -> Option<ScreenTransition>;
}

pub enum ScreenTransition {
    GoTo(ScreenId),
    Back,
}

/// Identifies a screen without owning its state.
#[allow(dead_code)]
pub enum ScreenId {
    Chat,
    Menu,
}

pub enum UIEvent {
    // Navigation (UART debug commands for now; GPIO button ISR later)
    NavMenu,
    NavBack,
    NavUp,
    NavDown,
    NavSelect,

    // Chat data
    MessageReceived { id: u8, text: String<124> },
    MessageSent { id: u8, text: String<124> },
    MessageConfirmed { id: u8 },
    MessageDiscarded { id: u8 },
}

enum ActiveScreen {
    Chat,
    Menu,
}

struct Display {
    hw: OledDisplay,
    active: ActiveScreen,
    // Persistent screen states — never dropped on screen change.
    chat: ChatScreen,
    menu: MenuScreen,
}

impl Display {
    fn new(hw: OledDisplay) -> Self {
        Self {
            hw,
            active: ActiveScreen::Chat,
            chat: ChatScreen::new(),
            menu: MenuScreen::new(),
        }
    }

    fn handle(&mut self, event: UIEvent) {
        let transition = match self.active {
            ActiveScreen::Chat => self.chat.on_event(&event),
            ActiveScreen::Menu => self.menu.on_event(&event),
        };

        if let Some(t) = transition {
            self.apply(t);
        }
    }

    fn apply(&mut self, t: ScreenTransition) {
        match t {
            ScreenTransition::GoTo(id) => {
                self.active = match id {
                    ScreenId::Chat => ActiveScreen::Chat,
                    ScreenId::Menu => ActiveScreen::Menu,
                };
            }
            // Back from top-level defaults to Chat.
            ScreenTransition::Back => self.active = ActiveScreen::Chat,
        }
    }

    fn render(&mut self) {
        let _ = self.hw.clear(BinaryColor::Off);
        match self.active {
            ActiveScreen::Chat => self.chat.render(&mut self.hw),
            ActiveScreen::Menu => self.menu.render(&mut self.hw),
        }
        if let Err(e) = self.hw.flush() {
            log::warn!("Display flush failed: {:?}", e);
        }
    }
}

#[embassy_executor::task]
pub async fn display_task(mut hw: OledDisplay) {
    if let Err(e) = hw.init() {
        log::error!("Display init failed: {:?}", e);
        return;
    }
    let _ = hw.clear(BinaryColor::Off);

    // Splash screen
    InitScreen::new().render(&mut hw);
    if let Err(e) = hw.flush() {
        log::warn!("Display flush failed: {:?}", e);
    }
    Timer::after(SPLASH_DURATION).await;

    let mut display = Display::new(hw);
    // Clear splash and draw the initial chat screen immediately.
    display.render();

    let receiver = crate::UI_CHANNEL.receiver();

    loop {
        let event = receiver.receive().await;
        display.handle(event);
        display.render();
    }
}
