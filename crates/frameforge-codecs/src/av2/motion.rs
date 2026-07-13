#![allow(dead_code)]

use super::planar::Av2PlanarYuvLayout;
use super::{Av2ChromaFormat, Av2VideoGeometry};
use crate::picture::SampleBitDepth;

pub(crate) const AV2_LOSSLESS_ME_BLOCK_SIZE: usize = 8;
const AV2_LOSSLESS_ME_SEARCH_STEPS: [i16; 4] = [8, 16, 32, 64];

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Av2LosslessInterBlock {
    pub(crate) mv: Av2MotionVector,
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
}

impl Av2LosslessMotionStats {
    pub(crate) fn selected_inter_blocks(self) -> usize {
        self.selected_zero_mv_blocks
            + self.selected_neighbor_mv_blocks
            + self.selected_local_search_blocks
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
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Av2MotionCandidate {
    mv: Av2MotionVector,
    source: Av2MotionCandidateSource,
}

pub(crate) fn build_lossless_motion_map(
    current: &[u8],
    reference: &[u8],
    geometry: Av2VideoGeometry,
    chroma_format: Av2ChromaFormat,
    bit_depth: SampleBitDepth,
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
    let mut reference_hashes = vec![0u64; blocks_wide * blocks_high];
    for block_y in 0..blocks_high {
        for block_x in 0..blocks_wide {
            let x0 = block_x * AV2_LOSSLESS_ME_BLOCK_SIZE;
            let y0 = block_y * AV2_LOSSLESS_ME_BLOCK_SIZE;
            reference_hashes[block_y * blocks_wide + block_x] = layout.hash_region(
                reference,
                x0,
                y0,
                AV2_LOSSLESS_ME_BLOCK_SIZE,
                AV2_LOSSLESS_ME_BLOCK_SIZE,
            );
        }
    }

    let mut blocks = vec![None; blocks_wide * blocks_high];
    let mut stats = Av2LosslessMotionStats::default();
    for block_y in 0..blocks_high {
        for block_x in 0..blocks_wide {
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
            let candidates = motion_candidates(&blocks, blocks_wide, blocks_high, block_x, block_y);
            let mut selected = None;
            for candidate in candidates {
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
                let ref_index = ref_block_y * blocks_wide + ref_block_x;
                if reference_hashes[ref_index] != current_hash {
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
                    }
                    selected = Some(Av2LosslessInterBlock { mv: candidate.mv });
                    break;
                }
            }
            blocks[block_y * blocks_wide + block_x] = selected;
        }
    }

    Ok(Av2LosslessMotionMap {
        blocks,
        blocks_wide,
        blocks_high,
        stats,
    })
}

fn motion_candidates(
    blocks: &[Option<Av2LosslessInterBlock>],
    blocks_wide: usize,
    blocks_high: usize,
    block_x: usize,
    block_y: usize,
) -> Vec<Av2MotionCandidate> {
    let mut candidates = Vec::with_capacity(48);
    push_unique_candidate(
        &mut candidates,
        Av2MotionVector::zero(),
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
                push_unique_candidate(
                    &mut candidates,
                    block.mv,
                    Av2MotionCandidateSource::Neighbor,
                );
            }
        }
    }

    let predictors: Vec<_> = candidates.iter().map(|candidate| candidate.mv).collect();
    for predictor in predictors {
        for step in AV2_LOSSLESS_ME_SEARCH_STEPS {
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
                    push_unique_candidate(
                        &mut candidates,
                        mv,
                        Av2MotionCandidateSource::LocalSearch,
                    );
                }
            }
        }
    }
    candidates
}

fn push_unique_candidate(
    candidates: &mut Vec<Av2MotionCandidate>,
    mv: Av2MotionVector,
    source: Av2MotionCandidateSource,
) {
    if candidates.iter().any(|candidate| candidate.mv == mv) {
        return;
    }
    candidates.push(Av2MotionCandidate { mv, source });
}

fn offset_motion_vector(
    predictor: Av2MotionVector,
    row_delta: i16,
    col_delta: i16,
) -> Option<Av2MotionVector> {
    Some(Av2MotionVector {
        row_px: predictor.row_px.checked_add(row_delta)?,
        col_px: predictor.col_px.checked_add(col_delta)?,
    })
}

fn reference_block_for_mv(
    blocks_wide: usize,
    blocks_high: usize,
    block_x: usize,
    block_y: usize,
    mv: Av2MotionVector,
) -> Option<(usize, usize, usize, usize)> {
    if mv.row_px % AV2_LOSSLESS_ME_BLOCK_SIZE as i16 != 0
        || mv.col_px % AV2_LOSSLESS_ME_BLOCK_SIZE as i16 != 0
    {
        return None;
    }
    let ref_block_x = (block_x as isize)
        .checked_add(isize::from(mv.col_px) / AV2_LOSSLESS_ME_BLOCK_SIZE as isize)?;
    let ref_block_y = (block_y as isize)
        .checked_add(isize::from(mv.row_px) / AV2_LOSSLESS_ME_BLOCK_SIZE as isize)?;
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
