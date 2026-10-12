//! An SQL identifier as the server reads it: the cursor reading one, a
//! qualified name, and a type name's canonical spelling — what the DDL
//! grammar records a declared type under, and what the catalog and the type
//! register compare when they look one up by name. Below all three, so
//! neither the model nor the codec reaches the parsers for it.

/// A byte cursor reading SQL as the server does: an identifier quoted or
/// bare, a keyword case-insensitively, the spaces between them. Read by
/// the `COPY` header grammar, by [`parse_type_name`] here, by `pgdt_dump::ddl`
/// for the preamble's DDL grammars (`CREATE TABLE` / `TYPE` / `DOMAIN` /
/// `EXTENSION`), and by pgdt-codec's `compare` for a collation's name.
pub struct Cursor<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Cursor<'a> {
    pub fn new(buf: &'a [u8]) -> Self {
        Self { buf, pos: 0 }
    }

    pub(crate) fn at_end(&self) -> bool {
        self.pos >= self.buf.len()
    }

    fn peek(&self) -> Option<u8> {
        self.buf.get(self.pos).copied()
    }

    pub fn skip_spaces(&mut self) {
        while matches!(self.peek(), Some(b' ' | b'\t')) {
            self.pos += 1;
        }
    }

    /// Whitespace that the grammar requires to be present (keyword separator).
    pub(crate) fn skip_spaces_required(&mut self) -> Option<()> {
        let before = self.pos;
        self.skip_spaces();
        (self.pos > before).then_some(())
    }

    pub fn eat_byte(&mut self, b: u8) -> bool {
        if self.peek() == Some(b) {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    /// Match an ASCII-lowercase keyword case-insensitively.
    pub(crate) fn eat_keyword(&mut self, kw: &[u8]) -> Option<()> {
        let end = self.pos.checked_add(kw.len())?;
        let slice = self.buf.get(self.pos..end)?;
        if slice.iter().zip(kw).all(|(a, b)| a.to_ascii_lowercase() == *b) {
            self.pos = end;
            Some(())
        } else {
            None
        }
    }

    /// Byte offset of the next unconsumed byte, usable to slice the original
    /// `&str`/`&[u8]` this cursor was built over.
    pub fn pos(&self) -> usize {
        self.pos
    }

    /// Parse a SQL identifier: either double-quoted (with `""` escaping) or
    /// bare (ASCII-case-folded, as unquoted SQL identifiers are).
    pub fn parse_ident(&mut self) -> Option<String> {
        if self.eat_byte(b'"') {
            let mut out: Vec<u8> = Vec::new();
            loop {
                let b = self.peek()?;
                self.pos += 1;
                if b == b'"' {
                    if self.peek() == Some(b'"') {
                        self.pos += 1;
                        out.push(b'"');
                        continue;
                    }
                    break;
                }
                out.push(b);
            }
            return String::from_utf8(out).ok();
        }

        let start = self.pos;
        while let Some(b) = self.peek() {
            if b.is_ascii_alphanumeric() || b == b'_' || b == b'$' || b >= 0x80 {
                self.pos += 1;
            } else {
                break;
            }
        }
        if self.pos == start {
            return None;
        }
        let s = std::str::from_utf8(&self.buf[start..self.pos]).ok()?;
        Some(s.to_ascii_lowercase())
    }
}

/// Parse a (possibly schema-qualified) type name from the start of `s` into
/// its **canonical spelling**, returning it and how many bytes of `s` it
/// consumed — what [`pgdt_model::TypeDef::name`] holds, and what every lookup of a type by
/// name compares ([`canonical_type_name`]).
///
/// Each part is read as the server reads it — an unquoted identifier folded to
/// lower case, a quoted one taken verbatim (I29) — and written back bare where
/// it is a plain lower-case identifier, quoted with `""` doubled everywhere
/// else. So `public."Mood"`, `PUBLIC."Mood"` and `"public"."Mood"` agree while
/// `public.Mood` is `public.mood`, and no two distinct names share a spelling:
/// a bare part holds no `.` or `"`. It is `fmtId()`'s spelling but for a
/// keyword, which `pg_dump` quotes and this leaves bare.
pub fn parse_type_name(s: &str) -> Option<(String, usize)> {
    fn push_part(out: &mut String, part: &str) {
        let bytes = part.as_bytes();
        let plain = bytes.first().is_some_and(|b| b.is_ascii_lowercase() || *b == b'_')
            && bytes.iter().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || *b == b'_');
        if plain {
            out.push_str(part);
        } else {
            out.push('"');
            out.push_str(&part.replace('"', "\"\""));
            out.push('"');
        }
    }
    let mut cur = Cursor::new(s.as_bytes());
    cur.skip_spaces();
    let mut name = String::new();
    push_part(&mut name, &cur.parse_ident()?);
    let before_dot = cur.pos();
    cur.skip_spaces();
    if cur.eat_byte(b'.') {
        cur.skip_spaces();
        name.push('.');
        push_part(&mut name, &cur.parse_ident()?);
        return Some((name, cur.pos()));
    }
    Some((name, before_dot))
}

/// `reference` — a declared type, a domain's base, a multirange companion's
/// name — in [`parse_type_name`]'s canonical spelling, or `None` where it is
/// not one type name alone (`integer[]`, `double precision`, text the
/// identifier grammar does not accept). Applied to both sides of a lookup, so
/// two spellings of one name find each other and nothing else does.
pub fn canonical_type_name(reference: &str) -> Option<String> {
    let (name, consumed) = parse_type_name(reference)?;
    reference[consumed..].trim().is_empty().then_some(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A type name is kept in one spelling however the DDL wrote it: each part
    /// read as the server reads it — an unquoted one folded, a quoted one
    /// verbatim — and quoted back only where it is not a plain lower-case
    /// identifier (I29). Text that is not one name has no spelling.
    #[test]
    fn a_type_name_has_one_canonical_spelling() {
        for (reference, canonical) in [
            ("public.mood", "public.mood"),
            ("PUBLIC.Mood", "public.mood"),
            (r#""public"."mood""#, "public.mood"),
            (r#"public."Mood""#, r#"public."Mood""#),
            (r#"s."x ARRAY""#, r#"s."x ARRAY""#),
            (r#"s."a""b""#, r#"s."a""b""#),
            (r#"s."a.b""#, r#"s."a.b""#),
            ("s.x$1", r#"s."x$1""#),
            ("s . t ", "s.t"),
            ("mood", "mood"),
        ] {
            assert_eq!(canonical_type_name(reference).as_deref(), Some(canonical), "{reference}");
            assert_eq!(canonical_type_name(canonical).as_deref(), Some(canonical), "{canonical}");
        }
        for reference in ["integer[]", "double precision", r#"s."a"[]"#, "numeric(10,2)", "s."] {
            assert_eq!(canonical_type_name(reference), None, "{reference}");
        }
    }
}
