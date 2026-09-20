//! The oracle: a deliberately misbehaving HTTP/1.1 origin server, in-process.
//!
//! **It is written rather than depended on, and that generalizes to nothing.**
//! Two of the knobs below cannot be emitted by anything that ships — an honest
//! short 206, whose `Content-Range` describes the smaller span it really sent,
//! and a declared `Content-Length` the body then contradicts, which a correct
//! server exists to prevent and which no byte-cutting proxy can produce, being
//! unable to rewrite the header it truncates under. Socket-level code is owed
//! either way, and once it is, control and treatment must be one
//! implementation: a knob-off oracle is a correct origin, where reading a
//! misbehaving case against *another* server's baseline credits the knob with
//! what may be the implementation. A mock-server dev-dependency was refused on
//! the same two knobs, and an off-the-shelf origin behind a proxy on all four.
//!
//! The failures a remote source has to get right are all *server* behaviours —
//! a server that ignores `Range`, one whose validator changes between the probe
//! and the read, one that answers fewer bytes than were asked for, one that
//! dies mid-body, one that stops answering mid-scan, one that accepts a
//! connection and then goes quiet. None of them is reachable
//! against a well-behaved server, which is why serving a fixture over a real
//! web server is the weakest instrument available and this is not that.
//!
//! **It speaks HTTP/1.1 over loopback on a port the kernel picks**, in one
//! thread, with no dependency beyond `std`. What it is not is a general web
//! server: it answers `GET` and `HEAD` for one object, never chunks, never
//! compresses, and closes every connection it answers.
//!
//! **Requests are served one at a time, in arrival order.** That is what makes
//! an ordinal-addressed knob — "fail from the third request onward" — mean
//! something: a test states the request number a misbehaviour starts at and
//! gets it, rather than a race. A knob written `…_after(n)` applies to every
//! request whose 1-based ordinal is greater than `n`, so `_after(0)` is "from
//! the first request".
//!
//! [`Oracle::requests`] is the other half of the instrument: what the client
//! actually sent, in order, so a test can assert that a probe cost one round
//! trip rather than two, or that a precondition rode on every ranged GET.

use std::collections::VecDeque;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// How long a connection may sit idle before the oracle abandons it. A test
/// that wedges fails on its own timeout rather than hanging the suite.
const CONNECTION_TIMEOUT: Duration = Duration::from_secs(30);

/// How often a stalling connection looks to see whether the oracle has been
/// dropped. Short enough that a test's teardown is not the thing it waits on.
const STALL_POLL: Duration = Duration::from_millis(10);

/// The largest request head the oracle will read. Nothing legitimate comes
/// close; the cap is what stops a malformed client from growing a buffer
/// without bound.
const MAX_REQUEST_HEAD_BYTES: usize = 64 * 1024;

/// The modification time the oracle serves before any `etag_changing_after`
/// knob fires — an arbitrary fixed instant, so a snapshot of a response is
/// reproducible. 2026-07-23T00:00:00Z, the koji sample's date.
const FIRST_MODIFIED: i64 = 1_784_764_800;

/// The modification time served after it fires: one hour later, so the "the
/// object changed" case is a *newer* object rather than merely a different
/// string.
const SECOND_MODIFIED: i64 = FIRST_MODIFIED + 3600;

/// What the client sent, as the oracle read it. Header names are lowercased;
/// values keep their spelling.
#[derive(Clone, Debug)]
pub struct Request {
    pub method: String,
    pub path: String,
    pub headers: Vec<(String, String)>,
}

impl Request {
    /// A header by name, lowercased, or `None` where the client sent none.
    pub fn header(&self, name: &str) -> Option<&str> {
        let name = name.to_ascii_lowercase();
        self.headers.iter().find(|(k, _)| *k == name).map(|(_, v)| v.as_str())
    }

    /// The `Range` header, for the common assertion that a read was ranged at
    /// all.
    pub fn range(&self) -> Option<&str> {
        self.header("range")
    }
}

