//! Whether `json_in` reads a text. Not `decode`'s API; a strict parse and a
//! filter's literal call it.

/// Whether `json_in` reads `text`, which it stores as written: one JSON value
/// in RFC 8259's grammar, with blanks of `' '`, `\t`, `\n` and `\r` around
/// any token. A string holds no byte below `0x20` and no escape but the eight
/// single-character ones and `\u` with four hex digits; **what a `\u` escape
/// names is not checked**, so `\u0000` and a lone surrogate half, which
/// `jsonb_in` refuses, are read, as is a number past `numeric`'s range.
///
/// Iterative, over a stack of the containers open, so no depth refuses: the
/// server's recursion is bounded by its `max_stack_depth`, a setting of the
/// server restoring the dump (I72).
// pg-refuses: I72 — every refusal here is `json_in`'s.
pub(crate) fn json_in(text: &str) -> bool {
    let b = text.as_bytes();
    let mut at = 0;
    // The containers open around the value being read, innermost last:
    // `true` for an object.
    let mut open: Vec<bool> = Vec::new();
    loop {
        // A value is expected at `at`.
        json_blanks(b, &mut at);
        let ok = match b.get(at) {
            Some(b'{') => {
                at += 1;
                json_blanks(b, &mut at);
                if b.get(at) == Some(&b'}') {
                    at += 1;
                    true
                } else {
                    if !json_member_key(b, &mut at) {
                        return false;
                    }
                    open.push(true);
                    continue;
                }
            }
            Some(b'[') => {
                at += 1;
                json_blanks(b, &mut at);
                if b.get(at) == Some(&b']') {
                    at += 1;
                    true
                } else {
                    open.push(false);
                    continue;
                }
            }
            Some(b'"') => json_string(b, &mut at),
            Some(b'-' | b'0'..=b'9') => json_number(b, &mut at),
            _ => ["true", "false", "null"].iter().any(|word| {
                let found = b[at..].starts_with(word.as_bytes());
                at += if found { word.len() } else { 0 };
                found
            }),
        };
        if !ok {
            return false;
        }
        // A value has been read: close what it ends, or go on to the next.
        loop {
            json_blanks(b, &mut at);
            let Some(&object) = open.last() else { return at == b.len() };
            match b.get(at) {
                Some(b',') => {
                    at += 1;
                    if object && !json_member_key(b, &mut at) {
                        return false;
                    }
                    break;
                }
                Some(b'}') if object => {
                    at += 1;
                    open.pop();
                }
                Some(b']') if !object => {
                    at += 1;
                    open.pop();
                }
                _ => return false,
            }
        }
    }
}

/// `json_lex`'s blanks.
fn json_blanks(b: &[u8], at: &mut usize) {
    while matches!(b.get(*at), Some(b' ' | b'\t' | b'\n' | b'\r')) {
        *at += 1;
    }
}

/// An object member's key and its `:`, blanks around each.
fn json_member_key(b: &[u8], at: &mut usize) -> bool {
    json_blanks(b, at);
    if !(b.get(*at) == Some(&b'"') && json_string(b, at)) {
        return false;
    }
    json_blanks(b, at);
    let colon = b.get(*at) == Some(&b':');
    *at += usize::from(colon);
    colon
}

/// `json_lex_string` without de-escaping, its opening `"` at `at`.
fn json_string(b: &[u8], at: &mut usize) -> bool {
    *at += 1;
    loop {
        match b.get(*at) {
            Some(b'"') => {
                *at += 1;
                return true;
            }
            Some(b'\\') => match b.get(*at + 1) {
                Some(b'"' | b'\\' | b'/' | b'b' | b'f' | b'n' | b'r' | b't') => *at += 2,
                Some(b'u') => match b.get(*at + 2..*at + 6) {
                    Some(hex) if hex.iter().all(u8::is_ascii_hexdigit) => *at += 6,
                    _ => return false,
                },
                _ => return false,
            },
            Some(0x00..=0x1f) | None => return false,
            Some(_) => *at += 1,
        }
    }
}

/// `json_lex_number`: `-?(0|[1-9][0-9]*)(\.[0-9]+)?([eE][+-]?[0-9]+)?`. A
/// letter, digit or `_` right after it would join the token in the server's
/// lexer and fail it; here it fails as the next token, which no position takes.
fn json_number(b: &[u8], at: &mut usize) -> bool {
    let digits = |at: &mut usize| {
        let start = *at;
        while b.get(*at).is_some_and(u8::is_ascii_digit) {
            *at += 1;
        }
        *at > start
    };
    *at += usize::from(b.get(*at) == Some(&b'-'));
    match b.get(*at) {
        Some(b'0') => *at += 1,
        Some(b'1'..=b'9') => {
            digits(at);
        }
        _ => return false,
    }
    if b.get(*at) == Some(&b'.') {
        *at += 1;
        if !digits(at) {
            return false;
        }
    }
    if matches!(b.get(*at), Some(b'e' | b'E')) {
        *at += 1;
        *at += usize::from(matches!(b.get(*at), Some(b'+' | b'-')));
        if !digits(at) {
            return false;
        }
    }
    true
}
