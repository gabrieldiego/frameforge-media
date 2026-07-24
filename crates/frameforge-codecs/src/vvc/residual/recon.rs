use crate::picture::{ChromaSampling, PlanarYuvGeometry, SampleBitDepth};

use super::super::{
    chroma_subsample_x, chroma_subsample_y, vvc_chroma_transform_nodes, vvc_luma_transform_nodes,
    vvc_neutral_sample, VvcChromaIntraPredictionMode, VvcCodingTreeNode, VvcCtuPartitionParams,
    VvcIntraPredictionMode, VvcSample, VvcSampledFrame, VvcVideoGeometry,
};
use super::quant::{
    reconstruct_vvc_chroma_bdpcm_transform_skip_residuals_into_with_qp,
    reconstruct_vvc_chroma_transform_skip_residuals_into_with_qp,
    reconstruct_vvc_luma_bdpcm_transform_skip_residuals_into_with_qp,
    reconstruct_vvc_luma_transform_skip_residuals_into_with_qp,
};
use super::{
    fill_visible_chroma_node, fill_visible_luma_node,
    inverse_transform_vvc_chroma_quantized_block_into_with_qp,
    inverse_transform_vvc_luma_quantized_block_into_with_qp_and_mts,
    predict_vvc_chroma_bdpcm_block_into_with_availability,
    predict_vvc_chroma_cclm_block_into_with_availability,
    predict_vvc_chroma_intra_block_into_with_availability,
    predict_vvc_luma_bdpcm_block_into_with_availability,
    predict_vvc_luma_intra_block_into_with_mrl_and_availability, VvcDcPredictionScratch,
    VvcInverseTransformScratch, VvcPlaneAvailability, VvcQuantizedColor, MAX_VVC_LUMA_TUS,
    VVC_DEFAULT_LOSSY_CHROMA_QP, VVC_DEFAULT_LOSSY_LUMA_QP,
};

pub(in crate::vvc) fn reconstruct_vvc_residual_frame(
    frame: &VvcSampledFrame,
    quantized: VvcQuantizedColor,
    partition_params: VvcCtuPartitionParams,
) -> Vec<VvcSample> {
    reconstruct_vvc_residual_frame_with_qp(
        frame,
        quantized,
        partition_params,
        VVC_DEFAULT_LOSSY_LUMA_QP,
        VVC_DEFAULT_LOSSY_CHROMA_QP,
    )
}

pub(in crate::vvc) fn reconstruct_vvc_residual_frame_with_qp(
    frame: &VvcSampledFrame,
    quantized: VvcQuantizedColor,
    partition_params: VvcCtuPartitionParams,
    luma_qp: i32,
    chroma_qp: i32,
) -> Vec<VvcSample> {
    match frame.format.chroma_sampling {
        ChromaSampling::Cs420 | ChromaSampling::Cs422 | ChromaSampling::Cs444 => {
            reconstruct_vvc_residual_frame_planar(
                frame,
                quantized,
                partition_params,
                luma_qp,
                chroma_qp,
            )
        }
        other => {
            debug_assert!(false, "residual reconstruction is not wired for {other:?}");
            Vec::new()
        }
    }
}

