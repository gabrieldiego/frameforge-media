use super::super::{
    chroma_subsample_x, chroma_subsample_y, vvc_chroma_transform_nodes, vvc_downshift_sample_to_u8,
    vvc_luma_transform_nodes, vvc_neutral_sample, VvcCodingTreeNode, VvcCtuPartitionShape,
    VvcPictureFormat, VvcSample, VvcSampledColor, VvcSampledFrame, VvcVideoGeometry, VVC_CTU_SIZE,
    VVC_CURRENT_MAX_LUMA_LEAF_SIZE, VVC_LOSSLESS_LUMA_LEAF_SIZE,
};
use super::{
    fill_visible_chroma_node, fill_visible_luma_node,
    inverse_transform_vvc_chroma_quantized_block_into,
    inverse_transform_vvc_luma_quantized_block_into, predict_vvc_chroma_dc_block_into,
    predict_vvc_luma_dc_block_into, quantize_vvc_chroma_residual_greedy,
    quantize_vvc_chroma_sample, quantize_vvc_luma_residual_greedy, reconstruct_vvc_chroma,
    VvcDcPredictionScratch, VvcInverseTransformScratch, VvcQuantizedColor,
    VvcQuantizedResidualFrame, MAX_VVC_CHROMA_TUS, MAX_VVC_LUMA_TUS, VVC_CHROMA_AC_COEFFS_PER_TU,
    VVC_CHROMA_AC_POSITIONS_4X4,
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
    let mut luma_tu_remainders = [0; MAX_VVC_LUMA_TUS];
    let mut luma_tu_negative = [false; MAX_VVC_LUMA_TUS];
    let mut luma_tu_dc_levels = [0; MAX_VVC_LUMA_TUS];
    let mut luma_tu_ac_levels = [[0; super::VVC_LUMA_AC_COEFFS_PER_TU]; MAX_VVC_LUMA_TUS];
    let neutral = vvc_neutral_sample(frame.format.bit_depth);
    let mut reconstructed_luma = vec![neutral; frame.geometry.luma_samples()];
    let mut luma_tu_count = 0;
    let mut cb_tu_dc_levels = [0; MAX_VVC_CHROMA_TUS];
    let mut cr_tu_dc_levels = [0; MAX_VVC_CHROMA_TUS];
    let mut chroma_tu_count = 0;
    let mut cb_tu_ac_levels = [[0; VVC_CHROMA_AC_COEFFS_PER_TU]; MAX_VVC_CHROMA_TUS];
    let mut cr_tu_ac_levels = [[0; VVC_CHROMA_AC_COEFFS_PER_TU]; MAX_VVC_CHROMA_TUS];
    let mut reconstructed_cb = vec![neutral; frame.chroma_len];
    let mut reconstructed_cr = vec![neutral; frame.chroma_len];
    let mut prediction_scratch = VvcDcPredictionScratch::default();
    let mut predicted_luma = Vec::new();
    let mut predicted_cb = Vec::new();
    let mut predicted_cr = Vec::new();
    let mut transform_scratch = VvcInverseTransformScratch::default();
    let mut reconstructed_residual = Vec::new();

    for node in vvc_luma_tu_nodes(frame.geometry, frame.format.chroma_sampling) {
        if luma_tu_count >= MAX_VVC_LUMA_TUS {
            break;
        }
        predict_vvc_luma_dc_block_into(
            &mut predicted_luma,
            &mut prediction_scratch,
            &reconstructed_luma,
            frame.geometry,
            node,
            frame.format.bit_depth,
        );
        let residuals = residual_luma_tu_at(
            frame,
            usize::from(node.x),
            usize::from(node.y),
            usize::from(node.width),
            usize::from(node.height),
            &predicted_luma,
        );
        let quantized = quantize_vvc_luma_residual_greedy(
            &residuals,
            node.width,
            node.height,
            frame.format.bit_depth,
        );
        luma_tu_remainders[luma_tu_count] = quantized.abs_remainder;
        luma_tu_negative[luma_tu_count] =
            quantized.reconstructed_dc_coeff < 0 && quantized.abs_remainder != 0;
        luma_tu_dc_levels[luma_tu_count] = quantized.reconstructed_dc_coeff;
        luma_tu_ac_levels[luma_tu_count] = quantized.reconstructed_ac_coeffs;
        inverse_transform_vvc_luma_quantized_block_into(
            &mut reconstructed_residual,
            &mut transform_scratch,
            node.width,
            node.height,
            quantized.reconstructed_dc_coeff,
            &quantized.reconstructed_ac_coeffs,
            frame.format.bit_depth,
        );
        fill_visible_luma_node(
            &mut reconstructed_luma,
            frame.geometry,
            node,
            &predicted_luma,
            &reconstructed_residual,
            frame.format.bit_depth,
        );
        luma_tu_count += 1;
    }

    for node in vvc_chroma_transform_nodes(chroma_partition_shape(
        frame.geometry,
        frame.format.chroma_sampling,
    )) {
        if chroma_tu_count >= MAX_VVC_CHROMA_TUS {
            break;
        }
        let subsample_x = chroma_subsample_x(frame.format.chroma_sampling);
        let subsample_y = chroma_subsample_y(frame.format.chroma_sampling);
        let chroma_x = usize::from(node.x) / subsample_x;
        let chroma_y = usize::from(node.y) / subsample_y;
        let chroma_width = usize::from(node.width) / subsample_x;
        let chroma_height = usize::from(node.height) / subsample_y;
        predict_vvc_chroma_dc_block_into(
            &mut predicted_cb,
            &mut prediction_scratch,
            &reconstructed_cb,
            frame.geometry,
            node,
            frame.format.chroma_sampling,
            frame.format.bit_depth,
        );
        predict_vvc_chroma_dc_block_into(
            &mut predicted_cr,
            &mut prediction_scratch,
            &reconstructed_cr,
            frame.geometry,
            node,
            frame.format.chroma_sampling,
            frame.format.bit_depth,
        );
        let cb_residuals = residual_chroma_tu_at(
            &frame.cb,
            frame.geometry,
            frame.format,
            chroma_x,
            chroma_y,
            chroma_width,
            chroma_height,
            &predicted_cb,
        );
        let cr_residuals = residual_chroma_tu_at(
            &frame.cr,
            frame.geometry,
            frame.format,
            chroma_x,
            chroma_y,
            chroma_width,
            chroma_height,
            &predicted_cr,
        );
        let cb_quantized = quantize_vvc_chroma_residual_greedy(
            &cb_residuals,
            chroma_width as u16,
            chroma_height as u16,
            frame.format.bit_depth,
        );
        let cr_quantized = quantize_vvc_chroma_residual_greedy(
            &cr_residuals,
            chroma_width as u16,
            chroma_height as u16,
            frame.format.bit_depth,
        );
        cb_tu_dc_levels[chroma_tu_count] = cb_quantized.reconstructed_dc_coeff;
        cr_tu_dc_levels[chroma_tu_count] = cr_quantized.reconstructed_dc_coeff;
        cb_tu_ac_levels[chroma_tu_count] = cb_quantized.reconstructed_ac_coeffs;
        cr_tu_ac_levels[chroma_tu_count] = cr_quantized.reconstructed_ac_coeffs;
        inverse_transform_vvc_chroma_quantized_block_into(
            &mut reconstructed_residual,
            &mut transform_scratch,
            chroma_width as u16,
            chroma_height as u16,
            cb_quantized.reconstructed_dc_coeff,
            &cb_quantized.reconstructed_ac_coeffs,
            frame.format.bit_depth,
        );
        fill_visible_chroma_node(
            &mut reconstructed_cb,
            frame.geometry,
            node,
            frame.format.chroma_sampling,
            &predicted_cb,
            &reconstructed_residual,
            frame.format.bit_depth,
        );
        inverse_transform_vvc_chroma_quantized_block_into(
            &mut reconstructed_residual,
            &mut transform_scratch,
            chroma_width as u16,
            chroma_height as u16,
            cr_quantized.reconstructed_dc_coeff,
            &cr_quantized.reconstructed_ac_coeffs,
            frame.format.bit_depth,
        );
        fill_visible_chroma_node(
            &mut reconstructed_cr,
            frame.geometry,
            node,
            frame.format.chroma_sampling,
            &predicted_cr,
            &reconstructed_residual,
            frame.format.bit_depth,
        );
        chroma_tu_count += 1;
    }

    let color = frame.sampled_color();
    let cb_rem =
        quantize_vvc_chroma_sample(vvc_downshift_sample_to_u8(color.u, frame.format.bit_depth));
    let cr_rem =
        quantize_vvc_chroma_sample(vvc_downshift_sample_to_u8(color.v, frame.format.bit_depth));
    let quantized_cb = reconstruct_vvc_chroma(cb_rem);
    let quantized_cr = reconstruct_vvc_chroma(cr_rem);
    let mut reconstruction_yuv =
        Vec::with_capacity(frame.geometry.luma_samples() + frame.chroma_len * 2);
    reconstruction_yuv.extend_from_slice(&reconstructed_luma);
    reconstruction_yuv.extend_from_slice(&reconstructed_cb);
    reconstruction_yuv.extend_from_slice(&reconstructed_cr);
    VvcQuantizedResidualFrame {
        quantized: VvcQuantizedColor {
            y: reconstructed_luma
                .first()
                .copied()
                .map(|sample| vvc_downshift_sample_to_u8(sample, frame.format.bit_depth))
                .unwrap_or(128),
            u: quantized_cb,
            v: quantized_cr,
            luma_tu_remainders,
            luma_tu_negative,
            luma_tu_dc_levels,
            luma_tu_ac_levels,
            luma_tu_count,
            chroma_tu_count,
            cb_tu_dc_levels,
            cr_tu_dc_levels,
            cb_tu_ac_levels,
            cr_tu_ac_levels,
            cb_rem,
            cr_rem,
        },
        reconstruction_yuv,
    }
}

