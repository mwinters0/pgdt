use std::collections::BTreeMap;
use std::sync::mpsc;

use super::*;
use crate::Forward;
use crate::test_support::{
    Pass, bgzf, bgzf_member_of_empty_blocks, drive, forward, gzip, gzip_levels, gzip_members,
    payload, spacing,
};

/// How a test runs the tasks a pass hands out.
#[derive(Debug, Clone, Copy)]
enum Driver {
    /// None at all: the serial pass.
    Serial,
    /// Every one, run to its end as soon as the pass hands it out, before the
    /// pass reads on.
    Ahead,
    /// At most `n` handed out at once, a speculation run as soon as it is
    /// handed out and a decode only once the pass waits for it.
    Lazily(usize),
    /// Every one taken and none ever run.
    Hoarding,
    /// On `n` threads, each result handed back as it comes.
    Threads(usize),
}

/// Run `task` over `file`, fed `feed` bytes at a time.
fn run(mut task: ChunkTask, file: &[u8], feed: usize) -> ChunkDone {
    while let Some(at) = task.wants() {
        let at = at as usize;
        task.supply(&file[at..(at + feed).min(file.len())]);
    }
    task.finish()
}

/// What a test does to each result before the pass is handed it.
type Tamper<'a> = &'a (dyn Fn(&mut ChunkDone) + Sync);

/// A parallel pass over `file`, its chunks `chunk` bytes, run by `driver`,
/// fed at most `input` bytes and given at most `out` bytes of room a step:
/// what it handed out, and how many decodes it took.
fn parallel(
    file: &[u8],
    spacing: u64,
    chunk: u64,
    limit: usize,
    driver: Driver,
    (input, out): (usize, usize),
) -> Result<(Pass, u64)> {
    let chunk = NonZeroU64::new(chunk).unwrap();
    let pass = Parallel::new(self::spacing(spacing), chunk, limit, file.len() as u64)?;
    drive_parallel(pass, file, driver, (input, out), Vec::new(), &|_| {})
}

fn drive_parallel(
    mut pass: Parallel,
    file: &[u8],
    driver: Driver,
    (input, out): (usize, usize),
    mut windows: Vec<Vec<u8>>,
    tamper: Tamper,
) -> Result<(Pass, u64)> {
    let mut buf = vec![0u8; out];
    let mut plaintext = Vec::new();
    let mut handed: BTreeMap<TaskId, ChunkTask> = BTreeMap::new();
    let finish = |task, feed| {
        let mut done = run(task, file, feed);
        tamper(&mut done);
        done
    };
    std::thread::scope(|scope| {
        let (tasks, results) = match driver {
            Driver::Threads(n) => {
                let (to_worker, from_main) = mpsc::channel::<ChunkTask>();
                let (to_main, from_worker) = mpsc::channel::<ChunkDone>();
                let from_main = std::sync::Arc::new(std::sync::Mutex::new(from_main));
                for _ in 0..n {
                    let (from_main, to_main) = (from_main.clone(), to_main.clone());
                    scope.spawn(move || {
                        loop {
                            let task = from_main.lock().unwrap().recv();
                            let Ok(task) = task else { break };
                            if to_main.send(finish(task, 7000)).is_err() {
                                break;
                            }
                        }
                    });
                }
                (Some(to_worker), Some(from_worker))
            }
            _ => (None, None),
        };
        let mut in_flight = 0;
        let result = loop {
            match driver {
                Driver::Ahead => {
                    while let Some(task) = pass.task() {
                        pass.done(finish(task, 1 << 20));
                    }
                }
                Driver::Lazily(n) => {
                    handed.retain(|&id, _| pass.wants(id));
                    while handed.len() < n
                        && let Some(task) = pass.task()
                    {
                        match task.id().decode_of() {
                            None => pass.done(finish(task, 5000)),
                            Some(_) => drop(handed.insert(task.id(), task)),
                        }
                    }
                }
                Driver::Hoarding => {
                    while let Some(task) = pass.task() {
                        handed.insert(task.id(), task);
                    }
                }
                Driver::Threads(n) => {
                    if let Some(results) = &results {
                        while let Ok(result) = results.try_recv() {
                            in_flight -= 1;
                            pass.done(result);
                        }
                    }
                    while in_flight < 2 * n
                        && let Some(task) = pass.task()
                    {
                        tasks.as_ref().unwrap().send(task).unwrap();
                        in_flight += 1;
                    }
                }
                Driver::Serial => {}
            }
            let at = pass.compressed_position() as usize;
            let supply = &file[at.min(file.len())..(at + input).min(file.len())];
            let step = match pass.read(supply, &mut buf) {
                Ok(step) => step,
                Err(e) => break Err(e),
            };
            plaintext.extend_from_slice(&buf[..step.progress.produced]);
            if step.checkpoint.is_some() {
                windows.push(pass.window().to_vec());
            }
            if let Some(id) = step.wait {
                match driver {
                    Driver::Lazily(_) => {
                        let task = handed.remove(&id).expect("a handed-out task");
                        pass.done(finish(task, 5000));
                    }
                    Driver::Threads(_) => {
                        let result = results.as_ref().unwrap().recv().unwrap();
                        in_flight -= 1;
                        pass.done(result);
                    }
                    _ => unreachable!("{driver:?} leaves no task in flight"),
                }
                continue;
            }
            if step.needs_input() {
                break Ok(());
            }
        };
        drop(tasks);
        if let Some(results) = results {
            while in_flight > 0 {
                results.recv().unwrap();
                in_flight -= 1;
            }
        }
        result
    })?;
    let taken = pass.taken();
    Ok((Pass { index: pass.finish()?, plaintext, windows }, taken))
}

