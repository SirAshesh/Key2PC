#[derive(Debug, Default, Clone)]
pub struct SequenceWindow {
    initialized: bool,
    last_seq: u64,
    seen_mask: u64,
}

impl SequenceWindow {
    pub fn reset(&mut self) {
        self.initialized = false;
        self.last_seq = 0;
        self.seen_mask = 0;
    }

    pub fn accept(&mut self, seq: u64) -> bool {
        if !self.initialized {
            self.initialized = true;
            self.last_seq = seq;
            self.seen_mask = 1;
            return true;
        }

        if seq > self.last_seq {
            let shift = seq - self.last_seq;
            self.seen_mask = if shift >= 64 {
                1
            } else {
                (self.seen_mask << shift) | 1
            };
            self.last_seq = seq;
            return true;
        }

        let diff = self.last_seq - seq;
        if diff >= 64 {
            return false;
        }

        let bit = 1u64 << diff;
        if (self.seen_mask & bit) != 0 {
            return false;
        }

        self.seen_mask |= bit;
        true
    }
}
