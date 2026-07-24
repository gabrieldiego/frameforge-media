use super::ctu_split::{
    vvc_chroma_height, vvc_chroma_split_availability, vvc_chroma_width, VvcChromaSplitAvailability,
    VvcCodingTreeNode, VvcCtuCabacOp, VvcCtuPartitionParams, VvcLumaNeighbourState, VvcPartSplit,
    VvcQtSplitCtxInput, VvcSplitCtxInput, VvcTreeType,
};
use super::{VvcCabacContext, VvcCabacContexts, VvcCabacEncoder};
use crate::picture::ChromaSampling;
use crate::vvc::residual::{VvcResidualCabacEncoder, VvcResidualCabacSymbolStream};
use crate::vvc::{
    chroma_subsample_x, chroma_subsample_y, vvc_chroma_cclm_node_allowed,
    vvc_chroma_explicit_candidate_index, VvcBdpcmMode, VvcChromaCclmMode,
    VvcChromaIntraPredictionMode, VvcIntraPredictionMode, VvcResidualComponent,
    VvcSliceSyntaxConfig, VvcVideoGeometry, VVC_CHROMA_AC_COEFFS_PER_TU, VVC_CTU_SIZE,
    VVC_CURRENT_ENCODER_CHROMA_420_TB_SIZE, VVC_CURRENT_MAX_LUMA_MTT_DEPTH,
};

const VVC_LUMA_ANGULAR_BASE: i16 = 2;
const VVC_LUMA_MODE_NEIGHBOUR_CELL_SIZE: u16 = 4;
const VVC_CHROMA_NEIGHBOUR_CELL_SIZE: u16 = 2;
const VVC_NUM_LUMA_MODES: u32 = 67;
const VVC_NUM_MOST_PROBABLE_LUMA_MODES: usize = 6;
const VVC_REMAINING_LUMA_MODE_COUNT: u32 =
    VVC_NUM_LUMA_MODES - VVC_NUM_MOST_PROBABLE_LUMA_MODES as u32;
const VVC_NUM_INTRA_ANGULAR_MODES: i16 = 65;
const VVC_NUM_INTRA_ANGULAR_MODE_WRAP: i16 = VVC_NUM_INTRA_ANGULAR_MODES - 1;

pub(in crate::vvc) fn encode_ctu_partition_body(
    cabac: &mut VvcCabacEncoder,
    params: &VvcCtuPartitionParams,
    slice_config: VvcSliceSyntaxConfig,
) {
    let mut contexts = initial_vvc_cabac_contexts(slice_config);
    encode_ctu_partition_body_with_contexts(cabac, &mut contexts, params, slice_config);
}

fn vvc_luma_mpm_list(
    left: Option<VvcIntraPredictionMode>,
    above: Option<VvcIntraPredictionMode>,
) -> [u8; VVC_NUM_MOST_PROBABLE_LUMA_MODES] {
    let left = left
        .unwrap_or(VvcIntraPredictionMode::Planar)
        .luma_mode_index();
    let above = above
        .unwrap_or(VvcIntraPredictionMode::Planar)
        .luma_mode_index();
    let min = left.min(above);
    let max = left.max(above);
    let mut mpm = [0; VVC_NUM_MOST_PROBABLE_LUMA_MODES];
    mpm[0] = VvcIntraPredictionMode::Planar.luma_mode_index();
    if max < VVC_LUMA_ANGULAR_BASE as u8 {
        mpm[1] = VvcIntraPredictionMode::Dc.luma_mode_index();
        mpm[2] = VvcIntraPredictionMode::Vertical.luma_mode_index();
        mpm[3] = VvcIntraPredictionMode::Horizontal.luma_mode_index();
        mpm[4] = vvc_wrap_luma_angular_mode(
            i16::from(VvcIntraPredictionMode::Vertical.luma_mode_index()) - 4,
        );
        mpm[5] = vvc_wrap_luma_angular_mode(
            i16::from(VvcIntraPredictionMode::Vertical.luma_mode_index()) + 4,
        );
        return mpm;
    }
    if left == above || min < VVC_LUMA_ANGULAR_BASE as u8 {
        mpm[1] = max;
        mpm[2] = vvc_wrap_luma_angular_mode(i16::from(max) - 1);
        mpm[3] = vvc_wrap_luma_angular_mode(i16::from(max) + 1);
        mpm[4] = vvc_wrap_luma_angular_mode(i16::from(max) - 2);
        mpm[5] = vvc_wrap_luma_angular_mode(i16::from(max) + 2);
        return mpm;
    }

    mpm[1] = left;
    mpm[2] = above;
    let diff = max - min;
    if diff == 1 {
        mpm[3] = vvc_wrap_luma_angular_mode(i16::from(min) - 1);
        mpm[4] = vvc_wrap_luma_angular_mode(i16::from(max) + 1);
        mpm[5] = vvc_wrap_luma_angular_mode(i16::from(min) - 2);
    } else if diff >= VVC_NUM_INTRA_ANGULAR_MODES as u8 - 3 {
        mpm[3] = vvc_wrap_luma_angular_mode(i16::from(min) + 1);
        mpm[4] = vvc_wrap_luma_angular_mode(i16::from(max) - 1);
        mpm[5] = vvc_wrap_luma_angular_mode(i16::from(min) + 2);
    } else if diff == 2 {
        mpm[3] = vvc_wrap_luma_angular_mode(i16::from(min) + 1);
        mpm[4] = vvc_wrap_luma_angular_mode(i16::from(min) - 1);
        mpm[5] = vvc_wrap_luma_angular_mode(i16::from(max) + 1);
    } else {
        mpm[3] = vvc_wrap_luma_angular_mode(i16::from(min) - 1);
        mpm[4] = vvc_wrap_luma_angular_mode(i16::from(min) + 1);
        mpm[5] = vvc_wrap_luma_angular_mode(i16::from(max) - 1);
    }
    mpm
}

#[cfg(test)]
pub(in crate::vvc) fn vvc_luma_mpm_list_for_test(
    left: Option<VvcIntraPredictionMode>,
    above: Option<VvcIntraPredictionMode>,
) -> [u8; VVC_NUM_MOST_PROBABLE_LUMA_MODES] {
    vvc_luma_mpm_list(left, above)
}

pub(in crate::vvc) fn vvc_luma_intra_mode_syntax_bin_count(
    mode: VvcIntraPredictionMode,
    left: Option<VvcIntraPredictionMode>,
    above: Option<VvcIntraPredictionMode>,
) -> u8 {
    let mode_index = mode.luma_mode_index();
    let mpm = vvc_luma_mpm_list(left, above);
    if let Some(mpm_idx) = vvc_luma_mpm_index_for_mode_index(mode_index, mpm) {
        let bypass_bins = if mpm_idx == 0 { 0 } else { mpm_idx.min(4) };
        return 2 + bypass_bins as u8;
    }

    1 + vvc_trunc_bin_code_ep_bin_count(
        vvc_luma_remaining_mode_index(mode_index, mpm),
        VVC_REMAINING_LUMA_MODE_COUNT,
    )
}

pub(in crate::vvc) fn vvc_luma_intra_mode_is_mpm(
    mode: VvcIntraPredictionMode,
    left: Option<VvcIntraPredictionMode>,
    above: Option<VvcIntraPredictionMode>,
) -> bool {
    vvc_luma_mpm_index_for_mode_index(mode.luma_mode_index(), vvc_luma_mpm_list(left, above))
        .is_some()
}

fn vvc_luma_mpm_index_for_mode_index(
    mode_index: u8,
    mpm: [u8; VVC_NUM_MOST_PROBABLE_LUMA_MODES],
) -> Option<usize> {
    mpm.iter().position(|candidate| *candidate == mode_index)
}

