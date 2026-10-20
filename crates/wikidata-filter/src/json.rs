//! JSON as written, objects in their key order: the order of named filters
//! (`{"any": {"venue": …, "festival": …}}`) is the order they are reported
//! in. (`serde_json::Value` sorts keys unless its `preserve_order` feature
//! is on, which would change map order for every crate of a build.)

use std::fmt;

use serde::de::{Deserialize, Deserializer, MapAccess, SeqAccess, Visitor};

/// A JSON value.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Json {
    Null,
    Bool(bool),
    Number(serde_json::Number),
    String(String),
    Array(Vec<Json>),
    Object(Obj),
}

/// A JSON object, in key order.
#[derive(Clone, Debug, PartialEq, Default)]
pub(crate) struct Obj(Vec<(String, Json)>);

impl Obj {
    pub(crate) fn get(&self, key: &str) -> Option<&Json> {
        self.0.iter().find(|(k, _)| k == key).map(|(_, v)| v)
    }

    pub(crate) fn contains_key(&self, key: &str) -> bool {
        self.get(key).is_some()
    }

    pub(crate) fn iter(&self) -> std::slice::Iter<'_, (String, Json)> {
        self.0.iter()
    }
}

impl<'a> IntoIterator for &'a Obj {
    type Item = &'a (String, Json);
    type IntoIter = std::slice::Iter<'a, (String, Json)>;
    fn into_iter(self) -> Self::IntoIter {
        self.0.iter()
    }
}

impl std::ops::Index<&str> for Obj {
    type Output = Json;
    fn index(&self, key: &str) -> &Json {
        static NULL: Json = Json::Null;
        self.get(key).unwrap_or(&NULL)
    }
}

impl Json {
    pub(crate) fn as_object(&self) -> Option<&Obj> {
        match self {
            Json::Object(o) => Some(o),
            _ => None,
        }
    }

    pub(crate) fn as_str(&self) -> Option<&str> {
        match self {
            Json::String(s) => Some(s),
            _ => None,
        }
    }

    pub(crate) fn as_u64(&self) -> Option<u64> {
        match self {
            Json::Number(n) => n.as_u64(),
            _ => None,
        }
    }

    pub(crate) fn as_bool(&self) -> Option<bool> {
        match self {
            Json::Bool(b) => Some(*b),
            _ => None,
        }
    }

    pub(crate) fn get(&self, key: &str) -> Option<&Json> {
        self.as_object().and_then(|o| o.get(key))
    }
}

impl From<&serde_json::Value> for Json {
    fn from(v: &serde_json::Value) -> Json {
        use serde_json::Value as V;
        match v {
            V::Null => Json::Null,
            V::Bool(b) => Json::Bool(*b),
            V::Number(n) => Json::Number(n.clone()),
            V::String(s) => Json::String(s.clone()),
            V::Array(a) => Json::Array(a.iter().map(Json::from).collect()),
            V::Object(o) => Json::Object(Obj(o.iter().map(|(k, v)| (k.clone(), Json::from(v))).collect())),
        }
    }
}

impl<'de> Deserialize<'de> for Json {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Json, D::Error> {
        struct V;
        impl<'de> Visitor<'de> for V {
            type Value = Json;
            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("JSON")
            }
            fn visit_unit<E>(self) -> Result<Json, E> {
                Ok(Json::Null)
            }
            fn visit_bool<E>(self, b: bool) -> Result<Json, E> {
                Ok(Json::Bool(b))
            }
            fn visit_i64<E>(self, n: i64) -> Result<Json, E> {
                Ok(Json::Number(n.into()))
            }
            fn visit_u64<E>(self, n: u64) -> Result<Json, E> {
                Ok(Json::Number(n.into()))
            }
            fn visit_f64<E>(self, n: f64) -> Result<Json, E> {
                Ok(serde_json::Number::from_f64(n).map_or(Json::Null, Json::Number))
            }
            fn visit_str<E>(self, s: &str) -> Result<Json, E> {
                Ok(Json::String(s.to_string()))
            }
            fn visit_string<E>(self, s: String) -> Result<Json, E> {
                Ok(Json::String(s))
            }
            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Json, A::Error> {
                let mut a = Vec::new();
                while let Some(v) = seq.next_element()? {
                    a.push(v);
                }
                Ok(Json::Array(a))
            }
            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Json, A::Error> {
                let mut o = Vec::new();
                while let Some((k, v)) = map.next_entry::<String, Json>()? {
                    o.push((k, v));
                }
                Ok(Json::Object(Obj(o)))
            }
        }
        d.deserialize_any(V)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_key_order() {
        let v: Json = serde_json::from_str(r#"{"b":1,"a":[true,null,"x",-2,1.5]}"#).unwrap();
        let keys: Vec<&str> = v.as_object().unwrap().iter().map(|(k, _)| k.as_str()).collect();
        assert_eq!(keys, ["b", "a"]);
        assert_eq!(v.get("b").and_then(Json::as_u64), Some(1));
        assert!(matches!(v.get("a"), Some(Json::Array(a)) if a.len() == 5));
    }
}