/// The files the tests read: one member at each level, one changing level
/// across a static block, several members, BGZF and BGZF mixed with plain
/// members.
fn files() -> Vec<(&'static str, Vec<u8>)> {
    let (a, b) = (payload(1_200_000, 3), payload(300_000, 5));
    vec![
        ("level 0", gzip(&a, 0)),
        ("level 1", gzip(&a, 1)),
        ("level 6", gzip(&a, 6)),
        ("level 9", gzip(&a, 9)),
        ("levels", gzip_levels(&[(&b, 6), (&a[..400_000], 1), (&b, 6)])),
        ("members", gzip_members(&[&a, b"", &b, &a[..100]], 6)),
        ("bgzf", bgzf(&a)),
        ("mixed", [bgzf(&b), gzip(&a, 6), bgzf(&b)].concat()),
    ]
}

/// **The index is the serial pass's at every worker count**: the same
/// checkpoints, windows and plaintext whatever the chunk size, the limit, and
/// whether the speculations run ahead, lazily, on threads or not at all.
#[test]
fn the_index_is_the_serial_pass_s_at_every_worker_count() {
    for (name, file) in files() {
        let serial = forward(&file, 64 * 1024, 1 << 16, 1 << 16).unwrap();
        for chunk in [8 * 1024, 40 * 1024, 200 * 1024] {
            let drivers = [
                Driver::Serial,
                Driver::Ahead,
                Driver::Lazily(3),
                Driver::Threads(4),
                Driver::Hoarding,
            ];
            for driver in drivers {
                for limit in [usize::MAX, 100_000] {
                    let what = format!("{name}, chunk {chunk}, {driver:?}, limit {limit}");
                    let (pass, taken) =
                        parallel(&file, 64 * 1024, chunk, limit, driver, (1 << 16, 1 << 16))
                            .unwrap_or_else(|e| panic!("{what}: {e}"));
                    assert_eq!(pass.index, serial.index, "{what}");
                    assert_eq!(pass.windows, serial.windows, "{what}");
                    assert!(pass.plaintext == serial.plaintext, "{what}");
                    if let Driver::Serial | Driver::Hoarding = driver {
                        assert_eq!(taken, 0, "{what}");
                    }
                }
            }
        }
    }
}

