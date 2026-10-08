//! Ajv 8.18 with `allErrors: true`, for the keywords the vendored schemas use. Keywords run
//! in Ajv's order: a `type` check up front unless the schema has keywords of that type,
//! then `const`, `enum`, `not`, `anyOf`, `allOf`, `if`, then the number, string, array and
//! object keywords, each set only for data of its type.

use super::parse::Value;
use crate::js;

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

/// A schema's keywords, read in one pass over its members.
#[derive(Default)]
struct Keywords<'s> {
    r#ref: Option<&'s str>,
    ty: Option<&'s str>,
    r#const: Option<&'s Value>,
    r#enum: Option<&'s [Value]>,
    not: Option<&'s Value>,
    any_of: Option<&'s [Value]>,
    all_of: Option<&'s [Value]>,
    r#if: Option<&'s Value>,
    then: Option<&'s Value>,
    exclusive_minimum: Option<f64>,
    min_length: Option<f64>,
    pattern: Option<&'s str>,
    format: bool,
    min_items: Option<f64>,
    items: Option<&'s Value>,
    contains: Option<&'s Value>,
    required: Option<&'s [Value]>,
    properties: Option<&'s [(String, Value)]>,
}

impl<'s> Keywords<'s> {
    // ponytail: read again on every check; compile the schemas into a tree of these once
    // if JSON validation matters more.
    fn of(s: &'s Value) -> Self {
        let mut k = Keywords::default();
        let Value::Object(members) = s else { return k };
        for (name, v) in members {
            match (name.as_str(), v) {
                ("$ref", Value::String(r)) => k.r#ref = Some(r),
                ("type", Value::String(t)) => k.ty = Some(t),
                ("const", v) => k.r#const = Some(v),
                ("enum", Value::Array(a)) => k.r#enum = Some(a),
                ("not", v) => k.not = Some(v),
                ("anyOf", Value::Array(a)) => k.any_of = Some(a),
                ("allOf", Value::Array(a)) => k.all_of = Some(a),
                ("if", v) => k.r#if = Some(v),
                ("then", v) => k.then = Some(v),
                ("exclusiveMinimum", Value::Number(n)) => k.exclusive_minimum = Some(*n),
                ("minLength", Value::Number(n)) => k.min_length = Some(*n),
                ("pattern", Value::String(p)) => k.pattern = Some(p),
                ("format", _) => k.format = true,
                ("minItems", Value::Number(n)) => k.min_items = Some(*n),
                ("items", v) => k.items = Some(v),
                ("contains", v) => k.contains = Some(v),
                ("required", Value::Array(a)) => k.required = Some(a),
                ("properties", Value::Object(m)) => k.properties = Some(m),
                _ => {}
            }
        }
        k
    }

