//! Ajv 8.18 with `allErrors: true`, for the keywords the vendored schemas use. Keywords run
//! in Ajv's order: a `type` check up front unless the schema has keywords of that type,
//! then `const`, `enum`, `not`, `anyOf`, `allOf`, `if`, then the number, string, array and
//! object keywords, each set only for data of its type.

use super::parse::Value;
use crate::js;
use std::fmt::{Display, Write as _};

/// Ajv's `ErrorObject`, as far as upstream reads it.
pub struct Error<'s> {
    /// `instancePath`.
    pub path: String,
    pub keyword: &'static str,
    pub message: String,
    /// The keyword's value in the schema (Ajv's `verbose`), where alerts keep `$message`.
    pub schema: Option<&'s Value>,
}

/// Validates `data` against `schema`; `root` holds the `definitions` that `$ref`s name.
pub fn validate<'s>(root: &'s Value, schema: &'s Value, data: &Value) -> Vec<Error<'s>> {
    let mut run = Run {
        definitions: root.get("definitions"),
        path: String::new(),
        errors: Vec::new(),
        failed: 0,
        quiet: 0,
    };
    run.check(schema, data);
    run.errors
}

/// Keywords by the data type they apply to, in Ajv's rule order.
const TYPED: [(&str, &[&str]); 4] = [
    ("number", &["exclusiveMinimum", "format"]),
    ("string", &["minLength", "pattern", "format"]),
    ("array", &["minItems", "items", "contains"]),
    ("object", &["required", "properties"]),
];

// ponytail: keywords are looked up by name on every check, about a third of the run time on
// large files; compile each schema object into a struct of keyword fields if that matters.
struct Run<'s> {
    definitions: Option<&'s Value>,
    path: String,
    errors: Vec<Error<'s>>,
    /// Errors found, kept or not: a subschema is valid when it adds none.
    failed: usize,
    /// Inside a check whose errors are never kept: `not`, `if`, `contains`, `anyOf` trials.
    quiet: usize,
}

impl<'s> Run<'s> {
    fn error(&mut self, s: &'s Value, keyword: &'static str, message: impl FnOnce() -> String) {
        self.failed += 1;
        if self.quiet == 0 {
            self.errors.push(Error {
                path: self.path.clone(),
                keyword,
                message: message(),
                schema: s.get(keyword),
            });
        }
    }

    /// Whether `data` is valid against `schema`, without errors.
    fn valid(&mut self, schema: &'s Value, data: &Value) -> bool {
        let failed = self.failed;
        self.quiet += 1;
        self.check(schema, data);
        self.quiet -= 1;
        std::mem::replace(&mut self.failed, failed) == failed
    }

    fn check(&mut self, s: &'s Value, data: &Value) {
        if let Some(Value::String(r)) = s.get("$ref") {
            let name = r.strip_prefix("#/definitions/").expect("local refs only");
            let target = self.definitions.and_then(|d| d.get(name));
            return self.check(target.expect("defined"), data);
        }
        let has = |keywords: &[&str]| keywords.iter().any(|k| s.get(k).is_some());
        let ty = s.get("type").map(|t| match t {
            Value::String(t) => t.as_str(),
            _ => unreachable!("one type per schema"),
        });
        // The type is checked up front only if no keyword of that type does it.
        let up_front = ty.is_some_and(|t| !TYPED.iter().any(|&(g, k)| g == t && has(k)));
        if let Some(t) = ty
            && up_front
            && !is(t, data)
        {
            self.error(s, "type", || format!("must be {t}"));
        }

        if let Some(c) = s.get("const")
            && data != c
        {
            self.error(s, "const", || "must be equal to constant".into());
        }
        if let Some(Value::Array(allowed)) = s.get("enum")
            && !allowed.contains(data)
        {
            self.error(s, "enum", || {
                "must be equal to one of the allowed values".into()
            });
        }
        if let Some(not) = s.get("not")
            && self.valid(not, data)
        {
            self.error(s, "not", || "must NOT be valid".into());
        }
        // One passing branch discards the others' errors; with none, all of them stay.
        if let Some(Value::Array(branches)) = s.get("anyOf")
            && !branches.iter().any(|b| self.valid(b, data))
        {
            for b in branches {
                self.check(b, data);
            }
            self.error(s, "anyOf", || "must match a schema in anyOf".into());
        }
        if let Some(Value::Array(all)) = s.get("allOf") {
            for b in all {
                self.check(b, data);
            }
        }
        if let (Some(cond), Some(then)) = (s.get("if"), s.get("then"))
            && self.valid(cond, data)
        {
            let failed = self.failed;
            self.check(then, data);
            if self.failed > failed {
                self.error(s, "if", || "must match \"then\" schema".into());
            }
        }

        for (group, keywords) in TYPED {
            if !has(keywords) {
                continue;
            }
            if !is(group, data) {
                if ty == Some(group) {
                    self.error(s, "type", || format!("must be {group}"));
                }
                continue;
            }
            match data {
                Value::Number(n) => self.number(s, *n),
                Value::String(v) => self.string(s, v),
                Value::Array(items) => self.array(s, items),
                Value::Object(_) => self.object(s, data),
                _ => unreachable!(),
            }
        }
    }

