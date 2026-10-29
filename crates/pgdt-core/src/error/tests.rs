use std::error::Error as _;
use std::io::ErrorKind;

use super::*;

/// **An error carried in an `io::Error` comes back as itself**: an
/// [`Error::File`] keeps its path and its cause's kind across the trip, and an
/// [`Error::Io`] is the `io::Error` it held, so a read behind an `io::Result`
/// trait loses nothing a reader names.
#[test]
fn an_error_carried_in_an_io_error_comes_back_as_itself() {
    let cause = std::io::Error::from_raw_os_error(5);
    let carried = Error::file("read the dump at", "/data/dump.sql.xz", cause).into_io();
    assert_eq!(carried.kind(), std::io::Error::from_raw_os_error(5).kind());
    let message = carried.to_string();
    assert!(message.starts_with("cannot read the dump at /data/dump.sql.xz: "), "{message}");
    match Error::from(carried) {
        Error::File { doing, path, cause } => {
            assert_eq!(doing, "read the dump at");
            assert_eq!(path, Path::new("/data/dump.sql.xz"));
            assert_eq!(cause.raw_os_error(), Some(5));
        }
        other => panic!("not handed back as itself: {other:?}"),
    }

    let io = Error::Io(std::io::Error::new(ErrorKind::UnexpectedEof, "short")).into_io();
    assert_eq!(io.kind(), ErrorKind::UnexpectedEof);
    assert!(io.get_ref().is_some_and(|payload| !payload.is::<Error>()));
    assert!(matches!(Error::from(io), Error::Io(e) if e.kind() == ErrorKind::UnexpectedEof));
}

/// An `io::Error` carrying anything else stays [`Error::Io`], its payload
/// and its message untouched.
#[test]
fn an_io_error_carrying_another_payload_stays_io() {
    let error = Error::from(std::io::Error::other("not ours"));
    assert!(matches!(&error, Error::Io(e) if e.to_string() == "not ours"));
    assert!(error.source().is_none(), "the message is displayed, not chained");
}
