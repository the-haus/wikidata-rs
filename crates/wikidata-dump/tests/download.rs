//! Downloading a dump from two local "mirrors".

mod common;

use std::collections::HashMap;
use std::sync::Arc;

use common::*;
use sha1::{Digest, Sha1};
use wikidata_dump::download::{download, latest_dump, DownloadOptions};
use wikidata_dump::HttpOptions;

const MB: usize = 1 << 20;

fn hex(b: &[u8]) -> String {
    Sha1::digest(b).iter().map(|x| format!("{x:02x}")).collect()
}

/// A dumps directory: three dates, the newest still running (its checksums
/// do not list the file yet).
fn site(body: &Arc<Vec<u8>>) -> HashMap<String, Arc<Vec<u8>>> {
    let mut f = HashMap::new();
    let index = r#"<html><body><a href="../">../</a>
<a href="20261001/">20261001/</a>
<a href="20261012/">20261012/</a>
<a href="20261019/">20261019/</a>
<a href="latest-all.json.bz2">latest-all.json.bz2</a></body></html>"#;
    f.insert("/entities/".to_string(), Arc::new(index.as_bytes().to_vec()));
    let sums = format!(
        "{}  wikidata-20261012-all.json.gz\n{}  wikidata-20261012-all.json.bz2\n",
        "0".repeat(40),
        hex(body)
    );
    f.insert(
        "/entities/20261012/wikidata-20261012-sha1sums.txt".into(),
        Arc::new(sums.into_bytes()),
    );
    f.insert(
        "/entities/20261019/wikidata-20261019-sha1sums.txt".into(),
        Arc::new(b"0000  wikidata-20261019-all.json.gz\n".to_vec()),
    );
    f.insert("/entities/20261012/wikidata-20261012-all.json.bz2".into(), body.clone());
    f.insert(
        "/entities/20261001/wikidata-20261001-all.json.bz2".into(),
        Arc::new(vec![1, 2, 3]),
    );
    f
}

fn opts(a: &Running, b: &Running, dir: &std::path::Path) -> DownloadOptions {
    DownloadOptions {
        base: format!("{}/entities/", a.url),
        mirrors: vec![format!("{}/entities", b.url)],
        dir: dir.to_path_buf(),
        http: HttpOptions {
            connections: Some(2),
            range_size: 256 << 10,
            retries: 2,
            ..Default::default()
        },
        ..Default::default()
    }
}

