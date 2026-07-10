use std::fmt;
use std::str::FromStr;

use crate::error::{MediaError, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct SampleBitDepth(u8);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChromaSampling {
    Monochrome,
    Cs420,
    Cs422,
    Cs444,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PixelFormat {
    PlanarYuv {
        chroma_sampling: ChromaSampling,
        bit_depth: SampleBitDepth,
    },
    Gray {
        bit_depth: SampleBitDepth,
    },
    Rgb24,
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
    pub const fn bits(self) -> u8 {
        self.0
    }

    pub const fn new(bits: u8) -> Option<Self> {
        if bits >= 8 && bits <= 16 {
            Some(Self(bits))
        } else {
            None
        }
    }

    pub const fn from_bits(bits: u8) -> Option<Self> {
        Self::new(bits)
    }

    const fn new_unchecked(bits: u8) -> Self {
        Self(bits)
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
    // TODO(deprecated): prefer numeric constructors such as
    // `PixelFormat::yuv420(8)` over named depth constants.
    #[allow(non_upper_case_globals)]
    pub const Yuv420p8: Self =
        Self::planar_yuv(ChromaSampling::Cs420, SampleBitDepth::new_unchecked(8));
    #[allow(non_upper_case_globals)]
    pub const Yuv420p10: Self =
        Self::planar_yuv(ChromaSampling::Cs420, SampleBitDepth::new_unchecked(10));
    #[allow(non_upper_case_globals)]
    pub const Yuv420p12: Self =
        Self::planar_yuv(ChromaSampling::Cs420, SampleBitDepth::new_unchecked(12));
    #[allow(non_upper_case_globals)]
    pub const Yuv420p16: Self =
        Self::planar_yuv(ChromaSampling::Cs420, SampleBitDepth::new_unchecked(16));
    #[allow(non_upper_case_globals)]
    pub const Yuv422p8: Self =
        Self::planar_yuv(ChromaSampling::Cs422, SampleBitDepth::new_unchecked(8));
    #[allow(non_upper_case_globals)]
    pub const Yuv422p10: Self =
        Self::planar_yuv(ChromaSampling::Cs422, SampleBitDepth::new_unchecked(10));
    #[allow(non_upper_case_globals)]
    pub const Yuv422p12: Self =
        Self::planar_yuv(ChromaSampling::Cs422, SampleBitDepth::new_unchecked(12));
    #[allow(non_upper_case_globals)]
    pub const Yuv422p16: Self =
        Self::planar_yuv(ChromaSampling::Cs422, SampleBitDepth::new_unchecked(16));
    #[allow(non_upper_case_globals)]
    pub const Yuv444p8: Self =
        Self::planar_yuv(ChromaSampling::Cs444, SampleBitDepth::new_unchecked(8));
    #[allow(non_upper_case_globals)]
    pub const Yuv444p10: Self =
        Self::planar_yuv(ChromaSampling::Cs444, SampleBitDepth::new_unchecked(10));
    #[allow(non_upper_case_globals)]
    pub const Yuv444p12: Self =
        Self::planar_yuv(ChromaSampling::Cs444, SampleBitDepth::new_unchecked(12));
    #[allow(non_upper_case_globals)]
    pub const Yuv444p16: Self =
        Self::planar_yuv(ChromaSampling::Cs444, SampleBitDepth::new_unchecked(16));
    #[allow(non_upper_case_globals)]
    pub const Gray8: Self = Self::gray_with_depth(SampleBitDepth::new_unchecked(8));
    #[allow(non_upper_case_globals)]
    pub const Gray10: Self = Self::gray_with_depth(SampleBitDepth::new_unchecked(10));
    #[allow(non_upper_case_globals)]
    pub const Gray12: Self = Self::gray_with_depth(SampleBitDepth::new_unchecked(12));
    #[allow(non_upper_case_globals)]
    pub const Gray16: Self = Self::gray_with_depth(SampleBitDepth::new_unchecked(16));

    pub const fn planar_yuv(chroma_sampling: ChromaSampling, bit_depth: SampleBitDepth) -> Self {
        Self::PlanarYuv {
            chroma_sampling,
            bit_depth,
        }
    }

    pub const fn yuv(chroma_sampling: ChromaSampling, bit_depth: u8) -> Option<Self> {
        match SampleBitDepth::new(bit_depth) {
            Some(bit_depth) => Some(Self::planar_yuv(chroma_sampling, bit_depth)),
            None => None,
        }
    }

    pub const fn yuv420(bit_depth: u8) -> Option<Self> {
        Self::yuv(ChromaSampling::Cs420, bit_depth)
    }

    pub const fn yuv422(bit_depth: u8) -> Option<Self> {
        Self::yuv(ChromaSampling::Cs422, bit_depth)
    }

    pub const fn yuv444(bit_depth: u8) -> Option<Self> {
        Self::yuv(ChromaSampling::Cs444, bit_depth)
    }

    pub const fn gray(bit_depth: u8) -> Option<Self> {
        match SampleBitDepth::new(bit_depth) {
            Some(bit_depth) => Some(Self::Gray { bit_depth }),
            None => None,
        }
    }

    const fn gray_with_depth(bit_depth: SampleBitDepth) -> Self {
        Self::Gray { bit_depth }
    }

    pub fn name(self) -> String {
        match self {
            Self::PlanarYuv {
                chroma_sampling,
                bit_depth,
            } => {
                let sampling = match chroma_sampling {
                    ChromaSampling::Cs420 => "420",
                    ChromaSampling::Cs422 => "422",
                    ChromaSampling::Cs444 => "444",
                    ChromaSampling::Monochrome => unreachable!("YUV cannot be monochrome"),
                };
                if bit_depth.bits() == 8 {
                    format!("yuv{sampling}p8")
                } else {
                    format!("yuv{}p{}le", sampling, bit_depth.bits())
                }
            }
            Self::Gray { bit_depth } => {
                if bit_depth.bits() == 8 {
                    "gray8".to_string()
                } else {
                    format!("gray{}le", bit_depth.bits())
                }
            }
            Self::Rgb24 => "rgb24".to_string(),
        }
    }

    pub fn bit_depth(self) -> SampleBitDepth {
        match self {
            Self::PlanarYuv { bit_depth, .. } | Self::Gray { bit_depth } => bit_depth,
            Self::Rgb24 => SampleBitDepth::new_unchecked(8),
        }
    }

    pub fn bytes_per_sample(self) -> usize {
        self.bit_depth().bytes_per_sample()
    }

    pub fn is_yuv420(self) -> bool {
        self.chroma_sampling() == Some(ChromaSampling::Cs420)
    }

    pub fn is_yuv(self) -> bool {
        self.chroma_sampling()
            .is_some_and(|sampling| sampling != ChromaSampling::Monochrome)
    }

    pub fn chroma_sampling(self) -> Option<ChromaSampling> {
        match self {
            Self::PlanarYuv {
                chroma_sampling, ..
            } => Some(chroma_sampling),
            Self::Gray { .. } => Some(ChromaSampling::Monochrome),
            Self::Rgb24 => None,
        }
    }

    pub fn chroma_plane_samples(self, width: usize, height: usize) -> Option<usize> {
        self.chroma_sampling()?.chroma_plane_samples(width, height)
    }

    pub fn with_bit_depth(self, bit_depth: SampleBitDepth) -> Option<Self> {
        match self.chroma_sampling()? {
            ChromaSampling::Cs420 | ChromaSampling::Cs422 | ChromaSampling::Cs444 => {
                Some(Self::planar_yuv(self.chroma_sampling()?, bit_depth))
            }
            ChromaSampling::Monochrome => Some(Self::gray_with_depth(bit_depth)),
        }
    }

    pub fn frame_len(self, width: usize, height: usize) -> Option<usize> {
        let luma = width.checked_mul(height)?;
        let bytes_per_sample = self.bytes_per_sample();
        match self.chroma_sampling() {
            Some(ChromaSampling::Cs420 | ChromaSampling::Cs422 | ChromaSampling::Cs444) => {
                let chroma_plane = self.chroma_plane_samples(width, height)?;
                luma.checked_add(chroma_plane.checked_mul(2)?)?
                    .checked_mul(bytes_per_sample)
            }
            Some(ChromaSampling::Monochrome) => luma.checked_mul(bytes_per_sample),
            None => luma.checked_mul(3),
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

pub fn convert_planar_frame_bit_depth(
    input: &[u8],
    width: usize,
    height: usize,
    source_format: PixelFormat,
    target_format: PixelFormat,
) -> Result<Vec<u8>> {
    if source_format.chroma_sampling().is_none() || target_format.chroma_sampling().is_none() {
        return Err(MediaError::Message(
            "bit-depth conversion supports only planar YUV and gray formats".to_string(),
        ));
    }
    if source_format.chroma_sampling() != target_format.chroma_sampling() {
        return Err(MediaError::Message(format!(
            "bit-depth conversion cannot change chroma layout: {source_format} -> {target_format}"
        )));
    }

    source_format.validate_geometry(width, height)?;
    target_format.validate_geometry(width, height)?;
    let expected = source_format
        .frame_len(width, height)
        .ok_or(MediaError::LengthOverflow)?;
    if input.len() != expected {
        return Err(MediaError::BufferLength {
            expected,
            actual: input.len(),
        });
    }

    let source_bytes = source_format.bytes_per_sample();
    let target_bytes = target_format.bytes_per_sample();
    let sample_count = expected / source_bytes;
    let mut output = vec![0; sample_count * target_bytes];
    for sample_idx in 0..sample_count {
        let source_offset = sample_idx * source_bytes;
        let target_offset = sample_idx * target_bytes;
        let source_sample = read_planar_sample(input, source_offset, source_format.bit_depth());
        let target_sample = scale_sample_bit_depth(
            source_sample,
            source_format.bit_depth(),
            target_format.bit_depth(),
        );
        write_planar_sample(
            &mut output,
            target_offset,
            target_sample,
            target_format.bit_depth(),
        );
    }
    Ok(output)
}

pub fn scale_sample_bit_depth(
    sample: u16,
    source_depth: SampleBitDepth,
    target_depth: SampleBitDepth,
) -> u16 {
    let source_max = u32::from(source_depth.max_sample());
    let target_max = u32::from(target_depth.max_sample());
    let sample = u32::from(sample).min(source_max);
    if source_max == target_max {
        return sample as u16;
    }
    ((sample * target_max + (source_max / 2)) / source_max) as u16
}

fn read_planar_sample(input: &[u8], offset: usize, bit_depth: SampleBitDepth) -> u16 {
    if bit_depth.bits() <= 8 {
        input[offset] as u16
    } else {
        u16::from_le_bytes([input[offset], input[offset + 1]])
    }
}

fn write_planar_sample(output: &mut [u8], offset: usize, sample: u16, bit_depth: SampleBitDepth) {
    if bit_depth.bits() <= 8 {
        output[offset] = sample as u8;
    } else {
        let bytes = sample.to_le_bytes();
        output[offset] = bytes[0];
        output[offset + 1] = bytes[1];
    }
}

impl FromStr for PixelFormat {
    type Err = String;

    fn from_str(value: &str) -> std::result::Result<Self, Self::Err> {
        let value = value.trim().to_ascii_lowercase();
        match value.as_str() {
            "yuv420p" | "yuv420p8" | "i420" => Ok(Self::Yuv420p8),
            "yuv422p" | "yuv422p8" | "i422" => Ok(Self::Yuv422p8),
            "yuv444p" | "yuv444p8" | "i444" => Ok(Self::Yuv444p8),
            "gray8" | "y8" => Ok(Self::Gray8),
            "rgb24" => Ok(Self::Rgb24),
            other => {
                if let Some(format) = parse_planar_yuv_pixel_format(other)? {
                    return Ok(format);
                }
                if let Some(format) = parse_gray_pixel_format(other)? {
                    return Ok(format);
                }
                if let Some(format) = parse_hardware_yuv_alias(other)? {
                    return Ok(format);
                }
                Err(format!(
                    "unsupported format '{other}'; supported formats: yuv420p8..16le, yuv422p8..16le, yuv444p8..16le, gray8..16le, and rgb24"
                ))
            }
        }
    }
}

fn parse_planar_yuv_pixel_format(value: &str) -> std::result::Result<Option<PixelFormat>, String> {
    for (prefix, chroma_sampling) in [
        ("yuv420p", ChromaSampling::Cs420),
        ("yuv422p", ChromaSampling::Cs422),
        ("yuv444p", ChromaSampling::Cs444),
    ] {
        let Some(suffix) = value.strip_prefix(prefix) else {
            continue;
        };
        let bit_depth = parse_planar_bit_depth_suffix(value, suffix)?;
        return Ok(Some(PixelFormat::planar_yuv(chroma_sampling, bit_depth)));
    }
    Ok(None)
}

fn parse_gray_pixel_format(value: &str) -> std::result::Result<Option<PixelFormat>, String> {
    for prefix in ["gray", "y"] {
        let Some(suffix) = value.strip_prefix(prefix) else {
            continue;
        };
        let bit_depth = parse_planar_bit_depth_suffix(value, suffix)?;
        return Ok(Some(PixelFormat::gray_with_depth(bit_depth)));
    }
    Ok(None)
}

fn parse_hardware_yuv_alias(value: &str) -> std::result::Result<Option<PixelFormat>, String> {
    let Some(rest) = value.strip_prefix('i') else {
        return Ok(None);
    };
    if rest.len() != 3 {
        return Ok(None);
    }
    let (sampling_digit, depth_text) = rest.split_at(1);
    let chroma_sampling = match sampling_digit {
        "0" => ChromaSampling::Cs420,
        "2" => ChromaSampling::Cs422,
        "4" => ChromaSampling::Cs444,
        _ => return Ok(None),
    };
    let bit_depth = parse_bit_depth(value, depth_text)?;
    Ok(Some(PixelFormat::planar_yuv(chroma_sampling, bit_depth)))
}

fn parse_planar_bit_depth_suffix(
    value: &str,
    suffix: &str,
) -> std::result::Result<SampleBitDepth, String> {
    let suffix = suffix.strip_suffix("le").unwrap_or(suffix);
    if suffix.ends_with("be") {
        return Err(format!(
            "unsupported format '{value}'; big-endian raw samples are not supported yet"
        ));
    }
    if suffix.is_empty() {
        return Ok(SampleBitDepth::new_unchecked(8));
    }
    parse_bit_depth(value, suffix)
}

fn parse_bit_depth(value: &str, bits: &str) -> std::result::Result<SampleBitDepth, String> {
    let parsed = bits
        .parse::<u8>()
        .map_err(|_| format!("unsupported format '{value}'; bit depth must be 8..16"))?;
    SampleBitDepth::new(parsed)
        .ok_or_else(|| format!("unsupported format '{value}'; bit depth must be 8..16"))
}

impl fmt::Display for PixelFormat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.name())
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
        assert_eq!(PixelFormat::yuv420(9).unwrap().frame_len(16, 16), Some(768));
        assert_eq!(PixelFormat::Yuv420p10.frame_len(16, 16), Some(768));
        assert_eq!(
            PixelFormat::yuv444(15).unwrap().frame_len(16, 16),
            Some(1536)
        );
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

    #[test]
    fn parses_planar_input_depths_between_8_and_16_bits() {
        assert_eq!(
            "yuv420p9le".parse::<PixelFormat>(),
            Ok(PixelFormat::yuv420(9).unwrap())
        );
        assert_eq!(
            "yuv422p11le".parse::<PixelFormat>(),
            Ok(PixelFormat::yuv422(11).unwrap())
        );
        assert_eq!(
            "yuv444p14le".parse::<PixelFormat>(),
            Ok(PixelFormat::yuv444(14).unwrap())
        );
        assert_eq!(
            "gray15le".parse::<PixelFormat>(),
            Ok(PixelFormat::gray(15).unwrap())
        );
        assert_eq!(
            SampleBitDepth::from_bits(13),
            Some(SampleBitDepth::new(13).unwrap())
        );
    }

    #[test]
    fn maps_planar_formats_to_the_same_layout_at_another_depth() {
        assert_eq!(
            PixelFormat::yuv420(13)
                .unwrap()
                .with_bit_depth(SampleBitDepth::new(8).unwrap()),
            Some(PixelFormat::Yuv420p8)
        );
        assert_eq!(
            PixelFormat::Yuv444p8.with_bit_depth(SampleBitDepth::new(16).unwrap()),
            Some(PixelFormat::Yuv444p16)
        );
        assert_eq!(
            PixelFormat::Rgb24.with_bit_depth(SampleBitDepth::new(8).unwrap()),
            None
        );
    }

    #[test]
    fn converts_planar_frame_bit_depth_without_changing_layout() {
        let input = [
            0u16, 1023, 512, 256, // Y
            128, // U
            768, // V
        ]
        .into_iter()
        .flat_map(u16::to_le_bytes)
        .collect::<Vec<_>>();

        let output = convert_planar_frame_bit_depth(
            &input,
            2,
            2,
            PixelFormat::Yuv420p10,
            PixelFormat::Yuv420p8,
        )
        .unwrap();

        assert_eq!(output, vec![0, 255, 128, 64, 32, 191]);
    }

    #[test]
    fn rejects_bit_depth_conversion_that_changes_chroma_layout() {
        let input = vec![0; PixelFormat::Yuv420p10.frame_len(2, 2).unwrap()];
        let err = convert_planar_frame_bit_depth(
            &input,
            2,
            2,
            PixelFormat::Yuv420p10,
            PixelFormat::Yuv444p8,
        )
        .unwrap_err();
        assert!(err.to_string().contains("cannot change chroma layout"));
    }
}
