//! The CSV branch: upstream's row phase machine, the header rows and the column analysis of
//! row 3, which hands the shape-specific parts to the `tall` and `wide` leaves.

mod reader;
mod tall;
mod wide;

use crate::{Code, FileType, Finding, Mode, Options, Report, js};
use reader::Reader;
use std::io::{self, Read};

/// Data dictionary versions upstream accepts for CSV, after `semver.coerce`.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Version {
    V2_0,
    V2_1,
    V2_2,
    V3_0,
}

const VERSIONS: &[&str] = &["2.0.0", "2.1.0", "2.2.0", "3.0.0"];

const AFFIRMATION: &str = "To the best of its knowledge and belief, the hospital has included all applicable standard charge information in accordance with the requirements of 45 CFR 180.50, and the information encoded is true, accurate, and complete as of the date indicated.";

const ATTESTATION: &str = "To the best of its knowledge and belief, this hospital has included all applicable standard charge information in accordance with the requirements of 45 CFR 180.50, and the information encoded is true, accurate, and complete as of the date in the file. This hospital has included all payer-specific negotiated charges in dollars that can be expressed as a dollar amount. For payer-specific negotiated charges that cannot be expressed as a dollar amount in the machine-readable file or not knowable in advance, the hospital attests that the payer-specific negotiated charge is based on a contractual algorithm, percentage or formula that precludes the provision of a dollar amount and has provided all necessary information available to the hospital for the public to be able to derive the dollar amount, including, but not limited to, the specific fee schedule or components referenced in such percentage, algorithm or formula.";

const LICENSE: &str = "license_number | [state]";

const STATE_CODES: &[&str] = &[
    "AL", "AK", "AS", "AZ", "AR", "CA", "CO", "CT", "DE", "DC", "FL", "GA", "GU", "HI", "ID", "IL",
    "IN", "IA", "KS", "KY", "LA", "ME", "MD", "MA", "MI", "MN", "MS", "MO", "MT", "NE", "NV", "NH",
    "NJ", "NM", "NY", "NC", "ND", "MP", "OH", "OK", "OR", "PA", "PR", "RI", "SC", "SD", "TN", "TX",
    "UT", "VT", "VI", "VA", "WA", "WV", "WI", "WY",
];

const TRUE_FALSE: &[&str] = &["true", "false"];

/// Row 3 columns every file has, before the code pairs.
const SHARED_COLUMNS: [&str; 7] = [
    "description",
    "setting",
    "standard_charge | gross",
    "standard_charge | discounted_cash",
    "standard_charge | min",
    "standard_charge | max",
    "additional_generic_notes",
];

enum Shape {
    Tall,
    /// Payer/plan keys, lower-cased, in discovery order.
    Wide(Vec<String>),
}

