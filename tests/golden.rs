//! Every case in `tests/cases` against upstream's own result in `tests/golden`, in both modes.

use hpt_validator::{Finding, Format, Mode, Options, Report, validate};
use serde_json::{Value, json};
use std::fs::{self, File};
use std::path::{Path, PathBuf};

fn files(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            files(&path, out);
        } else {
            out.push(path);
        }
    }
}

/// The report as upstream's `JSON.stringify` writes it, without the keys the port adds.
fn upstream_json(report: &Report) -> Value {
    let list = |findings: &[Finding]| {
        let strip = |f: &Finding| {
            let mut v: Value = serde_json::from_str(&f.to_json()).unwrap();
            let keys = v.as_object_mut().unwrap();
            keys.retain(|k, _| !matches!(k.as_str(), "code" | "keyword" | "line"));
            v
        };
        findings.iter().map(strip).collect::<Vec<_>>()
    };
    json!({ "valid": report.valid(), "errors": list(&report.errors), "alerts": list(&report.alerts) })
}

/// Upstream's result cut to rows 1–3, which roadmap Phase 2 covers. Header and column
/// findings always come before data-row ones, so each list keeps a prefix.
fn by_row_3(result: &Value) -> Value {
    let early = |list: &str| {
        let findings = result[list].as_array().unwrap().iter().filter(|f| {
            let path = f["path"].as_str().unwrap();
            let row = path.trim_start_matches(|c: char| !c.is_ascii_digit());
            row.parse::<u64>().unwrap() <= 3
        });
        findings.cloned().collect::<Vec<_>>()
    };
    let errors = early("errors");
    json!({ "valid": errors.is_empty(), "errors": errors, "alerts": early("alerts") })
}

#[test]
fn golden() {
    let tests = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests");
    let mut goldens = Vec::new();
    files(&tests.join("golden"), &mut goldens);
    let (mut checked, mut later, mut failed) = (0, 0, Vec::new());
    for golden in goldens {
        let case = tests
            .join("cases")
            .join(golden.strip_prefix(tests.join("golden")).unwrap())
            .with_extension("");
        if case.extension().unwrap() != "csv" {
            later += 1; // JSON: roadmap Phase 4
            continue;
        }
        let results: Value = serde_json::from_str(&fs::read_to_string(&golden).unwrap()).unwrap();
        for (key, expected) in results.as_object().unwrap() {
            let want = serde_json::to_string(&by_row_3(expected)).unwrap();
            let (version, max_errors) = key.split_once(" maxErrors=").unwrap();
            for mode in [Mode::Default, Mode::Extended] {
                let options = Options {
                    max_errors: max_errors.parse().unwrap(),
                    mode,
                };
                let report =
                    validate(File::open(&case).unwrap(), Format::Csv, version, &options).unwrap();
                let got = serde_json::to_string(&upstream_json(&report)).unwrap();
                let plain = report.file_type.is_none() && report.extended.is_empty();
                if got != want || (mode == Mode::Default) != plain {
                    failed.push(format!(
                        "{} {key} {mode:?}\n  got  {got}\n  want {want}",
                        case.display()
                    ));
                }
            }
            checked += 1;
        }
    }
    eprintln!("golden: {checked} CSV results checked up to row 3, {later} JSON cases left");
    assert!(checked > 0);
    assert!(
        failed.is_empty(),
        "{} failed:\n{}",
        failed.len(),
        failed[..failed.len().min(10)].join("\n")
    );
}
