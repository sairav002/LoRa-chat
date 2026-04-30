use embassy_futures::{
    join::join,
    select::{Either, select},
};
use esp_radio::ble::controller::BleConnector;
use heapless::{String, Vec};
use trouble_host::prelude::*;

use crate::{BLE_CHANNEL, TX_CHANNEL, config, events::TxRequest};

pub struct BleMessage(pub String<124>);

// Single-central peripheral. Bump if you want concurrent phone + laptop.
const CONNECTIONS_MAX: usize = 1;
const L2CAP_CHANNELS_MAX: usize = 2; // signalling + ATT

/// Characteristic value buffer size.
/// Pool MTU 128 − 4 HCI ACL − 4 L2CAP − 3 ATT opcode+handle = 117 bytes max
/// payload per notification. Matches `default-packet-pool-mtu-128` in Cargo.toml;
/// bump both in lockstep if you change the pool MTU.
const CHAR_MAX: usize = 117;

#[gatt_service(uuid = "6e400001-b5a3-f393-e0a9-e50e24dcca9e")]
struct NordicUartService {
    #[characteristic(
        uuid = "6e400002-b5a3-f393-e0a9-e50e24dcca9e",
        write,
        write_without_response,
        value = [0u8; 117]
    )]
    rx: [u8; CHAR_MAX],

    #[characteristic(
        uuid = "6e400003-b5a3-f393-e0a9-e50e24dcca9e",
        notify,
        value = [0u8; 117]
    )]
    tx: [u8; CHAR_MAX],
}

#[gatt_server]
struct Server {
    nus: NordicUartService,
}

/// Concrete controller type used by the spawned task. `#[embassy_executor::task]`
/// requires a fully-monomorphized signature.
pub type BleController = ExternalController<BleConnector<'static>, 20>;

#[embassy_executor::task]
pub async fn ble_task(controller: BleController) {
    let address: Address = Address::random(config::BLE_ADDRESS);
    log::info!("BLE address: {:?}", address);

    let mut resources: HostResources<DefaultPacketPool, CONNECTIONS_MAX, L2CAP_CHANNELS_MAX> =
        HostResources::new();
    // Stack is consumed by set_random_address (returns Self by value) and then
    // build() borrows it to produce Host<'stack, _, _>. The named binding keeps
    // the Stack alive for the whole task so the Host's borrow stays valid.
    let stack = trouble_host::new(controller, &mut resources).set_random_address(address);
    let Host {
        runner,
        mut peripheral,
        ..
    } = stack.build();

    let server = match Server::new_with_config(GapConfig::Peripheral(PeripheralConfig {
        name: config::BLE_DEVICE_NAME,
        appearance: &appearance::power_device::GENERIC_POWER_DEVICE,
    })) {
        Ok(s) => s,
        Err(e) => {
            log::error!("BLE server init failed: {:?}", e);
            return;
        }
    };

    log::info!(
        "BLE peripheral '{}' advertising (NUS)",
        config::BLE_DEVICE_NAME
    );

    // Two concurrent halves: the transport runner (pumps HCI forever) and the
    // advertise → connect → service → disconnect loop.
    let _ = join(run_stack(runner), connection_loop(&mut peripheral, &server)).await;
}

async fn run_stack<C: Controller, P: PacketPool>(mut runner: Runner<'_, C, P>) {
    if let Err(e) = runner.run().await {
        log::error!("BLE stack runner error: {:?}", e);
    }
}

async fn connection_loop<'a, 'server, C: Controller>(
    peripheral: &mut Peripheral<'a, C, DefaultPacketPool>,
    server: &'server Server<'a>,
) {
    loop {
        match advertise(config::BLE_DEVICE_NAME, peripheral, server).await {
            Ok(conn) => {
                log::info!("BLE central connected");
                let _ = handle_connection(server, &conn).await;
                log::info!("BLE central disconnected — re-advertising");
            }
            Err(e) => {
                log::error!("BLE advertise error: {:?}", e);
            }
        }
    }
}