pub fn validate(input: impl Read, version: &str, options: &Options) -> io::Result<Report> {
    let max = options.max_errors;
    let done = |errors, alerts, file_type| Report {
        errors,
        alerts,
        file_type: (options.mode == Mode::Extended).then_some(file_type),
        extended: Vec::new(),
    };
    let coerced = js::coerce(version);
    let Some(v) = VERSIONS.iter().position(|&v| v == coerced) else {
        let message = format!(
            "Invalid version supplied. Allowed versions are: {}",
            VERSIONS.join(", ")
        );
        let error = Finding {
            versions: Some(VERSIONS),
            ..at(Code::CsvInvalidVersion, 0, Some(0), message)
        };
        return Ok(done(vec![error], Vec::new(), FileType::Unknown));
    };
    let v = [Version::V2_0, Version::V2_1, Version::V2_2, Version::V3_0][v];

    let mut reader = Reader::new(input);
    let (mut errors, mut alerts) = (Vec::new(), Vec::new());
    let mut names = Vec::new();
    let mut file_type = FileType::Unknown;
    let mut index = 0;
    while let Some(row) = reader.next_row()? {
        let row: Vec<&str> = row.fields.iter().map(|f| js::trim(f)).collect();
        let blank = row.iter().all(|f| f.is_empty());
        if blank && (index == 0 || index == 2) {
            // The errors found so far are dropped, the alerts kept (F-15).
            let message = format!(
                "Required headers must be defined on rows 1 and 3. Row {} is blank",
                index + 1
            );
            let error = at(Code::CsvHeaderBlank, index, Some(0), message);
            return Ok(done(vec![error], alerts, file_type));
        }
        if blank && index != 1 {
            index += 1;
            continue;
        }
        match index {
            0 => names = row.iter().map(|f| f.to_string()).collect(),
            1 => {
                let (found, header_errors) = header(v, &names, &row);
                // A header error suppresses every header alert (F-16).
                let clean = header_errors.is_empty();
                add(&mut errors, header_errors, max);
                if clean {
                    add(&mut alerts, header_alerts(v, &found, &row), max);
                }
            }
            2 => {
                let (shape, column_errors) = columns(v, &row);
                file_type = match shape {
                    Some(Shape::Tall) => FileType::Tall,
                    Some(Shape::Wide(_)) => FileType::Wide,
                    None => FileType::Unknown,
                };
                if !column_errors.is_empty() {
                    add(&mut errors, column_errors, max);
                    // Added after the limit is applied (F-14).
                    let message = "Errors were found in the headers in row 3, so the remaining rows were not evaluated.";
                    errors.push(at(Code::CsvProblemsInHeader, 0, Some(0), message.into()));
                    return Ok(done(errors, alerts, file_type));
                }
                if v == Version::V3_0 {
                    let column_alerts = match &shape {
                        Some(Shape::Wide(payers_plans)) => wide::alerts(&row, payers_plans),
                        _ => tall::alerts(&row),
                    };
                    add(&mut alerts, column_alerts, max);
                }
            }
            // Data rows: roadmap Phase 3.
            _ => {}
        }
        if max > 0 && errors.len() >= max {
            return Ok(done(errors, alerts, file_type));
        }
        index += 1;
    }
    if index < 4 {
        // The errors found so far are dropped, the alerts kept (F-20).
        let message = "At least one row must be present".into();
        let error = at(Code::CsvMinRows, 0, Some(0), message);
        return Ok(done(vec![error], alerts, file_type));
    }
    Ok(done(errors, alerts, file_type))
}

/// Upstream's `addItemsWithLimit`: `max` 0 is unlimited.
fn add(list: &mut Vec<Finding>, items: Vec<Finding>, max: usize) {
    let room = if max > 0 {
        max.saturating_sub(list.len())
    } else {
        usize::MAX
    };
    list.extend(items.into_iter().take(room));
}

