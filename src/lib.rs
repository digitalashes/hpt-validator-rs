//! Validator for CMS Hospital Price Transparency machine-readable files, a port of
//! [`@cmsgov/hpt-validator`](https://github.com/CMSgov/hpt-validator).

use std::fmt::{self, Write as _};
use std::io;

#[expect(dead_code, reason = "not wired to a validator yet")]
mod csv;
#[cfg_attr(not(test), expect(dead_code, reason = "not wired to a validator yet"))]
mod js;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    Csv,
    Json,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Mode {
    /// Exactly what upstream returns.
    #[default]
    Default,
    /// The default result plus `file_type` and `extended` findings.
    Extended,
}

#[derive(Clone, Debug, Default)]
pub struct Options {
    /// `0` means unlimited.
    pub max_errors: usize,
    pub mode: Mode,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Report {
    pub errors: Vec<Finding>,
    pub alerts: Vec<Finding>,
    /// Extended only.
    pub file_type: Option<FileType>,
    /// Extended only.
    pub extended: Vec<Finding>,
}

impl Report {
    /// Upstream's `valid`, which is always "no errors".
    pub fn valid(&self) -> bool {
        self.errors.is_empty()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileType {
    Json,
    Tall,
    Wide,
    /// CSV whose shape could not be determined.
    Unknown,
}

#[derive(Debug)]
pub enum Error {
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
    pub code: Code,
    /// `"C4"`, `"row 4"`, or a JSON pointer.
    pub path: String,
    pub message: String,
    pub field: Option<String>,
    pub column_name: Option<String>,
    pub value: Option<String>,
    pub allowed_values: Option<&'static [&'static str]>,
    pub versions: Option<&'static [&'static str]>,
    pub column: Option<i64>,
    /// The Ajv keyword of a JSON schema finding.
    pub keyword: Option<&'static str>,
    /// Extended only: the physical line where the CSV row starts.
    pub line: Option<u64>,
}

impl Finding {
    /// The finding as upstream's `JSON.stringify` renders it, then `code`, `keyword` and `line`.
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
            write!(out, "{v}").unwrap();
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
            write!(out, "{v}").unwrap();
        }
        out.push('}');
        out
    }
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
            c if c < ' ' => write!(out, "\\u{:04x}", c as u32).unwrap(),
            c => out.push(c),
        }
    }
    out.push('"');
}

macro_rules! codes {
    ($($variant:ident = $code:literal,)*) => {
        /// Stable finding codes. Renaming one is a breaking change; rewording a message is not.
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
        #[non_exhaustive]
        pub enum Code {
            $($variant,)*
        }

        impl Code {
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
        // `.+` matches anything but line terminators
        Some(!middle.is_empty() && !middle.contains(['\n', '\r', '\u{2028}', '\u{2029}']))
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
