use super::super::{
    chroma_subsample_x, chroma_subsample_y, select_vvc_residual_chroma_intra_mode_from_costs,
    select_vvc_residual_luma_intra_mode, vvc_chroma_explicit_candidates,
    vvc_chroma_transform_nodes, vvc_downshift_sample_to_u8, vvc_luma_transform_nodes,
    vvc_neutral_sample, vvc_residual_chroma_explicit_candidate_allowed,
    vvc_residual_luma_directional_candidate_allowed, vvc_residual_luma_planar_candidate_allowed,
    VvcChromaIntraCandidateCosts, VvcChromaIntraPredictionMode, VvcCodingTreeNode,
    VvcCtuPartitionShape, VvcCtuRegion, VvcIntraPredictionMode, VvcLumaIntraCandidateCosts,
    VvcPictureFormat, VvcReconstructionFrame, VvcResidualCodingMode,
    VvcResidualModeDecisionContext, VvcSample, VvcSampledColor, VvcSampledFrame, VvcVideoGeometry,
    VVC_CTU_SIZE, VVC_LOSSY_LUMA_ANGULAR_CANDIDATES,
};
use super::{
    fill_visible_chroma_node, fill_visible_luma_node,
    inverse_transform_vvc_chroma_quantized_block_into_with_qp,
    inverse_transform_vvc_luma_quantized_block_into_with_qp,
    predict_vvc_chroma_intra_block_into_with_availability,
    predict_vvc_luma_intra_block_into_with_availability,
    quantize_vvc_chroma_residual_greedy_with_qp, quantize_vvc_chroma_sample,
    quantize_vvc_luma_residual_greedy_with_qp, reconstruct_vvc_chroma, VvcDcPredictionScratch,
    VvcInverseTransformScratch, VvcQuantizedColor, VvcQuantizedResidualFrame, MAX_VVC_CHROMA_TUS,
    MAX_VVC_LUMA_TUS, VVC_CHROMA_AC_COEFFS_PER_TU, VVC_CHROMA_AC_POSITIONS_4X4,
    VVC_LUMA_AC_COEFFS_PER_TU,
};

pub fn quantize_vvc_color(color: VvcSampledColor) -> VvcQuantizedColor {
    quantize_vvc_frame(&VvcSampledFrame::solid(color))
}

pub(in crate::vvc) fn quantize_vvc_frame(frame: &VvcSampledFrame) -> VvcQuantizedColor {
    quantize_vvc_frame_with_reconstruction(frame).quantized
}

pub(in crate::vvc) fn quantize_vvc_frame_with_reconstruction(
    frame: &VvcSampledFrame,
) -> VvcQuantizedResidualFrame {
    let mut reconstruction = VvcReconstructionFrame::new_neutral(frame.geometry, frame.format);
    let region = VvcCtuRegion {
        slice_address: 0,
        origin_x: 0,
        origin_y: 0,
        geometry: frame.geometry,
    };
    let quantized = quantize_vvc_residual_ctu_into_frame_reconstruction(
        frame,
        &mut reconstruction,
        region,
        VvcResidualCodingMode::Lossy,
    );
    let mut reconstruction_yuv =
        Vec::with_capacity(frame.geometry.luma_samples() + frame.chroma_len * 2);
    reconstruction_yuv.extend_from_slice(&reconstruction.luma);
    reconstruction_yuv.extend_from_slice(&reconstruction.cb);
    reconstruction_yuv.extend_from_slice(&reconstruction.cr);
    VvcQuantizedResidualFrame {
        quantized,
        reconstruction_yuv,
    }
}

pub(in crate::vvc) fn quantize_vvc_residual_ctu_into_frame_reconstruction(
    source_frame: &VvcSampledFrame,
    frame_recon: &mut VvcReconstructionFrame,
    region: VvcCtuRegion,
    residual_mode: VvcResidualCodingMode,
) -> VvcQuantizedColor {
    quantize_vvc_residual_ctu_into_frame_reconstruction_with_qp(
        source_frame,
        frame_recon,
        region,
        residual_mode,
        super::VVC_DEFAULT_LOSSY_LUMA_QP,
        super::VVC_DEFAULT_LOSSY_CHROMA_QP,
    )
}

