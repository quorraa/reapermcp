//! A JSON Schema 2020-12 *subset* validator.
//!
//! The subset is exactly what the project's own schemas need — no remote
//! references, no dynamic anchors, no vocabularies:
//!
//! * core: `$schema`, `$id`, `$defs`, `$ref` (only `#` and `#/$defs/NAME`),
//!   `title`, `description`
//! * generic: `type` (string or array), `enum`, `const`
//! * objects: `properties`, `required`, `additionalProperties`,
//!   `patternProperties`, `propertyNames`, `minProperties`, `maxProperties`
//! * arrays: `items`, `prefixItems`, `minItems`, `maxItems`, `uniqueItems`
//! * numbers: `minimum`, `maximum`, `exclusiveMinimum`, `exclusiveMaximum`,
//!   `multipleOf`
//! * strings: `minLength`, `maxLength`, `pattern`, `format` (annotation only,
//!   never enforced)
//! * applicators: `allOf`, `anyOf`, `oneOf`, `not`
//!
//! Boolean schemas (`true` / `false`) are supported wherever a subschema is
//! expected. Keywords outside the list above are ignored, as the specification
//! requires of unknown vocabulary.
//!
//! `pattern` and `patternProperties` are evaluated with [`crate::regex`]. If a
//! pattern would backtrack catastrophically, a `pattern` [`Violation`] is
//! reported instead of hanging.
//!
//! ```
//! use qjson::{schema::Schema, Json};
//!
//! let doc = Json::parse(r#"{
//!   "type": "object",
//!   "properties": { "bpm": { "type": "number", "minimum": 20 } },
//!   "required": ["bpm"]
//! }"#).expect("schema");
//! let s = Schema::compile(&doc).expect("compile");
//! assert!(s.validate(&Json::parse(r#"{"bpm":120}"#).expect("v")).is_empty());
//! assert!(!s.validate(&Json::parse(r#"{"bpm":5}"#).expect("v")).is_empty());
//! ```

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use crate::json::{Json, JsonMap};
use crate::regex::Regex;

/// Maximum `$ref` expansion depth; guards against self-referential schemas.
const MAX_REF_DEPTH: u32 = 64;

/// A problem found while compiling a schema document.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SchemaError {
    /// Human-readable explanation, including the schema location.
    pub message: String,
}

impl fmt::Display for SchemaError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for SchemaError {}

/// One reason a value failed validation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Violation {
    /// JSON pointer to the offending value, empty for the document root.
    pub instance_path: String,
    /// The schema keyword that rejected the value.
    pub keyword: String,
    /// Human-readable explanation.
    pub message: String,
}

impl fmt::Display for Violation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.instance_path.is_empty() {
            write!(f, "{}: {}", self.keyword, self.message)
        } else {
            write!(
                f,
                "{} at {}: {}",
                self.keyword, self.instance_path, self.message
            )
        }
    }
}

/// The JSON type names accepted by the `type` keyword.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum JType {
    /// `"null"`
    Null,
    /// `"boolean"`
    Boolean,
    /// `"object"`
    Object,
    /// `"array"`
    Array,
    /// `"number"`
    Number,
    /// `"string"`
    String,
    /// `"integer"`
    Integer,
}

impl JType {
    /// Parses a `type` keyword value.
    fn parse(s: &str) -> Option<JType> {
        Some(match s {
            "null" => JType::Null,
            "boolean" => JType::Boolean,
            "object" => JType::Object,
            "array" => JType::Array,
            "number" => JType::Number,
            "string" => JType::String,
            "integer" => JType::Integer,
            _ => return None,
        })
    }

    /// Tests a value against this type.
    fn matches(self, v: &Json) -> bool {
        match self {
            JType::Null => v.is_null(),
            JType::Boolean => matches!(v, Json::Bool(_)),
            JType::Object => matches!(v, Json::Obj(_)),
            JType::Array => matches!(v, Json::Arr(_)),
            JType::String => matches!(v, Json::Str(_)),
            JType::Number => matches!(v, Json::Int(_) | Json::Float(_)),
            // 2020-12: a float with zero fractional part is an integer.
            JType::Integer => match v {
                Json::Int(_) => true,
                Json::Float(f) => f.is_finite() && f.fract() == 0.0,
                _ => false,
            },
        }
    }
}

/// Where a `$ref` points.
#[derive(Clone, Debug, PartialEq, Eq)]
enum RefTarget {
    /// `"#"` — the root schema.
    Root,
    /// `"#/$defs/NAME"`.
    Def(String),
}

/// A compiled subschema.
#[derive(Clone, Debug, Default)]
struct Node {
    /// Set when the schema was the literal `true` or `false`.
    boolean: Option<bool>,
    /// `$ref`
    ref_target: Option<RefTarget>,
    /// `type`
    types: Option<Vec<JType>>,
    /// `enum`
    enum_values: Option<Vec<Json>>,
    /// `const`
    const_value: Option<Json>,
    /// `properties`
    properties: Vec<(String, Node)>,
    /// `required`
    required: Vec<String>,
    /// `additionalProperties`
    additional_properties: Option<Box<Node>>,
    /// `patternProperties`, as `(source, compiled, subschema)`.
    pattern_properties: Vec<(String, Regex, Node)>,
    /// `propertyNames`
    property_names: Option<Box<Node>>,
    /// `items`
    items: Option<Box<Node>>,
    /// `prefixItems`
    prefix_items: Vec<Node>,
    /// `minItems`
    min_items: Option<u64>,
    /// `maxItems`
    max_items: Option<u64>,
    /// `uniqueItems`
    unique_items: bool,
    /// `minimum`
    minimum: Option<f64>,
    /// `maximum`
    maximum: Option<f64>,
    /// `exclusiveMinimum`
    exclusive_minimum: Option<f64>,
    /// `exclusiveMaximum`
    exclusive_maximum: Option<f64>,
    /// `multipleOf`
    multiple_of: Option<f64>,
    /// `minLength`
    min_length: Option<u64>,
    /// `maxLength`
    max_length: Option<u64>,
    /// `pattern`
    pattern: Option<Regex>,
    /// `minProperties`
    min_properties: Option<u64>,
    /// `maxProperties`
    max_properties: Option<u64>,
    /// `allOf`
    all_of: Vec<Node>,
    /// `anyOf`
    any_of: Vec<Node>,
    /// `oneOf`
    one_of: Vec<Node>,
    /// `not`
    not: Option<Box<Node>>,
}

