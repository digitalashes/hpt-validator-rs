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
        let format = match case.extension().unwrap().to_str() {
            Some("csv") => Format::Csv,
            _ => Format::Json,
        };
        let results: Value = serde_json::from_str(&fs::read_to_string(&golden).unwrap()).unwrap();
        for (key, expected) in results.as_object().unwrap() {
            let want = serde_json::to_string(expected).unwrap();
            let (version, max_errors) = key.split_once(" maxErrors=").unwrap();
            for mode in [Mode::Default, Mode::Extended] {
                let options = Options {
                    max_errors: max_errors.parse().unwrap(),
                    mode,
                };
                let (got, plain) =
                    match validate(File::open(&case).unwrap(), format, version, &options) {
                        Ok(report) => (
                            upstream_json(&report),
                            report.file_type.is_none() && report.extended.is_empty(),
                        ),
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
