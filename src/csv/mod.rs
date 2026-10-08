//! The CSV branch: upstream's row phase machine, the header rows and the column analysis of
//! row 3, which hands the shape-specific parts to the `tall` and `wide` leaves.

mod reader;
mod rules;
mod tall;
mod wide;

use crate::{
    Code, FileType, Finding, Options, Report, VERSIONS, Version, add, full, js, version_mismatch,
};
use reader::Reader;
use rules::{Amounts, Group, Layout};
use std::collections::{HashMap, VecDeque};
use std::io::{self, Read};

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

/// A charge group's charge columns.
const CHARGES: [&str; 4] =
    ["negotiated_dollar", "negotiated_percentage", "negotiated_algorithm", "methodology"];

#[expect(clippy::too_many_lines, reason = "upstream's phase machine, kept in one place")]
pub fn validate(input: impl Read, version: &str, options: &Options) -> io::Result<Report> {
    let max = options.max_errors;
    let Some(v) = Version::parse(version) else {
        let message =
            format!("Invalid version supplied. Allowed versions are: {}", VERSIONS.join(", "));
        let error = Finding {
            versions: Some(VERSIONS),
            ..at(Code::CsvInvalidVersion, 0, Some(0), message)
        };
        return Ok(Report {
            errors: vec![error],
            alerts: Vec::new(),
            file_type: Some(FileType::Unknown),
            extended: Vec::new(),
        });
    };

    let mut reader = Reader::new(input);
    let (mut alerts, mut extended) = (Vec::new(), Vec::new());
    let mut names = Vec::new();
    let mut file_type = FileType::Unknown;
    let mut layout = None;
    let mut index = 0;
    // The last row's cells, emptied: its buffer outlives the row it borrowed from.
    let mut spare = Vec::new();
    let errors = 'rows: {
        let mut errors = Vec::new();
        while let Some(row) = reader.next_row()? {
            let line = row.line;
            // Every finding gets the physical line of the row it was found on.
            let lined = move |found: Vec<Finding>| {
                found.into_iter().map(move |f| Finding { line: Some(line), ..f })
            };
            // Upstream never reads papaparse's quote errors.
            for &(code, column) in &row.quotes {
                let message = if code == Code::XCsvMissingQuotes {
                    "A quoted field is never closed, so the rest of the file was read as this one field."
                } else {
                    "A quote in a quoted field is neither doubled nor followed by a comma or a line end, so the field runs on to the next closing quote and may have taken in the fields and lines after it."
                };
                let finding = at(code, index, Some(column), message.into());
                add(&mut extended, lined(vec![finding]), max);
            }
            let mut cells = recycle(std::mem::take(&mut spare));
            cells.extend(row.fields().map(js::trim));
            let row = cells;
            let blank = row.iter().all(|f| f.is_empty());
            if blank && (index == 0 || index == 2) {
                // Upstream drops the errors found so far and keeps the alerts; Extended
                // keeps the errors.
                add(&mut extended, errors, max);
                let message = format!(
                    "Required headers must be defined on rows 1 and 3. Row {} is blank",
                    index + 1
                );
                let error = at(Code::CsvHeaderBlank, index, Some(0), message);
                break 'rows lined(vec![error]).collect();
            }
            if blank && index != 1 {
                index += 1;
                spare = recycle(row);
                continue;
            }
            match index {
                0 => names = row.iter().map(ToString::to_string).collect(),
                1 => {
                    let (found, header_errors) = header(v, &names, &row);
                    // A header error suppresses every header alert.
                    if header_errors.is_empty() {
                        add(&mut alerts, lined(header_alerts(v, &found, &row)), max);
                    }
                    add(&mut errors, lined(header_errors), max);
                }
                2 => {
                    let (shape, result) = columns(v, &row);
                    file_type = shape;
                    match result {
                        Ok((resolved, column_alerts)) => {
                            add(&mut alerts, lined(column_alerts), max);
                            layout = Some(resolved);
                        }
                        Err(column_errors) => {
                            add(&mut errors, lined(column_errors), max);
                            // Added after the limit is applied, so it can exceed it by one.
                            let message = "Errors were found in the headers in row 3, so the remaining rows were not evaluated.";
                            let error = at(Code::CsvProblemsInHeader, 0, Some(0), message.into());
                            errors.extend(lined(vec![error]));
                            break 'rows errors;
                        }
                    }
                }
                _ => {
                    let layout = layout.as_ref().expect("data rows follow a valid row 3");
                    add(&mut errors, lined(rules::errors(layout, index, &row)), max);
                    if !full(&alerts, max) {
                        add(&mut alerts, lined(rules::alerts(layout, index, &row)), max);
                    }
                }
            }
            if full(&errors, max) {
                break 'rows errors;
            }
            index += 1;
            spare = recycle(row);
        }
        if index < 4 {
            // Upstream drops the errors found so far and keeps the alerts; Extended keeps
            // the errors.
            add(&mut extended, errors, max);
            let message = "At least one row must be present".into();
            break 'rows vec![at(Code::CsvMinRows, 0, Some(0), message)];
        }
        errors
    };
    // Found first, reported first.
    if reader.bom_lines > 0 {
        let n = reader.bom_lines;
        let message = format!(
            "The byte order mark was removed together with the {n} blank line{} after it, so row numbers are {n} lower than physical line numbers.",
            if n == 1 { "" } else { "s" }
        );
        let finding =
            Finding { line: Some(1), ..at(Code::XCsvBomSwallowedLines, 0, Some(0), message) };
        extended.insert(0, finding);
        if max > 0 {
            extended.truncate(max);
        }
    }
    Ok(Report { errors, alerts, file_type: Some(file_type), extended })
}

