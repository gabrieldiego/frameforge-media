// Keep 4:4:4 palette FSC gated until its chroma coefficient path is
// reference-clean on larger screen-content frames.
const AV2_ENABLE_LUMA_PALETTE_FSC_444: bool = false;

#[cfg(test)]
pub(crate) fn av2_black_444_tile_entropy_payload(
    geometry: Av2VideoGeometry,
    profile: Av2Black444MvpProfile,
) -> Av2EntropyPayload {
    av2_black_444_tile_entropy_payload_for_region_with_fields(
        Av2TileRegion::root(geometry),
        profile,
        true,
    )
}

pub(crate) fn av2_black_444_tile_entropy_payload_for_region_with_fields(
    region: Av2TileRegion,
    profile: Av2Black444MvpProfile,
    record_fields: bool,
) -> Av2EntropyPayload {
    av2_black_444_tile_entropy_payload_for_region_with_intrabc_and_fields(
        region,
        profile,
        false,
        record_fields,
    )
}

pub(crate) fn av2_black_444_tile_entropy_payload_for_region_with_intrabc_and_fields(
    region: Av2TileRegion,
    profile: Av2Black444MvpProfile,
    allow_intrabc: bool,
    record_fields: bool,
) -> Av2EntropyPayload {
    let plan = Av2Black444TilePlan::for_region(
        region,
        profile,
        Av2ChromaFormat::Yuv444,
        false,
        allow_intrabc,
        None,
        None,
    );
    let mut writer =
        Av2EntropyWriter::with_cdf_updates_and_fields(!profile.disable_cdf_update, record_fields);
    plan.write_entropy(&mut writer, None, None);
    writer.finish()
}

pub(crate) fn av2_black_tile_entropy_payload_for_region(
    region: Av2TileRegion,
    profile: Av2Black444MvpProfile,
    chroma_format: Av2ChromaFormat,
) -> Av2EntropyPayload {
    av2_black_tile_entropy_payload_for_region_with_fields(region, profile, chroma_format, true)
}

pub(crate) fn av2_black_tile_entropy_payload_for_region_with_fields(
    region: Av2TileRegion,
    profile: Av2Black444MvpProfile,
    chroma_format: Av2ChromaFormat,
    record_fields: bool,
) -> Av2EntropyPayload {
    let plan =
        Av2Black444TilePlan::for_region(region, profile, chroma_format, false, false, None, None);
    let mut writer =
        Av2EntropyWriter::with_cdf_updates_and_fields(!profile.disable_cdf_update, record_fields);
    plan.write_entropy(&mut writer, None, None);
    writer.finish()
}

pub(crate) fn av2_luma_palette_444_tile_entropy_payload_for_region_with_fields(
    region: Av2TileRegion,
    profile: Av2Black444MvpProfile,
    allow_intrabc: bool,
    palette: &Av2LumaPalette444,
    ibc: Option<&Av2LocalIbc444>,
    record_fields: bool,
) -> Av2EntropyPayload {
    let mut best: Option<Av2EntropyPayload> = None;
    // Larger merged palette leaves currently add mode-search cost without a
    // measured size win on 1080p screen-content baselines.
    for partition_policy in [Av2PartitionPolicy::Fixed8x8Leaves] {
        let payload = av2_luma_palette_444_tile_entropy_payload_for_region_with_policy(
            region,
            profile,
            allow_intrabc,
            palette,
            ibc,
            partition_policy,
            record_fields,
        );
        let replace = best.as_ref().is_none_or(|best_payload| {
            (payload.bytes.len(), payload.symbol_bits)
                < (best_payload.bytes.len(), best_payload.symbol_bits)
        });
        if replace {
            best = Some(payload);
        }
    }

    best.expect("AV2 4:4:4 palette has fixed partition candidates")
}

fn av2_luma_palette_444_tile_entropy_payload_for_region_with_policy(
    region: Av2TileRegion,
    profile: Av2Black444MvpProfile,
    allow_intrabc: bool,
    palette: &Av2LumaPalette444,
    ibc: Option<&Av2LocalIbc444>,
    partition_policy: Av2PartitionPolicy,
    record_fields: bool,
) -> Av2EntropyPayload {
    let plan = Av2Black444TilePlan::for_region_with_partition_policy(
        region,
        profile,
        Av2ChromaFormat::Yuv444,
        partition_policy,
        true,
        allow_intrabc,
        ibc,
        Some(palette),
    );
    let mut writer =
        Av2EntropyWriter::with_cdf_updates_and_fields(!profile.disable_cdf_update, record_fields);
    plan.write_entropy(&mut writer, Some(palette), ibc);
    writer.finish()
}

pub(crate) fn av2_lossy_420_tile_entropy_payload_for_region(
    region: Av2TileRegion,
    profile: Av2Black444MvpProfile,
    geometry: Av2VideoGeometry,
    bit_depth: SampleBitDepth,
    source: &[u8],
    recon: &mut [u8],
) -> Av2EntropyPayload {
    av2_lossy_420_tile_entropy_payload_for_region_with_fields(
        region, profile, geometry, bit_depth, source, recon, true,
    )
}

pub(crate) fn av2_lossy_420_tile_entropy_payload_for_region_with_fields(
    region: Av2TileRegion,
    profile: Av2Black444MvpProfile,
    geometry: Av2VideoGeometry,
    bit_depth: SampleBitDepth,
    source: &[u8],
    recon: &mut [u8],
    record_fields: bool,
) -> Av2EntropyPayload {
    let plan = Av2Black444TilePlan::for_region(
        region,
        profile,
        Av2ChromaFormat::Yuv420,
        false,
        false,
        None,
        None,
    );
    let mut writer =
        Av2EntropyWriter::with_cdf_updates_and_fields(!profile.disable_cdf_update, record_fields);
    let mut lossy = Av2Lossy420TileState::new(geometry, region, bit_depth, source, recon);
    plan.write_lossy_420_entropy(&mut writer, &mut lossy);
    writer.finish()
}

pub(crate) fn av2_lossless_subsampled_tile_entropy_payload_for_region_with_fields(
    region: Av2TileRegion,
    profile: Av2Black444MvpProfile,
    geometry: Av2VideoGeometry,
    chroma_format: Av2ChromaFormat,
    bit_depth: SampleBitDepth,
    source: &[u8],
    recon: &mut [u8],
    palette: Option<&Av2LumaPalette444>,
    ibc: Option<&Av2LocalIbc444>,
    record_fields: bool,
) -> Av2EntropyPayload {
    debug_assert!(matches!(
        chroma_format,
        Av2ChromaFormat::Yuv420 | Av2ChromaFormat::Yuv422 | Av2ChromaFormat::Yuv444
    ));
    if use_fast_lossless_subsampled_path(region) {
        return av2_lossless_subsampled_tile_entropy_payload_for_region_with_policy(
            region,
            profile,
            geometry,
            chroma_format,
            bit_depth,
            source,
            recon,
            palette,
            if ibc.is_some() {
                Av2PartitionPolicy::Fixed8x8Leaves
            } else {
                Av2PartitionPolicy::LosslessAdaptive32
            },
            Av2LosslessSubsampledModeSearch::FastScreenContent,
            ibc,
            record_fields,
            true,
        );
    }

    let mut best: Option<(Av2EntropyPayload, Vec<u8>)> = None;
    // Larger subsampled lossless leaves can emit AVM-rejected edge-block
    // syntax while still matching the internal reconstruction.
    for partition_policy in [Av2PartitionPolicy::Fixed8x8Leaves] {
        let mut candidate_recon = recon.to_vec();
        let payload = av2_lossless_subsampled_tile_entropy_payload_for_region_with_policy(
            region,
            profile,
            geometry,
            chroma_format,
            bit_depth,
            source,
            &mut candidate_recon,
            palette,
            partition_policy,
            Av2LosslessSubsampledModeSearch::Exhaustive,
            ibc,
            record_fields,
            true,
        );
        let replace = best.as_ref().is_none_or(|(best_payload, _)| {
            (payload.bytes.len(), payload.symbol_bits)
                < (best_payload.bytes.len(), best_payload.symbol_bits)
        });
        if replace {
            best = Some((payload, candidate_recon));
        }
    }

    let (payload, candidate_recon) =
        best.expect("AV2 planar lossless has fixed partition candidates");
    recon.copy_from_slice(&candidate_recon);
    payload
}

