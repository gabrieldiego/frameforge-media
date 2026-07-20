use std::io::{Read, Write};

use crate::picture::{ChromaSampling, Picture, PixelFormat, SampleBitDepth};

mod decision;
pub mod entropy;
mod ibc;
mod intra_prediction;
mod motion;
mod palette;
mod planar;
#[cfg(feature = "av2-sb-bit-profile")]
mod sb_bits;
mod syntax;
mod tile;

use ibc::{Av2LocalIbc444, Av2LocalIbcStats, Av2LocalIbcTileBounds};
use motion::{
    Av2LosslessMotionMap, Av2MotionSearchRegion, Av2MotionVector, AV2_LOSSLESS_ME_BLOCK_SIZE,
};
use palette::Av2LumaPalette444;
use syntax::{Av2SyntaxPayload, Av2SyntaxWriter};
use tile::{
    av2_black_444_tile_entropy_payload_for_region_with_fields,
    av2_black_444_tile_entropy_payload_for_region_with_intrabc_and_fields,
    av2_black_tile_entropy_payload_for_region,
    av2_lossless_mixed_inter_intra_tile_entropy_payload_for_region_with_fields,
    av2_lossless_mixed_inter_tile_entropy_payload_for_region_with_fields,
    av2_lossless_new_mv_inter_tile_entropy_payload_for_region_with_fields,
    av2_lossless_subsampled_fast_tile_entropy_payload_for_region_with_fields,
    av2_lossless_subsampled_regular_inter_intra_tile_entropy_payload_for_region_with_fields,
    av2_lossless_subsampled_tile_entropy_payload_for_region_with_fields,
    av2_lossless_zero_mv_inter_tile_entropy_payload_for_region_with_fields,
    av2_lossy_fixed_inter_intra_tile_entropy_payload_for_region_with_fields,
    av2_lossy_subsampled_tile_entropy_payload_for_region,
    av2_lossy_subsampled_tile_entropy_payload_for_region_with_fields,
    av2_luma_palette_444_tile_entropy_payload_for_region_with_fields, Av2LosslessInterBlockMode,
    Av2LosslessInterTileBlockModes, Av2TileRegion,
};

pub const AV2_CODEC_NAME: &str = "av2";
pub const AV2_BITSTREAM_EXTENSION: &str = "av2";
pub const AV2_FIXED_BLACK_444_WIDTH: usize = 64;
pub const AV2_FIXED_BLACK_444_HEIGHT: usize = 64;

pub(crate) type Av2Sample = u16;

const AV2_PROFILE_BITS: u8 = 5;
const AV2_LEVEL_BITS: u8 = 5;
const AV2_SEQUENCE_PROFILE_MAIN_422_10_IP1: u8 = 3;
const AV2_SEQUENCE_PROFILE_MAIN_444_10_IP1: u8 = 4;
const AV2_SEQUENCE_LEVEL_MAX: u8 = 31;
const AV2_CHROMA_FORMAT_420: u32 = 0;
const AV2_CHROMA_FORMAT_444: u32 = 2;
const AV2_CHROMA_FORMAT_422: u32 = 3;
const AV2_BITDEPTH_INDEX_10BIT: u32 = 0;
const AV2_BITDEPTH_INDEX_8BIT: u32 = 1;
const AV2_BITDEPTH_INDEX_12BIT: u32 = 2;
const AV2_DELTA_DCQUANT_MIN: i8 = -23;
const AV2_MAX_MAX_DRL_BITS_MINUS_MIN_PLUS_ONE: u16 = 5;
const AV2_MAX_MAX_IBC_DRL_BITS_MINUS_MIN_PLUS_ONE: u16 = 3;
const AV2_PREDICTIVE_ORDER_HINT_BITS: u8 = 8;
const AV2_MVP_SUPERBLOCK_SIZE: usize = 64;
const AV2_TILE_SIZE_BYTES: usize = 4;
const AV2_MIN_TILE_SIZE_BYTES: usize = 1;
const AV2_MI_SIZE: usize = 4;
const AV2_MIB_SIZE_LOG2_64X64: u8 = 4;
const AV2_SEQ_MIB_SIZE_LOG2_64X64: u8 = 4;
const AV2_MAX_TILE_WIDTH: usize = 4096;
const AV2_MAX_TILE_AREA: usize = 4096 * 2304;
const AV2_MAX_TILE_COLS: usize = 64;
const AV2_MAX_TILE_ROWS: usize = 64;
const AV2_TILE_WIDTH_SCALING_LEVEL_2_0_TIER_0: usize = 4;
const AV2_TILE_AREA_SCALING_LEVEL_2_0_TIER_0: usize = 4;
const AV2_ENABLE_LOSSLESS_SUBSAMPLED_IBC: bool = true;
const AV2_ENABLE_LUMA_PALETTE_INTRABC_444: bool = false;
const AV2_LOSSY_DEFAULT_QP: u8 = 8;
const AV2_COLOR_DESCRIPTION_IDC_SRGB: u32 = 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Av2ChromaFormat {
    Yuv420,
    Yuv422,
    Yuv444,
}

impl Av2ChromaFormat {
    fn sequence_header_idc(self) -> u32 {
        match self {
            // AV2 v1.0.0 av2/common/blockd.h: CHROMA_FORMAT_420 is coded as
            // zero. This differs from the project-level AXI chroma_format_idc
            // register convention, which follows the older 1/2/3 sampling IDs.
            Self::Yuv420 => AV2_CHROMA_FORMAT_420,
            Self::Yuv422 => AV2_CHROMA_FORMAT_422,
            Self::Yuv444 => AV2_CHROMA_FORMAT_444,
        }
    }

