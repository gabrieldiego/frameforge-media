use crate::picture::{ChromaSampling, SampleBitDepth};

use super::super::{
    chroma_subsample_x, chroma_subsample_y, vvc_neutral_sample, VvcCodingTreeNode,
    VvcIntraPredictionMode, VvcSample, VvcVideoGeometry, VVC_CTU_SIZE,
};

const VVC_LUMA_MODE_DIAGONAL: u8 = 34;
const VVC_LUMA_MODE_VERTICAL: u8 = 50;
const VVC_LUMA_MODE_HORIZONTAL: u8 = 18;
const VVC_ANGULAR_REFERENCE_CAPACITY: usize = VVC_CTU_SIZE * 2 + 4;
const VVC_INTRA_ANG_TABLE: [i32; 32] = [
    0, 1, 2, 3, 4, 6, 8, 10, 12, 14, 16, 18, 20, 23, 26, 29, 32, 35, 39, 45, 51, 57, 64, 73, 86,
    102, 128, 171, 256, 341, 512, 1024,
];
const VVC_INTRA_INV_ANG_TABLE: [i32; 32] = [
    0, 16384, 8192, 5461, 4096, 2731, 2048, 1638, 1365, 1170, 1024, 910, 819, 712, 630, 565, 512,
    468, 420, 364, 321, 287, 256, 224, 191, 161, 128, 96, 64, 48, 32, 16,
];
const VVC_CHROMA_422_INTRA_ANGLE_MAPPING_TABLE: [u8; 67] = [
    0, 1, 61, 62, 63, 64, 65, 66, 2, 3, 5, 6, 8, 10, 12, 13, 14, 16, 18, 20, 22, 23, 24, 26, 28,
    30, 31, 33, 34, 35, 36, 37, 38, 39, 40, 41, 41, 42, 43, 43, 44, 44, 45, 45, 46, 47, 48, 48, 49,
    49, 50, 51, 51, 52, 52, 53, 54, 55, 55, 56, 56, 57, 57, 58, 59, 59, 60,
];

pub(in crate::vvc) fn predict_vvc_luma_intra_block_into_with_availability(
    prediction: &mut Vec<VvcSample>,
    scratch: &mut VvcDcPredictionScratch,
    mode: VvcIntraPredictionMode,
    luma: &[VvcSample],
    geometry: VvcVideoGeometry,
    node: VvcCodingTreeNode,
    bit_depth: SampleBitDepth,
    availability: Option<VvcPlaneAvailability<'_>>,
) {
    match mode {
        VvcIntraPredictionMode::Planar => predict_vvc_luma_planar_block_into(
            prediction,
            scratch,
            luma,
            geometry,
            node,
            bit_depth,
            availability,
        ),
        VvcIntraPredictionMode::Dc => predict_vvc_luma_dc_block_into(
            prediction,
            scratch,
            luma,
            geometry,
            node,
            bit_depth,
            availability,
        ),
        VvcIntraPredictionMode::Horizontal
        | VvcIntraPredictionMode::Vertical
        | VvcIntraPredictionMode::Angular(_) => predict_vvc_luma_angular_block_into(
            prediction,
            scratch,
            mode,
            luma,
            geometry,
            node,
            bit_depth,
            availability,
        ),
    }
}

pub(in crate::vvc) fn predict_vvc_luma_dc_block_into(
    prediction: &mut Vec<VvcSample>,
    scratch: &mut VvcDcPredictionScratch,
    luma: &[VvcSample],
    geometry: VvcVideoGeometry,
    node: VvcCodingTreeNode,
    bit_depth: SampleBitDepth,
    availability: Option<VvcPlaneAvailability<'_>>,
) {
    predict_vvc_dc_block_into(
        prediction,
        scratch,
        luma,
        geometry.width,
        geometry.height,
        usize::from(node.x),
        usize::from(node.y),
        usize::from(node.width),
        usize::from(node.height),
        bit_depth,
        availability,
    );
}

pub(in crate::vvc) fn predict_vvc_luma_planar_block_into(
    prediction: &mut Vec<VvcSample>,
    scratch: &mut VvcDcPredictionScratch,
    luma: &[VvcSample],
    geometry: VvcVideoGeometry,
    node: VvcCodingTreeNode,
    bit_depth: SampleBitDepth,
    availability: Option<VvcPlaneAvailability<'_>>,
) {
    predict_vvc_planar_block_into(
        prediction,
        scratch,
        luma,
        geometry.width,
        geometry.height,
        usize::from(node.x),
        usize::from(node.y),
        usize::from(node.width),
        usize::from(node.height),
        bit_depth,
        true,
        availability,
    );
}

fn predict_vvc_luma_angular_block_into(
    prediction: &mut Vec<VvcSample>,
    scratch: &mut VvcDcPredictionScratch,
    mode: VvcIntraPredictionMode,
    luma: &[VvcSample],
    geometry: VvcVideoGeometry,
    node: VvcCodingTreeNode,
    bit_depth: SampleBitDepth,
    availability: Option<VvcPlaneAvailability<'_>>,
) {
    let mode_index = mode.luma_mode_index();
    predict_vvc_angular_block_into(
        prediction,
        scratch,
        luma,
        geometry.width,
        geometry.height,
        usize::from(node.x),
        usize::from(node.y),
        usize::from(node.width),
        usize::from(node.height),
        mode_index,
        bit_depth,
        availability,
    );
}

fn predict_vvc_chroma_dc_block_into_with_availability(
    prediction: &mut Vec<VvcSample>,
    scratch: &mut VvcDcPredictionScratch,
    chroma: &[VvcSample],
    geometry: VvcVideoGeometry,
    node: VvcCodingTreeNode,
    chroma_sampling: ChromaSampling,
    bit_depth: SampleBitDepth,
    availability: Option<VvcPlaneAvailability<'_>>,
) {
    let subsample_x = chroma_subsample_x(chroma_sampling);
    let subsample_y = chroma_subsample_y(chroma_sampling);
    predict_vvc_dc_block_into(
        prediction,
        scratch,
        chroma,
        geometry.width / subsample_x,
        geometry.height / subsample_y,
        usize::from(node.x) / subsample_x,
        usize::from(node.y) / subsample_y,
        usize::from(node.width) / subsample_x,
        usize::from(node.height) / subsample_y,
        bit_depth,
        availability,
    );
}

