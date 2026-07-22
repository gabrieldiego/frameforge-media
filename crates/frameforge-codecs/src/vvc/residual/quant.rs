use crate::picture::{ChromaSampling, SampleBitDepth};

use super::super::{
    chroma_subsample_x, chroma_subsample_y, select_vvc_chroma_mode_syntax_tie_breaker,
    select_vvc_chroma_tu_coding_decision, select_vvc_luma_max_leaf_size,
    select_vvc_luma_tu_coding_decision, select_vvc_residual_chroma_intra_mode_from_costs,
    select_vvc_residual_luma_intra_mode, select_vvc_residual_score_metric,
    vvc_chroma_cclm_node_allowed, vvc_chroma_explicit_candidates,
    vvc_chroma_intra_mode_syntax_bin_count, vvc_chroma_transform_nodes, vvc_downshift_sample_to_u8,
    vvc_luma_intra_mode_from_index, vvc_luma_intra_mode_syntax_bin_count, vvc_luma_transform_nodes,
    vvc_neutral_sample, vvc_residual_chroma_cclm_candidate_allowed,
    vvc_residual_chroma_explicit_candidate_allowed,
    vvc_residual_luma_directional_candidate_allowed, vvc_residual_luma_planar_candidate_allowed,
    VvcChromaCclmMode, VvcChromaIntraCandidateCosts, VvcChromaIntraPredictionMode,
    VvcChromaTuCodingDecision, VvcCodingTreeNode, VvcCtuPartitionShape, VvcCtuRegion,
    VvcIntraPredictionMode, VvcLumaIntraCandidateCosts, VvcLumaTuCodingDecision, VvcPictureFormat,
    VvcReconstructionFrame, VvcResidualCodingMode, VvcResidualModeDecisionContext,
    VvcResidualScoreMetric, VvcSample, VvcSampledColor, VvcSampledFrame, VvcTuResidualCodingMode,
    VvcVideoGeometry, VVC_CTU_SIZE,
};
#[cfg(feature = "vvc-stats")]
use super::VvcIntraSearchStats;
use super::{
    fill_visible_chroma_node, fill_visible_luma_node,
    inverse_transform_vvc_chroma_quantized_block_into_with_qp,
    inverse_transform_vvc_luma_quantized_block_into_with_qp,
    predict_vvc_chroma_cclm_block_into_with_availability,
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
    let mut luma_tu_transform_skip = [false; MAX_VVC_LUMA_TUS];
    let mut luma_tu_mrl_index = [0; MAX_VVC_LUMA_TUS];
    let mut luma_tu_mts_index = [0; MAX_VVC_LUMA_TUS];
    let mut cb_tu_dc_levels = [0; MAX_VVC_CHROMA_TUS];
    let mut cr_tu_dc_levels = [0; MAX_VVC_CHROMA_TUS];
    let mut cb_tu_ac_levels = [[0; VVC_CHROMA_AC_COEFFS_PER_TU]; MAX_VVC_CHROMA_TUS];
    let mut cr_tu_ac_levels = [[0; VVC_CHROMA_AC_COEFFS_PER_TU]; MAX_VVC_CHROMA_TUS];
    let mut cb_tu_has_ac = [false; MAX_VVC_CHROMA_TUS];
    let mut cr_tu_has_ac = [false; MAX_VVC_CHROMA_TUS];
    let mut cb_tu_transform_skip = [false; MAX_VVC_CHROMA_TUS];
    let mut cr_tu_transform_skip = [false; MAX_VVC_CHROMA_TUS];
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
    #[cfg(feature = "vvc-stats")]
    let mut intra_search_stats = VvcIntraSearchStats::default();

    let mode_context = VvcResidualModeDecisionContext::new(source_frame.format, residual_mode);
    let score_metric = select_vvc_residual_score_metric(mode_context);
    let chroma_syntax_tie_breaker = select_vvc_chroma_mode_syntax_tie_breaker(mode_context);
    let luma_max_leaf_size = select_vvc_luma_max_leaf_size(mode_context);
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
    let mut luma_mode_search_state = VvcLumaModeSearchState::new();
    for local_node in luma_nodes.iter().copied() {
        if luma_tu_count >= MAX_VVC_LUMA_TUS {
            break;
        }
        let node = vvc_global_ctu_node(local_node, region);
        let left_luma_mode = luma_mode_search_state.left_of(local_node);
        let above_luma_mode = luma_mode_search_state.above_of(local_node);
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
        let dc_score = luma_mode_selection_score(
            score_metric,
            &luma_residuals,
            left_luma_mode,
            above_luma_mode,
            VvcIntraPredictionMode::Dc,
        );
        let mut best_luma_mode = VvcIntraPredictionMode::Dc;
        let mut best_luma_score = dc_score;
        let mut luma_candidate_costs = VvcLumaIntraCandidateCosts::new(dc_score);
        #[cfg(feature = "vvc-stats")]
        intra_search_stats.add_luma_dc();
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
            let candidate_score = luma_mode_selection_score(
                score_metric,
                &candidate_luma_residuals,
                left_luma_mode,
                above_luma_mode,
                VvcIntraPredictionMode::Planar,
            );
            #[cfg(feature = "vvc-stats")]
            intra_search_stats.add_luma_planar();
            luma_candidate_costs = luma_candidate_costs
                .with_candidate(VvcIntraPredictionMode::Planar, Some(candidate_score));
            if candidate_score < best_luma_score {
                best_luma_score = candidate_score;
                best_luma_mode = VvcIntraPredictionMode::Planar;
                std::mem::swap(&mut predicted_luma, &mut candidate_luma_prediction);
                std::mem::swap(&mut luma_residuals, &mut candidate_luma_residuals);
            }
        }
        if vvc_residual_luma_directional_candidate_allowed(mode_context, node) {
            let mut luma_directional_candidates = vvc_luma_directional_search_candidates(
                source_frame,
                &luma_mode_search_state,
                local_node,
                node,
            );
            for mode in luma_directional_candidates.iter() {
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
                let candidate_score = luma_mode_selection_score(
                    score_metric,
                    &candidate_luma_residuals,
                    left_luma_mode,
                    above_luma_mode,
                    mode,
                );
                #[cfg(feature = "vvc-stats")]
                intra_search_stats.add_luma_directional_coarse();
                luma_candidate_costs =
                    luma_candidate_costs.with_candidate(mode, Some(candidate_score));
                if candidate_score < best_luma_score {
                    best_luma_score = candidate_score;
                    best_luma_mode = mode;
                    std::mem::swap(&mut predicted_luma, &mut candidate_luma_prediction);
                    std::mem::swap(&mut luma_residuals, &mut candidate_luma_residuals);
                }
            }
            if (2..=66).contains(&best_luma_mode.luma_mode_index()) {
                let refinement_start = luma_directional_candidates.count();
                luma_directional_candidates.add_refinement(best_luma_mode.luma_mode_index());
                for mode in luma_directional_candidates.iter_from(refinement_start) {
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
                    let candidate_score = luma_mode_selection_score(
                        score_metric,
                        &candidate_luma_residuals,
                        left_luma_mode,
                        above_luma_mode,
                        mode,
                    );
                    #[cfg(feature = "vvc-stats")]
                    intra_search_stats.add_luma_directional_refinement();
                    luma_candidate_costs =
                        luma_candidate_costs.with_candidate(mode, Some(candidate_score));
                    if candidate_score < best_luma_score {
                        best_luma_score = candidate_score;
                        best_luma_mode = mode;
                        std::mem::swap(&mut predicted_luma, &mut candidate_luma_prediction);
                        std::mem::swap(&mut luma_residuals, &mut candidate_luma_residuals);
                    }
                }
            }
        }
        let luma_mode =
            select_vvc_residual_luma_intra_mode(mode_context, node, luma_candidate_costs);
        debug_assert_eq!(luma_mode, best_luma_mode);
        let _best_luma_score = best_luma_score;
        luma_tu_intra_modes[luma_tu_count] = luma_mode;
        luma_mode_search_state.mark_node(local_node, luma_mode);
        let luma_coding_decision =
            select_vvc_luma_tu_coding_decision(mode_context, node, luma_mode);
        let luma_tu = finalize_vvc_luma_tu(
            luma_coding_decision,
            source_frame,
            frame_recon,
            node,
            &predicted_luma,
            &luma_residuals,
            luma_qp,
            &mut transform_scratch,
            &mut reconstructed_residual,
        );
        luma_tu_remainders[luma_tu_count] = luma_tu.abs_remainder;
        luma_tu_negative[luma_tu_count] = luma_tu.negative;
        luma_tu_dc_levels[luma_tu_count] = luma_tu.dc_level;
        luma_tu_ac_levels[luma_tu_count] = luma_tu.ac_levels;
        luma_tu_has_ac[luma_tu_count] = luma_tu.has_ac;
        luma_tu_transform_skip[luma_tu_count] = luma_tu.transform_skip;
        luma_tu_mrl_index[luma_tu_count] = luma_tu.mrl_index;
        luma_tu_mts_index[luma_tu_count] = luma_tu.mts_index;
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
        let co_located_luma_mode =
            luma_mode_search_state.co_located_mode_for_chroma_node(node, region);
        let cclm_syntax_enabled = vvc_chroma_cclm_node_allowed(node);
        let initial_chroma_mode = VvcChromaIntraPredictionMode::Derived;
        predict_vvc_chroma_mode_block_into_with_availability(
            &mut predicted_cb,
            &mut prediction_scratch,
            initial_chroma_mode,
            co_located_luma_mode,
            &frame_recon.cb,
            &frame_recon.luma,
            source_frame.geometry,
            node,
            source_frame.format.chroma_sampling,
            source_frame.format.bit_depth,
            Some(frame_recon.cb_availability()),
            Some(frame_recon.luma_availability()),
        );
        predict_vvc_chroma_mode_block_into_with_availability(
            &mut predicted_cr,
            &mut prediction_scratch,
            initial_chroma_mode,
            co_located_luma_mode,
            &frame_recon.cr,
            &frame_recon.luma,
            source_frame.geometry,
            node,
            source_frame.format.chroma_sampling,
            source_frame.format.bit_depth,
            Some(frame_recon.cr_availability()),
            Some(frame_recon.luma_availability()),
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
        let initial_score = chroma_mode_selection_score(
            score_metric,
            &cb_residuals,
            &cr_residuals,
            initial_chroma_mode,
            cclm_syntax_enabled,
            chroma_syntax_tie_breaker,
        );
        let mut best_chroma_mode = initial_chroma_mode;
        let mut best_chroma_score = initial_score;
        let mut chroma_candidate_costs = VvcChromaIntraCandidateCosts::new(initial_score);
        #[cfg(feature = "vvc-stats")]
        intra_search_stats.add_chroma_derived();
        for explicit_mode in vvc_chroma_explicit_candidates(co_located_luma_mode) {
            if !vvc_residual_chroma_explicit_candidate_allowed(explicit_mode) {
                continue;
            }
            let chroma_mode = VvcChromaIntraPredictionMode::Explicit(explicit_mode);
            predict_vvc_chroma_mode_block_into_with_availability(
                &mut candidate_cb_prediction,
                &mut prediction_scratch,
                chroma_mode,
                co_located_luma_mode,
                &frame_recon.cb,
                &frame_recon.luma,
                source_frame.geometry,
                node,
                source_frame.format.chroma_sampling,
                source_frame.format.bit_depth,
                Some(frame_recon.cb_availability()),
                Some(frame_recon.luma_availability()),
            );
            predict_vvc_chroma_mode_block_into_with_availability(
                &mut candidate_cr_prediction,
                &mut prediction_scratch,
                chroma_mode,
                co_located_luma_mode,
                &frame_recon.cr,
                &frame_recon.luma,
                source_frame.geometry,
                node,
                source_frame.format.chroma_sampling,
                source_frame.format.bit_depth,
                Some(frame_recon.cr_availability()),
                Some(frame_recon.luma_availability()),
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
            let candidate_score = chroma_mode_selection_score(
                score_metric,
                &candidate_cb_residuals,
                &candidate_cr_residuals,
                chroma_mode,
                cclm_syntax_enabled,
                chroma_syntax_tie_breaker,
            );
            #[cfg(feature = "vvc-stats")]
            intra_search_stats.add_chroma_explicit();
            chroma_candidate_costs =
                chroma_candidate_costs.with_candidate(chroma_mode, Some(candidate_score));
            if candidate_score < best_chroma_score {
                best_chroma_score = candidate_score;
                best_chroma_mode = chroma_mode;
                std::mem::swap(&mut predicted_cb, &mut candidate_cb_prediction);
                std::mem::swap(&mut predicted_cr, &mut candidate_cr_prediction);
                std::mem::swap(&mut cb_residuals, &mut candidate_cb_residuals);
                std::mem::swap(&mut cr_residuals, &mut candidate_cr_residuals);
            }
        }
        if vvc_residual_chroma_cclm_candidate_allowed(mode_context, node, source_frame.geometry) {
            for cclm_mode in [
                VvcChromaCclmMode::Linear,
                VvcChromaCclmMode::MdlmLeft,
                VvcChromaCclmMode::MdlmTop,
            ] {
                let chroma_mode = VvcChromaIntraPredictionMode::Cclm(cclm_mode);
                predict_vvc_chroma_mode_block_into_with_availability(
                    &mut candidate_cb_prediction,
                    &mut prediction_scratch,
                    chroma_mode,
                    co_located_luma_mode,
                    &frame_recon.cb,
                    &frame_recon.luma,
                    source_frame.geometry,
                    node,
                    source_frame.format.chroma_sampling,
                    source_frame.format.bit_depth,
                    Some(frame_recon.cb_availability()),
                    Some(frame_recon.luma_availability()),
                );
                predict_vvc_chroma_mode_block_into_with_availability(
                    &mut candidate_cr_prediction,
                    &mut prediction_scratch,
                    chroma_mode,
                    co_located_luma_mode,
                    &frame_recon.cr,
                    &frame_recon.luma,
                    source_frame.geometry,
                    node,
                    source_frame.format.chroma_sampling,
                    source_frame.format.bit_depth,
                    Some(frame_recon.cr_availability()),
                    Some(frame_recon.luma_availability()),
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
                let candidate_score = chroma_mode_selection_score(
                    score_metric,
                    &candidate_cb_residuals,
                    &candidate_cr_residuals,
                    chroma_mode,
                    cclm_syntax_enabled,
                    chroma_syntax_tie_breaker,
                );
                #[cfg(feature = "vvc-stats")]
                intra_search_stats.add_chroma_cclm();
                chroma_candidate_costs =
                    chroma_candidate_costs.with_candidate(chroma_mode, Some(candidate_score));
                if candidate_score < best_chroma_score {
                    best_chroma_score = candidate_score;
                    best_chroma_mode = chroma_mode;
                    std::mem::swap(&mut predicted_cb, &mut candidate_cb_prediction);
                    std::mem::swap(&mut predicted_cr, &mut candidate_cr_prediction);
                    std::mem::swap(&mut cb_residuals, &mut candidate_cb_residuals);
                    std::mem::swap(&mut cr_residuals, &mut candidate_cr_residuals);
                }
            }
        }
        let chroma_mode = select_vvc_residual_chroma_intra_mode_from_costs(
            mode_context,
            node,
            chroma_candidate_costs,
        );
        debug_assert_eq!(chroma_mode, best_chroma_mode);
        let _best_chroma_score = best_chroma_score;
        chroma_tu_intra_modes[chroma_tu_count] = chroma_mode;
        let chroma_coding_decision =
            select_vvc_chroma_tu_coding_decision(mode_context, node, chroma_mode);
        let chroma_tu = finalize_vvc_chroma_tu(
            chroma_coding_decision,
            source_frame,
            frame_recon,
            node,
            &predicted_cb,
            &predicted_cr,
            &cb_residuals,
            &cr_residuals,
            chroma_width,
            chroma_height,
            chroma_qp,
            &mut transform_scratch,
            &mut reconstructed_residual,
        );
        cb_tu_dc_levels[chroma_tu_count] = chroma_tu.cb_dc_level;
        cr_tu_dc_levels[chroma_tu_count] = chroma_tu.cr_dc_level;
        cb_tu_ac_levels[chroma_tu_count] = chroma_tu.cb_ac_levels;
        cr_tu_ac_levels[chroma_tu_count] = chroma_tu.cr_ac_levels;
        cb_tu_has_ac[chroma_tu_count] = chroma_tu.cb_has_ac;
        cr_tu_has_ac[chroma_tu_count] = chroma_tu.cr_has_ac;
        cb_tu_transform_skip[chroma_tu_count] = chroma_tu.cb_transform_skip;
        cr_tu_transform_skip[chroma_tu_count] = chroma_tu.cr_transform_skip;
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
        u: finalized_vvc_chroma_sample(
            residual_mode,
            color.u,
            cb_rem,
            source_frame.format.bit_depth,
        ),
        v: finalized_vvc_chroma_sample(
            residual_mode,
            color.v,
            cr_rem,
            source_frame.format.bit_depth,
        ),
        luma_tu_intra_modes,
        luma_tu_remainders,
        luma_tu_negative,
        luma_tu_dc_levels,
        luma_tu_ac_levels,
        luma_tu_has_ac,
        luma_tu_transform_skip,
        luma_tu_mrl_index,
        luma_tu_mts_index,
        luma_tu_count,
        chroma_tu_count,
        chroma_tu_intra_modes,
        cb_tu_dc_levels,
        cr_tu_dc_levels,
        cb_tu_ac_levels,
        cr_tu_ac_levels,
        cb_tu_has_ac,
        cr_tu_has_ac,
        cb_tu_transform_skip,
        cr_tu_transform_skip,
        cb_rem,
        cr_rem,
        #[cfg(feature = "vvc-stats")]
        intra_search_stats,
    }
}

fn finalized_vvc_chroma_sample(
    residual_mode: VvcResidualCodingMode,
    source: VvcSample,
    quantized_remainder: u8,
    bit_depth: SampleBitDepth,
) -> u8 {
    match residual_mode {
        VvcResidualCodingMode::Lossless => vvc_downshift_sample_to_u8(source, bit_depth),
        VvcResidualCodingMode::Lossy => reconstruct_vvc_chroma(quantized_remainder),
    }
}

const VVC_LUMA_DIRECTIONAL_SEARCH_CANDIDATE_CAPACITY: usize = 65;
const VVC_LUMA_DEFAULT_DIRECTIONAL_SEEDS: [u8; 9] = [18, 50, 34, 10, 26, 42, 58, 2, 66];
const VVC_LUMA_NEARBY_DIRECTIONAL_OFFSETS: [i16; 7] = [0, -1, 1, -2, 2, -4, 4];

#[derive(Debug, Clone, Copy)]
struct VvcLumaDirectionalSearchCandidates {
    modes: [VvcIntraPredictionMode; VVC_LUMA_DIRECTIONAL_SEARCH_CANDIDATE_CAPACITY],
    count: usize,
}

impl VvcLumaDirectionalSearchCandidates {
    fn new() -> Self {
        Self {
            modes: [VvcIntraPredictionMode::Horizontal;
                VVC_LUMA_DIRECTIONAL_SEARCH_CANDIDATE_CAPACITY],
            count: 0,
        }
    }

    fn add_mode(&mut self, mode: VvcIntraPredictionMode) {
        debug_assert!((2..=66).contains(&mode.luma_mode_index()));
        if self
            .modes
            .iter()
            .take(self.count)
            .any(|candidate| candidate.luma_mode_index() == mode.luma_mode_index())
        {
            return;
        }
        assert!(self.count < self.modes.len());
        self.modes[self.count] = mode;
        self.count += 1;
    }

    fn add_index(&mut self, index: u8) {
        if (2..=66).contains(&index) {
            self.add_mode(vvc_luma_intra_mode_from_index(index));
        }
    }

    fn add_family(&mut self, center: u8) {
        for offset in VVC_LUMA_NEARBY_DIRECTIONAL_OFFSETS {
            let index = i16::from(center) + offset;
            if (2..=66).contains(&index) {
                self.add_index(index as u8);
            }
        }
    }

    fn add_refinement(&mut self, center: u8) {
        for offset in -8..=8 {
            let index = i16::from(center) + offset;
            if (2..=66).contains(&index) {
                self.add_index(index as u8);
            }
        }
    }

    fn count(self) -> usize {
        self.count
    }

    fn iter(self) -> impl Iterator<Item = VvcIntraPredictionMode> {
        self.modes.into_iter().take(self.count)
    }

    fn iter_from(self, start: usize) -> impl Iterator<Item = VvcIntraPredictionMode> {
        self.modes.into_iter().skip(start).take(self.count - start)
    }
}

#[derive(Debug, Clone)]
struct VvcLumaModeSearchState {
    valid: [bool; VVC_CTU_SIZE * VVC_CTU_SIZE],
    modes: [VvcIntraPredictionMode; VVC_CTU_SIZE * VVC_CTU_SIZE],
}

impl VvcLumaModeSearchState {
    fn new() -> Self {
        Self {
            valid: [false; VVC_CTU_SIZE * VVC_CTU_SIZE],
            modes: [VvcIntraPredictionMode::Planar; VVC_CTU_SIZE * VVC_CTU_SIZE],
        }
    }

    fn left_of(&self, node: VvcCodingTreeNode) -> Option<VvcIntraPredictionMode> {
        let x = node.x.checked_sub(1)?;
        let y = node.y.saturating_add(node.height >> 1);
        self.mode_at(x, y)
    }

    fn above_of(&self, node: VvcCodingTreeNode) -> Option<VvcIntraPredictionMode> {
        let y = node.y.checked_sub(1)?;
        let x = node.x.saturating_add(node.width >> 1);
        self.mode_at(x, y)
    }

    fn mode_at(&self, x: u16, y: u16) -> Option<VvcIntraPredictionMode> {
        if usize::from(x) >= VVC_CTU_SIZE || usize::from(y) >= VVC_CTU_SIZE {
            return None;
        }
        let idx = usize::from(y) * VVC_CTU_SIZE + usize::from(x);
        self.valid[idx].then_some(self.modes[idx])
    }

    fn mark_node(&mut self, node: VvcCodingTreeNode, mode: VvcIntraPredictionMode) {
        let end_x = node.x.saturating_add(node.width).min(VVC_CTU_SIZE as u16);
        let end_y = node.y.saturating_add(node.height).min(VVC_CTU_SIZE as u16);
        for y in node.y..end_y {
            for x in node.x..end_x {
                let idx = usize::from(y) * VVC_CTU_SIZE + usize::from(x);
                self.valid[idx] = true;
                self.modes[idx] = mode;
            }
        }
    }

    fn co_located_mode_for_chroma_node(
        &self,
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
        let local_x = ref_x.saturating_sub(region.origin_x as u16);
        let local_y = ref_y.saturating_sub(region.origin_y as u16);
        self.mode_at(local_x, local_y)
            .unwrap_or(VvcIntraPredictionMode::Dc)
    }
}

fn vvc_luma_directional_search_candidates(
    source_frame: &VvcSampledFrame,
    mode_state: &VvcLumaModeSearchState,
    local_node: VvcCodingTreeNode,
    global_node: VvcCodingTreeNode,
) -> VvcLumaDirectionalSearchCandidates {
    let mut candidates = VvcLumaDirectionalSearchCandidates::new();
    for index in VVC_LUMA_DEFAULT_DIRECTIONAL_SEEDS {
        candidates.add_index(index);
    }
    for mode in [
        mode_state.left_of(local_node),
        mode_state.above_of(local_node),
    ]
    .into_iter()
    .flatten()
    {
        candidates.add_family(mode.luma_mode_index());
    }
    if let Some(index) = vvc_source_luma_directional_seed(source_frame, global_node) {
        candidates.add_family(index);
    }
    candidates
}

fn vvc_source_luma_directional_seed(
    source_frame: &VvcSampledFrame,
    node: VvcCodingTreeNode,
) -> Option<u8> {
    let x0 = usize::from(node.x);
    let y0 = usize::from(node.y);
    let x1 = x0
        .saturating_add(usize::from(node.width))
        .min(source_frame.geometry.width);
    let y1 = y0
        .saturating_add(usize::from(node.height))
        .min(source_frame.geometry.height);
    if x1 <= x0 + 1 || y1 <= y0 + 1 {
        return None;
    }

    let stride = source_frame.geometry.width;
    let mut gxx = 0f64;
    let mut gyy = 0f64;
    let mut gxy = 0f64;
    for y in (y0 + 1)..y1 {
        for x in (x0 + 1)..x1 {
            let sample = f64::from(source_frame.luma[y * stride + x]);
            let dx = sample - f64::from(source_frame.luma[y * stride + x - 1]);
            let dy = sample - f64::from(source_frame.luma[(y - 1) * stride + x]);
            gxx += dx * dx;
            gyy += dy * dy;
            gxy += dx * dy;
        }
    }
    if gxx + gyy == 0.0 {
        return None;
    }

    let gradient_angle = 0.5 * (2.0 * gxy).atan2(gxx - gyy);
    let mut edge_angle = gradient_angle + std::f64::consts::FRAC_PI_2;
    while edge_angle < 0.0 {
        edge_angle += std::f64::consts::PI;
    }
    while edge_angle >= std::f64::consts::PI {
        edge_angle -= std::f64::consts::PI;
    }
    let folded_edge_angle = if edge_angle > std::f64::consts::FRAC_PI_2 {
        std::f64::consts::PI - edge_angle
    } else {
        edge_angle
    };
    let mode_offset = (folded_edge_angle / std::f64::consts::FRAC_PI_2 * 32.0).round() as i16;
    Some((18 + mode_offset).clamp(2, 66) as u8)
}

fn residual_sad(residuals: &[i16]) -> u64 {
    residuals
        .iter()
        .map(|residual| u64::from(residual.unsigned_abs()))
        .sum()
}

fn luma_mode_selection_score(
    metric: VvcResidualScoreMetric,
    residuals: &[i16],
    left: Option<VvcIntraPredictionMode>,
    above: Option<VvcIntraPredictionMode>,
    mode: VvcIntraPredictionMode,
) -> u64 {
    const SYNTAX_TIE_BREAKER_SCALE: u64 = 64;
    residual_mode_selection_score(metric, residuals)
        .saturating_mul(SYNTAX_TIE_BREAKER_SCALE)
        .saturating_add(u64::from(vvc_luma_intra_mode_syntax_bin_count(
            mode, left, above,
        )))
}

fn chroma_mode_selection_score(
    metric: VvcResidualScoreMetric,
    cb_residuals: &[i16],
    cr_residuals: &[i16],
    mode: VvcChromaIntraPredictionMode,
    cclm_enabled: bool,
    syntax_tie_breaker_enabled: bool,
) -> u64 {
    const SYNTAX_TIE_BREAKER_SCALE: u64 = 64;
    let residual_score = chroma_residual_mode_selection_score(metric, cb_residuals, cr_residuals);
    let syntax_tie_breaker = if syntax_tie_breaker_enabled {
        vvc_chroma_intra_mode_syntax_bin_count(mode, cclm_enabled)
    } else {
        0
    };
    residual_score
        .saturating_mul(SYNTAX_TIE_BREAKER_SCALE)
        .saturating_add(u64::from(syntax_tie_breaker))
}

fn residual_mode_selection_score(metric: VvcResidualScoreMetric, residuals: &[i16]) -> u64 {
    match metric {
        VvcResidualScoreMetric::Sad => residual_sad(residuals),
        VvcResidualScoreMetric::Sse => residual_sse(residuals),
    }
}

fn chroma_residual_mode_selection_score(
    metric: VvcResidualScoreMetric,
    cb_residuals: &[i16],
    cr_residuals: &[i16],
) -> u64 {
    residual_mode_selection_score(metric, cb_residuals)
        .saturating_add(residual_mode_selection_score(metric, cr_residuals))
}

fn residual_sse(residuals: &[i16]) -> u64 {
    residuals
        .iter()
        .map(|residual| {
            let residual = i64::from(*residual);
            (residual * residual) as u64
        })
        .sum()
}

#[derive(Debug, Clone, Copy)]
struct VvcFinalizedLumaTu {
    abs_remainder: u8,
    negative: bool,
    dc_level: i16,
    ac_levels: [i16; VVC_LUMA_AC_COEFFS_PER_TU],
    has_ac: bool,
    transform_skip: bool,
    mrl_index: u8,
    mts_index: u8,
}

fn finalize_vvc_luma_tu(
    coding_decision: VvcLumaTuCodingDecision,
    source_frame: &VvcSampledFrame,
    frame_recon: &mut VvcReconstructionFrame,
    node: VvcCodingTreeNode,
    predicted_luma: &[VvcSample],
    residuals: &[i16],
    luma_qp: i32,
    transform_scratch: &mut VvcInverseTransformScratch,
    reconstructed_residual: &mut Vec<i16>,
) -> VvcFinalizedLumaTu {
    let finalized = match coding_decision.residual_coding {
        VvcTuResidualCodingMode::TransformSkip => {
            let dc_level = residuals.first().copied().unwrap_or(0);
            let (ac_levels, has_ac) =
                transform_skip_luma_ac_levels_and_flag(residuals, usize::from(node.width));
            reconstruct_vvc_luma_transform_skip_residuals_into(
                reconstructed_residual,
                dc_level,
                &ac_levels,
                usize::from(node.width),
                usize::from(node.height),
            );
            fill_visible_luma_node(
                &mut frame_recon.luma,
                source_frame.geometry,
                node,
                predicted_luma,
                reconstructed_residual,
                source_frame.format.bit_depth,
            );
            VvcFinalizedLumaTu {
                abs_remainder: dc_level.unsigned_abs().min(u8::MAX as u16) as u8,
                negative: dc_level < 0,
                dc_level,
                ac_levels,
                has_ac,
                transform_skip: true,
                mrl_index: coding_decision.mrl_index,
                mts_index: coding_decision.mts_index,
            }
        }
        VvcTuResidualCodingMode::Transformed => {
            let quantized = quantize_vvc_luma_residual_greedy_with_qp(
                residuals,
                node.width,
                node.height,
                source_frame.format.bit_depth,
                luma_qp,
            );
            inverse_transform_vvc_luma_quantized_block_into_with_qp(
                reconstructed_residual,
                transform_scratch,
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
                predicted_luma,
                reconstructed_residual,
                source_frame.format.bit_depth,
            );
            VvcFinalizedLumaTu {
                abs_remainder: quantized.abs_remainder,
                negative: quantized.reconstructed_dc_coeff < 0 && quantized.abs_remainder != 0,
                dc_level: quantized.reconstructed_dc_coeff,
                ac_levels: quantized.reconstructed_ac_coeffs,
                has_ac: quantized.has_ac,
                transform_skip: false,
                mrl_index: coding_decision.mrl_index,
                mts_index: coding_decision.mts_index,
            }
        }
    };
    frame_recon.mark_luma_node_available(node);
    finalized
}

#[derive(Debug, Clone, Copy)]
struct VvcFinalizedChromaTu {
    cb_dc_level: i16,
    cr_dc_level: i16,
    cb_ac_levels: [i16; VVC_CHROMA_AC_COEFFS_PER_TU],
    cr_ac_levels: [i16; VVC_CHROMA_AC_COEFFS_PER_TU],
    cb_has_ac: bool,
    cr_has_ac: bool,
    cb_transform_skip: bool,
    cr_transform_skip: bool,
}

fn finalize_vvc_chroma_tu(
    coding_decision: VvcChromaTuCodingDecision,
    source_frame: &VvcSampledFrame,
    frame_recon: &mut VvcReconstructionFrame,
    node: VvcCodingTreeNode,
    predicted_cb: &[VvcSample],
    predicted_cr: &[VvcSample],
    cb_residuals: &[i16],
    cr_residuals: &[i16],
    chroma_width: usize,
    chroma_height: usize,
    chroma_qp: i32,
    transform_scratch: &mut VvcInverseTransformScratch,
    reconstructed_residual: &mut Vec<i16>,
) -> VvcFinalizedChromaTu {
    let finalized = match coding_decision.residual_coding {
        VvcTuResidualCodingMode::TransformSkip => {
            let cb_dc_level = cb_residuals.first().copied().unwrap_or(0);
            let cr_dc_level = cr_residuals.first().copied().unwrap_or(0);
            let (cb_ac_levels, cb_has_ac) =
                transform_skip_chroma_ac_levels_and_flag(cb_residuals, chroma_width);
            let (cr_ac_levels, cr_has_ac) =
                transform_skip_chroma_ac_levels_and_flag(cr_residuals, chroma_width);
            reconstruct_vvc_chroma_transform_skip_residuals_into(
                reconstructed_residual,
                cb_dc_level,
                &cb_ac_levels,
                chroma_width,
                chroma_height,
            );
            fill_visible_chroma_node(
                &mut frame_recon.cb,
                source_frame.geometry,
                node,
                source_frame.format.chroma_sampling,
                predicted_cb,
                reconstructed_residual,
                source_frame.format.bit_depth,
            );
            reconstruct_vvc_chroma_transform_skip_residuals_into(
                reconstructed_residual,
                cr_dc_level,
                &cr_ac_levels,
                chroma_width,
                chroma_height,
            );
            fill_visible_chroma_node(
                &mut frame_recon.cr,
                source_frame.geometry,
                node,
                source_frame.format.chroma_sampling,
                predicted_cr,
                reconstructed_residual,
                source_frame.format.bit_depth,
            );
            VvcFinalizedChromaTu {
                cb_dc_level,
                cr_dc_level,
                cb_ac_levels,
                cr_ac_levels,
                cb_has_ac,
                cr_has_ac,
                cb_transform_skip: true,
                cr_transform_skip: true,
            }
        }
        VvcTuResidualCodingMode::Transformed => {
            let cb_quantized = quantize_vvc_chroma_residual_greedy_with_qp(
                cb_residuals,
                chroma_width as u16,
                chroma_height as u16,
                source_frame.format.bit_depth,
                chroma_qp,
            );
            let cr_quantized = quantize_vvc_chroma_residual_greedy_with_qp(
                cr_residuals,
                chroma_width as u16,
                chroma_height as u16,
                source_frame.format.bit_depth,
                chroma_qp,
            );
            inverse_transform_vvc_chroma_quantized_block_into_with_qp(
                reconstructed_residual,
                transform_scratch,
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
                predicted_cb,
                reconstructed_residual,
                source_frame.format.bit_depth,
            );
            inverse_transform_vvc_chroma_quantized_block_into_with_qp(
                reconstructed_residual,
                transform_scratch,
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
                predicted_cr,
                reconstructed_residual,
                source_frame.format.bit_depth,
            );
            VvcFinalizedChromaTu {
                cb_dc_level: cb_quantized.reconstructed_dc_coeff,
                cr_dc_level: cr_quantized.reconstructed_dc_coeff,
                cb_ac_levels: cb_quantized.reconstructed_ac_coeffs,
                cr_ac_levels: cr_quantized.reconstructed_ac_coeffs,
                cb_has_ac: cb_quantized.has_ac,
                cr_has_ac: cr_quantized.has_ac,
                cb_transform_skip: false,
                cr_transform_skip: false,
            }
        }
    };
    frame_recon.mark_chroma_node_available(node);
    finalized
}

fn vvc_global_ctu_node(mut node: VvcCodingTreeNode, region: VvcCtuRegion) -> VvcCodingTreeNode {
    node.x += region.origin_x as u16;
    node.y += region.origin_y as u16;
    node
}

fn predict_vvc_chroma_mode_block_into_with_availability(
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
    chroma_availability: Option<super::VvcPlaneAvailability<'_>>,
    luma_availability: Option<super::VvcPlaneAvailability<'_>>,
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

pub(in crate::vvc) fn reconstruct_vvc_luma_transform_skip_residuals_into(
    residuals: &mut Vec<i16>,
    dc_level: i16,
    ac_levels: &[i16; super::VVC_LUMA_AC_COEFFS_PER_TU],
    width: usize,
    height: usize,
) {
    residuals.clear();
    residuals.resize(width * height, 0);
    if residuals.is_empty() {
        return;
    }
    residuals[0] = dc_level;
    for y in 0..height.min(4) {
        for x in 0..width.min(4) {
            if x == 0 && y == 0 {
                continue;
            }
            residuals[y * width + x] = ac_levels[y * 4 + x - 1];
        }
    }
}

pub(in crate::vvc) fn reconstruct_vvc_chroma_transform_skip_residuals_into(
    residuals: &mut Vec<i16>,
    dc_level: i16,
    ac_levels: &[i16; VVC_CHROMA_AC_COEFFS_PER_TU],
    width: usize,
    height: usize,
) {
    residuals.clear();
    residuals.resize(width * height, 0);
    if residuals.is_empty() {
        return;
    }
    residuals[0] = dc_level;
    for (slot, (x, y)) in VVC_CHROMA_AC_POSITIONS_4X4.iter().copied().enumerate() {
        if x < width && y < height {
            residuals[y * width + x] = ac_levels[slot];
        }
    }
}

pub(in crate::vvc) fn transform_skip_luma_ac_levels_and_flag(
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

pub(in crate::vvc) fn transform_skip_chroma_ac_levels_and_flag(
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
