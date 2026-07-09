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
pub use frame::{Frame, FrameInfo, PixelFormat};
pub use packet::{Packet, StreamId, Timestamp};
pub use pipeline::{Decoder, Encoder, Filter, Sink, Source};

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
