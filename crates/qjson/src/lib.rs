//! `qjson` — the zero-dependency foundation crate for the QLabs REAPER Music
//! Intelligence MCP server.
//!
//! Everything here is pure safe `std` Rust: no `serde`, no `sha2`, no `uuid`,
//! no `regex`, no build scripts. The crate exists because the product needs
//! byte-level determinism and has a hard no-external-dependency constraint.
//!
//! # Modules
//!
//! | Module | Purpose |
//! |---|---|
//! | [`json`] | the [`Json`] value, its parser, and compact / pretty / canonical serializers |
//! | [`macros`] | the [`json_obj!`] and [`json_arr!`] literal constructors |
//! | [`schema`] | a JSON Schema 2020-12 subset validator |
//! | [`regex`] | a tiny backtracking regex engine (used by `schema`'s `pattern`) |
//! | [`sha256`] | streaming SHA-256 and [`sha256::sha256_hex`] |
//! | [`rng`] | [`rng::DetRng`], a deterministic xoshiro256\*\* generator |
//! | [`uuid`] | deterministic and content-derived UUID-shaped ids |
//! | [`time`] | ISO-8601 UTC formatting and parsing |
//!
//! # Determinism
//!
//! Object member order is insertion order and survives every round trip;
//! [`Json::to_canonical_string`] is the explicitly sorted form used for content
//! hashing; [`rng::DetRng`] and [`uuid::UuidGen::from_seed`] never read the
//! clock, the OS entropy pool or any address-dependent state. Nothing in this
//! crate iterates a `HashMap`.
//!
//! # Example
//!
//! ```
//! use qjson::{json_arr, json_obj, Json};
//!
//! let doc = json_obj! {
//!     "id" => qjson::uuid::uuid_from_name("qlabs.demo", "example"),
//!     "beats" => json_arr![1, 2, 3],
//!     "swing" => 0.56,
//! };
//! let text = doc.to_string();
//! assert_eq!(Json::parse(&text).expect("round trip"), doc);
//!
//! let digest = qjson::sha256::sha256_hex(doc.to_canonical_string().as_bytes());
//! assert_eq!(digest.len(), 64);
//! ```

#![warn(missing_docs)]

pub mod json;
pub mod macros;
pub mod regex;
pub mod rng;
pub mod schema;
pub mod sha256;
pub mod time;
pub mod uuid;

pub use json::{Json, JsonError, JsonErrorKind, JsonMap, MAX_PARSE_DEPTH};

// `json_obj!` and `json_arr!` are `#[macro_export]`ed by `macros`, which places
// them at the crate root: `qjson::json_obj! { .. }`.

#[cfg(test)]
mod contract_surface {
    //! Compile-time assertions that the frozen cross-crate API exists with the
    //! exact signatures the other workspace crates are written against. If any
    //! of these stops compiling, a downstream crate has just been broken.

    use super::*;
    use crate::rng::DetRng;
    use crate::schema::{Schema, SchemaError, Violation};
    use crate::sha256::Sha256;
    use crate::uuid::{uuid_from_name, UuidGen};

    #[test]
    fn json_map_signatures() {
        let _: fn() -> JsonMap = JsonMap::new;
        let _: fn(&mut JsonMap, String, Json) -> Option<Json> = JsonMap::insert;
        let _: fn(&mut JsonMap, &'static str, Json) -> Option<Json> = JsonMap::insert;
        let _: for<'a> fn(&'a JsonMap, &str) -> Option<&'a Json> = JsonMap::get;
        let _: for<'a> fn(&'a mut JsonMap, &str) -> Option<&'a mut Json> = JsonMap::get_mut;
        let _: fn(&mut JsonMap, &str) -> Option<Json> = JsonMap::remove;
        let _: fn(&JsonMap, &str) -> bool = JsonMap::contains_key;
        let _: fn(&JsonMap) -> usize = JsonMap::len;
        let _: fn(&JsonMap) -> bool = JsonMap::is_empty;
        let _: fn(&mut JsonMap) = JsonMap::sort_keys;

