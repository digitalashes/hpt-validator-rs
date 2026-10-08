//! JavaScript semantics upstream depends on, where the natural Rust choice answers differently.
//! `tests/js.json` and the reader corpus, written by Node itself, pin every function here.

use std::io::{self, Read};

/// JS whitespace (`trim`, `\s`): Rust's `char::is_whitespace` minus U+0085, plus U+FEFF.
pub fn is_space(c: char) -> bool {
    (c.is_whitespace() && c != '\u{85}') || c == '\u{feff}'
}

pub fn trim(s: &str) -> &str {
    // ASCII above the space is never JS whitespace, and most values start and end with it.
    let plain = |c: Option<&u8>| c.is_some_and(|&c| c > b' ' && c.is_ascii());
    if plain(s.as_bytes().first()) && plain(s.as_bytes().last()) {
        return s;
    }
    s.trim_matches(is_space)
}

pub fn trim_start(s: &str) -> &str {
    s.trim_start_matches(is_space)
}

/// What `.` in a regular expression does not match.
pub fn is_line_terminator(c: char) -> bool {
    matches!(c, '\n' | '\r' | '\u{2028}' | '\u{2029}')
}

/// Letters Node 20 does not know: its case tables are Unicode 16, Rust's std is on Unicode 17.
fn newer_than_node(c: char) -> bool {
    matches!(c, '\u{a7ce}' | '\u{a7cf}' | '\u{a7d2}' | '\u{a7d4}' | '\u{16ea0}'..='\u{16edf}')
}

/// `toUpperCase` / `toLocaleUpperCase`.
pub fn to_upper(s: &str) -> String {
    if s.is_ascii() {
        return s.to_ascii_uppercase();
    }
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        let upper = c.to_uppercase();
        // An older letter whose capital is newer (ꟓ, ꟕ) stays as it is too.
        if newer_than_node(c) || upper.clone().any(newer_than_node) {
            out.push(c);
        } else {
            out.extend(upper);
        }
    }
    out
}

/// `toLowerCase` / `toLocaleLowerCase`, final sigma included.
// ponytail: final sigma uses std's Unicode 17 cased and case-ignorable classes, which differ
// from Node's at a few dozen code points ("ʕΣ" lowers to "ʕσ", Node says "ʕς"); carry Node's
// classes if that ever matters
pub fn to_lower(s: &str) -> String {
    // To Node the newer letters are unassigned: kept as they are, and a final-sigma boundary.
    let mut out = String::with_capacity(s.len());
    let mut last = 0;
    for (i, c) in s.match_indices(newer_than_node) {
        out += &s[last..i].to_lowercase();
        out += c;
        last = i + c.len();
    }
    out + &s[last..].to_lowercase()
}

/// `Number(s)`.
pub fn number(s: &str) -> f64 {
    let s = trim(s);
    if s.is_empty() {
        return 0.0;
    }
    let radix = match s.get(..2) {
        Some("0x" | "0X") => Some(16),
        Some("0o" | "0O") => Some(8),
        Some("0b" | "0B") => Some(2),
        _ => None,
    };
    if let Some(radix) = radix {
        let digits = &s[2..];
        // ponytail: not correctly rounded above 2^53, which no caller can tell apart
        let n = (digits.chars())
            .try_fold(0.0, |n, c| Some(n * f64::from(radix) + f64::from(c.to_digit(radix)?)));
        return n.filter(|_| !digits.is_empty()).unwrap_or(f64::NAN);
    }
    // Rust's grammar is JS's StrDecimalLiteral plus `inf`, `nan` and `infinity` in any case,
    // where JS has only `Infinity`.
    let unsigned = s.strip_prefix(['+', '-']).unwrap_or(s);
    if unsigned != "Infinity" && unsigned.starts_with(|c: char| c.is_ascii_alphabetic()) {
        return f64::NAN;
    }
    s.parse().unwrap_or(f64::NAN)
}

/// Upstream's `isValidDate`: `M/D/YYYY` through `Date.UTC`, which maps years 0–99 to
/// 1900–1999 and so rejects them, or `YYYY-MM-DD` through `Date` parsing, which does not.
pub fn is_valid_date(s: &str) -> bool {
    let parts: Vec<&str> = s.split('/').collect();
    if let [m, d, y] = parts[..] {
        return match (digits(m, 1..=2), digits(d, 1..=2), digits(y, 4..=4)) {
            (Some(m), Some(d), Some(y)) => y >= 100 && is_real_date(y, m, d),
            _ => false,
        };
    }
    is_iso_date(s)
}

/// `YYYY-MM-DD` naming a real day: `isValidDate`'s second form and ajv-formats' `date`.
pub fn is_iso_date(s: &str) -> bool {
    let parts: Vec<&str> = s.split('-').collect();
    match parts[..] {
        [y, m, d] => match (digits(y, 4..=4), digits(m, 2..=2), digits(d, 2..=2)) {
            (Some(y), Some(m), Some(d)) => is_real_date(y, m, d),
            _ => false,
        },
        _ => false,
    }
}