/// **Decodes are taken** where the tasks are run: most of a file's chunks,
/// wherever its writer ends blocks more often than a chunk is long — or, in a
/// BGZF file, of its chunks holding a member start, a chunk read off its
/// members' headers stopping at the first at or past its end.
#[test]
fn a_pass_takes_the_chunks_it_is_handed() {
    for (name, file) in files() {
        let chunk = 32 * 1024;
        let mut chunks = file.len() as u64 / chunk;
        if name == "bgzf" {
            let (mut starts, mut at) = (std::collections::BTreeSet::new(), 0);
            while let Some(len) = crate::bgzf::bgzf_member_len(&file[at..]) {
                starts.insert(at as u64 / chunk);
                at += len as usize;
            }
            chunks = starts.range(1..chunks).count() as u64;
        }
        let (_, taken) =
            parallel(&file, 64 * 1024, chunk, usize::MAX, Driver::Ahead, (1 << 16, 1 << 16))
                .unwrap();
        // zlib-rs's level 1 ends a block only where its writer finishes.
        if name != "level 1" && name != "levels" {
            assert!(taken * 4 >= chunks * 3, "{name}: {taken} of {chunks} chunks taken");
        }
    }
}

/// **A decode run ahead is ready before the frontier takes it**: with every
/// task run as soon as it is handed out, the pass holds a decode before the
/// frontier has taken any, so a caller can wait for one rather than race it.
#[test]
fn a_decode_run_ahead_is_ready_before_the_frontier_takes_it() {
    let file = gzip(&payload(1_200_000, 3), 6);
    let chunk = NonZeroU64::new(8 * 1024).unwrap();
    let mut pass = Parallel::new(spacing(64 * 1024), chunk, usize::MAX, file.len() as u64).unwrap();
    let mut buf = vec![0u8; 1024];
    let mut ready_before_a_take = false;
    loop {
        while let Some(task) = pass.task() {
            pass.done(run(task, &file, 1 << 20));
        }
        ready_before_a_take |= pass.taken() == 0 && pass.ready() > 0;
        let at = (pass.compressed_position() as usize).min(file.len());
        let step = pass.read(&file[at..], &mut buf).unwrap();
        assert_eq!(step.wait, None, "every task handed out has been run");
        if step.needs_input() {
            break;
        }
    }
    assert!(ready_before_a_take);
    assert!(pass.taken() > 0);
}

/// **The frontier never waits for a speculation**: with every task taken
/// and none run, it decodes the whole file itself, the serial pass, no step
/// naming a task to wait for.
#[test]
fn the_frontier_never_waits_for_a_speculation() {
    for (name, file) in files() {
        let serial = forward(&file, 64 * 1024, 1 << 16, 1 << 16).unwrap();
        let chunk = NonZeroU64::new(16 * 1024).unwrap();
        let mut pass =
            Parallel::new(spacing(64 * 1024), chunk, usize::MAX, file.len() as u64).unwrap();
        let (mut buf, mut plaintext, mut hoarded) = (vec![0u8; 4096], Vec::new(), 0);
        loop {
            while let Some(task) = pass.task() {
                assert_eq!(task.id().decode_of(), None, "{name}: a decode with no guess run");
                hoarded += 1;
            }
            let at = (pass.compressed_position() as usize).min(file.len());
            let step = pass.read(&file[at..], &mut buf).unwrap();
            assert_eq!(step.wait, None, "{name}: the frontier waits for a speculation");
            plaintext.extend_from_slice(&buf[..step.progress.produced]);
            if step.needs_input() {
                break;
            }
        }
        assert!(hoarded > 1, "{name}: {hoarded} speculations handed out");
        assert!(plaintext == serial.plaintext, "{name}");
        assert_eq!(pass.finish().unwrap(), serial.index, "{name}");
    }
}

