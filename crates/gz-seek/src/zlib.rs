//! The raw deflate inflater: zlib-rs, driven through its C API, and the only
//! module in this crate holding `unsafe`.
//!
//! An index needs three things of an inflater that zlib-rs's safe `zlib_rs::Inflate`
//! does not give: where a `Z_BLOCK` stop left the bit reader (`data_type`),
//! seeding the bit reader at a position inside a byte (`inflatePrime`), and the
//! sliding window read out (`inflateGetDictionary`) — beside the window set
//! (`inflateSetDictionary`), which it has. `libz-rs-sys` is the same engine's
//! zlib-compatible API, stable as zlib's own is, and every call into it is here,
//! behind a type whose methods are safe: the stream is boxed so it never moves,
//! its pointers are set from slices for the length of one call and cleared after
//! it, and it is ended once, on drop.
// upstream: UF4
#![allow(unsafe_code)]

use std::ffi::{CStr, c_int, c_uint};

use libz_rs_sys::{
    Z_BLOCK, Z_BUF_ERROR, Z_DATA_ERROR, Z_OK, Z_STREAM_END, crc32_z, inflate, inflateEnd,
    inflateGetDictionary, inflateInit2_, inflatePrime, inflateReset, inflateSetDictionary, z_off_t,
    z_stream, zlibVersion,
};

use crate::error::{Error, Result};

/// The deflate window: the most history any back-reference reaches.
pub const WINDOW_SIZE: usize = 32 * 1024;

/// The CRC-32 of `bytes`, continuing `crc`: gzip's check, the same engine's.
pub(crate) fn crc32(crc: u32, bytes: &[u8]) -> u32 {
    if bytes.is_empty() {
        return crc;
    }
    // SAFETY: `bytes` is a live, non-empty slice of `bytes.len()` bytes.
    unsafe { crc32_z(crc.into(), bytes.as_ptr(), bytes.len()) as u32 }
}

/// The CRC-32 of two runs of plaintext end to end, from each one's CRC-32 and
/// the second's length.
pub(crate) fn crc32_combine(first: u32, second: u32, second_len: u64) -> u32 {
    let len = z_off_t::try_from(second_len).expect("a run's length fits z_off_t");
    libz_rs_sys::crc32_combine(first.into(), second.into(), len) as u32
}

/// A raw deflate stream: no zlib or gzip framing, a 32 KiB window.
const RAW_WINDOW_BITS: c_int = -15;

/// What one [`Raw::inflate`] call did.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Inflated {
    pub(crate) consumed: usize,
    pub(crate) produced: usize,
    /// The deflate stream's final block has ended.
    pub(crate) stream_end: bool,
    /// zlib's `data_type`: the bits the reader holds unused in its low six
    /// bits, `64` if the last block begun was the final one, `128` if the
    /// decode stopped just after a block's end.
    pub(crate) data_type: c_int,
}

impl Inflated {
    /// Stopped at the end of a block that is not the final one: a place a
    /// checkpoint can be taken.
    pub(crate) fn at_block_boundary(&self) -> bool {
        self.data_type & 128 != 0 && self.data_type & 64 == 0
    }

    /// Stopped just after the final block's end, short of the stream's.
    pub(crate) fn at_final_block_end(&self) -> bool {
        !self.stream_end && self.data_type & 192 == 192
    }

    /// This call's result, following `earlier`'s in the same step.
    pub(crate) fn after(self, earlier: Inflated) -> Inflated {
        Inflated {
            consumed: earlier.consumed + self.consumed,
            produced: earlier.produced + self.produced,
            ..self
        }
    }

    /// How many bits the reader holds that it has not used — whole bytes
    /// among them, which is why this is not zlib's documented `& 7`.
    pub(crate) fn unused_bits(&self) -> u64 {
        (self.data_type & 63) as u64
    }
}

/// One raw inflate stream.
pub(crate) struct Raw {
    stream: Box<z_stream>,
}

// SAFETY: the stream's state is a heap allocation this value owns alone; its
// `next_in`/`next_out` pointers are null between calls, so no borrow outlives
// the call that lent it, and nothing in the state is tied to a thread. Every
// method takes `&mut self` or reads through `&self` without mutation.
unsafe impl Send for Raw {}

