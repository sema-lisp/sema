//! Parsing for the `Content-Length` headers used by LSP and DAP transports.

use std::fmt;

/// Maximum header size accepted by Sema's language protocol servers.
pub const MAX_HEADER_BYTES: usize = 16 * 1024;

/// Maximum JSON message size accepted by Sema's language protocol servers.
pub const MAX_CONTENT_BYTES: usize = 16 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContentLengthError {
    InvalidHeader,
    Missing,
    Duplicate,
    InvalidValue,
    TooLarge,
}

impl fmt::Display for ContentLengthError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            Self::InvalidHeader => "header is not valid ASCII",
            Self::Missing => "missing Content-Length header",
            Self::Duplicate => "duplicate Content-Length header",
            Self::InvalidValue => "invalid Content-Length value",
            Self::TooLarge => "Content-Length exceeds the message size limit",
        };
        f.write_str(message)
    }
}

impl std::error::Error for ContentLengthError {}

/// Parse one HTTP-style header block and return its declared body size.
///
/// Header names are ASCII case-insensitive. A duplicate `Content-Length` is
/// rejected even when both values agree, because accepting one value makes
/// framing ambiguous across protocol implementations.
pub fn parse_content_length(
    header: &[u8],
    max_content_bytes: usize,
) -> Result<usize, ContentLengthError> {
    let header = std::str::from_utf8(header).map_err(|_| ContentLengthError::InvalidHeader)?;
    if !header.is_ascii() {
        return Err(ContentLengthError::InvalidHeader);
    }

    let mut content_length = None;
    for line in header.split("\r\n") {
        if line.is_empty() {
            continue;
        }
        let Some((name, value)) = line.split_once(':') else {
            return Err(ContentLengthError::InvalidHeader);
        };
        if name.trim().eq_ignore_ascii_case("content-length") {
            if content_length.is_some() {
                return Err(ContentLengthError::Duplicate);
            }
            let value = value.trim();
            if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
                return Err(ContentLengthError::InvalidValue);
            }
            let length = value
                .parse::<usize>()
                .map_err(|_| ContentLengthError::InvalidValue)?;
            if length > max_content_bytes {
                return Err(ContentLengthError::TooLarge);
            }
            content_length = Some(length);
        }
    }
    content_length.ok_or(ContentLengthError::Missing)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_case_insensitive_header_with_flexible_spacing() {
        assert_eq!(
            parse_content_length(b"content-LENGTH:\t42\r\nX-Test: yes", 100),
            Ok(42)
        );
    }

    #[test]
    fn rejects_duplicate_invalid_and_oversized_lengths() {
        assert_eq!(
            parse_content_length(b"Content-Length: 1\r\ncontent-length: 1", 10),
            Err(ContentLengthError::Duplicate)
        );
        assert_eq!(
            parse_content_length(b"Content-Length: -1", 10),
            Err(ContentLengthError::InvalidValue)
        );
        assert_eq!(
            parse_content_length(b"Content-Length: 11", 10),
            Err(ContentLengthError::TooLarge)
        );
    }
}