pub(crate) fn av2_lossless_subsampled_fast_tile_entropy_payload_for_region_with_fields(
    region: Av2TileRegion,
    profile: Av2Black444MvpProfile,
    geometry: Av2VideoGeometry,
    chroma_format: Av2ChromaFormat,
    bit_depth: SampleBitDepth,
    source: &[u8],
    palette: Option<&Av2LumaPalette444>,
    record_fields: bool,
) -> Av2EntropyPayload {
    debug_assert!(matches!(
        chroma_format,
        Av2ChromaFormat::Yuv420 | Av2ChromaFormat::Yuv422 | Av2ChromaFormat::Yuv444
    ));
    debug_assert!(use_fast_lossless_subsampled_path(region));
    let mut scratch_recon = [];
    av2_lossless_subsampled_tile_entropy_payload_for_region_with_policy(
        region,
        profile,
        geometry,
        chroma_format,
        bit_depth,
        source,
        &mut scratch_recon,
        palette,
        Av2PartitionPolicy::LosslessAdaptive32,
        Av2LosslessSubsampledModeSearch::FastScreenContent,
        None,
        record_fields,
        false,
    )
}

const AV2_FAST_LOSSLESS_SUBSAMPLED_MIN_PIXELS: usize = 128 * 128;
const AV2_LOSSLESS_ADAPTIVE_LEAF_SIZE: usize = 64;
const AV2_LOSSLESS_BASE_LEAF_SIZE: usize = 16;
const AV2_LOSSLESS_ADAPTIVE_SAMPLE_STEP: usize = 4;
const AV2_LOSSLESS_ADAPTIVE_UNIQUE_LIMIT: usize = 8;
const AV2_LOSSLESS_ADAPTIVE_GRADIENT_UNIQUE_LIMIT: usize = 16;
const AV2_LOSSLESS_ADAPTIVE_GRADIENT_Q8_LIMIT: u64 = 192;
const AV2_LOSSLESS_ADAPTIVE_RANGE_LIMIT: u16 = 8;
const AV2_LOSSLESS_PALETTE_PARTITION_UNIQUE_LIMIT: usize = 4;

fn use_fast_lossless_subsampled_path(region: Av2TileRegion) -> bool {
    region.width * region.height >= AV2_FAST_LOSSLESS_SUBSAMPLED_MIN_PIXELS
}

fn lossless_partition_features_for_source(
    region: Av2TileRegion,
    geometry: Av2VideoGeometry,
    bit_depth: SampleBitDepth,
    source: &[u8],
    palette_enabled: bool,
) -> Av2LosslessPartitionFeatures {
    let cols = region.width.div_ceil(AV2_LOSSLESS_ADAPTIVE_LEAF_SIZE);
    let rows = region.height.div_ceil(AV2_LOSSLESS_ADAPTIVE_LEAF_SIZE);
    let mut simple_leaves = Vec::with_capacity(cols * rows);
    for row in 0..rows {
        let y0 = region.origin_y + row * AV2_LOSSLESS_ADAPTIVE_LEAF_SIZE;
        let height =
            (region.origin_y + region.height - y0).min(AV2_LOSSLESS_ADAPTIVE_LEAF_SIZE);
        for col in 0..cols {
            let x0 = region.origin_x + col * AV2_LOSSLESS_ADAPTIVE_LEAF_SIZE;
            let width =
                (region.origin_x + region.width - x0).min(AV2_LOSSLESS_ADAPTIVE_LEAF_SIZE);
            simple_leaves.push(lossless_luma_region_is_simple_for_adaptive_leaf(
                geometry, bit_depth, source, x0, y0, width, height,
            ));
        }
    }
    let forced_micro_cols = region.width / MVP_LEAF_BLOCK_SIZE;
    let forced_micro_rows = region.height / MVP_LEAF_BLOCK_SIZE;
    let mut forced_micro_blocks = Vec::new();
    if palette_enabled {
        forced_micro_blocks.reserve(forced_micro_cols * forced_micro_rows);
        for row in 0..forced_micro_rows {
            let y0 = region.origin_y + row * MVP_LEAF_BLOCK_SIZE;
            for col in 0..forced_micro_cols {
                let x0 = region.origin_x + col * MVP_LEAF_BLOCK_SIZE;
                forced_micro_blocks.push(lossless_luma_8x8_is_palette_worthy(
                    geometry, bit_depth, source, x0, y0,
                ));
            }
        }
    }
    Av2LosslessPartitionFeatures {
        simple_leaves,
        cols,
        leaf_size: AV2_LOSSLESS_ADAPTIVE_LEAF_SIZE,
        forced_micro_blocks,
        forced_micro_cols,
    }
}

fn lossless_luma_8x8_is_palette_worthy(
    geometry: Av2VideoGeometry,
    bit_depth: SampleBitDepth,
    source: &[u8],
    x0: usize,
    y0: usize,
) -> bool {
    let mut values = [0u16; AV2_LUMA_PALETTE_MAX_COLORS + 1];
    let mut unique = 0usize;
    for y in y0..(y0 + MVP_LEAF_BLOCK_SIZE) {
        for x in x0..(x0 + MVP_LEAF_BLOCK_SIZE) {
            let sample = read_validated_planar_sample(source, y * geometry.width + x, bit_depth);
            if values[..unique].contains(&sample) {
                continue;
            }
            if unique == values.len() {
                return false;
            }
            values[unique] = sample;
            unique += 1;
        }
    }
    (2..=AV2_LOSSLESS_PALETTE_PARTITION_UNIQUE_LIMIT).contains(&unique)
}