/// `cells` emptied, its buffer kept for cells that borrow from another row. Collecting an
/// emptied `Vec` into one of the same element size reuses its buffer.
fn recycle<'b>(mut cells: Vec<&str>) -> Vec<&'b str> {
    cells.clear();
    cells.into_iter().map(|_| "").collect()
}

/// Rows 1 and 2, upstream's `validateHeaderColumns` then `validateHeaderRow`: the recognised
/// row 1 names by position, and the errors.
fn header<'a>(
    v: Version,
    names: &'a [String],
    values: &[&str],
) -> (Vec<Option<&'a str>>, Vec<Finding>) {
    let mut remaining =
        vec!["hospital_name", "last_updated_on", "version", "hospital_address", LICENSE];
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
            let split: Vec<&str> = parts(name).collect();
            let [license, state] = split[..] else {
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
            let problem = "is not in a valid format. You must encode the date using the ISO 8601 format: YYYY-MM-DD or the month/day/year format: MM/DD/YYYY, M/D/YYYY";
            errors.push(invalid_value(Code::CsvInvalidDate, 1, i, name, value, problem));
        } else if (sep_eq(name, AFFIRMATION) || sep_eq(name, ATTESTATION))
            && !TRUE_FALSE.iter().any(|t| same(value, t))
        {
            errors.push(allowed_values(1, i, name, value, TRUE_FALSE));
        }
    }
    (found, errors)
}

/// Upstream skips the value check for `/^license_number\s*\|\s*.{2}$/`, case-sensitive
/// unlike the header match; `.` is a UTF-16 unit other than a line terminator.
fn is_license_skipped(name: &str) -> bool {
    let Some(rest) = name.strip_prefix("license_number") else {
        return false;
    };
    let Some(rest) = js::trim_start(rest).strip_prefix('|') else {
        return false;
    };
    let units: Vec<u16> = rest.encode_utf16().collect();
    let Some((head, tail)) = units.split_last_chunk::<2>() else {
        return false;
    };
    char::decode_utf16(head.iter().copied()).all(|c| c.is_ok_and(js::is_space))
        && char::decode_utf16(tail.iter().copied()).all(|c| !c.is_ok_and(js::is_line_terminator))
}

/// Upstream's `alertHeaderRow`, anchored at row 3 although the values are in row 2.
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
        let message = version_mismatch(value(i), &[expected]);
        alerts.push(at(Code::CsvVersionMismatch, 2, Some(i), message));
    }
    let (statement, code, message) = if v == Version::V3_0 {
        (ATTESTATION, Code::CsvFalseAttestation, "Attestation value is false.")
    } else {
        (AFFIRMATION, Code::CsvFalseAffirmation, "Affirmation value is false.")
    };
    if let Some(i) = find(statement)
        && same(value(i), "false")
    {
        alerts.push(at(code, 2, Some(i), message.into()));
    }
    alerts
}

/// Row 3's outcome: the columns resolved for the data rows with the alerts, or the errors.
type Columns = Result<(Layout, Vec<Finding>), Vec<Finding>>;