/// **A speculation is handed out past where the frontier will be when it
/// lands**: one that lands with the frontier some chunks on from where it was
/// handed out has the next handed out at least that many chunks past the one
/// after the frontier's, and one landing with the frontier where it was lets
/// the next be the one after.
#[test]
fn a_speculation_is_handed_out_as_far_ahead_as_the_last_one_lagged() {
    let file = gzip(&payload(1_200_000, 3), 6);
    let chunk = NonZeroU64::new(8 * 1024).unwrap();
    let chunk_bits = 8 * chunk.get();
    let mut pass = Parallel::new(spacing(64 * 1024), chunk, usize::MAX, file.len() as u64).unwrap();
    let first = pass.task().unwrap();
    assert_eq!(first.id().chunk(), 1);
    let mut buf = vec![0u8; 1024];
    while pass.at / chunk_bits < 4 {
        let at = (pass.compressed_position() as usize).min(file.len());
        assert_eq!(pass.read(&file[at..], &mut buf).unwrap().wait, None);
    }
    let crossed = pass.at / chunk_bits;
    pass.done(run(first, &file, 1 << 20));
    assert_eq!(pass.lag, crossed);
    let next = pass.task().unwrap();
    assert!(
        next.id().chunk() > 2 * crossed,
        "chunk {} handed out with the frontier in {crossed}, {crossed} crossed",
        next.id().chunk()
    );
    let frontier = pass.at / chunk_bits;
    pass.done(run(next, &file, 1 << 20));
    assert_eq!(pass.lag, 0);
    let after = pass.task().unwrap();
    assert_eq!(after.id().chunk(), (frontier + 1).max(2 * crossed + 2));
}

/// **A lead places a speculation that many chunks past where it would land**,
/// where the bound it is handed out under reaches that far, and where it does
/// not, the speculation is handed out where it lands.
#[test]
fn a_speculation_is_placed_its_lead_past_where_it_lands() {
    let file = gzip(&payload(1_200_000, 3), 6);
    let chunk = NonZeroU64::new(8 * 1024).unwrap();
    assert!(file.len() as u64 > 10 * chunk.get(), "{} bytes", file.len());
    let mut pass = Parallel::new(spacing(64 * 1024), chunk, usize::MAX, file.len() as u64).unwrap();
    pass.set_lead(3);
    let mut through = |last| {
        std::iter::from_fn(|| pass.speculation_through(last))
            .map(|t| t.id().chunk())
            .collect::<Vec<_>>()
    };
    assert_eq!(through(2), [1, 2]);
    assert_eq!(through(7), [4, 5, 6, 7]);
}

/// **Decodes and speculations are handed out apart and counted apart**: no
/// decode is handed out before a guess lets one begin, a speculation counts
/// as a guess from when it is handed out until the frontier passes its
/// guess, and a decode counts as one from when it is handed out.
#[test]
fn decodes_and_speculations_are_handed_out_and_counted_apart() {
    let file = gzip(&payload(1_200_000, 3), 6);
    // Each chunk long enough to hold a block start of its own.
    let chunk = NonZeroU64::new(64 * 1024).unwrap();
    assert!(file.len() as u64 > 5 * chunk.get(), "{} bytes", file.len());
    let mut pass = Parallel::new(spacing(64 * 1024), chunk, usize::MAX, file.len() as u64).unwrap();
    assert!(pass.decode_task().is_none());
    let speculations = std::iter::from_fn(|| pass.speculation_through(4)).collect::<Vec<_>>();
    assert_eq!((pass.guesses_pending(), pass.decodes_pending()), (4, 0));
    for task in speculations {
        assert!(!task.id().is_decode());
        pass.done(run(task, &file, 1 << 20));
    }
    assert_eq!((pass.guesses_pending(), pass.decodes_pending()), (4, 0));
    let mut buf = vec![0u8; 1024];
    let decode = loop {
        if let Some(task) = pass.decode_task() {
            break task;
        }
        let at = (pass.compressed_position() as usize).min(file.len());
        let step = pass.read(&file[at..], &mut buf).unwrap();
        assert!(!step.needs_input(), "no decode was handed out before the file's end");
    };
    assert!(decode.id().is_decode());
    assert_eq!(pass.decodes_pending(), 1);
    assert!(pass.speculation_through(4).is_none());
}

