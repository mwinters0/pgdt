//! The line a carry holds ([`ChunkCarry`](super::ChunkCarry)): in the carry's
//! own buffer while it fits a chunk, and once it outgrows one, in a mapping of
//! its own ([`LineMapping`]) that grows by remapping and is unmapped with its
//! last reference (`docs/design/decisions.md`, "D139").

use bytes::Bytes;

/// The line being carried across chunk boundaries: a `Vec` the carry reuses
/// while the line is no longer than `outgrows` bytes, a chunk's, and a
/// [`LineMapping`] from the extension that takes it past them, so a line
/// that outgrows its chunk holds what it has written and no more, whatever
/// the global allocator keeps.
#[derive(Debug)]
pub(super) struct CarriedLine {
    /// The line while it fits its chunk; empty once it is mapped.
    small: Vec<u8>,
    /// The line once it has outgrown its chunk.
    mapped: Option<LineMapping>,
    /// The length past which a line leaves `small`: the chunk a read asks
    /// for.
    outgrows: usize,
}

impl CarriedLine {
    /// An empty line that moves to a mapping once longer than `outgrows`.
    pub(super) fn new(outgrows: usize) -> Self {
        Self { small: Vec::new(), mapped: None, outgrows }
    }

    pub(super) fn bytes(&self) -> &[u8] {
        match &self.mapped {
            Some(mapped) => mapped.as_ref(),
            None => &self.small,
        }
    }

    pub(super) fn len(&self) -> usize {
        self.bytes().len()
    }

    pub(super) fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Append `bytes`, moving the line to a mapping sized for the whole of
    /// it where they take it past its chunk.
    pub(super) fn extend(&mut self, bytes: &[u8]) {
        if let Some(mapped) = &mut self.mapped {
            mapped.extend(bytes);
        } else if self.small.len() + bytes.len() > self.outgrows {
            let mut mapped = LineMapping::reserving(self.small.len() + bytes.len(), &self.small);
            mapped.extend(bytes);
            self.small.clear();
            self.mapped = Some(mapped);
        } else {
            self.small.extend_from_slice(bytes);
        }
    }

    /// Exchange an empty line's buffer with `tail`, the start of the next
    /// line an in-place pass left, which becomes the line without a copy.
    pub(super) fn swap_in(&mut self, tail: &mut Vec<u8>) {
        debug_assert!(self.is_empty(), "a line swapped in over one being carried");
        self.mapped = None;
        std::mem::swap(&mut self.small, tail);
    }

    /// The line as one allocation the carry pass scans and a replay may keep,
    /// leaving this empty: a mapped line hands its mapping over whole, to be
    /// unmapped when the last reference drops.
    pub(super) fn freeze(&mut self) -> Bytes {
        match self.mapped.take() {
            Some(mapped) => Bytes::from_owner(mapped),
            None => Bytes::from(std::mem::take(&mut self.small)),
        }
    }

    /// Drop the line's bytes, keeping the buffer a short one reuses.
    pub(super) fn clear(&mut self) {
        self.small.clear();
        self.mapped = None;
    }

    /// The bytes the line keeps allocated: its buffer's capacity, or its
    /// mapping's length.
    #[cfg(test)]
    pub(super) fn allocated(&self) -> usize {
        self.small.capacity() + self.mapped.as_ref().map_or(0, LineMapping::mapped)
    }

    /// Whether the line has moved to a mapping.
    #[cfg(test)]
    pub(super) fn is_mapped(&self) -> bool {
        self.mapped.is_some()
    }
}

/// **A line's bytes in an anonymous mapping of its own**, grown by `mremap`
/// rather than copied and unmapped on drop, so what it holds is what it has
/// written, to a page: none of an outgrown buffer stays resident beside its
/// successor, and nothing waits on an allocator's purge
/// (`docs/design/runtime-invariants.md`, "RT29"). A mapping it cannot make or
/// grow is an allocation failure, aborting as a `Vec`'s does.
///
/// Linux's; elsewhere it is a `Vec`, which P34 replaces.
pub struct LineMapping {
    #[cfg(target_os = "linux")]
    start: std::ptr::NonNull<u8>,
    #[cfg(target_os = "linux")]
    len: usize,
    /// The bytes mapped, of which `len` are written.
    #[cfg(target_os = "linux")]
    mapped: usize,
    #[cfg(not(target_os = "linux"))]
    bytes: Vec<u8>,
}

impl std::fmt::Debug for LineMapping {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LineMapping")
            .field("len", &self.as_ref().len())
            .field("mapped", &self.mapped())
            .finish()
    }
}

