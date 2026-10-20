//! The normalised entity.
//!
//! Everything serialises to JSON with a stable shape: top-level fields are
//! always present (`null`, `[]` or `{}` when empty), maps keep a
//! deterministic order (languages in the requested order, properties in
//! dump order), and every claim value is tagged with its `type`.

use serde::ser::{SerializeMap, SerializeStruct};
use serde::{Serialize, Serializer};

/// An ordered map, serialised as a JSON object.
#[derive(Clone, Debug, PartialEq)]
pub struct Map<K, V>(pub Vec<(K, V)>);

impl<K, V> Default for Map<K, V> {
    fn default() -> Self {
        Map(Vec::new())
    }
}

impl<K: PartialEq, V> Map<K, V> {
    /// The value for `key`.
    pub fn get<Q>(&self, key: &Q) -> Option<&V>
    where
        K: PartialEq<Q>,
        Q: ?Sized,
    {
        self.0.iter().find(|(k, _)| k == key).map(|(_, v)| v)
    }

    /// Number of entries.
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Whether there are no entries.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// The entries.
    pub fn iter(&self) -> impl Iterator<Item = &(K, V)> {
        self.0.iter()
    }
}

impl<K: Serialize, V: Serialize> Serialize for Map<K, V> {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let mut m = s.serialize_map(Some(self.0.len()))?;
        for (k, v) in &self.0 {
            m.serialize_entry(k, v)?;
        }
        m.end()
    }
}

/// A property id (`P31` is `Pid(31)`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Pid(pub u32);

impl Pid {
    /// Parse `P123`.
    pub fn parse(s: &str) -> Option<Pid> {
        let d = s.strip_prefix('P').or_else(|| s.strip_prefix('p'))?;
        if d.is_empty() || d.len() > 10 || !d.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        d.parse().ok().map(Pid)
    }
}

impl std::fmt::Display for Pid {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "P{}", self.0)
    }
}

impl Serialize for Pid {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_str(self)
    }
}

impl PartialEq<str> for Pid {
    fn eq(&self, other: &str) -> bool {
        Pid::parse(other) == Some(*self)
    }
}

/// Kinds of entity.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum EntityType {
    /// `Q…`
    #[default]
    Item,
    /// `P…`
    Property,
    /// `L…`
    Lexeme,
    /// `M…` (Commons structured data)
    #[serde(rename = "mediainfo")]
    MediaInfo,
    /// `E…`
    #[serde(rename = "entity-schema")]
    EntitySchema,
}

impl EntityType {
    /// From the dump's `type` field.
    pub fn parse(s: &str) -> Option<EntityType> {
        Some(match s {
            "item" => EntityType::Item,
            "property" => EntityType::Property,
            "lexeme" => EntityType::Lexeme,
            "mediainfo" => EntityType::MediaInfo,
            "entity-schema" | "entityschema" => EntityType::EntitySchema,
            _ => return None,
        })
    }

    /// As written in the dump.
    pub fn as_str(self) -> &'static str {
        match self {
            EntityType::Item => "item",
            EntityType::Property => "property",
            EntityType::Lexeme => "lexeme",
            EntityType::MediaInfo => "mediainfo",
            EntityType::EntitySchema => "entity-schema",
        }
    }

    /// The id letter (`Q`, `P`, `L`, `M`, `E`).
    pub fn letter(self) -> u8 {
        match self {
            EntityType::Item => b'Q',
            EntityType::Property => b'P',
            EntityType::Lexeme => b'L',
            EntityType::MediaInfo => b'M',
            EntityType::EntitySchema => b'E',
        }
    }

    /// The type of an id letter.
    pub fn from_letter(l: u8) -> Option<EntityType> {
        Some(match l {
            b'Q' => EntityType::Item,
            b'P' => EntityType::Property,
            b'L' => EntityType::Lexeme,
            b'M' => EntityType::MediaInfo,
            b'E' => EntityType::EntitySchema,
            _ => return None,
        })
    }

    /// A small code (0–4) for columnar output.
    pub fn code(self) -> u8 {
        match self {
            EntityType::Item => 0,
            EntityType::Property => 1,
            EntityType::Lexeme => 2,
            EntityType::MediaInfo => 3,
            EntityType::EntitySchema => 4,
        }
    }
}

