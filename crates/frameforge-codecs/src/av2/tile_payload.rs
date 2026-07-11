#[cfg(test)]
pub(crate) fn av2_black_444_tile_entropy_payload(
    geometry: Av2VideoGeometry,
    profile: Av2Black444MvpProfile,
) -> Av2EntropyPayload {
    av2_black_444_tile_entropy_payload_for_region(Av2TileRegion::root(geometry), profile)
}

pub(crate) fn av2_black_444_tile_entropy_payload_for_region(
    region: Av2TileRegion,
    profile: Av2Black444MvpProfile,
) -> Av2EntropyPayload {
    av2_black_444_tile_entropy_payload_for_region_with_intrabc(region, profile, false)
}

pub(crate) fn av2_black_444_tile_entropy_payload_for_region_with_intrabc(
    region: Av2TileRegion,
    profile: Av2Black444MvpProfile,
    allow_intrabc: bool,
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
    let mut writer = Av2EntropyWriter::with_cdf_updates(!profile.disable_cdf_update);
    plan.write_entropy(&mut writer, None, None);
    writer.finish()
}

pub(crate) fn av2_black_tile_entropy_payload_for_region(
    region: Av2TileRegion,
    profile: Av2Black444MvpProfile,
    chroma_format: Av2ChromaFormat,
) -> Av2EntropyPayload {
    let plan =
        Av2Black444TilePlan::for_region(region, profile, chroma_format, false, false, None, None);
    let mut writer = Av2EntropyWriter::with_cdf_updates(!profile.disable_cdf_update);
    plan.write_entropy(&mut writer, None, None);
    writer.finish()
}