/// Row 3, upstream's `validateColumns` and `alertColumns`.
#[expect(clippy::too_many_lines, reason = "builds upstream's expected column list in its order")]
fn columns(v: Version, row: &[&str]) -> (FileType, Columns) {
    let tall = tall::detect(row);
    let (payers_plans, mut errors) = wide::payers_plans(v, row);
    let mut codes = 0;
    for (i, column) in row.iter().enumerate() {
        let Some(n) = code_index(column) else {
            continue;
        };
        let digits: String = n.chars().filter(char::is_ascii_digit).collect();
        // ponytail: unbounded like upstream, which throws above 2^32 - 1 code pairs;
        // bound it with the fuzzing in roadmap Phase 7
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "digits only: NaN or a non-negative integer, saturated"
        )]
        let count = js::number(&digits) as u64;
        codes = codes.max(count);
        if v == Version::V3_0 && same(n, "[i]") {
            errors.push(placeholder(i, column));
        }
    }
    if tall != payers_plans.is_empty() {
        let message = "Required payer-specific information data element headers are missing or miscoded from the MRF that does not follow the specifications for the CSV \"Tall\" or CSV \"Wide\" format.";
        errors.push(at(Code::CsvAmbiguousFormat, 2, None, message.into()));
        return (FileType::Unknown, Err(errors));
    }
    // Group column names end in the group's suffix: none for tall's one group, the
    // payer/plan for each wide one.
    let (file_type, suffixes) = if tall {
        (FileType::Tall, vec![String::new()])
    } else {
        let suffixes = payers_plans.iter().map(|pp| format!(" | {pp}"));
        (FileType::Wide, suffixes.collect())
    };

    // upstream's getExpectedDataColumns, each column's slot kept for the data rows
    let mut expected = Vec::new();
    let mut want = |name: String| {
        expected.push(name);
        expected.len() - 1
    };
    let [description, setting, gross, discounted, min, max, notes] =
        SHARED_COLUMNS.map(|c| want(c.into()));
    let code_pairs = (1..=codes.max(1))
        .map(|i| (want(format!("code | {i}")), want(format!("code | {i} | type"))))
        .collect();
    let payer_plan = tall.then(|| tall::COLUMNS.map(|c| want(c.into())));
    let mut groups: Vec<Group> = (suffixes.iter())
        .map(|g| {
            let [dollar, percentage, algorithm, methodology] =
                CHARGES.map(|c| want(format!("standard_charge{g} | {c}")));
            Group {
                dollar,
                percentage,
                algorithm,
                methodology,
                // Tall's notes are the shared additional_generic_notes.
                notes: if tall { notes } else { want(format!("additional_payer_notes{g}")) },
                payer_plan,
                amounts: Amounts::None,
            }
        })
        .collect();
    let v2_2 = v >= Version::V2_2;
    let drug = v2_2
        .then(|| ["drug_unit_of_measurement", "drug_type_of_measurement"].map(|c| want(c.into())));
    let modifiers = v2_2.then(|| want("modifiers".into()));
    for (group, g) in groups.iter_mut().zip(&suffixes) {
        let amount_slots: Vec<usize> = amounts(v).iter().map(|a| want(format!("{a}{g}"))).collect();
        group.amounts = match amount_slots[..] {
            [estimated] => Amounts::Estimated(estimated),
            [median, p10, p90, count] => Amounts::Allowed([median, p10, p90, count]),
            _ => Amounts::None,
        };
    }

    // Each key's expected positions, earliest first: names can share a key once the
    // lower-cased payer and plan names are upper-cased again.
    let mut slots: HashMap<String, VecDeque<usize>> = HashMap::new();
    for (j, name) in expected.iter().enumerate() {
        slots.entry(key(name)).or_default().push_back(j);
    }
    // The row 3 column each expected one matched.
    let mut matched = vec![None; expected.len()];
    let keys: Vec<String> = row.iter().map(|c| key(c)).collect();
    // The first column with each key, recognised or not.
    let mut seen: HashMap<&str, usize> = HashMap::new();
    for (i, k) in keys.iter().enumerate() {
        if row[i].is_empty() {
            continue;
        }
        if let Some(j) = slots.get_mut(k).and_then(VecDeque::pop_front) {
            matched[j] = Some(i);
        } else if seen.contains_key(k.as_str()) {
            // Recognised or not: two identical unknown columns abort the run too.
            let message = format!(
                "Column {} duplicated in header. You must review and revise your column headers so that each header appears only once in the third row.",
                row[i]
            );
            errors.push(Finding {
                column_name: Some(row[i].into()),
                ..at(Code::CsvDuplicateColumn, 2, Some(i), message)
            });
            continue;
        }
        seen.entry(k).or_insert(i);
    }
    for (name, matched) in expected.into_iter().zip(&matched) {
        if matched.is_some() {
            continue;
        }
        let message = format!(
            "Column {name} is miscoded or missing from row 3. You must include this column and confirm that it is encoded as specified in the data dictionary."
        );
        errors.push(Finding {
            column_name: Some(name),
            ..at(Code::CsvColumnMissing, 2, None, message)
        });
    }
    if !errors.is_empty() {
        return (file_type, Err(errors));
    }

    let mut alerts = Vec::new();
    if v == Version::V3_0 {
        for g in &suffixes {
            if let Some(&i) = seen.get(key(&format!("estimated_amount{g}")).as_str()) {
                let message = format!(
                    "Column {} found in column headers. The 'estimated amount' is no longer a required data element as of January 1, 2026. It has been replaced by the 10th percentile, median, 90th percentile, and count of allowed amounts.",
                    row[i]
                );
                alerts.push(at(Code::CsvEstimatedAmountDeprecated, 2, Some(i), message));
            }
        }
    }

    let columns: Vec<usize> =
        (matched.into_iter().collect::<Option<_>>()).expect("no error, so every slot matched");
    let layout = Layout {
        v,
        names: columns.iter().map(|&i| row[i].to_string()).collect(),
        columns,
        description,
        setting,
        codes: code_pairs,
        standard: [gross, discounted, min, max],
        notes,
        groups,
        drug,
        modifiers,
    };
    (file_type, Ok((layout, alerts)))
}