fn tmp(name: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("wikidata-download-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

#[test]
fn the_newest_complete_dump_from_two_mirrors() {
    let body = Arc::new(noise(3 * MB + 777));
    let a = serve_files(site(&body), Server::default());
    let b = serve_files(site(&body), Server::default());
    let o = opts(&a, &b, &tmp("two"));
    let (date, sum) = latest_dump(&o.base, &o.file, &o.http).unwrap();
    assert_eq!((date.as_str(), sum.as_str()), ("20261012", hex(&body).as_str()));
    let path = o.dir.join("wikidata-20261012-all.json.bz2");
    let mut seen_early = false;
    let stats = download(&o, &mut |_| {
        seen_early |= path.exists();
        true
    })
    .unwrap();
    assert!(!seen_early, "the final name only after verification");
    assert_eq!(std::fs::read(&stats.path).unwrap(), *body);
    assert_eq!(stats.sha1.as_deref(), Some(hex(&body).as_str()));
    assert!(
        stats.per_server.iter().all(|&n| n > 0),
        "both mirrors: {:?}",
        stats.per_server
    );
    let leftovers: Vec<_> = std::fs::read_dir(&o.dir)
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert_eq!(leftovers.len(), 1, "{leftovers:?}");
    // Run again: already there.
    assert!(download(&o, &mut |_| true).unwrap().already_done);
}

#[test]
fn a_stopped_download_resumes() {
    let body = Arc::new(noise(4 * MB));
    let slow = Server {
        rate: 4 * MB as u64,
        ..Default::default()
    };
    let a = serve_files(site(&body), slow.clone());
    let b = serve_files(site(&body), slow);
    let o = opts(&a, &b, &tmp("resume"));
    let err = download(&o, &mut |p| p.bytes_done < MB as u64).unwrap_err();
    assert!(err.to_string().contains("stopped"), "{err}");
    let part = o.dir.join("wikidata-20261012-all.json.bz2.part");
    assert!(part.exists() && o.dir.join("wikidata-20261012-all.json.bz2.part.state").exists());
    assert!(!o.dir.join("wikidata-20261012-all.json.bz2").exists());
    let stats = download(&o, &mut |_| true).unwrap();
    assert!(stats.bytes_resumed >= MB as u64 / 2, "{stats:?}");
    assert!(stats.bytes_fetched < body.len() as u64);
    assert_eq!(std::fs::read(&stats.path).unwrap(), *body);
    assert!(!part.exists());
}

#[test]
fn a_corrupted_range_fails_verification() {
    let body = Arc::new(noise(2 * MB));
    let a = serve_files(site(&body), Server::default());
    let b = serve_files(
        site(&body),
        Server {
            corrupt_at: Some(MB as u64 + 5),
            ..Default::default()
        },
    );
    let mut o = opts(&a, &b, &tmp("corrupt"));
    // Only the corrupting mirror, so the bad range surely comes from it.
    o.base.clone_from(&o.mirrors[0]);
    o.mirrors.clear();
    let err = download(&o, &mut |_| true).unwrap_err();
    assert!(err.to_string().contains("SHA-1 mismatch"), "{err}");
    assert!(!o.dir.join("wikidata-20261012-all.json.bz2").exists());
    // An explicit date that is not complete.
    let mut o = opts(&a, &b, &tmp("incomplete"));
    o.date = Some("20261019".into());
    assert!(download(&o, &mut |_| true)
        .unwrap_err()
        .to_string()
        .contains("not complete"));
}

/// The database dumps' directory: the SQL tables of the newest date are
/// not all done yet.
fn wiki_site(redirect: &Arc<Vec<u8>>, page: &Arc<Vec<u8>>) -> HashMap<String, Arc<Vec<u8>>> {
    let mut f = HashMap::new();
    f.insert(
        "/wikidatawiki/".to_string(),
        Arc::new(br#"<a href="20261001/">20261001/</a> <a href="20261101/">20261101/</a>"#.to_vec()),
    );
    let job = |table: &str, date: &str, body: &[u8], status: &str| {
        let name = format!("wikidatawiki-{date}-{table}.sql.gz");
        format!(
            r#""{table}table":{{"status":"{status}","files":{{"{name}":{{"size":{},"url":"/wikidatawiki/{date}/{name}","sha1":"{}"}}}}}}"#,
            body.len(),
            hex(body)
        )
    };
    let status = |date: &str, page_status: &str| {
        format!(
            r#"{{"jobs":{{{},{},"xmlstubsdump":{{"status":"done"}}}},"version":"0.8"}}"#,
            job("redirect", date, redirect, "done"),
            job("page", date, page, page_status)
        )
    };
    f.insert(
        "/wikidatawiki/20261001/dumpstatus.json".into(),
        Arc::new(status("20261001", "done").into_bytes()),
    );
    f.insert(
        "/wikidatawiki/20261101/dumpstatus.json".into(),
        Arc::new(status("20261101", "in-progress").into_bytes()),
    );
    f.insert(
        "/wikidatawiki/20261001/wikidatawiki-20261001-redirect.sql.gz".into(),
        redirect.clone(),
    );
    f.insert(
        "/wikidatawiki/20261001/wikidatawiki-20261001-page.sql.gz".into(),
        page.clone(),
    );
    f
}

#[test]
fn the_sql_tables_of_the_newest_complete_database_dump() {
    use wikidata_dump::download::{download_sql, latest_sql_dump, SqlDownloadOptions};
    let redirect = Arc::new(noise(300_000));
    let page = Arc::new(noise(MB * 2 + 3));
    let a = serve_files(wiki_site(&redirect, &page), Server::default());
    let b = serve_files(wiki_site(&redirect, &page), Server::default());
    let o = SqlDownloadOptions {
        base: format!("{}/wikidatawiki/", a.url),
        mirrors: vec![format!("{}/wikidatawiki/", b.url)],
        dir: tmp("sql"),
        http: HttpOptions {
            range_size: 256 << 10,
            ..Default::default()
        },
        ..Default::default()
    };
    let status = latest_sql_dump(&o.base, &o.tables, &o.http).unwrap();
    assert_eq!(status.date, "20261001", "20261101 has not done the page table");
    assert_eq!(
        status.files.iter().map(|f| f.name.as_str()).collect::<Vec<_>>(),
        [
            "wikidatawiki-20261001-redirect.sql.gz",
            "wikidatawiki-20261001-page.sql.gz"
        ]
    );
    assert_eq!(status.files[1].sha1, hex(&page));
    assert_eq!(
        status.files[1].url,
        format!("{}/wikidatawiki/20261001/wikidatawiki-20261001-page.sql.gz", a.url)
    );
    let got = download_sql(&o, &mut |_| true).unwrap();
    assert_eq!(std::fs::read(&got[0].path).unwrap(), *redirect);
    assert_eq!(std::fs::read(&got[1].path).unwrap(), *page);
    assert!(got.iter().all(|s| s.sha1.is_some() && s.date == "20261001"));
    // A date whose tables are not done.
    let mut o = o;
    o.date = Some("20261101".into());
    assert!(download_sql(&o, &mut |_| true)
        .unwrap_err()
        .to_string()
        .contains("has not done"));
}
