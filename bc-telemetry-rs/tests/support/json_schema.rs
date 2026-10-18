//! A JSON Schema validator over the keywords Grafana's dashboard schema uses.
//!
//! Shared by path between this workspace's suites that render dashboards: the
//! dashboard model's, and the generator's. The schema is Grafana's own, vendored
//! at `crates/dashboard/schema/dashboard.jsonschema.json`, and it is the
//! dashboards' oracle (`crates/dashboard/README.md`, "The oracle").
//!
//! # Why a validator of our own
//!
//! The JSON Schema crates measured on 2026-10-12 brought 56 and 79 crates each
//! for a test-only check (`crates/dashboard/README.md`). A self-contained
//! draft-07 schema needs few keywords, and this file holds exactly the ones the
//! vendored schema uses. A keyword it does not hold is refused by name rather
//! than skipped, so a schema that grows one fails here instead of passing
//! dashboards nothing checked. Each keyword's refusal is a planted test in
//! `crates/dashboard/tests/suite/schema_validator.rs`.
//!
//! Both the schema and the instance are read through bc-json-rs: validated
//! strictly first, then split into raw slices by its reader. No value tree is
//! built, and no second JSON parser is written.
//!
//! # Draft-07, as far as this schema needs it
//!
//! - `$ref` names a local definition, `#/definitions/<name>`, and — as draft-07
//!   says — every other keyword beside it is ignored.
//! - `format` is an annotation, as draft-07 makes it by default: a value is not
//!   refused for its format.
//! - `description`, `default`, `deprecated` and `x-`-prefixed keys are
//!   annotations.
#![allow(
    dead_code,
    unreachable_pub,
    reason = "compiled into each suite that includes it, and each uses a different part"
)]

use bc_json::JsonReader;

/// A schema, held as the slices of its text.
pub struct Schema<'text> {
    root: &'text str,
    definitions: Vec<(String, &'text str)>,
}

/// The kinds of JSON value, as `type` names them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Object,
    Array,
    String,
    Number,
    Boolean,
    Null,
}

/// The kind of the value `slice` holds, from its first character.
fn kind_of(slice: &str) -> Kind {
    match slice.trim_start().as_bytes().first() {
        Some(b'{') => Kind::Object,
        Some(b'[') => Kind::Array,
        Some(b'"') => Kind::String,
        Some(b't' | b'f') => Kind::Boolean,
        Some(b'n') => Kind::Null,
        _ => Kind::Number,
    }
}

/// A string value's decoded text.
fn decoded_string(slice: &str) -> String {
    let decoded = JsonReader::extract_string_array(&format!("[{slice}]"));
    let [text] = &decoded[..] else {
        panic!("`{slice}` is not one string");
    };
    text.clone()
}

/// A number value.
fn number(slice: &str) -> f64 {
    slice
        .trim()
        .parse()
        .unwrap_or_else(|_| panic!("`{slice}` is not a number"))
}

/// Whether two values are equal as JSON values: strings by their decoded text,
/// numbers by value, anything else by its text.
fn same_value(left: &str, right: &str) -> bool {
    match (kind_of(left), kind_of(right)) {
        (Kind::String, Kind::String) => decoded_string(left) == decoded_string(right),
        (Kind::Number, Kind::Number) => number(left).total_cmp(&number(right)).is_eq(),
        (left_kind, right_kind) if left_kind == right_kind => left.trim() == right.trim(),
        _ => false,
    }
}

/// Whether `slice` is of the type `type_name` names.
fn is_of_type(slice: &str, type_name: &str) -> bool {
    let kind = kind_of(slice);
    match type_name {
        "object" => kind == Kind::Object,
        "array" => kind == Kind::Array,
        "string" => kind == Kind::String,
        "number" => kind == Kind::Number,
        "integer" => {
            kind == Kind::Number && {
                let value = number(slice);
                value.trunc().total_cmp(&value).is_eq()
            }
        }
        "boolean" => kind == Kind::Boolean,
        "null" => kind == Kind::Null,
        other => panic!("`type` names `{other}`, which is no JSON type"),
    }
}