/// One misbehaviour apiece. Every field is off by default, so an [`Oracle`]
/// built and started with no knob set is a correct, boring origin server —
/// which is itself worth having, as the control the misbehaving cases are read
/// against.
#[derive(Clone, Copy, Default)]
struct Knobs {
    ignore_range: bool,
    suppress_etag: bool,
    suppress_last_modified: bool,
    etag_changes_after: Option<usize>,
    not_found_after: Option<usize>,
    short_range_after: Option<(usize, usize)>,
    truncating_body_after: Option<(usize, usize)>,
    stalling_request: Option<(usize, Duration)>,
}

impl Knobs {
    /// Whether an ordinal-addressed knob is live for this request. Ordinals
    /// are 1-based and the comparison is strict, so `_after(0)` fires on the
    /// first request and `_after(2)` on the third.
    fn fires(after: Option<usize>, ordinal: usize) -> bool {
        after.is_some_and(|n| ordinal > n)
    }
}

struct State {
    body: Vec<u8>,
    etag: String,
    changed_etag: String,
    knobs: Knobs,
    served: AtomicUsize,
    log: Mutex<Vec<Request>>,
    stop: AtomicBool,
}

impl State {
    /// The validator and modification time this request sees — the second pair
    /// once `etag_changing_after` has fired, the first otherwise.
    fn version(&self, ordinal: usize) -> (&str, i64) {
        if Knobs::fires(self.knobs.etag_changes_after, ordinal) {
            (&self.changed_etag, SECOND_MODIFIED)
        } else {
            (&self.etag, FIRST_MODIFIED)
        }
    }
}

/// A running oracle. Dropping it stops the server thread and frees the port.
pub struct Oracle {
    addr: SocketAddr,
    state: Arc<State>,
    thread: Option<JoinHandle<()>>,
}

/// An oracle being configured. Every method sets one knob; [`OracleBuilder::start`]
/// binds the port and spawns the thread.
pub struct OracleBuilder {
    body: Vec<u8>,
    knobs: Knobs,
}

impl Oracle {
    /// An oracle serving `body`.
    pub fn serving(body: impl Into<Vec<u8>>) -> OracleBuilder {
        OracleBuilder { body: body.into(), knobs: Knobs::default() }
    }

    /// An oracle serving a file's bytes — a fixture, read once at build time,
    /// so the server holds no file handle and the test tree is never served
    /// from disk under a request.
    pub fn serving_file(path: &std::path::Path) -> OracleBuilder {
        let body = std::fs::read(path)
            .unwrap_or_else(|e| panic!("the oracle cannot read {}: {e}", path.display()));
        Oracle::serving(body)
    }

    /// The URL of the object, which is what a `--source` argument gets.
    pub fn url(&self) -> String {
        format!("http://{}/dump.sql", self.addr)
    }

    /// The URL of some other path on the same server. Everything is served
    /// from every path; this exists so a test can name a second object without
    /// a second server.
    pub fn url_for(&self, path: &str) -> String {
        format!("http://{}/{}", self.addr, path.trim_start_matches('/'))
    }

    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// The bytes being served.
    pub fn body(&self) -> &[u8] {
        &self.state.body
    }

    /// The validator served before any `etag_changing_after` knob fires,
    /// quoted as it goes on the wire.
    pub fn etag(&self) -> &str {
        &self.state.etag
    }

    /// The validator served after it fires.
    pub fn changed_etag(&self) -> &str {
        &self.state.changed_etag
    }

    /// Every request the oracle has answered, in arrival order.
    pub fn requests(&self) -> Vec<Request> {
        self.state.log.lock().unwrap().clone()
    }

    /// How many requests have been answered.
    pub fn request_count(&self) -> usize {
        self.state.log.lock().unwrap().len()
    }
}

impl OracleBuilder {
    /// Answer `200` with the whole body where a `Range` was asked for — the
    /// range-ignorant server `object_store` refuses on the status line.
    pub fn ignoring_range(mut self) -> Self {
        self.knobs.ignore_range = true;
        self
    }

    /// Send no `ETag`. The weak identity is then half-present, which is what
    /// `docs/design/decisions.md`, "D21" reads as absence.
    pub fn without_etag(mut self) -> Self {
        self.knobs.suppress_etag = true;
        self
    }

