//! Checkpoints: where a scan can resume after a stop or a crash.
//!
//! A checkpoint is taken after a chunk whose records the sink has taken
//! (so resuming from it yields exactly the records after them). It holds
//! the position in the input (a bzip2 block, or a byte of plain JSON, and
//! how much of it was read), the counters so far, the ids of the filter's
//! `ids` already seen (for `missing_ids`), and the filter's fingerprint, so
//! a pass resumes only on the same input with the same filter.

use wikidata_dump::ResumePoint;

use crate::scan::TypeCounts;
use crate::ExportError;

const MAGIC: &[u8; 8] = b"WDCKPT01";

/// Where a scan can resume (see the module docs and
/// [`crate::ScanOptions::resume`]).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Checkpoint {
    /// The position in the input.
    pub position: ResumePoint,
    /// The input's file name.
    pub input: String,
    /// The input's size, when known.
    pub size: Option<u64>,
    /// The filter's fingerprint (0: no filter).
    pub filter: u64,
    /// Entities seen.
    pub entities: u64,
    /// Entities that matched.
    pub matched: u64,
    /// Entities parsed.
    pub parsed: u64,
    /// Unchanged entities skipped.
    pub unchanged: u64,
    /// Entities by type.
    pub types: TypeCounts,
    /// Highest id numbers by type.
    pub max_ids: [u64; 5],
    /// Keys ([`wikidata_filter::id_key`]) of the filter's `ids` seen so
    /// far, sorted.
    pub seen: Vec<i64>,
}

fn put_varint(out: &mut Vec<u8>, mut v: u64) {
    while v >= 0x80 {
        out.push(v as u8 | 0x80);
        v >>= 7;
    }
    out.push(v as u8);
}

struct Reader<'a>(&'a [u8]);

impl Reader<'_> {
    fn varint(&mut self) -> Option<u64> {
        let mut v = 0u64;
        for shift in (0..64).step_by(7) {
            let (&b, rest) = self.0.split_first()?;
            self.0 = rest;
            v |= u64::from(b & 0x7f) << shift;
            if b < 0x80 {
                return Some(v);
            }
        }
        None
    }

    fn bytes(&mut self, n: usize) -> Option<&[u8]> {
        if self.0.len() < n {
            return None;
        }
        let (b, rest) = self.0.split_at(n);
        self.0 = rest;
        Some(b)
    }
}

impl Checkpoint {
    /// Serialise: `WDCKPT01`, then varints (the seen keys as deltas).
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = MAGIC.to_vec();
        let p = &self.position;
        let t = &self.types;
        put_varint(&mut out, self.input.len() as u64);
        out.extend_from_slice(self.input.as_bytes());
        for v in [
            self.size.map_or(0, |s| s + 1),
            self.filter,
            p.bit,
            u64::from(p.crc),
            p.blocks,
            p.streams,
            p.skip,
            p.offset,
            p.chunks,
            u64::from(p.array) | u64::from(p.closed) << 1,
            self.entities,
            self.matched,
            self.parsed,
            self.unchanged,
            t.items,
            t.properties,
            t.lexemes,
            t.mediainfo,
            t.entity_schemas,
        ]
        .into_iter()
        .chain(self.max_ids)
        {
            put_varint(&mut out, v);
        }
        put_varint(&mut out, self.seen.len() as u64);
        let mut prev = 0i64;
        for &k in &self.seen {
            put_varint(&mut out, (k - prev) as u64);
            prev = k;
        }
        out
    }

    /// Deserialise [`Checkpoint::to_bytes`].
    pub fn from_bytes(bytes: &[u8]) -> Result<Checkpoint, ExportError> {
        let bad = || ExportError::Invalid("not a checkpoint (or one of another version)".into());
        let body = bytes.strip_prefix(MAGIC).ok_or_else(bad)?;
        let mut r = Reader(body);
        let n = r.varint().ok_or_else(bad)? as usize;
        let input = std::str::from_utf8(r.bytes(n).ok_or_else(bad)?)
            .map_err(|_| bad())?
            .to_string();
        let mut v = [0u64; 24];
        for x in &mut v {
            *x = r.varint().ok_or_else(bad)?;
        }
        let count = r.varint().ok_or_else(bad)? as usize;
        let mut seen = Vec::with_capacity(count.min(body.len()));
        let mut prev = 0i64;
        for _ in 0..count {
            prev += r.varint().ok_or_else(bad)? as i64;
            seen.push(prev);
        }
        if !r.0.is_empty() {
            return Err(bad());
        }
        Ok(Checkpoint {
            input,
            size: v[0].checked_sub(1),
            filter: v[1],
            position: ResumePoint {
                bit: v[2],
                crc: u32::try_from(v[3]).map_err(|_| bad())?,
                blocks: v[4],
                streams: v[5],
                skip: v[6],
                offset: v[7],
                chunks: v[8],
                array: v[9] & 1 != 0,
                closed: v[9] & 2 != 0,
            },
            entities: v[10],
            matched: v[11],
            parsed: v[12],
            unchanged: v[13],
            types: TypeCounts {
                items: v[14],
                properties: v[15],
                lexemes: v[16],
                mediainfo: v[17],
                entity_schemas: v[18],
            },
            max_ids: [v[19], v[20], v[21], v[22], v[23]],
            seen,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let c = Checkpoint {
            position: ResumePoint {
                bit: 1 << 40,
                crc: u32::MAX,
                blocks: 7,
                streams: 1,
                skip: 900_000,
                offset: 123,
                chunks: 8,
                array: true,
                closed: false,
            },
            input: "wikidata-20261012-all.json.bz2".into(),
            size: Some(103_409_622_204),
            filter: u64::MAX,
            entities: 5,
            matched: 2,
            parsed: 3,
            unchanged: 1,
            types: TypeCounts {
                items: 4,
                lexemes: 1,
                ..Default::default()
            },
            max_ids: [31, 0, 7, 0, 0],
            seen: vec![1, 31, 1 << 34, 3 << 34],
        };
        let b = c.to_bytes();
        assert_eq!(Checkpoint::from_bytes(&b).unwrap(), c);
        assert!(Checkpoint::from_bytes(&b[..b.len() - 1]).is_err());
        assert!(Checkpoint::from_bytes(b"nope").is_err());
        let none = Checkpoint::default();
        assert_eq!(Checkpoint::from_bytes(&none.to_bytes()).unwrap(), none);
    }
}
