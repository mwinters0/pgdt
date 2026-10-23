//! The pass's forward decodes — the one at the frontier placing checkpoints,
//! one behind it reading what the frontier has passed — the compressed input
//! each is fed from, and the windows the pass has placed.

use std::sync::{Arc, RwLock};

use pgdt_core::{Error, Result};

use super::super::super::{Positioned, short_read};
use super::super::{INPUT_CHUNK, WindowProvider, gzip_error};

/// How many plaintext bytes a read skipping forward to its first wanted byte
/// throws away per decoder call.
const SKIP_CHUNK: usize = 64 * 1024;

/// The forward decodes and what the pass has learnt.
pub(super) struct Pass {
    /// The decode at the frontier, until it reaches the end or fails.
    pub(super) frontier: Option<Inflating>,
    /// The complete index, once the frontier has reached the end.
    pub(super) ended: Option<gz_seek::Index>,
    /// The index as far as a frontier decode got before it failed, which
    /// nothing continues.
    pub(super) stopped: Option<gz_seek::Index>,
    /// How many checkpoints the frontier's pass holds: the next it places is
    /// this one.
    pub(super) placed: usize,
    /// The index the pass was resumed over, answered until the pass places a
    /// checkpoint of its own: a pass resumed at a member start sets that
    /// checkpoint again with its first step.
    pub(super) resumed_over: Option<gz_seek::Index>,
    /// A decode behind the frontier, and the plaintext offset it began at.
    pub(super) behind: Option<(Inflating, u64)>,
    /// Where a skipped read's plaintext is thrown away.
    pub(super) scratch: Vec<u8>,
}

/// Which decode a read is served by.
#[derive(Clone, Copy)]
pub(super) enum Route {
    Frontier,
    Behind,
}

/// What a decode step left a read at.
pub(super) enum Served {
    /// The read's buffer is full.
    Full,
    /// The decode needs the compressed bytes from this offset on.
    Needs(u64),
    /// The decode reached the file's end, the read holding what there was.
    Ended,
}

impl Pass {
    /// The pass with `frontier` its decode, `placed` checkpoints held, and
    /// the index it was resumed over, if it was.
    pub(super) fn new(
        frontier: Inflating,
        placed: usize,
        resumed_over: Option<gz_seek::Index>,
    ) -> Pass {
        Pass {
            frontier: Some(frontier),
            ended: None,
            stopped: None,
            placed,
            resumed_over,
            behind: None,
            scratch: vec![0u8; SKIP_CHUNK],
        }
    }

    /// The index as far as the pass has got.
    pub(super) fn index(&self) -> gz_seek::Index {
        let known = self.ended.as_ref().or(self.resumed_over.as_ref()).or(self.stopped.as_ref());
        match (known, &self.frontier) {
            (Some(index), _) => index.clone(),
            (None, Some(frontier)) => frontier.forward.index(),
            (None, None) => unreachable!("a pass that lost its frontier records where it stood"),
        }
    }

    /// The frontier's plaintext offset.
    pub(super) fn frontier_position(&self) -> Result<u64> {
        match &self.frontier {
            Some(frontier) => Ok(frontier.forward.position()),
            None => Err(Error::Io(std::io::Error::other(
                "an earlier read of this gzip file's first pass failed, and the pass does not \
                 continue past a failure",
            ))),
        }
    }

    /// Which decode serves a read from `offset`: the frontier at or past it,
    /// a decode behind it otherwise — the one kept from an earlier read where
    /// it can continue there, or one begun at the last checkpoint at or
    /// before `offset`.
    pub(super) fn route(&mut self, offset: u64, windows: &PassWindows) -> Result<Route> {
        if offset >= self.frontier_position()? {
            return Ok(Route::Frontier);
        }
        let index = self.index();
        let checkpoints = index.checkpoints();
        let at = checkpoints.partition_point(|c| c.plaintext <= offset).saturating_sub(1);
        let begins = checkpoints[at].plaintext;
        // Continuing is never dearer than restarting where the kept decode
        // stands between the checkpoint and the offset.
        if self.behind.as_ref().is_some_and(|(decode, _)| {
            let position = decode.forward.position();
            position <= offset && position >= begins
        }) {
            return Ok(Route::Behind);
        }
        // deficiency: KD132 — the decode behind the frontier begins with its
        // checkpoint's CRC-32 cleared and is never finished, so what it hands
        // out is compared with nothing: over the windows this pass placed it
        // decodes the bytes the frontier checks, but over a resumed index's,
        // read through its provider, a bad window hands out wrong bytes
        // unrefused. **(b) owned by P15**, its slice 15.11 checking a read
        // inside a closed unit against the CRC-32 that unit recorded.
        let mut parts = checkpoints[..=at].to_vec();
        parts[at].crc = None;
        let prefix =
            gz_seek::Index::from_parts(index.spacing(), parts, None).map_err(gzip_error)?;
        let mut window = vec![0u8; checkpoints[at].window_len()];
        if !window.is_empty() {
            windows.read_window(at, &mut window)?;
        }
        let forward = gz_seek::Forward::resume(&prefix, &window).map_err(gzip_error)?;
        self.behind = Some((Inflating::new(forward), begins));
        Ok(Route::Behind)
    }

