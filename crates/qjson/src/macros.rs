//! Literal constructors for [`Json`](crate::Json) values.
//!
//! Both macros are exported at the crate root, so downstream crates write
//! `qjson::json_obj!{ ... }` and `qjson::json_arr![ ... ]`.
//!
//! ```
//! use qjson::{json_arr, json_obj, Json};
//!
//! let doc = json_obj! {
//!     "name" => "voicing",
//!     "steps" => json_arr![1, 2, 3],
//!     "weight" => 0.75,
//!     "enabled" => true,
//!     "parent" => None::<&str>,
//! };
//! assert_eq!(doc.str_field("name").expect("name"), "voicing");
//! assert!(doc.get("parent").expect("parent").is_null());
//! ```
//!
//! Values may be anything implementing `Into<Json>`; keys anything implementing
//! `Into<String>`. A trailing comma is always accepted.

/// Builds a [`Json::Obj`](crate::Json::Obj) from `"key" => value` pairs.
///
/// Keys accept any `Into<String>`, values any `Into<Json>`. A trailing comma is
/// optional. Repeating a key replaces the earlier value while keeping the key's
/// original position, matching [`JsonMap::insert`](crate::JsonMap::insert).
///
/// ```
/// use qjson::json_obj;
/// let v = json_obj! { "a" => 1, "b" => "x", };
/// assert_eq!(v.to_string(), r#"{"a":1,"b":"x"}"#);
/// assert_eq!(json_obj!{}.to_string(), "{}");
/// ```
#[macro_export]
macro_rules! json_obj {
    () => {
        $crate::Json::Obj($crate::JsonMap::new())
    };
    ($($key:expr => $value:expr),+ $(,)?) => {{
        let mut map = $crate::JsonMap::new();
        $(
            map.insert($key, $crate::Json::from($value));
        )+
        $crate::Json::Obj(map)
    }};
}

/// Builds a [`Json::Arr`](crate::Json::Arr) from a list of `Into<Json>` values.
///
/// Supports the `[value; count]` repetition form as well as a plain list, with
/// an optional trailing comma.
///
/// ```
/// use qjson::json_arr;
/// assert_eq!(json_arr![1, 2, 3].to_string(), "[1,2,3]");
/// assert_eq!(json_arr![].to_string(), "[]");
/// assert_eq!(json_arr!["x"; 2].to_string(), r#"["x","x"]"#);
/// ```
#[macro_export]
macro_rules! json_arr {
    () => {
        $crate::Json::Arr(::std::vec::Vec::new())
    };
    ($value:expr; $count:expr) => {
        $crate::Json::Arr(::std::vec![$crate::Json::from($value); $count])
    };
    ($($value:expr),+ $(,)?) => {
        $crate::Json::Arr(::std::vec![$($crate::Json::from($value)),+])
    };
}

#[cfg(test)]
mod tests {
    use crate::{Json, JsonMap};

    #[test]
    fn json_obj_builds_objects() {
        let v = json_obj! { "a" => 1, "b" => "x", "c" => true };
        assert_eq!(v.to_string(), r#"{"a":1,"b":"x","c":true}"#);
    }

    #[test]
    fn json_obj_accepts_trailing_comma_and_empty() {
        let v = json_obj! { "a" => 1, };
        assert_eq!(v.to_string(), r#"{"a":1}"#);
        assert_eq!(json_obj! {}, Json::Obj(JsonMap::new()));
    }

    #[test]
    fn json_arr_builds_arrays() {
        assert_eq!(json_arr![1, 2, 3].to_string(), "[1,2,3]");
        assert_eq!(json_arr![].to_string(), "[]");
        assert_eq!(
            json_arr![1, "a", true, 1.5,].to_string(),
            r#"[1,"a",true,1.5]"#
        );
        assert_eq!(json_arr![0u32; 3].to_string(), "[0,0,0]");
    }

    #[test]
    fn macros_nest_and_accept_any_into_json() {
        let v = json_obj! {
            "name" => String::from("x"),
            "nested" => json_obj! { "k" => json_arr![1, 2] },
            "list" => json_arr![json_obj!{ "i" => 0usize }],
            "opt" => None::<i64>,
            "some" => Some(4i64),
            "f" => 2.5f64,
        };
        assert_eq!(
            v.to_string(),
            r#"{"name":"x","nested":{"k":[1,2]},"list":[{"i":0}],"opt":null,"some":4,"f":2.5}"#
        );
    }

    #[test]
    fn json_obj_keys_may_be_computed() {
        let key = format!("k{}", 1);
        let v = json_obj! { key => 1, "b" => 2 };
        assert_eq!(v.to_string(), r#"{"k1":1,"b":2}"#);
    }

    #[test]
    fn repeated_keys_replace_in_place() {
        let v = json_obj! { "a" => 1, "b" => 2, "a" => 3 };
        assert_eq!(v.to_string(), r#"{"a":3,"b":2}"#);
    }
}
