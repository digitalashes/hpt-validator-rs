//! The validator never panics and Extended only adds to Default, on every case in `tests/cases`
//! mutated at random. `fuzz/` runs the same check under libFuzzer.

use hpt_validator::{Format, Mode, Options, validate};

/// Validates `data` at every version, with one error and with upstream CLI's default limit,
/// in both modes. A small input seldom reaches that limit, and it bounds the work a huge code
/// count asks for (`fuzzed` covers that without one).
///
/// # Panics
///
/// When validating panics, or the Extended report without its additions is not the Default
/// one.
pub fn check(format: Format, data: &[u8]) {
    for version in ["2.0.0", "2.1.0", "2.2.0", "3.0.0"] {
        for max_errors in [1, 1000] {
            let run = |mode| {
                validate(data, format, version, &Options { max_errors, mode })
                    .expect("a known version, and a slice always reads")
            };
            let (default, mut extended) = (run(Mode::Default), run(Mode::Extended));
            extended.file_type = None;
            extended.extended.clear();
            for f in extended.errors.iter_mut().chain(&mut extended.alerts) {
                f.line = None;
            }
            assert_eq!(default, extended, "v{version} maxErrors={max_errors}");
        }
    }
}

#[test]
fn mutated_cases() {
    use std::fs;
    use std::path::Path;

    // Bytes that delimit CSV and JSON, and one that is never UTF-8.
    const BYTES: &[u8] = b",\"\n\r|{}[]:0 \xff";
    let mut state: u64 = 0x9e37_79b9_7f4a_7c15;
    let mut below = |n: usize| {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        usize::try_from(state % n as u64).expect("below a usize")
    };
    let mut dirs = vec![Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/cases")];
    let mut checked = 0;
    while let Some(dir) = dirs.pop() {
        for entry in fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                dirs.push(path);
                continue;
            }
            let format = match path.extension().and_then(|e| e.to_str()) {
                Some("csv") => Format::Csv,
                _ => Format::Json,
            };
            let case = fs::read(&path).unwrap();
            for _ in 0..2 {
                let mut data = case.clone();
                for _ in 0..=below(4) {
                    let at = below(data.len() + 1);
                    let byte = BYTES[below(BYTES.len())];
                    match below(3) {
                        0 if at < data.len() => data[at] = byte,
                        1 => data.insert(at, byte),
                        _ => drop(data.drain(at..data.len().min(at + 1 + below(8)))),
                    }
                }
                check(format, &data);
                checked += 1;
            }
        }
    }
    assert!(checked > 0);
}

/// Inputs the fuzzer found.
#[test]
fn fuzzed() {
    // A code count far past row 3's columns, here past what a u64 holds.
    let input = b"a\nb\npayer_name,plan_name,code | 99999999999999999999\n";
    check(Format::Csv, input);
    validate(&input[..], Format::Csv, "3.0.0", &Options::default()).unwrap();
}
