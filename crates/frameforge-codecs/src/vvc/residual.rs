mod prediction;
mod quant;
#[cfg(test)]
mod recon;
mod syntax;
pub(super) mod transform;

use super::VvcSample;
use super::{VvcChromaIntraPredictionMode, VvcIntraPredictionMode};

#[cfg(test)]
mod tests;

#[cfg(test)]
pub(super) use transform::{
    inverse_transform_vvc_chroma_quantized_block_into,
    inverse_transform_vvc_luma_quantized_block_into, inverse_transform_vvc_luma_residual_levels,
    quantize_vvc_chroma, quantize_vvc_luma_residual_greedy, transform_vvc_tu, VVC_CHROMA_DC_BASE,
    VVC_LUMA_DC_BASE,
};
pub(super) use transform::{
    inverse_transform_vvc_chroma_quantized_block_into_with_qp,
    inverse_transform_vvc_luma_quantized_block_into_with_qp_and_mts,
    quantize_vvc_chroma_residual_greedy_with_qp, quantize_vvc_chroma_sample,
    quantize_vvc_luma_residual_greedy_with_qp_and_mts, reconstruct_vvc_chroma,
    VvcInverseTransformScratch, VVC_DEFAULT_LOSSY_CHROMA_QP, VVC_DEFAULT_LOSSY_LUMA_QP,
};

pub(super) use prediction::{
    fill_visible_chroma_node, fill_visible_luma_node,
    predict_vvc_chroma_cclm_block_into_with_availability,
    predict_vvc_chroma_intra_block_into_with_availability,
    predict_vvc_luma_intra_block_into_with_availability, VvcDcPredictionScratch,
    VvcPlaneAvailability,
};
pub use quant::quantize_vvc_color;
#[cfg(test)]
pub(super) use quant::quantize_vvc_frame_with_reconstruction;
#[cfg(test)]
pub(super) use quant::quantize_vvc_residual_ctu_into_frame_reconstruction;
pub(super) use quant::{
    quantize_vvc_frame, quantize_vvc_residual_ctu_into_frame_reconstruction_with_qp,
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
    pub(super) luma_tu_transform_skip: [bool; MAX_VVC_LUMA_TUS],
    pub(super) luma_tu_mrl_index: [u8; MAX_VVC_LUMA_TUS],
    pub(super) luma_tu_mts_index: [u8; MAX_VVC_LUMA_TUS],
    pub(super) luma_tu_count: usize,
    pub(super) chroma_tu_count: usize,
    pub(super) chroma_tu_intra_modes: [VvcChromaIntraPredictionMode; MAX_VVC_CHROMA_TUS],
    pub(super) cb_tu_dc_levels: [i16; MAX_VVC_CHROMA_TUS],
    pub(super) cr_tu_dc_levels: [i16; MAX_VVC_CHROMA_TUS],
    pub(super) cb_tu_ac_levels: [[i16; VVC_CHROMA_AC_COEFFS_PER_TU]; MAX_VVC_CHROMA_TUS],
    pub(super) cr_tu_ac_levels: [[i16; VVC_CHROMA_AC_COEFFS_PER_TU]; MAX_VVC_CHROMA_TUS],
    pub(super) cb_tu_has_ac: [bool; MAX_VVC_CHROMA_TUS],
    pub(super) cr_tu_has_ac: [bool; MAX_VVC_CHROMA_TUS],
    pub(super) cb_tu_transform_skip: [bool; MAX_VVC_CHROMA_TUS],
    pub(super) cr_tu_transform_skip: [bool; MAX_VVC_CHROMA_TUS],
    pub(super) cb_rem: u8,
    pub(super) cr_rem: u8,
    #[cfg(feature = "vvc-stats")]
    pub(super) intra_search_stats: VvcIntraSearchStats,
    #[cfg(feature = "vvc-stats")]
    pub(super) residual_energy_stats: VvcResidualEnergyStats,
}

#[cfg(feature = "vvc-stats")]
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(super) struct VvcIntraSearchStats {
    pub(super) luma_dc_candidates: usize,
    pub(super) luma_planar_candidates: usize,
    pub(super) luma_directional_coarse_candidates: usize,
    pub(super) luma_directional_refinement_candidates: usize,
    pub(super) chroma_derived_candidates: usize,
    pub(super) chroma_explicit_candidates: usize,
    pub(super) chroma_cclm_candidates: usize,
}

#[cfg(feature = "vvc-stats")]
impl VvcIntraSearchStats {
    pub(super) const fn luma_directional_candidates(self) -> usize {
        self.luma_directional_coarse_candidates + self.luma_directional_refinement_candidates
    }

