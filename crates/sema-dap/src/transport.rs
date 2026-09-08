use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncWrite, AsyncWriteExt};

use sema_core::content_length::{parse_content_length, MAX_CONTENT_BYTES, MAX_HEADER_BYTES};

/// Retains framing progress when the server cancels a read to handle an event.
pub(crate) struct MessageReader<R> {
    reader: R,
    headers: Vec<u8>,
    line_start: usize,
    content_length: Option<usize>,
    body: Vec<u8>,
}

impl<R: AsyncBufRead + Unpin> MessageReader<R> {
    pub(crate) fn new(reader: R) -> Self {
        Self {
            reader,
            headers: Vec::new(),
            line_start: 0,
            content_length: None,
            body: Vec::new(),
        }
    }

    pub(crate) async fn read_message(&mut self) -> std::io::Result<Option<String>> {
        while self.content_length.is_none() {
            // fill_buf is cancellation safe. All consumed bytes and parsing
            // progress remain in self before the next await.
            let available = self.reader.fill_buf().await?;
            if available.is_empty() {
                return if self.headers.is_empty() {
                    Ok(None)
                } else {
                    Err(std::io::Error::new(
                        std::io::ErrorKind::UnexpectedEof,
                        "incomplete DAP header",
                    ))
                };
            }
            let count = available
                .iter()
                .position(|byte| *byte == b'\n')
                .map_or(available.len(), |index| index + 1);
            if count > MAX_HEADER_BYTES.saturating_sub(self.headers.len()) {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "DAP header exceeds the size limit",
                ));
            }
            self.headers.extend_from_slice(&available[..count]);
            self.reader.consume(count);
            let line = &self.headers[self.line_start..];
            if !line.ends_with(b"\n") {
                continue;
            }
            if !line.ends_with(b"\r\n") {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "DAP headers must end with CRLF",
                ));
            }
            if line == b"\r\n" {
                let len = parse_content_length(&self.headers[..self.line_start], MAX_CONTENT_BYTES)
                    .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
                self.content_length = Some(len);
            }
            self.line_start = self.headers.len();
        }

        let len = self.content_length.unwrap_or(0);
        while self.body.len() < len {
            let available = self.reader.fill_buf().await?;
            if available.is_empty() {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::UnexpectedEof,
                    "incomplete DAP body",
                ));
            }
            let count = available.len().min(len - self.body.len());
            self.body.extend_from_slice(&available[..count]);
            self.reader.consume(count);
        }
        let body = String::from_utf8(std::mem::take(&mut self.body))
            .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
        self.headers.clear();
        self.line_start = 0;
        self.content_length = None;
        Ok(Some(body))
    }
}

/// Read a DAP message from a Content-Length framed stream.
///
/// This one-shot helper must not be cancelled and restarted on the same stream.
/// The server uses `MessageReader` to retain progress across debugger events.
pub async fn read_message(
    reader: &mut (impl AsyncBufRead + Unpin),
) -> std::io::Result<Option<String>> {
    MessageReader::new(reader).read_message().await
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
    async fn framing_preserves_partial_requests_when_events_interrupt_reads() {
        let frame = b"Content-Length: 7\r\n\r\n{\"x\":1}";
        for split in 1..frame.len() {
            let (mut input, output) = tokio::io::duplex(256);
            let mut reader = MessageReader::new(BufReader::new(output));
            input.write_all(&frame[..split]).await.unwrap();
            // Poll the read first so it consumes the available bytes, then
            // cancel it as the server does when a debugger event is ready.
            tokio::select! {
                biased;
                result = reader.read_message() => panic!("partial frame completed: {result:?}"),
                () = std::future::ready(()) => {}
            }
            input.write_all(&frame[split..]).await.unwrap();
            input.write_all(frame).await.unwrap();
            drop(input);
            assert_eq!(
                reader.read_message().await.unwrap().as_deref(),
                Some("{\"x\":1}"),
                "frame split at byte {split}"
            );
            assert_eq!(
                reader.read_message().await.unwrap().as_deref(),
                Some("{\"x\":1}")
            );
            assert_eq!(reader.read_message().await.unwrap(), None);
        }
    }

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
