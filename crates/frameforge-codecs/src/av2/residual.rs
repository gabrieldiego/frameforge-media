fn round_div_i32(value: i32, divisor: i32) -> i32 {
    debug_assert!(divisor > 0);
    if value >= 0 {
        (value + divisor / 2) / divisor
    } else {
        -((-value + divisor / 2) / divisor)
    }
}

fn quantize_i32_to_step(value: i32, step: i32) -> i32 {
    debug_assert!(step > 0);
    round_div_i32(value, step) * step
}

fn write_lossy_subsampled_residual_coefficients(
    writer: &mut Av2EntropyWriter,
    decision: Av2TileDecision,
    visible_rows_mi: usize,
    visible_cols_mi: usize,
    contexts: &mut Av2TxbEntropyContexts,
    lossy: &mut Av2LossySubsampledTileState<'_>,
) {
    let txb_width = decision
        .block_size
        .tx4x4_width()
        .min(visible_cols_mi.saturating_sub(decision.col));
    let txb_height = decision
        .block_size
        .tx4x4_height()
        .min(visible_rows_mi.saturating_sub(decision.row));
    for row in 0..txb_height {
        let abs_row = decision.row + row;
        for col in 0..txb_width {
            let abs_col = decision.col + col;
            let skip_ctx =
                luma_txb_skip_context(contexts.y_above[abs_col], contexts.y_left[abs_row]);
            let dc_sign_ctx = dc_sign_context(contexts.y_above[abs_col], contexts.y_left[abs_row]);
            let (x0, y0) = lossy.txb_origin(Av2LossyPlane::Y, abs_col, abs_row);
            let context = write_lossy_luma_txb(writer, skip_ctx, dc_sign_ctx, lossy, x0, y0);
            contexts.y_above[abs_col] = context;
            contexts.y_left[abs_row] = context;
        }
    }

    let chroma_span = chroma_tx4x4_span(
        decision,
        visible_rows_mi,
        visible_cols_mi,
        lossy.chroma_format,
    );
    let mut last_u_txb_nonzero = false;
    for row in 0..chroma_span.height {
        let abs_row = chroma_span.row + row;
        for col in 0..chroma_span.width {
            let abs_col = chroma_span.col + col;
            let skip_ctx =
                chroma_txb_skip_base_context(contexts.u_above[abs_col], contexts.u_left[abs_row])
                    + 6;
            let (x0, y0) = lossy.txb_origin(Av2LossyPlane::U, abs_col, abs_row);
            let (context, nonzero) = write_lossy_chroma_txb(
                writer,
                Av2ChromaPlane::U,
                skip_ctx,
                lossy,
                x0,
                y0,
            );
            contexts.u_above[abs_col] = context;
            contexts.u_left[abs_row] = context;
            last_u_txb_nonzero = nonzero;
        }
    }

    for row in 0..chroma_span.height {
        let abs_row = chroma_span.row + row;
        for col in 0..chroma_span.width {
            let abs_col = chroma_span.col + col;
            let skip_ctx = v_txb_skip_context_for_chroma_format(
                contexts.v_above[abs_col],
                contexts.v_left[abs_row],
                last_u_txb_nonzero,
                lossy.chroma_format,
                decision.block_size,
            );
            let (x0, y0) = lossy.txb_origin(Av2LossyPlane::V, abs_col, abs_row);
            let (context, _) = write_lossy_chroma_txb(
                writer,
                Av2ChromaPlane::V,
                skip_ctx,
                lossy,
                x0,
                y0,
            );
            contexts.v_above[abs_col] = context;
            contexts.v_left[abs_row] = context;
        }
    }
}

fn write_lossy_luma_txb(
    writer: &mut Av2EntropyWriter,
    skip_ctx: u8,
    dc_sign_ctx: u8,
    lossy: &mut Av2LossySubsampledTileState<'_>,
    x0: usize,
    y0: usize,
) -> u8 {
    let plane = Av2LossyPlane::Y;
    let analysis = lossy.analyze_txb(plane, x0, y0);
    let coefficients = tx4x4_coefficients_from_residual(&analysis.residual, false);
    let quantized_candidate =
        if lossy_should_try_ac_quantized(analysis.dc_sse, lossy.quant_step()) {
            Some(lossy.quantized_residual_candidate(&analysis))
        } else {
            None
        };
    let choice = choose_lossy_txb(
        analysis.delta,
        &analysis.residual,
        &coefficients,
        Av2CoefficientProxyKind::LumaTransform,
        lossy.quant_step(),
        analysis.dc_sse,
        quantized_candidate,
    );
    match choice {
        Av2LossyTxbChoice::Exact => {
            let (context, _) = if tx4x4_residual_is_zero(&analysis.residual) {
                write_y_txb_all_zero(writer, skip_ctx);
                (0, false)
            } else {
                write_luma_palette_residual_txb(writer, skip_ctx, dc_sign_ctx, &coefficients)
            };
            lossy.copy_source_to_recon_txb(plane, x0, y0, &analysis);
            context
        }
        Av2LossyTxbChoice::QuantizedResidual(quantized_residual) => {
            let quantized_coefficients =
                tx4x4_coefficients_from_residual(&quantized_residual, false);
            let (context, _) = if tx4x4_residual_is_zero(&quantized_residual) {
                write_y_txb_all_zero(writer, skip_ctx);
                (0, false)
            } else {
                write_luma_palette_residual_txb(
                    writer,
                    skip_ctx,
                    dc_sign_ctx,
                    &quantized_coefficients,
                )
            };
            lossy.fill_residual_recon_txb(plane, x0, y0, &analysis, &quantized_residual);
            context
        }
        Av2LossyTxbChoice::DcDelta(delta) => {
            let context = write_y_dc_delta_txb(writer, skip_ctx, dc_sign_ctx, delta);
            lossy.fill_quantized_recon_txb(plane, x0, y0, &analysis);
            context
        }
    }
}

