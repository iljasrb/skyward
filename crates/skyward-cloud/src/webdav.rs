//! A WebDAV client: the handful of methods the recovery needs to use a
//! server as a folder, and the PROPFIND answer read into the same entries a
//! filesystem shows.

use crate::config::Config;
use crate::http;
use base64::Engine;
use percent_encoding::percent_decode_str;
use skyward_net::http::Servers;
use std::fmt;
use std::fs::File;
use std::io::Write;
use std::time::{SystemTime, UNIX_EPOCH};
use url::Url;

/// What a WebDAV answer means to the filesystem.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    File,
    Folder,
}

/// An entry on the server, as the filesystem shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stat {
    pub kind: Kind,
    pub size: u64,
    /// Unix seconds; zero when the server gave no time.
    pub modified: i64,
    pub etag: Option<String>,
}

/// A folder's entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub name: String,
    pub stat: Stat,
}

/// What the server says about itself when it is tested.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Info {
    pub server: Option<String>,
    pub free: Option<u64>,
}

/// Why the server could not be used.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorKind {
    NotFound,
    Forbidden,
    Unauthorized,
    Conflict,
    NoSpace,
    Network,
    Invalid,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Error {
    pub kind: ErrorKind,
    pub message: String,
}

impl Error {
    pub fn new(kind: ErrorKind, message: impl Into<String>) -> Error {
        Error { kind, message: message.into() }
    }

    fn invalid(message: impl Into<String>) -> Error {
        Error::new(ErrorKind::Invalid, message)
    }

    fn network(error: impl fmt::Display) -> Error {
        Error::new(ErrorKind::Network, error.to_string())
    }

