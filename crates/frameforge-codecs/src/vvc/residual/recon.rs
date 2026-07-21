use crate::picture::{ChromaSampling, PlanarYuvGeometry};

use super::super::{
    chroma_subsample_x, chroma_subsample_y, vvc_chroma_transform_nodes, vvc_luma_transform_nodes,
    vvc_neutral_sample, VvcCodingTreeNode, VvcCtuPartitionParams, VvcSample, VvcSampledFrame,
};
use super::{
    fill_visible_chroma_node, fill_visible_luma_node, inverse_transform_vvc_chroma_residual_levels,
    inverse_transform_vvc_luma_residual_levels, predict_vvc_chroma_dc_block_into,
    predict_vvc_luma_dc_block_into, VvcDcPredictionScratch, VvcQuantizedColor,
    VVC_LUMA_AC_COEFFS_PER_TU,
};

pub(in crate::vvc) fn reconstruct_vvc_residual_frame(
    frame: &VvcSampledFrame,
    quantized: VvcQuantizedColor,
    partition_params: VvcCtuPartitionParams,
) -> Vec<VvcSample> {
    match frame.format.chroma_sampling {
        ChromaSampling::Cs420 | ChromaSampling::Cs422 => {
            reconstruct_vvc_residual_frame_subsampled(frame, quantized, partition_params)
        }
        ChromaSampling::Cs444 => {
            unreachable!("4:4:4 pictures are reconstructed by the palette path for now")
        }
        other => {
            unimplemented!("residual reconstruction is not wired for {other:?}")
        }
    }
}

fn reconstruct_vvc_residual_frame_subsampled(
    frame: &VvcSampledFrame,
    quantized: VvcQuantizedColor,
    partition_params: VvcCtuPartitionParams,
) -> Vec<VvcSample> {
    let layout = PlanarYuvGeometry::for_validated_shape(
        frame.geometry.width,
        frame.geometry.height,
        frame.format.chroma_sampling,
        frame.format.bit_depth,
    );
    let neutral = vvc_neutral_sample(frame.format.bit_depth);
    let mut luma = vec![neutral; layout.luma_samples()];
    let mut tu_idx = 0;
    let mut prediction_scratch = VvcDcPredictionScratch::default();
    let mut predicted_luma = Vec::new();
    for node in vvc_luma_transform_nodes(
        partition_params.shape(),
        partition_params.luma_max_leaf_size,
    ) {
        predict_vvc_luma_dc_block_into(
            &mut predicted_luma,
            &mut prediction_scratch,
            &luma,
            frame.geometry,
            node,
            frame.format.bit_depth,
        );
        let coeff_levels = quantized_luma_coeff_levels(node.width, node.height, quantized, tu_idx);
        let residuals = inverse_transform_vvc_luma_residual_levels(
            node.width,
            node.height,
            &coeff_levels,
            frame.format.bit_depth,
        );
        fill_visible_luma_node(
            &mut luma,
            frame.geometry,
            node,
            &predicted_luma,
            &residuals,
            frame.format.bit_depth,
        );
        tu_idx += 1;
    }

    let chroma_len = layout.chroma_samples();
    let chroma_width = layout.chroma_width();
    let chroma_height = layout.chroma_height();
    let mut cb = vec![neutral; chroma_len];
    let mut cr = vec![neutral; chroma_len];
    let chroma_sampling = frame.format.chroma_sampling;
    let mut predicted_cb = Vec::new();
    let mut predicted_cr = Vec::new();
    for (tu_idx, node) in vvc_chroma_transform_nodes(partition_params.shape())
        .into_iter()
        .enumerate()
    {
        predict_vvc_chroma_dc_block_into(
            &mut predicted_cb,
            &mut prediction_scratch,
            &cb,
            frame.geometry,
            node,
            chroma_sampling,
            frame.format.bit_depth,
        );
        let cb_residuals = inverse_transform_vvc_chroma_residual_levels(
            node.width / chroma_subsample_x(chroma_sampling) as u16,
            node.height / chroma_subsample_y(chroma_sampling) as u16,
            &quantized_chroma_coeff_levels(
                node,
                chroma_sampling,
                quantized.cb_tu_dc_levels[tu_idx],
                quantized.cb_tu_ac_levels[tu_idx],
            ),
            frame.format.bit_depth,
        );
        fill_visible_chroma_node(
            &mut cb,
            frame.geometry,
            node,
            chroma_sampling,
            &predicted_cb,
            &cb_residuals,
            frame.format.bit_depth,
        );
        predict_vvc_chroma_dc_block_into(
            &mut predicted_cr,
            &mut prediction_scratch,
            &cr,
            frame.geometry,
            node,
            chroma_sampling,
            frame.format.bit_depth,
        );
        let cr_residuals = inverse_transform_vvc_chroma_residual_levels(
            node.width / chroma_subsample_x(chroma_sampling) as u16,
            node.height / chroma_subsample_y(chroma_sampling) as u16,
            &quantized_chroma_coeff_levels(
                node,
                chroma_sampling,
                quantized.cr_tu_dc_levels[tu_idx],
                quantized.cr_tu_ac_levels[tu_idx],
            ),
            frame.format.bit_depth,
        );
        fill_visible_chroma_node(
            &mut cr,
            frame.geometry,
            node,
            chroma_sampling,
            &predicted_cr,
            &cr_residuals,
            frame.format.bit_depth,
        );
    }

    let mut out = Vec::with_capacity(layout.luma_samples() + chroma_len * 2);
    out.extend_from_slice(&luma);
    out.extend_from_slice(&cb[..chroma_width * chroma_height]);
    out.extend_from_slice(&cr[..chroma_width * chroma_height]);
    out
}

fn quantized_chroma_coeff_levels(
    node: VvcCodingTreeNode,
    chroma_sampling: ChromaSampling,
    dc_level: i16,
    ac_levels: [i16; super::VVC_CHROMA_AC_COEFFS_PER_TU],
) -> Vec<i16> {
    let width = usize::from(node.width / chroma_subsample_x(chroma_sampling) as u16);
    let height = usize::from(node.height / chroma_subsample_y(chroma_sampling) as u16);
    let mut levels = vec![0; width * height];
    levels[0] = dc_level;
    for (slot, level) in ac_levels.iter().enumerate() {
        let (x, y) = super::VVC_CHROMA_AC_POSITIONS_4X4[slot];
        if x < width && y < height {
            levels[y * width + x] = *level;
        }
    }
    levels
}

fn quantized_luma_coeff_levels(
    width: u16,
    height: u16,
    quantized: VvcQuantizedColor,
    tu_idx: usize,
) -> Vec<i16> {
    let mut levels = vec![0; usize::from(width) * usize::from(height)];
    levels[0] = quantized.luma_tu_dc_levels[tu_idx];
    let ac_levels = quantized.luma_tu_ac_levels[tu_idx];
    for y in 0..usize::from(height).min(4) {
        for x in 0..usize::from(width).min(4) {
            let coeff_index = y * usize::from(width) + x;
            if coeff_index == 0 {
                continue;
            }
            let ac_index = y * 4 + x - 1;
            debug_assert!(ac_index < VVC_LUMA_AC_COEFFS_PER_TU);
            levels[coeff_index] = ac_levels[ac_index];
        }
    }
    levels
}