fn write_lossy_chroma_txb(
    writer: &mut Av2EntropyWriter,
    chroma_plane: Av2ChromaPlane,
    skip_ctx: u8,
    lossy: &mut Av2LossySubsampledTileState<'_>,
    x0: usize,
    y0: usize,
) -> (u8, bool) {
    let plane = match chroma_plane {
        Av2ChromaPlane::U => Av2LossyPlane::U,
        Av2ChromaPlane::V => Av2LossyPlane::V,
    };
    let analysis = lossy.analyze_txb(plane, x0, y0);
    let coefficients = tx4x4_coefficients_from_residual(&analysis.residual, false);
    let quantized_candidate =
        if lossy_should_try_ac_quantized(analysis.dc_sse, lossy.quant_step()) {
            Some(lossy.quantized_residual_candidate(&analysis))
        } else {
            None
        };
    let choice = choose_lossy_txb(
        analysis.delta,
        &analysis.residual,
        &coefficients,
        Av2CoefficientProxyKind::ChromaTransform,
        lossy.quant_step(),
        analysis.dc_sse,
        quantized_candidate,
    );
    match choice {
        Av2LossyTxbChoice::Exact => {
            let result = if tx4x4_residual_is_zero(&analysis.residual) {
                match chroma_plane {
                    Av2ChromaPlane::U => write_u_txb_all_zero(writer, skip_ctx, false),
                    Av2ChromaPlane::V => write_v_txb_all_zero(writer, skip_ctx),
                }
                (0, false)
            } else {
                write_chroma_bdpcm_txb(writer, chroma_plane, skip_ctx, &coefficients, false)
            };
            lossy.copy_source_to_recon_txb(plane, x0, y0, &analysis);
            result
        }
        Av2LossyTxbChoice::QuantizedResidual(quantized_residual) => {
            let quantized_coefficients =
                tx4x4_coefficients_from_residual(&quantized_residual, false);
            let result = if tx4x4_residual_is_zero(&quantized_residual) {
                match chroma_plane {
                    Av2ChromaPlane::U => write_u_txb_all_zero(writer, skip_ctx, false),
                    Av2ChromaPlane::V => write_v_txb_all_zero(writer, skip_ctx),
                }
                (0, false)
            } else {
                write_chroma_bdpcm_txb(
                    writer,
                    chroma_plane,
                    skip_ctx,
                    &quantized_coefficients,
                    false,
                )
            };
            lossy.fill_residual_recon_txb(plane, x0, y0, &analysis, &quantized_residual);
            result
        }
        Av2LossyTxbChoice::DcDelta(delta) => {
            let result = write_chroma_dc_delta_txb(writer, chroma_plane, skip_ctx, delta);
            lossy.fill_quantized_recon_txb(plane, x0, y0, &analysis);
            result
        }
    }
}

fn choose_lossy_txb(
    quantized_delta: i16,
    exact_residual: &[i32; TX4X4_SAMPLES],
    exact_coefficients: &[i32; TX4X4_SAMPLES],
    kind: Av2CoefficientProxyKind,
    quant_step: i32,
    dc_sse: usize,
    quantized_candidate: Option<([i32; TX4X4_SAMPLES], usize)>,
) -> Av2LossyTxbChoice {
    let exact_score = coefficient_proxy_score(exact_coefficients, kind);
    let mut best_choice = Av2LossyTxbChoice::Exact;
    let mut best_score = exact_score;

    let mut dc_coefficients = [0i32; TX4X4_SAMPLES];
    dc_coefficients[0] = i32::from(quantized_delta) * 32;
    let dc_score = coefficient_proxy_score(&dc_coefficients, kind);
    let dc_candidate_score = lossy_txb_score(dc_score, dc_sse, quant_step);
    if dc_candidate_score < best_score {
        best_score = dc_candidate_score;
        best_choice = Av2LossyTxbChoice::DcDelta(quantized_delta);
    }

    if let Some((quantized_residual, quantized_sse)) = quantized_candidate {
        let quantized_coefficients = tx4x4_coefficients_from_residual(&quantized_residual, false);
        if lossy_ac_candidate_is_reference_clean(&quantized_coefficients) {
            let quantized_score = coefficient_proxy_score(&quantized_coefficients, kind);
            let quantized_candidate_score =
                lossy_txb_score(quantized_score, quantized_sse, quant_step);
            if quantized_candidate_score < best_score {
                best_choice = if *exact_residual == quantized_residual {
                    Av2LossyTxbChoice::Exact
                } else {
                    Av2LossyTxbChoice::QuantizedResidual(quantized_residual)
                };
            }
        }
    }

    best_choice
}

fn lossy_txb_score(rate_score: usize, sse: usize, quant_step: i32) -> usize {
    rate_score.saturating_add(sse / quant_step.max(1) as usize)
}

fn lossy_should_try_ac_quantized(dc_sse: usize, quant_step: i32) -> bool {
    const AC_DISTORTION_GATE_MULTIPLIER: usize = 4;
    let step = quant_step.max(1) as usize;
    let threshold = step
        .saturating_mul(step)
        .saturating_mul(TX4X4_SAMPLES)
        .saturating_mul(AC_DISTORTION_GATE_MULTIPLIER);
    dc_sse >= threshold
}

fn lossy_ac_candidate_is_reference_clean(coefficients: &[i32; TX4X4_SAMPLES]) -> bool {
    const MAX_REFERENCE_CLEAN_EOB: usize = TX4X4_SAMPLES;
    let (_, bounds) = lossless_coefficient_levels_and_bounds(coefficients);
    bounds.is_none_or(|(_, eob)| eob <= MAX_REFERENCE_CLEAN_EOB)
}