    /// Send no `Last-Modified`.
    pub fn without_last_modified(mut self) -> Self {
        self.knobs.suppress_last_modified = true;
        self
    }

    /// Serve a different validator and a newer modification time from request
    /// `after + 1` onward — the object rewritten between the probe and the
    /// read. A precondition carrying the first validator is then refused
    /// `412`, which is how a remote source detects it.
    pub fn etag_changing_after(mut self, after: usize) -> Self {
        self.knobs.etag_changes_after = Some(after);
        self
    }

    /// Answer `404` from request `after + 1` onward — the object withdrawn
    /// mid-scan.
    pub fn not_found_after(mut self, after: usize) -> Self {
        self.knobs.not_found_after = Some(after);
        self
    }

    /// From request `after + 1` onward, answer a ranged GET with at most
    /// `max_bytes`, **declared honestly** in `Content-Range` and
    /// `Content-Length`. This is a legal response the caller must notice: it
    /// asked for a span and got a prefix of it. One byte is the floor — an
    /// empty range has no `Content-Range` spelling.
    pub fn short_range_after(mut self, after: usize, max_bytes: usize) -> Self {
        self.knobs.short_range_after = Some((after, max_bytes));
        self
    }

    /// From request `after + 1` onward, declare the full length and then write
    /// only `sent_bytes` before closing the connection — a transport failure
    /// partway through a body, rather than a short answer.
    pub fn truncating_body_after(mut self, after: usize, sent_bytes: usize) -> Self {
        self.knobs.truncating_body_after = Some((after, sent_bytes));
        self
    }

    /// On request `ordinal` — 1-based, and that request alone — read the
    /// request, log it, and then answer **nothing** for `stall`: the origin
    /// that accepts a connection and goes quiet.
    ///
    /// It is the one misbehaviour a truncated body cannot stand in for. A
    /// liveness deadline and a cancellation are both timed against a *wait*,
    /// and a failure that arrives promptly ends the wait before either can be
    /// observed.
    ///
    /// **Addressed to one request rather than to a suffix, unlike every other
    /// knob here.** What a stall is used to observe is what the client does
    /// *next* — abandon the request and ask again — and a suffix knob stalls
    /// that attempt too, so the recovery it exists to show could never happen.
    ///
    /// **The stall ends early when the client hangs up**, which is what a real
    /// server does and what this one must do: connections are served one at a
    /// time, so a stall that outlived the client would hold the retry it is
    /// waiting for. It also ends when the oracle is dropped.
    pub fn stalling_request(mut self, ordinal: usize, stall: Duration) -> Self {
        self.knobs.stalling_request = Some((ordinal, stall));
        self
    }

    /// Bind a loopback port the kernel picks and start serving.
    pub fn start(self) -> Oracle {
        let listener =
            TcpListener::bind(("127.0.0.1", 0)).expect("the oracle binds a loopback port");
        let addr = listener.local_addr().expect("a bound listener has an address");
        let etag = format!("\"{:016x}\"", fnv1a(&self.body));
        let changed_etag = format!("\"{:016x}\"", fnv1a(&self.body).wrapping_add(1));
        let state = Arc::new(State {
            body: self.body,
            etag,
            changed_etag,
            knobs: self.knobs,
            served: AtomicUsize::new(0),
            log: Mutex::new(Vec::new()),
            stop: AtomicBool::new(false),
        });
        let thread = {
            let state = Arc::clone(&state);
            std::thread::spawn(move || serve(&listener, &state))
        };
        Oracle { addr, state, thread: Some(thread) }
    }
}

