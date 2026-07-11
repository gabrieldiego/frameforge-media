#[derive(Debug, Clone, Copy)]
struct DirectionalIdifEdges {
    above: [Av2Sample; 12],
    left: [Av2Sample; 12],
}

impl DirectionalIdifEdges {
    const OFFSET: i32 = 2;

    fn new(bit_depth: SampleBitDepth) -> Self {
        Self {
            above: [av2_lossless_v_pred_above_edge(bit_depth); 12],
            left: [av2_lossless_h_pred_left_edge(bit_depth); 12],
        }
    }

    fn above(self, index: i32) -> Av2Sample {
        self.above[(index + Self::OFFSET) as usize]
    }

    fn left(self, index: i32) -> Av2Sample {
        self.left[(index + Self::OFFSET) as usize]
    }

    fn set_above(&mut self, index: i32, sample: Av2Sample) {
        self.above[(index + Self::OFFSET) as usize] = sample;
    }

    fn set_left(&mut self, index: i32, sample: Av2Sample) {
        self.left[(index + Self::OFFSET) as usize] = sample;
    }
}

const AV2_DR_INTERP_FILTER_BITS: u32 = 7;
const AV2_DR_INTRA_DERIVATIVE: [i32; 90] = [
    0, 4096, 2048, 1365, 1024, 819, 682, 585, 512, 455, 409, 409, 409, 372, 341, 292, 273, 256,
    227, 215, 204, 186, 178, 170, 157, 151, 146, 136, 132, 128, 117, 110, 107, 99, 97, 97, 93, 87,
    83, 81, 77, 74, 73, 69, 66, 64, 62, 59, 56, 55, 53, 50, 49, 47, 44, 42, 42, 41, 38, 37, 35, 32,
    31, 30, 28, 27, 26, 24, 23, 22, 20, 19, 18, 16, 15, 14, 12, 11, 10, 10, 10, 9, 8, 7, 6, 5, 4,
    3, 2, 1,
];
const AV2_DR_INTERP_FILTER: [[i32; 4]; 32] = [
    [0, 128, 0, 0],
    [-2, 127, 4, -1],
    [-3, 125, 8, -2],
    [-5, 123, 13, -3],
    [-6, 121, 17, -4],
    [-7, 118, 22, -5],
    [-9, 116, 27, -6],
    [-9, 112, 32, -7],
    [-10, 109, 37, -8],
    [-11, 106, 41, -8],
    [-11, 102, 46, -9],
    [-12, 98, 52, -10],
    [-12, 94, 56, -10],
    [-12, 90, 61, -11],
    [-12, 85, 66, -11],
    [-12, 81, 71, -12],
    [-12, 76, 76, -12],
    [-12, 71, 81, -12],
    [-11, 66, 85, -12],
    [-11, 61, 90, -12],
    [-10, 56, 94, -12],
    [-10, 52, 98, -12],
    [-9, 46, 102, -11],
    [-8, 41, 106, -11],
    [-8, 37, 109, -10],
    [-7, 32, 112, -9],
    [-6, 27, 116, -9],
    [-5, 22, 118, -7],
    [-4, 17, 121, -6],
    [-3, 13, 123, -5],
    [-2, 8, 125, -3],
    [-1, 4, 127, -2],
];

fn av2_directional_dx(angle: i16) -> i32 {
    if angle > 0 && angle < 90 {
        AV2_DR_INTRA_DERIVATIVE[angle as usize]
    } else if angle > 90 && angle < 180 {
        AV2_DR_INTRA_DERIVATIVE[(180 - angle) as usize]
    } else {
        1
    }
}

fn av2_directional_dy(angle: i16) -> i32 {
    if angle > 90 && angle < 180 {
        AV2_DR_INTRA_DERIVATIVE[(angle - 90) as usize]
    } else if angle > 180 && angle < 270 {
        AV2_DR_INTRA_DERIVATIVE[(270 - angle) as usize]
    } else {
        1
    }
}