fn write_lossless_subsampled_residual_coefficients(
    writer: &mut Av2EntropyWriter,
    decision: Av2TileDecision,
    visible_rows_mi: usize,
    visible_cols_mi: usize,
    contexts: &mut Av2TxbEntropyContexts,
    coded_mi_context: &Av2CodedMiContext,
    lossless: &mut Av2LosslessSubsampledTileState<'_>,
    mode: Av2LosslessSubsampledModeDecision,
    palette: Option<&Av2LumaPalette444>,
) {
    let txb_width = decision
        .block_size
        .tx4x4_width()
        .min(visible_cols_mi.saturating_sub(decision.col));
    let txb_height = decision
        .block_size
        .tx4x4_height()
        .min(visible_rows_mi.saturating_sub(decision.row));
    if mode.use_fsc {
        write_lossless_tx_size_4x4(writer, decision.block_size);
    }
    let (luma_leaf_x0, luma_leaf_y0) =
        lossless.txb_origin(Av2LosslessPlane::Y, decision.col, decision.row);
    let luma_leaf_width = txb_width * TX4X4_SIZE;
    let luma_leaf_height = txb_height * TX4X4_SIZE;
    let luma_palette_region = mode.use_luma_palette.then(|| {
        palette
            .expect("AV2 luma palette mode needs palette state")
            .syntax_region_palette(
                luma_leaf_x0,
                luma_leaf_y0,
                luma_leaf_width,
                luma_leaf_height,
            )
    });
    for row in 0..txb_height {
        let abs_row = decision.row + row;
        for col in 0..txb_width {
            let abs_col = decision.col + col;
            let skip_ctx =
                luma_txb_skip_context(contexts.y_above[abs_col], contexts.y_left[abs_row]);
            let dc_sign_ctx = dc_sign_context(contexts.y_above[abs_col], contexts.y_left[abs_row]);
            let (x0, y0) = lossless.txb_origin(Av2LosslessPlane::Y, abs_col, abs_row);
            let residual = if let Some(region) = luma_palette_region.as_ref() {
                lossless.luma_palette_residual4x4(
                    palette.expect("AV2 luma palette mode needs palette state"),
                    region,
                    x0,
                    y0,
                )
            } else {
                lossless.tx4x4_residual_for_mode(
                    Av2LosslessPlane::Y,
                    x0,
                    y0,
                    mode,
                    luma_leaf_x0,
                    luma_leaf_y0,
                    luma_leaf_width,
                    luma_leaf_height,
                    coded_mi_context,
                )
            };
            let (context, _) = if tx4x4_residual_is_zero(&residual) {
                if mode.use_fsc {
                    write_y_fsc_txb_all_zero(writer);
                } else {
                    write_y_txb_all_zero(writer, skip_ctx);
                }
                (0, false)
            } else if mode.use_fsc {
                let coefficients = tx4x4_coefficients_from_residual(&residual, true);
                write_luma_palette_fsc_txb(writer, &coefficients)
            } else {
                let coefficients = tx4x4_coefficients_from_residual(&residual, false);
                write_luma_palette_residual_txb(writer, skip_ctx, dc_sign_ctx, &coefficients)
            };
            lossless.copy_source_to_recon_txb(Av2LosslessPlane::Y, x0, y0);
            contexts.y_above[abs_col] = context;
            contexts.y_left[abs_row] = context;
        }
    }

    let chroma_span = chroma_tx4x4_span(
        decision,
        visible_rows_mi,
        visible_cols_mi,
        lossless.chroma_format,
    );
    let (chroma_leaf_x0, chroma_leaf_y0) =
        lossless.txb_origin(Av2LosslessPlane::U, chroma_span.col, chroma_span.row);
    let chroma_leaf_width = chroma_span.width * TX4X4_SIZE;
    let chroma_leaf_height = chroma_span.height * TX4X4_SIZE;
    let mut last_u_txb_nonzero = false;
    for row in 0..chroma_span.height {
        let abs_row = chroma_span.row + row;
        for col in 0..chroma_span.width {
            let abs_col = chroma_span.col + col;
            let skip_ctx =
                chroma_txb_skip_base_context(contexts.u_above[abs_col], contexts.u_left[abs_row])
                    + 6;
            let (x0, y0) = lossless.txb_origin(Av2LosslessPlane::U, abs_col, abs_row);
            let residual = lossless.tx4x4_residual_for_mode(
                Av2LosslessPlane::U,
                x0,
                y0,
                mode,
                chroma_leaf_x0,
                chroma_leaf_y0,
                chroma_leaf_width,
                chroma_leaf_height,
                coded_mi_context,
            );
            let (context, nonzero) = if tx4x4_residual_is_zero(&residual) {
                write_u_txb_all_zero(writer, skip_ctx, mode.use_fsc);
                (0, false)
            } else {
                let coefficients = tx4x4_coefficients_from_residual(&residual, mode.use_fsc);
                write_chroma_bdpcm_txb(
                    writer,
                    Av2ChromaPlane::U,
                    skip_ctx,
                    &coefficients,
                    mode.use_fsc,
                )
            };
            lossless.copy_source_to_recon_txb(Av2LosslessPlane::U, x0, y0);
            contexts.u_above[abs_col] = context;
            contexts.u_left[abs_row] = context;
            last_u_txb_nonzero = nonzero;
        }
    }

    for row in 0..chroma_span.height {
        let abs_row = chroma_span.row + row;
        for col in 0..chroma_span.width {
            let abs_col = chroma_span.col + col;
            let skip_ctx = v_txb_skip_context_for_chroma_format(
                contexts.v_above[abs_col],
                contexts.v_left[abs_row],
                last_u_txb_nonzero,
                lossless.chroma_format,
                decision.block_size,
            );
            let (x0, y0) = lossless.txb_origin(Av2LosslessPlane::V, abs_col, abs_row);
            let residual = lossless.tx4x4_residual_for_mode(
                Av2LosslessPlane::V,
                x0,
                y0,
                mode,
                chroma_leaf_x0,
                chroma_leaf_y0,
                chroma_leaf_width,
                chroma_leaf_height,
                coded_mi_context,
            );
            let (context, _) = if tx4x4_residual_is_zero(&residual) {
                write_v_txb_all_zero(writer, skip_ctx);
                (0, false)
            } else {
                let coefficients = tx4x4_coefficients_from_residual(&residual, mode.use_fsc);
                write_chroma_bdpcm_txb(
                    writer,
                    Av2ChromaPlane::V,
                    skip_ctx,
                    &coefficients,
                    mode.use_fsc,
                )
            };
            lossless.copy_source_to_recon_txb(Av2LosslessPlane::V, x0, y0);
            contexts.v_above[abs_col] = context;
            contexts.v_left[abs_row] = context;
        }
    }
}

