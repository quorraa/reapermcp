//! The [`Json`] value type, its parser, and three serializers.
//!
//! # Design notes
//!
//! * Integers and floats are kept apart: `1` parses to [`Json::Int`] and `1.0`
//!   to [`Json::Float`], and both round-trip through every serializer.
//! * Object key order is *insertion order*, preserved by [`JsonMap`], so output
//!   is byte-for-byte deterministic. [`Json::to_canonical_string`] is the
//!   opt-in sorted form used for content hashing.
//! * Floats are written with the shortest decimal text that reparses to exactly
//!   the same bit pattern. Non-finite floats (`NaN`, `±inf`) have no JSON
//!   representation and are serialized as `null`.
//! * Parsing is depth-limited to [`MAX_PARSE_DEPTH`] and rejects trailing
//!   content, `NaN`/`Infinity` literals, unescaped control characters and lone
//!   surrogates.

use std::fmt;

/// Maximum array/object nesting depth accepted by [`Json::parse`].
pub const MAX_PARSE_DEPTH: usize = 128;

/// The category of a [`JsonError`].
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum JsonErrorKind {
    /// The text is not well-formed JSON.
    Syntax,
    /// A value was present but had the wrong type.
    UnexpectedType,
    /// A required object member was absent.
    MissingField,
    /// Nesting exceeded [`MAX_PARSE_DEPTH`].
    DepthLimit,
    /// Well-formed JSON was followed by extra non-whitespace content.
    Trailing,
}

impl JsonErrorKind {
    /// A short stable identifier, handy for machine-readable error payloads.
    pub fn id(self) -> &'static str {
        match self {
            JsonErrorKind::Syntax => "syntax",
            JsonErrorKind::UnexpectedType => "unexpected_type",
            JsonErrorKind::MissingField => "missing_field",
            JsonErrorKind::DepthLimit => "depth_limit",
            JsonErrorKind::Trailing => "trailing",
        }
    }
}

/// An error produced while parsing JSON or extracting a typed field.
#[derive(Clone, Debug, PartialEq)]
pub struct JsonError {
    /// What went wrong.
    pub kind: JsonErrorKind,
    /// JSON-pointer-ish location of the problem, e.g. `/tracks/0/name`.
    /// Empty for the document root.
    pub path: String,
    /// Human-readable description.
    pub message: String,
}

impl JsonError {
    /// Builds an error at the given path.
    pub fn new(kind: JsonErrorKind, path: impl Into<String>, message: impl Into<String>) -> Self {
        JsonError {
            kind,
            path: path.into(),
            message: message.into(),
        }
    }

    /// Prefixes the error path with a parent segment.
    ///
    /// Useful when bubbling an error up through a nested decoder.
    pub fn in_path(mut self, segment: &str) -> Self {
        self.path = format!("/{}{}", escape_pointer(segment), self.path);
        self
    }
}

impl fmt::Display for JsonError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.path.is_empty() {
            write!(f, "json {}: {}", self.kind.id(), self.message)
        } else {
            write!(
                f,
                "json {} at {}: {}",
                self.kind.id(),
                self.path,
                self.message
            )
        }
    }
}

impl std::error::Error for JsonError {}

/// Escapes a path segment per RFC 6901 pointer rules.
fn escape_pointer(segment: &str) -> String {
    segment.replace('~', "~0").replace('/', "~1")
}

/// An insertion-ordered, string-keyed JSON object.
///
/// Iteration and serialization always follow insertion order, which keeps
/// output deterministic. [`JsonMap::insert`] on an existing key replaces the
/// value *in place*, keeping the key's original position.
///
/// Equality is JSON-semantic and therefore order-insensitive: two maps are
/// equal when they hold the same keys mapped to equal values. Use
/// [`Json::to_canonical_string`] when you need order-independent *bytes*.
#[derive(Clone, Default)]
pub struct JsonMap {
    /// The entries, in insertion order.
    entries: Vec<(String, Json)>,
}

impl JsonMap {
    /// Creates an empty map.
    pub fn new() -> Self {
        JsonMap {
            entries: Vec::new(),
        }
    }

    /// Creates an empty map with room for `n` entries.
    pub fn with_capacity(n: usize) -> Self {
        JsonMap {
            entries: Vec::with_capacity(n),
        }
    }

    /// Inserts a key/value pair, returning the previous value if the key existed.
    ///
    /// An existing key keeps its position in the iteration order.
    pub fn insert(&mut self, k: impl Into<String>, v: Json) -> Option<Json> {
        let k = k.into();
        for entry in self.entries.iter_mut() {
            if entry.0 == k {
                return Some(std::mem::replace(&mut entry.1, v));
            }
        }
        self.entries.push((k, v));
        None
    }

    /// Returns a reference to the value for `k`.
    pub fn get(&self, k: &str) -> Option<&Json> {
        self.entries.iter().find(|e| e.0 == k).map(|e| &e.1)
    }

    /// Returns a mutable reference to the value for `k`.
    pub fn get_mut(&mut self, k: &str) -> Option<&mut Json> {
        self.entries.iter_mut().find(|e| e.0 == k).map(|e| &mut e.1)
    }

    /// Removes `k`, returning its value. Remaining entries keep their relative order.
    pub fn remove(&mut self, k: &str) -> Option<Json> {
        let idx = self.entries.iter().position(|e| e.0 == k)?;
        Some(self.entries.remove(idx).1)
    }

    /// Returns `true` if `k` is present.
    pub fn contains_key(&self, k: &str) -> bool {
        self.entries.iter().any(|e| e.0 == k)
    }

    /// Number of entries.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// `true` when the map holds no entries.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Removes every entry.
    pub fn clear(&mut self) {
        self.entries.clear();
    }

    /// Iterates the keys in insertion order.
    pub fn keys(&self) -> impl Iterator<Item = &str> + '_ {
        self.entries.iter().map(|e| e.0.as_str())
    }

    /// Iterates the values in insertion order.
    pub fn values(&self) -> impl Iterator<Item = &Json> + '_ {
        self.entries.iter().map(|e| &e.1)
    }

    /// Iterates key/value pairs in insertion order.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &Json)> + '_ {
        self.entries.iter().map(|e| (e.0.as_str(), &e.1))
    }

    /// Iterates key/value pairs with mutable values, in insertion order.
    pub fn iter_mut(&mut self) -> impl Iterator<Item = (&str, &mut Json)> + '_ {
        self.entries.iter_mut().map(|e| (e.0.as_str(), &mut e.1))
    }

    /// Sorts this map's own keys by Unicode code point.
    ///
    /// Only this level is reordered; nested objects are untouched. Use
    /// [`JsonMap::sort_keys_recursive`] to sort the whole subtree, or
    /// [`Json::to_canonical_string`] to serialize in sorted order without
    /// mutating anything.
    pub fn sort_keys(&mut self) {
        // Rust's `str` ordering is byte-wise over UTF-8, which is the same
        // order as comparing Unicode code points.
        self.entries.sort_by(|a, b| a.0.cmp(&b.0));
    }

    /// Sorts this map's keys and, recursively, the keys of every nested object.
    pub fn sort_keys_recursive(&mut self) {
        self.sort_keys();
        for entry in self.entries.iter_mut() {
            entry.1.sort_keys_recursive();
        }
    }
}

impl fmt::Debug for JsonMap {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_map().entries(self.iter()).finish()
    }
}

impl PartialEq for JsonMap {
    fn eq(&self, other: &Self) -> bool {
        if self.entries.len() != other.entries.len() {
            return false;
        }
        self.entries
            .iter()
            .all(|(k, v)| other.get(k).is_some_and(|o| o == v))
    }
}

impl FromIterator<(String, Json)> for JsonMap {
    fn from_iter<T: IntoIterator<Item = (String, Json)>>(iter: T) -> Self {
        let mut m = JsonMap::new();
        for (k, v) in iter {
            m.insert(k, v);
        }
        m
    }
}

/// Borrowing iterator over a [`JsonMap`].
pub struct JsonMapIter<'a> {
    /// Underlying slice iterator.
    inner: std::slice::Iter<'a, (String, Json)>,
}

impl<'a> Iterator for JsonMapIter<'a> {
    type Item = (&'a str, &'a Json);

    fn next(&mut self) -> Option<Self::Item> {
        self.inner.next().map(|e| (e.0.as_str(), &e.1))
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.inner.size_hint()
    }
}

impl ExactSizeIterator for JsonMapIter<'_> {}

impl<'a> IntoIterator for &'a JsonMap {
    type Item = (&'a str, &'a Json);
    type IntoIter = JsonMapIter<'a>;

    fn into_iter(self) -> Self::IntoIter {
        JsonMapIter {
            inner: self.entries.iter(),
        }
    }
}

/// A JSON value.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum Json {
    /// `null`
    #[default]
    Null,
    /// `true` / `false`
    Bool(bool),
    /// A JSON number written without a fraction or exponent.
    Int(i64),
    /// A JSON number with a fraction or exponent, or one too large for [`i64`].
    Float(f64),
    /// A string.
    Str(String),
    /// An array.
    Arr(Vec<Json>),
    /// An object with insertion-ordered members.
    Obj(JsonMap),
}

