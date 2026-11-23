//! Bounded single-producer/single-consumer queue of interleaved f32 samples.
//!
//! Slots are atomic u32s holding f32 bit patterns: writes and reads never
//! allocate, lock or block. The producer owns `tail` and the consumer owns
//! `head`; release/acquire ordering publishes each batch of samples.

use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RingError {
    Full,
}

pub struct RingBuffer {
    capacity: u64,
    samples: Box<[AtomicU32]>,
    head: AtomicU64,
    tail: AtomicU64,
}

impl RingBuffer {
    pub fn new(capacity_frames: usize, channels: usize) -> Self {
        let capacity = capacity_frames.saturating_mul(channels);
        Self {
            capacity: capacity as u64,
            samples: (0..capacity)
                .map(|_| AtomicU32::new(0.0f32.to_bits()))
                .collect(),
            head: AtomicU64::new(0),
            tail: AtomicU64::new(0),
        }
    }

    pub(crate) fn capacity_samples(&self) -> usize {
        self.samples.len()
    }

    /// Enqueue a whole batch, or fail without touching the queue. Partial
    /// writes would let the writer publish a gap where samples were expected.
    pub fn push(&self, samples: &[f32]) -> Result<(), RingError> {
        if self.capacity == 0 {
            return Err(RingError::Full);
        }
        let tail = self.tail.load(Ordering::Relaxed);
        let head = self.head.load(Ordering::Acquire);
        if samples.len() as u64 > self.capacity - tail.wrapping_sub(head) {
            return Err(RingError::Full);
        }
        let mut offset = (tail % self.capacity) as usize;
        for &sample in samples {
            self.samples[offset].store(sample.to_bits(), Ordering::Relaxed);
            offset += 1;
            if offset == self.samples.len() {
                offset = 0;
            }
        }
        self.tail
            .store(tail.wrapping_add(samples.len() as u64), Ordering::Release);
        Ok(())
    }

    /// Dequeue up to `out.len()` samples. `None` means the queue is empty,
    /// which is different from a failed operation.
    pub fn pop(&self, out: &mut [f32]) -> Option<usize> {
        if self.capacity == 0 || out.is_empty() {
            return None;
        }
        let head = self.head.load(Ordering::Relaxed);
        let tail = self.tail.load(Ordering::Acquire);
        let available = tail.wrapping_sub(head);
        if available == 0 {
            return None;
        }
        let count = (out.len() as u64).min(available) as usize;
        let mut offset = (head % self.capacity) as usize;
        for slot in out.iter_mut().take(count) {
            *slot = f32::from_bits(self.samples[offset].load(Ordering::Relaxed));
            offset += 1;
            if offset == self.samples.len() {
                offset = 0;
            }
        }
        self.head
            .store(head.wrapping_add(count as u64), Ordering::Release);
        Some(count)
    }
}
