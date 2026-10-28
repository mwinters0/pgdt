use super::*;

/// Leave `threads` of the runtime's blocking threads idle, each having
/// run a job, as the tasks ahead of a frontier leave them.
async fn idle_blocking_threads(threads: usize) {
    let barrier = Arc::new(std::sync::Barrier::new(threads));
    let jobs: Vec<_> = (0..threads)
        .map(|_| {
            let barrier = Arc::clone(&barrier);
            tokio::task::spawn_blocking(move || {
                barrier.wait();
            })
        })
        .collect();
    for job in jobs {
        job.await.unwrap();
    }
}

#[tokio::test(flavor = "current_thread")]
async fn every_job_runs_on_one_thread_while_other_blocking_threads_idle() {
    idle_blocking_threads(8).await;
    // The pool's own choice, for contrast: a job a `spawn_blocking` goes
    // to whichever idle thread its condvar wakes, on Linux the one idle
    // longest, so successive ones move between threads.
    let mut pooled = std::collections::HashSet::new();
    for _ in 0..16 {
        pooled.insert(tokio::task::spawn_blocking(|| std::thread::current().id()).await.unwrap());
    }
    assert!(pooled.len() > 1, "the pool rotates jobs over its idle threads");
    let kept = Kept::new();
    let mut ran_on = std::collections::HashSet::new();
    for _ in 0..16 {
        ran_on.insert(kept.run(|| std::thread::current().id()).await.unwrap());
    }
    assert_eq!(ran_on.len(), 1);
}

#[tokio::test(flavor = "current_thread")]
async fn a_panicking_job_is_an_error_and_the_next_runs_on_the_same_thread() {
    let kept = Kept::new();
    let before = kept.run(|| std::thread::current().id()).await.unwrap();
    let failed = kept.run(|| -> u8 { panic!("a job panics") }).await;
    assert!(matches!(failed, Err(Error::Join(_))));
    assert_eq!(kept.run(|| std::thread::current().id()).await.unwrap(), before);
}

/// A runtime of one blocking thread, so a job outside [`Kept`] runs only once
/// the kept thread has given it back.
fn one_blocking_thread() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .max_blocking_threads(1)
        .enable_time()
        .build()
        .unwrap()
}

/// Whether a `spawn_blocking` of the runtime's runs, within a bound no
/// thread given back approaches.
async fn a_blocking_thread_is_free() -> bool {
    let job = tokio::task::spawn_blocking(|| ());
    tokio::time::timeout(std::time::Duration::from_secs(10), job).await.is_ok()
}

#[test]
fn a_retired_or_dropped_thread_is_given_back_and_a_later_job_starts_one() {
    one_blocking_thread().block_on(async {
        let kept = Kept::new();
        assert_eq!(kept.run(|| 1).await.unwrap(), 1);
        kept.retire();
        assert!(a_blocking_thread_is_free().await, "the retired thread is given back");
        assert_eq!(kept.run(|| 2).await.unwrap(), 2);
        drop(kept);
        assert!(a_blocking_thread_is_free().await, "the dropped pass's thread is given back");
    });
}

/// No timeout gives the thread back: a job outside [`Kept`] queued behind it
/// waits out a gap longer than any wait the thread could have had, and the
/// next read runs where the last did.
#[test]
fn a_thread_stays_kept_between_reads_however_long_the_gap() {
    one_blocking_thread().block_on(async {
        let kept = Kept::new();
        let before = kept.run(|| std::thread::current().id()).await.unwrap();
        let outside = tokio::task::spawn_blocking(|| ());
        let gap = std::time::Duration::from_millis(500);
        assert!(
            tokio::time::timeout(gap, outside).await.is_err(),
            "the kept thread was given back"
        );
        assert_eq!(kept.run(|| std::thread::current().id()).await.unwrap(), before);
    });
}

/// Drop `runtime` on a thread of its own, and say whether that returned
/// within a bound no shutdown waiting on nothing approaches.
fn shuts_down_at_once(runtime: tokio::runtime::Runtime) -> bool {
    let (done, finished) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        drop(runtime);
        let _ = done.send(());
    });
    finished.recv_timeout(std::time::Duration::from_secs(10)).is_ok()
}

fn both_flavours() -> [tokio::runtime::Runtime; 2] {
    [
        tokio::runtime::Builder::new_current_thread().build().unwrap(),
        tokio::runtime::Builder::new_multi_thread().worker_threads(2).build().unwrap(),
    ]
}

#[test]
fn a_runtime_shuts_down_at_once_with_a_pass_left_unretired() {
    for runtime in both_flavours() {
        let kept = Kept::new();
        runtime.block_on(kept.run(|| ())).unwrap();
        assert!(shuts_down_at_once(runtime), "the shutdown waited on the kept thread");
        drop(kept);
    }
}

#[test]
fn a_pass_reads_on_across_two_runtimes() {
    let kept = Kept::new();
    for runtime in both_flavours() {
        assert_eq!(runtime.block_on(kept.run(|| 1)).unwrap(), 1);
        assert!(shuts_down_at_once(runtime), "the shutdown waited on the kept thread");
    }
    let last = tokio::runtime::Builder::new_current_thread().build().unwrap();
    assert_eq!(last.block_on(kept.run(|| 2)).unwrap(), 2);
    drop(kept);
}