pub(in crate::vvc) fn predict_vvc_chroma_intra_block_into_with_availability(
    prediction: &mut Vec<VvcSample>,
    scratch: &mut VvcDcPredictionScratch,
    mode: VvcIntraPredictionMode,
    chroma: &[VvcSample],
    geometry: VvcVideoGeometry,
    node: VvcCodingTreeNode,
    chroma_sampling: ChromaSampling,
    bit_depth: SampleBitDepth,
    availability: Option<VvcPlaneAvailability<'_>>,
) {
    match mode {
        VvcIntraPredictionMode::Planar => predict_vvc_chroma_planar_block_into(
            prediction,
            scratch,
            chroma,
            geometry,
            node,
            chroma_sampling,
            bit_depth,
            availability,
        ),
        VvcIntraPredictionMode::Dc => predict_vvc_chroma_dc_block_into_with_availability(
            prediction,
            scratch,
            chroma,
            geometry,
            node,
            chroma_sampling,
            bit_depth,
            availability,
        ),
        VvcIntraPredictionMode::Horizontal
        | VvcIntraPredictionMode::Vertical
        | VvcIntraPredictionMode::Angular(_) => predict_vvc_chroma_angular_block_into(
            prediction,
            scratch,
            mode,
            chroma,
            geometry,
            node,
            chroma_sampling,
            bit_depth,
            availability,
        ),
    }
}

pub(in crate::vvc) fn predict_vvc_chroma_cclm_block_into_with_availability(
    prediction: &mut Vec<VvcSample>,
    chroma: &[VvcSample],
    luma: &[VvcSample],
    geometry: VvcVideoGeometry,
    node: VvcCodingTreeNode,
    chroma_sampling: ChromaSampling,
    bit_depth: SampleBitDepth,
    chroma_availability: Option<VvcPlaneAvailability<'_>>,
    luma_availability: Option<VvcPlaneAvailability<'_>>,
) {
    let subsample_x = chroma_subsample_x(chroma_sampling);
    let subsample_y = chroma_subsample_y(chroma_sampling);
    let chroma_width = usize::from(node.width) / subsample_x;
    let chroma_height = usize::from(node.height) / subsample_y;
    let chroma_x = usize::from(node.x) / subsample_x;
    let chroma_y = usize::from(node.y) / subsample_y;
    let plane_width = geometry.width / subsample_x;
    let plane_height = geometry.height / subsample_y;
    let left_available = cclm_left_template_available(
        chroma_availability,
        plane_width,
        plane_height,
        chroma_x,
        chroma_y,
        chroma_height,
    );
    let above_available = cclm_top_template_available(
        chroma_availability,
        plane_width,
        plane_height,
        chroma_x,
        chroma_y,
        chroma_width,
    );
    let params = derive_vvc_cclm_parameters(
        chroma,
        luma,
        geometry,
        node,
        chroma_sampling,
        bit_depth,
        chroma_availability,
        luma_availability,
        above_available,
        left_available,
    );
    prediction.clear();
    prediction.resize(chroma_width * chroma_height, 0);
    let max_sample = i32::from(bit_depth.max_sample());
    for y in 0..chroma_height {
        for x in 0..chroma_width {
            let luma_sample = cclm_downsample_inner_luma(
                luma,
                geometry,
                node,
                chroma_sampling,
                bit_depth,
                luma_availability,
                x,
                y,
                left_available,
            );
            let predicted = right_shift_i32(params.a * luma_sample, params.shift) + params.b;
            prediction[y * chroma_width + x] = predicted.clamp(0, max_sample) as VvcSample;
        }
    }
}

pub(in crate::vvc) struct VvcDcPredictionScratch {
    top: [VvcSample; VVC_ANGULAR_REFERENCE_CAPACITY],
    left: [VvcSample; VVC_ANGULAR_REFERENCE_CAPACITY],
    top_work: [i32; VVC_CTU_SIZE],
    bottom_delta: [i32; VVC_CTU_SIZE],
}

#[derive(Clone, Copy)]
pub(in crate::vvc) struct VvcPlaneAvailability<'a> {
    samples: &'a [bool],
    stride: usize,
}

impl<'a> VvcPlaneAvailability<'a> {
    pub(in crate::vvc) const fn new(samples: &'a [bool], stride: usize) -> Self {
        Self { samples, stride }
    }