/// A compiled JSON Schema.
#[derive(Clone, Debug)]
pub struct Schema {
    /// The `$id` annotation, if the document carried one.
    id: Option<String>,
    /// The root subschema.
    root: Node,
    /// Compiled `$defs`, keyed by name.
    defs: BTreeMap<String, Node>,
}

impl Schema {
    /// Compiles a schema document.
    ///
    /// # Errors
    ///
    /// Returns [`SchemaError`] when a supported keyword has the wrong shape, a
    /// `pattern` does not compile, or a `$ref` uses an unsupported form or
    /// names an unknown `$defs` entry.
    pub fn compile(doc: &Json) -> Result<Schema, SchemaError> {
        let id = doc.get("$id").and_then(Json::as_str).map(str::to_string);

        // Collect definition names first so `$ref` can be checked eagerly.
        let mut names: BTreeSet<String> = BTreeSet::new();
        if let Some(defs) = doc.get("$defs") {
            let map = defs.as_obj().ok_or_else(|| SchemaError {
                message: "`$defs` must be an object".to_string(),
            })?;
            for k in map.keys() {
                names.insert(k.to_string());
            }
        }

        let root = compile_node(doc, &names, "#")?;

        let mut defs = BTreeMap::new();
        if let Some(map) = doc.get("$defs").and_then(Json::as_obj) {
            for (k, v) in map.iter() {
                defs.insert(
                    k.to_string(),
                    compile_node(v, &names, &format!("#/$defs/{k}"))?,
                );
            }
        }

        Ok(Schema { id, root, defs })
    }

    /// The schema's `$id`, if it declared one.
    pub fn id(&self) -> Option<&str> {
        self.id.as_deref()
    }

    /// Validates `value`, returning every violation found.
    ///
    /// An empty vector means the value is valid.
    pub fn validate(&self, value: &Json) -> Vec<Violation> {
        let mut out = Vec::new();
        self.check(&self.root, value, "", 0, &mut out);
        out
    }

    /// Validates `value`, collapsing success into `Ok(())`.
    ///
    /// # Errors
    ///
    /// Returns the non-empty violation list when `value` does not conform.
    pub fn validate_ok(&self, value: &Json) -> Result<(), Vec<Violation>> {
        let v = self.validate(value);
        if v.is_empty() {
            Ok(())
        } else {
            Err(v)
        }
    }

    /// `true` when `node` accepts `value`, without recording violations.
    fn matches(&self, node: &Node, value: &Json, path: &str, depth: u32) -> bool {
        let mut scratch = Vec::new();
        self.check(node, value, path, depth, &mut scratch);
        scratch.is_empty()
    }

    /// Core validation walk.
    fn check(&self, node: &Node, value: &Json, path: &str, depth: u32, out: &mut Vec<Violation>) {
        if depth > MAX_REF_DEPTH {
            out.push(Violation {
                instance_path: path.to_string(),
                keyword: "$ref".to_string(),
                message: format!("schema reference nested deeper than {MAX_REF_DEPTH} levels"),
            });
            return;
        }

        if let Some(b) = node.boolean {
            if !b {
                out.push(Violation {
                    instance_path: path.to_string(),
                    keyword: "false".to_string(),
                    message: "schema `false` rejects every value".to_string(),
                });
            }
            return;
        }

        if let Some(target) = &node.ref_target {
            match target {
                RefTarget::Root => self.check(&self.root, value, path, depth + 1, out),
                RefTarget::Def(name) => match self.defs.get(name) {
                    Some(n) => self.check(n, value, path, depth + 1, out),
                    None => out.push(Violation {
                        instance_path: path.to_string(),
                        keyword: "$ref".to_string(),
                        message: format!("unresolved reference `#/$defs/{name}`"),
                    }),
                },
            }
        }

        self.check_type(node, value, path, out);
        self.check_enum_const(node, value, path, out);
        self.check_number(node, value, path, out);
        self.check_string(node, value, path, out);
        self.check_array(node, value, path, depth, out);
        self.check_object(node, value, path, depth, out);
        self.check_applicators(node, value, path, depth, out);
    }

    /// `type`
    fn check_type(&self, node: &Node, value: &Json, path: &str, out: &mut Vec<Violation>) {
        let Some(types) = &node.types else { return };
        if types.iter().any(|t| t.matches(value)) {
            return;
        }
        let names: Vec<&str> = types.iter().map(type_name).collect();
        out.push(Violation {
            instance_path: path.to_string(),
            keyword: "type".to_string(),
            message: format!(
                "expected type {}, found {}",
                names.join(" or "),
                value.type_name()
            ),
        });
    }

    /// `enum` and `const`
    fn check_enum_const(&self, node: &Node, value: &Json, path: &str, out: &mut Vec<Violation>) {
        if let Some(values) = &node.enum_values {
            let canonical = value.to_canonical_string();
            if !values.iter().any(|v| v.to_canonical_string() == canonical) {
                out.push(Violation {
                    instance_path: path.to_string(),
                    keyword: "enum".to_string(),
                    message: format!("{canonical} is not one of the permitted values"),
                });
            }
        }
        if let Some(expected) = &node.const_value {
            if expected.to_canonical_string() != value.to_canonical_string() {
                out.push(Violation {
                    instance_path: path.to_string(),
                    keyword: "const".to_string(),
                    message: format!("expected the constant {}", expected.to_canonical_string()),
                });
            }
        }
    }