        let mut m = JsonMap::new();
        m.insert("k", Json::Null);
        let _: Vec<&str> = m.keys().collect();
        let _: Vec<(&str, &Json)> = m.iter().collect();
        let _: Vec<(&str, &Json)> = (&m).into_iter().collect();
        let _: JsonMap = std::iter::once(("k".to_string(), Json::Null)).collect();
    }

    #[test]
    fn json_signatures() {
        let _: fn(&str) -> Result<Json, JsonError> = Json::parse;
        let _: fn(&Json) -> String = Json::to_string;
        let _: fn(&Json) -> String = Json::to_string_pretty;
        let _: fn(&Json) -> String = Json::to_canonical_string;

        let _: fn(&Json) -> Option<bool> = Json::as_bool;
        let _: fn(&Json) -> Option<i64> = Json::as_i64;
        let _: fn(&Json) -> Option<f64> = Json::as_f64;
        let _: for<'a> fn(&'a Json) -> Option<&'a str> = Json::as_str;
        let _: for<'a> fn(&'a Json) -> Option<&'a [Json]> = Json::as_arr;
        let _: for<'a> fn(&'a Json) -> Option<&'a JsonMap> = Json::as_obj;
        let _: for<'a> fn(&'a Json, &str) -> Option<&'a Json> = Json::get;
        let _: for<'a> fn(&'a Json, usize) -> Option<&'a Json> = Json::idx;
        let _: fn(&Json) -> &'static str = Json::type_name;
        let _: fn(&Json) -> bool = Json::is_null;

        let _: for<'a> fn(&'a Json, &str) -> Result<&'a Json, JsonError> = Json::field;
        let _: for<'a> fn(&'a Json, &str) -> Result<&'a str, JsonError> = Json::str_field;
        let _: fn(&Json, &str) -> Result<i64, JsonError> = Json::i64_field;
        let _: fn(&Json, &str) -> Result<f64, JsonError> = Json::f64_field;
        let _: fn(&Json, &str) -> Result<bool, JsonError> = Json::bool_field;
        let _: for<'a> fn(&'a Json, &str) -> Result<&'a [Json], JsonError> = Json::arr_field;
        let _: for<'a> fn(&'a Json, &str) -> Result<&'a JsonMap, JsonError> = Json::obj_field;
        let _: for<'a> fn(&'a Json, &str) -> Result<Option<&'a str>, JsonError> =
            Json::opt_str_field;
        let _: fn(&Json, &str) -> Result<Option<f64>, JsonError> = Json::opt_f64_field;
        let _: fn(&Json, &str) -> Result<Option<i64>, JsonError> = Json::opt_i64_field;
        let _: fn(&Json, &str) -> Result<Option<bool>, JsonError> = Json::opt_bool_field;

        assert_eq!(MAX_PARSE_DEPTH, 128);
    }

    #[test]
    fn conversion_signatures() {
        let _: fn(bool) -> Json = Json::from;
        let _: fn(i64) -> Json = Json::from;
        let _: fn(i32) -> Json = Json::from;
        let _: fn(u32) -> Json = Json::from;
        let _: fn(usize) -> Json = Json::from;
        let _: fn(f64) -> Json = Json::from;
        let _: fn(String) -> Json = Json::from;
        let _: fn(&'static str) -> Json = Json::from;
        let _: fn(Vec<Json>) -> Json = Json::from;
        let _: fn(JsonMap) -> Json = Json::from;
        let _: fn(Option<i64>) -> Json = Json::from;
        let _: fn(Option<&'static str>) -> Json = Json::from;
    }

    #[test]
    fn error_signatures() {
        let e = JsonError {
            kind: JsonErrorKind::Syntax,
            path: String::new(),
            message: String::new(),
        };
        let _: &dyn std::error::Error = &e;
        let _: String = e.to_string();
        for kind in [
            JsonErrorKind::Syntax,
            JsonErrorKind::UnexpectedType,
            JsonErrorKind::MissingField,
            JsonErrorKind::DepthLimit,
            JsonErrorKind::Trailing,
        ] {
            assert!(!kind.id().is_empty());
        }
    }

    #[test]
    fn schema_signatures() {
        let _: fn(&Json) -> Result<Schema, SchemaError> = Schema::compile;
        let _: fn(&Schema, &Json) -> Vec<Violation> = Schema::validate;
        let _: fn(&Schema, &Json) -> Result<(), Vec<Violation>> = Schema::validate_ok;
        let _: for<'a> fn(&'a Schema) -> Option<&'a str> = Schema::id;
        let v = Violation {
            instance_path: String::new(),
            keyword: String::new(),
            message: String::new(),
        };
        let _ = v.clone();
        let e = SchemaError {
            message: String::new(),
        };
        let _ = e.clone();
    }

    #[test]
    fn hash_rng_uuid_and_time_signatures() {
        let _: fn(&[u8]) -> String = sha256::sha256_hex;
        let _: fn() -> Sha256 = Sha256::new;
        let _: fn(&mut Sha256, &[u8]) = Sha256::update;
        let _: fn(Sha256) -> String = Sha256::finish_hex;
        let _: fn(Sha256) -> [u8; 32] = Sha256::finish;

        let _: fn(u64) -> DetRng = DetRng::new;
        let _: fn(&DetRng, &str) -> DetRng = DetRng::derive;
        let _: fn(&mut DetRng) -> u64 = DetRng::next_u64;
        let _: fn(&mut DetRng) -> f64 = DetRng::next_f64;
        let _: fn(&mut DetRng, usize) -> usize = DetRng::below;
        let _: fn(&mut DetRng, &mut [u8]) = DetRng::shuffle;

        let _: fn(u64) -> UuidGen = UuidGen::from_seed;
        let _: fn() -> UuidGen = UuidGen::process_unique;
        let _: fn(&UuidGen) -> String = UuidGen::next;
        let _: fn(&str, &str) -> String = uuid_from_name;

        let _: fn() -> String = time::now_iso8601;
        let _: fn(i64) -> String = time::iso8601_from_unix;
        let _: fn() -> i64 = time::unix_now;
        let _: fn(&str) -> Option<i64> = time::parse_iso8601;
    }

    #[test]
    fn macros_are_reachable_from_the_crate_root() {
        let v = crate::json_obj! { "a" => crate::json_arr![1, 2, 3] };
        assert_eq!(v.to_string(), r#"{"a":[1,2,3]}"#);
    }
}