    pub(super) const fn luma_candidates(self) -> usize {
        self.luma_dc_candidates + self.luma_planar_candidates + self.luma_directional_candidates()
    }

    pub(super) const fn chroma_candidates(self) -> usize {
        self.chroma_derived_candidates
            + self.chroma_explicit_candidates
            + self.chroma_cclm_candidates
    }

    pub(super) fn add_luma_dc(&mut self) {
        self.luma_dc_candidates += 1;
    }

    pub(super) fn add_luma_planar(&mut self) {
        self.luma_planar_candidates += 1;
    }

    pub(super) fn add_luma_directional_coarse(&mut self) {
        self.luma_directional_coarse_candidates += 1;
    }

    pub(super) fn add_luma_directional_refinement(&mut self) {
        self.luma_directional_refinement_candidates += 1;
    }

    pub(super) fn add_chroma_derived(&mut self) {
        self.chroma_derived_candidates += 1;
    }

    pub(super) fn add_chroma_explicit(&mut self) {
        self.chroma_explicit_candidates += 1;
    }

    pub(super) fn add_chroma_cclm(&mut self) {
        self.chroma_cclm_candidates += 1;
    }
}

#[cfg(feature = "vvc-stats")]
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(super) struct VvcResidualEnergyStats {
    pub(super) luma_total_sse: u64,
    pub(super) luma_coded_first4x4_sse: u64,
    pub(super) luma_uncoded_tail_sse: u64,
    pub(super) chroma_total_sse: u64,
    pub(super) chroma_coded_first4x4_sse: u64,
    pub(super) chroma_uncoded_tail_sse: u64,
}

#[cfg(feature = "vvc-stats")]
impl VvcResidualEnergyStats {
    pub(super) fn add_luma_residuals(&mut self, residuals: &[i16], width: usize, height: usize) {
        let split = residual_energy_split(residuals, width, height);
        self.luma_total_sse = self.luma_total_sse.saturating_add(split.total_sse);
        self.luma_coded_first4x4_sse = self
            .luma_coded_first4x4_sse
            .saturating_add(split.coded_first4x4_sse);
        self.luma_uncoded_tail_sse = self
            .luma_uncoded_tail_sse
            .saturating_add(split.uncoded_tail_sse);
    }

    pub(super) fn add_chroma_residuals(&mut self, residuals: &[i16], width: usize, height: usize) {
        let split = residual_energy_split(residuals, width, height);
        self.chroma_total_sse = self.chroma_total_sse.saturating_add(split.total_sse);
        self.chroma_coded_first4x4_sse = self
            .chroma_coded_first4x4_sse
            .saturating_add(split.coded_first4x4_sse);
        self.chroma_uncoded_tail_sse = self
            .chroma_uncoded_tail_sse
            .saturating_add(split.uncoded_tail_sse);
    }
}

#[cfg(feature = "vvc-stats")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct VvcResidualEnergySplit {
    total_sse: u64,
    coded_first4x4_sse: u64,
    uncoded_tail_sse: u64,
}

#[cfg(feature = "vvc-stats")]
fn residual_energy_split(residuals: &[i16], width: usize, height: usize) -> VvcResidualEnergySplit {
    debug_assert_eq!(residuals.len(), width * height);
    let mut split = VvcResidualEnergySplit {
        total_sse: 0,
        coded_first4x4_sse: 0,
        uncoded_tail_sse: 0,
    };
    for y in 0..height {
        for x in 0..width {
            let residual = i64::from(residuals[y * width + x]);
            let sse = (residual * residual) as u64;
            split.total_sse = split.total_sse.saturating_add(sse);
            if x < 4 && y < 4 {
                split.coded_first4x4_sse = split.coded_first4x4_sse.saturating_add(sse);
            } else {
                split.uncoded_tail_sse = split.uncoded_tail_sse.saturating_add(sse);
            }
        }
    }
    split
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
pub(super) struct VvcQuantizedTransformBlock<const AC_COEFFS: usize> {
    pub(super) reconstructed_dc_coeff: i16,
    pub(super) reconstructed_ac_coeffs: [i16; AC_COEFFS],
    pub(super) has_ac: bool,
    pub(super) abs_remainder: u8,
}

pub(super) type VvcQuantizedLumaTransformBlock =
    VvcQuantizedTransformBlock<VVC_LUMA_AC_COEFFS_PER_TU>;
pub(super) type VvcQuantizedChromaTransformBlock =
    VvcQuantizedTransformBlock<VVC_CHROMA_AC_COEFFS_PER_TU>;

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

pub(super) const VVC_LUMA_AC_COEFFS_PER_TU: usize = 63;
pub(super) const VVC_CHROMA_AC_COEFFS_PER_TU: usize = 15;
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
