pub(crate) use frameforge_core::{ChromaSampling, PixelFormat, SampleBitDepth};

pub(crate) struct Picture;

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
}
