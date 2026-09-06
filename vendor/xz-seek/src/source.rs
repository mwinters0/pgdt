//! Where the compressed bytes come from.
//!
//! The crate opens nothing. A caller hands it something that can answer "give
//! me `len` bytes at `offset`" and "how big are you", and that is the whole
//! contract — see `docs/design/roadmap.md`, "The compressed bytes arrive through
//! `CompressedSource`". A local file, a memory map, a ranged HTTP endpoint and
//! an object store all satisfy it; only the first two are implemented here.

use std::io;

/// A byte source the reader addresses positionally.
///
/// `&self`, against the reader's `&mut self`, so that N readers can share one
/// source. The trait deliberately carries no `Send`, `Sync` or `Clone` bound:
/// the parallel entry point will ask for what it needs at its own signature
/// rather than taxing every implementor today.
pub trait CompressedSource {
    /// Fill `buf` with the bytes at `offset`, returning how many were written.
    ///
    /// **Fill-or-EOF.** A return shorter than `buf.len()` means the source ended
    /// there, and means nothing else — never a partial read that a retry would
    /// complete. An implementation over a transport that short-reads must loop
    /// internally, as the [`std::fs::File`] implementation does.
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> io::Result<usize>;

    /// The source's whole size in bytes.
    ///
    /// Required, not optional: the footer walk starts at the end of the file.
    fn size(&self) -> io::Result<u64>;
}

/// Positioned reads on a file, with no cursor and no `&mut`.
///
/// `FileExt::read_at` is permitted to return short for reasons that are not end
/// of file, so this loops until the buffer is full or a read returns zero. That
/// loop is what makes the trait's fill-or-EOF contract true of a `File`.
///
/// **Unix only.** Windows has the same capability under a different name
/// (`std::os::windows::fs::FileExt::seek_read`) and the impl is eight lines, but
/// nothing here can compile or run it, and shipping a block that has never been
/// built is worse than an absence a compiler names. The crate still builds
/// elsewhere; only this impl is missing.
#[cfg(unix)]
impl CompressedSource for std::fs::File {
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> io::Result<usize> {
        use std::os::unix::fs::FileExt;
        let mut done = 0;
        while done < buf.len() {
            match FileExt::read_at(self, &mut buf[done..], offset + done as u64) {
                Ok(0) => break,
                Ok(n) => done += n,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                Err(e) => return Err(e),
            }
        }
        Ok(done)
    }

    fn size(&self) -> io::Result<u64> {
        Ok(self.metadata()?.len())
    }
}

/// A file already in memory.
impl CompressedSource for &[u8] {
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> io::Result<usize> {
        let start = usize::try_from(offset)
            .unwrap_or(usize::MAX)
            .min(self.len());
        let n = (self.len() - start).min(buf.len());
        buf[..n].copy_from_slice(&self[start..start + n]);
        Ok(n)
    }

    fn size(&self) -> io::Result<u64> {
        Ok(self.len() as u64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_slice_fills_what_it_can_and_stops_at_its_end() {
        let data: &[u8] = b"0123456789";
        let mut buf = [0u8; 4];
        assert_eq!(data.read_at(0, &mut buf).unwrap(), 4);
        assert_eq!(&buf, b"0123");
        assert_eq!(data.read_at(6, &mut buf).unwrap(), 4);
        assert_eq!(&buf, b"6789");

        // Short at the end, zero past it, and never an error.
        assert_eq!(data.read_at(8, &mut buf).unwrap(), 2);
        assert_eq!(data.read_at(10, &mut buf).unwrap(), 0);
        assert_eq!(data.read_at(u64::MAX, &mut buf).unwrap(), 0);
        assert_eq!(data.size().unwrap(), 10);

        let empty: &[u8] = b"";
        assert_eq!(empty.read_at(0, &mut buf).unwrap(), 0);
        assert_eq!(empty.size().unwrap(), 0);
    }

    #[test]
    fn a_file_and_a_slice_answer_identically() {
        let dir = std::env::temp_dir().join(format!("xz-seek-source-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("bytes");
        let data: Vec<u8> = (0..=255u8).cycle().take(9999).collect();
        std::fs::write(&path, &data).unwrap();
        let file = std::fs::File::open(&path).unwrap();

        let slice: &[u8] = &data;
        assert_eq!(
            CompressedSource::size(&file).unwrap(),
            CompressedSource::size(&slice).unwrap()
        );
        for (offset, len) in [
            (0u64, 1usize),
            (1, 4096),
            (9998, 8),
            (9999, 8),
            (5000, 4999),
        ] {
            let mut a = vec![0u8; len];
            let mut b = vec![0u8; len];
            let na = CompressedSource::read_at(&file, offset, &mut a).unwrap();
            let nb = CompressedSource::read_at(&slice, offset, &mut b).unwrap();
            assert_eq!(na, nb, "at {offset}+{len}");
            assert_eq!(a[..na], b[..nb], "at {offset}+{len}");
        }

        std::fs::remove_dir_all(&dir).unwrap();
    }
}
