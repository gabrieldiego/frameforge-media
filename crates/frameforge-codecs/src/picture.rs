pub(crate) use frameforge_core::{ChromaSampling, PixelFormat, SampleBitDepth};

pub(crate) struct Picture;

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
