use super::palette::Av2LumaPalette444;
use super::planar::Av2PlanarYuvLayout;
use super::tile::{
    av2_luma_palette_leaf_order_for_region, av2_mvp_8x8_leaf_order_for_region, Av2MvpLeafRegion,
};
use super::{Av2ChromaFormat, Av2VideoGeometry};
use crate::picture::{Picture, PixelFormat, SampleBitDepth};

pub(crate) const AV2_IBC_HASH_BLOCK_SIZE: usize = 8;
const AV2_IBC_TILE_SIZE: usize = 64;
const AV2_IBC_HASH_OFFSET: u32 = 0x811c_9dc5;
const AV2_IBC_MAX_BVP_SIZE: usize = 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Av2LocalIbcVector {
    row_px: i16,
    col_px: i16,
}

const AV2_IBC_BV_SUPERBLOCK_ABOVE: Av2LocalIbcVector = Av2LocalIbcVector {
    row_px: -64,
    col_px: 0,
};
const AV2_IBC_BV_SUPERBLOCK_DELAYED_LEFT: Av2LocalIbcVector = Av2LocalIbcVector {
    row_px: 0,
    col_px: -320,
};
const AV2_IBC_BV_ABOVE_8X8: Av2LocalIbcVector = Av2LocalIbcVector {
    row_px: -8,
    col_px: 0,
};
const AV2_IBC_BV_LEFT_8X8: Av2LocalIbcVector = Av2LocalIbcVector {
    row_px: 0,
    col_px: -8,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Av2IntrabcExplicitDv {
    pub(crate) drl_idx: u8,
    pub(crate) mv_row: i16,
    pub(crate) mv_col: i16,
    pub(crate) ref_row: i16,
    pub(crate) ref_col: i16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Av2LocalIbcCopy {
    DirectDrl {
        drl_idx: u8,
    },
    #[allow(dead_code)]
    ExplicitDv(Av2IntrabcExplicitDv),
}

impl Av2LocalIbcCopy {
    pub(crate) fn drl_idx(self) -> u8 {
        match self {
            Self::DirectDrl { drl_idx } => drl_idx,
            Self::ExplicitDv(dv) => dv.drl_idx,
        }
    }

    pub(crate) fn explicit_dv(self) -> Option<Av2IntrabcExplicitDv> {
        match self {
            Self::DirectDrl { .. } => None,
            Self::ExplicitDv(dv) => Some(dv),
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct Av2LocalIbcStats {
    pub(crate) total_blocks: usize,
    pub(crate) blocks_with_above_in_tile: usize,
    pub(crate) blocks_with_left_in_tile: usize,
    pub(crate) fixed_drl_supported_blocks: usize,
    pub(crate) raw_above_hash_matches: usize,
    pub(crate) raw_left_hash_matches: usize,
    pub(crate) direct_above_hash_matches: usize,
    pub(crate) direct_left_hash_matches: usize,
    pub(crate) above_hash_matches_blocked_by_fixed_drl_guard: usize,
    pub(crate) left_hash_matches_blocked_by_fixed_drl_guard: usize,
    pub(crate) above_hash_matches_blocked_by_copied_candidate: usize,
    pub(crate) left_hash_matches_blocked_by_copied_candidate: usize,
    pub(crate) selected_above_copy_blocks: usize,
    pub(crate) selected_left_copy_blocks: usize,
}

impl Av2LocalIbcStats {
    pub(crate) fn selected_copy_blocks(self) -> usize {
        self.selected_above_copy_blocks + self.selected_left_copy_blocks
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Av2LocalIbcBlock444 {
    hash: u32,
    candidate_copy: Option<Av2LocalIbcCopy>,
    copy_vector: Option<Av2LocalIbcVector>,
}

#[derive(Debug, Default)]
struct Av2TileIbcHashIndex {
    buckets: Vec<Av2TileIbcHashBucket>,
}

#[derive(Debug)]
struct Av2TileIbcHashBucket {
    hash: u32,
    block_indices: Vec<usize>,
}

impl Av2TileIbcHashIndex {
    fn insert(&mut self, hash: u32, block_index: usize) {
        let bucket =
            if let Some(bucket) = self.buckets.iter_mut().find(|bucket| bucket.hash == hash) {
                bucket
            } else {
                self.buckets.push(Av2TileIbcHashBucket {
                    hash,
                    block_indices: Vec::new(),
                });
                self.buckets
                    .last_mut()
                    .expect("new IBC hash bucket was just inserted")
            };
        match bucket.block_indices.binary_search(&block_index) {
            Ok(_) => {}
            Err(index) => bucket.block_indices.insert(index, block_index),
        }
    }

    fn candidates(&self, hash: u32) -> &[usize] {
        self.buckets
            .iter()
            .find(|bucket| bucket.hash == hash)
            .map_or(&[], |bucket| bucket.block_indices.as_slice())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Av2LocalIbc444 {
    blocks: Vec<Av2LocalIbcBlock444>,
    blocks_wide: usize,
    blocks_high: usize,
    any_copy: bool,
    stats: Av2LocalIbcStats,
}

impl Av2LocalIbc444 {
    #[cfg(test)]
    pub(crate) fn any_copy(&self) -> bool {
        self.any_copy
    }

    pub(crate) fn candidate_copy(&self, x0: usize, y0: usize) -> Option<Av2LocalIbcCopy> {
        assert_eq!(x0 % AV2_IBC_HASH_BLOCK_SIZE, 0);
        assert_eq!(y0 % AV2_IBC_HASH_BLOCK_SIZE, 0);
        let block_x = x0 / AV2_IBC_HASH_BLOCK_SIZE;
        let block_y = y0 / AV2_IBC_HASH_BLOCK_SIZE;
        assert!(block_x < self.blocks_wide && block_y < self.blocks_high);
        self.blocks[block_y * self.blocks_wide + block_x].candidate_copy
    }

    #[cfg(test)]
    pub(crate) fn candidate_drl_idx(&self, x0: usize, y0: usize) -> Option<u8> {
        self.candidate_copy(x0, y0).map(Av2LocalIbcCopy::drl_idx)
    }

    pub(crate) fn stats(&self) -> Av2LocalIbcStats {
        self.stats
    }
}

#[cfg(test)]
pub(crate) fn build_local_ibc_444(
    frame: &[u8],
    geometry: Av2VideoGeometry,
) -> Result<Av2LocalIbc444, String> {
    build_local_ibc_with_palette(frame, geometry, Av2ChromaFormat::Yuv444, None)
}

pub(crate) fn build_local_ibc_444_for_palette(
    frame: &[u8],
    geometry: Av2VideoGeometry,
    palette: &Av2LumaPalette444,
) -> Result<Av2LocalIbc444, String> {
    build_local_ibc_with_palette(frame, geometry, Av2ChromaFormat::Yuv444, Some(palette))
}

pub(crate) fn build_local_ibc_subsampled(
    frame: &[u8],
    geometry: Av2VideoGeometry,
    chroma_format: Av2ChromaFormat,
    bit_depth: SampleBitDepth,
) -> Result<Av2LocalIbc444, String> {
    assert!(
        matches!(
            chroma_format,
            Av2ChromaFormat::Yuv420 | Av2ChromaFormat::Yuv422 | Av2ChromaFormat::Yuv444
        ),
        "AV2 planar IBC expects 4:2:0, 4:2:2, or 4:4:4 input"
    );
    build_local_ibc(frame, geometry, chroma_format, bit_depth, None)
}

fn build_local_ibc_with_palette(
    frame: &[u8],
    geometry: Av2VideoGeometry,
    chroma_format: Av2ChromaFormat,
    palette: Option<&Av2LumaPalette444>,
) -> Result<Av2LocalIbc444, String> {
    let bit_depth = palette
        .map(Av2LumaPalette444::bit_depth)
        .unwrap_or_else(|| SampleBitDepth::new(8).expect("8-bit depth is supported"));
    build_local_ibc(frame, geometry, chroma_format, bit_depth, palette)
}

fn build_local_ibc(
    frame: &[u8],
    geometry: Av2VideoGeometry,
    chroma_format: Av2ChromaFormat,
    bit_depth: SampleBitDepth,
    palette: Option<&Av2LumaPalette444>,
) -> Result<Av2LocalIbc444, String> {
    if palette.is_some() {
        assert_eq!(
            chroma_format,
            Av2ChromaFormat::Yuv444,
            "AV2 palette IBC expects 4:4:4 input"
        );
    }
    let format = PixelFormat::planar_yuv(chroma_format.chroma_sampling(), bit_depth);
    let expected_len = Picture::expected_len(geometry.width, geometry.height, format);
    if frame.len() != expected_len {
        return Err(format!(
            "AV2 {:?} IBC input length mismatch: expected {expected_len} byte(s), got {}",
            chroma_format,
            frame.len()
        ));
    }
    if geometry.width % AV2_IBC_HASH_BLOCK_SIZE != 0
        || geometry.height % AV2_IBC_HASH_BLOCK_SIZE != 0
    {
        return Err(format!(
            "AV2 IBC hash path expects dimensions in {}-pixel units, got {}x{}",
            AV2_IBC_HASH_BLOCK_SIZE, geometry.width, geometry.height
        ));
    }

    let blocks_wide = geometry.width / AV2_IBC_HASH_BLOCK_SIZE;
    let blocks_high = geometry.height / AV2_IBC_HASH_BLOCK_SIZE;
    let mut blocks = vec![
        Av2LocalIbcBlock444 {
            hash: 0,
            candidate_copy: None,
            copy_vector: None,
        };
        blocks_wide * blocks_high
    ];
    for block_y in 0..blocks_high {
        for block_x in 0..blocks_wide {
            let x0 = block_x * AV2_IBC_HASH_BLOCK_SIZE;
            let y0 = block_y * AV2_IBC_HASH_BLOCK_SIZE;
            blocks[block_y * blocks_wide + block_x].hash =
                hash_planar_yuv_8x8(frame, geometry, chroma_format, bit_depth, x0, y0);
        }
    }

    let mut any_copy = false;
    let mut stats = Av2LocalIbcStats::default();
    let mut coded_blocks = vec![false; blocks_wide * blocks_high];

    for tile_y0 in (0..blocks_high).step_by(AV2_IBC_TILE_SIZE / AV2_IBC_HASH_BLOCK_SIZE) {
        for tile_x0 in (0..blocks_wide).step_by(AV2_IBC_TILE_SIZE / AV2_IBC_HASH_BLOCK_SIZE) {
            let tile_blocks_wide =
                (blocks_wide - tile_x0).min(AV2_IBC_TILE_SIZE / AV2_IBC_HASH_BLOCK_SIZE);
            let tile_blocks_high =
                (blocks_high - tile_y0).min(AV2_IBC_TILE_SIZE / AV2_IBC_HASH_BLOCK_SIZE);
            let leaf_order = if let Some(palette) = palette {
                av2_luma_palette_leaf_order_for_region(
                    tile_x0 * AV2_IBC_HASH_BLOCK_SIZE,
                    tile_y0 * AV2_IBC_HASH_BLOCK_SIZE,
                    tile_blocks_wide * AV2_IBC_HASH_BLOCK_SIZE,
                    tile_blocks_high * AV2_IBC_HASH_BLOCK_SIZE,
                    palette,
                )
            } else {
                av2_mvp_8x8_leaf_order_for_region(
                    tile_blocks_wide * AV2_IBC_HASH_BLOCK_SIZE,
                    tile_blocks_high * AV2_IBC_HASH_BLOCK_SIZE,
                )
                .into_iter()
                .map(|(x, y)| Av2MvpLeafRegion {
                    x,
                    y,
                    width: AV2_IBC_HASH_BLOCK_SIZE,
                    height: AV2_IBC_HASH_BLOCK_SIZE,
                })
                .collect()
            };
            let mut hash_index = Av2TileIbcHashIndex::default();
            for leaf in leaf_order {
                assert_eq!(leaf.x % AV2_IBC_HASH_BLOCK_SIZE, 0);
                assert_eq!(leaf.y % AV2_IBC_HASH_BLOCK_SIZE, 0);
                assert_eq!(leaf.width % AV2_IBC_HASH_BLOCK_SIZE, 0);
                assert_eq!(leaf.height % AV2_IBC_HASH_BLOCK_SIZE, 0);
                let block_x = tile_x0 + leaf.x / AV2_IBC_HASH_BLOCK_SIZE;
                let block_y = tile_y0 + leaf.y / AV2_IBC_HASH_BLOCK_SIZE;
                if leaf.width == AV2_IBC_HASH_BLOCK_SIZE && leaf.height == AV2_IBC_HASH_BLOCK_SIZE {
                    visit_local_ibc_block(
                        &mut blocks,
                        &mut coded_blocks,
                        frame,
                        geometry,
                        chroma_format,
                        bit_depth,
                        blocks_wide,
                        blocks_high,
                        block_x,
                        block_y,
                        &mut any_copy,
                        &mut stats,
                        &mut hash_index,
                    );
                } else {
                    mark_local_ibc_intra_leaf(
                        &mut blocks,
                        &mut coded_blocks,
                        blocks_wide,
                        block_x,
                        block_y,
                        leaf.width / AV2_IBC_HASH_BLOCK_SIZE,
                        leaf.height / AV2_IBC_HASH_BLOCK_SIZE,
                        &mut stats,
                        &mut hash_index,
                    );
                }
            }
        }
    }

    Ok(Av2LocalIbc444 {
        blocks,
        blocks_wide,
        blocks_high,
        any_copy,
        stats,
    })
}

fn visit_local_ibc_block(
    blocks: &mut [Av2LocalIbcBlock444],
    coded_blocks: &mut [bool],
    frame: &[u8],
    geometry: Av2VideoGeometry,
    chroma_format: Av2ChromaFormat,
    bit_depth: SampleBitDepth,
    blocks_wide: usize,
    blocks_high: usize,
    block_x: usize,
    block_y: usize,
    any_copy: &mut bool,
    stats: &mut Av2LocalIbcStats,
    hash_index: &mut Av2TileIbcHashIndex,
) {
    let block_index = block_y * blocks_wide + block_x;
    let hash = blocks[block_index].hash;
    stats.total_blocks += 1;
    let x0 = block_x * AV2_IBC_HASH_BLOCK_SIZE;
    let y0 = block_y * AV2_IBC_HASH_BLOCK_SIZE;
    // AV2 v1.0.0 IntraBC syntax codes a block vector. The local MVP
    // search stores only 32-bit signatures. Its candidate list mirrors
    // the AVM mvref_common.c order used by setup_ref_mv_list() for
    // local 8x8 IntraBC: already-coded spatial IBC BVs are inserted
    // first, then AVM's default reference-BV entries fill the remaining
    // slots. This permits DRL 0/1 when a neighbor already carries the
    // desired {0,-8} or {-8,0} vector, while still using DRL 2/3 when
    // the defaults are unshifted.
    let left_in_same_tile = local_ibc_vector_is_valid_8x8(
        block_x,
        block_y,
        blocks_wide,
        blocks_high,
        AV2_IBC_BV_LEFT_8X8,
    );
    let above_in_same_tile = local_ibc_vector_is_valid_8x8(
        block_x,
        block_y,
        blocks_wide,
        blocks_high,
        AV2_IBC_BV_ABOVE_8X8,
    );
    if left_in_same_tile {
        stats.blocks_with_left_in_tile += 1;
    }
    if above_in_same_tile {
        stats.blocks_with_above_in_tile += 1;
    }
    let above_index = block_y
        .checked_sub(1)
        .map(|above_y| above_y * blocks_wide + block_x);
    let left_index = block_x
        .checked_sub(1)
        .map(|left_x| block_y * blocks_wide + left_x);
    let bvp_stack = build_bvp_stack_8x8(
        blocks,
        coded_blocks,
        blocks_wide,
        blocks_high,
        block_x,
        block_y,
    );
    let above_drl_idx = bvp_stack
        .iter()
        .position(|candidate| *candidate == AV2_IBC_BV_ABOVE_8X8)
        .map(|index| index as u8);
    let left_drl_idx = bvp_stack
        .iter()
        .position(|candidate| *candidate == AV2_IBC_BV_LEFT_8X8)
        .map(|index| index as u8);
    // AVM setup_ref_mv_list() scans spatial IntraBC BVs before appending the
    // same-superblock 8x8 defaults. build_bvp_stack_8x8() mirrors the fixed
    // 8x8-leaf subset of that scan order, so direct mode may select either a
    // spatially inherited vector or the later default Above8x8/Left8x8 entry.
    let default_above_bvp_supported = true;
    let default_left_bvp_supported = true;
    let fixed_drl_candidate_supported = above_drl_idx.is_some() || left_drl_idx.is_some();
    if fixed_drl_candidate_supported {
        stats.fixed_drl_supported_blocks += 1;
    }

    let raw_above_match = above_in_same_tile
        && above_index.is_some_and(|index| {
            coded_blocks[index]
                && blocks[index].hash == hash
                && planar_yuv_8x8_regions_equal(
                    frame,
                    geometry,
                    chroma_format,
                    bit_depth,
                    x0,
                    y0,
                    x0,
                    y0 - AV2_IBC_HASH_BLOCK_SIZE,
                )
        });
    let raw_left_match = left_in_same_tile
        && left_index.is_some_and(|index| {
            coded_blocks[index]
                && blocks[index].hash == hash
                && planar_yuv_8x8_regions_equal(
                    frame,
                    geometry,
                    chroma_format,
                    bit_depth,
                    x0,
                    y0,
                    x0 - AV2_IBC_HASH_BLOCK_SIZE,
                    y0,
                )
        });
    let direct_above_match = raw_above_match && above_drl_idx.is_some();
    let direct_left_match = raw_left_match && left_drl_idx.is_some();
    if raw_above_match {
        stats.raw_above_hash_matches += 1;
    }
    if raw_left_match {
        stats.raw_left_hash_matches += 1;
    }
    if direct_above_match {
        stats.direct_above_hash_matches += 1;
    } else if raw_above_match {
        stats.above_hash_matches_blocked_by_copied_candidate += 1;
    }
    if direct_left_match {
        stats.direct_left_hash_matches += 1;
    } else if raw_left_match {
        stats.left_hash_matches_blocked_by_copied_candidate += 1;
    }
    if direct_above_match && !fixed_drl_candidate_supported {
        stats.above_hash_matches_blocked_by_fixed_drl_guard += 1;
    }
    if direct_left_match && !fixed_drl_candidate_supported {
        stats.left_hash_matches_blocked_by_fixed_drl_guard += 1;
    }

    // AV2 v1.0.0 av2_is_dv_in_local_range()/setup_ref_mv_list(): direct copies
    // are cheapest when the local BVP stack already contains the adjacent
    // vector. Explicit-DV copies can reference any already-coded 8x8 block in
    // the same local tile as long as the source is outside the uncoded
    // bottom-right overlap region.
    let above_match = default_above_bvp_supported && above_in_same_tile && direct_above_match;
    let left_match = default_left_bvp_supported && left_in_same_tile && direct_left_match;
    let direct_candidate = match (above_match, left_match) {
        (true, true) => {
            let above_idx = above_drl_idx.expect("above match has a DRL index");
            let left_idx = left_drl_idx.expect("left match has a DRL index");
            if above_idx <= left_idx {
                Some((above_idx, AV2_IBC_BV_ABOVE_8X8))
            } else {
                Some((left_idx, AV2_IBC_BV_LEFT_8X8))
            }
        }
        (true, false) => Some((
            above_drl_idx.expect("above match has a DRL index"),
            AV2_IBC_BV_ABOVE_8X8,
        )),
        (false, true) => Some((
            left_drl_idx.expect("left match has a DRL index"),
            AV2_IBC_BV_LEFT_8X8,
        )),
        (false, false) => None,
    };
    let candidate = direct_candidate
        .map(|(drl_idx, vector)| (Av2LocalIbcCopy::DirectDrl { drl_idx }, vector))
        .or_else(|| {
            find_local_explicit_candidate(
                blocks,
                hash_index.candidates(hash),
                frame,
                geometry,
                chroma_format,
                bit_depth,
                blocks_wide,
                blocks_high,
                block_x,
                block_y,
                &bvp_stack,
            )
        });
    let (candidate_copy, copy_vector) = if let Some((copy, vector)) = candidate {
        if vector == AV2_IBC_BV_ABOVE_8X8 {
            stats.selected_above_copy_blocks += 1;
        } else if vector == AV2_IBC_BV_LEFT_8X8 {
            stats.selected_left_copy_blocks += 1;
        }
        (Some(copy), Some(vector))
    } else {
        (None, None)
    };
    *any_copy |= candidate_copy.is_some();
    blocks[block_index].candidate_copy = candidate_copy;
    blocks[block_index].copy_vector = copy_vector;
    coded_blocks[block_index] = true;
    hash_index.insert(hash, block_index);
}

fn find_local_explicit_candidate(
    blocks: &[Av2LocalIbcBlock444],
    candidate_indices: &[usize],
    frame: &[u8],
    geometry: Av2VideoGeometry,
    chroma_format: Av2ChromaFormat,
    bit_depth: SampleBitDepth,
    blocks_wide: usize,
    blocks_high: usize,
    block_x: usize,
    block_y: usize,
    bvp_stack: &[Av2LocalIbcVector],
) -> Option<(Av2LocalIbcCopy, Av2LocalIbcVector)> {
    let mut best: Option<(Av2LocalIbcCopy, Av2LocalIbcVector, u32)> = None;

    for &ref_index in candidate_indices {
        let ref_block_y = ref_index / blocks_wide;
        let ref_block_x = ref_index % blocks_wide;
        let x0 = block_x * AV2_IBC_HASH_BLOCK_SIZE;
        let y0 = block_y * AV2_IBC_HASH_BLOCK_SIZE;
        let ref_x0 = ref_block_x * AV2_IBC_HASH_BLOCK_SIZE;
        let ref_y0 = ref_block_y * AV2_IBC_HASH_BLOCK_SIZE;
        debug_assert_eq!(
            blocks[ref_index].hash,
            blocks[block_y * blocks_wide + block_x].hash
        );
        if !planar_yuv_8x8_regions_equal(
            frame,
            geometry,
            chroma_format,
            bit_depth,
            x0,
            y0,
            ref_x0,
            ref_y0,
        ) {
            continue;
        }
        let vector = Av2LocalIbcVector {
            row_px: ((ref_block_y as isize - block_y as isize) * AV2_IBC_HASH_BLOCK_SIZE as isize)
                as i16,
            col_px: ((ref_block_x as isize - block_x as isize) * AV2_IBC_HASH_BLOCK_SIZE as isize)
                as i16,
        };
        if !local_ibc_vector_is_valid_8x8(block_x, block_y, blocks_wide, blocks_high, vector) {
            continue;
        }
        update_best_explicit_candidate(&mut best, bvp_stack, vector);
    }

    best.map(|(copy, vector, _)| (copy, vector))
}

fn local_ibc_vector_is_valid_8x8(
    block_x: usize,
    block_y: usize,
    blocks_wide: usize,
    blocks_high: usize,
    vector: Av2LocalIbcVector,
) -> bool {
    const LOCAL_LEFT_SB_COUNT: isize = 4;
    let width = AV2_IBC_HASH_BLOCK_SIZE as isize;
    let height = AV2_IBC_HASH_BLOCK_SIZE as isize;
    let frame_width = (blocks_wide * AV2_IBC_HASH_BLOCK_SIZE) as isize;
    let frame_height = (blocks_high * AV2_IBC_HASH_BLOCK_SIZE) as isize;
    let x0 = (block_x * AV2_IBC_HASH_BLOCK_SIZE) as isize;
    let y0 = (block_y * AV2_IBC_HASH_BLOCK_SIZE) as isize;
    let src_left_x = x0 + isize::from(vector.col_px);
    let src_top_y = y0 + isize::from(vector.row_px);
    let src_right_x = src_left_x + width - 1;
    let src_bottom_y = src_top_y + height - 1;

    if src_left_x < 0 || src_top_y < 0 || src_right_x >= frame_width || src_bottom_y >= frame_height
    {
        return false;
    }

    // AV2 v1.0.0 av2_is_dv_in_local_range(): the source must not overlap the
    // uncoded bottom-right region of the active block.
    if isize::from(vector.col_px) + width > 0 && isize::from(vector.row_px) + height > 0 {
        return false;
    }

    let act_sb_col = x0 / AV2_IBC_TILE_SIZE as isize;
    let act_sb_row = y0 / AV2_IBC_TILE_SIZE as isize;
    let src_left_sb_col = src_left_x / AV2_IBC_TILE_SIZE as isize;
    let src_right_sb_col = src_right_x / AV2_IBC_TILE_SIZE as isize;
    let src_top_sb_row = src_top_y / AV2_IBC_TILE_SIZE as isize;
    let src_bottom_sb_row = src_bottom_y / AV2_IBC_TILE_SIZE as isize;

    // Local IntraBC references stay in the active superblock row and may look
    // back only through the spec's 4x64x64 local reference buffer.
    src_top_sb_row == act_sb_row
        && src_bottom_sb_row == act_sb_row
        && src_right_sb_col <= act_sb_col
        && src_left_sb_col >= act_sb_col - LOCAL_LEFT_SB_COUNT
}

fn update_best_explicit_candidate(
    best: &mut Option<(Av2LocalIbcCopy, Av2LocalIbcVector, u32)>,
    bvp_stack: &[Av2LocalIbcVector],
    vector: Av2LocalIbcVector,
) {
    let Some((drl_idx, ref_vector, distance)) = closest_explicit_ref_bv(bvp_stack, vector) else {
        return;
    };
    let copy = Av2LocalIbcCopy::ExplicitDv(Av2IntrabcExplicitDv {
        drl_idx,
        mv_row: vector.row_px,
        mv_col: vector.col_px,
        ref_row: ref_vector.row_px,
        ref_col: ref_vector.col_px,
    });
    let score = distance * 8 + u32::from(drl_idx);
    if best
        .as_ref()
        .is_none_or(|(_, _, best_score)| score < *best_score)
    {
        *best = Some((copy, vector, score));
    }
}

fn closest_explicit_ref_bv(
    bvp_stack: &[Av2LocalIbcVector],
    vector: Av2LocalIbcVector,
) -> Option<(u8, Av2LocalIbcVector, u32)> {
    bvp_stack
        .iter()
        .copied()
        .enumerate()
        .min_by_key(|(_, candidate)| {
            (i32::from(vector.row_px) - i32::from(candidate.row_px)).unsigned_abs()
                + (i32::from(vector.col_px) - i32::from(candidate.col_px)).unsigned_abs()
        })
        .map(|(index, candidate)| {
            let distance = (i32::from(vector.row_px) - i32::from(candidate.row_px)).unsigned_abs()
                + (i32::from(vector.col_px) - i32::from(candidate.col_px)).unsigned_abs();
            (index as u8, candidate, distance)
        })
}

fn mark_local_ibc_intra_leaf(
    blocks: &mut [Av2LocalIbcBlock444],
    coded_blocks: &mut [bool],
    blocks_wide: usize,
    block_x0: usize,
    block_y0: usize,
    leaf_blocks_wide: usize,
    leaf_blocks_high: usize,
    stats: &mut Av2LocalIbcStats,
    hash_index: &mut Av2TileIbcHashIndex,
) {
    for local_y in 0..leaf_blocks_high {
        for local_x in 0..leaf_blocks_wide {
            let block_x = block_x0 + local_x;
            let block_y = block_y0 + local_y;
            let block_index = block_y * blocks_wide + block_x;
            let x0 = block_x * AV2_IBC_HASH_BLOCK_SIZE;
            let y0 = block_y * AV2_IBC_HASH_BLOCK_SIZE;
            stats.total_blocks += 1;
            if x0 % AV2_IBC_TILE_SIZE != 0 {
                stats.blocks_with_left_in_tile += 1;
            }
            if y0 % AV2_IBC_TILE_SIZE != 0 {
                stats.blocks_with_above_in_tile += 1;
            }
            blocks[block_index].candidate_copy = None;
            blocks[block_index].copy_vector = None;
            coded_blocks[block_index] = true;
            hash_index.insert(blocks[block_index].hash, block_index);
        }
    }
}

fn hash_planar_yuv_8x8(
    frame: &[u8],
    geometry: Av2VideoGeometry,
    chroma_format: Av2ChromaFormat,
    bit_depth: SampleBitDepth,
    x0: usize,
    y0: usize,
) -> u32 {
    let y_len = geometry.width * geometry.height;
    let sub_x = match chroma_format {
        Av2ChromaFormat::Yuv420 | Av2ChromaFormat::Yuv422 => 2,
        Av2ChromaFormat::Yuv444 => 1,
    };
    let sub_y = match chroma_format {
        Av2ChromaFormat::Yuv420 => 2,
        Av2ChromaFormat::Yuv422 | Av2ChromaFormat::Yuv444 => 1,
    };
    let c_width = geometry.width / sub_x;
    let c_height = geometry.height / sub_y;
    let c_len = c_width * c_height;
    let bytes_per_sample = bit_depth.bytes_per_sample();
    let y_bytes = y_len * bytes_per_sample;
    let c_bytes = c_len * bytes_per_sample;
    let mut hash = AV2_IBC_HASH_OFFSET;

    hash = hash_plane_region(
        &frame[..y_bytes],
        hash,
        0,
        geometry.width,
        x0,
        y0,
        AV2_IBC_HASH_BLOCK_SIZE,
        AV2_IBC_HASH_BLOCK_SIZE,
        bytes_per_sample,
    );
    let chroma_width = AV2_IBC_HASH_BLOCK_SIZE / sub_x;
    let chroma_height = AV2_IBC_HASH_BLOCK_SIZE / sub_y;
    for (plane, plane_data) in [
        (1, &frame[y_bytes..y_bytes + c_bytes]),
        (2, &frame[y_bytes + c_bytes..y_bytes + 2 * c_bytes]),
    ] {
        hash = hash_plane_region(
            plane_data,
            hash,
            plane,
            c_width,
            x0 / sub_x,
            y0 / sub_y,
            chroma_width,
            chroma_height,
            bytes_per_sample,
        );
    }
    hash
}

fn hash_plane_region(
    plane_data: &[u8],
    mut hash: u32,
    plane: u32,
    stride: usize,
    x0: usize,
    y0: usize,
    width: usize,
    height: usize,
    bytes_per_sample: usize,
) -> u32 {
    for local_y in 0..height {
        let row = y0 + local_y;
        let row_start = (row * stride + x0) * bytes_per_sample;
        let row_bytes = width * bytes_per_sample;
        for (chunk_index, packet) in plane_data[row_start..row_start + row_bytes]
            .chunks(8)
            .enumerate()
        {
            // Keep the hash mixer multiplier-free and packet-shaped so the AV2
            // RTL can hash one ingress packet with a shallow XOR network
            // instead of serial xorshift rounds.
            hash = mix_ibc_hash_packet(
                hash,
                packet,
                plane,
                (local_y * row_bytes + chunk_index * 8) as u32,
            );
        }
    }
    hash
}

fn planar_yuv_8x8_regions_equal(
    frame: &[u8],
    geometry: Av2VideoGeometry,
    chroma_format: Av2ChromaFormat,
    bit_depth: SampleBitDepth,
    x0: usize,
    y0: usize,
    ref_x0: usize,
    ref_y0: usize,
) -> bool {
    let layout = Av2PlanarYuvLayout::new(geometry, chroma_format, bit_depth)
        .expect("AV2 IBC has already validated planar geometry");
    layout.region_equal_in_frame(
        frame,
        x0,
        y0,
        ref_x0,
        ref_y0,
        AV2_IBC_HASH_BLOCK_SIZE,
        AV2_IBC_HASH_BLOCK_SIZE,
    )
}

fn mix_ibc_hash_packet(hash: u32, packet: &[u8], component: u32, sample_offset: u32) -> u32 {
    let mut low = 0u32;
    let mut high = 0u32;
    for (index, sample) in packet.iter().take(4).enumerate() {
        low |= u32::from(*sample) << (index * 8);
    }
    for (index, sample) in packet.iter().skip(4).take(4).enumerate() {
        high |= u32::from(*sample) << (index * 8);
    }
    let meta = (component << 10) | (sample_offset << 4) | packet.len().min(8) as u32;
    // Mirror rtl/av2/ibc/ff_av2_local_hash_matcher_444.sv. The high half is
    // rotated before XORing so constant 8-sample packets do not collapse when
    // low == high; false IBC hits must not replace exact block comparison.
    let round = hash ^ low ^ high.rotate_left(5) ^ meta;
    round ^ round.rotate_left(13) ^ (round << 7) ^ (round >> 11)
}

fn build_bvp_stack_8x8(
    blocks: &[Av2LocalIbcBlock444],
    coded_blocks: &[bool],
    blocks_wide: usize,
    blocks_high: usize,
    block_x: usize,
    block_y: usize,
) -> Vec<Av2LocalIbcVector> {
    let mut stack = Vec::with_capacity(AV2_IBC_MAX_BVP_SIZE);
    // AV2 v1.0.0 setup_ref_mv_list() spatial search, specialized to
    // FrameForge's fixed 8x8 leaves (2x2 MI units) and local 64x64 tiles.
    // Several AVM 4x4-MI probes collapse to the same 8x8 neighbor in this
    // subset; keep the first effective 8x8 probe for each neighbor because
    // push_unique_spatial_bv() would discard the duplicates anyway.
    for (row_mi_offset, col_mi_offset) in [(1, -1), (-1, 1), (2, -1), (-1, 2), (-1, -1), (1, -3)] {
        if let Some(index) = neighbor_index_for_mi_offset(
            blocks_wide,
            blocks_high,
            block_x,
            block_y,
            row_mi_offset,
            col_mi_offset,
        ) {
            if coded_blocks[index] {
                push_unique_spatial_bv(&mut stack, blocks[index].copy_vector);
            }
        }
    }
    for vector in [
        AV2_IBC_BV_SUPERBLOCK_ABOVE,
        AV2_IBC_BV_SUPERBLOCK_DELAYED_LEFT,
        AV2_IBC_BV_ABOVE_8X8,
        AV2_IBC_BV_LEFT_8X8,
    ] {
        if stack.len() >= AV2_IBC_MAX_BVP_SIZE {
            break;
        }
        // AVM add_to_ref_bv_list() intentionally does not de-duplicate the
        // default BVP entries. A duplicate later default is harmless because
        // the encoder always selects the first matching vector.
        stack.push(vector);
    }
    stack
}

fn push_unique_spatial_bv(stack: &mut Vec<Av2LocalIbcVector>, vector: Option<Av2LocalIbcVector>) {
    if stack.len() >= AV2_IBC_MAX_BVP_SIZE {
        return;
    }
    let Some(vector) = vector else {
        return;
    };
    if !stack.contains(&vector) {
        stack.push(vector);
    }
}

fn neighbor_index_for_mi_offset(
    blocks_wide: usize,
    blocks_high: usize,
    block_x: usize,
    block_y: usize,
    row_mi_offset: isize,
    col_mi_offset: isize,
) -> Option<usize> {
    let tile_block_x = block_x % (AV2_IBC_TILE_SIZE / AV2_IBC_HASH_BLOCK_SIZE);
    let tile_block_y = block_y % (AV2_IBC_TILE_SIZE / AV2_IBC_HASH_BLOCK_SIZE);
    let tile_origin_x = block_x - tile_block_x;
    let tile_origin_y = block_y - tile_block_y;
    let candidate_mi_col = (tile_block_x * 2) as isize + col_mi_offset;
    let candidate_mi_row = (tile_block_y * 2) as isize + row_mi_offset;
    if !(0..16).contains(&candidate_mi_col) || !(0..16).contains(&candidate_mi_row) {
        return None;
    }
    let candidate_block_x = tile_origin_x + (candidate_mi_col as usize / 2);
    let candidate_block_y = tile_origin_y + (candidate_mi_row as usize / 2);
    if candidate_block_x >= blocks_wide || candidate_block_y >= blocks_high {
        return None;
    }
    Some(candidate_block_y * blocks_wide + candidate_block_x)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn av2_local_ibc_hash_marks_repeated_left_8x8_block() {
        let geometry = Av2VideoGeometry {
            width: 16,
            height: 16,
        };
        let plane_len = geometry.width * geometry.height;
        let mut frame = vec![0; plane_len * 3];
        for plane in 0..3 {
            for y in 0..16 {
                for x in 0..8 {
                    let value = (plane * 29 + y * 11 + x * 7) as u8;
                    frame[plane * plane_len + y * geometry.width + x] = value;
                    frame[plane * plane_len + y * geometry.width + x + 8] =
                        if y >= 8 { value } else { value.wrapping_add(3) };
                }
            }
        }

        let ibc = build_local_ibc_444(&frame, geometry).expect("IBC hash map should build");
        assert_eq!(ibc.candidate_drl_idx(0, 0), None);
        assert_eq!(ibc.candidate_drl_idx(8, 0), None);
        assert_eq!(ibc.candidate_drl_idx(8, 8), Some(3));
        assert!(ibc.any_copy());
    }

    #[test]
    fn av2_local_ibc_hash_marks_repeated_above_8x8_block() {
        let geometry = Av2VideoGeometry {
            width: 8,
            height: 16,
        };
        let plane_len = geometry.width * geometry.height;
        let mut frame = vec![0; plane_len * 3];
        for plane in 0..3 {
            for y in 0..8 {
                for x in 0..8 {
                    let value = (plane * 17 + y * 19 + x * 3) as u8;
                    frame[plane * plane_len + y * geometry.width + x] = value;
                    frame[plane * plane_len + (y + 8) * geometry.width + x] = value;
                }
            }
        }

        let ibc = build_local_ibc_444(&frame, geometry).expect("IBC hash map should build");
        assert_eq!(ibc.candidate_drl_idx(0, 0), None);
        assert_eq!(ibc.candidate_drl_idx(0, 8), Some(2));
        assert_eq!(ibc.stats.raw_above_hash_matches, 1);
        assert!(ibc.any_copy());
    }

    #[test]
    fn av2_local_ibc_hash_reuses_adjacent_vertical_spatial_bvp() {
        let geometry = Av2VideoGeometry {
            width: 8,
            height: 24,
        };
        let plane_len = geometry.width * geometry.height;
        let mut frame = vec![0; plane_len * 3];
        for plane in 0..3 {
            for y in 0..8 {
                for x in 0..8 {
                    let value = (plane * 17 + y * 19 + x * 3) as u8;
                    for block in 0..3 {
                        frame[plane * plane_len + (y + block * 8) * geometry.width + x] = value;
                    }
                }
            }
        }

        let ibc = build_local_ibc_444(&frame, geometry).expect("IBC hash map should build");
        assert_eq!(ibc.candidate_drl_idx(0, 0), None);
        assert_eq!(ibc.candidate_drl_idx(0, 8), Some(2));
        assert_eq!(ibc.candidate_drl_idx(0, 16), Some(0));
        assert_eq!(ibc.stats.raw_above_hash_matches, 2);
    }

    #[test]
    fn av2_local_ibc_hash_reuses_adjacent_spatial_bvp() {
        let geometry = Av2VideoGeometry {
            width: 24,
            height: 16,
        };
        let plane_len = geometry.width * geometry.height;
        let mut frame = vec![0; plane_len * 3];
        for plane in 0..3 {
            for y in 0..16 {
                for x in 0..8 {
                    let value = (plane * 31 + y * 13 + x * 5) as u8;
                    for block in 0..3 {
                        frame[plane * plane_len + y * geometry.width + x + block * 8] = if y >= 8 {
                            value
                        } else {
                            value.wrapping_add(block as u8 + 1)
                        };
                    }
                }
            }
        }

        let ibc = build_local_ibc_444(&frame, geometry).expect("IBC hash map should build");
        assert_eq!(ibc.candidate_drl_idx(0, 0), None);
        assert_eq!(ibc.candidate_drl_idx(8, 0), None);
        assert_eq!(ibc.candidate_drl_idx(8, 8), Some(3));
        assert_eq!(ibc.candidate_drl_idx(16, 8), Some(0));
    }

    #[test]
    fn av2_local_ibc_hash_keeps_defaults_after_non_direct_spatial_bvp() {
        let geometry = Av2VideoGeometry {
            width: 32,
            height: 24,
        };
        let plane_len = geometry.width * geometry.height;
        let mut frame = vec![0; plane_len * 3];
        for plane in 0..3 {
            for y in 0..geometry.height {
                for x in 0..geometry.width {
                    frame[plane * plane_len + y * geometry.width + x] =
                        (plane * 41 + y * 17 + x * 9) as u8;
                }
            }
            for y in 8..16 {
                for x in 0..8 {
                    let value = (plane * 23 + y * 5 + x * 3) as u8;
                    frame[plane * plane_len + y * geometry.width + x] = value;
                    frame[plane * plane_len + y * geometry.width + x + 8] = value;
                }
            }
            for y in 16..24 {
                for x in 0..8 {
                    let value = (plane * 13 + y * 7 + x * 11) as u8;
                    frame[plane * plane_len + y * geometry.width + x + 8] = value;
                    frame[plane * plane_len + y * geometry.width + x + 16] = value;
                }
            }
        }

        let ibc = build_local_ibc_444(&frame, geometry).expect("IBC hash map should build");
        assert_eq!(ibc.candidate_drl_idx(8, 8), Some(3));
        assert_eq!(ibc.candidate_drl_idx(16, 16), Some(0));
        assert_eq!(ibc.stats.raw_left_hash_matches, 2);
    }

    #[test]
    fn av2_local_ibc_hash_allows_full_tile_bottom_row_without_adjacent_copy() {
        let geometry = Av2VideoGeometry {
            width: 16,
            height: 64,
        };
        let plane_len = geometry.width * geometry.height;
        let mut frame = vec![0; plane_len * 3];
        for plane in 0..3 {
            for y in 0..geometry.height {
                for x in 0..8 {
                    let value = (plane * 29 + y * 11 + x * 7) as u8;
                    frame[plane * plane_len + y * geometry.width + x] = value;
                    frame[plane * plane_len + y * geometry.width + x + 8] = if y >= 56 {
                        value
                    } else {
                        value.wrapping_add(13)
                    };
                }
            }
        }

        let ibc = build_local_ibc_444(&frame, geometry).expect("IBC hash map should build");
        assert_eq!(ibc.candidate_drl_idx(8, 56), Some(3));
        assert!(ibc.any_copy());
    }

    #[test]
    fn av2_local_ibc_hash_finds_non_adjacent_explicit_copy() {
        let geometry = Av2VideoGeometry {
            width: 32,
            height: 8,
        };
        let plane_len = geometry.width * geometry.height;
        let mut frame = vec![0; plane_len * 3];
        for plane in 0..3 {
            for y in 0..geometry.height {
                for x in 0..8 {
                    let repeated = (plane * 19 + y * 7 + x * 5) as u8;
                    let separator = repeated.wrapping_add(53);
                    let tail = repeated.wrapping_add(101);
                    frame[plane * plane_len + y * geometry.width + x] = repeated;
                    frame[plane * plane_len + y * geometry.width + x + 8] = separator;
                    frame[plane * plane_len + y * geometry.width + x + 16] = repeated;
                    frame[plane * plane_len + y * geometry.width + x + 24] = tail;
                }
            }
        }

        let ibc = build_local_ibc_444(&frame, geometry).expect("IBC hash map should build");
        assert_eq!(ibc.candidate_drl_idx(0, 0), None);
        assert_eq!(ibc.candidate_drl_idx(8, 0), None);
        assert!(matches!(
            ibc.candidate_copy(16, 0),
            Some(Av2LocalIbcCopy::ExplicitDv(_))
        ));
    }

    #[test]
    fn av2_local_ibc_hash_distinguishes_constant_blocks() {
        let geometry = Av2VideoGeometry {
            width: 16,
            height: 32,
        };
        let plane_len = geometry.width * geometry.height;
        let mut frame = vec![0; plane_len * 3];
        for plane in 0..3 {
            for y in 0..geometry.height {
                let value = if y < 16 {
                    7 + plane as u8 * 11
                } else if y < 24 {
                    23 + plane as u8 * 13
                } else {
                    41 + plane as u8 * 17
                };
                for x in 0..geometry.width {
                    frame[plane * plane_len + y * geometry.width + x] = value;
                }
            }
        }

        let ibc = build_local_ibc_444(&frame, geometry).expect("IBC hash map should build");
        assert_eq!(ibc.candidate_drl_idx(0, 24), None);
        assert!(ibc.candidate_drl_idx(8, 24).is_some());
    }
}