/// A text in a language.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Text {
    /// The text.
    pub text: String,
    /// Its language code.
    pub language: String,
}

/// Statement rank.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Rank {
    /// Preferred.
    Preferred,
    /// Normal.
    #[default]
    Normal,
    /// Deprecated.
    Deprecated,
}

/// A point in time, as Wikidata records it.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Time {
    /// The raw timestamp, e.g. `+1830-10-04T00:00:00Z`.
    pub time: String,
    /// The date at its precision: `1830-10-04` (day), `1830-10` (month),
    /// `1830` (year and coarser; negative years before the common era).
    pub iso: String,
    /// Wikidata's precision code (`11` day, `10` month, `9` year, `8`
    /// decade, `7` century, `6` millennium, … `0` billion years).
    pub precision: u8,
    /// The precision's name (`day`, `month`, `year`, `decade`, …).
    pub precision_name: &'static str,
    /// The year.
    pub year: i64,
    /// The month, at month precision or finer.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub month: Option<u8>,
    /// The day, at day precision or finer.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub day: Option<u8>,
    /// `gregorian`, `julian`, or the calendar's item id.
    pub calendar: String,
}

/// An amount, with an optional unit and bounds.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Quantity {
    /// The amount.
    pub amount: f64,
    /// The unit's item id (`None`: a pure number).
    pub unit: Option<String>,
    /// Lower bound.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lower: Option<f64>,
    /// Upper bound.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub upper: Option<f64>,
}

/// A point on a globe.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Coordinate {
    /// Latitude in degrees.
    pub lat: f64,
    /// Longitude in degrees.
    pub lon: f64,
    /// Precision in degrees, when recorded.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub precision: Option<f64>,
    /// The globe's item id (`Q2` is the Earth).
    pub globe: String,
}

/// A typed claim value.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum Value {
    /// A reference to an item, property, lexeme, form, sense or schema.
    Entity {
        /// Its id (`Q5`, `P31`, `L7`, `L7-F1`, `L7-S1`, `E10`).
        id: String,
    },
    /// A plain string.
    String {
        /// The string.
        value: String,
    },
    /// An identifier in another database.
    ExternalId {
        /// The identifier.
        value: String,
    },
    /// A URL.
    Url {
        /// The URL.
        value: String,
    },
    /// A file on Wikimedia Commons.
    Media {
        /// The file name (`Foo.jpg`).
        file: String,
        /// Its Commons page.
        url: String,
    },
    /// A text in a language.
    Monolingual {
        /// The text.
        text: String,
        /// Its language code.
        language: String,
    },
    /// A point in time.
    Time(Time),
    /// An amount.
    Quantity(Quantity),
    /// A point on a globe.
    Coordinate(Coordinate),
    /// A TeX formula.
    Math {
        /// The formula.
        value: String,
    },
    /// LilyPond notation.
    MusicalNotation {
        /// The notation.
        value: String,
    },
    /// A map on Commons (`Data:….map`).
    GeoShape {
        /// The page name.
        value: String,
        /// Its Commons page.
        url: String,
    },
    /// A table on Commons (`Data:….tab`).
    TabularData {
        /// The page name.
        value: String,
        /// Its Commons page.
        url: String,
    },
    /// Some value exists, but it is not known.
    SomeValue,
    /// There is no value.
    NoValue,
}

impl Value {
    /// The entity id of an entity value.
    pub fn entity_id(&self) -> Option<&str> {
        match self {
            Value::Entity { id } => Some(id),
            _ => None,
        }
    }

    /// The string of a string-like value (string, external id, url,
    /// math, musical notation, geo-shape, tabular data; a media file's name).
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::String { value }
            | Value::ExternalId { value }
            | Value::Url { value }
            | Value::Math { value }
            | Value::MusicalNotation { value }
            | Value::GeoShape { value, .. }
            | Value::TabularData { value, .. } => Some(value),
            Value::Media { file, .. } => Some(file),
            Value::Monolingual { text, .. } => Some(text),
            Value::Entity { id } => Some(id),
            _ => None,
        }
    }
}