    /// `minimum`, `maximum`, `exclusiveMinimum`, `exclusiveMaximum`, `multipleOf`
    fn check_number(&self, node: &Node, value: &Json, path: &str, out: &mut Vec<Violation>) {
        let Some(n) = value.as_f64() else { return };
        let mut push = |keyword: &str, message: String| {
            out.push(Violation {
                instance_path: path.to_string(),
                keyword: keyword.to_string(),
                message,
            });
        };
        if let Some(m) = node.minimum {
            if n < m {
                push("minimum", format!("{n} is below the minimum {m}"));
            }
        }
        if let Some(m) = node.maximum {
            if n > m {
                push("maximum", format!("{n} is above the maximum {m}"));
            }
        }
        if let Some(m) = node.exclusive_minimum {
            if n <= m {
                push(
                    "exclusiveMinimum",
                    format!("{n} must be strictly greater than {m}"),
                );
            }
        }
        if let Some(m) = node.exclusive_maximum {
            if n >= m {
                push(
                    "exclusiveMaximum",
                    format!("{n} must be strictly less than {m}"),
                );
            }
        }
        if let Some(m) = node.multiple_of {
            if !is_multiple_of(n, m) {
                push("multipleOf", format!("{n} is not a multiple of {m}"));
            }
        }
    }

    /// `minLength`, `maxLength`, `pattern`
    fn check_string(&self, node: &Node, value: &Json, path: &str, out: &mut Vec<Violation>) {
        let Some(s) = value.as_str() else { return };
        // JSON Schema counts Unicode code points, not UTF-8 bytes.
        let len = s.chars().count() as u64;
        if let Some(m) = node.min_length {
            if len < m {
                out.push(Violation {
                    instance_path: path.to_string(),
                    keyword: "minLength".to_string(),
                    message: format!("string of length {len} is shorter than {m}"),
                });
            }
        }
        if let Some(m) = node.max_length {
            if len > m {
                out.push(Violation {
                    instance_path: path.to_string(),
                    keyword: "maxLength".to_string(),
                    message: format!("string of length {len} is longer than {m}"),
                });
            }
        }
        if let Some(re) = &node.pattern {
            match re.is_match(s) {
                Ok(true) => {}
                Ok(false) => out.push(Violation {
                    instance_path: path.to_string(),
                    keyword: "pattern".to_string(),
                    message: format!("{s:?} does not match /{}/", re.pattern()),
                }),
                Err(e) => out.push(Violation {
                    instance_path: path.to_string(),
                    keyword: "pattern".to_string(),
                    message: format!("/{}/ could not be evaluated: {e}", re.pattern()),
                }),
            }
        }
    }

    /// `prefixItems`, `items`, `minItems`, `maxItems`, `uniqueItems`
    fn check_array(
        &self,
        node: &Node,
        value: &Json,
        path: &str,
        depth: u32,
        out: &mut Vec<Violation>,
    ) {
        let Some(items) = value.as_arr() else { return };
        let len = items.len() as u64;
        if let Some(m) = node.min_items {
            if len < m {
                out.push(Violation {
                    instance_path: path.to_string(),
                    keyword: "minItems".to_string(),
                    message: format!("array of {len} items has fewer than {m}"),
                });
            }
        }
        if let Some(m) = node.max_items {
            if len > m {
                out.push(Violation {
                    instance_path: path.to_string(),
                    keyword: "maxItems".to_string(),
                    message: format!("array of {len} items has more than {m}"),
                });
            }
        }
        if node.unique_items {
            let mut seen: BTreeSet<String> = BTreeSet::new();
            for (i, item) in items.iter().enumerate() {
                if !seen.insert(item.to_canonical_string()) {
                    out.push(Violation {
                        instance_path: format!("{path}/{i}"),
                        keyword: "uniqueItems".to_string(),
                        message: format!("duplicate item {}", item.to_canonical_string()),
                    });
                }
            }
        }
        for (i, item) in items.iter().enumerate() {
            let child = format!("{path}/{i}");
            if let Some(prefix) = node.prefix_items.get(i) {
                self.check(prefix, item, &child, depth, out);
            } else if let Some(items_schema) = &node.items {
                self.check(items_schema, item, &child, depth, out);
            }
        }
    }

    /// `properties`, `required`, `additionalProperties`, `patternProperties`,
    /// `propertyNames`, `minProperties`, `maxProperties`
    fn check_object(
        &self,
        node: &Node,
        value: &Json,
        path: &str,
        depth: u32,
        out: &mut Vec<Violation>,
    ) {
        let Some(map) = value.as_obj() else { return };
        let len = map.len() as u64;
        if let Some(m) = node.min_properties {
            if len < m {
                out.push(Violation {
                    instance_path: path.to_string(),
                    keyword: "minProperties".to_string(),
                    message: format!("object with {len} members has fewer than {m}"),
                });
            }
        }
        if let Some(m) = node.max_properties {
            if len > m {
                out.push(Violation {
                    instance_path: path.to_string(),
                    keyword: "maxProperties".to_string(),
                    message: format!("object with {len} members has more than {m}"),
                });
            }
        }
        for name in &node.required {
            if !map.contains_key(name) {
                out.push(Violation {
                    instance_path: path.to_string(),
                    keyword: "required".to_string(),
                    message: format!("missing required property `{name}`"),
                });
            }
        }

        for (key, member) in map.iter() {
            let child = format!("{path}/{}", escape_pointer(key));

            if let Some(names_schema) = &node.property_names {
                let as_value = Json::Str(key.to_string());
                if !self.matches(names_schema, &as_value, &child, depth) {
                    out.push(Violation {
                        instance_path: child.clone(),
                        keyword: "propertyNames".to_string(),
                        message: format!("property name `{key}` is not permitted"),
                    });
                }
            }

            let mut evaluated = false;
            if let Some((_, sub)) = node.properties.iter().find(|(k, _)| k == key) {
                evaluated = true;
                self.check(sub, member, &child, depth, out);
            }
            for (source, re, sub) in &node.pattern_properties {
                match re.is_match(key) {
                    Ok(true) => {
                        evaluated = true;
                        self.check(sub, member, &child, depth, out);
                    }
                    Ok(false) => {}
                    Err(e) => out.push(Violation {
                        instance_path: child.clone(),
                        keyword: "patternProperties".to_string(),
                        message: format!("/{source}/ could not be evaluated: {e}"),
                    }),
                }
            }
            if !evaluated {
                if let Some(extra) = &node.additional_properties {
                    if extra.boolean == Some(false) {
                        out.push(Violation {
                            instance_path: child.clone(),
                            keyword: "additionalProperties".to_string(),
                            message: format!("property `{key}` is not allowed"),
                        });
                    } else {
                        self.check(extra, member, &child, depth, out);
                    }
                }
            }
        }
    }