/// Rows 1 and 2, upstream's `validateHeaderColumns` then `validateHeaderRow`: the recognised
/// row 1 names by position, and the errors.
fn header<'a>(
    v: Version,
    names: &'a [String],
    values: &[&str],
) -> (Vec<Option<&'a str>>, Vec<Finding>) {
    let mut remaining = vec![
        "hospital_name",
        "last_updated_on",
        "version",
        "hospital_address",
        LICENSE,
    ];
    if v == Version::V3_0 {
        remaining.extend(["location_name", "type_2_npi", ATTESTATION, "attester_name"]);
    } else {
        remaining.extend(["hospital_location", AFFIRMATION]);
    }
    let mut found = vec![None; names.len()];
    let mut errors = Vec::new();
    for (i, name) in names.iter().enumerate() {
        if name.is_empty() {
            continue;
        }
        let matched = remaining.iter().position(|&expected| {
            if expected != LICENSE {
                return sep_eq(name, expected);
            }
            let parts: Vec<&str> = name.split('|').map(js::trim).collect();
            let [license, state] = parts[..] else {
                return false;
            };
            if !sep_eq(license, "license_number") {
                return false;
            }
            if STATE_CODES.contains(&js::to_upper(state).as_str()) {
                return true;
            }
            let message = format!(
                "{state} is not an allowed value for state abbreviation. You must fill in the state or territory abbreviation even if there is no license number to encode. See the table found here for the list of valid values for state and territory abbreviations https://github.com/CMSgov/hospital-price-transparency/blob/master/documentation/CSV/state_codes.md"
            );
            errors.push(at(Code::CsvInvalidStateCode, 0, Some(i), message));
            false
        });
        if let Some(m) = matched {
            found[i] = Some(name.as_str());
            remaining.remove(m);
        } else if found.iter().flatten().any(|f| sep_eq(f, name)) {
            let message = format!(
                "Column {name} duplicated in header. You must review and revise your column headers so that each header appears only once in the first row."
            );
            errors.push(Finding {
                column_name: Some(name.clone()),
                ..at(Code::CsvDuplicateHeaderColumn, 0, Some(i), message)
            });
        }
    }
    for name in remaining {
        let message = format!(
            "Header column \"{name}\" is miscoded or missing. You must include this header and confirm that it is encoded as specified in the data dictionary."
        );
        errors.push(Finding {
            column_name: Some(name.into()),
            ..at(Code::CsvHeaderColumnMissing, 0, None, message)
        });
    }

    for (i, name) in found.iter().enumerate() {
        let Some(name) = *name else { continue };
        if is_license_skipped(name) {
            continue;
        }
        let value = values.get(i).copied().unwrap_or("");
        if value.is_empty() {
            errors.push(required_value(1, i, name, ""));
        } else if sep_eq(name, "last_updated_on") && !js::is_valid_date(value) {
            let message = format!(
                "\"{name}\" value \"{value}\" is not in a valid format. You must encode the date using the ISO 8601 format: YYYY-MM-DD or the month/day/year format: MM/DD/YYYY, M/D/YYYY"
            );
            errors.push(Finding {
                column_name: Some(name.into()),
                value: Some(value.into()),
                ..at(Code::CsvInvalidDate, 1, Some(i), message)
            });
        } else if (sep_eq(name, AFFIRMATION) || sep_eq(name, ATTESTATION))
            && !TRUE_FALSE.iter().any(|t| same(value, t))
        {
            errors.push(allowed_values(1, i, name, value, TRUE_FALSE));
        }
    }
    (found, errors)
}

/// Upstream skips the value check for `/^license_number\s*\|\s*.{2}$/`, case-sensitive
/// unlike the header match (F-25); `.` is a UTF-16 unit other than a line terminator.
fn is_license_skipped(name: &str) -> bool {
    let Some(rest) = name.strip_prefix("license_number") else {
        return false;
    };
    let Some(rest) = js::trim_start(rest).strip_prefix('|') else {
        return false;
    };
    let (mut units, mut cut) = (0, rest.len());
    for (i, c) in rest.char_indices().rev() {
        if units >= 2 {
            break;
        }
        units += c.len_utf16();
        cut = i;
    }
    units == 2
        && rest[..cut].chars().all(js::is_space)
        && !rest[cut..].contains(['\n', '\r', '\u{2028}', '\u{2029}'])
}

/// Upstream's `alertHeaderRow`, anchored at row 3 (F-23).
fn header_alerts(v: Version, found: &[Option<&str>], values: &[&str]) -> Vec<Finding> {
    let find = |name| found.iter().position(|f| f.is_some_and(|f| same(f, name)));
    let value = |i: usize| values.get(i).copied().unwrap_or("");
    let mut alerts = Vec::new();
    let expected = match v {
        Version::V3_0 => Some("3.0.0"),
        Version::V2_2 | Version::V2_1 => Some("2.0.0"),
        Version::V2_0 => None,
    };
    if let Some(i) = find("version")
        && let Some(expected) = expected
        && value(i) != expected
    {
        let message = format!(
            "The value in this MRF's version data element \"{}\" does not match the expected template version \"{expected}\" for the data dictionary requirements selected for validation.",
            value(i)
        );
        alerts.push(at(Code::CsvVersionMismatch, 2, Some(i), message));
    }
    let (statement, code, message) = if v == Version::V3_0 {
        (
            ATTESTATION,
            Code::CsvFalseAttestation,
            "Attestation value is false.",
        )
    } else {
        (
            AFFIRMATION,
            Code::CsvFalseAffirmation,
            "Affirmation value is false.",
        )
    };
    if let Some(i) = find(statement)
        && same(value(i), "false")
    {
        alerts.push(at(code, 2, Some(i), message.into()));
    }
    alerts
}

