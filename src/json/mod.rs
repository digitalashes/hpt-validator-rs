//! The JSON branch: upstream's `JsonValidator`. Charge items are validated one at a time as
//! the parser completes them; the metadata members are collected and validated at the end.

mod ajv;
mod parse;

use crate::{Code, Error, FileType, Finding, Mode, Options, Report, Version, add, full};
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
            Key::Name(name) => match metadata.iter_mut().find(|(k, _)| *k == name) {
                Some(member) => *member = (name, value),
                None => metadata.push((name, value)),
            },
            Key::Index(i) => {
                charges = true;
                let prefix = format!("/{CHARGES}/{i}");
                let found = ajv::validate(&schemas.full, item, &value);
                add(
                    &mut errors,
                    found.into_iter().map(|e| error(e, &prefix)),
                    max,
                );
                if let Some(schema) = &schemas.alerts {
                    let found = ajv::validate(schema, schema, &value);
                    add(
                        &mut alerts,
                        found.into_iter().filter_map(|e| alert(e, &prefix)),
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
    match end {
        End::Done => {
            let metadata = Value::Object(metadata);
            // With no charge items the full schema reports them missing.
            let schema = if charges {
                &schemas.metadata
            } else {
                &schemas.full
            };
            let found = ajv::validate(&schemas.full, schema, &metadata);
            add(&mut errors, found.into_iter().map(|e| error(e, "")), max);
            add(&mut alerts, file_alerts(v, &metadata), max);
        }
        End::Stopped => {}
        // The errors found so far are dropped, the alerts kept (F-21).
        End::Invalid(e) => {
            let message = format!(
                "JSON parsing error: {e}. The validator is unable to review a syntactically invalid JSON file. Please ensure that your file is well-formatted JSON."
            );
            errors = vec![Finding::new(Code::JsonInvalid, String::new(), message)];
        }
    }
    Ok(Report {
        errors,
        alerts,
        file_type: (options.mode == Mode::Extended).then_some(FileType::Json),
        extended: Vec::new(),
    })
}

/// `errorObjectToValidationError`, the path then prefixed: `field` is the last segment of
/// the unprefixed path.
fn error(e: ajv::Error, prefix: &str) -> Finding {
    let field = e.path.rsplit('/').next().unwrap_or_default().to_string();
    Finding {
        field: Some(field),
        keyword: Some(e.keyword),
        ..Finding::new(Code::JsonSchema, format!("{prefix}{}", e.path), e.message)
    }
}

/// `errorObjectToValidationAlert` for the errors that carry a `$message`: the alert
/// schemas fail on purpose where CMS wants a warning.
fn alert(e: ajv::Error, prefix: &str) -> Option<Finding> {
    let not = e.schema?;
    let Some(Value::String(message)) = not.get("$message") else {
        return None;
    };
    let code = if not.get("required").is_some() {
        Code::JsonEstimatedAmountDeprecated
    } else {
        Code::JsonNineNines
    };
    let field = e.path.rsplit('/').next().unwrap_or_default().to_string();
    Some(Finding {
        field: Some(field),
        keyword: Some(e.keyword),
        ..Finding::new(code, format!("{prefix}{}", e.path), message.clone())
    })
}

/// `buildFileLevelChecks`, in its order.
fn file_alerts(v: Version, metadata: &Value) -> Vec<Finding> {
    let mut alerts = Vec::new();
    let is_false = |object, field| {
        metadata.get(object).and_then(|o| o.get(field)) == Some(&Value::Bool(false))
    };
    if v < Version::V3_0 && is_false("affirmation", "confirm_affirmation") {
        let path = "/affirmation/confirm_affirmation".into();
        let message = "Affirmation value is false.".into();
        alerts.push(Finding::new(Code::JsonFalseAffirmation, path, message));
    }
    if v >= Version::V3_0 && is_false("attestation", "confirm_attestation") {
        let path = "/attestation/confirm_attestation".into();
        let message = "Attestation value is false.".into();
        alerts.push(Finding::new(Code::JsonFalseAttestation, path, message));
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