pub(crate) fn av2_luma_palette_444_tile_entropy_payload_for_region(
    region: Av2TileRegion,
    profile: Av2Black444MvpProfile,
    allow_intrabc: bool,
    palette: &Av2LumaPalette444,
    ibc: &Av2LocalIbc444,
) -> Av2EntropyPayload {
    let mut best: Option<Av2EntropyPayload> = None;
    for partition_policy in [
        Av2PartitionPolicy::LargestLosslessLeaves,
        Av2PartitionPolicy::LosslessLeafLimit { max_size: 32 },
        Av2PartitionPolicy::LosslessLeafLimit { max_size: 16 },
        Av2PartitionPolicy::Fixed8x8Leaves,
    ] {
        let payload = av2_luma_palette_444_tile_entropy_payload_for_region_with_policy(
            region,
            profile,
            allow_intrabc,
            palette,
            ibc,
            partition_policy,
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
    ibc: &Av2LocalIbc444,
    partition_policy: Av2PartitionPolicy,
) -> Av2EntropyPayload {
    let plan = Av2Black444TilePlan::for_region_with_partition_policy(
        region,
        profile,
        Av2ChromaFormat::Yuv444,
        partition_policy,
        true,
        allow_intrabc,
        Some(ibc),
        Some(palette),
    );
    let mut writer = Av2EntropyWriter::with_cdf_updates(!profile.disable_cdf_update);
    plan.write_entropy(&mut writer, Some(palette), Some(ibc));
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
    let plan = Av2Black444TilePlan::for_region(
        region,
        profile,
        Av2ChromaFormat::Yuv420,
        false,
        false,
        None,
        None,
    );
    let mut writer = Av2EntropyWriter::with_cdf_updates(!profile.disable_cdf_update);
    let mut lossy = Av2Lossy420TileState::new(geometry, region, bit_depth, source, recon);
    plan.write_lossy_420_entropy(&mut writer, &mut lossy);
    writer.finish()
}

pub(crate) fn av2_lossless_subsampled_tile_entropy_payload_for_region(
    region: Av2TileRegion,
    profile: Av2Black444MvpProfile,
    geometry: Av2VideoGeometry,
    chroma_format: Av2ChromaFormat,
    bit_depth: SampleBitDepth,
    source: &[u8],
    recon: &mut [u8],
) -> Av2EntropyPayload {
    debug_assert!(matches!(
        chroma_format,
        Av2ChromaFormat::Yuv420 | Av2ChromaFormat::Yuv422
    ));
    let mut best: Option<(Av2EntropyPayload, Vec<u8>)> = None;
    for partition_policy in [
        Av2PartitionPolicy::LargestLosslessLeaves,
        Av2PartitionPolicy::LosslessLeafLimit { max_size: 32 },
        Av2PartitionPolicy::LosslessLeafLimit { max_size: 16 },
        Av2PartitionPolicy::Fixed8x8Leaves,
    ] {
        let mut candidate_recon = recon.to_vec();
        let payload = av2_lossless_subsampled_tile_entropy_payload_for_region_with_policy(
            region,
            profile,
            geometry,
            chroma_format,
            bit_depth,
            source,
            &mut candidate_recon,
            partition_policy,
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
        best.expect("AV2 subsampled lossless has fixed partition candidates");
    recon.copy_from_slice(&candidate_recon);
    payload
}

fn av2_lossless_subsampled_tile_entropy_payload_for_region_with_policy(
    region: Av2TileRegion,
    profile: Av2Black444MvpProfile,
    geometry: Av2VideoGeometry,
    chroma_format: Av2ChromaFormat,
    bit_depth: SampleBitDepth,
    source: &[u8],
    recon: &mut [u8],
    partition_policy: Av2PartitionPolicy,
) -> Av2EntropyPayload {
    let plan = Av2Black444TilePlan::for_region_with_partition_policy(
        region,
        profile,
        chroma_format,
        partition_policy,
        false,
        false,
        None,
        None,
    );
    let mut writer = Av2EntropyWriter::with_cdf_updates(!profile.disable_cdf_update);
    let mut lossless = Av2LosslessSubsampledTileState::new(
        geometry,
        region,
        chroma_format,
        bit_depth,
        source,
        recon,
    );
    plan.write_lossless_subsampled_entropy(&mut writer, &mut lossless);
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
        Self::for_region_with_partition_policy(
            region,
            profile,
            chroma_format,
            Av2PartitionPolicy::Fixed8x8Leaves,
            luma_palette,
            allow_intrabc,
            ibc,
            palette,
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
        let chroma_intra_mode = palette
            .map(|palette| palette.chroma_intra_mode_for_block(x0, y0))
            .unwrap_or(Av2ChromaIntraMode::Horizontal);
        let chroma_use_bdpcm = palette
            .map(|palette| palette.chroma_use_bdpcm_for_block(x0, y0))
            .unwrap_or(false);
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
                let use_fsc = use_luma_palette
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
    ) {
        let mut partition_context =
            Av2PartitionContext::new(self.visible_rows_mi, self.visible_cols_mi);
        let mut txb_contexts =
            Av2TxbEntropyContexts::new(self.visible_rows_mi, self.visible_cols_mi);
        let mut intrabc_context =
            Av2IntrabcContext::new(self.visible_rows_mi, self.visible_cols_mi);
        let mut coded_mi_context =
            Av2CodedMiContext::new(self.visible_rows_mi, self.visible_cols_mi);
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
                Av2TileDecisionKind::IntraLumaMode {
                    mode: _,
                    use_dpcm_y: _,
                    dpcm_horz: _,
                    use_fsc: _,
                } => {
                    let mode = lossless.mode_decision_for_leaf(
                        *decision,
                        self.visible_rows_mi,
                        self.visible_cols_mi,
                        &coded_mi_context,
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
                    let mode = lossless.mode_decision_for_leaf(
                        *decision,
                        self.visible_rows_mi,
                        self.visible_cols_mi,
                        &coded_mi_context,
                    );
                    write_intra_chroma_mode(
                        writer,
                        *decision,
                        mode.chroma_use_bdpcm,
                        mode.coded_luma_mode(),
                        mode.chroma_intra_mode,
                    );
                }
                Av2TileDecisionKind::BlackDcResidualCoefficients => {
                    write_lossless_subsampled_residual_coefficients(
                        writer,
                        *decision,
                        self.visible_rows_mi,
                        self.visible_cols_mi,
                        &mut txb_contexts,
                        &coded_mi_context,
                        lossless,
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
                Av2TileDecisionKind::IntrabcFlag(_)
                | Av2TileDecisionKind::IntrabcCopy { .. }
                | Av2TileDecisionKind::LumaPaletteModeInfo
                | Av2TileDecisionKind::LumaPaletteColorMap
                | Av2TileDecisionKind::LumaPaletteResidualCoefficients { .. } => {
                    unreachable!("AV2 subsampled lossless path disables palette and IntraBC")
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
    ) {
        let txb_width = block_size
            .tx4x4_width()
            .min(visible_cols_mi.saturating_sub(col_mi));
        let txb_height = block_size
            .tx4x4_height()
            .min(visible_rows_mi.saturating_sub(row_mi));
        for col in col_mi..(col_mi + txb_width).min(self.y_above.len()) {
            self.y_above[col] = 0;
            self.u_above[col] = 0;
            self.v_above[col] = 0;
        }
        for row in row_mi..(row_mi + txb_height).min(self.y_left.len()) {
            self.y_left[row] = 0;
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
