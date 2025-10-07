//! Buffer pool for memory efficiency

use std::sync::{Arc, Mutex};

/// A pool of reusable byte buffers to reduce allocations
#[derive(Clone)]
pub struct BufferPool {
    pool: Arc<Mutex<Vec<Vec<u8>>>>,
    max_capacity: usize,
    buffer_size: usize,
}

impl BufferPool {
    /// Create a new buffer pool
    ///
    /// # Arguments
    /// * `max_capacity` - Maximum number of buffers to keep in the pool
    /// * `buffer_size` - Initial size for new buffers
    pub fn new(max_capacity: usize, buffer_size: usize) -> Self {
        Self {
            pool: Arc::new(Mutex::new(Vec::with_capacity(max_capacity))),
            max_capacity,
            buffer_size,
        }
    }

    /// Get a buffer from the pool, or create a new one if pool is empty
    pub fn get(&self) -> Vec<u8> {
        let mut pool = self.pool.lock().unwrap();
        pool.pop()
            .unwrap_or_else(|| Vec::with_capacity(self.buffer_size))
    }

    /// Return a buffer to the pool for reuse
    pub fn put(&self, mut buffer: Vec<u8>) {
        buffer.clear();

        let mut pool = self.pool.lock().unwrap();
        if pool.len() < self.max_capacity {
            pool.push(buffer);
        }
        // If pool is full, buffer is dropped
    }

    /// Get the current number of buffers in the pool
    pub fn len(&self) -> usize {
        self.pool.lock().unwrap().len()
    }

    /// Check if the pool is empty
    pub fn is_empty(&self) -> bool {
        self.pool.lock().unwrap().is_empty()
    }

    /// Clear all buffers from the pool
    pub fn clear(&self) {
        self.pool.lock().unwrap().clear();
    }
}

impl Default for BufferPool {
    fn default() -> Self {
        Self::new(64, 8192) // Default: 64 buffers of 8KB each
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_buffer_pool_get_and_put() {
        let pool = BufferPool::new(10, 1024);

        // Get a buffer
        let buffer = pool.get();
        assert_eq!(buffer.capacity(), 1024);
        assert_eq!(pool.len(), 0);

        // Put it back
        pool.put(buffer);
        assert_eq!(pool.len(), 1);

        // Get it again
        let buffer2 = pool.get();
        assert_eq!(buffer2.capacity(), 1024);
        assert_eq!(pool.len(), 0);
    }

    #[test]
    fn test_buffer_pool_max_capacity() {
        let pool = BufferPool::new(3, 1024);

        // Add more buffers than max capacity
        for _ in 0..5 {
            pool.put(Vec::with_capacity(1024));
        }

        // Only max_capacity buffers should be kept
        assert_eq!(pool.len(), 3);
    }

    #[test]
    fn test_buffer_pool_clears_buffer() {
        let pool = BufferPool::new(10, 1024);

        let mut buffer = pool.get();
        buffer.extend_from_slice(b"test data");
        assert_eq!(buffer.len(), 9);

        pool.put(buffer);

        let buffer2 = pool.get();
        assert_eq!(buffer2.len(), 0); // Should be cleared
    }

    #[test]
    fn test_buffer_pool_default() {
        let pool = BufferPool::default();
        assert_eq!(pool.max_capacity, 64);
        assert_eq!(pool.buffer_size, 8192);
    }

    #[test]
    fn test_buffer_pool_concurrent_access() {
        use std::thread;

        let pool = BufferPool::new(10, 1024);
        let mut handles = vec![];

        for _ in 0..5 {
            let pool_clone = pool.clone();
            let handle = thread::spawn(move || {
                for _ in 0..10 {
                    let buffer = pool_clone.get();
                    pool_clone.put(buffer);
                }
            });
            handles.push(handle);
        }

        for handle in handles {
            handle.join().unwrap();
        }
    }

    #[test]
    fn test_buffer_pool_is_empty() {
        let pool = BufferPool::new(10, 1024);
        assert!(pool.is_empty());

        pool.put(Vec::new());
        assert!(!pool.is_empty());

        pool.clear();
        assert!(pool.is_empty());
    }
}