fn lossless_luma_region_is_simple_for_adaptive_leaf(
    geometry: Av2VideoGeometry,
    bit_depth: SampleBitDepth,
    source: &[u8],
    x0: usize,
    y0: usize,
    width: usize,
    height: usize,
) -> bool {
    debug_assert!(width <= AV2_LOSSLESS_ADAPTIVE_LEAF_SIZE);
    debug_assert!(height <= AV2_LOSSLESS_ADAPTIVE_LEAF_SIZE);
    let normalize_shift = bit_depth.bits().saturating_sub(8);
    let mut seen = [false; 256];
    let mut unique = 0usize;
    let mut min_sample = u16::MAX;
    let mut max_sample = 0u16;
    let mut gradient_sum = 0u64;
    let mut gradient_edges = 0u64;
    let mut above = [0u16; AV2_LOSSLESS_ADAPTIVE_LEAF_SIZE / AV2_LOSSLESS_ADAPTIVE_SAMPLE_STEP];
    for (sample_row, y) in (y0..(y0 + height))
        .step_by(AV2_LOSSLESS_ADAPTIVE_SAMPLE_STEP)
        .enumerate()
    {
        let mut prev = None;
        for (sample_col, x) in (x0..(x0 + width))
            .step_by(AV2_LOSSLESS_ADAPTIVE_SAMPLE_STEP)
            .enumerate()
        {
            let sample = read_validated_planar_sample(
                source,
                y * geometry.width + x,
                bit_depth,
            ) >> normalize_shift;
            let sample_index = usize::from(sample);
            if !seen[sample_index] {
                seen[sample_index] = true;
                unique += 1;
            }
            min_sample = min_sample.min(sample);
            max_sample = max_sample.max(sample);
            if let Some(left) = prev {
                gradient_sum += u64::from(sample.abs_diff(left));
                gradient_edges += 1;
            }
            if sample_row > 0 {
                gradient_sum += u64::from(sample.abs_diff(above[sample_col]));
                gradient_edges += 1;
            }
            above[sample_col] = sample;
            prev = Some(sample);
        }
    }

    let range = max_sample - min_sample;
    let gradient_q8 = if gradient_edges == 0 {
        0
    } else {
        (gradient_sum * 256) / gradient_edges
    };
    unique <= AV2_LOSSLESS_ADAPTIVE_UNIQUE_LIMIT
        || range <= AV2_LOSSLESS_ADAPTIVE_RANGE_LIMIT
        || (unique <= AV2_LOSSLESS_ADAPTIVE_GRADIENT_UNIQUE_LIMIT
            && gradient_q8 <= AV2_LOSSLESS_ADAPTIVE_GRADIENT_Q8_LIMIT)
}

fn choose_lossless_adaptive_32_partition(
    row_mi: usize,
    col_mi: usize,
    block_size: Av2MvpBlockSize,
    visible_rows_mi: usize,
    visible_cols_mi: usize,
    features: &Av2LosslessPartitionFeatures,
) -> Av2MvpPartition {
    if !block_size.is_partition_point() {
        return Av2MvpPartition::None;
    }

    let allowed = allowed_partitions(row_mi, col_mi, block_size, visible_rows_mi, visible_cols_mi);
    if let Some(forced) =
        forced_boundary_partition(row_mi, col_mi, block_size, visible_rows_mi, visible_cols_mi)
    {
        if allowed.contains(forced) {
            return forced;
        }
    }
    if let Some(only_allowed) = allowed.only() {
        return only_allowed;
    }
    if allowed.none
        && block_size.width <= AV2_LOSSLESS_ADAPTIVE_LEAF_SIZE
        && block_size.height <= AV2_LOSSLESS_ADAPTIVE_LEAF_SIZE
        && features.allows_larger_leaf(row_mi, col_mi, block_size)
    {
        return Av2MvpPartition::None;
    }
    let base_leaf_size = features.base_leaf_size(row_mi, col_mi, block_size);
    if allowed.none
        && block_size.width <= base_leaf_size
        && block_size.height <= base_leaf_size
    {
        return Av2MvpPartition::None;
    }

    let max_size = if block_size.width <= AV2_LOSSLESS_ADAPTIVE_LEAF_SIZE
        && block_size.height <= AV2_LOSSLESS_ADAPTIVE_LEAF_SIZE
    {
        base_leaf_size
    } else {
        AV2_LOSSLESS_ADAPTIVE_LEAF_SIZE
    };

    if block_size.width == block_size.height {
        if block_size.height > max_size && allowed.horz {
            return Av2MvpPartition::Horz;
        }
        if block_size.width > max_size && allowed.vert {
            return Av2MvpPartition::Vert;
        }
    } else if block_size.width > block_size.height {
        if block_size.width > max_size && allowed.vert {
            return Av2MvpPartition::Vert;
        }
        if block_size.height > max_size && allowed.horz {
            return Av2MvpPartition::Horz;
        }
    } else {
        if block_size.height > max_size && allowed.horz {
            return Av2MvpPartition::Horz;
        }
        if block_size.width > max_size && allowed.vert {
            return Av2MvpPartition::Vert;
        }
    }

    if allowed.none {
        Av2MvpPartition::None
    } else if allowed.horz {
        Av2MvpPartition::Horz
    } else if allowed.vert {
        Av2MvpPartition::Vert
    } else {
        Av2MvpPartition::None
    }
}

fn cached_lossless_subsampled_mode(
    cache: &mut Option<(Av2TileDecision, Av2LosslessSubsampledModeDecision)>,
    lossless: &Av2LosslessSubsampledTileState<'_>,
    decision: Av2TileDecision,
    visible_rows_mi: usize,
    visible_cols_mi: usize,
    coded_mi_context: &Av2CodedMiContext,
    palette: Option<&Av2LumaPalette444>,
) -> Av2LosslessSubsampledModeDecision {
    if let Some((cached_decision, cached_mode)) = cache {
        if *cached_decision == decision {
            return *cached_mode;
        }
    }
    let mode = lossless.mode_decision_for_leaf(
        decision,
        visible_rows_mi,
        visible_cols_mi,
        coded_mi_context,
        palette,
    );
    *cache = Some((decision, mode));
    mode
}

fn av2_lossless_subsampled_tile_entropy_payload_for_region_with_policy(
    region: Av2TileRegion,
    profile: Av2Black444MvpProfile,
    geometry: Av2VideoGeometry,
    chroma_format: Av2ChromaFormat,
    bit_depth: SampleBitDepth,
    source: &[u8],
    recon: &mut [u8],
    palette: Option<&Av2LumaPalette444>,
    partition_policy: Av2PartitionPolicy,
    mode_search: Av2LosslessSubsampledModeSearch,
    ibc: Option<&Av2LocalIbc444>,
    record_fields: bool,
    copy_fast_recon: bool,
) -> Av2EntropyPayload {
    let lossless_partition_features = (partition_policy == Av2PartitionPolicy::LosslessAdaptive32)
        .then(|| {
            lossless_partition_features_for_source(
                region,
                geometry,
                bit_depth,
                source,
                palette.is_some(),
            )
        });
    let plan = Av2Black444TilePlan::for_region_with_partition_policy_and_features(
        region,
        profile,
        chroma_format,
        partition_policy,
        false,
        ibc.is_some(),
        ibc,
        None,
        lossless_partition_features,
    );
    let mut writer =
        Av2EntropyWriter::with_cdf_updates_and_fields(!profile.disable_cdf_update, record_fields);
    let mut lossless = Av2LosslessSubsampledTileState::new(
        geometry,
        region,
        chroma_format,
        bit_depth,
        mode_search,
        source,
        recon,
    );
    plan.write_lossless_subsampled_entropy(&mut writer, &mut lossless, palette);
    if copy_fast_recon && mode_search == Av2LosslessSubsampledModeSearch::FastScreenContent {
        lossless.copy_source_to_recon_region();
    }
    writer.finish()
}

