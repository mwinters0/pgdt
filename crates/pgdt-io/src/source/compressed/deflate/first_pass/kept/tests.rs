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
    // to the thread idle longest, so successive ones move between threads.
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

#[tokio::test(flavor = "current_thread")]
async fn a_thread_idle_past_its_wait_or_retired_is_given_back_and_a_later_job_starts_one() {
    let kept = Kept::new();
    assert_eq!(kept.run(|| 1).await.unwrap(), 1);
    std::thread::sleep(IDLE * 3);
    assert!(lock(&kept.queue).is_none(), "the idle thread cleared its queue");
    assert_eq!(kept.run(|| 2).await.unwrap(), 2);
    kept.retire();
    assert!(lock(&kept.queue).is_none());
    assert_eq!(kept.run(|| 3).await.unwrap(), 3);
}

#[test]
fn a_runtime_shuts_down_with_a_pass_left_unretired() {
    let runtime = tokio::runtime::Builder::new_current_thread().build().unwrap();
    let kept = runtime.block_on(async {
        let kept = Kept::new();
        kept.run(|| ()).await.unwrap();
        kept
    });
    let started = std::time::Instant::now();
    drop(runtime);
    assert!(started.elapsed() < IDLE * 20, "shutdown waited {:?}", started.elapsed());
    drop(kept);
}
