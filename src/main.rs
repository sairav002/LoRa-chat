#![no_std]
#![no_main]

use embassy_executor::{SpawnError, Spawner};
use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, channel::Channel};
use embassy_time::Delay;
use embedded_hal_bus::spi::ExclusiveDevice;
use esp_backtrace as _;
use esp_hal::{
    clock::CpuClock,
    gpio::{Input, InputConfig, Level, Output, OutputConfig, Pull},
    i2c::master::{Config as I2cConfig, ConfigError as I2cConfigError, I2c},
    spi::master::{Config as SpiConfig, ConfigError as SpiConfigError, Spi},
    timer::timg::TimerGroup,
    uart::{Config as UartConfig, ConfigError as UartConfigError, Uart},
};
use esp_radio::ble::{
    Config as BleCtrlConfig,
    controller::{BleConnector, BleInitError},
};
use lora_phy::iv::GenericSx127xInterfaceVariant;
use ssd1306::{I2CDisplayInterface, Ssd1306, prelude::*};
use trouble_host::prelude::ExternalController;

use crate::{
    ble::BleMessage,
    config::{BLE_CHANNEL_SIZE, DISPLAY_CHANNEL_SIZE, TX_CHANNEL_SIZE},
    events::TxRequest,
    storage::{Storage, StorageError},
    uart::uart_task,
};

mod ble;
mod config;
mod coordinator;
mod display;
mod events;
mod protocol;
mod radio;
mod session;
mod settings;
mod storage;
mod uart;

pub static UI_CHANNEL: Channel<CriticalSectionRawMutex, display::UIEvent, DISPLAY_CHANNEL_SIZE> =
    Channel::new();

pub static TX_CHANNEL: Channel<CriticalSectionRawMutex, TxRequest, TX_CHANNEL_SIZE> =
    Channel::new();

pub static BLE_CHANNEL: Channel<CriticalSectionRawMutex, BleMessage, BLE_CHANNEL_SIZE> =
    Channel::new();

pub use settings::RADIO_SETTINGS;
pub use settings::SETTINGS_CHANGED;

#[derive(Debug)]
enum AppError {
    /// Flash storage failed to initialize
    Storage(StorageError),
    /// I2C bus configuration rejected
    I2c(I2cConfigError),
    /// SPI bus configuration rejecte
    Spi(SpiConfigError),
    /// UART configuration rejected
    Uart(UartConfigError),
    /// BLE controller failed to start
    Ble(BleInitError),
    /// SPI device creation failed
    SpiDevice,
    /// LoRa interface variant init failed
    LoraIv,
    /// LoRa radio failed to initialize
    LoraRadio,
    /// Task could not be spawned
    Spawn(SpawnError),
}

impl From<StorageError> for AppError {
    fn from(e: StorageError) -> Self {
        AppError::Storage(e)
    }
}

impl From<I2cConfigError> for AppError {
    fn from(e: I2cConfigError) -> Self {
        AppError::I2c(e)
    }
}

impl From<SpiConfigError> for AppError {
    fn from(e: SpiConfigError) -> Self {
        AppError::Spi(e)
    }
}

impl From<UartConfigError> for AppError {
    fn from(e: UartConfigError) -> Self {
        AppError::Uart(e)
    }
}

impl From<BleInitError> for AppError {
    fn from(e: BleInitError) -> Self {
        AppError::Ble(e)
    }
}

impl From<SpawnError> for AppError {
    fn from(e: SpawnError) -> Self {
        AppError::Spawn(e)
    }
}

#[esp_rtos::main]
async fn main(spawner: Spawner) {
    esp_println::logger::init_logger(log::LevelFilter::Info);

    if let Err(e) = run(spawner).await {
        log::error!("Fatal init error: {:?}", e);
    }
}

async fn run(spawner: Spawner) -> Result<(), AppError> {
    esp_bootloader_esp_idf::esp_app_desc!();

    log::info!("Booting...");

    let peripherals = esp_hal::init(esp_hal::Config::default().with_cpu_clock(CpuClock::max()));

    // Heap required by esp-radio (BT controller internal allocations) and by
    // trouble-host packet pools. 72 KiB is what the trouble-host ESP32 example
    // uses; tune down once BLE is stable.
    // BT controller task stack must be allocated from internal DRAM. The
    // `reclaimed` attribute places heap in IRAM which the radio driver cannot
    // use for task stacks. Use plain DRAM heap and keep it small so BT .bss
    // (~40 KiB) and task stacks have contiguous room.
    esp_alloc::heap_allocator!(size: 72 * 1024);

    log::info!("Free heap after init: {} bytes", esp_alloc::HEAP.free());

    let timg0 = TimerGroup::new(peripherals.TIMG0);
    let sw_ints =
        esp_hal::interrupt::software::SoftwareInterruptControl::new(peripherals.SW_INTERRUPT);
    esp_rtos::start(timg0.timer0, sw_ints.software_interrupt0);

    // -- Storage --
    //let storage = Storage::load(peripherals.FLASH)?;
    //log::info!("Storage loaded: {} known peers", storage.peer_count());

    // -- Display (SSD1306 over I2C) --
    let i2c = I2c::new(peripherals.I2C0, I2cConfig::default())?
        .with_sda(peripherals.GPIO21)
        .with_scl(peripherals.GPIO22);

    let display = Ssd1306::new(
        I2CDisplayInterface::new(i2c),
        DisplaySize128x64,
        DisplayRotation::Rotate0,
    )
    .into_buffered_graphics_mode();

    spawner.spawn(display::display_task(display)?);

    // -- UART --
    let uart = Uart::new(peripherals.UART0, UartConfig::default())?
        .with_tx(peripherals.GPIO1)
        .with_rx(peripherals.GPIO3)
        .into_async();

    spawner.spawn(uart_task(uart)?);

    // -- BLE (NUS peripheral) --
    // ESP32 internal RAM is tight. The esp-radio defaults target a beefier
    // budget (4 KiB BT task stack, 100-entry scan-duplicate cache, 3
    // connections). We don't scan, we accept 1 central, and we can live with a
    // smaller controller stack — this trims several KiB from both heap and
    // .bss so a 90 KiB `heap_allocator!` still has room to create tasks.
    let connector = BleConnector::new(peripherals.BT, BleCtrlConfig::default())?;
    let ble_controller: ble::BleController = ExternalController::new(connector);
    spawner.spawn(ble::ble_task(ble_controller)?);

    // -- LoRa Radio (SX1276 over SPI2) --
    let spi = Spi::new(peripherals.SPI2, SpiConfig::default())?
        .with_sck(peripherals.GPIO5)
        .with_miso(peripherals.GPIO19)
        .with_mosi(peripherals.GPIO27)
        .into_async();

    let cs = Output::new(peripherals.GPIO18, Level::High, OutputConfig::default());
    let reset = Output::new(peripherals.GPIO23, Level::High, OutputConfig::default());
    let dio0 = Input::new(
        peripherals.GPIO26,
        InputConfig::default().with_pull(Pull::Down),
    );

    let spi_device = ExclusiveDevice::new(spi, cs, Delay).map_err(|_| AppError::SpiDevice)?;
    let iv = GenericSx127xInterfaceVariant::new(reset, dio0, None, None)
        .map_err(|_| AppError::LoraIv)?;

    let radio = radio::Radio::new(spi_device, iv)
        .await
        .ok_or(AppError::LoraRadio)?;

    spawner.spawn(coordinator::coordinator_task(radio)?);

    Ok(())
}
