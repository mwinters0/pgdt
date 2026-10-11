use super::*;

/// **`network_in` is read as the server reads it** (I67): each spelling
/// below was cast on PostgreSQL 16, beside the value it answered, whose
/// address and netmask the port must give.
#[test]
fn an_inet_or_cidr_is_read_as_network_in_reads_it() {
    let inet: &[(&str, Option<&str>)] = &[
        ("10.0.0.1", Some("10.0.0.1/32")),
        ("10", None),
        ("10.1", None),
        ("10.1.2", None),
        ("10.1.2/24", Some("10.1.2.0/24")),
        ("10.1.2.3/24", Some("10.1.2.3/24")),
        ("010.1.2.3", Some("10.1.2.3/32")),
        ("1.2.3.4.", Some("1.2.3.4/32")),
        ("1.2.3.4.5", None),
        ("1.2.3.4/", None),
        ("1.2.3.4/33", None),
        ("1.2.3.4/032", Some("1.2.3.4/32")),
        ("1.2.3.4/4294967304", Some("1.2.3.4/8")),
        ("1.2.3.4/4294967328", Some("1.2.3.4/32")),
        ("1..2.3", None),
        ("256.1.1.1", None),
        ("1.2.3.4 ", None),
        (" 1.2.3.4", None),
        ("::1", Some("::1/128")),
        ("::1/08", None),
        ("::1/128", Some("::1/128")),
        ("::1/129", None),
        ("::1/0", Some("::1/0")),
        ("::1/00", None),
        ("::", Some("::/128")),
        ("1::", Some("1::/128")),
        ("1:2:3:4:5:6:7:8", Some("1:2:3:4:5:6:7:8/128")),
        ("1:2:3:4:5:6:7::", Some("1:2:3:4:5:6:7:0/128")),
        ("::1:2:3:4:5:6:7", Some("0:1:2:3:4:5:6:7/128")),
        ("1:2:3:4:5:6:7:8:9", None),
        ("12345::", None),
        ("::ffff:1.2.3.4", Some("::ffff:1.2.3.4/128")),
        ("::1.2.3", Some("::1.2.3.0/128")),
        ("::1..2.3", Some("::1.0.2.3/128")),
        ("::01.2.3.4", None),
        ("::1.2.3.4/96", Some("::1.2.3.4/96")),
        ("::1.2.3.4/096", None),
        (":1::", None),
        ("1:::2", None),
        ("1::2::3", None),
        ("1:", None),
        ("fe80::1%eth0", None),
        ("::g", None),
        ("0x0a", None),
        ("10/8", Some("10.0.0.0/8")),
        ("10.0.0.0/8", Some("10.0.0.0/8")),
        ("10.0.0.1/8", Some("10.0.0.1/8")),
        ("1.2.3.4/-1", None),
        ("::1.2.3.4.5", None),
        ("1:2:3:4:5:6:1.2.3.4", Some("1:2:3:4:5:6:102:304/128")),
        ("1:2:3:4:5:6:7:1.2.3.4", None),
        ("1.2.3.4/8x", None),
        ("", None),
        ("::/0", Some("::/0")),
        ("::1/1281", None),
        ("::ABCD", Some("::abcd/128")),
    ];
    let cidr: &[(&str, Option<&str>)] = &[
        ("10", Some("10.0.0.0/8")),
        ("10.1", Some("10.1.0.0/16")),
        ("10.1.2", Some("10.1.2.0/24")),
        ("128", Some("128.0.0.0/16")),
        ("192", Some("192.0.0.0/24")),
        ("224", Some("224.0.0.0/4")),
        ("224.1", Some("224.1.0.0/16")),
        ("240", Some("240.0.0.0/32")),
        ("0x0a", Some("10.0.0.0/8")),
        ("0x0A0B", Some("10.11.0.0/16")),
        ("0x0a0b0c0d0e", None),
        ("0xa", Some("160.0.0.0/16")),
        ("10.0.0.0/8", Some("10.0.0.0/8")),
        ("10.0.0.1/8", None),
        ("10.0.0.0/7", Some("10.0.0.0/7")),
        ("10.1.2.0/24", Some("10.1.2.0/24")),
        ("10/8", Some("10.0.0.0/8")),
        ("1.2.3.4/32", Some("1.2.3.4/32")),
        ("1.2.3.4/33", None),
        ("::/0", Some("::/0")),
        ("::1/127", None),
        ("::/1", Some("::/1")),
        ("8000::/1", Some("8000::/1")),
        ("010", Some("10.0.0.0/8")),
        ("10.", None),
        ("1.2.3.4.5", None),
        ("1.2.3.4/4294967304", None),
        ("0x0a/8", Some("10.0.0.0/8")),
        ("0x", None),
        ("0xg", None),
        ("0.0.0.0/0", Some("0.0.0.0/0")),
        ("1.2.3.4/032", Some("1.2.3.4/32")),
        ("::1/08", None),
    ];
    for (cases, is_cidr) in [(inet, false), (cidr, true)] {
        for (text, server) in cases {
            let server = server.map(|value| {
                let (address, bits) = value.split_once('/').unwrap();
                let mut addr = [0u8; 16];
                let v6 = match address.parse::<std::net::IpAddr>().unwrap() {
                    std::net::IpAddr::V4(v4) => {
                        addr[..4].copy_from_slice(&v4.octets());
                        false
                    }
                    std::net::IpAddr::V6(v6) => {
                        addr = v6.octets();
                        true
                    }
                };
                (v6, bits.parse::<u8>().unwrap(), addr)
            });
            assert_eq!(network_in(text, is_cidr), server, "{text:?} as cidr: {is_cidr}");
        }
    }
}

