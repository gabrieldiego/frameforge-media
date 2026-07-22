//! First-target VVC/H.266 syntax experiments.
//!
//! This module contains a clean-room VVC path for small all-intra validation
//! streams across parameterized geometries. It is still intentionally
//! incomplete: CABAC, CTU syntax generation, transform/quant, prediction, and
//! reconstruction semantics need to keep converging toward real implementations
//! before FrameForge can encode arbitrary input pictures.

use std::io::{Cursor, Read, Write};
#[cfg(feature = "vvc-stats")]
use std::time::Instant;

use crate::instrumentation::CountingWriter;
#[cfg(feature = "vvc-stats")]
use crate::instrumentation::JsonlInstrumentationSink;
use crate::picture::{
    chroma_subsample_x as planar_chroma_subsample_x,
    chroma_subsample_y as planar_chroma_subsample_y, read_input_frame, ChromaSampling, FrameLimit,
    Picture, PixelFormat, PlanarYuvGeometry, SampleBitDepth,
};

mod cabac;
mod header;
mod ibc;
mod nal;
mod palette;
mod residual;
mod syntax;
use cabac::{
    encode_ctu_partition_body, encode_ctu_partition_body_with_contexts, initial_vvc_cabac_contexts,
    vvc_chroma_transform_nodes, vvc_luma_transform_nodes, VvcCabacContext, VvcCabacContexts,
    VvcCabacDumpContextEvent, VvcCabacDumpSymbol, VvcCabacEncoder, VvcCodingTreeNode,
    VvcCtuCabacOp, VvcCtuPartitionParams, VvcCtuPartitionShape, VvcLastSigCoeffPrefixCtxInput,
};
#[cfg(test)]
use cabac::{VvcCtuCabacGenerator, VvcQtSplitCtxInput, VvcSplitCtxInput, VvcTreeType};
use header::{
    vvc_ctu_slice_unit_with_luma_max_leaf_size, vvc_frame_slice_unit, vvc_picture_ctu_cols,
    vvc_picture_ctu_count, vvc_picture_ctu_rows, vvc_picture_header_unit,
    vvc_poc_lsb_for_frame_idx, vvc_pps_unit, vvc_pps_unit_with_partitioning,
    vvc_slice_address_bits, vvc_slice_unit, vvc_sps_unit, VvcPictureKind, VvcPicturePartitioning,
};
#[cfg(test)]
use header::{
    vvc_pps_rbsp, vvc_slice_payload, vvc_slice_rbsp, vvc_sps_payload, vvc_sps_rbsp,
    write_vvc_coding_tree_entropy,
};
pub use nal::{
    nal_unit_header_bytes, parse_annex_b_nal_units, write_annex_b, write_nal_unit_header,
    VvcNalHeader, VvcNalInfo, VvcNalUnit, VvcNalUnitType,
};
pub use palette::vvc_palette_444_cabac_dump_json;
#[cfg(test)]
use palette::{
    vvc_palette_444_binarized_syntax_bits, vvc_palette_444_cabac_context_bins,
    vvc_palette_444_context_audit_rows, vvc_palette_444_cu_syntax,
    vvc_palette_444_cu_syntax_with_config, vvc_palette_444_decode_reconstruction,
    vvc_palette_444_new_entry_token_bit_counts, vvc_palette_444_reconstruction_yuv,
    vvc_palette_444_reconstruction_yuv_with_config, vvc_palette_444_single_entry_syntax,
    vvc_palette_444_syntax_tokens, vvc_palette_run_copy_context_id_for_audit,
    vvc_palette_transform_skip_coded_coeff_for_test,
    vvc_palette_transform_skip_coded_coeff_with_config_for_test, VvcPalettePredictorMode,
    VvcPaletteTreeType,
};
pub use residual::quantize_vvc_color;
#[cfg(test)]
use residual::VVC_LUMA_DC_BASE;
use residual::{
    fill_visible_chroma_node, fill_visible_luma_node,
    inverse_transform_vvc_chroma_quantized_block_into,
    inverse_transform_vvc_luma_quantized_block_into, lossless_chroma_ac_levels_and_flag,
    lossless_luma_ac_levels_and_flag, predict_vvc_chroma_dc_block_into,
    predict_vvc_luma_dc_block_into, quantize_vvc_chroma_residual_greedy,
    quantize_vvc_chroma_sample, quantize_vvc_frame, quantize_vvc_frame_with_reconstruction,
    quantize_vvc_luma_residual_greedy, reconstruct_vvc_chroma, residual_chroma_tu_at_into,
    residual_luma_tu_at_into, VvcDcPredictionScratch, VvcInverseTransformScratch,
    VvcQuantizedColor, VvcResidualCabacOptions, VvcResidualComponent, MAX_VVC_CHROMA_TUS,
    MAX_VVC_LUMA_TUS, VVC_CHROMA_AC_COEFFS_PER_TU, VVC_LUMA_AC_COEFFS_PER_TU,
};
#[cfg(test)]
use residual::{VvcResidualCabacEncoder, VvcResidualCtxConfig, VvcResidualPass1State};
pub use syntax::{VvcSyntaxCode, VvcSyntaxField, VvcSyntaxRbsp, VvcSyntaxWriter};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VvcProfileTarget {
    MinimalVvcAllIntra,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VvcSubset {
    pub all_intra: bool,
    pub single_picture: bool,
    pub one_tile: bool,
    pub one_slice: bool,
}

impl Default for VvcSubset {
    fn default() -> Self {
        Self {
            all_intra: true,
            single_picture: true,
            one_tile: true,
            one_slice: true,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VvcEncodeParams {
    pub frames: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct VvcEncodeRequest {
    params: VvcEncodeParams,
    geometry: VvcVideoGeometry,
    limits: VvcVideoLimits,
    format: PixelFormat,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct VvcValidatedEncodeRequest {
    frame_limit: FrameLimit,
    geometry: VvcVideoGeometry,
    format: VvcPictureFormat,
}

impl VvcEncodeRequest {
    fn validate(self) -> Result<VvcValidatedEncodeRequest, String> {
        self.geometry.validate_against(self.limits)?;
        let frame_limit = FrameLimit::from_frame_count(self.params.frames);
        let format = Picture::validate_format_shape(
            self.geometry.width,
            self.geometry.height,
            self.format,
            validate_vvc_input_format,
        )?;
        Ok(VvcValidatedEncodeRequest {
            frame_limit,
            geometry: self.geometry,
            format,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct VvcEncodeOptions {
    pub lossless: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VvcEncodeArtifacts {
    pub bitstream: Vec<u8>,
    pub reconstruction: Vec<u8>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VvcEncodeProgress {
    pub frame_idx: usize,
    pub frame_count: usize,
}

pub struct VvcEncodeFrameMetrics<'a> {
    pub frame_idx: usize,
    pub frame_count: usize,
    pub bitstream_bytes: usize,
    pub source: &'a [u8],
    pub reconstruction: &'a [u8],
}

#[cfg(feature = "vvc-stats")]
struct VvcStatsSink {
    sink: Option<JsonlInstrumentationSink>,
}

#[cfg(feature = "vvc-stats")]
impl VvcStatsSink {
    fn from_env() -> Result<Self, String> {
        Ok(Self {
            sink: JsonlInstrumentationSink::append_from_env("FRAMEFORGE_VVC_STATS")
                .map_err(|err| err.to_string())?,
        })
    }

    fn write_frame(&mut self, frame: &VvcFrameStats) -> Result<(), String> {
        let Some(sink) = self.sink.as_mut() else {
            return Ok(());
        };
        sink.write_json_line(&frame.to_json_line())
            .map_err(|err| err.to_string())?;
        sink.flush().map_err(|err| err.to_string())
    }
}

#[cfg(feature = "vvc-stats")]
struct VvcFrameStats {
    frame_idx: usize,
    width: usize,
    height: usize,
    chroma_sampling: ChromaSampling,
    bit_depth: SampleBitDepth,
    lossless: bool,
    ctu_count: usize,
    bitstream_bytes: usize,
    stages: Vec<VvcStageStats>,
    counters: Vec<VvcCounterStats>,
}

#[cfg(feature = "vvc-stats")]
impl VvcFrameStats {
    fn new(
        frame_idx: usize,
        geometry: VvcVideoGeometry,
        format: VvcPictureFormat,
        lossless: bool,
    ) -> Self {
        Self {
            frame_idx,
            width: geometry.width,
            height: geometry.height,
            chroma_sampling: format.chroma_sampling,
            bit_depth: format.bit_depth,
            lossless,
            ctu_count: vvc_picture_ctu_count(geometry),
            bitstream_bytes: 0,
            stages: Vec::new(),
            counters: Vec::new(),
        }
    }

    fn add_elapsed(&mut self, name: &'static str, start: Instant) {
        self.add_stage(name, start.elapsed().as_nanos() as u64, 1);
    }

    fn add_stage(&mut self, name: &'static str, nanos: u64, count: u64) {
        if let Some(stage) = self.stages.iter_mut().find(|stage| stage.name == name) {
            stage.nanos += nanos;
            stage.count += count;
        } else {
            self.stages.push(VvcStageStats { name, nanos, count });
        }
    }

    fn set_bitstream_bytes(&mut self, bitstream_bytes: usize) {
        self.bitstream_bytes = bitstream_bytes;
    }

    fn add_counter(&mut self, name: &'static str, value: u64) {
        if let Some(counter) = self
            .counters
            .iter_mut()
            .find(|counter| counter.name == name)
        {
            counter.value += value;
        } else {
            self.counters.push(VvcCounterStats { name, value });
        }
    }

    fn to_json_line(&self) -> String {
        let mut json = format!(
            "{{\"kind\":\"frameforge.vvc.stats.v1\",\"frame_index\":{},\"width\":{},\"height\":{},\"chroma_sampling\":\"{:?}\",\"bit_depth\":{},\"lossless\":{},\"ctu_count\":{},\"bitstream_bytes\":{},\"stages\":[",
            self.frame_idx,
            self.width,
            self.height,
            self.chroma_sampling,
            self.bit_depth.bits(),
            self.lossless,
            self.ctu_count,
            self.bitstream_bytes
        );
        for (index, stage) in self.stages.iter().enumerate() {
            if index > 0 {
                json.push(',');
            }
            json.push_str(&format!(
                "{{\"name\":\"{}\",\"ns\":{},\"count\":{}}}",
                stage.name, stage.nanos, stage.count
            ));
        }
        json.push_str("],\"counters\":[");
        for (index, counter) in self.counters.iter().enumerate() {
            if index > 0 {
                json.push(',');
            }
            json.push_str(&format!(
                "{{\"name\":\"{}\",\"value\":{}}}",
                counter.name, counter.value
            ));
        }
        json.push_str("]}");
        json
    }
}

#[cfg(feature = "vvc-stats")]
struct VvcStageStats {
    name: &'static str,
    nanos: u64,
    count: u64,
}

#[cfg(feature = "vvc-stats")]
struct VvcCounterStats {
    name: &'static str,
    value: u64,
}

/// Luma coded-picture dimensions are rounded to this granularity before SPS/PPS
/// signaling and crop-offset derivation.
///
/// This is a deliberately narrow property of the current VVC validation path,
/// not a claim about all legal VVC profiles or future FrameForge codec paths.
pub const VVC_CODED_DIMENSION_GRANULARITY: usize = 8;
const VVC_CTU_SIZE: usize = 64;
const VVC_CURRENT_MIN_LUMA_CB_SIZE: u16 = 4;
const VVC_CURRENT_MAX_LUMA_LEAF_SIZE: u16 = 8;
const VVC_LOSSLESS_LUMA_LEAF_SIZE: u16 = 4;
const VVC_CURRENT_MAX_LUMA_BT_SIZE: u16 = VVC_CURRENT_MIN_LUMA_QT_SIZE << 2;
const VVC_CURRENT_MAX_LUMA_TT_SIZE: u16 = VVC_CURRENT_MIN_LUMA_QT_SIZE << 2;
const VVC_CURRENT_MAX_LUMA_MTT_DEPTH: u8 = 5;
const VVC_CURRENT_ENCODER_CHROMA_420_TB_SIZE: u16 = 4;
const VVC_CURRENT_MAX_CHROMA_420_BT_SIZE: u16 = VVC_CURRENT_MIN_CHROMA_420_QT_SIZE << 3;
const VVC_CURRENT_MAX_CHROMA_420_TT_SIZE: u16 = VVC_CURRENT_MIN_CHROMA_420_QT_SIZE << 2;
const VVC_CURRENT_MAX_CHROMA_420_MTT_DEPTH: u8 = 3;
const VVC_CURRENT_MIN_CHROMA_420_QT_SIZE: u16 = VVC_CURRENT_MIN_LUMA_QT_SIZE;
const VVC_CURRENT_MIN_LUMA_QT_SIZE: u16 = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VvcVideoGeometry {
    pub width: usize,
    pub height: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct VvcCodedGeometry {
    width: usize,
    height: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VvcVideoLimits {
    pub max_width: usize,
    pub max_height: usize,
}

impl VvcVideoLimits {
    pub const fn max_64x64() -> Self {
        Self {
            max_width: 64,
            max_height: 64,
        }
    }

    pub const fn unbounded() -> Self {
        Self {
            max_width: usize::MAX,
            max_height: usize::MAX,
        }
    }
}

impl VvcVideoGeometry {
    pub const fn validation_minimum() -> Self {
        Self {
            width: 4,
            height: 4,
        }
    }

    pub fn validate_against(self, limits: VvcVideoLimits) -> Result<(), String> {
        self.validate_shape()?;
        if self.width > limits.max_width || self.height > limits.max_height {
            return Err(format!(
                "VVC geometry supports at most {}x{} visible pictures at this entry point; got {}x{}",
                limits.max_width, limits.max_height, self.width, self.height
            ));
        }
        Ok(())
    }

    fn validate_shape(self) -> Result<(), String> {
        if self.width == 0 || self.height == 0 {
            return Err("VVC geometry expects non-zero width and height".to_string());
        }
        if !self.width.is_multiple_of(2) || !self.height.is_multiple_of(2) {
            return Err(format!(
                "VVC geometry currently requires even dimensions for the emitted 4:2:0 stream; got {}x{}",
                self.width, self.height
            ));
        }
        Ok(())
    }

    fn luma_samples(self) -> usize {
        self.width * self.height
    }

    fn coded_width(self) -> usize {
        self.coded().width
    }

    fn coded_height(self) -> usize {
        self.coded().height
    }

    fn coded(self) -> VvcCodedGeometry {
        VvcCodedGeometry {
            width: coded_canvas_dimension(self.width),
            height: coded_canvas_dimension(self.height),
        }
    }

    fn crop_right(self, chroma_sampling: ChromaSampling) -> u32 {
        ((self.coded_width() - self.width) / chroma_subsample_x(chroma_sampling)) as u32
    }

    fn crop_bottom(self, chroma_sampling: ChromaSampling) -> u32 {
        ((self.coded_height() - self.height) / chroma_subsample_y(chroma_sampling)) as u32
    }
}

pub(in crate::vvc) fn chroma_subsample_x(chroma_sampling: ChromaSampling) -> usize {
    planar_chroma_subsample_x(chroma_sampling)
}

pub(in crate::vvc) fn chroma_subsample_y(chroma_sampling: ChromaSampling) -> usize {
    planar_chroma_subsample_y(chroma_sampling)
}

fn coded_canvas_dimension(value: usize) -> usize {
    value.div_ceil(VVC_CODED_DIMENSION_GRANULARITY) * VVC_CODED_DIMENSION_GRANULARITY
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VvcSampledColor {
    pub y: VvcSample,
    pub u: VvcSample,
    pub v: VvcSample,
}

pub(in crate::vvc) type VvcSample = u16;
pub(in crate::vvc) const VVC_MIN_BIT_DEPTH: u8 = 8;
pub(in crate::vvc) const VVC_MAX_BIT_DEPTH: u8 = 12;
const VVC_PALETTE_DEFAULT_SLICE_QP: i32 = 4;

#[derive(Debug, Clone, PartialEq, Eq)]
struct VvcSampledFrame {
    geometry: VvcVideoGeometry,
    format: VvcPictureFormat,
    luma: Vec<VvcSample>,
    cb: Vec<VvcSample>,
    cr: Vec<VvcSample>,
    chroma_len: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct VvcCtuRegion {
    slice_address: usize,
    origin_x: usize,
    origin_y: usize,
    geometry: VvcVideoGeometry,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::vvc) struct VvcQuantizedCtu {
    slice_address: usize,
    geometry: VvcVideoGeometry,
    color: VvcQuantizedColor,
    luma_max_leaf_size: u16,
}

#[cfg(feature = "vvc-stats")]
fn add_vvc_quantized_ctu_counters(stats: &mut VvcFrameStats, quantized: &VvcQuantizedColor) {
    stats.add_counter("luma_tu_count", quantized.luma_tu_count as u64);
    stats.add_counter("chroma_tu_count", quantized.chroma_tu_count as u64);
    for idx in 0..quantized.luma_tu_count {
        let dc_nonzero = quantized.luma_tu_dc_levels[idx] != 0;
        let ac_nonzero = quantized.luma_tu_ac_levels[idx]
            .iter()
            .filter(|level| **level != 0)
            .count();
        debug_assert_eq!(quantized.luma_tu_has_ac[idx], ac_nonzero != 0);
        stats.add_counter("luma_dc_nonzero", u64::from(dc_nonzero));
        stats.add_counter("luma_ac_nonzero", ac_nonzero as u64);
        stats.add_counter(
            "luma_cbf",
            u64::from(dc_nonzero || quantized.luma_tu_has_ac[idx]),
        );
    }
    for idx in 0..quantized.chroma_tu_count {
        let cb_dc_nonzero = quantized.cb_tu_dc_levels[idx] != 0;
        let cr_dc_nonzero = quantized.cr_tu_dc_levels[idx] != 0;
        let cb_ac_nonzero = quantized.cb_tu_ac_levels[idx]
            .iter()
            .filter(|level| **level != 0)
            .count();
        let cr_ac_nonzero = quantized.cr_tu_ac_levels[idx]
            .iter()
            .filter(|level| **level != 0)
            .count();
        debug_assert_eq!(quantized.cb_tu_has_ac[idx], cb_ac_nonzero != 0);
        debug_assert_eq!(quantized.cr_tu_has_ac[idx], cr_ac_nonzero != 0);
        stats.add_counter("cb_dc_nonzero", u64::from(cb_dc_nonzero));
        stats.add_counter("cr_dc_nonzero", u64::from(cr_dc_nonzero));
        stats.add_counter("cb_ac_nonzero", cb_ac_nonzero as u64);
        stats.add_counter("cr_ac_nonzero", cr_ac_nonzero as u64);
        stats.add_counter(
            "cb_cbf",
            u64::from(cb_dc_nonzero || quantized.cb_tu_has_ac[idx]),
        );
        stats.add_counter(
            "cr_cbf",
            u64::from(cr_dc_nonzero || quantized.cr_tu_has_ac[idx]),
        );
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct VvcReconstructionFrame {
    geometry: VvcVideoGeometry,
    format: VvcPictureFormat,
    luma: Vec<VvcSample>,
    cb: Vec<VvcSample>,
    cr: Vec<VvcSample>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct VvcPictureFormat {
    chroma_sampling: ChromaSampling,
    bit_depth: SampleBitDepth,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct VvcCodingTreeConfig {
    chroma_sampling: ChromaSampling,
}

impl VvcCodingTreeConfig {
    const fn yuv(chroma_sampling: ChromaSampling) -> Self {
        Self { chroma_sampling }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct VvcVuiSignal {
    progressive_source: bool,
    interlaced_source: bool,
    non_packed: bool,
    non_projected: bool,
    colour_primaries: u8,
    transfer_characteristics: u8,
    matrix_coeffs: u8,
    full_range: bool,
}

impl VvcVuiSignal {
    const fn srgb_gbr_compatible() -> Self {
        Self {
            progressive_source: true,
            interlaced_source: false,
            non_packed: true,
            non_projected: true,
            colour_primaries: 1,
            transfer_characteristics: 13,
            // H.266/VTM forbid identity matrix coefficients for 4:4:4 VUI.
            // Keep the colour volume explicit while leaving the RGB matrix
            // unspecified until a compatible VVC RGB signalling path is added.
            matrix_coeffs: 2,
            full_range: true,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct VvcSyntaxToolFlags {
    ibc_enabled: bool,
    palette_enabled: bool,
    transform_skip_enabled: bool,
    bdpcm_enabled: bool,
    mts_enabled: bool,
    explicit_mts_intra_enabled: bool,
    lfnst_enabled: bool,
    joint_cbcr_enabled: bool,
    mrl_enabled: bool,
    cclm_enabled: bool,
    dependent_quantization_enabled: bool,
    sign_data_hiding_enabled: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct VvcSliceSyntaxConfig {
    coding_tree: VvcCodingTreeConfig,
    tools: VvcSyntaxToolFlags,
    ref_pic_resampling_enabled: bool,
    entry_point_offsets_present: bool,
    slice_qp: i32,
    vui_signal: Option<VvcVuiSignal>,
}

impl VvcSyntaxToolFlags {
    const fn residual_lossy(chroma_sampling: ChromaSampling) -> Self {
        Self {
            ibc_enabled: false,
            palette_enabled: false,
            transform_skip_enabled: false,
            bdpcm_enabled: false,
            mts_enabled: false,
            explicit_mts_intra_enabled: false,
            lfnst_enabled: false,
            joint_cbcr_enabled: false,
            mrl_enabled: true,
            cclm_enabled: true,
            dependent_quantization_enabled: false,
            sign_data_hiding_enabled: false,
        }
        .without_unsupported_chroma_tools(chroma_sampling)
    }

    const fn yuv420_residual() -> Self {
        Self::residual_lossy(ChromaSampling::Cs420)
    }

    const fn yuv420_lossless() -> Self {
        Self {
            transform_skip_enabled: true,
            ..Self::yuv420_residual()
        }
    }

    const fn residual_lossless(chroma_sampling: ChromaSampling) -> Self {
        let mut tools = Self::yuv420_lossless();
        if matches!(chroma_sampling, ChromaSampling::Cs422) {
            tools.cclm_enabled = false;
        }
        tools
    }

    const fn without_unsupported_chroma_tools(mut self, chroma_sampling: ChromaSampling) -> Self {
        if matches!(chroma_sampling, ChromaSampling::Cs422) {
            self.cclm_enabled = false;
        }
        self
    }

    const fn palette_444() -> Self {
        Self {
            ibc_enabled: true,
            palette_enabled: true,
            transform_skip_enabled: true,
            bdpcm_enabled: true,
            mts_enabled: false,
            explicit_mts_intra_enabled: false,
            lfnst_enabled: false,
            joint_cbcr_enabled: false,
            mrl_enabled: false,
            cclm_enabled: false,
            dependent_quantization_enabled: false,
            sign_data_hiding_enabled: false,
        }
    }

    const fn mts_enabled(self) -> bool {
        self.mts_enabled || self.explicit_mts_intra_enabled
    }
}

impl VvcSliceSyntaxConfig {
    const fn new(coding_tree: VvcCodingTreeConfig, tools: VvcSyntaxToolFlags) -> Self {
        Self {
            coding_tree,
            tools,
            ref_pic_resampling_enabled: true,
            entry_point_offsets_present: true,
            slice_qp: 32,
            vui_signal: None,
        }
    }

    const fn yuv420_residual() -> Self {
        Self::residual_lossy(ChromaSampling::Cs420)
    }

    const fn residual_lossy(chroma_sampling: ChromaSampling) -> Self {
        Self::new(
            VvcCodingTreeConfig::yuv(chroma_sampling),
            VvcSyntaxToolFlags::residual_lossy(chroma_sampling),
        )
    }

    fn residual_lossless(chroma_sampling: ChromaSampling, bit_depth: SampleBitDepth) -> Self {
        let mut config = Self::new(
            VvcCodingTreeConfig::yuv(chroma_sampling),
            VvcSyntaxToolFlags::residual_lossless(chroma_sampling),
        );
        config.slice_qp = vvc_lossless_slice_qp(bit_depth);
        config
    }

    const fn palette_444() -> Self {
        let mut config = Self::new(
            VvcCodingTreeConfig {
                chroma_sampling: ChromaSampling::Cs444,
            },
            VvcSyntaxToolFlags::palette_444(),
        );
        config.slice_qp = VVC_PALETTE_DEFAULT_SLICE_QP;
        config
    }

    const fn palette_444_lossless(bit_depth: SampleBitDepth) -> Self {
        let mut config = Self::palette_444();
        config.slice_qp = vvc_palette_lossless_slice_qp(bit_depth);
        config
    }

    const fn for_picture_format(format: VvcPictureFormat) -> Self {
        // Current encoding-mode policy: the only implemented palette path is
        // 4:4:4, so 4:4:4 pictures select palette syntax. Keep this decision
        // behind a single helper so later work can replace the heuristic with
        // CU-level decisions, content analysis, or explicit encoder controls.
        match format.chroma_sampling {
            ChromaSampling::Cs444 => Self::palette_444(),
            _ => Self::residual_lossy(format.chroma_sampling),
        }
    }

    const fn with_vui_signal(mut self, vui_signal: VvcVuiSignal) -> Self {
        self.vui_signal = Some(vui_signal);
        self
    }

    const fn residual_options(self) -> VvcResidualCabacOptions {
        VvcResidualCabacOptions {
            transform_skip_enabled: self.tools.transform_skip_enabled,
            explicit_mts_intra_enabled: self.tools.explicit_mts_intra_enabled,
            dependent_quantization_enabled: self.tools.dependent_quantization_enabled,
            sign_data_hiding_enabled: self.tools.sign_data_hiding_enabled,
            lfnst_enabled: self.tools.lfnst_enabled,
            sbt_enabled: false,
        }
    }
}

fn vvc_lossless_slice_qp(bit_depth: SampleBitDepth) -> i32 {
    -((i32::from(bit_depth.bits()) - 8) * 6)
}

const fn vvc_palette_lossless_slice_qp(bit_depth: SampleBitDepth) -> i32 {
    4 - ((bit_depth.bits() as i32 - 8) * 6)
}

impl VvcSampledFrame {
    fn solid(color: VvcSampledColor) -> Self {
        let geometry = VvcVideoGeometry {
            width: 8,
            height: 8,
        };
        let format = VvcPictureFormat {
            chroma_sampling: ChromaSampling::Cs420,
            bit_depth: SampleBitDepth::new(8).expect("valid bit depth"),
        };
        let layout = PlanarYuvGeometry::for_validated_shape(
            geometry.width,
            geometry.height,
            format.chroma_sampling,
            format.bit_depth,
        );
        Self {
            geometry,
            format,
            luma: vec![color.y; layout.luma_samples()],
            cb: vec![color.u; layout.chroma_samples()],
            cr: vec![color.v; layout.chroma_samples()],
            chroma_len: layout.chroma_samples(),
        }
    }

    fn sampled_color(&self) -> VvcSampledColor {
        VvcSampledColor {
            y: self.luma[0],
            u: self.cb[0],
            v: self.cr[0],
        }
    }

    fn scratch(format: VvcPictureFormat) -> Self {
        Self {
            geometry: VvcVideoGeometry {
                width: 0,
                height: 0,
            },
            format,
            luma: Vec::new(),
            cb: Vec::new(),
            cr: Vec::new(),
            chroma_len: 0,
        }
    }

    fn decoder_compat_frame(self) -> Self {
        let format = VvcPictureFormat {
            chroma_sampling: ChromaSampling::Cs420,
            bit_depth: self.format.bit_depth,
        };
        let layout = PlanarYuvGeometry::for_validated_shape(
            self.geometry.width,
            self.geometry.height,
            format.chroma_sampling,
            format.bit_depth,
        );
        let chroma_len = layout.chroma_samples();
        if self.format.chroma_sampling == ChromaSampling::Cs420 {
            return Self {
                geometry: self.geometry,
                format,
                luma: self.luma,
                cb: self.cb,
                cr: self.cr,
                chroma_len,
            };
        }

        let color = self.sampled_color();
        Self {
            geometry: self.geometry,
            format,
            luma: self.luma,
            cb: vec![VvcSample::from(color.u); chroma_len],
            cr: vec![VvcSample::from(color.v); chroma_len],
            chroma_len,
        }
    }
}

pub(in crate::vvc) fn vvc_neutral_sample(bit_depth: SampleBitDepth) -> VvcSample {
    1u16 << u32::from(bit_depth.bits() - 1)
}

pub(in crate::vvc) fn vvc_downshift_sample_to_u8(
    sample: VvcSample,
    bit_depth: SampleBitDepth,
) -> u8 {
    let bits = bit_depth.bits();
    if bits <= 8 {
        sample.min(u8::MAX as u16) as u8
    } else {
        (sample >> u32::from(bits - 8)).min(u8::MAX as u16) as u8
    }
}

fn vvc_bit_depth_is_supported(bit_depth: SampleBitDepth) -> bool {
    (VVC_MIN_BIT_DEPTH..=VVC_MAX_BIT_DEPTH).contains(&bit_depth.bits())
}

#[cfg(test)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum VvcCodingTreeStep {
    LumaTransformUnit {
        width: usize,
        height: usize,
    },
    ChromaTransformUnit {
        x: usize,
        y: usize,
        cb_coded: bool,
        cr_coded: bool,
    },
}

#[cfg(test)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum VvcLumaPartitionStep {
    QuadSplit {
        x: usize,
        y: usize,
        width: usize,
        height: usize,
    },
    Leaf {
        x: usize,
        y: usize,
        width: usize,
        height: usize,
    },
}

pub fn eos_annex_b() -> Vec<u8> {
    write_annex_b(&[VvcNalUnit::eos()]).expect("hard-coded EOS NAL should be valid")
}

pub fn vvc_black_yuv420p8_annex_b(params: VvcEncodeParams) -> Result<Vec<u8>, String> {
    validate_vvc_exact_frame_count(params)?;
    vvc_yuv420p8_annex_b(
        params,
        VvcSampledFrame::solid(VvcSampledColor { y: 0, u: 0, v: 0 }),
    )
}

pub fn vvc_yuv420p8_annex_b_from_input(
    input: &[u8],
    params: VvcEncodeParams,
) -> Result<Vec<u8>, String> {
    vvc_yuv_annex_b_from_input(
        input,
        params,
        VvcVideoGeometry {
            width: 8,
            height: 8,
        },
        PixelFormat::Yuv420p8,
    )
}

pub fn vvc_yuv420p_annex_b_from_input(
    input: &[u8],
    params: VvcEncodeParams,
    format: PixelFormat,
) -> Result<Vec<u8>, String> {
    vvc_yuv_annex_b_from_input(
        input,
        params,
        VvcVideoGeometry {
            width: 8,
            height: 8,
        },
        format,
    )
}

pub fn vvc_default_yuv_annex_b_from_input(
    input: &[u8],
    params: VvcEncodeParams,
    format: PixelFormat,
) -> Result<Vec<u8>, String> {
    vvc_yuv_annex_b_from_input(
        input,
        params,
        VvcVideoGeometry {
            width: 8,
            height: 8,
        },
        format,
    )
}

pub fn vvc_yuv_annex_b_from_input(
    input: &[u8],
    params: VvcEncodeParams,
    geometry: VvcVideoGeometry,
    format: PixelFormat,
) -> Result<Vec<u8>, String> {
    vvc_yuv_annex_b_from_input_with_limits(
        input,
        params,
        geometry,
        VvcVideoLimits::unbounded(),
        format,
    )
}

pub fn vvc_yuv_annex_b_from_input_with_limits(
    input: &[u8],
    params: VvcEncodeParams,
    geometry: VvcVideoGeometry,
    limits: VvcVideoLimits,
    format: PixelFormat,
) -> Result<Vec<u8>, String> {
    Ok(
        vvc_yuv_encode_artifacts_from_input_with_limits(input, params, geometry, limits, format)?
            .bitstream,
    )
}

pub fn vvc_yuv_encode_artifacts_from_input_with_limits(
    input: &[u8],
    params: VvcEncodeParams,
    geometry: VvcVideoGeometry,
    limits: VvcVideoLimits,
    format: PixelFormat,
) -> Result<VvcEncodeArtifacts, String> {
    let mut reader = Cursor::new(input);
    let mut bitstream = Vec::new();
    let mut reconstruction = Vec::new();
    vvc_yuv_encode_stream_with_limits(
        &mut reader,
        &mut bitstream,
        Some(&mut reconstruction),
        params,
        geometry,
        limits,
        format,
    )?;
    Ok(VvcEncodeArtifacts {
        bitstream,
        reconstruction,
    })
}

pub fn vvc_yuv_encode_stream_with_limits<R: Read, W: Write>(
    input: &mut R,
    bitstream: &mut W,
    reconstruction: Option<&mut dyn Write>,
    params: VvcEncodeParams,
    geometry: VvcVideoGeometry,
    limits: VvcVideoLimits,
    format: PixelFormat,
) -> Result<(), String> {
    vvc_yuv_encode_stream_with_limits_and_progress_and_frame_metrics(
        input,
        bitstream,
        reconstruction,
        params,
        geometry,
        limits,
        format,
        VvcEncodeOptions::default(),
        None,
        None,
    )
}

pub fn vvc_yuv_encode_stream_with_limits_and_progress<R: Read, W: Write>(
    input: &mut R,
    bitstream: &mut W,
    reconstruction: Option<&mut dyn Write>,
    params: VvcEncodeParams,
    geometry: VvcVideoGeometry,
    limits: VvcVideoLimits,
    format: PixelFormat,
    progress: Option<&mut dyn FnMut(VvcEncodeProgress)>,
) -> Result<(), String> {
    vvc_yuv_encode_stream_with_limits_and_progress_and_frame_metrics(
        input,
        bitstream,
        reconstruction,
        params,
        geometry,
        limits,
        format,
        VvcEncodeOptions::default(),
        progress,
        None,
    )
}

pub fn vvc_yuv_encode_stream_with_limits_and_frame_metrics<R: Read, W: Write>(
    input: &mut R,
    bitstream: &mut W,
    reconstruction: Option<&mut dyn Write>,
    params: VvcEncodeParams,
    geometry: VvcVideoGeometry,
    limits: VvcVideoLimits,
    format: PixelFormat,
    frame_metrics: Option<&mut dyn for<'a> FnMut(VvcEncodeFrameMetrics<'a>)>,
) -> Result<(), String> {
    vvc_yuv_encode_stream_with_limits_and_options_and_frame_metrics(
        input,
        bitstream,
        reconstruction,
        params,
        geometry,
        limits,
        format,
        VvcEncodeOptions::default(),
        frame_metrics,
    )
}

pub fn vvc_yuv_encode_stream_with_limits_and_options_and_frame_metrics<R: Read, W: Write>(
    input: &mut R,
    bitstream: &mut W,
    reconstruction: Option<&mut dyn Write>,
    params: VvcEncodeParams,
    geometry: VvcVideoGeometry,
    limits: VvcVideoLimits,
    format: PixelFormat,
    options: VvcEncodeOptions,
    frame_metrics: Option<&mut dyn for<'a> FnMut(VvcEncodeFrameMetrics<'a>)>,
) -> Result<(), String> {
    vvc_yuv_encode_stream_with_limits_and_progress_and_frame_metrics(
        input,
        bitstream,
        reconstruction,
        params,
        geometry,
        limits,
        format,
        options,
        None,
        frame_metrics,
    )
}

fn vvc_yuv_encode_stream_with_limits_and_progress_and_frame_metrics<R: Read, W: Write>(
    input: &mut R,
    bitstream: &mut W,
    mut reconstruction: Option<&mut dyn Write>,
    params: VvcEncodeParams,
    geometry: VvcVideoGeometry,
    limits: VvcVideoLimits,
    format: PixelFormat,
    options: VvcEncodeOptions,
    mut progress: Option<&mut dyn FnMut(VvcEncodeProgress)>,
    mut frame_metrics: Option<&mut dyn for<'a> FnMut(VvcEncodeFrameMetrics<'a>)>,
) -> Result<(), String> {
    let request = VvcEncodeRequest {
        params,
        geometry,
        limits,
        format,
    }
    .validate()?;
    let geometry = request.geometry;
    let frame_limit = request.frame_limit;
    let stream_format = request.format;
    let stream_layout = PlanarYuvGeometry::new(
        geometry.width,
        geometry.height,
        stream_format.chroma_sampling,
        stream_format.bit_depth,
    )?;
    let frame_len = stream_layout.frame_len();
    if options.lossless
        && !matches!(
            stream_format.chroma_sampling,
            ChromaSampling::Cs420 | ChromaSampling::Cs422 | ChromaSampling::Cs444
        )
    {
        return Err(format!(
            "VVC lossless encode is not implemented for {format}"
        ));
    }
    let lossless_residual = options.lossless
        && matches!(
            stream_format.chroma_sampling,
            ChromaSampling::Cs420 | ChromaSampling::Cs422
        );
    let lossless_palette =
        options.lossless && stream_format.chroma_sampling == ChromaSampling::Cs444;
    let slice_config = if lossless_residual {
        VvcSliceSyntaxConfig::residual_lossless(
            stream_format.chroma_sampling,
            stream_format.bit_depth,
        )
    } else if lossless_palette {
        VvcSliceSyntaxConfig::palette_444_lossless(stream_format.bit_depth)
    } else {
        VvcSliceSyntaxConfig::for_picture_format(stream_format)
    };
    let slice_config = vvc_slice_config_for_input_format(slice_config, format);
    let picture_partitioning = if lossless_residual {
        VvcPicturePartitioning::SingleSlice
    } else {
        VvcPicturePartitioning::OneSlicePerCtu
    };
    write_annex_b_to(
        bitstream,
        &[
            vvc_sps_unit(geometry, slice_config, stream_format.bit_depth),
            vvc_pps_unit_with_partitioning(geometry, picture_partitioning),
        ],
    )?;

    #[cfg(feature = "vvc-stats")]
    let mut vvc_stats = VvcStatsSink::from_env()?;

    let mut frame_buf = vec![0; frame_len];
    let mut frame_idx = 0usize;
    while frame_limit.should_read(frame_idx) {
        #[cfg(feature = "vvc-stats")]
        let mut frame_stats =
            VvcFrameStats::new(frame_idx, geometry, stream_format, options.lossless);
        #[cfg(feature = "vvc-stats")]
        {
            frame_stats.add_counter(
                "slice_count",
                match picture_partitioning {
                    VvcPicturePartitioning::SingleSlice => 1,
                    VvcPicturePartitioning::OneSlicePerCtu => vvc_picture_ctu_count(geometry),
                } as u64,
            );
            frame_stats.add_counter(
                "single_slice_frame",
                u64::from(picture_partitioning == VvcPicturePartitioning::SingleSlice),
            );
        }
        #[cfg(feature = "vvc-stats")]
        let stage_start = Instant::now();
        let frame_available =
            read_input_frame(input, &mut frame_buf, frame_idx, frame_limit, "VVC input")?;
        #[cfg(feature = "vvc-stats")]
        frame_stats.add_elapsed("read_frame", stage_start);
        if !frame_available {
            break;
        }
        if let Some(progress) = progress.as_deref_mut() {
            progress(VvcEncodeProgress {
                frame_idx,
                frame_count: frame_limit.metric_count(),
            });
        }
        #[cfg(feature = "vvc-stats")]
        let stage_start = Instant::now();
        let source_frame =
            sample_vvc_yuv_frame(&frame_buf, VvcEncodeParams { frames: 1 }, geometry, format)?;
        #[cfg(feature = "vvc-stats")]
        frame_stats.add_elapsed("sample_frame", stage_start);
        let (frame_recon_yuv, frame_bitstream_bytes) = {
            let mut frame_bitstream = CountingWriter::new(bitstream);
            if picture_partitioning == VvcPicturePartitioning::OneSlicePerCtu
                && vvc_picture_ctu_count(geometry) > 1
            {
                #[cfg(feature = "vvc-stats")]
                let stage_start = Instant::now();
                write_annex_b_to(
                    &mut frame_bitstream,
                    &[vvc_picture_header_unit(frame_idx, slice_config)],
                )?;
                #[cfg(feature = "vvc-stats")]
                frame_stats.add_elapsed("picture_header_write", stage_start);
            }

            let frame_recon_yuv = if stream_format.chroma_sampling == ChromaSampling::Cs444 {
                let mut frame_recon = VvcReconstructionFrame::new_neutral(geometry, stream_format);
                let mut ctu_frame = VvcSampledFrame::scratch(stream_format);
                for region in vvc_ctu_regions(geometry) {
                    #[cfg(feature = "vvc-stats")]
                    let stage_start = Instant::now();
                    copy_vvc_ctu_frame_into(&source_frame, region, &mut ctu_frame);
                    #[cfg(feature = "vvc-stats")]
                    frame_stats.add_elapsed("ctu_copy_source", stage_start);
                    #[cfg(feature = "vvc-stats")]
                    let stage_start = Instant::now();
                    let ctu_recon = palette::vvc_palette_444_reconstruction_yuv_with_config(
                        &ctu_frame,
                        slice_config,
                    );
                    #[cfg(feature = "vvc-stats")]
                    frame_stats.add_elapsed("ctu_palette_reconstruct", stage_start);
                    #[cfg(feature = "vvc-stats")]
                    let stage_start = Instant::now();
                    frame_recon.copy_ctu_yuv(region, &ctu_frame, &ctu_recon)?;
                    #[cfg(feature = "vvc-stats")]
                    frame_stats.add_elapsed("ctu_recon_copy", stage_start);
                    #[cfg(feature = "vvc-stats")]
                    let stage_start = Instant::now();
                    write_annex_b_to(
                        &mut frame_bitstream,
                        &[palette::vvc_palette_444_ctu_slice_unit(
                            frame_idx,
                            geometry,
                            region.slice_address,
                            &ctu_frame,
                            slice_config,
                        )?],
                    )?;
                    #[cfg(feature = "vvc-stats")]
                    frame_stats.add_elapsed("ctu_entropy_write", stage_start);
                }
                #[cfg(feature = "vvc-stats")]
                let stage_start = Instant::now();
                let yuv = frame_recon.into_yuv();
                #[cfg(feature = "vvc-stats")]
                frame_stats.add_elapsed("frame_recon_finalize", stage_start);
                yuv
            } else {
                let mut frame_recon =
                    VvcReconstructionFrame::new_neutral(geometry, source_frame.format);
                if picture_partitioning == VvcPicturePartitioning::SingleSlice {
                    let mut frame_ctus = Vec::with_capacity(vvc_picture_ctu_count(geometry));
                    for region in vvc_ctu_regions(geometry) {
                        #[cfg(feature = "vvc-stats")]
                        let stage_start = Instant::now();
                        let quantized = quantize_vvc_residual_ctu_into_frame_reconstruction(
                            &source_frame,
                            &mut frame_recon,
                            region,
                            lossless_residual,
                        );
                        #[cfg(feature = "vvc-stats")]
                        add_vvc_quantized_ctu_counters(&mut frame_stats, &quantized);
                        #[cfg(feature = "vvc-stats")]
                        frame_stats.add_elapsed("ctu_quantize", stage_start);
                        frame_ctus.push(VvcQuantizedCtu {
                            slice_address: region.slice_address,
                            geometry: region.geometry,
                            color: quantized,
                            luma_max_leaf_size: VVC_LOSSLESS_LUMA_LEAF_SIZE,
                        });
                    }
                    #[cfg(feature = "vvc-stats")]
                    let stage_start = Instant::now();
                    write_annex_b_to(
                        &mut frame_bitstream,
                        &[vvc_frame_slice_unit(
                            frame_idx,
                            geometry,
                            &frame_ctus,
                            slice_config,
                        )?],
                    )?;
                    #[cfg(feature = "vvc-stats")]
                    frame_stats.add_elapsed("frame_entropy_write", stage_start);
                } else {
                    let mut ctu_frame = VvcSampledFrame::scratch(stream_format);
                    for region in vvc_ctu_regions(geometry) {
                        #[cfg(feature = "vvc-stats")]
                        let stage_start = Instant::now();
                        copy_vvc_ctu_frame_into(&source_frame, region, &mut ctu_frame);
                        #[cfg(feature = "vvc-stats")]
                        frame_stats.add_elapsed("ctu_copy_source", stage_start);
                        #[cfg(feature = "vvc-stats")]
                        let stage_start = Instant::now();
                        let quantized = quantize_vvc_frame_with_reconstruction(&ctu_frame);
                        #[cfg(feature = "vvc-stats")]
                        add_vvc_quantized_ctu_counters(&mut frame_stats, &quantized.quantized);
                        #[cfg(feature = "vvc-stats")]
                        frame_stats.add_elapsed("ctu_quantize", stage_start);
                        #[cfg(feature = "vvc-stats")]
                        let stage_start = Instant::now();
                        frame_recon.copy_ctu_yuv(
                            region,
                            &ctu_frame,
                            &quantized.reconstruction_yuv,
                        )?;
                        #[cfg(feature = "vvc-stats")]
                        frame_stats.add_elapsed("ctu_recon_copy", stage_start);
                        #[cfg(feature = "vvc-stats")]
                        let stage_start = Instant::now();
                        write_annex_b_to(
                            &mut frame_bitstream,
                            &[vvc_ctu_slice_unit_with_luma_max_leaf_size(
                                frame_idx,
                                geometry,
                                region.slice_address,
                                region.geometry,
                                quantized.quantized,
                                slice_config,
                                VVC_CURRENT_MAX_LUMA_LEAF_SIZE,
                            )?],
                        )?;
                        #[cfg(feature = "vvc-stats")]
                        frame_stats.add_elapsed("ctu_entropy_write", stage_start);
                    }
                }
                #[cfg(feature = "vvc-stats")]
                let stage_start = Instant::now();
                let yuv = frame_recon.into_yuv();
                #[cfg(feature = "vvc-stats")]
                frame_stats.add_elapsed("frame_recon_finalize", stage_start);
                yuv
            };
            (frame_recon_yuv, frame_bitstream.bytes_written())
        };
        #[cfg(feature = "vvc-stats")]
        frame_stats.set_bitstream_bytes(frame_bitstream_bytes);
        if let Some(writer) = reconstruction.as_deref_mut() {
            #[cfg(feature = "vvc-stats")]
            let stage_start = Instant::now();
            writer.write_all(&frame_recon_yuv).map_err(|err| {
                format!("failed to write VVC reconstruction frame {frame_idx}: {err}")
            })?;
            #[cfg(feature = "vvc-stats")]
            frame_stats.add_elapsed("write_reconstruction", stage_start);
        }
        if let Some(frame_metrics) = frame_metrics.as_deref_mut() {
            #[cfg(feature = "vvc-stats")]
            let stage_start = Instant::now();
            frame_metrics(VvcEncodeFrameMetrics {
                frame_idx,
                frame_count: frame_limit.metric_count(),
                bitstream_bytes: frame_bitstream_bytes,
                source: &frame_buf,
                reconstruction: &frame_recon_yuv,
            });
            #[cfg(feature = "vvc-stats")]
            frame_stats.add_elapsed("frame_metrics", stage_start);
        }
        #[cfg(feature = "vvc-stats")]
        vvc_stats.write_frame(&frame_stats)?;
        frame_idx += 1;
    }

    if let FrameLimit::Exact(frames) = frame_limit {
        let mut extra = [0; 1];
        match input.read(&mut extra) {
            Ok(0) => Ok(()),
            Ok(_) => Err(format!(
                "VVC input contains trailing bytes after {} frame(s)",
                frames
            )),
            Err(err) => Err(format!("failed to check VVC input length: {err}")),
        }
    } else {
        Ok(())
    }
}

fn write_annex_b_to<W: Write>(output: &mut W, units: &[VvcNalUnit]) -> Result<(), String> {
    let bytes = write_annex_b(units)?;
    output
        .write_all(&bytes)
        .map_err(|err| format!("failed to write VVC Annex-B stream: {err}"))
}

fn vvc_ctu_regions(geometry: VvcVideoGeometry) -> impl Iterator<Item = VvcCtuRegion> {
    let cols = vvc_picture_ctu_cols(geometry);
    let rows = vvc_picture_ctu_rows(geometry);
    (0..rows).flat_map(move |ctu_y| {
        (0..cols).map(move |ctu_x| {
            let origin_x = ctu_x * VVC_CTU_SIZE;
            let origin_y = ctu_y * VVC_CTU_SIZE;
            let width = VVC_CTU_SIZE.min(geometry.width.saturating_sub(origin_x).max(1));
            let height = VVC_CTU_SIZE.min(geometry.height.saturating_sub(origin_y).max(1));
            VvcCtuRegion {
                slice_address: ctu_y * cols + ctu_x,
                origin_x,
                origin_y,
                geometry: VvcVideoGeometry { width, height },
            }
        })
    })
}

fn copy_vvc_ctu_frame_into(
    frame: &VvcSampledFrame,
    region: VvcCtuRegion,
    ctu_frame: &mut VvcSampledFrame,
) {
    let ctu_layout = PlanarYuvGeometry::for_validated_shape(
        region.geometry.width,
        region.geometry.height,
        frame.format.chroma_sampling,
        frame.format.bit_depth,
    );
    let frame_layout = PlanarYuvGeometry::for_validated_shape(
        frame.geometry.width,
        frame.geometry.height,
        frame.format.chroma_sampling,
        frame.format.bit_depth,
    );
    ctu_frame.geometry = region.geometry;
    ctu_frame.format = frame.format;
    ctu_frame.chroma_len = ctu_layout.chroma_samples();
    ctu_frame.luma.resize(ctu_layout.luma_samples(), 0);
    for y in 0..region.geometry.height {
        let src = (region.origin_y + y) * frame.geometry.width + region.origin_x;
        let dst = y * region.geometry.width;
        ctu_frame.luma[dst..dst + region.geometry.width]
            .copy_from_slice(&frame.luma[src..src + region.geometry.width]);
    }

    let subsample_x = chroma_subsample_x(frame.format.chroma_sampling);
    let subsample_y = chroma_subsample_y(frame.format.chroma_sampling);
    let chroma_width = ctu_layout.chroma_width();
    let chroma_height = ctu_layout.chroma_height();
    let source_chroma_width = frame_layout.chroma_width();
    let source_origin_x = region.origin_x / subsample_x;
    let source_origin_y = region.origin_y / subsample_y;
    let neutral = vvc_neutral_sample(frame.format.bit_depth);
    ctu_frame.cb.resize(ctu_frame.chroma_len, neutral);
    ctu_frame.cr.resize(ctu_frame.chroma_len, neutral);
    for y in 0..chroma_height {
        let src = (source_origin_y + y) * source_chroma_width + source_origin_x;
        let dst = y * chroma_width;
        ctu_frame.cb[dst..dst + chroma_width].copy_from_slice(&frame.cb[src..src + chroma_width]);
        ctu_frame.cr[dst..dst + chroma_width].copy_from_slice(&frame.cr[src..src + chroma_width]);
    }
}

fn quantize_vvc_residual_ctu_into_frame_reconstruction(
    source_frame: &VvcSampledFrame,
    frame_recon: &mut VvcReconstructionFrame,
    region: VvcCtuRegion,
    lossless_residual: bool,
) -> VvcQuantizedColor {
    let mut luma_tu_remainders = [0; MAX_VVC_LUMA_TUS];
    let mut luma_tu_negative = [false; MAX_VVC_LUMA_TUS];
    let mut luma_tu_dc_levels = [0; MAX_VVC_LUMA_TUS];
    let mut luma_tu_ac_levels = [[0; VVC_LUMA_AC_COEFFS_PER_TU]; MAX_VVC_LUMA_TUS];
    let mut luma_tu_has_ac = [false; MAX_VVC_LUMA_TUS];
    let mut cb_tu_dc_levels = [0; MAX_VVC_CHROMA_TUS];
    let mut cr_tu_dc_levels = [0; MAX_VVC_CHROMA_TUS];
    let mut cb_tu_ac_levels = [[0; VVC_CHROMA_AC_COEFFS_PER_TU]; MAX_VVC_CHROMA_TUS];
    let mut cr_tu_ac_levels = [[0; VVC_CHROMA_AC_COEFFS_PER_TU]; MAX_VVC_CHROMA_TUS];
    let mut cb_tu_has_ac = [false; MAX_VVC_CHROMA_TUS];
    let mut cr_tu_has_ac = [false; MAX_VVC_CHROMA_TUS];
    let mut prediction_scratch = VvcDcPredictionScratch::default();
    let mut predicted_luma = Vec::new();
    let mut predicted_cb = Vec::new();
    let mut predicted_cr = Vec::new();
    let mut transform_scratch = VvcInverseTransformScratch::default();
    let mut reconstructed_residual = Vec::new();
    let mut luma_residuals = Vec::new();
    let mut cb_residuals = Vec::new();
    let mut cr_residuals = Vec::new();

    let luma_max_leaf_size = if lossless_residual {
        VVC_LOSSLESS_LUMA_LEAF_SIZE
    } else {
        VVC_CURRENT_MAX_LUMA_LEAF_SIZE
    };
    let ctu_shape = VvcCtuPartitionShape {
        root_width: VVC_CTU_SIZE as u16,
        root_height: VVC_CTU_SIZE as u16,
        visible_width: region.geometry.coded_width() as u16,
        visible_height: region.geometry.coded_height() as u16,
        chroma_sampling: source_frame.format.chroma_sampling,
    };

    let mut luma_tu_count = 0usize;
    for local_node in vvc_luma_transform_nodes(ctu_shape, luma_max_leaf_size) {
        if luma_tu_count >= MAX_VVC_LUMA_TUS {
            break;
        }
        let node = vvc_global_ctu_node(local_node, region);
        predict_vvc_luma_dc_block_into(
            &mut predicted_luma,
            &mut prediction_scratch,
            &frame_recon.luma,
            source_frame.geometry,
            node,
            source_frame.format.bit_depth,
        );
        residual_luma_tu_at_into(
            &mut luma_residuals,
            source_frame,
            usize::from(node.x),
            usize::from(node.y),
            usize::from(node.width),
            usize::from(node.height),
            &predicted_luma,
        );
        if lossless_residual {
            let dc_level = luma_residuals.first().copied().unwrap_or(0);
            luma_tu_remainders[luma_tu_count] = dc_level.unsigned_abs().min(u8::MAX as u16) as u8;
            luma_tu_negative[luma_tu_count] = dc_level < 0;
            luma_tu_dc_levels[luma_tu_count] = dc_level;
            (
                luma_tu_ac_levels[luma_tu_count],
                luma_tu_has_ac[luma_tu_count],
            ) = lossless_luma_ac_levels_and_flag(&luma_residuals, usize::from(node.width));
            fill_visible_luma_node(
                &mut frame_recon.luma,
                source_frame.geometry,
                node,
                &predicted_luma,
                &luma_residuals,
                source_frame.format.bit_depth,
            );
        } else {
            let quantized = quantize_vvc_luma_residual_greedy(
                &luma_residuals,
                node.width,
                node.height,
                source_frame.format.bit_depth,
            );
            luma_tu_remainders[luma_tu_count] = quantized.abs_remainder;
            luma_tu_negative[luma_tu_count] =
                quantized.reconstructed_dc_coeff < 0 && quantized.abs_remainder != 0;
            luma_tu_dc_levels[luma_tu_count] = quantized.reconstructed_dc_coeff;
            luma_tu_ac_levels[luma_tu_count] = quantized.reconstructed_ac_coeffs;
            luma_tu_has_ac[luma_tu_count] = quantized.has_ac;
            inverse_transform_vvc_luma_quantized_block_into(
                &mut reconstructed_residual,
                &mut transform_scratch,
                node.width,
                node.height,
                quantized.reconstructed_dc_coeff,
                &quantized.reconstructed_ac_coeffs,
                source_frame.format.bit_depth,
            );
            fill_visible_luma_node(
                &mut frame_recon.luma,
                source_frame.geometry,
                node,
                &predicted_luma,
                &reconstructed_residual,
                source_frame.format.bit_depth,
            );
        }
        luma_tu_count += 1;
    }

    let mut chroma_tu_count = 0usize;
    for local_node in vvc_chroma_transform_nodes(ctu_shape) {
        if chroma_tu_count >= MAX_VVC_CHROMA_TUS {
            break;
        }
        let node = vvc_global_ctu_node(local_node, region);
        let subsample_x = chroma_subsample_x(source_frame.format.chroma_sampling);
        let subsample_y = chroma_subsample_y(source_frame.format.chroma_sampling);
        let chroma_x = usize::from(node.x) / subsample_x;
        let chroma_y = usize::from(node.y) / subsample_y;
        let chroma_width = usize::from(node.width) / subsample_x;
        let chroma_height = usize::from(node.height) / subsample_y;
        predict_vvc_chroma_dc_block_into(
            &mut predicted_cb,
            &mut prediction_scratch,
            &frame_recon.cb,
            source_frame.geometry,
            node,
            source_frame.format.chroma_sampling,
            source_frame.format.bit_depth,
        );
        predict_vvc_chroma_dc_block_into(
            &mut predicted_cr,
            &mut prediction_scratch,
            &frame_recon.cr,
            source_frame.geometry,
            node,
            source_frame.format.chroma_sampling,
            source_frame.format.bit_depth,
        );
        residual_chroma_tu_at_into(
            &mut cb_residuals,
            &source_frame.cb,
            source_frame.geometry,
            source_frame.format,
            chroma_x,
            chroma_y,
            chroma_width,
            chroma_height,
            &predicted_cb,
        );
        residual_chroma_tu_at_into(
            &mut cr_residuals,
            &source_frame.cr,
            source_frame.geometry,
            source_frame.format,
            chroma_x,
            chroma_y,
            chroma_width,
            chroma_height,
            &predicted_cr,
        );
        if lossless_residual {
            cb_tu_dc_levels[chroma_tu_count] = cb_residuals.first().copied().unwrap_or(0);
            cr_tu_dc_levels[chroma_tu_count] = cr_residuals.first().copied().unwrap_or(0);
            (
                cb_tu_ac_levels[chroma_tu_count],
                cb_tu_has_ac[chroma_tu_count],
            ) = lossless_chroma_ac_levels_and_flag(&cb_residuals, chroma_width);
            (
                cr_tu_ac_levels[chroma_tu_count],
                cr_tu_has_ac[chroma_tu_count],
            ) = lossless_chroma_ac_levels_and_flag(&cr_residuals, chroma_width);
            fill_visible_chroma_node(
                &mut frame_recon.cb,
                source_frame.geometry,
                node,
                source_frame.format.chroma_sampling,
                &predicted_cb,
                &cb_residuals,
                source_frame.format.bit_depth,
            );
            fill_visible_chroma_node(
                &mut frame_recon.cr,
                source_frame.geometry,
                node,
                source_frame.format.chroma_sampling,
                &predicted_cr,
                &cr_residuals,
                source_frame.format.bit_depth,
            );
        } else {
            let cb_quantized = quantize_vvc_chroma_residual_greedy(
                &cb_residuals,
                chroma_width as u16,
                chroma_height as u16,
                source_frame.format.bit_depth,
            );
            let cr_quantized = quantize_vvc_chroma_residual_greedy(
                &cr_residuals,
                chroma_width as u16,
                chroma_height as u16,
                source_frame.format.bit_depth,
            );
            cb_tu_dc_levels[chroma_tu_count] = cb_quantized.reconstructed_dc_coeff;
            cr_tu_dc_levels[chroma_tu_count] = cr_quantized.reconstructed_dc_coeff;
            cb_tu_ac_levels[chroma_tu_count] = cb_quantized.reconstructed_ac_coeffs;
            cr_tu_ac_levels[chroma_tu_count] = cr_quantized.reconstructed_ac_coeffs;
            cb_tu_has_ac[chroma_tu_count] = cb_quantized.has_ac;
            cr_tu_has_ac[chroma_tu_count] = cr_quantized.has_ac;
            inverse_transform_vvc_chroma_quantized_block_into(
                &mut reconstructed_residual,
                &mut transform_scratch,
                chroma_width as u16,
                chroma_height as u16,
                cb_quantized.reconstructed_dc_coeff,
                &cb_quantized.reconstructed_ac_coeffs,
                source_frame.format.bit_depth,
            );
            fill_visible_chroma_node(
                &mut frame_recon.cb,
                source_frame.geometry,
                node,
                source_frame.format.chroma_sampling,
                &predicted_cb,
                &reconstructed_residual,
                source_frame.format.bit_depth,
            );
            inverse_transform_vvc_chroma_quantized_block_into(
                &mut reconstructed_residual,
                &mut transform_scratch,
                chroma_width as u16,
                chroma_height as u16,
                cr_quantized.reconstructed_dc_coeff,
                &cr_quantized.reconstructed_ac_coeffs,
                source_frame.format.bit_depth,
            );
            fill_visible_chroma_node(
                &mut frame_recon.cr,
                source_frame.geometry,
                node,
                source_frame.format.chroma_sampling,
                &predicted_cr,
                &reconstructed_residual,
                source_frame.format.bit_depth,
            );
        }
        chroma_tu_count += 1;
    }

    let color = source_frame.sampled_color();
    let cb_rem = quantize_vvc_chroma_sample(vvc_downshift_sample_to_u8(
        color.u,
        source_frame.format.bit_depth,
    ));
    let cr_rem = quantize_vvc_chroma_sample(vvc_downshift_sample_to_u8(
        color.v,
        source_frame.format.bit_depth,
    ));
    VvcQuantizedColor {
        y: vvc_downshift_sample_to_u8(color.y, source_frame.format.bit_depth),
        u: if lossless_residual {
            vvc_downshift_sample_to_u8(color.u, source_frame.format.bit_depth)
        } else {
            reconstruct_vvc_chroma(cb_rem)
        },
        v: if lossless_residual {
            vvc_downshift_sample_to_u8(color.v, source_frame.format.bit_depth)
        } else {
            reconstruct_vvc_chroma(cr_rem)
        },
        luma_tu_remainders,
        luma_tu_negative,
        luma_tu_dc_levels,
        luma_tu_ac_levels,
        luma_tu_has_ac,
        luma_tu_count,
        chroma_tu_count,
        cb_tu_dc_levels,
        cr_tu_dc_levels,
        cb_tu_ac_levels,
        cr_tu_ac_levels,
        cb_tu_has_ac,
        cr_tu_has_ac,
        cb_rem,
        cr_rem,
    }
}

fn vvc_global_ctu_node(mut node: VvcCodingTreeNode, region: VvcCtuRegion) -> VvcCodingTreeNode {
    node.x += region.origin_x as u16;
    node.y += region.origin_y as u16;
    node
}

impl VvcReconstructionFrame {
    fn new_neutral(geometry: VvcVideoGeometry, format: VvcPictureFormat) -> Self {
        let layout = PlanarYuvGeometry::for_validated_shape(
            geometry.width,
            geometry.height,
            format.chroma_sampling,
            format.bit_depth,
        );
        let neutral = vvc_neutral_sample(format.bit_depth);
        Self {
            geometry,
            format,
            luma: vec![neutral; layout.luma_samples()],
            cb: vec![neutral; layout.chroma_samples()],
            cr: vec![neutral; layout.chroma_samples()],
        }
    }

    fn copy_ctu_yuv(
        &mut self,
        region: VvcCtuRegion,
        ctu_frame: &VvcSampledFrame,
        ctu_yuv: &[VvcSample],
    ) -> Result<(), String> {
        if ctu_frame.format.chroma_sampling != self.format.chroma_sampling {
            return Err(format!(
                "VVC reconstruction CTU format mismatch: frame {:?}, CTU {:?}",
                self.format.chroma_sampling, ctu_frame.format.chroma_sampling
            ));
        }

        let luma_len = ctu_frame.geometry.luma_samples();
        if ctu_yuv.len() != luma_len + ctu_frame.chroma_len * 2 {
            return Err(format!(
                "VVC CTU reconstruction size mismatch: got {} bytes, expected {}",
                ctu_yuv.len(),
                luma_len + ctu_frame.chroma_len * 2
            ));
        }

        for y in 0..ctu_frame.geometry.height {
            let src = y * ctu_frame.geometry.width;
            let dst = (region.origin_y + y) * self.geometry.width + region.origin_x;
            self.luma[dst..dst + ctu_frame.geometry.width]
                .copy_from_slice(&ctu_yuv[src..src + ctu_frame.geometry.width]);
        }

        let subsample_x = chroma_subsample_x(self.format.chroma_sampling);
        let subsample_y = chroma_subsample_y(self.format.chroma_sampling);
        let frame_chroma_width = self.geometry.width / subsample_x;
        let ctu_chroma_width = ctu_frame.geometry.width / subsample_x;
        let ctu_chroma_height = ctu_frame.geometry.height / subsample_y;
        let dst_origin_x = region.origin_x / subsample_x;
        let dst_origin_y = region.origin_y / subsample_y;
        let cb_offset = luma_len;
        let cr_offset = cb_offset + ctu_frame.chroma_len;
        for y in 0..ctu_chroma_height {
            let src = y * ctu_chroma_width;
            let dst = (dst_origin_y + y) * frame_chroma_width + dst_origin_x;
            self.cb[dst..dst + ctu_chroma_width]
                .copy_from_slice(&ctu_yuv[cb_offset + src..cb_offset + src + ctu_chroma_width]);
            self.cr[dst..dst + ctu_chroma_width]
                .copy_from_slice(&ctu_yuv[cr_offset + src..cr_offset + src + ctu_chroma_width]);
        }

        Ok(())
    }

    fn into_yuv(self) -> Vec<u8> {
        let layout = PlanarYuvGeometry::for_validated_shape(
            self.geometry.width,
            self.geometry.height,
            self.format.chroma_sampling,
            self.format.bit_depth,
        );
        let mut output = vec![0; layout.frame_len()];
        let bytes_per_sample = layout.bytes_per_sample();
        let y_bytes = layout.luma_samples() * bytes_per_sample;
        let c_bytes = layout.chroma_samples() * bytes_per_sample;
        let (y_plane, chroma) = output.split_at_mut(y_bytes);
        let (cb_plane, cr_plane) = chroma.split_at_mut(c_bytes);
        pack_vvc_plane(&self.luma, y_plane, self.format.bit_depth);
        pack_vvc_plane(&self.cb, cb_plane, self.format.bit_depth);
        pack_vvc_plane(&self.cr, cr_plane, self.format.bit_depth);
        output
    }
}

fn pack_vvc_plane(samples: &[VvcSample], output: &mut [u8], bit_depth: SampleBitDepth) {
    debug_assert_eq!(output.len(), samples.len() * bit_depth.bytes_per_sample());
    if bit_depth.bits() <= 8 {
        for (dst, &sample) in output.iter_mut().zip(samples) {
            *dst = sample as u8;
        }
    } else {
        for (dst, &sample) in output.chunks_exact_mut(2).zip(samples) {
            let bytes = sample.to_le_bytes();
            dst[0] = bytes[0];
            dst[1] = bytes[1];
        }
    }
}

pub fn vvc_yuv420_cabac_vector_dump_json(
    input: &[u8],
    params: VvcEncodeParams,
    geometry: VvcVideoGeometry,
    format: PixelFormat,
) -> Result<String, String> {
    if format.chroma_sampling() != Some(ChromaSampling::Cs420) {
        return Err(format!(
            "VVC CABAC vector dump currently expects 4:2:0 input; got {format}"
        ));
    }
    let source_frame = sample_vvc_yuv_frame(input, params, geometry, format)?;
    let compat_frame = source_frame.decoder_compat_frame();
    let color = quantize_vvc_frame(&compat_frame);
    let params = vvc_ctu_partition_params(compat_frame.geometry, color).ok_or_else(|| {
        format!(
            "VVC CABAC vector dump has no generated CTU path for coded geometry {}x{}",
            compat_frame.geometry.coded_width(),
            compat_frame.geometry.coded_height()
        )
    })?;
    let dump = vvc_ctu_partition_cabac_dump(&params, VvcSliceSyntaxConfig::yuv420_residual());
    let mapped_context_symbols = dump
        .semantic_symbols
        .iter()
        .filter(|symbol| symbol.kind == 2)
        .count();
    if mapped_context_symbols != dump.context_bin_count {
        return Err(format!(
            "VVC CABAC vector dump used {} context bins but only {} have RTL context IDs; audit VvcCabacContext::rtl_context_id before using this as an RTL reference",
            dump.context_bin_count, mapped_context_symbols
        ));
    }
    Ok(vvc_cabac_vector_dump_json(
        compat_frame.geometry,
        &params,
        &dump.symbols,
        &dump.semantic_symbols,
        &dump.context_events,
        &dump.bin_engine_events,
        &dump.bits,
    ))
}

pub fn sample_vvc_first_yuv420p8(
    input: &[u8],
    params: VvcEncodeParams,
) -> Result<VvcSampledColor, String> {
    Ok(sample_vvc_yuv_frame(
        input,
        params,
        VvcVideoGeometry {
            width: 8,
            height: 8,
        },
        PixelFormat::Yuv420p8,
    )?
    .sampled_color())
}

fn sample_vvc_yuv_frame(
    input: &[u8],
    params: VvcEncodeParams,
    geometry: VvcVideoGeometry,
    format: PixelFormat,
) -> Result<VvcSampledFrame, String> {
    sample_vvc_yuv_frame_at(input, params, geometry, format, 0)
}

fn sample_vvc_yuv_frame_at(
    input: &[u8],
    params: VvcEncodeParams,
    geometry: VvcVideoGeometry,
    format: PixelFormat,
    frame_idx: usize,
) -> Result<VvcSampledFrame, String> {
    validate_vvc_exact_frame_count(params)?;
    if frame_idx >= params.frames {
        return Err(format!(
            "VVC input requested frame {frame_idx}, but stream has {} frame(s)",
            params.frames
        ));
    }
    geometry.validate_shape()?;
    let stream_format = Picture::validate_format_shape(
        geometry.width,
        geometry.height,
        format,
        validate_vvc_input_format,
    )?;
    let layout = PlanarYuvGeometry::new(
        geometry.width,
        geometry.height,
        stream_format.chroma_sampling,
        stream_format.bit_depth,
    )?;
    let frame_len = layout.frame_len();
    let expected_len = frame_len * params.frames;
    if input.len() != expected_len {
        return Err(format!(
            "VVC input size mismatch: got {} bytes, expected {} for {}x{} {format} with {} frame(s)",
            input.len(),
            expected_len,
            geometry.width,
            geometry.height,
            params.frames
        ));
    }
    let frame_base = frame_len * frame_idx;
    let frame = &input[frame_base..frame_base + frame_len];

    let luma_samples = layout.luma_samples();
    let mut luma = vec![0; luma_samples];

    let chroma_plane_samples = layout.chroma_samples();
    let mut cb = vec![0; chroma_plane_samples];
    let mut cr = vec![0; chroma_plane_samples];
    let bytes_per_sample = layout.bytes_per_sample();
    let y_bytes = luma_samples * bytes_per_sample;
    let c_bytes = chroma_plane_samples * bytes_per_sample;
    unpack_vvc_plane(&frame[..y_bytes], &mut luma, stream_format.bit_depth);
    unpack_vvc_plane(
        &frame[y_bytes..y_bytes + c_bytes],
        &mut cb,
        stream_format.bit_depth,
    );
    unpack_vvc_plane(
        &frame[y_bytes + c_bytes..y_bytes + c_bytes * 2],
        &mut cr,
        stream_format.bit_depth,
    );

    Ok(VvcSampledFrame {
        geometry,
        format: stream_format,
        luma,
        cb,
        cr,
        chroma_len: chroma_plane_samples,
    })
}

fn unpack_vvc_plane(input: &[u8], output: &mut [VvcSample], bit_depth: SampleBitDepth) {
    debug_assert_eq!(input.len(), output.len() * bit_depth.bytes_per_sample());
    if bit_depth.bits() <= 8 {
        for (dst, &sample) in output.iter_mut().zip(input) {
            *dst = VvcSample::from(sample);
        }
    } else {
        let max_sample = bit_depth.max_sample();
        for (dst, sample) in output.iter_mut().zip(input.chunks_exact(2)) {
            *dst = u16::from_le_bytes([sample[0], sample[1]]).min(max_sample);
        }
    }
}

fn validate_vvc_exact_frame_count(params: VvcEncodeParams) -> Result<FrameLimit, String> {
    let frame_limit = FrameLimit::from_frame_count(params.frames);
    if matches!(frame_limit, FrameLimit::UntilEof) {
        return Err("VVC encode expects at least one frame".to_string());
    }
    Ok(frame_limit)
}

fn validate_vvc_input_format(format: PixelFormat) -> Result<VvcPictureFormat, String> {
    if format == PixelFormat::Gbrp8 {
        return Ok(VvcPictureFormat {
            chroma_sampling: ChromaSampling::Cs444,
            bit_depth: format.bit_depth(),
        });
    }
    let Some(chroma_sampling) = format.chroma_sampling() else {
        return Err(format!(
            "VVC input expects planar YUV or gbrp8 format; got {format}"
        ));
    };
    match chroma_sampling {
        ChromaSampling::Cs420 | ChromaSampling::Cs422 | ChromaSampling::Cs444
            if vvc_bit_depth_is_supported(format.bit_depth()) =>
        {
            Ok(VvcPictureFormat {
                chroma_sampling,
                bit_depth: format.bit_depth(),
            })
        }
        ChromaSampling::Cs420 => Err(format!(
            "VVC 4:2:0 input currently supports bit depths {VVC_MIN_BIT_DEPTH}..{VVC_MAX_BIT_DEPTH}; got {format}"
        )),
        ChromaSampling::Cs422 => Err(format!(
            "VVC 4:2:2 input currently supports bit depths {VVC_MIN_BIT_DEPTH}..{VVC_MAX_BIT_DEPTH}; got {format}"
        )),
        ChromaSampling::Cs444 => Err(format!(
            "VVC 4:4:4 palette input currently supports bit depths {VVC_MIN_BIT_DEPTH}..{VVC_MAX_BIT_DEPTH}; got {format}"
        )),
        ChromaSampling::Monochrome => Err(format!(
            "VVC monochrome input is not wired yet; got {format}"
        )),
    }
}

fn vvc_slice_config_for_input_format(
    slice_config: VvcSliceSyntaxConfig,
    format: PixelFormat,
) -> VvcSliceSyntaxConfig {
    if format == PixelFormat::Gbrp8 {
        slice_config.with_vui_signal(VvcVuiSignal::srgb_gbr_compatible())
    } else {
        slice_config
    }
}

fn vvc_yuv420p8_annex_b(
    params: VvcEncodeParams,
    frame: VvcSampledFrame,
) -> Result<Vec<u8>, String> {
    vvc_annex_b(params, frame)
}

fn vvc_annex_b(params: VvcEncodeParams, frame: VvcSampledFrame) -> Result<Vec<u8>, String> {
    let geometry = frame.geometry;
    let quantized = quantize_vvc_frame(&frame);
    vvc_annex_b_from_quantized(
        params,
        geometry,
        quantized,
        VvcPictureFormat {
            chroma_sampling: ChromaSampling::Cs420,
            bit_depth: SampleBitDepth::new(8).expect("valid bit depth"),
        },
    )
}

fn vvc_annex_b_from_quantized(
    params: VvcEncodeParams,
    geometry: VvcVideoGeometry,
    quantized: VvcQuantizedColor,
    format: VvcPictureFormat,
) -> Result<Vec<u8>, String> {
    let quantized_frames = vec![quantized; params.frames];
    vvc_annex_b_from_quantized_frames(params, geometry, &quantized_frames, format)
}

fn vvc_annex_b_from_quantized_frames(
    params: VvcEncodeParams,
    geometry: VvcVideoGeometry,
    quantized_frames: &[VvcQuantizedColor],
    format: VvcPictureFormat,
) -> Result<Vec<u8>, String> {
    if quantized_frames.len() != params.frames {
        return Err(format!(
            "VVC residual encoder got {} frame(s), expected {}",
            quantized_frames.len(),
            params.frames
        ));
    }
    let mut units = Vec::with_capacity(params.frames + 3);
    let slice_config = VvcSliceSyntaxConfig::for_picture_format(format);
    units.push(vvc_sps_unit(geometry, slice_config, format.bit_depth));
    units.push(vvc_pps_unit(geometry));
    for (frame_idx, quantized) in quantized_frames.iter().copied().enumerate() {
        units.push(vvc_slice_unit(
            frame_idx,
            geometry,
            quantized,
            slice_config,
        )?);
    }
    write_annex_b(&units)
}

#[cfg(test)]
fn vvc_coding_tree_plan(geometry: VvcVideoGeometry) -> Vec<VvcCodingTreeStep> {
    vvc_coding_tree_plan_with_config(geometry, VvcCodingTreeConfig::yuv(ChromaSampling::Cs420))
}

#[cfg(test)]
fn vvc_coding_tree_plan_with_config(
    geometry: VvcVideoGeometry,
    config: VvcCodingTreeConfig,
) -> Vec<VvcCodingTreeStep> {
    let mut steps = Vec::new();
    steps.push(VvcCodingTreeStep::LumaTransformUnit {
        width: geometry.coded_width(),
        height: geometry.coded_height(),
    });

    let chroma_width = geometry.coded_width() / chroma_subsample_x(config.chroma_sampling);
    let chroma_height = geometry.coded_height() / chroma_subsample_y(config.chroma_sampling);
    for y in (0..chroma_height).step_by(4) {
        for x in (0..chroma_width).step_by(4) {
            let first = x == 0 && y == 0;
            steps.push(VvcCodingTreeStep::ChromaTransformUnit {
                x,
                y,
                cb_coded: first && geometry.coded_width() <= 8,
                cr_coded: first,
            });
        }
    }

    steps
}

#[cfg(test)]
fn vvc_luma_partition_plan(geometry: VvcVideoGeometry) -> Vec<VvcLumaPartitionStep> {
    let coded = geometry.coded();
    let mut steps = Vec::new();
    append_vvc_luma_partition(
        &mut steps,
        0,
        0,
        coded.width,
        coded.height,
        VvcCodedGeometry {
            width: VVC_CURRENT_MAX_LUMA_LEAF_SIZE as usize,
            height: VVC_CURRENT_MAX_LUMA_LEAF_SIZE as usize,
        },
    );
    steps
}

#[cfg(test)]
fn append_vvc_luma_partition(
    steps: &mut Vec<VvcLumaPartitionStep>,
    x: usize,
    y: usize,
    width: usize,
    height: usize,
    max_leaf: VvcCodedGeometry,
) {
    if width > max_leaf.width || height > max_leaf.height {
        steps.push(VvcLumaPartitionStep::QuadSplit {
            x,
            y,
            width,
            height,
        });
        let child_width = width / 2;
        let child_height = height / 2;
        for child_y in [y, y + child_height] {
            for child_x in [x, x + child_width] {
                append_vvc_luma_partition(
                    steps,
                    child_x,
                    child_y,
                    child_width,
                    child_height,
                    max_leaf,
                );
            }
        }
    } else {
        steps.push(VvcLumaPartitionStep::Leaf {
            x,
            y,
            width,
            height,
        });
    }
}

#[cfg(test)]
fn vvc_cabac_bits(
    geometry: VvcVideoGeometry,
    color: VvcQuantizedColor,
    slice_config: VvcSliceSyntaxConfig,
) -> Vec<bool> {
    vvc_cabac_bits_with_luma_max_leaf_size(
        geometry,
        color,
        slice_config,
        VVC_CURRENT_MAX_LUMA_LEAF_SIZE,
    )
}

fn vvc_cabac_bits_with_luma_max_leaf_size(
    geometry: VvcVideoGeometry,
    color: VvcQuantizedColor,
    slice_config: VvcSliceSyntaxConfig,
    luma_max_leaf_size: u16,
) -> Vec<bool> {
    if let Some(params) = vvc_ctu_partition_params_with_luma_max_leaf_size_and_chroma(
        geometry,
        color,
        luma_max_leaf_size,
        slice_config.coding_tree.chroma_sampling,
    ) {
        return vvc_ctu_partition_cabac_bits(&params, slice_config);
    }
    unimplemented!(
        "VVC coding tree for coded geometry {}x{} must be generated from syntax parameters",
        geometry.coded_width(),
        geometry.coded_height()
    );
}

fn vvc_frame_cabac_bits(
    picture_geometry: VvcVideoGeometry,
    ctus: &[VvcQuantizedCtu],
    slice_config: VvcSliceSyntaxConfig,
) -> Vec<bool> {
    debug_assert_eq!(ctus.len(), vvc_picture_ctu_count(picture_geometry));
    let mut cabac = VvcCabacEncoder::new();
    let mut contexts = initial_vvc_cabac_contexts(slice_config);
    cabac.start();
    for (expected_slice_address, ctu) in ctus.iter().enumerate() {
        debug_assert_eq!(ctu.slice_address, expected_slice_address);
        let params = vvc_ctu_partition_params_with_luma_max_leaf_size_and_chroma(
            ctu.geometry,
            ctu.color.clone(),
            ctu.luma_max_leaf_size,
            slice_config.coding_tree.chroma_sampling,
        )
        .unwrap_or_else(|| {
            panic!(
                "VVC frame CABAC CTU {} has unsupported coded geometry {}x{}",
                ctu.slice_address,
                ctu.geometry.coded_width(),
                ctu.geometry.coded_height()
            )
        });
        encode_ctu_partition_body_with_contexts(&mut cabac, &mut contexts, &params, slice_config);
    }
    cabac.encode_bin_trm(true);
    cabac.finish()
}

fn vvc_ctu_partition_params(
    geometry: VvcVideoGeometry,
    color: VvcQuantizedColor,
) -> Option<VvcCtuPartitionParams> {
    vvc_ctu_partition_params_with_luma_max_leaf_size(
        geometry,
        color,
        VVC_CURRENT_MAX_LUMA_LEAF_SIZE,
    )
}

fn vvc_ctu_partition_params_with_luma_max_leaf_size(
    geometry: VvcVideoGeometry,
    color: VvcQuantizedColor,
    luma_max_leaf_size: u16,
) -> Option<VvcCtuPartitionParams> {
    let coded = geometry.coded();
    if coded.width > VVC_CTU_SIZE
        || coded.height > VVC_CTU_SIZE
        || coded.width < 8
        || coded.height < 8
    {
        return None;
    }
    vvc_ctu_partition_params_with_luma_max_leaf_size_and_chroma(
        geometry,
        color,
        luma_max_leaf_size,
        ChromaSampling::Cs420,
    )
}

fn vvc_ctu_partition_params_with_luma_max_leaf_size_and_chroma(
    geometry: VvcVideoGeometry,
    color: VvcQuantizedColor,
    luma_max_leaf_size: u16,
    chroma_sampling: ChromaSampling,
) -> Option<VvcCtuPartitionParams> {
    let coded = geometry.coded();
    if coded.width > VVC_CTU_SIZE
        || coded.height > VVC_CTU_SIZE
        || coded.width < 8
        || coded.height < 8
    {
        return None;
    }
    let shape = VvcCtuPartitionShape {
        root_width: VVC_CTU_SIZE as u16,
        root_height: VVC_CTU_SIZE as u16,
        visible_width: coded.width as u16,
        visible_height: coded.height as u16,
        chroma_sampling,
    };
    let chroma_tu_count = if color.chroma_tu_count > 1 {
        color.chroma_tu_count
    } else {
        vvc_chroma_transform_nodes(shape).len()
    };
    let (
        luma_tu_count,
        luma_tu_abs_levels,
        luma_tu_negative,
        luma_tu_dc_levels,
        luma_tu_ac_levels,
        luma_tu_has_ac,
    ) = vvc_luma_residual_arrays_for_geometry(coded, chroma_sampling, luma_max_leaf_size, color);
    Some(VvcCtuPartitionParams {
        root_width: VVC_CTU_SIZE,
        root_height: VVC_CTU_SIZE,
        visible_width: coded.width,
        visible_height: coded.height,
        chroma_sampling,
        luma_max_leaf_size,
        chroma_tu_count,
        luma_tu_count,
        luma_tu_abs_levels,
        luma_tu_negative,
        luma_tu_dc_levels,
        luma_tu_ac_levels,
        luma_tu_has_ac,
        cb_dc_abs_level: color.cb_rem,
        cb_dc_negative: color.u < 128 && color.cb_rem != 0,
        cb_tu_dc_levels: color.cb_tu_dc_levels,
        cr_tu_dc_levels: color.cr_tu_dc_levels,
        cb_tu_ac_levels: color.cb_tu_ac_levels,
        cr_tu_ac_levels: color.cr_tu_ac_levels,
        cb_tu_has_ac: color.cb_tu_has_ac,
        cr_tu_has_ac: color.cr_tu_has_ac,
    })
}

fn vvc_luma_residual_arrays_for_geometry(
    coded: VvcCodedGeometry,
    chroma_sampling: ChromaSampling,
    luma_max_leaf_size: u16,
    color: VvcQuantizedColor,
) -> (
    usize,
    [u8; MAX_VVC_LUMA_TUS],
    [bool; MAX_VVC_LUMA_TUS],
    [i16; MAX_VVC_LUMA_TUS],
    [[i16; VVC_LUMA_AC_COEFFS_PER_TU]; MAX_VVC_LUMA_TUS],
    [bool; MAX_VVC_LUMA_TUS],
) {
    let mut luma_tu_count = color.luma_tu_count;
    let mut luma_tu_abs_levels = color.luma_tu_remainders;
    let mut luma_tu_negative = color.luma_tu_negative;
    let mut luma_tu_dc_levels = color.luma_tu_dc_levels;
    let mut luma_tu_ac_levels = color.luma_tu_ac_levels;
    let mut luma_tu_has_ac = color.luma_tu_has_ac;
    if color.luma_tu_count > 1 {
        return (
            luma_tu_count,
            luma_tu_abs_levels,
            luma_tu_negative,
            luma_tu_dc_levels,
            luma_tu_ac_levels,
            luma_tu_has_ac,
        );
    }

    let leaf_count = vvc_luma_leaf_count(coded, chroma_sampling, luma_max_leaf_size);
    luma_tu_count = leaf_count;
    for idx in 0..leaf_count.min(MAX_VVC_LUMA_TUS) {
        luma_tu_abs_levels[idx] = color.luma_tu_remainders[0];
        luma_tu_negative[idx] = color.luma_tu_negative[0];
        luma_tu_dc_levels[idx] = color.luma_tu_dc_levels[0];
        luma_tu_ac_levels[idx] = color.luma_tu_ac_levels[0];
        luma_tu_has_ac[idx] = color.luma_tu_has_ac[0];
    }
    (
        luma_tu_count,
        luma_tu_abs_levels,
        luma_tu_negative,
        luma_tu_dc_levels,
        luma_tu_ac_levels,
        luma_tu_has_ac,
    )
}

fn vvc_luma_leaf_count(
    coded: VvcCodedGeometry,
    chroma_sampling: ChromaSampling,
    luma_max_leaf_size: u16,
) -> usize {
    let shape = VvcCtuPartitionShape {
        root_width: VVC_CTU_SIZE as u16,
        root_height: VVC_CTU_SIZE as u16,
        visible_width: coded.width as u16,
        visible_height: coded.height as u16,
        chroma_sampling,
    };
    vvc_luma_transform_nodes(shape, luma_max_leaf_size).len()
}

fn vvc_ctu_partition_cabac_bits(
    params: &VvcCtuPartitionParams,
    slice_config: VvcSliceSyntaxConfig,
) -> Vec<bool> {
    debug_assert!((8..=64).contains(&params.root_width));
    debug_assert!((8..=64).contains(&params.root_height));
    debug_assert!(params.visible_width >= 8 && params.visible_height >= 8);

    let mut cabac = VvcCabacEncoder::new();
    cabac.start();
    encode_ctu_partition_body(&mut cabac, params, slice_config);
    cabac.encode_bin_trm(true);
    cabac.finish()
}

struct VvcCtuCabacDump {
    symbols: Vec<VvcCabacDumpSymbol>,
    semantic_symbols: Vec<VvcCabacDumpSymbol>,
    context_events: Vec<VvcCabacDumpContextEvent>,
    context_bin_count: usize,
    bin_engine_events: Vec<cabac::VvcCabacDumpBinEngineEvent>,
    bits: Vec<bool>,
}

fn vvc_ctu_partition_cabac_dump(
    params: &VvcCtuPartitionParams,
    slice_config: VvcSliceSyntaxConfig,
) -> VvcCtuCabacDump {
    debug_assert!((8..=64).contains(&params.root_width));
    debug_assert!((8..=64).contains(&params.root_height));
    debug_assert!(params.visible_width >= 8 && params.visible_height >= 8);

    let mut cabac = VvcCabacEncoder::new_with_dump();
    cabac.start();
    encode_ctu_partition_body(&mut cabac, params, slice_config);
    cabac.encode_bin_trm(true);
    let semantic_symbols = cabac.semantic_symbols.clone();
    let context_events = cabac.context_events.clone();
    let context_bin_count = cabac.context_bin_count;
    let bin_engine_events = cabac.bin_engine_events.clone();
    let symbols = cabac.dump_symbols.clone();
    let bits = cabac.finish();
    VvcCtuCabacDump {
        symbols,
        semantic_symbols,
        context_events,
        context_bin_count,
        bin_engine_events,
        bits,
    }
}

fn vvc_cabac_vector_dump_json(
    geometry: VvcVideoGeometry,
    params: &VvcCtuPartitionParams,
    symbols: &[VvcCabacDumpSymbol],
    semantic_symbols: &[VvcCabacDumpSymbol],
    context_events: &[VvcCabacDumpContextEvent],
    bin_engine_events: &[cabac::VvcCabacDumpBinEngineEvent],
    bits: &[bool],
) -> String {
    let mut json = String::new();
    json.push_str("{\"kind\":\"frameforge.vvc.cabac_vector.v1\"");
    json.push_str(&format!(",\"width\":{}", geometry.width));
    json.push_str(&format!(",\"height\":{}", geometry.height));
    json.push_str(",\"format\":\"yuv420p8\"");
    json.push_str(&format!(
        ",\"luma_dc_abs_level\":{}",
        params.luma_tu_abs_levels[0]
    ));
    json.push_str(&format!(
        ",\"luma_dc_negative\":{}",
        if params.luma_tu_negative[0] {
            "true"
        } else {
            "false"
        }
    ));
    json.push_str(",\"luma_ac_levels\":[");
    for (idx, level) in params.luma_tu_ac_levels[0].iter().enumerate() {
        if idx != 0 {
            json.push(',');
        }
        json.push_str(&level.to_string());
    }
    json.push(']');
    json.push_str(&format!(",\"luma_tu_count\":{}", params.luma_tu_count));
    json.push_str(",\"luma_tu_abs_levels_all\":[");
    for idx in 0..params.luma_tu_count {
        if idx != 0 {
            json.push(',');
        }
        json.push_str(&params.luma_tu_abs_levels[idx].to_string());
    }
    json.push(']');
    json.push_str(",\"luma_tu_negative_all\":[");
    for idx in 0..params.luma_tu_count {
        if idx != 0 {
            json.push(',');
        }
        json.push_str(if params.luma_tu_negative[idx] {
            "true"
        } else {
            "false"
        });
    }
    json.push(']');
    json.push_str(",\"luma_tu_ac_levels_all\":[");
    for tu_idx in 0..params.luma_tu_count {
        if tu_idx != 0 {
            json.push(',');
        }
        json.push('[');
        for (idx, level) in params.luma_tu_ac_levels[tu_idx].iter().enumerate() {
            if idx != 0 {
                json.push(',');
            }
            json.push_str(&level.to_string());
        }
        json.push(']');
    }
    json.push(']');
    json.push_str(&format!(",\"cb_dc_abs_level\":{}", params.cb_dc_abs_level));
    json.push_str(&format!(
        ",\"cb_dc_negative\":{}",
        if params.cb_dc_negative {
            "true"
        } else {
            "false"
        }
    ));
    json.push_str(&format!(
        ",\"cb_tu_dc_level\":{}",
        params.cb_tu_dc_levels[0]
    ));
    json.push_str(&format!(
        ",\"cr_tu_dc_level\":{}",
        params.cr_tu_dc_levels[0]
    ));
    json.push_str(",\"cb_tu_ac_levels\":[");
    for (idx, level) in params.cb_tu_ac_levels[0].iter().enumerate() {
        if idx != 0 {
            json.push(',');
        }
        json.push_str(&level.to_string());
    }
    json.push(']');
    json.push_str(",\"cr_tu_ac_levels\":[");
    for (idx, level) in params.cr_tu_ac_levels[0].iter().enumerate() {
        if idx != 0 {
            json.push(',');
        }
        json.push_str(&level.to_string());
    }
    json.push(']');
    json.push_str(&format!(",\"chroma_tu_count\":{}", params.chroma_tu_count));
    json.push_str(",\"cb_tu_dc_levels_all\":[");
    for idx in 0..params.chroma_tu_count {
        if idx != 0 {
            json.push(',');
        }
        json.push_str(&params.cb_tu_dc_levels[idx].to_string());
    }
    json.push(']');
    json.push_str(",\"cr_tu_dc_levels_all\":[");
    for idx in 0..params.chroma_tu_count {
        if idx != 0 {
            json.push(',');
        }
        json.push_str(&params.cr_tu_dc_levels[idx].to_string());
    }
    json.push(']');
    json.push_str(",\"cb_tu_ac_levels_all\":[");
    for tu_idx in 0..params.chroma_tu_count {
        if tu_idx != 0 {
            json.push(',');
        }
        json.push('[');
        for (idx, level) in params.cb_tu_ac_levels[tu_idx].iter().enumerate() {
            if idx != 0 {
                json.push(',');
            }
            json.push_str(&level.to_string());
        }
        json.push(']');
    }
    json.push(']');
    json.push_str(",\"cr_tu_ac_levels_all\":[");
    for tu_idx in 0..params.chroma_tu_count {
        if tu_idx != 0 {
            json.push(',');
        }
        json.push('[');
        for (idx, level) in params.cr_tu_ac_levels[tu_idx].iter().enumerate() {
            if idx != 0 {
                json.push(',');
            }
            json.push_str(&level.to_string());
        }
        json.push(']');
    }
    json.push(']');
    json.push_str(",\"symbol_record_bytes\":5");
    json.push_str(",\"context_id_bits\":10");
    json.push_str(",\"symbol_encoding\":\"kind_u8_data_u32be_hex\"");
    json.push_str(&format!(
        ",\"mapped_context_bin_count\":{}",
        context_events.len()
    ));
    json.push_str(&format!(",\"cabac_bit_len\":{}", bits.len()));
    json.push_str(",\"cabac_bytes_hex\":\"");
    append_hex_bytes(&mut json, bits);
    json.push_str("\",\"symbols_hex\":\"");
    append_symbol_records_hex(&mut json, symbols);
    json.push_str("\",\"semantic_symbols_hex\":\"");
    append_symbol_records_hex(&mut json, semantic_symbols);
    json.push_str("\",\"context_event_record_bytes\":8");
    json.push_str(
        ",\"context_event_encoding\":\"ctx_id_u16be_bin_u8_range_u16be_lps_u16be_mps_u8_hex\"",
    );
    json.push_str(",\"context_events_hex\":\"");
    append_context_event_records_hex(&mut json, context_events);
    json.push_str("\",\"bin_engine_event_record_bytes\":20");
    json.push_str(",\"bin_engine_event_encoding\":\"kind_u8_bin_u8_lps_u16be_mps_u8_low_in_u32be_range_in_u16be_bits_left_in_u8_low_out_u32be_range_out_u16be_bits_left_out_u8_write_out_u8_hex\"");
    json.push_str(",\"bin_engine_events_hex\":\"");
    append_bin_engine_event_records_hex(&mut json, bin_engine_events);
    json.push_str("\"}\n");
    json
}

fn append_bin_engine_event_records_hex(
    out: &mut String,
    events: &[cabac::VvcCabacDumpBinEngineEvent],
) {
    for event in events {
        append_byte_hex(out, event.kind);
        append_byte_hex(out, u8::from(event.bin));
        for byte in event.lps.to_be_bytes() {
            append_byte_hex(out, byte);
        }
        append_byte_hex(out, u8::from(event.mps));
        for byte in event.low_in.to_be_bytes() {
            append_byte_hex(out, byte);
        }
        for byte in event.range_in.to_be_bytes() {
            append_byte_hex(out, byte);
        }
        append_byte_hex(out, event.bits_left_in);
        for byte in event.low_out.to_be_bytes() {
            append_byte_hex(out, byte);
        }
        for byte in event.range_out.to_be_bytes() {
            append_byte_hex(out, byte);
        }
        append_byte_hex(out, event.bits_left_out);
        append_byte_hex(out, u8::from(event.write_out));
    }
}

fn append_context_event_records_hex(out: &mut String, events: &[VvcCabacDumpContextEvent]) {
    for event in events {
        for byte in event.ctx_id.to_be_bytes() {
            append_byte_hex(out, byte);
        }
        append_byte_hex(out, u8::from(event.bin));
        for byte in event.range.to_be_bytes() {
            append_byte_hex(out, byte);
        }
        for byte in event.lps.to_be_bytes() {
            append_byte_hex(out, byte);
        }
        append_byte_hex(out, u8::from(event.mps));
    }
}

fn append_hex_bytes(out: &mut String, bits: &[bool]) {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    for chunk in bits.chunks(8) {
        let mut byte = 0u8;
        for bit in chunk {
            byte = (byte << 1) | u8::from(*bit);
        }
        byte <<= 8 - chunk.len();
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0f) as usize] as char);
    }
}

fn append_symbol_records_hex(out: &mut String, symbols: &[VvcCabacDumpSymbol]) {
    for symbol in symbols {
        append_byte_hex(out, symbol.kind);
        for byte in symbol.data.to_be_bytes() {
            append_byte_hex(out, byte);
        }
    }
}

fn append_byte_hex(out: &mut String, byte: u8) {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    out.push(HEX[(byte >> 4) as usize] as char);
    out.push(HEX[(byte & 0x0f) as usize] as char);
}

#[cfg(test)]
mod tests;