fn luma_directional_idif_predictor(
    angle: i16,
    edges: DirectionalIdifEdges,
    local_x: usize,
    local_y: usize,
    bit_depth: SampleBitDepth,
) -> Av2Sample {
    if angle > 0 && angle < 90 {
        return luma_directional_idif_zone1(
            edges,
            av2_directional_dx(angle),
            local_x,
            local_y,
            bit_depth,
        );
    }
    if angle > 90 && angle < 180 {
        return luma_directional_idif_zone2(
            edges,
            av2_directional_dx(angle),
            av2_directional_dy(angle),
            local_x,
            local_y,
            bit_depth,
        );
    }
    if angle > 180 && angle < 270 {
        return luma_directional_idif_zone3(
            edges,
            av2_directional_dy(angle),
            local_x,
            local_y,
            bit_depth,
        );
    }
    unreachable!("IDIF is only used for non-cardinal luma directional angles")
}

fn luma_directional_idif_zone1(
    edges: DirectionalIdifEdges,
    dx: i32,
    local_x: usize,
    local_y: usize,
    bit_depth: SampleBitDepth,
) -> Av2Sample {
    let projected_x = dx * (local_y as i32 + 1);
    let base = (projected_x >> 6) + local_x as i32;
    let shift = ((projected_x & 0x3f) >> 1) as usize;
    if base <= 7 {
        luma_directional_idif_filter_above(edges, base, shift, bit_depth)
    } else {
        edges.above(7)
    }
}

fn luma_directional_idif_zone2(
    edges: DirectionalIdifEdges,
    dx: i32,
    dy: i32,
    local_x: usize,
    local_y: usize,
    bit_depth: SampleBitDepth,
) -> Av2Sample {
    let projected_x = ((local_x as i32) << 6) - (local_y as i32 + 1) * dx;
    let base_x = projected_x >> 6;
    if base_x >= -1 {
        let shift = ((projected_x & 0x3f) >> 1) as usize;
        return luma_directional_idif_filter_above(edges, base_x, shift, bit_depth);
    }

    let projected_y = ((local_y as i32) << 6) - (local_x as i32 + 1) * dy;
    let base_y = projected_y >> 6;
    debug_assert!(base_y >= -1);
    let shift = ((projected_y & 0x3f) >> 1) as usize;
    luma_directional_idif_filter_left(edges, base_y, shift, bit_depth)
}

fn luma_directional_idif_zone3(
    edges: DirectionalIdifEdges,
    dy: i32,
    local_x: usize,
    local_y: usize,
    bit_depth: SampleBitDepth,
) -> Av2Sample {
    let projected_y = dy * (local_x as i32 + 1);
    let base = (projected_y >> 6) + local_y as i32;
    let shift = ((projected_y & 0x3f) >> 1) as usize;
    if base <= 7 {
        luma_directional_idif_filter_left(edges, base, shift, bit_depth)
    } else {
        edges.left(7)
    }
}

fn luma_directional_idif_filter_above(
    edges: DirectionalIdifEdges,
    base: i32,
    shift: usize,
    bit_depth: SampleBitDepth,
) -> Av2Sample {
    luma_directional_idif_filter(
        [
            edges.above(base - 1),
            edges.above(base),
            edges.above(base + 1),
            edges.above(base + 2),
        ],
        shift,
        bit_depth,
    )
}

fn luma_directional_idif_filter_left(
    edges: DirectionalIdifEdges,
    base: i32,
    shift: usize,
    bit_depth: SampleBitDepth,
) -> Av2Sample {
    luma_directional_idif_filter(
        [
            edges.left(base - 1),
            edges.left(base),
            edges.left(base + 1),
            edges.left(base + 2),
        ],
        shift,
        bit_depth,
    )
}

fn luma_directional_idif_filter(
    refs: [Av2Sample; 4],
    shift: usize,
    bit_depth: SampleBitDepth,
) -> Av2Sample {
    let filter = AV2_DR_INTERP_FILTER[shift];
    let value = filter[0] * i32::from(refs[0])
        + filter[1] * i32::from(refs[1])
        + filter[2] * i32::from(refs[2])
        + filter[3] * i32::from(refs[3]);
    let rounded = (value + (1 << (AV2_DR_INTERP_FILTER_BITS - 1))) >> AV2_DR_INTERP_FILTER_BITS;
    rounded.clamp(0, i32::from(bit_depth.max_sample())) as Av2Sample
}