fn write_lossless_inter_residual_coefficients(
    writer: &mut Av2EntropyWriter,
    decision: Av2TileDecision,
    visible_rows_mi: usize,
    visible_cols_mi: usize,
    contexts: &mut Av2TxbEntropyContexts,
    lossless: &mut Av2LosslessSubsampledTileState<'_>,
    reference: &[u8],
    mv_row_px: i16,
    mv_col_px: i16,
) {
    let txb_width = decision
        .block_size
        .tx4x4_width()
        .min(visible_cols_mi.saturating_sub(decision.col));
    let txb_height = decision
        .block_size
        .tx4x4_height()
        .min(visible_rows_mi.saturating_sub(decision.row));
    for row in 0..txb_height {
        let abs_row = decision.row + row;
        for col in 0..txb_width {
            let abs_col = decision.col + col;
            let skip_ctx =
                luma_txb_skip_context(contexts.y_above[abs_col], contexts.y_left[abs_row]);
            let dc_sign_ctx = dc_sign_context(contexts.y_above[abs_col], contexts.y_left[abs_row]);
            let (x0, y0) = lossless.txb_origin(Av2LosslessPlane::Y, abs_col, abs_row);
            let residual = lossless.inter_residual4x4(
                reference,
                Av2LosslessPlane::Y,
                x0,
                y0,
                mv_row_px,
                mv_col_px,
            );
            let (context, _) = if tx4x4_residual_is_zero(&residual) {
                write_y_txb_all_zero(writer, skip_ctx);
                (0, false)
            } else {
                let coefficients = tx4x4_coefficients_from_residual(&residual, false);
                write_luma_palette_residual_txb(writer, skip_ctx, dc_sign_ctx, &coefficients)
            };
            lossless.copy_source_to_recon_txb(Av2LosslessPlane::Y, x0, y0);
            contexts.y_above[abs_col] = context;
            contexts.y_left[abs_row] = context;
        }
    }

    let chroma_span = chroma_tx4x4_span(
        decision,
        visible_rows_mi,
        visible_cols_mi,
        lossless.chroma_format,
    );
    let mut last_u_txb_nonzero = false;
    for row in 0..chroma_span.height {
        let abs_row = chroma_span.row + row;
        for col in 0..chroma_span.width {
            let abs_col = chroma_span.col + col;
            let skip_ctx =
                chroma_txb_skip_base_context(contexts.u_above[abs_col], contexts.u_left[abs_row])
                    + 6;
            let (x0, y0) = lossless.txb_origin(Av2LosslessPlane::U, abs_col, abs_row);
            let residual = lossless.inter_residual4x4(
                reference,
                Av2LosslessPlane::U,
                x0,
                y0,
                mv_row_px,
                mv_col_px,
            );
            let (context, nonzero) = if tx4x4_residual_is_zero(&residual) {
                write_u_txb_all_zero(writer, skip_ctx, false);
                (0, false)
            } else {
                let coefficients = tx4x4_coefficients_from_residual(&residual, false);
                write_chroma_bdpcm_txb(
                    writer,
                    Av2ChromaPlane::U,
                    skip_ctx,
                    &coefficients,
                    false,
                )
            };
            lossless.copy_source_to_recon_txb(Av2LosslessPlane::U, x0, y0);
            contexts.u_above[abs_col] = context;
            contexts.u_left[abs_row] = context;
            last_u_txb_nonzero = nonzero;
        }
    }

    for row in 0..chroma_span.height {
        let abs_row = chroma_span.row + row;
        for col in 0..chroma_span.width {
            let abs_col = chroma_span.col + col;
            let skip_ctx = v_txb_skip_context_for_chroma_format(
                contexts.v_above[abs_col],
                contexts.v_left[abs_row],
                last_u_txb_nonzero,
                lossless.chroma_format,
                decision.block_size,
            );
            let (x0, y0) = lossless.txb_origin(Av2LosslessPlane::V, abs_col, abs_row);
            let residual = lossless.inter_residual4x4(
                reference,
                Av2LosslessPlane::V,
                x0,
                y0,
                mv_row_px,
                mv_col_px,
            );
            let (context, _) = if tx4x4_residual_is_zero(&residual) {
                write_v_txb_all_zero(writer, skip_ctx);
                (0, false)
            } else {
                let coefficients = tx4x4_coefficients_from_residual(&residual, false);
                write_chroma_bdpcm_txb(
                    writer,
                    Av2ChromaPlane::V,
                    skip_ctx,
                    &coefficients,
                    false,
                )
            };
            lossless.copy_source_to_recon_txb(Av2LosslessPlane::V, x0, y0);
            contexts.v_above[abs_col] = context;
            contexts.v_left[abs_row] = context;
        }
    }
}

