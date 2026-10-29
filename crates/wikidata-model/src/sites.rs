//! Page URLs for sitelinks and Commons files, and the site groups.

/// Wikimedia projects by their site-id suffix.
const PROJECTS: [(&str, &str); 8] = [
    ("wiki", "wikipedia.org"),
    ("wikivoyage", "wikivoyage.org"),
    ("wikiquote", "wikiquote.org"),
    ("wikisource", "wikisource.org"),
    ("wikinews", "wikinews.org"),
    ("wikibooks", "wikibooks.org"),
    ("wikiversity", "wikiversity.org"),
    ("wiktionary", "wiktionary.org"),
];

/// Sites that are not `<language><project>` (the `special` group).
const SPECIAL: [(&str, &str); 11] = [
    ("commonswiki", "commons.wikimedia.org"),
    ("specieswiki", "species.wikimedia.org"),
    ("metawiki", "meta.wikimedia.org"),
    ("mediawikiwiki", "www.mediawiki.org"),
    ("wikidatawiki", "www.wikidata.org"),
    ("sourceswiki", "wikisource.org"),
    ("incubatorwiki", "incubator.wikimedia.org"),
    ("outreachwiki", "outreach.wikimedia.org"),
    ("wikimaniawiki", "wikimania.wikimedia.org"),
    ("wikifunctionswiki", "www.wikifunctions.org"),
    ("foundationwiki", "foundation.wikimedia.org"),
];

/// A `<language><project>` site: its language code (`zh_yue`), project
/// suffix and domain.
fn project(site: &str) -> Option<(&str, &'static str, &'static str)> {
    if SPECIAL.iter().any(|(s, _)| *s == site) {
        return None;
    }
    // The longest suffix first: `wiki` is a suffix of the others' ids too.
    let mut best: Option<(&str, &str)> = None;
    for (suffix, domain) in PROJECTS {
        if site.len() > suffix.len() && site.ends_with(suffix) && best.is_none_or(|(s, _)| suffix.len() > s.len()) {
            best = Some((suffix, domain));
        }
    }
    let (suffix, domain) = best?;
    let lang = &site[..site.len() - suffix.len()];
    lang.bytes()
        .all(|b| b.is_ascii_lowercase() || b == b'_')
        .then_some((lang, suffix, domain))
}

/// The site groups, as Wikidata names them: one per project
/// (`wikipedia`, `wikivoyage`, `wikiquote`, `wikisource`, `wikinews`,
/// `wikibooks`, `wikiversity`, `wiktionary`) and `special` for Commons,
/// Wikispecies, Wikidata, Meta and the other sites of [`site_group`].
pub const SITE_GROUPS: [&str; 9] = [
    "wikipedia",
    "wikivoyage",
    "wikiquote",
    "wikisource",
    "wikinews",
    "wikibooks",
    "wikiversity",
    "wiktionary",
    "special",
];

/// The group of a site id: `nlwiki` → `wikipedia`, `dewikivoyage` →
/// `wikivoyage`, `commonswiki` → `special` (likewise `specieswiki`,
/// `metawiki`, `mediawikiwiki`, `wikidatawiki`, `sourceswiki`,
/// `incubatorwiki`, `outreachwiki`, `wikimaniawiki`, `wikifunctionswiki`,
/// `foundationwiki`); `None` for an id that is neither.
pub fn site_group(site: &str) -> Option<&'static str> {
    if SPECIAL.iter().any(|(s, _)| *s == site) {
        return Some("special");
    }
    project(site).map(|(_, _, domain)| domain.trim_end_matches(".org"))
}

/// Whether a site id is a Wikipedia (`<code>wiki`, not one of the special
/// sites): what [`crate::Entity::wikipedia_count`] counts.
#[inline]
pub fn is_wikipedia(site: &str) -> bool {
    site.len() > 4
        && site.ends_with("wiki")
        && !SPECIAL.iter().any(|(s, _)| *s == site)
        && site[..site.len() - 4]
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b == b'_')
}

/// The host of a site id (`nlwiki` → `nl.wikipedia.org`).
pub fn site_host(site: &str) -> Option<String> {
    if let Some((_, host)) = SPECIAL.iter().find(|(s, _)| *s == site) {
        return Some((*host).to_string());
    }
    let (lang, _, domain) = project(site)?;
    Some(format!("{}.{domain}", lang.replace('_', "-")))
}