impl Json {
    /// Parses a complete JSON document.
    ///
    /// # Errors
    ///
    /// Returns [`JsonError`] with kind [`JsonErrorKind::Syntax`] for malformed
    /// input, [`JsonErrorKind::DepthLimit`] when nesting exceeds
    /// [`MAX_PARSE_DEPTH`], and [`JsonErrorKind::Trailing`] when a valid value
    /// is followed by extra content.
    pub fn parse(s: &str) -> Result<Json, JsonError> {
        let mut p = Parser {
            bytes: s.as_bytes(),
            pos: 0,
            depth: 0,
            path: Vec::new(),
        };
        p.skip_ws();
        let value = p.parse_value()?;
        p.skip_ws();
        if p.pos != p.bytes.len() {
            return Err(p.error(
                JsonErrorKind::Trailing,
                format!(
                    "unexpected trailing content at byte {}: {:?}",
                    p.pos,
                    p.remaining_preview()
                ),
            ));
        }
        Ok(value)
    }

    /// Serializes to compact JSON with no insignificant whitespace.
    // `Display` writes exactly the same bytes; the inherent method is part of
    // the frozen cross-crate API.
    #[allow(clippy::inherent_to_string_shadow_display)]
    pub fn to_string(&self) -> String {
        let mut out = String::new();
        write_compact(self, false, &mut out);
        out
    }

    /// Serializes to indented JSON using two spaces per level.
    ///
    /// No trailing newline is appended.
    pub fn to_string_pretty(&self) -> String {
        let mut out = String::new();
        write_pretty(self, 0, &mut out);
        out
    }

    /// Serializes to the canonical form used for content hashing.
    ///
    /// Object keys are sorted by Unicode code point at every level, there is no
    /// insignificant whitespace, and floats use their shortest round-tripping
    /// representation. Two values that are JSON-equal always produce identical
    /// canonical bytes.
    pub fn to_canonical_string(&self) -> String {
        let mut out = String::new();
        write_compact(self, true, &mut out);
        out
    }

    /// Sorts the keys of this value and every nested object, in place.
    pub fn sort_keys_recursive(&mut self) {
        match self {
            Json::Obj(m) => m.sort_keys_recursive(),
            Json::Arr(items) => {
                for item in items.iter_mut() {
                    item.sort_keys_recursive();
                }
            }
            _ => {}
        }
    }

    /// The JSON type name: one of `null`, `bool`, `integer`, `number`,
    /// `string`, `array`, `object`.
    pub fn type_name(&self) -> &'static str {
        match self {
            Json::Null => "null",
            Json::Bool(_) => "bool",
            Json::Int(_) => "integer",
            Json::Float(_) => "number",
            Json::Str(_) => "string",
            Json::Arr(_) => "array",
            Json::Obj(_) => "object",
        }
    }

    /// `true` when the value is [`Json::Null`].
    pub fn is_null(&self) -> bool {
        matches!(self, Json::Null)
    }

    /// The boolean payload, or `None`.
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Json::Bool(b) => Some(*b),
            _ => None,
        }
    }

    /// The integer payload.
    ///
    /// A [`Json::Float`] whose value is exactly integral and within `i64` range
    /// also converts, so `2.0` reads back as `2`.
    pub fn as_i64(&self) -> Option<i64> {
        match self {
            Json::Int(i) => Some(*i),
            Json::Float(f) => {
                // 2^63 is the first value that no longer fits; the lower bound
                // -2^63 is exactly representable and does fit.
                const LIMIT: f64 = 9_223_372_036_854_775_808.0;
                if f.is_finite() && f.fract() == 0.0 && *f >= -LIMIT && *f < LIMIT {
                    Some(*f as i64)
                } else {
                    None
                }
            }
            _ => None,
        }
    }

    /// The numeric payload as `f64`; integers are widened.
    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Json::Int(i) => Some(*i as f64),
            Json::Float(f) => Some(*f),
            _ => None,
        }
    }

    /// The string payload, or `None`.
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Json::Str(s) => Some(s.as_str()),
            _ => None,
        }
    }

    /// The array payload, or `None`.
    pub fn as_arr(&self) -> Option<&[Json]> {
        match self {
            Json::Arr(a) => Some(a.as_slice()),
            _ => None,
        }
    }

    /// The object payload, or `None`.
    pub fn as_obj(&self) -> Option<&JsonMap> {
        match self {
            Json::Obj(m) => Some(m),
            _ => None,
        }
    }

    /// The object payload as a mutable reference, or `None`.
    pub fn as_obj_mut(&mut self) -> Option<&mut JsonMap> {
        match self {
            Json::Obj(m) => Some(m),
            _ => None,
        }
    }

    /// Looks up an object member; `None` unless this is an object holding `key`.
    pub fn get(&self, key: &str) -> Option<&Json> {
        self.as_obj().and_then(|m| m.get(key))
    }

    /// Looks up an array element; `None` unless this is an array with index `i`.
    pub fn idx(&self, i: usize) -> Option<&Json> {
        self.as_arr().and_then(|a| a.get(i))
    }

    /// Returns a required object member.
    ///
    /// # Errors
    ///
    /// [`JsonErrorKind::UnexpectedType`] if this value is not an object,
    /// [`JsonErrorKind::MissingField`] if `key` is absent.
    pub fn field(&self, key: &str) -> Result<&Json, JsonError> {
        let obj = self.as_obj().ok_or_else(|| {
            JsonError::new(
                JsonErrorKind::UnexpectedType,
                String::new(),
                format!("expected object, found {}", self.type_name()),
            )
        })?;
        obj.get(key).ok_or_else(|| {
            JsonError::new(
                JsonErrorKind::MissingField,
                format!("/{}", escape_pointer(key)),
                format!("missing required field `{key}`"),
            )
        })
    }

    /// Type mismatch error for a named field.
    fn wrong_type(&self, key: &str, want: &str) -> JsonError {
        JsonError::new(
            JsonErrorKind::UnexpectedType,
            format!("/{}", escape_pointer(key)),
            format!(
                "field `{key}` should be {want}, found {}",
                self.get(key).map_or("nothing", Json::type_name)
            ),
        )
    }

    /// Returns a required string member.
    ///
    /// # Errors
    ///
    /// See [`Json::field`]; also [`JsonErrorKind::UnexpectedType`] if the member
    /// is not a string.
    pub fn str_field(&self, key: &str) -> Result<&str, JsonError> {
        self.field(key)?
            .as_str()
            .ok_or_else(|| self.wrong_type(key, "a string"))
    }

    /// Returns a required integer member.
    ///
    /// A float that is exactly integral is accepted.
    ///
    /// # Errors
    ///
    /// See [`Json::field`]; also [`JsonErrorKind::UnexpectedType`] if the member
    /// is not an integer.
    pub fn i64_field(&self, key: &str) -> Result<i64, JsonError> {
        self.field(key)?
            .as_i64()
            .ok_or_else(|| self.wrong_type(key, "an integer"))
    }

    /// Returns a required numeric member as `f64`.
    ///
    /// # Errors
    ///
    /// See [`Json::field`]; also [`JsonErrorKind::UnexpectedType`] if the member
    /// is not a number.
    pub fn f64_field(&self, key: &str) -> Result<f64, JsonError> {
        self.field(key)?
            .as_f64()
            .ok_or_else(|| self.wrong_type(key, "a number"))
    }

    /// Returns a required boolean member.
    ///
    /// # Errors
    ///
    /// See [`Json::field`]; also [`JsonErrorKind::UnexpectedType`] if the member
    /// is not a boolean.
    pub fn bool_field(&self, key: &str) -> Result<bool, JsonError> {
        self.field(key)?
            .as_bool()
            .ok_or_else(|| self.wrong_type(key, "a bool"))
    }

    /// Returns a required array member.
    ///
    /// # Errors
    ///
    /// See [`Json::field`]; also [`JsonErrorKind::UnexpectedType`] if the member
    /// is not an array.
    pub fn arr_field(&self, key: &str) -> Result<&[Json], JsonError> {
        self.field(key)?
            .as_arr()
            .ok_or_else(|| self.wrong_type(key, "an array"))
    }

    /// Returns a required object member.
    ///
    /// # Errors
    ///
    /// See [`Json::field`]; also [`JsonErrorKind::UnexpectedType`] if the member
    /// is not an object.
    pub fn obj_field(&self, key: &str) -> Result<&JsonMap, JsonError> {
        self.field(key)?
            .as_obj()
            .ok_or_else(|| self.wrong_type(key, "an object"))
    }

    /// Looks up an optional string member.
    ///
    /// A missing member and an explicit `null` both yield `Ok(None)`.
    ///
    /// # Errors
    ///
    /// [`JsonErrorKind::UnexpectedType`] if this value is not an object, or the
    /// member is present, non-null and not a string.
    pub fn opt_str_field(&self, key: &str) -> Result<Option<&str>, JsonError> {
        match self.opt_raw(key)? {
            None => Ok(None),
            Some(v) => v
                .as_str()
                .map(Some)
                .ok_or_else(|| self.wrong_type(key, "a string")),
        }
    }

    /// Looks up an optional numeric member.
    ///
    /// # Errors
    ///
    /// As [`Json::opt_str_field`], for numbers.
    pub fn opt_f64_field(&self, key: &str) -> Result<Option<f64>, JsonError> {
        match self.opt_raw(key)? {
            None => Ok(None),
            Some(v) => v
                .as_f64()
                .map(Some)
                .ok_or_else(|| self.wrong_type(key, "a number")),
        }
    }

    /// Looks up an optional integer member.
    ///
    /// # Errors
    ///
    /// As [`Json::opt_str_field`], for integers.
    pub fn opt_i64_field(&self, key: &str) -> Result<Option<i64>, JsonError> {
        match self.opt_raw(key)? {
            None => Ok(None),
            Some(v) => v
                .as_i64()
                .map(Some)
                .ok_or_else(|| self.wrong_type(key, "an integer")),
        }
    }

    /// Looks up an optional boolean member.
    ///
    /// # Errors
    ///
    /// As [`Json::opt_str_field`], for booleans.
    pub fn opt_bool_field(&self, key: &str) -> Result<Option<bool>, JsonError> {
        match self.opt_raw(key)? {
            None => Ok(None),
            Some(v) => v
                .as_bool()
                .map(Some)
                .ok_or_else(|| self.wrong_type(key, "a bool")),
        }
    }

    /// Looks up an optional array member.
    ///
    /// # Errors
    ///
    /// As [`Json::opt_str_field`], for arrays.
    pub fn opt_arr_field(&self, key: &str) -> Result<Option<&[Json]>, JsonError> {
        match self.opt_raw(key)? {
            None => Ok(None),
            Some(v) => v
                .as_arr()
                .map(Some)
                .ok_or_else(|| self.wrong_type(key, "an array")),
        }
    }

    /// Looks up an optional object member.
    ///
    /// # Errors
    ///
    /// As [`Json::opt_str_field`], for objects.
    pub fn opt_obj_field(&self, key: &str) -> Result<Option<&JsonMap>, JsonError> {
        match self.opt_raw(key)? {
            None => Ok(None),
            Some(v) => v
                .as_obj()
                .map(Some)
                .ok_or_else(|| self.wrong_type(key, "an object")),
        }
    }

    /// Shared body of the `opt_*_field` helpers: object check, then
    /// missing-or-null collapses to `None`.
    fn opt_raw(&self, key: &str) -> Result<Option<&Json>, JsonError> {
        let obj = self.as_obj().ok_or_else(|| {
            JsonError::new(
                JsonErrorKind::UnexpectedType,
                String::new(),
                format!("expected object, found {}", self.type_name()),
            )
        })?;
        Ok(obj.get(key).filter(|v| !v.is_null()))
    }
}

