// Copyright (c) 2026 Witalis Domitrz <witekdomitrz@gmail.com>
// AGPL License

//! Minimal HTTP/1.1 server: a buffered request reader and response
//! writer, mirroring the style of bot-web's `http` module but
//! self-contained. Only what this app needs: GET/POST with either raw
//! bodies (JSON) or multipart/form-data uploads (the image).

use std::io::{Read, Write};
use std::net::TcpStream;

/// Requests are images or small forms; 25 MB matches bot-web's upload cap.
const MAX_BODY_BYTES: usize = 25 * 1024 * 1024;
const MAX_HEAD_BYTES: usize = 32 * 1024;
const HEAD_SEPARATOR: &[u8] = b"\r\n\r\n";

pub(crate) struct Request {
    pub(crate) method: String,
    pub(crate) path: String,
    pub(crate) body: Vec<u8>,
    pub(crate) content_type: Option<String>,
}

pub(crate) struct Response {
    pub(crate) status: u16,
    pub(crate) content_type: &'static str,
    pub(crate) body: Vec<u8>,
}

impl Response {
    pub(crate) fn text(status: u16, body: impl Into<String>) -> Self {
        Self {
            status,
            content_type: "text/plain; charset=utf-8",
            body: body.into().into_bytes(),
        }
    }

    pub(crate) fn json(status: u16, body: impl Into<String>) -> Self {
        Self {
            status,
            content_type: "application/json",
            body: body.into().into_bytes(),
        }
    }

    pub(crate) fn html(status: u16, body: impl Into<String>) -> Self {
        Self {
            status,
            content_type: "text/html; charset=utf-8",
            body: body.into().into_bytes(),
        }
    }
}

fn reason(status: u16) -> &'static str {
    match status {
        400 => "Bad Request",
        404 => "Not Found",
        405 => "Method Not Allowed",
        413 => "Payload Too Large",
        500 => "Internal Server Error",
        _ => "OK",
    }
}

/// Write one response and flush; connection is closed afterwards.
pub(crate) fn respond(stream: &mut impl Write, response: &Response) -> std::io::Result<()> {
    write!(
        stream,
        "HTTP/1.1 {} {}\r\nContent-Type: {}\r\nCache-Control: no-store\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        response.status,
        reason(response.status),
        response.content_type,
        response.body.len()
    )?;
    stream.write_all(&response.body)
}

/// Read one request. `Ok(None)` means the client hung up or sent garbage —
/// just close. Oversized bodies still get a 413 response.
pub(crate) fn read_request(stream: &mut TcpStream) -> std::io::Result<Option<Request>> {
    let mut buffer = Vec::new();
    let mut chunk = [0_u8; 8192];
    let head_len = loop {
        if let Some(position) = find(&buffer, HEAD_SEPARATOR) {
            break position + HEAD_SEPARATOR.len();
        }
        if buffer.len() > MAX_HEAD_BYTES {
            return Ok(None);
        }
        let count = stream.read(&mut chunk)?;
        if count == 0 {
            return Ok(None);
        }
        buffer.extend_from_slice(&chunk[..count]);
    };
    let head = String::from_utf8_lossy(&buffer[..head_len]).into_owned();
    let mut start_line = head
        .split("\r\n")
        .next()
        .unwrap_or_default()
        .split_whitespace();
    let (Some(method), Some(path_full)) = (start_line.next(), start_line.next()) else {
        return Ok(None);
    };
    let path = path_full.split('?').next().unwrap_or_default().to_string();
    let content_type = head
        .lines()
        .find_map(|line| line.strip_prefix("Content-Type: "))
        .map(str::to_string);
    let content_length = head
        .lines()
        .find_map(|line| line.strip_prefix("Content-Length: "))
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(0);
    if content_length > MAX_BODY_BYTES {
        let _ = respond(stream, &Response::text(413, "request too large\n"));
        return Ok(None);
    }
    let mut body = buffer;
    body.drain(..head_len);
    while body.len() < content_length {
        let count = stream.read(&mut chunk)?;
        if count == 0 {
            break;
        }
        body.extend_from_slice(&chunk[..count]);
    }
    body.truncate(content_length);
    Ok(Some(Request {
        method: method.to_string(),
        path,
        body,
        content_type,
    }))
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || haystack.len() < needle.len() {
        return None;
    }
    (0..=haystack.len() - needle.len()).find(|&i| &haystack[i..i + needle.len()] == needle)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn find_locates_separator() {
        assert_eq!(find(b"abc\r\n\r\nrest", HEAD_SEPARATOR), Some(3));
        assert_eq!(find(b"no separator", HEAD_SEPARATOR), None);
        assert_eq!(find(b"", HEAD_SEPARATOR), None);
    }

    #[test]
    fn response_includes_status_and_length() {
        let mut stream = FakeStream::default();
        respond(&mut stream, &Response::text(404, "not found\n")).unwrap();
        let written = String::from_utf8(stream.0).unwrap();
        assert!(written.starts_with("HTTP/1.1 404 Not Found\r\n"));
        assert!(written.contains("Content-Length: 10\r\n"));
        assert!(written.ends_with("not found\n"));
    }

    /// A `TcpStream` stand-in so `respond` can be tested without a socket.
    #[derive(Default)]
    struct FakeStream(Vec<u8>);

    impl Read for FakeStream {
        fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
            Ok(0)
        }
    }

    impl Write for FakeStream {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn fake_stream_write_works() {
        let mut stream = FakeStream::default();
        stream.write_all(b"hello").unwrap();
        assert_eq!(stream.0, b"hello");
        // Silence the unused Read impl warning path: read returns Ok(0).
        let mut buf = [0_u8; 4];
        assert_eq!(stream.read(&mut buf).unwrap(), 0);
    }
}
