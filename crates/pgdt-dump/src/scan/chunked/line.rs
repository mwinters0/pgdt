//! The line a carry holds ([`ChunkCarry`](super::ChunkCarry)): in the carry's
//! own buffer while it fits a chunk, and once it outgrows one, in a mapping of
//! its own ([`LineMapping`]) that grows by remapping in huge-page steps and
//! is unmapped with its last reference (`docs/design/decisions.md`, "D139").

use bytes::Bytes;

/// The line being carried across chunk boundaries: a `Vec` while the line
/// is no longer than `outgrows` bytes, a chunk's, handed off with the row it
/// completes, and a
/// [`LineMapping`] from the extension that takes it past them, so a line
/// that outgrows its chunk holds what it has written and less than one step
/// more, beside the buffer the carry keeps for the next, whatever the global
/// allocator keeps.
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

    /// The length past which the line leaves the carry's buffer: its chunk.
    pub(super) fn outgrows(&self) -> usize {
        self.outgrows
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

    /// The bytes the line keeps allocated: its buffer's capacity, and its
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
/// rather than copied and unmapped on drop, so none of an outgrown buffer
/// stays resident beside its successor, and nothing waits on an allocator's
/// purge (`docs/design/runtime-invariants.md`, "RT29"). **It grows in steps
/// of `step`, its start kept aligned to one**, so its commit, address space
/// and resident set are what it was reserved for or has written, whichever
/// is longer, and less than one step more, and
/// the kernel can back each step it fills with a transparent huge page and
/// keep it across a move (RT30). A mapping it cannot make or grow is an
/// allocation failure, aborting as a `Vec`'s does.
///
/// **A move reserves the new length and a step beside the old mapping**,
/// inaccessible and uncommitted, so only an address-space limit
/// (`RLIMIT_AS`) sees it: an embedder under one meets about twice a long line,
/// transiently.
///
/// Linux's; elsewhere it is a `Vec`, which P34 replaces.
pub struct LineMapping {
    #[cfg(target_os = "linux")]
    start: std::ptr::NonNull<u8>,
    #[cfg(target_os = "linux")]
    len: usize,
    /// The bytes mapped, of which `len` are written: a multiple of
    /// [`step`].
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
    /// A mapping of `first`, at least `capacity` bytes long or `first`'s
    /// length where that is longer, past which [`extend`](Self::extend)
    /// remaps it. A carry maps a line at its length; a longer capacity is a
    /// line's known length written into a mapping sized for it.
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

    /// The step the mapping grows in and its start is aligned to; a byte
    /// where it is a `Vec`.
    pub fn step() -> usize {
        #[cfg(target_os = "linux")]
        return step();
        #[cfg(not(target_os = "linux"))]
        return 1;
    }
}

/// **The step a mapped line grows in, and the alignment it keeps**: the
/// kernel's PMD huge page where that is at most [`STEP_CEILING`], and the
/// base page elsewhere — on a kernel built without transparent huge pages,
/// or whose PMD huge page is larger, so that a step stays slack beside a
/// line of a chunk (`docs/design/decisions.md`, "D139"). The mapping asks
/// for no huge pages: whether a step is backed by one is the host's THP
/// policy.
#[cfg(target_os = "linux")]
fn step() -> usize {
    static STEP: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
    *STEP.get_or_init(|| {
        let base = base_page();
        std::fs::read_to_string("/sys/kernel/mm/transparent_hugepage/hpage_pmd_size")
            .ok()
            .and_then(|size| size.trim().parse::<usize>().ok())
            .filter(|&huge| huge <= STEP_CEILING && huge > base && huge % base == 0)
            .unwrap_or(base)
    })
}

/// The largest PMD huge page a mapped line steps in: x86-64's and a 4 KiB
/// base-page arm64 kernel's.
#[cfg(target_os = "linux")]
const STEP_CEILING: usize = 2 << 20;

/// `len` rounded up to a whole number of [`step`]s, at least one.
#[cfg(target_os = "linux")]
fn whole_steps(len: usize) -> usize {
    len.max(1).checked_next_multiple_of(step()).unwrap_or_else(|| mapping_failed(len))
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

// This crate's `unsafe` (`docs/design/decisions.md`, "D27"): the base page's
// `sysconf`; `mmap`, `mremap` and `munmap` over a mapping `LineMapping` alone
// owns, the copy into it and the slice over it; and its `Send` and `Sync`.

/// The base page, which every mapping's length and address are whole
/// multiples of.
#[cfg(target_os = "linux")]
#[allow(unsafe_code)]
fn base_page() -> usize {
    // SAFETY: `sysconf` reads a constant of the system and touches no memory
    // of ours.
    let page = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
    usize::try_from(page).ok().filter(|&page| page > 0).unwrap_or(4096)
}

/// **`len` bytes of address space, reserved inaccessible at an address
/// aligned to a [`step`]**: more than `len` is reserved where the kernel
/// places it, and what lies outside the aligned range is unmapped. Neither
/// resident nor charged to commit, being writable by nobody, and replaced
/// whole by the mapping placed over it, so no other mapping can take the
/// range between the reservation and that.
#[cfg(target_os = "linux")]
#[allow(unsafe_code)]
fn reserve_aligned(len: usize) -> *mut libc::c_void {
    let step = step();
    let over = len.checked_add(step).unwrap_or_else(|| mapping_failed(len));
    // SAFETY: a fresh private anonymous mapping, at an address the kernel
    // picks, aliasing nothing.
    let addr = unsafe {
        libc::mmap(
            std::ptr::null_mut(),
            over,
            libc::PROT_NONE,
            libc::MAP_PRIVATE | libc::MAP_ANONYMOUS | libc::MAP_NORESERVE,
            -1,
            0,
        )
    };
    if addr == libc::MAP_FAILED {
        mapping_failed(over);
    }
    let head = (addr as usize).next_multiple_of(step) - addr as usize;
    // SAFETY: both ranges lie inside the reservation just made, which nothing
    // else references; an unmap failing leaves inaccessible pages mapped.
    unsafe {
        if head > 0 {
            libc::munmap(addr, head);
        }
        libc::munmap(addr.cast::<u8>().add(head + len).cast(), over - head - len);
        addr.cast::<u8>().add(head).cast()
    }
}

#[cfg(target_os = "linux")]
#[allow(unsafe_code)]
impl LineMapping {
    fn map(capacity: usize) -> Self {
        let mapped = whole_steps(capacity);
        let at = reserve_aligned(mapped);
        // SAFETY: `at` is the reservation just made, `mapped` long and
        // referenced by nothing else, which `MAP_FIXED` replaces.
        let addr = unsafe {
            libc::mmap(
                at,
                mapped,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_PRIVATE | libc::MAP_ANONYMOUS | libc::MAP_FIXED,
                -1,
                0,
            )
        };
        if addr == libc::MAP_FAILED {
            // SAFETY: the reservation is this call's alone.
            unsafe { libc::munmap(at, mapped) };
            mapping_failed(mapped);
        }
        let start = std::ptr::NonNull::new(addr.cast()).unwrap_or_else(|| mapping_failed(mapped));
        Self { start, len: 0, mapped }
    }

    /// Append `bytes`, growing the mapping to the whole steps they reach
    /// where they pass what is mapped: the kernel moves the pages, never
    /// their bytes.
    pub fn extend(&mut self, bytes: &[u8]) {
        let len = self.len.checked_add(bytes.len()).unwrap_or_else(|| mapping_failed(usize::MAX));
        if len > self.mapped {
            self.grow(whole_steps(len));
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

    /// Grow to `mapped` bytes where the mapping stands, keeping its start and
    /// so its alignment, or else by a move.
    fn grow(&mut self, mapped: usize) {
        // SAFETY: `start` and `mapped` are this mapping's, which nothing else
        // references while `&mut self` is held; without `MREMAP_MAYMOVE` it
        // stays where it is or the call fails, changing nothing.
        let addr = unsafe { libc::mremap(self.start.as_ptr().cast(), self.mapped, mapped, 0) };
        if addr == libc::MAP_FAILED {
            self.move_to_grow(mapped);
        } else {
            self.mapped = mapped;
        }
    }

    /// Grow to `mapped` bytes by moving to an aligned address this chooses,
    /// so each huge page backing the mapping moves whole (RT30), which an
    /// address the kernel picks need not allow.
    fn move_to_grow(&mut self, mapped: usize) {
        let to = reserve_aligned(mapped);
        // SAFETY: the mapping is this value's, referenced by nothing while
        // `&mut self` is held, and `to` the reservation just made, which
        // `MREMAP_FIXED` replaces and which cannot overlap it; `start` is
        // replaced before it is read again.
        let addr = unsafe {
            libc::mremap(
                self.start.as_ptr().cast(),
                self.mapped,
                mapped,
                libc::MREMAP_MAYMOVE | libc::MREMAP_FIXED,
                to,
            )
        };
        if addr == libc::MAP_FAILED {
            // SAFETY: the reservation is this call's alone.
            unsafe { libc::munmap(to, mapped) };
            mapping_failed(mapped);
        }
        self.start = std::ptr::NonNull::new(addr.cast()).unwrap_or_else(|| mapping_failed(mapped));
        self.mapped = mapped;
    }

    /// Grow to `mapped` bytes by a move, as where the mapping cannot grow
    /// in place.
    #[cfg(test)]
    pub(super) fn grow_by_moving(&mut self, mapped: usize) {
        self.move_to_grow(whole_steps(mapped));
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