impl fmt::Display for Json {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut out = String::new();
        write_compact(self, false, &mut out);
        f.write_str(&out)
    }
}

// ---------------------------------------------------------------------------
// Conversions
// ---------------------------------------------------------------------------

impl From<bool> for Json {
    fn from(v: bool) -> Json {
        Json::Bool(v)
    }
}

/// Generates a `From<$t> for Json` producing [`Json::Int`].
macro_rules! from_int {
    ($($t:ty),* $(,)?) => {
        $(
            impl From<$t> for Json {
                fn from(v: $t) -> Json {
                    Json::Int(i64::from(v))
                }
            }
        )*
    };
}

from_int!(i8, i16, i32, u8, u16, u32);

impl From<i64> for Json {
    fn from(v: i64) -> Json {
        Json::Int(v)
    }
}

impl From<usize> for Json {
    fn from(v: usize) -> Json {
        match i64::try_from(v) {
            Ok(i) => Json::Int(i),
            // Only reachable on platforms where usize exceeds i64::MAX.
            Err(_) => Json::Float(v as f64),
        }
    }
}

impl From<u64> for Json {
    fn from(v: u64) -> Json {
        match i64::try_from(v) {
            Ok(i) => Json::Int(i),
            Err(_) => Json::Float(v as f64),
        }
    }
}

impl From<f64> for Json {
    fn from(v: f64) -> Json {
        Json::Float(v)
    }
}

impl From<f32> for Json {
    fn from(v: f32) -> Json {
        Json::Float(f64::from(v))
    }
}

impl From<String> for Json {
    fn from(v: String) -> Json {
        Json::Str(v)
    }
}

impl From<&str> for Json {
    fn from(v: &str) -> Json {
        Json::Str(v.to_string())
    }
}

impl From<&String> for Json {
    fn from(v: &String) -> Json {
        Json::Str(v.clone())
    }
}

impl From<char> for Json {
    fn from(v: char) -> Json {
        Json::Str(v.to_string())
    }
}

impl From<Vec<Json>> for Json {
    fn from(v: Vec<Json>) -> Json {
        Json::Arr(v)
    }
}

impl From<JsonMap> for Json {
    fn from(v: JsonMap) -> Json {
        Json::Obj(v)
    }
}

impl<T: Into<Json>> From<Option<T>> for Json {
    fn from(v: Option<T>) -> Json {
        match v {
            Some(x) => x.into(),
            None => Json::Null,
        }
    }
}

impl From<()> for Json {
    fn from(_: ()) -> Json {
        Json::Null
    }
}

// ---------------------------------------------------------------------------
// Serialization
// ---------------------------------------------------------------------------

/// Writes `value` compactly, optionally sorting object keys.
fn write_compact(value: &Json, canonical: bool, out: &mut String) {
    match value {
        Json::Null => out.push_str("null"),
        Json::Bool(true) => out.push_str("true"),
        Json::Bool(false) => out.push_str("false"),
        Json::Int(i) => out.push_str(&i.to_string()),
        Json::Float(f) => out.push_str(&format_f64(*f)),
        Json::Str(s) => write_json_string(s, out),
        Json::Arr(items) => {
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_compact(item, canonical, out);
            }
            out.push(']');
        }
        Json::Obj(map) => {
            out.push('{');
            let order = key_order(map, canonical);
            for (n, idx) in order.iter().enumerate() {
                let (k, v) = &map.entries[*idx];
                if n > 0 {
                    out.push(',');
                }
                write_json_string(k, out);
                out.push(':');
                write_compact(v, canonical, out);
            }
            out.push('}');
        }
    }
}

/// Writes `value` with two-space indentation.
fn write_pretty(value: &Json, indent: usize, out: &mut String) {
    match value {
        Json::Arr(items) if !items.is_empty() => {
            out.push_str("[\n");
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push_str(",\n");
                }
                push_indent(out, indent + 1);
                write_pretty(item, indent + 1, out);
            }
            out.push('\n');
            push_indent(out, indent);
            out.push(']');
        }
        Json::Obj(map) if !map.is_empty() => {
            out.push_str("{\n");
            for (i, (k, v)) in map.iter().enumerate() {
                if i > 0 {
                    out.push_str(",\n");
                }
                push_indent(out, indent + 1);
                write_json_string(k, out);
                out.push_str(": ");
                write_pretty(v, indent + 1, out);
            }
            out.push('\n');
            push_indent(out, indent);
            out.push('}');
        }
        other => write_compact(other, false, out),
    }
}

/// Appends `level` levels of two-space indentation.
fn push_indent(out: &mut String, level: usize) {
    for _ in 0..level {
        out.push_str("  ");
    }
}

/// Returns the indices of `map`'s entries in output order.
fn key_order(map: &JsonMap, canonical: bool) -> Vec<usize> {
    let mut order: Vec<usize> = (0..map.entries.len()).collect();
    if canonical {
        order.sort_by(|a, b| map.entries[*a].0.cmp(&map.entries[*b].0));
    }
    order
}