    /// Whether any keyword of each data type is set, in Ajv's rule order.
    fn typed(&self) -> [(&'static str, bool); 4] {
        [
            ("number", self.exclusive_minimum.is_some() || self.format),
            ("string", self.min_length.is_some() || self.pattern.is_some() || self.format),
            ("array", self.min_items.is_some() || self.items.is_some() || self.contains.is_some()),
            ("object", self.required.is_some() || self.properties.is_some()),
        ]
    }
}

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
        let k = Keywords::of(s);
        if let Some(r) = k.r#ref {
            let name = r.strip_prefix("#/definitions/").expect("local refs only");
            let target = self.definitions.and_then(|d| d.get(name));
            return self.check(target.expect("defined"), data);
        }
        let typed = k.typed();
        // The type is checked up front only if no keyword of that type does it.
        let up_front = k.ty.is_some_and(|t| !typed.iter().any(|&(g, has)| g == t && has));
        if let Some(t) = k.ty
            && up_front
            && !is(t, data)
        {
            self.error(s, "type", || format!("must be {t}"));
        }

        if let Some(c) = k.r#const
            && data != c
        {
            self.error(s, "const", || "must be equal to constant".into());
        }
        if let Some(allowed) = k.r#enum
            && !allowed.contains(data)
        {
            self.error(s, "enum", || "must be equal to one of the allowed values".into());
        }
        if let Some(not) = k.not
            && self.valid(not, data)
        {
            self.error(s, "not", || "must NOT be valid".into());
        }
        // One passing branch discards the others' errors; with none, all of them stay.
        if let Some(branches) = k.any_of
            && !branches.iter().any(|b| self.valid(b, data))
        {
            for b in branches {
                self.check(b, data);
            }
            self.error(s, "anyOf", || "must match a schema in anyOf".into());
        }
        for b in k.all_of.unwrap_or_default() {
            self.check(b, data);
        }
        if let (Some(cond), Some(then)) = (k.r#if, k.then)
            && self.valid(cond, data)
        {
            let failed = self.failed;
            self.check(then, data);
            if self.failed > failed {
                self.error(s, "if", || "must match \"then\" schema".into());
            }
        }

        for (group, has) in typed {
            if !has {
                continue;
            }
            if !is(group, data) {
                if k.ty == Some(group) {
                    self.error(s, "type", || format!("must be {group}"));
                }
                continue;
            }
            match data {
                Value::Number(n) => self.number(s, &k, *n),
                Value::String(v) => self.string(s, &k, v),
                Value::Array(items) => self.array(s, &k, items),
                Value::Object(_) => self.object(s, &k, data),
                _ => unreachable!(),
            }
        }
    }

    fn number(&mut self, s: &'s Value, k: &Keywords<'s>, n: f64) {
        if let Some(min) = k.exclusive_minimum
            && n <= min
        {
            self.error(s, "exclusiveMinimum", || format!("must be > {}", js::number_string(min)));
        }
        // `format` has no number formats here.
    }

    #[expect(clippy::cast_precision_loss, reason = "lengths stay far below 2^52")]
    fn string(&mut self, s: &'s Value, k: &Keywords<'s>, v: &str) {
        if let Some(min) = k.min_length
            && (v.chars().count() as f64) < min
        {
            let min = js::number_string(min);
            self.error(s, "minLength", || format!("must NOT have fewer than {min} characters"));
        }
        if let Some(p) = k.pattern
            && !is_count(v)
        {
            self.error(s, "pattern", || format!("must match pattern \"{p}\""));
        }
        if k.format && !js::is_iso_date(v) {
            self.error(s, "format", || "must match format \"date\"".into());
        }
    }

    #[expect(clippy::cast_precision_loss, reason = "lengths stay far below 2^52")]
    fn array(&mut self, s: &'s Value, k: &Keywords<'s>, items: &[Value]) {
        if let Some(min) = k.min_items
            && (items.len() as f64) < min
        {
            let min = js::number_string(min);
            self.error(s, "minItems", || format!("must NOT have fewer than {min} items"));
        }
        if let Some(schema) = k.items {
            for (i, item) in items.iter().enumerate() {
                self.at(|path| push_index(path, i), schema, item);
            }
        }
        if let Some(schema) = k.contains
            && !items.iter().any(|item| self.valid(schema, item))
        {
            self.error(s, "contains", || "must contain at least 1 valid item(s)".into());
        }
    }

    /// Checks `data` found at the segment that `segment` writes below the current path. No
    /// property name in the schemas needs JSON Pointer escaping.
    fn at(&mut self, segment: impl FnOnce(&mut String), schema: &'s Value, data: &Value) {
        let len = self.path.len();
        self.path.push('/');
        segment(&mut self.path);
        self.check(schema, data);
        self.path.truncate(len);
    }

    fn object(&mut self, s: &'s Value, k: &Keywords<'s>, data: &Value) {
        if let Some(required) = k.required {
            for name in required {
                let Value::String(name) = name else { continue };
                if data.get(name).is_none() {
                    self.error(s, "required", || format!("must have required property '{name}'"));
                }
            }
        }
        if let Some(properties) = k.properties {
            for (name, schema) in properties {
                if let Some(value) = data.get(name) {
                    self.at(|path| path.push_str(name), schema, value);
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

/// `i` in decimal, without `fmt`.
fn push_index(path: &mut String, i: usize) {
    if i >= 10 {
        push_index(path, i / 10);
    }
    path.push(char::from(b"0123456789"[i % 10]));
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
            let Value::Object(members) = s else { panic!("schemas are objects") };
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
