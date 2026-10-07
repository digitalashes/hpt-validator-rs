//! The JSON branch: upstream's `JsonValidator`. Charge items are validated one at a time as
//! the parser completes them; the metadata members are collected and validated at the end.

mod ajv;
mod parse;

use crate::{Code, Error, FileType, Finding, Options, Report, Version, add, full};
use parse::{End, Key, Value};
use std::io::Read;
use std::ops::ControlFlow;
use std::sync::OnceLock;

/// The parser's paths: the metadata members, then each charge item.
const PATHS: &[&[&str]] = &[
    &["hospital_name"],
    &["last_updated_on"],
    &["license_information"],
    &["version"],
    &["hospital_address"],
    &["hospital_location"],
    &["location_name"],
    &["affirmation"],
    &["attestation"],
    &["modifier_information"],
    &["type_2_npi"],
    &[CHARGES, "*"],
];

const CHARGES: &str = "standard_charge_information";

struct Schemas {
    full: Value,
    /// The full schema without the charge items (`buildMetadataSchema`, minus F-7).
    metadata: Value,
    alerts: Option<Value>,
}

fn schemas(v: Version) -> &'static Schemas {
    static SCHEMAS: [OnceLock<Schemas>; 4] = [const { OnceLock::new() }; 4];
    SCHEMAS[v as usize].get_or_init(|| {
        let (full, alerts) = match v {
            Version::V2_0 => (include_str!("../../schemas/v2.0.0.json"), None),
            Version::V2_1 => (include_str!("../../schemas/v2.1.0.json"), None),
            Version::V2_2 => (
                include_str!("../../schemas/v2.2.0.json"),
                Some(include_str!("../../schemas/alerts/v2.2.0.json")),
            ),
            Version::V3_0 => (
                include_str!("../../schemas/v3.0.0.json"),
                Some(include_str!("../../schemas/alerts/v3.0.0.json")),
            ),
        };
        let full = parse::document(full);
        let mut metadata = full.clone();
        if let Value::Object(members) = &mut metadata {
            for (name, value) in members {
                match (name.as_str(), value) {
                    ("properties", Value::Object(p)) => p.retain(|(k, _)| k != CHARGES),
                    ("required", Value::Array(r)) => {
                        r.retain(|k| *k != Value::String(CHARGES.into()))
                    }
                    _ => {}
                }
            }
        }
        Schemas {
            full,
            metadata,
            alerts: alerts.map(parse::document),
        }
    })
}

pub fn validate(input: impl Read, version: &str, options: &Options) -> Result<Report, Error> {
    // Upstream throws where CSV returns a finding (F-22).
    let v = Version::parse(version).ok_or_else(|| Error::UnknownJsonVersion(version.into()))?;
    let schemas = schemas(v);
    let item = (schemas.full.get("definitions").and_then(|d| d.get(CHARGES)))
        .expect("the schema defines a charge item");
    let max = options.max_errors;
    let (mut errors, mut alerts, mut metadata) = (Vec::new(), Vec::new(), Vec::new());
    let mut charges = false;
    let end = parse::parse(input, PATHS, |key, value| {
        match key {
            Key::Name(name) if name == CHARGES => {}
            Key::Name(name) => parse::set(&mut metadata, name, value),
            Key::Index(i) => {
                charges = true;
                let found = ajv::validate(&schemas.full, item, &value);
                add(
                    &mut errors,
                    found.into_iter().map(|e| error(e, Some(i))),
                    max,
                );
                if let Some(schema) = &schemas.alerts
                    && !full(&alerts, max)
                {
                    let found = ajv::validate(schema, schema, &value);
                    add(
                        &mut alerts,
                        found.into_iter().filter_map(|e| alert(e, i)),
                        max,
                    );
                }
                // Resolved here, before metadata and file-level alerts (F-29).
                if full(&errors, max) {
                    return ControlFlow::Break(());
                }
            }
        }
        ControlFlow::Continue(())
    })?;
    let extended = match end {
        End::Done => {
            let metadata = Value::Object(metadata);
            // With no charge items the full schema reports them missing.
            let schema = if charges {
                &schemas.metadata
            } else {
                &schemas.full
            };
            let found = ajv::validate(&schemas.full, schema, &metadata);
            add(&mut errors, found.into_iter().map(|e| error(e, None)), max);
            add(&mut alerts, file_alerts(v, &metadata), max);
            Vec::new()
        }
        End::Stopped => Vec::new(),
        // The errors found so far are dropped, the alerts kept (F-21).
        End::Invalid(e) => {
            let message = format!(
                "JSON parsing error: {e}. The validator is unable to review a syntactically invalid JSON file. Please ensure that your file is well-formatted JSON."
            );
            let error = Finding::new(Code::JsonInvalid, String::new(), message);
            std::mem::replace(&mut errors, vec![error])
        }
    };
    Ok(Report {
        errors,
        alerts,
        file_type: Some(FileType::Json),
        extended,
    })
}

/// `errorObjectToValidationError`; a charge item's errors then get the item's path in
/// front, while `field` stays the last segment of the path within the item.
fn error(e: ajv::Error, item: Option<usize>) -> Finding {
    let field = e.path.rsplit('/').next().unwrap_or_default().to_string();
    let path = match item {
        Some(i) => format!("/{CHARGES}/{i}{}", e.path),
        None => e.path,
    };
    Finding {
        field: Some(field),
        keyword: Some(e.keyword),
        ..Finding::new(Code::JsonSchema, path, e.message)
    }
}

/// `errorObjectToValidationAlert` for the errors that carry a `$message`: the alert
/// schemas fail on purpose where CMS wants a warning.
fn alert(e: ajv::Error, item: usize) -> Option<Finding> {
    let not = e.schema?;
    let Some(Value::String(message)) = not.get("$message") else {
        return None;
    };
    let code = if not.get("required").is_some() {
        Code::JsonEstimatedAmountDeprecated
    } else {
        Code::JsonNineNines
    };
    let message = message.clone();
    Some(Finding {
        code,
        message,
        ..error(e, Some(item))
    })
}

/// `buildFileLevelChecks`, in its order.
fn file_alerts(v: Version, metadata: &Value) -> Vec<Finding> {
    let mut alerts = Vec::new();
    let (object, field, code, statement) = if v < Version::V3_0 {
        let code = Code::JsonFalseAffirmation;
        ("affirmation", "confirm_affirmation", code, "Affirmation")
    } else {
        let code = Code::JsonFalseAttestation;
        ("attestation", "confirm_attestation", code, "Attestation")
    };
    if metadata.get(object).and_then(|o| o.get(field)) == Some(&Value::Bool(false)) {
        let message = format!("{statement} value is false.");
        alerts.push(Finding::new(code, format!("/{object}/{field}"), message));
    }
    let expected: &[&str] = match v {
        Version::V2_0 => &[],
        Version::V2_1 => &["2.0.0", "2.1.0"],
        Version::V2_2 => &["2.0.0", "2.2.0", "2.2.1"],
        Version::V3_0 => &["3.0.0"],
    };
    if let Some(Value::String(found)) = metadata.get("version")
        && !expected.is_empty()
        && !expected.contains(&found.as_str())
    {
        let message = crate::version_mismatch(found, expected);
        alerts.push(Finding::new(
            Code::JsonVersionMismatch,
            "/version".into(),
            message,
        ));
    }
    alerts
}