pub(in crate::vvc) fn vvc_chroma_intra_mode_syntax_bin_count(
    mode: VvcChromaIntraPredictionMode,
    cclm_enabled: bool,
) -> u8 {
    let cclm_flag_bins = u8::from(cclm_enabled);
    match mode {
        VvcChromaIntraPredictionMode::Cclm(cclm_mode) => {
            debug_assert!(cclm_enabled);
            cclm_flag_bins
                + 1
                + match cclm_mode {
                    VvcChromaCclmMode::Linear => 0,
                    VvcChromaCclmMode::MdlmLeft | VvcChromaCclmMode::MdlmTop => 1,
                }
        }
        VvcChromaIntraPredictionMode::Derived => cclm_flag_bins + 1,
        VvcChromaIntraPredictionMode::Explicit(_) => cclm_flag_bins + 3,
    }
}

fn vvc_wrap_luma_angular_mode(mode: i16) -> u8 {
    ((mode - VVC_LUMA_ANGULAR_BASE).rem_euclid(VVC_NUM_INTRA_ANGULAR_MODE_WRAP)
        + VVC_LUMA_ANGULAR_BASE) as u8
}

fn vvc_luma_remaining_mode_index(
    mode_index: u8,
    mut mpm: [u8; VVC_NUM_MOST_PROBABLE_LUMA_MODES],
) -> u32 {
    let mut remaining = u32::from(mode_index);
    mpm.sort_unstable();
    for candidate in mpm.into_iter().rev() {
        if remaining > u32::from(candidate) {
            remaining -= 1;
        }
    }
    debug_assert!(remaining < VVC_REMAINING_LUMA_MODE_COUNT);
    remaining
}

fn vvc_trunc_bin_code_ep_bin_count(symbol: u32, num_symbols: u32) -> u8 {
    debug_assert!(symbol < num_symbols);
    let thresh = 31 - num_symbols.leading_zeros();
    let val = 1 << thresh;
    let b = num_symbols - val;
    if symbol < val - b {
        thresh as u8
    } else {
        (thresh + 1) as u8
    }
}

fn encode_vvc_trunc_bin_code_ep(cabac: &mut VvcCabacEncoder, symbol: u32, num_symbols: u32) {
    debug_assert!(symbol < num_symbols);
    let thresh = 31 - num_symbols.leading_zeros();
    let val = 1 << thresh;
    let b = num_symbols - val;
    if symbol < val - b {
        cabac.encode_bins_ep(symbol, thresh);
    } else {
        cabac.encode_bins_ep(symbol + val - b, thresh + 1);
    }
}

pub(in crate::vvc) fn initial_vvc_cabac_contexts(
    slice_config: VvcSliceSyntaxConfig,
) -> VvcCabacContexts {
    if slice_config.tools.transform_skip_enabled {
        VvcCabacContexts::with_slice_qp(slice_config.slice_qp)
    } else {
        VvcCabacContexts::new()
    }
}

pub(in crate::vvc) fn encode_ctu_partition_body_with_contexts(
    cabac: &mut VvcCabacEncoder,
    contexts: &mut VvcCabacContexts,
    params: &VvcCtuPartitionParams,
    slice_config: VvcSliceSyntaxConfig,
) {
    let ops = VvcCtuCabacOp::ctu_partition(params);
    let mut ctu = VvcCtuCabacGenerator::new(contexts, params, slice_config);
    let mut luma_mode_neighbours =
        VvcLumaModeNeighbourState::new(params.visible_width as u16, params.visible_height as u16);
    for op in ops {
        ctu.emit_with_luma_mode_neighbours(cabac, op, &mut luma_mode_neighbours);
    }
}

pub(in crate::vvc) fn encode_frame_partition_body_with_contexts(
    cabac: &mut VvcCabacEncoder,
    contexts: &mut VvcCabacContexts,
    picture_geometry: VvcVideoGeometry,
    params: &[VvcCtuPartitionParams],
    slice_config: VvcSliceSyntaxConfig,
) {
    let Some(first_ctu) = params.first() else {
        return;
    };
    let picture_width = picture_geometry.coded_width() as u16;
    let picture_height = picture_geometry.coded_height() as u16;
    let ctu_cols = picture_geometry.coded_width().div_ceil(VVC_CTU_SIZE);
    let mut luma_neighbours = VvcLumaNeighbourState::new(picture_width, picture_height);
    let mut luma_mode_neighbours = VvcLumaModeNeighbourState::new(picture_width, picture_height);
    let mut chroma_neighbours =
        VvcChromaNeighbourState::new(picture_width, picture_height, first_ctu.chroma_sampling);

    for (slice_address, ctu) in params.iter().enumerate() {
        let ctu_x = slice_address % ctu_cols;
        let ctu_y = slice_address / ctu_cols;
        let origin_x = (ctu_x * VVC_CTU_SIZE) as u16;
        let origin_y = (ctu_y * VVC_CTU_SIZE) as u16;
        let mut ops = Vec::new();
        VvcCtuCabacOp::append_intra_ctu_partition_with_luma_neighbours(
            &mut ops,
            &mut luma_neighbours,
            ctu.shape(),
            origin_x,
            origin_y,
            picture_width,
            picture_height,
            ctu.luma_max_leaf_size,
        );
        let mut ctu_encoder = VvcCtuCabacGenerator::new(contexts, ctu, slice_config);
        for op in ops {
            ctu_encoder.emit_with_frame_neighbours(
                cabac,
                op,
                &mut luma_mode_neighbours,
                &mut chroma_neighbours,
            );
        }
    }
}

#[cfg(feature = "vvc-stats")]
pub(in crate::vvc) struct VvcFrameCtuCabacState {
    contexts: VvcCabacContexts,
    luma_neighbours: VvcLumaNeighbourState,
    luma_mode_neighbours: VvcLumaModeNeighbourState,
    chroma_neighbours: VvcChromaNeighbourState,
    picture_width: u16,
    picture_height: u16,
    ctu_cols: usize,
}

#[cfg(feature = "vvc-stats")]
impl VvcFrameCtuCabacState {
    pub(in crate::vvc) fn new(
        picture_geometry: VvcVideoGeometry,
        slice_config: VvcSliceSyntaxConfig,
    ) -> Self {
        let picture_width = picture_geometry.coded_width() as u16;
        let picture_height = picture_geometry.coded_height() as u16;
        Self {
            contexts: initial_vvc_cabac_contexts(slice_config),
            luma_neighbours: VvcLumaNeighbourState::new(picture_width, picture_height),
            luma_mode_neighbours: VvcLumaModeNeighbourState::new(picture_width, picture_height),
            chroma_neighbours: VvcChromaNeighbourState::new(
                picture_width,
                picture_height,
                slice_config.coding_tree.chroma_sampling,
            ),
            picture_width,
            picture_height,
            ctu_cols: picture_geometry.coded_width().div_ceil(VVC_CTU_SIZE),
        }
    }

    pub(in crate::vvc) fn encode_ctu(
        &mut self,
        cabac: &mut VvcCabacEncoder,
        slice_address: usize,
        params: &VvcCtuPartitionParams,
        slice_config: VvcSliceSyntaxConfig,
    ) {
        let ctu_x = slice_address % self.ctu_cols;
        let ctu_y = slice_address / self.ctu_cols;
        let origin_x = (ctu_x * VVC_CTU_SIZE) as u16;
        let origin_y = (ctu_y * VVC_CTU_SIZE) as u16;
        let mut ops = Vec::new();
        VvcCtuCabacOp::append_intra_ctu_partition_with_luma_neighbours(
            &mut ops,
            &mut self.luma_neighbours,
            params.shape(),
            origin_x,
            origin_y,
            self.picture_width,
            self.picture_height,
            params.luma_max_leaf_size,
        );
        let mut ctu_encoder = VvcCtuCabacGenerator::new(&mut self.contexts, params, slice_config);
        for op in ops {
            ctu_encoder.emit_with_frame_neighbours(
                cabac,
                op,
                &mut self.luma_mode_neighbours,
                &mut self.chroma_neighbours,
            );
        }
    }
}

