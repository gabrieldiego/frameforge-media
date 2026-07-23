use crate::picture::{ChromaSampling, SampleBitDepth};

#[cfg(test)]
use super::super::VvcTreeType;
use super::super::{
    chroma_subsample_x, chroma_subsample_y, vvc_chroma_cclm_node_allowed,
    vvc_chroma_explicit_candidates, vvc_chroma_intra_mode_syntax_bin_count,
    vvc_chroma_transform_nodes, vvc_downshift_sample_to_u8, vvc_luma_intra_mode_from_index,
    vvc_luma_intra_mode_is_mpm, vvc_luma_intra_mode_syntax_bin_count, vvc_luma_transform_nodes,
    vvc_neutral_sample, vvc_residual_chroma_explicit_candidate_allowed, VvcChromaCclmMode,
    VvcChromaIntraCandidateCosts, VvcChromaIntraPredictionMode, VvcChromaTuCodingDecision,
    VvcCodingTreeNode, VvcCtuPartitionShape, VvcCtuRegion, VvcIntraPredictionMode,
    VvcLumaIntraCandidateCosts, VvcLumaTuCodingDecision, VvcPictureFormat, VvcReconstructionFrame,
    VvcResidualCodingMode, VvcResidualCodingPolicy, VvcResidualScoreMetric, VvcSample,
    VvcSampledColor, VvcSampledFrame, VvcTuResidualCodingMode, VvcVideoGeometry, VVC_CTU_SIZE,
};
use super::transform::{
    luma_ac_syntax_cost_estimate, luma_coeff_rd_lambda, luma_reconstructed_residual_sse_with_mts,
};
use super::{
    fill_visible_chroma_node, fill_visible_luma_node,
    inverse_transform_vvc_chroma_quantized_block_into_with_qp,
    inverse_transform_vvc_luma_quantized_block_into_with_qp_and_mts,
    predict_vvc_chroma_cclm_block_into_with_availability,
    predict_vvc_chroma_intra_block_into_with_availability,
    predict_vvc_luma_intra_block_into_with_availability,
    predict_vvc_luma_intra_block_into_with_mrl_and_availability,
    quantize_vvc_chroma_residual_greedy_with_qp, quantize_vvc_chroma_sample,
    quantize_vvc_luma_residual_greedy_with_qp_and_mts, reconstruct_vvc_chroma,
    VvcDcPredictionScratch, VvcInverseTransformScratch, VvcQuantizedColor,
    VvcQuantizedResidualFrame, MAX_VVC_CHROMA_TUS, MAX_VVC_LUMA_TUS, VVC_CHROMA_AC_COEFFS_PER_TU,
    VVC_CHROMA_AC_POSITIONS_4X4, VVC_LUMA_AC_COEFFS_PER_TU,
};
#[cfg(feature = "vvc-stats")]
use super::{VvcIntraSearchStats, VvcResidualEnergyStats};
#[cfg(feature = "vvc-stats")]
use crate::instrumentation::JsonlInstrumentationSink;

#[cfg(feature = "vvc-stats")]
const VVC_TU_TRACE_ENV: &str = "FRAMEFORGE_VVC_TU_TRACE";
const VVC_ENABLE_LUMA_MRL_SELECTION: bool = true;
const VVC_ENABLE_LUMA_MTS_SELECTION: bool = false;

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
    let policy = VvcResidualCodingPolicy::new(source_frame.format, residual_mode);
    quantize_vvc_residual_ctu_into_frame_reconstruction_with_qp(
        source_frame,
        frame_recon,
        region,
        policy,
        super::VVC_DEFAULT_LOSSY_LUMA_QP,
        super::VVC_DEFAULT_LOSSY_CHROMA_QP,
    )
}

pub(in crate::vvc) fn quantize_vvc_residual_ctu_into_frame_reconstruction_with_qp(
    source_frame: &VvcSampledFrame,
    frame_recon: &mut VvcReconstructionFrame,
    region: VvcCtuRegion,
    policy: VvcResidualCodingPolicy,
    luma_qp: i32,
    chroma_qp: i32,
) -> VvcQuantizedColor {
    let mut luma_mode_search_state =
        VvcLumaModeSearchState::new_for_geometry(source_frame.geometry);
    quantize_vvc_residual_ctu_into_frame_reconstruction_with_qp_and_luma_modes(
        source_frame,
        frame_recon,
        region,
        policy,
        luma_qp,
        chroma_qp,
        &mut luma_mode_search_state,
    )
}

