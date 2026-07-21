use std::io::{ErrorKind, Read};

pub(crate) use frameforge_core::{ChromaSampling, PixelFormat, SampleBitDepth};

pub(crate) struct Picture;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PlanarYuvGeometry {
    bytes_per_sample: usize,
    luma_samples: usize,
    chroma_width: usize,
    chroma_height: usize,
    chroma_samples: usize,
    frame_len: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FrameLimit {
    UntilEof,
    Exact(usize),
}

impl FrameLimit {
    pub(crate) fn from_frame_count(frames: usize) -> Self {
        if frames == 0 {
            Self::UntilEof
        } else {
            Self::Exact(frames)
        }
    }

    pub(crate) fn should_read(self, frame_index: usize) -> bool {
        match self {
            Self::UntilEof => true,
            Self::Exact(frames) => frame_index < frames,
        }
    }

    pub(crate) fn metric_count(self) -> usize {
        match self {
            Self::UntilEof => 0,
            Self::Exact(frames) => frames,
        }
    }
}

impl Picture {
    pub(crate) fn expected_len(width: usize, height: usize, format: PixelFormat) -> usize {
        Self::checked_len(width, height, format).expect("picture dimensions overflow usize")
    }

    pub(crate) fn checked_len(width: usize, height: usize, format: PixelFormat) -> Option<usize> {
        format.frame_len(width, height)
    }

    pub(crate) fn validate_shape(
        width: usize,
        height: usize,
        format: PixelFormat,
    ) -> Result<(), String> {
        format
            .validate_geometry(width, height)
            .map_err(|err| err.to_string())
    }

    pub(crate) fn validate_format_shape<T>(
        width: usize,
        height: usize,
        format: PixelFormat,
        validate_format: impl FnOnce(PixelFormat) -> Result<T, String>,
    ) -> Result<T, String> {
        let validated = validate_format(format)?;
        Self::validate_shape(width, height, format)?;
        Ok(validated)
    }
}

impl PlanarYuvGeometry {
    pub(crate) fn new(
        width: usize,
        height: usize,
        chroma_sampling: ChromaSampling,
        bit_depth: SampleBitDepth,
    ) -> Result<Self, String> {
        if chroma_sampling == ChromaSampling::Monochrome {
            return Err("planar YUV geometry expects chroma planes".to_string());
        }
        let format = PixelFormat::planar_yuv(chroma_sampling, bit_depth);
        Picture::validate_shape(width, height, format)?;
        Ok(Self::for_validated_shape(
            width,
            height,
            chroma_sampling,
            bit_depth,
        ))
    }

    pub(crate) fn for_validated_shape(
        width: usize,
        height: usize,
        chroma_sampling: ChromaSampling,
        bit_depth: SampleBitDepth,
    ) -> Self {
        debug_assert_ne!(chroma_sampling, ChromaSampling::Monochrome);
        debug_assert!(
            Picture::validate_shape(
                width,
                height,
                PixelFormat::planar_yuv(chroma_sampling, bit_depth)
            )
            .is_ok(),
            "planar YUV geometry expects validated dimensions"
        );
        Self::from_parts(width, height, chroma_sampling, bit_depth)
    }

    fn from_parts(
        width: usize,
        height: usize,
        chroma_sampling: ChromaSampling,
        bit_depth: SampleBitDepth,
    ) -> Self {
        let format = PixelFormat::planar_yuv(chroma_sampling, bit_depth);
        let luma_samples = width
            .checked_mul(height)
            .expect("validated planar YUV luma sample count fits usize");
        let sub_x = chroma_subsample_x(chroma_sampling);
        let sub_y = chroma_subsample_y(chroma_sampling);
        let chroma_width = width / sub_x;
        let chroma_height = height / sub_y;
        let chroma_samples = chroma_width
            .checked_mul(chroma_height)
            .expect("validated planar YUV chroma sample count fits usize");
        let frame_len = Picture::expected_len(width, height, format);
        Self {
            bytes_per_sample: bit_depth.bytes_per_sample(),
            luma_samples,
            chroma_width,
            chroma_height,
            chroma_samples,
            frame_len,
        }
    }

    pub(crate) fn bytes_per_sample(self) -> usize {
        self.bytes_per_sample
    }

    pub(crate) fn luma_samples(self) -> usize {
        self.luma_samples
    }

    pub(crate) fn chroma_width(self) -> usize {
        self.chroma_width
    }

    pub(crate) fn chroma_height(self) -> usize {
        self.chroma_height
    }

    pub(crate) fn chroma_samples(self) -> usize {
        self.chroma_samples
    }

    pub(crate) fn frame_len(self) -> usize {
        self.frame_len
    }
}

pub(crate) fn chroma_subsample_x(chroma_sampling: ChromaSampling) -> usize {
    chroma_sampling.subsample_x()
}

pub(crate) fn chroma_subsample_y(chroma_sampling: ChromaSampling) -> usize {
    chroma_sampling.subsample_y()
}

pub(crate) fn read_input_frame<R: Read + ?Sized>(
    input: &mut R,
    frame: &mut [u8],
    frame_index: usize,
    frame_limit: FrameLimit,
    context: &str,
) -> Result<bool, String> {
    if let FrameLimit::Exact(requested_frames) = frame_limit {
        input.read_exact(frame).map_err(|err| {
            format!(
                "failed to read {context} frame {} of {}: {err}",
                frame_index + 1,
                requested_frames
            )
        })?;
        return Ok(true);
    }

    let mut filled = 0usize;
    while filled < frame.len() {
        match input.read(&mut frame[filled..]) {
            Ok(0) if filled == 0 => return Ok(false),
            Ok(0) => {
                return Err(format!(
                    "failed to read complete {context} frame {}: EOF after {} of {} byte(s)",
                    frame_index + 1,
                    filled,
                    frame.len()
                ));
            }
            Ok(read) => filled += read,
            Err(err) if err.kind() == ErrorKind::Interrupted => {}
            Err(err) => {
                return Err(format!(
                    "failed to read {context} frame {}: {err}",
                    frame_index + 1
                ));
            }
        }
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn planar_yuv_geometry_derives_422_lengths() {
        let bit_depth = SampleBitDepth::new(10).unwrap();
        let geometry = PlanarYuvGeometry::new(16, 8, ChromaSampling::Cs422, bit_depth).unwrap();

        assert_eq!(geometry.bytes_per_sample(), 2);
        assert_eq!(geometry.luma_samples(), 128);
        assert_eq!(geometry.chroma_width(), 8);
        assert_eq!(geometry.chroma_height(), 8);
        assert_eq!(geometry.chroma_samples(), 64);
        assert_eq!(geometry.frame_len(), 512);
    }

    #[test]
    fn planar_yuv_geometry_rejects_incompatible_sampling_shape() {
        let bit_depth = SampleBitDepth::new(8).unwrap();

        assert!(
            PlanarYuvGeometry::new(15, 8, ChromaSampling::Cs422, bit_depth).is_err(),
            "4:2:2 chroma requires even luma width"
        );
        assert!(
            PlanarYuvGeometry::new(16, 8, ChromaSampling::Monochrome, bit_depth).is_err(),
            "planar YUV geometry requires chroma planes"
        );
    }
}