    fn status(code: u16, body: &str) -> Error {
        let kind = match code {
            401 => ErrorKind::Unauthorized,
            403 => ErrorKind::Forbidden,
            404 => ErrorKind::NotFound,
            405 | 409 | 412 => ErrorKind::Conflict,
            507 => ErrorKind::NoSpace,
            _ => ErrorKind::Other,
        };
        let body = body.trim();
        let message = match body.is_empty() {
            true => format!("the server answered {code}"),
            false => format!("the server answered {code}: {}", body.lines().next().unwrap_or("")),
        };
        Error::new(kind, message)
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for Error {}

pub type Result<T> = std::result::Result<T, Error>;

/// What the FUSE filesystem asks of a server.
pub trait Backend: Send + Sync {
    fn stat(&self, path: &str) -> Result<Stat>;
    fn list(&self, path: &str) -> Result<Vec<Entry>>;
    fn get(&self, path: &str, out: &mut dyn Write) -> Result<u64>;
    fn put(&self, path: &str, file: &File) -> Result<()>;
    fn mkdir(&self, path: &str) -> Result<()>;
    fn remove(&self, path: &str) -> Result<()>;
    fn rename(&self, from: &str, to: &str) -> Result<()>;
    fn quota(&self) -> Result<Option<u64>>;
}

/// A WebDAV server reached over HTTP or HTTPS.
pub struct WebDav {
    client: http::Client,
    base: Url,
    root: String,
    auth: Option<String>,
}

impl Drop for WebDav {
    fn drop(&mut self) {
        use zeroize::Zeroize;
        if let Some(auth) = &mut self.auth {
            auth.zeroize();
        }
    }
}

impl WebDav {
    pub fn new(config: &Config, servers: Servers) -> Result<WebDav> {
        config.validate().map_err(|e| Error::invalid(format!("{e:#}")))?;
        let base = config.base_url().map_err(|e| Error::invalid(format!("{e:#}")))?;
        let auth = match config.username.is_empty() && config.password.is_empty() {
            true => None,
            false => {
                let joined = format!("{}:{}", config.username, config.password);
                Some(format!("Basic {}", base64::engine::general_purpose::STANDARD.encode(joined)))
            }
        };
        let client = http::Client::new(servers).map_err(Error::network)?;
        Ok(WebDav { client, base, root: config.folder().to_string(), auth })
    }

    /// The server's answer to a request for the folder itself.
    pub fn test(&self) -> Result<Info> {
        let (responses, free) = self.propfind("", "0")?;
        let first = responses.first().ok_or_else(|| Error::new(ErrorKind::Other, "the server returned no folder"))?;
        if first.stat.kind != Kind::Folder {
            return Err(Error::new(ErrorKind::Other, "the address is not a folder"));
        }
        let server = self.base.host_str().map(str::to_string);
        Ok(Info { server, free })
    }

    fn url(&self, path: &str) -> Result<Url> {
        let mut relative = String::new();
        for part in self.root.split('/').filter(|p| !p.is_empty()) {
            push_segment(&mut relative, part);
        }
        let empty = path.trim_matches('/').is_empty();
        for part in path.trim_matches('/').split('/').filter(|p| !p.is_empty()) {
            if part == "." || part == ".." {
                return Err(Error::invalid("a path may not climb out of the address"));
            }
            push_segment(&mut relative, part);
        }
        if empty && !relative.is_empty() {
            relative.push('/');
        }
        self.base.join(&relative).map_err(|e| Error::invalid(format!("{e}")))
    }

    fn propfind(&self, path: &str, depth: &str) -> Result<(Vec<Response>, Option<u64>)> {
        let url = self.url(path)?;
        let body = r#"<?xml version="1.0" encoding="utf-8"?>
<d:propfind xmlns:d="DAV:">
  <d:prop>
    <d:displayname/>
    <d:resourcetype/>
    <d:getcontentlength/>
    <d:getlastmodified/>
    <d:getetag/>
    <d:quota-available-bytes/>
    <d:quota-used-bytes/>
  </d:prop>
</d:propfind>"#;
        let headers = [("Depth", depth.to_string()), ("Content-Type", "application/xml; charset=utf-8".into())];
        let mut answer = Vec::new();
        let response = self
            .client
            .request("PROPFIND", &url, self.auth.as_deref(), &headers, None, Some(body.as_bytes()), &mut answer, 8 * 1024 * 1024)
            .map_err(Error::network)?;
        if !(response.status == 200 || response.status == 207) {
            return Err(Error::status(response.status, &String::from_utf8_lossy(&answer)));
        }
        let text = String::from_utf8_lossy(&answer);
        parse_propfind(&text, &self.base, &self.root)
    }

    fn simple(&self, method: &str, path: &str, allowed: &[u16]) -> Result<()> {
        let url = self.url(path)?;
        let mut answer = Vec::new();
        let response = self
            .client
            .request(method, &url, self.auth.as_deref(), &[], None, Some(&[]), &mut answer, 64 * 1024)
            .map_err(Error::network)?;
        if allowed.contains(&response.status) {
            return Ok(());
        }
        Err(Error::status(response.status, &String::from_utf8_lossy(&answer)))
    }
}

impl Backend for WebDav {
    fn stat(&self, path: &str) -> Result<Stat> {
        let (responses, _) = self.propfind(path, "0")?;
        responses.first().map(|r| r.stat.clone()).ok_or_else(|| Error::new(ErrorKind::NotFound, "the server returned no entry"))
    }

    fn list(&self, path: &str) -> Result<Vec<Entry>> {
        let (responses, _) = self.propfind(path, "1")?;
        let root = self.url(path)?.path().trim_end_matches('/').to_string();
        let mut entries = Vec::new();
        for response in responses {
            let Some(name) = response.name(&root) else { continue };
            entries.push(Entry { name, stat: response.stat });
        }
        entries.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(entries)
    }

    fn get(&self, path: &str, out: &mut dyn Write) -> Result<u64> {
        let url = self.url(path)?;
        let response = self
            .client
            .request("GET", &url, self.auth.as_deref(), &[], None, None, out, u64::MAX)
            .map_err(Error::network)?;
        if response.status != 200 {
            return Err(Error::status(response.status, ""));
        }
        Ok(response.body_len)
    }

    fn put(&self, path: &str, file: &File) -> Result<()> {
        let url = self.url(path)?;
        let mut answer = Vec::new();
        let response = self
            .client
            .request("PUT", &url, self.auth.as_deref(), &[], Some(file), None, &mut answer, 64 * 1024)
            .map_err(Error::network)?;
        if matches!(response.status, 200 | 201 | 204) {
            return Ok(());
        }
        Err(Error::status(response.status, &String::from_utf8_lossy(&answer)))
    }

    fn mkdir(&self, path: &str) -> Result<()> {
        self.simple("MKCOL", path, &[200, 201, 204])
    }

    fn remove(&self, path: &str) -> Result<()> {
        self.simple("DELETE", path, &[200, 202, 204, 404])
    }

    fn rename(&self, from: &str, to: &str) -> Result<()> {
        let url = self.url(from)?;
        let destination = self.url(to)?;
        let headers = [("Destination", destination.to_string()), ("Overwrite", "T".into())];
        let mut answer = Vec::new();
        let response = self
            .client
            .request("MOVE", &url, self.auth.as_deref(), &headers, None, Some(&[]), &mut answer, 64 * 1024)
            .map_err(Error::network)?;
        if matches!(response.status, 200 | 201 | 204) {
            return Ok(());
        }
        Err(Error::status(response.status, &String::from_utf8_lossy(&answer)))
    }

    fn quota(&self) -> Result<Option<u64>> {
        let (_, free) = self.propfind("", "0")?;
        Ok(free)
    }
}

fn push_segment(into: &mut String, segment: &str) {
    if !into.is_empty() {
        into.push('/');
    }
    for byte in segment.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => into.push(byte as char),
            _ => into.push_str(&format!("%{byte:02X}")),
        }
    }
}

