#![doc = include_str!("../README.md")]

use std::fmt::{self, Write as _};
use std::io::{self, Read};

mod csv;
mod js;
mod json;

/// Data dictionary versions upstream accepts, for CSV and JSON alike.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Version {
    V2_0,
    V2_1,
    V2_2,
    V3_0,
}

const VERSIONS: &[&str] = &["2.0.0", "2.1.0", "2.2.0", "3.0.0"];

/// This crate's version.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

impl Version {
    /// `semver.coerce(version)`, then exactly one of `VERSIONS`.
    fn parse(version: &str) -> Option<Version> {
        let coerced = js::coerce(version);
        let i = VERSIONS.iter().position(|&v| v == coerced)?;
        Some([Version::V2_0, Version::V2_1, Version::V2_2, Version::V3_0][i])
    }
}

/// Validates a machine-readable file against data dictionary `version` (`"3.0.0"`, `"v2.2"`
/// and the like, coerced as upstream does).
///
/// # Errors
///
/// [`Error::Io`] when reading `input` fails, [`Error::UnknownJsonVersion`] when a JSON
/// `version` is not one upstream knows (CSV reports that as a finding instead).
pub fn validate(
    input: impl Read,
    format: Format,
    version: &str,
    options: &Options,
) -> Result<Report, Error> {
    let mut report = match format {
        Format::Csv => csv::validate(input, version, options)?,
        Format::Json => json::validate(input, version, options)?,
    };
    if options.mode == Mode::Default {
        report.file_type = None;
        report.extended.clear();
        for f in report.errors.iter_mut().chain(&mut report.alerts) {
            f.line = None;
        }
    }
    Ok(report)
}

/// The format to read the input as. Upstream's CLI takes it from the file extension.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    /// CSV, tall or wide.
    Csv,
    /// JSON.
    Json,
}

/// Whether to return upstream's result alone or with the Extended additions.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Mode {
    /// Exactly what upstream returns.
    #[default]
    Default,
    /// The default result plus `file_type`, `extended` findings and each CSV finding's `line`.
    Extended,
}

/// How to validate. The default is unlimited findings in Default mode.
#[derive(Clone, Debug, Default)]
pub struct Options {
    /// The most errors to collect before stopping, as upstream's `maxErrors`; alerts and
    /// Extended findings are capped at the same number separately. `0` means unlimited.
    pub max_errors: usize,
    /// Default or Extended.
    pub mode: Mode,
}

/// The result of a validation: upstream's errors and alerts, then the Extended additions.
#[derive(Clone, Debug, PartialEq)]
pub struct Report {
    /// Errors in upstream's order. Any error makes the file invalid.
    pub errors: Vec<Finding>,
    /// Alerts in upstream's order: warnings that leave the file valid.
    pub alerts: Vec<Finding>,
    /// Extended only: the shape the file was read as.
    pub file_type: Option<FileType>,
    /// Extended only: findings upstream drops or never makes.
    pub extended: Vec<Finding>,
}

impl Report {
    /// Upstream's `valid`, which is always "no errors".
    #[must_use]
    pub fn valid(&self) -> bool {
        self.errors.is_empty()
    }
}

/// The shape of a validated file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileType {
    /// A JSON file.
    Json,
    /// A CSV file in the tall format: one row per payer and plan.
    Tall,
    /// A CSV file in the wide format: one column group per payer and plan.
    Wide,
    /// CSV whose shape could not be determined.
    Unknown,
}

impl FileType {
    /// The type as written in output: `json`, `tall`, `wide` or `unknown`.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            FileType::Json => "json",
            FileType::Tall => "tall",
            FileType::Wide => "wide",
            FileType::Unknown => "unknown",
        }
    }
}

/// Why no report could be produced. Problems with the file itself are findings, not errors.
#[derive(Debug)]
pub enum Error {
    /// Reading the input failed.
    Io(io::Error),
    /// Upstream throws for a JSON file validated against a version it has no schema for.
    UnknownJsonVersion(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Error::Io(e) => e.fmt(f),
            Error::UnknownJsonVersion(v) => {
                write!(f, "Could not load JSON schema with version: {v}")
            }
        }
    }
}

