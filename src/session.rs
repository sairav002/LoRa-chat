use embassy_time::{Duration, Instant};
use heapless::Vec;

use crate::config::{ACK_TIMEOUT_MS, DEDUP_WINDOW, LORA_MAX_PAYLOAD, MAX_PENDING_ACKS, MAX_RETRIES};

// -- TimeoutAction ----------------------------------------------------------

pub enum TimeoutAction {
    Retry {
        id: u8,
        wire: Vec<u8, LORA_MAX_PAYLOAD>,
    },
    GiveUp {
        id: u8,
    },
}

// -- DedupFilter ------------------------------------------------------------

struct DedupFilter {
    seen: Vec<u8, DEDUP_WINDOW>,
}

impl DedupFilter {
    pub fn new() -> Self {
        Self { seen: Vec::new() }
    }

    pub fn is_duplicate(&self, id: u8) -> bool {
        self.seen.contains(&id)
    }

    pub fn record(&mut self, id: u8) {
        if self.seen.is_full() {
            self.seen.remove(0);
        }
        let _ = self.seen.push(id);
    }
}

// -- AckTracker -------------------------------------------------------------

struct PendingAck {
    id: u8,
    wire: Vec<u8, LORA_MAX_PAYLOAD>,
    retries_left: u8,
    deadline: Instant,
}

struct AckTracker {
    next_seq: u8,
    pending: Vec<PendingAck, MAX_PENDING_ACKS>,
}

impl AckTracker {
    pub fn new() -> Self {
        Self {
            next_seq: 0,
            pending: Vec::new(),
        }
    }

    /// Vend the next sequence ID and advance the counter.
    pub fn next_id(&mut self) -> u8 {
        let id = self.next_seq;
        self.next_seq = self.next_seq.wrapping_add(1);
        id
    }

    /// Register a transmitted frame for ACK tracking.
    /// Returns `false` if the pending queue is full.
    pub fn track(&mut self, id: u8, wire: Vec<u8, LORA_MAX_PAYLOAD>) -> bool {
        if self.pending.is_full() {
            return false;
        }
        let _ = self.pending.push(PendingAck {
            id,
            wire,
            retries_left: MAX_RETRIES,
            deadline: Instant::now() + Duration::from_millis(ACK_TIMEOUT_MS),
        });
        true
    }

    /// Mark a frame as acknowledged. Returns `false` if the ID was not pending.
    pub fn on_ack(&mut self, id: u8) -> bool {
        if let Some(pos) = self.pending.iter().position(|p| p.id == id) {
            self.pending.remove(pos);
            true
        } else {
            false
        }
    }

    /// The earliest pending deadline, used by the coordinator to arm its timer.
    pub fn earliest_deadline(&self) -> Option<Instant> {
        self.pending.iter().map(|p| p.deadline).min()
    }

    /// Walk all expired entries and return the actions the coordinator must take.
    ///
    /// Entries with retries remaining are rescheduled with linear backoff and
    /// returned as `Retry` (with cloned wire bytes the coordinator can transmit).
    /// Entries with no retries left are removed and returned as `GiveUp`.
    pub fn process_timeouts(&mut self, now: Instant) -> Vec<TimeoutAction, MAX_PENDING_ACKS> {
        let mut actions: Vec<TimeoutAction, MAX_PENDING_ACKS> = Vec::new();

        let mut i = self.pending.len();
        while i > 0 {
            i -= 1;

            if self.pending[i].deadline > now {
                continue;
            }

            if self.pending[i].retries_left > 0 {
                self.pending[i].retries_left -= 1;
                let attempt = (MAX_RETRIES - self.pending[i].retries_left) as u64;
                self.pending[i].deadline = now + Duration::from_millis(ACK_TIMEOUT_MS * attempt);
                let _ = actions.push(TimeoutAction::Retry {
                    id: self.pending[i].id,
                    wire: self.pending[i].wire.clone(),
                });
            } else {
                let id = self.pending[i].id;
                self.pending.remove(i);
                let _ = actions.push(TimeoutAction::GiveUp { id });
            }
        }

        actions
    }
}