impl Drop for Oracle {
    fn drop(&mut self) {
        self.state.stop.store(true, Ordering::SeqCst);
        // The accept loop is blocked in `accept`; one connection wakes it, and
        // it checks `stop` before reading anything.
        let _ = TcpStream::connect(self.addr);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn serve(listener: &TcpListener, state: &Arc<State>) {
    for stream in listener.incoming() {
        if state.stop.load(Ordering::SeqCst) {
            break;
        }
        let Ok(mut stream) = stream else { break };
        let _ = stream.set_read_timeout(Some(CONNECTION_TIMEOUT));
        let _ = stream.set_write_timeout(Some(CONNECTION_TIMEOUT));
        answer(&mut stream, state);
    }
}

/// Read one request, answer it, and close. Every response says
/// `Connection: close`, so a client never reuses a connection and the ordinal
/// a knob is addressed by is the request number rather than the connection's.
fn answer(stream: &mut TcpStream, state: &Arc<State>) {
    let Some(request) = read_request(stream) else { return };
    state.log.lock().unwrap().push(request.clone());
    let ordinal = state.served.fetch_add(1, Ordering::SeqCst) + 1;
    if let Some((stalled, stall)) = state.knobs.stalling_request
        && stalled == ordinal
        && !stall_until_client_leaves(stream, stall, state)
    {
        // The client hung up while it was being ignored, so there is nobody
        // left to answer. The request is in the log all the same — it was
        // logged before the silence, which is what lets a test see it arrive
        // while the client is still waiting on it.
        return;
    }
    let response = respond(&request, ordinal, state);
    let _ = stream.write_all(&response.head);
    if request.method != "HEAD" {
        let _ = stream.write_all(&response.body);
    }
    let _ = stream.flush();
}

/// Answer nothing for `stall`, waking every [`STALL_POLL`] to see whether the
/// client is still there and whether the oracle is being dropped.
///
/// `false` where the client closed the connection first — an abandoned
/// request, which is what a deadline and a cancellation both produce.
fn stall_until_client_leaves(stream: &mut TcpStream, stall: Duration, state: &Arc<State>) -> bool {
    let until = Instant::now() + stall;
    let _ = stream.set_read_timeout(Some(STALL_POLL));
    let mut byte = [0u8; 1];
    while Instant::now() < until && !state.stop.load(Ordering::SeqCst) {
        match stream.read(&mut byte) {
            // End of stream: the client is gone.
            Ok(0) => return false,
            // A well-behaved client sends nothing more; anything that arrives
            // is not this server's business.
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::ConnectionReset => return false,
            // `WouldBlock` and `TimedOut` are both what a read timeout raises,
            // and they are the ordinary case: the client is still waiting.
            Err(_) => {}
        }
    }
    let _ = stream.set_read_timeout(Some(CONNECTION_TIMEOUT));
    true
}

/// A built response: the status line and headers, then whatever body is to go
/// on the wire — which for a truncated answer is deliberately shorter than the
/// `Content-Length` the head declares.
struct Response {
    head: Vec<u8>,
    body: Vec<u8>,
}

fn respond(request: &Request, ordinal: usize, state: &Arc<State>) -> Response {
    let knobs = state.knobs;
    if Knobs::fires(knobs.not_found_after, ordinal) {
        return simple(404, "Not Found", b"no such object\n".to_vec(), None);
    }
    if request.method != "GET" && request.method != "HEAD" {
        return simple(405, "Method Not Allowed", b"GET and HEAD only\n".to_vec(), None);
    }

    let (etag, modified) = state.version(ordinal);
    let etag = (!knobs.suppress_etag).then_some(etag);
    let modified = (!knobs.suppress_last_modified).then_some(modified);
    let validators = Validators { etag, modified };

    if let Some(refusal) = precondition(request, validators) {
        return refusal;
    }

    let total = state.body.len();
    let requested =
        if knobs.ignore_range { None } else { request.range().map(|r| parse_range(r, total)) };
    let (status, reason, start, mut end) = match requested {
        // A `Range` the oracle cannot make sense of is a 400 rather than a
        // silently-ignored header: an oracle that swallows a malformed request
        // hides the bug it exists to surface.
        Some(None) => {
            return simple(400, "Bad Request", b"unparseable Range\n".to_vec(), Some(validators));
        }
        Some(Some((start, _))) if start >= total => {
            let mut response = simple(416, "Range Not Satisfiable", Vec::new(), Some(validators));
            insert_header(&mut response.head, &format!("Content-Range: bytes */{total}"));
            return response;
        }
        Some(Some((start, end))) => (206, "Partial Content", start, end.min(total - 1)),
        None => (200, "OK", 0, total.saturating_sub(1)),
    };

    if let Some((after, max_bytes)) = knobs.short_range_after
        && status == 206
        && Knobs::fires(Some(after), ordinal)
    {
        end = end.min(start + max_bytes.saturating_sub(1));
    }

    let slice: Vec<u8> = if total == 0 { Vec::new() } else { state.body[start..=end].to_vec() };
    let declared = slice.len();
    let mut response = simple(status, reason, slice, Some(validators));
    if status == 206 {
        insert_header(&mut response.head, &format!("Content-Range: bytes {start}-{end}/{total}"));
    }

    if let Some((after, sent)) = knobs.truncating_body_after
        && Knobs::fires(Some(after), ordinal)
    {
        // `Content-Length` keeps the honest figure; the body does not. The
        // client sees the connection close with `declared - sent` bytes owing.
        response.body.truncate(sent.min(declared));
    }
    response
}

#[derive(Clone, Copy)]
struct Validators<'a> {
    etag: Option<&'a str>,
    modified: Option<i64>,
}

/// RFC 9110's precedence: `If-Match`, then `If-Unmodified-Since`, then
/// `If-None-Match`, then `If-Modified-Since`. A conditional header naming a
/// validator the object does not have fails, which is the case
/// `docs/design/decisions.md`, "D21" pins a ranged GET with.
fn precondition(request: &Request, validators: Validators) -> Option<Response> {
    let refused = || Some(simple(412, "Precondition Failed", Vec::new(), Some(validators)));
    let fresh = || Some(simple(304, "Not Modified", Vec::new(), Some(validators)));

    if let Some(given) = request.header("if-match")
        && !matches(given, validators.etag)
    {
        return refused();
    }
    if let Some(given) = request.header("if-unmodified-since") {
        let Some(given) = parse_http_date(given) else {
            return Some(simple(400, "Bad Request", b"unparseable date\n".to_vec(), None));
        };
        match validators.modified {
            Some(modified) if modified > given => return refused(),
            None => return refused(),
            _ => {}
        }
    }
    if let Some(given) = request.header("if-none-match")
        && matches(given, validators.etag)
    {
        return fresh();
    }
    if let Some(given) = request.header("if-modified-since") {
        let Some(given) = parse_http_date(given) else {
            return Some(simple(400, "Bad Request", b"unparseable date\n".to_vec(), None));
        };
        if validators.modified.is_some_and(|modified| modified <= given) {
            return fresh();
        }
    }
    None
}

/// Whether a conditional header's value selects the object's validator. `*`
/// matches any existing validator and nothing where there is none.
fn matches(given: &str, etag: Option<&str>) -> bool {
    let Some(etag) = etag else { return false };
    if given.trim() == "*" {
        return true;
    }
    given.split(',').any(|candidate| candidate.trim().trim_start_matches("W/") == etag)
}

/// Assemble a response. **`Content-Length` is always present and nothing is
/// ever chunked** — `object_store` requires the header unconditionally, so an
/// oracle that omitted it would be testing a case the crate refuses before it
/// reaches any of ours.
fn simple(status: u16, reason: &str, body: Vec<u8>, validators: Option<Validators>) -> Response {
    let mut head = format!("HTTP/1.1 {status} {reason}\r\n");
    head.push_str(&format!("Content-Length: {}\r\n", body.len()));
    head.push_str("Accept-Ranges: bytes\r\n");
    head.push_str("Content-Type: application/octet-stream\r\n");
    if let Some(validators) = validators {
        if let Some(etag) = validators.etag {
            head.push_str(&format!("ETag: {etag}\r\n"));
        }
        if let Some(modified) = validators.modified {
            head.push_str(&format!("Last-Modified: {}\r\n", http_date(modified)));
        }
    }
    head.push_str("Connection: close\r\n\r\n");
    Response { head: head.into_bytes(), body }
}

/// Put one more header line in front of the blank line that ends the head.
fn insert_header(head: &mut Vec<u8>, line: &str) {
    let end = head.len() - 2;
    head.splice(end..end, format!("{line}\r\n").into_bytes());
}

/// Read a request head, stopping at the blank line. `None` where the client
/// closed first, sent nothing, or sent something that is not a request line —
/// all of which are answered by hanging up, since there is no request to
/// answer and no ordinal to spend on it.
fn read_request(stream: &mut TcpStream) -> Option<Request> {
    let mut buffer = Vec::new();
    let mut byte = [0u8; 1];
    while !buffer.ends_with(b"\r\n\r\n") {
        if buffer.len() >= MAX_REQUEST_HEAD_BYTES {
            return None;
        }
        match stream.read(&mut byte) {
            Ok(0) | Err(_) => return None,
            Ok(_) => buffer.push(byte[0]),
        }
    }
    let text = String::from_utf8(buffer).ok()?;
    let mut lines = text.lines();
    let mut start = lines.next()?.split_whitespace();
    let method = start.next()?.to_string();
    let path = start.next()?.to_string();
    let headers = lines
        .filter(|line| !line.is_empty())
        .filter_map(|line| line.split_once(':'))
        .map(|(name, value)| (name.trim().to_ascii_lowercase(), value.trim().to_string()))
        .collect();
    Some(Request { method, path, headers })
}

/// `bytes=a-b`, `bytes=a-` and `bytes=-n`, as an inclusive pair against a body
/// of `total` bytes — the suffix form counts back from the end, so it needs the
/// length. `None` where the header is not one of those — a multi-range request
/// included, which this server does not answer and must not pretend to.
fn parse_range(header: &str, total: usize) -> Option<(usize, usize)> {
    let spec = header.trim().strip_prefix("bytes=")?;
    if spec.contains(',') {
        return None;
    }
    let (first, last) = spec.split_once('-')?;
    match (first.trim(), last.trim()) {
        ("", suffix) => {
            let suffix: usize = suffix.parse().ok()?;
            Some((total.saturating_sub(suffix), total.saturating_sub(1)))
        }
        (start, "") => {
            let start: usize = start.parse().ok()?;
            Some((start, usize::MAX))
        }
        (start, end) => Some((start.parse().ok()?, end.parse().ok()?)),
    }
}

/// FNV-1a over the body, so the validator is a function of the bytes rather
/// than a constant every oracle in the suite shares.
fn fnv1a(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

// ---------------------------------------------------------------------------
// HTTP dates
//
// IMF-fixdate (`Sun, 06 Nov 1994 08:49:37 GMT`) in both directions, over the
// civil-calendar conversions rather than a date crate: the oracle's whole
// point is that it has no dependencies, and two dozen lines of arithmetic is
// cheaper than a dependency the shipped binary would then carry in its
// lockfile.
// ---------------------------------------------------------------------------

const WEEKDAYS: [&str; 7] = ["Thu", "Fri", "Sat", "Sun", "Mon", "Tue", "Wed"];
const MONTHS: [&str; 12] =
    ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];

/// Unix seconds as an IMF-fixdate.
pub fn http_date(unix: i64) -> String {
    let days = unix.div_euclid(86_400);
    let seconds = unix.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    // 1970-01-01 was a Thursday, which is why `WEEKDAYS` starts there.
    let weekday = WEEKDAYS[days.rem_euclid(7) as usize];
    let month_name = MONTHS[(month - 1) as usize];
    let (hour, minute, second) = (seconds / 3600, (seconds % 3600) / 60, seconds % 60);
    format!("{weekday}, {day:02} {month_name} {year:04} {hour:02}:{minute:02}:{second:02} GMT")
}

/// An IMF-fixdate back to unix seconds. `None` for anything else, including
/// the two obsolete formats: nothing in this project's client stack emits
/// them, and accepting a format the oracle cannot round-trip would let a wrong
/// date pass as a right one.
pub fn parse_http_date(text: &str) -> Option<i64> {
    let text = text.trim();
    let (_, rest) = text.split_once(", ")?;
    let mut fields = rest.split_whitespace();
    let day: u32 = fields.next()?.parse().ok()?;
    let month_name = fields.next()?;
    let month = MONTHS.iter().position(|m| *m == month_name)? as u32 + 1;
    let year: i64 = fields.next()?.parse().ok()?;
    let mut clock = fields.next()?.split(':');
    let hour: i64 = clock.next()?.parse().ok()?;
    let minute: i64 = clock.next()?.parse().ok()?;
    let second: i64 = clock.next()?.parse().ok()?;
    if fields.next()? != "GMT" {
        return None;
    }
    Some(days_from_civil(year, month, day) * 86_400 + hour * 3600 + minute * 60 + second)
}

/// Days since the Unix epoch for a proleptic-Gregorian date (Hinnant).
fn days_from_civil(year: i64, month: u32, day: u32) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let year_of_era = year - era * 400;
    let month = i64::from(month);
    let day_of_year = (153 * (month + if month > 2 { -3 } else { 9 }) + 2) / 5 + i64::from(day) - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

/// The inverse.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let days = days + 719_468;
    let era = if days >= 0 { days } else { days - 146_096 } / 146_097;
    let day_of_era = days - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_position = (5 * day_of_year + 2) / 153;
    let day = (day_of_year - (153 * month_position + 2) / 5 + 1) as u32;
    let month = (if month_position < 10 { month_position + 3 } else { month_position - 9 }) as u32;
    (if month <= 2 { year + 1 } else { year }, month, day)
}

