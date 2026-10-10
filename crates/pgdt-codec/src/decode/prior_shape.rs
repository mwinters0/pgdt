use super::civil_from_days;
use pgdt_core::internal::hex_val;

fn format_hms_frac(total_micros: i64) -> String {
    let seconds = total_micros.div_euclid(1_000_000);
    let micros = total_micros.rem_euclid(1_000_000);
    let h = seconds / 3600;
    let mi = (seconds % 3600) / 60;
    let se = seconds % 60;
    if micros == 0 {
        format!("{h:02}:{mi:02}:{se:02}")
    } else {
        let mut frac = format!("{micros:06}");
        while frac.ends_with('0') {
            frac.pop();
        }
        format!("{h:02}:{mi:02}:{se:02}.{frac}")
    }
}

pub fn render_date32(days: i32) -> String {
    let (y, m, d) = civil_from_days(i64::from(days));
    let (out_year, bc) = if y <= 0 { (1 - y, true) } else { (y, false) };
    let mut s = format!("{out_year:04}-{m:02}-{d:02}");
    if bc {
        s.push_str(" BC");
    }
    s
}

pub fn render_time64_micros(v: i64) -> String {
    format_hms_frac(v)
}

pub fn render_timestamp_micros(v: i64, with_tz: bool) -> String {
    let days = v.div_euclid(86_400_000_000);
    let time_micros = v.rem_euclid(86_400_000_000);
    let (y, m, d) = civil_from_days(days);
    let (out_year, bc) = if y <= 0 { (1 - y, true) } else { (y, false) };
    let mut s = format!("{out_year:04}-{m:02}-{d:02} {}", format_hms_frac(time_micros));
    if with_tz {
        s.push_str("+00");
    }
    if bc {
        s.push_str(" BC");
    }
    s
}

pub fn render_uuid(bytes: &[u8; 16]) -> String {
    let hex = |b: &[u8]| b.iter().map(|x| format!("{x:02x}")).collect::<String>();
    format!(
        "{}-{}-{}-{}-{}",
        hex(&bytes[0..4]),
        hex(&bytes[4..6]),
        hex(&bytes[6..8]),
        hex(&bytes[8..10]),
        hex(&bytes[10..16])
    )
}

pub fn render_bytea(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(2 + bytes.len() * 2);
    s.push_str("\\x");
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

pub fn parse_time_of_day(s: &str) -> Option<(i64, i64)> {
    let (hms, frac) = s.split_once('.').unwrap_or((s, ""));
    let mut parts = hms.splitn(3, ':');
    let h: i64 = parts.next()?.parse().ok()?;
    let mi: i64 = parts.next()?.parse().ok()?;
    let se: i64 = parts.next()?.parse().ok()?;
    if parts.next().is_some() || frac.len() > 6 || !frac.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let mut frac_digits = frac.to_string();
    frac_digits.push_str(&"0".repeat(6 - frac_digits.len()));
    let micros: i64 = if frac_digits.is_empty() { 0 } else { frac_digits.parse().ok()? };
    Some((h * 3600 + mi * 60 + se, micros))
}

pub fn decode_uuid(s: &str) -> Option<[u8; 16]> {
    let clean: String = s.chars().filter(|c| *c != '-').collect();
    if clean.len() != 32 {
        return None;
    }
    let b = clean.as_bytes();
    let mut bytes = [0u8; 16];
    for (i, byte) in bytes.iter_mut().enumerate() {
        *byte = (hex_val(b[i * 2])? << 4 | hex_val(b[i * 2 + 1])?) as u8;
    }
    Some(bytes)
}

pub fn decode_bytea(s: &str) -> Option<Vec<u8>> {
    let hex = s.strip_prefix("\\x")?;
    if hex.len() % 2 != 0 {
        return None;
    }
    let b = hex.as_bytes();
    let mut out = Vec::with_capacity(hex.len() / 2);
    for i in (0..b.len()).step_by(2) {
        out.push((hex_val(b[i])? << 4 | hex_val(b[i + 1])?) as u8);
    }
    Some(out)
}

pub fn decimal_unscaled_digits(s: &str, scale: i8) -> Option<String> {
    if s == "NaN" {
        return None;
    }
    let (neg, s) = match s.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, s),
    };
    let (int_part, frac_part) = s.split_once('.').unwrap_or((s, ""));
    if int_part.is_empty() && frac_part.is_empty() {
        return None;
    }
    if !int_part.bytes().all(|b| b.is_ascii_digit())
        || !frac_part.bytes().all(|b| b.is_ascii_digit())
    {
        return None;
    }
    let digits = format!("{int_part}{frac_part}");
    let shift = i32::from(scale) - frac_part.len() as i32;
    let unscaled = if shift >= 0 {
        format!("{digits}{}", "0".repeat(shift as usize))
    } else {
        let cut = (-shift) as usize;
        if cut > digits.len() {
            return digits.bytes().all(|b| b == b'0').then(|| "0".to_string());
        }
        let (keep, dropped) = digits.split_at(digits.len() - cut);
        if !dropped.bytes().all(|b| b == b'0') {
            return None;
        }
        keep.to_string()
    };
    let unscaled = if unscaled.is_empty() { "0" } else { unscaled.trim_start_matches('0') };
    let unscaled = if unscaled.is_empty() { "0" } else { unscaled };
    Some(if neg && unscaled != "0" { format!("-{unscaled}") } else { unscaled.to_string() })
}