fn chroma_d135_edges(
    palette: &Av2LumaPalette444,
    plane: Av2ChromaPlane,
    txb_x0: usize,
    txb_y0: usize,
    tile_origin_x: usize,
    tile_origin_y: usize,
) -> ChromaD135Edges {
    let have_top = txb_y0 > tile_origin_y;
    let have_left = txb_x0 > tile_origin_x;
    let mut above = [av2_lossless_v_pred_above_edge(palette.bit_depth()); 4];
    let mut left = [av2_lossless_h_pred_left_edge(palette.bit_depth()); 4];
    if have_top {
        for local_x in 0..4 {
            above[local_x] = chroma_sample(palette, plane, txb_x0 + local_x, txb_y0 - 1);
        }
    } else if have_left {
        above.fill(chroma_sample(palette, plane, txb_x0 - 1, txb_y0));
    }
    if have_left {
        for local_y in 0..4 {
            left[local_y] = chroma_sample(palette, plane, txb_x0 - 1, txb_y0 + local_y);
        }
    } else if have_top {
        left.fill(chroma_sample(palette, plane, txb_x0, txb_y0 - 1));
    }
    let above_left = if have_top && have_left {
        chroma_sample(palette, plane, txb_x0 - 1, txb_y0 - 1)
    } else if have_top {
        above[0]
    } else if have_left {
        left[0]
    } else {
        av2_lossless_dc_predictor(palette.bit_depth())
    };
    ChromaD135Edges {
        above_left,
        above,
        left,
    }
}

fn chroma_d203_left_edge(
    palette: &Av2LumaPalette444,
    plane: Av2ChromaPlane,
    txb_x0: usize,
    txb_y0: usize,
    tile_origin_x: usize,
    tile_origin_y: usize,
    leaf_x0: usize,
    leaf_y0: usize,
    leaf_height: usize,
    coded_mi_context: &Av2CodedMiContext,
) -> [Av2Sample; 8] {
    let sb_origin_y = (txb_y0 / MVP_SUPERBLOCK_SIZE) * MVP_SUPERBLOCK_SIZE;
    let sb_bottom = (sb_origin_y + MVP_SUPERBLOCK_SIZE).min(palette.height());
    let have_top = txb_y0 > tile_origin_y;
    let have_left = txb_x0 > tile_origin_x;
    let mut left = [av2_lossless_h_pred_left_edge(palette.bit_depth()); 8];
    if have_left {
        for index in 0..left.len() {
            let y = txb_y0 + index;
            let external_bottom_left_coded = txb_x0 == leaf_x0
                && y < sb_bottom
                && coded_mi_context.is_coded(y / MI_SIZE, (txb_x0 - 1) / MI_SIZE);
            // Match AVM has_bottom_left(): only TXBs on the leaf's left edge
            // may use D203 bottom-left overhang samples.
            if y < txb_y0 + TX4X4_SIZE
                || (txb_x0 == leaf_x0 && (y < leaf_y0 + leaf_height || external_bottom_left_coded))
            {
                left[index] = chroma_sample(palette, plane, txb_x0 - 1, y);
            } else if index > 0 {
                left[index] = left[index - 1];
            }
        }
    } else if have_top {
        left.fill(chroma_sample(palette, plane, txb_x0, txb_y0 - 1));
    }
    left
}

fn chroma_above_left_predictor(
    palette: &Av2LumaPalette444,
    plane: Av2ChromaPlane,
    x0: usize,
    y0: usize,
    tile_origin_x: usize,
    tile_origin_y: usize,
) -> Av2Sample {
    let have_left = x0 > tile_origin_x;
    let have_top = y0 > tile_origin_y;
    if have_left && have_top {
        chroma_sample(palette, plane, x0 - 1, y0 - 1)
    } else if have_top {
        chroma_sample(palette, plane, x0, y0 - 1)
    } else if have_left {
        chroma_sample(palette, plane, x0 - 1, y0)
    } else {
        av2_lossless_dc_predictor(palette.bit_depth())
    }
}

