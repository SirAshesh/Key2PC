use std::collections::VecDeque;
use std::time::{Duration, Instant};

use crate::protocol::PACKET_SIZE;

#[derive(Debug, Clone)]
pub struct PendingPacket {
    pub seq: u64,
    pub raw: [u8; PACKET_SIZE],
    pub first_sent_at: Instant,
    pub last_sent_at: Instant,
    pub retries: u8,
}

#[derive(Debug, Default)]
pub struct RetransmitQueue {
    items: VecDeque<PendingPacket>,
}

impl RetransmitQueue {
    pub fn push(&mut self, seq: u64, raw: [u8; PACKET_SIZE], now: Instant) {
        if self.items.len() >= 256 {
            self.items.pop_front();
        }
        self.items.push_back(PendingPacket {
            seq,
            raw,
            first_sent_at: now,
            last_sent_at: now,
            retries: 0,
        });
    }

    pub fn ack(&mut self, seq: u64, now: Instant) -> Option<Duration> {
        let pos = self.items.iter().position(|item| item.seq == seq)?;
        let item = self.items.remove(pos)?;
        Some(now.saturating_duration_since(item.first_sent_at))
    }

    pub fn collect_due(
        &mut self,
        now: Instant,
        interval: Duration,
        max_retries: u8,
    ) -> Vec<[u8; PACKET_SIZE]> {
        let mut resend = Vec::new();
        let mut idx = 0;
        while idx < self.items.len() {
            if now.saturating_duration_since(self.items[idx].last_sent_at) >= interval {
                if self.items[idx].retries >= max_retries {
                    self.items.remove(idx);
                    continue;
                }
                self.items[idx].retries += 1;
                self.items[idx].last_sent_at = now;
                resend.push(self.items[idx].raw);
            }
            idx += 1;
        }
        resend
    }

    pub fn next_wake_delay(&self, now: Instant, interval: Duration) -> Option<Duration> {
        let mut earliest: Option<Duration> = None;
        for item in &self.items {
            let elapsed = now.saturating_duration_since(item.last_sent_at);
            let remaining = interval.saturating_sub(elapsed);
            earliest = Some(match earliest {
                Some(current) => current.min(remaining),
                None => remaining,
            });
        }
        earliest
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub fn clear(&mut self) {
        self.items.clear();
    }
}
