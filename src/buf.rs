//! Per-worker allocation cache. Tokio's multi-thread scheduler pins work to
//! worker threads, so reused vectors stay on the same thread and skip the
//! global allocator on the data path.

use bytes::{Bytes, BytesMut};
use std::cell::RefCell;

const MAX_CACHED: usize = 64;
const MAX_CAP: usize = 1024 * 1024;

thread_local! {
    static VECS: RefCell<Vec<Vec<u8>>> = const { RefCell::new(Vec::new()) };
    static BUFS: RefCell<Vec<BytesMut>> = const { RefCell::new(Vec::new()) };
}

#[must_use]
#[inline]
pub fn take_vec(min: usize) -> Vec<u8> {
    VECS.with(|cache| {
        let mut cache = cache.borrow_mut();
        match cache.pop() {
            Some(mut v) => {
                v.clear();
                if v.capacity() < min {
                    v.reserve(min);
                }
                v
            }
            None => Vec::with_capacity(min.max(64)),
        }
    })
}

#[inline]
pub fn give_vec(mut v: Vec<u8>) {
    v.clear();
    if v.capacity() == 0 || v.capacity() > MAX_CAP {
        return;
    }
    VECS.with(|cache| {
        let mut cache = cache.borrow_mut();
        if cache.len() < MAX_CACHED {
            cache.push(v);
        }
    });
}

#[must_use]
#[inline]
pub fn take_buf(min: usize) -> BytesMut {
    BUFS.with(|cache| {
        let mut cache = cache.borrow_mut();
        match cache.pop() {
            Some(mut b) => {
                b.clear();
                if b.capacity() < min {
                    b.reserve(min);
                }
                b
            }
            None => BytesMut::with_capacity(min.max(64)),
        }
    })
}

#[inline]
pub fn give_buf(mut b: BytesMut) {
    b.clear();
    if b.capacity() == 0 || b.capacity() > MAX_CAP {
        return;
    }
    BUFS.with(|cache| {
        let mut cache = cache.borrow_mut();
        if cache.len() < MAX_CACHED {
            cache.push(b);
        }
    });
}

#[must_use]
#[inline]
pub fn copy_bytes(src: &[u8]) -> Bytes {
    let mut b = take_buf(src.len());
    b.extend_from_slice(src);
    b.freeze()
}