pub(in crate::vvc) fn quantize_vvc_frame_lossless_residual(
    frame: &VvcSampledFrame,
) -> VvcQuantizedColor {
    debug_assert!(matches!(
        frame.format.chroma_sampling,
        crate::picture::ChromaSampling::Cs420 | crate::picture::ChromaSampling::Cs422
    ));
    let mut luma_tu_remainders = [0; MAX_VVC_LUMA_TUS];
    let mut luma_tu_negative = [false; MAX_VVC_LUMA_TUS];
    let mut luma_tu_dc_levels = [0; MAX_VVC_LUMA_TUS];
    let mut luma_tu_ac_levels = [[0; super::VVC_LUMA_AC_COEFFS_PER_TU]; MAX_VVC_LUMA_TUS];
    let mut cb_tu_dc_levels = [0; MAX_VVC_CHROMA_TUS];
    let mut cr_tu_dc_levels = [0; MAX_VVC_CHROMA_TUS];
    let mut cb_tu_ac_levels = [[0; VVC_CHROMA_AC_COEFFS_PER_TU]; MAX_VVC_CHROMA_TUS];
    let mut cr_tu_ac_levels = [[0; VVC_CHROMA_AC_COEFFS_PER_TU]; MAX_VVC_CHROMA_TUS];
    let neutral = vvc_neutral_sample(frame.format.bit_depth);
    let mut reconstructed_luma = vec![neutral; frame.geometry.luma_samples()];
    let mut reconstructed_cb = vec![neutral; frame.chroma_len];
    let mut reconstructed_cr = vec![neutral; frame.chroma_len];
    let mut prediction_scratch = VvcDcPredictionScratch::default();
    let mut predicted_luma = Vec::new();
    let mut predicted_cb = Vec::new();
    let mut predicted_cr = Vec::new();

    let mut luma_tu_count = 0;
    for node in vvc_luma_tu_nodes_with_leaf_size(
        frame.geometry,
        frame.format.chroma_sampling,
        VVC_LOSSLESS_LUMA_LEAF_SIZE,
    ) {
        if luma_tu_count >= MAX_VVC_LUMA_TUS {
            break;
        }
        predict_vvc_luma_dc_block_into(
            &mut predicted_luma,
            &mut prediction_scratch,
            &reconstructed_luma,
            frame.geometry,
            node,
            frame.format.bit_depth,
        );
        let residuals = residual_luma_tu_at(
            frame,
            usize::from(node.x),
            usize::from(node.y),
            usize::from(node.width),
            usize::from(node.height),
            &predicted_luma,
        );
        let dc_level = residuals.first().copied().unwrap_or(0);
        luma_tu_remainders[luma_tu_count] = dc_level.unsigned_abs().min(u8::MAX as u16) as u8;
        luma_tu_negative[luma_tu_count] = dc_level < 0;
        luma_tu_dc_levels[luma_tu_count] = dc_level;
        luma_tu_ac_levels[luma_tu_count] =
            lossless_luma_ac_levels(&residuals, usize::from(node.width));
        fill_visible_luma_node(
            &mut reconstructed_luma,
            frame.geometry,
            node,
            &predicted_luma,
            &residuals,
            frame.format.bit_depth,
        );
        luma_tu_count += 1;
    }

    let mut chroma_tu_count = 0;
    for node in vvc_chroma_transform_nodes(chroma_partition_shape(
        frame.geometry,
        frame.format.chroma_sampling,
    )) {
        if chroma_tu_count >= MAX_VVC_CHROMA_TUS {
            break;
        }
        let subsample_x = chroma_subsample_x(frame.format.chroma_sampling);
        let subsample_y = chroma_subsample_y(frame.format.chroma_sampling);
        let chroma_x = usize::from(node.x) / subsample_x;
        let chroma_y = usize::from(node.y) / subsample_y;
        let chroma_width = usize::from(node.width) / subsample_x;
        let chroma_height = usize::from(node.height) / subsample_y;
        predict_vvc_chroma_dc_block_into(
            &mut predicted_cb,
            &mut prediction_scratch,
            &reconstructed_cb,
            frame.geometry,
            node,
            frame.format.chroma_sampling,
            frame.format.bit_depth,
        );
        predict_vvc_chroma_dc_block_into(
            &mut predicted_cr,
            &mut prediction_scratch,
            &reconstructed_cr,
            frame.geometry,
            node,
            frame.format.chroma_sampling,
            frame.format.bit_depth,
        );
        let cb_residuals = residual_chroma_tu_at(
            &frame.cb,
            frame.geometry,
            frame.format,
            chroma_x,
            chroma_y,
            chroma_width,
            chroma_height,
            &predicted_cb,
        );
        let cr_residuals = residual_chroma_tu_at(
            &frame.cr,
            frame.geometry,
            frame.format,
            chroma_x,
            chroma_y,
            chroma_width,
            chroma_height,
            &predicted_cr,
        );
        cb_tu_dc_levels[chroma_tu_count] = cb_residuals.first().copied().unwrap_or(0);
        cr_tu_dc_levels[chroma_tu_count] = cr_residuals.first().copied().unwrap_or(0);
        cb_tu_ac_levels[chroma_tu_count] = lossless_chroma_ac_levels(&cb_residuals, chroma_width);
        cr_tu_ac_levels[chroma_tu_count] = lossless_chroma_ac_levels(&cr_residuals, chroma_width);
        fill_visible_chroma_node(
            &mut reconstructed_cb,
            frame.geometry,
            node,
            frame.format.chroma_sampling,
            &predicted_cb,
            &cb_residuals,
            frame.format.bit_depth,
        );
        fill_visible_chroma_node(
            &mut reconstructed_cr,
            frame.geometry,
            node,
            frame.format.chroma_sampling,
            &predicted_cr,
            &cr_residuals,
            frame.format.bit_depth,
        );
        chroma_tu_count += 1;
    }

    let color = frame.sampled_color();
    let cb_rem =
        quantize_vvc_chroma_sample(vvc_downshift_sample_to_u8(color.u, frame.format.bit_depth));
    let cr_rem =
        quantize_vvc_chroma_sample(vvc_downshift_sample_to_u8(color.v, frame.format.bit_depth));
    VvcQuantizedColor {
        y: vvc_downshift_sample_to_u8(color.y, frame.format.bit_depth),
        u: vvc_downshift_sample_to_u8(color.u, frame.format.bit_depth),
        v: vvc_downshift_sample_to_u8(color.v, frame.format.bit_depth),
        luma_tu_remainders,
        luma_tu_negative,
        luma_tu_dc_levels,
        luma_tu_ac_levels,
        luma_tu_count,
        chroma_tu_count,
        cb_tu_dc_levels,
        cr_tu_dc_levels,
        cb_tu_ac_levels,
        cr_tu_ac_levels,
        cb_rem,
        cr_rem,
    }
}

