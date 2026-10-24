use std::future::Future;
use std::pin::Pin;
use std::sync::Mutex as StdMutex;
use std::time::SystemTime;

use crate::Extent;

use super::*;

/// A transport over `bytes` recording every range it is asked for.
struct Recording {
    bytes: Bytes,
    asked: StdMutex<Vec<(u64, usize)>>,
}

impl Recording {
    fn over(len: usize) -> Arc<Recording> {
        let bytes: Vec<u8> = (0..len).map(|i| (i % 251) as u8).collect();
        Arc::new(Recording { bytes: Bytes::from(bytes), asked: StdMutex::new(Vec::new()) })
    }

    fn asked(&self) -> Vec<(u64, usize)> {
        self.asked.lock().unwrap().clone()
    }
}

impl ByteRangeSource for Recording {
    fn read_range(
        &self,
        offset: u64,
        len: usize,
    ) -> Pin<Box<dyn Future<Output = Result<Bytes>> + Send + '_>> {
        self.asked.lock().unwrap().push((offset, len));
        let from = offset as usize;
        Box::pin(async move { Ok(self.bytes.slice(from..from + len)) })
    }

    fn extent(&self) -> Pin<Box<dyn Future<Output = Result<Extent>> + Send + '_>> {
        Box::pin(async move { Ok(Extent::Exact(self.bytes.len() as u64)) })
    }

    fn modified(&self) -> Pin<Box<dyn Future<Output = Result<Option<SystemTime>>> + Send + '_>> {
        Box::pin(async move { Ok(None) })
    }

    fn stored_size(&self) -> Pin<Box<dyn Future<Output = Result<u64>> + Send + '_>> {
        Box::pin(async move { Ok(self.bytes.len() as u64) })
    }
}

const PIECE: NonZeroU64 = NonZeroU64::new(100).unwrap();

/// **Each piece is fetched once, whoever reads it**: reads at any offset in
/// a piece, by tasks or the frontier, are slices of the one fetch, and the
/// object's last piece is its remainder, an empty read past it.
#[tokio::test]
async fn each_piece_is_fetched_once_whoever_reads_it() {
    let raw = Recording::over(450);
    let download = Download::new(Arc::clone(&raw) as Arc<dyn ByteRangeSource>, PIECE, 450);
    download.set_room(10);
    assert_eq!(&download.read(130).await.unwrap().unwrap()[..], &raw.bytes[130..200]);
    assert_eq!(&download.read(100).await.unwrap().unwrap()[..], &raw.bytes[100..200]);
    assert_eq!(&download.read_frontier(150).await.unwrap()[..], &raw.bytes[150..200]);
    assert_eq!(&download.read(420).await.unwrap().unwrap()[..], &raw.bytes[420..]);
    assert!(download.read(450).await.unwrap().unwrap().is_empty());
    assert_eq!(raw.asked(), [(100, 100), (400, 50)]);
}

/// **What is held is the room from the frontier's piece on**: a piece the
/// frontier has passed is dropped and refused a task, as is one past the
/// room, while the frontier reads behind it without holding what it fetched;
/// the last chunk a speculation may be handed out on is the one whose next
/// piece the room still holds.
#[tokio::test]
async fn what_is_held_is_the_room_from_the_frontier_s_piece_on() {
    let raw = Recording::over(1000);
    let download = Download::new(Arc::clone(&raw) as Arc<dyn ByteRangeSource>, PIECE, 1000);
    download.set_room(3);
    assert_eq!(download.last_chunk(), 1);
    for at in [0, 100, 200] {
        assert!(download.read(at).await.unwrap().is_some(), "{at}");
    }
    assert!(download.read(300).await.unwrap().is_none(), "past the room");
    download.release_before(250);
    assert_eq!((download.held(), download.last_chunk()), (1, 3));
    assert!(download.read(100).await.unwrap().is_none(), "behind the frontier");
    assert!(download.read(400).await.unwrap().is_some());
    assert_eq!(&download.read_frontier(120).await.unwrap()[..], &raw.bytes[120..200]);
    assert_eq!(download.held(), 2, "the frontier's read behind is not held");
    download.clear();
    assert!(download.read(400).await.unwrap().is_none(), "nothing once handed over");
    assert_eq!(raw.asked(), [(0, 100), (100, 100), (200, 100), (400, 100), (100, 100)]);
}
