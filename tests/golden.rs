//! Every case in `tests/cases` against upstream's own result in `tests/golden`, in both modes,
//! and the Extended additions on the cases listed in `tests/extended.json`.

#![expect(clippy::unwrap_used, reason = "a broken case should fail the test")]

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

/// Findings as `Finding::to_json` renders them, without the keys the port adds unless `all`.
fn list(findings: &[Finding], all: bool) -> Vec<Value> {
    let render = |f: &Finding| {
        let mut v: Value = serde_json::from_str(&f.to_json()).unwrap();
        let keys = v.as_object_mut().unwrap();
        keys.retain(|k, _| all || !matches!(k.as_str(), "code" | "keyword" | "line"));
        v
    };
    findings.iter().map(render).collect()
}

/// The report as upstream's `JSON.stringify` writes it.
fn upstream_json(report: &Report) -> Value {
    json!({ "valid": report.valid(), "errors": list(&report.errors, false), "alerts": list(&report.alerts, false) })
}

fn run(case: &Path, key: &str, mode: Mode) -> Result<Report, hpt_validator::Error> {
    let format = match case.extension().unwrap().to_str() {
        Some("csv") => Format::Csv,
        _ => Format::Json,
    };
    let (version, max_errors) = key.split_once(" maxErrors=").unwrap();
    let options = Options {
        max_errors: max_errors.parse().unwrap(),
        mode,
    };
    validate(File::open(case).unwrap(), format, version, &options)
}

#[test]
fn golden() {
    let tests = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests");
    let mut goldens = Vec::new();
    files(&tests.join("golden"), &mut goldens);
    let (mut checked, mut failed) = (0, Vec::new());
    for golden in goldens {
        let case = tests
            .join("cases")
            .join(golden.strip_prefix(tests.join("golden")).unwrap())
            .with_extension("");
        let results: Value = serde_json::from_str(&fs::read_to_string(&golden).unwrap()).unwrap();
        for (key, expected) in results.as_object().unwrap() {
            let want = serde_json::to_string(expected).unwrap();
            for mode in [Mode::Default, Mode::Extended] {
                let (got, plain) = match run(&case, key, mode) {
                    Ok(report) => {
                        let mut all = report.errors.iter().chain(&report.alerts);
                        let plain = report.file_type.is_none()
                            && report.extended.is_empty()
                            && all.all(|f| f.line.is_none());
                        (upstream_json(&report), plain)
                    }
                    Err(e) => (
                        json!({ "throws": format!("Error: {e}") }),
                        mode == Mode::Default,
                    ),
                };
                let got = serde_json::to_string(&got).unwrap();
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
    eprintln!("golden: {checked} results checked");
    assert!(checked > 0);
    assert!(
        failed.is_empty(),
        "{} failed:\n{}",
        failed.len(),
        failed[..failed.len().min(10)].join("\n")
    );
}

#[test]
fn extended() {
    let tests = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests");
    let cases: Value =
        serde_json::from_str(&fs::read_to_string(tests.join("extended.json")).unwrap()).unwrap();
    let mut failed = Vec::new();
    for (name, expected) in cases.as_object().unwrap() {
        let (case, key) = name.split_once(' ').unwrap();
        let report = run(&tests.join("cases").join(case), key, Mode::Extended).unwrap();
        let got = json!({
            "file_type": format!("{:?}", report.file_type.unwrap()),
            "errors": list(&report.errors, true),
            "alerts": list(&report.alerts, true),
            "extended": list(&report.extended, true),
        });
        // As strings, so key order counts.
        if serde_json::to_string(&got).unwrap() != serde_json::to_string(expected).unwrap() {
            failed.push(format!(
                "\"{name}\": {}",
                serde_json::to_string_pretty(&got).unwrap()
            ));
        }
    }
    assert!(failed.is_empty(), "{}", failed.join(",\n"));
}