/// Twice the most plaintext between two places a decode of `file` may resume
/// at, as [`SPECULATION_LIMIT`](crate::SPECULATION_LIMIT) is twice the
/// largest block zlib ends.
fn twice_the_largest_block(file: &[u8]) -> usize {
    let truth = crate::test_support::truth(file);
    let mut places = truth.events.iter().filter(|(_, e)| e.point().is_some()).map(|&(at, _)| at);
    let ends = places.by_ref().chain([truth.plaintext.len() as u64]).collect::<Vec<_>>();
    let largest = ends.windows(2).map(|w| w[1] - w[0]).max().unwrap_or(0).max(ends[0]);
    2 * largest as usize
}

/// A pass over `file` with every task run as soon as it is handed out: what
/// it handed out, how many decodes it took, and how many times its chain was
/// anchored or dropped.
fn run_ahead(file: &[u8], chunk: u64, limit: usize) -> (Pass, u64, u64) {
    let chunk = NonZeroU64::new(chunk).unwrap();
    let mut pass = Parallel::new(spacing(64 * 1024), chunk, limit, file.len() as u64).unwrap();
    let (mut buf, mut plaintext, mut windows) = (vec![0u8; 1 << 16], Vec::new(), Vec::new());
    loop {
        while let Some(task) = pass.task() {
            pass.done(run(task, file, 1 << 20));
        }
        let at = (pass.compressed_position() as usize).min(file.len());
        let step = pass.read(&file[at..], &mut buf).unwrap();
        assert_eq!(step.wait, None, "every task handed out has been run");
        plaintext.extend_from_slice(&buf[..step.progress.produced]);
        if step.checkpoint.is_some() {
            windows.push(pass.window().to_vec());
        }
        if step.needs_input() {
            break;
        }
    }
    let (taken, anchors) = (pass.taken(), pass.chain.generation());
    (Pass { index: pass.finish().unwrap(), plaintext, windows }, taken, anchors)
}

/// **A limit never ends the chain of windows**: with chunks holding more
/// plaintext than the limit, each guess it cuts is continued from its stop,
/// so the frontier is anchored no more often than with no limit at all, and
/// takes more decodes than with none, building the serial pass's index.
#[test]
fn a_limit_never_ends_the_chain() {
    for (name, file) in files() {
        // zlib-rs's level 1 ends a block only where its writer finishes, which no chunk
        // holds under a limit.
        if name == "level 1" || name == "levels" {
            continue;
        }
        let serial = forward(&file, 64 * 1024, 1 << 16, 1 << 16).unwrap();
        let chunk = 160 * 1024;
        let limit = twice_the_largest_block(&file);
        let (_, whole, unlimited) = run_ahead(&file, chunk, usize::MAX);
        let (pass, taken, anchors) = run_ahead(&file, chunk, limit);
        assert_eq!(pass.index, serial.index, "{name}");
        assert_eq!(pass.windows, serial.windows, "{name}");
        assert!(pass.plaintext == serial.plaintext, "{name}");
        assert_eq!(anchors, unlimited, "{name}: anchored again at a limit {limit}");
        assert!(taken > whole, "{name}: {taken} pieces taken, {whole} chunks at no limit");
    }
}