/// Property → values (qualifiers, and the snaks of a reference).
pub type Snaks = Map<Pid, Vec<Value>>;

/// A statement: a value, its rank, qualifiers and references.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Statement {
    /// The value.
    pub value: Value,
    /// The rank.
    pub rank: Rank,
    /// Qualifiers (with `qualifiers`, the default).
    #[serde(skip_serializing_if = "Map::is_empty")]
    pub qualifiers: Snaks,
    /// References (with `references`; off by default).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub references: Vec<Snaks>,
}

/// A link to a page on a Wikimedia site.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Sitelink {
    /// The page title.
    pub title: String,
    /// The page URL, for known sites.
    pub url: Option<String>,
    /// Badges (item ids, e.g. `Q17437796` featured article).
    pub badges: Vec<String>,
}

/// Lexeme fields.
#[derive(Clone, Debug, PartialEq, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LexemeData {
    /// Lemmas by language.
    pub lemmas: Map<String, String>,
    /// The lexical category (item id).
    pub lexical_category: Option<String>,
    /// The language (item id).
    pub language: Option<String>,
    /// Forms.
    pub forms: Vec<Form>,
    /// Senses.
    pub senses: Vec<Sense>,
}

/// A lexeme form.
#[derive(Clone, Debug, PartialEq, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Form {
    /// `L7-F1`.
    pub id: String,
    /// Representations by language.
    pub representations: Map<String, String>,
    /// Grammatical features (item ids).
    pub grammatical_features: Vec<String>,
    /// Claims.
    pub claims: Map<Pid, Vec<Statement>>,
}

/// A lexeme sense.
#[derive(Clone, Debug, PartialEq, Default, Serialize)]
pub struct Sense {
    /// `L7-S1`.
    pub id: String,
    /// Glosses by language.
    pub glosses: Map<String, String>,
    /// Claims.
    pub claims: Map<Pid, Vec<Statement>>,
}

/// Fields every consumer wants, computed from the best-ranked ("truthy")
/// statements: preferred ones when there are any, else normal ones. They
/// are computed whatever `properties` selects.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Derived {
    /// P31 instance of.
    pub instance_of: Vec<String>,
    /// P279 subclass of.
    pub subclass_of: Vec<String>,
    /// P17 country.
    pub country: Vec<String>,
    /// P625 coordinate location (the first).
    pub coordinates: Option<(f64, f64)>,
    /// P571 inception, year.
    pub inception: Option<i64>,
    /// P576 dissolved, abolished or demolished, year.
    pub dissolved: Option<i64>,
    /// P18 image, Commons file name.
    pub image: Option<String>,
    /// P856 official website.
    pub website: Option<String>,
    /// Identifiers in other databases (see [`crate::EXTERNAL_IDS`]), in
    /// table order.
    pub external_ids: Vec<(&'static str, String)>,
}

/// A normalised entity.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Entity {
    /// `Q42`, `P31`, `L7`, …
    pub id: String,
    /// The kind.
    pub kind: EntityType,
    /// Properties: the datatype (`wikibase-item`, `external-id`, …).
    pub datatype: Option<String>,
    /// The label in the first language of the fallback chain that has one.
    pub label: Option<Text>,
    /// The description, likewise.
    pub description: Option<Text>,
    /// Labels by language (the requested languages only).
    pub labels: Map<String, String>,
    /// Descriptions by language.
    pub descriptions: Map<String, String>,
    /// Aliases by language.
    pub aliases: Map<String, Vec<String>>,
    /// Sitelinks by site (`enwiki`, …).
    pub sitelinks: Map<String, Sitelink>,
    /// Claims by property, in dump order.
    pub claims: Map<Pid, Vec<Statement>>,
    /// Derived fields.
    pub derived: Derived,
    /// Lexemes only.
    pub lexeme: Option<LexemeData>,
    /// Last revision id.
    pub lastrevid: Option<u64>,
    /// Last modification time.
    pub modified: Option<String>,
}