    /// Decode on into `out` for a read from `offset`, `filled` bytes of it
    /// already there.
    pub(super) fn serve(
        &mut self,
        route: Route,
        offset: u64,
        out: &mut [u8],
        filled: &mut usize,
        windows: &PassWindows,
        stored_size: u64,
    ) -> Result<Served> {
        let Pass { frontier, behind, scratch, placed, resumed_over, ended, stopped } = self;
        match route {
            Route::Frontier => {
                let decode = frontier.as_mut().expect("a frontier read has its decode");
                let served = decode.fill(offset, out, filled, stored_size, scratch, |forward| {
                    windows.place(*placed, forward.window());
                    *placed += 1;
                    *resumed_over = None;
                });
                match served {
                    Ok(Served::Ended) => {
                        let decode = frontier.take().expect("the frontier was decoding");
                        let before = decode.forward.index();
                        match decode.forward.finish() {
                            Ok(index) => {
                                *resumed_over = None;
                                *ended = Some(index);
                                Ok(Served::Ended)
                            }
                            Err(e) => {
                                *stopped = Some(before);
                                Err(gzip_error(e))
                            }
                        }
                    }
                    Ok(served) => Ok(served),
                    Err(e) => {
                        let decode = frontier.take().expect("the frontier was decoding");
                        *stopped = Some(decode.forward.index());
                        Err(e)
                    }
                }
            }
            Route::Behind => {
                let (decode, _) = behind.as_mut().expect("a read behind has its decode");
                let served = decode.fill(offset, out, filled, stored_size, scratch, |_| {});
                if served.is_err() {
                    *behind = None;
                }
                served
            }
        }
    }

    /// The routed decode's input, pulled from `file` at `at`.
    pub(super) fn refill_from_file(
        &mut self,
        route: Route,
        file: &std::fs::File,
        at: u64,
        stored_size: u64,
    ) -> Result<()> {
        self.decode(route).refill(at, stored_size, |buf| Ok(Positioned::read_at(file, at, buf)?))
    }

    /// The routed decode's input, `bytes` fetched from `at`.
    pub(super) fn supply(&mut self, route: Route, at: u64, bytes: &[u8]) {
        let decode = self.decode(route);
        decode.input.clear();
        decode.input.extend_from_slice(bytes);
        decode.input_at = at;
    }

    fn decode(&mut self, route: Route) -> &mut Inflating {
        match route {
            Route::Frontier => self.frontier.as_mut().expect("a frontier read has its decode"),
            Route::Behind => &mut self.behind.as_mut().expect("a read behind has its decode").0,
        }
    }
}

/// One forward decode, and the compressed input it is fed from.
pub(super) struct Inflating {
    pub(super) forward: gz_seek::Forward,
    input: Vec<u8>,
    /// The compressed offset of `input`'s first byte.
    input_at: u64,
}

impl Inflating {
    pub(super) fn new(forward: gz_seek::Forward) -> Inflating {
        Inflating { forward, input: Vec::new(), input_at: 0 }
    }