/// **A chain the event count cuts is continued as one the limit cuts**: a
/// BGZF file of more one-byte members, each of empty fixed blocks, than a
/// guess may span in a chunk is read off its headers in pieces, each taken,
/// building the serial pass's index run ahead and on threads — given a few
/// bytes of room a step, as a member's byte is all the plaintext it holds.
#[test]
fn a_chain_the_event_count_cuts_is_continued() {
    let members = 2 * crate::speculate::EVENTS_A_GUESS;
    let file: Vec<u8> =
        (0..members).flat_map(|i| bgzf_member_of_empty_blocks(4, b'a' + (i % 26) as u8)).collect();
    let serial = forward(&file, 64 * 1024, 1 << 16, 1 << 16).unwrap();
    let chunk = file.len() as u64 / 3;
    for driver in [Driver::Ahead, Driver::Threads(4)] {
        let (pass, taken) =
            parallel(&file, 64 * 1024, chunk, usize::MAX, driver, (1 << 16, 64)).unwrap();
        assert_eq!(pass.index, serial.index, "{driver:?}");
        assert!(pass.plaintext == serial.plaintext, "{driver:?}");
        if let Driver::Ahead = driver {
            assert!(taken > 2, "{taken} pieces taken over two chunks");
        }
    }
}

/// **A speculation is handed out no further ahead than its caller bounds
/// it**: `speculation_through` hands out each chunk up to its bound and then nothing,
/// and the first chunk past the bound is the first a wider one hands out.
#[test]
fn a_speculation_is_handed_out_no_further_than_its_bound() {
    let file = gzip(&payload(1_200_000, 3), 6);
    let chunk = NonZeroU64::new(8 * 1024).unwrap();
    assert!(file.len() as u64 > 6 * chunk.get(), "{} bytes", file.len());
    let mut pass =
        Parallel::new(self::spacing(64 * 1024), chunk, usize::MAX, file.len() as u64).unwrap();
    let mut through = |last| {
        std::iter::from_fn(|| pass.speculation_through(last))
            .map(|t| t.id().chunk())
            .collect::<Vec<_>>()
    };
    assert_eq!(through(3), [1, 2, 3]);
    assert_eq!(through(3), Vec::<u64>::new());
    assert_eq!(through(5), [4, 5]);
}

/// **A continuation is bounded as the chunk it begins in is**: handed out
/// ahead of any later chunk's speculation, and not past the bound.
#[test]
fn a_continuation_is_handed_out_within_its_bound() {
    let file = gzip(&payload(1_200_000, 3), 6);
    let chunk = NonZeroU64::new(160 * 1024).unwrap();
    let limit = twice_the_largest_block(&file);
    let mut pass = Parallel::new(spacing(64 * 1024), chunk, limit, file.len() as u64).unwrap();
    let first = pass.speculation_through(1).unwrap();
    assert_eq!(first.id().continues_from(), None);
    pass.done(run(first, &file, 1 << 20));
    let from = pass.to_continue.keys().next().copied().expect("a guess the limit cut");
    assert_eq!(from / (8 * chunk.get()), 1, "cut inside its chunk");
    assert!(pass.speculation_through(0).is_none());
    let next = pass.speculation_through(1).unwrap();
    assert_eq!((next.id().chunk(), next.id().continues_from()), (1, Some(from)));
    assert!(pass.wants(next.id()));
}

/// **A decode begun with a history the frontier contradicts is never taken**:
/// a guess whose history at its stop is wrong gives the next chunk's decode a
/// wrong window, and the plaintext it decodes from there is dropped, those
/// bytes decoded again by the engine from the frontier's own.
#[test]
fn a_decode_the_frontier_contradicts_is_decoded_again() {
    let file = gzip(&payload(1_500_000, 19), 6);
    let serial = forward(&file, 64 * 1024, 1 << 16, 1 << 16).unwrap();
    let pass = |tamper: Tamper| {
        let chunk = NonZeroU64::new(16 * 1024).unwrap();
        let pass = Parallel::new(spacing(64 * 1024), chunk, usize::MAX, file.len() as u64)?;
        drive_parallel(pass, &file, Driver::Ahead, (1 << 16, 1 << 16), Vec::new(), tamper)
    };
    let (_, whole) = pass(&|_| {}).unwrap();
    let tampered = std::sync::atomic::AtomicU64::new(0);
    let wrong = |done: &mut ChunkDone| {
        if let Finished::Speculated(result) = &mut done.0
            && result.chunk % 3 == 1
            && let Some(guess) = &mut result.found
            && let Some(byte) = guess.end.iter_mut().rev().find(|v| **v < 256)
        {
            *byte ^= 1;
            tampered.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }
    };
    let (contradicted, taken) = pass(&wrong).unwrap();
    assert!(tampered.into_inner() > 0);
    assert_eq!(contradicted.index, serial.index);
    assert_eq!(contradicted.windows, serial.windows);
    assert!(contradicted.plaintext == serial.plaintext);
    assert!(taken > 0 && taken < whole, "{taken} taken, {whole} untampered");
}