/// `p` as a number if it is `len` ASCII digits.
fn digits(p: &str, len: std::ops::RangeInclusive<usize>) -> Option<u32> {
    (len.contains(&p.len()) && p.bytes().all(|b| b.is_ascii_digit()))
        .then(|| p.parse().ok())
        .flatten()
}

fn is_real_date(y: u32, m: u32, d: u32) -> bool {
    let leap = y.is_multiple_of(4) && (!y.is_multiple_of(100) || y.is_multiple_of(400));
    let days = match m {
        2 if leap => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    };
    (1..=12).contains(&m) && (1..=days).contains(&d)
}

/// Upstream's positive-number rule for a non-empty value:
/// `/^(?:\d+|\d+\.\d+|\d+\.|\.\d+)$/` and `parseFloat(value) > 0`.
pub fn is_positive_number(s: &str) -> bool {
    // Rust's float grammar rejects the rest: no digit, or a second dot.
    s.bytes().all(|b| b.is_ascii_digit() || b == b'.') && s.parse::<f64>().is_ok_and(|n| n > 0.0)
}

/// `semver.coerce(v)?.toString() ?? v`, as upstream normalises the version argument.
pub fn coerce(v: &str) -> String {
    const MAX_SAFE_INTEGER: u64 = (1 << 53) - 1;
    let b = v.as_bytes();
    let run =
        |i: usize| b.get(i..).map_or(0, |r| r.iter().take_while(|c| c.is_ascii_digit()).count());
    // semver's COERCE regex: (^|[^\d])(\d{1,16})(?:\.(\d{1,16}))?(?:\.(\d{1,16}))?(?:$|[^\d])
    // The leftmost start whose digit run is 1–16 long matches; a longer run fails there.
    let starts = |&i: &usize| (i == 0 || !b[i - 1].is_ascii_digit()) && (1..=16).contains(&run(i));
    let Some(start) = (0..b.len()).find(starts) else {
        return v.to_string();
    };
    let mut i = start + run(start);
    let mut parts = vec![&v[start..i]];
    while parts.len() < 3 && b.get(i) == Some(&b'.') {
        let n = run(i + 1);
        if !(1..=16).contains(&n) {
            break;
        }
        parts.push(&v[i + 1..i + 1 + n]);
        i += 1 + n;
    }
    parts.resize(3, "0");
    // semver.parse then rejects leading zeros and unsafe integers
    let valid = parts.iter().all(|p| {
        (*p == "0" || !p.starts_with('0')) && p.parse::<u64>().is_ok_and(|n| n <= MAX_SAFE_INTEGER)
    });
    if valid { parts.join(".") } else { v.to_string() }
}

/// `String(n)`: the shortest digits that read back as `n`, laid out as `Number::toString` does.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_sign_loss,
    reason = "at most 17 digits, exponents within 324, each index cast only when non-negative"
)]
pub fn number_string(n: f64) -> String {
    if n.is_nan() {
        return "NaN".into();
    }
    if n == 0.0 {
        return "0".into();
    }
    let sign = if n < 0.0 { "-" } else { "" };
    if n.is_infinite() {
        return format!("{sign}Infinity");
    }
    // `{:e}` gives the shortest digits as `d.ddde-7`; ECMAScript names the count of digits k
    // and the decimal point's position n.
    let sci = format!("{:e}", n.abs());
    let (mantissa, exp) = sci.split_once('e').expect("{:e} has an exponent");
    let digits = mantissa.replace('.', "");
    let (k, e) =
        (digits.len() as i32, exp.parse::<i32>().expect("{:e} writes an integer exponent"));
    let n = e + 1;
    let body = if k <= n && n <= 21 {
        digits + &"0".repeat((n - k) as usize)
    } else if 0 < n && n <= 21 {
        format!("{}.{}", &digits[..n as usize], &digits[n as usize..])
    } else if -6 < n && n <= 0 {
        format!("0.{}{digits}", "0".repeat(-n as usize))
    } else {
        let exp = format!("e{}{}", if e < 0 { '-' } else { '+' }, e.abs());
        match digits.split_at(1) {
            (d, "") => format!("{d}{exp}"),
            (d, rest) => format!("{d}.{rest}{exp}"),
        }
    };
    format!("{sign}{body}")
}

/// `fs.createReadStream`'s chunk size.
pub const CHUNK: usize = 64 * 1024;

/// A file as `fs.createReadStream(path, "utf-8")` emits it: fixed-size reads decoded by Node's
/// streaming UTF-8 decoder, one string per `data` event, never an empty one.
pub struct Stream<R> {
    input: R,
    chunk: u64,
    /// Bytes of a UTF-8 sequence split by the chunk boundary.
    undecoded: Vec<u8>,
    ended: bool,
}

