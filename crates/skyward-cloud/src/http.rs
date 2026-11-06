//! The little HTTP/1.1 client WebDAV needs: the methods it uses are not
//! among those the recovery's other client knows, and a server chosen by
//! the user may answer with a certificate no authority vouches for.

use anyhow::{anyhow, bail, Context, Result};
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{ClientConfig, ClientConnection, DigitallySignedStruct, SignatureScheme, StreamOwned};
use skyward_net::dns;
use skyward_net::http::Servers;
use std::fs::File;
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom, Write};
use std::net::{IpAddr, SocketAddr, TcpStream};
use std::sync::Arc;
use std::time::Duration;
use url::Url;

const CONNECT: Duration = Duration::from_secs(15);
const IO: Duration = Duration::from_secs(30);
const MAX_LINE: usize = 8 * 1024;
const MAX_HEADERS: usize = 64;
const REDIRECTS: usize = 5;

/// What a server answered, its body already written where the caller asked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Response {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body_len: u64,
}

impl Response {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(held, _)| held.eq_ignore_ascii_case(name)).map(|(_, value)| value.as_str())
    }
}

/// A connection to one server, over HTTP or HTTPS.
pub struct Client {
    servers: Servers,
    tls: Arc<ClientConfig>,
}

impl Client {
    pub fn new(servers: Servers) -> Result<Client> {
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let tls = ClientConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .context("the TLS versions")?
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(AnyCertificate))
            .with_no_client_auth();
        Ok(Client { servers, tls: Arc::new(tls) })
    }

    /// Ask `method` of `url`, writing the answer's body to `out`. `body` is
    /// a file sent as the request's body, or `bytes`; `max_body` is the most
    /// of the answer read.
    #[allow(clippy::too_many_arguments)]
    pub fn request(
        &self,
        method: &str,
        url: &Url,
        auth: Option<&str>,
        headers: &[(&str, String)],
        body: Option<&File>,
        bytes: Option<&[u8]>,
        out: &mut dyn Write,
        max_body: u64,
    ) -> Result<Response> {
        let mut method = method.to_string();
        let mut url = url.clone();
        let mut body = body;
        let mut bytes = bytes;
        let mut auth = auth;
        for _ in 0..=REDIRECTS {
            let response = self.once(&method, &url, auth, headers, body, bytes, out, max_body)?;
            if !matches!(response.status, 301 | 302 | 303 | 307 | 308) {
                return Ok(response);
            }
            let Some(location) = response.header("location") else { return Ok(response) };
            let next = url.join(location).with_context(|| format!("follow {location}"))?;
            if response.status == 303 {
                method = "GET".into();
                body = None;
                bytes = None;
            }
            if let Some(file) = body {
                let mut file = file;
                file.seek(SeekFrom::Start(0)).context("rewind the request's body")?;
            }
            if next.host_str() != url.host_str() {
                auth = None;
            }
            url = next;
        }
        bail!("the server sends the request round too many times")
    }

    #[allow(clippy::too_many_arguments)]
    fn once(
        &self,
        method: &str,
        url: &Url,
        auth: Option<&str>,
        headers: &[(&str, String)],
        body: Option<&File>,
        bytes: Option<&[u8]>,
        out: &mut dyn Write,
        max_body: u64,
    ) -> Result<Response> {
        let host = url.host_str().context("the address names no host")?;
        let port = url.port_or_known_default().context("the address names no port")?;
        let mut stream = self.connect(host, port, url.scheme())?;
        let path = match url.query() {
            Some(query) => format!("{}?{query}", url.path()),
            None => url.path().to_string(),
        };
        let mut head = String::new();
        head.push_str(&format!("{method} {path} HTTP/1.1\r\n"));
        head.push_str(&format!("Host: {}\r\n", host_header(url)));
        head.push_str("User-Agent: skyward-recovery\r\nConnection: close\r\n");
        if let Some(auth) = auth {
            head.push_str(&format!("Authorization: {auth}\r\n"));
        }
        for (name, value) in headers {
            head.push_str(&format!("{name}: {value}\r\n"));
        }
        let length = match (bytes, body) {
            (Some(bytes), _) => Some(bytes.len() as u64),
            (None, Some(file)) => Some(file.metadata().context("read the request's body")?.len()),
            (None, None) => None,
        };
        if let Some(length) = length {
            head.push_str(&format!("Content-Length: {length}\r\n"));
        }
        head.push_str("\r\n");
        stream.write_all(head.as_bytes()).context("send the request's head")?;
        if let Some(bytes) = bytes {
            stream.write_all(bytes).context("send the request's body")?;
        }
        if let Some(file) = body {
            let mut file = file;
            file.seek(SeekFrom::Start(0)).context("rewind the request's body")?;
            std::io::copy(&mut file, &mut stream).context("send the request's body")?;
        }
        stream.flush().context("send the request")?;

        let mut reader = BufReader::new(stream);
        let (status, headers) = loop {
            let status = read_status(&mut reader)?;
            let headers = read_headers(&mut reader)?;
            if status >= 200 {
                break (status, headers);
            }
        };
        let redirect = matches!(status, 301 | 302 | 303 | 307 | 308);
        let has_body = headers.iter().any(|(name, _)| name == "content-length" || name == "transfer-encoding");
        let read = matches!(method, "GET" | "PROPFIND") || has_body;
        let mut discard = Vec::new();
        let sink: &mut dyn Write = if redirect { &mut discard } else { out };
        let body_len = if read { read_body(&mut reader, &headers, sink, max_body)? } else { 0 };
        Ok(Response { status, headers, body_len })
    }

    fn connect(&self, host: &str, port: u16, scheme: &str) -> Result<Box<dyn Stream>> {
        let bare = host.trim_start_matches('[').trim_end_matches(']');
        let addresses: Vec<IpAddr> = match bare.parse::<IpAddr>() {
            Ok(address) => vec![address],
            Err(_) => {
                let servers = self.servers.lock().expect("the DNS servers are not held by a thread that panicked").clone();
                dns::lookup(bare, &servers)?.into_iter().map(IpAddr::V4).collect()
            }
        };
        let mut refused = Vec::new();
        for address in addresses {
            let at = SocketAddr::new(address, port);
            match TcpStream::connect_timeout(&at, CONNECT) {
                Ok(stream) => {
                    stream.set_read_timeout(Some(IO)).context("time the connection")?;
                    stream.set_write_timeout(Some(IO)).context("time the connection")?;
                    if scheme == "https" {
                        let name = ServerName::try_from(bare.to_string()).context("the server's name")?;
                        let connection = ClientConnection::new(self.tls.clone(), name).context("start TLS")?;
                        return Ok(Box::new(StreamOwned::new(connection, stream)));
                    }
                    return Ok(Box::new(stream));
                }
                Err(e) => refused.push(format!("{at}: {e}")),
            }
        }
        Err(anyhow!("could not reach {host}: {}", refused.join("; ")))
    }
}