/// The buffers a pass is fed and given are its caller's to choose.
#[test]
fn the_index_is_one_whatever_the_buffers() {
    let file = gzip(&payload(500_000, 7), 6);
    let serial = forward(&file, 16 * 1024, 1 << 20, 1 << 20).unwrap();
    for buffers in [(1, 1 << 16), (7, 13), (4096, 1), (65_536, 100_003), (file.len(), 3)] {
        let (pass, _) =
            parallel(&file, 16 * 1024, 16 * 1024, usize::MAX, Driver::Lazily(2), buffers).unwrap();
        assert_eq!(pass.index, serial.index, "buffers {buffers:?}");
        assert_eq!(pass.windows, serial.windows, "buffers {buffers:?}");
        assert!(pass.plaintext == serial.plaintext, "buffers {buffers:?}");
    }
}

/// **A resumed parallel pass builds the uninterrupted index**, from any
/// checkpoint a serial pass stopped at.
#[test]
fn a_resumed_pass_builds_the_uninterrupted_index() {
    for (name, file) in files() {
        let whole = forward(&file, 64 * 1024, 1 << 20, 1 << 20).unwrap();
        let count = whole.index.checkpoints().len();
        for stop in (1..count).step_by(count.div_ceil(6)) {
            let first = drive(
                Forward::new(spacing(64 * 1024)).unwrap(),
                &file,
                5000,
                7000,
                Vec::new(),
                Some(stop),
            )
            .unwrap();
            let last = first.index.checkpoints().len() - 1;
            let at = first.index.checkpoints()[last];
            let chunk = NonZeroU64::new(24 * 1024).unwrap();
            let resumed = Parallel::resume(
                &first.index,
                &first.windows[last],
                chunk,
                1 << 20,
                file.len() as u64,
            )
            .unwrap();
            assert_eq!(resumed.position(), at.plaintext);
            let mut windows = first.windows.clone();
            if at.start == Start::Member {
                windows.pop();
            }
            let (rest, _) =
                drive_parallel(resumed, &file, Driver::Lazily(4), (3000, 9000), windows, &|_| {})
                    .unwrap();
            assert_eq!(rest.index, whole.index, "{name}, stopped after checkpoint {stop}");
            assert_eq!(rest.windows, whole.windows, "{name}, stopped after checkpoint {stop}");
            assert!(rest.plaintext == whole.plaintext[at.plaintext as usize..], "{name}");
        }
    }
}