#[derive(Debug, Clone)]
struct Response {
    href: String,
    stat: Stat,
}

impl Response {
    fn name(&self, root: &str) -> Option<String> {
        let path = percent_decode_str(&self.href).decode_utf8_lossy();
        let root = percent_decode_str(root).decode_utf8_lossy();
        let path = path.trim_end_matches('/');
        let root = root.trim_end_matches('/');
        let rest = path.strip_prefix(root)?.trim_start_matches('/');
        if rest.is_empty() || rest.contains('/') {
            return None;
        }
        let name = rest.to_string();
        if name.is_empty() || name == "." || name == ".." || name.contains('/') || name.chars().any(char::is_control) {
            return None;
        }
        Some(name)
    }
}

/// Read a PROPFIND answer. The first entry is the folder asked about.
fn parse_propfind(xml: &str, base: &Url, root: &str) -> Result<(Vec<Response>, Option<u64>)> {
    let mut parser = Xml::new(xml);
    let mut responses = Vec::new();
    let mut free = None;
    let mut response: Option<Response> = None;
    let mut propstat_ok = false;
    let mut in_propstat = false;
    let mut href = String::new();
    let mut kind = Kind::File;
    let mut size = 0u64;
    let mut modified = 0i64;
    let mut etag = None;
    let mut tag = String::new();
    let mut text = String::new();
    while let Some(event) = parser.next() {
        match event {
            Event::Start(name) => {
                tag = name.clone();
                text.clear();
                match name.as_str() {
                    "response" => {
                        response = Some(Response {
                            href: String::new(),
                            stat: Stat { kind: Kind::File, size: 0, modified: 0, etag: None },
                        });
                        propstat_ok = false;
                    }
                    "propstat" => {
                        in_propstat = true;
                        propstat_ok = false;
                    }
                    "resourcetype" => kind = Kind::File,
                    "collection" => kind = Kind::Folder,
                    _ => {}
                }
            }
            Event::End(name) => {
                match name.as_str() {
                    "href" => href = unescape(text.trim()),
                    "status" if in_propstat => propstat_ok = text.contains(" 200 "),
                    "getcontentlength" => size = text.trim().parse().unwrap_or(0),
                    "getlastmodified" => modified = parse_http_date(text.trim()).unwrap_or(0),
                    "getetag" => {
                        let value = unescape(text.trim());
                        if !value.is_empty() {
                            etag = Some(value.trim_matches('"').to_string());
                        }
                    }
                    "quota-available-bytes" => free = text.trim().parse().ok().or(free),
                    "propstat" => {
                        if propstat_ok {
                            if let Some(r) = response.as_mut() {
                                r.stat.kind = kind;
                                r.stat.size = size;
                                r.stat.modified = modified;
                                r.stat.etag = etag.clone();
                            }
                        }
                        in_propstat = false;
                    }
                    "response" => {
                        if let Some(mut r) = response.take() {
                            r.href = match href.is_empty() {
                                true => String::new(),
                                false => match Url::parse(&href) {
                                    Ok(url) => url.path().to_string(),
                                    Err(_) => base.join(&href).map(|u| u.path().to_string()).unwrap_or(href.clone()),
                                },
                            };
                            if !r.href.is_empty() {
                                responses.push(r);
                            }
                        }
                        href.clear();
                        kind = Kind::File;
                        size = 0;
                        modified = 0;
                        etag = None;
                    }
                    _ => {}
                }
                text.clear();
                tag.clear();
            }
            Event::Text(value) => {
                if tag == "status" || tag == "href" || tag == "getcontentlength" || tag == "getlastmodified" || tag == "getetag" || tag == "quota-available-bytes" {
                    text.push_str(&value);
                }
            }
        }
    }
    // The root's own path is what the caller asked for; the entries under it
    // are filtered by `Response::name`.
    let _ = root;
    Ok((responses, free))
}