pub(in crate::vvc) fn quantize_vvc_residual_ctu_into_frame_reconstruction_with_qp_and_luma_modes(
    source_frame: &VvcSampledFrame,
    frame_recon: &mut VvcReconstructionFrame,
    region: VvcCtuRegion,
    policy: VvcResidualCodingPolicy,
    luma_qp: i32,
    chroma_qp: i32,
    luma_mode_search_state: &mut VvcLumaModeSearchState,
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
    #[cfg(feature = "vvc-stats")]
    let mut residual_energy_stats = VvcResidualEnergyStats::default();
    #[cfg(feature = "vvc-stats")]
    let mut tu_trace_sink = vvc_tu_trace_sink();

    let score_metric = policy.score_metric();
    let chroma_syntax_tie_breaker = policy.chroma_syntax_tie_breaker();
    let luma_max_leaf_size = policy.luma_max_leaf_size();
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
        let left_luma_mode = luma_mode_search_state.left_of(node);
        let above_luma_mode = luma_mode_search_state.above_of(node);
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
        if policy.luma_planar_candidate_allowed(node) {
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
        if policy.luma_directional_candidate_allowed(node) {
            let mut luma_directional_candidates =
                vvc_luma_directional_search_candidates(source_frame, &luma_mode_search_state, node);
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
        let luma_mode = policy.select_luma_intra_mode(node, luma_candidate_costs);
        debug_assert_eq!(luma_mode, best_luma_mode);
        let _best_luma_score = best_luma_score;
        let mut luma_coding_decision = policy.select_luma_tu_coding_decision(node, luma_mode);
        luma_coding_decision.mrl_index = select_vvc_luma_mrl_prediction(
            policy,
            luma_coding_decision.residual_coding,
            node,
            luma_mode,
            left_luma_mode,
            above_luma_mode,
            luma_qp,
            frame_recon,
            source_frame,
            &mut prediction_scratch,
            &mut predicted_luma,
            &mut luma_residuals,
            &mut candidate_luma_prediction,
            &mut candidate_luma_residuals,
        );
        luma_tu_intra_modes[luma_tu_count] = luma_mode;
        luma_mode_search_state.mark_node(node, luma_mode);
        #[cfg(feature = "vvc-stats")]
        residual_energy_stats.add_luma_residuals(
            &luma_residuals,
            usize::from(node.width),
            usize::from(node.height),
        );
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
        #[cfg(feature = "vvc-stats")]
        write_vvc_luma_tu_trace(
            tu_trace_sink.as_mut(),
            region,
            luma_tu_count,
            node,
            luma_mode,
            luma_tu,
            &predicted_luma,
            &luma_residuals,
        );
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
        let co_located_luma_mode = luma_mode_search_state.co_located_mode_for_chroma_node(node);
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
        if policy.chroma_cclm_candidate_allowed(node, source_frame.geometry) {
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
        let chroma_mode = policy.select_chroma_intra_mode(node, chroma_candidate_costs);
        debug_assert_eq!(chroma_mode, best_chroma_mode);
        let _best_chroma_score = best_chroma_score;
        chroma_tu_intra_modes[chroma_tu_count] = chroma_mode;
        let chroma_coding_decision = policy.select_chroma_tu_coding_decision(node, chroma_mode);
        #[cfg(feature = "vvc-stats")]
        {
            residual_energy_stats.add_chroma_residuals(&cb_residuals, chroma_width, chroma_height);
            residual_energy_stats.add_chroma_residuals(&cr_residuals, chroma_width, chroma_height);
        }
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
        #[cfg(feature = "vvc-stats")]
        write_vvc_chroma_tu_trace(
            tu_trace_sink.as_mut(),
            region,
            chroma_tu_count,
            node,
            chroma_mode,
            co_located_luma_mode,
            chroma_tu,
            chroma_width,
            chroma_height,
            &predicted_cb,
            &predicted_cr,
            &cb_residuals,
            &cr_residuals,
        );
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
            cb_tu_transform_skip.first().copied().unwrap_or(false),
            color.u,
            cb_rem,
            source_frame.format.bit_depth,
        ),
        v: finalized_vvc_chroma_sample(
            cr_tu_transform_skip.first().copied().unwrap_or(false),
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
        #[cfg(feature = "vvc-stats")]
        residual_energy_stats,
    }
}

#[cfg(feature = "vvc-stats")]
fn vvc_tu_trace_sink() -> Option<JsonlInstrumentationSink> {
    match JsonlInstrumentationSink::append_from_env(VVC_TU_TRACE_ENV) {
        Ok(sink) => sink,
        Err(err) => {
            eprintln!("failed to open {VVC_TU_TRACE_ENV}: {err}");
            None
        }
    }
}

#[cfg(feature = "vvc-stats")]
fn write_vvc_luma_tu_trace(
    sink: Option<&mut JsonlInstrumentationSink>,
    region: VvcCtuRegion,
    tu_index: usize,
    node: VvcCodingTreeNode,
    mode: VvcIntraPredictionMode,
    tu: VvcFinalizedLumaTu,
    predicted: &[VvcSample],
    residuals: &[i16],
) {
    let Some(sink) = sink else {
        return;
    };
    let nonzero_ac = tu.ac_levels.iter().filter(|level| **level != 0).count();
    let line = format!(
        "{{\"event\":\"vvc_tu\",\"component\":\"luma\",\"slice\":{},\"tu\":{},\"x\":{},\"y\":{},\"w\":{},\"h\":{},\"mode\":\"{:?}\",\"mode_index\":{},\"transform_skip\":{},\"mrl_index\":{},\"mts_index\":{},\"dc\":{},\"has_ac\":{},\"nonzero_ac\":{},\"predicted\":{},\"residuals\":{}}}",
        region.slice_address,
        tu_index,
        node.x,
        node.y,
        node.width,
        node.height,
        mode,
        mode.luma_mode_index(),
        tu.transform_skip,
        tu.mrl_index,
        tu.mts_index,
        tu.dc_level,
        tu.has_ac,
        nonzero_ac,
        json_u16_slice(predicted),
        json_i16_slice(residuals),
    );
    if let Err(err) = sink.write_json_line(&line).and_then(|()| sink.flush()) {
        eprintln!("failed to write {VVC_TU_TRACE_ENV}: {err}");
    }
}

#[cfg(feature = "vvc-stats")]
fn write_vvc_chroma_tu_trace(
    sink: Option<&mut JsonlInstrumentationSink>,
    region: VvcCtuRegion,
    tu_index: usize,
    node: VvcCodingTreeNode,
    mode: VvcChromaIntraPredictionMode,
    co_located_luma_mode: VvcIntraPredictionMode,
    tu: VvcFinalizedChromaTu,
    chroma_width: usize,
    chroma_height: usize,
    predicted_cb: &[VvcSample],
    predicted_cr: &[VvcSample],
    cb_residuals: &[i16],
    cr_residuals: &[i16],
) {
    let Some(sink) = sink else {
        return;
    };
    let cb_nonzero_ac = tu.cb_ac_levels.iter().filter(|level| **level != 0).count();
    let cr_nonzero_ac = tu.cr_ac_levels.iter().filter(|level| **level != 0).count();
    let chroma_x = usize::from(node.x);
    let chroma_y = usize::from(node.y);
    let line = format!(
        "{{\"event\":\"vvc_tu\",\"component\":\"chroma\",\"slice\":{},\"tu\":{},\"x\":{},\"y\":{},\"w\":{},\"h\":{},\"chroma_w\":{},\"chroma_h\":{},\"mode\":\"{:?}\",\"co_located_luma_mode\":\"{:?}\",\"co_located_luma_mode_index\":{},\"cb_transform_skip\":{},\"cr_transform_skip\":{},\"cb_dc\":{},\"cr_dc\":{},\"cb_has_ac\":{},\"cr_has_ac\":{},\"cb_nonzero_ac\":{},\"cr_nonzero_ac\":{},\"predicted_cb\":{},\"predicted_cr\":{},\"cb_residuals\":{},\"cr_residuals\":{}}}",
        region.slice_address,
        tu_index,
        chroma_x,
        chroma_y,
        node.width,
        node.height,
        chroma_width,
        chroma_height,
        mode,
        co_located_luma_mode,
        co_located_luma_mode.luma_mode_index(),
        tu.cb_transform_skip,
        tu.cr_transform_skip,
        tu.cb_dc_level,
        tu.cr_dc_level,
        tu.cb_has_ac,
        tu.cr_has_ac,
        cb_nonzero_ac,
        cr_nonzero_ac,
        json_u16_slice(predicted_cb),
        json_u16_slice(predicted_cr),
        json_i16_slice(cb_residuals),
        json_i16_slice(cr_residuals),
    );
    if let Err(err) = sink.write_json_line(&line).and_then(|()| sink.flush()) {
        eprintln!("failed to write {VVC_TU_TRACE_ENV}: {err}");
    }
}

#[cfg(feature = "vvc-stats")]
fn json_i16_slice(values: &[i16]) -> String {
    let mut out = String::from("[");
    for (idx, value) in values.iter().enumerate() {
        if idx != 0 {
            out.push(',');
        }
        out.push_str(&value.to_string());
    }
    out.push(']');
    out
}

#[cfg(feature = "vvc-stats")]
fn json_u16_slice(values: &[VvcSample]) -> String {
    let mut out = String::from("[");
    for (idx, value) in values.iter().enumerate() {
        if idx != 0 {
            out.push(',');
        }
        out.push_str(&value.to_string());
    }
    out.push(']');
    out
}

fn select_vvc_luma_mrl_prediction(
    policy: VvcResidualCodingPolicy,
    residual_coding: VvcTuResidualCodingMode,
    node: VvcCodingTreeNode,
    mode: VvcIntraPredictionMode,
    left: Option<VvcIntraPredictionMode>,
    above: Option<VvcIntraPredictionMode>,
    luma_qp: i32,
    frame_recon: &VvcReconstructionFrame,
    source_frame: &VvcSampledFrame,
    prediction_scratch: &mut VvcDcPredictionScratch,
    selected_prediction: &mut Vec<VvcSample>,
    selected_residuals: &mut Vec<i16>,
    candidate_prediction: &mut Vec<VvcSample>,
    candidate_residuals: &mut Vec<i16>,
) -> u8 {
    if !VVC_ENABLE_LUMA_MRL_SELECTION
        || !policy.luma_mrl_candidate_allowed(node, mode)
        || !vvc_luma_intra_mode_is_mpm(mode, left, above)
    {
        return 0;
    }

    let score_metric = policy.score_metric();
    let mut best_mrl_index = 0u8;
    let mut best_score = luma_mrl_selection_score(
        score_metric,
        residual_coding,
        node,
        0,
        selected_residuals,
        source_frame.format.bit_depth,
        luma_qp,
    );
    for mrl_index in 1..=2 {
        predict_vvc_luma_intra_block_into_with_mrl_and_availability(
            candidate_prediction,
            prediction_scratch,
            mode,
            &frame_recon.luma,
            source_frame.geometry,
            node,
            source_frame.format.bit_depth,
            mrl_index,
            Some(frame_recon.luma_availability()),
        );
        residual_luma_tu_at_into(
            candidate_residuals,
            source_frame,
            usize::from(node.x),
            usize::from(node.y),
            usize::from(node.width),
            usize::from(node.height),
            candidate_prediction,
        );
        let candidate_score = luma_mrl_selection_score(
            score_metric,
            residual_coding,
            node,
            mrl_index,
            candidate_residuals,
            source_frame.format.bit_depth,
            luma_qp,
        );
        if candidate_score < best_score {
            best_score = candidate_score;
            best_mrl_index = mrl_index;
            std::mem::swap(selected_prediction, candidate_prediction);
            std::mem::swap(selected_residuals, candidate_residuals);
        }
    }
    best_mrl_index
}

fn luma_mrl_selection_score(
    metric: VvcResidualScoreMetric,
    residual_coding: VvcTuResidualCodingMode,
    node: VvcCodingTreeNode,
    mrl_index: u8,
    residuals: &[i16],
    bit_depth: SampleBitDepth,
    luma_qp: i32,
) -> u64 {
    const SYNTAX_TIE_BREAKER_SCALE: u64 = 64;
    if matches!(residual_coding, VvcTuResidualCodingMode::Transformed) {
        let (residual, mts_index) = select_vvc_luma_residual_block_with_mts(
            residual_coding,
            0,
            residuals,
            node.width,
            node.height,
            bit_depth,
            luma_qp,
        );
        let sse = luma_reconstructed_residual_sse_with_mts(
            residuals,
            node.width,
            node.height,
            bit_depth,
            luma_qp,
            residual.dc_level,
            &residual.ac_levels,
            mts_index,
        );
        let lambda = luma_coeff_rd_lambda(luma_qp, bit_depth);
        let coefficient_cost = u64::from(residual.dc_level != 0)
            .saturating_mul(8)
            .saturating_add(luma_ac_syntax_cost_estimate(
                node.width,
                node.height,
                &residual.ac_levels,
            ));
        let mrl_cost = u64::from(vvc_luma_mrl_syntax_bin_count(node, mrl_index));
        return sse
            .saturating_add(lambda.saturating_mul(coefficient_cost.saturating_add(mrl_cost)));
    }
    residual_mode_selection_score(metric, residuals)
        .saturating_mul(SYNTAX_TIE_BREAKER_SCALE)
        .saturating_add(u64::from(vvc_luma_mrl_syntax_bin_count(node, mrl_index)))
}

fn vvc_luma_mrl_syntax_bin_count(node: VvcCodingTreeNode, mrl_index: u8) -> u8 {
    if node.y % VVC_CTU_SIZE as u16 == 0 {
        0
    } else if mrl_index == 0 {
        1
    } else {
        2
    }
}

fn finalized_vvc_chroma_sample(
    transform_skip: bool,
    source: VvcSample,
    quantized_remainder: u8,
    bit_depth: SampleBitDepth,
) -> u8 {
    if transform_skip {
        vvc_downshift_sample_to_u8(source, bit_depth)
    } else {
        reconstruct_vvc_chroma(quantized_remainder)
    }
}

const VVC_LUMA_DIRECTIONAL_SEARCH_CANDIDATE_CAPACITY: usize = 65;
const VVC_LUMA_DEFAULT_DIRECTIONAL_SEEDS: [u8; 9] = [18, 50, 34, 10, 26, 42, 58, 2, 66];
const VVC_LUMA_NEARBY_DIRECTIONAL_OFFSETS: [i16; 7] = [0, -1, 1, -2, 2, -4, 4];
const VVC_LUMA_MODE_CELL_SIZE: usize = 4;

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
pub(in crate::vvc) struct VvcLumaModeSearchState {
    width: usize,
    height: usize,
    cell_cols: usize,
    valid: Vec<bool>,
    modes: Vec<VvcIntraPredictionMode>,
}

impl VvcLumaModeSearchState {
    pub(in crate::vvc) fn new_for_geometry(geometry: VvcVideoGeometry) -> Self {
        let width = geometry.coded_width();
        let height = geometry.coded_height();
        let cell_cols = width.div_ceil(VVC_LUMA_MODE_CELL_SIZE);
        let cell_rows = height.div_ceil(VVC_LUMA_MODE_CELL_SIZE);
        let cell_count = cell_cols.saturating_mul(cell_rows);
        Self {
            width,
            height,
            cell_cols,
            valid: vec![false; cell_count],
            modes: vec![VvcIntraPredictionMode::Planar; cell_count],
        }
    }

    fn left_of(&self, node: VvcCodingTreeNode) -> Option<VvcIntraPredictionMode> {
        let x = node.x.checked_sub(1)?;
        let y = node.y.saturating_add(node.height).saturating_sub(1);
        self.mode_at(x, y)
    }

    fn above_of(&self, node: VvcCodingTreeNode) -> Option<VvcIntraPredictionMode> {
        let y = node.y.checked_sub(1)?;
        let x = node.x.saturating_add(node.width).saturating_sub(1);
        self.mode_at(x, y)
    }

    fn mode_at(&self, x: u16, y: u16) -> Option<VvcIntraPredictionMode> {
        let x = usize::from(x);
        let y = usize::from(y);
        if x >= self.width || y >= self.height {
            return None;
        }
        let cell_x = x / VVC_LUMA_MODE_CELL_SIZE;
        let cell_y = y / VVC_LUMA_MODE_CELL_SIZE;
        let idx = cell_y * self.cell_cols + cell_x;
        self.valid[idx].then_some(self.modes[idx])
    }

    fn mark_node(&mut self, node: VvcCodingTreeNode, mode: VvcIntraPredictionMode) {
        let start_x = usize::from(node.x).min(self.width);
        let start_y = usize::from(node.y).min(self.height);
        let end_x = usize::from(node.x)
            .saturating_add(usize::from(node.width))
            .min(self.width);
        let end_y = usize::from(node.y)
            .saturating_add(usize::from(node.height))
            .min(self.height);
        if end_x <= start_x || end_y <= start_y {
            return;
        }
        let start_cell_x = usize::from(node.x) / VVC_LUMA_MODE_CELL_SIZE;
        let start_cell_y = usize::from(node.y) / VVC_LUMA_MODE_CELL_SIZE;
        let end_cell_x = end_x.div_ceil(VVC_LUMA_MODE_CELL_SIZE);
        let end_cell_y = end_y.div_ceil(VVC_LUMA_MODE_CELL_SIZE);
        for cell_y in start_cell_y..end_cell_y {
            for cell_x in start_cell_x..end_cell_x {
                let idx = cell_y * self.cell_cols + cell_x;
                self.valid[idx] = true;
                self.modes[idx] = mode;
            }
        }
    }

    fn co_located_mode_for_chroma_node(
        &self,
        chroma_node: VvcCodingTreeNode,
    ) -> VvcIntraPredictionMode {
        let max_x = self.width.saturating_sub(1).min(usize::from(u16::MAX)) as u16;
        let max_y = self.height.saturating_sub(1).min(usize::from(u16::MAX)) as u16;
        let ref_x = chroma_node
            .x
            .saturating_add(chroma_node.width >> 1)
            .min(max_x);
        let ref_y = chroma_node
            .y
            .saturating_add(chroma_node.height >> 1)
            .min(max_y);
        self.mode_at(ref_x, ref_y)
            .unwrap_or(VvcIntraPredictionMode::Dc)
    }
}

fn vvc_luma_directional_search_candidates(
    source_frame: &VvcSampledFrame,
    mode_state: &VvcLumaModeSearchState,
    global_node: VvcCodingTreeNode,
) -> VvcLumaDirectionalSearchCandidates {
    let mut candidates = VvcLumaDirectionalSearchCandidates::new();
    for index in VVC_LUMA_DEFAULT_DIRECTIONAL_SEEDS {
        candidates.add_index(index);
    }
    for mode in [
        mode_state.left_of(global_node),
        mode_state.above_of(global_node),
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
    let mut gxx = 0i64;
    let mut gyy = 0i64;
    let mut gxy = 0i64;
    for y in (y0 + 1)..y1 {
        for x in (x0 + 1)..x1 {
            let sample = i64::from(source_frame.luma[y * stride + x]);
            let dx = sample - i64::from(source_frame.luma[y * stride + x - 1]);
            let dy = sample - i64::from(source_frame.luma[(y - 1) * stride + x]);
            gxx += dx * dx;
            gyy += dy * dy;
            gxy += dx * dy;
        }
    }
    if gxx == 0 && gyy == 0 {
        return None;
    }

    let gradient_angle = 0.5 * (2.0 * gxy as f64).atan2((gxx - gyy) as f64);
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
struct VvcFinalizedResidualBlock<const AC_COEFFS: usize> {
    dc_level: i16,
    ac_levels: [i16; AC_COEFFS],
    has_ac: bool,
    transform_skip: bool,
}

impl<const AC_COEFFS: usize> VvcFinalizedResidualBlock<AC_COEFFS> {
    fn abs_remainder(self) -> u8 {
        self.dc_level.unsigned_abs().min(u8::MAX as u16) as u8
    }

    fn negative(self) -> bool {
        self.dc_level < 0 && self.abs_remainder() != 0
    }
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
    let (residual, mts_index) = select_vvc_luma_residual_block_with_mts(
        coding_decision.residual_coding,
        coding_decision.mts_index,
        residuals,
        node.width,
        node.height,
        source_frame.format.bit_depth,
        luma_qp,
    );
    reconstruct_vvc_luma_residual_block_into(
        residual,
        mts_index,
        reconstructed_residual,
        transform_scratch,
        node.width,
        node.height,
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
    let finalized = VvcFinalizedLumaTu {
        abs_remainder: residual.abs_remainder(),
        negative: residual.negative(),
        dc_level: residual.dc_level,
        ac_levels: residual.ac_levels,
        has_ac: residual.has_ac,
        transform_skip: residual.transform_skip,
        mrl_index: coding_decision.mrl_index,
        mts_index,
    };
    frame_recon.mark_luma_node_available(node);
    finalized
}

fn select_vvc_luma_residual_block_with_mts(
    residual_coding: VvcTuResidualCodingMode,
    requested_mts_index: u8,
    residuals: &[i16],
    width: u16,
    height: u16,
    bit_depth: SampleBitDepth,
    luma_qp: i32,
) -> (VvcFinalizedResidualBlock<VVC_LUMA_AC_COEFFS_PER_TU>, u8) {
    let base = finalize_vvc_luma_residual_block(
        residual_coding,
        requested_mts_index,
        residuals,
        width,
        height,
        bit_depth,
        luma_qp,
    );
    if !vvc_luma_mts_selection_allowed(residual_coding, requested_mts_index, width, height, base) {
        return (base, requested_mts_index);
    }

    let mut best_residual = base;
    let mut best_mts_index = 0u8;
    let mut best_score =
        luma_mts_candidate_score(residuals, width, height, bit_depth, luma_qp, base, 0);
    for mts_index in 2..=5 {
        let candidate = finalize_vvc_luma_residual_block(
            residual_coding,
            mts_index,
            residuals,
            width,
            height,
            bit_depth,
            luma_qp,
        );
        if !candidate.has_ac {
            continue;
        }
        let score = luma_mts_candidate_score(
            residuals, width, height, bit_depth, luma_qp, candidate, mts_index,
        );
        if score < best_score {
            best_score = score;
            best_residual = candidate;
            best_mts_index = mts_index;
        }
    }

    (best_residual, best_mts_index)
}

fn vvc_luma_mts_selection_allowed(
    residual_coding: VvcTuResidualCodingMode,
    requested_mts_index: u8,
    width: u16,
    height: u16,
    base: VvcFinalizedResidualBlock<VVC_LUMA_AC_COEFFS_PER_TU>,
) -> bool {
    VVC_ENABLE_LUMA_MTS_SELECTION
        && requested_mts_index == 0
        && matches!(residual_coding, VvcTuResidualCodingMode::Transformed)
        && width == 8
        && height == 8
        && base.has_ac
}

fn luma_mts_candidate_score(
    source_residuals: &[i16],
    width: u16,
    height: u16,
    bit_depth: SampleBitDepth,
    qp: i32,
    residual: VvcFinalizedResidualBlock<VVC_LUMA_AC_COEFFS_PER_TU>,
    mts_index: u8,
) -> u64 {
    let sse = luma_reconstructed_residual_sse_with_mts(
        source_residuals,
        width,
        height,
        bit_depth,
        qp,
        residual.dc_level,
        &residual.ac_levels,
        mts_index,
    );
    let lambda = luma_coeff_rd_lambda(qp, bit_depth);
    let coefficient_cost = luma_ac_syntax_cost_estimate(width, height, &residual.ac_levels);
    let mts_cost = luma_mts_syntax_cost_estimate(residual.has_ac, mts_index);
    sse.saturating_add(lambda.saturating_mul(coefficient_cost.saturating_add(mts_cost)))
}

fn luma_mts_syntax_cost_estimate(has_ac: bool, mts_index: u8) -> u64 {
    if !has_ac {
        return 0;
    }
    match mts_index {
        0 => 1,
        2 => 2,
        3 => 3,
        4 | 5 => 4,
        _ => 8,
    }
}

fn finalize_vvc_luma_residual_block(
    residual_coding: VvcTuResidualCodingMode,
    mts_index: u8,
    residuals: &[i16],
    width: u16,
    height: u16,
    bit_depth: SampleBitDepth,
    luma_qp: i32,
) -> VvcFinalizedResidualBlock<VVC_LUMA_AC_COEFFS_PER_TU> {
    match residual_coding {
        VvcTuResidualCodingMode::TransformSkip => {
            debug_assert_eq!(mts_index, 0);
            let dc_level = residuals.first().copied().unwrap_or(0);
            let (ac_levels, has_ac) =
                transform_skip_luma_ac_levels_and_flag(residuals, usize::from(width));
            VvcFinalizedResidualBlock {
                dc_level,
                ac_levels,
                has_ac,
                transform_skip: true,
            }
        }
        VvcTuResidualCodingMode::Transformed => {
            let quantized = quantize_vvc_luma_residual_greedy_with_qp_and_mts(
                residuals, width, height, bit_depth, luma_qp, mts_index,
            );
            VvcFinalizedResidualBlock {
                dc_level: quantized.reconstructed_dc_coeff,
                ac_levels: quantized.reconstructed_ac_coeffs,
                has_ac: quantized.has_ac,
                transform_skip: false,
            }
        }
    }
}

fn reconstruct_vvc_luma_residual_block_into(
    residual: VvcFinalizedResidualBlock<VVC_LUMA_AC_COEFFS_PER_TU>,
    mts_index: u8,
    reconstructed_residual: &mut Vec<i16>,
    transform_scratch: &mut VvcInverseTransformScratch,
    width: u16,
    height: u16,
    bit_depth: SampleBitDepth,
    luma_qp: i32,
) {
    if residual.transform_skip {
        reconstruct_vvc_luma_transform_skip_residuals_into(
            reconstructed_residual,
            residual.dc_level,
            &residual.ac_levels,
            usize::from(width),
            usize::from(height),
        );
    } else {
        inverse_transform_vvc_luma_quantized_block_into_with_qp_and_mts(
            reconstructed_residual,
            transform_scratch,
            width,
            height,
            residual.dc_level,
            &residual.ac_levels,
            bit_depth,
            luma_qp,
            mts_index,
        );
    }
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
    let cb_residual = finalize_vvc_chroma_residual_block(
        coding_decision.residual_coding,
        cb_residuals,
        chroma_width,
        chroma_height,
        source_frame.format.bit_depth,
        chroma_qp,
    );
    let cr_residual = finalize_vvc_chroma_residual_block(
        coding_decision.residual_coding,
        cr_residuals,
        chroma_width,
        chroma_height,
        source_frame.format.bit_depth,
        chroma_qp,
    );
    reconstruct_vvc_chroma_residual_block_into(
        cb_residual,
        reconstructed_residual,
        transform_scratch,
        chroma_width,
        chroma_height,
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
    reconstruct_vvc_chroma_residual_block_into(
        cr_residual,
        reconstructed_residual,
        transform_scratch,
        chroma_width,
        chroma_height,
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
    let finalized = VvcFinalizedChromaTu {
        cb_dc_level: cb_residual.dc_level,
        cr_dc_level: cr_residual.dc_level,
        cb_ac_levels: cb_residual.ac_levels,
        cr_ac_levels: cr_residual.ac_levels,
        cb_has_ac: cb_residual.has_ac,
        cr_has_ac: cr_residual.has_ac,
        cb_transform_skip: cb_residual.transform_skip,
        cr_transform_skip: cr_residual.transform_skip,
    };
    frame_recon.mark_chroma_node_available(node);
    finalized
}

fn finalize_vvc_chroma_residual_block(
    residual_coding: VvcTuResidualCodingMode,
    residuals: &[i16],
    width: usize,
    height: usize,
    bit_depth: SampleBitDepth,
    chroma_qp: i32,
) -> VvcFinalizedResidualBlock<VVC_CHROMA_AC_COEFFS_PER_TU> {
    match residual_coding {
        VvcTuResidualCodingMode::TransformSkip => {
            let dc_level = residuals.first().copied().unwrap_or(0);
            let (ac_levels, has_ac) = transform_skip_chroma_ac_levels_and_flag(residuals, width);
            VvcFinalizedResidualBlock {
                dc_level,
                ac_levels,
                has_ac,
                transform_skip: true,
            }
        }
        VvcTuResidualCodingMode::Transformed => {
            let quantized = quantize_vvc_chroma_residual_greedy_with_qp(
                residuals,
                width as u16,
                height as u16,
                bit_depth,
                chroma_qp,
            );
            VvcFinalizedResidualBlock {
                dc_level: quantized.reconstructed_dc_coeff,
                ac_levels: quantized.reconstructed_ac_coeffs,
                has_ac: quantized.has_ac,
                transform_skip: false,
            }
        }
    }
}

fn reconstruct_vvc_chroma_residual_block_into(
    residual: VvcFinalizedResidualBlock<VVC_CHROMA_AC_COEFFS_PER_TU>,
    reconstructed_residual: &mut Vec<i16>,
    transform_scratch: &mut VvcInverseTransformScratch,
    width: usize,
    height: usize,
    bit_depth: SampleBitDepth,
    chroma_qp: i32,
) {
    if residual.transform_skip {
        reconstruct_vvc_chroma_transform_skip_residuals_into(
            reconstructed_residual,
            residual.dc_level,
            &residual.ac_levels,
            width,
            height,
        );
    } else {
        inverse_transform_vvc_chroma_quantized_block_into_with_qp(
            reconstructed_residual,
            transform_scratch,
            width as u16,
            height as u16,
            residual.dc_level,
            &residual.ac_levels,
            bit_depth,
            chroma_qp,
        );
    }
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
    let active_width = if width == 8 && height == 8 {
        8
    } else {
        width.min(4)
    };
    let active_height = if width == 8 && height == 8 {
        8
    } else {
        height.min(4)
    };
    for y in 0..active_height {
        for x in 0..active_width {
            if x == 0 && y == 0 {
                continue;
            }
            residuals[y * width + x] = ac_levels[y * active_width + x - 1];
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
    let height = residuals.len() / width;
    let active_width = if width == 8 && height == 8 {
        8
    } else {
        width.min(4)
    };
    let active_height = if width == 8 && height == 8 {
        8
    } else {
        height.min(4)
    };
    for y in 0..active_height {
        for x in 0..active_width {
            if x == 0 && y == 0 {
                continue;
            }
            let raster_idx = y * width + x;
            if raster_idx < residuals.len() {
                let level = residuals[raster_idx];
                levels[y * active_width + x - 1] = level;
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

#[cfg(test)]
mod tests {
    use super::*;

    fn sampled_luma_frame(width: usize, height: usize, luma: Vec<VvcSample>) -> VvcSampledFrame {
        assert_eq!(luma.len(), width * height);
        let format = VvcPictureFormat {
            chroma_sampling: ChromaSampling::Cs420,
            bit_depth: SampleBitDepth::new(8).expect("valid bit depth"),
        };
        let chroma_len = (width / 2) * (height / 2);
        VvcSampledFrame {
            geometry: VvcVideoGeometry { width, height },
            format,
            luma,
            cb: vec![128; chroma_len],
            cr: vec![128; chroma_len],
            chroma_len,
        }
    }

    #[test]
    fn vvc_source_luma_directional_seed_maps_integer_gradients() {
        let node = VvcCodingTreeNode::root(8, 8, VvcTreeType::DualTreeLuma);
        let flat = sampled_luma_frame(8, 8, vec![64; 64]);
        assert_eq!(vvc_source_luma_directional_seed(&flat, node), None);

        let horizontal_ramp = sampled_luma_frame(
            8,
            8,
            (0..8)
                .flat_map(|_| (0..8).map(|x| (x * 16) as VvcSample))
                .collect(),
        );
        assert_eq!(
            vvc_source_luma_directional_seed(&horizontal_ramp, node),
            Some(50)
        );

        let vertical_ramp = sampled_luma_frame(
            8,
            8,
            (0..8)
                .flat_map(|y| (0..8).map(move |_| (y * 16) as VvcSample))
                .collect(),
        );
        assert_eq!(
            vvc_source_luma_directional_seed(&vertical_ramp, node),
            Some(18)
        );

        let diagonal_ramp = sampled_luma_frame(
            8,
            8,
            (0..8)
                .flat_map(|y| (0..8).map(move |x| ((x + y) * 8) as VvcSample))
                .collect(),
        );
        assert_eq!(
            vvc_source_luma_directional_seed(&diagonal_ramp, node),
            Some(34)
        );
    }
}