    /// `allOf`, `anyOf`, `oneOf`, `not`
    fn check_applicators(
        &self,
        node: &Node,
        value: &Json,
        path: &str,
        depth: u32,
        out: &mut Vec<Violation>,
    ) {
        for sub in &node.all_of {
            self.check(sub, value, path, depth, out);
        }
        if !node.any_of.is_empty()
            && !node
                .any_of
                .iter()
                .any(|s| self.matches(s, value, path, depth))
        {
            out.push(Violation {
                instance_path: path.to_string(),
                keyword: "anyOf".to_string(),
                message: format!(
                    "value matches none of the {} alternatives",
                    node.any_of.len()
                ),
            });
        }
        if !node.one_of.is_empty() {
            let hits: Vec<usize> = node
                .one_of
                .iter()
                .enumerate()
                .filter(|(_, s)| self.matches(s, value, path, depth))
                .map(|(i, _)| i)
                .collect();
            if hits.len() != 1 {
                out.push(Violation {
                    instance_path: path.to_string(),
                    keyword: "oneOf".to_string(),
                    message: if hits.is_empty() {
                        format!(
                            "value matches none of the {} alternatives",
                            node.one_of.len()
                        )
                    } else {
                        format!(
                            "value matches {} alternatives, expected exactly one",
                            hits.len()
                        )
                    },
                });
            }
        }
        if let Some(sub) = &node.not {
            if self.matches(sub, value, path, depth) {
                out.push(Violation {
                    instance_path: path.to_string(),
                    keyword: "not".to_string(),
                    message: "value matches a schema it must not match".to_string(),
                });
            }
        }
    }
}

/// The spelling of a [`JType`] as it appears in a schema.
fn type_name(t: &JType) -> &'static str {
    match t {
        JType::Null => "null",
        JType::Boolean => "boolean",
        JType::Object => "object",
        JType::Array => "array",
        JType::Number => "number",
        JType::String => "string",
        JType::Integer => "integer",
    }
}

/// Escapes a JSON pointer segment per RFC 6901.
fn escape_pointer(segment: &str) -> String {
    segment.replace('~', "~0").replace('/', "~1")
}

/// Exact-enough `multipleOf` test.
///
/// Integral operands compare exactly; fractional ones use a relative tolerance
/// so that `0.3` counts as a multiple of `0.1` despite binary rounding.
fn is_multiple_of(v: f64, m: f64) -> bool {
    if !m.is_finite() || m == 0.0 {
        return false;
    }
    if v.fract() == 0.0 && m.fract() == 0.0 {
        let (vi, mi) = (v as i128, m as i128);
        return mi != 0 && vi % mi == 0;
    }
    let q = v / m;
    if !q.is_finite() {
        return false;
    }
    (q - q.round()).abs() <= 1e-9 * q.abs().max(1.0)
}

/// Compiles one subschema.
fn compile_node(doc: &Json, defs: &BTreeSet<String>, at: &str) -> Result<Node, SchemaError> {
    if let Json::Bool(b) = doc {
        return Ok(Node {
            boolean: Some(*b),
            ..Node::default()
        });
    }
    let map = doc.as_obj().ok_or_else(|| SchemaError {
        message: format!(
            "schema at {at} must be an object or a boolean, found {}",
            doc.type_name()
        ),
    })?;

    let mut node = Node::default();

    if let Some(r) = map.get("$ref") {
        let text = r.as_str().ok_or_else(|| SchemaError {
            message: format!("`$ref` at {at} must be a string"),
        })?;
        node.ref_target = Some(parse_ref(text, defs, at)?);
    }

    if let Some(t) = map.get("type") {
        node.types = Some(compile_types(t, at)?);
    }

    if let Some(e) = map.get("enum") {
        let items = e.as_arr().ok_or_else(|| SchemaError {
            message: format!("`enum` at {at} must be an array"),
        })?;
        if items.is_empty() {
            return Err(SchemaError {
                message: format!("`enum` at {at} must not be empty"),
            });
        }
        node.enum_values = Some(items.to_vec());
    }

    if let Some(c) = map.get("const") {
        node.const_value = Some(c.clone());
    }

    if let Some(p) = map.get("properties") {
        let obj = p.as_obj().ok_or_else(|| SchemaError {
            message: format!("`properties` at {at} must be an object"),
        })?;
        for (k, v) in obj.iter() {
            node.properties.push((
                k.to_string(),
                compile_node(v, defs, &format!("{at}/properties/{k}"))?,
            ));
        }
    }

    if let Some(r) = map.get("required") {
        let items = r.as_arr().ok_or_else(|| SchemaError {
            message: format!("`required` at {at} must be an array"),
        })?;
        for item in items {
            let name = item.as_str().ok_or_else(|| SchemaError {
                message: format!("`required` at {at} must contain only strings"),
            })?;
            node.required.push(name.to_string());
        }
    }

    if let Some(a) = map.get("additionalProperties") {
        node.additional_properties = Some(Box::new(compile_node(
            a,
            defs,
            &format!("{at}/additionalProperties"),
        )?));
    }

    if let Some(p) = map.get("patternProperties") {
        let obj = p.as_obj().ok_or_else(|| SchemaError {
            message: format!("`patternProperties` at {at} must be an object"),
        })?;
        for (k, v) in obj.iter() {
            let re = Regex::new(k).map_err(|e| SchemaError {
                message: format!("`patternProperties` key /{k}/ at {at} is invalid: {e}"),
            })?;
            node.pattern_properties.push((
                k.to_string(),
                re,
                compile_node(v, defs, &format!("{at}/patternProperties/{k}"))?,
            ));
        }
    }

    if let Some(p) = map.get("propertyNames") {
        node.property_names = Some(Box::new(compile_node(
            p,
            defs,
            &format!("{at}/propertyNames"),
        )?));
    }

    if let Some(i) = map.get("items") {
        node.items = Some(Box::new(compile_node(i, defs, &format!("{at}/items"))?));
    }

    if let Some(p) = map.get("prefixItems") {
        let items = p.as_arr().ok_or_else(|| SchemaError {
            message: format!("`prefixItems` at {at} must be an array"),
        })?;
        for (i, item) in items.iter().enumerate() {
            node.prefix_items
                .push(compile_node(item, defs, &format!("{at}/prefixItems/{i}"))?);
        }
    }

    node.min_items = uint_keyword(map, "minItems", at)?;
    node.max_items = uint_keyword(map, "maxItems", at)?;
    node.min_length = uint_keyword(map, "minLength", at)?;
    node.max_length = uint_keyword(map, "maxLength", at)?;
    node.min_properties = uint_keyword(map, "minProperties", at)?;
    node.max_properties = uint_keyword(map, "maxProperties", at)?;

    if let Some(u) = map.get("uniqueItems") {
        node.unique_items = u.as_bool().ok_or_else(|| SchemaError {
            message: format!("`uniqueItems` at {at} must be a boolean"),
        })?;
    }

    node.minimum = num_keyword(map, "minimum", at)?;
    node.maximum = num_keyword(map, "maximum", at)?;
    node.exclusive_minimum = num_keyword(map, "exclusiveMinimum", at)?;
    node.exclusive_maximum = num_keyword(map, "exclusiveMaximum", at)?;
    node.multiple_of = num_keyword(map, "multipleOf", at)?;
    if let Some(m) = node.multiple_of {
        if m <= 0.0 {
            return Err(SchemaError {
                message: format!("`multipleOf` at {at} must be greater than zero"),
            });
        }
    }

    if let Some(p) = map.get("pattern") {
        let text = p.as_str().ok_or_else(|| SchemaError {
            message: format!("`pattern` at {at} must be a string"),
        })?;
        node.pattern = Some(Regex::new(text).map_err(|e| SchemaError {
            message: format!("`pattern` /{text}/ at {at} is invalid: {e}"),
        })?);
    }

    if let Some(f) = map.get("format") {
        // Annotation only: the value must be a string, but is never enforced.
        if f.as_str().is_none() {
            return Err(SchemaError {
                message: format!("`format` at {at} must be a string"),
            });
        }
    }

    node.all_of = compile_list(map, "allOf", defs, at)?;
    node.any_of = compile_list(map, "anyOf", defs, at)?;
    node.one_of = compile_list(map, "oneOf", defs, at)?;

    if let Some(n) = map.get("not") {
        node.not = Some(Box::new(compile_node(n, defs, &format!("{at}/not"))?));
    }

    Ok(node)
}

