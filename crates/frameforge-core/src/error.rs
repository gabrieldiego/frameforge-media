use std::error::Error;
use std::fmt;

pub type Result<T> = std::result::Result<T, MediaError>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MediaError {
    InvalidDimensions {
        width: usize,
        height: usize,
    },
    IncompatibleFormat {
        format: &'static str,
        reason: String,
    },
    LengthOverflow,
    BufferLength {
        expected: usize,
        actual: usize,
    },
    Message(String),
}

impl fmt::Display for MediaError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidDimensions { width, height } => {
                write!(f, "invalid frame dimensions {width}x{height}")
            }
            Self::IncompatibleFormat { format, reason } => {
                write!(f, "incompatible {format} frame: {reason}")
            }
            Self::LengthOverflow => f.write_str("frame length overflowed addressable memory"),
            Self::BufferLength { expected, actual } => {
                write!(
                    f,
                    "buffer length mismatch: expected {expected} byte(s), got {actual}"
                )
            }
            Self::Message(message) => f.write_str(message),
        }
    }
}

impl Error for MediaError {}