fn lossless_luma_ac_levels(
    residuals: &[i16],
    width: usize,
) -> [i16; super::VVC_LUMA_AC_COEFFS_PER_TU] {
    let mut levels = [0; super::VVC_LUMA_AC_COEFFS_PER_TU];
    for y in 0..4 {
        for x in 0..4 {
            if x == 0 && y == 0 {
                continue;
            }
            let raster_idx = y * width + x;
            if raster_idx < residuals.len() {
                levels[y * 4 + x - 1] = residuals[raster_idx];
            }
        }
    }
    levels
}

fn lossless_chroma_ac_levels(
    residuals: &[i16],
    width: usize,
) -> [i16; VVC_CHROMA_AC_COEFFS_PER_TU] {
    let mut levels = [0; VVC_CHROMA_AC_COEFFS_PER_TU];
    for (slot, (x, y)) in VVC_CHROMA_AC_POSITIONS_4X4.iter().copied().enumerate() {
        let raster_idx = y * width + x;
        if raster_idx < residuals.len() {
            levels[slot] = residuals[raster_idx];
        }
    }
    levels
}

fn chroma_partition_shape(
    geometry: VvcVideoGeometry,
    chroma_sampling: crate::picture::ChromaSampling,
) -> VvcCtuPartitionShape {
    VvcCtuPartitionShape {
        root_width: VVC_CTU_SIZE as u16,
        root_height: VVC_CTU_SIZE as u16,
        visible_width: geometry.coded_width() as u16,
        visible_height: geometry.coded_height() as u16,
        chroma_sampling,
    }
}