impl Av2Black444TilePlan {
    fn for_region(
        region: Av2TileRegion,
        profile: Av2Black444MvpProfile,
        chroma_format: Av2ChromaFormat,
        luma_palette: bool,
        allow_intrabc: bool,
        ibc: Option<&Av2LocalIbc444>,
        palette: Option<&Av2LumaPalette444>,
    ) -> Self {
        Self::for_region_with_partition_policy_and_features(
            region,
            profile,
            chroma_format,
            Av2PartitionPolicy::Fixed8x8Leaves,
            luma_palette,
            allow_intrabc,
            ibc,
            palette,
            None,
        )
    }

    fn for_region_with_partition_policy(
        region: Av2TileRegion,
        profile: Av2Black444MvpProfile,
        chroma_format: Av2ChromaFormat,
        partition_policy: Av2PartitionPolicy,
        luma_palette: bool,
        allow_intrabc: bool,
        ibc: Option<&Av2LocalIbc444>,
        palette: Option<&Av2LumaPalette444>,
    ) -> Self {
        Self::for_region_with_partition_policy_and_features(
            region,
            profile,
            chroma_format,
            partition_policy,
            luma_palette,
            allow_intrabc,
            ibc,
            palette,
            None,
        )
    }

    fn for_region_with_partition_policy_and_features(
        region: Av2TileRegion,
        profile: Av2Black444MvpProfile,
        chroma_format: Av2ChromaFormat,
        partition_policy: Av2PartitionPolicy,
        luma_palette: bool,
        allow_intrabc: bool,
        ibc: Option<&Av2LocalIbc444>,
        palette: Option<&Av2LumaPalette444>,
        lossless_partition_features: Option<Av2LosslessPartitionFeatures>,
    ) -> Self {
        assert!(
            !profile.enable_sdp,
            "AV2 MVP tile plan expects a shared luma/chroma partition tree"
        );
        assert!(
            region.origin_x % MVP_SUPERBLOCK_SIZE == 0
                && region.origin_y % MVP_SUPERBLOCK_SIZE == 0,
            "AV2 MVP tiles are aligned to 64x64 superblock origins"
        );
        assert!(
            region.width % 8 == 0 && region.height % 8 == 0,
            "AV2 MVP tile plan expects visible dimensions in 8-pixel units"
        );
        let geometry = region.geometry();
        let visible_rows_mi = geometry.height / MI_SIZE;
        let visible_cols_mi = geometry.width / MI_SIZE;
        let max_ref_bv_count = usize::from(profile.def_max_bvp_drl_bits_minus_min) + 2;
        let mut plan = Self {
            decisions: Vec::new(),
            origin_x: region.origin_x,
            origin_y: region.origin_y,
            chroma_format,
            partition_policy,
            visible_rows_mi,
            visible_cols_mi,
            luma_palette,
            allow_intrabc,
            max_ref_bv_count,
            lossless_partition_features,
        };
        let mut partition_context = Av2PartitionContext::new(visible_rows_mi, visible_cols_mi);
        for row_mi in (0..visible_rows_mi).step_by(PARTITION_CONTEXT_DIM) {
            for col_mi in (0..visible_cols_mi).step_by(PARTITION_CONTEXT_DIM) {
                plan.visit_block(
                    row_mi,
                    col_mi,
                    Av2MvpBlockSize::BLOCK_64X64,
                    visible_rows_mi,
                    visible_cols_mi,
                    &mut partition_context,
                    ibc,
                    palette,
                );
            }
        }
        plan
    }

    fn visit_block(
        &mut self,
        row_mi: usize,
        col_mi: usize,
        block_size: Av2MvpBlockSize,
        visible_rows_mi: usize,
        visible_cols_mi: usize,
        partition_context: &mut Av2PartitionContext,
        ibc: Option<&Av2LocalIbc444>,
        palette: Option<&Av2LumaPalette444>,
    ) {
        if row_mi >= visible_rows_mi || col_mi >= visible_cols_mi {
            return;
        }

        let partition = if self.luma_palette {
            choose_luma_palette_partition(
                row_mi,
                col_mi,
                block_size,
                visible_rows_mi,
                visible_cols_mi,
                self.partition_policy,
                palette,
            )
        } else {
            match self.partition_policy {
                Av2PartitionPolicy::Fixed8x8Leaves => {
                    choose_partition(row_mi, col_mi, block_size, visible_rows_mi, visible_cols_mi)
                }
                Av2PartitionPolicy::LargestLosslessLeaves => choose_largest_lossless_partition(
                    row_mi,
                    col_mi,
                    block_size,
                    visible_rows_mi,
                    visible_cols_mi,
                ),
                Av2PartitionPolicy::LosslessLeafLimit { max_size } => {
                    choose_lossless_leaf_limit_partition(
                        row_mi,
                        col_mi,
                        block_size,
                        visible_rows_mi,
                        visible_cols_mi,
                        max_size,
                    )
                }
                Av2PartitionPolicy::LosslessAdaptive32 => choose_lossless_adaptive_32_partition(
                    row_mi,
                    col_mi,
                    block_size,
                    visible_rows_mi,
                    visible_cols_mi,
                    self.lossless_partition_features
                        .as_ref()
                        .expect("adaptive lossless partitioning needs source features"),
                ),
            }
        };
        self.decisions.push(Av2TileDecision {
            kind: Av2TileDecisionKind::Partition(partition),
            row: row_mi,
            col: col_mi,
            block_size,
        });

        match partition {
            Av2MvpPartition::None => {
                self.visit_leaf(row_mi, col_mi, block_size, ibc, palette);
                partition_context.update_leaf(row_mi, col_mi, block_size);
            }
            Av2MvpPartition::Horz => {
                let subsize = block_size
                    .subsize(partition)
                    .expect("AV2 MVP horizontal partition must have a subsize");
                self.visit_block(
                    row_mi,
                    col_mi,
                    subsize,
                    visible_rows_mi,
                    visible_cols_mi,
                    partition_context,
                    ibc,
                    palette,
                );
                self.visit_block(
                    row_mi + block_size.mi_height() / 2,
                    col_mi,
                    subsize,
                    visible_rows_mi,
                    visible_cols_mi,
                    partition_context,
                    ibc,
                    palette,
                );
            }
            Av2MvpPartition::Vert => {
                let subsize = block_size
                    .subsize(partition)
                    .expect("AV2 MVP vertical partition must have a subsize");
                self.visit_block(
                    row_mi,
                    col_mi,
                    subsize,
                    visible_rows_mi,
                    visible_cols_mi,
                    partition_context,
                    ibc,
                    palette,
                );
                self.visit_block(
                    row_mi,
                    col_mi + block_size.mi_width() / 2,
                    subsize,
                    visible_rows_mi,
                    visible_cols_mi,
                    partition_context,
                    ibc,
                    palette,
                );
            }
        }
    }

