//! The external identifiers collected into `externalIds`.
//!
//! One table, so the list is easy to read and to extend: each row is the
//! key in the output, the Wikidata property and the URL of the identifier
//! (`$1` is replaced by the value). Values come from the best-ranked
//! statements; the first one wins (all of them stay in `claims` when the
//! property is selected).

use crate::entity::Pid;

/// An identifier in another database.
#[derive(Clone, Copy, Debug)]
pub struct ExternalId {
    /// The key in `externalIds`.
    pub key: &'static str,
    /// The Wikidata property.
    pub property: Pid,
    /// The identifier's URL, `$1` for the value.
    pub formatter: &'static str,
}

const fn id(key: &'static str, property: u32, formatter: &'static str) -> ExternalId {
    ExternalId {
        key,
        property: Pid(property),
        formatter,
    }
}

/// The identifiers collected into `externalIds`, in output order.
pub const EXTERNAL_IDS: &[ExternalId] = &[
    // Music
    id("musicbrainzArtist", 434, "https://musicbrainz.org/artist/$1"),
    id("musicbrainzWork", 435, "https://musicbrainz.org/work/$1"),
    id(
        "musicbrainzReleaseGroup",
        436,
        "https://musicbrainz.org/release-group/$1",
    ),
    id("musicbrainzRelease", 5813, "https://musicbrainz.org/release/$1"),
    id("musicbrainzRecording", 4404, "https://musicbrainz.org/recording/$1"),
    id("musicbrainzLabel", 966, "https://musicbrainz.org/label/$1"),
    id("musicbrainzArea", 982, "https://musicbrainz.org/area/$1"),
    id("musicbrainzPlace", 1004, "https://musicbrainz.org/place/$1"),
    id("musicbrainzEvent", 6423, "https://musicbrainz.org/event/$1"),
    id("musicbrainzSeries", 1407, "https://musicbrainz.org/series/$1"),
    id("discogsArtist", 1953, "https://www.discogs.com/artist/$1"),
    id("discogsMaster", 1954, "https://www.discogs.com/master/$1"),
    id("discogsLabel", 1955, "https://www.discogs.com/label/$1"),
    id("spotifyArtist", 1902, "https://open.spotify.com/artist/$1"),
    id("spotifyAlbum", 2205, "https://open.spotify.com/album/$1"),
    id("spotifyTrack", 2207, "https://open.spotify.com/track/$1"),
    id("appleMusicArtist", 2850, "https://music.apple.com/artist/$1"),
    id("deezerArtist", 2722, "https://www.deezer.com/artist/$1"),
    id("lastfm", 3192, "https://www.last.fm/music/$1"),
    id("soundcloud", 3040, "https://soundcloud.com/$1"),
    id("bandcamp", 3283, "https://$1.bandcamp.com/"),
    id("songkickArtist", 3478, "https://www.songkick.com/artists/$1"),
    id("geniusArtist", 2373, "https://genius.com/artists/$1"),
    id("allmusicArtist", 1728, "https://www.allmusic.com/artist/$1"),
    // Places
    id("osmRelation", 402, "https://www.openstreetmap.org/relation/$1"),
    id("osmWay", 10689, "https://www.openstreetmap.org/way/$1"),
    id("osmNode", 11693, "https://www.openstreetmap.org/node/$1"),
    id("geonames", 1566, "https://www.geonames.org/$1"),
    // Authority files
    id("isni", 213, "https://isni.org/isni/$1"),
    id("viaf", 214, "https://viaf.org/viaf/$1"),
    id("gnd", 227, "https://d-nb.info/gnd/$1"),
    id("loc", 244, "https://id.loc.gov/authorities/$1"),
    id("freebase", 646, "https://www.google.com/search?kgmid=$1"),
    id("imdb", 345, "https://www.imdb.com/Name?$1"),
    // Social media
    id("youtubeChannel", 2397, "https://www.youtube.com/channel/$1"),
    id("instagram", 2003, "https://www.instagram.com/$1/"),
    id("x", 2002, "https://x.com/$1"),
    id("facebook", 2013, "https://www.facebook.com/$1"),
];

/// The row for a property.
pub fn external_id(p: Pid) -> Option<(usize, &'static ExternalId)> {
    EXTERNAL_IDS.iter().enumerate().find(|(_, e)| e.property == p)
}

/// The URL of an identifier (spaces are dropped, as ISNI formatters do).
pub fn external_id_url(e: &ExternalId, value: &str) -> String {
    let v: String = value.chars().filter(|c| !c.is_whitespace()).collect();
    e.formatter.replace("$1", &v)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unique_keys_and_properties() {
        for (i, a) in EXTERNAL_IDS.iter().enumerate() {
            for b in &EXTERNAL_IDS[i + 1..] {
                assert_ne!(a.key, b.key);
                assert_ne!(a.property, b.property);
            }
            assert!(a.formatter.contains("$1"));
        }
        let (_, mb) = external_id(Pid(434)).unwrap();
        assert_eq!(
            external_id_url(mb, "a74b1b7f-71a5-4011-9441-d0b5e4122711"),
            "https://musicbrainz.org/artist/a74b1b7f-71a5-4011-9441-d0b5e4122711"
        );
        let (_, isni) = external_id(Pid(213)).unwrap();
        assert_eq!(
            external_id_url(isni, "0000 0001 2103 2683"),
            "https://isni.org/isni/0000000121032683"
        );
    }
}