/// **A damaged file is refused as the serial pass refuses it**: a trailer a
/// decode holds is checked before the decode is taken, so the frontier's
/// engine meets the damage and names it.
#[test]
fn damage_is_refused_as_the_serial_pass_refuses_it() {
    let plaintext = payload(800_000, 11);
    let file = gzip(&plaintext, 6);
    let n = file.len();
    let mut crc = file.clone();
    crc[n - 8] ^= 1;
    let mut len = file.clone();
    len[n - 1] ^= 1;
    for damaged in [crc, len] {
        let serial = forward(&damaged, 64 * 1024, 4096, 4096).err().unwrap();
        for driver in [Driver::Ahead, Driver::Threads(3)] {
            let result = parallel(&damaged, 64 * 1024, 16 * 1024, usize::MAX, driver, (4096, 4096));
            assert_eq!(result.err(), Some(serial.clone()), "{driver:?}");
        }
    }
    for i in 0..40 {
        let at = 20 + (i * 7919 * 131) % (n - 40);
        let mut data = file.clone();
        data[at] ^= 1 << (i % 8);
        let serial = forward(&data, 64 * 1024, 4096, 4096);
        let result = parallel(&data, 64 * 1024, 16 * 1024, usize::MAX, Driver::Ahead, (4096, 4096));
        match (serial, result) {
            (Err(serial), Err(parallel)) => assert_eq!(
                std::mem::discriminant(&parallel),
                std::mem::discriminant(&serial),
                "at {at}: {parallel} where the serial pass refuses {serial}"
            ),
            // A bit no decode reads, such as a padding bit's.
            (Ok(serial), Ok((parallel, _))) => {
                assert_eq!(parallel.index, serial.index, "at {at}");
                assert!(parallel.plaintext == serial.plaintext, "at {at}");
            }
            (serial, parallel) => panic!(
                "at {at}: the serial pass {:?}, the parallel {:?}",
                serial.err(),
                parallel.err()
            ),
        }
    }
    for cut in [n / 2, n - 9, n - 1] {
        let result =
            parallel(&file[..cut], 64 * 1024, 16 * 1024, usize::MAX, Driver::Ahead, (4096, 4096));
        assert!(matches!(result, Err(Error::Truncated { .. })), "cut at {cut}");
    }
}

/// **A BGZF file's damage is refused as the serial pass refuses it**, its
/// chunks read off their members' headers: a member's CRC-32 or `ISIZE` wrong
/// or its deflate data damaged is refused by the same error, and one stating
/// its own length wrong, which no decode reads, builds the same index.
#[test]
fn bgzf_damage_is_refused_as_the_serial_pass_refuses_it() {
    let file = bgzf(&payload(800_000, 37));
    let mut members = Vec::new();
    while let Some(len) = crate::bgzf::bgzf_member_len(&file[members.iter().sum::<usize>()..]) {
        members.push(len as usize);
    }
    let i = members.len() / 2;
    let (at, len) = (members[..i].iter().sum::<usize>(), members[i]);
    let damage = [("crc", at + len - 8, 1), ("isize", at + len - 4, 1), ("length", at + 16, 1)];
    let damage = damage.into_iter().chain([("deflate", at + 40, 0x10)]);
    for (what, byte, bit) in damage {
        let mut damaged = file.clone();
        damaged[byte] ^= bit;
        let serial = forward(&damaged, 64 * 1024, 4096, 4096);
        assert_eq!(serial.is_ok(), what == "length", "{what}: {:?}", serial.as_ref().err());
        for chunk in [16 * 1024, 64 * 1024] {
            for driver in [Driver::Ahead, Driver::Threads(3)] {
                let what = format!("{what}, chunk {chunk}, {driver:?}");
                let result = parallel(&damaged, 64 * 1024, chunk, usize::MAX, driver, (4096, 4096));
                match (&serial, result) {
                    (Err(serial), Err(parallel)) => assert_eq!(&parallel, serial, "{what}"),
                    (Ok(serial), Ok((parallel, _))) => {
                        assert_eq!(parallel.index, serial.index, "{what}");
                        assert!(parallel.plaintext == serial.plaintext, "{what}");
                    }
                    (serial, parallel) => panic!(
                        "{what}: the serial pass {:?}, the parallel {:?}",
                        serial.as_ref().err(),
                        parallel.err()
                    ),
                }
            }
        }
    }
}

/// **What follows the last member must be a member**, as for the serial
/// pass.
#[test]
fn bytes_after_the_last_member_are_refused() {
    let mut file = gzip(&payload(300_000, 13), 6);
    let end = file.len() as u64;
    file.extend_from_slice(&[0, 0, 0, 0]);
    let result = parallel(&file, 64 * 1024, 8 * 1024, usize::MAX, Driver::Ahead, (4096, 4096));
    assert_eq!(result.err(), Some(Error::NotGzip { offset: end }));
}
