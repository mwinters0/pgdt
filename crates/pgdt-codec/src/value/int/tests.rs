use super::*;

/// **`oidin` is read as each major reads it** (I66): each spelling below
/// was cast on PostgreSQL 16, which reads it in `strtoul`'s base 0 under a
/// C23 glibc, and on 15, which reads it in base 10, beside what each
/// answered. Only a text both refuse is refused.
#[test]
fn an_oid_is_read_as_each_major_reads_it() {
    let cases: &[(&str, Option<u32>, Option<u32>)] = &[
        ("0", Some(0), Some(0)),
        ("010", Some(8), Some(10)),
        ("08", None, Some(8)),
        ("0x1F", Some(31), None),
        ("0X1f", Some(31), None),
        ("0x", None, None),
        ("0xg", None, None),
        ("0b101", Some(5), None),
        ("0b2", None, None),
        ("-1", Some(4294967295), Some(4294967295)),
        (" -1 ", Some(4294967295), Some(4294967295)),
        ("+5", Some(5), Some(5)),
        ("+", None, None),
        ("-", None, None),
        ("", None, None),
        ("  ", None, None),
        ("5 ", Some(5), Some(5)),
        (" 5", Some(5), Some(5)),
        ("5x", None, None),
        ("1e3", None, None),
        ("4294967295", Some(4294967295), Some(4294967295)),
        ("4294967296", None, None),
        ("18446744073709551615", Some(4294967295), Some(4294967295)),
        ("18446744073709551616", None, None),
        ("18446744073709551614", Some(4294967294), Some(4294967294)),
        ("18446744071562067968", Some(2147483648), Some(2147483648)),
        ("18446744071562067967", None, None),
        ("-2147483648", Some(2147483648), Some(2147483648)),
        ("-2147483649", None, None),
        ("-4294967295", None, None),
        ("-0", Some(0), Some(0)),
        ("0777", Some(511), Some(777)),
        ("0x100000000", None, None),
        ("0xFFFFFFFF", Some(4294967295), None),
        ("-0x80000000", Some(2147483648), None),
        ("-0x80000001", None, None),
        ("\t7\n", Some(7), Some(7)),
        ("\u{b}7", Some(7), Some(7)),
        ("5\u{c}", Some(5), Some(5)),
        ("00", Some(0), Some(0)),
        ("+0x10", Some(16), None),
        ("0x 1", None, None),
        ("\u{663}", None, None),
    ];
    for &(text, v16, v15) in cases {
        assert_eq!(oid_in(text.as_bytes(), true, true), v16, "{text:?} from v16");
        assert_eq!(oid_in(text.as_bytes(), false, false), v15, "{text:?} before v16");
        let refused = v16.is_none() && v15.is_none();
        assert_eq!(oid_unread(text) == Unread::Refused, refused, "{text:?}");
    }
    // Before C23, glibc reads no `0b`.
    assert_eq!(oid_in(b"0b101", true, false), None);
}