/// The keywords that say nothing about whether a value is valid.
fn is_an_annotation(keyword: &str) -> bool {
    matches!(
        keyword,
        "description" | "default" | "deprecated" | "format" | "$schema" | "definitions" | "title"
    ) || keyword.starts_with("x-")
}

impl<'text> Schema<'text> {
    /// The schema `text` holds.
    ///
    /// # Panics
    ///
    /// When the text is not JSON, or not an object.
    pub fn new(text: &'text str) -> Self {
        if let Err(error) = bc_json::validate_json(text) {
            panic!("the schema is not JSON: {error}");
        }
        assert_eq!(kind_of(text), Kind::Object, "the schema is not an object");
        let definitions = JsonReader::extract_object_fields(text)
            .into_iter()
            .find(|(key, _)| key == "definitions")
            .map(|(_, definitions)| JsonReader::extract_object_fields(definitions))
            .unwrap_or_default();
        Self {
            root: text,
            definitions,
        }
    }

    /// Every way `instance` breaks the schema, each as `<pointer>: <what>`;
    /// empty when it holds.
    ///
    /// # Panics
    ///
    /// When the instance is not JSON, or the schema uses a keyword this
    /// validator does not hold.
    pub fn violations(&self, instance: &str) -> Vec<String> {
        if let Err(error) = bc_json::validate_json(instance) {
            panic!("the instance is not JSON: {error}");
        }
        let mut violations = Vec::new();
        self.check(self.root, instance, "", &mut violations);
        violations
    }

