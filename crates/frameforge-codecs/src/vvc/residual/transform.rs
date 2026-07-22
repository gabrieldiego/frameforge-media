#[cfg(test)]
use super::super::VvcSample;
use super::{VvcQuantizedTransformBlock, VVC_CHROMA_AC_COEFFS_PER_TU, VVC_CHROMA_AC_POSITIONS_4X4};
#[cfg(test)]
use super::{VvcTransformComponent, VvcTuTransformBlock};
use crate::picture::SampleBitDepth;

#[cfg(test)]
pub(in crate::vvc) const VVC_LUMA_DC_BASE: i16 = 114;
#[cfg(test)]
pub(in crate::vvc) const VVC_CHROMA_DC_BASE: i16 = 128;
const VVC_LUMA_DC_NUM: i32 = 5;
const VVC_LUMA_DC_DEN: i32 = 16;
pub(in crate::vvc) const VVC_DEFAULT_LOSSY_LUMA_QP: i32 = 32;
pub(in crate::vvc) const VVC_DEFAULT_LOSSY_CHROMA_QP: i32 = 34;
const VVC_LUMA_AC_HADAMARD_QUANT_SHIFT: u32 = 8;
const VVC_CHROMA_AC_QUANT_SHIFT_FOR_8X8: i32 = 19;
const VVC_LUMA_AC_LEVEL_LIMIT: i16 = 2;
const VVC_CHROMA_DC_LEVEL_LIMIT: i16 = 255;
const VVC_CHROMA_AC_LEVEL_LIMIT: i16 = 2;
const VVC_MAX_TRANSFORM_EDGE: usize = 32;
const VVC_MAX_TRANSFORM_COEFFS: usize = VVC_MAX_TRANSFORM_EDGE * VVC_MAX_TRANSFORM_EDGE;
const VVC_DCT2_4: [[i32; 4]; 4] = [
    [64, 64, 64, 64],
    [83, 36, -36, -83],
    [64, -64, -64, 64],
    [36, -83, 83, -36],
];
const VVC_DCT2_8: [[i32; 8]; 8] = [
    [64, 64, 64, 64, 64, 64, 64, 64],
    // H.266 inverse DCT-II 8-point matrix, matching VTM
    // g_trCoreDCT2P8[TRANSFORM_INVERSE]. The 89 entries differ from the
    // older HEVC-style 87 values and are required for bit-exact decoder-side
    // reconstruction.
    [89, 75, 50, 18, -18, -50, -75, -89],
    [83, 36, -36, -83, -83, -36, 36, 83],
    [75, -18, -89, -50, 50, 89, 18, -75],
    [64, -64, -64, 64, 64, -64, -64, 64],
    [50, -89, 18, 75, -75, -18, 89, -50],
    [36, -83, 83, -36, -36, 83, -83, 36],
    [18, -50, 75, -89, 89, -75, 50, -18],
];
const VVC_DCT2_16_AC_ROWS_1_TO_3: [[i32; 16]; 3] = [
    [
        90, 87, 80, 70, 57, 43, 25, 9, -9, -25, -43, -57, -70, -80, -87, -90,
    ],
    [
        89, 75, 50, 18, -18, -50, -75, -89, -89, -75, -50, -18, 18, 50, 75, 89,
    ],
    [
        87, 57, 9, -43, -80, -90, -70, -25, 25, 70, 90, 80, 43, -9, -57, -87,
    ],
];
const VVC_DCT2_32_AC_ROWS_1_TO_3: [[i32; 32]; 3] = [
    [
        90, 90, 88, 85, 82, 78, 73, 67, 61, 54, 46, 38, 31, 22, 13, 4, -4, -13, -22, -31, -38, -46,
        -54, -61, -67, -73, -78, -82, -85, -88, -90, -90,
    ],
    [
        90, 87, 80, 70, 57, 43, 25, 9, -9, -25, -43, -57, -70, -80, -87, -90, -90, -87, -80, -70,
        -57, -43, -25, -9, 9, 25, 43, 57, 70, 80, 87, 90,
    ],
    [
        90, 82, 67, 46, 22, -4, -31, -54, -73, -85, -90, -88, -78, -61, -38, -13, 13, 38, 61, 78,
        88, 90, 85, 73, 54, 31, 4, -22, -46, -67, -82, -90,
    ],
];

#[cfg(test)]
pub(in crate::vvc) fn transform_vvc_tu(
    component: VvcTransformComponent,
    width: u16,
    height: u16,
    samples: &[VvcSample],
) -> VvcTuTransformBlock {
    debug_assert!(width > 0);
    debug_assert!(height > 0);
    let sample_count = usize::from(width) * usize::from(height);
    assert_eq!(
        samples.len(),
        sample_count,
        "transform input must contain one sample per TU position"
    );
    let sum: u64 = samples.iter().map(|sample| u64::from(*sample)).sum();
    let dc_sample = ((sum + (sample_count as u64 / 2)) / sample_count as u64) as VvcSample;
    let mut ac_coeffs = Vec::with_capacity(sample_count.saturating_sub(1));
    for sample in samples.iter().skip(1) {
        ac_coeffs.push(
            (i32::from(*sample) - i32::from(dc_sample))
                .clamp(i32::from(i16::MIN), i32::from(i16::MAX)) as i16,
        );
    }
    VvcTuTransformBlock {
        component,
        width,
        height,
        dc_coeff: (i32::from(dc_sample) - i32::from(component.dc_base()))
            .clamp(i32::from(i16::MIN), i32::from(i16::MAX)) as i16,
        ac_coeffs,
    }
}

