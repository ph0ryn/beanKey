use std::error::Error;
use std::fmt;
use std::io::{self, Read, Write};

use prost::Message;

mod config;
mod engine;
mod server;
mod zenz;

pub const PROTOCOL_VERSION: u32 = 1;
pub const MAXIMUM_MESSAGE_SIZE: usize = 1024 * 1024;

pub mod protocol {
    include!(concat!(env!("OUT_DIR"), "/beankey.v1.rs"));
}

pub use config::{
    ConversionConfig, DaemonConfig, DaemonConfigError, InferenceConfig, InputStyleConfig,
    KeyboardLanguageConfig, LearningConfig, LearningModeConfig, LmTypoCorrectionConfig,
    LmTypoLanguageModel, PersonalizationConfig, PredictionConfig, PunctuationStyleConfig,
    TypoCorrectionConfig, TypoNGramConfig, ZenzConfig,
};
pub use engine::{ConversionResourceError, Engine, EngineOpenError};
pub use server::{DaemonServer, ServerError};
pub use zenz::{DEFAULT_INFERENCE_LIMIT, LlamaModel, ZenzConversionError};

#[derive(Debug)]
pub enum FrameError {
    Io(io::Error),
    InvalidLength,
    MessageTooLarge(usize),
    InvalidProtobuf(prost::DecodeError),
    UnsupportedVersion(u32),
    MissingPayload,
}

impl fmt::Display for FrameError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => error.fmt(formatter),
            Self::InvalidLength => write!(formatter, "invalid varint frame length"),
            Self::MessageTooLarge(size) => {
                write!(formatter, "frame size {size} exceeds the 1 MiB limit")
            }
            Self::InvalidProtobuf(error) => write!(formatter, "invalid protobuf payload: {error}"),
            Self::UnsupportedVersion(version) => {
                write!(formatter, "unsupported protocol version {version}")
            }
            Self::MissingPayload => write!(formatter, "envelope has no payload"),
        }
    }
}

impl Error for FrameError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::InvalidProtobuf(error) => Some(error),
            _ => None,
        }
    }
}

impl From<io::Error> for FrameError {
    fn from(value: io::Error) -> Self {
        Self::Io(value)
    }
}

pub fn read_envelope(reader: &mut impl Read) -> Result<protocol::Envelope, FrameError> {
    let size = read_varint(reader)?;
    if size > MAXIMUM_MESSAGE_SIZE {
        return Err(FrameError::MessageTooLarge(size));
    }
    let mut payload = vec![0; size];
    reader.read_exact(&mut payload)?;
    let envelope =
        protocol::Envelope::decode(payload.as_slice()).map_err(FrameError::InvalidProtobuf)?;
    validate_envelope(&envelope)?;
    Ok(envelope)
}

pub fn write_envelope(
    writer: &mut impl Write,
    envelope: &protocol::Envelope,
) -> Result<(), FrameError> {
    validate_envelope(envelope)?;
    let size = envelope.encoded_len();
    if size > MAXIMUM_MESSAGE_SIZE {
        return Err(FrameError::MessageTooLarge(size));
    }
    write_varint(writer, size)?;
    let mut payload = Vec::with_capacity(size);
    envelope
        .encode(&mut payload)
        .expect("encoding into a Vec cannot fail");
    writer.write_all(&payload)?;
    writer.flush()?;
    Ok(())
}

pub fn validate_envelope(envelope: &protocol::Envelope) -> Result<(), FrameError> {
    if envelope.protocol_version != PROTOCOL_VERSION {
        return Err(FrameError::UnsupportedVersion(envelope.protocol_version));
    }
    if envelope.payload.is_none() {
        return Err(FrameError::MissingPayload);
    }
    Ok(())
}

fn read_varint(reader: &mut impl Read) -> Result<usize, FrameError> {
    let mut value = 0_u64;
    for shift in (0..=63).step_by(7) {
        let mut byte = [0];
        reader.read_exact(&mut byte)?;
        value |= u64::from(byte[0] & 0x7f) << shift;
        if byte[0] & 0x80 == 0 {
            return usize::try_from(value).map_err(|_| FrameError::InvalidLength);
        }
    }
    Err(FrameError::InvalidLength)
}

fn write_varint(writer: &mut impl Write, value: usize) -> Result<(), FrameError> {
    let mut value = u64::try_from(value).map_err(|_| FrameError::InvalidLength)?;
    loop {
        let mut byte = u8::try_from(value & 0x7f).expect("masked varint byte fits u8");
        value >>= 7;
        if value != 0 {
            byte |= 0x80;
        }
        writer.write_all(&[byte])?;
        if value == 0 {
            return Ok(());
        }
    }
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use super::protocol::envelope::Payload;
    use super::*;

    fn reset_envelope() -> protocol::Envelope {
        protocol::Envelope {
            protocol_version: PROTOCOL_VERSION,
            request_id: 42,
            session_id: "session".into(),
            payload: Some(Payload::ResetSession(protocol::ResetSession {})),
            trace: Vec::new(),
        }
    }

    #[test]
    fn round_trips_a_length_delimited_envelope() {
        let expected = reset_envelope();
        let mut frame = Vec::new();
        write_envelope(&mut frame, &expected).unwrap();
        assert_eq!(read_envelope(&mut Cursor::new(frame)).unwrap(), expected);
    }

    #[test]
    fn rejects_a_frame_before_reading_an_oversized_payload() {
        let mut frame = Vec::new();
        write_varint(&mut frame, MAXIMUM_MESSAGE_SIZE + 1).unwrap();
        assert!(matches!(
            read_envelope(&mut Cursor::new(frame)),
            Err(FrameError::MessageTooLarge(size)) if size == MAXIMUM_MESSAGE_SIZE + 1
        ));
    }

    #[test]
    fn rejects_missing_payload_and_unknown_versions() {
        let mut envelope = reset_envelope();
        envelope.payload = None;
        assert!(matches!(
            validate_envelope(&envelope),
            Err(FrameError::MissingPayload)
        ));
        envelope.payload = Some(Payload::ResetSession(protocol::ResetSession {}));
        envelope.protocol_version = PROTOCOL_VERSION + 1;
        assert!(matches!(
            validate_envelope(&envelope),
            Err(FrameError::UnsupportedVersion(version)) if version == PROTOCOL_VERSION + 1
        ));
    }

    #[test]
    fn rejects_malformed_protobuf() {
        let frame = vec![1, 0xff];
        assert!(matches!(
            read_envelope(&mut Cursor::new(frame)),
            Err(FrameError::InvalidProtobuf(_))
        ));
    }
}