    fn visit_leaf(
        &mut self,
        row_mi: usize,
        col_mi: usize,
        block_size: Av2MvpBlockSize,
        ibc: Option<&Av2LocalIbc444>,
        palette: Option<&Av2LumaPalette444>,
    ) {
        assert!(
            block_size.width >= MVP_LEAF_BLOCK_SIZE && block_size.height >= MVP_LEAF_BLOCK_SIZE,
            "AV2 MVP coding leaves must be at least 8x8 blocks"
        );
        let x0 = self.origin_x + col_mi * MI_SIZE;
        let y0 = self.origin_y + row_mi * MI_SIZE;
        let ibc_copy = ibc.and_then(|ibc| ibc.candidate_copy(x0, y0));
        let ibc_drl_idx = ibc_copy.map(|copy| copy.drl_idx());
        let merged_luma_palette_leaf = self.luma_palette
            && (block_size.width > AV2_LUMA_PALETTE_BLOCK_SIZE
                || block_size.height > AV2_LUMA_PALETTE_BLOCK_SIZE);
        let luma_mode = if merged_luma_palette_leaf {
            Av2LumaIntraMode::Dc
        } else {
            palette
                .map(|palette| palette.luma_mode_for_block(x0, y0))
                .unwrap_or(Av2LumaIntraMode::Dc)
        };
        let luma_bdpcm_horz = if merged_luma_palette_leaf {
            None
        } else {
            palette.and_then(|palette| palette.luma_bdpcm_horz_for_block(x0, y0))
        };
        let mut chroma_intra_mode = palette
            .map(|palette| palette.chroma_intra_mode_for_block(x0, y0))
            .unwrap_or(Av2ChromaIntraMode::Horizontal);
        let chroma_use_bdpcm = palette
            .map(|palette| palette.chroma_use_bdpcm_for_block(x0, y0))
            .unwrap_or(false);
        if self.luma_palette
            && !chroma_use_bdpcm
            && chroma_intra_mode == Av2ChromaIntraMode::Dc
        {
            // Avoid fragile single-DC chroma residual patterns in the current
            // 4:4:4 palette path; vertical prediction keeps AVM lossless.
            chroma_intra_mode = Av2ChromaIntraMode::Vertical;
        }
        let prediction = decide_leaf_prediction(
            self.allow_intrabc,
            ibc_drl_idx,
            self.luma_palette,
            luma_mode,
            luma_bdpcm_horz,
            chroma_use_bdpcm,
            chroma_intra_mode,
        );
        if self.allow_intrabc {
            self.decisions.push(Av2TileDecision {
                kind: Av2TileDecisionKind::IntrabcFlag(prediction.intrabc_flag),
                row: row_mi,
                col: col_mi,
                block_size,
            });
        }
        match prediction.prediction {
            Av2LeafPredictionMode::IntrabcCopy { drl_idx } => {
                self.decisions.push(Av2TileDecision {
                    kind: Av2TileDecisionKind::IntrabcCopy {
                        drl_idx,
                        explicit_dv: ibc_copy.and_then(|copy| copy.explicit_dv()),
                    },
                    row: row_mi,
                    col: col_mi,
                    block_size,
                });
            }
            Av2LeafPredictionMode::Intra {
                luma_mode,
                use_luma_palette,
                use_dpcm_y,
                luma_bdpcm_horz,
                use_bdpcm_uv,
                chroma_intra_mode,
            } => {
                let use_fsc = AV2_ENABLE_LUMA_PALETTE_FSC_444
                    && use_luma_palette
                    && block_size.width == AV2_LUMA_PALETTE_BLOCK_SIZE
                    && block_size.height == AV2_LUMA_PALETTE_BLOCK_SIZE
                    && !use_dpcm_y
                    && palette.is_some_and(|palette| {
                        luma_palette_fsc_is_rate_worthy(
                            palette,
                            x0,
                            y0,
                            self.origin_x,
                            self.origin_y,
                            chroma_use_bdpcm,
                            chroma_intra_mode,
                        )
                    });
                self.decisions.push(Av2TileDecision {
                    kind: Av2TileDecisionKind::IntraLumaMode {
                        mode: luma_mode,
                        use_dpcm_y,
                        dpcm_horz: luma_bdpcm_horz,
                        use_fsc,
                    },
                    row: row_mi,
                    col: col_mi,
                    block_size,
                });
                let coded_luma_mode = if use_dpcm_y {
                    if luma_bdpcm_horz {
                        Av2LumaIntraMode::Horizontal
                    } else {
                        Av2LumaIntraMode::Vertical
                    }
                } else {
                    luma_mode
                };
                self.decisions.push(Av2TileDecision {
                    kind: Av2TileDecisionKind::IntraChromaMode {
                        use_bdpcm_uv,
                        luma_mode: coded_luma_mode,
                        chroma_intra_mode,
                    },
                    row: row_mi,
                    col: col_mi,
                    block_size,
                });
                if use_luma_palette {
                    self.decisions.push(Av2TileDecision {
                        kind: Av2TileDecisionKind::LumaPaletteModeInfo,
                        row: row_mi,
                        col: col_mi,
                        block_size,
                    });
                    self.decisions.push(Av2TileDecision {
                        kind: Av2TileDecisionKind::LumaPaletteColorMap,
                        row: row_mi,
                        col: col_mi,
                        block_size,
                    });
                }
                match prediction.residual {
                    Av2LeafResidualMode::BlackDc => {
                        self.decisions.push(Av2TileDecision {
                            kind: Av2TileDecisionKind::BlackDcResidualCoefficients,
                            row: row_mi,
                            col: col_mi,
                            block_size,
                        });
                    }
                    Av2LeafResidualMode::LumaPalette {
                        luma_bdpcm_horz,
                        chroma_use_bdpcm,
                        chroma_intra_mode,
                    } => {
                        self.decisions.push(Av2TileDecision {
                            kind: Av2TileDecisionKind::LumaPaletteResidualCoefficients {
                                luma_bdpcm_horz,
                                chroma_use_bdpcm,
                                chroma_intra_mode,
                                use_fsc,
                            },
                            row: row_mi,
                            col: col_mi,
                            block_size,
                        });
                    }
                    Av2LeafResidualMode::None => {}
                }
            }
        }
    }