impl std::error::Error for Error {}

impl From<io::Error> for Error {
    fn from(e: io::Error) -> Self {
        Error::Io(e)
    }
}

/// One error or alert. The fields up to `column` are upstream's; `code`, `keyword` and
/// `line` are added for consumers that should not parse messages.
#[derive(Clone, Debug, PartialEq)]
pub struct Finding {
    /// What was found, as a stable code.
    pub code: Code,
    /// `"C4"`, `"row 4"`, or a JSON pointer.
    pub path: String,
    /// Upstream's message.
    pub message: String,
    /// JSON findings: the last segment of the path within the item.
    pub field: Option<String>,
    /// The CSV column or header the finding is about, as upstream names it.
    pub column_name: Option<String>,
    /// The offending CSV value.
    pub value: Option<String>,
    /// The values the CSV cell may take.
    pub allowed_values: Option<&'static [&'static str]>,
    /// The versions that may be validated against, for an invalid CSV version.
    pub versions: Option<&'static [&'static str]>,
    /// The zero-based column of a placeholder in a CSV header.
    pub column: Option<i64>,
    /// The Ajv keyword of a JSON schema finding.
    pub keyword: Option<&'static str>,
    /// Extended only: the physical line, from 1, where the CSV row it was found on starts.
    pub line: Option<u64>,
}

impl Finding {
    fn new(code: Code, path: String, message: String) -> Self {
        Finding {
            code,
            path,
            message,
            field: None,
            column_name: None,
            value: None,
            allowed_values: None,
            versions: None,
            column: None,
            keyword: None,
            line: None,
        }
    }

    /// The finding as upstream's `JSON.stringify` renders it, then `code`, `keyword` and `line`.
    #[must_use]
    pub fn to_json(&self) -> String {
        let mut out = String::from("{");
        let key = |out: &mut String, k: &str| {
            if out.len() > 1 {
                out.push(',');
            }
            push_json_str(out, k);
            out.push(':');
        };
        for (k, v) in [
            ("path", Some(&self.path)),
            ("message", Some(&self.message)),
            ("field", self.field.as_ref()),
            ("columnName", self.column_name.as_ref()),
            ("value", self.value.as_ref()),
        ] {
            if let Some(v) = v {
                key(&mut out, k);
                push_json_str(&mut out, v);
            }
        }
        for (k, v) in [
            ("allowedValues", self.allowed_values),
            ("versions", self.versions),
        ] {
            if let Some(v) = v {
                key(&mut out, k);
                out.push('[');
                for (i, s) in v.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    push_json_str(&mut out, s);
                }
                out.push(']');
            }
        }
        if let Some(v) = self.column {
            key(&mut out, "column");
            write!(out, "{v}").expect("a String write cannot fail");
        }
        if self.code == Code::JsonInvalid {
            key(&mut out, "originalError"); // an Error object stringifies to {}
            out.push_str("{}");
        }
        key(&mut out, "code");
        push_json_str(&mut out, self.code.as_str());
        if let Some(v) = self.keyword {
            key(&mut out, "keyword");
            push_json_str(&mut out, v);
        }
        if let Some(v) = self.line {
            key(&mut out, "line");
            write!(out, "{v}").expect("a String write cannot fail");
        }
        out.push('}');
        out
    }
}

/// Whether `list` has reached the limit; `max` 0 is unlimited.
fn full(list: &[Finding], max: usize) -> bool {
    max > 0 && list.len() >= max
}

/// Upstream's `addItemsWithLimit`: `max` 0 is unlimited.
fn add(list: &mut Vec<Finding>, items: impl IntoIterator<Item = Finding>, max: usize) {
    let room = if max > 0 {
        max.saturating_sub(list.len())
    } else {
        usize::MAX
    };
    list.extend(items.into_iter().take(room));
}