#[derive(Debug)]
pub(in crate::vvc) struct VvcCtuCabacGenerator<'a, 'p> {
    contexts: &'a mut VvcCabacContexts,
    params: &'p VvcCtuPartitionParams,
    luma_tu_index: usize,
    chroma_tu_index: usize,
    slice_config: VvcSliceSyntaxConfig,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct VvcChromaNeighbourInfo {
    cb_width: u16,
    cb_height: u16,
    cqt_depth: u8,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct VvcLumaModeNeighbourState {
    width: u16,
    height: u16,
    cell_width: usize,
    valid: Vec<bool>,
    modes: Vec<VvcIntraPredictionMode>,
    mip_flags: Vec<bool>,
}

impl VvcLumaModeNeighbourState {
    fn new(width: u16, height: u16) -> Self {
        let cell_width = usize::from(width.div_ceil(VVC_LUMA_MODE_NEIGHBOUR_CELL_SIZE));
        let cell_height = usize::from(height.div_ceil(VVC_LUMA_MODE_NEIGHBOUR_CELL_SIZE));
        let samples = cell_width * cell_height;
        Self {
            width,
            height,
            cell_width,
            valid: vec![false; samples],
            modes: vec![VvcIntraPredictionMode::Planar; samples],
            mip_flags: vec![false; samples],
        }
    }

    fn index(&self, x: u16, y: u16) -> Option<usize> {
        if x >= self.width || y >= self.height {
            return None;
        }
        let cell_x = usize::from(x / VVC_LUMA_MODE_NEIGHBOUR_CELL_SIZE);
        let cell_y = usize::from(y / VVC_LUMA_MODE_NEIGHBOUR_CELL_SIZE);
        Some(cell_y * self.cell_width + cell_x)
    }

    fn mode_at(&self, x: u16, y: u16) -> Option<VvcIntraPredictionMode> {
        let index = self.index(x, y)?;
        self.valid[index].then_some(self.modes[index])
    }

    fn mip_flag_at(&self, x: u16, y: u16) -> bool {
        self.index(x, y)
            .filter(|&index| self.valid[index])
            .is_some_and(|index| self.mip_flags[index])
    }

    fn left_of(&self, node: VvcCodingTreeNode) -> Option<VvcIntraPredictionMode> {
        let x = node.x.checked_sub(1)?;
        let y = (node.y + node.height)
            .saturating_sub(1)
            .min(self.height.saturating_sub(1));
        self.mode_at(x, y)
    }

    fn left_mip_flag_of(&self, node: VvcCodingTreeNode) -> bool {
        let Some(x) = node.x.checked_sub(1) else {
            return false;
        };
        let y = (node.y + node.height)
            .saturating_sub(1)
            .min(self.height.saturating_sub(1));
        self.mip_flag_at(x, y)
    }

    fn above_of(&self, node: VvcCodingTreeNode) -> Option<VvcIntraPredictionMode> {
        let y = node.y.checked_sub(1)?;
        if node.y % VVC_CTU_SIZE as u16 == 0 {
            return None;
        }
        let x = (node.x + node.width)
            .saturating_sub(1)
            .min(self.width.saturating_sub(1));
        self.mode_at(x, y)
    }

    fn above_mip_flag_of(&self, node: VvcCodingTreeNode) -> bool {
        let Some(y) = node.y.checked_sub(1) else {
            return false;
        };
        let x = (node.x + node.width)
            .saturating_sub(1)
            .min(self.width.saturating_sub(1));
        self.mip_flag_at(x, y)
    }

    fn co_located_for_chroma(&self, node: VvcCodingTreeNode) -> Option<VvcIntraPredictionMode> {
        if self.width == 0 || self.height == 0 {
            return None;
        }
        let x = node
            .x
            .saturating_add(node.width >> 1)
            .min(self.width.saturating_sub(1));
        let y = node
            .y
            .saturating_add(node.height >> 1)
            .min(self.height.saturating_sub(1));
        self.mode_at(x, y)
    }

    fn mark_leaf(&mut self, node: VvcCodingTreeNode, mode: VvcIntraPredictionMode) {
        let end_x = (node.x + node.width).min(self.width);
        let end_y = (node.y + node.height).min(self.height);
        let start_cell_x = node.x / VVC_LUMA_MODE_NEIGHBOUR_CELL_SIZE;
        let start_cell_y = node.y / VVC_LUMA_MODE_NEIGHBOUR_CELL_SIZE;
        let end_cell_x = end_x.div_ceil(VVC_LUMA_MODE_NEIGHBOUR_CELL_SIZE);
        let end_cell_y = end_y.div_ceil(VVC_LUMA_MODE_NEIGHBOUR_CELL_SIZE);
        for cell_y in start_cell_y..end_cell_y {
            for cell_x in start_cell_x..end_cell_x {
                let index = usize::from(cell_y) * self.cell_width + usize::from(cell_x);
                self.valid[index] = true;
                self.modes[index] = mode;
                self.mip_flags[index] = false;
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct VvcChromaNeighbourState {
    width: u16,
    height: u16,
    chroma_sampling: ChromaSampling,
    cell_width: usize,
    valid: Vec<bool>,
    cb_width: Vec<u16>,
    cb_height: Vec<u16>,
    cqt_depth: Vec<u8>,
}

impl VvcChromaNeighbourState {
    fn new(visible_width: u16, visible_height: u16, chroma_sampling: ChromaSampling) -> Self {
        let width = visible_width / chroma_subsample_x(chroma_sampling) as u16;
        let height = visible_height / chroma_subsample_y(chroma_sampling) as u16;
        let cell_width = usize::from(width.div_ceil(VVC_CHROMA_NEIGHBOUR_CELL_SIZE));
        let cell_height = usize::from(height.div_ceil(VVC_CHROMA_NEIGHBOUR_CELL_SIZE));
        let cells = cell_width * cell_height;
        Self {
            width,
            height,
            chroma_sampling,
            cell_width,
            valid: vec![false; cells],
            cb_width: vec![0; cells],
            cb_height: vec![0; cells],
            cqt_depth: vec![0; cells],
        }
    }

    fn node_x(&self, node: VvcCodingTreeNode) -> u16 {
        node.x / chroma_subsample_x(self.chroma_sampling) as u16
    }

    fn node_y(&self, node: VvcCodingTreeNode) -> u16 {
        node.y / chroma_subsample_y(self.chroma_sampling) as u16
    }

    fn node_width(&self, node: VvcCodingTreeNode) -> u16 {
        vvc_chroma_width(node, self.chroma_sampling)
    }

    fn node_height(&self, node: VvcCodingTreeNode) -> u16 {
        vvc_chroma_height(node, self.chroma_sampling)
    }

    fn index(&self, x: u16, y: u16) -> Option<usize> {
        if x >= self.width || y >= self.height {
            return None;
        }
        let cell_x = usize::from(x / VVC_CHROMA_NEIGHBOUR_CELL_SIZE);
        let cell_y = usize::from(y / VVC_CHROMA_NEIGHBOUR_CELL_SIZE);
        Some(cell_y * self.cell_width + cell_x)
    }

    fn info_at(&self, x: u16, y: u16) -> Option<VvcChromaNeighbourInfo> {
        let index = self.index(x, y)?;
        self.valid[index].then_some(VvcChromaNeighbourInfo {
            cb_width: self.cb_width[index],
            cb_height: self.cb_height[index],
            cqt_depth: self.cqt_depth[index],
        })
    }

    fn left_of(&self, node: VvcCodingTreeNode) -> Option<VvcChromaNeighbourInfo> {
        let y = self.node_y(node);
        self.node_x(node)
            .checked_sub(1)
            .and_then(|x| self.info_at(x, y))
    }

    fn above_of(&self, node: VvcCodingTreeNode) -> Option<VvcChromaNeighbourInfo> {
        let x = self.node_x(node);
        self.node_y(node)
            .checked_sub(1)
            .and_then(|y| self.info_at(x, y))
    }

    fn mark_leaf(&mut self, node: VvcCodingTreeNode) {
        let start_x = self.node_x(node);
        let start_y = self.node_y(node);
        let node_width = self.node_width(node);
        let node_height = self.node_height(node);
        let end_x = (start_x + node_width).min(self.width);
        let end_y = (start_y + node_height).min(self.height);
        let start_cell_x = start_x / VVC_CHROMA_NEIGHBOUR_CELL_SIZE;
        let start_cell_y = start_y / VVC_CHROMA_NEIGHBOUR_CELL_SIZE;
        let end_cell_x = end_x.div_ceil(VVC_CHROMA_NEIGHBOUR_CELL_SIZE);
        let end_cell_y = end_y.div_ceil(VVC_CHROMA_NEIGHBOUR_CELL_SIZE);
        for cell_y in start_cell_y..end_cell_y {
            for cell_x in start_cell_x..end_cell_x {
                let index = usize::from(cell_y) * self.cell_width + usize::from(cell_x);
                self.valid[index] = true;
                self.cb_width[index] = node_width;
                self.cb_height[index] = node_height;
                self.cqt_depth[index] = node.cqt_depth;
            }
        }
    }
}

impl<'a, 'p> VvcCtuCabacGenerator<'a, 'p> {
    pub(in crate::vvc) fn new(
        contexts: &'a mut VvcCabacContexts,
        params: &'p VvcCtuPartitionParams,
        slice_config: VvcSliceSyntaxConfig,
    ) -> Self {
        Self {
            contexts,
            params,
            luma_tu_index: 0,
            chroma_tu_index: 0,
            slice_config,
        }
    }

    #[cfg(test)]
    pub(in crate::vvc) fn emit(&mut self, cabac: &mut VvcCabacEncoder, op: VvcCtuCabacOp) {
        let mut luma_mode_neighbours = VvcLumaModeNeighbourState::new(
            self.params.visible_width as u16,
            self.params.visible_height as u16,
        );
        self.emit_with_luma_mode_neighbours(cabac, op, &mut luma_mode_neighbours);
    }

    fn emit_with_luma_mode_neighbours(
        &mut self,
        cabac: &mut VvcCabacEncoder,
        op: VvcCtuCabacOp,
        luma_mode_neighbours: &mut VvcLumaModeNeighbourState,
    ) {
        if vvc_cabac_op_trace_enabled() {
            eprintln!("FF_CABAC_OP {op:?}");
        }
        match op {
            VvcCtuCabacOp::QtSplit {
                node,
                split_ctx,
                write_split_flag,
                write_qt_flag,
                qt_ctx,
            } => self.emit_qt_split(
                cabac,
                node,
                split_ctx,
                write_split_flag,
                write_qt_flag,
                qt_ctx,
            ),
            op @ VvcCtuCabacOp::BtSplit { .. } => self.emit_bt_split(cabac, op),
            VvcCtuCabacOp::LumaLeafWithSplitCtx {
                node,
                write_split_flag,
                split_ctx,
            } => {
                self.emit_luma_leaf_split_with_ctx(cabac, node, write_split_flag, split_ctx);
                if !self.emit_luma_bdpcm_mode(cabac, node, luma_mode_neighbours) {
                    if !self.emit_luma_mip_mode(cabac, node, luma_mode_neighbours) {
                        self.emit_luma_multi_ref_line(cabac, node);
                        self.emit_luma_isp_mode(cabac, node);
                        self.emit_luma_intra_prediction_mode(cabac, node, luma_mode_neighbours);
                    }
                }
                self.emit_luma_residual(cabac, node);
            }
            VvcCtuCabacOp::ChromaTree {
                node,
                visible_width,
                visible_height,
            } => self.emit_chroma_tree(
                cabac,
                node,
                visible_width,
                visible_height,
                luma_mode_neighbours,
            ),
        }
    }

    fn emit_with_frame_neighbours(
        &mut self,
        cabac: &mut VvcCabacEncoder,
        op: VvcCtuCabacOp,
        luma_mode_neighbours: &mut VvcLumaModeNeighbourState,
        chroma_neighbours: &mut VvcChromaNeighbourState,
    ) {
        if vvc_cabac_op_trace_enabled() {
            eprintln!("FF_CABAC_OP {op:?}");
        }
        match op {
            VvcCtuCabacOp::ChromaTree {
                node,
                visible_width,
                visible_height,
            } => self.emit_chroma_tree_with_neighbours(
                cabac,
                node,
                visible_width,
                visible_height,
                luma_mode_neighbours,
                chroma_neighbours,
            ),
            other => self.emit_with_luma_mode_neighbours(cabac, other, luma_mode_neighbours),
        }
    }

    fn emit_bt_split(&mut self, cabac: &mut VvcCabacEncoder, op: VvcCtuCabacOp) {
        let VvcCtuCabacOp::BtSplit {
            node,
            vertical,
            split_ctx,
            write_split_flag,
            write_qt_flag,
            qt_ctx,
            write_mtt_vertical_flag,
            mtt_vertical_ctx,
            write_binary_flag,
            mtt_binary_ctx,
            mtt_binary_value,
        } = op
        else {
            unreachable!("emit_bt_split expects a binary split operation");
        };
        debug_assert!(node.cqt_depth >= 1 || node.mtt_depth > 0 || (node.x == 0 && node.y == 0));
        debug_assert_eq!(node.tree_type, VvcTreeType::DualTreeLuma);
        if write_split_flag {
            self.contexts
                .encode(cabac, VvcCabacContext::SplitFlag(split_ctx), true);
        }
        if write_qt_flag {
            self.contexts
                .encode(cabac, VvcCabacContext::SplitQtFlag(qt_ctx), false);
        }
        if write_mtt_vertical_flag {
            self.contexts.encode(
                cabac,
                VvcCabacContext::MttSplitCuVerticalFlag(mtt_vertical_ctx),
                vertical,
            );
        }
        if write_binary_flag {
            self.contexts.encode(
                cabac,
                VvcCabacContext::MttSplitCuBinaryFlag(mtt_binary_ctx),
                mtt_binary_value,
            );
        }
    }

    fn emit_qt_split(
        &mut self,
        cabac: &mut VvcCabacEncoder,
        node: VvcCodingTreeNode,
        split_ctx: u8,
        write_split_flag: bool,
        write_qt_flag: bool,
        qt_ctx: u8,
    ) {
        debug_assert!(node.cqt_depth <= 3);
        debug_assert_eq!(node.mtt_depth, 0);
        debug_assert_eq!(node.tree_type, VvcTreeType::DualTreeLuma);
        // VVC 7.3.11.4 coding_tree emits split_cu_flag for QT-split luma
        // nodes. Some root-only geometries infer split_qt_flag, while boundary
        // constrained rectangular CTU views write it explicitly.
        if write_split_flag {
            self.contexts
                .encode(cabac, VvcCabacContext::SplitFlag(split_ctx), true);
        }
        if write_qt_flag {
            self.contexts
                .encode(cabac, VvcCabacContext::SplitQtFlag(qt_ctx), true);
        }
    }

    fn emit_luma_leaf_split_with_ctx(
        &mut self,
        cabac: &mut VvcCabacEncoder,
        node: VvcCodingTreeNode,
        write_split_flag: bool,
        split_ctx: u8,
    ) {
        debug_assert!(node.cqt_depth >= 1 || node.mtt_depth > 0 || (node.x == 0 && node.y == 0));
        debug_assert!(node.mtt_depth <= VVC_CURRENT_MAX_LUMA_MTT_DEPTH + node.depth_offset);
        debug_assert_eq!(node.tree_type, VvcTreeType::DualTreeLuma);
        if !write_split_flag {
            return;
        }
        self.contexts
            .encode(cabac, VvcCabacContext::SplitFlag(split_ctx), false);
    }

    fn emit_luma_intra_prediction_mode(
        &mut self,
        cabac: &mut VvcCabacEncoder,
        node: VvcCodingTreeNode,
        neighbours: &mut VvcLumaModeNeighbourState,
    ) {
        debug_assert_eq!(node.tree_type, VvcTreeType::DualTreeLuma);
        let mode = self.params.luma_tu_intra_modes[self.luma_tu_index];
        let mode_index = mode.luma_mode_index();
        let mrl_index = self.params.luma_tu_mrl_index[self.luma_tu_index];
        let mpm = vvc_luma_mpm_list(neighbours.left_of(node), neighbours.above_of(node));
        let mpm_idx = vvc_luma_mpm_index_for_mode_index(mode_index, mpm);
        if mrl_index == 0 {
            self.contexts
                .encode(cabac, VvcCabacContext::IntraLumaMpmFlag, mpm_idx.is_some());
        } else {
            assert!(
                mpm_idx.is_some(),
                "VVC nonzero MRL luma modes must be coded through MPM syntax"
            );
        }
        if let Some(mpm_idx) = mpm_idx {
            if mrl_index == 0 {
                self.contexts
                    .encode(cabac, VvcCabacContext::IntraLumaPlanarFlag(1), mpm_idx > 0);
            } else {
                assert_ne!(
                    mpm_idx, 0,
                    "VVC nonzero MRL cannot be combined with planar luma prediction"
                );
            }
            if mpm_idx > 0 {
                cabac.encode_bin_ep(mpm_idx > 1);
            }
            if mpm_idx > 1 {
                cabac.encode_bin_ep(mpm_idx > 2);
            }
            if mpm_idx > 2 {
                cabac.encode_bin_ep(mpm_idx > 3);
            }
            if mpm_idx > 3 {
                cabac.encode_bin_ep(mpm_idx > 4);
            }
        } else {
            assert_eq!(
                mrl_index, 0,
                "VVC remaining-mode syntax is not legal for nonzero MRL"
            );
            encode_vvc_trunc_bin_code_ep(
                cabac,
                vvc_luma_remaining_mode_index(mode_index, mpm),
                VVC_REMAINING_LUMA_MODE_COUNT,
            );
        }
        neighbours.mark_leaf(node, mode);
    }

    fn emit_luma_bdpcm_mode(
        &mut self,
        cabac: &mut VvcCabacEncoder,
        node: VvcCodingTreeNode,
        neighbours: &mut VvcLumaModeNeighbourState,
    ) -> bool {
        if !self.luma_bdpcm_allowed(node) {
            return false;
        }
        let mode = self.params.luma_tu_bdpcm_modes[self.luma_tu_index];
        self.contexts
            .encode(cabac, VvcCabacContext::BdpcmMode(0), mode.is_enabled());
        if mode.is_enabled() {
            self.contexts.encode(
                cabac,
                VvcCabacContext::BdpcmMode(1),
                matches!(mode, VvcBdpcmMode::Vertical),
            );
            neighbours.mark_leaf(
                node,
                mode.inferred_intra_mode()
                    .expect("enabled BDPCM mode has an inferred intra mode"),
            );
        }
        mode.is_enabled()
    }

    fn emit_luma_mip_mode(
        &mut self,
        cabac: &mut VvcCabacEncoder,
        node: VvcCodingTreeNode,
        neighbours: &mut VvcLumaModeNeighbourState,
    ) -> bool {
        if !self.slice_config.tools.mip_enabled {
            return false;
        }
        let ctx = self.luma_mip_flag_ctx(node, neighbours);
        // Active MIP prediction still needs the matrix predictor tables and
        // syntax payload. Keep the spec flag site wired and emit no-MIP for
        // now when a gated config enables the SPS capability.
        self.contexts
            .encode(cabac, VvcCabacContext::MipFlag(ctx), false);
        false
    }

    fn luma_mip_flag_ctx(
        &self,
        node: VvcCodingTreeNode,
        neighbours: &VvcLumaModeNeighbourState,
    ) -> u8 {
        if node.width > node.height.saturating_mul(2) || node.height > node.width.saturating_mul(2)
        {
            return 3;
        }
        u8::from(neighbours.left_mip_flag_of(node)) + u8::from(neighbours.above_mip_flag_of(node))
    }

    fn emit_luma_multi_ref_line(&mut self, cabac: &mut VvcCabacEncoder, node: VvcCodingTreeNode) {
        debug_assert_eq!(node.tree_type, VvcTreeType::DualTreeLuma);
        // With sps_mrl_enabled_flag set, VVC extend_ref_line emits
        // MultiRefLineIdx for intra luma CUs that are not on the first luma
        // line of the CTU. VTM's MULTI_REF_LINE_IDX table is [0, 1, 2].
        if self.slice_config.tools.mrl_enabled && node.y % VVC_CTU_SIZE as u16 != 0 {
            let mrl_index = self.params.luma_tu_mrl_index[self.luma_tu_index];
            assert_eq!(
                mrl_index.min(2),
                mrl_index,
                "VVC MRL index must be one of 0, 1, or 2"
            );
            self.contexts
                .encode(cabac, VvcCabacContext::MultiRefLineIdx(0), mrl_index != 0);
            if mrl_index != 0 {
                self.contexts
                    .encode(cabac, VvcCabacContext::MultiRefLineIdx(1), mrl_index != 1);
            }
        }
    }

    fn emit_luma_isp_mode(&mut self, cabac: &mut VvcCabacEncoder, node: VvcCodingTreeNode) {
        if !self.luma_isp_allowed(node) {
            return;
        }
        // Active ISP needs split transform-tree ownership. Emit the NONE flag
        // at the VTM syntax site while the production selector remains absent.
        self.contexts
            .encode(cabac, VvcCabacContext::IspMode(0), false);
    }

    fn luma_bdpcm_allowed(&self, node: VvcCodingTreeNode) -> bool {
        self.slice_config.tools.bdpcm_enabled
            && node.width <= 8
            && node.height <= 8
            && node.width >= 4
            && node.height >= 4
    }

    fn luma_isp_allowed(&self, node: VvcCodingTreeNode) -> bool {
        self.slice_config.tools.isp_enabled
            && self.params.luma_tu_mrl_index[self.luma_tu_index] == 0
            && node.width >= 4
            && node.height >= 4
            && node.width <= 64
            && node.height <= 64
            && node.width.is_power_of_two()
            && node.height.is_power_of_two()
    }

    fn emit_luma_cbf(
        &mut self,
        cabac: &mut VvcCabacEncoder,
        node: VvcCodingTreeNode,
        cbf: bool,
        bdpcm: bool,
    ) {
        debug_assert_eq!(node.tree_type, VvcTreeType::DualTreeLuma);
        // VVC 7.3.11.10 transform_unit emits tu_y_coded_flag / cbf_comp
        // through QtCbf[Y].
        self.contexts
            .encode(cabac, VvcCabacContext::QtCbfY(u8::from(bdpcm)), cbf);
    }

    fn emit_luma_residual(&mut self, cabac: &mut VvcCabacEncoder, node: VvcCodingTreeNode) {
        let tu_idx = self.luma_tu_index;
        self.luma_tu_index += 1;
        assert!(
            tu_idx < self.params.luma_tu_count,
            "missing luma TU coefficient data for coding-tree leaf {tu_idx}"
        );
        let dc_level = self.params.luma_tu_dc_levels[tu_idx];
        let cbf = dc_level != 0 || self.params.luma_tu_has_ac[tu_idx];
        let bdpcm_mode = self.params.luma_tu_bdpcm_modes[tu_idx];
        self.emit_luma_cbf(cabac, node, cbf, bdpcm_mode.is_enabled());
        if !cbf {
            return;
        }

        let log2_width = node.width.ilog2() as u8;
        let log2_height = node.height.ilog2() as u8;
        let ac_levels = &self.params.luma_tu_ac_levels[tu_idx];
        let has_ac = self.params.luma_tu_has_ac[tu_idx];
        let transform_skip = self.params.luma_tu_transform_skip[tu_idx];
        let mts_index = self.params.luma_tu_mts_index[tu_idx];
        let mut residual =
            VvcResidualCabacEncoder::new(&mut *self.contexts, self.slice_config.residual_options());
        VvcResidualCabacSymbolStream::emit_luma_stored_coefficients(
            log2_width,
            log2_height,
            dc_level,
            ac_levels,
            has_ac,
            transform_skip,
            bdpcm_mode.is_enabled(),
            mts_index,
            &mut residual,
            cabac,
        );
        self.emit_luma_post_residual_tools(cabac, node, has_ac, transform_skip, mts_index);
    }

    fn emit_luma_post_residual_tools(
        &mut self,
        cabac: &mut VvcCabacEncoder,
        node: VvcCodingTreeNode,
        has_ac: bool,
        transform_skip: bool,
        mts_index: u8,
    ) {
        self.emit_luma_lfnst_idx(cabac, node, has_ac, transform_skip, mts_index);
        self.emit_luma_mts_idx(cabac, node, has_ac, transform_skip, mts_index);
    }

    fn emit_luma_lfnst_idx(
        &mut self,
        cabac: &mut VvcCabacEncoder,
        node: VvcCodingTreeNode,
        has_ac: bool,
        transform_skip: bool,
        mts_index: u8,
    ) {
        if !self.slice_config.tools.lfnst_enabled
            || transform_skip
            || mts_index != 0
            || !has_ac
            || node.width > 64
            || node.height > 64
        {
            return;
        }
        // Active LFNST needs transform-domain candidate ownership and
        // coefficient-group constraints. Keep the syntax site wired and emit
        // lfnst_idx=0 while production selection is absent.
        self.contexts
            .encode(cabac, VvcCabacContext::LfnstIdx(0), false);
    }

    fn emit_luma_mts_idx(
        &mut self,
        cabac: &mut VvcCabacEncoder,
        node: VvcCodingTreeNode,
        has_ac: bool,
        transform_skip: bool,
        mts_index: u8,
    ) {
        if !self.slice_config.tools.explicit_mts_intra_enabled
            || transform_skip
            || !has_ac
            || node.width > 32
            || node.height > 32
        {
            return;
        }
        assert!(
            matches!(mts_index, 0 | 2..=5),
            "VVC MTS index must be DCT2_DCT2 or one of the explicit MTS transform types"
        );

        // H.266 cu_residual() writes mts_idx after the transform tree. The
        // current selector still chooses DCT2_DCT2, but keep the VTM-shaped
        // syntax ready for later non-default transform candidates.
        self.contexts
            .encode(cabac, VvcCabacContext::MtsIdx(0), mts_index != 0);
        if mts_index != 0 {
            for offset in 0..3 {
                let bin = mts_index > 2 + offset;
                self.contexts
                    .encode(cabac, VvcCabacContext::MtsIdx(1 + offset), bin);
                if !bin {
                    break;
                }
            }
        }
    }

    fn emit_chroma_tree(
        &mut self,
        cabac: &mut VvcCabacEncoder,
        node: VvcCodingTreeNode,
        visible_width: u16,
        visible_height: u16,
        luma_mode_neighbours: &VvcLumaModeNeighbourState,
    ) {
        debug_assert_eq!(node.tree_type, VvcTreeType::DualTreeChroma);
        let mut neighbours = VvcChromaNeighbourState::new(
            visible_width,
            visible_height,
            self.params.chroma_sampling,
        );
        self.emit_chroma_tree_with_neighbours(
            cabac,
            node,
            visible_width,
            visible_height,
            luma_mode_neighbours,
            &mut neighbours,
        );
    }

    fn emit_chroma_tree_with_neighbours(
        &mut self,
        cabac: &mut VvcCabacEncoder,
        node: VvcCodingTreeNode,
        visible_width: u16,
        visible_height: u16,
        luma_mode_neighbours: &VvcLumaModeNeighbourState,
        neighbours: &mut VvcChromaNeighbourState,
    ) {
        debug_assert_eq!(node.tree_type, VvcTreeType::DualTreeChroma);
        self.emit_chroma_visible_qt_subtree(
            cabac,
            node,
            visible_width,
            visible_height,
            4,
            luma_mode_neighbours,
            neighbours,
        );
    }

    fn emit_chroma_visible_qt_subtree(
        &mut self,
        cabac: &mut VvcCabacEncoder,
        node: VvcCodingTreeNode,
        visible_width: u16,
        visible_height: u16,
        min_leaf_size: u16,
        luma_mode_neighbours: &VvcLumaModeNeighbourState,
        neighbours: &mut VvcChromaNeighbourState,
    ) {
        debug_assert_eq!(node.tree_type, VvcTreeType::DualTreeChroma);
        if !node.intersects_visible(visible_width, visible_height) {
            return;
        }
        if node.fits_visible(visible_width, visible_height) && self.chroma_leaf_allowed(node) {
            self.emit_chroma_transform_only_leaf(
                cabac,
                node,
                vvc_chroma_split_availability(
                    node,
                    visible_width,
                    visible_height,
                    self.params.chroma_sampling,
                ),
                0,
                luma_mode_neighbours,
                neighbours,
            );
            return;
        }

        if !node.fits_visible(visible_width, visible_height) {
            self.emit_chroma_implicit_boundary_children(
                cabac,
                node,
                visible_width,
                visible_height,
                min_leaf_size,
                luma_mode_neighbours,
                neighbours,
            );
            return;
        }

        let split = vvc_chroma_split_availability(
            node,
            visible_width,
            visible_height,
            self.params.chroma_sampling,
        );
        if split.allow_qt {
            self.emit_chroma_visible_qt_split(cabac, node, split, neighbours);
            for child_idx in 0..4 {
                self.emit_chroma_visible_qt_subtree(
                    cabac,
                    node.qt_child(child_idx),
                    visible_width,
                    visible_height,
                    min_leaf_size,
                    luma_mode_neighbours,
                    neighbours,
                );
            }
        } else {
            // H.266 6.4.1 supplies the available MTT directions after QT is no
            // longer signaled. The current hardware residual subset chooses a
            // legal BT direction that drives the larger remaining axis toward
            // the 8x8 luma-coordinate leaf.
            let vertical = Self::chroma_prefer_vertical_bt(node, split);
            self.emit_chroma_visible_mtt_split(cabac, node, split, vertical, true, neighbours);
            for child_idx in 0..2 {
                self.emit_chroma_visible_qt_subtree(
                    cabac,
                    node.mtt_child(vertical, child_idx),
                    visible_width,
                    visible_height,
                    min_leaf_size,
                    luma_mode_neighbours,
                    neighbours,
                );
            }
        }
    }

    fn emit_chroma_implicit_boundary_children(
        &mut self,
        cabac: &mut VvcCabacEncoder,
        node: VvcCodingTreeNode,
        visible_width: u16,
        visible_height: u16,
        min_leaf_size: u16,
        luma_mode_neighbours: &VvcLumaModeNeighbourState,
        neighbours: &mut VvcChromaNeighbourState,
    ) {
        let split = vvc_chroma_split_availability(
            node,
            visible_width,
            visible_height,
            self.params.chroma_sampling,
        );
        if split.allow_qt {
            if split.allow_btt() {
                self.contexts.encode(
                    cabac,
                    VvcCabacContext::SplitQtFlag(Self::chroma_qt_split_ctx(node, neighbours)),
                    true,
                );
            }
            for child_idx in 0..4 {
                self.emit_chroma_visible_qt_subtree(
                    cabac,
                    node.qt_child(child_idx),
                    visible_width,
                    visible_height,
                    min_leaf_size,
                    luma_mode_neighbours,
                    neighbours,
                );
            }
            return;
        }
        match split.implicit_split {
            VvcPartSplit::Quad => {
                for child_idx in 0..4 {
                    self.emit_chroma_visible_qt_subtree(
                        cabac,
                        node.qt_child(child_idx),
                        visible_width,
                        visible_height,
                        min_leaf_size,
                        luma_mode_neighbours,
                        neighbours,
                    );
                }
            }
            VvcPartSplit::HorizontalBinary | VvcPartSplit::VerticalBinary => {
                let vertical = split.implicit_split == VvcPartSplit::VerticalBinary;
                self.emit_chroma_boundary_bt_split(cabac, node, split, vertical, neighbours);
                for child_idx in 0..2 {
                    self.emit_chroma_visible_qt_subtree(
                        cabac,
                        node.mtt_child_with_boundary_depth_offset(
                            vertical,
                            child_idx,
                            visible_width,
                            visible_height,
                        ),
                        visible_width,
                        visible_height,
                        min_leaf_size,
                        luma_mode_neighbours,
                        neighbours,
                    );
                }
            }
            VvcPartSplit::None => {
                debug_assert!(
                    !node.intersects_visible(visible_width, visible_height),
                    "boundary chroma node must have an implicit split"
                );
            }
        }
    }

    fn emit_chroma_visible_qt_split(
        &mut self,
        cabac: &mut VvcCabacEncoder,
        node: VvcCodingTreeNode,
        split: VvcChromaSplitAvailability,
        neighbours: &VvcChromaNeighbourState,
    ) {
        let qt_ctx = Self::chroma_qt_split_ctx(node, neighbours);
        if split.can_no {
            self.contexts.encode(
                cabac,
                VvcCabacContext::SplitFlag(Self::chroma_split_ctx(node, split, neighbours)),
                true,
            );
        }
        if split.allow_btt() {
            self.contexts
                .encode(cabac, VvcCabacContext::SplitQtFlag(qt_ctx), true);
        }
    }

    fn emit_chroma_visible_mtt_split(
        &mut self,
        cabac: &mut VvcCabacEncoder,
        node: VvcCodingTreeNode,
        split: VvcChromaSplitAvailability,
        vertical: bool,
        binary: bool,
        neighbours: &VvcChromaNeighbourState,
    ) {
        debug_assert!(!split.allow_qt || split.allow_btt());
        if split.can_no {
            self.contexts.encode(
                cabac,
                VvcCabacContext::SplitFlag(Self::chroma_split_ctx(node, split, neighbours)),
                true,
            );
        }
        if split.allow_qt {
            let qt_ctx = Self::chroma_qt_split_ctx(node, neighbours);
            self.contexts
                .encode(cabac, VvcCabacContext::SplitQtFlag(qt_ctx), false);
        }

        let can_hor = split.allow_bt_horizontal || split.allow_tt_horizontal;
        let can_ver = split.allow_bt_vertical || split.allow_tt_vertical;
        if can_ver && can_hor {
            self.contexts.encode(
                cabac,
                VvcCabacContext::MttSplitCuVerticalFlag(Self::chroma_mtt_vertical_ctx(
                    node, split, neighbours,
                )),
                vertical,
            );
        }

        let can_binary = if vertical {
            split.allow_bt_vertical
        } else {
            split.allow_bt_horizontal
        };
        let can_ternary = if vertical {
            split.allow_tt_vertical
        } else {
            split.allow_tt_horizontal
        };
        if can_binary && can_ternary {
            self.contexts.encode(
                cabac,
                VvcCabacContext::MttSplitCuBinaryFlag(VvcCtuCabacOp::mtt_binary_ctx(
                    vertical,
                    node.mtt_depth,
                )),
                binary,
            );
        }
    }

    fn emit_chroma_boundary_bt_split(
        &mut self,
        cabac: &mut VvcCabacEncoder,
        node: VvcCodingTreeNode,
        split: VvcChromaSplitAvailability,
        _vertical: bool,
        neighbours: &VvcChromaNeighbourState,
    ) {
        // H.266 7.3.11.4 still signals split_qt_flag for an implicit
        // boundary BT when both QT and BTT are available; split_cu_flag itself
        // is inferred by 7.4.12.4 and therefore not written.
        if split.allow_qt && split.allow_btt() {
            self.contexts.encode(
                cabac,
                VvcCabacContext::SplitQtFlag(Self::chroma_qt_split_ctx(node, neighbours)),
                false,
            );
        }
    }

    fn emit_chroma_residual(
        contexts: &mut VvcCabacContexts,
        slice_config: VvcSliceSyntaxConfig,
        chroma_sampling: ChromaSampling,
        cabac: &mut VvcCabacEncoder,
        component: VvcResidualComponent,
        node: VvcCodingTreeNode,
        dc_level: i16,
        ac_levels: &[i16; VVC_CHROMA_AC_COEFFS_PER_TU],
        has_ac: bool,
        transform_skip: bool,
        bdpcm: bool,
    ) {
        let width = usize::from(vvc_chroma_width(node, chroma_sampling));
        let height = usize::from(vvc_chroma_height(node, chroma_sampling));
        let log2_width = (width as u16).ilog2() as u8;
        let log2_height = (height as u16).ilog2() as u8;
        let mut residual = VvcResidualCabacEncoder::new(contexts, slice_config.residual_options());
        VvcResidualCabacSymbolStream::emit_chroma_stored_coefficients(
            component,
            log2_width,
            log2_height,
            dc_level,
            ac_levels,
            has_ac,
            transform_skip,
            bdpcm,
            &mut residual,
            cabac,
        );
    }

    fn emit_chroma_transform_only_leaf(
        &mut self,
        cabac: &mut VvcCabacEncoder,
        node: VvcCodingTreeNode,
        split: VvcChromaSplitAvailability,
        cbf_cb_ctx: u8,
        luma_mode_neighbours: &VvcLumaModeNeighbourState,
        neighbours: &mut VvcChromaNeighbourState,
    ) {
        debug_assert_eq!(node.tree_type, VvcTreeType::DualTreeChroma);
        if split.can_no && split.can_split() {
            self.contexts.encode(
                cabac,
                VvcCabacContext::SplitFlag(Self::chroma_split_ctx(node, split, neighbours)),
                false,
            );
        }
        let tu_idx = self.chroma_tu_index;
        assert!(
            tu_idx < self.params.chroma_tu_count,
            "missing chroma TU coefficient data for coding-tree leaf {tu_idx}"
        );
        let chroma_bdpcm_mode = self.params.chroma_tu_bdpcm_modes[tu_idx];
        if !self.emit_chroma_bdpcm_mode(cabac, node, chroma_bdpcm_mode) {
            self.emit_chroma_intra_prediction_mode(cabac, node, tu_idx, luma_mode_neighbours);
        }
        self.chroma_tu_index += 1;
        let cb_dc_level = self.params.cb_tu_dc_levels[tu_idx];
        let cr_dc_level = self.params.cr_tu_dc_levels[tu_idx];
        let cbf_cb = cb_dc_level != 0 || self.params.cb_tu_has_ac[tu_idx];
        let cbf_cr = cr_dc_level != 0 || self.params.cr_tu_has_ac[tu_idx];
        let cbf_cb_ctx = if chroma_bdpcm_mode.is_enabled() {
            1
        } else {
            cbf_cb_ctx
        };
        let cbf_cr_ctx = if chroma_bdpcm_mode.is_enabled() {
            2
        } else {
            u8::from(cbf_cb)
        };
        self.contexts
            .encode(cabac, VvcCabacContext::QtCbfCb(cbf_cb_ctx), cbf_cb);
        self.contexts
            .encode(cabac, VvcCabacContext::QtCbfCr(cbf_cr_ctx), cbf_cr);
        if cbf_cb {
            Self::emit_chroma_residual(
                &mut *self.contexts,
                self.slice_config,
                self.params.chroma_sampling,
                cabac,
                VvcResidualComponent::ChromaCb,
                node,
                cb_dc_level,
                &self.params.cb_tu_ac_levels[tu_idx],
                self.params.cb_tu_has_ac[tu_idx],
                self.params.cb_tu_transform_skip[tu_idx],
                chroma_bdpcm_mode.is_enabled(),
            );
        }
        if cbf_cr {
            Self::emit_chroma_residual(
                &mut *self.contexts,
                self.slice_config,
                self.params.chroma_sampling,
                cabac,
                VvcResidualComponent::ChromaCr,
                node,
                cr_dc_level,
                &self.params.cr_tu_ac_levels[tu_idx],
                self.params.cr_tu_has_ac[tu_idx],
                self.params.cr_tu_transform_skip[tu_idx],
                chroma_bdpcm_mode.is_enabled(),
            );
        }
        neighbours.mark_leaf(node);
    }

    fn emit_chroma_bdpcm_mode(
        &mut self,
        cabac: &mut VvcCabacEncoder,
        node: VvcCodingTreeNode,
        mode: VvcBdpcmMode,
    ) -> bool {
        if !self.chroma_bdpcm_allowed(node) {
            return false;
        }
        self.contexts
            .encode(cabac, VvcCabacContext::BdpcmMode(2), mode.is_enabled());
        if mode.is_enabled() {
            self.contexts.encode(
                cabac,
                VvcCabacContext::BdpcmMode(3),
                matches!(mode, VvcBdpcmMode::Vertical),
            );
        }
        mode.is_enabled()
    }

    fn emit_chroma_intra_prediction_mode(
        &mut self,
        cabac: &mut VvcCabacEncoder,
        node: VvcCodingTreeNode,
        tu_idx: usize,
        luma_mode_neighbours: &VvcLumaModeNeighbourState,
    ) {
        let mode = self.params.chroma_tu_intra_modes[tu_idx];
        if self.chroma_cclm_enabled(node) {
            let cclm_mode = match mode {
                VvcChromaIntraPredictionMode::Cclm(cclm_mode) => Some(cclm_mode),
                _ => None,
            };
            self.contexts
                .encode(cabac, VvcCabacContext::CclmModeFlag, cclm_mode.is_some());
            if let Some(cclm_mode) = cclm_mode {
                let symbol = match cclm_mode {
                    VvcChromaCclmMode::Linear => 0,
                    VvcChromaCclmMode::MdlmLeft => 1,
                    VvcChromaCclmMode::MdlmTop => 2,
                };
                self.contexts
                    .encode(cabac, VvcCabacContext::CclmModeIdx, symbol != 0);
                if symbol > 0 {
                    cabac.encode_bin_ep(symbol == 2);
                }
                return;
            }
        }
        match mode {
            VvcChromaIntraPredictionMode::Derived => {
                self.contexts
                    .encode(cabac, VvcCabacContext::IntraChromaPredMode(0), false);
            }
            VvcChromaIntraPredictionMode::Explicit(mode) => {
                self.contexts
                    .encode(cabac, VvcCabacContext::IntraChromaPredMode(0), true);
                let co_located_luma_mode = luma_mode_neighbours
                    .co_located_for_chroma(node)
                    .unwrap_or(VvcIntraPredictionMode::Dc);
                let candidate_index =
                    vvc_chroma_explicit_candidate_index(mode, co_located_luma_mode)
                        .expect("selected VVC chroma explicit mode must be in the candidate table");
                cabac.encode_bins_ep(u32::from(candidate_index), 2);
            }
            VvcChromaIntraPredictionMode::Cclm(_) => {
                debug_assert!(
                    false,
                    "selected VVC CCLM mode for a node where CCLM is not signaled"
                );
                self.contexts
                    .encode(cabac, VvcCabacContext::IntraChromaPredMode(0), false);
            }
        }
    }

    fn chroma_leaf_allowed(&self, node: VvcCodingTreeNode) -> bool {
        let chroma_width = vvc_chroma_width(node, self.params.chroma_sampling);
        let chroma_height = vvc_chroma_height(node, self.params.chroma_sampling);
        // H.266 7.3.11.10 transform_unit() is reached after the encoder's
        // chosen legal coding-tree split. The spec maximum for this SPS remains
        // MaxTbSizeY/SubWidthC by MaxTbSizeY/SubHeightC, but this hardware
        // residual subset chooses 8x8 luma-coordinate leaves so each 4:2:0
        // chroma TU is 4x4 samples and shares the luma TU cadence.
        chroma_width <= VVC_CURRENT_ENCODER_CHROMA_420_TB_SIZE
            && chroma_height <= VVC_CURRENT_ENCODER_CHROMA_420_TB_SIZE
    }

    fn chroma_cclm_enabled(&self, node: VvcCodingTreeNode) -> bool {
        if !self.slice_config.tools.cclm_enabled {
            return false;
        }
        vvc_chroma_cclm_node_allowed(node)
    }

    fn chroma_bdpcm_allowed(&self, node: VvcCodingTreeNode) -> bool {
        if !self.slice_config.tools.bdpcm_enabled {
            return false;
        }
        let chroma_width = vvc_chroma_width(node, self.params.chroma_sampling);
        let chroma_height = vvc_chroma_height(node, self.params.chroma_sampling);
        chroma_width <= 8 && chroma_height <= 8 && chroma_width >= 4 && chroma_height >= 4
    }

    fn chroma_split_ctx(
        node: VvcCodingTreeNode,
        split: VvcChromaSplitAvailability,
        neighbours: &VvcChromaNeighbourState,
    ) -> u8 {
        // H.266 9.3.4.2.2 Table 133 derives chroma split_cu_flag condL from
        // the left chroma CU height being smaller than the current chroma CU
        // height, and condA from the above chroma CU width being smaller.
        let left = neighbours.left_of(node);
        let above = neighbours.above_of(node);
        VvcSplitCtxInput {
            available_left: left.is_some(),
            available_above: above.is_some(),
            condition_left: left.is_some_and(|info| info.cb_height < neighbours.node_height(node)),
            condition_above: above.is_some_and(|info| info.cb_width < neighbours.node_width(node)),
            allow_bt_vertical: split.allow_bt_vertical,
            allow_bt_horizontal: split.allow_bt_horizontal,
            allow_tt_vertical: split.allow_tt_vertical,
            allow_tt_horizontal: split.allow_tt_horizontal,
            allow_qt: split.allow_qt,
        }
        .split_cu_flag_ctx()
    }

    fn chroma_qt_split_ctx(node: VvcCodingTreeNode, neighbours: &VvcChromaNeighbourState) -> u8 {
        // H.266 9.3.4.2.2 Table 133 derives split_qt_flag condL/condA from
        // neighbouring chroma CqtDepth being greater than the current depth.
        let left = neighbours.left_of(node);
        let above = neighbours.above_of(node);
        VvcQtSplitCtxInput {
            available_left: left.is_some(),
            available_above: above.is_some(),
            left_deeper_qt: left.is_some_and(|info| info.cqt_depth > node.cqt_depth),
            above_deeper_qt: above.is_some_and(|info| info.cqt_depth > node.cqt_depth),
            cqt_depth: node.cqt_depth,
        }
        .split_qt_flag_ctx()
    }

    fn chroma_mtt_vertical_ctx(
        node: VvcCodingTreeNode,
        split: VvcChromaSplitAvailability,
        neighbours: &VvcChromaNeighbourState,
    ) -> u8 {
        // H.266 9.3.4.2.3 first compares vertical-vs-horizontal BT/TT choices.
        // If tied, it uses the above chroma CU width and left chroma CU height.
        let vertical_choices =
            u8::from(split.allow_bt_vertical) + u8::from(split.allow_tt_vertical);
        let horizontal_choices =
            u8::from(split.allow_bt_horizontal) + u8::from(split.allow_tt_horizontal);
        if vertical_choices > horizontal_choices {
            return 4;
        }
        if vertical_choices < horizontal_choices {
            return 3;
        }
        let Some(above) = neighbours.above_of(node) else {
            return 0;
        };
        let Some(left) = neighbours.left_of(node) else {
            return 0;
        };
        let d_a = neighbours.node_width(node) / above.cb_width.max(1);
        let d_l = neighbours.node_height(node) / left.cb_height.max(1);
        if d_a == d_l {
            0
        } else if d_a < d_l {
            1
        } else {
            2
        }
    }

    fn chroma_prefer_vertical_bt(
        node: VvcCodingTreeNode,
        split: VvcChromaSplitAvailability,
    ) -> bool {
        if !split.allow_bt_vertical {
            return false;
        }
        if !split.allow_bt_horizontal {
            return true;
        }
        node.width >= node.height
    }
}

fn vvc_cabac_op_trace_enabled() -> bool {
    static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ENABLED.get_or_init(|| {
        std::env::var_os("FRAMEFORGE_CABAC_OP_TRACE").is_some_and(|value| value != "0")
    })
}