fn chroma_smooth_edges(
    palette: &Av2LumaPalette444,
    plane: Av2ChromaPlane,
    txb_x0: usize,
    txb_y0: usize,
    tile_origin_x: usize,
    tile_origin_y: usize,
    leaf_x0: usize,
    leaf_y0: usize,
    leaf_width: usize,
    leaf_height: usize,
    coded_mi_context: &Av2CodedMiContext,
) -> ([Av2Sample; 5], [Av2Sample; 5]) {
    debug_assert!(txb_x0 >= leaf_x0 && txb_y0 >= leaf_y0);
    debug_assert!(txb_x0 + TX4X4_SIZE <= leaf_x0 + leaf_width);
    debug_assert!(txb_y0 + TX4X4_SIZE <= leaf_y0 + leaf_height);
    let have_top = txb_y0 > tile_origin_y;
    let have_left = txb_x0 > tile_origin_x;
    let mut above = [av2_lossless_v_pred_above_edge(palette.bit_depth()); TX4X4_SIZE + 1];
    let mut left = [av2_lossless_h_pred_left_edge(palette.bit_depth()); TX4X4_SIZE + 1];

    if have_top {
        for local_x in 0..TX4X4_SIZE {
            above[local_x] = chroma_sample(palette, plane, txb_x0 + local_x, txb_y0 - 1);
        }
    } else if have_left {
        above[..TX4X4_SIZE].fill(chroma_sample(palette, plane, txb_x0 - 1, txb_y0));
    }

    if have_left {
        for local_y in 0..TX4X4_SIZE {
            left[local_y] = chroma_sample(palette, plane, txb_x0 - 1, txb_y0 + local_y);
        }
    } else if have_top {
        left[..TX4X4_SIZE].fill(chroma_sample(palette, plane, txb_x0, txb_y0 - 1));
    }

    let sb_origin_x = (txb_x0 / MVP_SUPERBLOCK_SIZE) * MVP_SUPERBLOCK_SIZE;
    let sb_right = (sb_origin_x + MVP_SUPERBLOCK_SIZE).min(palette.width());
    let external_top_right_coded = have_top
        && txb_y0 == leaf_y0
        && txb_x0 + TX4X4_SIZE < sb_right
        && coded_mi_context.is_coded((txb_y0 - 1) / MI_SIZE, (txb_x0 + TX4X4_SIZE) / MI_SIZE);
    if have_top && (txb_x0 + TX4X4_SIZE < leaf_x0 + leaf_width || external_top_right_coded) {
        above[TX4X4_SIZE] = chroma_sample(palette, plane, txb_x0 + TX4X4_SIZE, txb_y0 - 1);
    } else {
        above[TX4X4_SIZE] = above[TX4X4_SIZE - 1];
    }

    let sb_origin_y = (txb_y0 / MVP_SUPERBLOCK_SIZE) * MVP_SUPERBLOCK_SIZE;
    let sb_bottom = (sb_origin_y + MVP_SUPERBLOCK_SIZE).min(palette.height());
    let external_bottom_left_coded = have_left
        && txb_x0 == leaf_x0
        && txb_y0 + TX4X4_SIZE < sb_bottom
        && coded_mi_context.is_coded((txb_y0 + TX4X4_SIZE) / MI_SIZE, (txb_x0 - 1) / MI_SIZE);
    if have_left
        && txb_x0 == leaf_x0
        && (txb_y0 + TX4X4_SIZE < leaf_y0 + leaf_height || external_bottom_left_coded)
    {
        left[TX4X4_SIZE] = chroma_sample(palette, plane, txb_x0 - 1, txb_y0 + TX4X4_SIZE);
    } else {
        left[TX4X4_SIZE] = left[TX4X4_SIZE - 1];
    }

    (above, left)
}

fn chroma_dc_predictor(
    palette: &Av2LumaPalette444,
    plane: Av2ChromaPlane,
    x0: usize,
    y0: usize,
) -> Av2Sample {
    let tile_origin_x = (x0 / MVP_SUPERBLOCK_SIZE) * MVP_SUPERBLOCK_SIZE;
    let tile_origin_y = (y0 / MVP_SUPERBLOCK_SIZE) * MVP_SUPERBLOCK_SIZE;
    let have_left = x0 != tile_origin_x;
    let have_top = y0 != tile_origin_y;
    if !have_left && !have_top {
        return av2_lossless_dc_predictor(palette.bit_depth());
    }

    let mut sum = 0u32;
    let mut count = 0u32;
    if have_top {
        for local_x in 0..TX4X4_SIZE {
            sum += u32::from(chroma_sample(palette, plane, x0 + local_x, y0 - 1));
            count += 1;
        }
    }
    if have_left {
        for local_y in 0..TX4X4_SIZE {
            sum += u32::from(chroma_sample(palette, plane, x0 - 1, y0 + local_y));
            count += 1;
        }
    }
    ((sum + count / 2) / count) as Av2Sample
}