/// Writes a JSON string literal, escaping only what JSON requires.
///
/// `"`, `\` and the C0 control characters are escaped; every other character,
/// including all non-ASCII text, is emitted verbatim as UTF-8.
fn write_json_string(s: &str, out: &mut String) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{08}' => out.push_str("\\b"),
            '\u{0c}' => out.push_str("\\f"),
            c if (c as u32) < 0x20 => {
                out.push_str("\\u");
                let code = c as u32;
                const HEX: &[u8; 16] = b"0123456789abcdef";
                for shift in [12u32, 8, 4, 0] {
                    out.push(HEX[((code >> shift) & 0xf) as usize] as char);
                }
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

/// Renders `v` as the shortest decimal text that reparses to the same bits.
///
/// Non-finite values have no JSON representation and render as `null`.
/// The result always contains `.` or `e`, so it never reparses as an integer.
pub fn format_f64(v: f64) -> String {
    if !v.is_finite() {
        return "null".to_string();
    }

    let mut best: Option<String> = None;
    let mut consider = |cand: String| {
        if !cand.contains(['.', 'e', 'E']) {
            return;
        }
        match cand.parse::<f64>() {
            Ok(back) if back.to_bits() == v.to_bits() => {}
            _ => return,
        }
        match &best {
            Some(b) if b.len() <= cand.len() => {}
            _ => best = Some(cand),
        }
    };

    consider(format!("{v}"));
    consider(format!("{v:?}"));
    for precision in 0..=17usize {
        consider(format!("{:.*e}", precision, v));
    }

    match best {
        Some(b) => b,
        // `{:?}` always round-trips for finite f64; this arm is belt and braces.
        None => {
            let d = format!("{v:?}");
            if d.contains(['.', 'e', 'E']) {
                d
            } else {
                format!("{d}.0")
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Parsing
// ---------------------------------------------------------------------------

/// A path segment recorded while descending into a document.
enum Seg {
    /// An object member name.
    Key(String),
    /// An array index.
    Index(usize),
}

/// Recursive-descent JSON parser over UTF-8 bytes.
struct Parser<'a> {
    /// The document bytes (guaranteed valid UTF-8: the input is a `&str`).
    bytes: &'a [u8],
    /// Current read offset.
    pos: usize,
    /// Current nesting depth.
    depth: usize,
    /// Path to the value currently being parsed.
    path: Vec<Seg>,
}

impl Parser<'_> {
    /// Builds an error carrying the current path.
    fn error(&self, kind: JsonErrorKind, message: impl Into<String>) -> JsonError {
        JsonError::new(kind, self.path_string(), message)
    }

    /// Renders the current path as a JSON pointer.
    fn path_string(&self) -> String {
        let mut s = String::new();
        for seg in &self.path {
            s.push('/');
            match seg {
                Seg::Key(k) => s.push_str(&escape_pointer(k)),
                Seg::Index(i) => s.push_str(&i.to_string()),
            }
        }
        s
    }

    /// A short, printable preview of the unconsumed input.
    fn remaining_preview(&self) -> String {
        let end = (self.pos + 16).min(self.bytes.len());
        String::from_utf8_lossy(&self.bytes[self.pos..end]).into_owned()
    }

    /// Advances past JSON whitespace.
    fn skip_ws(&mut self) {
        while let Some(b) = self.bytes.get(self.pos) {
            match b {
                b' ' | b'\t' | b'\n' | b'\r' => self.pos += 1,
                _ => break,
            }
        }
    }

    /// Returns the byte at the cursor.
    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.pos).copied()
    }

    /// Parses any value at the cursor.
    fn parse_value(&mut self) -> Result<Json, JsonError> {
        match self.peek() {
            None => Err(self.error(JsonErrorKind::Syntax, "unexpected end of input")),
            Some(b'{') => self.parse_object(),
            Some(b'[') => self.parse_array(),
            Some(b'"') => Ok(Json::Str(self.parse_string()?)),
            Some(b't') => self.parse_literal("true", Json::Bool(true)),
            Some(b'f') => self.parse_literal("false", Json::Bool(false)),
            Some(b'n') => self.parse_literal("null", Json::Null),
            Some(b'-') | Some(b'0'..=b'9') => self.parse_number(),
            Some(b) => Err(self.error(
                JsonErrorKind::Syntax,
                format!(
                    "unexpected character {:?} at byte {}",
                    char::from(b),
                    self.pos
                ),
            )),
        }
    }

    /// Matches a bare literal such as `true`.
    fn parse_literal(&mut self, text: &str, value: Json) -> Result<Json, JsonError> {
        let end = self.pos + text.len();
        if end <= self.bytes.len() && &self.bytes[self.pos..end] == text.as_bytes() {
            self.pos = end;
            Ok(value)
        } else {
            Err(self.error(
                JsonErrorKind::Syntax,
                format!(
                    "invalid literal at byte {}; expected `{text}` (JSON has no NaN or Infinity)",
                    self.pos
                ),
            ))
        }
    }

    /// Parses an object.
    fn parse_object(&mut self) -> Result<Json, JsonError> {
        self.enter()?;
        self.pos += 1; // '{'
        let mut map = JsonMap::new();
        self.skip_ws();
        if self.peek() == Some(b'}') {
            self.pos += 1;
            self.depth -= 1;
            return Ok(Json::Obj(map));
        }
        loop {
            self.skip_ws();
            if self.peek() != Some(b'"') {
                return Err(self.error(
                    JsonErrorKind::Syntax,
                    format!("expected a quoted object key at byte {}", self.pos),
                ));
            }
            let key = self.parse_string()?;
            self.skip_ws();
            if self.peek() != Some(b':') {
                return Err(self.error(
                    JsonErrorKind::Syntax,
                    format!("expected ':' after object key at byte {}", self.pos),
                ));
            }
            self.pos += 1;
            self.skip_ws();
            self.path.push(Seg::Key(key.clone()));
            let value = self.parse_value()?;
            self.path.pop();
            map.insert(key, value);
            self.skip_ws();
            match self.peek() {
                Some(b',') => {
                    self.pos += 1;
                }
                Some(b'}') => {
                    self.pos += 1;
                    break;
                }
                _ => {
                    return Err(self.error(
                        JsonErrorKind::Syntax,
                        format!("expected ',' or '}}' in object at byte {}", self.pos),
                    ))
                }
            }
        }
        self.depth -= 1;
        Ok(Json::Obj(map))
    }

    /// Parses an array.
    fn parse_array(&mut self) -> Result<Json, JsonError> {
        self.enter()?;
        self.pos += 1; // '['
        let mut items: Vec<Json> = Vec::new();
        self.skip_ws();
        if self.peek() == Some(b']') {
            self.pos += 1;
            self.depth -= 1;
            return Ok(Json::Arr(items));
        }
        loop {
            self.skip_ws();
            self.path.push(Seg::Index(items.len()));
            let value = self.parse_value()?;
            self.path.pop();
            items.push(value);
            self.skip_ws();
            match self.peek() {
                Some(b',') => {
                    self.pos += 1;
                }
                Some(b']') => {
                    self.pos += 1;
                    break;
                }
                _ => {
                    return Err(self.error(
                        JsonErrorKind::Syntax,
                        format!("expected ',' or ']' in array at byte {}", self.pos),
                    ))
                }
            }
        }
        self.depth -= 1;
        Ok(Json::Arr(items))
    }

    /// Increments the depth counter, enforcing [`MAX_PARSE_DEPTH`].
    fn enter(&mut self) -> Result<(), JsonError> {
        self.depth += 1;
        if self.depth > MAX_PARSE_DEPTH {
            return Err(self.error(
                JsonErrorKind::DepthLimit,
                format!("nesting deeper than the {MAX_PARSE_DEPTH} level limit"),
            ));
        }
        Ok(())
    }

    /// Parses a string literal, resolving escapes and surrogate pairs.
    fn parse_string(&mut self) -> Result<String, JsonError> {
        self.pos += 1; // opening quote
        let mut buf: Vec<u8> = Vec::new();
        let mut run_start = self.pos;
        loop {
            let b = match self.bytes.get(self.pos) {
                Some(b) => *b,
                None => {
                    return Err(self.error(
                        JsonErrorKind::Syntax,
                        "unterminated string literal at end of input",
                    ))
                }
            };
            match b {
                b'"' => {
                    buf.extend_from_slice(&self.bytes[run_start..self.pos]);
                    self.pos += 1;
                    return String::from_utf8(buf).map_err(|_| {
                        self.error(JsonErrorKind::Syntax, "string is not valid UTF-8")
                    });
                }
                b'\\' => {
                    buf.extend_from_slice(&self.bytes[run_start..self.pos]);
                    self.pos += 1;
                    self.parse_escape(&mut buf)?;
                    run_start = self.pos;
                }
                0x00..=0x1f => {
                    return Err(self.error(
                        JsonErrorKind::Syntax,
                        format!(
                            "unescaped control character U+{b:04X} inside string at byte {}",
                            self.pos
                        ),
                    ))
                }
                _ => self.pos += 1,
            }
        }
    }

    /// Parses one backslash escape and appends the decoded text to `buf`.
    fn parse_escape(&mut self, buf: &mut Vec<u8>) -> Result<(), JsonError> {
        let e = match self.bytes.get(self.pos) {
            Some(b) => *b,
            None => {
                return Err(self.error(
                    JsonErrorKind::Syntax,
                    "string ends with an incomplete escape",
                ))
            }
        };
        self.pos += 1;
        match e {
            b'"' => buf.push(b'"'),
            b'\\' => buf.push(b'\\'),
            b'/' => buf.push(b'/'),
            b'b' => buf.push(0x08),
            b'f' => buf.push(0x0c),
            b'n' => buf.push(b'\n'),
            b'r' => buf.push(b'\r'),
            b't' => buf.push(b'\t'),
            b'u' => {
                let first = self.parse_hex4()?;
                let scalar = if (0xd800..0xdc00).contains(&first) {
                    // High surrogate: a low surrogate must follow.
                    if self.bytes.get(self.pos) != Some(&b'\\')
                        || self.bytes.get(self.pos + 1) != Some(&b'u')
                    {
                        return Err(self.error(
                            JsonErrorKind::Syntax,
                            format!(
                                "lone high surrogate \\u{first:04X}; a \\uDC00-\\uDFFF escape must follow"
                            ),
                        ));
                    }
                    self.pos += 2;
                    let second = self.parse_hex4()?;
                    if !(0xdc00..0xe000).contains(&second) {
                        return Err(self.error(
                            JsonErrorKind::Syntax,
                            format!(
                                "\\u{first:04X} must be followed by a low surrogate, found \\u{second:04X}"
                            ),
                        ));
                    }
                    0x10000 + ((first - 0xd800) << 10) + (second - 0xdc00)
                } else if (0xdc00..0xe000).contains(&first) {
                    return Err(self.error(
                        JsonErrorKind::Syntax,
                        format!("lone low surrogate \\u{first:04X}"),
                    ));
                } else {
                    first
                };
                let ch = char::from_u32(scalar).ok_or_else(|| {
                    self.error(
                        JsonErrorKind::Syntax,
                        format!("escape resolves to invalid scalar value U+{scalar:04X}"),
                    )
                })?;
                let mut tmp = [0u8; 4];
                buf.extend_from_slice(ch.encode_utf8(&mut tmp).as_bytes());
            }
            other => {
                return Err(self.error(
                    JsonErrorKind::Syntax,
                    format!(
                        "unsupported escape `\\{}` at byte {}",
                        char::from(other),
                        self.pos - 1
                    ),
                ))
            }
        }
        Ok(())
    }

    /// Reads exactly four hexadecimal digits.
    fn parse_hex4(&mut self) -> Result<u32, JsonError> {
        if self.pos + 4 > self.bytes.len() {
            return Err(self.error(
                JsonErrorKind::Syntax,
                "incomplete \\u escape at end of input",
            ));
        }
        let mut v: u32 = 0;
        for _ in 0..4 {
            let b = self.bytes[self.pos];
            let d = match b {
                b'0'..=b'9' => u32::from(b - b'0'),
                b'a'..=b'f' => u32::from(b - b'a') + 10,
                b'A'..=b'F' => u32::from(b - b'A') + 10,
                _ => {
                    return Err(self.error(
                        JsonErrorKind::Syntax,
                        format!(
                            "invalid hex digit {:?} in \\u escape at byte {}",
                            char::from(b),
                            self.pos
                        ),
                    ))
                }
            };
            v = v * 16 + d;
            self.pos += 1;
        }
        Ok(v)
    }

    /// Parses a number, preserving the integer/float distinction.
    fn parse_number(&mut self) -> Result<Json, JsonError> {
        let start = self.pos;
        if self.peek() == Some(b'-') {
            self.pos += 1;
        }
        // Integer part: `0` alone, or a non-zero digit run. JSON forbids
        // leading zeros such as `01`.
        match self.peek() {
            Some(b'0') => {
                self.pos += 1;
                if matches!(self.peek(), Some(b'0'..=b'9')) {
                    return Err(self.error(
                        JsonErrorKind::Syntax,
                        format!("number at byte {start} has a leading zero"),
                    ));
                }
            }
            Some(b'1'..=b'9') => {
                while matches!(self.peek(), Some(b'0'..=b'9')) {
                    self.pos += 1;
                }
            }
            _ => {
                return Err(self.error(
                    JsonErrorKind::Syntax,
                    format!("expected a digit in number at byte {}", self.pos),
                ))
            }
        }

        let mut is_float = false;
        if self.peek() == Some(b'.') {
            is_float = true;
            self.pos += 1;
            if !matches!(self.peek(), Some(b'0'..=b'9')) {
                return Err(self.error(
                    JsonErrorKind::Syntax,
                    format!("number at byte {start} has no digits after the decimal point"),
                ));
            }
            while matches!(self.peek(), Some(b'0'..=b'9')) {
                self.pos += 1;
            }
        }
        if matches!(self.peek(), Some(b'e') | Some(b'E')) {
            is_float = true;
            self.pos += 1;
            if matches!(self.peek(), Some(b'+') | Some(b'-')) {
                self.pos += 1;
            }
            if !matches!(self.peek(), Some(b'0'..=b'9')) {
                return Err(self.error(
                    JsonErrorKind::Syntax,
                    format!("number at byte {start} has no digits in its exponent"),
                ));
            }
            while matches!(self.peek(), Some(b'0'..=b'9')) {
                self.pos += 1;
            }
        }

        let text = std::str::from_utf8(&self.bytes[start..self.pos])
            .map_err(|_| self.error(JsonErrorKind::Syntax, "number is not valid UTF-8"))?;

        if !is_float {
            if let Ok(i) = text.parse::<i64>() {
                return Ok(Json::Int(i));
            }
            // Integers beyond i64 fall back to f64, as permitted by RFC 8259.
        }
        match text.parse::<f64>() {
            Ok(f) if f.is_finite() => Ok(Json::Float(f)),
            _ => Err(self.error(
                JsonErrorKind::Syntax,
                format!("number `{text}` is outside the range of a 64-bit float"),
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(s: &str) -> Json {
        Json::parse(s).unwrap_or_else(|e| panic!("parse {s:?}: {e}"))
    }

    fn err(s: &str) -> JsonError {
        Json::parse(s).expect_err(s)
    }

    // ---- basic parsing -------------------------------------------------

    #[test]
    fn parses_scalars() {
        assert_eq!(p("null"), Json::Null);
        assert_eq!(p("true"), Json::Bool(true));
        assert_eq!(p("false"), Json::Bool(false));
        assert_eq!(p("42"), Json::Int(42));
        assert_eq!(p("-7"), Json::Int(-7));
        assert_eq!(p("\"hi\""), Json::Str("hi".into()));
    }

    #[test]
    fn parses_whitespace_around_values() {
        assert_eq!(p("  \t\r\n  1  \n"), Json::Int(1));
        assert_eq!(p("{\n  \"a\" : 1\n}"), p("{\"a\":1}"));
        assert_eq!(p("[ 1 , 2 ]"), p("[1,2]"));
    }

    #[test]
    fn parses_nested_structures() {
        let v = p(r#"{"a":[1,{"b":null}],"c":{}}"#);
        assert_eq!(v.get("a").and_then(|a| a.idx(0)), Some(&Json::Int(1)));
        assert_eq!(
            v.get("a").and_then(|a| a.idx(1)).and_then(|o| o.get("b")),
            Some(&Json::Null)
        );
        assert_eq!(v.get("c"), Some(&Json::Obj(JsonMap::new())));
    }

    #[test]
    fn empty_containers() {
        assert_eq!(p("[]"), Json::Arr(vec![]));
        assert_eq!(p("{}"), Json::Obj(JsonMap::new()));
        assert_eq!(p("[]").to_string(), "[]");
        assert_eq!(p("{}").to_string(), "{}");
        assert_eq!(p("[]").to_string_pretty(), "[]");
        assert_eq!(p("{}").to_string_pretty(), "{}");
    }

    // ---- integer / float preservation -----------------------------------

    #[test]
    fn integer_and_float_are_distinct() {
        assert_eq!(p("1"), Json::Int(1));
        assert_eq!(p("1.0"), Json::Float(1.0));
        assert_eq!(p("1e0"), Json::Float(1.0));
        assert_eq!(p("-0"), Json::Int(0));
        assert_eq!(p("1").to_string(), "1");
        assert_eq!(p("1.0").to_string(), "1.0");
        assert_ne!(p("1"), p("1.0"));
        assert_eq!(p("1").type_name(), "integer");
        assert_eq!(p("1.0").type_name(), "number");
    }

    #[test]
    fn big_integers_fall_back_to_float() {
        let v = p("123456789012345678901234567890");
        assert!(matches!(v, Json::Float(_)));
        assert_eq!(p("9223372036854775807"), Json::Int(i64::MAX));
        assert_eq!(p("-9223372036854775808"), Json::Int(i64::MIN));
        assert!(matches!(p("9223372036854775808"), Json::Float(_)));
    }

    #[test]
    fn float_round_trip_for_tricky_values() {
        for v in [
            0.1f64,
            -0.1,
            1.0 / 3.0,
            1e300,
            1e-300,
            1e-320,
            f64::MIN_POSITIVE,
            f64::MAX,
            f64::MIN,
            -0.0,
            0.0,
            2.2250738585072014e-308,
            1.7976931348623157e308,
            123456.789,
            5e-324,
            0.3,
            9007199254740993.0,
        ] {
            let text = format_f64(v);
            let back = Json::parse(&text).unwrap_or_else(|e| panic!("{text}: {e}"));
            let got = back
                .as_f64()
                .unwrap_or_else(|| panic!("{text} not numeric"));
            assert_eq!(got.to_bits(), v.to_bits(), "{v:?} -> {text}");
        }
    }

    #[test]
    fn negative_zero_keeps_its_sign() {
        let s = Json::Float(-0.0).to_string();
        assert_eq!(s, "-0.0");
        let back = p(&s).as_f64().expect("float");
        assert!(back.is_sign_negative());
        assert_eq!(Json::Float(0.0).to_string(), "0.0");
    }

    #[test]
    fn float_uses_short_exponent_form_when_shorter() {
        assert_eq!(format_f64(1e300), "1e300");
        assert_eq!(format_f64(1e-320), "1e-320");
        assert_eq!(format_f64(0.1), "0.1");
        assert_eq!(format_f64(1.0), "1.0");
    }

    #[test]
    fn non_finite_floats_serialize_as_null() {
        assert_eq!(Json::Float(f64::NAN).to_string(), "null");
        assert_eq!(Json::Float(f64::INFINITY).to_string(), "null");
        assert_eq!(Json::Float(f64::NEG_INFINITY).to_string(), "null");
        assert_eq!(format_f64(f64::NAN), "null");
    }

    #[test]
    fn nan_and_infinity_literals_are_rejected() {
        for bad in ["NaN", "Infinity", "-Infinity", "nan", "inf", "-inf"] {
            let e = err(bad);
            assert_eq!(e.kind, JsonErrorKind::Syntax, "{bad}");
        }
    }

    // ---- strings and unicode --------------------------------------------

    #[test]
    fn string_escapes_are_decoded() {
        assert_eq!(p(r#""a\"b""#), Json::Str("a\"b".into()));
        assert_eq!(p(r#""a\\b""#), Json::Str("a\\b".into()));
        assert_eq!(p(r#""a\/b""#), Json::Str("a/b".into()));
        assert_eq!(p(r#""\b\f\n\r\t""#), Json::Str("\u{8}\u{c}\n\r\t".into()));
        assert_eq!(p(r#""\u0041""#), Json::Str("A".into()));
        assert_eq!(p(r#""\u00e9""#), Json::Str("\u{e9}".into()));
        assert_eq!(p(r#""\u4e2d""#), Json::Str("\u{4e2d}".into()));
        assert_eq!(p(r#""\u0000""#), Json::Str("\0".into()));
    }

    #[test]
    fn surrogate_pairs_decode_to_astral_chars() {
        assert_eq!(p(r#""\ud83c\udfb5""#), Json::Str("\u{1f3b5}".into()));
        assert_eq!(p(r#""\uD834\uDD1E""#), Json::Str("\u{1d11e}".into()));
        assert_eq!(p(r#""x\ud83c\udfb5y""#), Json::Str("x\u{1f3b5}y".into()));
    }

    #[test]
    fn lone_surrogates_are_rejected() {
        for bad in [
            r#""\ud83c""#,
            r#""\ud83cx""#,
            r#""\udfb5""#,
            r#""\ud83c\u0041""#,
            r#""\ud83c\ud83c""#,
        ] {
            assert_eq!(err(bad).kind, JsonErrorKind::Syntax, "{bad}");
        }
    }

    #[test]
    fn non_ascii_is_not_escaped_on_output() {
        let v = Json::Str("caf\u{e9} \u{4e2d}\u{6587} \u{1f3b5}".into());
        assert_eq!(v.to_string(), "\"caf\u{e9} \u{4e2d}\u{6587} \u{1f3b5}\"");
        assert_eq!(p(&v.to_string()), v);
    }

    #[test]
    fn control_chars_are_escaped_on_output() {
        let v = Json::Str("a\u{1}b\nc\td\u{8}e\u{c}f\"g\\h".into());
        let s = v.to_string();
        assert_eq!(s, "\"a\\u0001b\\nc\\td\\be\\ff\\\"g\\\\h\"");
        assert_eq!(p(&s), v);
    }

    #[test]
    fn raw_control_chars_inside_strings_are_rejected() {
        assert_eq!(err("\"a\nb\"").kind, JsonErrorKind::Syntax);
        assert_eq!(err("\"a\tb\"").kind, JsonErrorKind::Syntax);
        assert_eq!(err("\"a\u{1}b\"").kind, JsonErrorKind::Syntax);
    }

    // ---- malformed input, one case per error kind -----------------------

    #[test]
    fn syntax_errors_are_reported() {
        for bad in [
            "",
            "   ",
            "{",
            "[",
            "}",
            "]",
            "{\"a\"}",
            "{\"a\":}",
            "{a:1}",
            "{\"a\":1,}",
            "[1,]",
            "[1 2]",
            "\"unterminated",
            "\"bad\\escape\"",
            "\"\\u12\"",
            "\"\\uZZZZ\"",
            "tru",
            "01",
            "-01",
            "1.",
            ".5",
            "1e",
            "1e+",
            "+1",
            "--1",
            "'single'",
        ] {
            let e = err(bad);
            assert_eq!(e.kind, JsonErrorKind::Syntax, "{bad:?} -> {e}");
        }
    }

    #[test]
    fn trailing_content_is_rejected() {
        for bad in ["1 2", "{} {}", "[] x", "null null", "\"a\" \"b\"", "1.2.3"] {
            assert_eq!(err(bad).kind, JsonErrorKind::Trailing, "{bad}");
        }
    }

    #[test]
    fn depth_limit_is_enforced() {
        let ok = "[".repeat(MAX_PARSE_DEPTH) + &"]".repeat(MAX_PARSE_DEPTH);
        assert!(Json::parse(&ok).is_ok());
        let too_deep = "[".repeat(MAX_PARSE_DEPTH + 1) + &"]".repeat(MAX_PARSE_DEPTH + 1);
        assert_eq!(err(&too_deep).kind, JsonErrorKind::DepthLimit);
        let deep_obj =
            "{\"a\":".repeat(MAX_PARSE_DEPTH + 1) + "1" + &"}".repeat(MAX_PARSE_DEPTH + 1);
        assert_eq!(err(&deep_obj).kind, JsonErrorKind::DepthLimit);
    }

    #[test]
    fn error_paths_point_at_the_problem() {
        let e = err(r#"{"a":{"b":[0,1,tru]}}"#);
        assert_eq!(e.path, "/a/b/2");
        let e2 = err(r#"[{"x/y":nul}]"#);
        assert_eq!(e2.path, "/0/x~1y");
        let e3 = err(r#"{"a~b":q}"#);
        assert_eq!(e3.path, "/a~0b");
    }

    #[test]
    fn error_display_and_kind_ids() {
        let e = err("{");
        assert!(format!("{e}").starts_with("json syntax"));
        assert_eq!(JsonErrorKind::MissingField.id(), "missing_field");
        assert_eq!(JsonErrorKind::DepthLimit.id(), "depth_limit");
        assert_eq!(JsonErrorKind::Trailing.id(), "trailing");
        assert_eq!(JsonErrorKind::UnexpectedType.id(), "unexpected_type");
        let nested = JsonError::new(JsonErrorKind::Syntax, "/b", "x").in_path("a");
        assert_eq!(nested.path, "/a/b");
    }

    // ---- serialization ---------------------------------------------------

    #[test]
    fn compact_output_has_no_spaces() {
        let v = p(r#"{ "a" : [ 1 , 2 ] , "b" : { "c" : true } }"#);
        assert_eq!(v.to_string(), r#"{"a":[1,2],"b":{"c":true}}"#);
    }

    #[test]
    fn pretty_output_uses_two_space_indent() {
        let v = p(r#"{"a":[1,2],"b":{},"c":{"d":null}}"#);
        let expected =
            "{\n  \"a\": [\n    1,\n    2\n  ],\n  \"b\": {},\n  \"c\": {\n    \"d\": null\n  }\n}";
        assert_eq!(v.to_string_pretty(), expected);
        assert!(!v.to_string_pretty().ends_with('\n'));
        assert_eq!(p(&v.to_string_pretty()), v);
    }

    #[test]
    fn round_trip_through_all_serializers() {
        let src = r#"{"z":[1,2.5,"x",null,true],"a":{"n":-3,"deep":{"k":[]}},"m":"caf\u00e9"}"#;
        let v = p(src);
        for text in [v.to_string(), v.to_string_pretty(), v.to_canonical_string()] {
            assert_eq!(p(&text), v, "{text}");
        }
    }

    #[test]
    fn canonical_form_sorts_keys_recursively() {
        let v = p(r#"{"b":1,"a":{"z":1,"y":[{"d":1,"c":2}]},"A":3}"#);
        assert_eq!(
            v.to_canonical_string(),
            r#"{"A":3,"a":{"y":[{"c":2,"d":1}],"z":1},"b":1}"#
        );
        // Insertion order is untouched by canonical serialization.
        assert_eq!(
            v.to_string(),
            r#"{"b":1,"a":{"z":1,"y":[{"d":1,"c":2}]},"A":3}"#
        );
    }

    #[test]
    fn canonical_form_is_order_independent() {
        let a = p(r#"{"x":1,"y":{"p":1,"q":2}}"#);
        let b = p(r#"{"y":{"q":2,"p":1},"x":1}"#);
        assert_ne!(a.to_string(), b.to_string());
        assert_eq!(a.to_canonical_string(), b.to_canonical_string());
    }

    #[test]
    fn canonical_key_order_is_by_code_point() {
        let v = p(r#"{"\u00e9":1,"Z":2,"a":3,"A":4,"_":5,"\u4e2d":6}"#);
        let canonical = p(&v.to_canonical_string());
        let keys: Vec<&str> = canonical.as_obj().expect("obj").keys().collect();
        assert_eq!(keys, vec!["A", "Z", "_", "a", "\u{e9}", "\u{4e2d}"]);
    }

    #[test]
    fn display_matches_to_string() {
        let v = p(r#"{"a":[1,"b"]}"#);
        assert_eq!(format!("{v}"), v.to_string());
    }

    // ---- JsonMap semantics -----------------------------------------------

    #[test]
    fn map_preserves_insertion_order() {
        let mut m = JsonMap::new();
        m.insert("z", Json::Int(1));
        m.insert("a", Json::Int(2));
        m.insert("m", Json::Int(3));
        assert_eq!(m.keys().collect::<Vec<_>>(), vec!["z", "a", "m"]);
        assert_eq!(Json::Obj(m).to_string(), r#"{"z":1,"a":2,"m":3}"#);
    }

    #[test]
    fn map_insert_replaces_in_place() {
        let mut m = JsonMap::new();
        m.insert("a", Json::Int(1));
        m.insert("b", Json::Int(2));
        let old = m.insert("a", Json::Int(9));
        assert_eq!(old, Some(Json::Int(1)));
        assert_eq!(m.keys().collect::<Vec<_>>(), vec!["a", "b"]);
        assert_eq!(m.get("a"), Some(&Json::Int(9)));
        assert_eq!(m.len(), 2);
    }

    #[test]
    fn map_get_mut_remove_and_contains() {
        let mut m = JsonMap::new();
        m.insert("a", Json::Int(1));
        m.insert("b", Json::Int(2));
        m.insert("c", Json::Int(3));
        if let Some(v) = m.get_mut("b") {
            *v = Json::Str("two".into());
        }
        assert_eq!(m.get("b"), Some(&Json::Str("two".into())));
        assert!(m.contains_key("c"));
        assert!(!m.contains_key("d"));
        assert_eq!(m.remove("b"), Some(Json::Str("two".into())));
        assert_eq!(m.remove("zz"), None);
        assert_eq!(m.keys().collect::<Vec<_>>(), vec!["a", "c"]);
        assert!(m.get_mut("zz").is_none());
    }

    #[test]
    fn map_len_empty_clear_and_iteration() {
        let mut m = JsonMap::new();
        assert!(m.is_empty());
        assert_eq!(m.len(), 0);
        m.insert("a", Json::Int(1));
        m.insert("b", Json::Int(2));
        assert!(!m.is_empty());
        let pairs: Vec<(&str, &Json)> = m.iter().collect();
        assert_eq!(pairs, vec![("a", &Json::Int(1)), ("b", &Json::Int(2))]);
        let by_ref: Vec<(&str, &Json)> = IntoIterator::into_iter(&m).collect();
        assert_eq!(by_ref, pairs);
        assert_eq!(m.values().count(), 2);
        for (_, v) in m.iter_mut() {
            *v = Json::Null;
        }
        assert_eq!(m.get("a"), Some(&Json::Null));
        m.clear();
        assert!(m.is_empty());
    }

    #[test]
    fn map_sort_keys_is_shallow_and_recursive_variant_is_deep() {
        let mut v = p(r#"{"b":{"z":1,"a":2},"a":1}"#);
        let obj = v.as_obj_mut().expect("obj");
        obj.sort_keys();
        assert_eq!(
            Json::Obj(obj.clone()).to_string(),
            r#"{"a":1,"b":{"z":1,"a":2}}"#
        );
        obj.sort_keys_recursive();
        assert_eq!(
            Json::Obj(obj.clone()).to_string(),
            r#"{"a":1,"b":{"a":2,"z":1}}"#
        );
    }

    #[test]
    fn map_equality_ignores_order_but_not_content() {
        assert_eq!(p(r#"{"a":1,"b":2}"#), p(r#"{"b":2,"a":1}"#));
        assert_ne!(p(r#"{"a":1,"b":2}"#), p(r#"{"a":1,"b":3}"#));
        assert_ne!(p(r#"{"a":1}"#), p(r#"{"a":1,"b":2}"#));
        assert_ne!(p(r#"{"a":1}"#), p(r#"{"b":1}"#));
    }

    #[test]
    fn map_from_iterator_and_capacity() {
        let m: JsonMap = vec![
            ("a".to_string(), Json::Int(1)),
            ("b".to_string(), Json::Int(2)),
            ("a".to_string(), Json::Int(3)),
        ]
        .into_iter()
        .collect();
        assert_eq!(m.len(), 2);
        assert_eq!(m.get("a"), Some(&Json::Int(3)));
        assert_eq!(m.keys().collect::<Vec<_>>(), vec!["a", "b"]);
        assert!(JsonMap::with_capacity(8).is_empty());
    }

    #[test]
    fn duplicate_keys_in_source_keep_the_last_value() {
        let v = p(r#"{"a":1,"a":2}"#);
        assert_eq!(v.get("a"), Some(&Json::Int(2)));
        assert_eq!(v.as_obj().expect("obj").len(), 1);
    }

    #[test]
    fn map_debug_is_readable() {
        let mut m = JsonMap::new();
        m.insert("a", Json::Int(1));
        let s = format!("{m:?}");
        assert!(s.contains("\"a\""), "{s}");
    }

    // ---- accessors --------------------------------------------------------

    #[test]
    fn accessors_are_non_panicking() {
        let v = p(r#"{"s":"x","i":5,"f":1.5,"b":true,"a":[1],"o":{},"n":null}"#);
        assert_eq!(v.get("s").and_then(Json::as_str), Some("x"));
        assert_eq!(v.get("i").and_then(Json::as_i64), Some(5));
        assert_eq!(v.get("i").and_then(Json::as_f64), Some(5.0));
        assert_eq!(v.get("f").and_then(Json::as_f64), Some(1.5));
        assert_eq!(v.get("f").and_then(Json::as_i64), None);
        assert_eq!(v.get("b").and_then(Json::as_bool), Some(true));
        assert_eq!(
            v.get("a").and_then(Json::as_arr).map(<[Json]>::len),
            Some(1)
        );
        assert!(v.get("o").and_then(Json::as_obj).is_some());
        assert!(v.get("n").expect("n").is_null());
        assert_eq!(v.get("missing"), None);
        assert_eq!(v.idx(0), None);
        assert_eq!(Json::Int(1).get("a"), None);
        assert_eq!(p("[10,20]").idx(1), Some(&Json::Int(20)));
        assert_eq!(p("[10,20]").idx(5), None);
        assert_eq!(Json::Float(2.0).as_i64(), Some(2));
        assert_eq!(Json::Str("x".into()).as_f64(), None);
    }

    #[test]
    fn type_names_cover_every_variant() {
        assert_eq!(Json::Null.type_name(), "null");
        assert_eq!(Json::Bool(true).type_name(), "bool");
        assert_eq!(Json::Int(0).type_name(), "integer");
        assert_eq!(Json::Float(0.0).type_name(), "number");
        assert_eq!(Json::Str(String::new()).type_name(), "string");
        assert_eq!(Json::Arr(vec![]).type_name(), "array");
        assert_eq!(Json::Obj(JsonMap::new()).type_name(), "object");
        assert!(Json::default().is_null());
    }

    // ---- strict field extraction -----------------------------------------

    #[test]
    fn field_extractors_succeed() {
        let v = p(r#"{"s":"x","i":5,"f":1.5,"b":true,"a":[1,2],"o":{"k":1}}"#);
        assert_eq!(v.str_field("s").expect("s"), "x");
        assert_eq!(v.i64_field("i").expect("i"), 5);
        assert_eq!(v.f64_field("f").expect("f"), 1.5);
        assert_eq!(v.f64_field("i").expect("i as f64"), 5.0);
        assert!(v.bool_field("b").expect("b"));
        assert_eq!(v.arr_field("a").expect("a").len(), 2);
        assert_eq!(v.obj_field("o").expect("o").len(), 1);
        assert_eq!(v.field("s").expect("field"), &Json::Str("x".into()));
    }

    #[test]
    fn field_extractors_report_missing_fields() {
        let v = p(r#"{"a":1}"#);
        for e in [
            v.field("zz").expect_err("field"),
            v.str_field("zz").expect_err("str"),
            v.i64_field("zz").expect_err("i64"),
            v.f64_field("zz").expect_err("f64"),
            v.bool_field("zz").expect_err("bool"),
            v.arr_field("zz").expect_err("arr"),
            v.obj_field("zz").expect_err("obj"),
        ] {
            assert_eq!(e.kind, JsonErrorKind::MissingField);
            assert_eq!(e.path, "/zz");
            assert!(e.message.contains("zz"));
        }
    }

    #[test]
    fn field_extractors_report_type_mismatches() {
        let v = p(r#"{"s":"x","i":5,"b":true,"a":[],"o":{},"f":1.5}"#);
        for e in [
            v.str_field("i").expect_err("str"),
            v.i64_field("s").expect_err("i64"),
            v.i64_field("f").expect_err("i64 from non-integral float"),
            v.f64_field("s").expect_err("f64"),
            v.bool_field("s").expect_err("bool"),
            v.arr_field("s").expect_err("arr"),
            v.obj_field("s").expect_err("obj"),
        ] {
            assert_eq!(e.kind, JsonErrorKind::UnexpectedType);
        }
    }

    #[test]
    fn field_extraction_on_a_non_object_fails() {
        let v = p("[1,2]");
        let e = v.field("a").expect_err("field");
        assert_eq!(e.kind, JsonErrorKind::UnexpectedType);
        assert!(e.message.contains("array"), "{}", e.message);
        assert_eq!(
            v.opt_str_field("a").expect_err("opt").kind,
            JsonErrorKind::UnexpectedType
        );
    }

    #[test]
    fn optional_field_extractors_succeed() {
        let v = p(r#"{"s":"x","i":5,"f":1.5,"b":false,"n":null,"a":[1],"o":{}}"#);
        assert_eq!(v.opt_str_field("s").expect("s"), Some("x"));
        assert_eq!(v.opt_i64_field("i").expect("i"), Some(5));
        assert_eq!(v.opt_f64_field("f").expect("f"), Some(1.5));
        assert_eq!(v.opt_bool_field("b").expect("b"), Some(false));
        assert_eq!(v.opt_arr_field("a").expect("a").map(<[Json]>::len), Some(1));
        assert!(v.opt_obj_field("o").expect("o").is_some());
    }

    #[test]
    fn optional_field_extractors_treat_missing_and_null_as_none() {
        let v = p(r#"{"n":null}"#);
        assert_eq!(v.opt_str_field("n").expect("n"), None);
        assert_eq!(v.opt_i64_field("n").expect("n"), None);
        assert_eq!(v.opt_f64_field("n").expect("n"), None);
        assert_eq!(v.opt_bool_field("n").expect("n"), None);
        assert_eq!(v.opt_str_field("zz").expect("zz"), None);
        assert_eq!(v.opt_i64_field("zz").expect("zz"), None);
        assert_eq!(v.opt_f64_field("zz").expect("zz"), None);
        assert_eq!(v.opt_bool_field("zz").expect("zz"), None);
    }

    #[test]
    fn optional_field_extractors_reject_wrong_types() {
        let v = p(r#"{"s":"x","i":5}"#);
        assert_eq!(
            v.opt_i64_field("s").expect_err("i64").kind,
            JsonErrorKind::UnexpectedType
        );
        assert_eq!(
            v.opt_str_field("i").expect_err("str").kind,
            JsonErrorKind::UnexpectedType
        );
        assert_eq!(
            v.opt_bool_field("i").expect_err("bool").kind,
            JsonErrorKind::UnexpectedType
        );
        assert_eq!(
            v.opt_f64_field("s").expect_err("f64").kind,
            JsonErrorKind::UnexpectedType
        );
        assert_eq!(
            v.opt_arr_field("s").expect_err("arr").kind,
            JsonErrorKind::UnexpectedType
        );
        assert_eq!(
            v.opt_obj_field("s").expect_err("obj").kind,
            JsonErrorKind::UnexpectedType
        );
    }

    // ---- conversions ------------------------------------------------------

    #[test]
    fn from_impls_cover_the_contract() {
        assert_eq!(Json::from(true), Json::Bool(true));
        assert_eq!(Json::from(5i64), Json::Int(5));
        assert_eq!(Json::from(5i32), Json::Int(5));
        assert_eq!(Json::from(5u32), Json::Int(5));
        assert_eq!(Json::from(5usize), Json::Int(5));
        assert_eq!(Json::from(5u64), Json::Int(5));
        assert_eq!(Json::from(1.5f64), Json::Float(1.5));
        assert_eq!(Json::from(String::from("s")), Json::Str("s".into()));
        assert_eq!(Json::from("s"), Json::Str("s".into()));
        assert_eq!(
            Json::from(vec![Json::Int(1)]),
            Json::Arr(vec![Json::Int(1)])
        );
        assert_eq!(Json::from(JsonMap::new()), Json::Obj(JsonMap::new()));
        assert_eq!(Json::from(Some(3i64)), Json::Int(3));
        assert_eq!(Json::from(None::<i64>), Json::Null);
        assert_eq!(Json::from(Some("x")), Json::Str("x".into()));
        assert_eq!(Json::from(()), Json::Null);
        assert_eq!(Json::from('c'), Json::Str("c".into()));
        assert_eq!(Json::from(&String::from("r")), Json::Str("r".into()));
        assert_eq!(Json::from(u64::MAX), Json::Float(u64::MAX as f64));
    }

    // ---- misc -------------------------------------------------------------

    #[test]
    fn large_document_round_trips() {
        let mut items = Vec::new();
        for i in 0..500 {
            let mut m = JsonMap::new();
            m.insert("i", Json::Int(i));
            m.insert("name", Json::Str(format!("item-{i}")));
            m.insert("ratio", Json::Float(i as f64 / 7.0));
            items.push(Json::Obj(m));
        }
        let doc = Json::Arr(items);
        let text = doc.to_string();
        assert_eq!(Json::parse(&text).expect("reparse"), doc);
        assert_eq!(
            Json::parse(&doc.to_canonical_string()).expect("reparse canonical"),
            doc
        );
    }

    #[test]
    fn float_round_trip_fuzz_over_random_bit_patterns() {
        let mut rng = crate::rng::DetRng::new(0xf10a7);
        let mut checked = 0u32;
        for _ in 0..20_000 {
            let v = f64::from_bits(rng.next_u64());
            if !v.is_finite() {
                continue;
            }
            checked += 1;
            let text = format_f64(v);
            let back = Json::parse(&text).unwrap_or_else(|e| panic!("{v:?} -> {text}: {e}"));
            let got = back
                .as_f64()
                .unwrap_or_else(|| panic!("{text} not numeric"));
            assert_eq!(got.to_bits(), v.to_bits(), "{v:?} -> {text}");
        }
        assert!(checked > 15_000, "only {checked} finite samples");
    }

    #[test]
    fn random_document_round_trip_fuzz() {
        /// Builds a pseudo-random value of bounded depth.
        fn build(rng: &mut crate::rng::DetRng, depth: usize) -> Json {
            let choice = if depth == 0 {
                rng.below(5)
            } else {
                rng.below(7)
            };
            match choice {
                0 => Json::Null,
                1 => Json::Bool(rng.next_u64() % 2 == 0),
                2 => Json::Int(rng.next_u64() as i64),
                3 => Json::Float(rng.next_f64() * 1e6 - 5e5),
                4 => Json::Str(
                    // Mix ASCII, Latin-1, CJK, astral and control characters.
                    [
                        'a',
                        'Z',
                        '9',
                        '\u{e9}',
                        '\u{4e2d}',
                        '\u{1f3b5}',
                        '"',
                        '\\',
                        '\n',
                        '\u{1}',
                    ]
                    .iter()
                    .cycle()
                    .skip(rng.below(10))
                    .take(rng.below(12))
                    .collect(),
                ),
                5 => Json::Arr((0..rng.below(5)).map(|_| build(rng, depth - 1)).collect()),
                _ => {
                    let mut m = JsonMap::new();
                    for i in 0..rng.below(5) {
                        m.insert(format!("k{}{}", i, rng.below(3)), build(rng, depth - 1));
                    }
                    Json::Obj(m)
                }
            }
        }

        let mut rng = crate::rng::DetRng::new(2024);
        for _ in 0..400 {
            let v = build(&mut rng, 4);
            for text in [v.to_string(), v.to_string_pretty(), v.to_canonical_string()] {
                let back = Json::parse(&text).unwrap_or_else(|e| panic!("{text}: {e}"));
                assert_eq!(back, v, "{text}");
            }
            // Canonical form is a stable fingerprint.
            assert_eq!(v.to_canonical_string(), v.clone().to_canonical_string());
        }
    }

    #[test]
    fn canonical_string_is_stable_for_hashing() {
        let a = p(r#"{"b":[1,2],"a":1.5}"#);
        let b = p(r#"{"a":1.50,"b":[1,2]}"#);
        assert_eq!(a.to_canonical_string(), b.to_canonical_string());
        assert_eq!(a.to_canonical_string(), r#"{"a":1.5,"b":[1,2]}"#);
    }

    #[test]
    fn sort_keys_recursive_on_json_walks_arrays() {
        let mut v = p(r#"[{"b":1,"a":2}]"#);
        v.sort_keys_recursive();
        assert_eq!(v.to_string(), r#"[{"a":2,"b":1}]"#);
        let mut scalar = Json::Int(1);
        scalar.sort_keys_recursive();
        assert_eq!(scalar, Json::Int(1));
    }
}