/// Parses a supported `$ref` string.
fn parse_ref(text: &str, defs: &BTreeSet<String>, at: &str) -> Result<RefTarget, SchemaError> {
    if text == "#" {
        return Ok(RefTarget::Root);
    }
    if let Some(name) = text.strip_prefix("#/$defs/") {
        if name.is_empty() || name.contains('/') {
            return Err(SchemaError {
                message: format!("unsupported `$ref` `{text}` at {at}"),
            });
        }
        if !defs.contains(name) {
            return Err(SchemaError {
                message: format!("`$ref` `{text}` at {at} names an unknown definition"),
            });
        }
        return Ok(RefTarget::Def(name.to_string()));
    }
    Err(SchemaError {
        message: format!(
            "unsupported `$ref` `{text}` at {at}; only `#` and `#/$defs/NAME` are supported"
        ),
    })
}

/// Compiles the `type` keyword, which is a string or an array of strings.
fn compile_types(value: &Json, at: &str) -> Result<Vec<JType>, SchemaError> {
    let bad = |s: &str| SchemaError {
        message: format!("unknown type `{s}` in `type` at {at}"),
    };
    match value {
        Json::Str(s) => Ok(vec![JType::parse(s).ok_or_else(|| bad(s))?]),
        Json::Arr(items) => {
            if items.is_empty() {
                return Err(SchemaError {
                    message: format!("`type` at {at} must not be an empty array"),
                });
            }
            let mut out = Vec::with_capacity(items.len());
            for item in items {
                let s = item.as_str().ok_or_else(|| SchemaError {
                    message: format!("`type` array at {at} must contain only strings"),
                })?;
                out.push(JType::parse(s).ok_or_else(|| bad(s))?);
            }
            Ok(out)
        }
        other => Err(SchemaError {
            message: format!(
                "`type` at {at} must be a string or an array of strings, found {}",
                other.type_name()
            ),
        }),
    }
}

/// Compiles an array-of-subschemas keyword such as `allOf`.
fn compile_list(
    map: &JsonMap,
    keyword: &str,
    defs: &BTreeSet<String>,
    at: &str,
) -> Result<Vec<Node>, SchemaError> {
    let Some(value) = map.get(keyword) else {
        return Ok(Vec::new());
    };
    let items = value.as_arr().ok_or_else(|| SchemaError {
        message: format!("`{keyword}` at {at} must be an array"),
    })?;
    if items.is_empty() {
        return Err(SchemaError {
            message: format!("`{keyword}` at {at} must not be empty"),
        });
    }
    let mut out = Vec::with_capacity(items.len());
    for (i, item) in items.iter().enumerate() {
        out.push(compile_node(item, defs, &format!("{at}/{keyword}/{i}"))?);
    }
    Ok(out)
}

/// Reads a non-negative integer keyword.
fn uint_keyword(map: &JsonMap, keyword: &str, at: &str) -> Result<Option<u64>, SchemaError> {
    let Some(value) = map.get(keyword) else {
        return Ok(None);
    };
    let n = value
        .as_i64()
        .filter(|v| *v >= 0)
        .ok_or_else(|| SchemaError {
            message: format!("`{keyword}` at {at} must be a non-negative integer"),
        })?;
    Ok(Some(n as u64))
}