fn reconstruct_vvc_residual_frame_planar(
    frame: &VvcSampledFrame,
    quantized: VvcQuantizedColor,
    partition_params: VvcCtuPartitionParams,
    luma_qp: i32,
    chroma_qp: i32,
) -> Vec<VvcSample> {
    let layout = PlanarYuvGeometry::for_validated_shape(
        frame.geometry.width,
        frame.geometry.height,
        frame.format.chroma_sampling,
        frame.format.bit_depth,
    );
    let neutral = vvc_neutral_sample(frame.format.bit_depth);
    let mut luma = vec![neutral; layout.luma_samples()];
    let mut luma_available = vec![false; layout.luma_samples()];
    let mut tu_idx = 0;
    let mut prediction_scratch = VvcDcPredictionScratch::default();
    let mut predicted_luma = Vec::new();
    let mut transform_scratch = VvcInverseTransformScratch::default();
    let mut residuals = Vec::new();
    let shape = partition_params.shape();
    let luma_nodes = vvc_luma_transform_nodes(shape, partition_params.luma_max_leaf_size);
    for node in luma_nodes.iter().copied() {
        let luma_bdpcm_mode = quantized.luma_tu_bdpcm_modes[tu_idx];
        if luma_bdpcm_mode.is_enabled() {
            predict_vvc_luma_bdpcm_block_into_with_availability(
                &mut predicted_luma,
                &mut prediction_scratch,
                luma_bdpcm_mode,
                &luma,
                frame.geometry,
                node,
                frame.format.bit_depth,
                Some(VvcPlaneAvailability::new(
                    &luma_available,
                    frame.geometry.width,
                )),
            );
        } else {
            predict_vvc_luma_intra_block_into_with_mrl_and_availability(
                &mut predicted_luma,
                &mut prediction_scratch,
                quantized.luma_tu_intra_modes[tu_idx],
                &luma,
                frame.geometry,
                node,
                frame.format.bit_depth,
                quantized.luma_tu_mrl_index[tu_idx],
                Some(VvcPlaneAvailability::new(
                    &luma_available,
                    frame.geometry.width,
                )),
            );
        }
        if quantized.luma_tu_transform_skip[tu_idx] {
            if luma_bdpcm_mode.is_enabled() {
                reconstruct_vvc_luma_bdpcm_transform_skip_residuals_into_with_qp(
                    &mut residuals,
                    quantized.luma_tu_dc_levels[tu_idx],
                    &quantized.luma_tu_ac_levels[tu_idx],
                    usize::from(node.width),
                    usize::from(node.height),
                    frame.format.bit_depth,
                    luma_qp,
                    luma_bdpcm_mode,
                );
            } else {
                reconstruct_vvc_luma_transform_skip_residuals_into_with_qp(
                    &mut residuals,
                    quantized.luma_tu_dc_levels[tu_idx],
                    &quantized.luma_tu_ac_levels[tu_idx],
                    usize::from(node.width),
                    usize::from(node.height),
                    frame.format.bit_depth,
                    luma_qp,
                );
            }
        } else {
            inverse_transform_vvc_luma_quantized_block_into_with_qp_and_mts(
                &mut residuals,
                &mut transform_scratch,
                node.width,
                node.height,
                quantized.luma_tu_dc_levels[tu_idx],
                &quantized.luma_tu_ac_levels[tu_idx],
                frame.format.bit_depth,
                luma_qp,
                quantized.luma_tu_mts_index[tu_idx],
            );
        }
        fill_visible_luma_node(
            &mut luma,
            frame.geometry,
            node,
            &predicted_luma,
            &residuals,
            frame.format.bit_depth,
        );
        mark_vvc_recon_plane_available(
            &mut luma_available,
            frame.geometry.width,
            frame.geometry.height,
            usize::from(node.x),
            usize::from(node.y),
            usize::from(node.width),
            usize::from(node.height),
        );
        tu_idx += 1;
    }

    let chroma_len = layout.chroma_samples();
    let frame_chroma_width = layout.chroma_width();
    let frame_chroma_height = layout.chroma_height();
    let mut cb = vec![neutral; chroma_len];
    let mut cr = vec![neutral; chroma_len];
    let mut cb_available = vec![false; chroma_len];
    let mut cr_available = vec![false; chroma_len];
    let chroma_sampling = frame.format.chroma_sampling;
    let mut predicted_cb = Vec::new();
    let mut predicted_cr = Vec::new();
    for (tu_idx, node) in vvc_chroma_transform_nodes(shape).into_iter().enumerate() {
        let chroma_mode = quantized.chroma_tu_intra_modes[tu_idx];
        let co_located_luma_mode = vvc_co_located_luma_mode_for_chroma_node(
            &luma_nodes,
            &quantized.luma_tu_intra_modes,
            partition_params.luma_tu_count,
            node,
        );
        predict_vvc_recon_chroma_mode_into(
            &mut predicted_cb,
            &mut prediction_scratch,
            chroma_mode,
            co_located_luma_mode,
            &cb,
            &luma,
            frame.geometry,
            node,
            chroma_sampling,
            frame.format.bit_depth,
            Some(VvcPlaneAvailability::new(&cb_available, frame_chroma_width)),
            Some(VvcPlaneAvailability::new(
                &luma_available,
                frame.geometry.width,
            )),
        );
        let chroma_node_width = node.width / chroma_subsample_x(chroma_sampling) as u16;
        let chroma_node_height = node.height / chroma_subsample_y(chroma_sampling) as u16;
        let chroma_bdpcm_mode = quantized.chroma_tu_bdpcm_modes[tu_idx];
        if chroma_bdpcm_mode.is_enabled() {
            predict_vvc_chroma_bdpcm_block_into_with_availability(
                &mut predicted_cb,
                &mut prediction_scratch,
                chroma_bdpcm_mode,
                &cb,
                frame.geometry,
                node,
                chroma_sampling,
                frame.format.bit_depth,
                Some(VvcPlaneAvailability::new(&cb_available, frame_chroma_width)),
            );
        }
        if quantized.cb_tu_transform_skip[tu_idx] {
            if chroma_bdpcm_mode.is_enabled() {
                reconstruct_vvc_chroma_bdpcm_transform_skip_residuals_into_with_qp(
                    &mut residuals,
                    quantized.cb_tu_dc_levels[tu_idx],
                    &quantized.cb_tu_ac_levels[tu_idx],
                    usize::from(chroma_node_width),
                    usize::from(chroma_node_height),
                    frame.format.bit_depth,
                    chroma_qp,
                    chroma_bdpcm_mode,
                );
            } else {
                reconstruct_vvc_chroma_transform_skip_residuals_into_with_qp(
                    &mut residuals,
                    quantized.cb_tu_dc_levels[tu_idx],
                    &quantized.cb_tu_ac_levels[tu_idx],
                    usize::from(chroma_node_width),
                    usize::from(chroma_node_height),
                    frame.format.bit_depth,
                    chroma_qp,
                );
            }
        } else {
            inverse_transform_vvc_chroma_quantized_block_into_with_qp(
                &mut residuals,
                &mut transform_scratch,
                chroma_node_width,
                chroma_node_height,
                quantized.cb_tu_dc_levels[tu_idx],
                &quantized.cb_tu_ac_levels[tu_idx],
                frame.format.bit_depth,
                chroma_qp,
            );
        }
        fill_visible_chroma_node(
            &mut cb,
            frame.geometry,
            node,
            chroma_sampling,
            &predicted_cb,
            &residuals,
            frame.format.bit_depth,
        );
        if chroma_bdpcm_mode.is_enabled() {
            predict_vvc_chroma_bdpcm_block_into_with_availability(
                &mut predicted_cr,
                &mut prediction_scratch,
                chroma_bdpcm_mode,
                &cr,
                frame.geometry,
                node,
                chroma_sampling,
                frame.format.bit_depth,
                Some(VvcPlaneAvailability::new(&cr_available, frame_chroma_width)),
            );
        } else {
            predict_vvc_recon_chroma_mode_into(
                &mut predicted_cr,
                &mut prediction_scratch,
                chroma_mode,
                co_located_luma_mode,
                &cr,
                &luma,
                frame.geometry,
                node,
                chroma_sampling,
                frame.format.bit_depth,
                Some(VvcPlaneAvailability::new(&cr_available, frame_chroma_width)),
                Some(VvcPlaneAvailability::new(
                    &luma_available,
                    frame.geometry.width,
                )),
            );
        }
        if quantized.cr_tu_transform_skip[tu_idx] {
            if chroma_bdpcm_mode.is_enabled() {
                reconstruct_vvc_chroma_bdpcm_transform_skip_residuals_into_with_qp(
                    &mut residuals,
                    quantized.cr_tu_dc_levels[tu_idx],
                    &quantized.cr_tu_ac_levels[tu_idx],
                    usize::from(chroma_node_width),
                    usize::from(chroma_node_height),
                    frame.format.bit_depth,
                    chroma_qp,
                    chroma_bdpcm_mode,
                );
            } else {
                reconstruct_vvc_chroma_transform_skip_residuals_into_with_qp(
                    &mut residuals,
                    quantized.cr_tu_dc_levels[tu_idx],
                    &quantized.cr_tu_ac_levels[tu_idx],
                    usize::from(chroma_node_width),
                    usize::from(chroma_node_height),
                    frame.format.bit_depth,
                    chroma_qp,
                );
            }
        } else {
            inverse_transform_vvc_chroma_quantized_block_into_with_qp(
                &mut residuals,
                &mut transform_scratch,
                chroma_node_width,
                chroma_node_height,
                quantized.cr_tu_dc_levels[tu_idx],
                &quantized.cr_tu_ac_levels[tu_idx],
                frame.format.bit_depth,
                chroma_qp,
            );
        }
        fill_visible_chroma_node(
            &mut cr,
            frame.geometry,
            node,
            chroma_sampling,
            &predicted_cr,
            &residuals,
            frame.format.bit_depth,
        );
        let subsample_x = chroma_subsample_x(chroma_sampling);
        let subsample_y = chroma_subsample_y(chroma_sampling);
        mark_vvc_recon_plane_available(
            &mut cb_available,
            frame_chroma_width,
            frame_chroma_height,
            usize::from(node.x) / subsample_x,
            usize::from(node.y) / subsample_y,
            usize::from(node.width) / subsample_x,
            usize::from(node.height) / subsample_y,
        );
        mark_vvc_recon_plane_available(
            &mut cr_available,
            frame_chroma_width,
            frame_chroma_height,
            usize::from(node.x) / subsample_x,
            usize::from(node.y) / subsample_y,
            usize::from(node.width) / subsample_x,
            usize::from(node.height) / subsample_y,
        );
    }

    let mut out = Vec::with_capacity(layout.luma_samples() + chroma_len * 2);
    out.extend_from_slice(&luma);
    out.extend_from_slice(&cb[..frame_chroma_width * frame_chroma_height]);
    out.extend_from_slice(&cr[..frame_chroma_width * frame_chroma_height]);
    out
}

