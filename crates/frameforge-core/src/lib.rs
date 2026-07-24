//! Core media primitives for FrameForge.
//!
//! This crate is intentionally small at project bootstrap. It holds the stable
//! concepts that codec crates and tools can share without forcing AV2, VVC, or
//! future codecs into one internal design.

pub mod error;
pub mod frame;
pub mod packet;
pub mod pipeline;

pub use error::{MediaError, Result};
pub use frame::{
    convert_frame_format, convert_planar_frame_bit_depth, planar_sample_sse, read_planar_sample,
    scale_sample_bit_depth, write_planar_sample, ChromaSampling, Frame, FrameInfo, PixelFormat,
    SampleBitDepth,
};
pub use packet::{Packet, StreamId, Timestamp};
pub use pipeline::{
    run_frame_encode_pipeline, run_frame_filter_pipeline, Decoder, EncodePipelineStats, Encoder,
    Filter, FilterPipelineStats, IdentityFilter, Sink, Source,
};

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