fn write_luma_palette_residual_coefficients(
    writer: &mut Av2EntropyWriter,
    decision: Av2TileDecision,
    visible_rows_mi: usize,
    visible_cols_mi: usize,
    palette: &Av2LumaPalette444,
    contexts: &mut Av2TxbEntropyContexts,
    coded_mi_context: &Av2CodedMiContext,
    tile_origin_x: usize,
    tile_origin_y: usize,
    luma_bdpcm_horz: Option<bool>,
    chroma_use_bdpcm: bool,
    chroma_intra_mode: Av2ChromaIntraMode,
    use_fsc: bool,
) {
    // AV2 v1.0.0 Sections 5.20.8.4 palette_tokens() and 5.20.7.27 coeffs():
    // palette supplies a luma predictor, not an escape-coded lossless sample
    // stream. Blocks with more than eight luma values therefore need normal
    // lossless TX_4X4 coefficients for original_y - palette_prediction_y.
    // Chroma palette is not legal in this AV2 branch: av2_allow_palette()
    // accepts PLANE_TYPE_Y only, and AVM keeps palette_size[1] at zero. Chroma
    // therefore remains on an allowed DPCM residual path even though the public
    // FrameForge leaf and input packet are 8x8.
    let txb_width = decision
        .block_size
        .tx4x4_width()
        .min(visible_cols_mi.saturating_sub(decision.col));
    let txb_height = decision
        .block_size
        .tx4x4_height()
        .min(visible_rows_mi.saturating_sub(decision.row));
    let leaf_x0 = tile_origin_x + decision.col * MI_SIZE;
    let leaf_y0 = tile_origin_y + decision.row * MI_SIZE;
    let leaf_width = decision.block_size.width;
    let leaf_height = decision.block_size.height;
    let luma_region = (!use_fsc && luma_bdpcm_horz.is_none())
        .then(|| palette.syntax_region_palette(leaf_x0, leaf_y0, leaf_width, leaf_height));
    if use_fsc {
        write_lossless_tx_size_4x4(writer, decision.block_size);
    }
    for row in 0..txb_height {
        let abs_row = decision.row + row;
        for col in 0..txb_width {
            let abs_col = decision.col + col;
            let skip_ctx =
                luma_txb_skip_context(contexts.y_above[abs_col], contexts.y_left[abs_row]);
            let dc_sign_ctx = dc_sign_context(contexts.y_above[abs_col], contexts.y_left[abs_row]);
            let txb_x0 = tile_origin_x + abs_col * TX4X4_SIZE;
            let txb_y0 = tile_origin_y + abs_row * TX4X4_SIZE;
            let coefficients = if use_fsc {
                luma_palette_idtx4x4_coefficients(palette, txb_x0, txb_y0)
            } else if let Some(horz) = luma_bdpcm_horz {
                luma_bdpcm_tx4x4_coefficients(
                    palette,
                    txb_x0,
                    txb_y0,
                    tile_origin_x,
                    tile_origin_y,
                    horz,
                )
            } else {
                luma_palette_tx4x4_coefficients(
                    palette,
                    luma_region
                        .as_ref()
                        .expect("luma palette residual needs a region palette"),
                    txb_x0,
                    txb_y0,
                )
            };
            let (context, _) = if use_fsc {
                write_luma_palette_fsc_txb(writer, &coefficients)
            } else {
                write_luma_palette_residual_txb(writer, skip_ctx, dc_sign_ctx, &coefficients)
            };
            contexts.y_above[abs_col] = context;
            contexts.y_left[abs_row] = context;
        }
    }

    let mut last_u_txb_nonzero = false;
    for row in 0..txb_height {
        let abs_row = decision.row + row;
        for col in 0..txb_width {
            let abs_col = decision.col + col;
            let skip_ctx =
                chroma_txb_skip_base_context(contexts.u_above[abs_col], contexts.u_left[abs_row])
                    + 6;
            let txb_x0 = tile_origin_x + abs_col * TX4X4_SIZE;
            let txb_y0 = tile_origin_y + abs_row * TX4X4_SIZE;
            let coefficients = if use_fsc {
                chroma_idtx4x4_coefficients(
                    palette,
                    Av2ChromaPlane::U,
                    txb_x0,
                    txb_y0,
                    tile_origin_x,
                    tile_origin_y,
                    leaf_x0,
                    leaf_y0,
                    leaf_width,
                    leaf_height,
                    coded_mi_context,
                    chroma_use_bdpcm,
                    chroma_intra_mode,
                )
            } else if chroma_use_bdpcm {
                chroma_bdpcm_tx4x4_coefficients(
                    palette,
                    Av2ChromaPlane::U,
                    txb_x0,
                    txb_y0,
                    tile_origin_x,
                    tile_origin_y,
                    chroma_intra_mode.is_horizontal(),
                )
            } else {
                chroma_intra_tx4x4_coefficients(
                    palette,
                    Av2ChromaPlane::U,
                    txb_x0,
                    txb_y0,
                    tile_origin_x,
                    tile_origin_y,
                    leaf_x0,
                    leaf_y0,
                    leaf_width,
                    leaf_height,
                    coded_mi_context,
                    chroma_intra_mode,
                )
            };
            let (context, nonzero) =
                write_chroma_bdpcm_txb(writer, Av2ChromaPlane::U, skip_ctx, &coefficients, use_fsc);
            contexts.u_above[abs_col] = context;
            contexts.u_left[abs_row] = context;
            last_u_txb_nonzero = nonzero;
        }
    }

    for row in 0..txb_height {
        let abs_row = decision.row + row;
        for col in 0..txb_width {
            let abs_col = decision.col + col;
            let skip_ctx = v_txb_skip_context(
                contexts.v_above[abs_col],
                contexts.v_left[abs_row],
                last_u_txb_nonzero,
            );
            let txb_x0 = tile_origin_x + abs_col * TX4X4_SIZE;
            let txb_y0 = tile_origin_y + abs_row * TX4X4_SIZE;
            let coefficients = if use_fsc {
                chroma_idtx4x4_coefficients(
                    palette,
                    Av2ChromaPlane::V,
                    txb_x0,
                    txb_y0,
                    tile_origin_x,
                    tile_origin_y,
                    leaf_x0,
                    leaf_y0,
                    leaf_width,
                    leaf_height,
                    coded_mi_context,
                    chroma_use_bdpcm,
                    chroma_intra_mode,
                )
            } else if chroma_use_bdpcm {
                chroma_bdpcm_tx4x4_coefficients(
                    palette,
                    Av2ChromaPlane::V,
                    txb_x0,
                    txb_y0,
                    tile_origin_x,
                    tile_origin_y,
                    chroma_intra_mode.is_horizontal(),
                )
            } else {
                chroma_intra_tx4x4_coefficients(
                    palette,
                    Av2ChromaPlane::V,
                    txb_x0,
                    txb_y0,
                    tile_origin_x,
                    tile_origin_y,
                    leaf_x0,
                    leaf_y0,
                    leaf_width,
                    leaf_height,
                    coded_mi_context,
                    chroma_intra_mode,
                )
            };
            let (context, _) =
                write_chroma_bdpcm_txb(writer, Av2ChromaPlane::V, skip_ctx, &coefficients, use_fsc);
            contexts.v_above[abs_col] = context;
            contexts.v_left[abs_row] = context;
        }
    }
}

fn write_y_black_dc_txb(writer: &mut Av2EntropyWriter, skip_ctx: u8, dc_sign_ctx: u8) {
    write_y_txb_nonzero(writer, skip_ctx);
    write_eob_one_y(writer);
    write_y_dc_level(writer, BLACK_LOSSLESS_DC_LEVEL);
    write_y_negative_dc_sign(writer, dc_sign_ctx);
    write_y_dc_high_range(writer, BLACK_LOSSLESS_DC_LEVEL);
}

fn write_y_dc_delta_txb(
    writer: &mut Av2EntropyWriter,
    skip_ctx: u8,
    dc_sign_ctx: u8,
    delta: i16,
) -> u8 {
    let coefficients = dc_delta_coefficients(delta);
    let (context, _) = write_luma_palette_residual_txb(writer, skip_ctx, dc_sign_ctx, &coefficients);
    context
}

fn write_u_black_dc_txb(writer: &mut Av2EntropyWriter, skip_ctx: u8) {
    let context = write_u_lossless_dc_txb(writer, skip_ctx, 0);
    assert_eq!(context, NONZERO_NEGATIVE_DC_ENTROPY_CONTEXT);
}

fn write_v_black_dc_txb(writer: &mut Av2EntropyWriter, skip_ctx: u8) {
    let context = write_v_lossless_dc_txb(writer, skip_ctx, 0);
    assert_eq!(context, NONZERO_NEGATIVE_DC_ENTROPY_CONTEXT);
}

fn write_u_lossless_dc_txb(writer: &mut Av2EntropyWriter, skip_ctx: u8, sample: u8) -> u8 {
    if sample == LOSSLESS_DC_PREDICTOR {
        write_u_txb_all_zero(writer, skip_ctx, false);
        return 0;
    }
    let (level, negative) = lossless_dc_level_for_sample(sample);
    write_u_txb_nonzero(writer, skip_ctx, false);
    write_eob_one_uv(writer);
    write_uv_dc_level(writer, level);
    writer.write_literal_bit("tile.coeff.u.dc_sign_negative", negative);
    write_uv_dc_high_range(writer, level);
    nonzero_dc_entropy_context(negative)
}

fn write_v_lossless_dc_txb(writer: &mut Av2EntropyWriter, skip_ctx: u8, sample: u8) -> u8 {
    if sample == LOSSLESS_DC_PREDICTOR {
        write_v_txb_all_zero(writer, skip_ctx);
        return 0;
    }
    let (level, negative) = lossless_dc_level_for_sample(sample);
    write_v_txb_nonzero(writer, skip_ctx);
    write_eob_one_uv(writer);
    write_uv_dc_level(writer, level);
    writer.write_literal_bit("tile.coeff.v.dc_sign_negative", negative);
    write_uv_dc_high_range(writer, level);
    nonzero_dc_entropy_context(negative)
}