pub(in crate::vvc) fn quantize_vvc_residual_ctu_into_frame_reconstruction_with_qp(
    source_frame: &VvcSampledFrame,
    frame_recon: &mut VvcReconstructionFrame,
    region: VvcCtuRegion,
    residual_mode: VvcResidualCodingMode,
    luma_qp: i32,
    chroma_qp: i32,
) -> VvcQuantizedColor {
    let mut luma_tu_remainders = [0; MAX_VVC_LUMA_TUS];
    let mut luma_tu_negative = [false; MAX_VVC_LUMA_TUS];
    let mut luma_tu_dc_levels = [0; MAX_VVC_LUMA_TUS];
    let mut luma_tu_intra_modes = [VvcIntraPredictionMode::Dc; MAX_VVC_LUMA_TUS];
    let mut luma_tu_ac_levels = [[0; VVC_LUMA_AC_COEFFS_PER_TU]; MAX_VVC_LUMA_TUS];
    let mut luma_tu_has_ac = [false; MAX_VVC_LUMA_TUS];
    let mut cb_tu_dc_levels = [0; MAX_VVC_CHROMA_TUS];
    let mut cr_tu_dc_levels = [0; MAX_VVC_CHROMA_TUS];
    let mut cb_tu_ac_levels = [[0; VVC_CHROMA_AC_COEFFS_PER_TU]; MAX_VVC_CHROMA_TUS];
    let mut cr_tu_ac_levels = [[0; VVC_CHROMA_AC_COEFFS_PER_TU]; MAX_VVC_CHROMA_TUS];
    let mut cb_tu_has_ac = [false; MAX_VVC_CHROMA_TUS];
    let mut cr_tu_has_ac = [false; MAX_VVC_CHROMA_TUS];
    let mut chroma_tu_intra_modes = [VvcChromaIntraPredictionMode::Derived; MAX_VVC_CHROMA_TUS];
    let mut prediction_scratch = VvcDcPredictionScratch::default();
    let mut predicted_luma = Vec::new();
    let mut predicted_cb = Vec::new();
    let mut predicted_cr = Vec::new();
    let mut transform_scratch = VvcInverseTransformScratch::default();
    let mut reconstructed_residual = Vec::new();
    let mut luma_residuals = Vec::new();
    let mut candidate_luma_prediction = Vec::new();
    let mut candidate_luma_residuals = Vec::new();
    let mut cb_residuals = Vec::new();
    let mut cr_residuals = Vec::new();
    let mut candidate_cb_prediction = Vec::new();
    let mut candidate_cr_prediction = Vec::new();
    let mut candidate_cb_residuals = Vec::new();
    let mut candidate_cr_residuals = Vec::new();

    let mode_context = VvcResidualModeDecisionContext::new(source_frame.format, residual_mode);
    let lossless_residual = residual_mode.is_lossless();
    let luma_max_leaf_size = residual_mode.luma_max_leaf_size();
    let ctu_shape = VvcCtuPartitionShape {
        root_width: VVC_CTU_SIZE as u16,
        root_height: VVC_CTU_SIZE as u16,
        visible_width: region.geometry.coded_width() as u16,
        visible_height: region.geometry.coded_height() as u16,
        chroma_sampling: source_frame.format.chroma_sampling,
        dual_tree_intra: true,
    };

    let mut luma_tu_count = 0usize;
    let luma_nodes = vvc_luma_transform_nodes(ctu_shape, luma_max_leaf_size);
    for local_node in luma_nodes.iter().copied() {
        if luma_tu_count >= MAX_VVC_LUMA_TUS {
            break;
        }
        let node = vvc_global_ctu_node(local_node, region);
        predict_vvc_luma_intra_block_into_with_availability(
            &mut predicted_luma,
            &mut prediction_scratch,
            VvcIntraPredictionMode::Dc,
            &frame_recon.luma,
            source_frame.geometry,
            node,
            source_frame.format.bit_depth,
            Some(frame_recon.luma_availability()),
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
        let dc_sad = residual_sad(&luma_residuals);
        let mut best_luma_mode = VvcIntraPredictionMode::Dc;
        let mut best_luma_sad = dc_sad;
        let mut luma_candidate_costs = VvcLumaIntraCandidateCosts::new(dc_sad);
        if vvc_residual_luma_planar_candidate_allowed(mode_context, node) {
            predict_vvc_luma_intra_block_into_with_availability(
                &mut candidate_luma_prediction,
                &mut prediction_scratch,
                VvcIntraPredictionMode::Planar,
                &frame_recon.luma,
                source_frame.geometry,
                node,
                source_frame.format.bit_depth,
                Some(frame_recon.luma_availability()),
            );
            residual_luma_tu_at_into(
                &mut candidate_luma_residuals,
                source_frame,
                usize::from(node.x),
                usize::from(node.y),
                usize::from(node.width),
                usize::from(node.height),
                &candidate_luma_prediction,
            );
            let candidate_sad = residual_sad(&candidate_luma_residuals);
            luma_candidate_costs = luma_candidate_costs
                .with_candidate(VvcIntraPredictionMode::Planar, Some(candidate_sad));
            if candidate_sad < best_luma_sad {
                best_luma_sad = candidate_sad;
                best_luma_mode = VvcIntraPredictionMode::Planar;
                std::mem::swap(&mut predicted_luma, &mut candidate_luma_prediction);
                std::mem::swap(&mut luma_residuals, &mut candidate_luma_residuals);
            }
        }
        if vvc_residual_luma_directional_candidate_allowed(mode_context, node) {
            for mode in VVC_LOSSY_LUMA_ANGULAR_CANDIDATES {
                predict_vvc_luma_intra_block_into_with_availability(
                    &mut candidate_luma_prediction,
                    &mut prediction_scratch,
                    mode,
                    &frame_recon.luma,
                    source_frame.geometry,
                    node,
                    source_frame.format.bit_depth,
                    Some(frame_recon.luma_availability()),
                );
                residual_luma_tu_at_into(
                    &mut candidate_luma_residuals,
                    source_frame,
                    usize::from(node.x),
                    usize::from(node.y),
                    usize::from(node.width),
                    usize::from(node.height),
                    &candidate_luma_prediction,
                );
                let candidate_sad = residual_sad(&candidate_luma_residuals);
                luma_candidate_costs =
                    luma_candidate_costs.with_candidate(mode, Some(candidate_sad));
                if candidate_sad < best_luma_sad {
                    best_luma_sad = candidate_sad;
                    best_luma_mode = mode;
                    std::mem::swap(&mut predicted_luma, &mut candidate_luma_prediction);
                    std::mem::swap(&mut luma_residuals, &mut candidate_luma_residuals);
                }
            }
        }
        let luma_mode =
            select_vvc_residual_luma_intra_mode(mode_context, node, luma_candidate_costs);
        debug_assert_eq!(luma_mode, best_luma_mode);
        let _best_luma_sad = best_luma_sad;
        luma_tu_intra_modes[luma_tu_count] = luma_mode;
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
            frame_recon.mark_luma_node_available(node);
        } else {
            let quantized = quantize_vvc_luma_residual_greedy_with_qp(
                &luma_residuals,
                node.width,
                node.height,
                source_frame.format.bit_depth,
                luma_qp,
            );
            luma_tu_remainders[luma_tu_count] = quantized.abs_remainder;
            luma_tu_negative[luma_tu_count] =
                quantized.reconstructed_dc_coeff < 0 && quantized.abs_remainder != 0;
            luma_tu_dc_levels[luma_tu_count] = quantized.reconstructed_dc_coeff;
            luma_tu_ac_levels[luma_tu_count] = quantized.reconstructed_ac_coeffs;
            luma_tu_has_ac[luma_tu_count] = quantized.has_ac;
            inverse_transform_vvc_luma_quantized_block_into_with_qp(
                &mut reconstructed_residual,
                &mut transform_scratch,
                node.width,
                node.height,
                quantized.reconstructed_dc_coeff,
                &quantized.reconstructed_ac_coeffs,
                source_frame.format.bit_depth,
                luma_qp,
            );
            fill_visible_luma_node(
                &mut frame_recon.luma,
                source_frame.geometry,
                node,
                &predicted_luma,
                &reconstructed_residual,
                source_frame.format.bit_depth,
            );
            frame_recon.mark_luma_node_available(node);
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
        let co_located_luma_mode = vvc_co_located_luma_mode_for_chroma_node(
            &luma_nodes,
            &luma_tu_intra_modes,
            luma_tu_count,
            node,
            region,
        );
        let initial_chroma_mode =
            if lossless_residual && co_located_luma_mode != VvcIntraPredictionMode::Dc {
                VvcChromaIntraPredictionMode::Explicit(VvcIntraPredictionMode::Dc)
            } else {
                VvcChromaIntraPredictionMode::Derived
            };
        let initial_prediction_mode = initial_chroma_mode.prediction_mode(co_located_luma_mode);
        predict_vvc_chroma_intra_block_into_with_availability(
            &mut predicted_cb,
            &mut prediction_scratch,
            initial_prediction_mode,
            &frame_recon.cb,
            source_frame.geometry,
            node,
            source_frame.format.chroma_sampling,
            source_frame.format.bit_depth,
            Some(frame_recon.cb_availability()),
        );
        predict_vvc_chroma_intra_block_into_with_availability(
            &mut predicted_cr,
            &mut prediction_scratch,
            initial_prediction_mode,
            &frame_recon.cr,
            source_frame.geometry,
            node,
            source_frame.format.chroma_sampling,
            source_frame.format.bit_depth,
            Some(frame_recon.cr_availability()),
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
        let initial_sad = residual_sad(&cb_residuals) + residual_sad(&cr_residuals);
        let mut best_chroma_mode = initial_chroma_mode;
        let mut best_chroma_sad = initial_sad;
        let mut chroma_candidate_costs = VvcChromaIntraCandidateCosts::new(initial_sad);
        if !lossless_residual {
            for explicit_mode in vvc_chroma_explicit_candidates(co_located_luma_mode) {
                if !vvc_residual_chroma_explicit_candidate_allowed(explicit_mode) {
                    continue;
                }
                let chroma_mode = VvcChromaIntraPredictionMode::Explicit(explicit_mode);
                predict_vvc_chroma_intra_block_into_with_availability(
                    &mut candidate_cb_prediction,
                    &mut prediction_scratch,
                    explicit_mode,
                    &frame_recon.cb,
                    source_frame.geometry,
                    node,
                    source_frame.format.chroma_sampling,
                    source_frame.format.bit_depth,
                    Some(frame_recon.cb_availability()),
                );
                predict_vvc_chroma_intra_block_into_with_availability(
                    &mut candidate_cr_prediction,
                    &mut prediction_scratch,
                    explicit_mode,
                    &frame_recon.cr,
                    source_frame.geometry,
                    node,
                    source_frame.format.chroma_sampling,
                    source_frame.format.bit_depth,
                    Some(frame_recon.cr_availability()),
                );
                residual_chroma_tu_at_into(
                    &mut candidate_cb_residuals,
                    &source_frame.cb,
                    source_frame.geometry,
                    source_frame.format,
                    chroma_x,
                    chroma_y,
                    chroma_width,
                    chroma_height,
                    &candidate_cb_prediction,
                );
                residual_chroma_tu_at_into(
                    &mut candidate_cr_residuals,
                    &source_frame.cr,
                    source_frame.geometry,
                    source_frame.format,
                    chroma_x,
                    chroma_y,
                    chroma_width,
                    chroma_height,
                    &candidate_cr_prediction,
                );
                let candidate_sad =
                    residual_sad(&candidate_cb_residuals) + residual_sad(&candidate_cr_residuals);
                chroma_candidate_costs =
                    chroma_candidate_costs.with_candidate(chroma_mode, Some(candidate_sad));
                if candidate_sad < best_chroma_sad {
                    best_chroma_sad = candidate_sad;
                    best_chroma_mode = chroma_mode;
                    std::mem::swap(&mut predicted_cb, &mut candidate_cb_prediction);
                    std::mem::swap(&mut predicted_cr, &mut candidate_cr_prediction);
                    std::mem::swap(&mut cb_residuals, &mut candidate_cb_residuals);
                    std::mem::swap(&mut cr_residuals, &mut candidate_cr_residuals);
                }
            }
        }
        let chroma_mode = if lossless_residual {
            best_chroma_mode
        } else {
            select_vvc_residual_chroma_intra_mode_from_costs(
                mode_context,
                node,
                chroma_candidate_costs,
            )
        };
        debug_assert_eq!(chroma_mode, best_chroma_mode);
        let _best_chroma_sad = best_chroma_sad;
        chroma_tu_intra_modes[chroma_tu_count] = chroma_mode;
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
            frame_recon.mark_chroma_node_available(node);
        } else {
            let cb_quantized = quantize_vvc_chroma_residual_greedy_with_qp(
                &cb_residuals,
                chroma_width as u16,
                chroma_height as u16,
                source_frame.format.bit_depth,
                chroma_qp,
            );
            let cr_quantized = quantize_vvc_chroma_residual_greedy_with_qp(
                &cr_residuals,
                chroma_width as u16,
                chroma_height as u16,
                source_frame.format.bit_depth,
                chroma_qp,
            );
            cb_tu_dc_levels[chroma_tu_count] = cb_quantized.reconstructed_dc_coeff;
            cr_tu_dc_levels[chroma_tu_count] = cr_quantized.reconstructed_dc_coeff;
            cb_tu_ac_levels[chroma_tu_count] = cb_quantized.reconstructed_ac_coeffs;
            cr_tu_ac_levels[chroma_tu_count] = cr_quantized.reconstructed_ac_coeffs;
            cb_tu_has_ac[chroma_tu_count] = cb_quantized.has_ac;
            cr_tu_has_ac[chroma_tu_count] = cr_quantized.has_ac;
            inverse_transform_vvc_chroma_quantized_block_into_with_qp(
                &mut reconstructed_residual,
                &mut transform_scratch,
                chroma_width as u16,
                chroma_height as u16,
                cb_quantized.reconstructed_dc_coeff,
                &cb_quantized.reconstructed_ac_coeffs,
                source_frame.format.bit_depth,
                chroma_qp,
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
            inverse_transform_vvc_chroma_quantized_block_into_with_qp(
                &mut reconstructed_residual,
                &mut transform_scratch,
                chroma_width as u16,
                chroma_height as u16,
                cr_quantized.reconstructed_dc_coeff,
                &cr_quantized.reconstructed_ac_coeffs,
                source_frame.format.bit_depth,
                chroma_qp,
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
            frame_recon.mark_chroma_node_available(node);
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
        luma_tu_intra_modes,
        luma_tu_remainders,
        luma_tu_negative,
        luma_tu_dc_levels,
        luma_tu_ac_levels,
        luma_tu_has_ac,
        luma_tu_count,
        chroma_tu_count,
        chroma_tu_intra_modes,
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

fn residual_sad(residuals: &[i16]) -> u64 {
    residuals
        .iter()
        .map(|residual| u64::from(residual.unsigned_abs()))
        .sum()
}

fn vvc_global_ctu_node(mut node: VvcCodingTreeNode, region: VvcCtuRegion) -> VvcCodingTreeNode {
    node.x += region.origin_x as u16;
    node.y += region.origin_y as u16;
    node
}

fn vvc_co_located_luma_mode_for_chroma_node(
    local_luma_nodes: &[VvcCodingTreeNode],
    luma_modes: &[VvcIntraPredictionMode; MAX_VVC_LUMA_TUS],
    luma_tu_count: usize,
    chroma_node: VvcCodingTreeNode,
    region: VvcCtuRegion,
) -> VvcIntraPredictionMode {
    let ref_x = chroma_node
        .x
        .saturating_add(chroma_node.width >> 1)
        .min((region.origin_x + region.geometry.coded_width()).saturating_sub(1) as u16);
    let ref_y = chroma_node
        .y
        .saturating_add(chroma_node.height >> 1)
        .min((region.origin_y + region.geometry.coded_height()).saturating_sub(1) as u16);
    for (idx, local_luma_node) in local_luma_nodes
        .iter()
        .copied()
        .take(luma_tu_count)
        .enumerate()
    {
        let luma_node = vvc_global_ctu_node(local_luma_node, region);
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

pub(in crate::vvc) fn lossless_luma_ac_levels_and_flag(
    residuals: &[i16],
    width: usize,
) -> ([i16; super::VVC_LUMA_AC_COEFFS_PER_TU], bool) {
    let mut levels = [0; super::VVC_LUMA_AC_COEFFS_PER_TU];
    let mut has_ac = false;
    for y in 0..4 {
        for x in 0..4 {
            if x == 0 && y == 0 {
                continue;
            }
            let raster_idx = y * width + x;
            if raster_idx < residuals.len() {
                let level = residuals[raster_idx];
                levels[y * 4 + x - 1] = level;
                has_ac |= level != 0;
            }
        }
    }
    (levels, has_ac)
}

pub(in crate::vvc) fn lossless_chroma_ac_levels_and_flag(
    residuals: &[i16],
    width: usize,
) -> ([i16; VVC_CHROMA_AC_COEFFS_PER_TU], bool) {
    let mut levels = [0; VVC_CHROMA_AC_COEFFS_PER_TU];
    let mut has_ac = false;
    for (slot, (x, y)) in VVC_CHROMA_AC_POSITIONS_4X4.iter().copied().enumerate() {
        let raster_idx = y * width + x;
        if raster_idx < residuals.len() {
            let level = residuals[raster_idx];
            levels[slot] = level;
            has_ac |= level != 0;
        }
    }
    (levels, has_ac)
}

pub(in crate::vvc) fn residual_luma_tu_at_into(
    residuals: &mut Vec<i16>,
    frame: &VvcSampledFrame,
    origin_x: usize,
    origin_y: usize,
    width: usize,
    height: usize,
    predicted: &[VvcSample],
) {
    debug_assert_eq!(predicted.len(), width * height);
    residuals.clear();
    residuals.extend(
        predicted
            .iter()
            .map(|predicted| vvc_sample_delta_i16(0, *predicted)),
    );
    let copy_width = width.min(frame.geometry.width.saturating_sub(origin_x));
    let copy_height = height.min(frame.geometry.height.saturating_sub(origin_y));
    for y in 0..copy_height {
        let src = (origin_y + y) * frame.geometry.width + origin_x;
        let dst = y * width;
        for ((residual, sample), predicted) in residuals[dst..dst + copy_width]
            .iter_mut()
            .zip(&frame.luma[src..src + copy_width])
            .zip(&predicted[dst..dst + copy_width])
        {
            *residual = vvc_sample_delta_i16(*sample, *predicted);
        }
    }
}

pub(in crate::vvc) fn residual_chroma_tu_at_into(
    residuals: &mut Vec<i16>,
    samples: &[VvcSample],
    geometry: VvcVideoGeometry,
    format: VvcPictureFormat,
    origin_x: usize,
    origin_y: usize,
    width: usize,
    height: usize,
    predicted: &[VvcSample],
) {
    debug_assert_eq!(predicted.len(), width * height);
    let chroma_width = geometry.width / chroma_subsample_x(format.chroma_sampling);
    let chroma_height = geometry.height / chroma_subsample_y(format.chroma_sampling);
    let neutral = vvc_neutral_sample(format.bit_depth);
    residuals.clear();
    residuals.extend(
        predicted
            .iter()
            .map(|predicted| vvc_sample_delta_i16(neutral, *predicted)),
    );
    let copy_width = width.min(chroma_width.saturating_sub(origin_x));
    let copy_height = height.min(chroma_height.saturating_sub(origin_y));
    for y in 0..copy_height {
        let src = (origin_y + y) * chroma_width + origin_x;
        let dst = y * width;
        for ((residual, sample), predicted) in residuals[dst..dst + copy_width]
            .iter_mut()
            .zip(&samples[src..src + copy_width])
            .zip(&predicted[dst..dst + copy_width])
        {
            *residual = vvc_sample_delta_i16(*sample, *predicted);
        }
    }
}

fn vvc_sample_delta_i16(sample: VvcSample, predicted: VvcSample) -> i16 {
    (i32::from(sample) - i32::from(predicted)).clamp(i32::from(i16::MIN), i32::from(i16::MAX))
        as i16
}
