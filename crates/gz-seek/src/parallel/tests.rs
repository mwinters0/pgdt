use std::sync::mpsc;

use super::*;
use crate::Forward;
use crate::test_support::{Pass, bgzf, drive, forward, gzip, gzip_members, payload, spacing};

/// How a test runs the speculations a pass hands out.
#[derive(Debug, Clone, Copy)]
enum Driver {
    /// None at all: the serial pass.
    Serial,
    /// Every one, run to its end before the pass reads a byte.
    Ahead,
    /// At most `n` handed out at once, each run only once the pass waits for
    /// it.
    Lazily(usize),
    /// On `n` threads, each result handed back as it comes.
    Threads(usize),
}

/// Run `task` over `file`, fed `feed` bytes at a time.
fn run(mut task: Speculation, file: &[u8], feed: usize) -> Speculated {
    while let Some(at) = task.wants() {
        let at = at as usize;
        task.supply(&file[at..(at + feed).min(file.len())]);
    }
    task.finish()
}

/// A parallel pass over `file`, its chunks `chunk` bytes, run by `driver`,
/// fed at most `input` bytes and given at most `out` bytes of room a step:
/// what it handed out, and how many chunks it took.
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
    drive_parallel(pass, file, driver, (input, out), Vec::new())
}

fn drive_parallel(
    mut pass: Parallel,
    file: &[u8],
    driver: Driver,
    (input, out): (usize, usize),
    mut windows: Vec<Vec<u8>>,
) -> Result<(Pass, u64)> {
    let mut buf = vec![0u8; out];
    let mut plaintext = Vec::new();
    let mut handed: BTreeMap<u64, Speculation> = BTreeMap::new();
    if let Driver::Ahead = driver {
        while let Some(task) = pass.speculate() {
            pass.speculated(run(task, file, 1 << 20));
        }
    }
    std::thread::scope(|scope| {
        let (tasks, results) = match driver {
            Driver::Threads(n) => {
                let (to_worker, from_main) = mpsc::channel::<Speculation>();
                let (to_main, from_worker) = mpsc::channel::<Speculated>();
                let from_main = std::sync::Arc::new(std::sync::Mutex::new(from_main));
                for _ in 0..n {
                    let (from_main, to_main) = (from_main.clone(), to_main.clone());
                    scope.spawn(move || {
                        loop {
                            let task = from_main.lock().unwrap().recv();
                            let Ok(task) = task else { break };
                            if to_main.send(run(task, file, 7000)).is_err() {
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
                Driver::Lazily(n) => {
                    while handed.len() < n
                        && let Some(task) = pass.speculate()
                    {
                        handed.insert(task.chunk(), task);
                    }
                }
                Driver::Threads(n) => {
                    if let Some(results) = &results {
                        while let Ok(result) = results.try_recv() {
                            in_flight -= 1;
                            pass.speculated(result);
                        }
                    }
                    while in_flight < 2 * n
                        && let Some(task) = pass.speculate()
                    {
                        tasks.as_ref().unwrap().send(task).unwrap();
                        in_flight += 1;
                    }
                }
                _ => {}
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
            if let Some(chunk) = step.wait {
                match driver {
                    Driver::Lazily(_) => {
                        let task = handed.remove(&chunk).expect("a handed-out chunk");
                        pass.speculated(run(task, file, 5000));
                    }
                    Driver::Threads(_) => {
                        let result = results.as_ref().unwrap().recv().unwrap();
                        in_flight -= 1;
                        pass.speculated(result);
                    }
                    _ => unreachable!("{driver:?} leaves no speculation in flight"),
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

/// The files the tests read: one member at each level, several members, BGZF
/// and BGZF mixed with plain members.
fn files() -> Vec<(&'static str, Vec<u8>)> {
    let (a, b) = (payload(1_200_000, 3), payload(300_000, 5));
    vec![
        ("level 0", gzip(&a, 0)),
        ("level 1", gzip(&a, 1)),
        ("level 6", gzip(&a, 6)),
        ("level 9", gzip(&a, 9)),
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
            for driver in [Driver::Serial, Driver::Ahead, Driver::Lazily(3), Driver::Threads(4)] {
                for limit in [usize::MAX, 100_000] {
                    let what = format!("{name}, chunk {chunk}, {driver:?}, limit {limit}");
                    let (pass, taken) =
                        parallel(&file, 64 * 1024, chunk, limit, driver, (1 << 16, 1 << 16))
                            .unwrap_or_else(|e| panic!("{what}: {e}"));
                    assert_eq!(pass.index, serial.index, "{what}");
                    assert_eq!(pass.windows, serial.windows, "{what}");
                    assert!(pass.plaintext == serial.plaintext, "{what}");
                    if let Driver::Serial = driver {
                        assert_eq!(taken, 0, "{what}");
                    }
                }
            }
        }
    }
}

/// **Chunks are taken** where the speculations are run: most of a file's,
/// wherever its writer ends blocks more often than a chunk is long.
#[test]
fn a_pass_takes_the_chunks_it_is_handed() {
    for (name, file) in files() {
        let chunk = 32 * 1024;
        let chunks = file.len() as u64 / chunk;
        let (_, taken) =
            parallel(&file, 64 * 1024, chunk, usize::MAX, Driver::Ahead, (1 << 16, 1 << 16))
                .unwrap();
        // zlib-rs's level 1 ends no block before the last (KD129), and level
        // 0's stored blocks are each a chunk or more long.
        if name != "level 1" {
            assert!(taken * 4 >= chunks * 3, "{name}: {taken} of {chunks} chunks taken");
        }
    }
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
                drive_parallel(resumed, &file, Driver::Lazily(4), (3000, 9000), windows).unwrap();
            assert_eq!(rest.index, whole.index, "{name}, stopped after checkpoint {stop}");
            assert_eq!(rest.windows, whole.windows, "{name}, stopped after checkpoint {stop}");
            assert!(rest.plaintext == whole.plaintext[at.plaintext as usize..], "{name}");
        }
    }
}

/// **A damaged file is refused as the serial pass refuses it**: a trailer a
/// chunk holds is checked before the chunk is taken, so the engine meets the
/// damage and names it.
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