impl LineMapping {
    /// A mapping of `first`, `capacity` bytes long or `first`'s length where
    /// that is longer, past which [`extend`](Self::extend) remaps it. A carry
    /// maps a line at its length; a longer capacity is a line's known length
    /// written into a mapping sized for it.
    pub fn reserving(capacity: usize, first: &[u8]) -> Self {
        let mut mapping = Self::map(capacity.max(first.len()));
        mapping.extend(first);
        mapping
    }

    /// The bytes mapped, of which [`as_ref`](AsRef::as_ref)'s are written.
    pub fn mapped(&self) -> usize {
        #[cfg(target_os = "linux")]
        return self.mapped;
        #[cfg(not(target_os = "linux"))]
        return self.bytes.capacity();
    }
}

#[cfg(not(target_os = "linux"))]
impl LineMapping {
    fn map(capacity: usize) -> Self {
        Self { bytes: Vec::with_capacity(capacity) }
    }

    /// Append `bytes`.
    pub fn extend(&mut self, bytes: &[u8]) {
        self.bytes.extend_from_slice(bytes);
    }
}

#[cfg(not(target_os = "linux"))]
impl AsRef<[u8]> for LineMapping {
    fn as_ref(&self) -> &[u8] {
        &self.bytes
    }
}

/// The allocation failure a mapping of `len` bytes is.
#[cfg(target_os = "linux")]
fn mapping_failed(len: usize) -> ! {
    std::alloc::handle_alloc_error(
        std::alloc::Layout::array::<u8>(len).unwrap_or(std::alloc::Layout::new::<u8>()),
    )
}

// The one `unsafe` this crate holds (`docs/design/decisions.md`, "D27"):
// `mmap`, `mremap` and `munmap` over a mapping this type alone owns.
#[cfg(target_os = "linux")]
#[allow(unsafe_code)]
impl LineMapping {
    fn map(capacity: usize) -> Self {
        // A zero-length mapping is refused, so an empty one maps a byte.
        let mapped = capacity.max(1);
        // SAFETY: a fresh private anonymous mapping, at an address the kernel
        // picks, aliasing nothing.
        let addr = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                mapped,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_PRIVATE | libc::MAP_ANONYMOUS,
                -1,
                0,
            )
        };
        if addr == libc::MAP_FAILED {
            mapping_failed(mapped);
        }
        let start = std::ptr::NonNull::new(addr.cast()).unwrap_or_else(|| mapping_failed(mapped));
        Self { start, len: 0, mapped }
    }

    /// Append `bytes`, remapping to exactly the length written where they
    /// pass what is mapped: the kernel moves the pages, never their bytes.
    pub fn extend(&mut self, bytes: &[u8]) {
        let len = self.len.checked_add(bytes.len()).unwrap_or_else(|| mapping_failed(usize::MAX));
        if len > self.mapped {
            // SAFETY: `start` and `mapped` are this mapping's, which nothing
            // else references while `&mut self` is held; it may move, and
            // `start` is replaced before it is read again.
            let addr = unsafe {
                libc::mremap(self.start.as_ptr().cast(), self.mapped, len, libc::MREMAP_MAYMOVE)
            };
            if addr == libc::MAP_FAILED {
                mapping_failed(len);
            }
            self.start = std::ptr::NonNull::new(addr.cast()).unwrap_or_else(|| mapping_failed(len));
            self.mapped = len;
        }
        // SAFETY: `[len - bytes.len(), len)` lies inside the mapping, which
        // `bytes`, borrowed apart from `&mut self`, cannot overlap.
        unsafe {
            std::ptr::copy_nonoverlapping(
                bytes.as_ptr(),
                self.start.as_ptr().add(self.len),
                bytes.len(),
            );
        }
        self.len = len;
    }
}

#[cfg(target_os = "linux")]
#[allow(unsafe_code)]
impl AsRef<[u8]> for LineMapping {
    fn as_ref(&self) -> &[u8] {
        // SAFETY: the first `len` bytes of the mapping are written, and stay
        // where they are while `&self` is held.
        unsafe { std::slice::from_raw_parts(self.start.as_ptr(), self.len) }
    }
}

#[cfg(target_os = "linux")]
#[allow(unsafe_code)]
impl Drop for LineMapping {
    fn drop(&mut self) {
        // SAFETY: the mapping is this value's, unreferenced once it drops. An
        // unmap failing leaves the pages mapped and nothing else wrong.
        unsafe { libc::munmap(self.start.as_ptr().cast(), self.mapped) };
    }
}

// SAFETY: the mapping is owned by this value alone, read through `&self` and
// written only through `&mut self`, as a `Vec<u8>`'s buffer is.
#[cfg(target_os = "linux")]
#[allow(unsafe_code)]
unsafe impl Send for LineMapping {}
// SAFETY: as `Send`'s.
#[cfg(target_os = "linux")]
#[allow(unsafe_code)]
unsafe impl Sync for LineMapping {}

#[cfg(test)]
mod tests;