fn luma_h_predictor(
    palette: &Av2LumaPalette444,
    x0: usize,
    y0: usize,
    local_y: usize,
    tile_origin_x: usize,
    tile_origin_y: usize,
) -> Av2Sample {
    // AV2 v1.0.0 Section 7.11 H_PRED with lossless DPCM uses the same
    // intra-prediction edge as normal horizontal prediction before
    // avm_highbd_subtract_block_horz() differentials src-pred.
    if x0 > tile_origin_x {
        palette.y_sample(x0 - 1, y0 + local_y)
    } else if y0 > tile_origin_y {
        palette.y_sample(x0, y0 - 1)
    } else {
        av2_lossless_h_pred_left_edge(palette.bit_depth())
    }
}

fn luma_v_predictor(
    palette: &Av2LumaPalette444,
    x0: usize,
    y0: usize,
    local_x: usize,
    tile_origin_x: usize,
    tile_origin_y: usize,
) -> Av2Sample {
    // AV2 v1.0.0 Section 7.11 V_PRED with lossless DPCM uses the same
    // intra-prediction edge as normal vertical prediction before
    // avm_highbd_subtract_block_vert() differentials src-pred.
    if y0 > tile_origin_y {
        palette.y_sample(x0 + local_x, y0 - 1)
    } else if x0 > tile_origin_x {
        palette.y_sample(x0 - 1, y0)
    } else {
        av2_lossless_v_pred_above_edge(palette.bit_depth())
    }
}

fn chroma_h_predictor(
    palette: &Av2LumaPalette444,
    plane: Av2ChromaPlane,
    x0: usize,
    y0: usize,
    local_y: usize,
    tile_origin_x: usize,
    tile_origin_y: usize,
) -> Av2Sample {
    // AV2 v1.0.0 Section 7.11 intra prediction, mirrored from AVM
    // av2_build_intra_predictors_high(): H_PRED uses the left reference
    // column; if the left edge is unavailable, AVM falls back to above[0] when
    // available and to base+1 at the top-left tile/frame corner. Independent
    // 64x64 superblock tiles must not borrow the left/top predictor from the
    // previous tile even though the global frame coordinate is non-zero.
    if x0 > tile_origin_x {
        chroma_sample(palette, plane, x0 - 1, y0 + local_y)
    } else if y0 > tile_origin_y {
        chroma_sample(palette, plane, x0, y0 - 1)
    } else {
        av2_lossless_h_pred_left_edge(palette.bit_depth())
    }
}

fn chroma_v_predictor(
    palette: &Av2LumaPalette444,
    plane: Av2ChromaPlane,
    x0: usize,
    y0: usize,
    local_x: usize,
    tile_origin_x: usize,
    tile_origin_y: usize,
) -> Av2Sample {
    // AV2 v1.0.0 Section 7.11 intra prediction, implemented in AVM
    // reconintra.c: V_PRED uses the above reference row. If the above edge is
    // unavailable, AVM fills it from left[0] when left is available and from
    // base-1 at the tile top-left. Independent 64x64 tiles must not borrow
    // predictors across tile boundaries.
    if y0 > tile_origin_y {
        chroma_sample(palette, plane, x0 + local_x, y0 - 1)
    } else if x0 > tile_origin_x {
        chroma_sample(palette, plane, x0 - 1, y0)
    } else {
        av2_lossless_v_pred_above_edge(palette.bit_depth())
    }
}

fn chroma_sample(
    palette: &Av2LumaPalette444,
    plane: Av2ChromaPlane,
    x: usize,
    y: usize,
) -> Av2Sample {
    match plane {
        Av2ChromaPlane::U => palette.u_sample(x, y),
        Av2ChromaPlane::V => palette.v_sample(x, y),
    }
}