async fn handle_connection<P: PacketPool>(
    server: &Server<'_>,
    conn: &GattConnection<'_, '_, P>,
) -> Result<(), Error> {
    let sender = TX_CHANNEL.sender();
    let rx_handle = server.nus.rx.handle;

    loop {
        match select(BLE_CHANNEL.receive(), conn.next()).await {
            Either::First(msg) => {
                let bytes = msg.0.as_bytes();
                let take = bytes.len().min(CHAR_MAX);
                let mut buf = [0u8; CHAR_MAX];
                buf[..take].copy_from_slice(&bytes[..take]);
                if let Err(e) = server.nus.tx.notify(conn, &buf).await {
                    log::warn!("BLE notify error: {:?}", e);
                }
            }
            Either::Second(event) => {
                if handle_gatt_event(event, rx_handle, &sender).await {
                    // Drain stale notifications before the next central connects.
                    while BLE_CHANNEL.try_receive().is_ok() {}
                    return Ok(());
                }
            }
        }
    }
}

async fn handle_gatt_event<P: PacketPool, M: embassy_sync::blocking_mutex::raw::RawMutex>(
    event: GattConnectionEvent<'_, '_, P>,
    rx_handle: u16,
    sender: &embassy_sync::channel::Sender<'_, M, TxRequest, { config::TX_CHANNEL_SIZE }>,
) -> bool {
    match event {
        GattConnectionEvent::Disconnected { reason } => {
            log::info!("BLE disconnect reason: {:?}", reason);
            return true;
        }
        GattConnectionEvent::Gatt { event } => {
            if let GattEvent::Write(write) = &event {
                if write.handle() == rx_handle {
                    forward_to_tx_channel(sender, write.data()).await;
                }
            }
            match event.accept() {
                Ok(reply) => reply.send().await,
                Err(e) => log::warn!("BLE gatt reply error: {:?}", e),
            }
        }
        _ => {}
    }
    false
}

async fn forward_to_tx_channel<M: embassy_sync::blocking_mutex::raw::RawMutex>(
    sender: &embassy_sync::channel::Sender<'_, M, TxRequest, { config::TX_CHANNEL_SIZE }>,
    data: &[u8],
) {
    let take = data.len().min(config::BLE_MAX_PAYLOAD);
    let mut payload: Vec<u8, { config::BLE_MAX_PAYLOAD }> = Vec::new();
    if payload.extend_from_slice(&data[..take]).is_err() {
        log::warn!("BLE payload copy failed");
        return;
    }

    // Non-blocking: if the coordinator is backed up we'd rather drop a BLE
    // message than stall the BLE stack (which can trigger link supervision
    // timeouts). The coordinator-equivalent UART path uses blocking send; we
    // differ here specifically to protect the radio link.
    if sender.try_send(TxRequest { payload }).is_err() {
        log::warn!("TX_CHANNEL full — dropping BLE message");
    }
}

async fn advertise<'a, 'b, C: Controller>(
    name: &'a str,
    peripheral: &mut Peripheral<'a, C, DefaultPacketPool>,
    server: &'b Server<'a>,
) -> Result<GattConnection<'a, 'b, DefaultPacketPool>, BleHostError<C::Error>> {
    let mut adv_data = [0u8; 31];
    let len = AdStructure::encode_slice(
        &[
            AdStructure::Flags(LE_GENERAL_DISCOVERABLE | BR_EDR_NOT_SUPPORTED),
            AdStructure::CompleteLocalName(name.as_bytes()),
        ],
        &mut adv_data[..],
    )?;
    let advertiser = peripheral
        .advertise(
            &Default::default(),
            Advertisement::ConnectableScannableUndirected {
                adv_data: &adv_data[..len],
                scan_data: &[],
            },
        )
        .await?;
    log::info!("Waiting for conn");
    let conn = advertiser.accept().await?.with_attribute_server(server)?;
    log::info!("Connection received!");
    Ok(conn)
}