// ---------------------------------------------------------------------------
// A client that is not the subject
//
// The oracle's own tests need a client, and using the one the oracle exists to
// test would make the proof circular -- an instrument checked with the thing it
// judges agrees with it by construction. So this reads bytes off a socket and
// parses only what an
// assertion needs.
// ---------------------------------------------------------------------------

/// A response as it arrived on the wire.
#[derive(Debug)]
pub struct RawResponse {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    /// What actually arrived, which for a truncated answer is shorter than
    /// [`RawResponse::content_length`].
    pub body: Vec<u8>,
}

impl RawResponse {
    pub fn header(&self, name: &str) -> Option<&str> {
        let name = name.to_ascii_lowercase();
        self.headers.iter().find(|(k, _)| *k == name).map(|(_, v)| v.as_str())
    }

    /// The declared length, which is the oracle's claim rather than an
    /// observation.
    pub fn content_length(&self) -> Option<usize> {
        self.header("content-length")?.parse().ok()
    }

    /// Whether the body that arrived is the length the head declared. `false`
    /// is the transport truncation `truncating_body_after` produces.
    pub fn complete(&self) -> bool {
        self.content_length() == Some(self.body.len())
    }
}

/// Send one request and read until the server closes. `extra` holds whole
/// header lines without their terminators (`"Range: bytes=0-15"`).
pub fn raw_request(oracle: &Oracle, method: &str, path: &str, extra: &[&str]) -> RawResponse {
    let mut stream = TcpStream::connect(oracle.addr()).expect("the oracle accepts a connection");
    stream.set_read_timeout(Some(CONNECTION_TIMEOUT)).unwrap();
    let mut request = format!("{method} {path} HTTP/1.1\r\nHost: {}\r\n", oracle.addr());
    for line in extra {
        request.push_str(line);
        request.push_str("\r\n");
    }
    request.push_str("Connection: close\r\n\r\n");
    stream.write_all(request.as_bytes()).expect("the oracle reads the request");
    let mut raw = Vec::new();
    stream.read_to_end(&mut raw).expect("the oracle answers");

    let split = raw.windows(4).position(|w| w == b"\r\n\r\n").expect("a response has a head");
    let head = String::from_utf8(raw[..split].to_vec()).expect("the head is ASCII");
    let body = raw[split + 4..].to_vec();
    let mut lines: VecDeque<&str> = head.lines().collect();
    let status = lines
        .pop_front()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|code| code.parse().ok())
        .expect("a response has a status");
    let headers = lines
        .into_iter()
        .filter_map(|line| line.split_once(':'))
        .map(|(name, value)| (name.trim().to_ascii_lowercase(), value.trim().to_string()))
        .collect();
    RawResponse { status, headers, body }
}

/// A `GET` of the oracle's object.
pub fn raw_get(oracle: &Oracle, extra: &[&str]) -> RawResponse {
    raw_request(oracle, "GET", "/dump.sql", extra)
}

/// A `HEAD` of the oracle's object.
pub fn raw_head(oracle: &Oracle, extra: &[&str]) -> RawResponse {
    raw_request(oracle, "HEAD", "/dump.sql", extra)
}
