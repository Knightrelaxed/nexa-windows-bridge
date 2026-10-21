// ============================================================
// LOCK-FREE SPSC AUDIO RING BUFFER
// Zero-Allocation, Lock-Free Transfer for Real-Time Audio Threads
// ============================================================

use ringbuf::traits::{Consumer, Observer, Producer, Split};
use ringbuf::HeapRb;

pub struct AudioProducer<T> {
    inner: <HeapRb<T> as Split>::Prod,
}

pub struct AudioConsumer<T> {
    inner: <HeapRb<T> as Split>::Cons,
}

impl<T> AudioProducer<T> {
    /// Push a single item into the ring buffer. Returns Err(item) if full.
    #[allow(dead_code)]
    pub fn push(&mut self, item: T) -> Result<(), T> {
        self.inner.try_push(item)
    }

    /// Push a slice of items into the ring buffer. Returns number of items pushed.
    pub fn push_slice(&mut self, slice: &[T]) -> usize
    where
        T: Copy,
    {
        self.inner.push_slice(slice)
    }

    /// Check number of available slots for writing.
    #[allow(dead_code)]
    pub fn free_len(&self) -> usize {
        self.inner.vacant_len()
    }

    /// Check if buffer is full.
    #[allow(dead_code)]
    pub fn is_full(&self) -> bool {
        self.inner.is_full()
    }
}

impl<T> AudioConsumer<T> {
    /// Pop a single item from the ring buffer. Returns None if empty.
    #[allow(dead_code)]
    pub fn pop(&mut self) -> Option<T> {
        self.inner.try_pop()
    }

    /// Pop items into a destination slice. Returns number of items popped.
    pub fn pop_slice(&mut self, slice: &mut [T]) -> usize
    where
        T: Copy,
    {
        self.inner.pop_slice(slice)
    }

    /// Check number of items available for reading.
    pub fn len(&self) -> usize {
        self.inner.occupied_len()
    }

    /// Check if buffer is empty.
    #[allow(dead_code)]
    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    /// Drain and discard all queued items.
    #[allow(dead_code)]
    pub fn clear(&mut self) {
        self.inner.clear();
    }
}

/// Create a lock-free Single-Producer Single-Consumer (SPSC) ring buffer with given capacity.
pub fn create_audio_ring_buffer<T>(capacity: usize) -> (AudioProducer<T>, AudioConsumer<T>) {
    let rb = HeapRb::<T>::new(capacity);
    let (prod, cons) = rb.split();
    (
        AudioProducer { inner: prod },
        AudioConsumer { inner: cons },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ring_buffer_push_pop() {
        let (mut prod, mut cons) = create_audio_ring_buffer::<f32>(16);
        assert_eq!(cons.len(), 0);
        assert!(cons.is_empty());

        let samples = [0.1, 0.2, 0.3, 0.4];
        let pushed = prod.push_slice(&samples);
        assert_eq!(pushed, 4);
        assert_eq!(cons.len(), 4);

        let mut read_buf = [0.0; 4];
        let popped = cons.pop_slice(&mut read_buf);
        assert_eq!(popped, 4);
        assert_eq!(read_buf, samples);
        assert!(cons.is_empty());
    }

    #[test]
    fn test_ring_buffer_overflow_protection() {
        let (mut prod, mut cons) = create_audio_ring_buffer::<i16>(4);
        let pushed = prod.push_slice(&[1, 2, 3, 4, 5, 6]);
        assert_eq!(pushed, 4); // Only fits 4
        assert!(prod.is_full());

        let mut out = [0; 4];
        let popped = cons.pop_slice(&mut out);
        assert_eq!(popped, 4);
        assert_eq!(out, [1, 2, 3, 4]);
    }
}
