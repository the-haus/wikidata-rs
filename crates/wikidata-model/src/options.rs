//! What to keep of an entity.

use rustc_hash::FxHashMap;

use crate::entity::Pid;
use crate::external::EXTERNAL_IDS;

/// What [`crate::parse_entity`] keeps. Everything left out is skipped
/// while parsing, without allocating.
#[derive(Clone, Debug)]
pub struct ModelOptions {
    /// Languages to keep for labels, descriptions, aliases and glosses,
    /// in fallback order (`["nl", "en", "mul"]`): `label` and
    /// `description` take the first that exists. `None`: all languages,
    /// with `["en", "mul"]` as the fallback chain.
    pub languages: Option<Vec<String>>,
    /// Sites to keep sitelinks for (`["nlwiki", "enwiki"]`). `None`: all.
    pub sitelinks: Option<Vec<String>>,
    /// More sites for the items of some classes (every Wikipedia for
    /// artists), on top of `sitelinks`. See [`SitesFor`].
    pub sitelinks_for: Option<SitesFor>,
    /// Count each entity's sitelinks, the pruned ones included, into
    /// [`crate::Entity::sitelink_count`] and
    /// [`crate::Entity::wikipedia_count`] (default false). The skipped
    /// sitelinks are counted from their keys, without allocating.
    pub sitelink_counts: bool,
    /// Properties to keep claims for. `None`: all.
    pub properties: Option<Vec<Pid>>,
    /// Keep labels (default true).
    pub labels: bool,
    /// Keep descriptions (default true).
    pub descriptions: bool,
    /// Keep aliases (default true).
    pub aliases: bool,
    /// Keep qualifiers (default true).
    pub qualifiers: bool,
    /// With `qualifiers`, keep only the qualifiers of these properties
    /// (`[P585, P580, P582]`); the others are skipped while parsing.
    /// `None`: all.
    pub qualifier_properties: Option<Vec<Pid>>,
    /// Keep references (default false).
    pub references: bool,
    /// Keep deprecated statements (default false: dropped).
    pub deprecated: bool,
    /// Keep only the best-ranked statements of each property: the
    /// preferred ones if there are any, else the normal ones ("truthy").
    pub best_rank: bool,
    /// Compute the derived fields (default true).
    pub derived: bool,
}

impl Default for ModelOptions {
    fn default() -> Self {
        Self {
            languages: None,
            sitelinks: None,
            sitelinks_for: None,
            sitelink_counts: false,
            properties: None,
            labels: true,
            descriptions: true,
            aliases: true,
            qualifiers: true,
            qualifier_properties: None,
            references: false,
            deprecated: false,
            best_rank: false,
            derived: true,
        }
    }
}

/// Sites kept for the items of some classes, on top of what `sitelinks`
/// keeps for every entity: with `instance_of` `[Q5, Q215380]` and
/// `groups` `[wikipedia]`, a human or a musical group keeps every
/// Wikipedia sitelink and any other item only `sitelinks`. The classes
/// are matched against the truthy P31 statements (not their subclasses),
/// read before the sitelinks, so the other items' extra sitelinks are
/// skipped without allocating.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SitesFor {
    /// The classes (`Q5`), as values of P31.
    pub instance_of: Vec<String>,
    /// The site groups to keep (see [`crate::sites::SITE_GROUPS`]); empty:
    /// every site.
    pub groups: Vec<String>,
}

impl SitesFor {
    /// Check the groups.
    pub fn validate(&self) -> Result<(), String> {
        for g in &self.groups {
            if !crate::sites::SITE_GROUPS.contains(&g.as_str()) {
                return Err(format!(
                    "unknown site group {g:?} (expected one of {})",
                    crate::sites::SITE_GROUPS.join(", ")
                ));
            }
        }
        Ok(())
    }

