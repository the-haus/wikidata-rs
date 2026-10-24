//! Reading MediaWiki's SQL dumps (`wikidatawiki-<date>-page.sql.gz`,
//! `…-redirect.sql.gz`): `mysqldump` output, one `CREATE TABLE` and then
//! `INSERT INTO … VALUES (…),(…);` statements of about a megabyte each.
//!
//! [`SqlLines`] reads the statements of one table, learning its column
//! names from `CREATE TABLE`, so a schema change moves columns without
//! breaking the reader; [`parse_insert`] reads the rows of one statement
//! (independent of the others, so statements can be parsed on many
//! threads) and hands the wanted columns of each row to a function.

use std::io::{BufRead, BufReader, Read};

use crate::{Compression, DumpError, Result};

/// A field of a row.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Field<'a> {
    /// `NULL`.
    Null,
    /// A number, as written (`42`, `-1`, `0.5`).
    Number(&'a [u8]),
    /// A string, as written between the quotes: still escaped
    /// (see [`unescape`]).
    Str(&'a [u8]),
}

impl Field<'_> {
    /// The field as an integer.
    pub fn int(&self) -> Option<i64> {
        match self {
            Field::Number(n) => std::str::from_utf8(n).ok()?.parse().ok(),
            _ => None,
        }
    }

    /// The field as text (unescaped).
    pub fn text(&self) -> Option<String> {
        match self {
            Field::Str(s) => Some(String::from_utf8_lossy(&unescape(s)).into_owned()),
            Field::Number(n) => Some(String::from_utf8_lossy(n).into_owned()),
            Field::Null => None,
        }
    }
}

/// Undo `mysqldump`'s escapes (`\'`, `\\`, `\n`, …).
pub fn unescape(s: &[u8]) -> std::borrow::Cow<'_, [u8]> {
    if !s.contains(&b'\\') {
        return s.into();
    }
    let mut out = Vec::with_capacity(s.len());
    let mut it = s.iter();
    while let Some(&b) = it.next() {
        if b != b'\\' {
            out.push(b);
            continue;
        }
        match it.next() {
            Some(b'n') => out.push(b'\n'),
            Some(b'r') => out.push(b'\r'),
            Some(b't') => out.push(b'\t'),
            Some(b'0') => out.push(0),
            Some(b'Z') => out.push(0x1a),
            Some(&c) => out.push(c),
            None => {}
        }
    }
    out.into()
}

/// The `INSERT` statements of one table of a SQL dump, in order.
pub struct SqlLines {
    reader: Box<dyn BufRead + Send>,
    table: String,
    /// The table's columns, from its `CREATE TABLE`.
    pub columns: Vec<String>,
    prefix: Vec<u8>,
    done: bool,
}

impl SqlLines {
    /// Read `table` from a (possibly gzip-, bzip2- or zstd-compressed)
    /// SQL dump; its `CREATE TABLE` must come before its rows.
    pub fn new(source: impl Read + Send + 'static, table: &str) -> Result<SqlLines> {
        let mut r = BufReader::with_capacity(1 << 20, source);
        let head = r.fill_buf()?;
        let reader: Box<dyn BufRead + Send> = match Compression::sniff(head) {
            Compression::Gzip => Box::new(BufReader::with_capacity(1 << 20, flate2::read::MultiGzDecoder::new(r))),
            Compression::Bzip2 => Box::new(BufReader::with_capacity(1 << 20, bzip2::read::MultiBzDecoder::new(r))),
            Compression::Zstd => Box::new(BufReader::with_capacity(1 << 20, zstd::stream::read::Decoder::new(r)?)),
            Compression::None => Box::new(r),
        };
        let mut s = SqlLines {
            reader,
            table: table.to_string(),
            columns: Vec::new(),
            prefix: format!("INSERT INTO `{table}` VALUES ").into_bytes(),
            done: false,
        };
        s.read_schema()?;
        Ok(s)
    }

    fn read_schema(&mut self) -> Result<()> {
        let start = format!("CREATE TABLE `{}` (", self.table);
        let mut line = Vec::new();
        let mut inside = false;
        loop {
            line.clear();
            if self.reader.read_until(b'\n', &mut line)? == 0 {
                return Err(DumpError::Format(format!(
                    "not a SQL dump of the `{}` table (no CREATE TABLE)",
                    self.table
                )));
            }
            let l = String::from_utf8_lossy(&line);
            let t = l.trim();
            if !inside {
                inside = t.starts_with(&start);
                continue;
            }
            if let Some(rest) = t.strip_prefix('`') {
                if let Some(end) = rest.find('`') {
                    self.columns.push(rest[..end].to_string());
                }
                continue;
            }
            // `PRIMARY KEY …`, `KEY …`, then `) ENGINE=…`.
            if t.starts_with(')') || !t.starts_with('`') {
                if self.columns.is_empty() {
                    return Err(DumpError::Format(format!(
                        "`{}`: CREATE TABLE without columns",
                        self.table
                    )));
                }
                return Ok(());
            }
        }
    }

    /// The index of a column.
    pub fn column(&self, name: &str) -> Result<usize> {
        self.columns.iter().position(|c| c == name).ok_or_else(|| {
            DumpError::Format(format!(
                "`{}` has no column `{name}` (it has {})",
                self.table,
                self.columns.join(", ")
            ))
        })
    }
}