/// Reads a numeric keyword.
fn num_keyword(map: &JsonMap, keyword: &str, at: &str) -> Result<Option<f64>, SchemaError> {
    let Some(value) = map.get(keyword) else {
        return Ok(None);
    };
    let n = value.as_f64().ok_or_else(|| SchemaError {
        message: format!("`{keyword}` at {at} must be a number"),
    })?;
    Ok(Some(n))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn compile(src: &str) -> Schema {
        let doc = Json::parse(src).unwrap_or_else(|e| panic!("schema json: {e}"));
        Schema::compile(&doc).unwrap_or_else(|e| panic!("compile: {e}"))
    }

    fn compile_err(src: &str) -> SchemaError {
        let doc = Json::parse(src).unwrap_or_else(|e| panic!("schema json: {e}"));
        Schema::compile(&doc).expect_err("should not compile")
    }

    fn ok(schema: &Schema, src: &str) {
        let v = Json::parse(src).unwrap_or_else(|e| panic!("value json: {e}"));
        let violations = schema.validate(&v);
        assert!(
            violations.is_empty(),
            "{src} should be valid: {violations:?}"
        );
        assert!(schema.validate_ok(&v).is_ok());
    }

    fn bad(schema: &Schema, src: &str, keyword: &str) {
        let v = Json::parse(src).unwrap_or_else(|e| panic!("value json: {e}"));
        let violations = schema.validate(&v);
        assert!(
            violations.iter().any(|x| x.keyword == keyword),
            "{src} should fail on `{keyword}`, got {violations:?}"
        );
        assert!(schema.validate_ok(&v).is_err());
    }

    #[test]
    fn boolean_schemas() {
        let t = compile("true");
        ok(&t, "1");
        ok(&t, r#"{"a":1}"#);
        let f = compile("false");
        bad(&f, "1", "false");
        bad(&f, "null", "false");
    }

    #[test]
    fn type_keyword_single_and_array() {
        let s = compile(r#"{"type":"string"}"#);
        ok(&s, r#""x""#);
        bad(&s, "1", "type");
        let multi = compile(r#"{"type":["string","null"]}"#);
        ok(&multi, r#""x""#);
        ok(&multi, "null");
        bad(&multi, "1", "type");
    }

    #[test]
    fn integer_versus_number_types() {
        let i = compile(r#"{"type":"integer"}"#);
        ok(&i, "5");
        ok(&i, "5.0");
        bad(&i, "5.5", "type");
        bad(&i, r#""5""#, "type");
        let n = compile(r#"{"type":"number"}"#);
        ok(&n, "5");
        ok(&n, "5.5");
        bad(&n, "true", "type");
    }

    #[test]
    fn every_type_name_is_recognised() {
        for (name, good, wrong) in [
            ("null", "null", "0"),
            ("boolean", "true", "0"),
            ("object", "{}", "[]"),
            ("array", "[]", "{}"),
            ("string", "\"\"", "0"),
            ("number", "0.5", "\"\""),
            ("integer", "3", "3.5"),
        ] {
            let s = compile(&format!(r#"{{"type":"{name}"}}"#));
            ok(&s, good);
            bad(&s, wrong, "type");
        }
    }

    #[test]
    fn enum_and_const() {
        let e = compile(r#"{"enum":["a","b",3,null]}"#);
        ok(&e, r#""a""#);
        ok(&e, "3");
        ok(&e, "null");
        bad(&e, r#""z""#, "enum");
        let c = compile(r#"{"const":{"a":1,"b":2}}"#);
        ok(&c, r#"{"b":2,"a":1}"#);
        bad(&c, r#"{"a":1}"#, "const");
    }

    #[test]
    fn properties_and_required() {
        let s = compile(
            r#"{"type":"object","properties":{"a":{"type":"integer"},"b":{"type":"string"}},"required":["a"]}"#,
        );
        ok(&s, r#"{"a":1}"#);
        ok(&s, r#"{"a":1,"b":"x"}"#);
        bad(&s, r#"{"b":"x"}"#, "required");
        bad(&s, r#"{"a":"nope"}"#, "type");
    }

    #[test]
    fn additional_properties_false_and_schema() {
        let strict =
            compile(r#"{"type":"object","properties":{"a":{}},"additionalProperties":false}"#);
        ok(&strict, r#"{"a":1}"#);
        bad(&strict, r#"{"a":1,"b":2}"#, "additionalProperties");

        let typed = compile(
            r#"{"type":"object","properties":{"a":{}},"additionalProperties":{"type":"string"}}"#,
        );
        ok(&typed, r#"{"a":1,"b":"x"}"#);
        bad(&typed, r#"{"a":1,"b":2}"#, "type");
    }

    #[test]
    fn pattern_properties() {
        let s = compile(
            r#"{"type":"object","patternProperties":{"^x_":{"type":"integer"}},"additionalProperties":false}"#,
        );
        ok(&s, r#"{"x_a":1,"x_b":2}"#);
        bad(&s, r#"{"x_a":"no"}"#, "type");
        bad(&s, r#"{"y":1}"#, "additionalProperties");
    }

    #[test]
    fn property_names() {
        let s = compile(r#"{"type":"object","propertyNames":{"pattern":"^[a-z_]+$"}}"#);
        ok(&s, r#"{"alpha":1,"be_ta":2}"#);
        bad(&s, r#"{"Alpha":1}"#, "propertyNames");
        let sized = compile(r#"{"propertyNames":{"maxLength":3}}"#);
        ok(&sized, r#"{"abc":1}"#);
        bad(&sized, r#"{"abcd":1}"#, "propertyNames");
    }

    #[test]
    fn min_and_max_properties() {
        let s = compile(r#"{"minProperties":1,"maxProperties":2}"#);
        ok(&s, r#"{"a":1}"#);
        ok(&s, r#"{"a":1,"b":2}"#);
        bad(&s, "{}", "minProperties");
        bad(&s, r#"{"a":1,"b":2,"c":3}"#, "maxProperties");
    }

    #[test]
    fn items_and_prefix_items() {
        let s = compile(r#"{"type":"array","items":{"type":"integer"}}"#);
        ok(&s, "[1,2,3]");
        ok(&s, "[]");
        bad(&s, "[1,\"x\"]", "type");

        let tuple = compile(
            r#"{"type":"array","prefixItems":[{"type":"string"},{"type":"integer"}],"items":{"type":"boolean"}}"#,
        );
        ok(&tuple, r#"["a",1]"#);
        ok(&tuple, r#"["a",1,true,false]"#);
        bad(&tuple, r#"[1,1]"#, "type");
        bad(&tuple, r#"["a",1,"x"]"#, "type");
    }

    #[test]
    fn min_and_max_items() {
        let s = compile(r#"{"minItems":2,"maxItems":3}"#);
        ok(&s, "[1,2]");
        ok(&s, "[1,2,3]");
        bad(&s, "[1]", "minItems");
        bad(&s, "[1,2,3,4]", "maxItems");
    }

    #[test]
    fn unique_items() {
        let s = compile(r#"{"uniqueItems":true}"#);
        ok(&s, "[1,2,3]");
        ok(&s, r#"[{"a":1},{"a":2}]"#);
        bad(&s, "[1,1]", "uniqueItems");
        // Uniqueness is semantic: member order does not make objects distinct.
        bad(&s, r#"[{"a":1,"b":2},{"b":2,"a":1}]"#, "uniqueItems");
        let off = compile(r#"{"uniqueItems":false}"#);
        ok(&off, "[1,1]");
    }

    #[test]
    fn numeric_bounds() {
        let s = compile(r#"{"minimum":0,"maximum":10}"#);
        ok(&s, "0");
        ok(&s, "10");
        ok(&s, "5.5");
        bad(&s, "-0.1", "minimum");
        bad(&s, "10.1", "maximum");
        // Non-numbers are unaffected.
        ok(&s, r#""x""#);
    }

    #[test]
    fn exclusive_numeric_bounds() {
        let s = compile(r#"{"exclusiveMinimum":0,"exclusiveMaximum":1}"#);
        ok(&s, "0.5");
        bad(&s, "0", "exclusiveMinimum");
        bad(&s, "1", "exclusiveMaximum");
    }

    #[test]
    fn multiple_of() {
        let s = compile(r#"{"multipleOf":3}"#);
        ok(&s, "9");
        ok(&s, "0");
        ok(&s, "-6");
        bad(&s, "7", "multipleOf");
        let frac = compile(r#"{"multipleOf":0.1}"#);
        ok(&frac, "0.3");
        ok(&frac, "1.2");
        bad(&frac, "0.35", "multipleOf");
    }

    #[test]
    fn string_length_and_pattern() {
        let s = compile(r#"{"type":"string","minLength":2,"maxLength":4,"pattern":"^[a-z]+$"}"#);
        ok(&s, r#""ab""#);
        ok(&s, r#""abcd""#);
        bad(&s, r#""a""#, "minLength");
        bad(&s, r#""abcde""#, "maxLength");
        bad(&s, r#""AB""#, "pattern");
    }

    #[test]
    fn string_length_counts_code_points() {
        let s = compile(r#"{"maxLength":2}"#);
        ok(&s, "\"\u{e9}\u{4e2d}\"");
        bad(&s, "\"\u{e9}\u{4e2d}x\"", "maxLength");
    }

    #[test]
    fn pattern_backtracking_overflow_becomes_a_violation() {
        let s = compile(r#"{"pattern":"^(a+)+$"}"#);
        let subject = Json::Str("a".repeat(40) + "b");
        let violations = s.validate(&subject);
        assert_eq!(violations.len(), 1, "{violations:?}");
        assert_eq!(violations[0].keyword, "pattern");
        assert!(
            violations[0].message.contains("could not be evaluated"),
            "{}",
            violations[0].message
        );
    }

    #[test]
    fn format_is_annotation_only() {
        let s = compile(r#"{"type":"string","format":"date-time"}"#);
        ok(&s, r#""not a date at all""#);
        assert!(compile_err(r#"{"format":5}"#).message.contains("format"));
    }

    #[test]
    fn all_of_any_of_one_of_and_not() {
        let all = compile(r#"{"allOf":[{"type":"integer"},{"minimum":5}]}"#);
        ok(&all, "5");
        bad(&all, "4", "minimum");
        bad(&all, r#""x""#, "type");

        let any = compile(r#"{"anyOf":[{"type":"string"},{"type":"integer"}]}"#);
        ok(&any, r#""x""#);
        ok(&any, "1");
        bad(&any, "true", "anyOf");

        let not = compile(r#"{"not":{"type":"string"}}"#);
        ok(&not, "1");
        bad(&not, r#""x""#, "not");
    }

    #[test]
    fn one_of_requires_exactly_one_match() {
        let s = compile(r#"{"oneOf":[{"type":"integer"},{"type":"string"}]}"#);
        ok(&s, "1");
        ok(&s, r#""x""#);
        bad(&s, "true", "oneOf");

        // Overlapping alternatives: 5 satisfies both, so oneOf must reject it.
        let overlap = compile(r#"{"oneOf":[{"minimum":0},{"maximum":10}]}"#);
        bad(&overlap, "5", "oneOf");
        ok(&overlap, "-1");
        ok(&overlap, "11");
        let v = overlap.validate(&Json::Int(5));
        assert!(v[0].message.contains("2 alternatives"), "{:?}", v[0]);
    }

    #[test]
    fn ref_to_defs_and_root() {
        let s = compile(
            r##"{
              "$id": "https://qlabs.example/note",
              "$defs": { "pc": { "type":"integer", "minimum":0, "maximum":11 } },
              "type": "object",
              "properties": { "root": { "$ref": "#/$defs/pc" }, "bass": { "$ref": "#/$defs/pc" } },
              "required": ["root"]
            }"##,
        );
        assert_eq!(s.id(), Some("https://qlabs.example/note"));
        ok(&s, r#"{"root":0,"bass":11}"#);
        bad(&s, r#"{"root":12}"#, "maximum");
        bad(&s, r#"{"root":"x"}"#, "type");
    }

    #[test]
    fn recursive_ref_to_root_terminates() {
        let s = compile(
            r##"{
              "type": ["object","integer"],
              "properties": { "child": { "$ref": "#" } }
            }"##,
        );
        ok(&s, "1");
        ok(&s, r#"{"child":{"child":1}}"#);
        bad(&s, r#"{"child":"x"}"#, "type");
    }

    #[test]
    fn ref_alongside_other_keywords_applies_both() {
        let s = compile(
            r##"{
              "$defs": { "small": { "maximum": 10 } },
              "$ref": "#/$defs/small",
              "type": "integer"
            }"##,
        );
        ok(&s, "3");
        bad(&s, "11", "maximum");
        bad(&s, "3.5", "type");
    }

    #[test]
    fn unresolvable_or_unsupported_refs_fail_to_compile() {
        assert!(compile_err(r##"{"$ref":"#/$defs/missing"}"##)
            .message
            .contains("unknown definition"));
        assert!(compile_err(r#"{"$ref":"https://example.com/x"}"#)
            .message
            .contains("unsupported"));
        assert!(compile_err(r##"{"$ref":"#/properties/a"}"##)
            .message
            .contains("unsupported"));
        assert!(compile_err(r#"{"$ref":5}"#).message.contains("$ref"));
    }

    #[test]
    fn malformed_schemas_are_rejected() {
        for src in [
            "5",
            r#""a string""#,
            r#"{"type":"widget"}"#,
            r#"{"type":5}"#,
            r#"{"type":[]}"#,
            r#"{"type":[1]}"#,
            r#"{"enum":5}"#,
            r#"{"enum":[]}"#,
            r#"{"properties":[]}"#,
            r#"{"required":"a"}"#,
            r#"{"required":[1]}"#,
            r#"{"patternProperties":{"[":{}}}"#,
            r#"{"patternProperties":5}"#,
            r#"{"pattern":5}"#,
            r#"{"pattern":"("}"#,
            r#"{"prefixItems":{}}"#,
            r#"{"minItems":-1}"#,
            r#"{"minItems":"x"}"#,
            r#"{"minimum":"x"}"#,
            r#"{"multipleOf":0}"#,
            r#"{"uniqueItems":"yes"}"#,
            r#"{"allOf":{}}"#,
            r#"{"allOf":[]}"#,
            r#"{"oneOf":[5]}"#,
            r#"{"$defs":[]}"#,
            r#"{"not":5}"#,
        ] {
            let e = compile_err(src);
            assert!(!e.message.is_empty(), "{src}");
        }
    }

    #[test]
    fn unknown_keywords_are_ignored() {
        let s = compile(
            r#"{"type":"object","title":"T","description":"D","$comment":"c","examples":[1],"default":0,"deprecated":true}"#,
        );
        ok(&s, "{}");
    }

    #[test]
    fn violation_paths_point_at_the_instance() {
        let s = compile(
            r#"{"type":"object","properties":{"list":{"type":"array","items":{"type":"integer"}}}}"#,
        );
        let v = Json::parse(r#"{"list":[1,"x",3]}"#).expect("value");
        let violations = s.validate(&v);
        assert_eq!(violations.len(), 1);
        assert_eq!(violations[0].instance_path, "/list/1");
        assert_eq!(violations[0].keyword, "type");
        assert!(format!("{}", violations[0]).contains("/list/1"));
    }

    #[test]
    fn violation_paths_escape_pointer_characters() {
        let s = compile(r#"{"additionalProperties":false}"#);
        let v = Json::parse(r#"{"a/b":1}"#).expect("value");
        assert_eq!(s.validate(&v)[0].instance_path, "/a~1b");
    }

    #[test]
    fn schema_without_id_reports_none() {
        assert_eq!(compile("{}").id(), None);
        assert_eq!(compile("true").id(), None);
    }

    #[test]
    fn realistic_profile_schema() {
        let s = compile(
            r##"{
              "$schema": "https://json-schema.org/draft/2020-12/schema",
              "$id": "qlabs://profiles",
              "$defs": {
                "weight": { "type": "number", "minimum": 0, "maximum": 1 }
              },
              "type": "object",
              "required": ["id", "weights"],
              "additionalProperties": false,
              "properties": {
                "id": { "type": "string", "pattern": "^[a-z][a-z0-9_]*$" },
                "weights": {
                  "type": "object",
                  "minProperties": 1,
                  "patternProperties": { "^[a-z_]+$": { "$ref": "#/$defs/weight" } },
                  "additionalProperties": false
                },
                "tags": { "type": "array", "items": { "type": "string" }, "uniqueItems": true }
              }
            }"##,
        );
        ok(
            &s,
            r#"{"id":"jazz_standard","weights":{"melody_fit":0.8},"tags":["a","b"]}"#,
        );
        bad(
            &s,
            r#"{"id":"Jazz","weights":{"melody_fit":0.8}}"#,
            "pattern",
        );
        bad(
            &s,
            r#"{"id":"jazz","weights":{"melody_fit":1.5}}"#,
            "maximum",
        );
        bad(&s, r#"{"id":"jazz","weights":{}}"#, "minProperties");
        bad(
            &s,
            r#"{"id":"jazz","weights":{"a":1},"tags":["x","x"]}"#,
            "uniqueItems",
        );
        bad(
            &s,
            r#"{"id":"jazz","weights":{"a":1},"extra":1}"#,
            "additionalProperties",
        );
        bad(&s, r#"{"weights":{"a":1}}"#, "required");
    }

    #[test]
    fn multiple_violations_are_all_reported() {
        let s = compile(
            r#"{"type":"object","required":["a","b"],"properties":{"c":{"type":"integer"}}}"#,
        );
        let v = Json::parse(r#"{"c":"x"}"#).expect("value");
        let violations = s.validate(&v);
        assert_eq!(violations.len(), 3, "{violations:?}");
        assert_eq!(
            violations
                .iter()
                .filter(|x| x.keyword == "required")
                .count(),
            2
        );
    }

    #[test]
    fn schema_error_and_violation_display() {
        let e = compile_err(r#"{"type":"widget"}"#);
        assert_eq!(format!("{e}"), e.message);
        let v = Violation {
            instance_path: String::new(),
            keyword: "type".into(),
            message: "m".into(),
        };
        assert_eq!(format!("{v}"), "type: m");
    }
}
