use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use sema_core::content_length::{parse_content_length, MAX_CONTENT_BYTES, MAX_HEADER_BYTES};

/// Read a DAP message from a Content-Length framed stream.
pub async fn read_message(
    reader: &mut (impl AsyncBufRead + Unpin),
) -> std::io::Result<Option<String>> {
    let mut headers = Vec::new();
    let mut line = Vec::new();
    loop {
        line.clear();
        let remaining = MAX_HEADER_BYTES.saturating_sub(headers.len());
        let mut bounded = reader.take(remaining.saturating_add(1) as u64);
        let n = bounded.read_until(b'\n', &mut line).await?;
        if n == 0 {
            return if headers.is_empty() {
                Ok(None)
            } else {
                Err(std::io::Error::new(
                    std::io::ErrorKind::UnexpectedEof,
                    "incomplete DAP header",
                ))
            };
        }
        if line.len() > remaining {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "DAP header exceeds the size limit",
            ));
        }
        headers.extend_from_slice(&line);
        if line == b"\r\n" {
            break;
        }
        if !line.ends_with(b"\r\n") {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "DAP headers must end with CRLF",
            ));
        }
    }
    let header_end = headers.len().saturating_sub(2);
    let len = parse_content_length(&headers[..header_end], MAX_CONTENT_BYTES)
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
    let mut body = vec![0u8; len];
    reader.read_exact(&mut body).await?;
    let body = String::from_utf8(body)
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
    Ok(Some(body))
}

/// Write a DAP message with Content-Length framing.
pub async fn write_message(
    writer: &mut (impl AsyncWrite + Unpin),
    body: &str,
) -> std::io::Result<()> {
    let header = format!("Content-Length: {}\r\n\r\n", body.len());
    writer.write_all(header.as_bytes()).await?;
    writer.write_all(body.as_bytes()).await?;
    writer.flush().await
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::BufReader;

    #[tokio::test]
    async fn framing_accepts_case_insensitive_length_and_rejects_ambiguity() {
        let mut reader = BufReader::new(b"content-length: 2\r\n\r\n{}".as_slice());
        assert_eq!(
            read_message(&mut reader).await.unwrap().as_deref(),
            Some("{}")
        );

        let mut duplicate =
            BufReader::new(b"Content-Length: 2\r\ncontent-length: 2\r\n\r\n{}".as_slice());
        assert_eq!(
            read_message(&mut duplicate).await.unwrap_err().kind(),
            std::io::ErrorKind::InvalidData
        );

        let oversized = format!("Content-Length: {}\r\n\r\n", MAX_CONTENT_BYTES + 1);
        let mut oversized = BufReader::new(oversized.as_bytes());
        assert_eq!(
            read_message(&mut oversized).await.unwrap_err().kind(),
            std::io::ErrorKind::InvalidData
        );
    }

    #[tokio::test]
    async fn framing_rejects_truncated_bodies() {
        let mut reader = BufReader::new(b"Content-Length: 4\r\n\r\n{}".as_slice());
        assert_eq!(
            read_message(&mut reader).await.unwrap_err().kind(),
            std::io::ErrorKind::UnexpectedEof
        );
    }

    #[tokio::test]
    async fn framing_bounds_a_header_without_a_newline() {
        let input = vec![b'x'; MAX_HEADER_BYTES + 1];
        let mut reader = BufReader::new(input.as_slice());
        assert_eq!(
            read_message(&mut reader).await.unwrap_err().kind(),
            std::io::ErrorKind::InvalidData
        );
    }
}
