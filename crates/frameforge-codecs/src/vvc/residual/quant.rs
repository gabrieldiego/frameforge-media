use crate::picture::{ChromaSampling, SampleBitDepth};

#[cfg(test)]
use super::super::VvcTreeType;
use super::super::{
    chroma_subsample_x, chroma_subsample_y, vvc_chroma_cclm_node_allowed,
    vvc_chroma_explicit_candidates, vvc_chroma_intra_mode_syntax_bin_count,
    vvc_chroma_transform_nodes, vvc_downshift_sample_to_u8, vvc_luma_intra_mode_from_index,
    vvc_luma_intra_mode_is_mpm, vvc_luma_intra_mode_syntax_bin_count, vvc_luma_transform_nodes,
    vvc_neutral_sample, vvc_residual_chroma_explicit_candidate_allowed, VvcBdpcmMode,
    VvcChromaCclmMode, VvcChromaIntraCandidateCost, VvcChromaIntraCandidateCosts,
    VvcChromaIntraPredictionMode, VvcChromaTuCodingDecision, VvcCodingTreeNode,
    VvcCtuPartitionShape, VvcCtuRegion, VvcIntraPredictionMode, VvcLumaIntraCandidateCost,
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
    predict_vvc_chroma_bdpcm_block_into_with_availability,
    predict_vvc_chroma_cclm_block_into_with_availability,
    predict_vvc_chroma_intra_block_into_with_availability,
    predict_vvc_luma_bdpcm_block_into_with_availability,
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
const VVC_ENABLE_LOSSY_TRANSFORM_SKIP_SELECTION: bool = true;
const VVC_ENABLE_BDPCM_SELECTION: bool = true;
const VVC_LUMA_LOSSY_RD_SHORTLIST_SIZE: usize = 2;
const VVC_CHROMA_LOSSY_RD_SHORTLIST_SIZE: usize = 2;
const VVC_TRANSFORM_SKIP_MAX_SIZE: u16 = 8;
const VVC_TRANSFORM_SKIP_INV_QUANT_SCALES: [i32; 6] = [40, 45, 51, 57, 64, 72];

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
    let (luma_qp, chroma_qp) = match residual_mode {
        VvcResidualCodingMode::Lossless => {
            let qp = super::super::vvc_lossless_slice_qp(source_frame.format.bit_depth);
            (qp, qp)
        }
        VvcResidualCodingMode::Lossy => (
            super::VVC_DEFAULT_LOSSY_LUMA_QP,
            super::VVC_DEFAULT_LOSSY_CHROMA_QP,
        ),
    };
    quantize_vvc_residual_ctu_into_frame_reconstruction_with_qp(
        source_frame,
        frame_recon,
        region,
        policy,
        luma_qp,
        chroma_qp,
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
    let mut luma_tu_bdpcm_modes = [VvcBdpcmMode::None; MAX_VVC_LUMA_TUS];
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
    let mut chroma_tu_bdpcm_modes = [VvcBdpcmMode::None; MAX_VVC_CHROMA_TUS];
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
        let raw_luma_mode = policy.select_luma_intra_mode(node, luma_candidate_costs);
        debug_assert_eq!(raw_luma_mode, best_luma_mode);
        let _best_luma_score = best_luma_score;
        let selected_luma_mode = select_vvc_luma_mode_with_rd_refinement(
            policy,
            node,
            raw_luma_mode,
            luma_candidate_costs,
            left_luma_mode,
            above_luma_mode,
            source_frame,
            frame_recon,
            luma_qp,
            &mut prediction_scratch,
            &mut predicted_luma,
            &mut luma_residuals,
            &mut candidate_luma_prediction,
            &mut candidate_luma_residuals,
        );
        #[cfg(feature = "vvc-stats")]
        if selected_luma_mode.residual.is_some() {
            intra_search_stats.add_luma_rd_refinement_attempt();
            if selected_luma_mode.mode != raw_luma_mode {
                intra_search_stats.add_luma_rd_refinement_switch();
            }
        }
        let mut luma_mode = selected_luma_mode.mode;
        let mut luma_coding_decision = policy.select_luma_tu_coding_decision(node, luma_mode);
        let selected_luma_mrl = select_vvc_luma_mrl_prediction(
            policy,
            luma_coding_decision.residual_coding,
            luma_coding_decision.mts_index,
            node,
            luma_mode,
            left_luma_mode,
            above_luma_mode,
            luma_qp,
            selected_luma_mode.residual,
            frame_recon,
            source_frame,
            &mut prediction_scratch,
            &mut predicted_luma,
            &mut luma_residuals,
            &mut candidate_luma_prediction,
            &mut candidate_luma_residuals,
        );
        luma_coding_decision.mrl_index = selected_luma_mrl.mrl_index;
        let mut selected_luma_residual = selected_luma_mrl.residual;
        if let Some(selected_bdpcm) = select_vvc_luma_bdpcm_prediction(
            policy,
            node,
            luma_mode,
            luma_coding_decision,
            left_luma_mode,
            above_luma_mode,
            luma_qp,
            selected_luma_residual,
            frame_recon,
            source_frame,
            &mut prediction_scratch,
            &mut predicted_luma,
            &mut luma_residuals,
            &mut candidate_luma_prediction,
            &mut candidate_luma_residuals,
        ) {
            luma_mode = selected_bdpcm.mode;
            luma_coding_decision = selected_bdpcm.coding_decision;
            selected_luma_residual = Some(selected_bdpcm.residual);
        }
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
            selected_luma_residual,
            &mut transform_scratch,
            &mut reconstructed_residual,
        );
        luma_tu_remainders[luma_tu_count] = luma_tu.abs_remainder;
        luma_tu_negative[luma_tu_count] = luma_tu.negative;
        luma_tu_dc_levels[luma_tu_count] = luma_tu.dc_level;
        luma_tu_ac_levels[luma_tu_count] = luma_tu.ac_levels;
        luma_tu_has_ac[luma_tu_count] = luma_tu.has_ac;
        luma_tu_transform_skip[luma_tu_count] = luma_tu.transform_skip;
        luma_tu_bdpcm_modes[luma_tu_count] = luma_tu.bdpcm_mode;
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
        let raw_chroma_mode = policy.select_chroma_intra_mode(node, chroma_candidate_costs);
        debug_assert_eq!(raw_chroma_mode, best_chroma_mode);
        let _best_chroma_score = best_chroma_score;
        let selected_chroma_mode = select_vvc_chroma_mode_with_rd_refinement(
            policy,
            node,
            raw_chroma_mode,
            chroma_candidate_costs,
            co_located_luma_mode,
            cclm_syntax_enabled,
            source_frame,
            frame_recon,
            chroma_width,
            chroma_height,
            chroma_qp,
            &mut prediction_scratch,
            &mut predicted_cb,
            &mut predicted_cr,
            &mut cb_residuals,
            &mut cr_residuals,
            &mut candidate_cb_prediction,
            &mut candidate_cr_prediction,
            &mut candidate_cb_residuals,
            &mut candidate_cr_residuals,
            &mut transform_scratch,
            &mut reconstructed_residual,
        );
        #[cfg(feature = "vvc-stats")]
        if selected_chroma_mode.residual.is_some() {
            intra_search_stats.add_chroma_rd_refinement_attempt();
            if selected_chroma_mode.mode != raw_chroma_mode {
                intra_search_stats.add_chroma_rd_refinement_switch();
            }
        }
        let mut chroma_mode = selected_chroma_mode.mode;
        let mut selected_chroma_residual = selected_chroma_mode.residual;
        if let Some(selected_bdpcm) = select_vvc_chroma_bdpcm_prediction(
            policy,
            node,
            chroma_mode,
            cclm_syntax_enabled,
            source_frame,
            frame_recon,
            chroma_width,
            chroma_height,
            chroma_qp,
            selected_chroma_residual,
            &mut prediction_scratch,
            &mut predicted_cb,
            &mut predicted_cr,
            &mut cb_residuals,
            &mut cr_residuals,
            &mut candidate_cb_prediction,
            &mut candidate_cr_prediction,
            &mut candidate_cb_residuals,
            &mut candidate_cr_residuals,
            &mut transform_scratch,
            &mut reconstructed_residual,
        ) {
            chroma_mode = selected_bdpcm.mode;
            selected_chroma_residual = Some(selected_bdpcm.residual);
        }
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
            selected_chroma_residual,
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
        chroma_tu_bdpcm_modes[chroma_tu_count] = chroma_tu.bdpcm_mode;
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
        luma_tu_bdpcm_modes,
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
        chroma_tu_bdpcm_modes,
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
        "{{\"event\":\"vvc_tu\",\"component\":\"luma\",\"slice\":{},\"tu\":{},\"x\":{},\"y\":{},\"w\":{},\"h\":{},\"mode\":\"{:?}\",\"mode_index\":{},\"transform_skip\":{},\"bdpcm_mode\":\"{:?}\",\"mrl_index\":{},\"mts_index\":{},\"dc\":{},\"has_ac\":{},\"nonzero_ac\":{},\"predicted\":{},\"residuals\":{}}}",
        region.slice_address,
        tu_index,
        node.x,
        node.y,
        node.width,
        node.height,
        mode,
        mode.luma_mode_index(),
        tu.transform_skip,
        tu.bdpcm_mode,
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
        "{{\"event\":\"vvc_tu\",\"component\":\"chroma\",\"slice\":{},\"tu\":{},\"x\":{},\"y\":{},\"w\":{},\"h\":{},\"chroma_w\":{},\"chroma_h\":{},\"mode\":\"{:?}\",\"co_located_luma_mode\":\"{:?}\",\"co_located_luma_mode_index\":{},\"cb_transform_skip\":{},\"cr_transform_skip\":{},\"bdpcm_mode\":\"{:?}\",\"cb_dc\":{},\"cr_dc\":{},\"cb_has_ac\":{},\"cr_has_ac\":{},\"cb_nonzero_ac\":{},\"cr_nonzero_ac\":{},\"predicted_cb\":{},\"predicted_cr\":{},\"cb_residuals\":{},\"cr_residuals\":{}}}",
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
        tu.bdpcm_mode,
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

fn select_vvc_luma_mode_with_rd_refinement(
    policy: VvcResidualCodingPolicy,
    node: VvcCodingTreeNode,
    raw_mode: VvcIntraPredictionMode,
    candidate_costs: VvcLumaIntraCandidateCosts,
    left: Option<VvcIntraPredictionMode>,
    above: Option<VvcIntraPredictionMode>,
    source_frame: &VvcSampledFrame,
    frame_recon: &VvcReconstructionFrame,
    luma_qp: i32,
    prediction_scratch: &mut VvcDcPredictionScratch,
    selected_prediction: &mut Vec<VvcSample>,
    selected_residuals: &mut Vec<i16>,
    candidate_prediction: &mut Vec<VvcSample>,
    candidate_residuals: &mut Vec<i16>,
) -> VvcSelectedLumaMode {
    let raw_decision = policy.select_luma_tu_coding_decision(node, raw_mode);
    if !vvc_luma_lossy_rd_refinement_allowed(policy, node, raw_decision) {
        return VvcSelectedLumaMode {
            mode: raw_mode,
            residual: None,
        };
    }

    let mut best_mode = raw_mode;
    let mut best_candidate = score_vvc_luma_mode_rd_candidate(
        raw_decision,
        node,
        raw_mode,
        left,
        above,
        selected_residuals,
        source_frame.format.bit_depth,
        luma_qp,
    );
    let shortlist = VvcLumaModeRdShortlist::from_candidate_costs(candidate_costs);
    for candidate in shortlist.iter() {
        let mode = candidate.mode();
        if mode.luma_mode_index() == raw_mode.luma_mode_index() {
            continue;
        }
        let coding_decision = policy.select_luma_tu_coding_decision(node, mode);
        if !matches!(
            coding_decision.residual_coding,
            VvcTuResidualCodingMode::Transformed
        ) {
            continue;
        }
        predict_vvc_luma_intra_block_into_with_availability(
            candidate_prediction,
            prediction_scratch,
            mode,
            &frame_recon.luma,
            source_frame.geometry,
            node,
            source_frame.format.bit_depth,
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
        let rd_candidate = score_vvc_luma_mode_rd_candidate(
            coding_decision,
            node,
            mode,
            left,
            above,
            candidate_residuals,
            source_frame.format.bit_depth,
            luma_qp,
        );
        if rd_candidate.selects_over(best_candidate) {
            best_mode = mode;
            best_candidate = rd_candidate;
            std::mem::swap(selected_prediction, candidate_prediction);
            std::mem::swap(selected_residuals, candidate_residuals);
        }
    }

    VvcSelectedLumaMode {
        mode: best_mode,
        residual: Some(best_candidate.residual),
    }
}

fn vvc_luma_lossy_rd_refinement_allowed(
    policy: VvcResidualCodingPolicy,
    node: VvcCodingTreeNode,
    decision: VvcLumaTuCodingDecision,
) -> bool {
    policy.residual_mode() == VvcResidualCodingMode::Lossy
        && matches!(
            decision.residual_coding,
            VvcTuResidualCodingMode::Transformed
        )
        && [4, 8, 16, 32].contains(&node.width)
        && [4, 8, 16, 32].contains(&node.height)
}

#[derive(Debug, Clone, Copy)]
struct VvcSelectedLumaMode {
    mode: VvcIntraPredictionMode,
    residual: Option<VvcSelectedLumaResidual>,
}

#[derive(Debug, Clone, Copy)]
struct VvcLumaModeRdCandidate {
    distortion: u64,
    rate_cost: u64,
    residual: VvcSelectedLumaResidual,
}

impl VvcLumaModeRdCandidate {
    fn selects_over(self, best: Self) -> bool {
        (self.rate_cost < best.rate_cost && self.distortion <= best.distortion)
            || (self.rate_cost <= best.rate_cost && self.distortion < best.distortion)
    }
}

fn score_vvc_luma_mode_rd_candidate(
    coding_decision: VvcLumaTuCodingDecision,
    node: VvcCodingTreeNode,
    mode: VvcIntraPredictionMode,
    left: Option<VvcIntraPredictionMode>,
    above: Option<VvcIntraPredictionMode>,
    residuals: &[i16],
    bit_depth: SampleBitDepth,
    luma_qp: i32,
) -> VvcLumaModeRdCandidate {
    let (block, mts_index) = select_vvc_luma_residual_block_with_mts(
        coding_decision.residual_coding,
        coding_decision.mts_index,
        residuals,
        node.width,
        node.height,
        bit_depth,
        luma_qp,
    );
    let residual = VvcSelectedLumaResidual { block, mts_index };
    let mode_cost = u64::from(vvc_luma_intra_mode_syntax_bin_count(mode, left, above));
    let score =
        vvc_luma_quantized_residual_score(residuals, node, bit_depth, luma_qp, residual, mode_cost);
    VvcLumaModeRdCandidate {
        distortion: score.distortion,
        rate_cost: score.rate_cost,
        residual,
    }
}

#[derive(Debug, Clone, Copy)]
struct VvcLumaModeRdShortlist {
    candidates: [VvcLumaIntraCandidateCost; VVC_LUMA_LOSSY_RD_SHORTLIST_SIZE],
    count: usize,
}

impl VvcLumaModeRdShortlist {
    fn from_candidate_costs(costs: VvcLumaIntraCandidateCosts) -> Self {
        let mut shortlist = Self {
            candidates: [VvcLumaIntraCandidateCost::new(VvcIntraPredictionMode::Dc, u64::MAX);
                VVC_LUMA_LOSSY_RD_SHORTLIST_SIZE],
            count: 0,
        };
        for candidate in costs.iter() {
            shortlist.add(candidate);
        }
        shortlist
    }

    fn add(&mut self, candidate: VvcLumaIntraCandidateCost) {
        if let Some(existing) =
            self.candidates.iter().take(self.count).position(|entry| {
                entry.mode().luma_mode_index() == candidate.mode().luma_mode_index()
            })
        {
            if candidate.score() < self.candidates[existing].score() {
                self.candidates[existing] = candidate;
                self.sort();
            }
            return;
        }
        if self.count < self.candidates.len() {
            self.candidates[self.count] = candidate;
            self.count += 1;
            self.sort();
            return;
        }
        let worst = self.count - 1;
        if candidate.score() < self.candidates[worst].score() {
            self.candidates[worst] = candidate;
            self.sort();
        }
    }

    fn sort(&mut self) {
        self.candidates[..self.count].sort_by_key(|candidate| candidate.score());
    }

    fn iter(self) -> impl Iterator<Item = VvcLumaIntraCandidateCost> {
        self.candidates.into_iter().take(self.count)
    }
}

fn select_vvc_chroma_mode_with_rd_refinement(
    policy: VvcResidualCodingPolicy,
    node: VvcCodingTreeNode,
    raw_mode: VvcChromaIntraPredictionMode,
    candidate_costs: VvcChromaIntraCandidateCosts,
    co_located_luma_mode: VvcIntraPredictionMode,
    cclm_syntax_enabled: bool,
    source_frame: &VvcSampledFrame,
    frame_recon: &VvcReconstructionFrame,
    chroma_width: usize,
    chroma_height: usize,
    chroma_qp: i32,
    prediction_scratch: &mut VvcDcPredictionScratch,
    selected_cb_prediction: &mut Vec<VvcSample>,
    selected_cr_prediction: &mut Vec<VvcSample>,
    selected_cb_residuals: &mut Vec<i16>,
    selected_cr_residuals: &mut Vec<i16>,
    candidate_cb_prediction: &mut Vec<VvcSample>,
    candidate_cr_prediction: &mut Vec<VvcSample>,
    candidate_cb_residuals: &mut Vec<i16>,
    candidate_cr_residuals: &mut Vec<i16>,
    transform_scratch: &mut VvcInverseTransformScratch,
    reconstructed_residual: &mut Vec<i16>,
) -> VvcSelectedChromaMode {
    let raw_decision = policy.select_chroma_tu_coding_decision(node, raw_mode);
    if !vvc_chroma_lossy_rd_refinement_allowed(policy, node, raw_decision) {
        return VvcSelectedChromaMode {
            mode: raw_mode,
            residual: None,
        };
    }

    let mut best_mode = raw_mode;
    let mut best_candidate = score_vvc_chroma_mode_rd_candidate(
        raw_decision,
        raw_mode,
        cclm_syntax_enabled,
        selected_cb_residuals,
        selected_cr_residuals,
        chroma_width,
        chroma_height,
        source_frame.format.bit_depth,
        chroma_qp,
        transform_scratch,
        reconstructed_residual,
    );
    let shortlist = VvcChromaModeRdShortlist::from_candidate_costs(candidate_costs);
    for candidate in shortlist.iter() {
        let mode = candidate.mode();
        if mode == raw_mode {
            continue;
        }
        let coding_decision = policy.select_chroma_tu_coding_decision(node, mode);
        if !matches!(
            coding_decision.residual_coding,
            VvcTuResidualCodingMode::Transformed
        ) {
            continue;
        }
        predict_vvc_chroma_mode_block_into_with_availability(
            candidate_cb_prediction,
            prediction_scratch,
            mode,
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
            candidate_cr_prediction,
            prediction_scratch,
            mode,
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
        let chroma_x =
            usize::from(node.x) / chroma_subsample_x(source_frame.format.chroma_sampling);
        let chroma_y =
            usize::from(node.y) / chroma_subsample_y(source_frame.format.chroma_sampling);
        residual_chroma_tu_at_into(
            candidate_cb_residuals,
            &source_frame.cb,
            source_frame.geometry,
            source_frame.format,
            chroma_x,
            chroma_y,
            chroma_width,
            chroma_height,
            candidate_cb_prediction,
        );
        residual_chroma_tu_at_into(
            candidate_cr_residuals,
            &source_frame.cr,
            source_frame.geometry,
            source_frame.format,
            chroma_x,
            chroma_y,
            chroma_width,
            chroma_height,
            candidate_cr_prediction,
        );
        let rd_candidate = score_vvc_chroma_mode_rd_candidate(
            coding_decision,
            mode,
            cclm_syntax_enabled,
            candidate_cb_residuals,
            candidate_cr_residuals,
            chroma_width,
            chroma_height,
            source_frame.format.bit_depth,
            chroma_qp,
            transform_scratch,
            reconstructed_residual,
        );
        if rd_candidate.selects_over(best_candidate) {
            best_mode = mode;
            best_candidate = rd_candidate;
            std::mem::swap(selected_cb_prediction, candidate_cb_prediction);
            std::mem::swap(selected_cr_prediction, candidate_cr_prediction);
            std::mem::swap(selected_cb_residuals, candidate_cb_residuals);
            std::mem::swap(selected_cr_residuals, candidate_cr_residuals);
        }
    }

    VvcSelectedChromaMode {
        mode: best_mode,
        residual: Some(best_candidate.residual),
    }
}

fn vvc_chroma_lossy_rd_refinement_allowed(
    policy: VvcResidualCodingPolicy,
    node: VvcCodingTreeNode,
    decision: VvcChromaTuCodingDecision,
) -> bool {
    policy.residual_mode() == VvcResidualCodingMode::Lossy
        && matches!(
            decision.residual_coding,
            VvcTuResidualCodingMode::Transformed
        )
        && [4, 8, 16, 32].contains(&node.width)
        && [4, 8, 16, 32].contains(&node.height)
}

#[derive(Debug, Clone, Copy)]
struct VvcSelectedChromaMode {
    mode: VvcChromaIntraPredictionMode,
    residual: Option<VvcSelectedChromaResidual>,
}

fn select_vvc_chroma_bdpcm_prediction(
    policy: VvcResidualCodingPolicy,
    node: VvcCodingTreeNode,
    selected_mode: VvcChromaIntraPredictionMode,
    cclm_syntax_enabled: bool,
    source_frame: &VvcSampledFrame,
    frame_recon: &VvcReconstructionFrame,
    chroma_width: usize,
    chroma_height: usize,
    chroma_qp: i32,
    selected_residual: Option<VvcSelectedChromaResidual>,
    prediction_scratch: &mut VvcDcPredictionScratch,
    selected_cb_prediction: &mut Vec<VvcSample>,
    selected_cr_prediction: &mut Vec<VvcSample>,
    selected_cb_residuals: &mut Vec<i16>,
    selected_cr_residuals: &mut Vec<i16>,
    candidate_cb_prediction: &mut Vec<VvcSample>,
    candidate_cr_prediction: &mut Vec<VvcSample>,
    candidate_cb_residuals: &mut Vec<i16>,
    candidate_cr_residuals: &mut Vec<i16>,
    transform_scratch: &mut VvcInverseTransformScratch,
    reconstructed_residual: &mut Vec<i16>,
) -> Option<VvcSelectedChromaBdpcm> {
    if !vvc_chroma_bdpcm_selection_allowed(policy, chroma_width, chroma_height) {
        return None;
    }

    let baseline_decision = policy.select_chroma_tu_coding_decision(node, selected_mode);
    let baseline_residual = selected_residual.unwrap_or_else(|| VvcSelectedChromaResidual {
        cb: finalize_vvc_chroma_residual_block(
            baseline_decision.residual_coding,
            selected_cb_residuals,
            chroma_width,
            chroma_height,
            source_frame.format.bit_depth,
            chroma_qp,
        ),
        cr: finalize_vvc_chroma_residual_block(
            baseline_decision.residual_coding,
            selected_cr_residuals,
            chroma_width,
            chroma_height,
            source_frame.format.bit_depth,
            chroma_qp,
        ),
    });
    let mut best_score = vvc_chroma_quantized_residual_score(
        selected_cb_residuals,
        selected_cr_residuals,
        chroma_width,
        chroma_height,
        source_frame.format.bit_depth,
        chroma_qp,
        baseline_residual,
        u64::from(vvc_bdpcm_mode_syntax_bin_count(VvcBdpcmMode::None)).saturating_add(u64::from(
            vvc_chroma_intra_mode_syntax_bin_count(selected_mode, cclm_syntax_enabled),
        )),
        transform_scratch,
        reconstructed_residual,
    );
    let mut best = None;

    for bdpcm_mode in [VvcBdpcmMode::Horizontal, VvcBdpcmMode::Vertical] {
        predict_vvc_chroma_bdpcm_block_into_with_availability(
            candidate_cb_prediction,
            prediction_scratch,
            bdpcm_mode,
            &frame_recon.cb,
            source_frame.geometry,
            node,
            source_frame.format.chroma_sampling,
            source_frame.format.bit_depth,
            Some(frame_recon.cb_availability()),
        );
        predict_vvc_chroma_bdpcm_block_into_with_availability(
            candidate_cr_prediction,
            prediction_scratch,
            bdpcm_mode,
            &frame_recon.cr,
            source_frame.geometry,
            node,
            source_frame.format.chroma_sampling,
            source_frame.format.bit_depth,
            Some(frame_recon.cr_availability()),
        );
        let chroma_x =
            usize::from(node.x) / chroma_subsample_x(source_frame.format.chroma_sampling);
        let chroma_y =
            usize::from(node.y) / chroma_subsample_y(source_frame.format.chroma_sampling);
        residual_chroma_tu_at_into(
            candidate_cb_residuals,
            &source_frame.cb,
            source_frame.geometry,
            source_frame.format,
            chroma_x,
            chroma_y,
            chroma_width,
            chroma_height,
            candidate_cb_prediction,
        );
        residual_chroma_tu_at_into(
            candidate_cr_residuals,
            &source_frame.cr,
            source_frame.geometry,
            source_frame.format,
            chroma_x,
            chroma_y,
            chroma_width,
            chroma_height,
            candidate_cr_prediction,
        );
        let residual = VvcSelectedChromaResidual {
            cb: finalize_vvc_chroma_bdpcm_transform_skip_residual_block(
                candidate_cb_residuals,
                chroma_width,
                chroma_height,
                source_frame.format.bit_depth,
                chroma_qp,
                bdpcm_mode,
            ),
            cr: finalize_vvc_chroma_bdpcm_transform_skip_residual_block(
                candidate_cr_residuals,
                chroma_width,
                chroma_height,
                source_frame.format.bit_depth,
                chroma_qp,
                bdpcm_mode,
            ),
        };
        let candidate_score = vvc_chroma_quantized_residual_score(
            candidate_cb_residuals,
            candidate_cr_residuals,
            chroma_width,
            chroma_height,
            source_frame.format.bit_depth,
            chroma_qp,
            residual,
            u64::from(vvc_bdpcm_mode_syntax_bin_count(bdpcm_mode)),
            transform_scratch,
            reconstructed_residual,
        );
        if candidate_score.selects_over(best_score) {
            best_score = candidate_score;
            let mode = VvcChromaIntraPredictionMode::Explicit(
                bdpcm_mode
                    .inferred_intra_mode()
                    .expect("enabled BDPCM mode has an inferred intra mode"),
            );
            best = Some(VvcSelectedChromaBdpcm { mode, residual });
            std::mem::swap(selected_cb_prediction, candidate_cb_prediction);
            std::mem::swap(selected_cr_prediction, candidate_cr_prediction);
            std::mem::swap(selected_cb_residuals, candidate_cb_residuals);
            std::mem::swap(selected_cr_residuals, candidate_cr_residuals);
        }
    }

    best
}

fn vvc_chroma_bdpcm_selection_allowed(
    policy: VvcResidualCodingPolicy,
    chroma_width: usize,
    chroma_height: usize,
) -> bool {
    VVC_ENABLE_BDPCM_SELECTION
        && chroma_width <= usize::from(VVC_TRANSFORM_SKIP_MAX_SIZE)
        && chroma_height <= usize::from(VVC_TRANSFORM_SKIP_MAX_SIZE)
        && chroma_width >= 4
        && chroma_height >= 4
        && matches!(
            policy.residual_mode(),
            VvcResidualCodingMode::Lossy | VvcResidualCodingMode::Lossless
        )
}

#[derive(Debug, Clone, Copy)]
struct VvcSelectedChromaBdpcm {
    mode: VvcChromaIntraPredictionMode,
    residual: VvcSelectedChromaResidual,
}

#[derive(Debug, Clone, Copy)]
struct VvcChromaModeRdCandidate {
    distortion: u64,
    rate_cost: u64,
    residual: VvcSelectedChromaResidual,
}

impl VvcChromaModeRdCandidate {
    fn selects_over(self, best: Self) -> bool {
        (self.rate_cost < best.rate_cost && self.distortion <= best.distortion)
            || (self.rate_cost <= best.rate_cost && self.distortion < best.distortion)
    }
}

fn score_vvc_chroma_mode_rd_candidate(
    coding_decision: VvcChromaTuCodingDecision,
    mode: VvcChromaIntraPredictionMode,
    cclm_syntax_enabled: bool,
    cb_residuals: &[i16],
    cr_residuals: &[i16],
    chroma_width: usize,
    chroma_height: usize,
    bit_depth: SampleBitDepth,
    chroma_qp: i32,
    transform_scratch: &mut VvcInverseTransformScratch,
    reconstructed_residual: &mut Vec<i16>,
) -> VvcChromaModeRdCandidate {
    let residual = VvcSelectedChromaResidual {
        cb: finalize_vvc_chroma_residual_block(
            coding_decision.residual_coding,
            cb_residuals,
            chroma_width,
            chroma_height,
            bit_depth,
            chroma_qp,
        ),
        cr: finalize_vvc_chroma_residual_block(
            coding_decision.residual_coding,
            cr_residuals,
            chroma_width,
            chroma_height,
            bit_depth,
            chroma_qp,
        ),
    };
    let mode_cost = u64::from(vvc_chroma_intra_mode_syntax_bin_count(
        mode,
        cclm_syntax_enabled,
    ));
    let score = vvc_chroma_quantized_residual_score(
        cb_residuals,
        cr_residuals,
        chroma_width,
        chroma_height,
        bit_depth,
        chroma_qp,
        residual,
        mode_cost,
        transform_scratch,
        reconstructed_residual,
    );
    VvcChromaModeRdCandidate {
        distortion: score.distortion,
        rate_cost: score.rate_cost,
        residual,
    }
}

fn vvc_chroma_quantized_residual_score(
    cb_residuals: &[i16],
    cr_residuals: &[i16],
    chroma_width: usize,
    chroma_height: usize,
    bit_depth: SampleBitDepth,
    chroma_qp: i32,
    residual: VvcSelectedChromaResidual,
    extra_syntax_cost: u64,
    transform_scratch: &mut VvcInverseTransformScratch,
    reconstructed_residual: &mut Vec<i16>,
) -> VvcChromaQuantizedResidualScore {
    let cb_sse = chroma_reconstructed_residual_sse(
        cb_residuals,
        chroma_width,
        chroma_height,
        bit_depth,
        chroma_qp,
        residual.cb,
        transform_scratch,
        reconstructed_residual,
    );
    let cr_sse = chroma_reconstructed_residual_sse(
        cr_residuals,
        chroma_width,
        chroma_height,
        bit_depth,
        chroma_qp,
        residual.cr,
        transform_scratch,
        reconstructed_residual,
    );
    let distortion = cb_sse.saturating_add(cr_sse);
    let coefficient_cost =
        chroma_coeff_syntax_cost_estimate(chroma_width, chroma_height, residual.cb).saturating_add(
            chroma_coeff_syntax_cost_estimate(chroma_width, chroma_height, residual.cr),
        );
    let rate_cost = coefficient_cost.saturating_add(extra_syntax_cost);
    VvcChromaQuantizedResidualScore {
        distortion,
        rate_cost,
    }
}

fn chroma_reconstructed_residual_sse(
    source_residuals: &[i16],
    width: usize,
    height: usize,
    bit_depth: SampleBitDepth,
    chroma_qp: i32,
    residual: VvcFinalizedResidualBlock<VVC_CHROMA_AC_COEFFS_PER_TU>,
    transform_scratch: &mut VvcInverseTransformScratch,
    reconstructed_residual: &mut Vec<i16>,
) -> u64 {
    reconstruct_vvc_chroma_residual_block_into(
        residual,
        reconstructed_residual,
        transform_scratch,
        width,
        height,
        bit_depth,
        chroma_qp,
    );
    source_residuals
        .iter()
        .zip(reconstructed_residual.iter())
        .map(|(source, reconstructed)| {
            let diff = i64::from(*source) - i64::from(*reconstructed);
            (diff * diff) as u64
        })
        .sum()
}

fn chroma_coeff_syntax_cost_estimate(
    width: usize,
    height: usize,
    residual: VvcFinalizedResidualBlock<VVC_CHROMA_AC_COEFFS_PER_TU>,
) -> u64 {
    let mut nonzero = u64::from(residual.dc_level != 0);
    let mut abs_sum = u64::from(residual.dc_level.unsigned_abs());
    let mut last_pos = 0u64;
    let active_width = width.min(4);
    let active_height = height.min(4);
    for (slot, (x, y)) in VVC_CHROMA_AC_POSITIONS_4X4.iter().copied().enumerate() {
        if x >= active_width || y >= active_height {
            continue;
        }
        let abs_level = u64::from(residual.ac_levels[slot].unsigned_abs());
        if abs_level != 0 {
            nonzero += 1;
            abs_sum += abs_level;
            last_pos = (y * active_width + x) as u64;
        }
    }
    nonzero
        .saturating_mul(18)
        .saturating_add(abs_sum.saturating_mul(4))
        .saturating_add(last_pos.saturating_mul(2))
}

#[derive(Debug, Clone, Copy)]
struct VvcChromaQuantizedResidualScore {
    distortion: u64,
    rate_cost: u64,
}

impl VvcChromaQuantizedResidualScore {
    fn selects_over(self, best: Self) -> bool {
        (self.rate_cost < best.rate_cost && self.distortion <= best.distortion)
            || (self.rate_cost <= best.rate_cost && self.distortion < best.distortion)
    }
}

#[derive(Debug, Clone, Copy)]
struct VvcChromaModeRdShortlist {
    candidates: [VvcChromaIntraCandidateCost; VVC_CHROMA_LOSSY_RD_SHORTLIST_SIZE],
    count: usize,
}

impl VvcChromaModeRdShortlist {
    fn from_candidate_costs(costs: VvcChromaIntraCandidateCosts) -> Self {
        let mut shortlist = Self {
            candidates: [VvcChromaIntraCandidateCost::new(
                VvcChromaIntraPredictionMode::Derived,
                u64::MAX,
            ); VVC_CHROMA_LOSSY_RD_SHORTLIST_SIZE],
            count: 0,
        };
        for candidate in costs.iter() {
            shortlist.add(candidate);
        }
        shortlist
    }

    fn add(&mut self, candidate: VvcChromaIntraCandidateCost) {
        if let Some(existing) = self
            .candidates
            .iter()
            .take(self.count)
            .position(|entry| entry.mode() == candidate.mode())
        {
            if candidate.score() < self.candidates[existing].score() {
                self.candidates[existing] = candidate;
                self.sort();
            }
            return;
        }
        if self.count < self.candidates.len() {
            self.candidates[self.count] = candidate;
            self.count += 1;
            self.sort();
            return;
        }
        let worst = self.count - 1;
        if candidate.score() < self.candidates[worst].score() {
            self.candidates[worst] = candidate;
            self.sort();
        }
    }

    fn sort(&mut self) {
        self.candidates[..self.count].sort_by_key(|candidate| candidate.score());
    }

    fn iter(self) -> impl Iterator<Item = VvcChromaIntraCandidateCost> {
        self.candidates.into_iter().take(self.count)
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
    requested_mts_index: u8,
    node: VvcCodingTreeNode,
    mode: VvcIntraPredictionMode,
    left: Option<VvcIntraPredictionMode>,
    above: Option<VvcIntraPredictionMode>,
    luma_qp: i32,
    preselected_residual: Option<VvcSelectedLumaResidual>,
    frame_recon: &VvcReconstructionFrame,
    source_frame: &VvcSampledFrame,
    prediction_scratch: &mut VvcDcPredictionScratch,
    selected_prediction: &mut Vec<VvcSample>,
    selected_residuals: &mut Vec<i16>,
    candidate_prediction: &mut Vec<VvcSample>,
    candidate_residuals: &mut Vec<i16>,
) -> VvcSelectedLumaMrl {
    if !VVC_ENABLE_LUMA_MRL_SELECTION
        || !policy.luma_mrl_candidate_allowed(node, mode)
        || !vvc_luma_intra_mode_is_mpm(mode, left, above)
    {
        return VvcSelectedLumaMrl {
            mrl_index: 0,
            residual: preselected_residual,
        };
    }

    let score_metric = policy.score_metric();
    let mut best_mrl_index = 0u8;
    let mut best_candidate = preselected_residual.map_or_else(
        || {
            score_vvc_luma_mrl_candidate(
                score_metric,
                residual_coding,
                requested_mts_index,
                node,
                0,
                selected_residuals,
                source_frame.format.bit_depth,
                luma_qp,
            )
        },
        |residual| VvcLumaMrlCandidate {
            score: vvc_luma_quantized_residual_score(
                selected_residuals,
                node,
                source_frame.format.bit_depth,
                luma_qp,
                residual,
                u64::from(vvc_luma_mrl_syntax_bin_count(node, 0)),
            )
            .score,
            residual: Some(residual),
        },
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
        let candidate = score_vvc_luma_mrl_candidate(
            score_metric,
            residual_coding,
            requested_mts_index,
            node,
            mrl_index,
            candidate_residuals,
            source_frame.format.bit_depth,
            luma_qp,
        );
        if candidate.score < best_candidate.score {
            best_candidate = candidate;
            best_mrl_index = mrl_index;
            std::mem::swap(selected_prediction, candidate_prediction);
            std::mem::swap(selected_residuals, candidate_residuals);
        }
    }
    VvcSelectedLumaMrl {
        mrl_index: best_mrl_index,
        residual: best_candidate.residual,
    }
}

fn select_vvc_luma_bdpcm_prediction(
    policy: VvcResidualCodingPolicy,
    node: VvcCodingTreeNode,
    selected_mode: VvcIntraPredictionMode,
    selected_decision: VvcLumaTuCodingDecision,
    left: Option<VvcIntraPredictionMode>,
    above: Option<VvcIntraPredictionMode>,
    luma_qp: i32,
    selected_residual: Option<VvcSelectedLumaResidual>,
    frame_recon: &VvcReconstructionFrame,
    source_frame: &VvcSampledFrame,
    prediction_scratch: &mut VvcDcPredictionScratch,
    selected_prediction: &mut Vec<VvcSample>,
    selected_residuals: &mut Vec<i16>,
    candidate_prediction: &mut Vec<VvcSample>,
    candidate_residuals: &mut Vec<i16>,
) -> Option<VvcSelectedLumaBdpcm> {
    if !vvc_luma_bdpcm_selection_allowed(policy, node) {
        return None;
    }

    let baseline_residual = selected_residual.unwrap_or_else(|| {
        let (block, mts_index) = select_vvc_luma_residual_block_with_mts(
            selected_decision.residual_coding,
            selected_decision.mts_index,
            selected_residuals,
            node.width,
            node.height,
            source_frame.format.bit_depth,
            luma_qp,
        );
        VvcSelectedLumaResidual { block, mts_index }
    });
    let mut best_score = vvc_luma_quantized_residual_score(
        selected_residuals,
        node,
        source_frame.format.bit_depth,
        luma_qp,
        baseline_residual,
        vvc_luma_regular_prediction_syntax_cost(
            node,
            selected_mode,
            left,
            above,
            selected_decision,
        ),
    );
    let mut best = None;

    for bdpcm_mode in [VvcBdpcmMode::Horizontal, VvcBdpcmMode::Vertical] {
        predict_vvc_luma_bdpcm_block_into_with_availability(
            candidate_prediction,
            prediction_scratch,
            bdpcm_mode,
            &frame_recon.luma,
            source_frame.geometry,
            node,
            source_frame.format.bit_depth,
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
        let residual = VvcSelectedLumaResidual {
            block: finalize_vvc_luma_bdpcm_transform_skip_residual_block(
                candidate_residuals,
                node.width,
                node.height,
                source_frame.format.bit_depth,
                luma_qp,
                bdpcm_mode,
            ),
            mts_index: 0,
        };
        let candidate_score = vvc_luma_quantized_residual_score(
            candidate_residuals,
            node,
            source_frame.format.bit_depth,
            luma_qp,
            residual,
            u64::from(vvc_bdpcm_mode_syntax_bin_count(bdpcm_mode)),
        );
        if candidate_score.selects_over(best_score) {
            best_score = candidate_score;
            let mode = bdpcm_mode
                .inferred_intra_mode()
                .expect("enabled BDPCM mode has an inferred intra mode");
            best = Some(VvcSelectedLumaBdpcm {
                mode,
                coding_decision: VvcLumaTuCodingDecision {
                    residual_coding: VvcTuResidualCodingMode::TransformSkip,
                    mrl_index: 0,
                    mts_index: 0,
                },
                residual,
            });
            std::mem::swap(selected_prediction, candidate_prediction);
            std::mem::swap(selected_residuals, candidate_residuals);
        }
    }

    best
}

fn vvc_luma_bdpcm_selection_allowed(
    policy: VvcResidualCodingPolicy,
    node: VvcCodingTreeNode,
) -> bool {
    VVC_ENABLE_BDPCM_SELECTION
        && node.width <= VVC_TRANSFORM_SKIP_MAX_SIZE
        && node.height <= VVC_TRANSFORM_SKIP_MAX_SIZE
        && node.width >= 4
        && node.height >= 4
        && node.width.is_power_of_two()
        && node.height.is_power_of_two()
        && matches!(
            policy.residual_mode(),
            VvcResidualCodingMode::Lossy | VvcResidualCodingMode::Lossless
        )
}

fn vvc_luma_regular_prediction_syntax_cost(
    node: VvcCodingTreeNode,
    mode: VvcIntraPredictionMode,
    left: Option<VvcIntraPredictionMode>,
    above: Option<VvcIntraPredictionMode>,
    decision: VvcLumaTuCodingDecision,
) -> u64 {
    u64::from(vvc_bdpcm_mode_syntax_bin_count(VvcBdpcmMode::None))
        .saturating_add(u64::from(vvc_luma_mrl_syntax_bin_count(
            node,
            decision.mrl_index,
        )))
        .saturating_add(u64::from(vvc_luma_intra_mode_syntax_bin_count(
            mode, left, above,
        )))
}

fn vvc_bdpcm_mode_syntax_bin_count(mode: VvcBdpcmMode) -> u8 {
    match mode {
        VvcBdpcmMode::None => 1,
        VvcBdpcmMode::Horizontal | VvcBdpcmMode::Vertical => 2,
    }
}

#[derive(Debug, Clone, Copy)]
struct VvcSelectedLumaBdpcm {
    mode: VvcIntraPredictionMode,
    coding_decision: VvcLumaTuCodingDecision,
    residual: VvcSelectedLumaResidual,
}

#[derive(Debug, Clone, Copy, Default)]
struct VvcSelectedLumaMrl {
    mrl_index: u8,
    residual: Option<VvcSelectedLumaResidual>,
}

#[derive(Debug, Clone, Copy)]
struct VvcLumaMrlCandidate {
    score: u64,
    residual: Option<VvcSelectedLumaResidual>,
}

fn score_vvc_luma_mrl_candidate(
    metric: VvcResidualScoreMetric,
    residual_coding: VvcTuResidualCodingMode,
    requested_mts_index: u8,
    node: VvcCodingTreeNode,
    mrl_index: u8,
    residuals: &[i16],
    bit_depth: SampleBitDepth,
    luma_qp: i32,
) -> VvcLumaMrlCandidate {
    const SYNTAX_TIE_BREAKER_SCALE: u64 = 64;
    if matches!(residual_coding, VvcTuResidualCodingMode::Transformed) {
        let (residual, mts_index) = select_vvc_luma_residual_block_with_mts(
            residual_coding,
            requested_mts_index,
            residuals,
            node.width,
            node.height,
            bit_depth,
            luma_qp,
        );
        let mrl_cost = u64::from(vvc_luma_mrl_syntax_bin_count(node, mrl_index));
        let selected_residual = VvcSelectedLumaResidual {
            block: residual,
            mts_index,
        };
        return VvcLumaMrlCandidate {
            score: vvc_luma_quantized_residual_score(
                residuals,
                node,
                bit_depth,
                luma_qp,
                selected_residual,
                mrl_cost,
            )
            .score,
            residual: Some(selected_residual),
        };
    }
    VvcLumaMrlCandidate {
        score: residual_mode_selection_score(metric, residuals)
            .saturating_mul(SYNTAX_TIE_BREAKER_SCALE)
            .saturating_add(u64::from(vvc_luma_mrl_syntax_bin_count(node, mrl_index))),
        residual: None,
    }
}

fn vvc_luma_quantized_residual_score(
    residuals: &[i16],
    node: VvcCodingTreeNode,
    bit_depth: SampleBitDepth,
    luma_qp: i32,
    residual: VvcSelectedLumaResidual,
    extra_syntax_cost: u64,
) -> VvcLumaQuantizedResidualScore {
    let block = residual.block;
    let sse = luma_reconstructed_residual_sse(
        residuals,
        node.width,
        node.height,
        bit_depth,
        luma_qp,
        block,
        residual.mts_index,
    );
    let lambda = luma_coeff_rd_lambda(luma_qp, bit_depth);
    let coefficient_cost = u64::from(block.dc_level != 0)
        .saturating_mul(8)
        .saturating_add(luma_ac_syntax_cost_estimate(
            node.width,
            node.height,
            &block.ac_levels,
        ))
        .saturating_add(luma_mts_syntax_cost_estimate(
            block.has_ac && !block.transform_skip,
            residual.mts_index,
        ))
        .saturating_add(u64::from(block.transform_skip));
    let rate_cost = coefficient_cost.saturating_add(extra_syntax_cost);
    VvcLumaQuantizedResidualScore {
        score: sse.saturating_add(lambda.saturating_mul(rate_cost)),
        distortion: sse,
        rate_cost,
    }
}

#[derive(Debug, Clone, Copy)]
struct VvcLumaQuantizedResidualScore {
    score: u64,
    distortion: u64,
    rate_cost: u64,
}

impl VvcLumaQuantizedResidualScore {
    fn selects_over(self, best: Self) -> bool {
        (self.rate_cost < best.rate_cost && self.distortion <= best.distortion)
            || (self.rate_cost <= best.rate_cost && self.distortion < best.distortion)
    }
}

fn luma_reconstructed_residual_sse(
    source_residuals: &[i16],
    width: u16,
    height: u16,
    bit_depth: SampleBitDepth,
    qp: i32,
    residual: VvcFinalizedResidualBlock<VVC_LUMA_AC_COEFFS_PER_TU>,
    mts_index: u8,
) -> u64 {
    if residual.transform_skip {
        let mut reconstructed = Vec::new();
        reconstruct_vvc_luma_transform_skip_residuals_into_with_qp(
            &mut reconstructed,
            residual.dc_level,
            &residual.ac_levels,
            usize::from(width),
            usize::from(height),
            bit_depth,
            qp,
        );
        return source_residuals
            .iter()
            .zip(reconstructed.iter())
            .map(|(source, reconstructed)| {
                let diff = i64::from(*source) - i64::from(*reconstructed);
                (diff * diff) as u64
            })
            .sum();
    }
    luma_reconstructed_residual_sse_with_mts(
        source_residuals,
        width,
        height,
        bit_depth,
        qp,
        residual.dc_level,
        &residual.ac_levels,
        mts_index,
    )
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
        if node.y % VVC_CTU_SIZE as u16 == 0 {
            return None;
        }
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
    bdpcm_mode: VvcBdpcmMode,
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
struct VvcSelectedLumaResidual {
    block: VvcFinalizedResidualBlock<VVC_LUMA_AC_COEFFS_PER_TU>,
    mts_index: u8,
}

#[derive(Debug, Clone, Copy)]
struct VvcFinalizedLumaTu {
    abs_remainder: u8,
    negative: bool,
    dc_level: i16,
    ac_levels: [i16; VVC_LUMA_AC_COEFFS_PER_TU],
    has_ac: bool,
    transform_skip: bool,
    bdpcm_mode: VvcBdpcmMode,
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
    preselected_residual: Option<VvcSelectedLumaResidual>,
    transform_scratch: &mut VvcInverseTransformScratch,
    reconstructed_residual: &mut Vec<i16>,
) -> VvcFinalizedLumaTu {
    let selected_residual = preselected_residual.unwrap_or_else(|| {
        let (block, mts_index) = select_vvc_luma_residual_block_with_mts(
            coding_decision.residual_coding,
            coding_decision.mts_index,
            residuals,
            node.width,
            node.height,
            source_frame.format.bit_depth,
            luma_qp,
        );
        VvcSelectedLumaResidual { block, mts_index }
    });
    let residual = selected_residual.block;
    let mts_index = selected_residual.mts_index;
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
        bdpcm_mode: residual.bdpcm_mode,
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
        0,
        residuals,
        width,
        height,
        bit_depth,
        luma_qp,
    );
    if let Some(transform_skip) = select_vvc_luma_transform_skip_candidate(
        residual_coding,
        residuals,
        width,
        height,
        bit_depth,
        luma_qp,
        base,
        0,
    ) {
        return transform_skip;
    }
    let Some(candidate_mts_index) = select_vvc_luma_mts_candidate_index(
        residual_coding,
        requested_mts_index,
        residuals,
        width,
        height,
        base,
    ) else {
        return select_vvc_luma_residual_block_with_transform_skip(
            residual_coding,
            residuals,
            width,
            height,
            bit_depth,
            luma_qp,
            base,
            0,
        );
    };

    let mut best_residual = base;
    let mut best_mts_index = 0u8;
    let candidate = finalize_vvc_luma_residual_block(
        residual_coding,
        candidate_mts_index,
        residuals,
        width,
        height,
        bit_depth,
        luma_qp,
    );
    if candidate.has_ac {
        let base_score =
            vvc_luma_residual_block_score(residuals, width, height, bit_depth, luma_qp, base, 0);
        let candidate_score = vvc_luma_residual_block_score(
            residuals,
            width,
            height,
            bit_depth,
            luma_qp,
            candidate,
            candidate_mts_index,
        );
        if candidate_score.selects_over(base_score) {
            best_residual = candidate;
            best_mts_index = candidate_mts_index;
        }
    }

    (best_residual, best_mts_index)
}

fn select_vvc_luma_residual_block_with_transform_skip(
    residual_coding: VvcTuResidualCodingMode,
    residuals: &[i16],
    width: u16,
    height: u16,
    bit_depth: SampleBitDepth,
    luma_qp: i32,
    transformed: VvcFinalizedResidualBlock<VVC_LUMA_AC_COEFFS_PER_TU>,
    mts_index: u8,
) -> (VvcFinalizedResidualBlock<VVC_LUMA_AC_COEFFS_PER_TU>, u8) {
    select_vvc_luma_transform_skip_candidate(
        residual_coding,
        residuals,
        width,
        height,
        bit_depth,
        luma_qp,
        transformed,
        mts_index,
    )
    .unwrap_or((transformed, mts_index))
}

fn select_vvc_luma_transform_skip_candidate(
    residual_coding: VvcTuResidualCodingMode,
    residuals: &[i16],
    width: u16,
    height: u16,
    bit_depth: SampleBitDepth,
    luma_qp: i32,
    transformed: VvcFinalizedResidualBlock<VVC_LUMA_AC_COEFFS_PER_TU>,
    mts_index: u8,
) -> Option<(VvcFinalizedResidualBlock<VVC_LUMA_AC_COEFFS_PER_TU>, u8)> {
    if !vvc_luma_lossy_transform_skip_selection_allowed(residual_coding, width, height, luma_qp) {
        return None;
    }

    let transform_skip = finalize_vvc_luma_transform_skip_residual_block(
        residuals, width, height, bit_depth, luma_qp,
    );
    if !transform_skip.has_ac && transform_skip.dc_level == 0 {
        return None;
    }

    let transformed_score = vvc_luma_residual_block_score(
        residuals,
        width,
        height,
        bit_depth,
        luma_qp,
        transformed,
        mts_index,
    );
    let transform_skip_score = vvc_luma_residual_block_score(
        residuals,
        width,
        height,
        bit_depth,
        luma_qp,
        transform_skip,
        0,
    );
    if transform_skip_score.selects_over(transformed_score) {
        Some((transform_skip, 0))
    } else {
        None
    }
}

fn vvc_luma_lossy_transform_skip_selection_allowed(
    residual_coding: VvcTuResidualCodingMode,
    width: u16,
    height: u16,
    luma_qp: i32,
) -> bool {
    VVC_ENABLE_LOSSY_TRANSFORM_SKIP_SELECTION
        && matches!(residual_coding, VvcTuResidualCodingMode::Transformed)
        && luma_qp > 0
        && width <= VVC_TRANSFORM_SKIP_MAX_SIZE
        && height <= VVC_TRANSFORM_SKIP_MAX_SIZE
}

fn vvc_luma_residual_block_score(
    source_residuals: &[i16],
    width: u16,
    height: u16,
    bit_depth: SampleBitDepth,
    qp: i32,
    residual: VvcFinalizedResidualBlock<VVC_LUMA_AC_COEFFS_PER_TU>,
    mts_index: u8,
) -> VvcResidualBlockScore {
    let distortion = luma_reconstructed_residual_sse(
        source_residuals,
        width,
        height,
        bit_depth,
        qp,
        residual,
        mts_index,
    );
    let rate_cost = u64::from(residual.dc_level != 0)
        .saturating_mul(8)
        .saturating_add(luma_ac_syntax_cost_estimate(
            width,
            height,
            &residual.ac_levels,
        ))
        .saturating_add(luma_mts_syntax_cost_estimate(
            residual.has_ac && !residual.transform_skip,
            mts_index,
        ))
        .saturating_add(u64::from(residual.transform_skip));
    VvcResidualBlockScore {
        distortion,
        rate_cost,
    }
}

#[derive(Debug, Clone, Copy)]
struct VvcResidualBlockScore {
    distortion: u64,
    rate_cost: u64,
}

impl VvcResidualBlockScore {
    fn selects_over(self, best: Self) -> bool {
        (self.rate_cost < best.rate_cost && self.distortion <= best.distortion)
            || (self.rate_cost <= best.rate_cost && self.distortion < best.distortion)
    }
}

fn select_vvc_luma_mts_candidate_index(
    residual_coding: VvcTuResidualCodingMode,
    requested_mts_index: u8,
    residuals: &[i16],
    width: u16,
    height: u16,
    base: VvcFinalizedResidualBlock<VVC_LUMA_AC_COEFFS_PER_TU>,
) -> Option<u8> {
    if !VVC_ENABLE_LUMA_MTS_SELECTION
        || !matches!(residual_coding, VvcTuResidualCodingMode::Transformed)
        || width != 8
        || height != 8
        || !base.has_ac
    {
        return None;
    }
    if matches!(requested_mts_index, 2..=5) {
        return Some(requested_mts_index);
    }
    if requested_mts_index != 0 {
        return None;
    }

    let (horizontal_gradient, vertical_gradient) =
        luma_residual_directional_gradients(residuals, usize::from(width), usize::from(height));
    let gradient_floor = residuals
        .iter()
        .map(|sample| i64::from(*sample).unsigned_abs())
        .sum::<u64>()
        / 16;
    if horizontal_gradient.saturating_add(vertical_gradient) <= gradient_floor {
        return None;
    }
    if horizontal_gradient > vertical_gradient.saturating_mul(2) {
        Some(3)
    } else if vertical_gradient > horizontal_gradient.saturating_mul(2) {
        Some(4)
    } else {
        Some(2)
    }
}

fn luma_residual_directional_gradients(
    residuals: &[i16],
    width: usize,
    height: usize,
) -> (u64, u64) {
    debug_assert_eq!(residuals.len(), width * height);
    let mut horizontal = 0u64;
    let mut vertical = 0u64;
    for y in 0..height {
        for x in 1..width {
            horizontal = horizontal.saturating_add(
                i32::from(residuals[y * width + x])
                    .saturating_sub(i32::from(residuals[y * width + x - 1]))
                    .unsigned_abs() as u64,
            );
        }
    }
    for y in 1..height {
        for x in 0..width {
            vertical = vertical.saturating_add(
                i32::from(residuals[y * width + x])
                    .saturating_sub(i32::from(residuals[(y - 1) * width + x]))
                    .unsigned_abs() as u64,
            );
        }
    }
    (horizontal, vertical)
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
            finalize_vvc_luma_transform_skip_residual_block(
                residuals, width, height, bit_depth, luma_qp,
            )
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
                bdpcm_mode: VvcBdpcmMode::None,
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
        if residual.bdpcm_mode.is_enabled() {
            reconstruct_vvc_luma_bdpcm_transform_skip_residuals_into_with_qp(
                reconstructed_residual,
                residual.dc_level,
                &residual.ac_levels,
                usize::from(width),
                usize::from(height),
                bit_depth,
                luma_qp,
                residual.bdpcm_mode,
            );
        } else {
            reconstruct_vvc_luma_transform_skip_residuals_into_with_qp(
                reconstructed_residual,
                residual.dc_level,
                &residual.ac_levels,
                usize::from(width),
                usize::from(height),
                bit_depth,
                luma_qp,
            );
        }
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

fn finalize_vvc_luma_transform_skip_residual_block(
    residuals: &[i16],
    width: u16,
    height: u16,
    bit_depth: SampleBitDepth,
    luma_qp: i32,
) -> VvcFinalizedResidualBlock<VVC_LUMA_AC_COEFFS_PER_TU> {
    debug_assert_eq!(residuals.len(), usize::from(width) * usize::from(height));
    let dc_level = residuals
        .first()
        .copied()
        .map(|level| quantize_vvc_transform_skip_level(level, bit_depth, luma_qp))
        .unwrap_or(0);
    let (ac_levels, has_ac) = transform_skip_luma_ac_levels_and_flag_with_qp(
        residuals,
        usize::from(width),
        bit_depth,
        luma_qp,
    );
    VvcFinalizedResidualBlock {
        dc_level,
        ac_levels,
        has_ac,
        transform_skip: true,
        bdpcm_mode: VvcBdpcmMode::None,
    }
}

fn finalize_vvc_luma_bdpcm_transform_skip_residual_block(
    residuals: &[i16],
    width: u16,
    height: u16,
    bit_depth: SampleBitDepth,
    luma_qp: i32,
    bdpcm_mode: VvcBdpcmMode,
) -> VvcFinalizedResidualBlock<VVC_LUMA_AC_COEFFS_PER_TU> {
    debug_assert!(bdpcm_mode.is_enabled());
    debug_assert_eq!(residuals.len(), usize::from(width) * usize::from(height));
    let (active_width, active_height) =
        vvc_luma_transform_skip_active_extent(usize::from(width), usize::from(height));
    let mut quantized_levels = [0i16; 64];
    let mut ac_levels = [0; VVC_LUMA_AC_COEFFS_PER_TU];
    let mut dc_level = 0i16;
    let mut has_ac = false;
    for y in 0..active_height {
        for x in 0..active_width {
            let level = quantize_vvc_transform_skip_level(
                residuals[y * usize::from(width) + x],
                bit_depth,
                luma_qp,
            );
            quantized_levels[y * active_width + x] = level;
            let predictor = match bdpcm_mode {
                VvcBdpcmMode::None => unreachable!("BDPCM block requires a direction"),
                VvcBdpcmMode::Horizontal if x > 0 => quantized_levels[y * active_width + x - 1],
                VvcBdpcmMode::Vertical if y > 0 => quantized_levels[(y - 1) * active_width + x],
                VvcBdpcmMode::Horizontal | VvcBdpcmMode::Vertical => 0,
            };
            let coeff = (i32::from(level) - i32::from(predictor))
                .clamp(i32::from(i16::MIN), i32::from(i16::MAX)) as i16;
            if x == 0 && y == 0 {
                dc_level = coeff;
            } else {
                ac_levels[y * active_width + x - 1] = coeff;
                has_ac |= coeff != 0;
            }
        }
    }
    VvcFinalizedResidualBlock {
        dc_level,
        ac_levels,
        has_ac,
        transform_skip: true,
        bdpcm_mode,
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
    bdpcm_mode: VvcBdpcmMode,
}

#[derive(Debug, Clone, Copy)]
struct VvcSelectedChromaResidual {
    cb: VvcFinalizedResidualBlock<VVC_CHROMA_AC_COEFFS_PER_TU>,
    cr: VvcFinalizedResidualBlock<VVC_CHROMA_AC_COEFFS_PER_TU>,
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
    preselected_residual: Option<VvcSelectedChromaResidual>,
    transform_scratch: &mut VvcInverseTransformScratch,
    reconstructed_residual: &mut Vec<i16>,
) -> VvcFinalizedChromaTu {
    let selected_residual = preselected_residual.unwrap_or_else(|| VvcSelectedChromaResidual {
        cb: finalize_vvc_chroma_residual_block(
            coding_decision.residual_coding,
            cb_residuals,
            chroma_width,
            chroma_height,
            source_frame.format.bit_depth,
            chroma_qp,
        ),
        cr: finalize_vvc_chroma_residual_block(
            coding_decision.residual_coding,
            cr_residuals,
            chroma_width,
            chroma_height,
            source_frame.format.bit_depth,
            chroma_qp,
        ),
    });
    let cb_residual = selected_residual.cb;
    let cr_residual = selected_residual.cr;
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
        bdpcm_mode: cb_residual
            .bdpcm_mode
            .is_enabled()
            .then_some(cb_residual.bdpcm_mode)
            .unwrap_or(cr_residual.bdpcm_mode),
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
            finalize_vvc_chroma_transform_skip_residual_block(
                residuals, width, height, bit_depth, chroma_qp,
            )
        }
        VvcTuResidualCodingMode::Transformed => {
            let quantized = quantize_vvc_chroma_residual_greedy_with_qp(
                residuals,
                width as u16,
                height as u16,
                bit_depth,
                chroma_qp,
            );
            let transformed = VvcFinalizedResidualBlock {
                dc_level: quantized.reconstructed_dc_coeff,
                ac_levels: quantized.reconstructed_ac_coeffs,
                has_ac: quantized.has_ac,
                transform_skip: false,
                bdpcm_mode: VvcBdpcmMode::None,
            };
            select_vvc_chroma_residual_block_with_transform_skip(
                residual_coding,
                residuals,
                width,
                height,
                bit_depth,
                chroma_qp,
                transformed,
            )
        }
    }
}

fn select_vvc_chroma_residual_block_with_transform_skip(
    residual_coding: VvcTuResidualCodingMode,
    residuals: &[i16],
    width: usize,
    height: usize,
    bit_depth: SampleBitDepth,
    chroma_qp: i32,
    transformed: VvcFinalizedResidualBlock<VVC_CHROMA_AC_COEFFS_PER_TU>,
) -> VvcFinalizedResidualBlock<VVC_CHROMA_AC_COEFFS_PER_TU> {
    if !vvc_chroma_lossy_transform_skip_selection_allowed(residual_coding, width, height, chroma_qp)
    {
        return transformed;
    }

    let transform_skip = finalize_vvc_chroma_transform_skip_residual_block(
        residuals, width, height, bit_depth, chroma_qp,
    );
    if !transform_skip.has_ac && transform_skip.dc_level == 0 {
        return transformed;
    }

    let transformed_score = vvc_chroma_residual_block_score(
        residuals,
        width,
        height,
        bit_depth,
        chroma_qp,
        transformed,
    );
    let transform_skip_score = vvc_chroma_residual_block_score(
        residuals,
        width,
        height,
        bit_depth,
        chroma_qp,
        transform_skip,
    );
    if transform_skip_score.selects_over(transformed_score) {
        transform_skip
    } else {
        transformed
    }
}

fn vvc_chroma_lossy_transform_skip_selection_allowed(
    residual_coding: VvcTuResidualCodingMode,
    width: usize,
    height: usize,
    chroma_qp: i32,
) -> bool {
    VVC_ENABLE_LOSSY_TRANSFORM_SKIP_SELECTION
        && matches!(residual_coding, VvcTuResidualCodingMode::Transformed)
        && chroma_qp > 0
        && width <= usize::from(VVC_TRANSFORM_SKIP_MAX_SIZE)
        && height <= usize::from(VVC_TRANSFORM_SKIP_MAX_SIZE)
}

fn vvc_chroma_residual_block_score(
    source_residuals: &[i16],
    width: usize,
    height: usize,
    bit_depth: SampleBitDepth,
    qp: i32,
    residual: VvcFinalizedResidualBlock<VVC_CHROMA_AC_COEFFS_PER_TU>,
) -> VvcResidualBlockScore {
    let mut scratch = VvcInverseTransformScratch::default();
    let mut reconstructed = Vec::new();
    let distortion = chroma_reconstructed_residual_sse(
        source_residuals,
        width,
        height,
        bit_depth,
        qp,
        residual,
        &mut scratch,
        &mut reconstructed,
    );
    let rate_cost = u64::from(residual.dc_level != 0)
        .saturating_mul(8)
        .saturating_add(chroma_coeff_syntax_cost_estimate(width, height, residual))
        .saturating_add(u64::from(residual.transform_skip));
    VvcResidualBlockScore {
        distortion,
        rate_cost,
    }
}

fn finalize_vvc_chroma_transform_skip_residual_block(
    residuals: &[i16],
    width: usize,
    height: usize,
    bit_depth: SampleBitDepth,
    chroma_qp: i32,
) -> VvcFinalizedResidualBlock<VVC_CHROMA_AC_COEFFS_PER_TU> {
    debug_assert_eq!(residuals.len(), width * height);
    let dc_level = residuals
        .first()
        .copied()
        .map(|level| quantize_vvc_transform_skip_level(level, bit_depth, chroma_qp))
        .unwrap_or(0);
    let (ac_levels, has_ac) =
        transform_skip_chroma_ac_levels_and_flag_with_qp(residuals, width, bit_depth, chroma_qp);
    VvcFinalizedResidualBlock {
        dc_level,
        ac_levels,
        has_ac,
        transform_skip: true,
        bdpcm_mode: VvcBdpcmMode::None,
    }
}

fn finalize_vvc_chroma_bdpcm_transform_skip_residual_block(
    residuals: &[i16],
    width: usize,
    height: usize,
    bit_depth: SampleBitDepth,
    chroma_qp: i32,
    bdpcm_mode: VvcBdpcmMode,
) -> VvcFinalizedResidualBlock<VVC_CHROMA_AC_COEFFS_PER_TU> {
    debug_assert!(bdpcm_mode.is_enabled());
    debug_assert_eq!(residuals.len(), width * height);
    let active_width = width.min(4);
    let active_height = height.min(4);
    let mut quantized_levels = [0i16; 16];
    let mut ac_levels = [0; VVC_CHROMA_AC_COEFFS_PER_TU];
    let mut dc_level = 0i16;
    let mut has_ac = false;
    for y in 0..active_height {
        for x in 0..active_width {
            let level =
                quantize_vvc_transform_skip_level(residuals[y * width + x], bit_depth, chroma_qp);
            quantized_levels[y * 4 + x] = level;
            let predictor = match bdpcm_mode {
                VvcBdpcmMode::None => unreachable!("BDPCM block requires a direction"),
                VvcBdpcmMode::Horizontal if x > 0 => quantized_levels[y * 4 + x - 1],
                VvcBdpcmMode::Vertical if y > 0 => quantized_levels[(y - 1) * 4 + x],
                VvcBdpcmMode::Horizontal | VvcBdpcmMode::Vertical => 0,
            };
            let coeff = (i32::from(level) - i32::from(predictor))
                .clamp(i32::from(i16::MIN), i32::from(i16::MAX)) as i16;
            if x == 0 && y == 0 {
                dc_level = coeff;
            } else {
                let slot = y * 4 + x - 1;
                ac_levels[slot] = coeff;
                has_ac |= coeff != 0;
            }
        }
    }
    VvcFinalizedResidualBlock {
        dc_level,
        ac_levels,
        has_ac,
        transform_skip: true,
        bdpcm_mode,
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
        if residual.bdpcm_mode.is_enabled() {
            reconstruct_vvc_chroma_bdpcm_transform_skip_residuals_into_with_qp(
                reconstructed_residual,
                residual.dc_level,
                &residual.ac_levels,
                width,
                height,
                bit_depth,
                chroma_qp,
                residual.bdpcm_mode,
            );
        } else {
            reconstruct_vvc_chroma_transform_skip_residuals_into_with_qp(
                reconstructed_residual,
                residual.dc_level,
                &residual.ac_levels,
                width,
                height,
                bit_depth,
                chroma_qp,
            );
        }
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

#[cfg(test)]
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

pub(in crate::vvc) fn reconstruct_vvc_luma_transform_skip_residuals_into_with_qp(
    residuals: &mut Vec<i16>,
    dc_level: i16,
    ac_levels: &[i16; super::VVC_LUMA_AC_COEFFS_PER_TU],
    width: usize,
    height: usize,
    bit_depth: SampleBitDepth,
    qp: i32,
) {
    residuals.clear();
    residuals.resize(width * height, 0);
    if residuals.is_empty() {
        return;
    }
    residuals[0] = reconstruct_vvc_transform_skip_level(dc_level, bit_depth, qp);
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
            residuals[y * width + x] = reconstruct_vvc_transform_skip_level(
                ac_levels[y * active_width + x - 1],
                bit_depth,
                qp,
            );
        }
    }
}

pub(in crate::vvc) fn reconstruct_vvc_luma_bdpcm_transform_skip_residuals_into_with_qp(
    residuals: &mut Vec<i16>,
    dc_level: i16,
    ac_levels: &[i16; super::VVC_LUMA_AC_COEFFS_PER_TU],
    width: usize,
    height: usize,
    bit_depth: SampleBitDepth,
    qp: i32,
    bdpcm_mode: VvcBdpcmMode,
) {
    debug_assert!(bdpcm_mode.is_enabled());
    residuals.clear();
    residuals.resize(width * height, 0);
    if residuals.is_empty() {
        return;
    }
    let (active_width, active_height) = vvc_luma_transform_skip_active_extent(width, height);
    let mut levels = [0i16; 64];
    levels[0] = dc_level;
    for y in 0..active_height {
        for x in 0..active_width {
            if x == 0 && y == 0 {
                continue;
            }
            levels[y * active_width + x] = ac_levels[y * active_width + x - 1];
        }
    }
    inverse_bdpcm_quantized_levels_in_place(&mut levels, active_width, active_height, bdpcm_mode);
    for y in 0..active_height {
        for x in 0..active_width {
            residuals[y * width + x] =
                reconstruct_vvc_transform_skip_level(levels[y * active_width + x], bit_depth, qp);
        }
    }
}

#[cfg(test)]
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

pub(in crate::vvc) fn reconstruct_vvc_chroma_transform_skip_residuals_into_with_qp(
    residuals: &mut Vec<i16>,
    dc_level: i16,
    ac_levels: &[i16; VVC_CHROMA_AC_COEFFS_PER_TU],
    width: usize,
    height: usize,
    bit_depth: SampleBitDepth,
    qp: i32,
) {
    residuals.clear();
    residuals.resize(width * height, 0);
    if residuals.is_empty() {
        return;
    }
    residuals[0] = reconstruct_vvc_transform_skip_level(dc_level, bit_depth, qp);
    for (slot, (x, y)) in VVC_CHROMA_AC_POSITIONS_4X4.iter().copied().enumerate() {
        if x < width && y < height {
            residuals[y * width + x] =
                reconstruct_vvc_transform_skip_level(ac_levels[slot], bit_depth, qp);
        }
    }
}

pub(in crate::vvc) fn reconstruct_vvc_chroma_bdpcm_transform_skip_residuals_into_with_qp(
    residuals: &mut Vec<i16>,
    dc_level: i16,
    ac_levels: &[i16; VVC_CHROMA_AC_COEFFS_PER_TU],
    width: usize,
    height: usize,
    bit_depth: SampleBitDepth,
    qp: i32,
    bdpcm_mode: VvcBdpcmMode,
) {
    debug_assert!(bdpcm_mode.is_enabled());
    residuals.clear();
    residuals.resize(width * height, 0);
    if residuals.is_empty() {
        return;
    }
    let active_width = width.min(4);
    let active_height = height.min(4);
    let mut levels = [0i16; 16];
    levels[0] = dc_level;
    for (slot, (x, y)) in VVC_CHROMA_AC_POSITIONS_4X4.iter().copied().enumerate() {
        if x < active_width && y < active_height {
            levels[y * 4 + x] = ac_levels[slot];
        }
    }
    inverse_bdpcm_quantized_levels_in_place(&mut levels, 4, active_height, bdpcm_mode);
    for y in 0..active_height {
        for x in 0..active_width {
            residuals[y * width + x] =
                reconstruct_vvc_transform_skip_level(levels[y * 4 + x], bit_depth, qp);
        }
    }
}

#[cfg(test)]
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

pub(in crate::vvc) fn transform_skip_luma_ac_levels_and_flag_with_qp(
    residuals: &[i16],
    width: usize,
    bit_depth: SampleBitDepth,
    qp: i32,
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
                let level = quantize_vvc_transform_skip_level(residuals[raster_idx], bit_depth, qp);
                levels[y * active_width + x - 1] = level;
                has_ac |= level != 0;
            }
        }
    }
    (levels, has_ac)
}

fn vvc_luma_transform_skip_active_extent(width: usize, height: usize) -> (usize, usize) {
    if width == 8 && height == 8 {
        (8, 8)
    } else {
        (width.min(4), height.min(4))
    }
}

fn inverse_bdpcm_quantized_levels_in_place(
    levels: &mut [i16],
    stride: usize,
    height: usize,
    bdpcm_mode: VvcBdpcmMode,
) {
    match bdpcm_mode {
        VvcBdpcmMode::None => unreachable!("BDPCM inverse requires a direction"),
        VvcBdpcmMode::Horizontal => {
            for y in 0..height {
                let row = y * stride;
                for x in 1..stride {
                    let idx = row + x;
                    levels[idx] = (i32::from(levels[idx]) + i32::from(levels[idx - 1]))
                        .clamp(i32::from(i16::MIN), i32::from(i16::MAX))
                        as i16;
                }
            }
        }
        VvcBdpcmMode::Vertical => {
            for y in 1..height {
                let row = y * stride;
                let above = row - stride;
                for x in 0..stride {
                    let idx = row + x;
                    levels[idx] = (i32::from(levels[idx]) + i32::from(levels[above + x]))
                        .clamp(i32::from(i16::MIN), i32::from(i16::MAX))
                        as i16;
                }
            }
        }
    }
}

#[cfg(test)]
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

pub(in crate::vvc) fn transform_skip_chroma_ac_levels_and_flag_with_qp(
    residuals: &[i16],
    width: usize,
    bit_depth: SampleBitDepth,
    qp: i32,
) -> ([i16; VVC_CHROMA_AC_COEFFS_PER_TU], bool) {
    let mut levels = [0; VVC_CHROMA_AC_COEFFS_PER_TU];
    let mut has_ac = false;
    for (slot, (x, y)) in VVC_CHROMA_AC_POSITIONS_4X4.iter().copied().enumerate() {
        let raster_idx = y * width + x;
        if raster_idx < residuals.len() {
            let level = quantize_vvc_transform_skip_level(residuals[raster_idx], bit_depth, qp);
            levels[slot] = level;
            has_ac |= level != 0;
        }
    }
    (levels, has_ac)
}

pub(in crate::vvc) fn quantize_vvc_transform_skip_level(
    residual: i16,
    bit_depth: SampleBitDepth,
    qp: i32,
) -> i16 {
    if residual == 0 {
        return 0;
    }
    let (scale, right_shift) = vvc_transform_skip_dequant_params(bit_depth, qp);
    let estimate = if right_shift > 0 {
        div_round_nearest_i64(i64::from(residual) << right_shift, i64::from(scale))
    } else {
        div_round_nearest_i64(
            i64::from(residual),
            i64::from(scale) << (-right_shift as u32),
        )
    };
    let mut best_level = estimate.clamp(i64::from(i16::MIN), i64::from(i16::MAX)) as i16;
    let mut best_error = vvc_transform_skip_level_error(best_level, residual, bit_depth, qp);
    for candidate in (estimate - 2)..=(estimate + 2) {
        let level = candidate.clamp(i64::from(i16::MIN), i64::from(i16::MAX)) as i16;
        let error = vvc_transform_skip_level_error(level, residual, bit_depth, qp);
        if error < best_error
            || (error == best_error && level.unsigned_abs() < best_level.unsigned_abs())
        {
            best_error = error;
            best_level = level;
        }
    }
    best_level
}

fn div_round_nearest_i64(value: i64, divisor: i64) -> i64 {
    debug_assert!(divisor > 0);
    if value < 0 {
        -(((-value) + (divisor / 2)) / divisor)
    } else {
        (value + (divisor / 2)) / divisor
    }
}

fn vvc_transform_skip_level_error(
    level: i16,
    residual: i16,
    bit_depth: SampleBitDepth,
    qp: i32,
) -> u64 {
    let reconstructed = reconstruct_vvc_transform_skip_level(level, bit_depth, qp);
    let diff = i64::from(residual) - i64::from(reconstructed);
    (diff * diff) as u64
}

fn reconstruct_vvc_transform_skip_level(level: i16, bit_depth: SampleBitDepth, qp: i32) -> i16 {
    if level == 0 {
        return 0;
    }
    let (scale, right_shift) = vvc_transform_skip_dequant_params(bit_depth, qp);
    let value = if right_shift > 0 {
        let add = 1i64 << ((right_shift - 1) as u32);
        (i64::from(level) * i64::from(scale) + add) >> (right_shift as u32)
    } else {
        i64::from(level) * i64::from(scale) * (1i64 << ((-right_shift) as u32))
    };
    value.clamp(i64::from(i16::MIN), i64::from(i16::MAX)) as i16
}

fn vvc_transform_skip_dequant_params(bit_depth: SampleBitDepth, qp: i32) -> (i32, i32) {
    let qp_bd_offset = (i32::from(bit_depth.bits()) - 8) * 6;
    let transform_skip_qp = (qp + qp_bd_offset).max(4);
    let qp_rem = transform_skip_qp.rem_euclid(6) as usize;
    let qp_per = transform_skip_qp.div_euclid(6);
    let scale = VVC_TRANSFORM_SKIP_INV_QUANT_SCALES[qp_rem];
    let right_shift = 6 - qp_per;
    (scale, right_shift)
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
