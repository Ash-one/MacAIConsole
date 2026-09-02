use std::fmt;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

pub const RUNNER_PROTOCOL_V1: &str = "macai.runner.v1";
pub const DEFAULT_MAX_FRAME_BYTES: usize = 8 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Envelope {
    pub protocol: String,
    #[serde(rename = "type")]
    pub message_type: String,
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instance_id: Option<String>,
    pub payload: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extensions: Option<Value>,
}

impl Envelope {
    pub fn new(message_type: impl Into<String>, id: impl Into<String>, payload: Value) -> Self {
        Self {
            protocol: RUNNER_PROTOCOL_V1.to_string(),
            message_type: message_type.into(),
            id: id.into(),
            instance_id: None,
            payload,
            extensions: None,
        }
    }
}

#[derive(Debug)]
pub enum ProtocolError {
    Io(std::io::Error),
    Json(serde_json::Error),
    FrameTooLarge { actual: usize, maximum: usize },
    NotAnObject,
    InvalidEnvelope(String),
}

impl fmt::Display for ProtocolError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(formatter, "runner protocol I/O error: {error}"),
            Self::Json(error) => write!(formatter, "runner protocol JSON error: {error}"),
            Self::FrameTooLarge { actual, maximum } => {
                write!(
                    formatter,
                    "runner protocol frame is {actual} bytes; maximum is {maximum}"
                )
            }
            Self::NotAnObject => write!(formatter, "runner protocol frame must be a JSON object"),
            Self::InvalidEnvelope(message) => {
                write!(formatter, "invalid runner protocol envelope: {message}")
            }
        }
    }
}

impl std::error::Error for ProtocolError {}

impl From<std::io::Error> for ProtocolError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<serde_json::Error> for ProtocolError {
    fn from(error: serde_json::Error) -> Self {
        Self::Json(error)
    }
}

pub async fn write_frame<W>(writer: &mut W, envelope: &Envelope) -> Result<(), ProtocolError>
where
    W: AsyncWrite + Unpin,
{
    let payload = serde_json::to_vec(envelope)?;
    let length = u32::try_from(payload.len()).map_err(|_| ProtocolError::FrameTooLarge {
        actual: payload.len(),
        maximum: u32::MAX as usize,
    })?;
    writer.write_all(&length.to_be_bytes()).await?;
    writer.write_all(&payload).await?;
    writer.flush().await?;
    Ok(())
}

pub async fn read_frame<R>(reader: &mut R, maximum: usize) -> Result<Envelope, ProtocolError>
where
    R: AsyncRead + Unpin,
{
    let mut header = [0_u8; 4];
    reader.read_exact(&mut header).await?;
    let length = u32::from_be_bytes(header) as usize;
    if length > maximum {
        return Err(ProtocolError::FrameTooLarge {
            actual: length,
            maximum,
        });
    }

    let mut payload = vec![0_u8; length];
    reader.read_exact(&mut payload).await?;
    let value: Value = serde_json::from_slice(&payload)?;
    if !value.is_object() {
        return Err(ProtocolError::NotAnObject);
    }
    let envelope: Envelope = serde_json::from_value(value)?;
    if envelope.protocol != RUNNER_PROTOCOL_V1 {
        return Err(ProtocolError::InvalidEnvelope(format!(
            "unsupported protocol '{}'",
            envelope.protocol
        )));
    }
    if envelope.message_type.trim().is_empty() || envelope.id.trim().is_empty() {
        return Err(ProtocolError::InvalidEnvelope(
            "type and id must not be empty".to_string(),
        ));
    }
    Ok(envelope)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tokio::io::AsyncWriteExt;

    #[tokio::test]
    async fn codec_handles_partial_and_coalesced_frames() {
        let (mut writer, mut reader) = tokio::io::duplex(4096);
        let first = Envelope::new("hello", "one", json!({"runner_id": "fake"}));
        let second = Envelope::new("initialized", "two", json!({}));
        let first_bytes = serde_json::to_vec(&first).unwrap();
        let second_bytes = serde_json::to_vec(&second).unwrap();
        tokio::spawn(async move {
            writer
                .write_all(&(first_bytes.len() as u32).to_be_bytes())
                .await
                .unwrap();
            writer.write_all(&first_bytes[..3]).await.unwrap();
            writer.write_all(&first_bytes[3..]).await.unwrap();
            writer
                .write_all(&(second_bytes.len() as u32).to_be_bytes())
                .await
                .unwrap();
            writer.write_all(&second_bytes).await.unwrap();
        });

        assert_eq!(read_frame(&mut reader, 1024).await.unwrap(), first);
        assert_eq!(read_frame(&mut reader, 1024).await.unwrap(), second);
    }

    #[tokio::test]
    async fn codec_rejects_oversize_and_non_object_frames() {
        let (mut writer, mut reader) = tokio::io::duplex(64);
        writer.write_all(&9_u32.to_be_bytes()).await.unwrap();
        let error = read_frame(&mut reader, 8).await.unwrap_err();
        assert!(matches!(error, ProtocolError::FrameTooLarge { .. }));

        let (mut writer, mut reader) = tokio::io::duplex(64);
        writer.write_all(&4_u32.to_be_bytes()).await.unwrap();
        writer.write_all(b"null").await.unwrap();
        let error = read_frame(&mut reader, 8).await.unwrap_err();
        assert!(matches!(error, ProtocolError::NotAnObject));
    }
}
