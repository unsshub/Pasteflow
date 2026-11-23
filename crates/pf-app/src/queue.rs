//! Paste Queue controller state machine (FIFO sequential paste, Universal Paste auto-collect,
//! SelectionRequest confirmation, rapid-press buffering, and manual-paste fallback).

#[derive(Debug, Clone)]
pub struct InFlightPaste {
    pub queue_item_id: i64,
    pub generation: u64,
}

#[derive(Debug, Default)]
pub struct QueueController {
    /// When true, every newly captured external clip is automatically appended to the queue.
    pub universal_paste: bool,
    /// When true, `paste_next` loads the clipboard and notifies the user instead of injecting XTEST.
    pub manual_mode: bool,
    /// Currently active paste waiting for `SelectionRequest` confirmation (2 s timeout).
    pub in_flight: Option<InFlightPaste>,
    /// Queued hotkey presses that arrived while `in_flight` was active.
    pub buffered_presses: usize,
    /// Monotonic token to ignore stale 2 s timeout callbacks.
    next_gen: u64,
}

impl QueueController {
    pub fn begin_in_flight(&mut self, queue_item_id: i64) -> u64 {
        self.next_gen = self.next_gen.wrapping_add(1);
        let generation = self.next_gen;
        self.in_flight = Some(InFlightPaste {
            queue_item_id,
            generation,
        });
        generation
    }

    /// Confirm an in-flight paste when a real target app requests clipboard data.
    /// Returns the confirmed `queue_item_id` (to be removed from the queue).
    pub fn confirm_served(&mut self) -> Option<i64> {
        let inflight = self.in_flight.take()?;
        Some(inflight.queue_item_id)
    }

    /// Handle 2 s confirmation timeout; returns `true` if `generation` matched the active paste.
    pub fn handle_timeout(&mut self, generation: u64) -> bool {
        if let Some(ref inf) = self.in_flight
            && inf.generation == generation
        {
            self.in_flight = None;
            self.buffered_presses = 0;
            return true;
        }
        false
    }

    pub fn take_buffered_press(&mut self) -> bool {
        if self.buffered_presses > 0 {
            self.buffered_presses -= 1;
            true
        } else {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sequential_in_flight_and_buffered_presses() {
        let mut q = QueueController::default();
        let g1 = q.begin_in_flight(10);
        q.buffered_presses += 1;
        assert_eq!(q.confirm_served(), Some(10));
        // Stale timeout after confirmation must be ignored
        assert!(!q.handle_timeout(g1));
        assert!(q.take_buffered_press());
        assert!(!q.take_buffered_press());

        let g2 = q.begin_in_flight(11);
        assert!(q.handle_timeout(g2));
        assert!(q.in_flight.is_none());
    }
}