enum Event {
    Start(String),
    End(String),
    Text(String),
}

/// A small XML reader: enough for the elements a PROPFIND answer holds,
/// with namespaces ignored and comments and declarations skipped.
struct Xml<'a> {
    text: &'a str,
    at: usize,
    /// A self-closing tag's end, which follows its start.
    pending_end: Option<String>,
}

impl<'a> Xml<'a> {
    fn new(text: &'a str) -> Xml<'a> {
        Xml { text, at: 0, pending_end: None }
    }

    fn next(&mut self) -> Option<Event> {
        if let Some(name) = self.pending_end.take() {
            return Some(Event::End(name));
        }
        loop {
            if self.at >= self.text.len() {
                return None;
            }
            let rest = &self.text[self.at..];
            if let Some(start) = rest.find('<') {
                if start > 0 {
                    let text = &rest[..start];
                    self.at += start;
                    return Some(Event::Text(unescape(text)));
                }
            } else {
                let text = rest;
                self.at = self.text.len();
                return Some(Event::Text(unescape(text)));
            }
            let rest = &self.text[self.at..];
            if rest.starts_with("<!--") {
                self.at += rest.find("-->").map_or(rest.len(), |n| n + 3);
                continue;
            }
            if rest.starts_with("<?") {
                self.at += rest.find("?>").map_or(rest.len(), |n| n + 2);
                continue;
            }
            if let Some(body) = rest.strip_prefix("<![CDATA[") {
                let end = body.find("]]>").unwrap_or(body.len());
                self.at += "<![CDATA[".len() + end + 3;
                return Some(Event::Text(body[..end].to_string()));
            }
            if rest.starts_with("<!") {
                self.at += rest.find('>').map_or(rest.len(), |n| n + 1);
                continue;
            }
            let end = rest.find('>')?;
            let inside = &rest[1..end];
            self.at += end + 1;
            let self_closing = inside.ends_with('/');
            let inside = inside.trim_end_matches('/').trim();
            if inside.is_empty() {
                continue;
            }
            let closing = inside.starts_with('/');
            let inside = inside.trim_start_matches('/').trim();
            let name = inside.split_whitespace().next().unwrap_or("");
            let name = name.split(':').next_back().unwrap_or(name).to_ascii_lowercase();
            if closing {
                return Some(Event::End(name));
            }
            if self_closing {
                // The caller sees a start and an end, as a pair of tags.
                self.pending_end = Some(name.clone());
            }
            return Some(Event::Start(name));
        }
    }
}

fn unescape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut at = 0;
    while let Some(amp) = text[at..].find('&') {
        out.push_str(&text[at..at + amp]);
        let rest = &text[at + amp..];
        let Some(end) = rest.find(';') else {
            out.push_str(rest);
            return out;
        };
        let entity = &rest[1..end];
        let replacement = match entity {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            _ if entity.starts_with("#x") || entity.starts_with("#X") => {
                u32::from_str_radix(&entity[2..], 16).ok().and_then(char::from_u32)
            }
            _ if entity.starts_with('#') => entity[1..].parse::<u32>().ok().and_then(char::from_u32),
            _ => None,
        };
        match replacement {
            Some(ch) => out.push(ch),
            None => out.push_str(&rest[..=end]),
        }
        at += amp + end + 1;
    }
    out.push_str(&text[at..]);
    out
}

/// A date as HTTP writes it, in Unix seconds; zero when it is not one.
fn parse_http_date(text: &str) -> Option<i64> {
    let text = text.trim();
    let rest = text.split_once(", ").map_or(text, |(_, rest)| rest);
    let mut parts = rest.split_whitespace();
    let day: i64 = parts.next()?.parse().ok()?;
    let month = match parts.next()? {
        "Jan" => 1,
        "Feb" => 2,
        "Mar" => 3,
        "Apr" => 4,
        "May" => 5,
        "Jun" => 6,
        "Jul" => 7,
        "Aug" => 8,
        "Sep" => 9,
        "Oct" => 10,
        "Nov" => 11,
        "Dec" => 12,
        _ => return None,
    };
    let year: i64 = parts.next()?.parse().ok()?;
    let clock = parts.next()?;
    let mut clock = clock.split(':');
    let hour: i64 = clock.next()?.parse().ok()?;
    let minute: i64 = clock.next()?.parse().ok()?;
    let second: i64 = clock.next()?.parse().ok()?;
    let days = days_from_civil(year, month, day);
    Some(days * 86_400 + hour * 3_600 + minute * 60 + second)
}