trait Stream: Read + Write {}
impl<T: Read + Write> Stream for T {}

fn host_header(url: &Url) -> String {
    let host = url.host_str().unwrap_or_default();
    match url.port() {
        Some(port) => format!("{host}:{port}"),
        None => host.to_string(),
    }
}

fn read_status(reader: &mut BufReader<Box<dyn Stream>>) -> Result<u16> {
    let mut line = String::new();
    let n = reader.read_line(&mut line).context("read the answer's status")?;
    if n == 0 || n > MAX_LINE {
        bail!("the server closed before its answer");
    }
    let mut parts = line.split_whitespace();
    let version = parts.next().context("the answer has no version")?;
    if !version.starts_with("HTTP/1.") {
        bail!("the server answered {version}");
    }
    parts.next().context("the answer has no status")?.parse().context("the answer's status")
}

fn read_headers(reader: &mut BufReader<Box<dyn Stream>>) -> Result<Vec<(String, String)>> {
    let mut headers = Vec::new();
    loop {
        let mut line = String::new();
        let n = reader.read_line(&mut line).context("read the answer's head")?;
        if n == 0 || n > MAX_LINE {
            bail!("the answer's head ended short");
        }
        let line = line.trim_end_matches(['\r', '\n']);
        if line.is_empty() {
            return Ok(headers);
        }
        if headers.len() >= MAX_HEADERS {
            bail!("the answer's head holds too many headers");
        }
        let Some((name, value)) = line.split_once(':') else { bail!("the answer's head has a line without a colon") };
        headers.push((name.trim().to_ascii_lowercase(), value.trim().to_string()));
    }
}

