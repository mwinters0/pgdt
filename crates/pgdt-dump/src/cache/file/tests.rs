use super::*;

/// Every name in `dir`, sorted.
fn listing(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .collect();
    names.sort();
    names
}

/// A kill mid-save leaves the previous cache whole. No test races a
/// signal (`docs/design/decisions.md`, "D73"), so the save is stopped at
/// the moment a kill would land — partway through its encoding — and what
/// it leaves on disk is read there: the previous cache, untouched, and the
/// partial file beside it. A save that fails removes that file, and one
/// that finishes replaces the cache with nothing left beside it.
#[test]
fn a_save_leaves_the_previous_cache_whole_until_it_renames() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("dump.sql.dtcache");
    std::fs::write(&path, b"the previous cache").unwrap();

    let failed = write_beside(&path, |writer, _| {
        writer.write_all(b"the first half of the next")?;
        writer.flush()?;
        assert_eq!(std::fs::read(&path).unwrap(), b"the previous cache");
        let names = listing(dir.path());
        assert_eq!(names.len(), 2, "the cache and the save's own file: {names:?}");
        let partial = names.iter().find(|name| *name != "dump.sql.dtcache").unwrap();
        assert!(
            partial.starts_with("dump.sql.dtcache.") && partial.ends_with(".tmp"),
            "the partial file is named for its cache: {partial}"
        );
        assert_eq!(std::fs::read(dir.path().join(partial)).unwrap(), b"the first half of the next");
        Err(Error::CacheDisabled { operation: "the save stops here" })
    });
    assert!(matches!(failed, Err(Error::CacheDisabled { .. })));
    assert_eq!(std::fs::read(&path).unwrap(), b"the previous cache");
    assert_eq!(listing(dir.path()), ["dump.sql.dtcache"]);

    write_beside(&path, |writer, _| {
        writer.write_all(b"the next cache")?;
        assert_eq!(std::fs::read(&path).unwrap(), b"the previous cache");
        Ok(())
    })
    .unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), b"the next cache");
    assert_eq!(listing(dir.path()), ["dump.sql.dtcache"]);
}