/// **`macaddr_in` is refused where its `sscanf` layouts all refuse, and
/// `macaddr8_in` read as the server reads it** (I68): each spelling below
/// was cast on PostgreSQL 16, beside what it answered. A `macaddr` the
/// server reads is never refused, the ones the model leaves to glibc
/// among them — a sign, a `0x`, a run past eight digits — and nor is one
/// it refuses there.
#[test]
fn a_macaddr_is_refused_only_where_macaddr_in_refuses_it() {
    let macaddr: &[(&str, Option<&str>)] = &[
        ("08:00:2b:01:02:03", Some("08:00:2b:01:02:03")),
        ("08-00-2b-01-02-03", Some("08:00:2b:01:02:03")),
        ("08002b:010203", Some("08:00:2b:01:02:03")),
        ("08002b-010203", Some("08:00:2b:01:02:03")),
        ("0800.2b01.0203", Some("08:00:2b:01:02:03")),
        ("0800-2b01-0203", Some("08:00:2b:01:02:03")),
        ("08002b010203", Some("08:00:2b:01:02:03")),
        ("08:00:2b:01:02:3", Some("08:00:2b:01:02:03")),
        ("8:0:2b:1:2:3", Some("08:00:2b:01:02:03")),
        (" 08:00:2b:01:02:03 ", Some("08:00:2b:01:02:03")),
        ("08:00:2b:01:02:03x", None),
        ("08:00:2b:01:02", None),
        ("08:00:2b:01:02:03:04", None),
        ("08:00:2b:01:02:100", None),
        ("08:00:2b:01:02:0100", None),
        ("08:00:2b:01:02:ffffffff", None),
        ("08:00:2b:01:02:100000000ff", Some("08:00:2b:01:02:ff")),
        ("08:00:2b:01:02:-1", None),
        ("08:00:2b:01:02:+3", Some("08:00:2b:01:02:03")),
        ("08:00:2b:01:02:-0", Some("08:00:2b:01:02:00")),
        ("08:00:2b:01:02:0x3", Some("08:00:2b:01:02:03")),
        ("08:00:2b:01:02: 3", Some("08:00:2b:01:02:03")),
        ("08 :00:2b:01:02:03", None),
        ("zz:00:2b:01:02:03", None),
        ("08:00:2B:01:02:03", Some("08:00:2b:01:02:03")),
        ("08002b01020", Some("08:00:2b:01:02:00")),
        ("08002b0102030", None),
        ("08002b01020304", None),
        ("0x0800.2b01.0203", None),
        ("08:00:2b:01:02:03 x", None),
        ("", None),
        ("08:00-2b:01:02:03", None),
        ("0800.2b01.020", Some("08:00:2b:01:02:00")),
        ("08002b:01020", Some("08:00:2b:01:02:00")),
    ];
    // Refused by the server, through a conversion the model leaves to glibc.
    let unmodelled = ["08:00:2b:01:02:-1", "0x0800.2b01.0203"];
    for (text, server) in macaddr {
        let want = if server.is_some() || unmodelled.contains(text) {
            Unread::Unparsed
        } else {
            Unread::Refused
        };
        assert_eq!(macaddr_unread(text), want, "{text:?}");
    }
    let macaddr8: &[(&str, Option<&str>)] = &[
        ("08:00:2b:01:02:03:04:05", Some("08:00:2b:01:02:03:04:05")),
        ("08:00:2b:01:02:03", Some("08:00:2b:ff:fe:01:02:03")),
        ("08-00-2b-01-02-03-04-05", Some("08:00:2b:01:02:03:04:05")),
        ("08002b0102030405", Some("08:00:2b:01:02:03:04:05")),
        ("08002b010203", Some("08:00:2b:ff:fe:01:02:03")),
        ("0800.2b01.0203.0405", Some("08:00:2b:01:02:03:04:05")),
        ("08:00:2b:01:02:03:04", None),
        ("08:00:2b:01:02:03:04:05:06", None),
        ("08002b0102031", Some("08:00:2b:ff:fe:01:02:03")),
        ("08002b010203040", None),
        ("08002b01020304051", Some("08:00:2b:01:02:03:04:05")),
        (" 08:00:2b:01:02:03 ", Some("08:00:2b:ff:fe:01:02:03")),
        ("08:00:2b:01:02:03  ", Some("08:00:2b:ff:fe:01:02:03")),
        ("08:00:2b:01:02:03 x", None),
        ("08:00:2b:01:02:03:", Some("08:00:2b:ff:fe:01:02:03")),
        ("08:00:2b:01:02:03:04:05:", Some("08:00:2b:01:02:03:04:05")),
        ("08:00-2b:01:02:03", None),
        ("08::00:2b:01:02:03", None),
        ("8:00:2b:01:02:03", None),
        ("08:00:2b:01:02:03:04:05 ", Some("08:00:2b:01:02:03:04:05")),
        ("zz:00:2b:01:02:03", None),
        ("", None),
        ("08:00:2b:01:02:03\t", Some("08:00:2b:ff:fe:01:02:03")),
        ("08:00:2b:01:02:03:04 05", None),
        ("08:00:2b:01:02 03", None),
    ];
    for (text, server) in macaddr8 {
        let server = server.map(|value| {
            let mut bytes = [0u8; 8];
            for (byte, pair) in bytes.iter_mut().zip(value.split(':')) {
                *byte = u8::from_str_radix(pair, 16).unwrap();
            }
            bytes
        });
        assert_eq!(macaddr8_in(text), server, "{text:?}");
    }
}