#[cfg(test)]
pub(in crate::vvc) fn quantize_vvc_luma_residual_greedy(
    residuals: &[i16],
    width: u16,
    height: u16,
    bit_depth: SampleBitDepth,
) -> VvcQuantizedTransformBlock {
    quantize_vvc_luma_residual_greedy_with_qp(
        residuals,
        width,
        height,
        bit_depth,
        VVC_DEFAULT_LOSSY_LUMA_QP,
    )
}

pub(in crate::vvc) fn quantize_vvc_luma_residual_greedy_with_qp(
    residuals: &[i16],
    width: u16,
    height: u16,
    bit_depth: SampleBitDepth,
    qp: i32,
) -> VvcQuantizedTransformBlock {
    let coefficient_count = usize::from(width) * usize::from(height);
    assert_eq!(residuals.len(), coefficient_count);
    debug_assert!([4, 8, 16, 32].contains(&width));
    debug_assert!([4, 8, 16, 32].contains(&height));
    debug_assert!((0..=63).contains(&qp));

    let dc_level = quantize_vvc_luma_residual_dc_by_search(residuals, width, height, bit_depth, qp);

    // H.266 7.3.11.10 transform_unit() can carry all AC coefficients. The
    // current luma subset keeps the full first 4x4 coefficient group so the
    // residual syntax remains ready for the next transform expansion.
    let (ac_coeffs, has_ac) = quantize_direct_luma_ac_coeffs(residuals, width, height, qp);
    VvcQuantizedTransformBlock {
        reconstructed_dc_coeff: dc_level,
        reconstructed_ac_coeffs: ac_coeffs,
        has_ac,
        abs_remainder: dc_level.unsigned_abs().min(u8::MAX as u16) as u8,
    }
}

fn quantize_vvc_luma_residual_dc_by_search(
    residuals: &[i16],
    width: u16,
    height: u16,
    bit_depth: SampleBitDepth,
    qp: i32,
) -> i16 {
    let sample_count = residuals.len() as i64;
    let (residual_sum, original_sse) = residual_sum_and_sse(residuals);
    let unit = dc_only_residual_from_level(1, width, height, qp, bit_depth);
    if unit == 0 {
        let residual_avg = div_round_nearest_i64(residual_sum, sample_count);
        return div_round_nearest_i64(
            residual_avg * i64::from(VVC_LUMA_DC_NUM),
            i64::from(VVC_LUMA_DC_DEN),
        )
        .clamp(i64::from(i16::MIN), i64::from(i16::MAX)) as i16;
    }

    let estimate = div_round_nearest_i64(residual_sum, sample_count * i64::from(unit))
        .clamp(i64::from(i16::MIN), i64::from(i16::MAX));
    let mut best_level = 0i16;
    let mut best_sse = original_sse;
    for candidate in (estimate - 4)..=(estimate + 4) {
        let level = candidate.clamp(i64::from(i16::MIN), i64::from(i16::MAX)) as i16;
        let reconstructed = i64::from(dc_only_residual_from_level(
            level, width, height, qp, bit_depth,
        ));
        let sse = original_sse + (sample_count * reconstructed * reconstructed)
            - (2 * reconstructed * residual_sum);
        if sse < best_sse {
            best_sse = sse;
            best_level = level;
        }
    }
    best_level
}

pub(in crate::vvc) struct VvcInverseTransformScratch {
    dequantized: [i32; VVC_MAX_TRANSFORM_COEFFS],
    vertical: [i32; VVC_MAX_TRANSFORM_COEFFS],
}

impl Default for VvcInverseTransformScratch {
    fn default() -> Self {
        Self {
            dequantized: [0; VVC_MAX_TRANSFORM_COEFFS],
            vertical: [0; VVC_MAX_TRANSFORM_COEFFS],
        }
    }
}

#[cfg(test)]
pub(in crate::vvc) fn inverse_transform_vvc_luma_quantized_block_into(
    residuals: &mut Vec<i16>,
    scratch: &mut VvcInverseTransformScratch,
    width: u16,
    height: u16,
    dc_level: i16,
    ac_levels: &[i16; 15],
    bit_depth: SampleBitDepth,
) {
    inverse_transform_vvc_luma_quantized_block_into_with_qp(
        residuals,
        scratch,
        width,
        height,
        dc_level,
        ac_levels,
        bit_depth,
        VVC_DEFAULT_LOSSY_LUMA_QP,
    );
}

pub(in crate::vvc) fn inverse_transform_vvc_luma_quantized_block_into_with_qp(
    residuals: &mut Vec<i16>,
    scratch: &mut VvcInverseTransformScratch,
    width: u16,
    height: u16,
    dc_level: i16,
    ac_levels: &[i16; 15],
    bit_depth: SampleBitDepth,
    qp: i32,
) {
    inverse_transform_vvc_quantized_block_into(
        residuals, scratch, width, height, dc_level, ac_levels, qp, bit_depth,
    );
}