/// Row 3, upstream's `validateColumns`: the shape, unless the columns fit both or neither,
/// and the errors.
fn columns(v: Version, row: &[&str]) -> (Option<Shape>, Vec<Finding>) {
    let tall = tall::detect(row);
    let mut payers_plans = wide::payers_plans(v, row);
    let mut errors = Vec::new();
    if v == Version::V3_0 {
        errors.extend(wide::placeholders(v, row));
        payers_plans.retain(|pp| !wide::is_placeholder(pp));
    }
    let codes = row
        .iter()
        .filter_map(|c| code_index(c))
        .map(|n| {
            let digits: String = n.chars().filter(char::is_ascii_digit).collect();
            // ponytail: unbounded like upstream, which throws above 2^32 - 1 code pairs;
            // bound it with the fuzzing in roadmap Phase 7
            if digits.is_empty() {
                0
            } else {
                digits.parse().unwrap_or(u64::MAX)
            }
        })
        .max()
        .unwrap_or(0);
    if v == Version::V3_0 {
        for (i, c) in row.iter().enumerate() {
            if code_index(c).is_some_and(|n| same(n, "[i]")) {
                errors.push(placeholder(i, c));
            }
        }
    }
    if tall == !payers_plans.is_empty() {
        let message = "Required payer-specific information data element headers are missing or miscoded from the MRF that does not follow the specifications for the CSV \"Tall\" or CSV \"Wide\" format.";
        errors.push(at(Code::CsvAmbiguousFormat, 2, None, message.into()));
        return (None, errors);
    }
    let shape = if tall {
        Shape::Tall
    } else {
        Shape::Wide(payers_plans)
    };

    // upstream's getExpectedDataColumns
    let mut expected: Vec<String> = SHARED_COLUMNS.map(String::from).into();
    for i in 1..=codes.max(1) {
        expected.push(format!("code | {i}"));
        expected.push(format!("code | {i} | type"));
    }
    match &shape {
        Shape::Tall => expected.extend(tall::COLUMNS.map(String::from)),
        Shape::Wide(payers_plans) => expected.extend(wide::columns(payers_plans)),
    }
    if v >= Version::V2_2 {
        expected.extend(
            [
                "drug_unit_of_measurement",
                "drug_type_of_measurement",
                "modifiers",
            ]
            .map(String::from),
        );
    }
    match &shape {
        Shape::Tall => expected.extend(amounts(v).iter().map(|a| a.to_string())),
        Shape::Wide(payers_plans) => expected.extend(wide::amount_columns(v, payers_plans)),
    }

    let mut keys: Vec<String> = expected.iter().map(|c| key(c)).collect();
    let mut seen = Vec::new();
    for (i, &column) in row.iter().enumerate() {
        if column.is_empty() {
            continue;
        }
        let k = key(column);
        if let Some(m) = keys.iter().position(|e| *e == k) {
            keys.remove(m);
            expected.remove(m);
        } else if seen.contains(&k) {
            // Recognised or not: two identical unknown columns abort the run too.
            let message = format!(
                "Column {column} duplicated in header. You must review and revise your column headers so that each header appears only once in the third row."
            );
            errors.push(Finding {
                column_name: Some(column.into()),
                ..at(Code::CsvDuplicateColumn, 2, Some(i), message)
            });
            continue;
        }
        seen.push(k);
    }
    for name in expected {
        let message = format!(
            "Column {name} is miscoded or missing from row 3. You must include this column and confirm that it is encoded as specified in the data dictionary."
        );
        errors.push(Finding {
            column_name: Some(name),
            ..at(Code::CsvColumnMissing, 2, None, message)
        });
    }
    (Some(shape), errors)
}