    fn definition(&self, reference: &str) -> &'text str {
        let name = reference
            .strip_prefix("#/definitions/")
            .unwrap_or_else(|| panic!("`$ref` `{reference}` is not a local definition"));
        let Some((_, definition)) = self.definitions.iter().find(|(key, _)| key == name) else {
            panic!("`$ref` names `{name}`, which is not defined");
        };
        definition
    }

    /// Whether `instance` holds `schema`, with no violation recorded.
    fn holds(&self, schema: &str, instance: &str) -> bool {
        let mut violations = Vec::new();
        self.check(schema, instance, "", &mut violations);
        violations.is_empty()
    }

    fn check(&self, schema: &str, instance: &str, pointer: &str, violations: &mut Vec<String>) {
        // `true` and `{}` hold anything; `false` holds nothing.
        match schema.trim() {
            "true" => return,
            "false" => {
                violations.push(format!("{pointer}: no value is allowed here"));
                return;
            }
            _ => {}
        }
        let keywords = JsonReader::extract_object_fields(schema);
        if let Some((_, reference)) = keywords.iter().find(|(key, _)| key == "$ref") {
            // Draft-07: a `$ref` stands alone.
            let definition = self.definition(&decoded_string(reference));
            self.check(definition, instance, pointer, violations);
            return;
        }
        for (keyword, value) in &keywords {
            self.check_keyword(keyword, value, &keywords, instance, pointer, violations);
        }
    }

    #[expect(
        clippy::too_many_lines,
        reason = "one arm per keyword, read as the table of what this validator holds"
    )]
    fn check_keyword(
        &self,
        keyword: &str,
        value: &str,
        keywords: &[(String, &str)],
        instance: &str,
        pointer: &str,
        violations: &mut Vec<String>,
    ) {
        match keyword {
            "type" => {
                let names = if kind_of(value) == Kind::Array {
                    JsonReader::extract_string_array(value)
                } else {
                    vec![decoded_string(value)]
                };
                if !names.iter().any(|name| is_of_type(instance, name)) {
                    violations.push(format!("{pointer}: is not of type {}", names.join(" or ")));
                }
            }
            "enum" => {
                let allowed = JsonReader::extract_array_elements(value);
                if !allowed.iter().any(|member| same_value(member, instance)) {
                    violations.push(format!(
                        "{pointer}: `{}` is not one of {value}",
                        instance.trim()
                    ));
                }
            }
            "const" => {
                if !same_value(value, instance) {
                    violations.push(format!("{pointer}: `{}` is not {value}", instance.trim()));
                }
            }
            "properties" => {
                if kind_of(instance) != Kind::Object {
                    return;
                }
                let schemas = JsonReader::extract_object_fields(value);
                for (name, member) in JsonReader::extract_object_fields(instance) {
                    if let Some((_, property)) = schemas.iter().find(|(key, _)| *key == name) {
                        self.check(property, member, &format!("{pointer}/{name}"), violations);
                    }
                }
            }
            "additionalProperties" => {
                if kind_of(instance) != Kind::Object {
                    return;
                }
                let declared: Vec<String> = keywords
                    .iter()
                    .find(|(key, _)| key == "properties")
                    .map(|(_, properties)| {
                        JsonReader::extract_object_fields(properties)
                            .into_iter()
                            .map(|(name, _)| name)
                            .collect()
                    })
                    .unwrap_or_default();
                for (name, member) in JsonReader::extract_object_fields(instance) {
                    if !declared.contains(&name) {
                        let at = format!("{pointer}/{name}");
                        if value.trim() == "false" {
                            violations.push(format!("{at}: is not a property this object has"));
                        } else {
                            self.check(value, member, &at, violations);
                        }
                    }
                }
            }
            "required" => {
                if kind_of(instance) != Kind::Object {
                    return;
                }
                let present: Vec<String> = JsonReader::extract_object_fields(instance)
                    .into_iter()
                    .map(|(name, _)| name)
                    .collect();
                for name in JsonReader::extract_string_array(value) {
                    if !present.contains(&name) {
                        violations.push(format!("{pointer}: is missing `{name}`"));
                    }
                }
            }
            "items" => {
                if kind_of(instance) != Kind::Array {
                    return;
                }
                assert_ne!(
                    kind_of(value),
                    Kind::Array,
                    "`items` as a list of schemas is a keyword this validator does not hold"
                );
                for (index, element) in JsonReader::extract_array_elements(instance)
                    .iter()
                    .enumerate()
                {
                    self.check(value, element, &format!("{pointer}/{index}"), violations);
                }
            }
            "allOf" => {
                for schema in JsonReader::extract_array_elements(value) {
                    self.check(schema, instance, pointer, violations);
                }
            }
            "anyOf" => {
                let schemas = JsonReader::extract_array_elements(value);
                if !schemas.iter().any(|schema| self.holds(schema, instance)) {
                    violations.push(format!("{pointer}: holds none of `anyOf`'s schemas"));
                }
            }
            "oneOf" => {
                let holding = JsonReader::extract_array_elements(value)
                    .iter()
                    .filter(|schema| self.holds(schema, instance))
                    .count();
                if holding != 1 {
                    violations.push(format!(
                        "{pointer}: holds {holding} of `oneOf`'s schemas, not exactly one"
                    ));
                }
            }
            "minimum" | "maximum" | "exclusiveMinimum" | "exclusiveMaximum" => {
                if kind_of(instance) != Kind::Number {
                    return;
                }
                let (bound, actual) = (number(value), number(instance));
                let holds = match keyword {
                    "minimum" => actual >= bound,
                    "maximum" => actual <= bound,
                    "exclusiveMinimum" => actual > bound,
                    _ => actual < bound,
                };
                if !holds {
                    violations.push(format!("{pointer}: {actual} breaks `{keyword}` {bound}"));
                }
            }
            "minLength" => {
                if kind_of(instance) != Kind::String {
                    return;
                }
                let length = decoded_string(instance).chars().count();
                let minimum: usize = value.trim().parse().expect("`minLength` is a count");
                if length < minimum {
                    violations.push(format!("{pointer}: is shorter than {value} characters"));
                }
            }
            annotation if is_an_annotation(annotation) => {}
            unknown => {
                panic!("the schema uses `{unknown}`, a keyword this validator does not hold")
            }
        }
    }
}
