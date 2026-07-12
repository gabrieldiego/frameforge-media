use super::{palette::Av2ChromaIntraMode, Av2Sample};
use crate::picture::SampleBitDepth;

#[derive(Debug, Clone, Copy)]
pub(crate) struct ChromaD135Edges {
    pub(crate) above_left: Av2Sample,
    pub(crate) above: [Av2Sample; 4],
    pub(crate) left: [Av2Sample; 4],
}

pub(crate) fn paeth_predictor(
    left: Av2Sample,
    above: Av2Sample,
    above_left: Av2Sample,
) -> Av2Sample {
    let left = i32::from(left);
    let above = i32::from(above);
    let above_left = i32::from(above_left);
    let base = left + above - above_left;
    let p_left = (base - left).abs();
    let p_above = (base - above).abs();
    let p_above_left = (base - above_left).abs();
    if p_left <= p_above && p_left <= p_above_left {
        left as Av2Sample
    } else if p_above <= p_above_left {
        above as Av2Sample
    } else {
        above_left as Av2Sample
    }
}

pub(crate) fn av2_chroma_directional_angle(mode: Av2ChromaIntraMode) -> Option<i16> {
    match mode {
        Av2ChromaIntraMode::Directional45 => Some(45),
        Av2ChromaIntraMode::Directional67 => Some(67),
        Av2ChromaIntraMode::Vertical => Some(90),
        Av2ChromaIntraMode::Directional113 => Some(113),
        Av2ChromaIntraMode::Directional135 => Some(135),
        Av2ChromaIntraMode::Directional157 => Some(157),
        Av2ChromaIntraMode::Horizontal => Some(180),
        Av2ChromaIntraMode::Directional203 => Some(203),
        Av2ChromaIntraMode::Dc
        | Av2ChromaIntraMode::Smooth
        | Av2ChromaIntraMode::SmoothVertical
        | Av2ChromaIntraMode::SmoothHorizontal
        | Av2ChromaIntraMode::Paeth => None,
    }
}

pub(crate) fn av2_intra_residual4x4<Sample, Dc, H, V, AboveLeft, Directional, Smooth>(
    mode: Av2ChromaIntraMode,
    directional_angle: Option<i16>,
    bit_depth: SampleBitDepth,
    sample: Sample,
    dc_predictor: Dc,
    h_predictor: H,
    v_predictor: V,
    above_left_predictor: AboveLeft,
    directional_predictor: Directional,
    smooth_edges: Smooth,
) -> [i32; 16]
where
    Sample: Fn(usize, usize) -> Av2Sample,
    Dc: Fn() -> Av2Sample,
    H: Fn(usize) -> Av2Sample,
    V: Fn(usize) -> Av2Sample,
    AboveLeft: Fn() -> Av2Sample,
    Directional: Fn(i16, usize, usize) -> Av2Sample,
    Smooth: Fn() -> ([Av2Sample; 5], [Av2Sample; 5]),
{
    const TX4X4_SIZE: usize = 4;
    const TX4X4_SAMPLES: usize = TX4X4_SIZE * TX4X4_SIZE;

    let dc_predictor = (mode == Av2ChromaIntraMode::Dc).then(dc_predictor);
    let smooth_edges = matches!(
        mode,
        Av2ChromaIntraMode::Smooth
            | Av2ChromaIntraMode::SmoothVertical
            | Av2ChromaIntraMode::SmoothHorizontal
    )
    .then(smooth_edges);
    let mut residual = [0i32; TX4X4_SAMPLES];
    for local_y in 0..TX4X4_SIZE {
        for local_x in 0..TX4X4_SIZE {
            let predictor = match mode {
                Av2ChromaIntraMode::Dc => dc_predictor.expect("DC predictor is precomputed"),
                Av2ChromaIntraMode::Horizontal => match directional_angle {
                    Some(angle) if angle != 180 => directional_predictor(angle, local_x, local_y),
                    _ => h_predictor(local_y),
                },
                Av2ChromaIntraMode::Vertical => match directional_angle {
                    Some(angle) if angle != 90 => directional_predictor(angle, local_x, local_y),
                    _ => v_predictor(local_x),
                },
                Av2ChromaIntraMode::Directional45
                | Av2ChromaIntraMode::Directional67
                | Av2ChromaIntraMode::Directional135
                | Av2ChromaIntraMode::Directional113
                | Av2ChromaIntraMode::Directional157
                | Av2ChromaIntraMode::Directional203 => directional_predictor(
                    directional_angle.unwrap_or_else(|| {
                        av2_chroma_directional_angle(mode)
                            .expect("directional chroma mode must have an angle")
                    }),
                    local_x,
                    local_y,
                ),
                Av2ChromaIntraMode::Smooth
                | Av2ChromaIntraMode::SmoothVertical
                | Av2ChromaIntraMode::SmoothHorizontal => {
                    let (above, left) =
                        smooth_edges.expect("smooth predictor edges are precomputed");
                    av2_highbd_smooth_intra_predictor(
                        mode, above, left, local_x, local_y, bit_depth,
                    )
                }
                Av2ChromaIntraMode::Paeth => paeth_predictor(
                    h_predictor(local_y),
                    v_predictor(local_x),
                    above_left_predictor(),
                ),
            };
            residual[local_y * TX4X4_SIZE + local_x] =
                i32::from(sample(local_x, local_y)) - i32::from(predictor);
        }
    }

    residual
}