#[cfg(test)]
pub(in crate::vvc) fn inverse_transform_vvc_chroma_quantized_block_into(
    residuals: &mut Vec<i16>,
    scratch: &mut VvcInverseTransformScratch,
    width: u16,
    height: u16,
    dc_level: i16,
    ac_levels: &[i16; 15],
    bit_depth: SampleBitDepth,
) {
    inverse_transform_vvc_chroma_quantized_block_into_with_qp(
        residuals,
        scratch,
        width,
        height,
        dc_level,
        ac_levels,
        bit_depth,
        VVC_DEFAULT_LOSSY_CHROMA_QP,
    );
}

pub(in crate::vvc) fn inverse_transform_vvc_chroma_quantized_block_into_with_qp(
    residuals: &mut Vec<i16>,
    scratch: &mut VvcInverseTransformScratch,
    width: u16,
    height: u16,
    dc_level: i16,
    ac_levels: &[i16; 15],
    bit_depth: SampleBitDepth,
    chroma_qp: i32,
) {
    inverse_transform_vvc_quantized_block_into(
        residuals, scratch, width, height, dc_level, ac_levels, chroma_qp, bit_depth,
    );
}

fn inverse_transform_vvc_quantized_block_into(
    residuals: &mut Vec<i16>,
    scratch: &mut VvcInverseTransformScratch,
    width: u16,
    height: u16,
    dc_level: i16,
    ac_levels: &[i16; 15],
    qp: i32,
    bit_depth: SampleBitDepth,
) {
    let width_usize = usize::from(width);
    let height_usize = usize::from(height);
    let coefficient_count = width_usize * height_usize;
    debug_assert!(coefficient_count <= VVC_MAX_TRANSFORM_COEFFS);
    debug_assert!([4, 8, 16, 32].contains(&width));
    debug_assert!([4, 8, 16, 32].contains(&height));

    let active_width = width_usize.min(4);
    let active_height = height_usize.min(4);
    let dequantized = &mut scratch.dequantized[..coefficient_count];
    for y in 0..active_height {
        for x in 0..active_width {
            dequantized[y * width_usize + x] = 0;
        }
    }
    dequantized[0] = dequantize_vvc_transform_level(dc_level, width, height, qp);
    for y in 0..active_height {
        for x in 0..active_width {
            if x == 0 && y == 0 {
                continue;
            }
            let level = ac_levels[y * 4 + x - 1];
            if level != 0 {
                dequantized[y * width_usize + x] =
                    dequantize_vvc_transform_level(level, width, height, qp);
            }
        }
    }
    inverse_transform_vvc_dequantized_levels_into(
        residuals,
        scratch,
        width,
        height,
        active_width,
        active_height,
        bit_depth,
    );
}

#[cfg(test)]
pub(in crate::vvc) fn inverse_transform_vvc_luma_residual_levels(
    width: u16,
    height: u16,
    coeff_levels: &[i16],
    bit_depth: SampleBitDepth,
) -> Vec<i16> {
    let mut scratch = VvcInverseTransformScratch::default();
    let mut residuals = Vec::new();
    inverse_transform_vvc_residual_levels_into(
        &mut residuals,
        &mut scratch,
        width,
        height,
        coeff_levels,
        VVC_DEFAULT_LOSSY_LUMA_QP,
        bit_depth,
    );
    residuals
}

#[cfg(test)]
fn inverse_transform_vvc_residual_levels_into(
    residuals: &mut Vec<i16>,
    scratch: &mut VvcInverseTransformScratch,
    width: u16,
    height: u16,
    coeff_levels: &[i16],
    qp: i32,
    bit_depth: SampleBitDepth,
) {
    let width_usize = usize::from(width);
    let height_usize = usize::from(height);
    assert_eq!(coeff_levels.len(), width_usize * height_usize);
    debug_assert!([4, 8, 16, 32].contains(&width));
    debug_assert!([4, 8, 16, 32].contains(&height));

    let dequantized = &mut scratch.dequantized[..coeff_levels.len()];
    for (dst, level) in dequantized.iter_mut().zip(coeff_levels.iter().copied()) {
        *dst = dequantize_vvc_transform_level(level, width, height, qp);
    }
    inverse_transform_vvc_dequantized_levels_into(
        residuals,
        scratch,
        width,
        height,
        width_usize,
        height_usize,
        bit_depth,
    );
}

fn inverse_transform_vvc_dequantized_levels_into(
    residuals: &mut Vec<i16>,
    scratch: &mut VvcInverseTransformScratch,
    width: u16,
    height: u16,
    active_width: usize,
    active_height: usize,
    bit_depth: SampleBitDepth,
) {
    let width_usize = usize::from(width);
    let height_usize = usize::from(height);
    let coefficient_count = width_usize * height_usize;
    let dequantized = &scratch.dequantized[..coefficient_count];
    let vertical = &mut scratch.vertical[..coefficient_count];
    debug_assert!(active_width <= width_usize);
    debug_assert!(active_height <= height_usize);
    for x in 0..active_width {
        for y in 0..height_usize {
            let mut sum = 0;
            for k in 0..active_height {
                let coeff = dequantized[k * width_usize + x];
                if coeff != 0 {
                    sum += dct2_value(height, k, y) * coeff;
                }
            }
            vertical[y * width_usize + x] = if height > 1 { (sum + 64) >> 7 } else { sum };
        }
    }

    let residual_bd_shift = if width > 1 && height > 1 {
        5 + 15 - i32::from(bit_depth.bits())
    } else {
        6 + 15 - i32::from(bit_depth.bits())
    };
    let residual_offset = 1 << (residual_bd_shift - 1);
    residuals.clear();
    residuals.resize(coefficient_count, 0);
    for y in 0..height_usize {
        for x in 0..width_usize {
            let mut sum = 0;
            for k in 0..active_width {
                let coeff = vertical[y * width_usize + k];
                if coeff != 0 {
                    sum += dct2_value(width, k, x) * coeff;
                }
            }
            residuals[y * width_usize + x] = ((sum + residual_offset) >> residual_bd_shift) as i16;
        }
    }
}