fn read_body(reader: &mut BufReader<Box<dyn Stream>>, headers: &[(String, String)], out: &mut dyn Write, max_body: u64) -> Result<u64> {
    let header = |name: &str| headers.iter().find(|(held, _)| held.eq_ignore_ascii_case(name)).map(|(_, value)| value.as_str());
    if header("transfer-encoding").is_some_and(|v| v.to_ascii_lowercase().contains("chunked")) {
        return read_chunked(reader, out, max_body);
    }
    if let Some(length) = header("content-length") {
        let length: u64 = length.parse().context("the answer's length")?;
        if length > max_body {
            bail!("the answer is {length} bytes, more than {max_body}");
        }
        let mut limited = reader.take(length);
        let copied = std::io::copy(&mut limited, out).context("read the answer")?;
        if copied != length {
            bail!("the answer ended after {copied} of {length} bytes");
        }
        return Ok(copied);
    }
    let mut limited = reader.take(max_body.saturating_add(1));
    let copied = std::io::copy(&mut limited, out).context("read the answer")?;
    if copied > max_body {
        bail!("the answer is more than {max_body} bytes");
    }
    Ok(copied)
}

fn read_chunked(reader: &mut BufReader<Box<dyn Stream>>, out: &mut dyn Write, max_body: u64) -> Result<u64> {
    let mut total = 0u64;
    loop {
        let mut line = String::new();
        let n = reader.read_line(&mut line).context("read a chunk's size")?;
        if n == 0 || n > MAX_LINE {
            bail!("the answer's chunks ended short");
        }
        let size = line.trim().split(';').next().unwrap_or_default();
        let size = u64::from_str_radix(size, 16).context("read a chunk's size")?;
        if size == 0 {
            loop {
                let mut trailer = String::new();
                let n = reader.read_line(&mut trailer).context("read the answer's trailers")?;
                if n == 0 || trailer.trim().is_empty() {
                    return Ok(total);
                }
            }
        }
        total = total.saturating_add(size);
        if total > max_body {
            bail!("the answer is more than {max_body} bytes");
        }
        let mut limited = reader.take(size);
        let copied = std::io::copy(&mut limited, out).context("read a chunk")?;
        if copied != size {
            bail!("a chunk ended after {copied} of {size} bytes");
        }
        let mut crlf = [0u8; 2];
        reader.read_exact(&mut crlf).context("read a chunk's end")?;
    }
}

/// Takes any certificate the server gives: the cloud page lets the user
/// choose the server, and its certificate is theirs to judge.
#[derive(Debug)]
struct AnyCertificate;

impl ServerCertVerifier for AnyCertificate {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> std::result::Result<ServerCertVerified, rustls::Error> {
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> std::result::Result<HandshakeSignatureValid, rustls::Error> {
        Ok(HandshakeSignatureValid::assertion())
    }

    fn verify_tls13_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> std::result::Result<HandshakeSignatureValid, rustls::Error> {
        Ok(HandshakeSignatureValid::assertion())
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        rustls::crypto::ring::default_provider().signature_verification_algorithms.supported_schemes()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_host_header_keeps_an_ipv6_address_in_brackets() {
        let url = Url::parse("http://[::1]:8080/dav").unwrap();
        assert_eq!(host_header(&url), "[::1]:8080");
    }
}
