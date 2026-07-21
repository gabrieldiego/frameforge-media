use std::io::{ErrorKind, Read};

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
