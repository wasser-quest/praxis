//! Bounded length-prefixed transport for checked provider wire envelopes.
//!
//! Reading a frame does not authenticate or validate its contents: callers must
//! pass the returned bytes through the appropriate checked wire decoder.
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use crate::{WireLimits, MAX_FRAME_BYTES};

#[derive(Debug, thiserror::Error)]
pub enum TransportError {
    #[error("invalid frame limit")]
    InvalidLimit,
    #[error("provider frame exceeds limit")]
    FrameTooLarge,
    #[error("truncated provider frame")]
    TruncatedFrame,
    #[error("provider transport I/O failed")]
    Io(#[source] std::io::Error),
}

fn limit(limits: WireLimits) -> Result<usize, TransportError> {
    let size = limits.frame_bytes;
    if size == 0 || size > MAX_FRAME_BYTES {
        return Err(TransportError::InvalidLimit);
    }
    Ok(size)
}

/// Clean EOF is returned only between frames. Length is checked before allocation.
pub async fn read_frame<R: AsyncRead + Unpin>(
    reader: &mut R,
    limits: WireLimits,
) -> Result<Option<Vec<u8>>, TransportError> {
    let maximum = limit(limits)?;
    let mut header = [0; 4];
    match reader.read(&mut header[..1]).await {
        Ok(0) => return Ok(None),
        Ok(_) => {}
        Err(error) => return Err(TransportError::Io(error)),
    }
    read_exact(reader, &mut header[1..]).await?;
    let length = u32::from_be_bytes(header) as usize;
    if length == 0 || length > maximum {
        return Err(TransportError::FrameTooLarge);
    }
    let mut bytes = vec![0; length];
    read_exact(reader, &mut bytes).await?;
    Ok(Some(bytes))
}

async fn read_exact<R: AsyncRead + Unpin>(
    reader: &mut R,
    bytes: &mut [u8],
) -> Result<(), TransportError> {
    reader.read_exact(bytes).await.map(|_| ()).map_err(|error| {
        if error.kind() == std::io::ErrorKind::UnexpectedEof {
            TransportError::TruncatedFrame
        } else {
            TransportError::Io(error)
        }
    })
}

/// Writes bytes already produced by a checked wire encoder.
pub async fn write_frame<W: AsyncWrite + Unpin>(
    writer: &mut W,
    bytes: &[u8],
    limits: WireLimits,
) -> Result<(), TransportError> {
    let maximum = limit(limits)?;
    if bytes.is_empty() || bytes.len() > maximum {
        return Err(TransportError::FrameTooLarge);
    }
    writer
        .write_all(&(bytes.len() as u32).to_be_bytes())
        .await
        .map_err(TransportError::Io)?;
    writer.write_all(bytes).await.map_err(TransportError::Io)?;
    writer.flush().await.map_err(TransportError::Io)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn roundtrip_and_clean_eof() {
        let mut bytes = Vec::new();
        write_frame(&mut bytes, b"{}", WireLimits::default())
            .await
            .unwrap();
        assert_eq!(&bytes[..4], &[0, 0, 0, 2]);
        let mut reader = bytes.as_slice();
        assert_eq!(
            read_frame(&mut reader, WireLimits::default())
                .await
                .unwrap(),
            Some(b"{}".to_vec())
        );
        assert!(read_frame(&mut reader, WireLimits::default())
            .await
            .unwrap()
            .is_none());
    }

    #[tokio::test]
    async fn incomplete_header_or_payload_is_not_clean_eof() {
        for bytes in [&[0u8][..], &[0, 0, 0][..], &[0, 0, 0, 2, b'{'][..]] {
            assert!(matches!(
                read_frame(&mut &*bytes, WireLimits::default()).await,
                Err(TransportError::TruncatedFrame)
            ));
        }
    }

    #[tokio::test]
    async fn rejects_length_before_reading_payload() {
        for length in [0, MAX_FRAME_BYTES as u32 + 1, u32::MAX] {
            let bytes = length.to_be_bytes();
            assert!(matches!(
                read_frame(&mut bytes.as_slice(), WireLimits::default()).await,
                Err(TransportError::FrameTooLarge)
            ));
        }
    }

    #[tokio::test]
    async fn invalid_limits_and_oversized_writes_emit_nothing() {
        let mut bytes = Vec::new();
        let mut limits = WireLimits {
            frame_bytes: 0,
            ..WireLimits::default()
        };
        assert!(matches!(
            write_frame(&mut bytes, b"x", limits).await,
            Err(TransportError::InvalidLimit)
        ));
        limits.frame_bytes = 1;
        assert!(matches!(
            write_frame(&mut bytes, b"xx", limits).await,
            Err(TransportError::FrameTooLarge)
        ));
        assert!(bytes.is_empty());
    }
}