impl Iterator for SqlLines {
    type Item = Result<Vec<u8>>;

    /// The next `INSERT` statement of the table (other lines are skipped).
    fn next(&mut self) -> Option<Result<Vec<u8>>> {
        if self.done {
            return None;
        }
        loop {
            let mut line = Vec::new();
            match self.reader.read_until(b'\n', &mut line) {
                Ok(0) => {
                    self.done = true;
                    return None;
                }
                Ok(_) if line.starts_with(&self.prefix) => return Some(Ok(line)),
                Ok(_) => {}
                Err(e) => {
                    self.done = true;
                    return Some(Err(e.into()));
                }
            }
        }
    }
}

/// Read the rows of an `INSERT INTO … VALUES (…),(…);` statement, handing
/// `f` the fields of each row (`row[i]` for column `i`; `max_column` is
/// the highest column read, so the rest of a row is skipped unparsed).
pub fn parse_insert(line: &[u8], max_column: usize, f: &mut dyn FnMut(&[Field<'_>])) -> Result<()> {
    let bad = |at: usize| DumpError::Format(format!("SQL: unexpected input at byte {at} of an INSERT statement"));
    let Some(start) = memchr::memmem::find(line, b" VALUES ") else {
        return Err(bad(0));
    };
    let mut i = start + 8;
    let mut row: Vec<Field<'_>> = Vec::with_capacity(max_column + 1);
    loop {
        if line.get(i) != Some(&b'(') {
            return Err(bad(i));
        }
        i += 1;
        row.clear();
        // Fields until `)`.
        loop {
            let (field, next) = match line.get(i) {
                Some(b'\'') => {
                    // A string: up to the next unescaped quote.
                    let s = i + 1;
                    let mut j = s;
                    loop {
                        let k = memchr::memchr2(b'\'', b'\\', &line[j..]).ok_or_else(|| bad(j))?;
                        if line[j + k] == b'\\' {
                            j += k + 2;
                        } else {
                            j += k;
                            break;
                        }
                    }
                    (Field::Str(&line[s..j]), j + 1)
                }
                Some(_) => {
                    let len = line[i..]
                        .iter()
                        .position(|&b| b == b',' || b == b')')
                        .ok_or_else(|| bad(i))?;
                    let v = &line[i..i + len];
                    (if v == b"NULL" { Field::Null } else { Field::Number(v) }, i + len)
                }
                None => return Err(bad(i)),
            };
            if row.len() <= max_column {
                row.push(field);
            }
            i = next;
            match line.get(i) {
                Some(b',') => i += 1,
                Some(b')') => {
                    i += 1;
                    break;
                }
                _ => return Err(bad(i)),
            }
        }
        if row.len() > max_column {
            f(&row);
        }
        match line.get(i) {
            Some(b',') => i += 1,
            Some(b';') => return Ok(()),
            _ => return Err(bad(i)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DUMP: &str = "-- MySQL dump 10.19\n\
        DROP TABLE IF EXISTS `redirect`;\n\
        CREATE TABLE `redirect` (\n\
        \x20 `rd_from` int(8) unsigned NOT NULL DEFAULT 0,\n\
        \x20 `rd_namespace` int(11) NOT NULL DEFAULT 0,\n\
        \x20 `rd_title` varbinary(255) NOT NULL DEFAULT '',\n\
        \x20 `rd_interwiki` varbinary(32) DEFAULT NULL,\n\
        \x20 `rd_fragment` varbinary(255) DEFAULT NULL,\n\
        \x20 PRIMARY KEY (`rd_from`)\n\
        ) ENGINE=InnoDB DEFAULT CHARSET=binary;\n\
        INSERT INTO `redirect` VALUES (101,0,'Q2','',''),(102,0,'It\\'s, (odd)',NULL,'x'),(103,120,'P31','','');\n\
        INSERT INTO `redirect` VALUES (104,0,'Q5\\\\','','');\n";

    #[test]
    fn reads_rows() {
        let lines = SqlLines::new(DUMP.as_bytes(), "redirect").unwrap();
        assert_eq!(
            lines.columns,
            ["rd_from", "rd_namespace", "rd_title", "rd_interwiki", "rd_fragment"]
        );
        let title = lines.column("rd_title").unwrap();
        assert!(lines.column("nope").is_err());
        let mut rows = Vec::new();
        for l in lines {
            parse_insert(&l.unwrap(), title, &mut |r| {
                rows.push((r[0].int().unwrap(), r[1].int().unwrap(), r[2].text().unwrap()))
            })
            .unwrap();
        }
        assert_eq!(
            rows,
            [
                (101, 0, "Q2".to_string()),
                (102, 0, "It's, (odd)".into()),
                (103, 120, "P31".into()),
                (104, 0, "Q5\\".into())
            ]
        );
        let mut nulls = 0;
        let line = b"INSERT INTO `redirect` VALUES (102,0,'a',NULL,'x');";
        parse_insert(line, 4, &mut |r| nulls += usize::from(r[3] == Field::Null)).unwrap();
        assert_eq!(nulls, 1);
        assert!(parse_insert(b"INSERT INTO `redirect` VALUES (1,'open", 1, &mut |_| {}).is_err());
        assert!(SqlLines::new(&b"-- nothing\n"[..], "redirect").is_err());
    }
}
