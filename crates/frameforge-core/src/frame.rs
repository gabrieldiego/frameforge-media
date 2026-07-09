use crate::error::{MediaError, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PixelFormat {
    Yuv420p8,
    Yuv444p8,
    Rgb24,
}

impl PixelFormat {
    pub fn name(self) -> &'static str {
        match self {
            Self::Yuv420p8 => "yuv420p8",
            Self::Yuv444p8 => "yuv444p8",
            Self::Rgb24 => "rgb24",
        }
    }

    pub fn frame_len(self, width: usize, height: usize) -> Option<usize> {
        let pixels = width.checked_mul(height)?;
        match self {
            Self::Yuv420p8 => pixels.checked_add(pixels.checked_div(2)?),
            Self::Yuv444p8 | Self::Rgb24 => pixels.checked_mul(3),
        }
    }

    fn validate_geometry(self, width: usize, height: usize) -> Result<()> {
        if width == 0 || height == 0 {
            return Err(MediaError::InvalidDimensions { width, height });
        }

        if self == Self::Yuv420p8 && (width % 2 != 0 || height % 2 != 0) {
            return Err(MediaError::IncompatibleFormat {
                format: self.name(),
                reason: "width and height must be even".to_string(),
            });
        }

        self.frame_len(width, height)
            .ok_or(MediaError::LengthOverflow)?;
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameInfo {
    pub width: usize,
    pub height: usize,
    pub format: PixelFormat,
}

impl FrameInfo {
    pub fn new(width: usize, height: usize, format: PixelFormat) -> Result<Self> {
        format.validate_geometry(width, height)?;
        Ok(Self {
            width,
            height,
            format,
        })
    }

    pub fn expected_len(self) -> usize {
        self.format
            .frame_len(self.width, self.height)
            .expect("validated frame dimensions must have a byte length")
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    info: FrameInfo,
    data: Vec<u8>,
}

impl Frame {
    pub fn new(info: FrameInfo, data: Vec<u8>) -> Result<Self> {
        let expected = info.expected_len();
        let actual = data.len();
        if actual != expected {
            return Err(MediaError::BufferLength { expected, actual });
        }

        Ok(Self { info, data })
    }

    pub fn blank(info: FrameInfo) -> Self {
        Self {
            info,
            data: vec![0; info.expected_len()],
        }
    }

    pub fn info(&self) -> FrameInfo {
        self.info
    }

    pub fn data(&self) -> &[u8] {
        &self.data
    }

    pub fn into_data(self) -> Vec<u8> {
        self.data
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn computes_common_frame_lengths() {
        assert_eq!(PixelFormat::Yuv420p8.frame_len(16, 16), Some(384));
        assert_eq!(PixelFormat::Yuv444p8.frame_len(16, 16), Some(768));
        assert_eq!(PixelFormat::Rgb24.frame_len(16, 16), Some(768));
    }

    #[test]
    fn rejects_odd_420_dimensions() {
        let err = FrameInfo::new(15, 16, PixelFormat::Yuv420p8).unwrap_err();
        assert!(err.to_string().contains("must be even"));
    }

    #[test]
    fn validates_frame_buffer_length() {
        let info = FrameInfo::new(8, 8, PixelFormat::Yuv444p8).unwrap();
        let frame = Frame::new(info, vec![0; 192]);
        assert!(frame.is_ok());

        let err = Frame::new(info, vec![0; 191]).unwrap_err();
        assert_eq!(
            err,
            MediaError::BufferLength {
                expected: 192,
                actual: 191,
            }
        );
    }
}
