use std::fmt;
use std::str::FromStr;

use crate::error::{MediaError, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PixelFormat {
    Yuv420p8,
    Yuv420p10,
    Yuv420p12,
    Yuv420p16,
    Yuv422p8,
    Yuv422p10,
    Yuv422p12,
    Yuv422p16,
    Yuv444p8,
    Yuv444p10,
    Yuv444p12,
    Yuv444p16,
    Gray8,
    Gray10,
    Gray12,
    Gray16,
    Rgb24,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SampleBitDepth {
    Eight,
    Ten,
    Twelve,
    Sixteen,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChromaSampling {
    Monochrome,
    Cs420,
    Cs422,
    Cs444,
}

impl ChromaSampling {
    pub fn chroma_plane_samples(self, width: usize, height: usize) -> Option<usize> {
        let luma = width.checked_mul(height)?;
        match self {
            Self::Monochrome => Some(0),
            Self::Cs420 => luma.checked_div(4),
            Self::Cs422 => luma.checked_div(2),
            Self::Cs444 => Some(luma),
        }
    }
}

impl SampleBitDepth {
    pub fn bits(self) -> u8 {
        match self {
            Self::Eight => 8,
            Self::Ten => 10,
            Self::Twelve => 12,
            Self::Sixteen => 16,
        }
    }

    pub fn bytes_per_sample(self) -> usize {
        if self.bits() <= 8 {
            1
        } else {
            2
        }
    }

    pub fn max_sample(self) -> u16 {
        (1u32.checked_shl(self.bits() as u32).unwrap() - 1) as u16
    }
}

impl PixelFormat {
    pub fn name(self) -> &'static str {
        match self {
            Self::Yuv420p8 => "yuv420p8",
            Self::Yuv420p10 => "yuv420p10le",
            Self::Yuv420p12 => "yuv420p12le",
            Self::Yuv420p16 => "yuv420p16le",
            Self::Yuv422p8 => "yuv422p8",
            Self::Yuv422p10 => "yuv422p10le",
            Self::Yuv422p12 => "yuv422p12le",
            Self::Yuv422p16 => "yuv422p16le",
            Self::Yuv444p8 => "yuv444p8",
            Self::Yuv444p10 => "yuv444p10le",
            Self::Yuv444p12 => "yuv444p12le",
            Self::Yuv444p16 => "yuv444p16le",
            Self::Gray8 => "gray8",
            Self::Gray10 => "gray10le",
            Self::Gray12 => "gray12le",
            Self::Gray16 => "gray16le",
            Self::Rgb24 => "rgb24",
        }
    }

    pub fn bit_depth(self) -> SampleBitDepth {
        match self {
            Self::Yuv420p8 | Self::Yuv422p8 | Self::Yuv444p8 | Self::Gray8 | Self::Rgb24 => {
                SampleBitDepth::Eight
            }
            Self::Yuv420p10 | Self::Yuv422p10 | Self::Yuv444p10 | Self::Gray10 => {
                SampleBitDepth::Ten
            }
            Self::Yuv420p12 | Self::Yuv422p12 | Self::Yuv444p12 | Self::Gray12 => {
                SampleBitDepth::Twelve
            }
            Self::Yuv420p16 | Self::Yuv422p16 | Self::Yuv444p16 | Self::Gray16 => {
                SampleBitDepth::Sixteen
            }
        }
    }

    pub fn bytes_per_sample(self) -> usize {
        self.bit_depth().bytes_per_sample()
    }

    pub fn is_yuv420(self) -> bool {
        matches!(
            self,
            Self::Yuv420p8 | Self::Yuv420p10 | Self::Yuv420p12 | Self::Yuv420p16
        )
    }

    pub fn is_yuv(self) -> bool {
        self.chroma_sampling()
            .is_some_and(|sampling| sampling != ChromaSampling::Monochrome)
    }

    pub fn chroma_sampling(self) -> Option<ChromaSampling> {
        match self {
            Self::Yuv420p8 | Self::Yuv420p10 | Self::Yuv420p12 | Self::Yuv420p16 => {
                Some(ChromaSampling::Cs420)
            }
            Self::Yuv422p8 | Self::Yuv422p10 | Self::Yuv422p12 | Self::Yuv422p16 => {
                Some(ChromaSampling::Cs422)
            }
            Self::Yuv444p8 | Self::Yuv444p10 | Self::Yuv444p12 | Self::Yuv444p16 => {
                Some(ChromaSampling::Cs444)
            }
            Self::Gray8 | Self::Gray10 | Self::Gray12 | Self::Gray16 => {
                Some(ChromaSampling::Monochrome)
            }
            Self::Rgb24 => None,
        }
    }

    pub fn chroma_plane_samples(self, width: usize, height: usize) -> Option<usize> {
        self.chroma_sampling()?.chroma_plane_samples(width, height)
    }

    pub fn frame_len(self, width: usize, height: usize) -> Option<usize> {
        let luma = width.checked_mul(height)?;
        let bytes_per_sample = self.bytes_per_sample();
        match self {
            Self::Yuv420p8
            | Self::Yuv420p10
            | Self::Yuv420p12
            | Self::Yuv420p16
            | Self::Yuv422p8
            | Self::Yuv422p10
            | Self::Yuv422p12
            | Self::Yuv422p16
            | Self::Yuv444p8
            | Self::Yuv444p10
            | Self::Yuv444p12
            | Self::Yuv444p16 => {
                let chroma_plane = self.chroma_plane_samples(width, height)?;
                luma.checked_add(chroma_plane.checked_mul(2)?)?
                    .checked_mul(bytes_per_sample)
            }
            Self::Gray8 | Self::Gray10 | Self::Gray12 | Self::Gray16 => {
                luma.checked_mul(bytes_per_sample)
            }
            Self::Rgb24 => luma.checked_mul(3),
        }
    }

    pub fn validate_geometry(self, width: usize, height: usize) -> Result<()> {
        if width == 0 || height == 0 {
            return Err(MediaError::InvalidDimensions { width, height });
        }

        if self.is_yuv420() && (width % 2 != 0 || height % 2 != 0) {
            return Err(MediaError::IncompatibleFormat {
                format: self.name(),
                reason: "width and height must be even".to_string(),
            });
        }

        if matches!(self.chroma_sampling(), Some(ChromaSampling::Cs422)) && width % 2 != 0 {
            return Err(MediaError::IncompatibleFormat {
                format: self.name(),
                reason: "width must be even".to_string(),
            });
        }

        self.frame_len(width, height)
            .ok_or(MediaError::LengthOverflow)?;
        Ok(())
    }
}

impl FromStr for PixelFormat {
    type Err = String;

    fn from_str(value: &str) -> std::result::Result<Self, Self::Err> {
        match value {
            "yuv420p" | "yuv420p8" | "i420" => Ok(Self::Yuv420p8),
            "yuv420p10" | "yuv420p10le" | "i010" => Ok(Self::Yuv420p10),
            "yuv420p12" | "yuv420p12le" | "i012" => Ok(Self::Yuv420p12),
            "yuv420p16" | "yuv420p16le" | "i016" => Ok(Self::Yuv420p16),
            "yuv422p" | "yuv422p8" | "i422" => Ok(Self::Yuv422p8),
            "yuv422p10" | "yuv422p10le" | "i210" => Ok(Self::Yuv422p10),
            "yuv422p12" | "yuv422p12le" | "i212" => Ok(Self::Yuv422p12),
            "yuv422p16" | "yuv422p16le" | "i216" => Ok(Self::Yuv422p16),
            "yuv444p" | "yuv444p8" | "i444" => Ok(Self::Yuv444p8),
            "yuv444p10" | "yuv444p10le" | "i410" => Ok(Self::Yuv444p10),
            "yuv444p12" | "yuv444p12le" | "i412" => Ok(Self::Yuv444p12),
            "yuv444p16" | "yuv444p16le" | "i416" => Ok(Self::Yuv444p16),
            "gray8" | "y8" => Ok(Self::Gray8),
            "gray10" | "gray10le" | "y10" | "y10le" => Ok(Self::Gray10),
            "gray12" | "gray12le" | "y12" | "y12le" => Ok(Self::Gray12),
            "gray16" | "gray16le" | "y16" | "y16le" => Ok(Self::Gray16),
            "rgb24" => Ok(Self::Rgb24),
            other => Err(format!(
                "unsupported format '{other}'; supported formats: yuv420p, yuv422p, yuv444p, gray, and rgb24"
            )),
        }
    }
}

impl fmt::Display for PixelFormat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
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
        assert_eq!(PixelFormat::Yuv420p10.frame_len(16, 16), Some(768));
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

    #[test]
    fn parses_hardware_model_pixel_format_aliases() {
        assert_eq!("i010".parse::<PixelFormat>(), Ok(PixelFormat::Yuv420p10));
        assert_eq!("yuv444p".parse::<PixelFormat>(), Ok(PixelFormat::Yuv444p8));
        assert_eq!("rgb24".parse::<PixelFormat>(), Ok(PixelFormat::Rgb24));
    }
}