#[cfg(test)]
#[allow(dead_code)]
pub(in crate::vvc) fn quantize_vvc_chroma_residual_dc(
    residuals: &[i16],
    width: u16,
    height: u16,
    bit_depth: SampleBitDepth,
) -> i16 {
    quantize_vvc_chroma_residual_dc_with_qp(
        residuals,
        width,
        height,
        bit_depth,
        VVC_DEFAULT_LOSSY_CHROMA_QP,
    )
}

pub(in crate::vvc) fn quantize_vvc_chroma_residual_dc_with_qp(
    residuals: &[i16],
    width: u16,
    height: u16,
    bit_depth: SampleBitDepth,
    chroma_qp: i32,
) -> i16 {
    let coefficient_count = usize::from(width) * usize::from(height);
    assert_eq!(residuals.len(), coefficient_count);
    debug_assert!([4, 8, 16, 32].contains(&width));
    debug_assert!([4, 8, 16, 32].contains(&height));
    debug_assert!((0..=63).contains(&chroma_qp));

    let (residual_sum, original_sse) = residual_sum_and_sse(residuals);
    if width == 4
        && height == 4
        && bit_depth.bits() == 8
        && chroma_qp == VVC_DEFAULT_LOSSY_CHROMA_QP
    {
        return quantize_vvc_chroma_4x4_dc_level_from_sum(residual_sum);
    }

    quantize_vvc_chroma_residual_dc_by_search(
        residual_sum,
        original_sse,
        residuals.len() as i64,
        width,
        height,
        bit_depth,
        chroma_qp,
    )
}

fn quantize_vvc_chroma_4x4_dc_level_from_sum(residual_sum: i64) -> i16 {
    // H.266 8.7.3 inverse coefficient scaling plus 8.7.4 inverse transform
    // reduces to reconstructed_residual = 8 * level for a 4x4 chroma TB at
    // chroma QP 34. The older exhaustive SSE search initialized at level 0 and
    // replaced only on strict improvement, so exact half-step ties keep the
    // earlier level.
    let level = if (-64..=64).contains(&residual_sum) {
        0
    } else if residual_sum > 64 {
        (residual_sum + 63) / 128
    } else {
        -(((-residual_sum) + 64) / 128)
    };
    level.clamp(
        i64::from(-VVC_CHROMA_DC_LEVEL_LIMIT),
        i64::from(VVC_CHROMA_DC_LEVEL_LIMIT),
    ) as i16
}

fn quantize_vvc_chroma_residual_dc_by_search(
    residual_sum: i64,
    original_sse: i64,
    sample_count: i64,
    width: u16,
    height: u16,
    bit_depth: SampleBitDepth,
    chroma_qp: i32,
) -> i16 {
    if !chroma_dc_fast_search_allowed(width, height, bit_depth, chroma_qp) {
        return quantize_vvc_chroma_residual_dc_by_exhaustive_search(
            residual_sum,
            original_sse,
            sample_count,
            width,
            height,
            bit_depth,
            chroma_qp,
        );
    }

    let target_level = first_chroma_dc_level_at_or_above_target(
        residual_sum,
        sample_count,
        width,
        height,
        bit_depth,
        chroma_qp,
    );
    let mut candidates = [0i16; 3];
    let mut candidate_count = 0usize;
    push_chroma_dc_candidate(&mut candidates, &mut candidate_count, 0);
    if target_level <= i32::from(VVC_CHROMA_DC_LEVEL_LIMIT) {
        let level = target_level as i16;
        push_chroma_dc_candidate(
            &mut candidates,
            &mut candidate_count,
            first_chroma_dc_level_with_reconstructed_residual(
                dc_only_residual_from_level(level, width, height, chroma_qp, bit_depth),
                width,
                height,
                bit_depth,
                chroma_qp,
            ),
        );
    }
    if target_level > i32::from(-VVC_CHROMA_DC_LEVEL_LIMIT) {
        let level = (target_level - 1).min(i32::from(VVC_CHROMA_DC_LEVEL_LIMIT)) as i16;
        push_chroma_dc_candidate(
            &mut candidates,
            &mut candidate_count,
            first_chroma_dc_level_with_reconstructed_residual(
                dc_only_residual_from_level(level, width, height, chroma_qp, bit_depth),
                width,
                height,
                bit_depth,
                chroma_qp,
            ),
        );
    }
    candidates[..candidate_count].sort_unstable();

    let mut best_level = 0;
    let mut best_sse = original_sse;

    for level in candidates.into_iter().take(candidate_count) {
        let sse = chroma_dc_sse_for_level(
            level,
            residual_sum,
            original_sse,
            sample_count,
            width,
            height,
            bit_depth,
            chroma_qp,
        );
        if sse < best_sse {
            best_sse = sse;
            best_level = level;
        }
    }
    best_level
}