    fn number(&mut self, s: &'s Value, n: f64) {
        if let Some(Value::Number(min)) = s.get("exclusiveMinimum")
            && n <= *min
        {
            self.error(s, "exclusiveMinimum", || {
                format!("must be > {}", js::number_string(*min))
            });
        }
        // `format` has no number formats here.
    }

    #[expect(clippy::cast_precision_loss, reason = "lengths stay far below 2^52")]
    fn string(&mut self, s: &'s Value, v: &str) {
        if let Some(Value::Number(min)) = s.get("minLength")
            && (v.chars().count() as f64) < *min
        {
            let min = js::number_string(*min);
            self.error(s, "minLength", || {
                format!("must NOT have fewer than {min} characters")
            });
        }
        if let Some(Value::String(p)) = s.get("pattern")
            && !is_count(v)
        {
            self.error(s, "pattern", || format!("must match pattern \"{p}\""));
        }
        if s.get("format").is_some() && !js::is_iso_date(v) {
            self.error(s, "format", || "must match format \"date\"".into());
        }
    }

    #[expect(clippy::cast_precision_loss, reason = "lengths stay far below 2^52")]
    fn array(&mut self, s: &'s Value, items: &[Value]) {
        if let Some(Value::Number(min)) = s.get("minItems")
            && (items.len() as f64) < *min
        {
            let min = js::number_string(*min);
            self.error(s, "minItems", || {
                format!("must NOT have fewer than {min} items")
            });
        }
        if let Some(schema) = s.get("items") {
            for (i, item) in items.iter().enumerate() {
                self.at(i, schema, item);
            }
        }
        if let Some(schema) = s.get("contains")
            && !items.iter().any(|item| self.valid(schema, item))
        {
            self.error(s, "contains", || {
                "must contain at least 1 valid item(s)".into()
            });
        }
    }

    /// Checks `data` found at `segment` below the current path. No property name in the
    /// schemas needs JSON Pointer escaping.
    fn at(&mut self, segment: impl Display, schema: &'s Value, data: &Value) {
        let len = self.path.len();
        write!(self.path, "/{segment}").expect("a String write cannot fail");
        self.check(schema, data);
        self.path.truncate(len);
    }

    fn object(&mut self, s: &'s Value, data: &Value) {
        if let Some(Value::Array(required)) = s.get("required") {
            for name in required {
                let Value::String(name) = name else { continue };
                if data.get(name).is_none() {
                    self.error(s, "required", || {
                        format!("must have required property '{name}'")
                    });
                }
            }
        }
        if let Some(Value::Object(properties)) = s.get("properties") {
            for (name, schema) in properties {
                if let Some(value) = data.get(name) {
                    self.at(name, schema, value);
                }
            }
        }
    }
}

/// Ajv's type check, `strictNumbers` included.
fn is(ty: &str, v: &Value) -> bool {
    match (ty, v) {
        ("number", Value::Number(n)) => n.is_finite(),
        ("object", Value::Object(_))
        | ("array", Value::Array(_))
        | ("string", Value::String(_))
        | ("boolean", Value::Bool(_))
        | ("null", Value::Null) => true,
        _ => false,
    }
}

/// The one `pattern` in the schemas, v3.0's count of allowed amounts (`COUNT` in the
/// test): `0`, `1 through 10`, or an integer from 11.
fn is_count(v: &str) -> bool {
    v == "0" || v == "1 through 10" || crate::is_count_integer(v)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Version;

    const COUNT: &str = r"^(0|1 through 10|1[1-9]|[2-9]\d+|[1-9]\d{2,})$";

    /// `check` reads every keyword the vendored schemas use, in the form it assumes.
    #[test]
    fn schemas_stay_in_the_subset() {
        fn walk(s: &Value) {
            let Value::Object(members) = s else {
                panic!("schemas are objects")
            };
            for (keyword, v) in members {
                match (keyword.as_str(), v) {
                    ("$ref", Value::String(r)) => {
                        assert!(r.starts_with("#/definitions/") && members.len() == 1);
                    }
                    ("type", Value::String(_))
                    | ("const" | "enum" | "required" | "$schema" | "$message", _)
                    | ("minLength" | "minItems" | "exclusiveMinimum", Value::Number(_)) => {}
                    ("pattern", Value::String(p)) => assert_eq!(p, COUNT),
                    ("format", Value::String(f)) => assert_eq!(f, "date"),
                    ("properties" | "definitions", Value::Object(m)) => {
                        for (name, s) in m {
                            assert!(!name.contains(['~', '/']), "{name} needs escaping");
                            walk(s);
                        }
                    }
                    ("items" | "contains" | "not" | "if" | "then", s) => walk(s),
                    ("anyOf" | "allOf", Value::Array(all)) => all.iter().for_each(walk),
                    (keyword, _) => panic!("{keyword} is outside the subset"),
                }
            }
        }
        for v in [Version::V2_0, Version::V2_1, Version::V2_2, Version::V3_0] {
            let schemas = crate::json::schemas(v);
            walk(&schemas.full);
            schemas.alerts.iter().for_each(walk);
        }
    }
}