fn write_chroma_dc_delta_txb(
    writer: &mut Av2EntropyWriter,
    plane: Av2ChromaPlane,
    skip_ctx: u8,
    delta: i16,
) -> (u8, bool) {
    let coefficients = dc_delta_coefficients(delta);
    write_chroma_bdpcm_txb(writer, plane, skip_ctx, &coefficients, false)
}

fn dc_delta_coefficients(delta: i16) -> [i32; TX4X4_SAMPLES] {
    let mut coefficients = [0i32; TX4X4_SAMPLES];
    coefficients[0] = i32::from(delta) * 32;
    coefficients
}

fn luma_palette_tx4x4_coefficients(
    palette: &Av2LumaPalette444,
    region: &Av2LumaPaletteRegion,
    x0: usize,
    y0: usize,
) -> [i32; TX4X4_SAMPLES] {
    let mut residual = [0i32; TX4X4_SAMPLES];
    for local_y in 0..TX4X4_SIZE {
        let y = y0 + local_y;
        for local_x in 0..TX4X4_SIZE {
            let x = x0 + local_x;
            let original = i32::from(palette.y_sample(x, y));
            let predicted = i32::from(palette.region_prediction_sample(region, x, y));
            residual[local_y * TX4X4_SIZE + local_x] = original - predicted;
        }
    }

    av2_fwht4x4(&residual)
}

fn luma_palette_idtx4x4_coefficients(
    palette: &Av2LumaPalette444,
    x0: usize,
    y0: usize,
) -> [i32; TX4X4_SAMPLES] {
    let mut residual = [0i32; TX4X4_SAMPLES];
    for local_y in 0..TX4X4_SIZE {
        let y = y0 + local_y;
        for local_x in 0..TX4X4_SIZE {
            let x = x0 + local_x;
            let original = i32::from(palette.y_sample(x, y));
            let predicted = i32::from(palette.luma_prediction_sample(x, y));
            residual[local_y * TX4X4_SIZE + local_x] = original - predicted;
        }
    }

    idtx4x4_coefficients(&residual)
}