    fn is_available(self, x: usize, y: usize) -> bool {
        self.samples
            .get(y.saturating_mul(self.stride).saturating_add(x))
            .copied()
            .unwrap_or(false)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct VvcCclmParameters {
    a: i32,
    b: i32,
    shift: i32,
}

fn derive_vvc_cclm_parameters(
    chroma: &[VvcSample],
    luma: &[VvcSample],
    geometry: VvcVideoGeometry,
    node: VvcCodingTreeNode,
    chroma_sampling: ChromaSampling,
    bit_depth: SampleBitDepth,
    chroma_availability: Option<VvcPlaneAvailability<'_>>,
    luma_availability: Option<VvcPlaneAvailability<'_>>,
    above_available: bool,
    left_available: bool,
) -> VvcCclmParameters {
    if !above_available && !left_available {
        return VvcCclmParameters {
            a: 0,
            b: i32::from(vvc_neutral_sample(bit_depth)),
            shift: 0,
        };
    }

    let subsample_x = chroma_subsample_x(chroma_sampling);
    let subsample_y = chroma_subsample_y(chroma_sampling);
    let chroma_width = usize::from(node.width) / subsample_x;
    let chroma_height = usize::from(node.height) / subsample_y;
    let actual_top = if above_available { chroma_width } else { 0 };
    let actual_left = if left_available { chroma_height } else { 0 };
    let above_is4 = usize::from(!left_available);
    let left_is4 = usize::from(!above_available);
    let top_start = actual_top >> (2 + above_is4);
    let top_step = (actual_top >> (1 + above_is4)).max(1);
    let left_start = actual_left >> (2 + left_is4);
    let left_step = (actual_left >> (1 + left_is4)).max(1);

    let mut select_luma = [0i32; 4];
    let mut select_chroma = [0i32; 4];
    let mut top_count = 0usize;
    let mut total_count = 0usize;
    if above_available {
        top_count = actual_top.min((1 + above_is4) << 1);
        let mut pos = top_start;
        for idx in 0..top_count {
            select_luma[idx] = cclm_downsample_top_luma(
                luma,
                geometry,
                node,
                chroma_sampling,
                bit_depth,
                luma_availability,
                pos,
                left_available,
            );
            select_chroma[idx] = i32::from(cclm_chroma_sample(
                chroma,
                geometry,
                node,
                chroma_sampling,
                bit_depth,
                chroma_availability,
                pos as isize,
                -1,
            ));
            pos += top_step;
            total_count += 1;
        }
    }
    if left_available {
        let left_count = actual_left.min((1 + left_is4) << 1);
        let mut pos = left_start;
        for idx in 0..left_count {
            let dst = top_count + idx;
            select_luma[dst] = cclm_downsample_left_luma(
                luma,
                geometry,
                node,
                chroma_sampling,
                bit_depth,
                luma_availability,
                pos,
            );
            select_chroma[dst] = i32::from(cclm_chroma_sample(
                chroma,
                geometry,
                node,
                chroma_sampling,
                bit_depth,
                chroma_availability,
                -1,
                pos as isize,
            ));
            pos += left_step;
            total_count += 1;
        }
    }

    if total_count == 2 {
        select_luma[3] = select_luma[0];
        select_chroma[3] = select_chroma[0];
        select_luma[2] = select_luma[1];
        select_chroma[2] = select_chroma[1];
        select_luma[0] = select_luma[1];
        select_chroma[0] = select_chroma[1];
        select_luma[1] = select_luma[3];
        select_chroma[1] = select_chroma[3];
    }

    let mut min_group = [0usize, 2usize];
    let mut max_group = [1usize, 3usize];
    if select_luma[min_group[0]] > select_luma[min_group[1]] {
        min_group.swap(0, 1);
    }
    if select_luma[max_group[0]] > select_luma[max_group[1]] {
        max_group.swap(0, 1);
    }
    if select_luma[min_group[0]] > select_luma[max_group[1]] {
        std::mem::swap(&mut min_group, &mut max_group);
    }
    if select_luma[min_group[1]] > select_luma[max_group[0]] {
        std::mem::swap(&mut min_group[1], &mut max_group[0]);
    }

    let min_luma = (select_luma[min_group[0]] + select_luma[min_group[1]] + 1) >> 1;
    let min_chroma = (select_chroma[min_group[0]] + select_chroma[min_group[1]] + 1) >> 1;
    let max_luma = (select_luma[max_group[0]] + select_luma[max_group[1]] + 1) >> 1;
    let max_chroma = (select_chroma[max_group[0]] + select_chroma[max_group[1]] + 1) >> 1;
    let diff = max_luma - min_luma;
    if diff <= 0 {
        return VvcCclmParameters {
            a: 0,
            b: min_chroma,
            shift: 0,
        };
    }
    let diff_chroma = max_chroma - min_chroma;
    let mut x = floor_log2_i32(diff);
    const DIV_SIG_TABLE: [i32; 16] = [0, 7, 6, 5, 5, 4, 4, 3, 3, 2, 2, 1, 1, 1, 1, 0];
    let norm_diff = ((diff << 4) >> x) & 15;
    let v = DIV_SIG_TABLE[norm_diff as usize] | 8;
    x += i32::from(norm_diff != 0);
    let y = floor_log2_i32(diff_chroma.abs()) + 1;
    let add = 1 << y >> 1;
    let mut a = (diff_chroma * v + add) >> y;
    let mut shift = 3 + x - y;
    if shift < 1 {
        shift = 1;
        a = if a == 0 {
            0
        } else if a < 0 {
            -15
        } else {
            15
        };
    }
    let b = min_chroma - right_shift_i32(a * min_luma, shift);
    VvcCclmParameters { a, b, shift }
}

fn cclm_top_template_available(
    availability: Option<VvcPlaneAvailability<'_>>,
    plane_width: usize,
    _plane_height: usize,
    start_x: usize,
    start_y: usize,
    width: usize,
) -> bool {
    if start_y == 0 || start_x >= plane_width {
        return false;
    }
    let checked_width = width.min(plane_width - start_x);
    checked_width > 0
        && (0..checked_width)
            .all(|x| reference_sample_available(availability, start_x + x, start_y - 1))
}

fn cclm_left_template_available(
    availability: Option<VvcPlaneAvailability<'_>>,
    _plane_width: usize,
    plane_height: usize,
    start_x: usize,
    start_y: usize,
    height: usize,
) -> bool {
    if start_x == 0 || start_y >= plane_height {
        return false;
    }
    let checked_height = height.min(plane_height - start_y);
    checked_height > 0
        && (0..checked_height)
            .all(|y| reference_sample_available(availability, start_x - 1, start_y + y))
}

fn cclm_chroma_sample(
    chroma: &[VvcSample],
    geometry: VvcVideoGeometry,
    node: VvcCodingTreeNode,
    chroma_sampling: ChromaSampling,
    bit_depth: SampleBitDepth,
    availability: Option<VvcPlaneAvailability<'_>>,
    rel_x: isize,
    rel_y: isize,
) -> VvcSample {
    let subsample_x = chroma_subsample_x(chroma_sampling);
    let subsample_y = chroma_subsample_y(chroma_sampling);
    let plane_width = geometry.width / subsample_x;
    let plane_height = geometry.height / subsample_y;
    let start_x = usize::from(node.x) / subsample_x;
    let start_y = usize::from(node.y) / subsample_y;
    let Some((x, y)) =
        clamp_relative_sample_position(start_x, start_y, rel_x, rel_y, plane_width, plane_height)
    else {
        return vvc_neutral_sample(bit_depth);
    };
    if !reference_sample_available(availability, x, y) {
        return vvc_neutral_sample(bit_depth);
    }
    chroma[y * plane_width + x]
}

fn cclm_downsample_inner_luma(
    luma: &[VvcSample],
    geometry: VvcVideoGeometry,
    node: VvcCodingTreeNode,
    chroma_sampling: ChromaSampling,
    bit_depth: SampleBitDepth,
    availability: Option<VvcPlaneAvailability<'_>>,
    rel_x: usize,
    rel_y: usize,
    left_available: bool,
) -> i32 {
    let luma_x = node.x as isize + (rel_x * chroma_subsample_x(chroma_sampling)) as isize;
    let luma_y = node.y as isize + (rel_y * chroma_subsample_y(chroma_sampling)) as isize;
    cclm_downsample_luma_at(
        luma,
        geometry,
        chroma_sampling,
        bit_depth,
        availability,
        luma_x,
        luma_y,
        rel_x == 0 && !left_available,
    )
}

fn cclm_downsample_top_luma(
    luma: &[VvcSample],
    geometry: VvcVideoGeometry,
    node: VvcCodingTreeNode,
    chroma_sampling: ChromaSampling,
    bit_depth: SampleBitDepth,
    availability: Option<VvcPlaneAvailability<'_>>,
    rel_x: usize,
    left_available: bool,
) -> i32 {
    let luma_x = node.x as isize + (rel_x * chroma_subsample_x(chroma_sampling)) as isize;
    match chroma_sampling {
        ChromaSampling::Cs444 => cclm_luma_sample(
            luma,
            geometry,
            luma_x,
            node.y as isize - 1,
            bit_depth,
            availability,
        ),
        ChromaSampling::Cs422 => cclm_downsample_luma_at(
            luma,
            geometry,
            chroma_sampling,
            bit_depth,
            availability,
            luma_x,
            node.y as isize - 1,
            rel_x == 0 && !left_available,
        ),
        ChromaSampling::Cs420 => {
            let first_row_of_ctu = usize::from(node.y) % VVC_CTU_SIZE == 0;
            let luma_y = if first_row_of_ctu {
                node.y as isize - 1
            } else {
                node.y as isize - 2
            };
            cclm_downsample_luma_at(
                luma,
                geometry,
                if first_row_of_ctu {
                    ChromaSampling::Cs422
                } else {
                    ChromaSampling::Cs420
                },
                bit_depth,
                availability,
                luma_x,
                luma_y,
                rel_x == 0 && !left_available,
            )
        }
        ChromaSampling::Monochrome => i32::from(vvc_neutral_sample(bit_depth)),
    }
}

fn cclm_downsample_left_luma(
    luma: &[VvcSample],
    geometry: VvcVideoGeometry,
    node: VvcCodingTreeNode,
    chroma_sampling: ChromaSampling,
    bit_depth: SampleBitDepth,
    availability: Option<VvcPlaneAvailability<'_>>,
    rel_y: usize,
) -> i32 {
    let luma_x = node.x as isize - chroma_subsample_x(chroma_sampling) as isize;
    let luma_y = node.y as isize + (rel_y * chroma_subsample_y(chroma_sampling)) as isize;
    match chroma_sampling {
        ChromaSampling::Cs444 => {
            cclm_luma_sample(luma, geometry, luma_x, luma_y, bit_depth, availability)
        }
        ChromaSampling::Cs422 => {
            let center = cclm_luma_sample(luma, geometry, luma_x, luma_y, bit_depth, availability);
            let left =
                cclm_luma_sample(luma, geometry, luma_x - 1, luma_y, bit_depth, availability);
            let right =
                cclm_luma_sample(luma, geometry, luma_x + 1, luma_y, bit_depth, availability);
            (2 + 2 * center + left + right) >> 2
        }
        ChromaSampling::Cs420 => {
            let top = luma_y;
            let center0 = cclm_luma_sample(luma, geometry, luma_x, top, bit_depth, availability);
            let left0 = cclm_luma_sample(luma, geometry, luma_x - 1, top, bit_depth, availability);
            let right0 = cclm_luma_sample(luma, geometry, luma_x + 1, top, bit_depth, availability);
            let center1 =
                cclm_luma_sample(luma, geometry, luma_x, top + 1, bit_depth, availability);
            let left1 =
                cclm_luma_sample(luma, geometry, luma_x - 1, top + 1, bit_depth, availability);
            let right1 =
                cclm_luma_sample(luma, geometry, luma_x + 1, top + 1, bit_depth, availability);
            (4 + 2 * center0 + left0 + right0 + 2 * center1 + left1 + right1) >> 3
        }
        ChromaSampling::Monochrome => i32::from(vvc_neutral_sample(bit_depth)),
    }
}

fn cclm_downsample_luma_at(
    luma: &[VvcSample],
    geometry: VvcVideoGeometry,
    chroma_sampling: ChromaSampling,
    bit_depth: SampleBitDepth,
    availability: Option<VvcPlaneAvailability<'_>>,
    luma_x: isize,
    luma_y: isize,
    left_padding: bool,
) -> i32 {
    match chroma_sampling {
        ChromaSampling::Cs444 => {
            cclm_luma_sample(luma, geometry, luma_x, luma_y, bit_depth, availability)
        }
        ChromaSampling::Cs422 => {
            let left_x = luma_x - isize::from(!left_padding);
            let center = cclm_luma_sample(luma, geometry, luma_x, luma_y, bit_depth, availability);
            let left = cclm_luma_sample(luma, geometry, left_x, luma_y, bit_depth, availability);
            let right =
                cclm_luma_sample(luma, geometry, luma_x + 1, luma_y, bit_depth, availability);
            (2 + 2 * center + left + right) >> 2
        }
        ChromaSampling::Cs420 => {
            let left_x = luma_x - isize::from(!left_padding);
            let center0 = cclm_luma_sample(luma, geometry, luma_x, luma_y, bit_depth, availability);
            let left0 = cclm_luma_sample(luma, geometry, left_x, luma_y, bit_depth, availability);
            let right0 =
                cclm_luma_sample(luma, geometry, luma_x + 1, luma_y, bit_depth, availability);
            let center1 =
                cclm_luma_sample(luma, geometry, luma_x, luma_y + 1, bit_depth, availability);
            let left1 =
                cclm_luma_sample(luma, geometry, left_x, luma_y + 1, bit_depth, availability);
            let right1 = cclm_luma_sample(
                luma,
                geometry,
                luma_x + 1,
                luma_y + 1,
                bit_depth,
                availability,
            );
            (4 + 2 * center0 + left0 + right0 + 2 * center1 + left1 + right1) >> 3
        }
        ChromaSampling::Monochrome => i32::from(vvc_neutral_sample(bit_depth)),
    }
}

fn cclm_luma_sample(
    luma: &[VvcSample],
    geometry: VvcVideoGeometry,
    x: isize,
    y: isize,
    bit_depth: SampleBitDepth,
    availability: Option<VvcPlaneAvailability<'_>>,
) -> i32 {
    let Some((x, y)) = clamp_sample_position(x, y, geometry.width, geometry.height) else {
        return i32::from(vvc_neutral_sample(bit_depth));
    };
    if !reference_sample_available(availability, x, y) {
        return i32::from(vvc_neutral_sample(bit_depth));
    }
    i32::from(luma[y * geometry.width + x])
}

fn clamp_relative_sample_position(
    start_x: usize,
    start_y: usize,
    rel_x: isize,
    rel_y: isize,
    plane_width: usize,
    plane_height: usize,
) -> Option<(usize, usize)> {
    let x = start_x as isize + rel_x;
    let y = start_y as isize + rel_y;
    clamp_sample_position(x, y, plane_width, plane_height)
}

fn clamp_sample_position(
    x: isize,
    y: isize,
    plane_width: usize,
    plane_height: usize,
) -> Option<(usize, usize)> {
    if plane_width == 0 || plane_height == 0 {
        return None;
    }
    Some((
        x.clamp(0, plane_width.saturating_sub(1) as isize) as usize,
        y.clamp(0, plane_height.saturating_sub(1) as isize) as usize,
    ))
}

fn floor_log2_i32(value: i32) -> i32 {
    if value <= 0 {
        -1
    } else {
        31 - value.leading_zeros() as i32
    }
}

fn right_shift_i32(value: i32, shift: i32) -> i32 {
    if shift >= 0 {
        value >> shift
    } else {
        value << (-shift)
    }
}

fn predict_vvc_chroma_planar_block_into(
    prediction: &mut Vec<VvcSample>,
    scratch: &mut VvcDcPredictionScratch,
    chroma: &[VvcSample],
    geometry: VvcVideoGeometry,
    node: VvcCodingTreeNode,
    chroma_sampling: ChromaSampling,
    bit_depth: SampleBitDepth,
    availability: Option<VvcPlaneAvailability<'_>>,
) {
    let subsample_x = chroma_subsample_x(chroma_sampling);
    let subsample_y = chroma_subsample_y(chroma_sampling);
    predict_vvc_planar_block_into(
        prediction,
        scratch,
        chroma,
        geometry.width / subsample_x,
        geometry.height / subsample_y,
        usize::from(node.x) / subsample_x,
        usize::from(node.y) / subsample_y,
        usize::from(node.width) / subsample_x,
        usize::from(node.height) / subsample_y,
        bit_depth,
        false,
        availability,
    );
}

fn predict_vvc_chroma_angular_block_into(
    prediction: &mut Vec<VvcSample>,
    scratch: &mut VvcDcPredictionScratch,
    mode: VvcIntraPredictionMode,
    chroma: &[VvcSample],
    geometry: VvcVideoGeometry,
    node: VvcCodingTreeNode,
    chroma_sampling: ChromaSampling,
    bit_depth: SampleBitDepth,
    availability: Option<VvcPlaneAvailability<'_>>,
) {
    let subsample_x = chroma_subsample_x(chroma_sampling);
    let subsample_y = chroma_subsample_y(chroma_sampling);
    let mode_index = vvc_chroma_prediction_mode_index(mode, chroma_sampling);
    predict_vvc_angular_block_into(
        prediction,
        scratch,
        chroma,
        geometry.width / subsample_x,
        geometry.height / subsample_y,
        usize::from(node.x) / subsample_x,
        usize::from(node.y) / subsample_y,
        usize::from(node.width) / subsample_x,
        usize::from(node.height) / subsample_y,
        mode_index,
        bit_depth,
        availability,
    );
}

fn vvc_chroma_prediction_mode_index(
    mode: VvcIntraPredictionMode,
    chroma_sampling: ChromaSampling,
) -> u8 {
    let mode_index = mode.luma_mode_index();
    if chroma_sampling == ChromaSampling::Cs422 {
        VVC_CHROMA_422_INTRA_ANGLE_MAPPING_TABLE[usize::from(mode_index)]
    } else {
        mode_index
    }
}

impl Default for VvcDcPredictionScratch {
    fn default() -> Self {
        Self {
            top: [0; VVC_ANGULAR_REFERENCE_CAPACITY],
            left: [0; VVC_ANGULAR_REFERENCE_CAPACITY],
            top_work: [0; VVC_CTU_SIZE],
            bottom_delta: [0; VVC_CTU_SIZE],
        }
    }
}

fn predict_vvc_dc_block_into(
    prediction: &mut Vec<VvcSample>,
    scratch: &mut VvcDcPredictionScratch,
    plane: &[VvcSample],
    plane_width: usize,
    plane_height: usize,
    start_x: usize,
    start_y: usize,
    width: usize,
    height: usize,
    bit_depth: SampleBitDepth,
    availability: Option<VvcPlaneAvailability<'_>>,
) {
    debug_assert!(width <= VVC_CTU_SIZE);
    debug_assert!(height <= VVC_CTU_SIZE);
    top_references_into(
        &mut scratch.top,
        plane,
        plane_width,
        plane_height,
        start_x,
        start_y,
        width,
        bit_depth,
        availability,
    );
    left_references_into(
        &mut scratch.left,
        plane,
        plane_width,
        plane_height,
        start_x,
        start_y,
        height,
        bit_depth,
        availability,
    );
    let top = &scratch.top[..width];
    let left = &scratch.left[..height];
    let dc = dc_prediction_value(top, left, width, height);
    prediction.clear();
    prediction.resize(width * height, dc);

    // VTM IntraPrediction::predIntraAng applies PDPC to DC mode when the
    // luma TU is at least MIN_TB_SIZEY in both dimensions and multiRefIdx is
    // zero. FrameForge currently always signals multiRefIdx = 0.
    if width >= 4 && height >= 4 {
        let scale = ((width.ilog2() as i32 - 2 + height.ilog2() as i32 - 2 + 2) >> 2) as u32;
        let max_sample = i32::from(bit_depth.max_sample());
        for y in 0..height {
            let wt = 32i32 >> ((y << 1) >> scale).min(31);
            let left_sample = i32::from(left[y]);
            for x in 0..width {
                let wl = 32i32 >> ((x << 1) >> scale).min(31);
                let top_sample = i32::from(top[x]);
                let val = i32::from(dc);
                prediction[y * width + x] = (val
                    + ((wl * (left_sample - val) + wt * (top_sample - val) + 32) >> 6))
                    .clamp(0, max_sample) as VvcSample;
            }
        }
    }
}

fn predict_vvc_planar_block_into(
    prediction: &mut Vec<VvcSample>,
    scratch: &mut VvcDcPredictionScratch,
    plane: &[VvcSample],
    plane_width: usize,
    plane_height: usize,
    start_x: usize,
    start_y: usize,
    width: usize,
    height: usize,
    bit_depth: SampleBitDepth,
    filter_luma_references: bool,
    availability: Option<VvcPlaneAvailability<'_>>,
) {
    debug_assert!(width <= VVC_CTU_SIZE);
    debug_assert!(height <= VVC_CTU_SIZE);
    top_references_into(
        &mut scratch.top,
        plane,
        plane_width,
        plane_height,
        start_x,
        start_y,
        width + 2,
        bit_depth,
        availability,
    );
    left_references_into(
        &mut scratch.left,
        plane,
        plane_width,
        plane_height,
        start_x,
        start_y,
        height + 2,
        bit_depth,
        availability,
    );
    if filter_luma_references && width * height > 32 {
        let top_left = top_left_reference(
            plane,
            plane_width,
            plane_height,
            start_x,
            start_y,
            bit_depth,
            availability,
        );
        filter_vvc_planar_references_in_place(
            &mut scratch.top,
            &mut scratch.left,
            top_left,
            width,
            height,
        );
    }
    let log2_w = width.ilog2();
    let log2_h = height.ilog2();
    let offset = 1i32 << (log2_w + log2_h);
    let final_shift = 1 + log2_w + log2_h;
    let bottom_left = i32::from(scratch.left[height]);
    let top_right = i32::from(scratch.top[width]);

    for x in 0..width {
        let top = i32::from(scratch.top[x]);
        scratch.bottom_delta[x] = bottom_left - top;
        scratch.top_work[x] = top << log2_h;
    }

    prediction.clear();
    prediction.resize(width * height, 0);
    for y in 0..height {
        let left = i32::from(scratch.left[y]);
        let right_delta = top_right - left;
        let mut hor_pred = left << log2_w;
        for x in 0..width {
            hor_pred += right_delta;
            scratch.top_work[x] += scratch.bottom_delta[x];
            let vert_pred = scratch.top_work[x];
            prediction[y * width + x] =
                (((hor_pred << log2_h) + (vert_pred << log2_w) + offset) >> final_shift)
                    .clamp(0, i32::from(bit_depth.max_sample())) as VvcSample;
        }
    }

    if width >= 4 && height >= 4 {
        apply_vvc_planar_dc_pdpc(
            prediction,
            &scratch.top[..width],
            &scratch.left[..height],
            width,
            height,
            bit_depth,
        );
    }
}

fn predict_vvc_angular_block_into(
    prediction: &mut Vec<VvcSample>,
    scratch: &mut VvcDcPredictionScratch,
    plane: &[VvcSample],
    plane_width: usize,
    plane_height: usize,
    start_x: usize,
    start_y: usize,
    width: usize,
    height: usize,
    mode_index: u8,
    bit_depth: SampleBitDepth,
    availability: Option<VvcPlaneAvailability<'_>>,
) {
    debug_assert!(width <= VVC_CTU_SIZE);
    debug_assert!(height <= VVC_CTU_SIZE);
    debug_assert!((2..=66).contains(&mode_index));
    let reference_len = width + height + 4;
    top_references_into(
        &mut scratch.top,
        plane,
        plane_width,
        plane_height,
        start_x,
        start_y,
        reference_len,
        bit_depth,
        availability,
    );
    left_references_into(
        &mut scratch.left,
        plane,
        plane_width,
        plane_height,
        start_x,
        start_y,
        reference_len,
        bit_depth,
        availability,
    );
    let top_left = top_left_reference(
        plane,
        plane_width,
        plane_height,
        start_x,
        start_y,
        bit_depth,
        availability,
    );
    let is_vertical_mode = mode_index >= VVC_LUMA_MODE_DIAGONAL;
    let intra_pred_angle_mode = if is_vertical_mode {
        i32::from(mode_index) - i32::from(VVC_LUMA_MODE_VERTICAL)
    } else {
        i32::from(VVC_LUMA_MODE_HORIZONTAL) - i32::from(mode_index)
    };
    let abs_ang_mode = intra_pred_angle_mode.unsigned_abs() as usize;
    let abs_ang = VVC_INTRA_ANG_TABLE[abs_ang_mode];
    let angle = if intra_pred_angle_mode < 0 {
        -abs_ang
    } else {
        abs_ang
    };
    let abs_inv_angle = VVC_INTRA_INV_ANG_TABLE[abs_ang_mode];

    prediction.clear();
    prediction.resize(width * height, 0);
    if is_vertical_mode {
        predict_vvc_vertical_oriented_angular_block(
            prediction,
            &scratch.top[..reference_len],
            &scratch.left[..reference_len],
            top_left,
            width,
            height,
            angle,
            abs_inv_angle,
            bit_depth,
        );
    } else {
        predict_vvc_horizontal_oriented_angular_block(
            prediction,
            &scratch.left[..reference_len],
            &scratch.top[..reference_len],
            top_left,
            width,
            height,
            angle,
            abs_inv_angle,
            bit_depth,
        );
    }
}

fn predict_vvc_vertical_oriented_angular_block(
    prediction: &mut [VvcSample],
    main: &[VvcSample],
    side: &[VvcSample],
    top_left: VvcSample,
    width: usize,
    height: usize,
    angle: i32,
    abs_inv_angle: i32,
    bit_depth: SampleBitDepth,
) {
    for y in 0..height {
        let delta_pos = angle * (y as i32 + 1);
        let delta_int = delta_pos >> 5;
        let delta_fract = delta_pos & 31;
        for x in 0..width {
            prediction[y * width + x] = angular_reference_prediction(
                main,
                side,
                top_left,
                delta_int + x as i32 + 1,
                delta_fract,
                abs_inv_angle,
            );
        }
        apply_vvc_angular_pdpc_to_vertical_row(
            &mut prediction[y * width..(y + 1) * width],
            side,
            top_left,
            y,
            width,
            height,
            angle,
            abs_inv_angle,
            bit_depth,
        );
    }
}

fn predict_vvc_horizontal_oriented_angular_block(
    prediction: &mut [VvcSample],
    main: &[VvcSample],
    side: &[VvcSample],
    top_left: VvcSample,
    width: usize,
    height: usize,
    angle: i32,
    abs_inv_angle: i32,
    bit_depth: SampleBitDepth,
) {
    for x in 0..width {
        let delta_pos = angle * (x as i32 + 1);
        let delta_int = delta_pos >> 5;
        let delta_fract = delta_pos & 31;
        for y in 0..height {
            prediction[y * width + x] = angular_reference_prediction(
                main,
                side,
                top_left,
                delta_int + y as i32 + 1,
                delta_fract,
                abs_inv_angle,
            );
        }
        apply_vvc_angular_pdpc_to_horizontal_column(
            prediction,
            side,
            top_left,
            x,
            width,
            height,
            angle,
            abs_inv_angle,
            bit_depth,
        );
    }
}

fn angular_reference_prediction(
    main: &[VvcSample],
    side: &[VvcSample],
    top_left: VvcSample,
    reference_index: i32,
    fract: i32,
    abs_inv_angle: i32,
) -> VvcSample {
    if fract == 0 {
        return angular_reference_sample(main, side, top_left, reference_index, abs_inv_angle);
    }
    let a = i32::from(angular_reference_sample(
        main,
        side,
        top_left,
        reference_index,
        abs_inv_angle,
    ));
    let b = i32::from(angular_reference_sample(
        main,
        side,
        top_left,
        reference_index + 1,
        abs_inv_angle,
    ));
    (a + ((fract * (b - a) + 16) >> 5)) as VvcSample
}

fn angular_reference_sample(
    main: &[VvcSample],
    side: &[VvcSample],
    top_left: VvcSample,
    index: i32,
    abs_inv_angle: i32,
) -> VvcSample {
    if index == 0 {
        return top_left;
    }
    if index > 0 {
        return main[(index as usize - 1).min(main.len().saturating_sub(1))];
    }
    let side_index = ((-index * abs_inv_angle + 256) >> 9).max(0) as usize;
    if side_index == 0 {
        top_left
    } else {
        side[(side_index - 1).min(side.len().saturating_sub(1))]
    }
}

fn apply_vvc_planar_dc_pdpc(
    prediction: &mut [VvcSample],
    top: &[VvcSample],
    left: &[VvcSample],
    width: usize,
    height: usize,
    bit_depth: SampleBitDepth,
) {
    let scale = ((width.ilog2() as i32 - 2 + height.ilog2() as i32 - 2 + 2) >> 2) as u32;
    let max_sample = i32::from(bit_depth.max_sample());
    for y in 0..height {
        let wt = 32i32 >> ((y << 1) >> scale).min(31);
        let left_sample = i32::from(left[y]);
        for x in 0..width {
            let wl = 32i32 >> ((x << 1) >> scale).min(31);
            let top_sample = i32::from(top[x]);
            let val = i32::from(prediction[y * width + x]);
            prediction[y * width + x] = (val
                + ((wl * (left_sample - val) + wt * (top_sample - val) + 32) >> 6))
                .clamp(0, max_sample) as VvcSample;
        }
    }
}

fn apply_vvc_angular_pdpc_to_vertical_row(
    row: &mut [VvcSample],
    side: &[VvcSample],
    top_left: VvcSample,
    y: usize,
    width: usize,
    height: usize,
    angle: i32,
    abs_inv_angle: i32,
    bit_depth: SampleBitDepth,
) {
    let Some(scale) = vvc_angular_pdpc_scale(width, height, angle, abs_inv_angle, height) else {
        return;
    };
    let span = (3usize << scale).min(width);
    let max_sample = i32::from(bit_depth.max_sample());
    let top_left = i32::from(top_left);
    let mut inv_angle_sum = 256;
    for (x, sample) in row.iter_mut().take(span).enumerate() {
        let side_index = if angle == 0 {
            y + 1
        } else {
            inv_angle_sum += abs_inv_angle;
            y + (inv_angle_sum >> 9).max(0) as usize + 1
        };
        let weight = 32i32 >> ((2 * x) >> scale).min(31);
        let val = i32::from(*sample);
        let side_sample = i32::from(angular_side_reference_sample(side, side_index));
        *sample = (val + ((weight * (side_sample - top_left) + 32) >> 6)).clamp(0, max_sample)
            as VvcSample;
    }
}

fn apply_vvc_angular_pdpc_to_horizontal_column(
    prediction: &mut [VvcSample],
    side: &[VvcSample],
    top_left: VvcSample,
    x: usize,
    width: usize,
    height: usize,
    angle: i32,
    abs_inv_angle: i32,
    bit_depth: SampleBitDepth,
) {
    let Some(scale) = vvc_angular_pdpc_scale(width, height, angle, abs_inv_angle, width) else {
        return;
    };
    let span = (3usize << scale).min(height);
    let max_sample = i32::from(bit_depth.max_sample());
    let top_left = i32::from(top_left);
    let mut inv_angle_sum = 256;
    for y in 0..span {
        let side_index = if angle == 0 {
            x + 1
        } else {
            inv_angle_sum += abs_inv_angle;
            x + (inv_angle_sum >> 9).max(0) as usize + 1
        };
        let weight = 32i32 >> ((2 * y) >> scale).min(31);
        let idx = y * width + x;
        let val = i32::from(prediction[idx]);
        let side_sample = i32::from(angular_side_reference_sample(side, side_index));
        prediction[idx] = (val + ((weight * (side_sample - top_left) + 32) >> 6))
            .clamp(0, max_sample) as VvcSample;
    }
}

fn vvc_angular_pdpc_scale(
    width: usize,
    height: usize,
    angle: i32,
    abs_inv_angle: i32,
    side_size: usize,
) -> Option<u32> {
    if width < 4 || height < 4 || angle < 0 {
        return None;
    }
    if angle == 0 {
        return Some(((width.ilog2() + height.ilog2() - 2) >> 2).min(31));
    }
    let scale = (side_size.ilog2() as i32 - ((3 * abs_inv_angle - 2).ilog2() as i32 - 8)).min(2);
    (scale >= 0).then_some(scale as u32)
}

fn angular_side_reference_sample(side: &[VvcSample], index: usize) -> VvcSample {
    if index == 0 {
        return side[0];
    }
    side[(index - 1).min(side.len().saturating_sub(1))]
}

pub(in crate::vvc) fn fill_visible_luma_node(
    luma: &mut [VvcSample],
    geometry: VvcVideoGeometry,
    node: VvcCodingTreeNode,
    predicted: &[VvcSample],
    residuals: &[i16],
    bit_depth: SampleBitDepth,
) {
    let node_width = usize::from(node.width);
    let start_x = usize::from(node.x);
    let start_y = usize::from(node.y);
    let end_x = (start_x + node_width).min(geometry.width);
    let end_y = (start_y + usize::from(node.height)).min(geometry.height);
    let max_sample = i32::from(bit_depth.max_sample());
    for y in start_y..end_y {
        let row = y * geometry.width;
        let src_y = y - start_y;
        for x in start_x..end_x {
            let src_x = x - start_x;
            let idx = src_y * node_width + src_x;
            luma[row + x] = (i32::from(predicted[idx]) + i32::from(residuals[idx]))
                .clamp(0, max_sample) as VvcSample;
        }
    }
}

pub(in crate::vvc) fn fill_visible_chroma_node(
    chroma: &mut [VvcSample],
    geometry: VvcVideoGeometry,
    node: VvcCodingTreeNode,
    chroma_sampling: ChromaSampling,
    predicted: &[VvcSample],
    residuals: &[i16],
    bit_depth: SampleBitDepth,
) {
    let subsample_x = chroma_subsample_x(chroma_sampling);
    let subsample_y = chroma_subsample_y(chroma_sampling);
    let node_width = usize::from(node.width) / subsample_x;
    let node_height = usize::from(node.height) / subsample_y;
    let start_x = usize::from(node.x) / subsample_x;
    let start_y = usize::from(node.y) / subsample_y;
    let chroma_width = geometry.width / subsample_x;
    let chroma_height = geometry.height / subsample_y;
    let end_x = (start_x + node_width).min(chroma_width);
    let end_y = (start_y + node_height).min(chroma_height);
    let max_sample = i32::from(bit_depth.max_sample());
    for y in start_y..end_y {
        let row = y * chroma_width;
        let src_y = y - start_y;
        for x in start_x..end_x {
            let src_x = x - start_x;
            let idx = src_y * node_width + src_x;
            chroma[row + x] = (i32::from(predicted[idx]) + i32::from(residuals[idx]))
                .clamp(0, max_sample) as VvcSample;
        }
    }
}

fn top_references_into(
    out: &mut [VvcSample],
    plane: &[VvcSample],
    plane_width: usize,
    plane_height: usize,
    start_x: usize,
    start_y: usize,
    width: usize,
    bit_depth: SampleBitDepth,
    availability: Option<VvcPlaneAvailability<'_>>,
) {
    debug_assert!(out.len() >= width);
    let fallback = if start_x > 0
        && start_y < plane_height
        && reference_sample_available(availability, start_x - 1, start_y)
    {
        plane[start_y * plane_width + start_x - 1]
    } else {
        vvc_neutral_sample(bit_depth)
    };
    if start_y == 0 {
        out[..width].fill(fallback);
        return;
    }

    let row_y = start_y - 1;
    let mut first_available = None;
    let mut last_sample = fallback;
    for (x, dst) in out.iter_mut().take(width).enumerate() {
        let src_x = (start_x + x).min(plane_width.saturating_sub(1));
        if reference_sample_available(availability, src_x, row_y) {
            last_sample = plane[row_y * plane_width + src_x];
            if first_available.is_none() {
                first_available = Some((x, last_sample));
            }
        }
        *dst = last_sample;
    }
    if let Some((first_index, first_sample)) = first_available {
        out[..first_index].fill(first_sample);
    }
}

fn left_references_into(
    out: &mut [VvcSample],
    plane: &[VvcSample],
    plane_width: usize,
    plane_height: usize,
    start_x: usize,
    start_y: usize,
    height: usize,
    bit_depth: SampleBitDepth,
    availability: Option<VvcPlaneAvailability<'_>>,
) {
    debug_assert!(out.len() >= height);
    let fallback = if start_y > 0
        && start_x < plane_width
        && reference_sample_available(availability, start_x, start_y - 1)
    {
        plane[(start_y - 1) * plane_width + start_x]
    } else {
        vvc_neutral_sample(bit_depth)
    };
    if start_x == 0 {
        out[..height].fill(fallback);
        return;
    }

    let col_x = start_x - 1;
    let mut first_available = None;
    let mut last_sample = fallback;
    for (y, dst) in out.iter_mut().take(height).enumerate() {
        let src_y = (start_y + y).min(plane_height.saturating_sub(1));
        if reference_sample_available(availability, col_x, src_y) {
            last_sample = plane[src_y * plane_width + col_x];
            if first_available.is_none() {
                first_available = Some((y, last_sample));
            }
        }
        *dst = last_sample;
    }
    if let Some((first_index, first_sample)) = first_available {
        out[..first_index].fill(first_sample);
    }
}

fn top_left_reference(
    plane: &[VvcSample],
    plane_width: usize,
    plane_height: usize,
    start_x: usize,
    start_y: usize,
    bit_depth: SampleBitDepth,
    availability: Option<VvcPlaneAvailability<'_>>,
) -> VvcSample {
    if start_x > 0
        && start_y > 0
        && reference_sample_available(availability, start_x - 1, start_y - 1)
    {
        return plane[(start_y - 1) * plane_width + start_x - 1];
    }
    if start_y > 0
        && start_x < plane_width
        && reference_sample_available(availability, start_x, start_y - 1)
    {
        return plane[(start_y - 1) * plane_width + start_x];
    }
    if start_x > 0
        && start_y < plane_height
        && reference_sample_available(availability, start_x - 1, start_y)
    {
        return plane[start_y * plane_width + start_x - 1];
    }
    vvc_neutral_sample(bit_depth)
}

fn reference_sample_available(
    availability: Option<VvcPlaneAvailability<'_>>,
    x: usize,
    y: usize,
) -> bool {
    availability.map_or(true, |availability| availability.is_available(x, y))
}

fn filter_vvc_planar_references_in_place(
    top: &mut [VvcSample],
    left: &mut [VvcSample],
    top_left: VvcSample,
    width: usize,
    height: usize,
) {
    let mut previous = top_left;
    for index in 0..=width {
        let current = top[index];
        top[index] =
            ((u32::from(previous) + u32::from(current) * 2 + u32::from(top[index + 1]) + 2) >> 2)
                as VvcSample;
        previous = current;
    }

    previous = top_left;
    for index in 0..=height {
        let current = left[index];
        left[index] =
            ((u32::from(previous) + u32::from(current) * 2 + u32::from(left[index + 1]) + 2) >> 2)
                as VvcSample;
        previous = current;
    }
}

fn dc_prediction_value(
    top: &[VvcSample],
    left: &[VvcSample],
    width: usize,
    height: usize,
) -> VvcSample {
    let mut sum = 0u64;
    if width >= height {
        sum += top.iter().map(|sample| u64::from(*sample)).sum::<u64>();
    }
    if width <= height {
        sum += left.iter().map(|sample| u64::from(*sample)).sum::<u64>();
    }
    let denom = if width == height {
        width << 1
    } else {
        width.max(height)
    } as u64;
    ((sum + (denom >> 1)) >> denom.ilog2()) as VvcSample
}