    fn write_entropy(
        &self,
        writer: &mut Av2EntropyWriter,
        palette: Option<&Av2LumaPalette444>,
        _ibc: Option<&Av2LocalIbc444>,
    ) {
        let mut partition_context =
            Av2PartitionContext::new(self.visible_rows_mi, self.visible_cols_mi);
        let mut txb_contexts =
            Av2TxbEntropyContexts::new(self.visible_rows_mi, self.visible_cols_mi);
        let mut intrabc_context =
            Av2IntrabcContext::new(self.visible_rows_mi, self.visible_cols_mi);
        let mut coded_mi_context =
            Av2CodedMiContext::new(self.visible_rows_mi, self.visible_cols_mi);
        let mut palette_cache_context =
            Av2PaletteColorCacheContext::new(self.visible_rows_mi, self.visible_cols_mi);
        let mut luma_mode_context =
            Av2LumaModeContext::new(self.visible_rows_mi, self.visible_cols_mi);
        let mut fsc_mode_context =
            Av2FscModeContext::new(self.visible_rows_mi, self.visible_cols_mi);
        for decision in &self.decisions {
            match decision.kind {
                Av2TileDecisionKind::Partition(partition) => {
                    write_partition(
                        writer,
                        *decision,
                        partition,
                        &partition_context,
                        self.visible_rows_mi,
                        self.visible_cols_mi,
                    );
                    if partition == Av2MvpPartition::None {
                        partition_context.update_leaf(
                            decision.row,
                            decision.col,
                            decision.block_size,
                        );
                    }
                }
                Av2TileDecisionKind::IntrabcFlag(use_intrabc) => {
                    write_intrabc_flag(writer, *decision, &intrabc_context, use_intrabc);
                }
                Av2TileDecisionKind::IntrabcCopy {
                    drl_idx,
                    explicit_dv,
                } => {
                    write_intrabc_copy(
                        writer,
                        *decision,
                        &intrabc_context,
                        self.profile_max_ref_bv_count(),
                        drl_idx,
                        explicit_dv,
                    );
                    intrabc_context.update_leaf(
                        decision.row,
                        decision.col,
                        decision.block_size,
                        true,
                        true,
                    );
                    txb_contexts.clear_leaf(
                        decision.row,
                        decision.col,
                        decision.block_size,
                        self.visible_rows_mi,
                        self.visible_cols_mi,
                        self.chroma_format,
                    );
                    palette_cache_context.clear_leaf(
                        decision.row,
                        decision.col,
                        decision.block_size,
                    );
                    // AVM av2_get_joint_mode() reports DC_PRED for inter and
                    // IntraBC neighbors. Keep the luma-mode context tied to
                    // actual coded leaves rather than palette pre-analysis so
                    // enabling more IBC copies cannot desynchronize later
                    // intra-mode symbols.
                    luma_mode_context.update_leaf(
                        decision.row,
                        decision.col,
                        decision.block_size,
                        Av2LumaIntraMode::Dc,
                    );
                    fsc_mode_context.update_leaf(
                        decision.row,
                        decision.col,
                        decision.block_size,
                        false,
                    );
                    coded_mi_context.update_leaf(decision.row, decision.col, decision.block_size);
                }
                Av2TileDecisionKind::IntraLumaMode {
                    mode,
                    use_dpcm_y,
                    dpcm_horz,
                    use_fsc,
                } => {
                    let mode_syntax = luma_mode_context.syntax_for_leaf(
                        decision.row,
                        decision.col,
                        decision.block_size,
                    );
                    let mode_context = mode_syntax.context;
                    let mode_index = mode_syntax.index_for(mode);
                    let fsc_context =
                        fsc_mode_context.context(decision.row, decision.col, decision.block_size);
                    write_intra_luma_mode(
                        writer,
                        *decision,
                        mode,
                        mode_context,
                        mode_index,
                        use_dpcm_y,
                        dpcm_horz,
                        use_fsc,
                        fsc_context,
                    );
                    if mode != Av2LumaIntraMode::Dc || use_dpcm_y {
                        palette_cache_context.clear_leaf(
                            decision.row,
                            decision.col,
                            decision.block_size,
                        );
                    }
                    let coded_mode = if use_dpcm_y {
                        if dpcm_horz {
                            Av2LumaIntraMode::Horizontal
                        } else {
                            Av2LumaIntraMode::Vertical
                        }
                    } else {
                        mode
                    };
                    luma_mode_context.update_leaf(
                        decision.row,
                        decision.col,
                        decision.block_size,
                        coded_mode,
                    );
                    fsc_mode_context.update_leaf(
                        decision.row,
                        decision.col,
                        decision.block_size,
                        use_fsc,
                    );
                }
                Av2TileDecisionKind::IntraChromaMode {
                    use_bdpcm_uv,
                    luma_mode,
                    chroma_intra_mode,
                } => {
                    write_intra_chroma_mode(
                        writer,
                        *decision,
                        use_bdpcm_uv,
                        luma_mode,
                        chroma_intra_mode,
                    );
                }
                Av2TileDecisionKind::LumaPaletteModeInfo => {
                    write_luma_palette_mode_info(
                        writer,
                        *decision,
                        palette.expect("luma palette decision needs palette state"),
                        &mut palette_cache_context,
                        self.origin_x,
                        self.origin_y,
                    );
                }
                Av2TileDecisionKind::LumaPaletteColorMap => {
                    write_luma_palette_color_map(
                        writer,
                        *decision,
                        palette.expect("luma palette decision needs palette state"),
                        self.origin_x,
                        self.origin_y,
                    );
                }
                Av2TileDecisionKind::BlackDcResidualCoefficients => {
                    write_black_dc_residual_coefficients(
                        writer,
                        *decision,
                        self.visible_rows_mi,
                        self.visible_cols_mi,
                        self.chroma_format,
                        &mut txb_contexts,
                    );
                    intrabc_context.update_leaf(
                        decision.row,
                        decision.col,
                        decision.block_size,
                        false,
                        false,
                    );
                    coded_mi_context.update_leaf(decision.row, decision.col, decision.block_size);
                }
                Av2TileDecisionKind::LumaPaletteResidualCoefficients {
                    luma_bdpcm_horz,
                    chroma_use_bdpcm,
                    chroma_intra_mode,
                    use_fsc,
                } => {
                    write_luma_palette_residual_coefficients(
                        writer,
                        *decision,
                        self.visible_rows_mi,
                        self.visible_cols_mi,
                        palette.expect("luma palette residual needs palette state"),
                        &mut txb_contexts,
                        &coded_mi_context,
                        self.origin_x,
                        self.origin_y,
                        luma_bdpcm_horz,
                        chroma_use_bdpcm,
                        chroma_intra_mode,
                        use_fsc,
                    );
                    intrabc_context.update_leaf(
                        decision.row,
                        decision.col,
                        decision.block_size,
                        false,
                        false,
                    );
                    coded_mi_context.update_leaf(decision.row, decision.col, decision.block_size);
                }
            }
        }
    }

    fn profile_max_ref_bv_count(&self) -> usize {
        self.max_ref_bv_count
    }

    fn write_lossy_420_entropy(
        &self,
        writer: &mut Av2EntropyWriter,
        lossy: &mut Av2Lossy420TileState<'_>,
    ) {
        let mut partition_context =
            Av2PartitionContext::new(self.visible_rows_mi, self.visible_cols_mi);
        let mut txb_contexts =
            Av2TxbEntropyContexts::new(self.visible_rows_mi, self.visible_cols_mi);
        let mut intrabc_context =
            Av2IntrabcContext::new(self.visible_rows_mi, self.visible_cols_mi);
        for decision in &self.decisions {
            match decision.kind {
                Av2TileDecisionKind::Partition(partition) => {
                    write_partition(
                        writer,
                        *decision,
                        partition,
                        &partition_context,
                        self.visible_rows_mi,
                        self.visible_cols_mi,
                    );
                    if partition == Av2MvpPartition::None {
                        partition_context.update_leaf(
                            decision.row,
                            decision.col,
                            decision.block_size,
                        );
                    }
                }
                Av2TileDecisionKind::IntraLumaMode {
                    mode,
                    use_dpcm_y: _,
                    dpcm_horz: _,
                    use_fsc: _,
                } => {
                    write_intra_luma_mode(
                        writer,
                        *decision,
                        mode,
                        0,
                        mode.mode_index() as u8,
                        false,
                        false,
                        false,
                        0,
                    );
                }
                Av2TileDecisionKind::IntraChromaMode {
                    use_bdpcm_uv: _,
                    luma_mode,
                    chroma_intra_mode: _,
                } => {
                    write_intra_chroma_mode(
                        writer,
                        *decision,
                        false,
                        luma_mode,
                        Av2ChromaIntraMode::Horizontal,
                    );
                }
                Av2TileDecisionKind::BlackDcResidualCoefficients => {
                    write_lossy_420_residual_coefficients(
                        writer,
                        *decision,
                        self.visible_rows_mi,
                        self.visible_cols_mi,
                        &mut txb_contexts,
                        lossy,
                    );
                    intrabc_context.update_leaf(
                        decision.row,
                        decision.col,
                        decision.block_size,
                        false,
                        false,
                    );
                }
                Av2TileDecisionKind::IntrabcFlag(_)
                | Av2TileDecisionKind::IntrabcCopy { .. }
                | Av2TileDecisionKind::LumaPaletteModeInfo
                | Av2TileDecisionKind::LumaPaletteColorMap
                | Av2TileDecisionKind::LumaPaletteResidualCoefficients { .. } => {
                    unreachable!("AV2 4:2:0 residual path disables palette and IntraBC")
                }
            }
        }
    }