/// Upstream's `removeBOM`, applied to the first chunk: a leading BOM goes with all the
/// whitespace after it.
fn remove_bom(chunk: &str) -> &str {
    if chunk.starts_with('\u{feff}') {
        js::trim_start(chunk)
    } else {
        chunk
    }
}

/// A count of allowed amounts written as an integer: `1[1-9]|[2-9]\d+|[1-9]\d{2,}`, so 11
/// or more without leading zeros. Both formats also take `0` and `1 through 10`.
fn is_count_integer(v: &str) -> bool {
    v.len() >= 2 && v.bytes().all(|b| b.is_ascii_digit()) && !v.starts_with('0') && v != "10"
}

/// `VersionMismatchAlert`'s message, CSV and JSON alike.
fn version_mismatch(found: &str, expected: &[&str]) -> String {
    let expected = match expected {
        [one] => format!("the expected template version \"{one}\""),
        many => {
            let quoted: Vec<String> = many.iter().map(|v| format!("\"{v}\"")).collect();
            format!("an expected template version {}", quoted.join(", "))
        }
    };
    format!(
        "The value in this MRF's version data element \"{found}\" does not match {expected} for the data dictionary requirements selected for validation."
    )
}

/// A string literal as `JSON.stringify` writes it.
fn push_json_str(out: &mut String, s: &str) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            c if c < ' ' => {
                write!(out, "\\u{:04x}", u32::from(c)).expect("a String write cannot fail");
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

macro_rules! codes {
    ($($variant:ident = $code:literal,)*) => {
        /// Stable finding codes, listed with their upstream classes in the crate
        /// documentation. Renaming one is a breaking change; rewording a message is not.
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
        #[non_exhaustive]
        pub enum Code {
            $($variant,)*
        }

        impl Code {
            /// The code as written in output, such as `HPT_CSV_REQUIRED_VALUE`.
            pub fn as_str(self) -> &'static str {
                match self {
                    $(Code::$variant => $code,)*
                }
            }
        }
    };
}

