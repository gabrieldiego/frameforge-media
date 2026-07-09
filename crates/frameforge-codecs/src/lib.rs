//! Experimental codec models imported from the FrameForge hardware workspace.
//!
//! The modules in this crate are software-only codec models. They share public
//! frame/pixel primitives with `frameforge-core`, but intentionally keep AV2
//! and VVC internals separate while the APIs settle.

#[cfg(feature = "av2")]
pub mod av2;
pub mod bitstream;
#[cfg(any(feature = "av2", feature = "vvc"))]
mod picture;
pub mod trace;
#[cfg(feature = "vvc")]
pub mod vvc;

pub use frameforge_core::{ChromaSampling, PixelFormat, SampleBitDepth};