struct Coordinates(f64, f64);

impl Serialize for Coordinates {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let mut st = s.serialize_struct("Coordinates", 2)?;
        st.serialize_field("lat", &self.0)?;
        st.serialize_field("lon", &self.1)?;
        st.end()
    }
}

struct ExternalIds<'a>(&'a [(&'static str, String)]);

impl Serialize for ExternalIds<'_> {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let mut m = s.serialize_map(Some(self.0.len()))?;
        for (k, v) in self.0 {
            m.serialize_entry(k, v)?;
        }
        m.end()
    }
}

impl Serialize for Entity {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let d = &self.derived;
        let mut st = s.serialize_struct("Entity", 24)?;
        st.serialize_field("id", &self.id)?;
        st.serialize_field("type", &self.kind)?;
        if self.kind == EntityType::Property {
            st.serialize_field("datatype", &self.datatype)?;
        }
        st.serialize_field("label", &self.label)?;
        st.serialize_field("description", &self.description)?;
        st.serialize_field("instanceOf", &d.instance_of)?;
        st.serialize_field("subclassOf", &d.subclass_of)?;
        st.serialize_field("country", &d.country)?;
        st.serialize_field("coordinates", &d.coordinates.map(|(a, b)| Coordinates(a, b)))?;
        st.serialize_field("inception", &d.inception)?;
        st.serialize_field("dissolved", &d.dissolved)?;
        st.serialize_field("image", &d.image)?;
        st.serialize_field("website", &d.website)?;
        st.serialize_field("externalIds", &ExternalIds(&d.external_ids))?;
        st.serialize_field("labels", &self.labels)?;
        st.serialize_field("descriptions", &self.descriptions)?;
        st.serialize_field("aliases", &self.aliases)?;
        st.serialize_field("sitelinks", &self.sitelinks)?;
        st.serialize_field("claims", &self.claims)?;
        if let Some(l) = &self.lexeme {
            st.serialize_field("lemmas", &l.lemmas)?;
            st.serialize_field("lexicalCategory", &l.lexical_category)?;
            st.serialize_field("language", &l.language)?;
            st.serialize_field("forms", &l.forms)?;
            st.serialize_field("senses", &l.senses)?;
        }
        st.serialize_field("lastrevid", &self.lastrevid)?;
        st.serialize_field("modified", &self.modified)?;
        st.end()
    }
}

impl Entity {
    /// The statements of a property.
    pub fn statements(&self, p: Pid) -> &[Statement] {
        self.claims.get(&p).map(Vec::as_slice).unwrap_or(&[])
    }

    /// Serialise as one line of JSON into `out`.
    pub fn write_json(&self, out: &mut Vec<u8>) {
        serde_json::to_writer(out, self).expect("serialising to memory cannot fail");
    }

    /// As a JSON string.
    pub fn to_json(&self) -> String {
        let mut v = Vec::with_capacity(1024);
        self.write_json(&mut v);
        String::from_utf8(v).expect("serde_json writes UTF-8")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pids() {
        assert_eq!(Pid::parse("P31"), Some(Pid(31)));
        assert_eq!(Pid::parse("Q31"), None);
        assert_eq!(Pid::parse("P"), None);
        assert_eq!(Pid::parse("P3x"), None);
        assert_eq!(Pid(17).to_string(), "P17");
        assert_eq!(serde_json::to_string(&Pid(17)).unwrap(), "\"P17\"");
    }

    #[test]
    fn values_are_tagged() {
        let v = Value::Entity { id: "Q5".into() };
        assert_eq!(serde_json::to_string(&v).unwrap(), r#"{"type":"entity","id":"Q5"}"#);
        assert_eq!(
            serde_json::to_string(&Value::SomeValue).unwrap(),
            r#"{"type":"someValue"}"#
        );
        let q = Value::Quantity(Quantity {
            amount: 3.5,
            unit: None,
            lower: None,
            upper: Some(4.0),
        });
        assert_eq!(
            serde_json::to_string(&q).unwrap(),
            r#"{"type":"quantity","amount":3.5,"unit":null,"upper":4.0}"#
        );
    }
}