    /// Whether an item with these P31 values is one of the classes.
    pub fn matches<'a>(&self, instance_of: impl IntoIterator<Item = &'a str>) -> bool {
        instance_of.into_iter().any(|c| self.instance_of.iter().any(|x| x == c))
    }

    /// Whether a site is in the groups.
    #[inline]
    pub fn keeps_site(&self, site: &str) -> bool {
        match self.groups.as_slice() {
            [] => true,
            [g] if g == "wikipedia" => crate::sites::is_wikipedia(site),
            gs => crate::sites::site_group(site).is_some_and(|s| gs.iter().any(|g| g == s)),
        }
    }
}

/// Which derived field a property feeds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Slot {
    InstanceOf,
    SubclassOf,
    Country,
    Coordinates,
    Inception,
    Dissolved,
    Image,
    Website,
    External(u16),
}

/// The derived fields' properties.
pub(crate) const DERIVED: [(u32, Slot); 8] = [
    (31, Slot::InstanceOf),
    (279, Slot::SubclassOf),
    (17, Slot::Country),
    (625, Slot::Coordinates),
    (571, Slot::Inception),
    (576, Slot::Dissolved),
    (18, Slot::Image),
    (856, Slot::Website),
];

/// [`ModelOptions`] prepared for parsing.
#[derive(Clone, Debug)]
pub struct Compiled {
    pub(crate) opts: ModelOptions,
    pub(crate) fallback: Vec<String>,
    pub(crate) properties: Option<rustc_hash::FxHashSet<u32>>,
    pub(crate) qualifier_properties: Option<rustc_hash::FxHashSet<u32>>,
    pub(crate) slots: FxHashMap<u32, Slot>,
    pub(crate) backend: crate::Backend,
}

impl Compiled {
    /// Prepare `opts`.
    pub fn new(opts: ModelOptions) -> Compiled {
        let fallback = opts
            .languages
            .clone()
            .unwrap_or_else(|| vec!["en".to_string(), "mul".to_string()]);
        let properties = opts.properties.as_ref().map(|p| p.iter().map(|p| p.0).collect());
        let qualifier_properties = opts
            .qualifier_properties
            .as_ref()
            .map(|p| p.iter().map(|p| p.0).collect());
        let mut slots = FxHashMap::default();
        if opts.derived {
            for (p, s) in DERIVED {
                slots.insert(p, s);
            }
            for (i, e) in EXTERNAL_IDS.iter().enumerate() {
                slots.insert(e.property.0, Slot::External(i as u16));
            }
        }
        let selective = opts.languages.is_some() || opts.properties.is_some();
        #[cfg(feature = "sonic")]
        let backend = if selective {
            crate::Backend::SerdeJson
        } else {
            crate::Backend::Sonic
        };
        #[cfg(not(feature = "sonic"))]
        let backend = {
            let _ = selective;
            crate::Backend::SerdeJson
        };
        Compiled {
            opts,
            fallback,
            properties,
            qualifier_properties,
            slots,
            backend,
        }
    }

    /// The options.
    pub fn options(&self) -> &ModelOptions {
        &self.opts
    }

    /// Whether claims of `p` are kept in `claims`.
    #[inline]
    pub fn keeps_property(&self, p: u32) -> bool {
        self.properties.as_ref().is_none_or(|s| s.contains(&p))
    }

    /// The index of a language in `languages` (`Some(0)` for any language
    /// when all are kept); `None` when it is not kept.
    #[inline]
    pub(crate) fn language(&self, lang: &str) -> Option<usize> {
        match &self.opts.languages {
            None => Some(0),
            Some(l) => l.iter().position(|x| x == lang),
        }
    }

    /// Whether a sitelink is kept for every entity (`sitelinks`; see
    /// [`ModelOptions::sitelinks_for`] for the items of some classes).
    #[inline]
    pub fn keeps_site(&self, site: &str) -> bool {
        self.opts.sitelinks.as_ref().is_none_or(|s| s.iter().any(|x| x == site))
    }
}

impl Default for Compiled {
    fn default() -> Self {
        Compiled::new(ModelOptions::default())
    }
}

impl From<ModelOptions> for Compiled {
    fn from(o: ModelOptions) -> Self {
        Compiled::new(o)
    }
}
