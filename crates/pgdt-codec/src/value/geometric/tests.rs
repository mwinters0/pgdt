use super::*;

/// **A geometric field is refused exactly where its input function
/// refuses it at every supported major** (I74): every case below was cast
/// to its type on PostgreSQL 13.23, 16.15 and 18.6, and is read here where
/// any of the three reads it. They disagree on two lines alone, built
/// from two points by arithmetic v14 changed: v13 reads the one whose
/// slope is `DBL_MAX` and refuses the one whose run is infinite.
#[test]
fn a_geometric_field_is_refused_only_where_every_major_refuses_it() {
    // `(input function, text, some major reads it)`.
    type Case = (fn(&str) -> bool, &'static str, bool);
    let cases: &[Case] = &[
        (point_in, "(1,2)", true),
        (point_in, "1,2", true),
        (point_in, " ( 1 , 2 ) ", true),
        (point_in, "(1,2", false),
        (point_in, "1,2)", false),
        (point_in, "(1 2)", false),
        (point_in, "(1,2,3)", false),
        (point_in, "", false),
        (point_in, "(,2)", false),
        (point_in, "(1e400,2)", false),
        (point_in, "(1e-400,0)", false),
        (point_in, "(0e-400,0)", true),
        (point_in, "(1e-310,0)", true),
        (point_in, "(NaN,inf)", true),
        (point_in, "(-Infinity,+inf)", true),
        (point_in, "(nan(abc),1)", true),
        (point_in, "(nan(),1)", true),
        (point_in, "(nan(a-b),1)", false),
        (point_in, "(-nan,1)", true),
        (point_in, "(INF,infinity)", true),
        (point_in, "(infinit,1)", false),
        (point_in, "(0x1p3,1)", true),
        (point_in, "(0x1.8p1,0X.8)", true),
        (point_in, "(0x,1)", false),
        (point_in, "(0x.p1,1)", false),
        (point_in, "(0x1p,0)", false),
        (point_in, "(0x1p+,0)", false),
        (point_in, "(0x1p99999,1)", false),
        (point_in, "(0x1p1024,0)", false),
        (point_in, "(0x1.fffffffffffff8p1023,0)", false),
        (point_in, "(0x1.fffffffffffff7p1023,0)", true),
        (point_in, "(0x1p-1074,1)", true),
        (point_in, "(-0x1p-1074,0)", true),
        (point_in, "(0x1p-1075,1)", false),
        (point_in, "(0x1.00000000000000000001p-1075,0)", true),
        (point_in, "(0x0p-99999,0)", true),
        (point_in, "(0x0.0000000000000000000000000000001p+99999,0)", false),
        (point_in, "(1.,.5)", true),
        (point_in, "(.,1)", false),
        (point_in, "(1e,2)", false),
        (point_in, "(1e+,2)", false),
        (point_in, "(1.e5,2E-3)", true),
        (point_in, "(1,2) x", false),
        (point_in, "((1,2))", false),
        (point_in, "(1_0,2)", false),
        (point_in, "(١,2)", false),
        (line_in, "{1,2,3}", true),
        (line_in, " {1,2,3} ", true),
        (line_in, "{0,0,1}", false),
        (line_in, "{0,1e-7,1}", false),
        (line_in, "{1e-6,0,1}", false),
        (line_in, "{2e-6,0,1}", true),
        (line_in, "{1,2,3", false),
        (line_in, "{1,2}", false),
        (line_in, "{1,2,3,4}", false),
        (line_in, "{1 ,2, 3 }", true),
        (line_in, "{NaN,0,1}", true),
        (line_in, "{1,1,1e400}", false),
        (line_in, "[(0,0),(1,1)]", true),
        (line_in, "(0,0),(1,1)", true),
        (line_in, "((0,0),(1,1))", true),
        (line_in, "0,0,1,1", true),
        (line_in, "[(0,0),(0,0)]", false),
        (line_in, "[(0,0),(0.0000001,0)]", false),
        (line_in, "[(1,1),(1,2)]", true),
        (line_in, "[(1,1),(2,1)]", true),
        (line_in, "[(2,0),(3,1.7976931348623157e308)]", true),
        (line_in, "[(Infinity,1),(0,2)]", true),
        (line_in, "[(1e300,1e300),(-1e300,-1e300)]", true),
        (line_in, "[(1e308,0),(-1e308,1)]", false),
        (line_in, "[(1e-200,0),(1e308,1)]", false),
        (line_in, "[(100000,0),(100000.001,1e305)]", false),
        (line_in, "[(NaN,0),(NaN,0)]", false),
        (line_in, "[(NaN,0),(1,1)]", true),
        (line_in, "[(1,2),(3,4)", false),
        (line_in, "[(1,2),(3,4),(5,6)]", false),
        (lseg_in, "[(1,2),(3,4)]", true),
        (lseg_in, "((1,2),(3,4))", true),
        (lseg_in, "(1,2),(3,4)", true),
        (lseg_in, "1,2,3,4", true),
        (lseg_in, "(1,2,3,4)", true),
        (lseg_in, "[(1,2),(3,4))", true),
        (lseg_in, "((1,2),(3,4)]", false),
        (lseg_in, "[(1,2),(3,4)", false),
        (lseg_in, "(1,2),(3,4),(5,6)", false),
        (lseg_in, "[(1,2)]", false),
        (lseg_in, "[(1,1),(1,1)]", true),
        (lseg_in, " [ ( 1 , 2 ) , ( 3 , 4 ) ] ", true),
        (box_in, "(1,2),(3,4)", true),
        (box_in, "((1,2),(3,4))", true),
        (box_in, "1,2,3,4", true),
        (box_in, "(1,2,3,4)", true),
        (box_in, "[(1,2),(3,4)]", false),
        (box_in, "(1,2)", false),
        (box_in, "((1,2),(3,4)", false),
        (box_in, "(nan,1),(2,3)", true),
        (box_in, " ( 1 , 2 ) , ( 3 , 4 ) ", true),
        (box_in, "(1,2) (3,4)", true),
        (box_in, "(1,2),(3,4),", true),
        (path_in, "[(1,2),(3,4)]", true),
        (path_in, "((1,2),(3,4))", true),
        (path_in, "(1,2),(3,4)", true),
        (path_in, "[(1,2)]", true),
        (path_in, "(1,2)", true),
        (path_in, "1,2", true),
        (path_in, "[1,2,3,4]", true),
        (path_in, "[(1,2),(3,4)", false),
        (path_in, "", false),
        (path_in, "(1,2),(3,4),", false),
        (path_in, "[(1,2),(3,4),(5,6)]", true),
        (path_in, "((1,2),(3,4)]", false),
        (path_in, "[(1,2),(3,4))", true),
        (path_in, "( (1,2),(3,4) )", true),
        (path_in, "[1,2,3]", false),
        (path_in, "((1,2),(3,4)) )", false),
        (path_in, "(1,2),(3,4) x", false),
        (poly_in, "((1,2),(3,4),(5,6))", true),
        (poly_in, "(1,2),(3,4)", true),
        (poly_in, "1,2,3,4,5,6", true),
        (poly_in, "[(1,2),(3,4)]", false),
        (poly_in, "(1,2)", true),
        (poly_in, "", false),
        (poly_in, "(1,2),(3,4", false),
        (poly_in, "((1,2),(3,4)", false),
        (poly_in, "((1,2),(3,4),(5,6)) ", true),
        (poly_in, "(1,2),(3,4),", false),
        (circle_in, "<(1,2),3>", true),
        (circle_in, "((1,2),3)", true),
        (circle_in, "(1,2),3", true),
        (circle_in, "1,2,3", true),
        (circle_in, "(1,2,3)", false),
        (circle_in, "<(1,2),-1>", false),
        (circle_in, "<(1,2),NaN>", true),
        (circle_in, "<(1,2),-0>", true),
        (circle_in, "<(1,2),3", false),
        (circle_in, "<(1,2),3>>", false),
        (circle_in, "<(1,2) 3>", true),
        (circle_in, "<(1,2),3)", true),
        (circle_in, "((1,2),3>", true),
        (circle_in, "<1,2,3>", true),
        (circle_in, "<(1,2),-inf>", false),
        (circle_in, "<(1,2),1e400>", false),
        (circle_in, "(1,2)3", true),
        (circle_in, " < ( 1 , 2 ) , 3 > ", true),
    ];
    for &(read, text, server) in cases {
        assert_eq!(read(text), server, "{text:?}");
    }
}

/// **A number in a geometric field is read as glibc's `strtod` reads
/// it**, a hexadecimal one rounded to nearest, ties to even, at each edge
/// of `double precision`'s range: the value is what decides a `line`'s
/// coefficients and a `circle`'s radius.
#[test]
fn a_geometric_number_is_read_as_strtod_reads_it() {
    let read = |text: &str| strtod(text.as_bytes(), 0);
    for (text, value, end) in [
        ("0x1p3", 8.0, 5),
        ("-0X.8", -0.5, 5),
        ("0x1.8p1x", 3.0, 7),
        ("0x", 0.0, 1),
        ("0x1p", 1.0, 3),
        ("0x1p-1074", f64::from_bits(1), 9),
        ("0x1.00000000000000000001p-1075", f64::from_bits(1), 30),
        ("0x1.8p-1074", f64::from_bits(2), 11),
        ("0x1.fffffffffffff7p1023", f64::MAX, 23),
        ("0x0.fffffffffffff8p-1022", f64::MIN_POSITIVE, 24),
        ("0x1.0000000000000800p0", 1.0, 22),
        ("0x1.0000000000000801p0", 1.0 + f64::EPSILON, 22),
        ("0x1.0000000000001800p0", 1.0 + 2.0 * f64::EPSILON, 22),
        ("1.e5,", 1e5, 4),
        ("1e+,", 1.0, 1),
        ("infinit", f64::INFINITY, 3),
        ("-Infinity)", f64::NEG_INFINITY, 9),
        ("1e-310", 1e-310, 6),
    ] {
        assert_eq!(read(text), Some((value, end, false)), "{text}");
    }
    for text in ["0x1p-1075", "0x1.fffffffffffff8p1023", "0x1p1024", "1e400", "-1e-400"] {
        assert!(read(text).is_some_and(|(_, _, out_of_range)| out_of_range), "{text}");
    }
    assert!(read("nan(a_1)x").is_some_and(|(v, end, _)| v.is_nan() && end == 8));
    assert!(read("nan(a-1)").is_some_and(|(v, end, _)| v.is_nan() && end == 3));
    for text in ["", ".", "+", "-.e1", "x1", "_1"] {
        assert_eq!(read(text), None, "{text:?}");
    }
    assert!(path_in(&format!("[{}1,1]", "1,1,".repeat(16))));
    assert!(fits_allocation(PATH_HEADER, (MAX_ALLOC_SIZE - PATH_HEADER) / 16));
    assert!(!fits_allocation(PATH_HEADER, (MAX_ALLOC_SIZE - PATH_HEADER) / 16 + 1));
    assert!(!fits_allocation(POLYGON_HEADER, (MAX_ALLOC_SIZE - POLYGON_HEADER) / 16 + 1));
}
