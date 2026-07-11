fn write_black_dc_residual_coefficients(
    writer: &mut Av2EntropyWriter,
    decision: Av2TileDecision,
    visible_rows_mi: usize,
    visible_cols_mi: usize,
    chroma_format: Av2ChromaFormat,
    contexts: &mut Av2TxbEntropyContexts,
) {
    // AV2 v1.0.0 Section 5.20.7.23 residual() sets lossless residuals to
    // TX_4X4 transform blocks.
    // DC_PRED reconstructs 128 at frame/tile boundaries, so a black input
    // needs one negative DC coefficient per TXB. With qindex 0, dequant is 64
    // and the lossless 4x4 inverse WHT divides a DC-only coefficient by four;
    // level 512 therefore produces -128 at every sample after dequant.
    // AV2 v1.0.0 decoding clips residual visits to the visible frame edge;
    // AVM does this through max_block_wide()/max_block_high() after setting
    // the nominal partition block. Match that by emitting only visible TXBs.
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
            write_y_black_dc_txb(writer, skip_ctx, dc_sign_ctx);
            contexts.y_above[abs_col] = NONZERO_NEGATIVE_DC_ENTROPY_CONTEXT;
            contexts.y_left[abs_row] = NONZERO_NEGATIVE_DC_ENTROPY_CONTEXT;
        }
    }

    let chroma_span = chroma_tx4x4_span(decision, visible_rows_mi, visible_cols_mi, chroma_format);

    for row in 0..chroma_span.height {
        let abs_row = chroma_span.row + row;
        for col in 0..chroma_span.width {
            let abs_col = chroma_span.col + col;
            let skip_ctx =
                chroma_txb_skip_base_context(contexts.u_above[abs_col], contexts.u_left[abs_row])
                    + 6;
            write_u_black_dc_txb(writer, skip_ctx);
            contexts.u_above[abs_col] = NONZERO_NEGATIVE_DC_ENTROPY_CONTEXT;
            contexts.u_left[abs_row] = NONZERO_NEGATIVE_DC_ENTROPY_CONTEXT;
        }
    }

    let last_u_txb_nonzero = chroma_span.width != 0 && chroma_span.height != 0;
    for row in 0..chroma_span.height {
        let abs_row = chroma_span.row + row;
        for col in 0..chroma_span.width {
            let abs_col = chroma_span.col + col;
            let skip_ctx = v_txb_skip_context_for_chroma_format(
                contexts.v_above[abs_col],
                contexts.v_left[abs_row],
                last_u_txb_nonzero,
                chroma_format,
                decision.block_size,
            );
            write_v_black_dc_txb(writer, skip_ctx);
            contexts.v_above[abs_col] = NONZERO_NEGATIVE_DC_ENTROPY_CONTEXT;
            contexts.v_left[abs_row] = NONZERO_NEGATIVE_DC_ENTROPY_CONTEXT;
        }
    }
}
