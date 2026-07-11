use crate::picture::{ChromaSampling, SampleBitDepth};

use super::super::{
    chroma_subsample_x, chroma_subsample_y, vvc_neutral_sample, VvcCodingTreeNode, VvcSample,
    VvcVideoGeometry,
};

pub(in crate::vvc) fn predict_vvc_luma_dc_block(
    luma: &[VvcSample],
    geometry: VvcVideoGeometry,
    node: VvcCodingTreeNode,
    bit_depth: SampleBitDepth,
) -> Vec<VvcSample> {
    predict_vvc_dc_block(
        luma,
        geometry.width,
        geometry.height,
        usize::from(node.x),
        usize::from(node.y),
        usize::from(node.width),
        usize::from(node.height),
        bit_depth,
    )
}

pub(in crate::vvc) fn predict_vvc_chroma_dc_block(
    chroma: &[VvcSample],
    geometry: VvcVideoGeometry,
    node: VvcCodingTreeNode,
    chroma_sampling: ChromaSampling,
    bit_depth: SampleBitDepth,
) -> Vec<VvcSample> {
    let subsample_x = chroma_subsample_x(chroma_sampling);
    let subsample_y = chroma_subsample_y(chroma_sampling);
    predict_vvc_dc_block(
        chroma,
        geometry.width / subsample_x,
        geometry.height / subsample_y,
        usize::from(node.x) / subsample_x,
        usize::from(node.y) / subsample_y,
        usize::from(node.width) / subsample_x,
        usize::from(node.height) / subsample_y,
        bit_depth,
    )
}

fn predict_vvc_dc_block(
    plane: &[VvcSample],
    plane_width: usize,
    plane_height: usize,
    start_x: usize,
    start_y: usize,
    width: usize,
    height: usize,
    bit_depth: SampleBitDepth,
) -> Vec<VvcSample> {
    let top = top_references(
        plane,
        plane_width,
        plane_height,
        start_x,
        start_y,
        width,
        bit_depth,
    );
    let left = left_references(
        plane,
        plane_width,
        plane_height,
        start_x,
        start_y,
        height,
        bit_depth,
    );
    let dc = dc_prediction_value(&top, &left, width, height);
    let mut prediction = vec![dc; width * height];

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

    prediction
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

fn top_references(
    plane: &[VvcSample],
    plane_width: usize,
    plane_height: usize,
    start_x: usize,
    start_y: usize,
    width: usize,
    bit_depth: SampleBitDepth,
) -> Vec<VvcSample> {
    if start_y > 0 {
        let row = (start_y - 1) * plane_width;
        return (0..width)
            .map(|x| {
                let src_x = (start_x + x).min(plane_width.saturating_sub(1));
                plane[row + src_x]
            })
            .collect();
    }

    let fallback = if start_x > 0 && start_y < plane_height {
        plane[start_y * plane_width + start_x - 1]
    } else {
        vvc_neutral_sample(bit_depth)
    };
    vec![fallback; width]
}

fn left_references(
    plane: &[VvcSample],
    plane_width: usize,
    plane_height: usize,
    start_x: usize,
    start_y: usize,
    height: usize,
    bit_depth: SampleBitDepth,
) -> Vec<VvcSample> {
    if start_x > 0 {
        return (0..height)
            .map(|y| {
                let src_y = (start_y + y).min(plane_height.saturating_sub(1));
                plane[src_y * plane_width + start_x - 1]
            })
            .collect();
    }

    let fallback = if start_y > 0 && start_x < plane_width {
        plane[(start_y - 1) * plane_width + start_x]
    } else {
        vvc_neutral_sample(bit_depth)
    };
    vec![fallback; height]
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