    fn write_lossless_subsampled_entropy(
        &self,
        writer: &mut Av2EntropyWriter,
        lossless: &mut Av2LosslessSubsampledTileState<'_>,
        palette: Option<&Av2LumaPalette444>,
    ) {
        let mut partition_context =
            Av2PartitionContext::new(self.visible_rows_mi, self.visible_cols_mi);
        let mut txb_contexts =
            Av2TxbEntropyContexts::new(self.visible_rows_mi, self.visible_cols_mi);
        let mut intrabc_context =
            Av2IntrabcContext::new(self.visible_rows_mi, self.visible_cols_mi);
        let mut coded_mi_context =
            Av2CodedMiContext::new(self.visible_rows_mi, self.visible_cols_mi);
        let mut palette_cache_context =
            Av2PaletteColorCacheContext::new(self.visible_rows_mi, self.visible_cols_mi);
        let mut luma_mode_context =
            Av2LumaModeContext::new(self.visible_rows_mi, self.visible_cols_mi);
        let mut fsc_mode_context =
            Av2FscModeContext::new(self.visible_rows_mi, self.visible_cols_mi);
        let mut mode_cache: Option<(Av2TileDecision, Av2LosslessSubsampledModeDecision)> = None;
        for decision in &self.decisions {
            match decision.kind {
                Av2TileDecisionKind::Partition(partition) => {
                    write_partition(
                        writer,
                        *decision,
                        partition,
                        &partition_context,
                        self.visible_rows_mi,
                        self.visible_cols_mi,
                    );
                    if partition == Av2MvpPartition::None {
                        partition_context.update_leaf(
                            decision.row,
                            decision.col,
                            decision.block_size,
                        );
                    }
                }
                Av2TileDecisionKind::IntrabcFlag(use_intrabc) => {
                    write_intrabc_flag(writer, *decision, &intrabc_context, use_intrabc);
                }
                Av2TileDecisionKind::IntrabcCopy {
                    drl_idx,
                    explicit_dv,
                } => {
                    write_intrabc_copy(
                        writer,
                        *decision,
                        &intrabc_context,
                        self.profile_max_ref_bv_count(),
                        drl_idx,
                        explicit_dv,
                    );
                    intrabc_context.update_leaf(
                        decision.row,
                        decision.col,
                        decision.block_size,
                        true,
                        true,
                    );
                    txb_contexts.clear_leaf(
                        decision.row,
                        decision.col,
                        decision.block_size,
                        self.visible_rows_mi,
                        self.visible_cols_mi,
                        self.chroma_format,
                    );
                    luma_mode_context.update_leaf(
                        decision.row,
                        decision.col,
                        decision.block_size,
                        Av2LumaIntraMode::Dc,
                    );
                    fsc_mode_context.update_leaf(
                        decision.row,
                        decision.col,
                        decision.block_size,
                        false,
                    );
                    coded_mi_context.update_leaf(decision.row, decision.col, decision.block_size);
                    palette_cache_context.clear_leaf(
                        decision.row,
                        decision.col,
                        decision.block_size,
                    );
                    lossless.copy_source_to_recon_leaf(
                        *decision,
                        self.visible_rows_mi,
                        self.visible_cols_mi,
                    );
                }
                Av2TileDecisionKind::IntraLumaMode {
                    mode: _,
                    use_dpcm_y: _,
                    dpcm_horz: _,
                    use_fsc: _,
                } => {
                    let mode = cached_lossless_subsampled_mode(
                        &mut mode_cache,
                        lossless,
                        *decision,
                        self.visible_rows_mi,
                        self.visible_cols_mi,
                        &coded_mi_context,
                        palette,
                    );
                    let coded_luma_mode = mode.coded_luma_mode();
                    let mode_syntax = luma_mode_context.syntax_for_leaf(
                        decision.row,
                        decision.col,
                        decision.block_size,
                    );
                    let fsc_context =
                        fsc_mode_context.context(decision.row, decision.col, decision.block_size);
                    let mode_index = mode_syntax.index_for(coded_luma_mode);
                    write_intra_luma_mode(
                        writer,
                        *decision,
                        coded_luma_mode,
                        mode_syntax.context,
                        mode_index,
                        mode.luma_bdpcm_horz.is_some(),
                        mode.luma_bdpcm_horz.unwrap_or(false),
                        mode.use_fsc,
                        fsc_context,
                    );
                    luma_mode_context.update_leaf(
                        decision.row,
                        decision.col,
                        decision.block_size,
                        coded_luma_mode,
                    );
                    fsc_mode_context.update_leaf(
                        decision.row,
                        decision.col,
                        decision.block_size,
                        mode.use_fsc,
                    );
                }
                Av2TileDecisionKind::IntraChromaMode {
                    use_bdpcm_uv: _,
                    luma_mode: _,
                    chroma_intra_mode: _,
                } => {
                    let mode = cached_lossless_subsampled_mode(
                        &mut mode_cache,
                        lossless,
                        *decision,
                        self.visible_rows_mi,
                        self.visible_cols_mi,
                        &coded_mi_context,
                        palette,
                    );
                    write_intra_chroma_mode(
                        writer,
                        *decision,
                        mode.chroma_use_bdpcm,
                        mode.coded_luma_mode(),
                        mode.chroma_intra_mode,
                    );
                    if mode.use_luma_palette {
                        if let Some(palette) = palette {
                            write_luma_palette_mode_info(
                                writer,
                                *decision,
                                palette,
                                &mut palette_cache_context,
                                self.origin_x,
                                self.origin_y,
                            );
                            write_luma_palette_color_map(
                                writer,
                                *decision,
                                palette,
                                self.origin_x,
                                self.origin_y,
                            );
                        }
                    } else if (self.allow_intrabc || palette.is_some())
                        && mode.coded_luma_mode() == Av2LumaIntraMode::Dc
                        && mode.luma_bdpcm_horz.is_none()
                    {
                        write_luma_palette_absent_mode_info(
                            writer,
                            *decision,
                            &mut palette_cache_context,
                        );
                    } else if palette.is_some() {
                        palette_cache_context.clear_leaf(
                            decision.row,
                            decision.col,
                            decision.block_size,
                        );
                    }
                }
                Av2TileDecisionKind::BlackDcResidualCoefficients => {
                    let mode = cached_lossless_subsampled_mode(
                        &mut mode_cache,
                        lossless,
                        *decision,
                        self.visible_rows_mi,
                        self.visible_cols_mi,
                        &coded_mi_context,
                        palette,
                    );
                    write_lossless_subsampled_residual_coefficients(
                        writer,
                        *decision,
                        self.visible_rows_mi,
                        self.visible_cols_mi,
                        &mut txb_contexts,
                        &coded_mi_context,
                        lossless,
                        mode,
                        palette,
                    );
                    intrabc_context.update_leaf(
                        decision.row,
                        decision.col,
                        decision.block_size,
                        false,
                        false,
                    );
                    coded_mi_context.update_leaf(decision.row, decision.col, decision.block_size);
                }
                Av2TileDecisionKind::LumaPaletteModeInfo
                | Av2TileDecisionKind::LumaPaletteColorMap
                | Av2TileDecisionKind::LumaPaletteResidualCoefficients { .. } => {
                    unreachable!("AV2 planar lossless path emits palette inline")
                }
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Av2TxbEntropyContexts {
    y_above: Vec<u8>,
    y_left: Vec<u8>,
    u_above: Vec<u8>,
    u_left: Vec<u8>,
    v_above: Vec<u8>,
    v_left: Vec<u8>,
}

impl Av2TxbEntropyContexts {
    fn new(visible_rows_mi: usize, visible_cols_mi: usize) -> Self {
        Self {
            y_above: vec![0; visible_cols_mi],
            y_left: vec![0; visible_rows_mi],
            u_above: vec![0; visible_cols_mi],
            u_left: vec![0; visible_rows_mi],
            v_above: vec![0; visible_cols_mi],
            v_left: vec![0; visible_rows_mi],
        }
    }

    fn clear_leaf(
        &mut self,
        row_mi: usize,
        col_mi: usize,
        block_size: Av2MvpBlockSize,
        visible_rows_mi: usize,
        visible_cols_mi: usize,
        chroma_format: Av2ChromaFormat,
    ) {
        let txb_width = block_size
            .tx4x4_width()
            .min(visible_cols_mi.saturating_sub(col_mi));
        let txb_height = block_size
            .tx4x4_height()
            .min(visible_rows_mi.saturating_sub(row_mi));
        for col in col_mi..(col_mi + txb_width).min(self.y_above.len()) {
            self.y_above[col] = 0;
        }
        for row in row_mi..(row_mi + txb_height).min(self.y_left.len()) {
            self.y_left[row] = 0;
        }

        let chroma_span = chroma_tx4x4_span(
            Av2TileDecision {
                kind: Av2TileDecisionKind::Partition(Av2MvpPartition::None),
                row: row_mi,
                col: col_mi,
                block_size,
            },
            visible_rows_mi,
            visible_cols_mi,
            chroma_format,
        );
        for col in
            chroma_span.col..(chroma_span.col + chroma_span.width).min(self.u_above.len())
        {
            self.u_above[col] = 0;
            self.v_above[col] = 0;
        }
        for row in chroma_span.row..(chroma_span.row + chroma_span.height).min(self.u_left.len()) {
            self.u_left[row] = 0;
            self.v_left[row] = 0;
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Av2IntrabcContext {
    coded: Vec<bool>,
    ibc: Vec<bool>,
    skip: Vec<bool>,
    rows: usize,
    cols: usize,
}

impl Av2IntrabcContext {
    fn new(visible_rows_mi: usize, visible_cols_mi: usize) -> Self {
        Self {
            coded: vec![false; visible_rows_mi * visible_cols_mi],
            ibc: vec![false; visible_rows_mi * visible_cols_mi],
            skip: vec![false; visible_rows_mi * visible_cols_mi],
            rows: visible_rows_mi,
            cols: visible_cols_mi,
        }
    }

    fn intrabc_ctx(&self, row_mi: usize, col_mi: usize, block_size: Av2MvpBlockSize) -> usize {
        // AV2 v1.0.0 read_intra_frame_mode_info()/get_intrabc_ctx(): the
        // context is derived from the first two available spatial neighbors
        // in AVM's bottom-left, above-right, left, above scan. At a 64x64 SB
        // top boundary AVM suppresses above/above-right for this context.
        self.neighbor_sum(row_mi, col_mi, block_size, true, |state| state.ibc)
    }

    fn skip_txfm_ctx(&self, row_mi: usize, col_mi: usize, block_size: Av2MvpBlockSize) -> usize {
        // AV2 v1.0.0 read_skip_txfm()/get_txb_ctx() uses neighboring
        // skip_txfm state from the same two-neighbor scan, but the line-buffer
        // variant keeps above/above-right available at SB top boundaries.
        self.neighbor_sum(row_mi, col_mi, block_size, false, |state| state.skip)
    }

    fn update_leaf(
        &mut self,
        row_mi: usize,
        col_mi: usize,
        block_size: Av2MvpBlockSize,
        use_intrabc: bool,
        skip_txfm: bool,
    ) {
        for row in row_mi..(row_mi + block_size.mi_height()).min(self.rows) {
            for col in col_mi..(col_mi + block_size.mi_width()).min(self.cols) {
                let index = row * self.cols + col;
                self.coded[index] = true;
                self.ibc[index] = use_intrabc;
                self.skip[index] = skip_txfm;
            }
        }
    }

    fn neighbor_sum(
        &self,
        row_mi: usize,
        col_mi: usize,
        block_size: Av2MvpBlockSize,
        suppress_above_at_sb_top: bool,
        value: impl Fn(Av2IntrabcNeighborState) -> bool,
    ) -> usize {
        let not_at_sb_top_boundary = row_mi % PARTITION_CONTEXT_DIM != 0;
        let include_above = !suppress_above_at_sb_top || not_at_sb_top_boundary;
        let mut count = 0usize;
        let mut sum = 0usize;

        let mut push = |state: Option<Av2IntrabcNeighborState>| {
            if count >= 2 {
                return;
            }
            if let Some(state) = state {
                sum += usize::from(value(state));
                count += 1;
            }
        };

        push(self.bottom_left_state(row_mi, col_mi, block_size));
        if include_above {
            push(self.above_right_state(row_mi, col_mi, block_size));
        }
        push(self.left_state(row_mi, col_mi));
        if include_above {
            push(self.above_state(row_mi, col_mi));
        }
        sum
    }

    fn state_at(&self, row_mi: usize, col_mi: usize) -> Option<Av2IntrabcNeighborState> {
        if row_mi >= self.rows || col_mi >= self.cols {
            return None;
        }
        let index = row_mi * self.cols + col_mi;
        self.coded[index].then_some(Av2IntrabcNeighborState {
            ibc: self.ibc[index],
            skip: self.skip[index],
        })
    }

    fn bottom_left_state(
        &self,
        row_mi: usize,
        col_mi: usize,
        block_size: Av2MvpBlockSize,
    ) -> Option<Av2IntrabcNeighborState> {
        col_mi
            .checked_sub(1)
            .and_then(|col| self.state_at(row_mi + block_size.mi_height().saturating_sub(1), col))
    }

    fn above_right_state(
        &self,
        row_mi: usize,
        col_mi: usize,
        block_size: Av2MvpBlockSize,
    ) -> Option<Av2IntrabcNeighborState> {
        row_mi
            .checked_sub(1)
            .and_then(|row| self.state_at(row, col_mi + block_size.mi_width().saturating_sub(1)))
    }

    fn left_state(&self, row_mi: usize, col_mi: usize) -> Option<Av2IntrabcNeighborState> {
        col_mi
            .checked_sub(1)
            .and_then(|col| self.state_at(row_mi, col))
    }

    fn above_state(&self, row_mi: usize, col_mi: usize) -> Option<Av2IntrabcNeighborState> {
        row_mi
            .checked_sub(1)
            .and_then(|row| self.state_at(row, col_mi))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Av2IntrabcNeighborState {
    ibc: bool,
    skip: bool,
}
