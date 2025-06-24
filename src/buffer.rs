//#![allow(unused_imports)]
//#![allow(dead_code)]
//#![allow(unused_variables)]
//#![allow(unused_mut)]
//#![allow(deprecated)]

use std::alloc::{alloc, dealloc, Layout};
use std::ptr::NonNull;

/// Single aligned buffer for O_DIRECT I/O operations shared across all devices
pub struct SharedBuffer {
    ptr: NonNull<u8>,
    layout: Layout,
    size: usize,
}

impl SharedBuffer {
    /// Create a new aligned buffer suitable for O_DIRECT
    pub fn new(size: usize, alignment: usize) -> Result<Self, std::io::Error> {
        // Round up size to alignment boundary
        let aligned_size = ((size + alignment - 1) / alignment) * alignment;

        let layout = Layout::from_size_align(aligned_size, alignment)
            .map_err(|e| std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("Invalid buffer layout: {}", e)
            ))?;

        let ptr = unsafe { alloc(layout) };

        if ptr.is_null() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::OutOfMemory,
                "Failed to allocate aligned buffer"
            ));
        }

        Ok(SharedBuffer {
            ptr: NonNull::new(ptr).unwrap(),
            layout,
            size: aligned_size,
        })
    }

    /// Get a mutable slice to the buffer
    pub fn as_mut_slice(&mut self) -> &mut [u8] {
        unsafe { std::slice::from_raw_parts_mut(self.ptr.as_ptr(), self.size) }
    }

    /// Get the buffer size
    pub fn len(&self) -> usize {
        self.size
    }
}

impl Drop for SharedBuffer {
    fn drop(&mut self) {
        unsafe {
            dealloc(self.ptr.as_ptr(), self.layout);
        }
    }
}

// Make it Send + Sync for sharing across tasks
unsafe impl Send for SharedBuffer {}
unsafe impl Sync for SharedBuffer {}
