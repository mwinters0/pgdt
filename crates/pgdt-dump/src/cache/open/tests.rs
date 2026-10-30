use std::path::Path;

use super::*;

/// **A gzip first pass spills where its cache is, or else where the OS keeps
/// temporary files**: the enabled cache's directory, the working directory
/// for a cache named with none, and the temp directory under a disabled
/// cache, which names no directory (`docs/design/decisions.md`, "D115").
#[test]
fn a_first_pass_spills_beside_its_cache_or_in_the_temp_directory() {
    let beside = CacheMode::enabled("/srv/dumps/koji.sql.gz.pgdt");
    assert_eq!(beside.spill_dir(), Path::new("/srv/dumps"));
    assert_eq!(CacheMode::enabled("koji.sql.gz.pgdt").spill_dir(), Path::new("."));
    assert_eq!(CacheMode::DISABLED.spill_dir(), std::env::temp_dir());
}
