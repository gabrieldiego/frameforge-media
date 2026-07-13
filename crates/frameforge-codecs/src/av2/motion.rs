#![allow(dead_code)]

use super::planar::Av2PlanarYuvLayout;
use super::{Av2ChromaFormat, Av2VideoGeometry};
use crate::picture::SampleBitDepth;

pub(crate) const AV2_LOSSLESS_ME_BLOCK_SIZE: usize = 8;
const AV2_LOSSLESS_ME_HASH_BUCKET_LIMIT: usize = 8;
const AV2_LOSSLESS_ME_SEARCH_BLOCK_STEPS: [i16; 4] = [1, 2, 4, 8];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Av2MotionVector {
    pub(crate) row_px: i16,
    pub(crate) col_px: i16,
}

impl Av2MotionVector {
    fn zero() -> Self {
        Self {
            row_px: 0,
            col_px: 0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct Av2BlockMotionVector {
    row_blocks: i16,
    col_blocks: i16,
}

impl Av2BlockMotionVector {
    fn zero() -> Self {
        Self {
            row_blocks: 0,
            col_blocks: 0,
        }
    }

    fn from_pixel_mv(mv: Av2MotionVector) -> Option<Self> {
        if mv.row_px % AV2_LOSSLESS_ME_BLOCK_SIZE as i16 != 0
            || mv.col_px % AV2_LOSSLESS_ME_BLOCK_SIZE as i16 != 0
        {
            return None;
        }
        Some(Self {
            row_blocks: mv.row_px / AV2_LOSSLESS_ME_BLOCK_SIZE as i16,
            col_blocks: mv.col_px / AV2_LOSSLESS_ME_BLOCK_SIZE as i16,
        })
    }

    fn to_pixel_mv(self) -> Av2MotionVector {
        Av2MotionVector {
            row_px: self.row_blocks * AV2_LOSSLESS_ME_BLOCK_SIZE as i16,
            col_px: self.col_blocks * AV2_LOSSLESS_ME_BLOCK_SIZE as i16,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Av2LosslessInterBlock {
    pub(crate) mv: Av2MotionVector,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Av2MotionSearchRegion {
    pub(crate) x0: usize,
    pub(crate) y0: usize,
    pub(crate) width: usize,
    pub(crate) height: usize,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct Av2LosslessMotionStats {
    pub(crate) total_blocks: usize,
    pub(crate) candidate_checks: usize,
    pub(crate) out_of_bounds_candidates: usize,
    pub(crate) hash_rejected_candidates: usize,
    pub(crate) exact_candidate_checks: usize,
    pub(crate) selected_zero_mv_blocks: usize,
    pub(crate) selected_neighbor_mv_blocks: usize,
    pub(crate) selected_local_search_blocks: usize,
    pub(crate) selected_hash_index_blocks: usize,
}

impl Av2LosslessMotionStats {
    pub(crate) fn selected_inter_blocks(self) -> usize {
        self.selected_zero_mv_blocks
            + self.selected_neighbor_mv_blocks
            + self.selected_local_search_blocks
            + self.selected_hash_index_blocks
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Av2LosslessMotionMap {
    blocks: Vec<Option<Av2LosslessInterBlock>>,
    blocks_wide: usize,
    blocks_high: usize,
    stats: Av2LosslessMotionStats,
}

impl Av2LosslessMotionMap {
    pub(crate) fn candidate_at(&self, x0: usize, y0: usize) -> Option<Av2LosslessInterBlock> {
        assert_eq!(x0 % AV2_LOSSLESS_ME_BLOCK_SIZE, 0);
        assert_eq!(y0 % AV2_LOSSLESS_ME_BLOCK_SIZE, 0);
        let block_x = x0 / AV2_LOSSLESS_ME_BLOCK_SIZE;
        let block_y = y0 / AV2_LOSSLESS_ME_BLOCK_SIZE;
        assert!(block_x < self.blocks_wide && block_y < self.blocks_high);
        self.blocks[block_y * self.blocks_wide + block_x]
    }

    pub(crate) fn stats(&self) -> Av2LosslessMotionStats {
        self.stats
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Av2MotionCandidateSource {
    Zero,
    Neighbor,
    LocalSearch,
    HashIndex,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Av2MotionCandidate {
    mv: Av2BlockMotionVector,
    source: Av2MotionCandidateSource,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Av2ReferenceHashEntry {
    hash: u64,
    block_index: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Av2ReferenceHashIndex {
    entries: Vec<Av2ReferenceHashEntry>,
}

impl Av2ReferenceHashIndex {
    fn bucket(&self, hash: u64) -> Option<&[Av2ReferenceHashEntry]> {
        let start = self.entries.partition_point(|entry| entry.hash < hash);
        let end = start + self.entries[start..].partition_point(|entry| entry.hash == hash);
        (start < end).then_some(&self.entries[start..end])
    }
}

pub(crate) fn build_lossless_motion_map(
    current: &[u8],
    reference: &[u8],
    geometry: Av2VideoGeometry,
    chroma_format: Av2ChromaFormat,
    bit_depth: SampleBitDepth,
) -> Result<Av2LosslessMotionMap, String> {
    build_lossless_motion_map_with_regions(
        current,
        reference,
        geometry,
        chroma_format,
        bit_depth,
        None,
    )
}

pub(crate) fn build_lossless_motion_map_for_regions(
    current: &[u8],
    reference: &[u8],
    geometry: Av2VideoGeometry,
    chroma_format: Av2ChromaFormat,
    bit_depth: SampleBitDepth,
    search_regions: &[Av2MotionSearchRegion],
) -> Result<Av2LosslessMotionMap, String> {
    build_lossless_motion_map_with_regions(
        current,
        reference,
        geometry,
        chroma_format,
        bit_depth,
        Some(search_regions),
    )
}

fn build_lossless_motion_map_with_regions(
    current: &[u8],
    reference: &[u8],
    geometry: Av2VideoGeometry,
    chroma_format: Av2ChromaFormat,
    bit_depth: SampleBitDepth,
    search_regions: Option<&[Av2MotionSearchRegion]>,
) -> Result<Av2LosslessMotionMap, String> {
    let layout = Av2PlanarYuvLayout::new(geometry, chroma_format, bit_depth)?;
    layout.validate_frame_len(current, "current frame")?;
    layout.validate_frame_len(reference, "reference frame")?;
    if geometry.width % AV2_LOSSLESS_ME_BLOCK_SIZE != 0
        || geometry.height % AV2_LOSSLESS_ME_BLOCK_SIZE != 0
    {
        return Err(format!(
            "AV2 lossless motion estimation expects dimensions in {}-pixel units, got {}x{}",
            AV2_LOSSLESS_ME_BLOCK_SIZE, geometry.width, geometry.height
        ));
    }

    let blocks_wide = geometry.width / AV2_LOSSLESS_ME_BLOCK_SIZE;
    let blocks_high = geometry.height / AV2_LOSSLESS_ME_BLOCK_SIZE;
    let search_mask = motion_search_mask(blocks_wide, blocks_high, geometry, search_regions)?;
    let search_any = search_mask.iter().any(|search| *search);
    let mut blocks = vec![None; blocks_wide * blocks_high];
    let mut stats = Av2LosslessMotionStats::default();
    if !search_any {
        return Ok(Av2LosslessMotionMap {
            blocks,
            blocks_wide,
            blocks_high,
            stats,
        });
    }

    let mut reference_hashes = vec![None; blocks_wide * blocks_high];
    let mut reference_hash_index = None;
    let mut candidates = Vec::with_capacity(48);
    for block_y in 0..blocks_high {
        for block_x in 0..blocks_wide {
            let block_index = block_y * blocks_wide + block_x;
            if !search_mask[block_index] {
                continue;
            }
            stats.total_blocks += 1;
            let x0 = block_x * AV2_LOSSLESS_ME_BLOCK_SIZE;
            let y0 = block_y * AV2_LOSSLESS_ME_BLOCK_SIZE;
            let current_hash = layout.hash_region(
                current,
                x0,
                y0,
                AV2_LOSSLESS_ME_BLOCK_SIZE,
                AV2_LOSSLESS_ME_BLOCK_SIZE,
            );
            motion_candidates(
                &mut candidates,
                &blocks,
                blocks_wide,
                blocks_high,
                block_x,
                block_y,
            );
            let mut selected = None;
            for &candidate in &candidates {
                stats.candidate_checks += 1;
                let Some((ref_block_x, ref_block_y, ref_x0, ref_y0)) = reference_block_for_mv(
                    blocks_wide,
                    blocks_high,
                    block_x,
                    block_y,
                    candidate.mv,
                ) else {
                    stats.out_of_bounds_candidates += 1;
                    continue;
                };
                let reference_hash = reference_hash_for_block(
                    &mut reference_hashes,
                    layout,
                    reference,
                    blocks_wide,
                    ref_block_x,
                    ref_block_y,
                );
                if reference_hash != current_hash {
                    stats.hash_rejected_candidates += 1;
                    continue;
                }
                stats.exact_candidate_checks += 1;
                if layout.regions_equal_between(
                    current,
                    x0,
                    y0,
                    reference,
                    ref_x0,
                    ref_y0,
                    AV2_LOSSLESS_ME_BLOCK_SIZE,
                    AV2_LOSSLESS_ME_BLOCK_SIZE,
                ) {
                    match candidate.source {
                        Av2MotionCandidateSource::Zero => stats.selected_zero_mv_blocks += 1,
                        Av2MotionCandidateSource::Neighbor => {
                            stats.selected_neighbor_mv_blocks += 1
                        }
                        Av2MotionCandidateSource::LocalSearch => {
                            stats.selected_local_search_blocks += 1
                        }
                        Av2MotionCandidateSource::HashIndex => {
                            stats.selected_hash_index_blocks += 1
                        }
                    }
                    selected = Some(Av2LosslessInterBlock {
                        mv: candidate.mv.to_pixel_mv(),
                    });
                    break;
                }
            }
            if selected.is_none() {
                let hash_index = reference_hash_index.get_or_insert_with(|| {
                    build_reference_hash_index(
                        &mut reference_hashes,
                        layout,
                        reference,
                        blocks_wide,
                        blocks_high,
                    )
                });
                if let Some(candidate) = hash_index_candidate(
                    hash_index,
                    &candidates,
                    layout,
                    current,
                    reference,
                    blocks_wide,
                    block_x,
                    block_y,
                    x0,
                    y0,
                    current_hash,
                ) {
                    stats.candidate_checks += 1;
                    stats.exact_candidate_checks += 1;
                    stats.selected_hash_index_blocks += 1;
                    selected = Some(Av2LosslessInterBlock {
                        mv: candidate.mv.to_pixel_mv(),
                    });
                }
            }
            blocks[block_index] = selected;
        }
    }

    Ok(Av2LosslessMotionMap {
        blocks,
        blocks_wide,
        blocks_high,
        stats,
    })
}

fn reference_hash_for_block(
    reference_hashes: &mut [Option<u64>],
    layout: Av2PlanarYuvLayout,
    reference: &[u8],
    blocks_wide: usize,
    block_x: usize,
    block_y: usize,
) -> u64 {
    let index = block_y * blocks_wide + block_x;
    if let Some(hash) = reference_hashes[index] {
        return hash;
    }
    let hash = layout.hash_region(
        reference,
        block_x * AV2_LOSSLESS_ME_BLOCK_SIZE,
        block_y * AV2_LOSSLESS_ME_BLOCK_SIZE,
        AV2_LOSSLESS_ME_BLOCK_SIZE,
        AV2_LOSSLESS_ME_BLOCK_SIZE,
    );
    reference_hashes[index] = Some(hash);
    hash
}

fn build_reference_hash_index(
    reference_hashes: &mut [Option<u64>],
    layout: Av2PlanarYuvLayout,
    reference: &[u8],
    blocks_wide: usize,
    blocks_high: usize,
) -> Av2ReferenceHashIndex {
    let mut entries = Vec::with_capacity(blocks_wide * blocks_high);
    for block_y in 0..blocks_high {
        for block_x in 0..blocks_wide {
            let block_index = block_y * blocks_wide + block_x;
            let hash = reference_hash_for_block(
                reference_hashes,
                layout,
                reference,
                blocks_wide,
                block_x,
                block_y,
            );
            entries.push(Av2ReferenceHashEntry { hash, block_index });
        }
    }
    entries.sort_unstable_by_key(|entry| (entry.hash, entry.block_index));
    Av2ReferenceHashIndex { entries }
}

#[allow(clippy::too_many_arguments)]
fn hash_index_candidate(
    hash_index: &Av2ReferenceHashIndex,
    already_tested: &[Av2MotionCandidate],
    layout: Av2PlanarYuvLayout,
    current: &[u8],
    reference: &[u8],
    blocks_wide: usize,
    block_x: usize,
    block_y: usize,
    x0: usize,
    y0: usize,
    current_hash: u64,
) -> Option<Av2MotionCandidate> {
    let bucket = hash_index.bucket(current_hash)?;
    if bucket.len() > AV2_LOSSLESS_ME_HASH_BUCKET_LIMIT {
        return None;
    }

    let mut best = None;
    for entry in bucket {
        let ref_block_x = entry.block_index % blocks_wide;
        let ref_block_y = entry.block_index / blocks_wide;
        let Some(mv) = block_motion_between(block_x, block_y, ref_block_x, ref_block_y) else {
            continue;
        };
        if already_tested.iter().any(|candidate| candidate.mv == mv) {
            continue;
        }

        let ref_x0 = ref_block_x * AV2_LOSSLESS_ME_BLOCK_SIZE;
        let ref_y0 = ref_block_y * AV2_LOSSLESS_ME_BLOCK_SIZE;
        if !layout.regions_equal_between(
            current,
            x0,
            y0,
            reference,
            ref_x0,
            ref_y0,
            AV2_LOSSLESS_ME_BLOCK_SIZE,
            AV2_LOSSLESS_ME_BLOCK_SIZE,
        ) {
            continue;
        }

        let cost = fallback_motion_cost(mv);
        if best.as_ref().is_none_or(|(best_cost, best_mv)| {
            cost < *best_cost || (cost == *best_cost && mv < *best_mv)
        }) {
            best = Some((cost, mv));
        }
    }
    best.map(|(_, mv)| Av2MotionCandidate {
        mv,
        source: Av2MotionCandidateSource::HashIndex,
    })
}

fn motion_search_mask(
    blocks_wide: usize,
    blocks_high: usize,
    geometry: Av2VideoGeometry,
    search_regions: Option<&[Av2MotionSearchRegion]>,
) -> Result<Vec<bool>, String> {
    let Some(search_regions) = search_regions else {
        return Ok(vec![true; blocks_wide * blocks_high]);
    };

    let mut mask = vec![false; blocks_wide * blocks_high];
    for region in search_regions {
        if region.width == 0 || region.height == 0 {
            continue;
        }
        if region.x0 % AV2_LOSSLESS_ME_BLOCK_SIZE != 0
            || region.y0 % AV2_LOSSLESS_ME_BLOCK_SIZE != 0
            || region.width % AV2_LOSSLESS_ME_BLOCK_SIZE != 0
            || region.height % AV2_LOSSLESS_ME_BLOCK_SIZE != 0
        {
            return Err(format!(
                "AV2 lossless motion search region must be aligned to {} pixels, got {}x{}+{}+{}",
                AV2_LOSSLESS_ME_BLOCK_SIZE, region.width, region.height, region.x0, region.y0
            ));
        }
        let x1 = region
            .x0
            .checked_add(region.width)
            .ok_or("AV2 lossless motion search region width overflow")?;
        let y1 = region
            .y0
            .checked_add(region.height)
            .ok_or("AV2 lossless motion search region height overflow")?;
        if x1 > geometry.width || y1 > geometry.height {
            return Err(format!(
                "AV2 lossless motion search region {}x{}+{}+{} exceeds {}x{} frame",
                region.width, region.height, region.x0, region.y0, geometry.width, geometry.height
            ));
        }
        for block_y in region.y0 / AV2_LOSSLESS_ME_BLOCK_SIZE..y1 / AV2_LOSSLESS_ME_BLOCK_SIZE {
            for block_x in region.x0 / AV2_LOSSLESS_ME_BLOCK_SIZE..x1 / AV2_LOSSLESS_ME_BLOCK_SIZE {
                mask[block_y * blocks_wide + block_x] = true;
            }
        }
    }
    Ok(mask)
}

fn motion_candidates(
    candidates: &mut Vec<Av2MotionCandidate>,
    blocks: &[Option<Av2LosslessInterBlock>],
    blocks_wide: usize,
    blocks_high: usize,
    block_x: usize,
    block_y: usize,
) {
    candidates.clear();
    push_unique_candidate(
        candidates,
        Av2BlockMotionVector::zero(),
        Av2MotionCandidateSource::Zero,
    );

    for (neighbor_x, neighbor_y) in [
        block_x.checked_sub(1).map(|x| (x, block_y)),
        block_y.checked_sub(1).map(|y| (block_x, y)),
        block_x
            .checked_sub(1)
            .and_then(|x| block_y.checked_sub(1).map(|y| (x, y))),
        (block_x + 1 < blocks_wide)
            .then_some(block_x + 1)
            .and_then(|x| block_y.checked_sub(1).map(|y| (x, y))),
    ]
    .into_iter()
    .flatten()
    {
        if neighbor_x < blocks_wide && neighbor_y < blocks_high {
            if let Some(block) = blocks[neighbor_y * blocks_wide + neighbor_x] {
                if let Some(mv) = Av2BlockMotionVector::from_pixel_mv(block.mv) {
                    push_unique_candidate(candidates, mv, Av2MotionCandidateSource::Neighbor);
                }
            }
        }
    }

    let predictor_count = candidates.len();
    for predictor_index in 0..predictor_count {
        let predictor = candidates[predictor_index].mv;
        for step in AV2_LOSSLESS_ME_SEARCH_BLOCK_STEPS {
            for (row_delta, col_delta) in [
                (-step, 0),
                (0, -step),
                (0, step),
                (step, 0),
                (-step, -step),
                (-step, step),
                (step, -step),
                (step, step),
            ] {
                if let Some(mv) = offset_motion_vector(predictor, row_delta, col_delta) {
                    push_unique_candidate(candidates, mv, Av2MotionCandidateSource::LocalSearch);
                }
            }
        }
    }
}

fn push_unique_candidate(
    candidates: &mut Vec<Av2MotionCandidate>,
    mv: Av2BlockMotionVector,
    source: Av2MotionCandidateSource,
) {
    if candidates.iter().any(|candidate| candidate.mv == mv) {
        return;
    }
    candidates.push(Av2MotionCandidate { mv, source });
}

fn block_motion_between(
    block_x: usize,
    block_y: usize,
    ref_block_x: usize,
    ref_block_y: usize,
) -> Option<Av2BlockMotionVector> {
    Some(Av2BlockMotionVector {
        row_blocks: i16::try_from(ref_block_y as isize - block_y as isize).ok()?,
        col_blocks: i16::try_from(ref_block_x as isize - block_x as isize).ok()?,
    })
}

fn fallback_motion_cost(mv: Av2BlockMotionVector) -> u32 {
    u32::from(mv.row_blocks.unsigned_abs()) + u32::from(mv.col_blocks.unsigned_abs())
}

fn offset_motion_vector(
    predictor: Av2BlockMotionVector,
    row_delta: i16,
    col_delta: i16,
) -> Option<Av2BlockMotionVector> {
    Some(Av2BlockMotionVector {
        row_blocks: predictor.row_blocks.checked_add(row_delta)?,
        col_blocks: predictor.col_blocks.checked_add(col_delta)?,
    })
}

fn reference_block_for_mv(
    blocks_wide: usize,
    blocks_high: usize,
    block_x: usize,
    block_y: usize,
    mv: Av2BlockMotionVector,
) -> Option<(usize, usize, usize, usize)> {
    let ref_block_x = (block_x as isize).checked_add(isize::from(mv.col_blocks))?;
    let ref_block_y = (block_y as isize).checked_add(isize::from(mv.row_blocks))?;
    if ref_block_x < 0
        || ref_block_y < 0
        || ref_block_x >= blocks_wide as isize
        || ref_block_y >= blocks_high as isize
    {
        return None;
    }
    let ref_block_x = ref_block_x as usize;
    let ref_block_y = ref_block_y as usize;
    Some((
        ref_block_x,
        ref_block_y,
        ref_block_x * AV2_LOSSLESS_ME_BLOCK_SIZE,
        ref_block_y * AV2_LOSSLESS_ME_BLOCK_SIZE,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::picture::{ChromaSampling, Picture, PixelFormat};

    #[test]
    fn repeated_420_10bit_frame_selects_zero_mv_for_every_block() {
        let geometry = Av2VideoGeometry {
            width: 16,
            height: 16,
        };
        let bit_depth = SampleBitDepth::new(10).expect("10-bit depth is supported");
        let frame = patterned_frame(geometry, Av2ChromaFormat::Yuv420, bit_depth);

        let map =
            build_lossless_motion_map(&frame, &frame, geometry, Av2ChromaFormat::Yuv420, bit_depth)
                .expect("repeated frame motion search should succeed");

        assert_eq!(map.stats().total_blocks, 4);
        assert_eq!(map.stats().selected_inter_blocks(), 4);
        assert_eq!(map.stats().selected_zero_mv_blocks, 4);
        assert_eq!(
            map.candidate_at(8, 8).expect("block should be reused").mv,
            Av2MotionVector::zero()
        );
    }

    #[test]
    fn shifted_block_finds_block_aligned_motion_across_subsamplings() {
        let geometry = Av2VideoGeometry {
            width: 16,
            height: 16,
        };
        for (chroma_format, bit_depth) in [
            (
                Av2ChromaFormat::Yuv420,
                SampleBitDepth::new(10).expect("10-bit depth is supported"),
            ),
            (
                Av2ChromaFormat::Yuv422,
                SampleBitDepth::new(10).expect("10-bit depth is supported"),
            ),
            (
                Av2ChromaFormat::Yuv444,
                SampleBitDepth::new(8).expect("8-bit depth is supported"),
            ),
        ] {
            let reference = block_pattern_frame(geometry, chroma_format, bit_depth);
            let mut current = vec![0xee; reference.len()];
            copy_planar_block(
                &reference,
                0,
                0,
                &mut current,
                8,
                0,
                geometry,
                chroma_format,
                bit_depth,
            );

            let map =
                build_lossless_motion_map(&current, &reference, geometry, chroma_format, bit_depth)
                    .expect("shifted block motion search should succeed");

            assert_eq!(
                map.candidate_at(8, 0)
                    .expect("shifted block should match")
                    .mv,
                Av2MotionVector {
                    row_px: 0,
                    col_px: -8,
                }
            );
            assert_eq!(map.stats().selected_inter_blocks(), 1);
            assert_eq!(map.stats().selected_local_search_blocks, 1);
        }
    }

    #[test]
    fn hash_index_finds_exact_block_when_local_steps_miss() {
        let geometry = Av2VideoGeometry {
            width: 32,
            height: 16,
        };
        let chroma_format = Av2ChromaFormat::Yuv444;
        let bit_depth = SampleBitDepth::new(8).expect("8-bit depth is supported");
        let reference = block_pattern_frame(geometry, chroma_format, bit_depth);
        let mut current = vec![0xee; reference.len()];
        copy_planar_block(
            &reference,
            0,
            0,
            &mut current,
            24,
            0,
            geometry,
            chroma_format,
            bit_depth,
        );

        let map =
            build_lossless_motion_map(&current, &reference, geometry, chroma_format, bit_depth)
                .expect("hash index fallback should find non-step exact block");

        assert_eq!(
            map.candidate_at(24, 0)
                .expect("shifted block should match through hash index")
                .mv,
            Av2MotionVector {
                row_px: 0,
                col_px: -24,
            }
        );
        assert_eq!(map.stats().selected_inter_blocks(), 1);
        assert_eq!(map.stats().selected_hash_index_blocks, 1);
    }

    #[test]
    fn rejects_mismatched_reference_length() {
        let geometry = Av2VideoGeometry {
            width: 16,
            height: 16,
        };
        let bit_depth = SampleBitDepth::new(8).expect("8-bit depth is supported");
        let frame = patterned_frame(geometry, Av2ChromaFormat::Yuv422, bit_depth);
        let err = build_lossless_motion_map(
            &frame,
            &frame[..frame.len() - 1],
            geometry,
            Av2ChromaFormat::Yuv422,
            bit_depth,
        )
        .expect_err("mismatched reference length should fail");

        assert!(err.contains("reference frame length mismatch"));
    }

    fn patterned_frame(
        geometry: Av2VideoGeometry,
        chroma_format: Av2ChromaFormat,
        bit_depth: SampleBitDepth,
    ) -> Vec<u8> {
        let format = PixelFormat::planar_yuv(chroma_format.chroma_sampling(), bit_depth);
        let mut frame = vec![0; Picture::expected_len(geometry.width, geometry.height, format)];
        let max_sample = (1u16 << bit_depth.bits()) - 1;
        let bytes_per_sample = bit_depth.bytes_per_sample();
        for sample_index in 0..(frame.len() / bytes_per_sample) {
            let value = ((sample_index * 37 + 11) as u16) & max_sample;
            write_sample(&mut frame, sample_index, bit_depth, value);
        }
        frame
    }

    fn block_pattern_frame(
        geometry: Av2VideoGeometry,
        chroma_format: Av2ChromaFormat,
        bit_depth: SampleBitDepth,
    ) -> Vec<u8> {
        let mut frame = patterned_frame(geometry, chroma_format, bit_depth);
        for y in (0..geometry.height).step_by(AV2_LOSSLESS_ME_BLOCK_SIZE) {
            for x in (0..geometry.width).step_by(AV2_LOSSLESS_ME_BLOCK_SIZE) {
                let value = ((y * 3 + x * 5 + 17) % 251) as u16;
                fill_planar_block(&mut frame, x, y, geometry, chroma_format, bit_depth, value);
            }
        }
        frame
    }

    fn fill_planar_block(
        frame: &mut [u8],
        x0: usize,
        y0: usize,
        geometry: Av2VideoGeometry,
        chroma_format: Av2ChromaFormat,
        bit_depth: SampleBitDepth,
        value: u16,
    ) {
        for local_y in 0..AV2_LOSSLESS_ME_BLOCK_SIZE {
            for local_x in 0..AV2_LOSSLESS_ME_BLOCK_SIZE {
                write_sample(
                    frame,
                    (y0 + local_y) * geometry.width + x0 + local_x,
                    bit_depth,
                    value,
                );
            }
        }
        let (sub_x, sub_y) = subsampling(chroma_format);
        let chroma_width = geometry.width / sub_x;
        let chroma_height = AV2_LOSSLESS_ME_BLOCK_SIZE / sub_y;
        let y_samples = geometry.width * geometry.height;
        let c_samples = chroma_width * (geometry.height / sub_y);
        for plane in 0..2 {
            let plane_offset = y_samples + plane * c_samples;
            for local_y in 0..chroma_height {
                for local_x in 0..(AV2_LOSSLESS_ME_BLOCK_SIZE / sub_x) {
                    write_sample(
                        frame,
                        plane_offset + (y0 / sub_y + local_y) * chroma_width + x0 / sub_x + local_x,
                        bit_depth,
                        value + 1 + plane as u16,
                    );
                }
            }
        }
    }

    fn copy_planar_block(
        source: &[u8],
        source_x0: usize,
        source_y0: usize,
        dest: &mut [u8],
        dest_x0: usize,
        dest_y0: usize,
        geometry: Av2VideoGeometry,
        chroma_format: Av2ChromaFormat,
        bit_depth: SampleBitDepth,
    ) {
        let bytes_per_sample = bit_depth.bytes_per_sample();
        for local_y in 0..AV2_LOSSLESS_ME_BLOCK_SIZE {
            copy_sample_row(
                source,
                (source_y0 + local_y) * geometry.width + source_x0,
                dest,
                (dest_y0 + local_y) * geometry.width + dest_x0,
                AV2_LOSSLESS_ME_BLOCK_SIZE,
                bytes_per_sample,
            );
        }
        let (sub_x, sub_y) = subsampling(chroma_format);
        let chroma_width = geometry.width / sub_x;
        let y_samples = geometry.width * geometry.height;
        let c_samples = chroma_width * (geometry.height / sub_y);
        for plane in 0..2 {
            let plane_offset = y_samples + plane * c_samples;
            for local_y in 0..(AV2_LOSSLESS_ME_BLOCK_SIZE / sub_y) {
                copy_sample_row(
                    source,
                    plane_offset + (source_y0 / sub_y + local_y) * chroma_width + source_x0 / sub_x,
                    dest,
                    plane_offset + (dest_y0 / sub_y + local_y) * chroma_width + dest_x0 / sub_x,
                    AV2_LOSSLESS_ME_BLOCK_SIZE / sub_x,
                    bytes_per_sample,
                );
            }
        }
    }

    fn copy_sample_row(
        source: &[u8],
        source_sample: usize,
        dest: &mut [u8],
        dest_sample: usize,
        sample_count: usize,
        bytes_per_sample: usize,
    ) {
        let source_start = source_sample * bytes_per_sample;
        let source_end = source_start + sample_count * bytes_per_sample;
        let dest_start = dest_sample * bytes_per_sample;
        let dest_end = dest_start + sample_count * bytes_per_sample;
        dest[dest_start..dest_end].copy_from_slice(&source[source_start..source_end]);
    }

    fn write_sample(frame: &mut [u8], sample_index: usize, bit_depth: SampleBitDepth, value: u16) {
        if bit_depth.bits() <= 8 {
            frame[sample_index] = value as u8;
        } else {
            let offset = sample_index * 2;
            frame[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
        }
    }

    fn subsampling(chroma_format: Av2ChromaFormat) -> (usize, usize) {
        match chroma_format.chroma_sampling() {
            ChromaSampling::Cs420 => (2, 2),
            ChromaSampling::Cs422 => (2, 1),
            ChromaSampling::Cs444 => (1, 1),
            ChromaSampling::Monochrome => unreachable!("AV2 motion tests use YUV formats"),
        }
    }
}