// -- Session ----------------------------------------------------------------

pub struct Session {
    dedup: DedupFilter,
    ack_tracker: AckTracker,
}

impl Session {
    pub fn new() -> Self {
        Self {
            dedup: DedupFilter::new(),
            ack_tracker: AckTracker::new(),
        }
    }

    pub fn is_duplicate(&self, id: u8) -> bool {
        self.dedup.is_duplicate(id)
    }

    pub fn record(&mut self, id: u8) {
        self.dedup.record(id)
    }

    pub fn next_id(&mut self) -> u8 {
        self.ack_tracker.next_id()
    }

    pub fn track(&mut self, id: u8, wire: Vec<u8, LORA_MAX_PAYLOAD>) -> bool {
        self.ack_tracker.track(id, wire)
    }

    pub fn on_ack(&mut self, id: u8) -> bool {
        self.ack_tracker.on_ack(id)
    }

    pub fn earliest_deadline(&self) -> Option<Instant> {
        self.ack_tracker.earliest_deadline()
    }

    pub fn process_timeouts(&mut self, now: Instant) -> Vec<TimeoutAction, MAX_PENDING_ACKS> {
        self.ack_tracker.process_timeouts(now)
    }
}

// -- Tests ------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dedup_new_id_not_duplicate() {
        let f = DedupFilter::new();
        assert!(!f.is_duplicate(1));
    }

    #[test]
    fn dedup_recorded_id_is_duplicate() {
        let mut f = DedupFilter::new();
        f.record(42);
        assert!(f.is_duplicate(42));
    }

    #[test]
    fn dedup_ring_evicts_oldest() {
        let mut f = DedupFilter::new();
        for i in 0..DEDUP_WINDOW as u8 {
            f.record(i);
        }
        assert!(!f.is_duplicate(0));
        assert!(f.is_duplicate((DEDUP_WINDOW - 1) as u8));
    }

    #[test]
    fn ack_tracker_next_id_increments() {
        let mut t = AckTracker::new();
        assert_eq!(t.next_id(), 0);
        assert_eq!(t.next_id(), 1);
    }

    #[test]
    fn ack_tracker_next_id_wraps() {
        let mut t = AckTracker::new();
        t.next_seq = 255;
        assert_eq!(t.next_id(), 255);
        assert_eq!(t.next_id(), 0);
    }

    #[test]
    fn ack_tracker_on_ack_known_id() {
        let mut t = AckTracker::new();
        t.track(7, Vec::new());
        assert!(t.on_ack(7));
        assert!(t.pending.is_empty());
    }

    #[test]
    fn ack_tracker_on_ack_unknown_id() {
        let mut t = AckTracker::new();
        assert!(!t.on_ack(99));
    }

    #[test]
    fn ack_tracker_full_rejects_track() {
        let mut t = AckTracker::new();
        for i in 0..MAX_PENDING_ACKS as u8 {
            assert!(t.track(i, Vec::new()));
        }
        assert!(!t.track(99, Vec::new()));
    }

    #[test]
    fn process_timeouts_retries_then_gives_up() {
        let mut t = AckTracker::new();
        t.track(1, Vec::new());

        let mut retries = 0u8;
        let mut gave_up = false;

        for _ in 0..=MAX_RETRIES {
            t.pending[0].deadline = Instant::now();
            for action in t.process_timeouts(Instant::now()) {
                match action {
                    TimeoutAction::Retry { .. } => retries += 1,
                    TimeoutAction::GiveUp { .. } => gave_up = true,
                }
            }
        }

        assert_eq!(retries, MAX_RETRIES);
        assert!(gave_up);
        assert!(t.pending.is_empty());
    }
}
