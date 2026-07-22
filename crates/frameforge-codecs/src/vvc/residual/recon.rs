use crate::picture::{ChromaSampling, PlanarYuvGeometry};

use super::super::{
    chroma_subsample_x, chroma_subsample_y, vvc_chroma_transform_nodes, vvc_luma_transform_nodes,
    vvc_neutral_sample, VvcCtuPartitionParams, VvcSample, VvcSampledFrame,
};
use super::{
    fill_visible_chroma_node, fill_visible_luma_node,
    inverse_transform_vvc_chroma_quantized_block_into,
    inverse_transform_vvc_luma_quantized_block_into, predict_vvc_chroma_dc_block_into,
    predict_vvc_luma_intra_block_into, VvcDcPredictionScratch, VvcInverseTransformScratch,
    VvcQuantizedColor,
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
    let mut transform_scratch = VvcInverseTransformScratch::default();
    let mut residuals = Vec::new();
    for node in vvc_luma_transform_nodes(
        partition_params.shape(),
        partition_params.luma_max_leaf_size,
    ) {
        predict_vvc_luma_intra_block_into(
            &mut predicted_luma,
            &mut prediction_scratch,
            quantized.luma_tu_intra_modes[tu_idx],
            &luma,
            frame.geometry,
            node,
            frame.format.bit_depth,
        );
        inverse_transform_vvc_luma_quantized_block_into(
            &mut residuals,
            &mut transform_scratch,
            node.width,
            node.height,
            quantized.luma_tu_dc_levels[tu_idx],
            &quantized.luma_tu_ac_levels[tu_idx],
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
        let chroma_width = node.width / chroma_subsample_x(chroma_sampling) as u16;
        let chroma_height = node.height / chroma_subsample_y(chroma_sampling) as u16;
        inverse_transform_vvc_chroma_quantized_block_into(
            &mut residuals,
            &mut transform_scratch,
            chroma_width,
            chroma_height,
            quantized.cb_tu_dc_levels[tu_idx],
            &quantized.cb_tu_ac_levels[tu_idx],
            frame.format.bit_depth,
        );
        fill_visible_chroma_node(
            &mut cb,
            frame.geometry,
            node,
            chroma_sampling,
            &predicted_cb,
            &residuals,
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
        inverse_transform_vvc_chroma_quantized_block_into(
            &mut residuals,
            &mut transform_scratch,
            chroma_width,
            chroma_height,
            quantized.cr_tu_dc_levels[tu_idx],
            &quantized.cr_tu_ac_levels[tu_idx],
            frame.format.bit_depth,
        );
        fill_visible_chroma_node(
            &mut cr,
            frame.geometry,
            node,
            chroma_sampling,
            &predicted_cr,
            &residuals,
            frame.format.bit_depth,
        );
    }

    let mut out = Vec::with_capacity(layout.luma_samples() + chroma_len * 2);
    out.extend_from_slice(&luma);
    out.extend_from_slice(&cb[..chroma_width * chroma_height]);
    out.extend_from_slice(&cr[..chroma_width * chroma_height]);
    out
}
