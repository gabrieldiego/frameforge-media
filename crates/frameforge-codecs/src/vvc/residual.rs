mod prediction;
mod quant;
#[cfg(test)]
mod recon;
mod syntax;
pub(super) mod transform;

use super::VvcIntraPredictionMode;
use super::VvcSample;

#[cfg(test)]
mod tests;

pub(super) use transform::{
    inverse_transform_vvc_chroma_quantized_block_into,
    inverse_transform_vvc_luma_quantized_block_into, quantize_vvc_chroma_residual_greedy,
    quantize_vvc_chroma_sample, quantize_vvc_luma_residual_greedy, reconstruct_vvc_chroma,
    VvcInverseTransformScratch,
};
#[cfg(test)]
pub(super) use transform::{
    inverse_transform_vvc_luma_residual_levels, quantize_vvc_chroma, transform_vvc_tu,
    VVC_CHROMA_DC_BASE, VVC_LUMA_DC_BASE,
};

pub(super) use prediction::{
    fill_visible_chroma_node, fill_visible_luma_node, predict_vvc_chroma_dc_block_into,
    predict_vvc_luma_intra_block_into, VvcDcPredictionScratch,
};
pub use quant::quantize_vvc_color;
pub(super) use quant::{
    quantize_vvc_frame, quantize_vvc_frame_with_reconstruction,
    quantize_vvc_residual_ctu_into_frame_reconstruction,
};
#[cfg(test)]
pub(super) use recon::reconstruct_vvc_residual_frame;
pub(super) use syntax::{
    VvcResidualCabacEncoder, VvcResidualCabacOptions, VvcResidualCabacSymbolStream,
};
#[cfg(test)]
pub(super) use syntax::{
    VvcResidualCabacSymbol, VvcResidualCtxConfig, VvcResidualLocalStats, VvcResidualPass1State,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VvcQuantizedColor {
    pub y: u8,
    pub u: u8,
    pub v: u8,
    pub(super) luma_tu_intra_modes: [VvcIntraPredictionMode; MAX_VVC_LUMA_TUS],
    pub(super) luma_tu_remainders: [u8; MAX_VVC_LUMA_TUS],
    pub(super) luma_tu_negative: [bool; MAX_VVC_LUMA_TUS],
    pub(super) luma_tu_dc_levels: [i16; MAX_VVC_LUMA_TUS],
    pub(super) luma_tu_ac_levels: [[i16; VVC_LUMA_AC_COEFFS_PER_TU]; MAX_VVC_LUMA_TUS],
    pub(super) luma_tu_has_ac: [bool; MAX_VVC_LUMA_TUS],
    pub(super) luma_tu_count: usize,
    pub(super) chroma_tu_count: usize,
    pub(super) cb_tu_dc_levels: [i16; MAX_VVC_CHROMA_TUS],
    pub(super) cr_tu_dc_levels: [i16; MAX_VVC_CHROMA_TUS],
    pub(super) cb_tu_ac_levels: [[i16; VVC_CHROMA_AC_COEFFS_PER_TU]; MAX_VVC_CHROMA_TUS],
    pub(super) cr_tu_ac_levels: [[i16; VVC_CHROMA_AC_COEFFS_PER_TU]; MAX_VVC_CHROMA_TUS],
    pub(super) cb_tu_has_ac: [bool; MAX_VVC_CHROMA_TUS],
    pub(super) cr_tu_has_ac: [bool; MAX_VVC_CHROMA_TUS],
    pub(super) cb_rem: u8,
    pub(super) cr_rem: u8,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct VvcQuantizedResidualFrame {
    pub(super) quantized: VvcQuantizedColor,
    pub(super) reconstruction_yuv: Vec<VvcSample>,
}

#[cfg(test)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum VvcTransformComponent {
    Luma,
    ChromaCb,
    ChromaCr,
}

#[cfg(test)]
impl VvcTransformComponent {
    pub(super) const fn dc_base(self) -> i16 {
        match self {
            Self::Luma => VVC_LUMA_DC_BASE,
            Self::ChromaCb | Self::ChromaCr => VVC_CHROMA_DC_BASE,
        }
    }
}

#[cfg(test)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct VvcTuTransformBlock {
    pub(super) component: VvcTransformComponent,
    pub(super) width: u16,
    pub(super) height: u16,
    pub(super) dc_coeff: i16,
    pub(super) ac_coeffs: Vec<i16>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct VvcQuantizedTransformBlock {
    pub(super) reconstructed_dc_coeff: i16,
    pub(super) reconstructed_ac_coeffs: [i16; 15],
    pub(super) has_ac: bool,
    pub(super) abs_remainder: u8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum VvcResidualComponent {
    Luma,
    ChromaCb,
    ChromaCr,
}

impl VvcResidualComponent {
    pub(super) const fn transform_skip_ctx_inc(self) -> u8 {
        match self {
            Self::Luma => 0,
            Self::ChromaCb | Self::ChromaCr => 1,
        }
    }
}

pub(super) const VVC_LUMA_AC_COEFFS_PER_TU: usize = 15;
pub(super) const VVC_CHROMA_AC_COEFFS_PER_TU: usize = VVC_LUMA_AC_COEFFS_PER_TU;
pub(super) const VVC_CHROMA_AC_POSITIONS_4X4: [(usize, usize); VVC_CHROMA_AC_COEFFS_PER_TU] = [
    (1, 0),
    (2, 0),
    (3, 0),
    (0, 1),
    (1, 1),
    (2, 1),
    (3, 1),
    (0, 2),
    (1, 2),
    (2, 2),
    (3, 2),
    (0, 3),
    (1, 3),
    (2, 3),
    (3, 3),
];
pub(super) const MAX_VVC_LUMA_TUS: usize = 16 * 16;
pub(super) const MAX_VVC_CHROMA_TUS: usize = MAX_VVC_LUMA_TUS;