fn predict_vvc_recon_chroma_mode_into(
    prediction: &mut Vec<VvcSample>,
    scratch: &mut VvcDcPredictionScratch,
    mode: VvcChromaIntraPredictionMode,
    co_located_luma_mode: VvcIntraPredictionMode,
    chroma: &[VvcSample],
    luma: &[VvcSample],
    geometry: VvcVideoGeometry,
    node: VvcCodingTreeNode,
    chroma_sampling: ChromaSampling,
    bit_depth: SampleBitDepth,
    chroma_availability: Option<VvcPlaneAvailability<'_>>,
    luma_availability: Option<VvcPlaneAvailability<'_>>,
) {
    match mode {
        VvcChromaIntraPredictionMode::Derived => {
            predict_vvc_chroma_intra_block_into_with_availability(
                prediction,
                scratch,
                co_located_luma_mode,
                chroma,
                geometry,
                node,
                chroma_sampling,
                bit_depth,
                chroma_availability,
            );
        }
        VvcChromaIntraPredictionMode::Explicit(mode) => {
            predict_vvc_chroma_intra_block_into_with_availability(
                prediction,
                scratch,
                mode,
                chroma,
                geometry,
                node,
                chroma_sampling,
                bit_depth,
                chroma_availability,
            );
        }
        VvcChromaIntraPredictionMode::Cclm(cclm_mode) => {
            predict_vvc_chroma_cclm_block_into_with_availability(
                prediction,
                cclm_mode,
                chroma,
                luma,
                geometry,
                node,
                chroma_sampling,
                bit_depth,
                chroma_availability,
                luma_availability,
            );
        }
    }
}

fn mark_vvc_recon_plane_available(
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

fn vvc_co_located_luma_mode_for_chroma_node(
    local_luma_nodes: &[VvcCodingTreeNode],
    luma_modes: &[VvcIntraPredictionMode; MAX_VVC_LUMA_TUS],
    luma_tu_count: usize,
    chroma_node: VvcCodingTreeNode,
) -> VvcIntraPredictionMode {
    let ref_x = chroma_node.x.saturating_add(chroma_node.width >> 1);
    let ref_y = chroma_node.y.saturating_add(chroma_node.height >> 1);
    for (idx, luma_node) in local_luma_nodes
        .iter()
        .copied()
        .take(luma_tu_count)
        .enumerate()
    {
        if ref_x >= luma_node.x
            && ref_x < luma_node.x.saturating_add(luma_node.width)
            && ref_y >= luma_node.y
            && ref_y < luma_node.y.saturating_add(luma_node.height)
        {
            return luma_modes[idx];
        }
    }
    VvcIntraPredictionMode::Dc
}
