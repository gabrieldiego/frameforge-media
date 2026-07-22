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
    chroma_subsample_y as planar_chroma_subsample_y, pack_planar_samples, read_input_frame,
    unpack_planar_samples, ChromaSampling, FrameLimit, Picture, PixelFormat, PlanarYuvFrameLayout,
    PlanarYuvGeometry, SampleBitDepth,
};

mod cabac;
mod header;
mod ibc;
mod nal;
mod palette;
mod residual;
mod syntax;
use cabac::{
    encode_ctu_partition_body, encode_frame_partition_body_with_contexts,
    initial_vvc_cabac_contexts, vvc_chroma_transform_nodes, vvc_luma_transform_nodes,
    VvcCabacContext, VvcCabacContexts, VvcCabacDumpContextEvent, VvcCabacDumpSymbol,
    VvcCabacEncoder, VvcCodingTreeNode, VvcCtuCabacOp, VvcCtuPartitionParams, VvcCtuPartitionShape,
    VvcLastSigCoeffPrefixCtxInput, VvcPartSplit,
};
#[cfg(test)]
use cabac::{VvcCtuCabacGenerator, VvcQtSplitCtxInput, VvcSplitCtxInput, VvcTreeType};
use header::{
    vvc_frame_slice_unit, vvc_picture_ctu_cols, vvc_picture_ctu_count, vvc_picture_ctu_rows,
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
    quantize_vvc_frame, quantize_vvc_residual_ctu_into_frame_reconstruction_with_qp,
    VvcPlaneAvailability, VvcQuantizedColor, VvcResidualCabacOptions, VvcResidualComponent,
    MAX_VVC_CHROMA_TUS, MAX_VVC_LUMA_TUS, VVC_CHROMA_AC_COEFFS_PER_TU, VVC_DEFAULT_LOSSY_CHROMA_QP,
    VVC_DEFAULT_LOSSY_LUMA_QP, VVC_LUMA_AC_COEFFS_PER_TU,
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
    pub qp: Option<u8>,
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
const VVC_STATS_ENV: &str = "FRAMEFORGE_VVC_STATS";
#[cfg(feature = "vvc-stats")]
const VVC_CTU_BITS_ENV: &str = "FRAMEFORGE_VVC_CTU_BITS";

#[cfg(feature = "vvc-stats")]
impl VvcStatsSink {
    fn from_env() -> Result<Self, String> {
        Ok(Self {
            sink: JsonlInstrumentationSink::append_from_env(VVC_STATS_ENV)
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
    slice_qp: i32,
    chroma_qp: i32,
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
        slice_qp: i32,
        chroma_qp: i32,
    ) -> Self {
        Self {
            frame_idx,
            width: geometry.width,
            height: geometry.height,
            chroma_sampling: format.chroma_sampling,
            bit_depth: format.bit_depth,
            lossless,
            slice_qp,
            chroma_qp,
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
        self.add_counter_named(name, value);
    }

    fn add_counter_named(&mut self, name: &str, value: u64) {
        if let Some(counter) = self
            .counters
            .iter_mut()
            .find(|counter| counter.name == name)
        {
            counter.value += value;
        } else {
            self.counters.push(VvcCounterStats {
                name: name.to_owned(),
                value,
            });
        }
    }

    fn to_json_line(&self) -> String {
        let mut json = format!(
            "{{\"kind\":\"frameforge.vvc.stats.v1\",\"frame_index\":{},\"width\":{},\"height\":{},\"chroma_sampling\":\"{:?}\",\"bit_depth\":{},\"lossless\":{},\"slice_qp\":{},\"chroma_qp\":{},\"ctu_count\":{},\"bitstream_bytes\":{},\"stages\":[",
            self.frame_idx,
            self.width,
            self.height,
            self.chroma_sampling,
            self.bit_depth.bits(),
            self.lossless,
            self.slice_qp,
            self.chroma_qp,
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
    name: String,
    value: u64,
}

#[cfg(feature = "vvc-stats")]
struct VvcCtuBitSink {
    sink: Option<JsonlInstrumentationSink>,
}

#[cfg(feature = "vvc-stats")]
impl VvcCtuBitSink {
    fn from_env() -> Result<Self, String> {
        Ok(Self {
            sink: JsonlInstrumentationSink::append_from_env(VVC_CTU_BITS_ENV)
                .map_err(|err| err.to_string())?,
        })
    }

    fn is_enabled(&self) -> bool {
        self.sink.is_some()
    }

    fn write_ctu(
        &mut self,
        frame_idx: usize,
        region: VvcCtuRegion,
        format: VvcPictureFormat,
        lossless: bool,
        slice_qp: i32,
        chroma_qp: i32,
        quantized: &VvcQuantizedColor,
        luma_max_leaf_size: u16,
        slice_config: VvcSliceSyntaxConfig,
    ) -> Result<(), String> {
        let Some(sink) = self.sink.as_mut() else {
            return Ok(());
        };
        let Some(params) = vvc_ctu_partition_params_with_luma_max_leaf_size_and_chroma(
            region.geometry,
            quantized.clone(),
            luma_max_leaf_size,
            slice_config.coding_tree.chroma_sampling,
            slice_config.coding_tree.dual_tree_intra,
        ) else {
            return Ok(());
        };
        let dump = vvc_ctu_partition_cabac_dump(&params, slice_config);
        let luma_modes = vvc_luma_mode_counts(quantized);
        let chroma_modes = vvc_chroma_mode_counts(quantized);
        let residual_coding = vvc_tu_residual_coding_counts(quantized);
        let search = quantized.intra_search_stats;
        let line = format!(
            "{{\"codec\":\"vvc\",\"source\":\"frameforge\",\"path\":\"residual_ctu\",\"frame_index\":{},\"ctu_address\":{},\"sb_x\":{},\"sb_y\":{},\"x\":{},\"y\":{},\"width\":{},\"height\":{},\"superblock_size\":{},\"chroma_sampling\":\"{:?}\",\"bit_depth\":{},\"lossless\":{},\"slice_qp\":{},\"chroma_qp\":{},\"luma_tu_count\":{},\"chroma_tu_count\":{},\"luma_tu_transform_skip_count\":{},\"luma_tu_transformed_count\":{},\"cb_tu_transform_skip_count\":{},\"cb_tu_transformed_count\":{},\"cr_tu_transform_skip_count\":{},\"cr_tu_transformed_count\":{},\"chroma_tu_transform_skip_count\":{},\"chroma_tu_transformed_count\":{},\"luma_candidate_count\":{},\"luma_candidate_dc\":{},\"luma_candidate_planar\":{},\"luma_candidate_directional\":{},\"luma_candidate_directional_coarse\":{},\"luma_candidate_directional_refinement\":{},\"chroma_candidate_count\":{},\"chroma_candidate_derived\":{},\"chroma_candidate_explicit\":{},\"chroma_candidate_cclm\":{},\"luma_mode_dc\":{},\"luma_mode_planar\":{},\"luma_mode_horizontal\":{},\"luma_mode_vertical\":{},\"luma_mode_angular\":{},\"chroma_mode_derived\":{},\"chroma_mode_dc\":{},\"chroma_mode_planar\":{},\"chroma_mode_horizontal\":{},\"chroma_mode_vertical\":{},\"chroma_mode_angular\":{},\"chroma_mode_cclm\":{},\"chroma_mode_cclm_linear\":{},\"chroma_mode_mdlm_left\":{},\"chroma_mode_mdlm_top\":{},\"context_bins\":{},\"semantic_symbols\":{},\"bin_engine_events\":{},\"total_symbol_bits\":{}}}",
            frame_idx,
            region.slice_address,
            region.origin_x / VVC_CTU_SIZE,
            region.origin_y / VVC_CTU_SIZE,
            region.origin_x,
            region.origin_y,
            region.geometry.width,
            region.geometry.height,
            VVC_CTU_SIZE,
            format.chroma_sampling,
            format.bit_depth.bits(),
            lossless,
            slice_qp,
            chroma_qp,
            quantized.luma_tu_count,
            quantized.chroma_tu_count,
            residual_coding.luma_transform_skip,
            residual_coding.luma_transformed,
            residual_coding.cb_transform_skip,
            residual_coding.cb_transformed,
            residual_coding.cr_transform_skip,
            residual_coding.cr_transformed,
            residual_coding.chroma_transform_skip(),
            residual_coding.chroma_transformed(),
            search.luma_candidates(),
            search.luma_dc_candidates,
            search.luma_planar_candidates,
            search.luma_directional_candidates(),
            search.luma_directional_coarse_candidates,
            search.luma_directional_refinement_candidates,
            search.chroma_candidates(),
            search.chroma_derived_candidates,
            search.chroma_explicit_candidates,
            search.chroma_cclm_candidates,
            luma_modes.dc,
            luma_modes.planar,
            luma_modes.horizontal,
            luma_modes.vertical,
            luma_modes.angular,
            chroma_modes.derived,
            chroma_modes.dc,
            chroma_modes.planar,
            chroma_modes.horizontal,
            chroma_modes.vertical,
            chroma_modes.angular,
            chroma_modes.cclm,
            chroma_modes.cclm_linear,
            chroma_modes.mdlm_left,
            chroma_modes.mdlm_top,
            dump.context_bin_count,
            dump.semantic_symbols.len(),
            dump.bin_engine_events.len(),
            dump.bits.len(),
        );
        sink.write_json_line(&line).map_err(|err| err.to_string())?;
        sink.flush().map_err(|err| err.to_string())
    }
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
    let residual_coding = vvc_tu_residual_coding_counts(quantized);
    stats.add_counter(
        "luma_tu_transform_skip_count",
        residual_coding.luma_transform_skip as u64,
    );
    stats.add_counter(
        "luma_tu_transformed_count",
        residual_coding.luma_transformed as u64,
    );
    stats.add_counter(
        "cb_tu_transform_skip_count",
        residual_coding.cb_transform_skip as u64,
    );
    stats.add_counter(
        "cb_tu_transformed_count",
        residual_coding.cb_transformed as u64,
    );
    stats.add_counter(
        "cr_tu_transform_skip_count",
        residual_coding.cr_transform_skip as u64,
    );
    stats.add_counter(
        "cr_tu_transformed_count",
        residual_coding.cr_transformed as u64,
    );
    stats.add_counter(
        "chroma_tu_transform_skip_count",
        residual_coding.chroma_transform_skip() as u64,
    );
    stats.add_counter(
        "chroma_tu_transformed_count",
        residual_coding.chroma_transformed() as u64,
    );
    let search = quantized.intra_search_stats;
    stats.add_counter("luma_candidate_count", search.luma_candidates() as u64);
    stats.add_counter("luma_candidate_dc", search.luma_dc_candidates as u64);
    stats.add_counter(
        "luma_candidate_planar",
        search.luma_planar_candidates as u64,
    );
    stats.add_counter(
        "luma_candidate_directional",
        search.luma_directional_candidates() as u64,
    );
    stats.add_counter(
        "luma_candidate_directional_coarse",
        search.luma_directional_coarse_candidates as u64,
    );
    stats.add_counter(
        "luma_candidate_directional_refinement",
        search.luma_directional_refinement_candidates as u64,
    );
    stats.add_counter("chroma_candidate_count", search.chroma_candidates() as u64);
    stats.add_counter(
        "chroma_candidate_derived",
        search.chroma_derived_candidates as u64,
    );
    stats.add_counter(
        "chroma_candidate_explicit",
        search.chroma_explicit_candidates as u64,
    );
    stats.add_counter(
        "chroma_candidate_cclm",
        search.chroma_cclm_candidates as u64,
    );
    let modes = vvc_luma_mode_counts(quantized);
    stats.add_counter("luma_mode_dc", modes.dc as u64);
    stats.add_counter("luma_mode_planar", modes.planar as u64);
    stats.add_counter("luma_mode_horizontal", modes.horizontal as u64);
    stats.add_counter("luma_mode_vertical", modes.vertical as u64);
    stats.add_counter("luma_mode_angular", modes.angular as u64);
    add_vvc_mode_index_counters(stats, "luma_mode_angular_", &modes.angular_by_index);
    let chroma_modes = vvc_chroma_mode_counts(quantized);
    stats.add_counter("chroma_mode_derived", chroma_modes.derived as u64);
    stats.add_counter("chroma_mode_dc", chroma_modes.dc as u64);
    stats.add_counter("chroma_mode_planar", chroma_modes.planar as u64);
    stats.add_counter("chroma_mode_horizontal", chroma_modes.horizontal as u64);
    stats.add_counter("chroma_mode_vertical", chroma_modes.vertical as u64);
    stats.add_counter("chroma_mode_angular", chroma_modes.angular as u64);
    add_vvc_mode_index_counters(
        stats,
        "chroma_mode_angular_",
        &chroma_modes.angular_by_index,
    );
    stats.add_counter("chroma_mode_cclm", chroma_modes.cclm as u64);
    stats.add_counter("chroma_mode_cclm_linear", chroma_modes.cclm_linear as u64);
    stats.add_counter("chroma_mode_mdlm_left", chroma_modes.mdlm_left as u64);
    stats.add_counter("chroma_mode_mdlm_top", chroma_modes.mdlm_top as u64);
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

#[cfg(feature = "vvc-stats")]
#[derive(Debug, Default, Clone, Copy)]
struct VvcTuResidualCodingCounts {
    luma_transform_skip: usize,
    luma_transformed: usize,
    cb_transform_skip: usize,
    cb_transformed: usize,
    cr_transform_skip: usize,
    cr_transformed: usize,
}

#[cfg(feature = "vvc-stats")]
impl VvcTuResidualCodingCounts {
    const fn chroma_transform_skip(self) -> usize {
        self.cb_transform_skip + self.cr_transform_skip
    }

    const fn chroma_transformed(self) -> usize {
        self.cb_transformed + self.cr_transformed
    }
}

#[cfg(feature = "vvc-stats")]
fn vvc_tu_residual_coding_counts(quantized: &VvcQuantizedColor) -> VvcTuResidualCodingCounts {
    let mut counts = VvcTuResidualCodingCounts::default();
    for idx in 0..quantized.luma_tu_count {
        if quantized.luma_tu_transform_skip[idx] {
            counts.luma_transform_skip += 1;
        } else {
            counts.luma_transformed += 1;
        }
    }
    for idx in 0..quantized.chroma_tu_count {
        if quantized.cb_tu_transform_skip[idx] {
            counts.cb_transform_skip += 1;
        } else {
            counts.cb_transformed += 1;
        }
        if quantized.cr_tu_transform_skip[idx] {
            counts.cr_transform_skip += 1;
        } else {
            counts.cr_transformed += 1;
        }
    }
    counts
}

#[cfg(feature = "vvc-stats")]
fn add_vvc_mode_index_counters(stats: &mut VvcFrameStats, prefix: &str, counts: &[usize; 67]) {
    for (index, count) in counts.iter().copied().enumerate() {
        if count == 0 {
            continue;
        }
        stats.add_counter_named(&format!("{prefix}{index:02}"), count as u64);
    }
}

#[cfg(feature = "vvc-stats")]
#[derive(Debug, Clone, Copy)]
struct VvcLumaModeCounts {
    dc: usize,
    planar: usize,
    horizontal: usize,
    vertical: usize,
    angular: usize,
    angular_by_index: [usize; 67],
}

#[cfg(feature = "vvc-stats")]
impl Default for VvcLumaModeCounts {
    fn default() -> Self {
        Self {
            dc: 0,
            planar: 0,
            horizontal: 0,
            vertical: 0,
            angular: 0,
            angular_by_index: [0; 67],
        }
    }
}

#[cfg(feature = "vvc-stats")]
fn vvc_luma_mode_counts(quantized: &VvcQuantizedColor) -> VvcLumaModeCounts {
    let mut counts = VvcLumaModeCounts::default();
    for idx in 0..quantized.luma_tu_count {
        match quantized.luma_tu_intra_modes[idx] {
            VvcIntraPredictionMode::Dc => counts.dc += 1,
            VvcIntraPredictionMode::Planar => counts.planar += 1,
            VvcIntraPredictionMode::Horizontal => counts.horizontal += 1,
            VvcIntraPredictionMode::Vertical => counts.vertical += 1,
            VvcIntraPredictionMode::Angular(index) => {
                counts.angular += 1;
                counts.angular_by_index[usize::from(index)] += 1;
            }
        }
    }
    counts
}

#[cfg(feature = "vvc-stats")]
#[derive(Debug, Clone, Copy)]
struct VvcChromaModeCounts {
    derived: usize,
    dc: usize,
    planar: usize,
    horizontal: usize,
    vertical: usize,
    angular: usize,
    angular_by_index: [usize; 67],
    cclm: usize,
    cclm_linear: usize,
    mdlm_left: usize,
    mdlm_top: usize,
}

#[cfg(feature = "vvc-stats")]
impl Default for VvcChromaModeCounts {
    fn default() -> Self {
        Self {
            derived: 0,
            dc: 0,
            planar: 0,
            horizontal: 0,
            vertical: 0,
            angular: 0,
            angular_by_index: [0; 67],
            cclm: 0,
            cclm_linear: 0,
            mdlm_left: 0,
            mdlm_top: 0,
        }
    }
}

#[cfg(feature = "vvc-stats")]
fn vvc_chroma_mode_counts(quantized: &VvcQuantizedColor) -> VvcChromaModeCounts {
    let mut counts = VvcChromaModeCounts::default();
    for idx in 0..quantized.chroma_tu_count {
        match quantized.chroma_tu_intra_modes[idx] {
            VvcChromaIntraPredictionMode::Derived => counts.derived += 1,
            VvcChromaIntraPredictionMode::Explicit(VvcIntraPredictionMode::Dc) => counts.dc += 1,
            VvcChromaIntraPredictionMode::Explicit(VvcIntraPredictionMode::Planar) => {
                counts.planar += 1
            }
            VvcChromaIntraPredictionMode::Explicit(VvcIntraPredictionMode::Horizontal) => {
                counts.horizontal += 1
            }
            VvcChromaIntraPredictionMode::Explicit(VvcIntraPredictionMode::Vertical) => {
                counts.vertical += 1
            }
            VvcChromaIntraPredictionMode::Explicit(VvcIntraPredictionMode::Angular(index)) => {
                counts.angular += 1;
                counts.angular_by_index[usize::from(index)] += 1;
            }
            VvcChromaIntraPredictionMode::Cclm(mode) => {
                counts.cclm += 1;
                match mode {
                    VvcChromaCclmMode::Linear => counts.cclm_linear += 1,
                    VvcChromaCclmMode::MdlmLeft => counts.mdlm_left += 1,
                    VvcChromaCclmMode::MdlmTop => counts.mdlm_top += 1,
                }
            }
        }
    }
    counts
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct VvcReconstructionFrame {
    geometry: VvcVideoGeometry,
    format: VvcPictureFormat,
    luma: Vec<VvcSample>,
    cb: Vec<VvcSample>,
    cr: Vec<VvcSample>,
    luma_available: Vec<bool>,
    cb_available: Vec<bool>,
    cr_available: Vec<bool>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct VvcPictureFormat {
    chroma_sampling: ChromaSampling,
    bit_depth: SampleBitDepth,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct VvcCodingTreeConfig {
    chroma_sampling: ChromaSampling,
    dual_tree_intra: bool,
}

impl VvcCodingTreeConfig {
    const fn yuv(chroma_sampling: ChromaSampling) -> Self {
        Self {
            chroma_sampling,
            dual_tree_intra: true,
        }
    }

    const fn single_tree_444() -> Self {
        Self {
            chroma_sampling: ChromaSampling::Cs444,
            dual_tree_intra: false,
        }
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
    const fn residual(
        chroma_sampling: ChromaSampling,
        residual_mode: VvcResidualCodingMode,
    ) -> Self {
        Self {
            ibc_enabled: false,
            palette_enabled: false,
            transform_skip_enabled: residual_mode.is_lossless(),
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

    const fn without_unsupported_chroma_tools(self, _chroma_sampling: ChromaSampling) -> Self {
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

    #[cfg(test)]
    const fn yuv420_residual() -> Self {
        Self::residual_lossy(ChromaSampling::Cs420)
    }

    #[cfg(test)]
    const fn residual_lossy(chroma_sampling: ChromaSampling) -> Self {
        Self::residual(chroma_sampling, VvcResidualCodingMode::Lossy)
    }

    #[cfg(test)]
    fn residual_lossless(chroma_sampling: ChromaSampling, bit_depth: SampleBitDepth) -> Self {
        let mut config = Self::residual(chroma_sampling, VvcResidualCodingMode::Lossless);
        config.slice_qp = vvc_lossless_slice_qp(bit_depth);
        config
    }

    const fn residual(
        chroma_sampling: ChromaSampling,
        residual_mode: VvcResidualCodingMode,
    ) -> Self {
        Self::new(
            VvcCodingTreeConfig::yuv(chroma_sampling),
            VvcSyntaxToolFlags::residual(chroma_sampling, residual_mode),
        )
    }

    const fn palette_444() -> Self {
        let mut config = Self::new(
            VvcCodingTreeConfig::single_tree_444(),
            VvcSyntaxToolFlags::palette_444(),
        );
        config.slice_qp = VVC_PALETTE_DEFAULT_SLICE_QP;
        config
    }

    #[cfg(test)]
    const fn palette_444_lossless(bit_depth: SampleBitDepth) -> Self {
        let mut config = Self::palette_444();
        config.slice_qp = vvc_palette_lossless_slice_qp(bit_depth);
        config
    }

    const fn for_picture_format(format: VvcPictureFormat) -> Self {
        Self::residual(format.chroma_sampling, VvcResidualCodingMode::Lossy)
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::vvc) enum VvcResidualCodingMode {
    Lossy,
    Lossless,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::vvc) enum VvcTuResidualCodingMode {
    Transformed,
    TransformSkip,
}

impl VvcResidualCodingMode {
    const fn for_encode_options(options: VvcEncodeOptions) -> Self {
        match options.lossless {
            true => Self::Lossless,
            false => Self::Lossy,
        }
    }

    fn slice_config(self, stream_format: VvcPictureFormat, qp: Option<u8>) -> VvcSliceSyntaxConfig {
        let mut config = VvcSliceSyntaxConfig::residual(stream_format.chroma_sampling, self);
        if self.is_lossless() {
            config.slice_qp = vvc_lossless_slice_qp(stream_format.bit_depth);
        } else {
            config.slice_qp = vvc_lossy_slice_qp(qp);
        }
        config
    }

    const fn picture_partitioning(self) -> VvcPicturePartitioning {
        match self {
            Self::Lossy | Self::Lossless => VvcPicturePartitioning::SingleSlice,
        }
    }

    const fn is_lossless(self) -> bool {
        matches!(self, Self::Lossless)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::vvc) enum VvcIntraPredictionMode {
    Planar,
    Dc,
    Horizontal,
    Vertical,
    #[allow(dead_code)]
    Angular(u8),
}

impl VvcIntraPredictionMode {
    const fn luma_mode_index(self) -> u8 {
        match self {
            Self::Planar => 0,
            Self::Dc => 1,
            Self::Horizontal => 18,
            Self::Vertical => 50,
            Self::Angular(index) => index,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::vvc) enum VvcChromaIntraPredictionMode {
    Derived,
    Explicit(VvcIntraPredictionMode),
    Cclm(VvcChromaCclmMode),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::vvc) enum VvcChromaCclmMode {
    Linear,
    MdlmLeft,
    MdlmTop,
}

pub(in crate::vvc) const fn vvc_luma_intra_mode_from_index(index: u8) -> VvcIntraPredictionMode {
    match index {
        18 => VvcIntraPredictionMode::Horizontal,
        50 => VvcIntraPredictionMode::Vertical,
        _ => VvcIntraPredictionMode::Angular(index),
    }
}

const VVC_CHROMA_EXPLICIT_MODE_COUNT: usize = 4;
const VVC_CHROMA_VDIA_REPLACEMENT_MODE: VvcIntraPredictionMode =
    VvcIntraPredictionMode::Angular(66);

pub(in crate::vvc) fn vvc_chroma_explicit_candidates(
    co_located_luma_mode: VvcIntraPredictionMode,
) -> [VvcIntraPredictionMode; VVC_CHROMA_EXPLICIT_MODE_COUNT] {
    let mut modes = [
        VvcIntraPredictionMode::Planar,
        VvcIntraPredictionMode::Vertical,
        VvcIntraPredictionMode::Horizontal,
        VvcIntraPredictionMode::Dc,
    ];
    let luma_mode_index = co_located_luma_mode.luma_mode_index();
    let mut idx = 0;
    while idx < modes.len() {
        if modes[idx].luma_mode_index() == luma_mode_index {
            modes[idx] = VVC_CHROMA_VDIA_REPLACEMENT_MODE;
            break;
        }
        idx += 1;
    }
    modes
}

pub(in crate::vvc) fn vvc_chroma_explicit_candidate_index(
    mode: VvcIntraPredictionMode,
    co_located_luma_mode: VvcIntraPredictionMode,
) -> Option<u8> {
    let modes = vvc_chroma_explicit_candidates(co_located_luma_mode);
    modes
        .iter()
        .position(|candidate| candidate.luma_mode_index() == mode.luma_mode_index())
        .map(|index| index as u8)
}

pub(in crate::vvc) fn vvc_residual_chroma_explicit_candidate_allowed(
    mode: VvcIntraPredictionMode,
) -> bool {
    match mode {
        VvcIntraPredictionMode::Planar
        | VvcIntraPredictionMode::Horizontal
        | VvcIntraPredictionMode::Vertical => true,
        VvcIntraPredictionMode::Dc => true,
        VvcIntraPredictionMode::Angular(index) => (2..=66).contains(&index),
    }
}

pub(in crate::vvc) fn vvc_chroma_cclm_node_allowed(node: VvcCodingTreeNode) -> bool {
    // H.266 CodingUnit::checkCCLMAllowed allows CCLM on this dual-tree,
    // CTU-size subset for unsplit 64x64 chroma nodes, nodes below a root QT
    // split, root HBT 64x32 nodes, and root HBT followed by VBT.
    (node.width == 64 && node.height == 64 && node.cqt_depth == 0 && node.mtt_depth == 0)
        || node.cqt_depth > 0
        || (node.split_history[0] == VvcPartSplit::HorizontalBinary
            && node.width == 64
            && node.height == 32)
        || (node.split_history[0] == VvcPartSplit::HorizontalBinary
            && node.split_history[1] == VvcPartSplit::VerticalBinary)
}

pub(in crate::vvc) fn vvc_residual_chroma_cclm_candidate_allowed(
    context: VvcResidualModeDecisionContext,
    node: VvcCodingTreeNode,
    geometry: VvcVideoGeometry,
) -> bool {
    let _chroma_sampling = context.chroma_sampling();
    if !vvc_chroma_cclm_node_allowed(node) {
        return false;
    }
    node.fits_visible(
        geometry.coded_width() as u16,
        geometry.coded_height() as u16,
    )
}

const VVC_LUMA_INTRA_CANDIDATE_CAPACITY: usize = 67;
const VVC_CHROMA_INTRA_CANDIDATE_CAPACITY: usize = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::vvc) struct VvcLumaIntraCandidateCost {
    mode: VvcIntraPredictionMode,
    score: u64,
}

impl VvcLumaIntraCandidateCost {
    const fn new(mode: VvcIntraPredictionMode, score: u64) -> Self {
        Self { mode, score }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::vvc) struct VvcLumaIntraCandidateCosts {
    candidates: [VvcLumaIntraCandidateCost; VVC_LUMA_INTRA_CANDIDATE_CAPACITY],
    count: usize,
}

impl VvcLumaIntraCandidateCosts {
    pub(in crate::vvc) const fn new(dc_score: u64) -> Self {
        Self {
            candidates: [VvcLumaIntraCandidateCost::new(VvcIntraPredictionMode::Dc, 0);
                VVC_LUMA_INTRA_CANDIDATE_CAPACITY],
            count: 1,
        }
        .with_required_candidate(VvcIntraPredictionMode::Dc, dc_score)
    }

    const fn with_required_candidate(mut self, mode: VvcIntraPredictionMode, score: u64) -> Self {
        self.candidates[self.count - 1] = VvcLumaIntraCandidateCost::new(mode, score);
        self
    }

    pub(in crate::vvc) fn with_candidate(
        mut self,
        mode: VvcIntraPredictionMode,
        score: Option<u64>,
    ) -> Self {
        if let Some(score) = score {
            assert!(self.count < self.candidates.len());
            self.candidates[self.count] = VvcLumaIntraCandidateCost::new(mode, score);
            self.count += 1;
        }
        self
    }

    fn iter(self) -> impl Iterator<Item = VvcLumaIntraCandidateCost> {
        self.candidates.into_iter().take(self.count)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::vvc) struct VvcChromaIntraCandidateCost {
    mode: VvcChromaIntraPredictionMode,
    score: u64,
}

impl VvcChromaIntraCandidateCost {
    const fn new(mode: VvcChromaIntraPredictionMode, score: u64) -> Self {
        Self { mode, score }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::vvc) struct VvcChromaIntraCandidateCosts {
    candidates: [VvcChromaIntraCandidateCost; VVC_CHROMA_INTRA_CANDIDATE_CAPACITY],
    count: usize,
}

impl VvcChromaIntraCandidateCosts {
    pub(in crate::vvc) const fn new(derived_score: u64) -> Self {
        Self {
            candidates: [VvcChromaIntraCandidateCost::new(VvcChromaIntraPredictionMode::Derived, 0);
                VVC_CHROMA_INTRA_CANDIDATE_CAPACITY],
            count: 1,
        }
        .with_required_candidate(VvcChromaIntraPredictionMode::Derived, derived_score)
    }

    const fn with_required_candidate(
        mut self,
        mode: VvcChromaIntraPredictionMode,
        score: u64,
    ) -> Self {
        self.candidates[self.count - 1] = VvcChromaIntraCandidateCost::new(mode, score);
        self
    }

    pub(in crate::vvc) fn with_candidate(
        mut self,
        mode: VvcChromaIntraPredictionMode,
        score: Option<u64>,
    ) -> Self {
        if let Some(score) = score {
            assert!(self.count < self.candidates.len());
            self.candidates[self.count] = VvcChromaIntraCandidateCost::new(mode, score);
            self.count += 1;
        }
        self
    }

    fn iter(self) -> impl Iterator<Item = VvcChromaIntraCandidateCost> {
        self.candidates.into_iter().take(self.count)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::vvc) struct VvcResidualModeDecisionContext {
    format: VvcPictureFormat,
    residual_mode: VvcResidualCodingMode,
}

impl VvcResidualModeDecisionContext {
    pub(in crate::vvc) const fn new(
        format: VvcPictureFormat,
        residual_mode: VvcResidualCodingMode,
    ) -> Self {
        Self {
            format,
            residual_mode,
        }
    }

    const fn chroma_sampling(self) -> ChromaSampling {
        self.format.chroma_sampling
    }

    const fn bit_depth(self) -> SampleBitDepth {
        self.format.bit_depth
    }

    const fn is_lossless(self) -> bool {
        self.residual_mode.is_lossless()
    }

    pub(in crate::vvc) const fn residual_mode(self) -> VvcResidualCodingMode {
        self.residual_mode
    }
}

pub(in crate::vvc) fn select_vvc_residual_luma_intra_mode(
    context: VvcResidualModeDecisionContext,
    node: VvcCodingTreeNode,
    costs: VvcLumaIntraCandidateCosts,
) -> VvcIntraPredictionMode {
    let _selector_scope = (
        context.chroma_sampling(),
        context.bit_depth(),
        node.width,
        node.height,
    );
    let mut best_mode = VvcIntraPredictionMode::Dc;
    let mut best_score = u64::MAX;
    for candidate in costs.iter() {
        if candidate.score < best_score {
            best_score = candidate.score;
            best_mode = candidate.mode;
        }
    }
    best_mode
}

pub(in crate::vvc) fn select_vvc_luma_max_leaf_size(
    context: VvcResidualModeDecisionContext,
) -> u16 {
    match context.residual_mode() {
        VvcResidualCodingMode::Lossy => VVC_CURRENT_MAX_LUMA_LEAF_SIZE,
        VvcResidualCodingMode::Lossless => VVC_LOSSLESS_LUMA_LEAF_SIZE,
    }
}

pub(in crate::vvc) fn select_vvc_luma_tu_residual_coding(
    context: VvcResidualModeDecisionContext,
    node: VvcCodingTreeNode,
    _mode: VvcIntraPredictionMode,
) -> VvcTuResidualCodingMode {
    let _selector_scope = (
        context.chroma_sampling(),
        context.bit_depth(),
        node.width,
        node.height,
    );
    match context.residual_mode() {
        VvcResidualCodingMode::Lossless => VvcTuResidualCodingMode::TransformSkip,
        VvcResidualCodingMode::Lossy => VvcTuResidualCodingMode::Transformed,
    }
}

pub(in crate::vvc) fn vvc_residual_luma_planar_candidate_allowed(
    _context: VvcResidualModeDecisionContext,
    node: VvcCodingTreeNode,
) -> bool {
    node.width >= 4
        && node.height >= 4
        && node.width.is_power_of_two()
        && node.height.is_power_of_two()
}

pub(in crate::vvc) fn vvc_residual_luma_directional_candidate_allowed(
    _context: VvcResidualModeDecisionContext,
    node: VvcCodingTreeNode,
) -> bool {
    node.width >= 4
        && node.height >= 4
        && node.width.is_power_of_two()
        && node.height.is_power_of_two()
}

#[cfg(test)]
pub(in crate::vvc) fn select_vvc_residual_chroma_intra_mode(
    context: VvcResidualModeDecisionContext,
    node: VvcCodingTreeNode,
) -> VvcChromaIntraPredictionMode {
    select_vvc_residual_chroma_intra_mode_from_costs(
        context,
        node,
        VvcChromaIntraCandidateCosts::new(0),
    )
}

pub(in crate::vvc) fn select_vvc_residual_chroma_intra_mode_from_costs(
    context: VvcResidualModeDecisionContext,
    node: VvcCodingTreeNode,
    costs: VvcChromaIntraCandidateCosts,
) -> VvcChromaIntraPredictionMode {
    let _candidate_scope = (
        context.chroma_sampling(),
        context.bit_depth(),
        context.is_lossless(),
        node.width,
        node.height,
    );
    let mut best_mode = VvcChromaIntraPredictionMode::Derived;
    let mut best_score = u64::MAX;
    for candidate in costs.iter() {
        if candidate.score < best_score {
            best_score = candidate.score;
            best_mode = candidate.mode;
        }
    }
    best_mode
}

pub(in crate::vvc) fn select_vvc_chroma_tu_residual_coding(
    context: VvcResidualModeDecisionContext,
    node: VvcCodingTreeNode,
    _mode: VvcChromaIntraPredictionMode,
) -> VvcTuResidualCodingMode {
    let _selector_scope = (
        context.chroma_sampling(),
        context.bit_depth(),
        context.is_lossless(),
        node.width,
        node.height,
    );
    match context.residual_mode() {
        VvcResidualCodingMode::Lossless => VvcTuResidualCodingMode::TransformSkip,
        VvcResidualCodingMode::Lossy => VvcTuResidualCodingMode::Transformed,
    }
}

fn vvc_lossless_slice_qp(bit_depth: SampleBitDepth) -> i32 {
    -((i32::from(bit_depth.bits()) - 8) * 6)
}

fn vvc_lossy_slice_qp(qp: Option<u8>) -> i32 {
    qp.map_or(VVC_DEFAULT_LOSSY_LUMA_QP, |qp| i32::from(qp).clamp(1, 63))
}

fn vvc_lossy_chroma_qp_for_slice_qp(slice_qp: i32) -> i32 {
    if slice_qp == VVC_DEFAULT_LOSSY_LUMA_QP {
        return VVC_DEFAULT_LOSSY_CHROMA_QP;
    }
    (slice_qp + (VVC_DEFAULT_LOSSY_CHROMA_QP - VVC_DEFAULT_LOSSY_LUMA_QP)).clamp(0, 63)
}

#[cfg(test)]
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
    let residual_mode = VvcResidualCodingMode::for_encode_options(options);
    let mode_context = VvcResidualModeDecisionContext::new(stream_format, residual_mode);
    let luma_max_leaf_size = select_vvc_luma_max_leaf_size(mode_context);
    let slice_config = vvc_slice_config_for_input_format(
        residual_mode.slice_config(stream_format, options.qp),
        format,
    );
    let luma_qp = slice_config.slice_qp.clamp(0, 63);
    let chroma_qp = vvc_lossy_chroma_qp_for_slice_qp(luma_qp);
    let picture_partitioning = residual_mode.picture_partitioning();
    write_annex_b_to(
        bitstream,
        &[
            vvc_sps_unit(geometry, slice_config, stream_format.bit_depth),
            vvc_pps_unit_with_partitioning(geometry, picture_partitioning),
        ],
    )?;

    #[cfg(feature = "vvc-stats")]
    let mut vvc_stats = VvcStatsSink::from_env()?;
    #[cfg(feature = "vvc-stats")]
    let mut vvc_ctu_bits = VvcCtuBitSink::from_env()?;

    let mut frame_buf = vec![0; frame_len];
    let mut frame_idx = 0usize;
    while frame_limit.should_read(frame_idx) {
        #[cfg(feature = "vvc-stats")]
        let mut frame_stats = VvcFrameStats::new(
            frame_idx,
            geometry,
            stream_format,
            options.lossless,
            slice_config.slice_qp,
            chroma_qp,
        );
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
            let frame_recon_yuv = {
                let mut frame_recon =
                    VvcReconstructionFrame::new_neutral(geometry, source_frame.format);
                let mut frame_ctus = Vec::with_capacity(vvc_picture_ctu_count(geometry));
                for region in vvc_ctu_regions(geometry) {
                    #[cfg(feature = "vvc-stats")]
                    let stage_start = Instant::now();
                    let quantized = quantize_vvc_residual_ctu_into_frame_reconstruction_with_qp(
                        &source_frame,
                        &mut frame_recon,
                        region,
                        residual_mode,
                        luma_qp,
                        chroma_qp,
                    );
                    #[cfg(feature = "vvc-stats")]
                    add_vvc_quantized_ctu_counters(&mut frame_stats, &quantized);
                    #[cfg(feature = "vvc-stats")]
                    if vvc_ctu_bits.is_enabled() {
                        vvc_ctu_bits.write_ctu(
                            frame_idx,
                            region,
                            stream_format,
                            options.lossless,
                            slice_config.slice_qp,
                            chroma_qp,
                            &quantized,
                            luma_max_leaf_size,
                            slice_config,
                        )?;
                    }
                    #[cfg(feature = "vvc-stats")]
                    frame_stats.add_elapsed("ctu_quantize", stage_start);
                    frame_ctus.push(VvcQuantizedCtu {
                        slice_address: region.slice_address,
                        geometry: region.geometry,
                        color: quantized,
                        luma_max_leaf_size,
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
            luma_available: vec![false; layout.luma_samples()],
            cb_available: vec![false; layout.chroma_samples()],
            cr_available: vec![false; layout.chroma_samples()],
        }
    }

    fn luma_availability(&self) -> VvcPlaneAvailability<'_> {
        VvcPlaneAvailability::new(&self.luma_available, self.geometry.width)
    }

    fn cb_availability(&self) -> VvcPlaneAvailability<'_> {
        VvcPlaneAvailability::new(&self.cb_available, self.chroma_width())
    }

    fn cr_availability(&self) -> VvcPlaneAvailability<'_> {
        VvcPlaneAvailability::new(&self.cr_available, self.chroma_width())
    }

    fn mark_luma_node_available(&mut self, node: VvcCodingTreeNode) {
        mark_vvc_plane_node_available(
            &mut self.luma_available,
            self.geometry.width,
            self.geometry.height,
            usize::from(node.x),
            usize::from(node.y),
            usize::from(node.width),
            usize::from(node.height),
        );
    }

    fn mark_chroma_node_available(&mut self, node: VvcCodingTreeNode) {
        let subsample_x = chroma_subsample_x(self.format.chroma_sampling);
        let subsample_y = chroma_subsample_y(self.format.chroma_sampling);
        let chroma_width = self.chroma_width();
        let chroma_height = self.chroma_height();
        let x = usize::from(node.x) / subsample_x;
        let y = usize::from(node.y) / subsample_y;
        let width = usize::from(node.width) / subsample_x;
        let height = usize::from(node.height) / subsample_y;
        mark_vvc_plane_node_available(
            &mut self.cb_available,
            chroma_width,
            chroma_height,
            x,
            y,
            width,
            height,
        );
        mark_vvc_plane_node_available(
            &mut self.cr_available,
            chroma_width,
            chroma_height,
            x,
            y,
            width,
            height,
        );
    }

    fn chroma_width(&self) -> usize {
        self.geometry.width / chroma_subsample_x(self.format.chroma_sampling)
    }

    fn chroma_height(&self) -> usize {
        self.geometry.height / chroma_subsample_y(self.format.chroma_sampling)
    }

    fn into_yuv(self) -> Vec<u8> {
        let layout = PlanarYuvFrameLayout::for_validated_shape(
            self.geometry.width,
            self.geometry.height,
            self.format.chroma_sampling,
            self.format.bit_depth,
        );
        let mut output = vec![0; layout.frame_len()];
        let (y_plane, cb_plane, cr_plane) = layout.plane_slices_mut(&mut output);
        pack_planar_samples(&self.luma, y_plane, self.format.bit_depth);
        pack_planar_samples(&self.cb, cb_plane, self.format.bit_depth);
        pack_planar_samples(&self.cr, cr_plane, self.format.bit_depth);
        output
    }
}

fn mark_vvc_plane_node_available(
    available: &mut [bool],
    plane_width: usize,
    plane_height: usize,
    start_x: usize,
    start_y: usize,
    width: usize,
    height: usize,
) {
    let end_x = (start_x + width).min(plane_width);
    let end_y = (start_y + height).min(plane_height);
    for y in start_y..end_y {
        let row = y * plane_width;
        for x in start_x..end_x {
            available[row + x] = true;
        }
    }
}

pub fn vvc_cabac_vector_dump_json(
    input: &[u8],
    params: VvcEncodeParams,
    geometry: VvcVideoGeometry,
    format: PixelFormat,
) -> Result<String, String> {
    let source_frame = sample_vvc_yuv_frame(input, params, geometry, format)?;
    let slice_config = VvcSliceSyntaxConfig::for_picture_format(source_frame.format);
    let color = quantize_vvc_frame(&source_frame);
    let params = vvc_ctu_partition_params_with_luma_max_leaf_size_and_chroma(
        source_frame.geometry,
        color,
        VVC_CURRENT_MAX_LUMA_LEAF_SIZE,
        slice_config.coding_tree.chroma_sampling,
        slice_config.coding_tree.dual_tree_intra,
    )
    .ok_or_else(|| {
        format!(
            "VVC CABAC vector dump has no generated CTU path for coded geometry {}x{}",
            source_frame.geometry.coded_width(),
            source_frame.geometry.coded_height()
        )
    })?;
    let dump = vvc_ctu_partition_cabac_dump(&params, slice_config);
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
    Ok(format_vvc_cabac_vector_dump_json(
        source_frame.geometry,
        format,
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
    let layout = PlanarYuvFrameLayout::new(
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
    let (y_plane, cb_plane, cr_plane) = layout.plane_slices(frame);
    unpack_planar_samples(y_plane, &mut luma, stream_format.bit_depth);
    unpack_planar_samples(cb_plane, &mut cb, stream_format.bit_depth);
    unpack_planar_samples(cr_plane, &mut cr, stream_format.bit_depth);

    Ok(VvcSampledFrame {
        geometry,
        format: stream_format,
        luma,
        cb,
        cr,
        chroma_len: chroma_plane_samples,
    })
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
            "VVC 4:4:4 input currently supports bit depths {VVC_MIN_BIT_DEPTH}..{VVC_MAX_BIT_DEPTH}; got {format}"
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
        slice_config.coding_tree.dual_tree_intra,
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
    let mut params_by_ctu = Vec::with_capacity(ctus.len());
    cabac.start();
    for (expected_slice_address, ctu) in ctus.iter().enumerate() {
        debug_assert_eq!(ctu.slice_address, expected_slice_address);
        let params = vvc_ctu_partition_params_with_luma_max_leaf_size_and_chroma(
            ctu.geometry,
            ctu.color.clone(),
            ctu.luma_max_leaf_size,
            slice_config.coding_tree.chroma_sampling,
            slice_config.coding_tree.dual_tree_intra,
        )
        .unwrap_or_else(|| {
            panic!(
                "VVC frame CABAC CTU {} has unsupported coded geometry {}x{}",
                ctu.slice_address,
                ctu.geometry.coded_width(),
                ctu.geometry.coded_height()
            )
        });
        params_by_ctu.push(params);
    }
    encode_frame_partition_body_with_contexts(
        &mut cabac,
        &mut contexts,
        picture_geometry,
        &params_by_ctu,
        slice_config,
    );
    cabac.encode_bin_trm(true);
    cabac.finish()
}

#[cfg(test)]
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

#[cfg(test)]
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
        true,
    )
}

fn vvc_ctu_partition_params_with_luma_max_leaf_size_and_chroma(
    geometry: VvcVideoGeometry,
    color: VvcQuantizedColor,
    luma_max_leaf_size: u16,
    chroma_sampling: ChromaSampling,
    dual_tree_intra: bool,
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
        dual_tree_intra,
    };
    let chroma_tu_count = if color.chroma_tu_count > 1 {
        color.chroma_tu_count
    } else {
        vvc_chroma_transform_nodes(shape).len()
    };
    let mut chroma_tu_intra_modes = color.chroma_tu_intra_modes;
    if color.chroma_tu_count <= 1 {
        for idx in 0..chroma_tu_count.min(MAX_VVC_CHROMA_TUS) {
            chroma_tu_intra_modes[idx] = color.chroma_tu_intra_modes[0];
        }
    }
    let mut cb_tu_transform_skip = color.cb_tu_transform_skip;
    let mut cr_tu_transform_skip = color.cr_tu_transform_skip;
    if color.chroma_tu_count <= 1 {
        for idx in 0..chroma_tu_count.min(MAX_VVC_CHROMA_TUS) {
            cb_tu_transform_skip[idx] = color.cb_tu_transform_skip[0];
            cr_tu_transform_skip[idx] = color.cr_tu_transform_skip[0];
        }
    }
    let (
        luma_tu_count,
        luma_tu_intra_modes,
        luma_tu_abs_levels,
        luma_tu_negative,
        luma_tu_dc_levels,
        luma_tu_ac_levels,
        luma_tu_has_ac,
        luma_tu_transform_skip,
        luma_tu_mrl_index,
        luma_tu_mts_index,
    ) = vvc_luma_residual_arrays_for_geometry(
        coded,
        chroma_sampling,
        dual_tree_intra,
        luma_max_leaf_size,
        color,
    );
    Some(VvcCtuPartitionParams {
        root_width: VVC_CTU_SIZE,
        root_height: VVC_CTU_SIZE,
        visible_width: coded.width,
        visible_height: coded.height,
        chroma_sampling,
        dual_tree_intra,
        luma_max_leaf_size,
        chroma_tu_count,
        luma_tu_count,
        luma_tu_intra_modes,
        luma_tu_abs_levels,
        luma_tu_negative,
        luma_tu_dc_levels,
        luma_tu_ac_levels,
        luma_tu_has_ac,
        luma_tu_transform_skip,
        luma_tu_mrl_index,
        luma_tu_mts_index,
        cb_dc_abs_level: color.cb_rem,
        cb_dc_negative: color.u < 128 && color.cb_rem != 0,
        chroma_tu_intra_modes,
        cb_tu_dc_levels: color.cb_tu_dc_levels,
        cr_tu_dc_levels: color.cr_tu_dc_levels,
        cb_tu_ac_levels: color.cb_tu_ac_levels,
        cr_tu_ac_levels: color.cr_tu_ac_levels,
        cb_tu_has_ac: color.cb_tu_has_ac,
        cr_tu_has_ac: color.cr_tu_has_ac,
        cb_tu_transform_skip,
        cr_tu_transform_skip,
    })
}

fn vvc_luma_residual_arrays_for_geometry(
    coded: VvcCodedGeometry,
    chroma_sampling: ChromaSampling,
    dual_tree_intra: bool,
    luma_max_leaf_size: u16,
    color: VvcQuantizedColor,
) -> (
    usize,
    [VvcIntraPredictionMode; MAX_VVC_LUMA_TUS],
    [u8; MAX_VVC_LUMA_TUS],
    [bool; MAX_VVC_LUMA_TUS],
    [i16; MAX_VVC_LUMA_TUS],
    [[i16; VVC_LUMA_AC_COEFFS_PER_TU]; MAX_VVC_LUMA_TUS],
    [bool; MAX_VVC_LUMA_TUS],
    [bool; MAX_VVC_LUMA_TUS],
    [u8; MAX_VVC_LUMA_TUS],
    [u8; MAX_VVC_LUMA_TUS],
) {
    let mut luma_tu_count = color.luma_tu_count;
    let mut luma_tu_intra_modes = color.luma_tu_intra_modes;
    let mut luma_tu_abs_levels = color.luma_tu_remainders;
    let mut luma_tu_negative = color.luma_tu_negative;
    let mut luma_tu_dc_levels = color.luma_tu_dc_levels;
    let mut luma_tu_ac_levels = color.luma_tu_ac_levels;
    let mut luma_tu_has_ac = color.luma_tu_has_ac;
    let mut luma_tu_transform_skip = color.luma_tu_transform_skip;
    let mut luma_tu_mrl_index = color.luma_tu_mrl_index;
    let mut luma_tu_mts_index = color.luma_tu_mts_index;
    if color.luma_tu_count > 1 {
        return (
            luma_tu_count,
            luma_tu_intra_modes,
            luma_tu_abs_levels,
            luma_tu_negative,
            luma_tu_dc_levels,
            luma_tu_ac_levels,
            luma_tu_has_ac,
            luma_tu_transform_skip,
            luma_tu_mrl_index,
            luma_tu_mts_index,
        );
    }

    let leaf_count =
        vvc_luma_leaf_count(coded, chroma_sampling, dual_tree_intra, luma_max_leaf_size);
    luma_tu_count = leaf_count;
    for idx in 0..leaf_count.min(MAX_VVC_LUMA_TUS) {
        luma_tu_intra_modes[idx] = color.luma_tu_intra_modes[0];
        luma_tu_abs_levels[idx] = color.luma_tu_remainders[0];
        luma_tu_negative[idx] = color.luma_tu_negative[0];
        luma_tu_dc_levels[idx] = color.luma_tu_dc_levels[0];
        luma_tu_ac_levels[idx] = color.luma_tu_ac_levels[0];
        luma_tu_has_ac[idx] = color.luma_tu_has_ac[0];
        luma_tu_transform_skip[idx] = color.luma_tu_transform_skip[0];
        luma_tu_mrl_index[idx] = color.luma_tu_mrl_index[0];
        luma_tu_mts_index[idx] = color.luma_tu_mts_index[0];
    }
    (
        luma_tu_count,
        luma_tu_intra_modes,
        luma_tu_abs_levels,
        luma_tu_negative,
        luma_tu_dc_levels,
        luma_tu_ac_levels,
        luma_tu_has_ac,
        luma_tu_transform_skip,
        luma_tu_mrl_index,
        luma_tu_mts_index,
    )
}

fn vvc_luma_leaf_count(
    coded: VvcCodedGeometry,
    chroma_sampling: ChromaSampling,
    dual_tree_intra: bool,
    luma_max_leaf_size: u16,
) -> usize {
    let shape = VvcCtuPartitionShape {
        root_width: VVC_CTU_SIZE as u16,
        root_height: VVC_CTU_SIZE as u16,
        visible_width: coded.width as u16,
        visible_height: coded.height as u16,
        chroma_sampling,
        dual_tree_intra,
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

fn format_vvc_cabac_vector_dump_json(
    geometry: VvcVideoGeometry,
    format: PixelFormat,
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
    json.push_str(&format!(",\"format\":\"{format}\""));
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
