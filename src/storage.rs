use embedded_storage::nor_flash::{NorFlash, ReadNorFlash};
use esp_hal::peripherals::FLASH;
use esp_storage::{FlashStorage, FlashStorageError};

#[derive(Debug)]
pub enum StorageError {
    /// Flash read or write operation failed at the hardware level.
    Io(FlashStorageError),
    /// Sector erase failed — data on flash may be in an undefined state.
    EraseFailed(FlashStorageError),
    /// Magic byte not found — flash is uninitialized or corrupt.
    Uninitialized,
}

const FLASH_RANGE_START: u32 = 0x9000;
const FLASH_RANGE_END: u32 = 0xA000;

// Sector layout:
//   [0]      magic - 0xAB means initialized
//   [1]      count - number of stored peer IDs
//   [2..34]  peers - raw u8 IDs, up to MAX_PEERS
//   [34..]   unused - (0xFF erased state)
const MAGIC: u8 = 0xA5;
const HEADER_SIZE: usize = 2 + MAX_PEERS; // magic + count + peers
pub const MAX_PEERS: usize = 32;

pub struct Storage<'a> {
    storage: FlashStorage<'a>,
    peers: heapless::Vec<u8, MAX_PEERS>,
}

impl<'a> Storage<'a> {
    /// Load peer list from flash on boot.
    /// Returns `Uninitialized` if the sector has never been written.
    pub fn load(flash: FLASH<'a>) -> Result<Self, StorageError> {
        let mut storage = FlashStorage::new(flash);
        let mut buf = [0u8; HEADER_SIZE];

        storage
            .read(FLASH_RANGE_START, &mut buf)
            .map_err(StorageError::Io)?;

        let count = match buf[0] {
            MAGIC => (buf[1] as usize).min(MAX_PEERS),
            _ => 0,
        };

        let mut peers = heapless::Vec::new();
        for &id in &buf[2..2 + count] {
            peers.push(id).ok();
        }

        let storage = Self { storage, peers };

        Ok(storage)
    }

    /// Persist peer list to flash. Erases sector first (NorFlash requirement).
    /// Only call when the list actually changed — each call costs one 4 KB erase cycle.
    pub fn save(&mut self) -> Result<(), StorageError> {
        let storage = &mut self.storage;

        let mut buf = [0xFFu8; HEADER_SIZE];
        buf[0] = MAGIC;
        buf[1] = self.peers.len() as u8;
        buf[2..2 + self.peers.len()].copy_from_slice(self.peers.as_slice());

        storage
            .erase(FLASH_RANGE_START, FLASH_RANGE_END)
            .map_err(StorageError::EraseFailed)?;
        storage
            .write(FLASH_RANGE_START, &buf)
            .map_err(StorageError::Io)?;

        Ok(())
    }

    /// Add a peer ID if not already known. Returns true if it was new.
    /// Caller is responsible for calling save() to persist.
    pub fn insert(&mut self, id: u8) -> bool {
        if self.peers.contains(&id) {
            return false;
        }
        self.peers.push(id).ok();
        true
    }

    pub fn peer_count(&self) -> u32 {
        self.peers.len() as u32
    }
}
