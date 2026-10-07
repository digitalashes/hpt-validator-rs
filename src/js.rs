//! JavaScript semantics upstream depends on, where the natural Rust choice answers differently.
//! `tests/js.json`, written by Node itself, pins every function here.

/// JS whitespace (`trim`, `\s`): Rust's `char::is_whitespace` minus U+0085, plus U+FEFF.
pub fn is_space(c: char) -> bool {
    (c.is_whitespace() && c != '\u{85}') || c == '\u{feff}'
}

pub fn trim(s: &str) -> &str {
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
    matches!(
        c,
        '\u{a7ce}' | '\u{a7cf}' | '\u{a7d2}' | '\u{a7d4}' | '\u{16ea0}'..='\u{16edf}'
    )
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
        let n = (digits.chars()).try_fold(0.0, |n, c| {
            Some(n * radix as f64 + c.to_digit(radix)? as f64)
        });
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
    let num = |p: &str, len: std::ops::RangeInclusive<usize>| {
        (len.contains(&p.len()) && p.bytes().all(|b| b.is_ascii_digit()))
            .then(|| p.parse::<u32>().unwrap())
    };
    let parts: Vec<&str> = s.split('/').collect();
    if let [m, d, y] = parts[..] {
        return match (num(m, 1..=2), num(d, 1..=2), num(y, 4..=4)) {
            (Some(m), Some(d), Some(y)) => y >= 100 && is_real_date(y, m, d),
            _ => false,
        };
    }
    let parts: Vec<&str> = s.split('-').collect();
    if let [y, m, d] = parts[..]
        && let (Some(y), Some(m), Some(d)) = (num(y, 4..=4), num(m, 2..=2), num(d, 2..=2))
    {
        return is_real_date(y, m, d);
    }
    false
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
#[cfg_attr(
    not(test),
    expect(dead_code, reason = "data-row rules, roadmap Phase 3")
)]
pub fn is_positive_number(s: &str) -> bool {
    // Rust's float grammar rejects the rest: no digit, or a second dot.
    s.bytes().all(|b| b.is_ascii_digit() || b == b'.') && s.parse::<f64>().is_ok_and(|n| n > 0.0)
}

/// `semver.coerce(v)?.toString() ?? v`, as upstream normalises the version argument.
pub fn coerce(v: &str) -> String {
    const MAX_SAFE_INTEGER: u64 = (1 << 53) - 1;
    let b = v.as_bytes();
    let run = |i: usize| {
        b.get(i..)
            .map_or(0, |r| r.iter().take_while(|c| c.is_ascii_digit()).count())
    };
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
        (*p == "0" || !p.starts_with('0')) && p.parse::<u64>().unwrap() <= MAX_SAFE_INTEGER
    });
    if valid {
        parts.join(".")
    } else {
        v.to_string()
    }
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
        t[key]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| (p[0].as_str().unwrap(), &p[1]))
    }

    #[test]
    fn whitespace_and_case() {
        let unicode = char::UNICODE_VERSION;
        assert_eq!(
            unicode,
            (17, 0, 0),
            "recheck newer_than_node against Node's Unicode 16"
        );
        let t = table();
        let trim: Vec<u64> = t["trim"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_u64().unwrap())
            .collect();
        let mapped = |key: &str, c: char| {
            let v = t[key].get((c as u32).to_string());
            v.map_or(c.to_string(), |v| v.as_str().unwrap().into())
        };
        for c in (0..=0x10ffff).filter_map(char::from_u32) {
            assert_eq!(
                is_space(c),
                trim.contains(&(c as u64)),
                "trim {:#x}",
                c as u32
            );
            assert_eq!(
                to_upper(&c.to_string()),
                mapped("upper", c),
                "upper {:#x}",
                c as u32
            );
            assert_eq!(
                to_lower(&c.to_string()),
                mapped("lower", c),
                "lower {:#x}",
                c as u32
            );
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
            assert_eq!(
                is_positive_number(s),
                e.as_bool().unwrap(),
                "positive({s:?})"
            );
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
