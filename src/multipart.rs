// Copyright (c) 2026 Witalis Domitrz <witekdomitrz@gmail.com>
// AGPL License

//! Minimal `multipart/form-data` parsing — just enough for the convert
//! form: text fields and one file upload (the picture). Binary parts must
//! survive losslessly, so everything works on raw bytes.

/// One form part: `name` from Content-Disposition, plus either the decoded
/// UTF-8 value (text fields) or the raw bytes (files).
pub(crate) struct Part {
    pub(crate) name: String,
    /// Present on file uploads; only surfaced in tests, kept for clarity.
    #[allow(dead_code)]
    pub(crate) filename: Option<String>,
    pub(crate) body: Vec<u8>,
}

impl Part {
    /// The part's body as text (for form fields).
    pub(crate) fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }
}

/// Parse a multipart body. Returns `None` when the content type isn't
/// multipart or the body is malformed (an `Err` would overstate the
/// failure modes: the caller treats every `None` as a 400).
#[allow(clippy::unnecessary_wraps)]
pub(crate) fn parse(content_type: &str, body: &[u8]) -> Option<Vec<Part>> {
    let boundary = content_type
        .split(';')
        .map(str::trim)
        .find_map(|param| param.strip_prefix("boundary="))?
        .trim_matches('"');
    // Parts are delimited by --boundary, with CRLF line endings around
    // each part's headers.
    let delimiter = format!("--{boundary}");
    let mut parts = Vec::new();
    let mut cursor = find(body, delimiter.as_bytes())?;
    cursor += delimiter.len();
    loop {
        // After a delimiter: either `--` (final) or CRLF then the part.
        if body[cursor..].starts_with(b"--") {
            break;
        }
        cursor = skip_crlf(body, cursor);
        let headers_end = find(&body[cursor..], b"\r\n\r\n")? + cursor;
        let headers = String::from_utf8_lossy(&body[cursor..headers_end]).into_owned();
        let (name, filename) = disposition(&headers)?;
        let content_start = headers_end + 4;
        // The part body runs to the next delimiter, minus its trailing CRLF.
        let next = find(&body[content_start..], delimiter.as_bytes())? + content_start;
        let content_end = next.saturating_sub(2); // strip "\r\n"
        parts.push(Part {
            name,
            filename,
            body: body[content_start..content_end].to_vec(),
        });
        cursor = next + delimiter.len();
    }
    Some(parts)
}

/// Extract `name` and optional `filename` from the part's
/// Content-Disposition header.
fn disposition(headers: &str) -> Option<(String, Option<String>)> {
    let line = headers.lines().find(|line| {
        line.to_ascii_lowercase()
            .starts_with("content-disposition:")
    })?;
    let mut name = None;
    let mut filename = None;
    for param in line.split(';').map(str::trim) {
        if let Some(value) = param.strip_prefix("name=") {
            name = Some(value.trim_matches('"').to_string());
        } else if let Some(value) = param.strip_prefix("filename=") {
            filename = Some(value.trim_matches('"').to_string());
        }
    }
    Some((name?, filename))
}

/// Skip the CRLF after a boundary delimiter (present unless the part is
/// empty); the cursor is returned unchanged when there is none.
fn skip_crlf(body: &[u8], mut cursor: usize) -> usize {
    if body
        .get(cursor..)
        .is_some_and(|rest| rest.starts_with(b"\r\n"))
    {
        cursor += 2;
    }
    cursor
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

    const BOUNDARY: &str = "XyZZy123";

    fn build_body(fields: &[(&str, &str)], file: Option<(&str, &str, &[u8])>) -> Vec<u8> {
        let mut body = Vec::new();
        for (name, value) in fields {
            body.extend_from_slice(
                format!(
                    "--{BOUNDARY}\r\nContent-Disposition: form-data; name=\"{name}\"\r\n\r\n{value}\r\n"
                )
                .as_bytes(),
            );
        }
        if let Some((name, filename, bytes)) = file {
            body.extend_from_slice(
                format!(
                    "--{BOUNDARY}\r\nContent-Disposition: form-data; name=\"{name}\"; \
                     filename=\"{filename}\"\r\nContent-Type: image/png\r\n\r\n"
                )
                .as_bytes(),
            );
            body.extend_from_slice(bytes);
            body.extend_from_slice(b"\r\n");
        }
        body.extend_from_slice(format!("--{BOUNDARY}--\r\n").as_bytes());
        body
    }

    #[test]
    fn parses_fields_and_binary_file() {
        let file_bytes = [0_u8, 1, 2, 0x0D, 0x0A, 0xFF, 0x89, 0x50, 0x4E, 0x47];
        let body = build_body(
            &[("width", "48"), ("dither", "on")],
            Some(("image", "cat.png", &file_bytes)),
        );
        let parts = parse(&format!("multipart/form-data; boundary={BOUNDARY}"), &body).unwrap();
        assert_eq!(parts.len(), 3);
        assert_eq!(parts[0].name, "width");
        assert_eq!(parts[0].text(), "48");
        assert_eq!(parts[2].filename.as_deref(), Some("cat.png"));
        assert_eq!(parts[2].body, file_bytes);
    }

    #[test]
    fn binary_content_may_contain_delimiter_like_bytes() {
        // A file containing something that looks like a header must not
        // confuse the parser (delimiters are boundary-prefixed).
        let tricky = b"\r\nContent-Disposition: form-data; name=\"fake\"\r\n\r\nx";
        let body = build_body(&[], Some(("image", "a.png", tricky)));
        let parts = parse(&format!("multipart/form-data; boundary={BOUNDARY}"), &body).unwrap();
        assert_eq!(parts.len(), 1);
        assert_eq!(parts[0].body, tricky);
    }

    #[test]
    fn non_multipart_content_type_is_rejected() {
        assert!(parse("application/json", b"{}").is_none());
    }

    #[test]
    fn truncated_body_is_rejected() {
        let body = b"--XyZZy123\r\nContent-Disposition: form-data; name=\"a\"\r\n\r\nvalue";
        assert!(parse("multipart/form-data; boundary=XyZZy123", body).is_none());
    }
}