fn vvc_luma_tu_nodes(
    geometry: VvcVideoGeometry,
    chroma_sampling: crate::picture::ChromaSampling,
) -> Vec<VvcCodingTreeNode> {
    vvc_luma_tu_nodes_with_leaf_size(geometry, chroma_sampling, VVC_CURRENT_MAX_LUMA_LEAF_SIZE)
}

fn vvc_luma_tu_nodes_with_leaf_size(
    geometry: VvcVideoGeometry,
    chroma_sampling: crate::picture::ChromaSampling,
    luma_max_leaf_size: u16,
) -> Vec<VvcCodingTreeNode> {
    vvc_luma_transform_nodes(
        chroma_partition_shape(geometry, chroma_sampling),
        luma_max_leaf_size,
    )
}

fn residual_luma_tu_at(
    frame: &VvcSampledFrame,
    origin_x: usize,
    origin_y: usize,
    width: usize,
    height: usize,
    predicted: &[VvcSample],
) -> Vec<i16> {
    debug_assert_eq!(predicted.len(), width * height);
    let mut residuals: Vec<i16> = predicted
        .iter()
        .map(|predicted| vvc_sample_delta_i16(0, *predicted))
        .collect();
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
    residuals
}

fn residual_chroma_tu_at(
    samples: &[VvcSample],
    geometry: VvcVideoGeometry,
    format: VvcPictureFormat,
    origin_x: usize,
    origin_y: usize,
    width: usize,
    height: usize,
    predicted: &[VvcSample],
) -> Vec<i16> {
    debug_assert_eq!(predicted.len(), width * height);
    let chroma_width = geometry.width / chroma_subsample_x(format.chroma_sampling);
    let chroma_height = geometry.height / chroma_subsample_y(format.chroma_sampling);
    let neutral = vvc_neutral_sample(format.bit_depth);
    let mut residuals: Vec<i16> = predicted
        .iter()
        .map(|predicted| vvc_sample_delta_i16(neutral, *predicted))
        .collect();
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
    residuals
}

fn vvc_sample_delta_i16(sample: VvcSample, predicted: VvcSample) -> i16 {
    (i32::from(sample) - i32::from(predicted)).clamp(i32::from(i16::MIN), i32::from(i16::MAX))
        as i16
}