fn chroma_dc_fast_search_allowed(
    width: u16,
    height: u16,
    bit_depth: SampleBitDepth,
    chroma_qp: i32,
) -> bool {
    let min_reconstructed = dc_only_residual_from_level_i64(
        -VVC_CHROMA_DC_LEVEL_LIMIT,
        width,
        height,
        chroma_qp,
        bit_depth,
    );
    let max_reconstructed = dc_only_residual_from_level_i64(
        VVC_CHROMA_DC_LEVEL_LIMIT,
        width,
        height,
        chroma_qp,
        bit_depth,
    );
    min_reconstructed >= i64::from(i16::MIN) && max_reconstructed <= i64::from(i16::MAX)
}

fn first_chroma_dc_level_at_or_above_target(
    residual_sum: i64,
    sample_count: i64,
    width: u16,
    height: u16,
    bit_depth: SampleBitDepth,
    chroma_qp: i32,
) -> i32 {
    debug_assert!(sample_count > 0);
    let mut lo = i32::from(-VVC_CHROMA_DC_LEVEL_LIMIT);
    let mut hi = i32::from(VVC_CHROMA_DC_LEVEL_LIMIT) + 1;
    while lo < hi {
        let mid = lo + ((hi - lo) >> 1);
        let reconstructed = i64::from(dc_only_residual_from_level(
            mid as i16, width, height, chroma_qp, bit_depth,
        ));
        if sample_count * reconstructed >= residual_sum {
            hi = mid;
        } else {
            lo = mid + 1;
        }
    }
    lo
}

fn first_chroma_dc_level_with_reconstructed_residual(
    target_residual: i16,
    width: u16,
    height: u16,
    bit_depth: SampleBitDepth,
    chroma_qp: i32,
) -> i16 {
    let mut lo = i32::from(-VVC_CHROMA_DC_LEVEL_LIMIT);
    let mut hi = i32::from(VVC_CHROMA_DC_LEVEL_LIMIT);
    while lo < hi {
        let mid = lo + ((hi - lo) >> 1);
        let reconstructed =
            dc_only_residual_from_level(mid as i16, width, height, chroma_qp, bit_depth);
        if reconstructed >= target_residual {
            hi = mid;
        } else {
            lo = mid + 1;
        }
    }
    lo as i16
}

fn push_chroma_dc_candidate(candidates: &mut [i16; 3], count: &mut usize, level: i16) {
    if candidates
        .iter()
        .take(*count)
        .any(|candidate| *candidate == level)
    {
        return;
    }
    debug_assert!(*count < candidates.len());
    candidates[*count] = level;
    *count += 1;
}

fn chroma_dc_sse_for_level(
    level: i16,
    residual_sum: i64,
    original_sse: i64,
    sample_count: i64,
    width: u16,
    height: u16,
    bit_depth: SampleBitDepth,
    chroma_qp: i32,
) -> i64 {
    let reconstructed = i64::from(dc_only_residual_from_level(
        level, width, height, chroma_qp, bit_depth,
    ));
    original_sse + (sample_count * reconstructed * reconstructed)
        - (2 * reconstructed * residual_sum)
}

fn quantize_vvc_chroma_residual_dc_by_exhaustive_search(
    residual_sum: i64,
    original_sse: i64,
    sample_count: i64,
    width: u16,
    height: u16,
    bit_depth: SampleBitDepth,
    chroma_qp: i32,
) -> i16 {
    let mut best_level = 0;
    let mut best_sse = original_sse;

    for level in -VVC_CHROMA_DC_LEVEL_LIMIT..=VVC_CHROMA_DC_LEVEL_LIMIT {
        let sse = chroma_dc_sse_for_level(
            level,
            residual_sum,
            original_sse,
            sample_count,
            width,
            height,
            bit_depth,
            chroma_qp,
        );
        if sse < best_sse {
            best_sse = sse;
            best_level = level;
        }
    }
    best_level
}

#[cfg(test)]
#[allow(dead_code)]
pub(in crate::vvc) fn quantize_vvc_chroma_residual_greedy(
    residuals: &[i16],
    width: u16,
    height: u16,
    bit_depth: SampleBitDepth,
) -> VvcQuantizedTransformBlock {
    quantize_vvc_chroma_residual_greedy_with_qp(
        residuals,
        width,
        height,
        bit_depth,
        VVC_DEFAULT_LOSSY_CHROMA_QP,
    )
}