/// Days since 1970-01-01, as Howard Hinnant's civil calendar counts them.
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let y = year - i64::from(month <= 2);
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (month + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// Now, in Unix seconds.
pub fn now() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64)
}

#[cfg(test)]
mod tests {
    use super::*;

    const ANSWER: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<d:multistatus xmlns:d="DAV:">
 <d:response>
  <d:href>/dav/Skyward/</d:href>
  <d:propstat><d:prop><d:resourcetype><d:collection/></d:resourcetype></d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat>
 </d:response>
 <d:response>
  <d:href>/dav/Skyward/backups/</d:href>
  <d:propstat><d:prop><d:resourcetype><d:collection/></d:resourcetype><d:getlastmodified>Wed, 21 Oct 2015 07:28:00 GMT</d:getlastmodified></d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat>
 </d:response>
 <d:response>
  <d:href>/dav/Skyward/backup.toml</d:href>
  <d:propstat><d:prop><d:resourcetype/><d:getcontentlength>1234</d:getcontentlength><d:getetag>"abc"</d:getetag></d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat>
 </d:response>
</d:multistatus>"#;

    #[test]
    fn a_propfind_answer_is_read_into_entries() {
        let base = Url::parse("https://example.com/dav/").unwrap();
        let (responses, _) = parse_propfind(ANSWER, &base, "Skyward").unwrap();
        assert_eq!(responses.len(), 3);
        assert_eq!(responses[0].stat.kind, Kind::Folder);
        assert_eq!(responses[1].name("/dav/Skyward"), Some("backups".into()));
        assert_eq!(responses[2].name("/dav/Skyward"), Some("backup.toml".into()));
        assert_eq!(responses[2].stat.size, 1234);
        assert_eq!(responses[2].stat.etag.as_deref(), Some("abc"));
        assert_eq!(responses[1].stat.modified, 1_445_412_480);
    }

    #[test]
    fn a_date_is_read_as_http_writes_it() {
        assert_eq!(parse_http_date("Wed, 21 Oct 2015 07:28:00 GMT"), Some(1_445_412_480));
        assert_eq!(parse_http_date("not a date"), None);
    }

    #[test]
    fn segments_are_escaped() {
        let mut out = String::new();
        push_segment(&mut out, "a b/c");
        assert_eq!(out, "a%20b%2Fc");
    }

    #[test]
    fn a_propfind_asks_with_the_depth_and_the_way_in() {
        use std::io::{Read, Write};
        use std::net::TcpListener;
        use std::sync::{Arc, Mutex};
        use std::thread;

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = Vec::new();
            let mut buf = [0u8; 4096];
            while !request.windows(4).any(|w| w == b"\r\n\r\n") {
                let n = stream.read(&mut buf).unwrap();
                if n == 0 {
                    break;
                }
                request.extend_from_slice(&buf[..n]);
            }
            let text = String::from_utf8_lossy(&request).to_ascii_lowercase();
            assert!(text.starts_with("propfind /dav/skyward/ http/1.1"), "{text}");
            assert!(text.contains("depth: 0"), "{text}");
            assert!(text.contains("authorization: basic "), "{text}");
            let response = format!("HTTP/1.1 207 Multi-Status\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", ANSWER.len(), ANSWER);
            stream.write_all(response.as_bytes()).unwrap();
        });
        let config = Config {
            url: format!("http://127.0.0.1:{port}/dav"),
            username: "me".into(),
            password: "secret".into(),
            folder: "Skyward".into(),
        };
        let servers = Arc::new(Mutex::new(Vec::new()));
        let dav = WebDav::new(&config, servers).unwrap();
        let info = dav.test().unwrap();
        assert_eq!(info.server.as_deref(), Some("127.0.0.1"));
        server.join().unwrap();
    }

    #[test]
    fn the_folder_and_the_path_are_joined_under_the_address() {
        let config = Config {
            url: "https://example.com/dav".into(),
            username: String::new(),
            password: String::new(),
            folder: "Skyward".into(),
        };
        let servers = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let dav = WebDav::new(&config, servers).unwrap();
        assert_eq!(dav.url("/a b/c").unwrap().as_str(), "https://example.com/dav/Skyward/a%20b/c");
        assert_eq!(dav.url("").unwrap().as_str(), "https://example.com/dav/Skyward/");
    }
}