fn luma_palette_fsc_is_rate_worthy(
    palette: &Av2LumaPalette444,
    leaf_x0: usize,
    leaf_y0: usize,
    tile_origin_x: usize,
    tile_origin_y: usize,
    chroma_use_bdpcm: bool,
    chroma_intra_mode: Av2ChromaIntraMode,
) -> bool {
    let coded_mi_context = Av2CodedMiContext::new(PARTITION_CONTEXT_DIM, PARTITION_CONTEXT_DIM);
    let region = palette.syntax_region_palette(
        leaf_x0,
        leaf_y0,
        AV2_LUMA_PALETTE_BLOCK_SIZE,
        AV2_LUMA_PALETTE_BLOCK_SIZE,
    );
    let mut fsc_score = 96usize;
    let mut transform_score = 0usize;

    for row in 0..(AV2_LUMA_PALETTE_BLOCK_SIZE / TX4X4_SIZE) {
        for col in 0..(AV2_LUMA_PALETTE_BLOCK_SIZE / TX4X4_SIZE) {
            let txb_x0 = leaf_x0 + col * TX4X4_SIZE;
            let txb_y0 = leaf_y0 + row * TX4X4_SIZE;

            fsc_score += coefficient_proxy_score(
                &luma_palette_idtx4x4_coefficients(palette, txb_x0, txb_y0),
                Av2CoefficientProxyKind::LumaIdtx,
            );
            transform_score += coefficient_proxy_score(
                &luma_palette_tx4x4_coefficients(palette, &region, txb_x0, txb_y0),
                Av2CoefficientProxyKind::LumaTransform,
            );

            for plane in [Av2ChromaPlane::U, Av2ChromaPlane::V] {
                fsc_score += coefficient_proxy_score(
                    &chroma_idtx4x4_coefficients(
                        palette,
                        plane,
                        txb_x0,
                        txb_y0,
                        tile_origin_x,
                        tile_origin_y,
                        leaf_x0,
                        leaf_y0,
                        AV2_LUMA_PALETTE_BLOCK_SIZE,
                        AV2_LUMA_PALETTE_BLOCK_SIZE,
                        &coded_mi_context,
                        chroma_use_bdpcm,
                        chroma_intra_mode,
                    ),
                    Av2CoefficientProxyKind::ChromaTransform,
                );
                let transform_coefficients = if chroma_use_bdpcm {
                    chroma_bdpcm_tx4x4_coefficients(
                        palette,
                        plane,
                        txb_x0,
                        txb_y0,
                        tile_origin_x,
                        tile_origin_y,
                        chroma_intra_mode.is_horizontal(),
                    )
                } else {
                    chroma_intra_tx4x4_coefficients(
                        palette,
                        plane,
                        txb_x0,
                        txb_y0,
                        tile_origin_x,
                        tile_origin_y,
                        leaf_x0,
                        leaf_y0,
                        AV2_LUMA_PALETTE_BLOCK_SIZE,
                        AV2_LUMA_PALETTE_BLOCK_SIZE,
                        &coded_mi_context,
                        chroma_intra_mode,
                    )
                };
                transform_score += coefficient_proxy_score(
                    &transform_coefficients,
                    Av2CoefficientProxyKind::ChromaTransform,
                );
            }
        }
    }

    fsc_score < transform_score
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Av2CoefficientProxyKind {
    LumaTransform,
    LumaIdtx,
    ChromaTransform,
}

fn coefficient_proxy_score(
    coefficients: &[i32; TX4X4_SAMPLES],
    kind: Av2CoefficientProxyKind,
) -> usize {
    let (levels, bounds) = lossless_coefficient_levels_and_bounds(coefficients);
    let Some((first, eob)) = bounds else {
        return 16;
    };

    let range = match kind {
        Av2CoefficientProxyKind::LumaIdtx => first..TX4X4_SAMPLES,
        Av2CoefficientProxyKind::LumaTransform | Av2CoefficientProxyKind::ChromaTransform => 0..eob,
    };
    let mut score = 96 + range.len() * 10;
    for scan_index in range {
        let pos = TX4X4_SCAN[scan_index];
        let level = levels[pos] as usize;
        if level == 0 {
            continue;
        }
        score += 80 + level.min(12) * 14;
    }

    let mut high_range_avg = 0u32;
    match kind {
        Av2CoefficientProxyKind::LumaIdtx => {
            for scan_index in 0..TX4X4_SAMPLES {
                score += coefficient_high_range_proxy_score(
                    &levels,
                    kind,
                    TX4X4_SCAN[scan_index],
                    &mut high_range_avg,
                );
            }
        }
        Av2CoefficientProxyKind::LumaTransform | Av2CoefficientProxyKind::ChromaTransform => {
            for scan_index in (0..eob).rev() {
                score += coefficient_high_range_proxy_score(
                    &levels,
                    kind,
                    TX4X4_SCAN[scan_index],
                    &mut high_range_avg,
                );
            }
        }
    }

    score
}

fn coefficient_high_range_proxy_score(
    levels: &[u32; TX4X4_SAMPLES],
    kind: Av2CoefficientProxyKind,
    pos: usize,
    high_range_avg: &mut u32,
) -> usize {
    let level = levels[pos];
    if level == 0 {
        return 0;
    }
    let (threshold, decoded_base) = match kind {
        Av2CoefficientProxyKind::LumaIdtx => (5, 6),
        Av2CoefficientProxyKind::LumaTransform if luma_lf_limits(pos) => (7, 8),
        Av2CoefficientProxyKind::LumaTransform => (5, 6),
        Av2CoefficientProxyKind::ChromaTransform if chroma_lf_limits(pos) => (4, 5),
        Av2CoefficientProxyKind::ChromaTransform => (5, 6),
    };
    if level <= threshold {
        return 0;
    }
    let high_range = level.saturating_sub(decoded_base);
    let score = adaptive_high_range_score_bits(high_range, *high_range_avg) * 64;
    *high_range_avg = (*high_range_avg + high_range) >> 1;
    score
}

fn adaptive_high_range_score_bits(value: u32, context: u32) -> usize {
    let m = adaptive_high_range_rice_parameter(context);
    truncated_rice_score_bits(value, m, m + 1, (m + 4).min(6))
}

fn truncated_rice_score_bits(value: u32, m: u8, k: u8, cmax: u8) -> usize {
    let q = value >> m;
    if q >= u32::from(cmax) {
        usize::from(cmax) + exp_golomb_score_bits(value - (u32::from(cmax) << m), k)
    } else {
        q as usize + 1 + usize::from(m)
    }
}

fn exp_golomb_score_bits(value: u32, k: u8) -> usize {
    let x = value + (1u32 << k);
    let length = (u32::BITS - x.leading_zeros()) as u8;
    usize::from(length - 1 - k + length)
}

fn luma_bdpcm_tx4x4_coefficients(
    palette: &Av2LumaPalette444,
    x0: usize,
    y0: usize,
    tile_origin_x: usize,
    tile_origin_y: usize,
    horz: bool,
) -> [i32; TX4X4_SAMPLES] {
    let mut residual = [0i32; TX4X4_SAMPLES];
    for local_y in 0..TX4X4_SIZE {
        let y = y0 + local_y;
        for local_x in 0..TX4X4_SIZE {
            let x = x0 + local_x;
            let sample = i32::from(palette.y_sample(x, y));
            let predicted_delta = if horz {
                let row_predictor = i32::from(luma_h_predictor(
                    palette,
                    x0,
                    y0,
                    local_y,
                    tile_origin_x,
                    tile_origin_y,
                ));
                if local_x == 0 {
                    sample - row_predictor
                } else {
                    let previous = i32::from(palette.y_sample(x - 1, y));
                    sample - previous
                }
            } else if local_y == 0 {
                let col_predictor = i32::from(luma_v_predictor(
                    palette,
                    x0,
                    y0,
                    local_x,
                    tile_origin_x,
                    tile_origin_y,
                ));
                sample - col_predictor
            } else {
                let previous = i32::from(palette.y_sample(x, y - 1));
                sample - previous
            };
            residual[local_y * TX4X4_SIZE + local_x] = predicted_delta;
        }
    }

    av2_fwht4x4(&residual)
}

fn chroma_bdpcm_tx4x4_coefficients(
    palette: &Av2LumaPalette444,
    plane: Av2ChromaPlane,
    x0: usize,
    y0: usize,
    tile_origin_x: usize,
    tile_origin_y: usize,
    horz: bool,
) -> [i32; TX4X4_SAMPLES] {
    let residual =
        chroma_bdpcm_residual4x4(palette, plane, x0, y0, tile_origin_x, tile_origin_y, horz);
    av2_fwht4x4(&residual)
}

fn chroma_bdpcm_residual4x4(
    palette: &Av2LumaPalette444,
    plane: Av2ChromaPlane,
    x0: usize,
    y0: usize,
    tile_origin_x: usize,
    tile_origin_y: usize,
    horz: bool,
) -> [i32; TX4X4_SAMPLES] {
    let mut residual = [0i32; TX4X4_SAMPLES];
    for local_y in 0..TX4X4_SIZE {
        let y = y0 + local_y;
        for local_x in 0..TX4X4_SIZE {
            let x = x0 + local_x;
            let sample = i32::from(chroma_sample(palette, plane, x, y));
            let predicted_delta = if horz {
                let row_predictor = i32::from(chroma_h_predictor(
                    palette,
                    plane,
                    x0,
                    y0,
                    local_y,
                    tile_origin_x,
                    tile_origin_y,
                ));
                if local_x == 0 {
                    sample - row_predictor
                } else {
                    let previous = i32::from(chroma_sample(palette, plane, x - 1, y));
                    sample - previous
                }
            } else if local_y == 0 {
                let col_predictor = i32::from(chroma_v_predictor(
                    palette,
                    plane,
                    x0,
                    y0,
                    local_x,
                    tile_origin_x,
                    tile_origin_y,
                ));
                sample - col_predictor
            } else {
                let previous = i32::from(chroma_sample(palette, plane, x, y - 1));
                sample - previous
            };
            residual[local_y * TX4X4_SIZE + local_x] = predicted_delta;
        }
    }

    residual
}

fn chroma_intra_tx4x4_coefficients(
    palette: &Av2LumaPalette444,
    plane: Av2ChromaPlane,
    x0: usize,
    y0: usize,
    tile_origin_x: usize,
    tile_origin_y: usize,
    leaf_x0: usize,
    leaf_y0: usize,
    leaf_width: usize,
    leaf_height: usize,
    coded_mi_context: &Av2CodedMiContext,
    mode: Av2ChromaIntraMode,
) -> [i32; TX4X4_SAMPLES] {
    let residual = chroma_intra_residual4x4(
        palette,
        plane,
        x0,
        y0,
        tile_origin_x,
        tile_origin_y,
        leaf_x0,
        leaf_y0,
        leaf_width,
        leaf_height,
        coded_mi_context,
        mode,
    );
    av2_fwht4x4(&residual)
}

fn chroma_intra_residual4x4(
    palette: &Av2LumaPalette444,
    plane: Av2ChromaPlane,
    x0: usize,
    y0: usize,
    tile_origin_x: usize,
    tile_origin_y: usize,
    leaf_x0: usize,
    leaf_y0: usize,
    leaf_width: usize,
    leaf_height: usize,
    coded_mi_context: &Av2CodedMiContext,
    mode: Av2ChromaIntraMode,
) -> [i32; TX4X4_SAMPLES] {
    av2_intra_residual4x4(
        mode,
        None,
        palette.bit_depth(),
        |local_x, local_y| chroma_sample(palette, plane, x0 + local_x, y0 + local_y),
        || chroma_dc_predictor(palette, plane, x0, y0),
        |local_y| chroma_h_predictor(palette, plane, x0, y0, local_y, tile_origin_x, tile_origin_y),
        |local_x| chroma_v_predictor(palette, plane, x0, y0, local_x, tile_origin_x, tile_origin_y),
        || chroma_above_left_predictor(palette, plane, x0, y0, tile_origin_x, tile_origin_y),
        |_angle, local_x, local_y| match mode {
            Av2ChromaIntraMode::Directional45 => {
                let above = chroma_d45_above_edge(
                    palette,
                    plane,
                    x0,
                    y0,
                    tile_origin_x,
                    tile_origin_y,
                    leaf_x0,
                    leaf_y0,
                    leaf_width,
                    coded_mi_context,
                );
                above[local_y + local_x + 1]
            }
            Av2ChromaIntraMode::Directional67 => {
                let above = chroma_d45_above_edge(
                    palette,
                    plane,
                    x0,
                    y0,
                    tile_origin_x,
                    tile_origin_y,
                    leaf_x0,
                    leaf_y0,
                    leaf_width,
                    coded_mi_context,
                );
                directional_interpolate(above, local_x, local_y)
            }
            Av2ChromaIntraMode::Directional135 => {
                let edges = chroma_d135_edges(palette, plane, x0, y0, tile_origin_x, tile_origin_y);
                if local_x >= local_y {
                    let offset = local_x - local_y;
                    if offset == 0 {
                        edges.above_left
                    } else {
                        edges.above[offset - 1]
                    }
                } else {
                    edges.left[local_y - local_x - 1]
                }
            }
            Av2ChromaIntraMode::Directional113 => {
                let edges = chroma_d135_edges(palette, plane, x0, y0, tile_origin_x, tile_origin_y);
                zone2_directional_predictor(edges, 24, 170, local_x, local_y)
            }
            Av2ChromaIntraMode::Directional157 => {
                let edges = chroma_d135_edges(palette, plane, x0, y0, tile_origin_x, tile_origin_y);
                zone2_directional_predictor(edges, 170, 24, local_x, local_y)
            }
            Av2ChromaIntraMode::Directional203 => {
                let left = chroma_d203_left_edge(
                    palette,
                    plane,
                    x0,
                    y0,
                    tile_origin_x,
                    tile_origin_y,
                    leaf_x0,
                    leaf_y0,
                    leaf_height,
                    coded_mi_context,
                );
                directional_interpolate(left, local_y, local_x)
            }
            _ => unreachable!("4:4:4 chroma path only dispatches directional modes here"),
        },
        || {
            chroma_smooth_edges(
                palette,
                plane,
                x0,
                y0,
                tile_origin_x,
                tile_origin_y,
                leaf_x0,
                leaf_y0,
                leaf_width,
                leaf_height,
                coded_mi_context,
            )
        },
    )
}

fn chroma_idtx4x4_coefficients(
    palette: &Av2LumaPalette444,
    plane: Av2ChromaPlane,
    x0: usize,
    y0: usize,
    tile_origin_x: usize,
    tile_origin_y: usize,
    leaf_x0: usize,
    leaf_y0: usize,
    leaf_width: usize,
    leaf_height: usize,
    coded_mi_context: &Av2CodedMiContext,
    use_bdpcm: bool,
    mode: Av2ChromaIntraMode,
) -> [i32; TX4X4_SAMPLES] {
    let residual = if use_bdpcm {
        chroma_bdpcm_residual4x4(
            palette,
            plane,
            x0,
            y0,
            tile_origin_x,
            tile_origin_y,
            mode.is_horizontal(),
        )
    } else {
        chroma_intra_residual4x4(
            palette,
            plane,
            x0,
            y0,
            tile_origin_x,
            tile_origin_y,
            leaf_x0,
            leaf_y0,
            leaf_width,
            leaf_height,
            coded_mi_context,
            mode,
        )
    };
    idtx4x4_coefficients(&residual)
}

fn idtx4x4_coefficients(residual: &[i32; TX4X4_SAMPLES]) -> [i32; TX4X4_SAMPLES] {
    let mut coefficients = [0i32; TX4X4_SAMPLES];
    for (coefficient, residual) in coefficients.iter_mut().zip(residual.iter()) {
        *coefficient = *residual * 8;
    }
    coefficients
}

fn chroma_d45_above_edge(
    palette: &Av2LumaPalette444,
    plane: Av2ChromaPlane,
    txb_x0: usize,
    txb_y0: usize,
    tile_origin_x: usize,
    tile_origin_y: usize,
    leaf_x0: usize,
    leaf_y0: usize,
    leaf_width: usize,
    coded_mi_context: &Av2CodedMiContext,
) -> [Av2Sample; 8] {
    let sb_origin_x = (txb_x0 / MVP_SUPERBLOCK_SIZE) * MVP_SUPERBLOCK_SIZE;
    let sb_right = (sb_origin_x + MVP_SUPERBLOCK_SIZE).min(palette.width());
    let have_top = txb_y0 > tile_origin_y;
    let have_left = txb_x0 > tile_origin_x;
    let mut above = [av2_lossless_v_pred_above_edge(palette.bit_depth()); 8];
    if have_top {
        for index in 0..above.len() {
            let x = txb_x0 + index;
            let external_top_right_coded = txb_y0 == leaf_y0
                && x < sb_right
                && coded_mi_context.is_coded((txb_y0 - 1) / MI_SIZE, x / MI_SIZE);
            if x < leaf_x0 + leaf_width || external_top_right_coded {
                above[index] = chroma_sample(palette, plane, x, txb_y0 - 1);
            } else if index > 0 {
                above[index] = above[index - 1];
            }
        }
    } else if have_left {
        above.fill(chroma_sample(palette, plane, txb_x0 - 1, txb_y0));
    }
    above
}