pub(in crate::vvc) fn quantize_vvc_chroma_residual_greedy_with_qp(
    residuals: &[i16],
    width: u16,
    height: u16,
    bit_depth: SampleBitDepth,
    chroma_qp: i32,
) -> VvcQuantizedTransformBlock {
    let coefficient_count = usize::from(width) * usize::from(height);
    assert_eq!(residuals.len(), coefficient_count);
    debug_assert!([4, 8, 16, 32].contains(&width));
    debug_assert!([4, 8, 16, 32].contains(&height));
    debug_assert!((0..=63).contains(&chroma_qp));

    let dc_level =
        quantize_vvc_chroma_residual_dc_with_qp(residuals, width, height, bit_depth, chroma_qp);
    let mut ac_coeffs = [0; 15];
    let mut has_ac = false;
    if residuals_have_ac_energy(residuals) {
        // H.266 7.3.11.10 transform_unit() can carry the full 4x4 chroma
        // coefficient group. Keep the stored residual shape ready for the
        // lossless transform-skip path even when the lossy quantizer selects
        // only a small subset of nonzero levels.
        (ac_coeffs, has_ac) = quantize_direct_chroma_ac_coeffs(residuals, width, height, chroma_qp);
    }

    VvcQuantizedTransformBlock {
        reconstructed_dc_coeff: dc_level,
        reconstructed_ac_coeffs: ac_coeffs,
        has_ac,
        abs_remainder: dc_level.unsigned_abs().min(u8::MAX as u16) as u8,
    }
}

#[cfg(test)]
pub(in crate::vvc) fn quantize_vvc_chroma(u: u8, v: u8) -> u8 {
    quantize_vvc_chroma_sample(u).max(quantize_vvc_chroma_sample(v))
}

pub(in crate::vvc) fn quantize_vvc_chroma_sample(sample: u8) -> u8 {
    let mut best_rem = 0;
    let mut best_error = u16::MAX;
    for rem in 0..=16 {
        let value = reconstruct_vvc_chroma(rem);
        let error = sample.abs_diff(value) as u16;
        if error < best_error {
            best_rem = rem;
            best_error = error;
        }
    }
    best_rem
}

pub(in crate::vvc) fn reconstruct_vvc_chroma(chroma_residual: u8) -> u8 {
    (((16 - chroma_residual.min(16)) as u16 * 128 + 8) / 16) as u8
}

fn quantize_direct_luma_ac_coeffs(
    residuals: &[i16],
    width: u16,
    height: u16,
    qp: i32,
) -> ([i16; 15], bool) {
    let cell_sums = luma_hadamard_cell_sums(residuals, width);
    let mut ac_coeffs = [0; 15];
    let mut has_ac = false;
    let quant_shift = luma_ac_quant_shift(qp);
    let level_limit = luma_ac_level_limit(qp);
    for ky in 0..usize::from(height).min(4) {
        for kx in 0..usize::from(width).min(4) {
            if kx == 0 && ky == 0 {
                continue;
            }
            let mut acc = 0i64;
            for cell_y in 0..4 {
                for cell_x in 0..4 {
                    acc += cell_sums[cell_y * 4 + cell_x]
                        * i64::from(luma_lossy_hadamard4_basis(kx, cell_x))
                        * i64::from(luma_lossy_hadamard4_basis(ky, cell_y));
                }
            }
            let level = div_round_nearest_i64(acc, 1i64 << quant_shift);
            ac_coeffs[ky * 4 + kx - 1] =
                level.clamp(i64::from(-level_limit), i64::from(level_limit)) as i16;
            has_ac |= ac_coeffs[ky * 4 + kx - 1] != 0;
        }
    }
    (ac_coeffs, has_ac)
}

fn luma_ac_quant_shift(qp: i32) -> u32 {
    qp_adjusted_quant_shift(
        VVC_LUMA_AC_HADAMARD_QUANT_SHIFT,
        qp,
        VVC_DEFAULT_LOSSY_LUMA_QP,
    )
}

fn luma_ac_level_limit(qp: i32) -> i16 {
    qp_adjusted_level_limit(VVC_LUMA_AC_LEVEL_LIMIT, qp, VVC_DEFAULT_LOSSY_LUMA_QP)
}

fn luma_hadamard_cell_sums(residuals: &[i16], width: u16) -> [i64; 16] {
    let width_usize = usize::from(width);
    let height_usize = residuals.len() / width_usize;
    let cell_width = (width_usize / 4).max(1);
    let cell_height = (height_usize / 4).max(1);
    let mut cell_sums = [0i64; 16];

    // H.266 7.3.11.10 still carries the first 4x4 transform coefficient
    // group. For the current lossy hardware subset, encoder-side luma
    // projection is a 4x4 box/Hadamard approximation so the RTL does not need
    // a full 8x8 DCT datapath. Decoder reconstruction remains the H.266 8.7.3
    // and 8.7.4 inverse transform over these emitted coefficient levels.
    for cell_y in 0..4 {
        for cell_x in 0..4 {
            let mut cell_sum = 0i64;
            let y_start = cell_y * cell_height;
            let y_end = ((cell_y + 1) * cell_height).min(height_usize);
            let x_start = cell_x * cell_width;
            let x_end = ((cell_x + 1) * cell_width).min(width_usize);
            for y in y_start..y_end {
                for x in x_start..x_end {
                    cell_sum += i64::from(residuals[y * width_usize + x]);
                }
            }
            cell_sums[cell_y * 4 + cell_x] = cell_sum;
        }
    }
    cell_sums
}

fn luma_lossy_hadamard4_basis(k: usize, n: usize) -> i32 {
    match k {
        0 => 1,
        1 => {
            if n < 2 {
                1
            } else {
                -1
            }
        }
        2 => {
            if n == 0 || n == 3 {
                1
            } else {
                -1
            }
        }
        3 => {
            if n == 0 || n == 2 {
                1
            } else {
                -1
            }
        }
        _ => 0,
    }
}

