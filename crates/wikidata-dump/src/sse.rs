//! Server-sent events (Wikimedia's EventStreams): a long-lived HTTP
//! response of `id:`, `event:` and `data:` lines, one blank line after
//! each event.
//!
//! [`SseReader`] reconnects when the connection drops, sending the last
//! event's id as `Last-Event-ID`, so the server resumes right after it:
//! no event is missed, and none is read twice. Reconnects back off and
//! give up after `retries` attempts in a row without an event.

use std::io::{self, BufRead, BufReader, Read};
use std::time::Duration;

use crate::{DumpError, HttpOptions, Result};

/// An event.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SseEvent {
    /// Its id (`id:`), to resume after it.
    pub id: Option<String>,
    /// Its type (`event:`; `message` when absent).
    pub event: Option<String>,
    /// Its data (`data:` lines, joined by newlines).
    pub data: String,
}

/// A stream of events (see the module docs).
pub struct SseReader {
    url: String,
    opts: HttpOptions,
    agent: ureq::Agent,
    body: Option<BufReader<Box<dyn Read + Send>>>,
    last_id: Option<String>,
    /// Connections opened.
    pub connections: u64,
}

impl SseReader {
    /// Connect to `url`, resuming after `last_event_id` when given (a
    /// failed connection is retried, as dropped ones are).
    pub fn open(url: &str, last_event_id: Option<String>, opts: HttpOptions) -> Result<SseReader> {
        let config = ureq::Agent::config_builder()
            .user_agent(opts.user_agent.as_str())
            .timeout_connect(Some(opts.timeout))
            .timeout_recv_response(Some(opts.timeout))
            .timeout_recv_body(None)
            .http_status_as_error(false)
            .build();
        let mut r = SseReader {
            url: url.to_string(),
            opts,
            agent: config.into(),
            body: None,
            last_id: last_event_id,
            connections: 0,
        };
        let mut attempts = 0u32;
        while let Err(e) = r.connect() {
            if attempts >= r.opts.retries {
                return Err(DumpError::Http(format!("{url}: {e}")));
            }
            attempts += 1;
            std::thread::sleep(Duration::from_millis(200 * u64::from(attempts.min(10))));
        }
        Ok(r)
    }

    /// The id of the last event read (or the one resumed after).
    pub fn last_event_id(&self) -> Option<&str> {
        self.last_id.as_deref()
    }

    fn connect(&mut self) -> io::Result<()> {
        let mut req = self.agent.get(&self.url).header("Accept", "text/event-stream");
        if let Some(id) = &self.last_id {
            req = req.header("Last-Event-ID", id.as_str());
        }
        let resp = req.call().map_err(io::Error::other)?;
        let status = resp.status().as_u16();
        if status != 200 {
            return Err(io::Error::other(format!("HTTP status {status}")));
        }
        self.connections += 1;
        let (_, body) = resp.into_parts();
        let reader: Box<dyn Read + Send> = Box::new(body.into_reader());
        self.body = Some(BufReader::with_capacity(64 << 10, reader));
        Ok(())
    }

    /// Read one event from the current connection; `Ok(None)` when it
    /// ends.
    fn read_event(&mut self) -> io::Result<Option<SseEvent>> {
        let body = self.body.as_mut().ok_or_else(|| io::Error::other("not connected"))?;
        let mut ev = SseEvent::default();
        let mut has_data = false;
        let mut line = String::new();
        loop {
            line.clear();
            if body.read_line(&mut line)? == 0 {
                return Ok(None);
            }
            let l = line.trim_end_matches(['\n', '\r']);
            if l.is_empty() {
                if has_data {
                    return Ok(Some(ev));
                }
                // A blank line without data (after a comment): no event.
                ev = SseEvent::default();
                continue;
            }
            if l.starts_with(':') {
                continue;
            }
            let (field, value) = match l.split_once(':') {
                Some((f, v)) => (f, v.strip_prefix(' ').unwrap_or(v)),
                None => (l, ""),
            };
            match field {
                "id" => ev.id = Some(value.to_string()),
                "event" => ev.event = Some(value.to_string()),
                "data" => {
                    if has_data {
                        ev.data.push('\n');
                    }
                    ev.data.push_str(value);
                    has_data = true;
                }
                _ => {}
            }
        }
    }

    /// The next event, reconnecting as needed (see the module docs).
    pub fn next_event(&mut self) -> Result<SseEvent> {
        let mut attempts = 0u32;
        loop {
            let err = match self.read_event() {
                Ok(Some(ev)) => {
                    if ev.id.is_some() {
                        self.last_id.clone_from(&ev.id);
                    }
                    return Ok(ev);
                }
                Ok(None) => io::Error::new(io::ErrorKind::UnexpectedEof, "the stream ended"),
                Err(e) => e,
            };
            loop {
                if attempts >= self.opts.retries {
                    return Err(DumpError::Http(format!(
                        "{}: gave up after {attempts} reconnects: {err}",
                        self.url
                    )));
                }
                attempts += 1;
                self.body = None;
                std::thread::sleep(Duration::from_millis(200 * u64::from(attempts.min(10))));
                if self.connect().is_ok() {
                    break;
                }
            }
        }
    }

    /// Whether an event may be waiting in the buffer (data was read but
    /// not handed out).
    pub fn has_buffered(&self) -> bool {
        self.body.as_ref().is_some_and(|b| !b.buffer().is_empty())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_events() {
        let text = ":ok\n\nevent: message\nid: [1]\ndata: {\"a\":1}\n\n: comment\ndata: x\ndata: y\n\n";
        let mut r = SseReader {
            url: "test".into(),
            opts: HttpOptions {
                retries: 0,
                ..Default::default()
            },
            agent: ureq::Agent::config_builder().build().into(),
            body: Some(BufReader::new(Box::new(io::Cursor::new(text.as_bytes().to_vec())))),
            last_id: None,
            connections: 1,
        };
        let a = r.next_event().unwrap();
        assert_eq!(
            a,
            SseEvent {
                id: Some("[1]".into()),
                event: Some("message".into()),
                data: "{\"a\":1}".into()
            }
        );
        let b = r.next_event().unwrap();
        assert_eq!((b.id, b.data.as_str()), (None, "x\ny"));
        assert_eq!(r.last_event_id(), Some("[1]"));
        assert!(r.next_event().is_err());
    }
}