    fn chroma_sampling(self) -> ChromaSampling {
        match self {
            Self::Yuv420 => ChromaSampling::Cs420,
            Self::Yuv422 => ChromaSampling::Cs422,
            Self::Yuv444 => ChromaSampling::Cs444,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Av2StreamFormat {
    chroma_format: Av2ChromaFormat,
    bit_depth: SampleBitDepth,
}

impl Av2StreamFormat {
    fn from_pixel_format(format: PixelFormat) -> Option<Self> {
        if format == PixelFormat::Rgb24 {
            return Some(Self {
                chroma_format: Av2ChromaFormat::Yuv444,
                bit_depth: SampleBitDepth::new(8).expect("rgb24 is 8-bit"),
            });
        }
        let bit_depth = format.bit_depth();
        let chroma_format = match (format.chroma_sampling()?, bit_depth.bits()) {
            // AV2 has a 12-bit test-only profile in AVM, but the normal
            // reference-validation profiles support 8/10-bit streams.
            (ChromaSampling::Cs420, 8 | 10) => Av2ChromaFormat::Yuv420,
            (ChromaSampling::Cs422, 8 | 10) => Av2ChromaFormat::Yuv422,
            (ChromaSampling::Cs444, 8 | 10) => Av2ChromaFormat::Yuv444,
            (ChromaSampling::Monochrome, _) => return None,
            (ChromaSampling::Cs420 | ChromaSampling::Cs422 | ChromaSampling::Cs444, _) => {
                return None
            }
        };
        Some(Self {
            chroma_format,
            bit_depth,
        })
    }

    #[cfg(test)]
    fn yuv420_8() -> Self {
        Self {
            chroma_format: Av2ChromaFormat::Yuv420,
            bit_depth: SampleBitDepth::new(8).expect("8-bit depth is supported"),
        }
    }

    #[cfg(test)]
    fn yuv444_8() -> Self {
        Self {
            chroma_format: Av2ChromaFormat::Yuv444,
            bit_depth: SampleBitDepth::new(8).expect("8-bit depth is supported"),
        }
    }

    fn pixel_format(self) -> PixelFormat {
        PixelFormat::planar_yuv(self.chroma_format.chroma_sampling(), self.bit_depth)
    }

    fn sequence_profile_idc(self) -> u8 {
        match self.chroma_format {
            Av2ChromaFormat::Yuv422 => AV2_SEQUENCE_PROFILE_MAIN_422_10_IP1,
            // Profile 4 admits 4:2:0 and 4:4:4 in the AVM reference build.
            Av2ChromaFormat::Yuv420 | Av2ChromaFormat::Yuv444 => {
                AV2_SEQUENCE_PROFILE_MAIN_444_10_IP1
            }
        }
    }

    fn bitdepth_lut_index(self) -> u32 {
        match self.bit_depth.bits() {
            10 => AV2_BITDEPTH_INDEX_10BIT,
            8 => AV2_BITDEPTH_INDEX_8BIT,
            12 => AV2_BITDEPTH_INDEX_12BIT,
            bits => unreachable!("unsupported AV2 bit depth {bits}"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Av2DeltaQParams {
    present: bool,
    resolution_log2: u8,
}

impl Av2DeltaQParams {
    const fn disabled() -> Self {
        Self {
            present: false,
            resolution_log2: 0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Av2QuantizationParams {
    base_qindex: u16,
    delta_q: Av2DeltaQParams,
    using_qmatrix: bool,
}

impl Av2QuantizationParams {
    const fn lossless() -> Self {
        Self {
            base_qindex: 0,
            delta_q: Av2DeltaQParams::disabled(),
            using_qmatrix: false,
        }
    }

    fn regular_qp(qp: u8, bit_depth: SampleBitDepth) -> Self {
        Self {
            base_qindex: av2_base_qindex_for_qp(qp, bit_depth),
            delta_q: Av2DeltaQParams::disabled(),
            using_qmatrix: false,
        }
    }

    const fn is_coded_lossless(self) -> bool {
        self.base_qindex == 0 && !self.delta_q.present && !self.using_qmatrix
    }
}

fn av2_base_qindex_for_qp(qp: u8, bit_depth: SampleBitDepth) -> u16 {
    let scaled = (u32::from(qp.max(1)) * 10).div_ceil(3);
    (scaled as u16).min(av2_max_qindex(bit_depth))
}

fn av2_predictive_inter_qp_for_qp(qp: u8, bit_depth: SampleBitDepth) -> u8 {
    let qp = u16::from(qp.max(1));
    // Until delta-q is active, changed predictive tiles share one inter-frame
    // qindex. Keep it below the key-frame QP so zero-MV residuals do not spend
    // the accumulated prediction quality budget too aggressively.
    let scaled = if bit_depth.bits() > 8 {
        qp.div_ceil(6)
    } else {
        (qp * 2).div_ceil(3)
    };
    scaled.clamp(1, u16::from(u8::MAX)) as u8
}

fn av2_qindex_bits(bit_depth: SampleBitDepth) -> u8 {
    if bit_depth.bits() == 8 {
        8
    } else {
        9
    }
}

fn av2_max_qindex(bit_depth: SampleBitDepth) -> u16 {
    match bit_depth.bits() {
        8 => 255,
        10 => 255 + 2 * 24,
        12 => 255 + 4 * 24,
        bits => unreachable!("unsupported AV2 bit depth {bits}"),
    }
}

fn av2_lossless_dc_predictor(bit_depth: SampleBitDepth) -> Av2Sample {
    128u16 << u32::from(bit_depth.bits() - 8)
}

fn av2_lossless_h_pred_left_edge(bit_depth: SampleBitDepth) -> Av2Sample {
    av2_lossless_dc_predictor(bit_depth) + 1
}

fn av2_lossless_v_pred_above_edge(bit_depth: SampleBitDepth) -> Av2Sample {
    av2_lossless_dc_predictor(bit_depth) - 1
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Av2Black444MvpProfile {
    enable_sdp: bool,
    enable_ext_partitions: bool,
    enable_uneven_4way_partitions: bool,
    enable_intra_edge_filter: bool,
    enable_mrls: bool,
    enable_cfl_intra: bool,
    enable_mhccp: bool,
    enable_ibp: bool,
    enable_refmvbank: bool,
    is_drl_reorder_disable: bool,
    def_max_bvp_drl_bits_minus_min: u16,
    allow_frame_max_bvp_drl_bits: bool,
    enable_bawp: bool,
    enable_fsc: bool,
    enable_idtx_intra: bool,
    enable_chroma_dctonly: bool,
    enable_cctx: bool,
    disable_cdf_update: bool,
}

impl Av2Black444MvpProfile {
    fn current() -> Self {
        Self {
            // Keep the first tile payload on the shared luma/chroma tree. AVM
            // decode_partition() enters separate luma/chroma trees at 64x64
            // when SDP is enabled, which is unnecessary for the first black
            // 4:4:4 bring-up stream.
            enable_sdp: false,
            enable_ext_partitions: false,
            enable_uneven_4way_partitions: false,
            enable_intra_edge_filter: false,
            enable_mrls: false,
            enable_cfl_intra: false,
            enable_mhccp: false,
            enable_ibp: false,
            enable_refmvbank: false,
            is_drl_reorder_disable: true,
            def_max_bvp_drl_bits_minus_min: 0,
            allow_frame_max_bvp_drl_bits: false,
            enable_bawp: false,
            enable_fsc: true,
            // AVM read_sequence_transform_quant_entropy_group_tool_flags()
            // derives IDTX intra from FSC when FSC is enabled.
            enable_idtx_intra: true,
            // The regular-q writer reconstructs chroma as DCT_DCT until it
            // grows chroma tx-type selection and signaling.
            enable_chroma_dctonly: true,
            enable_cctx: false,
            // AV2 v1.0.0 tile_group_obu() updates CDFs while decode_tile()
            // parses symbols unless this header flag disables adaptation.
            disable_cdf_update: false,
        }
    }

    fn with_local_ibc_candidates(mut self) -> Self {
        // AVM derives above/left 8x8 block vectors as default IntraBC BV
        // candidates 2 and 3 in mvref_common.c. AV2 sequence syntax stores
        // max_bvp_drl_bits minus MIN_MAX_IBC_DRL_BITS; value 2 therefore
        // permits DRL indices 0..3 without frame-level overrides.
        self.def_max_bvp_drl_bits_minus_min = 2;
        self
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Av2ObuType {
    SequenceHeader = 1,
    TemporalDelimiter = 2,
    ClosedLoopKey = 4,
    RegularTileGroup = 7,
    RegularSef = 12,
    ContentInterpretation = 24,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Av2VideoGeometry {
    pub width: usize,
    pub height: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Av2TileLayout {
    regions: Vec<Av2TileRegion>,
    cols: usize,
    rows: usize,
    log2_cols: u8,
    log2_rows: u8,
    min_log2_cols: u8,
    min_log2_rows: u8,
    max_log2_cols: u8,
    max_log2_rows: u8,
}

impl Av2TileLayout {
    fn for_geometry(geometry: Av2VideoGeometry) -> Self {
        let cols = geometry.width.div_ceil(AV2_MVP_SUPERBLOCK_SIZE);
        let rows = geometry.height.div_ceil(AV2_MVP_SUPERBLOCK_SIZE);
        let mut regions = Vec::with_capacity(cols * rows);
        for tile_row in 0..rows {
            let origin_y = tile_row * AV2_MVP_SUPERBLOCK_SIZE;
            let height = (geometry.height - origin_y).min(AV2_MVP_SUPERBLOCK_SIZE);
            for tile_col in 0..cols {
                let origin_x = tile_col * AV2_MVP_SUPERBLOCK_SIZE;
                let width = (geometry.width - origin_x).min(AV2_MVP_SUPERBLOCK_SIZE);
                regions.push(Av2TileRegion {
                    origin_x,
                    origin_y,
                    width,
                    height,
                });
            }
        }
        let limits = Av2TileLimits::for_geometry(geometry);
        let log2_cols = ceil_log2_usize(cols).max(limits.min_log2_cols);
        let min_log2_rows = limits.min_log2.saturating_sub(log2_cols);
        let log2_rows = ceil_log2_usize(rows).max(min_log2_rows);
        assert!(
            log2_cols <= limits.max_log2_cols,
            "AV2 MVP tile columns exceed the Level 2.0 tile limit"
        );
        assert!(
            log2_rows <= limits.max_log2_rows,
            "AV2 MVP tile rows exceed the Level 2.0 tile limit"
        );
        Self {
            regions,
            cols,
            rows,
            log2_cols,
            log2_rows,
            min_log2_cols: limits.min_log2_cols,
            min_log2_rows,
            max_log2_cols: limits.max_log2_cols,
            max_log2_rows: limits.max_log2_rows,
        }
    }

    fn single_for_geometry(geometry: Av2VideoGeometry) -> Self {
        Self::try_single_for_geometry(geometry)
            .expect("AV2 MVP single-tile layout exceeds the configured tile limits")
    }

    fn try_single_for_geometry(geometry: Av2VideoGeometry) -> Option<Self> {
        let limits = Av2TileLimits::for_geometry(geometry);
        if limits.min_log2_cols != 0 || limits.min_log2 != 0 {
            return None;
        }
        Some(Self {
            regions: vec![Av2TileRegion {
                origin_x: 0,
                origin_y: 0,
                width: geometry.width,
                height: geometry.height,
            }],
            cols: 1,
            rows: 1,
            log2_cols: 0,
            log2_rows: 0,
            min_log2_cols: limits.min_log2_cols,
            min_log2_rows: limits.min_log2,
            max_log2_cols: limits.max_log2_cols,
            max_log2_rows: limits.max_log2_rows,
        })
    }

    fn uniform_for_geometry(geometry: Av2VideoGeometry, log2_cols: u8, log2_rows: u8) -> Self {
        let limits = Av2TileLimits::for_geometry(geometry);
        assert!(log2_cols >= limits.min_log2_cols);
        assert!(log2_cols <= limits.max_log2_cols);
        assert!(log2_rows <= limits.max_log2_rows);
        assert!(log2_cols + log2_rows >= limits.min_log2);
        let mi_cols = align_power_of_two(geometry.width, 3) / AV2_MI_SIZE;
        let mi_rows = align_power_of_two(geometry.height, 3) / AV2_MI_SIZE;
        let col_starts_sb = uniform_tile_starts_sb(mi_cols, log2_cols);
        let row_starts_sb = uniform_tile_starts_sb(mi_rows, log2_rows);
        let cols = col_starts_sb.len() - 1;
        let rows = row_starts_sb.len() - 1;
        let mut regions = Vec::with_capacity(cols * rows);
        for tile_row in 0..rows {
            let origin_sb_y = row_starts_sb[tile_row];
            let end_sb_y = row_starts_sb[tile_row + 1];
            let origin_y = origin_sb_y * AV2_MVP_SUPERBLOCK_SIZE;
            let end_y = (end_sb_y * AV2_MVP_SUPERBLOCK_SIZE).min(geometry.height);
            for tile_col in 0..cols {
                let origin_sb_x = col_starts_sb[tile_col];
                let end_sb_x = col_starts_sb[tile_col + 1];
                let origin_x = origin_sb_x * AV2_MVP_SUPERBLOCK_SIZE;
                let end_x = (end_sb_x * AV2_MVP_SUPERBLOCK_SIZE).min(geometry.width);
                regions.push(Av2TileRegion {
                    origin_x,
                    origin_y,
                    width: end_x - origin_x,
                    height: end_y - origin_y,
                });
            }
        }
        let min_log2_rows = limits.min_log2.saturating_sub(log2_cols);
        Self {
            regions,
            cols,
            rows,
            log2_cols: ceil_log2_usize(cols),
            log2_rows: ceil_log2_usize(rows),
            min_log2_cols: limits.min_log2_cols,
            min_log2_rows,
            max_log2_cols: limits.max_log2_cols,
            max_log2_rows: limits.max_log2_rows,
        }
    }

    fn lossless_subsampled_fast_for_geometry(geometry: Av2VideoGeometry) -> Self {
        let limits = Av2TileLimits::for_geometry(geometry);
        let target_log2_cols = if geometry.width >= 1920 {
            2
        } else if geometry.width >= 1024 {
            1
        } else {
            0
        };
        let log2_cols = target_log2_cols
            .max(limits.min_log2_cols)
            .min(limits.max_log2_cols);
        let target_log2_rows = if geometry.height >= 1080 { 1 } else { 0 };
        let min_log2_rows = limits.min_log2.saturating_sub(log2_cols);
        let log2_rows = target_log2_rows
            .max(min_log2_rows)
            .min(limits.max_log2_rows);
        if log2_cols == 0 && log2_rows == 0 {
            Self::single_for_geometry(geometry)
        } else {
            Self::uniform_for_geometry(geometry, log2_cols, log2_rows)
        }
    }

    fn lossy_subsampled_for_geometry(geometry: Av2VideoGeometry) -> Self {
        Self::lossless_subsampled_fast_for_geometry(geometry)
    }

    fn tile_count(&self) -> usize {
        self.regions.len()
    }

    fn local_ibc_tile_bounds(&self) -> Vec<Av2LocalIbcTileBounds> {
        self.regions
            .iter()
            .map(|region| Av2LocalIbcTileBounds {
                origin_x: region.origin_x,
                origin_y: region.origin_y,
                width: region.width,
                height: region.height,
            })
            .collect()
    }

    fn lossless_subsampled_ibc_for_geometry(geometry: Av2VideoGeometry) -> Self {
        Self::try_single_for_geometry(geometry).unwrap_or_else(|| Self::for_geometry(geometry))
    }

    fn is_single_tile(&self) -> bool {
        self.tile_count() == 1
    }
}

fn av2_tile_layout_for_frame_mode(
    geometry: Av2VideoGeometry,
    frame_mode: &Av2Mvp444FrameMode,
) -> Av2TileLayout {
    match frame_mode {
        Av2Mvp444FrameMode::Black => Av2TileLayout::for_geometry(geometry),
        Av2Mvp444FrameMode::LumaPalette { .. } => Av2TileLayout::single_for_geometry(geometry),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Av2TileLimits {
    min_log2_cols: u8,
    max_log2_cols: u8,
    max_log2_rows: u8,
    min_log2: u8,
}

impl Av2TileLimits {
    fn for_geometry(geometry: Av2VideoGeometry) -> Self {
        assert!(
            AV2_SEQ_MIB_SIZE_LOG2_64X64 >= AV2_MIB_SIZE_LOG2_64X64
                && AV2_SEQ_MIB_SIZE_LOG2_64X64 - AV2_MIB_SIZE_LOG2_64X64 <= 1,
            "AV2 MVP only supports the AVM tile-limit scale used by 64x64 sequence superblocks"
        );
        let mi_cols = align_power_of_two(geometry.width, 3) / AV2_MI_SIZE;
        let mi_rows = align_power_of_two(geometry.height, 3) / AV2_MI_SIZE;
        let aligned_mi_cols = align_power_of_two(mi_cols, AV2_MIB_SIZE_LOG2_64X64 as usize);
        let aligned_mi_rows = align_power_of_two(mi_rows, AV2_MIB_SIZE_LOG2_64X64 as usize);
        let sb_cols = aligned_mi_cols >> AV2_MIB_SIZE_LOG2_64X64;
        let sb_rows = aligned_mi_rows >> AV2_MIB_SIZE_LOG2_64X64;
        let sb_size_log2 = AV2_MIB_SIZE_LOG2_64X64 + 2;
        let max_width_sb =
            (AV2_TILE_WIDTH_SCALING_LEVEL_2_0_TIER_0 * AV2_MAX_TILE_WIDTH) >> (sb_size_log2 + 2);
        let max_area_sb = (AV2_TILE_AREA_SCALING_LEVEL_2_0_TIER_0 * AV2_MAX_TILE_AREA)
            >> ((2 * sb_size_log2) + 2);
        let min_log2_cols = tile_log2(max_width_sb, sb_cols);
        let max_log2_cols = tile_log2(1, sb_cols.min(AV2_MAX_TILE_COLS));
        let max_log2_rows = tile_log2(1, sb_rows.min(AV2_MAX_TILE_ROWS));
        let min_log2 = tile_log2(max_area_sb, sb_cols * sb_rows).max(min_log2_cols);
        Self {
            min_log2_cols,
            max_log2_cols,
            max_log2_rows,
            min_log2,
        }
    }
}

fn uniform_tile_starts_sb(mi_size: usize, log2_tiles: u8) -> Vec<usize> {
    let aligned_mi = align_power_of_two(mi_size, AV2_MIB_SIZE_LOG2_64X64 as usize);
    let sb_count = aligned_mi >> AV2_MIB_SIZE_LOG2_64X64;
    let seq_mib_size_log2 = AV2_SEQ_MIB_SIZE_LOG2_64X64 as usize;
    let seq_sb_count = align_power_of_two(mi_size, seq_mib_size_log2) >> seq_mib_size_log2;
    let full_sb_count = mi_size >> seq_mib_size_log2;
    let target_tiles = 1usize << log2_tiles;
    let base_size_sb = full_sb_count >> log2_tiles;
    let mut extra_sbs = full_sb_count - (base_size_sb << log2_tiles);
    if base_size_sb == 0 {
        extra_sbs += seq_sb_count - full_sb_count;
    }
    let mut starts = Vec::with_capacity(target_tiles + 1);
    let mut start_sb = 0usize;
    while start_sb < seq_sb_count && starts.len() < target_tiles {
        starts.push(start_sb);
        start_sb += base_size_sb + usize::from(extra_sbs > 0);
        extra_sbs = extra_sbs.saturating_sub(1);
    }
    starts.push(sb_count);
    starts
}

fn align_power_of_two(value: usize, power: usize) -> usize {
    let alignment = 1usize << power;
    (value + alignment - 1) & !(alignment - 1)
}

fn tile_log2(block_size: usize, target: usize) -> u8 {
    assert!(block_size > 0);
    assert!(target > 0);
    let mut log2 = 0u8;
    while (block_size << log2) < target {
        log2 += 1;
    }
    log2
}

fn ceil_log2_usize(value: usize) -> u8 {
    assert!(value > 0);
    let mut bits = 0u8;
    let mut threshold = 1usize;
    while threshold < value {
        threshold <<= 1;
        bits += 1;
    }
    bits
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Av2EncodeParams {
    pub frames: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Av2EncodeRequest {
    pub params: Av2EncodeParams,
    pub geometry: Av2VideoGeometry,
    pub format: PixelFormat,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Av2EncodeOptions {
    pub lossless: bool,
    pub qp: Option<u8>,
    pub predictive: bool,
}

pub struct Av2EncodeFrameMetrics<'a> {
    pub frame_idx: usize,
    pub frame_count: usize,
    pub bitstream_bytes: usize,
    pub source: &'a [u8],
    pub reconstruction: &'a [u8],
}

impl Av2EncodeRequest {
    pub fn validate(&self) -> Result<(), String> {
        if self.geometry.width == 0 || self.geometry.height == 0 {
            return Err("AV2 encode expects positive dimensions".to_string());
        }
        if self.params.frames == 0 {
            return Err("AV2 encode expects at least one frame".to_string());
        }
        if !self.format.is_yuv() && self.format != PixelFormat::Rgb24 {
            return Err(format!(
                "AV2 encode expects planar YUV or rgb24 input; got {}",
                self.format
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Av2Mvp444FrameMode {
    Black,
    LumaPalette {
        palette: Av2LumaPalette444,
        ibc: Option<Av2LocalIbc444>,
    },
}

impl Av2Mvp444FrameMode {
    fn from_frame(
        frame: &[u8],
        geometry: Av2VideoGeometry,
        bit_depth: SampleBitDepth,
    ) -> Result<Self, String> {
        let black = av2_black_444_reconstruction_for_geometry_with_depth(geometry, bit_depth);
        if frame == black {
            return Ok(Self::Black);
        }
        let palette = palette::build_luma_palette_444(frame, geometry, bit_depth)?;
        let ibc = if AV2_ENABLE_LUMA_PALETTE_INTRABC_444 {
            Some(ibc::build_local_ibc_444_for_palette(
                frame, geometry, &palette,
            )?)
        } else {
            None
        };
        Ok(Self::LumaPalette { palette, ibc })
    }

    fn allow_screen_content_tools(&self) -> bool {
        true
    }

    fn allow_intrabc(&self) -> bool {
        match self {
            Self::Black => false,
            // Single-tile palette coding reuses prediction and entropy state
            // across 64x64 superblocks. The current local IBC model is still
            // tied to independent 64x64 tiles, so leave it off until the block
            // vector predictor is modeled for multi-superblock tiles.
            Self::LumaPalette { ibc, .. } => AV2_ENABLE_LUMA_PALETTE_INTRABC_444 && ibc.is_some(),
        }
    }

    fn profile(&self) -> Av2Black444MvpProfile {
        let profile = Av2Black444MvpProfile::current();
        if self.allow_intrabc() {
            profile.with_local_ibc_candidates()
        } else {
            profile
        }
    }

    fn reconstruction(&self, geometry: Av2VideoGeometry, bit_depth: SampleBitDepth) -> Vec<u8> {
        match self {
            Self::Black => {
                av2_black_444_reconstruction_for_geometry_with_depth(geometry, bit_depth)
            }
            Self::LumaPalette { palette, .. } => palette.reconstruction().to_vec(),
        }
    }
}

pub fn av2_encode_fixed_black_444(
    input: &mut dyn Read,
    output: &mut dyn Write,
    recon: Option<&mut dyn Write>,
    request: Av2EncodeRequest,
) -> Result<(), String> {
    av2_encode_fixed_black_444_with_frame_metrics(input, output, recon, request, None)
}

pub fn av2_encode_fixed_black_444_with_frame_metrics(
    input: &mut dyn Read,
    output: &mut dyn Write,
    recon: Option<&mut dyn Write>,
    request: Av2EncodeRequest,
    frame_metrics: Option<&mut dyn for<'a> FnMut(Av2EncodeFrameMetrics<'a>)>,
) -> Result<(), String> {
    av2_encode_fixed_black_444_with_options_and_frame_metrics(
        input,
        output,
        recon,
        request,
        Av2EncodeOptions::default(),
        frame_metrics,
    )
}

pub fn av2_encode_fixed_black_444_with_options_and_frame_metrics(
    input: &mut dyn Read,
    output: &mut dyn Write,
    mut recon: Option<&mut dyn Write>,
    request: Av2EncodeRequest,
    options: Av2EncodeOptions,
    mut frame_metrics: Option<&mut dyn for<'a> FnMut(Av2EncodeFrameMetrics<'a>)>,
) -> Result<(), String> {
    request.validate()?;
    let geometry = validate_mvp_request(request)?;
    let stream_format = Av2StreamFormat::from_pixel_format(request.format)
        .expect("validate_mvp_request accepts only supported AV2 stream formats");
    let rgb_identity = request.format == PixelFormat::Rgb24;

    let source_expected_len =
        Picture::expected_len(geometry.width, geometry.height, request.format);
    let coded_expected_len = Picture::expected_len(
        geometry.width,
        geometry.height,
        stream_format.pixel_format(),
    );
    debug_assert_eq!(source_expected_len, coded_expected_len);
    let mut predictive_started = false;
    let mut predictive_reference: Option<Vec<u8>> = None;
    let mut predictive_reconstruction: Option<Vec<u8>> = None;
    for frame_index in 0..request.params.frames {
        #[cfg(feature = "av2-sb-bit-profile")]
        sb_bits::set_current_frame(frame_index);
        let mut source_frame = vec![0; source_expected_len];
        input.read_exact(&mut source_frame).map_err(|err| {
            format!(
                "failed to read AV2 MVP input frame {} of {}: {err}",
                frame_index + 1,
                request.params.frames
            )
        })?;
        let coded_frame: Vec<u8>;
        let frame = if rgb_identity {
            coded_frame = rgb24_to_planar_gbr(&source_frame, geometry);
            coded_frame.as_slice()
        } else {
            source_frame.as_slice()
        };
        // The MVP stream keeps each input picture independently decodable.
        // Concatenating one single-picture OBU sequence per frame avoids
        // hidden single-frame tooling assumptions while inter-frame AV2 syntax
        // is still being built out.
        if options.lossless
            && matches!(
                stream_format.chroma_format,
                Av2ChromaFormat::Yuv420 | Av2ChromaFormat::Yuv422 | Av2ChromaFormat::Yuv444
            )
        {
            let (bitstream, reconstruction) = if options.predictive {
                let order_hint = av2_order_hint_for_frame(frame_index);
                if predictive_reference.as_deref() == Some(frame) {
                    av2_lossless_subsampled_regular_sef_bitstream_and_reconstruction_for_frame(
                        frame, order_hint,
                    )
                } else if let Some((bitstream, reconstruction)) = predictive_reference
                    .as_deref()
                    .and_then(|reference| {
                        av2_lossless_subsampled_regular_inter_tiles_bitstream_and_reconstruction_for_frame(
                            geometry,
                            stream_format,
                            frame,
                            reference,
                            order_hint,
                        )
                    })
                {
                    predictive_reference = Some(frame.to_vec());
                    (bitstream, reconstruction)
                } else {
                    let result =
                        av2_lossless_subsampled_predictive_key_bitstream_and_reconstruction_for_frame(
                            geometry,
                            stream_format,
                            frame,
                            !predictive_started,
                            order_hint,
                            rgb_identity,
                        );
                    predictive_started = true;
                    predictive_reference = Some(frame.to_vec());
                    result
                }
            } else {
                av2_lossless_subsampled_bitstream_and_reconstruction_for_frame(
                    geometry,
                    stream_format,
                    frame,
                    rgb_identity,
                )
            };
            output
                .write_all(&bitstream)
                .map_err(|err| format!("failed to write AV2 bitstream: {err}"))?;
            let public_reconstruction: Vec<u8>;
            let reconstruction = if rgb_identity {
                public_reconstruction = planar_gbr_to_rgb24(&reconstruction, geometry);
                public_reconstruction.as_slice()
            } else {
                reconstruction.as_slice()
            };
            if let Some(recon) = recon.as_deref_mut() {
                recon
                    .write_all(reconstruction)
                    .map_err(|err| format!("failed to write AV2 reconstruction: {err}"))?;
            }
            if let Some(frame_metrics) = frame_metrics.as_deref_mut() {
                frame_metrics(Av2EncodeFrameMetrics {
                    frame_idx: frame_index,
                    frame_count: request.params.frames,
                    bitstream_bytes: bitstream.len(),
                    source: &source_frame,
                    reconstruction,
                });
            }
            continue;
        }
        if stream_format.chroma_format == Av2ChromaFormat::Yuv422 && options.qp.is_none() {
            return Err(format!(
                "AV2 non-lossless encode is not implemented for {}; pass --qp to use the experimental lossy residual path",
                request.format
            ));
        }
        let use_lossy_residual_path =
            options.qp.is_some() || stream_format.chroma_format == Av2ChromaFormat::Yuv420;
        if use_lossy_residual_path {
            let qp = options.qp.unwrap_or(AV2_LOSSY_DEFAULT_QP);
            let (bitstream, reconstruction) = if options.predictive {
                let order_hint = av2_order_hint_for_frame(frame_index);
                if predictive_reference.as_deref() == Some(frame) {
                    if let Some(reference_reconstruction) = predictive_reconstruction.as_deref() {
                        av2_lossy_subsampled_regular_sef_bitstream_and_reconstruction_for_frame(
                            reference_reconstruction,
                            order_hint,
                        )
                    } else {
                        av2_lossy_subsampled_predictive_key_bitstream_and_reconstruction_for_frame(
                            geometry,
                            stream_format,
                            frame,
                            qp,
                            !predictive_started,
                            order_hint,
                            rgb_identity,
                        )
                    }
                } else {
                    if let (Some(reference), Some(reference_reconstruction)) = (
                        predictive_reference.as_deref(),
                        predictive_reconstruction.as_deref(),
                    ) {
                        av2_lossy_subsampled_zero_mv_inter_tiles_bitstream_and_reconstruction_for_frame(
                            geometry,
                            stream_format,
                            frame,
                            reference,
                            reference_reconstruction,
                            qp,
                            order_hint,
                        )
                        .unwrap_or_else(|| {
                            av2_lossy_subsampled_predictive_key_bitstream_and_reconstruction_for_frame(
                                geometry,
                                stream_format,
                                frame,
                                qp,
                                !predictive_started,
                                order_hint,
                                rgb_identity,
                            )
                        })
                    } else {
                        av2_lossy_subsampled_predictive_key_bitstream_and_reconstruction_for_frame(
                            geometry,
                            stream_format,
                            frame,
                            qp,
                            !predictive_started,
                            order_hint,
                            rgb_identity,
                        )
                    }
                }
            } else {
                av2_lossy_subsampled_bitstream_and_reconstruction_for_frame(
                    geometry,
                    stream_format,
                    frame,
                    qp,
                    rgb_identity,
                )
            };
            if options.predictive {
                predictive_started = true;
                predictive_reference = Some(frame.to_vec());
                predictive_reconstruction = Some(reconstruction.clone());
            }
            output
                .write_all(&bitstream)
                .map_err(|err| format!("failed to write AV2 bitstream: {err}"))?;
            let public_reconstruction: Vec<u8>;
            let reconstruction = if rgb_identity {
                public_reconstruction = planar_gbr_to_rgb24(&reconstruction, geometry);
                public_reconstruction.as_slice()
            } else {
                reconstruction.as_slice()
            };
            if let Some(recon) = recon.as_deref_mut() {
                recon
                    .write_all(reconstruction)
                    .map_err(|err| format!("failed to write AV2 reconstruction: {err}"))?;
            }
            if let Some(frame_metrics) = frame_metrics.as_deref_mut() {
                frame_metrics(Av2EncodeFrameMetrics {
                    frame_idx: frame_index,
                    frame_count: request.params.frames,
                    bitstream_bytes: bitstream.len(),
                    source: &source_frame,
                    reconstruction,
                });
            }
            continue;
        }
        if options.predictive {
            return Err(format!(
                "AV2 predictive non-lossless encode for {} requires --qp to use the lossy residual path",
                request.format
            ));
        }

        let frame_mode = Av2Mvp444FrameMode::from_frame(frame, geometry, stream_format.bit_depth)?;

        let bitstream = av2_mvp_444_bitstream_for_mode(
            geometry,
            stream_format.bit_depth,
            &frame_mode,
            rgb_identity,
        );
        let reconstruction = frame_mode.reconstruction(geometry, stream_format.bit_depth);
        output
            .write_all(&bitstream)
            .map_err(|err| format!("failed to write AV2 bitstream: {err}"))?;
        let public_reconstruction: Vec<u8>;
        let reconstruction = if rgb_identity {
            public_reconstruction = planar_gbr_to_rgb24(&reconstruction, geometry);
            public_reconstruction.as_slice()
        } else {
            reconstruction.as_slice()
        };
        if let Some(recon) = recon.as_deref_mut() {
            recon
                .write_all(reconstruction)
                .map_err(|err| format!("failed to write AV2 reconstruction: {err}"))?;
        }
        if let Some(frame_metrics) = frame_metrics.as_deref_mut() {
            frame_metrics(Av2EncodeFrameMetrics {
                frame_idx: frame_index,
                frame_count: request.params.frames,
                bitstream_bytes: bitstream.len(),
                source: &source_frame,
                reconstruction,
            });
        }
    }
    Ok(())
}

fn rgb24_to_planar_gbr(frame: &[u8], geometry: Av2VideoGeometry) -> Vec<u8> {
    let pixels = geometry.width * geometry.height;
    debug_assert_eq!(frame.len(), pixels * 3);
    let mut out = vec![0; pixels * 3];
    let (g_plane, chroma) = out.split_at_mut(pixels);
    let (b_plane, r_plane) = chroma.split_at_mut(pixels);
    for pixel in 0..pixels {
        let src = pixel * 3;
        r_plane[pixel] = frame[src];
        g_plane[pixel] = frame[src + 1];
        b_plane[pixel] = frame[src + 2];
    }
    out
}

fn planar_gbr_to_rgb24(frame: &[u8], geometry: Av2VideoGeometry) -> Vec<u8> {
    let pixels = geometry.width * geometry.height;
    debug_assert_eq!(frame.len(), pixels * 3);
    let (g_plane, chroma) = frame.split_at(pixels);
    let (b_plane, r_plane) = chroma.split_at(pixels);
    let mut out = vec![0; pixels * 3];
    for pixel in 0..pixels {
        let dst = pixel * 3;
        out[dst] = r_plane[pixel];
        out[dst + 1] = g_plane[pixel];
        out[dst + 2] = b_plane[pixel];
    }
    out
}

#[cfg(test)]
fn av2_black_444_bitstream_for_geometry(geometry: Av2VideoGeometry) -> Vec<u8> {
    av2_black_bitstream_for_geometry(geometry, Av2StreamFormat::yuv444_8())
}

#[cfg(test)]
fn av2_black_bitstream_for_geometry(
    geometry: Av2VideoGeometry,
    stream_format: Av2StreamFormat,
) -> Vec<u8> {
    let mut out = Vec::new();
    let profile = Av2Black444MvpProfile::current();
    append_obu(
        &mut out,
        Av2ObuType::TemporalDelimiter,
        &Av2SyntaxPayload::default(),
    );
    append_obu(
        &mut out,
        Av2ObuType::SequenceHeader,
        &av2_mvp_sequence_header_payload(geometry, profile, stream_format),
    );
    append_obu(
        &mut out,
        Av2ObuType::ClosedLoopKey,
        &av2_black_closed_loop_key_payload(geometry, stream_format.chroma_format),
    );
    out
}

fn av2_lossy_subsampled_bitstream_and_reconstruction_for_frame(
    geometry: Av2VideoGeometry,
    stream_format: Av2StreamFormat,
    frame: &[u8],
    qp: u8,
    rgb_identity: bool,
) -> (Vec<u8>, Vec<u8>) {
    assert!(qp > 0, "AV2 lossy QP must be non-zero");
    let expected_len = Picture::expected_len(
        geometry.width,
        geometry.height,
        stream_format.pixel_format(),
    );
    assert_eq!(
        frame.len(),
        expected_len,
        "AV2 planar lossy input length must match geometry"
    );
    let mut reconstruction = vec![0; expected_len];
    let mut out = Vec::new();
    append_obu(
        &mut out,
        Av2ObuType::TemporalDelimiter,
        &Av2SyntaxPayload::default(),
    );
    append_obu(
        &mut out,
        Av2ObuType::SequenceHeader,
        &av2_mvp_sequence_header_payload(geometry, Av2Black444MvpProfile::current(), stream_format),
    );
    append_rgb_content_interpretation_if_needed(&mut out, rgb_identity);
    append_obu(
        &mut out,
        Av2ObuType::ClosedLoopKey,
        &av2_lossy_subsampled_closed_loop_key_payload(
            geometry,
            stream_format,
            frame,
            &mut reconstruction,
            qp,
        ),
    );
    (out, reconstruction)
}

fn av2_lossy_subsampled_predictive_key_bitstream_and_reconstruction_for_frame(
    geometry: Av2VideoGeometry,
    stream_format: Av2StreamFormat,
    frame: &[u8],
    qp: u8,
    include_sequence_header: bool,
    order_hint: u16,
    rgb_identity: bool,
) -> (Vec<u8>, Vec<u8>) {
    assert!(qp > 0, "AV2 lossy QP must be non-zero");
    let expected_len = Picture::expected_len(
        geometry.width,
        geometry.height,
        stream_format.pixel_format(),
    );
    assert_eq!(
        frame.len(),
        expected_len,
        "AV2 predictive planar lossy input length must match geometry"
    );
    let mut reconstruction = vec![0; expected_len];
    let mut out = Vec::new();
    append_obu(
        &mut out,
        Av2ObuType::TemporalDelimiter,
        &Av2SyntaxPayload::default(),
    );
    if include_sequence_header {
        append_obu(
            &mut out,
            Av2ObuType::SequenceHeader,
            &av2_mvp_predictive_sequence_header_payload(
                geometry,
                Av2Black444MvpProfile::current(),
                stream_format,
            ),
        );
        append_rgb_content_interpretation_if_needed(&mut out, rgb_identity);
    }
    append_obu(
        &mut out,
        Av2ObuType::ClosedLoopKey,
        &av2_lossy_subsampled_predictive_closed_loop_key_payload(
            geometry,
            stream_format,
            frame,
            &mut reconstruction,
            qp,
            order_hint,
        ),
    );
    (out, reconstruction)
}

fn av2_lossless_subsampled_bitstream_and_reconstruction_for_frame(
    geometry: Av2VideoGeometry,
    stream_format: Av2StreamFormat,
    frame: &[u8],
    rgb_identity: bool,
) -> (Vec<u8>, Vec<u8>) {
    debug_assert!(matches!(
        stream_format.chroma_format,
        Av2ChromaFormat::Yuv420 | Av2ChromaFormat::Yuv422 | Av2ChromaFormat::Yuv444
    ));
    let expected_len = Picture::expected_len(
        geometry.width,
        geometry.height,
        stream_format.pixel_format(),
    );
    assert_eq!(
        frame.len(),
        expected_len,
        "AV2 planar lossless input length must match geometry"
    );
    let tile_layout = Av2TileLayout::lossless_subsampled_ibc_for_geometry(geometry);
    let ibc_tile_bounds = tile_layout.local_ibc_tile_bounds();
    let ibc = if AV2_ENABLE_LOSSLESS_SUBSAMPLED_IBC {
        ibc::build_local_ibc_subsampled(
            frame,
            geometry,
            stream_format.chroma_format,
            stream_format.bit_depth,
            &ibc_tile_bounds,
        )
        .ok()
        .filter(|ibc| ibc.stats().selected_copy_blocks() > 0)
    } else {
        None
    };
    let profile = if ibc.is_some() {
        Av2Black444MvpProfile::current().with_local_ibc_candidates()
    } else {
        Av2Black444MvpProfile::current()
    };
    let palette = palette::build_luma_palette_lossless(
        frame,
        geometry,
        stream_format.chroma_format,
        stream_format.bit_depth,
    )
    .ok();
    let mut reconstruction = vec![0; expected_len];
    let mut out = Vec::new();
    append_obu(
        &mut out,
        Av2ObuType::TemporalDelimiter,
        &Av2SyntaxPayload::default(),
    );
    append_obu(
        &mut out,
        Av2ObuType::SequenceHeader,
        &av2_mvp_sequence_header_payload(geometry, profile, stream_format),
    );
    append_rgb_content_interpretation_if_needed(&mut out, rgb_identity);
    append_obu(
        &mut out,
        Av2ObuType::ClosedLoopKey,
        &av2_lossless_subsampled_closed_loop_key_payload(
            geometry,
            stream_format,
            frame,
            &mut reconstruction,
            profile,
            palette.as_ref(),
            ibc.as_ref(),
        ),
    );
    (out, reconstruction)
}

fn av2_lossless_subsampled_predictive_key_bitstream_and_reconstruction_for_frame(
    geometry: Av2VideoGeometry,
    stream_format: Av2StreamFormat,
    frame: &[u8],
    include_sequence_header: bool,
    order_hint: u16,
    rgb_identity: bool,
) -> (Vec<u8>, Vec<u8>) {
    debug_assert!(matches!(
        stream_format.chroma_format,
        Av2ChromaFormat::Yuv420 | Av2ChromaFormat::Yuv422 | Av2ChromaFormat::Yuv444
    ));
    let expected_len = Picture::expected_len(
        geometry.width,
        geometry.height,
        stream_format.pixel_format(),
    );
    assert_eq!(
        frame.len(),
        expected_len,
        "AV2 predictive lossless input length must match geometry"
    );
    let tile_layout = Av2TileLayout::lossless_subsampled_ibc_for_geometry(geometry);
    let ibc_tile_bounds = tile_layout.local_ibc_tile_bounds();
    let ibc = if AV2_ENABLE_LOSSLESS_SUBSAMPLED_IBC {
        ibc::build_local_ibc_subsampled(
            frame,
            geometry,
            stream_format.chroma_format,
            stream_format.bit_depth,
            &ibc_tile_bounds,
        )
        .ok()
        .filter(|ibc| ibc.stats().selected_copy_blocks() > 0)
    } else {
        None
    };
    let profile = if ibc.is_some() {
        Av2Black444MvpProfile::current().with_local_ibc_candidates()
    } else {
        Av2Black444MvpProfile::current()
    };
    let palette = palette::build_luma_palette_lossless(
        frame,
        geometry,
        stream_format.chroma_format,
        stream_format.bit_depth,
    )
    .ok();
    let mut reconstruction = vec![0; expected_len];
    let mut out = Vec::new();
    append_obu(
        &mut out,
        Av2ObuType::TemporalDelimiter,
        &Av2SyntaxPayload::default(),
    );
    if include_sequence_header {
        append_obu(
            &mut out,
            Av2ObuType::SequenceHeader,
            &av2_mvp_predictive_sequence_header_payload(geometry, profile, stream_format),
        );
        append_rgb_content_interpretation_if_needed(&mut out, rgb_identity);
    }
    append_obu(
        &mut out,
        Av2ObuType::ClosedLoopKey,
        &av2_lossless_subsampled_predictive_closed_loop_key_payload(
            geometry,
            stream_format,
            frame,
            &mut reconstruction,
            profile,
            palette.as_ref(),
            ibc.as_ref(),
            order_hint,
        ),
    );
    (out, reconstruction)
}

fn av2_lossless_subsampled_regular_sef_bitstream_and_reconstruction_for_frame(
    frame: &[u8],
    order_hint: u16,
) -> (Vec<u8>, Vec<u8>) {
    let mut out = Vec::new();
    append_obu(
        &mut out,
        Av2ObuType::TemporalDelimiter,
        &Av2SyntaxPayload::default(),
    );
    append_obu(
        &mut out,
        Av2ObuType::RegularSef,
        &av2_regular_sef_payload(order_hint),
    );
    (out, frame.to_vec())
}

fn av2_lossy_subsampled_regular_sef_bitstream_and_reconstruction_for_frame(
    reference_reconstruction: &[u8],
    order_hint: u16,
) -> (Vec<u8>, Vec<u8>) {
    let mut out = Vec::new();
    append_obu(
        &mut out,
        Av2ObuType::TemporalDelimiter,
        &Av2SyntaxPayload::default(),
    );
    append_obu(
        &mut out,
        Av2ObuType::RegularSef,
        &av2_regular_sef_payload(order_hint),
    );
    (out, reference_reconstruction.to_vec())
}

fn av2_lossy_subsampled_zero_mv_inter_tiles_bitstream_and_reconstruction_for_frame(
    geometry: Av2VideoGeometry,
    stream_format: Av2StreamFormat,
    frame: &[u8],
    reference_source: &[u8],
    reference_reconstruction: &[u8],
    qp: u8,
    order_hint: u16,
) -> Option<(Vec<u8>, Vec<u8>)> {
    let expected_len = Picture::expected_len(
        geometry.width,
        geometry.height,
        stream_format.pixel_format(),
    );
    if frame.len() != expected_len
        || reference_source.len() != expected_len
        || reference_reconstruction.len() != expected_len
    {
        return None;
    }
    let layout = planar::Av2PlanarYuvLayout::new(
        geometry,
        stream_format.chroma_format,
        stream_format.bit_depth,
    )
    .ok()?;
    let tile_layout = Av2TileLayout::lossy_subsampled_for_geometry(geometry);
    if tile_layout.is_single_tile() {
        return None;
    }

    let mut zero_mv_tiles = Vec::with_capacity(tile_layout.tile_count());
    let mut motion_search_regions = Vec::new();
    for region in &tile_layout.regions {
        let zero_mv = layout.regions_equal_between(
            frame,
            region.origin_x,
            region.origin_y,
            reference_source,
            region.origin_x,
            region.origin_y,
            region.width,
            region.height,
        );
        zero_mv_tiles.push(zero_mv);
        if !zero_mv {
            motion_search_regions.push(Av2MotionSearchRegion {
                x0: region.origin_x,
                y0: region.origin_y,
                width: region.width,
                height: region.height,
            });
        }
    }
    if motion_search_regions.is_empty() {
        return None;
    }

    let use_exact_motion_residuals = stream_format.bit_depth.bits() <= 8;
    let motion_map = use_exact_motion_residuals
        .then(|| {
            motion::build_lossless_motion_map_for_regions(
                frame,
                reference_source,
                geometry,
                stream_format.chroma_format,
                stream_format.bit_depth,
                &motion_search_regions,
            )
            .ok()
        })
        .flatten();
    let tile_modes: Vec<_> = tile_layout
        .regions
        .iter()
        .zip(zero_mv_tiles.iter())
        .map(|(region, zero_mv)| {
            if *zero_mv {
                Av2PredictiveTileMode::ZeroMv
            } else if let Some(blocks) = motion_map
                .as_ref()
                .and_then(|map| lossy_tile_inter_residual_block_modes(map, *region))
            {
                Av2PredictiveTileMode::Residual(blocks)
            } else {
                Av2PredictiveTileMode::Intra
            }
        })
        .collect();
    let has_zero_mv_tile = tile_modes
        .iter()
        .any(|mode| matches!(mode, Av2PredictiveTileMode::ZeroMv));
    let has_residual_tile = tile_modes.iter().any(|mode| {
        matches!(
            mode,
            Av2PredictiveTileMode::Intra | Av2PredictiveTileMode::Residual(_)
        )
    });
    let has_newmv_residual_tile = tile_modes
        .iter()
        .any(|mode| matches!(mode, Av2PredictiveTileMode::Residual(_)));
    if !has_residual_tile || (!has_zero_mv_tile && !has_newmv_residual_tile) {
        return None;
    }

    let profile = Av2Black444MvpProfile::current();
    let inter_qp = av2_predictive_inter_qp_for_qp(qp, stream_format.bit_depth);
    let quantization = Av2QuantizationParams::regular_qp(inter_qp, stream_format.bit_depth);
    let palette = palette::build_luma_palette_lossless(
        frame,
        geometry,
        stream_format.chroma_format,
        stream_format.bit_depth,
    )
    .ok();
    let palette_ref = palette.as_ref();
    let mut reconstruction = vec![0; expected_len];
    let mut tile_payloads = Vec::with_capacity(tile_layout.tile_count());
    for (&region, tile_mode) in tile_layout.regions.iter().zip(tile_modes.iter()) {
        match tile_mode {
            Av2PredictiveTileMode::ZeroMv => {
                if !layout.copy_region_between(
                    &mut reconstruction,
                    region.origin_x,
                    region.origin_y,
                    reference_reconstruction,
                    region.origin_x,
                    region.origin_y,
                    region.width,
                    region.height,
                ) {
                    return None;
                }
                tile_payloads.push(av2_lossless_predictive_tile_payload_for_mode(
                    region,
                    tile_mode,
                    profile,
                    geometry,
                    stream_format,
                    frame,
                    reference_source,
                    palette_ref,
                ));
            }
            Av2PredictiveTileMode::Residual(residual_blocks) => {
                tile_payloads.push(
                    av2_lossy_fixed_inter_intra_tile_entropy_payload_for_region_with_fields(
                        region,
                        profile,
                        geometry,
                        stream_format.chroma_format,
                        stream_format.bit_depth,
                        frame,
                        reference_reconstruction,
                        &mut reconstruction,
                        residual_blocks,
                        inter_qp,
                        quantization.base_qindex,
                        false,
                    ),
                );
            }
            Av2PredictiveTileMode::Intra => {
                let residual_blocks = lossless_tile_zero_mv_residual_block_modes(region)?;
                tile_payloads.push(
                    av2_lossy_fixed_inter_intra_tile_entropy_payload_for_region_with_fields(
                        region,
                        profile,
                        geometry,
                        stream_format.chroma_format,
                        stream_format.bit_depth,
                        frame,
                        reference_reconstruction,
                        &mut reconstruction,
                        &residual_blocks,
                        inter_qp,
                        quantization.base_qindex,
                        false,
                    ),
                );
            }
            Av2PredictiveTileMode::NewMv(_)
            | Av2PredictiveTileMode::Mixed(_)
            | Av2PredictiveTileMode::MixedInterIntraOrResidual { .. } => return None,
        }
    }

    let mut payload =
        av2_mvp_regular_inter_header_payload(&tile_layout, stream_format, quantization, order_hint);
    let tile_payload = tile_group_payload_from_entropy(&tile_payloads);
    let bit_offset = payload.bytes.len() * 8;
    payload.fields.push(syntax::Av2SyntaxField {
        name: "tile_group.tile_entropy_payload",
        code: syntax::Av2SyntaxCode::TileEntropyPayload,
        bit_offset,
        bit_count: tile_payload.len() * 8,
    });
    payload.bytes.extend_from_slice(&tile_payload);

    let mut out = Vec::new();
    append_obu(
        &mut out,
        Av2ObuType::TemporalDelimiter,
        &Av2SyntaxPayload::default(),
    );
    append_obu(&mut out, Av2ObuType::RegularTileGroup, &payload);
    Some((out, reconstruction))
}

fn av2_lossless_subsampled_regular_inter_tiles_bitstream_and_reconstruction_for_frame(
    geometry: Av2VideoGeometry,
    stream_format: Av2StreamFormat,
    frame: &[u8],
    reference: &[u8],
    order_hint: u16,
) -> Option<(Vec<u8>, Vec<u8>)> {
    let expected_len = Picture::expected_len(
        geometry.width,
        geometry.height,
        stream_format.pixel_format(),
    );
    if frame.len() != expected_len || reference.len() != expected_len {
        return None;
    }
    let layout = planar::Av2PlanarYuvLayout::new(
        geometry,
        stream_format.chroma_format,
        stream_format.bit_depth,
    )
    .ok()?;
    let tile_layout = Av2TileLayout::lossless_subsampled_fast_for_geometry(geometry);
    if tile_layout.is_single_tile() {
        return None;
    }

    let mut zero_mv_tiles = Vec::with_capacity(tile_layout.tile_count());
    let mut motion_search_regions = Vec::new();
    for region in &tile_layout.regions {
        let zero_mv = layout.regions_equal_between(
            frame,
            region.origin_x,
            region.origin_y,
            reference,
            region.origin_x,
            region.origin_y,
            region.width,
            region.height,
        );
        zero_mv_tiles.push(zero_mv);
        if !zero_mv {
            motion_search_regions.push(Av2MotionSearchRegion {
                x0: region.origin_x,
                y0: region.origin_y,
                width: region.width,
                height: region.height,
            });
        }
    }
    if motion_search_regions.is_empty() {
        return None;
    }

    let motion_map = motion::build_lossless_motion_map_for_regions(
        frame,
        reference,
        geometry,
        stream_format.chroma_format,
        stream_format.bit_depth,
        &motion_search_regions,
    )
    .ok();
    let tile_modes: Vec<_> = tile_layout
        .regions
        .iter()
        .zip(zero_mv_tiles.iter())
        .map(|(region, zero_mv)| {
            if *zero_mv {
                Av2PredictiveTileMode::ZeroMv
            } else if let Some(mv) = motion_map
                .as_ref()
                .and_then(|map| uniform_lossless_tile_motion(map, *region))
            {
                Av2PredictiveTileMode::NewMv(mv)
            } else if let Some(blocks) = motion_map
                .as_ref()
                .and_then(|map| lossless_tile_inter_block_modes(map, *region))
            {
                Av2PredictiveTileMode::Mixed(blocks)
            } else if let Some(intra_blocks) = motion_map
                .as_ref()
                .and_then(|map| lossless_tile_inter_intra_block_modes(map, *region))
            {
                let residual_blocks = motion_map
                    .as_ref()
                    .and_then(|map| lossless_tile_inter_residual_block_modes(map, *region))
                    .expect("mixed inter/intra blocks should also form residual candidates");
                Av2PredictiveTileMode::MixedInterIntraOrResidual {
                    intra_blocks,
                    residual_blocks,
                }
            } else {
                Av2PredictiveTileMode::Intra
            }
        })
        .collect();
    let inter_tile_count = tile_modes
        .iter()
        .filter(|mode| !matches!(mode, Av2PredictiveTileMode::Intra))
        .count();
    let all_zero_mv = tile_modes
        .iter()
        .all(|mode| matches!(mode, Av2PredictiveTileMode::ZeroMv));
    if inter_tile_count == 0 || all_zero_mv {
        return None;
    }

    let profile = Av2Black444MvpProfile::current();
    let palette = palette::build_luma_palette_lossless(
        frame,
        geometry,
        stream_format.chroma_format,
        stream_format.bit_depth,
    )
    .ok();
    let palette_ref = palette.as_ref();
    let tile_payloads: Vec<_> = if tile_layout.tile_count() > 1 {
        std::thread::scope(|scope| {
            let handles: Vec<_> = tile_layout
                .regions
                .iter()
                .zip(tile_modes.iter())
                .map(|(&region, tile_mode)| {
                    scope.spawn(move || {
                        av2_lossless_predictive_tile_payload_for_mode(
                            region,
                            tile_mode,
                            profile,
                            geometry,
                            stream_format,
                            frame,
                            reference,
                            palette_ref,
                        )
                    })
                })
                .collect();
            handles
                .into_iter()
                .map(|handle| handle.join().expect("AV2 predictive tile worker panicked"))
                .collect()
        })
    } else {
        tile_layout
            .regions
            .iter()
            .zip(tile_modes.iter())
            .map(|(&region, tile_mode)| {
                av2_lossless_predictive_tile_payload_for_mode(
                    region,
                    tile_mode,
                    profile,
                    geometry,
                    stream_format,
                    frame,
                    reference,
                    palette_ref,
                )
            })
            .collect()
    };

    let mut payload = av2_mvp_regular_inter_header_payload(
        &tile_layout,
        stream_format,
        Av2QuantizationParams::lossless(),
        order_hint,
    );
    let tile_payload = tile_group_payload_from_entropy(&tile_payloads);
    let bit_offset = payload.bytes.len() * 8;
    payload.fields.push(syntax::Av2SyntaxField {
        name: "tile_group.tile_entropy_payload",
        code: syntax::Av2SyntaxCode::TileEntropyPayload,
        bit_offset,
        bit_count: tile_payload.len() * 8,
    });
    payload.bytes.extend_from_slice(&tile_payload);

    let mut out = Vec::new();
    append_obu(
        &mut out,
        Av2ObuType::TemporalDelimiter,
        &Av2SyntaxPayload::default(),
    );
    append_obu(&mut out, Av2ObuType::RegularTileGroup, &payload);
    Some((out, frame.to_vec()))
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Av2PredictiveTileMode {
    ZeroMv,
    NewMv(Av2MotionVector),
    Mixed(Av2LosslessInterTileBlockModes),
    Residual(Av2LosslessInterTileBlockModes),
    MixedInterIntraOrResidual {
        intra_blocks: Av2LosslessInterTileBlockModes,
        residual_blocks: Av2LosslessInterTileBlockModes,
    },
    Intra,
}

fn av2_lossless_predictive_tile_payload_for_mode(
    region: Av2TileRegion,
    tile_mode: &Av2PredictiveTileMode,
    profile: Av2Black444MvpProfile,
    geometry: Av2VideoGeometry,
    stream_format: Av2StreamFormat,
    frame: &[u8],
    reference: &[u8],
    palette: Option<&Av2LumaPalette444>,
) -> entropy::Av2EntropyPayload {
    match tile_mode {
        Av2PredictiveTileMode::ZeroMv => {
            av2_lossless_zero_mv_inter_tile_entropy_payload_for_region_with_fields(
                region,
                profile,
                stream_format.chroma_format,
                false,
            )
        }
        Av2PredictiveTileMode::NewMv(mv) => {
            av2_lossless_new_mv_inter_tile_entropy_payload_for_region_with_fields(
                region,
                profile,
                stream_format.chroma_format,
                mv.row_px,
                mv.col_px,
                false,
            )
        }
        Av2PredictiveTileMode::Mixed(blocks) => {
            av2_lossless_mixed_inter_tile_entropy_payload_for_region_with_fields(
                region,
                profile,
                stream_format.chroma_format,
                blocks,
                false,
            )
        }
        Av2PredictiveTileMode::Residual(_) => {
            unreachable!("lossless predictive path does not emit lossy residual tile modes")
        }
        Av2PredictiveTileMode::MixedInterIntraOrResidual {
            intra_blocks,
            residual_blocks,
        } => {
            let mut scratch_reconstruction = Vec::new();
            let residual_payload =
                av2_lossless_mixed_inter_intra_tile_entropy_payload_for_region_with_fields(
                    region,
                    profile,
                    geometry,
                    stream_format.chroma_format,
                    stream_format.bit_depth,
                    frame,
                    reference,
                    &mut scratch_reconstruction,
                    palette,
                    residual_blocks,
                    false,
                );
            if av2_lossless_residual_payload_is_decisive(
                &residual_payload,
                region,
                stream_format.chroma_format,
                stream_format.bit_depth,
            ) {
                return residual_payload;
            }

            scratch_reconstruction.clear();
            let intra_payload =
                av2_lossless_mixed_inter_intra_tile_entropy_payload_for_region_with_fields(
                    region,
                    profile,
                    geometry,
                    stream_format.chroma_format,
                    stream_format.bit_depth,
                    frame,
                    reference,
                    &mut scratch_reconstruction,
                    palette,
                    intra_blocks,
                    false,
                );
            if av2_entropy_payload_rate_key(&residual_payload)
                < av2_entropy_payload_rate_key(&intra_payload)
            {
                residual_payload
            } else {
                intra_payload
            }
        }
        Av2PredictiveTileMode::Intra => {
            let mut scratch_reconstruction = Vec::new();
            av2_lossless_subsampled_regular_inter_intra_tile_entropy_payload_for_region_with_fields(
                region,
                profile,
                geometry,
                stream_format.chroma_format,
                stream_format.bit_depth,
                frame,
                &mut scratch_reconstruction,
                palette,
                false,
            )
        }
    }
}

fn av2_entropy_payload_rate_key(payload: &entropy::Av2EntropyPayload) -> (usize, usize) {
    (payload.bytes.len(), payload.symbol_bits)
}

const AV2_LOSSLESS_RESIDUAL_SHORTCUT_SOURCE_DENOMINATOR: usize = 32;

fn av2_lossless_residual_payload_is_decisive(
    payload: &entropy::Av2EntropyPayload,
    region: Av2TileRegion,
    chroma_format: Av2ChromaFormat,
    bit_depth: SampleBitDepth,
) -> bool {
    let chroma_samples = match chroma_format {
        Av2ChromaFormat::Yuv420 => region.width * region.height / 2,
        Av2ChromaFormat::Yuv422 => region.width * region.height,
        Av2ChromaFormat::Yuv444 => region.width * region.height * 2,
    };
    let source_bytes =
        (region.width * region.height + chroma_samples) * bit_depth.bytes_per_sample();
    payload.bytes.len() * AV2_LOSSLESS_RESIDUAL_SHORTCUT_SOURCE_DENOMINATOR <= source_bytes
}

fn uniform_lossless_tile_motion(
    motion_map: &Av2LosslessMotionMap,
    region: Av2TileRegion,
) -> Option<Av2MotionVector> {
    if region.origin_x % AV2_LOSSLESS_ME_BLOCK_SIZE != 0
        || region.origin_y % AV2_LOSSLESS_ME_BLOCK_SIZE != 0
        || region.width % AV2_LOSSLESS_ME_BLOCK_SIZE != 0
        || region.height % AV2_LOSSLESS_ME_BLOCK_SIZE != 0
    {
        return None;
    }

    let mut selected = None;
    for y in (region.origin_y..region.origin_y + region.height).step_by(AV2_LOSSLESS_ME_BLOCK_SIZE)
    {
        for x in
            (region.origin_x..region.origin_x + region.width).step_by(AV2_LOSSLESS_ME_BLOCK_SIZE)
        {
            let block = motion_map.candidate_at(x, y)?;
            if block.mv.row_px == 0 && block.mv.col_px == 0 {
                return None;
            }
            match selected {
                Some(mv) if mv != block.mv => return None,
                Some(_) => {}
                None => selected = Some(block.mv),
            }
        }
    }
    selected
}

fn lossless_tile_inter_block_modes(
    motion_map: &Av2LosslessMotionMap,
    region: Av2TileRegion,
) -> Option<Av2LosslessInterTileBlockModes> {
    if region.origin_x % AV2_LOSSLESS_ME_BLOCK_SIZE != 0
        || region.origin_y % AV2_LOSSLESS_ME_BLOCK_SIZE != 0
        || region.width % AV2_LOSSLESS_ME_BLOCK_SIZE != 0
        || region.height % AV2_LOSSLESS_ME_BLOCK_SIZE != 0
    {
        return None;
    }

    let blocks_wide = region.width / AV2_LOSSLESS_ME_BLOCK_SIZE;
    let blocks_high = region.height / AV2_LOSSLESS_ME_BLOCK_SIZE;
    let mut blocks = Vec::with_capacity(blocks_wide * blocks_high);
    let mut has_nonzero = false;
    for y in (region.origin_y..region.origin_y + region.height).step_by(AV2_LOSSLESS_ME_BLOCK_SIZE)
    {
        for x in
            (region.origin_x..region.origin_x + region.width).step_by(AV2_LOSSLESS_ME_BLOCK_SIZE)
        {
            let block = motion_map.candidate_at(x, y)?;
            if block.mv.row_px == 0 && block.mv.col_px == 0 {
                blocks.push(Av2LosslessInterBlockMode::ZeroMv);
            } else {
                has_nonzero = true;
                blocks.push(Av2LosslessInterBlockMode::NewMv {
                    row_px: block.mv.row_px,
                    col_px: block.mv.col_px,
                });
            }
        }
    }
    has_nonzero.then(|| Av2LosslessInterTileBlockModes::new(blocks_wide, blocks_high, blocks))
}

fn lossless_tile_inter_intra_block_modes(
    motion_map: &Av2LosslessMotionMap,
    region: Av2TileRegion,
) -> Option<Av2LosslessInterTileBlockModes> {
    lossless_tile_mixed_inter_block_modes(motion_map, region, Av2LosslessInterBlockMode::Intra)
}

fn lossless_tile_inter_residual_block_modes(
    motion_map: &Av2LosslessMotionMap,
    region: Av2TileRegion,
) -> Option<Av2LosslessInterTileBlockModes> {
    lossless_tile_mixed_inter_block_modes(
        motion_map,
        region,
        Av2LosslessInterBlockMode::ZeroMvResidual,
    )
}

fn lossy_tile_inter_residual_block_modes(
    motion_map: &Av2LosslessMotionMap,
    region: Av2TileRegion,
) -> Option<Av2LosslessInterTileBlockModes> {
    if region.origin_x % AV2_LOSSLESS_ME_BLOCK_SIZE != 0
        || region.origin_y % AV2_LOSSLESS_ME_BLOCK_SIZE != 0
        || region.width % AV2_LOSSLESS_ME_BLOCK_SIZE != 0
        || region.height % AV2_LOSSLESS_ME_BLOCK_SIZE != 0
    {
        return None;
    }

    let blocks_wide = region.width / AV2_LOSSLESS_ME_BLOCK_SIZE;
    let blocks_high = region.height / AV2_LOSSLESS_ME_BLOCK_SIZE;
    let mut blocks = Vec::with_capacity(blocks_wide * blocks_high);
    let mut has_newmv_residual = false;
    for y in (region.origin_y..region.origin_y + region.height).step_by(AV2_LOSSLESS_ME_BLOCK_SIZE)
    {
        for x in
            (region.origin_x..region.origin_x + region.width).step_by(AV2_LOSSLESS_ME_BLOCK_SIZE)
        {
            if let Some(block) = motion_map.candidate_at(x, y) {
                if block.mv.row_px != 0 || block.mv.col_px != 0 {
                    has_newmv_residual = true;
                    blocks.push(Av2LosslessInterBlockMode::NewMvResidual {
                        row_px: block.mv.row_px,
                        col_px: block.mv.col_px,
                    });
                    continue;
                }
            }
            blocks.push(Av2LosslessInterBlockMode::ZeroMvResidual);
        }
    }

    has_newmv_residual
        .then(|| Av2LosslessInterTileBlockModes::new(blocks_wide, blocks_high, blocks))
}

fn lossless_tile_zero_mv_residual_block_modes(
    region: Av2TileRegion,
) -> Option<Av2LosslessInterTileBlockModes> {
    if region.origin_x % AV2_LOSSLESS_ME_BLOCK_SIZE != 0
        || region.origin_y % AV2_LOSSLESS_ME_BLOCK_SIZE != 0
        || region.width % AV2_LOSSLESS_ME_BLOCK_SIZE != 0
        || region.height % AV2_LOSSLESS_ME_BLOCK_SIZE != 0
    {
        return None;
    }

    let blocks_wide = region.width / AV2_LOSSLESS_ME_BLOCK_SIZE;
    let blocks_high = region.height / AV2_LOSSLESS_ME_BLOCK_SIZE;
    Some(Av2LosslessInterTileBlockModes::new(
        blocks_wide,
        blocks_high,
        vec![Av2LosslessInterBlockMode::ZeroMvResidual; blocks_wide * blocks_high],
    ))
}

fn lossless_tile_mixed_inter_block_modes(
    motion_map: &Av2LosslessMotionMap,
    region: Av2TileRegion,
    missing_block_mode: Av2LosslessInterBlockMode,
) -> Option<Av2LosslessInterTileBlockModes> {
    if region.origin_x % AV2_LOSSLESS_ME_BLOCK_SIZE != 0
        || region.origin_y % AV2_LOSSLESS_ME_BLOCK_SIZE != 0
        || region.width % AV2_LOSSLESS_ME_BLOCK_SIZE != 0
        || region.height % AV2_LOSSLESS_ME_BLOCK_SIZE != 0
    {
        return None;
    }

    let blocks_wide = region.width / AV2_LOSSLESS_ME_BLOCK_SIZE;
    let blocks_high = region.height / AV2_LOSSLESS_ME_BLOCK_SIZE;
    let mut blocks = Vec::with_capacity(blocks_wide * blocks_high);
    let mut exact_inter_blocks = 0usize;
    for y in (region.origin_y..region.origin_y + region.height).step_by(AV2_LOSSLESS_ME_BLOCK_SIZE)
    {
        for x in
            (region.origin_x..region.origin_x + region.width).step_by(AV2_LOSSLESS_ME_BLOCK_SIZE)
        {
            if let Some(block) = motion_map.candidate_at(x, y) {
                if block.mv.row_px == 0 && block.mv.col_px == 0 {
                    blocks.push(Av2LosslessInterBlockMode::ZeroMv);
                } else {
                    blocks.push(Av2LosslessInterBlockMode::NewMv {
                        row_px: block.mv.row_px,
                        col_px: block.mv.col_px,
                    });
                }
                exact_inter_blocks += 1;
            } else {
                blocks.push(missing_block_mode);
            }
        }
    }
    if exact_inter_blocks == 0 || exact_inter_blocks == blocks.len() {
        return None;
    }
    Some(Av2LosslessInterTileBlockModes::new(
        blocks_wide,
        blocks_high,
        blocks,
    ))
}

fn av2_order_hint_for_frame(frame_index: usize) -> u16 {
    let mask = (1u16 << AV2_PREDICTIVE_ORDER_HINT_BITS) - 1;
    (frame_index as u16) & mask
}

fn av2_mvp_444_bitstream_for_mode(
    geometry: Av2VideoGeometry,
    bit_depth: SampleBitDepth,
    frame_mode: &Av2Mvp444FrameMode,
    rgb_identity: bool,
) -> Vec<u8> {
    let mut out = Vec::new();
    append_obu(
        &mut out,
        Av2ObuType::TemporalDelimiter,
        &Av2SyntaxPayload::default(),
    );
    append_obu(
        &mut out,
        Av2ObuType::SequenceHeader,
        &av2_mvp_444_sequence_header_payload(geometry, bit_depth, frame_mode.profile()),
    );
    append_rgb_content_interpretation_if_needed(&mut out, rgb_identity);
    append_obu(
        &mut out,
        Av2ObuType::ClosedLoopKey,
        &av2_mvp_444_closed_loop_key_payload(geometry, bit_depth, frame_mode),
    );
    out
}

pub fn av2_mvp_444_trace_jsonl_for_frame(
    frame: &[u8],
    request: Av2EncodeRequest,
) -> Result<String, String> {
    request.validate()?;
    let geometry = validate_mvp_request(request)?;
    let stream_format = Av2StreamFormat::from_pixel_format(request.format)
        .expect("validate_mvp_request accepts only supported AV2 stream formats");
    let coded_frame: Vec<u8>;
    let frame = if request.format == PixelFormat::Rgb24 {
        coded_frame = rgb24_to_planar_gbr(frame, geometry);
        coded_frame.as_slice()
    } else {
        frame
    };
    if stream_format.chroma_format == Av2ChromaFormat::Yuv420 {
        let black = av2_black_reconstruction_for_geometry(geometry, stream_format);
        if frame != black {
            return av2_lossy_subsampled_trace_jsonl_for_frame(
                geometry,
                stream_format,
                frame,
                AV2_LOSSY_DEFAULT_QP,
            );
        }
        return av2_black_trace_jsonl_for_format(geometry, stream_format);
    }
    let frame_mode = Av2Mvp444FrameMode::from_frame(frame, geometry, stream_format.bit_depth)?;
    av2_mvp_444_trace_jsonl_for_mode(geometry, stream_format.bit_depth, &frame_mode)
}

pub fn av2_mvp_444_ibc_stats_json_for_frame(
    frame: &[u8],
    request: Av2EncodeRequest,
) -> Result<String, String> {
    request.validate()?;
    let geometry = validate_mvp_request(request)?;
    let stream_format = Av2StreamFormat::from_pixel_format(request.format)
        .expect("validate_mvp_request accepts only supported AV2 stream formats");
    if stream_format.chroma_format != Av2ChromaFormat::Yuv444 {
        return Err(format!(
            "AV2 IBC stats expect yuv444p8 or yuv444p10le input; got {}",
            request.format
        ));
    }

    let coded_frame: Vec<u8>;
    let frame = if request.format == PixelFormat::Rgb24 {
        coded_frame = rgb24_to_planar_gbr(frame, geometry);
        coded_frame.as_slice()
    } else {
        frame
    };
    let frame_mode = Av2Mvp444FrameMode::from_frame(frame, geometry, stream_format.bit_depth)?;
    let (black_mode, stats) = match &frame_mode {
        Av2Mvp444FrameMode::Black => (true, Av2LocalIbcStats::default()),
        Av2Mvp444FrameMode::LumaPalette { ibc, .. } => (
            false,
            ibc.as_ref().map(Av2LocalIbc444::stats).unwrap_or_default(),
        ),
    };

    Ok(format!(
        concat!(
            "{{\n",
            "  \"codec\": \"av2\",\n",
            "  \"tool\": \"local_hash_ibc\",\n",
            "  \"width\": {},\n",
            "  \"height\": {},\n",
            "  \"format\": \"{}\",\n",
            "  \"black_mode\": {},\n",
            "  \"allow_intrabc\": {},\n",
            "  \"total_blocks\": {},\n",
            "  \"blocks_with_above_in_tile\": {},\n",
            "  \"blocks_with_left_in_tile\": {},\n",
            "  \"fixed_drl_supported_blocks\": {},\n",
            "  \"raw_above_hash_matches\": {},\n",
            "  \"raw_left_hash_matches\": {},\n",
            "  \"direct_above_hash_matches\": {},\n",
            "  \"direct_left_hash_matches\": {},\n",
            "  \"above_hash_matches_blocked_by_fixed_drl_guard\": {},\n",
            "  \"left_hash_matches_blocked_by_fixed_drl_guard\": {},\n",
            "  \"above_hash_matches_blocked_by_copied_candidate\": {},\n",
            "  \"left_hash_matches_blocked_by_copied_candidate\": {},\n",
            "  \"selected_above_copy_blocks\": {},\n",
            "  \"selected_left_copy_blocks\": {},\n",
            "  \"selected_copy_blocks\": {}\n",
            "}}\n"
        ),
        geometry.width,
        geometry.height,
        request.format,
        black_mode,
        frame_mode.allow_intrabc(),
        stats.total_blocks,
        stats.blocks_with_above_in_tile,
        stats.blocks_with_left_in_tile,
        stats.fixed_drl_supported_blocks,
        stats.raw_above_hash_matches,
        stats.raw_left_hash_matches,
        stats.direct_above_hash_matches,
        stats.direct_left_hash_matches,
        stats.above_hash_matches_blocked_by_fixed_drl_guard,
        stats.left_hash_matches_blocked_by_fixed_drl_guard,
        stats.above_hash_matches_blocked_by_copied_candidate,
        stats.left_hash_matches_blocked_by_copied_candidate,
        stats.selected_above_copy_blocks,
        stats.selected_left_copy_blocks,
        stats.selected_copy_blocks(),
    ))
}

pub fn av2_black_444_trace_jsonl(request: Av2EncodeRequest) -> Result<String, String> {
    request.validate()?;
    let geometry = validate_fixed_black_444_request(request)?;
    av2_mvp_444_trace_jsonl_for_mode(
        geometry,
        request.format.bit_depth(),
        &Av2Mvp444FrameMode::Black,
    )
}

fn av2_mvp_444_trace_jsonl_for_mode(
    geometry: Av2VideoGeometry,
    bit_depth: SampleBitDepth,
    frame_mode: &Av2Mvp444FrameMode,
) -> Result<String, String> {
    let tile_layout = av2_tile_layout_for_frame_mode(geometry, frame_mode);
    let sequence = av2_mvp_444_sequence_header_payload(geometry, bit_depth, frame_mode.profile());
    let closed_loop_header = av2_mvp_444_closed_loop_key_header_payload(
        frame_mode.allow_screen_content_tools(),
        frame_mode.allow_intrabc(),
        &tile_layout,
        Av2StreamFormat {
            chroma_format: Av2ChromaFormat::Yuv444,
            bit_depth,
        },
        Av2QuantizationParams::lossless(),
    );
    let entropy = av2_tile_entropy_payloads_for_mode(&tile_layout, frame_mode, true);
    let mut lines = String::new();

    push_av2_trace_line(
        &mut lines,
        "obu",
        "obu.temporal_delimiter",
        "AV2 v1.0.0 Section 5.4 OBU syntax",
        "header+payload",
        0,
        16,
    );
    for field in &sequence.fields {
        push_av2_trace_line(
            &mut lines,
            "sequence_header",
            field.name,
            av2_spec_section_for_syntax_field(field.name),
            &format!("{:?}", field.code),
            field.bit_offset,
            field.bit_count,
        );
    }
    push_av2_trace_line(
        &mut lines,
        "obu",
        "obu.closed_loop_key",
        "AV2 v1.0.0 Sections 5.19 and 5.20.1 tile group syntax",
        "header",
        0,
        8,
    );
    for field in &closed_loop_header.fields {
        push_av2_trace_line(
            &mut lines,
            "closed_loop_key_header",
            field.name,
            av2_spec_section_for_syntax_field(field.name),
            &format!("{:?}", field.code),
            field.bit_offset,
            field.bit_count,
        );
    }
    for (tile_index, entropy) in entropy.iter().enumerate() {
        for field in &entropy.fields {
            push_av2_entropy_trace_line(&mut lines, tile_index, field);
        }
    }
    Ok(lines)
}

fn av2_black_trace_jsonl_for_format(
    geometry: Av2VideoGeometry,
    stream_format: Av2StreamFormat,
) -> Result<String, String> {
    let tile_layout = Av2TileLayout::for_geometry(geometry);
    let profile = Av2Black444MvpProfile::current();
    let sequence = av2_mvp_sequence_header_payload(geometry, profile, stream_format);
    let closed_loop_header = av2_mvp_444_closed_loop_key_header_payload(
        false,
        false,
        &tile_layout,
        stream_format,
        Av2QuantizationParams::lossless(),
    );
    let entropy: Vec<_> = tile_layout
        .regions
        .iter()
        .map(|&region| {
            av2_black_tile_entropy_payload_for_region(region, profile, stream_format.chroma_format)
        })
        .collect();
    let mut lines = String::new();

    push_av2_trace_line(
        &mut lines,
        "obu",
        "obu.temporal_delimiter",
        "AV2 v1.0.0 Section 5.4 OBU syntax",
        "header+payload",
        0,
        16,
    );
    for field in &sequence.fields {
        push_av2_trace_line(
            &mut lines,
            "sequence_header",
            field.name,
            av2_spec_section_for_syntax_field(field.name),
            &format!("{:?}", field.code),
            field.bit_offset,
            field.bit_count,
        );
    }
    push_av2_trace_line(
        &mut lines,
        "obu",
        "obu.closed_loop_key",
        "AV2 v1.0.0 Sections 5.19 and 5.20.1 tile group syntax",
        "header",
        0,
        8,
    );
    for field in &closed_loop_header.fields {
        push_av2_trace_line(
            &mut lines,
            "closed_loop_key_header",
            field.name,
            av2_spec_section_for_syntax_field(field.name),
            &format!("{:?}", field.code),
            field.bit_offset,
            field.bit_count,
        );
    }
    for (tile_index, entropy) in entropy.iter().enumerate() {
        for field in &entropy.fields {
            push_av2_entropy_trace_line(&mut lines, tile_index, field);
        }
    }
    Ok(lines)
}

fn av2_lossy_subsampled_trace_jsonl_for_frame(
    geometry: Av2VideoGeometry,
    stream_format: Av2StreamFormat,
    frame: &[u8],
    qp: u8,
) -> Result<String, String> {
    let expected_len = Picture::expected_len(
        geometry.width,
        geometry.height,
        stream_format.pixel_format(),
    );
    if frame.len() != expected_len {
        return Err(format!(
            "AV2 {} trace input length mismatch: expected {expected_len}, got {}",
            stream_format.pixel_format(),
            frame.len()
        ));
    }
    let tile_layout = Av2TileLayout::lossy_subsampled_for_geometry(geometry);
    let profile = Av2Black444MvpProfile::current();
    let sequence = av2_mvp_sequence_header_payload(geometry, profile, stream_format);
    let closed_loop_header = av2_mvp_444_closed_loop_key_header_payload(
        false,
        false,
        &tile_layout,
        stream_format,
        Av2QuantizationParams::regular_qp(qp, stream_format.bit_depth),
    );
    let mut reconstruction = vec![0; expected_len];
    let entropy: Vec<_> = tile_layout
        .regions
        .iter()
        .map(|&region| {
            av2_lossy_subsampled_tile_entropy_payload_for_region(
                region,
                profile,
                geometry,
                stream_format.chroma_format,
                stream_format.bit_depth,
                frame,
                &mut reconstruction,
                qp,
                Av2QuantizationParams::regular_qp(qp, stream_format.bit_depth).base_qindex,
            )
        })
        .collect();
    let mut lines = String::new();

    push_av2_trace_line(
        &mut lines,
        "obu",
        "obu.temporal_delimiter",
        "AV2 v1.0.0 Section 5.4 OBU syntax",
        "header+payload",
        0,
        16,
    );
    for field in &sequence.fields {
        push_av2_trace_line(
            &mut lines,
            "sequence_header",
            field.name,
            av2_spec_section_for_syntax_field(field.name),
            &format!("{:?}", field.code),
            field.bit_offset,
            field.bit_count,
        );
    }
    push_av2_trace_line(
        &mut lines,
        "obu",
        "obu.closed_loop_key",
        "AV2 v1.0.0 Sections 5.19 and 5.20.1 tile group syntax",
        "header",
        0,
        8,
    );
    for field in &closed_loop_header.fields {
        push_av2_trace_line(
            &mut lines,
            "closed_loop_key_header",
            field.name,
            av2_spec_section_for_syntax_field(field.name),
            &format!("{:?}", field.code),
            field.bit_offset,
            field.bit_count,
        );
    }
    for (tile_index, entropy) in entropy.iter().enumerate() {
        for field in &entropy.fields {
            push_av2_entropy_trace_line(&mut lines, tile_index, field);
        }
    }
    Ok(lines)
}

pub fn av2_black_64x64_444_reconstruction() -> Vec<u8> {
    av2_black_444_reconstruction_for_geometry(Av2VideoGeometry {
        width: 64,
        height: 64,
    })
}

pub fn av2_black_444_reconstruction(geometry: Av2VideoGeometry) -> Option<Vec<u8>> {
    validate_fixed_black_444_geometry(geometry).map(av2_black_444_reconstruction_for_geometry)
}

fn av2_black_444_reconstruction_for_geometry(geometry: Av2VideoGeometry) -> Vec<u8> {
    av2_black_444_reconstruction_for_geometry_with_depth(
        geometry,
        SampleBitDepth::new(8).expect("8-bit depth is supported"),
    )
}

fn av2_black_444_reconstruction_for_geometry_with_depth(
    geometry: Av2VideoGeometry,
    bit_depth: SampleBitDepth,
) -> Vec<u8> {
    av2_black_reconstruction_for_geometry(
        geometry,
        Av2StreamFormat {
            chroma_format: Av2ChromaFormat::Yuv444,
            bit_depth,
        },
    )
}

fn av2_black_reconstruction_for_geometry(
    geometry: Av2VideoGeometry,
    stream_format: Av2StreamFormat,
) -> Vec<u8> {
    vec![
        0;
        Picture::expected_len(
            geometry.width,
            geometry.height,
            stream_format.pixel_format(),
        )
    ]
}

fn validate_fixed_black_444_request(request: Av2EncodeRequest) -> Result<Av2VideoGeometry, String> {
    let geometry = validate_mvp_444_request(request)?;
    Ok(geometry)
}

fn validate_mvp_444_request(request: Av2EncodeRequest) -> Result<Av2VideoGeometry, String> {
    let geometry = validate_mvp_request(request)?;
    if !matches!(
        Av2StreamFormat::from_pixel_format(request.format),
        Some(Av2StreamFormat {
            chroma_format: Av2ChromaFormat::Yuv444,
            ..
        })
    ) {
        return Err("AV2 4:4:4 MVP path only supports yuv444p8, yuv444p10le, or rgb24".to_string());
    }
    Ok(geometry)
}

fn validate_mvp_request(request: Av2EncodeRequest) -> Result<Av2VideoGeometry, String> {
    if Av2StreamFormat::from_pixel_format(request.format).is_none() {
        return Err(
            "AV2 MVP encoder only supports yuv420p8/10, yuv422p8/10, yuv444p8/10, and rgb24 streams at 8-pixel geometry"
                .to_string(),
        );
    }
    validate_fixed_black_444_geometry(request.geometry)
        .ok_or_else(|| "AV2 MVP encoder only supports dimensions in 8-pixel steps".to_string())
}

fn validate_fixed_black_444_geometry(geometry: Av2VideoGeometry) -> Option<Av2VideoGeometry> {
    let supported = geometry.width >= 8
        && geometry.height >= 8
        && geometry.width % 8 == 0
        && geometry.height % 8 == 0;
    supported.then_some(geometry)
}

#[cfg(test)]
fn av2_black_444_sequence_header_payload(geometry: Av2VideoGeometry) -> Av2SyntaxPayload {
    av2_mvp_444_sequence_header_payload(
        geometry,
        SampleBitDepth::new(8).expect("8-bit depth is supported"),
        Av2Black444MvpProfile::current(),
    )
}

fn av2_mvp_444_sequence_header_payload(
    geometry: Av2VideoGeometry,
    bit_depth: SampleBitDepth,
    profile: Av2Black444MvpProfile,
) -> Av2SyntaxPayload {
    av2_mvp_sequence_header_payload(
        geometry,
        profile,
        Av2StreamFormat {
            chroma_format: Av2ChromaFormat::Yuv444,
            bit_depth,
        },
    )
}

fn av2_mvp_sequence_header_payload(
    geometry: Av2VideoGeometry,
    profile: Av2Black444MvpProfile,
    stream_format: Av2StreamFormat,
) -> Av2SyntaxPayload {
    av2_mvp_sequence_header_payload_with_mode(geometry, profile, stream_format, true)
}

fn av2_mvp_predictive_sequence_header_payload(
    geometry: Av2VideoGeometry,
    profile: Av2Black444MvpProfile,
    stream_format: Av2StreamFormat,
) -> Av2SyntaxPayload {
    av2_mvp_sequence_header_payload_with_mode(geometry, profile, stream_format, false)
}

fn append_rgb_content_interpretation_if_needed(out: &mut Vec<u8>, rgb_identity: bool) {
    if !rgb_identity {
        return;
    }
    append_obu(
        out,
        Av2ObuType::ContentInterpretation,
        &av2_rgb_identity_content_interpretation_payload(),
    );
}

fn av2_rgb_identity_content_interpretation_payload() -> Av2SyntaxPayload {
    let mut writer = Av2SyntaxWriter::new();
    writer.write_literal("content_interpretation.ci_scan_type_idc", 0, 2);
    writer.write_flag(
        "content_interpretation.ci_color_description_present_flag",
        true,
    );
    writer.write_flag(
        "content_interpretation.ci_chroma_sample_position_present_flag",
        false,
    );
    writer.write_flag(
        "content_interpretation.ci_aspect_ratio_info_present_flag",
        false,
    );
    writer.write_flag("content_interpretation.ci_timing_info_present_flag", false);
    writer.write_literal("content_interpretation.ci_reserved_zero_2bits", 0, 2);
    writer.write_rice_golomb(
        "content_interpretation.color_description_idc",
        AV2_COLOR_DESCRIPTION_IDC_SRGB,
        2,
    );
    writer.write_flag("content_interpretation.full_range_flag", true);
    writer.write_flag("content_interpretation.ci_extension_present_flag", false);
    writer.trailing_bits();
    writer.finish()
}

fn av2_mvp_sequence_header_payload_with_mode(
    geometry: Av2VideoGeometry,
    profile: Av2Black444MvpProfile,
    stream_format: Av2StreamFormat,
    single_picture_header: bool,
) -> Av2SyntaxPayload {
    let mut writer = Av2SyntaxWriter::new();
    let width_bits = av2_frame_dimension_bits(geometry.width);
    let height_bits = av2_frame_dimension_bits(geometry.height);

    // AV2 v1.0.0 sequence_header_obu(), mirrored from AVM
    // av2_write_sequence_header_obu().
    writer.write_uvlc("sequence_header.seq_header_id", 0);
    writer.write_literal(
        "sequence_header.seq_profile_idc",
        u64::from(stream_format.sequence_profile_idc()),
        AV2_PROFILE_BITS,
    );
    writer.write_flag(
        "sequence_header.single_picture_header_flag",
        single_picture_header,
    );
    writer.write_literal(
        "sequence_header.seq_max_level_idx",
        u64::from(av2_sequence_level_for_geometry(geometry)),
        AV2_LEVEL_BITS,
    );
    if av2_sequence_level_for_geometry(geometry) >= 4 && !single_picture_header {
        writer.write_flag("sequence_header.seq_tier", false);
    }
    writer.write_uvlc(
        "sequence_header.seq_chroma_format_idc",
        stream_format.chroma_format.sequence_header_idc(),
    );
    writer.write_uvlc(
        "sequence_header.bitdepth_lut_idx",
        stream_format.bitdepth_lut_index(),
    );
    if !single_picture_header {
        writer.write_literal("sequence_header.seq_lcr_id", 0, 3);
        writer.write_flag("sequence_header.still_picture", false);
        writer.write_literal("sequence_header.max_tlayer_id", 0, 2);
        writer.write_literal("sequence_header.max_mlayer_id", 0, 3);
        writer.write_flag("sequence_header.monotonic_output_order_flag", true);
    }
    writer.write_literal(
        "sequence_header.num_bits_width_minus_1",
        (width_bits - 1) as u64,
        4,
    );
    writer.write_literal(
        "sequence_header.num_bits_height_minus_1",
        (height_bits - 1) as u64,
        4,
    );
    writer.write_literal(
        "sequence_header.max_frame_width_minus_1",
        (geometry.width - 1) as u64,
        width_bits,
    );
    writer.write_literal(
        "sequence_header.max_frame_height_minus_1",
        (geometry.height - 1) as u64,
        height_bits,
    );
    writer.write_flag("sequence_header.conf_win_enabled_flag", false);

    if !single_picture_header {
        writer.write_flag(
            "sequence_header.seq_max_display_model_info_present_flag",
            false,
        );
        writer.write_flag("sequence_header.decoder_model_info_present_flag", false);
    }

    write_fixed_black_444_sequence_tools(&mut writer, profile, single_picture_header);

    writer.write_flag("sequence_header.film_grain_params_present", false);
    writer.write_flag("sequence_header.seq_extension_present_flag", false);
    writer.trailing_bits();
    writer.finish()
}

fn av2_sequence_level_for_geometry(geometry: Av2VideoGeometry) -> u8 {
    const LEVELS: &[(u8, usize, usize, usize)] = &[
        (0, 147_456, 640, 640),
        (1, 278_784, 880, 880),
        (2, 665_856, 1360, 1360),
        (3, 1_065_024, 1720, 1720),
        (4, 2_359_296, 2560, 2560),
        (6, 8_912_896, 4975, 4975),
        (10, 35_651_584, 9951, 9951),
        (14, 142_606_336, 19902, 19902),
        (18, 570_425_344, 39804, 39804),
    ];
    let picture_size = geometry.width * geometry.height;
    LEVELS
        .iter()
        .find_map(|&(level, max_picture_size, max_width, max_height)| {
            (picture_size <= max_picture_size
                && geometry.width <= max_width
                && geometry.height <= max_height)
                .then_some(level)
        })
        .unwrap_or(AV2_SEQUENCE_LEVEL_MAX)
}

fn av2_frame_dimension_bits(dimension: usize) -> u8 {
    assert!(dimension > 0, "AV2 frame dimension must be positive");
    let max_index = (dimension - 1) as u64;
    (64 - max_index.leading_zeros()) as u8
}

fn write_fixed_black_444_sequence_tools(
    writer: &mut Av2SyntaxWriter,
    profile: Av2Black444MvpProfile,
    single_picture_header: bool,
) {
    // AV2 v1.0.0 sequence_header() tool groups, mirrored from AVM
    // write_sequence_header(). Values are the fixed AVM choices for one
    // black yuv444p8 still picture in the minimum viable bitstream subset.
    writer.write_flag("sequence_partition.sb_size_is_256", false);
    writer.write_flag("sequence_partition.sb_size_is_128", false);
    writer.write_flag("sequence_partition.enable_sdp", profile.enable_sdp);
    writer.write_flag(
        "sequence_partition.enable_ext_partitions",
        profile.enable_ext_partitions,
    );
    if profile.enable_ext_partitions {
        writer.write_flag(
            "sequence_partition.enable_uneven_4way_partitions",
            profile.enable_uneven_4way_partitions,
        );
    }
    writer.write_flag("sequence_partition.max_pb_aspect_ratio_lt2", false);

    writer.write_flag("sequence_segment.enable_ext_seg", false);
    writer.write_flag("sequence_segment.seq_seg_info_present_flag", false);

    writer.write_flag("sequence_intra.enable_intra_dip", false);
    writer.write_flag(
        "sequence_intra.enable_intra_edge_filter",
        profile.enable_intra_edge_filter,
    );
    writer.write_flag("sequence_intra.enable_mrls", profile.enable_mrls);
    writer.write_flag("sequence_intra.enable_cfl_intra", profile.enable_cfl_intra);
    writer.write_literal("sequence_intra.cfl_ds_filter_index", 0, 2);
    writer.write_flag("sequence_intra.enable_mhccp", profile.enable_mhccp);
    writer.write_flag("sequence_intra.enable_ibp", profile.enable_ibp);

    if !single_picture_header {
        for _ in 1..5 {
            writer.write_flag("sequence_inter.motion_mode_enabled", false);
        }
        writer.write_flag("sequence_inter.enable_masked_compound", false);
        writer.write_flag("sequence_inter.enable_ref_frame_mvs", false);
        writer.write_literal(
            "sequence_inter.order_hint_bits_minus_1",
            u64::from(AV2_PREDICTIVE_ORDER_HINT_BITS - 1),
            4,
        );
    }
    writer.write_flag("sequence_inter.enable_refmvbank", profile.enable_refmvbank);
    writer.write_flag(
        "sequence_inter.is_drl_reorder_disable",
        profile.is_drl_reorder_disable,
    );
    if !profile.is_drl_reorder_disable {
        writer.write_flag("sequence_inter.enable_drl_reorder_constraint", false);
    }
    if !single_picture_header {
        writer.write_flag("sequence_inter.enable_explicit_ref_frame_map", false);
        writer.write_flag("sequence_inter.signal_dpb_explicit", true);
        writer.write_literal("sequence_inter.ref_frames_minus_1", 1, 4);
        writer.write_literal("sequence_inter.number_of_bits_for_lt_frame_id", 0, 3);
        writer.write_quniform(
            "sequence_inter.def_max_drl_bits_minus_min",
            AV2_MAX_MAX_DRL_BITS_MINUS_MIN_PLUS_ONE,
            0,
        );
        writer.write_flag("sequence_inter.allow_frame_max_drl_bits", false);
    }
    writer.write_quniform(
        "sequence_inter.def_max_bvp_drl_bits_minus_min",
        AV2_MAX_MAX_IBC_DRL_BITS_MINUS_MIN_PLUS_ONE,
        profile.def_max_bvp_drl_bits_minus_min,
    );
    writer.write_flag(
        "sequence_inter.allow_frame_max_bvp_drl_bits",
        profile.allow_frame_max_bvp_drl_bits,
    );
    if !single_picture_header {
        writer.write_literal("sequence_inter.num_same_ref_compound", 0, 2);
        writer.write_flag("sequence_inter.enable_tip", false);
        writer.write_flag("sequence_inter.enable_mv_traj", false);
    }
    writer.write_flag("sequence_inter.enable_bawp", profile.enable_bawp);
    if !single_picture_header {
        writer.write_flag("sequence_inter.enable_cwp", false);
        writer.write_flag("sequence_inter.enable_imp_msk_bld", false);
        writer.write_flag("sequence_inter.enable_lf_sub_pu", false);
        writer.write_literal("sequence_inter.enable_opfl_refine", 0, 2);
        writer.write_flag("sequence_inter.enable_refinemv", false);
        writer.write_flag("sequence_inter.enable_bru", false);
        writer.write_flag("sequence_inter.enable_adaptive_mvd", false);
        writer.write_flag("sequence_inter.enable_mvd_sign_derive", false);
        writer.write_flag("sequence_inter.enable_flex_mvres", false);
        writer.write_flag("sequence_inter.enable_global_motion", false);
        writer.write_flag("sequence_inter.enable_short_refresh_frame_flags", false);
    }

    if !single_picture_header {
        writer.write_flag("sequence_scc.force_screen_content_tools_select", true);
        writer.write_flag("sequence_scc.force_integer_mv_select", true);
    }

    writer.write_flag("sequence_transform.enable_fsc", profile.enable_fsc);
    if !profile.enable_fsc {
        writer.write_flag(
            "sequence_transform.enable_idtx_intra",
            profile.enable_idtx_intra,
        );
    }
    writer.write_flag("sequence_transform.enable_ist", false);
    writer.write_flag("sequence_transform.enable_inter_ist", false);
    writer.write_flag(
        "sequence_transform.enable_chroma_dctonly",
        profile.enable_chroma_dctonly,
    );
    if !single_picture_header {
        writer.write_flag("sequence_transform.enable_inter_ddt", false);
    }
    writer.write_flag("sequence_transform.reduced_tx_part_set", false);
    writer.write_flag("sequence_transform.enable_cctx", profile.enable_cctx);
    writer.write_flag("sequence_transform.enable_tcq_nonzero", false);
    writer.write_flag("sequence_transform.enable_parity_hiding", false);
    if !single_picture_header {
        writer.write_flag("sequence_transform.enable_avg_cdf", true);
        writer.write_flag("sequence_transform.avg_cdf_type", true);
    }
    writer.write_flag("sequence_transform.separate_uv_delta_q", false);
    writer.write_flag("sequence_transform.equal_ac_dc_q", true);
    writer.write_literal(
        "sequence_transform.base_uv_ac_delta_q_minus_min",
        (0 - AV2_DELTA_DCQUANT_MIN as i16) as u64,
        5,
    );
    writer.write_flag("sequence_transform.uv_ac_delta_q_enabled", false);

    writer.write_flag("sequence_filter.disable_loopfilters_across_tiles", false);
    writer.write_flag("sequence_filter.enable_cdef", false);
    writer.write_flag("sequence_filter.enable_gdf", false);
    writer.write_flag("sequence_filter.enable_restoration", false);
    writer.write_flag("sequence_filter.enable_ccso", false);
    if !single_picture_header {
        writer.write_flag("sequence_filter.enable_cdef_on_skip_txfm_always_on", false);
        writer.write_flag("sequence_filter.enable_cdef_on_skip_txfm_disabled", true);
    }
    writer.write_literal("sequence_filter.df_par_bits_minus2", 1, 2);

    writer.write_flag("sequence_tile_config.seq_tile_info_present_flag", false);
}

#[cfg(test)]
fn av2_black_444_closed_loop_key_header_payload() -> Av2SyntaxPayload {
    av2_mvp_444_closed_loop_key_header_payload(
        false,
        false,
        &Av2TileLayout::for_geometry(Av2VideoGeometry {
            width: 64,
            height: 64,
        }),
        Av2StreamFormat::yuv444_8(),
        Av2QuantizationParams::lossless(),
    )
}

fn av2_mvp_444_closed_loop_key_header_payload(
    allow_screen_content_tools: bool,
    allow_intrabc: bool,
    tile_layout: &Av2TileLayout,
    stream_format: Av2StreamFormat,
    quantization: Av2QuantizationParams,
) -> Av2SyntaxPayload {
    av2_mvp_444_closed_loop_key_header_payload_with_mode(
        allow_screen_content_tools,
        allow_intrabc,
        tile_layout,
        stream_format,
        quantization,
        true,
        0,
    )
}

fn av2_mvp_444_predictive_closed_loop_key_header_payload(
    allow_screen_content_tools: bool,
    allow_intrabc: bool,
    tile_layout: &Av2TileLayout,
    stream_format: Av2StreamFormat,
    quantization: Av2QuantizationParams,
    order_hint: u16,
) -> Av2SyntaxPayload {
    av2_mvp_444_closed_loop_key_header_payload_with_mode(
        allow_screen_content_tools,
        allow_intrabc,
        tile_layout,
        stream_format,
        quantization,
        false,
        order_hint,
    )
}

fn av2_mvp_444_closed_loop_key_header_payload_with_mode(
    allow_screen_content_tools: bool,
    allow_intrabc: bool,
    tile_layout: &Av2TileLayout,
    stream_format: Av2StreamFormat,
    quantization: Av2QuantizationParams,
    single_picture_header: bool,
    order_hint: u16,
) -> Av2SyntaxPayload {
    let profile = Av2Black444MvpProfile::current();
    let mut writer = Av2SyntaxWriter::new();

    // AV2 v1.0.0 tile_group_obu() for an OBU_CLOSED_LOOP_KEY.
    // The uncompressed header follows AVM write_tilegroup_header() and
    // write_uncompressed_header(). The tile entropy payload is generated by
    // the AV2 range writer below. FrameForge fixes the current MVP to uniform
    // 64x64 superblock tiles and 8x8 coding leaves; each tile resets the local
    // syntax contexts so no prediction state crosses superblock boundaries.
    writer.write_flag("tile_group.first_tile_group_in_frame", true);
    writer.write_uvlc("uncompressed_header.cur_mfh_id", 0);
    writer.write_uvlc("uncompressed_header.seq_header_id", 0);
    if !single_picture_header {
        writer.write_flag("uncompressed_header.immediate_output_picture", true);
        writer.write_flag("uncompressed_header.frame_size_override_flag", false);
        writer.write_literal(
            "uncompressed_header.order_hint",
            u64::from(order_hint),
            AV2_PREDICTIVE_ORDER_HINT_BITS,
        );
    }
    writer.write_flag(
        "uncompressed_header.allow_screen_content_tools",
        allow_screen_content_tools,
    );
    if allow_screen_content_tools {
        // AV2 v1.0.0 is_intraBC_bv_precision_active(): forcing integer
        // precision suppresses the optional intrabc_bv_precision symbol for
        // intrabc_mode=0. Keep this tied to the current local-IBC path so
        // non-IBC screen-content frames retain their previous header.
        writer.write_flag(
            "uncompressed_header.cur_frame_force_integer_mv",
            allow_intrabc,
        );
    }
    writer.write_flag("uncompressed_header.allow_intrabc", allow_intrabc);
    if allow_intrabc {
        // AV2 v1.0.0 read_intrabc_params(): key frames signal global/local
        // availability after allow_intrabc. FrameForge's first IBC path is
        // local to the current 64x64 tile, so allow_global_intrabc is false
        // and allow_local_intrabc is inferred true by AVM.
        writer.write_flag("uncompressed_header.allow_global_intrabc", false);
    }
    writer.write_flag(
        "uncompressed_header.disable_cdf_update",
        profile.disable_cdf_update,
    );
    write_mvp_tile_info(&mut writer, tile_layout);
    write_av2_quantization_params(&mut writer, stream_format, quantization);
    writer.write_flag("segmentation.enabled", false);
    write_av2_quantization_matrix_params(&mut writer, quantization);
    write_av2_delta_q_params(&mut writer, quantization);
    write_av2_post_quantization_frame_tools(&mut writer, quantization);
    if !tile_layout.is_single_tile() {
        // AV2 v1.0.0 tile_group_obu(): a single tile group covering all tiles
        // still emits tile_start_and_end_present_flag when tiles_log2 > 0.
        // AVM write_tilegroup_header() packs this immediately after
        // write_uncompressed_header(); the tile-group header byte count is
        // rounded up only after this flag has been written.
        writer.write_flag("tile_group.tile_start_and_end_present_flag", false);
    }
    writer.byte_align_zero("tile_group.header_byte_alignment");

    writer.finish()
}

fn write_av2_quantization_params(
    writer: &mut Av2SyntaxWriter,
    stream_format: Av2StreamFormat,
    quantization: Av2QuantizationParams,
) {
    debug_assert!(quantization.base_qindex <= av2_max_qindex(stream_format.bit_depth));
    writer.write_literal(
        "quantization.base_qindex",
        u64::from(quantization.base_qindex),
        av2_qindex_bits(stream_format.bit_depth),
    );
}

fn write_av2_quantization_matrix_params(
    writer: &mut Av2SyntaxWriter,
    quantization: Av2QuantizationParams,
) {
    writer.write_flag(
        "quantization_matrix.using_qmatrix",
        quantization.using_qmatrix,
    );
}

fn write_av2_delta_q_params(writer: &mut Av2SyntaxWriter, quantization: Av2QuantizationParams) {
    if quantization.base_qindex == 0 {
        debug_assert!(!quantization.delta_q.present);
        return;
    }
    writer.write_flag("delta_q.present", quantization.delta_q.present);
    if quantization.delta_q.present {
        // TODO(av2-lossy): enable this after regular lossy coefficient coding
        // tracks and emits per-SB qindex changes.
        debug_assert!(quantization.delta_q.resolution_log2 <= 2);
        writer.write_literal(
            "delta_q.resolution_log2",
            u64::from(quantization.delta_q.resolution_log2),
            2,
        );
    }
}

fn write_av2_post_quantization_frame_tools(
    writer: &mut Av2SyntaxWriter,
    quantization: Av2QuantizationParams,
) {
    if !quantization.is_coded_lossless() {
        writer.write_flag("loop_filter.apply_deblocking_filter_y_vertical", false);
        writer.write_flag("loop_filter.apply_deblocking_filter_y_horizontal", false);
        writer.write_flag("uncompressed_header.tx_mode_select", true);
    }
    writer.write_literal(
        "uncompressed_header.reduced_tx_set_used",
        if quantization.is_coded_lossless() {
            0
        } else {
            2
        },
        2,
    );
}

fn write_mvp_tile_info(writer: &mut Av2SyntaxWriter, tile_layout: &Av2TileLayout) {
    writer.write_flag("tile_info.uniform_spacing_flag", true);
    write_uniform_tile_log2(
        writer,
        "tile_info.increment_log2_cols",
        "tile_info.stop_log2_cols",
        tile_layout.min_log2_cols,
        tile_layout.log2_cols,
        tile_layout.max_log2_cols,
    );
    write_uniform_tile_log2(
        writer,
        "tile_info.increment_log2_rows",
        "tile_info.stop_log2_rows",
        tile_layout.min_log2_rows,
        tile_layout.log2_rows,
        tile_layout.max_log2_rows,
    );
    if !tile_layout.is_single_tile() {
        writer.write_literal(
            "tile_info.tile_size_bytes_minus1",
            (AV2_TILE_SIZE_BYTES - 1) as u64,
            2,
        );
    }
}

fn write_uniform_tile_log2(
    writer: &mut Av2SyntaxWriter,
    increment_name: &'static str,
    stop_name: &'static str,
    min_log2: u8,
    target_log2: u8,
    max_log2: u8,
) {
    assert!(min_log2 <= target_log2);
    assert!(target_log2 <= max_log2);
    for _ in min_log2..target_log2 {
        writer.write_flag(increment_name, true);
    }
    if target_log2 < max_log2 {
        writer.write_flag(stop_name, false);
    }
}

#[cfg(test)]
fn av2_black_444_closed_loop_key_payload(geometry: Av2VideoGeometry) -> Av2SyntaxPayload {
    av2_mvp_444_closed_loop_key_payload(
        geometry,
        SampleBitDepth::new(8).expect("8-bit depth is supported"),
        &Av2Mvp444FrameMode::Black,
    )
}

#[cfg(test)]
fn av2_black_closed_loop_key_payload(
    geometry: Av2VideoGeometry,
    chroma_format: Av2ChromaFormat,
) -> Av2SyntaxPayload {
    let tile_layout = Av2TileLayout::for_geometry(geometry);
    let allow_screen_content_tools = chroma_format == Av2ChromaFormat::Yuv444;
    let allow_intrabc = false;
    let profile = Av2Black444MvpProfile::current();
    let mut payload = av2_mvp_444_closed_loop_key_header_payload(
        allow_screen_content_tools,
        allow_intrabc,
        &tile_layout,
        Av2StreamFormat {
            chroma_format,
            bit_depth: SampleBitDepth::new(8).expect("8-bit depth is supported"),
        },
        Av2QuantizationParams::lossless(),
    );
    let tile_payloads: Vec<_> = tile_layout
        .regions
        .iter()
        .map(|&region| {
            if allow_intrabc {
                av2_black_444_tile_entropy_payload_for_region_with_intrabc_and_fields(
                    region, profile, true, false,
                )
            } else {
                tile::av2_black_tile_entropy_payload_for_region_with_fields(
                    region,
                    profile,
                    chroma_format,
                    false,
                )
            }
        })
        .collect();
    let tile_payload = tile_group_payload_from_entropy(&tile_payloads);
    let bit_offset = payload.bytes.len() * 8;
    payload.fields.push(syntax::Av2SyntaxField {
        name: "tile_group.tile_entropy_payload",
        code: syntax::Av2SyntaxCode::TileEntropyPayload,
        bit_offset,
        bit_count: tile_payload.len() * 8,
    });
    payload.bytes.extend_from_slice(&tile_payload);
    payload
}

fn av2_lossy_subsampled_closed_loop_key_payload(
    geometry: Av2VideoGeometry,
    stream_format: Av2StreamFormat,
    frame: &[u8],
    reconstruction: &mut [u8],
    qp: u8,
) -> Av2SyntaxPayload {
    av2_lossy_subsampled_closed_loop_key_payload_with_mode(
        geometry,
        stream_format,
        frame,
        reconstruction,
        qp,
        true,
        0,
    )
}

fn av2_lossy_subsampled_predictive_closed_loop_key_payload(
    geometry: Av2VideoGeometry,
    stream_format: Av2StreamFormat,
    frame: &[u8],
    reconstruction: &mut [u8],
    qp: u8,
    order_hint: u16,
) -> Av2SyntaxPayload {
    av2_lossy_subsampled_closed_loop_key_payload_with_mode(
        geometry,
        stream_format,
        frame,
        reconstruction,
        qp,
        false,
        order_hint,
    )
}

fn av2_lossy_subsampled_closed_loop_key_payload_with_mode(
    geometry: Av2VideoGeometry,
    stream_format: Av2StreamFormat,
    frame: &[u8],
    reconstruction: &mut [u8],
    qp: u8,
    single_picture_header: bool,
    order_hint: u16,
) -> Av2SyntaxPayload {
    let tile_layout = Av2TileLayout::lossy_subsampled_for_geometry(geometry);
    let profile = Av2Black444MvpProfile::current();
    let quantization = Av2QuantizationParams::regular_qp(qp, stream_format.bit_depth);
    let mut payload = if single_picture_header {
        av2_mvp_444_closed_loop_key_header_payload(
            false,
            false,
            &tile_layout,
            stream_format,
            quantization,
        )
    } else {
        av2_mvp_444_predictive_closed_loop_key_header_payload(
            false,
            false,
            &tile_layout,
            stream_format,
            quantization,
            order_hint,
        )
    };
    let tile_payloads: Vec<_> = tile_layout
        .regions
        .iter()
        .map(|&region| {
            av2_lossy_subsampled_tile_entropy_payload_for_region_with_fields(
                region,
                profile,
                geometry,
                stream_format.chroma_format,
                stream_format.bit_depth,
                frame,
                reconstruction,
                qp,
                quantization.base_qindex,
                false,
            )
        })
        .collect();
    let tile_payload = tile_group_payload_from_entropy(&tile_payloads);
    let bit_offset = payload.bytes.len() * 8;
    payload.fields.push(syntax::Av2SyntaxField {
        name: "tile_group.tile_entropy_payload",
        code: syntax::Av2SyntaxCode::TileEntropyPayload,
        bit_offset,
        bit_count: tile_payload.len() * 8,
    });
    payload.bytes.extend_from_slice(&tile_payload);
    payload
}

fn av2_lossless_subsampled_closed_loop_key_payload(
    geometry: Av2VideoGeometry,
    stream_format: Av2StreamFormat,
    frame: &[u8],
    reconstruction: &mut [u8],
    profile: Av2Black444MvpProfile,
    palette: Option<&Av2LumaPalette444>,
    ibc: Option<&Av2LocalIbc444>,
) -> Av2SyntaxPayload {
    let tile_layout = if ibc.is_none() {
        Av2TileLayout::lossless_subsampled_fast_for_geometry(geometry)
    } else {
        Av2TileLayout::lossless_subsampled_ibc_for_geometry(geometry)
    };
    let allow_intrabc = ibc.is_some();
    let allow_screen_content_tools = allow_intrabc || palette.is_some();
    let mut payload = av2_mvp_444_closed_loop_key_header_payload(
        allow_screen_content_tools,
        allow_intrabc,
        &tile_layout,
        stream_format,
        Av2QuantizationParams::lossless(),
    );
    let tile_payloads: Vec<_> = if ibc.is_none() && tile_layout.tile_count() > 1 {
        std::thread::scope(|scope| {
            let handles: Vec<_> = tile_layout
                .regions
                .iter()
                .map(|&region| {
                    scope.spawn(move || {
                        av2_lossless_subsampled_fast_tile_entropy_payload_for_region_with_fields(
                            region,
                            profile,
                            geometry,
                            stream_format.chroma_format,
                            stream_format.bit_depth,
                            frame,
                            palette,
                            false,
                        )
                    })
                })
                .collect();
            handles
                .into_iter()
                .map(|handle| handle.join().expect("AV2 tile entropy worker panicked"))
                .collect()
        })
    } else {
        tile_layout
            .regions
            .iter()
            .map(|&region| {
                av2_lossless_subsampled_tile_entropy_payload_for_region_with_fields(
                    region,
                    profile,
                    geometry,
                    stream_format.chroma_format,
                    stream_format.bit_depth,
                    frame,
                    reconstruction,
                    palette,
                    ibc,
                    false,
                )
            })
            .collect()
    };
    if ibc.is_none() && tile_layout.tile_count() > 1 {
        reconstruction.copy_from_slice(frame);
    }
    let tile_payload = tile_group_payload_from_entropy(&tile_payloads);
    let bit_offset = payload.bytes.len() * 8;
    payload.fields.push(syntax::Av2SyntaxField {
        name: "tile_group.tile_entropy_payload",
        code: syntax::Av2SyntaxCode::TileEntropyPayload,
        bit_offset,
        bit_count: tile_payload.len() * 8,
    });
    payload.bytes.extend_from_slice(&tile_payload);
    payload
}

fn av2_lossless_subsampled_predictive_closed_loop_key_payload(
    geometry: Av2VideoGeometry,
    stream_format: Av2StreamFormat,
    frame: &[u8],
    reconstruction: &mut [u8],
    profile: Av2Black444MvpProfile,
    palette: Option<&Av2LumaPalette444>,
    ibc: Option<&Av2LocalIbc444>,
    order_hint: u16,
) -> Av2SyntaxPayload {
    let tile_layout = if ibc.is_none() {
        Av2TileLayout::lossless_subsampled_fast_for_geometry(geometry)
    } else {
        Av2TileLayout::lossless_subsampled_ibc_for_geometry(geometry)
    };
    let allow_intrabc = ibc.is_some();
    let allow_screen_content_tools = allow_intrabc || palette.is_some();
    let mut payload = av2_mvp_444_predictive_closed_loop_key_header_payload(
        allow_screen_content_tools,
        allow_intrabc,
        &tile_layout,
        stream_format,
        Av2QuantizationParams::lossless(),
        order_hint,
    );
    let tile_payloads: Vec<_> = if ibc.is_none() && tile_layout.tile_count() > 1 {
        std::thread::scope(|scope| {
            let handles: Vec<_> = tile_layout
                .regions
                .iter()
                .map(|&region| {
                    scope.spawn(move || {
                        av2_lossless_subsampled_fast_tile_entropy_payload_for_region_with_fields(
                            region,
                            profile,
                            geometry,
                            stream_format.chroma_format,
                            stream_format.bit_depth,
                            frame,
                            palette,
                            false,
                        )
                    })
                })
                .collect();
            handles
                .into_iter()
                .map(|handle| handle.join().expect("AV2 tile entropy worker panicked"))
                .collect()
        })
    } else {
        tile_layout
            .regions
            .iter()
            .map(|&region| {
                av2_lossless_subsampled_tile_entropy_payload_for_region_with_fields(
                    region,
                    profile,
                    geometry,
                    stream_format.chroma_format,
                    stream_format.bit_depth,
                    frame,
                    reconstruction,
                    palette,
                    ibc,
                    false,
                )
            })
            .collect()
    };
    if ibc.is_none() && tile_layout.tile_count() > 1 {
        reconstruction.copy_from_slice(frame);
    }
    let tile_payload = tile_group_payload_from_entropy(&tile_payloads);
    let bit_offset = payload.bytes.len() * 8;
    payload.fields.push(syntax::Av2SyntaxField {
        name: "tile_group.tile_entropy_payload",
        code: syntax::Av2SyntaxCode::TileEntropyPayload,
        bit_offset,
        bit_count: tile_payload.len() * 8,
    });
    payload.bytes.extend_from_slice(&tile_payload);
    payload
}

fn av2_regular_sef_payload(order_hint: u16) -> Av2SyntaxPayload {
    let mut writer = Av2SyntaxWriter::new();
    writer.write_uvlc("uncompressed_header.cur_mfh_id", 0);
    writer.write_uvlc("uncompressed_header.seq_header_id", 0);
    writer.write_literal("show_existing_frame.existing_frame_idx", 0, 1);
    writer.write_flag("show_existing_frame.derive_sef_order_hint", false);
    writer.write_literal(
        "show_existing_frame.order_hint",
        u64::from(order_hint),
        AV2_PREDICTIVE_ORDER_HINT_BITS,
    );
    writer.trailing_bits();
    writer.finish()
}

#[cfg(test)]
fn av2_lossless_zero_mv_regular_inter_payload(
    geometry: Av2VideoGeometry,
    stream_format: Av2StreamFormat,
    order_hint: u16,
) -> Av2SyntaxPayload {
    let tile_layout = Av2TileLayout::lossless_subsampled_fast_for_geometry(geometry);
    let profile = Av2Black444MvpProfile::current();
    let mut payload = av2_mvp_regular_inter_header_payload(
        &tile_layout,
        stream_format,
        Av2QuantizationParams::lossless(),
        order_hint,
    );
    let tile_payloads: Vec<_> = tile_layout
        .regions
        .iter()
        .map(|&region| {
            av2_lossless_zero_mv_inter_tile_entropy_payload_for_region_with_fields(
                region,
                profile,
                stream_format.chroma_format,
                false,
            )
        })
        .collect();
    let tile_payload = tile_group_payload_from_entropy(&tile_payloads);
    let bit_offset = payload.bytes.len() * 8;
    payload.fields.push(syntax::Av2SyntaxField {
        name: "tile_group.tile_entropy_payload",
        code: syntax::Av2SyntaxCode::TileEntropyPayload,
        bit_offset,
        bit_count: tile_payload.len() * 8,
    });
    payload.bytes.extend_from_slice(&tile_payload);
    payload
}

fn av2_mvp_regular_inter_header_payload(
    tile_layout: &Av2TileLayout,
    stream_format: Av2StreamFormat,
    quantization: Av2QuantizationParams,
    order_hint: u16,
) -> Av2SyntaxPayload {
    let profile = Av2Black444MvpProfile::current();
    let mut writer = Av2SyntaxWriter::new();

    writer.write_flag("tile_group.first_tile_group_in_frame", true);
    writer.write_uvlc("uncompressed_header.cur_mfh_id", 0);
    writer.write_uvlc("uncompressed_header.seq_header_id", 0);
    writer.write_flag("uncompressed_header.is_inter_frame", true);
    writer.write_flag("uncompressed_header.immediate_output_picture", true);
    writer.write_flag("uncompressed_header.frame_size_override_flag", false);
    writer.write_literal(
        "uncompressed_header.order_hint",
        u64::from(order_hint),
        AV2_PREDICTIVE_ORDER_HINT_BITS,
    );
    writer.write_flag("uncompressed_header.signal_primary_ref_frame", false);
    writer.write_flag("uncompressed_header.cross_frame_context_disabled", true);
    writer.write_literal("uncompressed_header.refresh_frame_flags", 1, 2);
    writer.write_flag("uncompressed_header.allow_screen_content_tools", true);
    writer.write_flag("uncompressed_header.cur_frame_force_integer_mv", true);
    writer.write_flag("uncompressed_header.allow_intrabc", false);
    writer.write_flag("uncompressed_header.frame_interp_filter_switchable", false);
    writer.write_literal("uncompressed_header.frame_interp_filter", 0, 2);
    writer.write_flag(
        "uncompressed_header.disable_cdf_update",
        profile.disable_cdf_update,
    );
    write_mvp_tile_info(&mut writer, tile_layout);
    write_av2_quantization_params(&mut writer, stream_format, quantization);
    writer.write_flag("segmentation.enabled", false);
    write_av2_quantization_matrix_params(&mut writer, quantization);
    write_av2_delta_q_params(&mut writer, quantization);
    if !quantization.is_coded_lossless() {
        writer.write_flag("loop_filter.apply_deblocking_filter_y_vertical", false);
        writer.write_flag("loop_filter.apply_deblocking_filter_y_horizontal", false);
        writer.write_flag("uncompressed_header.tx_mode_select", true);
    }
    writer.write_flag("uncompressed_header.reference_mode_select", false);
    writer.write_flag("uncompressed_header.skip_mode_flag", false);
    writer.write_literal(
        "uncompressed_header.reduced_tx_set_used",
        if quantization.is_coded_lossless() {
            0
        } else {
            2
        },
        2,
    );
    if !tile_layout.is_single_tile() {
        writer.write_flag("tile_group.tile_start_and_end_present_flag", false);
    }
    writer.byte_align_zero("tile_group.header_byte_alignment");

    writer.finish()
}

fn av2_mvp_444_closed_loop_key_payload(
    geometry: Av2VideoGeometry,
    bit_depth: SampleBitDepth,
    frame_mode: &Av2Mvp444FrameMode,
) -> Av2SyntaxPayload {
    let tile_layout = av2_tile_layout_for_frame_mode(geometry, frame_mode);
    let mut payload = av2_mvp_444_closed_loop_key_header_payload(
        frame_mode.allow_screen_content_tools(),
        frame_mode.allow_intrabc(),
        &tile_layout,
        Av2StreamFormat {
            chroma_format: Av2ChromaFormat::Yuv444,
            bit_depth,
        },
        Av2QuantizationParams::lossless(),
    );
    let tile_payload = tile_group_payload_from_entropy(&av2_tile_entropy_payloads_for_mode(
        &tile_layout,
        frame_mode,
        false,
    ));
    let bit_offset = payload.bytes.len() * 8;
    payload.fields.push(syntax::Av2SyntaxField {
        name: "tile_group.tile_entropy_payload",
        code: syntax::Av2SyntaxCode::TileEntropyPayload,
        bit_offset,
        bit_count: tile_payload.len() * 8,
    });
    payload.bytes.extend_from_slice(&tile_payload);
    payload
}

fn tile_group_payload_from_entropy(tile_payloads: &[entropy::Av2EntropyPayload]) -> Vec<u8> {
    if tile_payloads.len() == 1 {
        return tile_payloads[0].bytes.clone();
    }

    let mut out = Vec::new();
    for (tile_index, payload) in tile_payloads.iter().enumerate() {
        if tile_index + 1 != tile_payloads.len() {
            write_tile_size_prefix(payload.bytes.len(), &mut out);
        }
        out.extend_from_slice(&payload.bytes);
    }
    out
}

fn av2_tile_entropy_payloads_for_mode(
    tile_layout: &Av2TileLayout,
    frame_mode: &Av2Mvp444FrameMode,
    record_fields: bool,
) -> Vec<entropy::Av2EntropyPayload> {
    tile_layout
        .regions
        .iter()
        .map(|&region| av2_tile_entropy_payload_for_region(region, frame_mode, record_fields))
        .collect()
}

fn av2_tile_entropy_payload_for_region(
    region: Av2TileRegion,
    frame_mode: &Av2Mvp444FrameMode,
    record_fields: bool,
) -> entropy::Av2EntropyPayload {
    match frame_mode {
        Av2Mvp444FrameMode::Black => {
            av2_black_444_tile_entropy_payload_for_region_with_intrabc_and_fields(
                region,
                frame_mode.profile(),
                frame_mode.allow_intrabc(),
                record_fields,
            )
        }
        Av2Mvp444FrameMode::LumaPalette { palette, ibc } => {
            if !frame_mode.allow_intrabc() && av2_luma_palette_region_is_black(palette, region) {
                av2_black_444_tile_entropy_payload_for_region_with_fields(
                    region,
                    frame_mode.profile(),
                    record_fields,
                )
            } else {
                av2_luma_palette_444_tile_entropy_payload_for_region_with_fields(
                    region,
                    frame_mode.profile(),
                    frame_mode.allow_intrabc(),
                    palette,
                    ibc.as_ref(),
                    record_fields,
                )
            }
        }
    }
}

fn av2_luma_palette_region_is_black(palette: &Av2LumaPalette444, region: Av2TileRegion) -> bool {
    for y in region.origin_y..(region.origin_y + region.height) {
        for x in region.origin_x..(region.origin_x + region.width) {
            if palette.y_sample(x, y) != 0
                || palette.u_sample(x, y) != 0
                || palette.v_sample(x, y) != 0
            {
                return false;
            }
        }
    }
    true
}

fn write_tile_size_prefix(tile_size: usize, out: &mut Vec<u8>) {
    let stored = tile_size
        .checked_sub(AV2_MIN_TILE_SIZE_BYTES)
        .expect("AV2 tile payload must not be empty");
    assert!(
        stored <= u32::MAX as usize,
        "AV2 MVP tile payload size prefix is limited to 32 bits"
    );
    out.extend_from_slice(&(stored as u32).to_le_bytes());
}

fn append_obu(out: &mut Vec<u8>, obu_type: Av2ObuType, payload: &Av2SyntaxPayload) {
    let header = av2_obu_header(obu_type);
    let obu_payload_len = (header.len() + payload.bytes.len()) as u32;
    if obu_type == Av2ObuType::ClosedLoopKey {
        // AV2 v1.0.0 Section 5.3 defines OBU lengths as unsigned LEB128.
        // The RTL reserves three bytes for closed-loop frame OBUs so it can
        // stream tile payloads once and patch the final length afterward. Very
        // large software-only high-depth frames can exceed that envelope, so
        // fall back to the normal variable-width LEB128 form when needed.
        if leb128_len(obu_payload_len) <= 3 {
            write_leb128_fixed_width(obu_payload_len, 3, out);
        } else {
            write_leb128(obu_payload_len, out);
        }
    } else {
        write_leb128(obu_payload_len, out);
    }
    out.extend_from_slice(&header);
    out.extend_from_slice(&payload.bytes);
}

fn av2_obu_header(obu_type: Av2ObuType) -> Vec<u8> {
    let mut writer = Av2SyntaxWriter::new();
    writer.write_flag("obu_header.obu_header_extension_flag", false);
    writer.write_literal("obu_header.obu_type", obu_type as u64, 5);
    writer.write_literal("obu_header.obu_tlayer_id", 0, 2);
    writer.finish().bytes
}

fn write_leb128(mut value: u32, out: &mut Vec<u8>) {
    loop {
        let mut byte = (value & 0x7f) as u8;
        value >>= 7;
        if value != 0 {
            byte |= 0x80;
        }
        out.push(byte);
        if value == 0 {
            break;
        }
    }
}

fn leb128_len(mut value: u32) -> usize {
    let mut len = 1;
    while value >= 0x80 {
        value >>= 7;
        len += 1;
    }
    len
}

fn write_leb128_fixed_width(mut value: u32, width: usize, out: &mut Vec<u8>) {
    assert!(
        (1..=5).contains(&width),
        "AV2 fixed LEB width must be 1..=5"
    );
    for index in 0..width {
        let mut byte = (value & 0x7f) as u8;
        value >>= 7;
        if index + 1 != width {
            byte |= 0x80;
        }
        out.push(byte);
    }
    assert_eq!(value, 0, "AV2 fixed-width LEB is too narrow");
}

fn push_av2_trace_line(
    out: &mut String,
    phase: &str,
    name: &str,
    spec: &str,
    code: &str,
    bit_offset: usize,
    bit_count: usize,
) {
    out.push_str(&format!(
        "{{\"codec\":\"av2\",\"source\":\"software\",\"phase\":\"{}\",\"name\":\"{}\",\"spec\":\"{}\",\"code\":\"{}\",\"bit_offset\":{},\"bit_count\":{}}}\n",
        escape_json(phase),
        escape_json(name),
        escape_json(spec),
        escape_json(code),
        bit_offset,
        bit_count
    ));
}

fn push_av2_entropy_trace_line(
    out: &mut String,
    tile_index: usize,
    field: &entropy::Av2EntropyField,
) {
    let mut line = format!(
        "{{\"codec\":\"av2\",\"source\":\"software\",\"phase\":\"tile_entropy\",\"tile_index\":{},\"name\":\"{}\",\"spec\":\"{}\",\"code\":\"{}\",\"bit_offset\":{},\"bit_count\":{}",
        tile_index,
        escape_json(field.name),
        escape_json(av2_spec_section_for_entropy_field(field.name)),
        escape_json(&format!("{:?}", field.code)),
        field.symbol_offset,
        field.bit_count
    );
    if let Some(symbol) = field.symbol {
        line.push_str(&format!(",\"symbol\":{symbol}"));
    }
    if let Some(value) = field.literal_value {
        line.push_str(&format!(",\"literal_value\":{value}"));
    }
    if let Some(fl) = field.fl {
        line.push_str(&format!(",\"fl\":{fl}"));
    }
    if let Some(fh) = field.fh {
        line.push_str(&format!(",\"fh\":{fh}"));
    }
    if let Some(fl_inc) = field.fl_inc {
        line.push_str(&format!(",\"fl_inc\":{fl_inc}"));
    }
    if let Some(fh_inc) = field.fh_inc {
        line.push_str(&format!(",\"fh_inc\":{fh_inc}"));
    }
    line.push_str("}\n");
    out.push_str(&line);
}

fn av2_spec_section_for_syntax_field(name: &str) -> &'static str {
    if name.starts_with("sequence_header.") || name.starts_with("sequence_") {
        "AV2 v1.0.0 Section 5.4.1 sequence_header_obu()"
    } else if name.starts_with("tile_group.") || name.starts_with("uncompressed_header.") {
        "AV2 v1.0.0 Sections 5.19 and 5.20.1 tile_group_obu()"
    } else if name.starts_with("tile_info.")
        || name.starts_with("quantization.")
        || name.starts_with("segmentation.")
        || name.starts_with("quantization_matrix.")
    {
        "AV2 v1.0.0 Section 5.20.1 uncompressed header syntax"
    } else if name == "trailing_bits" {
        "AV2 v1.0.0 Section 5.4.1 trailing bits"
    } else {
        "AV2 v1.0.0 syntax"
    }
}

fn av2_spec_section_for_entropy_field(name: &str) -> &'static str {
    if name.starts_with("tile.partition.") {
        "AV2 v1.0.0 Section 5.20.3.2 partition()"
    } else if name.starts_with("tile.intrabc.") {
        "AV2 v1.0.0 Sections 5.20.5.1 and 5.20.5.3 intra block copy syntax"
    } else if name.starts_with("tile.intra.") {
        "AV2 v1.0.0 Sections 5.20.5.5 and 5.20.5.6 intra mode syntax"
    } else if name.starts_with("tile.palette.") {
        "AV2 v1.0.0 Sections 5.20.8.1 and 5.20.8.4 palette syntax"
    } else if name.starts_with("tile.coeff.") {
        "AV2 v1.0.0 Sections 5.20.7.23, 5.20.7.24, and 5.20.7.27 residual coefficient syntax"
    } else {
        "AV2 v1.0.0 tile entropy syntax"
    }
}

fn escape_json(value: &str) -> String {
    let mut out = String::new();
    for ch in value.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c.is_control() => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::syntax::Av2SyntaxCode;
    use super::*;

    #[test]
    fn av2_accepts_basic_yuv_request_shape() {
        let request = Av2EncodeRequest {
            params: Av2EncodeParams { frames: 1 },
            geometry: Av2VideoGeometry {
                width: 64,
                height: 64,
            },
            format: PixelFormat::Yuv420p8,
        };

        assert!(request.validate().is_ok());
    }

    #[test]
    fn av2_rgb24_repack_uses_planar_gbr_identity_order() {
        let geometry = Av2VideoGeometry {
            width: 2,
            height: 1,
        };
        let rgb = vec![1, 2, 3, 4, 5, 6];

        let planar = rgb24_to_planar_gbr(&rgb, geometry);

        assert_eq!(planar, vec![2, 5, 3, 6, 1, 4]);
        assert_eq!(planar_gbr_to_rgb24(&planar, geometry), rgb);
    }

    #[test]
    fn av2_rgb24_lossless_emits_identity_metadata_and_packed_recon() {
        let geometry = Av2VideoGeometry {
            width: 8,
            height: 8,
        };
        let request = Av2EncodeRequest {
            params: Av2EncodeParams { frames: 1 },
            geometry,
            format: PixelFormat::Rgb24,
        };
        let frame_len = Picture::expected_len(geometry.width, geometry.height, request.format);
        let input: Vec<u8> = (0..frame_len)
            .map(|index| ((index * 17 + 3) & 0xff) as u8)
            .collect();
        let mut source = input.as_slice();
        let mut output = Vec::new();
        let mut recon = Vec::new();

        av2_encode_fixed_black_444_with_options_and_frame_metrics(
            &mut source,
            &mut output,
            Some(&mut recon),
            request,
            Av2EncodeOptions {
                lossless: true,
                ..Default::default()
            },
            None,
        )
        .expect("AV2 rgb24 lossless encode should preserve packed RGB bytes");

        assert_eq!(recon, input);
        let ci_header = av2_obu_header(Av2ObuType::ContentInterpretation);
        assert!(
            output
                .windows(ci_header.len())
                .any(|window| window == ci_header.as_slice()),
            "RGB identity stream should carry a content-interpretation OBU"
        );
    }

    #[test]
    fn av2_rgb24_non_lossless_emits_identity_metadata_and_packed_recon() {
        let geometry = Av2VideoGeometry {
            width: 8,
            height: 8,
        };
        let request = Av2EncodeRequest {
            params: Av2EncodeParams { frames: 1 },
            geometry,
            format: PixelFormat::Rgb24,
        };
        let frame_len = Picture::expected_len(geometry.width, geometry.height, request.format);
        let input: Vec<u8> = (0..frame_len)
            .map(|index| ((index * 29 + 7) & 0xff) as u8)
            .collect();
        let mut source = input.as_slice();
        let mut output = Vec::new();
        let mut recon = Vec::new();

        av2_encode_fixed_black_444_with_options_and_frame_metrics(
            &mut source,
            &mut output,
            Some(&mut recon),
            request,
            Av2EncodeOptions::default(),
            None,
        )
        .expect("AV2 rgb24 non-lossless encode should keep public RGB byte layout");

        assert_eq!(recon, input);
        let ci_header = av2_obu_header(Av2ObuType::ContentInterpretation);
        assert!(
            output
                .windows(ci_header.len())
                .any(|window| window == ci_header.as_slice()),
            "RGB identity stream should carry a content-interpretation OBU"
        );
    }

    #[test]
    fn av2_rgb_identity_content_interpretation_uses_srgb_idc() {
        let payload = av2_rgb_identity_content_interpretation_payload();

        assert!(
            payload.fields.iter().any(|field| {
                field.name == "content_interpretation.color_description_idc"
                    && field.code == Av2SyntaxCode::RiceGolomb
                    && field.bit_count == 4
            }),
            "sRGB color_description_idc=4 should be Rice-Golomb coded with k=2"
        );
        assert!(payload.fields.iter().any(|field| {
            field.name == "content_interpretation.full_range_flag"
                && field.code == Av2SyntaxCode::Flag
        }));
    }

    #[test]
    fn av2_fixed_black_444_emits_generated_obu_stream_and_reconstruction() {
        for geometry in supported_black_444_geometries() {
            let request = Av2EncodeRequest {
                params: Av2EncodeParams { frames: 1 },
                geometry,
                format: PixelFormat::Yuv444p8,
            };
            let input =
                av2_black_444_reconstruction(geometry).expect("supported AV2 fixed black geometry");
            let mut source = input.as_slice();
            let mut output = Vec::new();
            let mut recon = Vec::new();

            let result =
                av2_encode_fixed_black_444(&mut source, &mut output, Some(&mut recon), request);

            result.expect("AV2 OBU encode should succeed");
            assert_eq!(output, av2_black_444_bitstream_for_geometry(geometry));
            assert_eq!(&output[..2], &[0x01, 0x08]);
            assert_ne!(output, input);
            assert_eq!(recon, input);
        }
    }

    #[test]
    fn av2_mvp_444_encodes_all_requested_frames() {
        let geometry = Av2VideoGeometry {
            width: 8,
            height: 8,
        };
        let request = Av2EncodeRequest {
            params: Av2EncodeParams { frames: 2 },
            geometry,
            format: PixelFormat::Yuv444p8,
        };
        let frame_len = Picture::expected_len(geometry.width, geometry.height, request.format);
        let first = vec![0; frame_len];
        let mut second = vec![0; frame_len];
        for sample in second.iter_mut().take(geometry.width * geometry.height) {
            *sample = 73;
        }
        let mut input = first.clone();
        input.extend_from_slice(&second);
        let mut source = input.as_slice();
        let mut output = Vec::new();
        let mut recon = Vec::new();

        av2_encode_fixed_black_444(&mut source, &mut output, Some(&mut recon), request)
            .expect("AV2 MVP stream encode should process every requested frame");

        let mut expected_output = av2_mvp_444_bitstream_for_mode(
            geometry,
            request.format.bit_depth(),
            &Av2Mvp444FrameMode::from_frame(&first, geometry, request.format.bit_depth())
                .expect("first frame mode"),
            false,
        );
        expected_output.extend_from_slice(&av2_mvp_444_bitstream_for_mode(
            geometry,
            request.format.bit_depth(),
            &Av2Mvp444FrameMode::from_frame(&second, geometry, request.format.bit_depth())
                .expect("second frame mode"),
            false,
        ));
        assert_eq!(output, expected_output);
        assert_eq!(recon, input);
    }

    #[test]
    fn av2_lossless_predictive_reuses_repeated_frames_as_sef() {
        let geometry = Av2VideoGeometry {
            width: 8,
            height: 8,
        };
        let request = Av2EncodeRequest {
            params: Av2EncodeParams { frames: 3 },
            geometry,
            format: PixelFormat::Yuv420p8,
        };
        let frame_len = Picture::expected_len(geometry.width, geometry.height, request.format);
        let frame: Vec<u8> = (0..frame_len)
            .map(|index| ((index * 17 + 23) & 0xff) as u8)
            .collect();
        let mut input = Vec::with_capacity(frame_len * request.params.frames);
        for _ in 0..request.params.frames {
            input.extend_from_slice(&frame);
        }
        let mut source = input.as_slice();
        let mut output = Vec::new();
        let mut recon = Vec::new();
        let mut frame_sizes = Vec::new();
        let mut metrics = |metrics: Av2EncodeFrameMetrics<'_>| {
            assert_eq!(metrics.source, metrics.reconstruction);
            frame_sizes.push(metrics.bitstream_bytes);
        };

        av2_encode_fixed_black_444_with_options_and_frame_metrics(
            &mut source,
            &mut output,
            Some(&mut recon),
            request,
            Av2EncodeOptions {
                lossless: true,
                qp: None,
                predictive: true,
            },
            Some(&mut metrics),
        )
        .expect("AV2 lossless predictive repeated-frame encode should succeed");

        assert_eq!(recon, input);
        assert_eq!(frame_sizes.len(), 3);
        assert!(frame_sizes[0] > frame_sizes[1]);
        assert_eq!(frame_sizes[1], 6);
        assert_eq!(frame_sizes[2], 6);
        assert_eq!(output.len(), frame_sizes.iter().sum());
    }

    #[test]
    fn av2_lossy_predictive_reuses_repeated_frames_as_sef() {
        let geometry = Av2VideoGeometry {
            width: 8,
            height: 8,
        };
        let request = Av2EncodeRequest {
            params: Av2EncodeParams { frames: 3 },
            geometry,
            format: PixelFormat::Yuv420p8,
        };
        let frame_len = Picture::expected_len(geometry.width, geometry.height, request.format);
        let frame: Vec<u8> = (0..frame_len)
            .map(|index| ((index * 17 + 23) & 0xff) as u8)
            .collect();
        let mut input = Vec::with_capacity(frame_len * request.params.frames);
        for _ in 0..request.params.frames {
            input.extend_from_slice(&frame);
        }
        let mut source = input.as_slice();
        let mut output = Vec::new();
        let mut recon = Vec::new();
        let mut frame_sizes = Vec::new();
        let mut metrics = |metrics: Av2EncodeFrameMetrics<'_>| {
            frame_sizes.push(metrics.bitstream_bytes);
        };

        av2_encode_fixed_black_444_with_options_and_frame_metrics(
            &mut source,
            &mut output,
            Some(&mut recon),
            request,
            Av2EncodeOptions {
                lossless: false,
                qp: Some(24),
                predictive: true,
            },
            Some(&mut metrics),
        )
        .expect("AV2 lossy predictive repeated-frame encode should succeed");

        assert_eq!(recon.len(), input.len());
        assert_eq!(frame_sizes.len(), 3);
        assert!(frame_sizes[0] > frame_sizes[1]);
        assert_eq!(frame_sizes[1], 6);
        assert_eq!(frame_sizes[2], 6);
        assert_eq!(&recon[..frame_len], &recon[frame_len..frame_len * 2]);
        assert_eq!(
            &recon[frame_len..frame_len * 2],
            &recon[frame_len * 2..frame_len * 3]
        );
    }

    #[test]
    fn av2_lossy_predictive_zero_mv_tiles_reuse_previous_reconstruction() {
        let geometry = Av2VideoGeometry {
            width: 1024,
            height: 64,
        };
        let format = PixelFormat::Yuv420p8;
        let stream_format =
            Av2StreamFormat::from_pixel_format(format).expect("yuv420p8 is an AV2 stream format");
        let frame_len = Picture::expected_len(geometry.width, geometry.height, format);
        let first: Vec<u8> = (0..frame_len)
            .map(|index| ((index * 13 + 19) & 0xff) as u8)
            .collect();
        let mut second = first.clone();
        for y in 0..geometry.height {
            let row = y * geometry.width;
            for x in 512..geometry.width {
                second[row + x] = second[row + x].wrapping_add(17);
            }
        }

        let (_, first_recon) =
            av2_lossy_subsampled_predictive_key_bitstream_and_reconstruction_for_frame(
                geometry,
                stream_format,
                &first,
                24,
                true,
                0,
                false,
            );
        let (_, inter_recon) =
            av2_lossy_subsampled_zero_mv_inter_tiles_bitstream_and_reconstruction_for_frame(
                geometry,
                stream_format,
                &second,
                &first,
                &first_recon,
                24,
                1,
            )
            .expect("unchanged left tile should use a zero-MV inter frame");
        let layout = planar::Av2PlanarYuvLayout::new(
            geometry,
            stream_format.chroma_format,
            stream_format.bit_depth,
        )
        .expect("valid planar layout");

        assert!(layout.regions_equal_between(
            &inter_recon,
            0,
            0,
            &first_recon,
            0,
            0,
            512,
            geometry.height
        ));
        assert!(!layout.regions_equal_between(
            &inter_recon,
            512,
            0,
            &first_recon,
            512,
            0,
            512,
            geometry.height
        ));
    }

    #[test]
    fn av2_lossy_exact_motion_residual_map_uses_newmv_for_shifted_8bit_blocks() {
        let geometry = Av2VideoGeometry {
            width: 1024,
            height: 64,
        };
        let format = PixelFormat::Yuv420p8;
        let first = shifted_tile_reference_frame(geometry);
        let second = shifted_tile_current_frame(&first, geometry);
        let stream_format =
            Av2StreamFormat::from_pixel_format(format).expect("yuv420p8 is an AV2 stream format");
        let motion_map = motion::build_lossless_motion_map(
            &second,
            &first,
            geometry,
            stream_format.chroma_format,
            stream_format.bit_depth,
        )
        .expect("shifted tile motion map should build");
        let right_tile = Av2TileRegion {
            origin_x: 512,
            origin_y: 0,
            width: 512,
            height: 64,
        };
        let residual_blocks = lossy_tile_inter_residual_block_modes(&motion_map, right_tile)
            .expect("shifted tile should expose exact NEWMV residual blocks");
        assert_eq!(
            residual_blocks.block_mode_at(0, 0),
            Some(Av2LosslessInterBlockMode::NewMvResidual {
                row_px: 0,
                col_px: -8
            })
        );
    }

    #[test]
    fn av2_lossy_predictive_requires_qp_for_legacy_444_path() {
        let geometry = Av2VideoGeometry {
            width: 8,
            height: 8,
        };
        let format = PixelFormat::Yuv444p8;
        let input = vec![0; Picture::expected_len(geometry.width, geometry.height, format)];
        let mut source = input.as_slice();
        let mut output = Vec::new();

        let err = av2_encode_fixed_black_444_with_options_and_frame_metrics(
            &mut source,
            &mut output,
            None,
            Av2EncodeRequest {
                params: Av2EncodeParams { frames: 1 },
                geometry,
                format,
            },
            Av2EncodeOptions {
                lossless: false,
                qp: None,
                predictive: true,
            },
            None,
        )
        .expect_err("predictive non-lossless 4:4:4 should require the QP residual path");

        assert!(
            err.contains("requires --qp"),
            "unexpected predictive fallback error: {err}"
        );
    }

    #[test]
    fn av2_lossless_predictive_uses_zero_mv_inter_for_unchanged_tiles() {
        let geometry = Av2VideoGeometry {
            width: 1024,
            height: 64,
        };
        let format = PixelFormat::Yuv420p8;
        let frame_len = Picture::expected_len(geometry.width, geometry.height, format);
        let first = vec![0u8; frame_len];
        let mut second = first.clone();
        for y in 0..geometry.height {
            let row = y * geometry.width;
            for x in 512..geometry.width {
                second[row + x] = 90;
            }
        }
        let mut input = first.clone();
        input.extend_from_slice(&second);
        let mut source = input.as_slice();
        let mut output = Vec::new();
        let mut recon = Vec::new();
        let mut frame_sizes = Vec::new();
        let mut metrics = |metrics: Av2EncodeFrameMetrics<'_>| {
            assert_eq!(metrics.source, metrics.reconstruction);
            frame_sizes.push(metrics.bitstream_bytes);
        };
        av2_encode_fixed_black_444_with_options_and_frame_metrics(
            &mut source,
            &mut output,
            Some(&mut recon),
            Av2EncodeRequest {
                params: Av2EncodeParams { frames: 2 },
                geometry,
                format,
            },
            Av2EncodeOptions {
                lossless: true,
                qp: None,
                predictive: true,
            },
            Some(&mut metrics),
        )
        .expect("AV2 tile-level zero-MV predictive encode should succeed");

        assert_eq!(recon, input);
        assert_eq!(frame_sizes.len(), 2);
        assert!(frame_sizes[1] > 6);
        assert!(
            frame_sizes[1] < frame_sizes[0],
            "unchanged tile should make the regular inter frame smaller than the first key frame"
        );
    }

    #[test]
    fn av2_lossless_predictive_uses_newmv_inter_for_shifted_tile() {
        let geometry = Av2VideoGeometry {
            width: 1024,
            height: 64,
        };
        let format = PixelFormat::Yuv420p8;
        let frame_len = Picture::expected_len(geometry.width, geometry.height, format);
        let first = shifted_tile_reference_frame(geometry);
        let second = shifted_tile_current_frame(&first, geometry);
        let stream_format =
            Av2StreamFormat::from_pixel_format(format).expect("yuv420p8 is an AV2 stream format");
        let motion_map = motion::build_lossless_motion_map(
            &second,
            &first,
            geometry,
            stream_format.chroma_format,
            stream_format.bit_depth,
        )
        .expect("shifted tile motion map should build");
        let shifted_mv = uniform_lossless_tile_motion(
            &motion_map,
            Av2TileRegion {
                origin_x: 512,
                origin_y: 0,
                width: 512,
                height: 64,
            },
        )
        .expect("right tile should have one uniform exact motion vector");
        assert_eq!(
            shifted_mv,
            Av2MotionVector {
                row_px: 0,
                col_px: -8
            }
        );
        let mut input = Vec::with_capacity(frame_len * 2);
        input.extend_from_slice(&first);
        input.extend_from_slice(&second);
        let mut source = input.as_slice();
        let mut output = Vec::new();
        let mut recon = Vec::new();
        let mut frame_sizes = Vec::new();
        let mut metrics = |metrics: Av2EncodeFrameMetrics<'_>| {
            assert_eq!(metrics.source, metrics.reconstruction);
            frame_sizes.push(metrics.bitstream_bytes);
        };
        av2_encode_fixed_black_444_with_options_and_frame_metrics(
            &mut source,
            &mut output,
            Some(&mut recon),
            Av2EncodeRequest {
                params: Av2EncodeParams { frames: 2 },
                geometry,
                format,
            },
            Av2EncodeOptions {
                lossless: true,
                qp: None,
                predictive: true,
            },
            Some(&mut metrics),
        )
        .expect("AV2 tile-level NEWMV predictive encode should succeed");

        assert_eq!(recon, input);
        assert_eq!(frame_sizes.len(), 2);
        assert!(
            frame_sizes[1] < frame_sizes[0],
            "shifted tile should make the regular inter frame smaller than the first key frame"
        );
    }

    #[test]
    fn av2_lossless_predictive_uses_mixed_newmv_inter_for_nonuniform_shifted_tile() {
        let geometry = Av2VideoGeometry {
            width: 1024,
            height: 64,
        };
        let format = PixelFormat::Yuv420p8;
        let frame_len = Picture::expected_len(geometry.width, geometry.height, format);
        let first = shifted_tile_reference_frame(geometry);
        let second = mixed_shifted_tile_current_frame(&first, geometry);
        let stream_format =
            Av2StreamFormat::from_pixel_format(format).expect("yuv420p8 is an AV2 stream format");
        let motion_map = motion::build_lossless_motion_map(
            &second,
            &first,
            geometry,
            stream_format.chroma_format,
            stream_format.bit_depth,
        )
        .expect("mixed shifted tile motion map should build");
        let right_tile = Av2TileRegion {
            origin_x: 512,
            origin_y: 0,
            width: 512,
            height: 64,
        };
        assert_eq!(uniform_lossless_tile_motion(&motion_map, right_tile), None);
        assert!(
            lossless_tile_inter_block_modes(&motion_map, right_tile).is_some(),
            "right tile should have exact non-uniform inter block modes"
        );

        let mut input = Vec::with_capacity(frame_len * 2);
        input.extend_from_slice(&first);
        input.extend_from_slice(&second);
        let mut source = input.as_slice();
        let mut output = Vec::new();
        let mut recon = Vec::new();
        let mut frame_sizes = Vec::new();
        let mut metrics = |metrics: Av2EncodeFrameMetrics<'_>| {
            assert_eq!(metrics.source, metrics.reconstruction);
            frame_sizes.push(metrics.bitstream_bytes);
        };
        av2_encode_fixed_black_444_with_options_and_frame_metrics(
            &mut source,
            &mut output,
            Some(&mut recon),
            Av2EncodeRequest {
                params: Av2EncodeParams { frames: 2 },
                geometry,
                format,
            },
            Av2EncodeOptions {
                lossless: true,
                qp: None,
                predictive: true,
            },
            Some(&mut metrics),
        )
        .expect("AV2 mixed tile-level NEWMV predictive encode should succeed");

        assert_eq!(recon, input);
        assert_eq!(frame_sizes.len(), 2);
        assert!(
            frame_sizes[1] < frame_sizes[0],
            "mixed shifted tile should make the regular inter frame smaller than the first key frame"
        );
    }

    #[test]
    fn av2_lossless_zero_mv_regular_inter_payload_emits_inter_symbols() {
        let geometry = Av2VideoGeometry {
            width: 16,
            height: 16,
        };
        let stream_format = Av2StreamFormat::from_pixel_format(PixelFormat::Yuv420p8)
            .expect("yuv420p8 is an AV2 stream format");
        let profile = Av2Black444MvpProfile::current();
        let entropy = av2_lossless_zero_mv_inter_tile_entropy_payload_for_region_with_fields(
            Av2TileRegion::root(geometry),
            profile,
            stream_format.chroma_format,
            true,
        );
        let names: Vec<_> = entropy.fields.iter().map(|field| field.name).collect();

        assert!(names.contains(&"tile.inter.is_inter"));
        assert!(names.contains(&"tile.inter.skip_txfm"));
        assert!(names.contains(&"tile.inter.single_mode"));

        let payload = av2_lossless_zero_mv_regular_inter_payload(geometry, stream_format, 1);
        assert!(payload
            .fields
            .iter()
            .any(|field| field.name == "tile_group.tile_entropy_payload"));

        let mut obu = Vec::new();
        append_obu(&mut obu, Av2ObuType::RegularTileGroup, &payload);
        assert!(!obu.is_empty());
    }

    #[test]
    fn av2_lossless_newmv_regular_inter_payload_emits_mv_symbols() {
        let geometry = Av2VideoGeometry {
            width: 16,
            height: 16,
        };
        let stream_format = Av2StreamFormat::from_pixel_format(PixelFormat::Yuv420p8)
            .expect("yuv420p8 is an AV2 stream format");
        let profile = Av2Black444MvpProfile::current();
        let entropy = av2_lossless_new_mv_inter_tile_entropy_payload_for_region_with_fields(
            Av2TileRegion::root(geometry),
            profile,
            stream_format.chroma_format,
            -8,
            16,
            true,
        );
        let fields: Vec<_> = entropy
            .fields
            .iter()
            .map(|field| (field.name, field.symbol, field.literal_value))
            .collect();

        assert!(fields
            .iter()
            .any(|(name, symbol, _)| { *name == "tile.inter.single_mode" && *symbol == Some(2) }));
        assert!(fields
            .iter()
            .any(|(name, _, _)| *name == "tile.inter.mv.shell_set"));
        assert!(fields
            .iter()
            .any(|(name, _, literal)| { *name == "tile.inter.mv.sign" && *literal == Some(1) }));
    }

    fn shifted_tile_reference_frame(geometry: Av2VideoGeometry) -> Vec<u8> {
        assert_eq!(geometry.width, 1024);
        assert_eq!(geometry.height, 64);
        let mut frame =
            vec![0; Picture::expected_len(geometry.width, geometry.height, PixelFormat::Yuv420p8,)];
        let y_len = geometry.width * geometry.height;
        let chroma_width = geometry.width / 2;
        let chroma_height = geometry.height / 2;
        let chroma_len = chroma_width * chroma_height;
        let u_offset = y_len;
        let v_offset = y_len + chroma_len;

        for y in 0..geometry.height {
            for x in 0..geometry.width {
                frame[y * geometry.width + x] = ((x * 3 + y * 5 + 17) & 0xff) as u8;
            }
        }
        for y in 0..chroma_height {
            for x in 0..chroma_width {
                frame[u_offset + y * chroma_width + x] = ((x * 7 + y * 11 + 31) & 0xff) as u8;
                frame[v_offset + y * chroma_width + x] = ((x * 13 + y * 17 + 59) & 0xff) as u8;
            }
        }
        frame
    }

    fn shifted_tile_current_frame(reference: &[u8], geometry: Av2VideoGeometry) -> Vec<u8> {
        let mut frame = vec![0; reference.len()];
        let y_len = geometry.width * geometry.height;
        let chroma_width = geometry.width / 2;
        let chroma_height = geometry.height / 2;
        let chroma_len = chroma_width * chroma_height;
        let u_offset = y_len;
        let v_offset = y_len + chroma_len;

        for y in 0..geometry.height {
            let row = y * geometry.width;
            for x in 0..512 {
                frame[row + x] = 233;
            }
            for x in 512..geometry.width {
                frame[row + x] = reference[row + x - 8];
            }
        }

        for y in 0..chroma_height {
            let row = y * chroma_width;
            for x in 0..256 {
                frame[u_offset + row + x] = 129;
                frame[v_offset + row + x] = 55;
            }
            for x in 256..chroma_width {
                frame[u_offset + row + x] = reference[u_offset + row + x - 4];
                frame[v_offset + row + x] = reference[v_offset + row + x - 4];
            }
        }

        frame
    }

    fn mixed_shifted_tile_current_frame(reference: &[u8], geometry: Av2VideoGeometry) -> Vec<u8> {
        let mut frame = vec![0; reference.len()];
        let y_len = geometry.width * geometry.height;
        let chroma_width = geometry.width / 2;
        let chroma_height = geometry.height / 2;
        let chroma_len = chroma_width * chroma_height;
        let u_offset = y_len;
        let v_offset = y_len + chroma_len;

        for y in 0..geometry.height {
            let row = y * geometry.width;
            let shift = if y < geometry.height / 2 { 8 } else { 16 };
            for x in 0..512 {
                frame[row + x] = 233;
            }
            for x in 512..geometry.width {
                frame[row + x] = reference[row + x - shift];
            }
        }

        for y in 0..chroma_height {
            let row = y * chroma_width;
            let shift = if y < chroma_height / 2 { 4 } else { 8 };
            for x in 0..256 {
                frame[u_offset + row + x] = 129;
                frame[v_offset + row + x] = 55;
            }
            for x in 256..chroma_width {
                frame[u_offset + row + x] = reference[u_offset + row + x - shift];
                frame[v_offset + row + x] = reference[v_offset + row + x - shift];
            }
        }

        frame
    }

    #[test]
    fn av2_mvp_444_accepts_high_bit_depth_yuv444_without_downscaling() {
        let geometry = Av2VideoGeometry {
            width: 8,
            height: 8,
        };
        for bits in [10] {
            let format = PixelFormat::yuv444(bits).expect("valid AV2 high-depth 4:4:4 format");
            let request = Av2EncodeRequest {
                params: Av2EncodeParams { frames: 1 },
                geometry,
                format,
            };
            let max_sample = format.bit_depth().max_sample();
            let mid_sample = 1u16 << u32::from(bits - 1);
            let plane_len = geometry.width * geometry.height;
            let frame_len = Picture::expected_len(geometry.width, geometry.height, format);
            let mut input = vec![0; frame_len];
            for sample_index in 0..plane_len {
                let x = sample_index % geometry.width;
                let y = sample_index / geometry.width;
                let y_sample = if (x + y) % 2 == 0 { 0 } else { max_sample - 3 };
                let u_sample = mid_sample + ((x * 3 + y) % 8) as u16;
                let v_sample = (max_sample / 8) + ((x + y * 5) % 16) as u16;
                frameforge_core::write_planar_sample(
                    &mut input,
                    sample_index,
                    y_sample,
                    format.bit_depth(),
                )
                .expect("write Y sample");
                frameforge_core::write_planar_sample(
                    &mut input,
                    plane_len + sample_index,
                    u_sample,
                    format.bit_depth(),
                )
                .expect("write U sample");
                frameforge_core::write_planar_sample(
                    &mut input,
                    2 * plane_len + sample_index,
                    v_sample,
                    format.bit_depth(),
                )
                .expect("write V sample");
            }
            let mut source = input.as_slice();
            let mut output = Vec::new();
            let mut recon = Vec::new();

            av2_encode_fixed_black_444(&mut source, &mut output, Some(&mut recon), request)
                .expect("AV2 high-depth 4:4:4 encode should succeed");

            assert!(!output.is_empty());
            assert_eq!(recon, input);
            let sequence = av2_mvp_444_sequence_header_payload(
                geometry,
                format.bit_depth(),
                Av2Black444MvpProfile::current(),
            );
            assert_has_field(
                &sequence,
                "sequence_header.bitdepth_lut_idx",
                Av2SyntaxCode::Uvlc,
                15,
                expected_uvlc_bit_count(
                    Av2StreamFormat::from_pixel_format(format)
                        .expect("valid AV2 stream format")
                        .bitdepth_lut_index(),
                ),
            );
        }
    }

    #[test]
    fn av2_fixed_black_420_can_use_exact_residual_reconstruction() {
        let geometry = Av2VideoGeometry {
            width: 16,
            height: 16,
        };
        let request = Av2EncodeRequest {
            params: Av2EncodeParams { frames: 1 },
            geometry,
            format: PixelFormat::Yuv420p8,
        };
        let input =
            vec![0; Picture::expected_len(geometry.width, geometry.height, PixelFormat::Yuv420p8,)];
        let mut source = input.as_slice();
        let mut output = Vec::new();
        let mut recon = Vec::new();

        av2_encode_fixed_black_444(&mut source, &mut output, Some(&mut recon), request)
            .expect("AV2 4:2:0 black OBU encode should succeed");

        assert_ne!(output, input);
        assert_ne!(
            output,
            av2_black_bitstream_for_geometry(geometry, Av2StreamFormat::yuv420_8())
        );
        assert_eq!(recon, input);
        assert_eq!(recon.len(), input.len());
        let sequence = av2_mvp_sequence_header_payload(
            geometry,
            Av2Black444MvpProfile::current(),
            Av2StreamFormat::yuv420_8(),
        );
        assert_has_field(
            &sequence,
            "sequence_header.seq_chroma_format_idc",
            Av2SyntaxCode::Uvlc,
            12,
            1,
        );
    }

    #[test]
    fn av2_yuv420_nonblack_emits_lossy_residual_syntax() {
        let geometry = Av2VideoGeometry {
            width: 8,
            height: 8,
        };
        let request = Av2EncodeRequest {
            params: Av2EncodeParams { frames: 1 },
            geometry,
            format: PixelFormat::Yuv420p8,
        };
        let mut input =
            vec![0; Picture::expected_len(geometry.width, geometry.height, PixelFormat::Yuv420p8,)];
        for (index, sample) in input.iter_mut().enumerate() {
            *sample = (17 + index * 5) as u8;
        }
        let mut source = input.as_slice();
        let mut output = Vec::new();
        let mut recon = Vec::new();

        av2_encode_fixed_black_444(&mut source, &mut output, Some(&mut recon), request)
            .expect("AV2 4:2:0 lossy residual encode should succeed");

        assert_ne!(
            output,
            av2_black_bitstream_for_geometry(geometry, Av2StreamFormat::yuv420_8())
        );
        assert_eq!(recon.len(), input.len());
        let trace = av2_mvp_444_trace_jsonl_for_frame(&input, request)
            .expect("AV2 4:2:0 lossy residual trace should be emitted");
        assert!(
            trace.contains("tile.coeff.y.txb_nonzero_tx4x4_ctx"),
            "non-black 4:2:0 inputs should emit residual coefficient syntax"
        );
    }

    #[test]
    fn av2_regular_qp_intra_modes_skip_lossless_bdpcm_flags() {
        let geometry = Av2VideoGeometry {
            width: 8,
            height: 8,
        };
        let format = PixelFormat::Yuv420p8;
        let bit_depth = SampleBitDepth::new(8).expect("8-bit depth is supported");
        let mut source = vec![0; Picture::expected_len(geometry.width, geometry.height, format)];
        for (index, sample) in source.iter_mut().enumerate() {
            *sample = (23 + index * 7) as u8;
        }
        let mut recon = vec![0; source.len()];
        let qp = 24;
        let payload = av2_lossy_subsampled_tile_entropy_payload_for_region_with_fields(
            Av2TileRegion::root(geometry),
            Av2Black444MvpProfile::current(),
            geometry,
            Av2ChromaFormat::Yuv420,
            bit_depth,
            &source,
            &mut recon,
            qp,
            Av2QuantizationParams::regular_qp(qp, bit_depth).base_qindex,
            true,
        );

        assert!(
            payload
                .fields
                .iter()
                .any(|field| field.name == "tile.intra.y_mode_set_index"),
            "regular-q lossy luma should start at read_intra_luma_mode syntax"
        );
        assert!(
            payload
                .fields
                .iter()
                .any(|field| field.name.starts_with("tile.intra.uv_mode_idx")),
            "regular-q lossy chroma should start at read_intra_uv_mode syntax"
        );
        assert!(
            payload
                .fields
                .iter()
                .all(|field| field.name != "tile.intra.use_dpcm_y"),
            "regular-q lossy luma must not emit lossless BDPCM syntax"
        );
        assert!(
            payload
                .fields
                .iter()
                .all(|field| field.name != "tile.intra.use_dpcm_uv"),
            "regular-q lossy chroma must not emit lossless BDPCM syntax"
        );
    }

    #[test]
    fn av2_qp_path_can_keep_yuv420_blocks_lossless() {
        let geometry = Av2VideoGeometry {
            width: 8,
            height: 8,
        };
        let format = PixelFormat::Yuv420p8;
        let request = Av2EncodeRequest {
            params: Av2EncodeParams { frames: 1 },
            geometry,
            format,
        };
        let mut input = vec![128; Picture::expected_len(geometry.width, geometry.height, format)];
        let y_len = geometry.width * geometry.height;
        for sample in &mut input[y_len..] {
            *sample = 129;
        }
        let mut source = input.as_slice();
        let mut output = Vec::new();
        let mut recon = Vec::new();

        av2_encode_fixed_black_444_with_options_and_frame_metrics(
            &mut source,
            &mut output,
            Some(&mut recon),
            request,
            Av2EncodeOptions {
                lossless: false,
                qp: Some(8),
                predictive: false,
            },
            None,
        )
        .expect("AV2 QP residual path should encode predictor-matched blocks");

        assert!(!output.is_empty());
        assert_eq!(recon, input);
    }

    #[test]
    fn av2_qp_path_accepts_yuv422() {
        let geometry = Av2VideoGeometry {
            width: 8,
            height: 8,
        };
        let format = PixelFormat::yuv422(8).expect("valid 8-bit 4:2:2 format");
        let request = Av2EncodeRequest {
            params: Av2EncodeParams { frames: 1 },
            geometry,
            format,
        };
        let mut input = vec![128; Picture::expected_len(geometry.width, geometry.height, format)];
        let y_len = geometry.width * geometry.height;
        for sample in &mut input[y_len..] {
            *sample = 129;
        }
        let mut source = input.as_slice();
        let mut output = Vec::new();
        let mut recon = Vec::new();

        av2_encode_fixed_black_444_with_options_and_frame_metrics(
            &mut source,
            &mut output,
            Some(&mut recon),
            request,
            Av2EncodeOptions {
                lossless: false,
                qp: Some(8),
                predictive: false,
            },
            None,
        )
        .expect("AV2 QP residual path should encode yuv422");

        assert!(!output.is_empty());
        assert_eq!(recon, input);
    }

    #[test]
    fn av2_yuv420_accepts_high_bit_depth_without_downscaling() {
        let geometry = Av2VideoGeometry {
            width: 8,
            height: 8,
        };
        for bits in [10] {
            let format = PixelFormat::yuv420(bits).expect("valid AV2 high-depth 4:2:0 format");
            let request = Av2EncodeRequest {
                params: Av2EncodeParams { frames: 1 },
                geometry,
                format,
            };
            let sample_count = Picture::expected_len(geometry.width, geometry.height, format)
                / format.bytes_per_sample();
            let max_sample = format.bit_depth().max_sample();
            let mut input = vec![0; Picture::expected_len(geometry.width, geometry.height, format)];
            for sample_index in 0..sample_count {
                frameforge_core::write_planar_sample(
                    &mut input,
                    sample_index,
                    max_sample,
                    format.bit_depth(),
                )
                .expect("write high-depth 4:2:0 sample");
            }
            let mut source = input.as_slice();
            let mut output = Vec::new();
            let mut recon = Vec::new();

            av2_encode_fixed_black_444(&mut source, &mut output, Some(&mut recon), request)
                .expect("AV2 high-depth 4:2:0 lossy residual encode should succeed");

            assert!(!output.is_empty());
            assert_eq!(recon.len(), input.len());
            assert!(
                frameforge_core::read_planar_sample(&recon, 0, format.bit_depth())
                    .expect("read reconstructed sample")
                    > u16::from(u8::MAX),
                "high-depth 4:2:0 reconstruction should not be downscaled to 8-bit"
            );
            let stream_format =
                Av2StreamFormat::from_pixel_format(format).expect("valid AV2 stream format");
            let sequence = av2_mvp_sequence_header_payload(
                geometry,
                Av2Black444MvpProfile::current(),
                stream_format,
            );
            assert_has_field_with_bit_count(
                &sequence,
                "sequence_header.bitdepth_lut_idx",
                Av2SyntaxCode::Uvlc,
                expected_uvlc_bit_count(stream_format.bitdepth_lut_index()),
            );
        }
    }

    #[test]
    fn av2_yuv420_lossless_preserves_high_bit_depth_samples() {
        let geometry = Av2VideoGeometry {
            width: 8,
            height: 8,
        };
        let format = PixelFormat::yuv420(10).expect("valid AV2 high-depth 4:2:0 format");
        let request = Av2EncodeRequest {
            params: Av2EncodeParams { frames: 1 },
            geometry,
            format,
        };
        let sample_count = Picture::expected_len(geometry.width, geometry.height, format)
            / format.bytes_per_sample();
        let max_sample = format.bit_depth().max_sample();
        let mut input = vec![0; Picture::expected_len(geometry.width, geometry.height, format)];
        for sample_index in 0..sample_count {
            let sample = ((sample_index * 37 + 11) as u16) & max_sample;
            frameforge_core::write_planar_sample(
                &mut input,
                sample_index,
                sample,
                format.bit_depth(),
            )
            .expect("write high-depth 4:2:0 lossless sample");
        }
        let mut source = input.as_slice();
        let mut output = Vec::new();
        let mut recon = Vec::new();

        av2_encode_fixed_black_444_with_options_and_frame_metrics(
            &mut source,
            &mut output,
            Some(&mut recon),
            request,
            Av2EncodeOptions {
                lossless: true,
                ..Default::default()
            },
            None,
        )
        .expect("AV2 lossless 4:2:0 should encode stream-exact");

        assert!(!output.is_empty());
        assert_eq!(recon, input);
    }

    #[test]
    fn av2_yuv420_lossless_fast_path_writes_reconstruction() {
        let geometry = Av2VideoGeometry {
            width: 128,
            height: 128,
        };
        let format = PixelFormat::Yuv420p8;
        let request = Av2EncodeRequest {
            params: Av2EncodeParams { frames: 1 },
            geometry,
            format,
        };
        let mut input = vec![0; Picture::expected_len(geometry.width, geometry.height, format)];
        for (index, sample) in input.iter_mut().enumerate() {
            *sample = ((index * 37 + index / 11 + 23) & 0xff) as u8;
        }
        let mut source = input.as_slice();
        let mut output = Vec::new();
        let mut recon = Vec::new();

        av2_encode_fixed_black_444_with_options_and_frame_metrics(
            &mut source,
            &mut output,
            Some(&mut recon),
            request,
            Av2EncodeOptions {
                lossless: true,
                ..Default::default()
            },
            None,
        )
        .expect("AV2 4:2:0 fast lossless path should encode stream-exact");

        assert!(!output.is_empty());
        assert_eq!(recon, input);
    }

    #[test]
    fn av2_yuv422_lossless_preserves_high_bit_depth_samples() {
        let geometry = Av2VideoGeometry {
            width: 8,
            height: 8,
        };
        let format = PixelFormat::yuv422(10).expect("valid AV2 high-depth 4:2:2 format");
        let request = Av2EncodeRequest {
            params: Av2EncodeParams { frames: 1 },
            geometry,
            format,
        };
        let sample_count = Picture::expected_len(geometry.width, geometry.height, format)
            / format.bytes_per_sample();
        let max_sample = format.bit_depth().max_sample();
        let mut input = vec![0; Picture::expected_len(geometry.width, geometry.height, format)];
        for sample_index in 0..sample_count {
            let sample = ((sample_index * 53 + 7) as u16) & max_sample;
            frameforge_core::write_planar_sample(
                &mut input,
                sample_index,
                sample,
                format.bit_depth(),
            )
            .expect("write high-depth 4:2:2 lossless sample");
        }
        let mut source = input.as_slice();
        let mut output = Vec::new();
        let mut recon = Vec::new();

        av2_encode_fixed_black_444_with_options_and_frame_metrics(
            &mut source,
            &mut output,
            Some(&mut recon),
            request,
            Av2EncodeOptions {
                lossless: true,
                ..Default::default()
            },
            None,
        )
        .expect("AV2 lossless 4:2:2 should encode stream-exact");

        assert!(!output.is_empty());
        assert_eq!(recon, input);
        let stream_format =
            Av2StreamFormat::from_pixel_format(format).expect("valid AV2 stream format");
        let sequence = av2_mvp_sequence_header_payload(
            geometry,
            Av2Black444MvpProfile::current(),
            stream_format,
        );
        assert_has_field(
            &sequence,
            "sequence_header.seq_chroma_format_idc",
            Av2SyntaxCode::Uvlc,
            12,
            expected_uvlc_bit_count(stream_format.chroma_format.sequence_header_idc()),
        );
    }

    #[test]
    fn av2_fixed_black_444_sequence_header_has_labeled_fields() {
        let payload = av2_black_444_sequence_header_payload(Av2VideoGeometry {
            width: 64,
            height: 64,
        });

        assert_eq!(
            payload.bytes,
            vec![0x92, 0x06, 0x95, 0x7f, 0xfc, 0x00, 0x01, 0x12, 0x0d, 0xc0, 0x44,]
        );
        assert_has_field(
            &payload,
            "sequence_header.seq_profile_idc",
            Av2SyntaxCode::Literal,
            1,
            5,
        );
        assert_has_field(
            &payload,
            "sequence_header.max_frame_width_minus_1",
            Av2SyntaxCode::Literal,
            26,
            6,
        );
        assert_has_field(
            &payload,
            "sequence_transform.enable_chroma_dctonly",
            Av2SyntaxCode::Flag,
            62,
            1,
        );
        assert_has_field(
            &payload,
            "sequence_transform.base_uv_ac_delta_q_minus_min",
            Av2SyntaxCode::Literal,
            69,
            5,
        );
        assert_has_field(
            &payload,
            "trailing_bits",
            Av2SyntaxCode::TrailingBits,
            85,
            3,
        );
    }

    #[test]
    fn av2_fixed_black_444_closed_loop_key_labels_header_fields() {
        let payload = av2_black_444_closed_loop_key_header_payload();

        assert_eq!(payload.bytes, vec![0xe2, 0x00, 0x00]);
        assert_has_field(
            &payload,
            "tile_group.first_tile_group_in_frame",
            Av2SyntaxCode::Flag,
            0,
            1,
        );
        assert_has_field(
            &payload,
            "quantization.base_qindex",
            Av2SyntaxCode::Literal,
            7,
            8,
        );
    }

    #[test]
    fn av2_lossless_header_stays_coded_lossless_compatible() {
        let tile_layout = Av2TileLayout::for_geometry(Av2VideoGeometry {
            width: 64,
            height: 64,
        });
        let payload = av2_mvp_444_closed_loop_key_header_payload(
            false,
            false,
            &tile_layout,
            Av2StreamFormat::yuv420_8(),
            Av2QuantizationParams::lossless(),
        );

        assert_has_field_with_bit_count(
            &payload,
            "quantization.base_qindex",
            Av2SyntaxCode::Literal,
            8,
        );
        assert_no_field(&payload, "delta_q.present");
        assert_no_field(&payload, "loop_filter.apply_deblocking_filter_y_vertical");
        assert_no_field(&payload, "uncompressed_header.tx_mode_select");
    }

    #[test]
    fn av2_regular_qp_header_can_signal_qindex_and_disabled_delta_q() {
        let tile_layout = Av2TileLayout::for_geometry(Av2VideoGeometry {
            width: 64,
            height: 64,
        });
        let bit_depth = SampleBitDepth::new(10).expect("10-bit depth is supported");
        let quantization = Av2QuantizationParams::regular_qp(24, bit_depth);
        assert_eq!(quantization.base_qindex, 80);
        let payload = av2_mvp_444_closed_loop_key_header_payload(
            false,
            false,
            &tile_layout,
            Av2StreamFormat {
                chroma_format: Av2ChromaFormat::Yuv420,
                bit_depth,
            },
            quantization,
        );

        assert_has_field_with_bit_count(
            &payload,
            "quantization.base_qindex",
            Av2SyntaxCode::Literal,
            9,
        );
        assert_has_field_with_bit_count(&payload, "delta_q.present", Av2SyntaxCode::Flag, 1);
        assert_has_field_with_bit_count(
            &payload,
            "loop_filter.apply_deblocking_filter_y_vertical",
            Av2SyntaxCode::Flag,
            1,
        );
        assert_has_field_with_bit_count(
            &payload,
            "loop_filter.apply_deblocking_filter_y_horizontal",
            Av2SyntaxCode::Flag,
            1,
        );
        assert_has_field_with_bit_count(
            &payload,
            "uncompressed_header.tx_mode_select",
            Av2SyntaxCode::Flag,
            1,
        );
        assert_no_field(&payload, "delta_q.resolution_log2");
    }

    #[test]
    fn av2_fixed_black_444_closed_loop_key_carries_generated_tile_entropy_payload() {
        let payload = av2_black_444_closed_loop_key_payload(Av2VideoGeometry {
            width: 64,
            height: 64,
        });

        assert_eq!(&payload.bytes[..3], &[0xf1, 0x00, 0x00]);
        assert!(payload.bytes.len() > 3);
        let entropy_field = payload
            .fields
            .iter()
            .find(|field| field.name == "tile_group.tile_entropy_payload")
            .expect("missing AV2 tile entropy payload field");
        assert_eq!(entropy_field.code, Av2SyntaxCode::TileEntropyPayload);
        assert_eq!(entropy_field.bit_offset, 24);
        assert_eq!(entropy_field.bit_count, (payload.bytes.len() - 3) * 8);
    }

    #[test]
    fn av2_luma_palette_444_accepts_two_luma_colors_with_zero_chroma() {
        let request = Av2EncodeRequest {
            params: Av2EncodeParams { frames: 1 },
            geometry: Av2VideoGeometry {
                width: AV2_FIXED_BLACK_444_WIDTH,
                height: AV2_FIXED_BLACK_444_HEIGHT,
            },
            format: PixelFormat::Yuv444p8,
        };
        let mut input = av2_black_64x64_444_reconstruction();
        let y_plane_len = AV2_FIXED_BLACK_444_WIDTH * AV2_FIXED_BLACK_444_HEIGHT;
        for sample in &mut input[y_plane_len / 2..y_plane_len] {
            *sample = 96;
        }
        let mut source = input.as_slice();
        let mut output = Vec::new();
        let mut recon = Vec::new();

        let result =
            av2_encode_fixed_black_444(&mut source, &mut output, Some(&mut recon), request);

        result.expect("two-color luma palette should encode");
        assert_ne!(
            output,
            av2_black_444_bitstream_for_geometry(request.geometry)
        );
        assert_eq!(recon, input);
    }

    #[test]
    fn av2_mvp_444_preserves_chroma_with_bdpcm_residuals() {
        let request = Av2EncodeRequest {
            params: Av2EncodeParams { frames: 1 },
            geometry: Av2VideoGeometry {
                width: AV2_FIXED_BLACK_444_WIDTH,
                height: AV2_FIXED_BLACK_444_HEIGHT,
            },
            format: PixelFormat::Yuv444p8,
        };
        let mut input = av2_black_64x64_444_reconstruction();
        let y_plane_len = AV2_FIXED_BLACK_444_WIDTH * AV2_FIXED_BLACK_444_HEIGHT;
        input[y_plane_len] = 1;
        let mut source = input.as_slice();
        let mut output = Vec::new();
        let mut recon = Vec::new();

        let result =
            av2_encode_fixed_black_444(&mut source, &mut output, Some(&mut recon), request);

        result.expect("content must not be rejected by the AV2 MVP path");
        assert_ne!(
            output,
            av2_black_444_bitstream_for_geometry(request.geometry)
        );
        assert_eq!(recon, input);
    }

    #[test]
    fn av2_mvp_444_can_select_vertical_chroma_bdpcm() {
        let request = Av2EncodeRequest {
            params: Av2EncodeParams { frames: 1 },
            geometry: Av2VideoGeometry {
                width: 8,
                height: 16,
            },
            format: PixelFormat::Yuv444p8,
        };
        let plane_len = request.geometry.width * request.geometry.height;
        let mut input = vec![0u8; plane_len * 3];
        for y in 0..16usize {
            for x in 0..8usize {
                let index = y * 8 + x;
                // Keep the two 8x8 blocks from becoming an IntraBC copy while
                // preserving the chroma edge that vertical DPCM can reuse.
                input[index] = if y < 8 { 0 } else { 1 };
                input[plane_len + index] = 127 + (x as u8 * 7);
                input[2 * plane_len + index] = 127 + (x as u8 * 7);
            }
        }
        let mut source = input.as_slice();
        let mut output = Vec::new();
        let mut recon = Vec::new();

        av2_encode_fixed_black_444(&mut source, &mut output, Some(&mut recon), request)
            .expect("vertical chroma BDPCM should encode");

        assert_eq!(recon, input);
        let trace = av2_mvp_444_trace_jsonl_for_frame(&input, request)
            .expect("AV2 trace should be emitted");
        assert!(
            trace.lines().any(|line| {
                line.contains("\"name\":\"tile.intra.dpcm_uv_horz\"")
                    && line.contains("\"symbol\":0")
            }),
            "vertical chroma BDPCM should signal dpcm_uv_horz=0"
        );
    }

    #[test]
    fn av2_mvp_444_preserves_over_limit_luma_colors_with_lossless_residual() {
        let request = Av2EncodeRequest {
            params: Av2EncodeParams { frames: 1 },
            geometry: Av2VideoGeometry {
                width: AV2_FIXED_BLACK_444_WIDTH,
                height: AV2_FIXED_BLACK_444_HEIGHT,
            },
            format: PixelFormat::Yuv444p8,
        };
        let mut input = av2_black_64x64_444_reconstruction();
        let y_plane_len = AV2_FIXED_BLACK_444_WIDTH * AV2_FIXED_BLACK_444_HEIGHT;
        for (index, sample) in input[..y_plane_len].iter_mut().enumerate() {
            *sample = (index & 0xff) as u8;
        }
        let mut source = input.as_slice();
        let mut output = Vec::new();
        let mut recon = Vec::new();

        let result =
            av2_encode_fixed_black_444(&mut source, &mut output, Some(&mut recon), request);

        result.expect("over-limit luma colors should encode through lossless residuals");
        assert_ne!(
            output,
            av2_black_444_bitstream_for_geometry(request.geometry)
        );
        assert_eq!(recon, input);
        let trace = av2_mvp_444_trace_jsonl_for_frame(&input, request)
            .expect("AV2 trace should be emitted");
        assert!(
            trace.contains("tile.coeff.y.idtx_base")
                || trace.contains("tile.coeff.y.txb_nonzero_tx4x4_ctx"),
            "over-limit luma palette blocks must emit lossless luma coefficient residuals"
        );
        assert!(recon[y_plane_len..].iter().all(|&sample| sample == 0));
    }

    #[test]
    fn av2_mvp_444_can_select_horizontal_luma_dpcm_prediction() {
        let request = Av2EncodeRequest {
            params: Av2EncodeParams { frames: 1 },
            geometry: Av2VideoGeometry {
                width: 16,
                height: 8,
            },
            format: PixelFormat::Yuv444p8,
        };
        let mut input = vec![0u8; 16 * 8 * 3];
        for y in 0..8usize {
            let edge = 16 + y as u8 * 28;
            input[y * 16 + 7] = edge;
            for x in 0..8usize {
                input[y * 16 + 8 + x] = if x < 3 { edge } else { edge + 20 };
            }
        }
        let mut source = input.as_slice();
        let mut output = Vec::new();
        let mut recon = Vec::new();

        av2_encode_fixed_black_444(&mut source, &mut output, Some(&mut recon), request)
            .expect("horizontal intra luma prediction should encode");

        assert_eq!(recon, input);
        let trace = av2_mvp_444_trace_jsonl_for_frame(&input, request)
            .expect("AV2 trace should be emitted");
        assert!(
            trace
                .lines()
                .any(|line| line.contains("\"name\":\"tile.intra.use_dpcm_y\"")
                    && line.contains("\"symbol\":1")),
            "lossless luma DPCM should be selected for the right block"
        );
        assert!(
            trace
                .lines()
                .any(|line| line.contains("\"name\":\"tile.intra.dpcm_y_horz\"")
                    && line.contains("\"symbol\":1")),
            "horizontal luma DPCM should be selected for the right block"
        );
    }

    #[test]
    fn av2_rejects_zero_frames() {
        let request = Av2EncodeRequest {
            params: Av2EncodeParams { frames: 0 },
            geometry: Av2VideoGeometry {
                width: 64,
                height: 64,
            },
            format: PixelFormat::Yuv420p8,
        };

        assert!(request.validate().is_err());
    }

    #[test]
    fn av2_closed_loop_key_uses_variable_leb_for_large_payloads() {
        assert_eq!(leb128_len((1 << 21) - 1), 3);
        assert_eq!(leb128_len(1 << 21), 4);

        let mut out = Vec::new();
        write_leb128(1 << 21, &mut out);
        assert_eq!(out, [0x80, 0x80, 0x80, 0x01]);
    }

    fn assert_has_field(
        payload: &Av2SyntaxPayload,
        name: &'static str,
        code: Av2SyntaxCode,
        bit_offset: usize,
        bit_count: usize,
    ) {
        assert!(
            payload.fields.iter().any(|field| {
                field.name == name
                    && field.code == code
                    && field.bit_offset == bit_offset
                    && field.bit_count == bit_count
            }),
            "missing AV2 syntax field {name} at bit {bit_offset} with {bit_count} bit(s)"
        );
    }

    fn assert_has_field_with_bit_count(
        payload: &Av2SyntaxPayload,
        name: &'static str,
        code: Av2SyntaxCode,
        bit_count: usize,
    ) {
        assert!(
            payload.fields.iter().any(|field| {
                field.name == name && field.code == code && field.bit_count == bit_count
            }),
            "missing AV2 syntax field {name} with {bit_count} bit(s)"
        );
    }

    fn assert_no_field(payload: &Av2SyntaxPayload, name: &'static str) {
        assert!(
            payload.fields.iter().all(|field| field.name != name),
            "unexpected AV2 syntax field {name}"
        );
    }

    fn expected_uvlc_bit_count(value: u32) -> usize {
        let code_num = value + 1;
        let bits = 32 - code_num.leading_zeros();
        (bits * 2 - 1) as usize
    }

    fn supported_black_444_geometries() -> Vec<Av2VideoGeometry> {
        let mut geometries = Vec::new();
        for height in (8..=64).step_by(8) {
            for width in (8..=64).step_by(8) {
                geometries.push(Av2VideoGeometry { width, height });
            }
        }
        geometries
    }
}