fn quantize_direct_chroma_ac_coeffs(
    residuals: &[i16],
    width: u16,
    height: u16,
    chroma_qp: i32,
) -> ([i16; VVC_CHROMA_AC_COEFFS_PER_TU], bool) {
    let width_usize = usize::from(width);
    let height_usize = usize::from(height);
    debug_assert_eq!(residuals.len(), width_usize * height_usize);
    let active_width = width_usize.min(4);
    let active_height = height_usize.min(4);
    let mut ac_coeffs = [0; VVC_CHROMA_AC_COEFFS_PER_TU];
    let mut has_ac = false;
    let mut vertical = [0i64; 4 * VVC_MAX_TRANSFORM_EDGE];
    let quant_shift = chroma_ac_quant_shift(width, height, chroma_qp);
    let level_limit = chroma_ac_level_limit(chroma_qp);
    for ky in 0..active_height {
        for x in 0..width_usize {
            let mut sum = 0i64;
            for y in 0..height_usize {
                sum += i64::from(residuals[y * width_usize + x])
                    * i64::from(dct2_value(height, ky, y));
            }
            vertical[ky * VVC_MAX_TRANSFORM_EDGE + x] = sum;
        }
    }

    for (kx, ky) in VVC_CHROMA_AC_POSITIONS_4X4 {
        if kx < active_width && ky < active_height {
            let mut acc = 0i64;
            for x in 0..width_usize {
                acc +=
                    vertical[ky * VVC_MAX_TRANSFORM_EDGE + x] * i64::from(dct2_value(width, kx, x));
            }
            let level = div_round_nearest_i64(acc, 1i64 << quant_shift);
            ac_coeffs[ky * 4 + kx - 1] =
                level.clamp(i64::from(-level_limit), i64::from(level_limit)) as i16;
            has_ac |= ac_coeffs[ky * 4 + kx - 1] != 0;
        }
    }
    (ac_coeffs, has_ac)
}

fn chroma_ac_quant_shift(width: u16, height: u16, chroma_qp: i32) -> u32 {
    let log2_sum = width.ilog2() as i32 + height.ilog2() as i32;
    let base_shift = (VVC_CHROMA_AC_QUANT_SHIFT_FOR_8X8 + log2_sum - 6).max(0) as u32;
    qp_adjusted_quant_shift(base_shift, chroma_qp, VVC_DEFAULT_LOSSY_CHROMA_QP)
}

fn chroma_ac_level_limit(chroma_qp: i32) -> i16 {
    qp_adjusted_level_limit(
        VVC_CHROMA_AC_LEVEL_LIMIT,
        chroma_qp,
        VVC_DEFAULT_LOSSY_CHROMA_QP,
    )
}

fn qp_adjusted_quant_shift(base_shift: u32, qp: i32, base_qp: i32) -> u32 {
    let delta = qp - base_qp;
    if delta >= 0 {
        base_shift.saturating_add((delta / 6) as u32)
    } else {
        base_shift.saturating_sub(((-delta + 5) / 6) as u32)
    }
}

fn qp_adjusted_level_limit(base_limit: i16, qp: i32, base_qp: i32) -> i16 {
    let delta = base_qp - qp;
    if delta <= 0 {
        return base_limit.max(1);
    }
    let shift = ((delta + 5) / 6).clamp(0, 4) as u32;
    ((i32::from(base_limit) << shift).min(i32::from(i16::MAX))) as i16
}

fn residuals_have_ac_energy(residuals: &[i16]) -> bool {
    residuals
        .first()
        .is_some_and(|first| residuals.iter().any(|value| value != first))
}

fn residual_sum_and_sse(residuals: &[i16]) -> (i64, i64) {
    residuals.iter().fold((0, 0), |(sum, sse), value| {
        let value = i64::from(*value);
        (sum + value, sse + value * value)
    })
}

fn div_round_nearest_i64(value: i64, divisor: i64) -> i64 {
    debug_assert!(divisor > 0);
    if value < 0 {
        -(((-value) + (divisor / 2)) / divisor)
    } else {
        (value + (divisor / 2)) / divisor
    }
}

fn dequantize_vvc_transform_level(level: i16, tb_width: u16, tb_height: u16, qp: i32) -> i32 {
    if level == 0 {
        return 0;
    }
    debug_assert!((0..=63).contains(&qp));

    let log2_width = tb_width.ilog2() as i32;
    let log2_height = tb_height.ilog2() as i32;
    let log2_sum = log2_width + log2_height;
    let rect_non_ts = (log2_sum & 1) as usize;
    let level_scale = [[40, 45, 51, 57, 64, 72], [57, 64, 72, 80, 90, 102]];
    let ls = 16 * level_scale[rect_non_ts][(qp % 6) as usize] * (1 << (qp / 6));
    let bd_shift = 8 + rect_non_ts as i32 + (log2_sum / 2) + 10 - 15;
    let bd_offset = 1 << (bd_shift - 1);
    (i32::from(level) * ls + bd_offset) >> bd_shift
}