    /// Decode on into `out[filled..]`, skipping the plaintext before
    /// `offset` through `scratch`, `placed` told of each checkpoint the pass
    /// places.
    fn fill(
        &mut self,
        offset: u64,
        out: &mut [u8],
        filled: &mut usize,
        stored_size: u64,
        scratch: &mut [u8],
        mut placed: impl FnMut(&gz_seek::Forward),
    ) -> Result<Served> {
        loop {
            let position = self.forward.position();
            let skipping = position < offset;
            if !skipping && *filled == out.len() {
                return Ok(Served::Full);
            }
            let at = self.forward.compressed_position();
            let held_end = self.input_at + self.input.len() as u64;
            let held: &[u8] = if at >= self.input_at && at <= held_end {
                &self.input[(at - self.input_at) as usize..]
            } else {
                &[]
            };
            let reaches_end = at + held.len() as u64 >= stored_size;
            if held.is_empty() && !reaches_end {
                return Ok(Served::Needs(at));
            }
            let target = if skipping {
                let skip = usize::try_from(offset - position).unwrap_or(usize::MAX);
                let len = skip.min(scratch.len());
                &mut scratch[..len]
            } else {
                &mut out[*filled..]
            };
            let step = self.forward.read(held, target).map_err(gzip_error)?;
            if !skipping {
                *filled += step.progress.produced;
            }
            if step.checkpoint.is_some() {
                placed(&self.forward);
            }
            if step.progress == gz_seek::Progress::default() && step.checkpoint.is_none() {
                if reaches_end {
                    return Ok(Served::Ended);
                }
                if held.len() as u64 >= INPUT_CHUNK {
                    return Err(Error::Io(std::io::Error::other(
                        "a gzip decode made no progress over a whole input chunk",
                    )));
                }
                return Ok(Served::Needs(at));
            }
        }
    }

    /// Replace the input with the compressed bytes from `at`, up to
    /// [`INPUT_CHUNK`] of them, read by `read`.
    fn refill(
        &mut self,
        at: u64,
        stored_size: u64,
        read: impl FnOnce(&mut [u8]) -> Result<usize>,
    ) -> Result<()> {
        let want = usize::try_from(INPUT_CHUNK.min(stored_size.saturating_sub(at)))
            .map_err(|_| short_read())?;
        self.input.resize(want, 0);
        let got = read(&mut self.input)?;
        if got < want {
            return Err(Error::Io(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "the gzip file ended before the length it was opened at",
            )));
        }
        self.input_at = at;
        Ok(())
    }
}

/// Every window the pass has placed, held in memory for as long as an index
/// carrying it lives — the one the pass hands over included — and
/// before them those of the index it was resumed over, read through that
/// index's own provider: what a saved index's windows are read through.
pub(super) struct PassWindows {
    /// The resumed index's provider, for the checkpoints before `base`.
    earlier: Option<Arc<dyn WindowProvider>>,
    /// The first checkpoint whose window is held here.
    base: usize,
    held: RwLock<Vec<Box<[u8]>>>,
}

impl PassWindows {
    /// The windows of a pass from the file's start: none yet.
    pub(super) fn fresh() -> PassWindows {
        PassWindows { earlier: None, base: 0, held: RwLock::default() }
    }

    /// The windows of a pass resumed over an index of `base` checkpoints,
    /// whose own are read through `earlier`.
    pub(super) fn after(earlier: Option<Arc<dyn WindowProvider>>, base: usize) -> PassWindows {
        PassWindows { earlier, base, held: RwLock::default() }
    }

    /// Checkpoint `checkpoint` was placed with `window`: held, unless it is
    /// one the resumed index already held — a member start placed again.
    pub(super) fn place(&self, checkpoint: usize, window: &[u8]) {
        if checkpoint < self.base {
            return;
        }
        let mut held = self.held.write().unwrap_or_else(|e| e.into_inner());
        debug_assert_eq!(checkpoint, self.base + held.len(), "checkpoints are placed in order");
        held.push(window.into());
    }
}

impl WindowProvider for PassWindows {
    fn read_window(&self, checkpoint: usize, out: &mut [u8]) -> std::io::Result<()> {
        if checkpoint < self.base {
            let earlier = self.earlier.as_ref().ok_or_else(|| {
                std::io::Error::other("a resumed gzip index's window has no provider")
            })?;
            return earlier.read_window(checkpoint, out);
        }
        let held = self.held.read().unwrap_or_else(|e| e.into_inner());
        let window = held
            .get(checkpoint - self.base)
            .ok_or_else(|| std::io::Error::other("a gzip checkpoint the pass has not placed"))?;
        if window.len() != out.len() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "a window is read whole, into a buffer of its length",
            ));
        }
        out.copy_from_slice(window);
        Ok(())
    }
}