impl Raw {
    pub(crate) fn new() -> Result<Raw> {
        let mut stream = Box::new(z_stream::default());
        let size = c_int::try_from(std::mem::size_of::<z_stream>()).expect("z_stream fits a c_int");
        // SAFETY: `stream` is a default `z_stream`, whose allocator fields
        // `Default` fills, and the version and size are the library's own.
        let rc = unsafe { inflateInit2_(&mut *stream, RAW_WINDOW_BITS, zlibVersion(), size) };
        if rc != Z_OK {
            return Err(Error::Backend { code: rc });
        }
        Ok(Raw { stream })
    }

    /// Back to the start of a fresh raw stream, the window emptied.
    pub(crate) fn reset(&mut self) -> Result<()> {
        // SAFETY: the stream was initialized by `new` and not ended.
        let rc = unsafe { inflateReset(&mut *self.stream) };
        self.check(rc)
    }

    /// Put `bits` (at most 16) low bits of `value` ahead of the next input.
    pub(crate) fn prime(&mut self, bits: u8, value: u32) -> Result<()> {
        debug_assert!(bits <= 16);
        // SAFETY: as `reset`.
        let rc = unsafe { inflatePrime(&mut *self.stream, c_int::from(bits), value as c_int) };
        self.check(rc)
    }

    /// Seed the window with `dictionary`, the plaintext preceding the resume
    /// point, at most [`WINDOW_SIZE`] of it.
    pub(crate) fn set_dictionary(&mut self, dictionary: &[u8]) -> Result<()> {
        if dictionary.is_empty() {
            return Ok(());
        }
        let len = c_uint::try_from(dictionary.len()).expect("a window fits a c_uint");
        // SAFETY: `dictionary` is a live slice of `len` bytes for the call.
        let rc = unsafe { inflateSetDictionary(&mut *self.stream, dictionary.as_ptr(), len) };
        self.check(rc)
    }

    /// Copy the window — the last plaintext this stream produced, up to
    /// [`WINDOW_SIZE`] of it — into `buf`, answering its length.
    pub(crate) fn dictionary(&self, buf: &mut [u8; WINDOW_SIZE]) -> usize {
        let mut len: c_uint = 0;
        // SAFETY: `buf` holds `WINDOW_SIZE` bytes, which the API documents as
        // always enough, and `len` is a live `c_uint`.
        let rc = unsafe { inflateGetDictionary(&*self.stream, buf.as_mut_ptr(), &mut len) };
        assert_eq!(rc, Z_OK, "inflateGetDictionary on a live stream");
        len as usize
    }

    /// Inflate from `input` into `out`, stopping at the next block boundary.
    pub(crate) fn inflate(&mut self, input: &[u8], out: &mut [u8]) -> Result<Inflated> {
        let avail_in = c_uint::try_from(input.len()).unwrap_or(c_uint::MAX);
        let avail_out = c_uint::try_from(out.len()).unwrap_or(c_uint::MAX);
        let stream = &mut *self.stream;
        stream.next_in = input.as_ptr();
        stream.avail_in = avail_in;
        stream.next_out = out.as_mut_ptr();
        stream.avail_out = avail_out;
        // SAFETY: the pointers above come from live slices at least as long
        // as the lengths beside them, held for this call only.
        let rc = unsafe { inflate(stream, Z_BLOCK) };
        let consumed = (avail_in - stream.avail_in) as usize;
        let produced = (avail_out - stream.avail_out) as usize;
        stream.next_in = std::ptr::null();
        stream.avail_in = 0;
        stream.next_out = std::ptr::null_mut();
        stream.avail_out = 0;
        let inflated = Inflated {
            consumed,
            produced,
            stream_end: rc == Z_STREAM_END,
            data_type: stream.data_type,
        };
        match rc {
            Z_OK | Z_STREAM_END | Z_BUF_ERROR => Ok(inflated),
            // The offset is the caller's to state, this layer knowing none.
            Z_DATA_ERROR => Err(Error::Deflate { offset: 0, message: self.message() }),
            code => Err(Error::Backend { code }),
        }
    }

    fn check(&self, rc: c_int) -> Result<()> {
        if rc == Z_OK { Ok(()) } else { Err(Error::Backend { code: rc }) }
    }

    /// The message zlib left for the last failure.
    fn message(&self) -> String {
        if self.stream.msg.is_null() {
            return "invalid deflate data".to_owned();
        }
        // SAFETY: a non-null `msg` points at one of the library's static,
        // NUL-terminated messages.
        unsafe { CStr::from_ptr(self.stream.msg) }.to_string_lossy().into_owned()
    }
}

impl Drop for Raw {
    fn drop(&mut self) {
        // SAFETY: the stream was initialized by `new` and is ended only here.
        unsafe { inflateEnd(&mut *self.stream) };
    }
}