impl<R: Read> Stream<R> {
    pub fn new(input: R, chunk: usize) -> Self {
        Stream { input, chunk: chunk as u64, undecoded: Vec::new(), ended: false }
    }

    /// Appends the next `data` event's text to `out`; false once the input is exhausted.
    pub fn next(&mut self, out: &mut String) -> io::Result<bool> {
        while !self.ended {
            let n = (self.input.by_ref()).take(self.chunk).read_to_end(&mut self.undecoded)?;
            if n == 0 {
                self.ended = true;
                // The decoder's `end()`: an unfinished sequence is one U+FFFD, its own event.
                if !self.undecoded.is_empty() {
                    self.undecoded.clear();
                    out.push('\u{fffd}');
                    return Ok(true);
                }
            } else {
                let start = out.len();
                decode(&mut self.undecoded, out);
                if out.len() > start {
                    return Ok(true);
                }
            }
        }
        Ok(false)
    }
}

/// Node's streaming UTF-8 decoder: complete sequences go to `out`, each maximal invalid
/// subpart becomes U+FFFD, and an incomplete sequence at the end stays in `bytes`.
fn decode(bytes: &mut Vec<u8>, out: &mut String) {
    let done = match std::str::from_utf8(bytes) {
        Ok(text) => {
            out.push_str(text);
            bytes.len()
        }
        // Valid but for a sequence cut short at the end: no lossy pass needed.
        Err(e) if e.error_len().is_none() => {
            out.push_str(
                std::str::from_utf8(&bytes[..e.valid_up_to()]).expect("valid up to there"),
            );
            e.valid_up_to()
        }
        Err(_) => {
            let incomplete = bytes.utf8_chunks().last().map_or(0, |c| {
                let bad = c.invalid();
                let cut_short = std::str::from_utf8(bad).is_err_and(|e| e.error_len().is_none());
                if cut_short { bad.len() } else { 0 }
            });
            let done = bytes.len() - incomplete;
            out.push_str(&String::from_utf8_lossy(&bytes[..done]));
            done
        }
    };
    bytes.drain(..done);
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    fn table() -> Value {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/js.json");
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
    }

    fn pairs<'a>(t: &'a Value, key: &str) -> impl Iterator<Item = (&'a str, &'a Value)> {
        t[key].as_array().unwrap().iter().map(|p| (p[0].as_str().unwrap(), &p[1]))
    }

    #[test]
    fn whitespace_and_case() {
        let unicode = char::UNICODE_VERSION;
        assert_eq!(unicode, (17, 0, 0), "recheck newer_than_node against Node's Unicode 16");
        let t = table();
        let trim: Vec<u64> =
            t["trim"].as_array().unwrap().iter().map(|v| v.as_u64().unwrap()).collect();
        let mapped = |key: &str, c: char| {
            let v = t[key].get((c as u32).to_string());
            v.map_or(c.to_string(), |v| v.as_str().unwrap().into())
        };
        for c in (0..=u32::from(char::MAX)).filter_map(char::from_u32) {
            assert_eq!(is_space(c), trim.contains(&(c as u64)), "trim {:#x}", c as u32);
            assert_eq!(to_upper(&c.to_string()), mapped("upper", c), "upper {:#x}", c as u32);
            assert_eq!(to_lower(&c.to_string()), mapped("lower", c), "lower {:#x}", c as u32);
        }
        for (s, e) in pairs(&t, "lowerStrings") {
            assert_eq!(to_lower(s), e.as_str().unwrap(), "lower {s:?}");
        }
    }

    #[test]
    fn number_table() {
        let t = table();
        for (s, expected) in pairs(&t, "number") {
            let n = number(s);
            let got = match expected.as_str() {
                Some("-0") => n == 0.0 && n.is_sign_negative(),
                Some(e) => n.to_string().replace("inf", "Infinity") == e,
                None => n.to_bits() == expected.as_f64().unwrap().to_bits(),
            };
            assert!(got, "Number({s:?}) = {n}, expected {expected}");
        }
    }

    #[test]
    fn helper_tables() {
        let t = table();
        for (s, e) in pairs(&t, "isValidDate") {
            assert_eq!(is_valid_date(s), e.as_bool().unwrap(), "isValidDate({s:?})");
        }
        for (s, e) in pairs(&t, "positive") {
            assert_eq!(is_positive_number(s), e.as_bool().unwrap(), "positive({s:?})");
        }
        for (s, e) in pairs(&t, "numberString") {
            assert_eq!(number_string(number(s)), e.as_str().unwrap(), "String(Number({s:?}))");
        }
        for (s, e) in pairs(&t, "coerce") {
            assert_eq!(coerce(s), e.as_str().unwrap(), "coerce({s:?})");
        }
        for (s, e) in pairs(&t, "validateFilename") {
            assert_eq!(
                crate::validate_filename(s),
                e.as_bool().unwrap(),
                "validateFilename({s:?})"
            );
        }
    }
}