/// Percent-encode a page title the way MediaWiki links do (spaces become
/// underscores).
pub fn encode_title(title: &str) -> String {
    let mut out = String::with_capacity(title.len() + 8);
    for &b in title.as_bytes() {
        match b {
            b' ' => out.push('_'),
            b'A'..=b'Z'
            | b'a'..=b'z'
            | b'0'..=b'9'
            | b'-'
            | b'.'
            | b'_'
            | b'~'
            | b':'
            | b'/'
            | b'!'
            | b'$'
            | b'('
            | b')'
            | b'*'
            | b','
            | b';'
            | b'@'
            | b'\'' => out.push(b as char),
            _ => {
                out.push('%');
                out.push(char::from(b"0123456789ABCDEF"[(b >> 4) as usize]));
                out.push(char::from(b"0123456789ABCDEF"[(b & 15) as usize]));
            }
        }
    }
    out
}

/// The URL of a page on a site.
pub fn sitelink_url(site: &str, title: &str) -> Option<String> {
    Some(format!("https://{}/wiki/{}", site_host(site)?, encode_title(title)))
}

/// The Commons page of a file.
pub fn commons_file_url(file: &str) -> String {
    format!("https://commons.wikimedia.org/wiki/File:{}", encode_title(file))
}

/// The Commons page of a `Data:` page (geo-shapes, tabular data).
pub fn commons_page_url(page: &str) -> String {
    format!("https://commons.wikimedia.org/wiki/{}", encode_title(page))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hosts() {
        assert_eq!(site_host("nlwiki").as_deref(), Some("nl.wikipedia.org"));
        assert_eq!(site_host("enwikivoyage").as_deref(), Some("en.wikivoyage.org"));
        assert_eq!(site_host("zh_yuewiki").as_deref(), Some("zh-yue.wikipedia.org"));
        assert_eq!(site_host("commonswiki").as_deref(), Some("commons.wikimedia.org"));
        assert_eq!(site_host("dewiktionary").as_deref(), Some("de.wiktionary.org"));
        assert_eq!(site_host("wiki"), None);
    }

    #[test]
    fn groups() {
        assert_eq!(site_group("nlwiki"), Some("wikipedia"));
        assert_eq!(site_group("be_x_oldwiki"), Some("wikipedia"));
        assert_eq!(site_group("simplewiki"), Some("wikipedia"));
        assert_eq!(site_group("dewikivoyage"), Some("wikivoyage"));
        assert_eq!(site_group("enwiktionary"), Some("wiktionary"));
        assert_eq!(site_group("frwikisource"), Some("wikisource"));
        for s in [
            "commonswiki",
            "specieswiki",
            "wikidatawiki",
            "metawiki",
            "mediawikiwiki",
            "sourceswiki",
            "wikimaniawiki",
            "foundationwiki",
            "outreachwiki",
            "incubatorwiki",
            "wikifunctionswiki",
        ] {
            assert_eq!(site_group(s), Some("special"), "{s}");
            assert!(!is_wikipedia(s), "{s}");
        }
        assert_eq!(site_group("wiki"), None);
        assert_eq!(site_group("EnWiki"), None);
        for s in ["nlwiki", "zh_yuewiki", "simplewiki"] {
            assert!(is_wikipedia(s), "{s}");
        }
        for s in ["wiki", "enwikivoyage", "enwiktionary", "EnWiki"] {
            assert!(!is_wikipedia(s), "{s}");
        }
    }

    #[test]
    fn urls() {
        assert_eq!(
            sitelink_url("nlwiki", "België").unwrap(),
            "https://nl.wikipedia.org/wiki/Belgi%C3%AB"
        );
        assert_eq!(
            sitelink_url("enwiki", "Rock & roll (song)").unwrap(),
            "https://en.wikipedia.org/wiki/Rock_%26_roll_(song)"
        );
        assert_eq!(
            commons_file_url("Flag of Belgium.svg"),
            "https://commons.wikimedia.org/wiki/File:Flag_of_Belgium.svg"
        );
    }
}