pub(crate) fn directional_interpolate(
    edge: [Av2Sample; 8],
    along: usize,
    across: usize,
) -> Av2Sample {
    // AVM dr_intra_derivative[67], used by both D67 and D203.
    const DERIVATIVE_67_203: i32 = 24;
    directional_interpolate_with_delta(edge, DERIVATIVE_67_203, along, across)
}

pub(crate) fn directional_interpolate_with_delta(
    edge: [Av2Sample; 8],
    derivative: i32,
    along: usize,
    across: usize,
) -> Av2Sample {
    let projected = derivative as usize * (across + 1);
    let base = (projected >> 6) + along;
    if base >= edge.len() - 1 {
        return edge[edge.len() - 1];
    }
    let shift = (projected & 0x3f) >> 1;
    let value =
        u32::from(edge[base]) * (32 - shift) as u32 + u32::from(edge[base + 1]) * shift as u32;
    ((value + 16) >> 5) as Av2Sample
}

pub(crate) fn zone2_directional_predictor(
    edges: ChromaD135Edges,
    dx: i32,
    dy: i32,
    local_x: usize,
    local_y: usize,
) -> Av2Sample {
    let projected_x = ((local_x as i32) << 6) - ((local_y as i32 + 1) * dx);
    let base_x = projected_x >> 6;
    if base_x >= -1 {
        let shift = ((projected_x & 0x3f) >> 1) as usize;
        return directional_weighted_sample(
            zone2_above_sample(edges, base_x),
            zone2_above_sample(edges, base_x + 1),
            shift,
        );
    }

    let projected_y = ((local_y as i32) << 6) - ((local_x as i32 + 1) * dy);
    let base_y = projected_y >> 6;
    debug_assert!(base_y >= -1);
    let shift = ((projected_y & 0x3f) >> 1) as usize;
    directional_weighted_sample(
        zone2_left_sample(edges, base_y),
        zone2_left_sample(edges, base_y + 1),
        shift,
    )
}

fn zone2_above_sample(edges: ChromaD135Edges, offset: i32) -> Av2Sample {
    if offset < 0 {
        edges.above_left
    } else {
        edges.above[offset as usize]
    }
}

fn zone2_left_sample(edges: ChromaD135Edges, offset: i32) -> Av2Sample {
    if offset < 0 {
        edges.above_left
    } else {
        edges.left[offset as usize]
    }
}

fn directional_weighted_sample(first: Av2Sample, second: Av2Sample, shift: usize) -> Av2Sample {
    let value = u32::from(first) * (32 - shift) as u32 + u32::from(second) * shift as u32;
    ((value + 16) >> 5) as Av2Sample
}

pub(crate) fn av2_highbd_smooth_intra_predictor(
    mode: Av2ChromaIntraMode,
    above: [Av2Sample; 5],
    left: [Av2Sample; 5],
    local_x: usize,
    local_y: usize,
    bit_depth: SampleBitDepth,
) -> Av2Sample {
    let (smooth, smooth_v, smooth_h) =
        av2_highbd_smooth_intra_predictor_set(above, left, local_x, local_y, bit_depth);
    match mode {
        Av2ChromaIntraMode::Smooth => smooth,
        Av2ChromaIntraMode::SmoothVertical => smooth_v,
        Av2ChromaIntraMode::SmoothHorizontal => smooth_h,
        _ => unreachable!("smooth predictor only supports smooth chroma modes"),
    }
}

pub(crate) fn av2_highbd_smooth_intra_predictor_set(
    above: [Av2Sample; 5],
    left: [Av2Sample; 5],
    local_x: usize,
    local_y: usize,
    bit_depth: SampleBitDepth,
) -> (Av2Sample, Av2Sample, Av2Sample) {
    debug_assert!(local_x < 4 && local_y < 4);
    const BLEND_WEIGHT_MAX: i32 = 32;
    const BLEND_MAX_LOG2: u8 = 5;
    const TX_LOG2: u8 = 2;

    fn divide_round(value: i32, bits: u8) -> i32 {
        (value + (1 << (bits - 1))) >> bits
    }

    let top = i32::from(above[local_x]);
    let left_sample = i32::from(left[local_y]);
    let bottom_left = i32::from(left[4]);
    let top_right = i32::from(above[4]);
    let row_weight = BLEND_WEIGHT_MAX >> ((local_y << 1).min(6) as u8);
    let col_weight = BLEND_WEIGHT_MAX >> ((local_x << 1).min(6) as u8);
    let pred_v = bottom_left + divide_round((top - bottom_left) * (3 - local_y) as i32, TX_LOG2);
    let pred_h =
        top_right + divide_round((left_sample - top_right) * (3 - local_x) as i32, TX_LOG2);
    let pred_v = pred_v + divide_round((top - pred_v) * row_weight, BLEND_MAX_LOG2 + 1);
    let pred_h = pred_h + divide_round((left_sample - pred_h) * col_weight, BLEND_MAX_LOG2 + 1);
    let max_sample = i32::from(bit_depth.max_sample());
    (
        divide_round(pred_v + pred_h, 1).clamp(0, max_sample) as Av2Sample,
        pred_v.clamp(0, max_sample) as Av2Sample,
        pred_h.clamp(0, max_sample) as Av2Sample,
    )
}