/// Per-group amount columns: `estimated_amount` only in 2.2, replaced by four in 3.0.
fn amounts(v: Version) -> &'static [&'static str] {
    match v {
        Version::V2_2 => &["estimated_amount"],
        Version::V3_0 => &[
            "median_amount",
            "10th_percentile",
            "90th_percentile",
            "count",
        ],
        _ => &[],
    }
}

/// The index part of a `code | N` or `code | N | type` column, as upstream's
/// `getCodeCount` splits it: trimmed parts, empty ones dropped.
fn code_index(column: &str) -> Option<&str> {
    let parts: Vec<&str> = column
        .split('|')
        .map(js::trim)
        .filter(|p| !p.is_empty())
        .collect();
    match parts[..] {
        [code, n] if same(code, "code") => Some(n),
        [code, n, ty] if same(code, "code") && same(ty, "type") => Some(n),
        _ => None,
    }
}

/// Upstream's `matchesString`.
fn same(a: &str, b: &str) -> bool {
    js::to_upper(a) == js::to_upper(b)
}

/// Upstream's `sepColumnsEqual` holds exactly when the keys are equal: parts trimmed and
/// upper-cased, and no upper-cased part contains `|`.
fn key(name: &str) -> String {
    let parts: Vec<String> = name.split('|').map(|p| js::to_upper(js::trim(p))).collect();
    parts.join("|")
}

fn sep_eq(a: &str, b: &str) -> bool {
    key(a) == key(b)
}

/// Upstream's `csvCellName`: `"C4"` for a column, `"row 4"` without one.
fn cell(row: u64, column: Option<usize>) -> String {
    let Some(mut c) = column else {
        return format!("row {}", row + 1);
    };
    let mut letters = Vec::new();
    loop {
        letters.push(b'A' + (c % 26) as u8);
        if c < 26 {
            break;
        }
        c = c / 26 - 1;
    }
    letters.reverse();
    format!("{}{}", String::from_utf8(letters).unwrap(), row + 1)
}

fn at(code: Code, row: u64, column: Option<usize>, message: String) -> Finding {
    Finding::new(code, cell(row, column), message)
}

fn required_value(row: u64, column: usize, name: &str, suffix: &str) -> Finding {
    let message = format!(
        "A value is required for \"{name}\"{suffix}. You must encode the missing information."
    );
    Finding {
        column_name: Some(name.into()),
        ..at(Code::CsvRequiredValue, row, Some(column), message)
    }
}

fn allowed_values(
    row: u64,
    column: usize,
    name: &str,
    value: &str,
    allowed: &'static [&'static str],
) -> Finding {
    let message = format!(
        "\"{name}\" value \"{value}\" is not one of the allowed valid values. You must encode one of these valid values: {}",
        allowed.join(", ")
    );
    Finding {
        column_name: Some(name.into()),
        value: Some(value.into()),
        allowed_values: Some(allowed),
        ..at(Code::CsvAllowedValues, row, Some(column), message)
    }
}

fn placeholder(column: usize, name: &str) -> Finding {
    let message = format!(
        "Column header {name} contains a placeholder. All placeholders must be removed from column headers."
    );
    Finding {
        column_name: Some(name.into()),
        column: Some(column as i64),
        ..at(Code::CsvPlaceholder, 2, Some(column), message)
    }
}

fn estimated_amount(column: usize, name: &str) -> Finding {
    let message = format!(
        "Column {name} found in column headers. The 'estimated amount' is no longer a required data element as of January 1, 2026. It has been replaced by the 10th percentile, median, 90th percentile, and count of allowed amounts."
    );
    at(Code::CsvEstimatedAmountDeprecated, 2, Some(column), message)
}