/// Per-group amount columns: `estimated_amount` only in 2.2, replaced by four in 3.0.
fn amounts(v: Version) -> &'static [&'static str] {
    match v {
        Version::V2_2 => &["estimated_amount"],
        Version::V3_0 => &["median_amount", "10th_percentile", "90th_percentile", "count"],
        _ => &[],
    }
}

/// The index part of a `code | N` or `code | N | type` column, as upstream's
/// `getCodeCount` splits it: trimmed parts, empty ones dropped.
fn code_index(column: &str) -> Option<&str> {
    let parts: Vec<&str> = parts(column).filter(|p| !p.is_empty()).collect();
    match parts[..] {
        [code, n] if same(code, "code") => Some(n),
        [code, n, ty] if same(code, "code") && same(ty, "type") => Some(n),
        _ => None,
    }
}

/// A column name's `|`-separated parts, trimmed, as upstream splits names everywhere.
fn parts(name: &str) -> impl Iterator<Item = &str> {
    name.split('|').map(js::trim)
}

/// Upstream's `matchesString`.
fn same(a: &str, b: &str) -> bool {
    if a.is_ascii() && b.is_ascii() {
        return a.eq_ignore_ascii_case(b);
    }
    js::to_upper(a) == js::to_upper(b)
}

/// Upstream's `sepColumnsEqual` holds exactly when the keys are equal: parts trimmed and
/// upper-cased, and no upper-cased part contains `|`.
fn key(name: &str) -> String {
    parts(name).map(js::to_upper).collect::<Vec<_>>().join("|")
}

fn sep_eq(a: &str, b: &str) -> bool {
    key(a) == key(b)
}

/// Upstream's `csvCellName`: `"C4"` for a column, `"row 4"` without one.
fn cell(row: u64, column: Option<usize>) -> String {
    let Some(mut c) = column else {
        return format!("row {}", row + 1);
    };
    let mut name = String::new();
    loop {
        name.insert(0, char::from(b"ABCDEFGHIJKLMNOPQRSTUVWXYZ"[c % 26]));
        if c < 26 {
            break;
        }
        c = c / 26 - 1;
    }
    format!("{name}{}", row + 1)
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

/// A finding about a cell's value: `"name" value "value" {problem}`.
fn invalid_value(
    code: Code,
    row: u64,
    column: usize,
    name: &str,
    value: &str,
    problem: &str,
) -> Finding {
    let message = format!("\"{name}\" value \"{value}\" {problem}");
    Finding {
        column_name: Some(name.into()),
        value: Some(value.into()),
        ..at(code, row, Some(column), message)
    }
}

fn allowed_values(
    row: u64,
    column: usize,
    name: &str,
    value: &str,
    allowed: &'static [&'static str],
) -> Finding {
    let problem = format!(
        "is not one of the allowed valid values. You must encode one of these valid values: {}",
        allowed.join(", ")
    );
    Finding {
        allowed_values: Some(allowed),
        ..invalid_value(Code::CsvAllowedValues, row, column, name, value, &problem)
    }
}

fn placeholder(column: usize, name: &str) -> Finding {
    let message = format!(
        "Column header {name} contains a placeholder. All placeholders must be removed from column headers."
    );
    Finding {
        column_name: Some(name.into()),
        column: i64::try_from(column).ok(),
        ..at(Code::CsvPlaceholder, 2, Some(column), message)
    }
}