fn dct2_value(size: u16, k: usize, n: usize) -> i32 {
    if k == 0 {
        return 64;
    }
    match size {
        4 => VVC_DCT2_4[k][n],
        8 => VVC_DCT2_8[k][n],
        16 if k <= 3 => VVC_DCT2_16_AC_ROWS_1_TO_3[k - 1][n],
        32 if k <= 3 => VVC_DCT2_32_AC_ROWS_1_TO_3[k - 1][n],
        16 | 32 => {
            unimplemented!("DCT-II AC subset for size {size} is not wired for coefficient {k}")
        }
        other => unimplemented!("DCT-II matrix size {other} is not wired yet"),
    }
}

fn dc_only_residual_from_level(
    level: i16,
    width: u16,
    height: u16,
    qp: i32,
    bit_depth: SampleBitDepth,
) -> i16 {
    dc_only_residual_from_level_i64(level, width, height, qp, bit_depth) as i16
}

fn dc_only_residual_from_level_i64(
    level: i16,
    width: u16,
    height: u16,
    qp: i32,
    bit_depth: SampleBitDepth,
) -> i64 {
    if level == 0 {
        return 0;
    }
    let dequantized = dequantize_vvc_transform_level(level, width, height, qp);
    let vertical = if height > 1 {
        (64 * dequantized + 64) >> 7
    } else {
        64 * dequantized
    };
    let residual_bd_shift = if width > 1 && height > 1 {
        5 + 15 - i32::from(bit_depth.bits())
    } else {
        6 + 15 - i32::from(bit_depth.bits())
    };
    let residual_offset = 1 << (residual_bd_shift - 1);
    i64::from((64 * vertical + residual_offset) >> residual_bd_shift)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vvc_chroma_dc_fast_search_matches_exhaustive_search() {
        let dimensions = [4, 8, 16, 32];
        let qps = [0, 1, 6, 13, 24, 32, 34, 41, 51, 63];
        let bit_depths = [
            SampleBitDepth::new(8).expect("8-bit depth"),
            SampleBitDepth::new(10).expect("10-bit depth"),
            SampleBitDepth::new(12).expect("12-bit depth"),
        ];

        for bit_depth in bit_depths {
            let max_residual = bit_depth.max_sample() as i16;
            let probes = [
                -max_residual,
                -(max_residual / 2),
                (-513i16).clamp(-max_residual, max_residual),
                (-129i16).clamp(-max_residual, max_residual),
                -17,
                -1,
                0,
                1,
                17,
                129i16.clamp(-max_residual, max_residual),
                513i16.clamp(-max_residual, max_residual),
                max_residual / 2,
                max_residual,
            ];
            for width in dimensions {
                for height in dimensions {
                    for qp in qps {
                        for probe in probes {
                            assert_chroma_dc_fast_search_matches_exhaustive(
                                vec![probe; width * height],
                                width as u16,
                                height as u16,
                                bit_depth,
                                qp,
                            );
                            assert_chroma_dc_fast_search_matches_exhaustive(
                                alternating_chroma_dc_residuals(width, height, probe),
                                width as u16,
                                height as u16,
                                bit_depth,
                                qp,
                            );
                            assert_chroma_dc_fast_search_matches_exhaustive(
                                patterned_chroma_dc_residuals(width, height, probe, max_residual),
                                width as u16,
                                height as u16,
                                bit_depth,
                                qp,
                            );
                        }
                    }
                }
            }
        }
    }

    fn assert_chroma_dc_fast_search_matches_exhaustive(
        residuals: Vec<i16>,
        width: u16,
        height: u16,
        bit_depth: SampleBitDepth,
        chroma_qp: i32,
    ) {
        let (residual_sum, original_sse) = residual_sum_and_sse(&residuals);
        let sample_count = residuals.len() as i64;
        let expected = quantize_vvc_chroma_residual_dc_by_exhaustive_search(
            residual_sum,
            original_sse,
            sample_count,
            width,
            height,
            bit_depth,
            chroma_qp,
        );
        let actual = quantize_vvc_chroma_residual_dc_by_search(
            residual_sum,
            original_sse,
            sample_count,
            width,
            height,
            bit_depth,
            chroma_qp,
        );
        assert_eq!(
            actual, expected,
            "fast chroma DC search differed from exhaustive for {width}x{height} qp={chroma_qp} bit_depth={} sum={residual_sum} sse={original_sse}",
            bit_depth.bits()
        );
        let public = quantize_vvc_chroma_residual_dc_with_qp(
            &residuals, width, height, bit_depth, chroma_qp,
        );
        assert_eq!(
            public, expected,
            "public chroma DC quantizer differed from exhaustive for {width}x{height} qp={chroma_qp} bit_depth={}",
            bit_depth.bits()
        );
    }

    fn alternating_chroma_dc_residuals(width: usize, height: usize, probe: i16) -> Vec<i16> {
        (0..width * height)
            .map(|idx| if idx & 1 == 0 { probe } else { -(probe / 2) })
            .collect()
    }

    fn patterned_chroma_dc_residuals(
        width: usize,
        height: usize,
        probe: i16,
        max_residual: i16,
    ) -> Vec<i16> {
        let scale = i32::from(probe).abs().max(1);
        (0..width * height)
            .map(|idx| {
                let centered = ((idx * 37 + width * 11 + height * 7) % 17) as i32 - 8;
                ((centered * scale) / 8).clamp(-i32::from(max_residual), i32::from(max_residual))
                    as i16
            })
            .collect()
    }
}