codes! {
    CsvAllowedValues = "HPT_CSV_ALLOWED_VALUES",
    CsvAmbiguousFormat = "HPT_CSV_AMBIGUOUS_FORMAT",
    CsvChargeWithPayerPlan = "HPT_CSV_CHARGE_WITH_PAYER_PLAN",
    CsvCodePairMissing = "HPT_CSV_CODE_PAIR_MISSING",
    CsvColumnMissing = "HPT_CSV_COLUMN_MISSING",
    CsvDollarNeedsMinMax = "HPT_CSV_DOLLAR_NEEDS_MIN_MAX",
    CsvDrugInformationRequired = "HPT_CSV_DRUG_INFORMATION_REQUIRED",
    CsvDuplicateColumn = "HPT_CSV_DUPLICATE_COLUMN",
    CsvDuplicateHeaderColumn = "HPT_CSV_DUPLICATE_HEADER_COLUMN",
    CsvHeaderBlank = "HPT_CSV_HEADER_BLANK",
    CsvHeaderColumnMissing = "HPT_CSV_HEADER_COLUMN_MISSING",
    CsvInvalidCountNumber = "HPT_CSV_INVALID_COUNT_NUMBER",
    CsvInvalidDate = "HPT_CSV_INVALID_DATE",
    CsvInvalidPositiveNumber = "HPT_CSV_INVALID_POSITIVE_NUMBER",
    CsvInvalidStateCode = "HPT_CSV_INVALID_STATE_CODE",
    CsvInvalidVersion = "HPT_CSV_INVALID_VERSION",
    CsvItemRequiresCharge = "HPT_CSV_ITEM_REQUIRES_CHARGE",
    CsvMinRows = "HPT_CSV_MIN_ROWS",
    CsvModifierMissingInfo = "HPT_CSV_MODIFIER_MISSING_INFO",
    CsvOtherMethodologyNotes = "HPT_CSV_OTHER_METHODOLOGY_NOTES",
    CsvAllowedCountZeroNotes = "HPT_CSV_ALLOWED_COUNT_ZERO_NOTES",
    CsvPercentageAlgorithmEstimate = "HPT_CSV_PERCENTAGE_ALGORITHM_ESTIMATE",
    CsvPercentageAlgorithmCount = "HPT_CSV_PERCENTAGE_ALGORITHM_COUNT",
    CsvPercentageAlgorithmMedian = "HPT_CSV_PERCENTAGE_ALGORITHM_MEDIAN",
    CsvPercentageAlgorithm10th = "HPT_CSV_PERCENTAGE_ALGORITHM_10TH",
    CsvPercentageAlgorithm90th = "HPT_CSV_PERCENTAGE_ALGORITHM_90TH",
    CsvPlaceholder = "HPT_CSV_PLACEHOLDER",
    CsvProblemsInHeader = "HPT_CSV_PROBLEMS_IN_HEADER",
    CsvRequiredValue = "HPT_CSV_REQUIRED_VALUE",
    CsvEstimatedAmountDeprecated = "HPT_CSV_ESTIMATED_AMOUNT_DEPRECATED",
    CsvFalseAffirmation = "HPT_CSV_FALSE_AFFIRMATION",
    CsvFalseAttestation = "HPT_CSV_FALSE_ATTESTATION",
    CsvNineNines = "HPT_CSV_NINE_NINES",
    CsvVersionMismatch = "HPT_CSV_VERSION_MISMATCH",
    JsonInvalid = "HPT_JSON_INVALID",
    JsonSchema = "HPT_JSON_SCHEMA",
    JsonEstimatedAmountDeprecated = "HPT_JSON_ESTIMATED_AMOUNT_DEPRECATED",
    JsonNineNines = "HPT_JSON_NINE_NINES",
    JsonFalseAffirmation = "HPT_JSON_FALSE_AFFIRMATION",
    JsonFalseAttestation = "HPT_JSON_FALSE_ATTESTATION",
    JsonVersionMismatch = "HPT_JSON_VERSION_MISMATCH",
    XCsvInvalidQuotes = "HPT_X_CSV_INVALID_QUOTES",
    XCsvMissingQuotes = "HPT_X_CSV_MISSING_QUOTES",
    XCsvBomSwallowedLines = "HPT_X_CSV_BOM_SWALLOWED_LINES",
}

/// Upstream's `validateFilename`:
/// `/^(\d{2}\-?\d{7})(\-\d{10})?(_.+_)(standardcharges)\.(csv|json)$/i`.
#[must_use]
pub fn validate_filename(name: &str) -> bool {
    fn digits(s: &str, n: usize) -> Option<&str> {
        let d = s.get(..n)?;
        d.bytes().all(|b| b.is_ascii_digit()).then(|| &s[n..])
    }
    fn check(name: &str) -> Option<bool> {
        let rest = digits(name, 2)?;
        let rest = digits(rest.strip_prefix('-').unwrap_or(rest), 7)?;
        let rest = match rest.strip_prefix('-') {
            Some(r) => digits(r, 10)?,
            None => rest,
        };
        let lower = rest.strip_prefix('_')?.to_ascii_lowercase();
        let middle = [".csv", ".json"]
            .iter()
            .find_map(|ext| lower.strip_suffix(ext)?.strip_suffix("_standardcharges"))?;
        Some(!middle.is_empty() && !middle.contains(js::is_line_terminator))
    }
    check(name).unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finding_json() {
        let f = Finding {
            code: Code::CsvAllowedValues,
            path: "G2".into(),
            message: "\"x\"\\\u{1}\u{8}\n\té".into(),
            field: None,
            column_name: Some("setting".into()),
            value: Some(String::new()),
            allowed_values: Some(&["a", "b"]),
            versions: None,
            column: Some(3),
            keyword: None,
            line: Some(7),
        };
        assert_eq!(
            f.to_json(),
            r#"{"path":"G2","message":"\"x\"\\\u0001\b\n\té","columnName":"setting","value":"","allowedValues":["a","b"],"column":3,"code":"HPT_CSV_ALLOWED_VALUES","line":7}"#
        );
    }
}
