#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Av2ChromaTx4x4Span {
    row: usize,
    col: usize,
    width: usize,
    height: usize,
}

fn chroma_tx4x4_span(
    decision: Av2TileDecision,
    visible_rows_mi: usize,
    visible_cols_mi: usize,
    chroma_format: Av2ChromaFormat,
) -> Av2ChromaTx4x4Span {
    match chroma_format {
        Av2ChromaFormat::Yuv444 => Av2ChromaTx4x4Span {
            row: decision.row,
            col: decision.col,
            width: decision
                .block_size
                .tx4x4_width()
                .min(visible_cols_mi.saturating_sub(decision.col)),
            height: decision
                .block_size
                .tx4x4_height()
                .min(visible_rows_mi.saturating_sub(decision.row)),
        },
        Av2ChromaFormat::Yuv422 => {
            // 4:2:2 chroma uses half-resolution columns and full-resolution
            // rows, so an 8x8 luma leaf maps to two vertical 4x4 chroma TXBs.
            let row = decision.row;
            let col = decision.col / 2;
            let visible_rows = visible_rows_mi;
            let visible_cols = visible_cols_mi / 2;
            Av2ChromaTx4x4Span {
                row,
                col,
                width: (decision.block_size.tx4x4_width() / 2)
                    .min(visible_cols.saturating_sub(col)),
                height: decision
                    .block_size
                    .tx4x4_height()
                    .min(visible_rows.saturating_sub(row)),
            }
        }
        Av2ChromaFormat::Yuv420 => {
            // AV2 v1.0.0 residual() uses chroma transform units in chroma
            // sample coordinates. FrameForge's first 4:2:0 milestone keeps
            // 8x8 luma leaves, so each leaf maps to one 4x4 U TXB and one
            // 4x4 V TXB.
            let row = decision.row / 2;
            let col = decision.col / 2;
            let visible_rows = visible_rows_mi / 2;
            let visible_cols = visible_cols_mi / 2;
            Av2ChromaTx4x4Span {
                row,
                col,
                width: (decision.block_size.tx4x4_width() / 2)
                    .min(visible_cols.saturating_sub(col)),
                height: (decision.block_size.tx4x4_height() / 2)
                    .min(visible_rows.saturating_sub(row)),
            }
        }
    }
}

struct Av2LossySubsampledTileState<'a> {
    geometry: Av2VideoGeometry,
    region: Av2TileRegion,
    chroma_format: Av2ChromaFormat,
    bit_depth: SampleBitDepth,
    source: &'a [u8],
    recon: &'a mut [u8],
    y_len: usize,
    c_width: usize,
    c_height: usize,
    c_len: usize,
    qp: u8,
}

#[derive(Clone, Copy)]
struct Av2LossyLeafPredictorContext<'a> {
    leaf_x0: usize,
    leaf_y0: usize,
    leaf_width: usize,
    leaf_height: usize,
    coded_mi_context: &'a Av2CodedMiContext,
}

impl<'a> Av2LossySubsampledTileState<'a> {
    fn new(
        geometry: Av2VideoGeometry,
        region: Av2TileRegion,
        chroma_format: Av2ChromaFormat,
        bit_depth: SampleBitDepth,
        source: &'a [u8],
        recon: &'a mut [u8],
        qp: u8,
    ) -> Self {
        assert!(qp > 0, "AV2 lossy QP must be non-zero");
        let y_len = geometry.width * geometry.height;
        let c_width = geometry.width / chroma_subsample_x(chroma_format);
        let c_height = geometry.height / chroma_subsample_y(chroma_format);
        let c_len = c_width * c_height;
        let expected_len = (y_len + 2 * c_len) * bit_depth.bytes_per_sample();
        assert_eq!(
            source.len(),
            expected_len,
            "AV2 planar lossy residual source length must match geometry"
        );
        assert_eq!(
            recon.len(),
            source.len(),
            "AV2 planar lossy residual reconstruction length must match source"
        );
        Self {
            geometry,
            region,
            chroma_format,
            bit_depth,
            source,
            recon,
            y_len,
            c_width,
            c_height,
            c_len,
            qp,
        }
    }

    fn plane_geometry(&self, plane: Av2LossyPlane) -> (usize, usize) {
        match plane {
            Av2LossyPlane::Y => (self.geometry.width, self.geometry.height),
            Av2LossyPlane::U | Av2LossyPlane::V => (self.c_width, self.c_height),
        }
    }

    fn plane_origin(&self, plane: Av2LossyPlane) -> (usize, usize) {
        match plane {
            Av2LossyPlane::Y => (self.region.origin_x, self.region.origin_y),
            Av2LossyPlane::U | Av2LossyPlane::V => (
                self.region.origin_x / chroma_subsample_x(self.chroma_format),
                self.region.origin_y / chroma_subsample_y(self.chroma_format),
            ),
        }
    }

    fn plane_subsampling(&self, plane: Av2LossyPlane) -> (usize, usize) {
        match plane {
            Av2LossyPlane::Y => (1, 1),
            Av2LossyPlane::U | Av2LossyPlane::V => (
                chroma_subsample_x(self.chroma_format),
                chroma_subsample_y(self.chroma_format),
            ),
        }
    }

    fn coded_mi_for_plane_sample(&self, plane: Av2LossyPlane, x: usize, y: usize) -> (usize, usize) {
        let (sub_x, sub_y) = self.plane_subsampling(plane);
        ((y * sub_y) / MI_SIZE, (x * sub_x) / MI_SIZE)
    }

    fn txb_origin(&self, plane: Av2LossyPlane, col: usize, row: usize) -> (usize, usize) {
        let (origin_x, origin_y) = self.plane_origin(plane);
        (origin_x + col * TX4X4_SIZE, origin_y + row * TX4X4_SIZE)
    }

    fn offset(&self, plane: Av2LossyPlane, x: usize, y: usize) -> usize {
        match plane {
            Av2LossyPlane::Y => y * self.geometry.width + x,
            Av2LossyPlane::U => self.y_len + y * self.c_width + x,
            Av2LossyPlane::V => self.y_len + self.c_len + y * self.c_width + x,
        }
    }

    fn source_sample(&self, plane: Av2LossyPlane, x: usize, y: usize) -> Av2Sample {
        self.read_sample(self.source, self.offset(plane, x, y))
    }

    fn recon_sample(&self, plane: Av2LossyPlane, x: usize, y: usize) -> Av2Sample {
        self.read_sample(self.recon, self.offset(plane, x, y))
    }

    fn set_recon_sample(&mut self, plane: Av2LossyPlane, x: usize, y: usize, sample: Av2Sample) {
        let offset = self.offset(plane, x, y);
        write_lossy_planar_sample(self.recon, offset, sample, self.bit_depth);
    }

    #[inline(always)]
    fn read_sample(&self, input: &[u8], sample_index: usize) -> Av2Sample {
        read_lossy_planar_sample(input, sample_index, self.bit_depth)
    }

    fn dc_predictor(&self, plane: Av2LossyPlane, x0: usize, y0: usize) -> Av2Sample {
        let (tile_origin_x, tile_origin_y) = self.plane_origin(plane);
        let have_left = x0 > tile_origin_x;
        let have_top = y0 > tile_origin_y;
        if !have_left && !have_top {
            return av2_lossless_dc_predictor(self.bit_depth);
        }

        let mut sum = 0u32;
        let mut count = 0u32;
        if have_top {
            for x in x0..(x0 + TX4X4_SIZE) {
                sum += u32::from(self.recon_sample(plane, x, y0 - 1));
                count += 1;
            }
        }
        if have_left {
            for y in y0..(y0 + TX4X4_SIZE) {
                sum += u32::from(self.recon_sample(plane, x0 - 1, y));
                count += 1;
            }
        }
        ((sum + count / 2) / count) as Av2Sample
    }

    fn h_predictor(&self, plane: Av2LossyPlane, x0: usize, y0: usize, local_y: usize) -> Av2Sample {
        let (tile_origin_x, tile_origin_y) = self.plane_origin(plane);
        if x0 > tile_origin_x {
            self.recon_sample(plane, x0 - 1, y0 + local_y)
        } else if y0 > tile_origin_y {
            self.recon_sample(plane, x0, y0 - 1)
        } else {
            av2_lossless_h_pred_left_edge(self.bit_depth)
        }
    }

    fn v_predictor(&self, plane: Av2LossyPlane, x0: usize, y0: usize, local_x: usize) -> Av2Sample {
        let (tile_origin_x, tile_origin_y) = self.plane_origin(plane);
        if y0 > tile_origin_y {
            self.recon_sample(plane, x0 + local_x, y0 - 1)
        } else if x0 > tile_origin_x {
            self.recon_sample(plane, x0 - 1, y0)
        } else {
            av2_lossless_v_pred_above_edge(self.bit_depth)
        }
    }

    fn above_left_predictor(&self, plane: Av2LossyPlane, x0: usize, y0: usize) -> Av2Sample {
        let (tile_origin_x, tile_origin_y) = self.plane_origin(plane);
        let have_left = x0 > tile_origin_x;
        let have_top = y0 > tile_origin_y;
        if have_left && have_top {
            self.recon_sample(plane, x0 - 1, y0 - 1)
        } else if have_top {
            self.recon_sample(plane, x0, y0 - 1)
        } else if have_left {
            self.recon_sample(plane, x0 - 1, y0)
        } else {
            av2_lossless_dc_predictor(self.bit_depth)
        }
    }

    fn analyze_txb(
        &self,
        plane: Av2LossyPlane,
        x0: usize,
        y0: usize,
        mode: Av2LossySubsampledModeDecision,
        context: Av2LossyLeafPredictorContext<'_>,
    ) -> Av2LossyTxbAnalysis {
        let mut source = [0; TX4X4_SAMPLES];
        let mut predictor = [0; TX4X4_SAMPLES];
        let mut residual = [0i32; TX4X4_SAMPLES];
        let mut sum = 0i32;
        let predictor_mode = match plane {
            Av2LossyPlane::Y => chroma_mode_for_luma_mode(mode.luma_intra_mode),
            Av2LossyPlane::U | Av2LossyPlane::V => mode.chroma_intra_mode,
        };
        let dc_pred = if predictor_mode == Av2ChromaIntraMode::Dc {
            self.dc_predictor(plane, x0, y0)
        } else {
            0
        };
        let mut h_pred = [0; TX4X4_SIZE];
        if matches!(
            predictor_mode,
            Av2ChromaIntraMode::Horizontal | Av2ChromaIntraMode::Paeth
        ) {
            for (local_y, pred) in h_pred.iter_mut().enumerate() {
                *pred = self.h_predictor(plane, x0, y0, local_y);
            }
        }
        let mut v_pred = [0; TX4X4_SIZE];
        if matches!(
            predictor_mode,
            Av2ChromaIntraMode::Vertical | Av2ChromaIntraMode::Paeth
        ) {
            for (local_x, pred) in v_pred.iter_mut().enumerate() {
                *pred = self.v_predictor(plane, x0, y0, local_x);
            }
        }
        let above_left = if predictor_mode == Av2ChromaIntraMode::Paeth {
            self.above_left_predictor(plane, x0, y0)
        } else {
            0
        };
        let smooth_edges = matches!(
            predictor_mode,
            Av2ChromaIntraMode::Smooth
                | Av2ChromaIntraMode::SmoothVertical
                | Av2ChromaIntraMode::SmoothHorizontal
        )
        .then(|| self.smooth_edges(plane, x0, y0, context));
        for local_y in 0..TX4X4_SIZE {
            for local_x in 0..TX4X4_SIZE {
                let index = local_y * TX4X4_SIZE + local_x;
                let predictor_sample = match predictor_mode {
                    Av2ChromaIntraMode::Dc => dc_pred,
                    Av2ChromaIntraMode::Horizontal => h_pred[local_y],
                    Av2ChromaIntraMode::Vertical => v_pred[local_x],
                    Av2ChromaIntraMode::Paeth => {
                        paeth_predictor(h_pred[local_y], v_pred[local_x], above_left)
                    }
                    Av2ChromaIntraMode::Smooth
                    | Av2ChromaIntraMode::SmoothVertical
                    | Av2ChromaIntraMode::SmoothHorizontal => {
                        let (above, left) =
                            smooth_edges.expect("smooth edges are precomputed");
                        let (smooth, smooth_v, smooth_h) =
                            av2_highbd_smooth_intra_predictor_set(
                                above,
                                left,
                                local_x,
                                local_y,
                                self.bit_depth,
                            );
                        match predictor_mode {
                            Av2ChromaIntraMode::Smooth => smooth,
                            Av2ChromaIntraMode::SmoothVertical => smooth_v,
                            Av2ChromaIntraMode::SmoothHorizontal => smooth_h,
                            _ => unreachable!("smooth predictor branch only handles smooth modes"),
                        }
                    }
                    _ => unreachable!(
                        "AV2 lossy mode search selects DC, H, V, Paeth, or smooth"
                    ),
                };
                let source_sample = self.source_sample(plane, x0 + local_x, y0 + local_y);
                let diff = i32::from(source_sample) - i32::from(predictor_sample);
                source[index] = source_sample;
                predictor[index] = predictor_sample;
                residual[index] = diff;
                sum += diff;
            }
        }
        let average = round_div_i32(sum, TX4X4_SAMPLES as i32);
        let max_delta = i32::from(self.bit_depth.max_sample());
        let delta = quantize_i32_to_step(average, self.quant_step()).clamp(-max_delta, max_delta)
            as i16;
        let dc_sse = txb_dc_recon_sse(&source, &predictor, delta, self.bit_depth);
        Av2LossyTxbAnalysis {
            source,
            predictor,
            residual,
            delta,
            dc_sse,
        }
    }

    fn fill_quantized_recon_txb(
        &mut self,
        plane: Av2LossyPlane,
        x0: usize,
        y0: usize,
        analysis: &Av2LossyTxbAnalysis,
    ) {
        let (plane_width, plane_height) = self.plane_geometry(plane);
        for local_y in 0..TX4X4_SIZE {
            let y = y0 + local_y;
            if y >= plane_height {
                continue;
            }
            for local_x in 0..TX4X4_SIZE {
                let x = x0 + local_x;
                if x < plane_width {
                    let index = local_y * TX4X4_SIZE + local_x;
                    let predictor = i32::from(analysis.predictor[index]);
                    let sample = (predictor + i32::from(analysis.delta))
                        .clamp(0, i32::from(self.bit_depth.max_sample()))
                        as Av2Sample;
                    self.set_recon_sample(plane, x, y, sample);
                }
            }
        }
    }

    fn fill_residual_recon_txb(
        &mut self,
        plane: Av2LossyPlane,
        x0: usize,
        y0: usize,
        analysis: &Av2LossyTxbAnalysis,
        residual: &[i32; TX4X4_SAMPLES],
    ) {
        let (plane_width, plane_height) = self.plane_geometry(plane);
        for local_y in 0..TX4X4_SIZE {
            let y = y0 + local_y;
            if y >= plane_height {
                continue;
            }
            for local_x in 0..TX4X4_SIZE {
                let x = x0 + local_x;
                if x < plane_width {
                    let index = local_y * TX4X4_SIZE + local_x;
                    let predictor = i32::from(analysis.predictor[index]);
                    let sample = (predictor + residual[index])
                        .clamp(0, i32::from(self.bit_depth.max_sample()))
                        as Av2Sample;
                    self.set_recon_sample(plane, x, y, sample);
                }
            }
        }
    }

    fn copy_source_to_recon_txb(
        &mut self,
        plane: Av2LossyPlane,
        x0: usize,
        y0: usize,
        analysis: &Av2LossyTxbAnalysis,
    ) {
        let (plane_width, plane_height) = self.plane_geometry(plane);
        for local_y in 0..TX4X4_SIZE {
            let y = y0 + local_y;
            if y >= plane_height {
                continue;
            }
            for local_x in 0..TX4X4_SIZE {
                let x = x0 + local_x;
                if x < plane_width {
                    let index = local_y * TX4X4_SIZE + local_x;
                    self.set_recon_sample(plane, x, y, analysis.source[index]);
                }
            }
        }
    }

    fn quantized_residual_candidate(
        &self,
        analysis: &Av2LossyTxbAnalysis,
    ) -> ([i32; TX4X4_SAMPLES], usize) {
        let mut residual = [0i32; TX4X4_SAMPLES];
        let step = self.quant_step();
        let max_sample = i32::from(self.bit_depth.max_sample());
        let mut sse = 0usize;
        for index in 0..TX4X4_SAMPLES {
            let predictor = i32::from(analysis.predictor[index]);
            let source = i32::from(analysis.source[index]);
            let quantized = quantize_i32_to_step(analysis.residual[index], step)
                .clamp(-predictor, max_sample - predictor);
            residual[index] = quantized;
            let recon = (predictor + quantized).clamp(0, max_sample);
            let diff = source - recon;
            sse += (diff * diff) as usize;
        }
        (residual, sse)
    }

    fn mode_decision_for_leaf(
        &self,
        decision: Av2TileDecision,
        visible_rows_mi: usize,
        visible_cols_mi: usize,
        coded_mi_context: &Av2CodedMiContext,
        luma_mode_syntax: Av2LumaModeSyntax,
    ) -> Av2LossySubsampledModeDecision {
        let txb_width = decision
            .block_size
            .tx4x4_width()
            .min(visible_cols_mi.saturating_sub(decision.col));
        let txb_height = decision
            .block_size
            .tx4x4_height()
            .min(visible_rows_mi.saturating_sub(decision.row));
        let (luma_leaf_x0, luma_leaf_y0) =
            self.txb_origin(Av2LossyPlane::Y, decision.col, decision.row);
        let luma_leaf_width = txb_width * TX4X4_SIZE;
        let luma_leaf_height = txb_height * TX4X4_SIZE;
        let luma_context = Av2LossyLeafPredictorContext {
            leaf_x0: luma_leaf_x0,
            leaf_y0: luma_leaf_y0,
            leaf_width: luma_leaf_width,
            leaf_height: luma_leaf_height,
            coded_mi_context,
        };
        let mut mode = Av2LossySubsampledModeDecision::default();
        let mut luma_scores = Av2LossyIntraTxbScores::default();
        let mut luma_sampled_txbs = 0usize;
        for row in 0..txb_height {
            for col in 0..txb_width {
                if !lossy_mode_search_samples_txb(row, col, txb_width, txb_height) {
                    continue;
                }
                let (x0, y0) =
                    self.txb_origin(Av2LossyPlane::Y, decision.col + col, decision.row + row);
                luma_scores.add_assign(self.intra_txb_scores_for_score(
                    Av2LossyPlane::Y,
                    x0,
                    y0,
                    luma_context,
                    Av2CoefficientProxyKind::LumaTransform,
                    true,
                ));
                luma_sampled_txbs += 1;
            }
        }
        luma_scores = luma_scores.scaled_to_txb_count(txb_width * txb_height, luma_sampled_txbs);
        let luma_smooth_scores = lossy_luma_smooth_search_allowed(luma_scores, txb_width * txb_height)
            .then(|| {
                let mut smooth_scores = Av2LossyIntraTxbScores::default();
                let mut sampled_txbs = 0usize;
                for row in 0..txb_height {
                    for col in 0..txb_width {
                        if !lossy_mode_search_samples_txb(row, col, txb_width, txb_height) {
                            continue;
                        }
                        let (x0, y0) = self.txb_origin(
                            Av2LossyPlane::Y,
                            decision.col + col,
                            decision.row + row,
                        );
                        smooth_scores.add_assign(self.smooth_txb_scores_for_score(
                            Av2LossyPlane::Y,
                            x0,
                            y0,
                            luma_context,
                            Av2CoefficientProxyKind::LumaTransform,
                        ));
                        sampled_txbs += 1;
                    }
                }
                smooth_scores.scaled_to_txb_count(txb_width * txb_height, sampled_txbs)
            });
        let mut best_luma = (mode.luma_intra_mode, usize::MAX);
        for luma_intra_mode in [
            Av2LumaIntraMode::Dc,
            Av2LumaIntraMode::Horizontal,
            Av2LumaIntraMode::Vertical,
            Av2LumaIntraMode::Paeth,
        ] {
            let syntax_penalty = lossy_luma_mode_syntax_penalty(luma_intra_mode, luma_mode_syntax);
            let score = match luma_intra_mode {
                Av2LumaIntraMode::Dc => luma_scores.dc,
                Av2LumaIntraMode::Horizontal => luma_scores.horizontal,
                Av2LumaIntraMode::Vertical => luma_scores.vertical,
                Av2LumaIntraMode::Paeth => luma_scores.paeth,
                _ => unreachable!(
                    "AV2 lossy luma mode search scores only DC, H, V, Paeth, and smooth"
                ),
            } + syntax_penalty;
            if score < best_luma.1 {
                best_luma = (luma_intra_mode, score);
            }
        }
        if let Some(smooth_scores) = luma_smooth_scores {
            for (luma_intra_mode, score) in [
                (Av2LumaIntraMode::Smooth, smooth_scores.smooth),
                (
                    Av2LumaIntraMode::SmoothVertical,
                    smooth_scores.smooth_vertical,
                ),
                (
                    Av2LumaIntraMode::SmoothHorizontal,
                    smooth_scores.smooth_horizontal,
                ),
            ] {
                let score = score + 192usize;
                if score < best_luma.1 {
                    best_luma = (luma_intra_mode, score);
                }
            }
        }
        mode.luma_intra_mode = best_luma.0;

        let chroma_span = chroma_tx4x4_span(
            decision,
            visible_rows_mi,
            visible_cols_mi,
            self.chroma_format,
        );
        let (chroma_leaf_x0, chroma_leaf_y0) =
            self.txb_origin(Av2LossyPlane::U, chroma_span.col, chroma_span.row);
        let chroma_leaf_width = chroma_span.width * TX4X4_SIZE;
        let chroma_leaf_height = chroma_span.height * TX4X4_SIZE;
        let mut chroma_scores = Av2LossyIntraTxbScores::default();
        let mut chroma_sampled_txbs = 0usize;
        for plane in [Av2LossyPlane::U, Av2LossyPlane::V] {
            for row in 0..chroma_span.height {
                for col in 0..chroma_span.width {
                    if !lossy_mode_search_samples_txb(
                        row,
                        col,
                        chroma_span.width,
                        chroma_span.height,
                    ) {
                        continue;
                    }
                    let (x0, y0) =
                        self.txb_origin(plane, chroma_span.col + col, chroma_span.row + row);
                    let chroma_context = Av2LossyLeafPredictorContext {
                        leaf_x0: chroma_leaf_x0,
                        leaf_y0: chroma_leaf_y0,
                        leaf_width: chroma_leaf_width,
                        leaf_height: chroma_leaf_height,
                        coded_mi_context,
                    };
                    chroma_scores.add_assign(self.intra_txb_scores_for_score(
                        plane,
                        x0,
                        y0,
                        chroma_context,
                        Av2CoefficientProxyKind::ChromaTransform,
                        false,
                    ));
                    chroma_sampled_txbs += 1;
                }
            }
        }
        chroma_scores = chroma_scores.scaled_to_txb_count(
            chroma_span.width * chroma_span.height * 2,
            chroma_sampled_txbs,
        );
        let mut best_chroma = (mode.chroma_intra_mode, usize::MAX);
        for chroma_intra_mode in [
            Av2ChromaIntraMode::Horizontal,
            Av2ChromaIntraMode::Vertical,
            Av2ChromaIntraMode::Dc,
        ] {
            let syntax_penalty =
                lossy_chroma_mode_syntax_penalty(mode.luma_intra_mode, chroma_intra_mode);
            let score = match chroma_intra_mode {
                Av2ChromaIntraMode::Dc => chroma_scores.dc,
                Av2ChromaIntraMode::Horizontal => chroma_scores.horizontal,
                Av2ChromaIntraMode::Vertical => chroma_scores.vertical,
                _ => unreachable!("AV2 lossy chroma mode search scores only DC, H, and V"),
            } + syntax_penalty;
            if score < best_chroma.1 {
                best_chroma = (chroma_intra_mode, score);
            }
        }
        mode.chroma_intra_mode = best_chroma.0;
        mode
    }

    fn intra_txb_scores_for_score(
        &self,
        plane: Av2LossyPlane,
        x0: usize,
        y0: usize,
        context: Av2LossyLeafPredictorContext<'_>,
        kind: Av2CoefficientProxyKind,
        score_paeth: bool,
    ) -> Av2LossyIntraTxbScores {
        let mut source = [0; TX4X4_SAMPLES];
        let dc = i32::from(self.dc_predictor_for_score(plane, x0, y0, context));
        let mut h_pred = [0; TX4X4_SIZE];
        let mut v_pred = [0; TX4X4_SIZE];
        for index in 0..TX4X4_SIZE {
            h_pred[index] = self.h_predictor_for_score(plane, x0, y0, index, context);
            v_pred[index] = self.v_predictor_for_score(plane, x0, y0, index, context);
        }
        let above_left = if score_paeth {
            self.above_left_predictor_for_score(plane, x0, y0, context)
        } else {
            0
        };
        let mut scores = Av2LossyIntraTxbScores {
            dc: 16,
            horizontal: 16,
            vertical: 16,
            paeth: 16,
            smooth: 0,
            smooth_vertical: 0,
            smooth_horizontal: 0,
        };
        let mut dc_sum = 0i32;
        let mut horizontal_sum = 0i32;
        let mut vertical_sum = 0i32;
        let mut paeth_sum = 0i32;
        let magnitude_scale = residual_sample_proxy_magnitude_scale(kind);
        for local_y in 0..TX4X4_SIZE {
            for local_x in 0..TX4X4_SIZE {
                let index = local_y * TX4X4_SIZE + local_x;
                let sample = i32::from(self.source_sample(plane, x0 + local_x, y0 + local_y));
                source[index] = sample as Av2Sample;
                let horizontal = i32::from(h_pred[local_y]);
                let vertical = i32::from(v_pred[local_x]);
                let dc_diff = sample - dc;
                let horizontal_diff = sample - horizontal;
                let vertical_diff = sample - vertical;
                dc_sum += dc_diff;
                horizontal_sum += horizontal_diff;
                vertical_sum += vertical_diff;
                add_residual_sample_proxy_score(&mut scores.dc, dc_diff, magnitude_scale);
                add_residual_sample_proxy_score(
                    &mut scores.horizontal,
                    horizontal_diff,
                    magnitude_scale,
                );
                add_residual_sample_proxy_score(
                    &mut scores.vertical,
                    vertical_diff,
                    magnitude_scale,
                );
                if score_paeth {
                    let paeth =
                        i32::from(paeth_predictor(h_pred[local_y], v_pred[local_x], above_left));
                    let paeth_diff = sample - paeth;
                    paeth_sum += paeth_diff;
                    add_residual_sample_proxy_score(&mut scores.paeth, paeth_diff, magnitude_scale);
                }
            }
        }
        let max_delta = i32::from(self.bit_depth.max_sample());
        let dc_delta = quantize_i32_to_step(round_div_i32(dc_sum, TX4X4_SAMPLES as i32), self.quant_step())
            .clamp(-max_delta, max_delta);
        let horizontal_delta = quantize_i32_to_step(
            round_div_i32(horizontal_sum, TX4X4_SAMPLES as i32),
            self.quant_step(),
        )
        .clamp(-max_delta, max_delta);
        let vertical_delta = quantize_i32_to_step(
            round_div_i32(vertical_sum, TX4X4_SAMPLES as i32),
            self.quant_step(),
        )
        .clamp(-max_delta, max_delta);
        let paeth_delta = if score_paeth {
            quantize_i32_to_step(
                round_div_i32(paeth_sum, TX4X4_SAMPLES as i32),
                self.quant_step(),
            )
            .clamp(-max_delta, max_delta)
        } else {
            0
        };
        let mut dc_sse = 0usize;
        let mut horizontal_sse = 0usize;
        let mut vertical_sse = 0usize;
        let mut paeth_sse = 0usize;
        let max_sample = i32::from(self.bit_depth.max_sample());
        for local_y in 0..TX4X4_SIZE {
            for local_x in 0..TX4X4_SIZE {
                let index = local_y * TX4X4_SIZE + local_x;
                let source = i32::from(source[index]);
                let dc_recon = (dc + dc_delta).clamp(0, max_sample);
                let horizontal_recon =
                    (i32::from(h_pred[local_y]) + horizontal_delta).clamp(0, max_sample);
                let vertical_recon =
                    (i32::from(v_pred[local_x]) + vertical_delta).clamp(0, max_sample);
                let dc_diff = source - dc_recon;
                let horizontal_diff = source - horizontal_recon;
                let vertical_diff = source - vertical_recon;
                dc_sse += (dc_diff * dc_diff) as usize;
                horizontal_sse += (horizontal_diff * horizontal_diff) as usize;
                vertical_sse += (vertical_diff * vertical_diff) as usize;
                if score_paeth {
                    let paeth =
                        i32::from(paeth_predictor(h_pred[local_y], v_pred[local_x], above_left));
                    let paeth_recon = (paeth + paeth_delta).clamp(0, max_sample);
                    let paeth_diff = source - paeth_recon;
                    paeth_sse += (paeth_diff * paeth_diff) as usize;
                }
            }
        }

        scores.dc = lossy_txb_score(scores.dc, dc_sse, self.quant_step());
        scores.horizontal = lossy_txb_score(scores.horizontal, horizontal_sse, self.quant_step());
        scores.vertical = lossy_txb_score(scores.vertical, vertical_sse, self.quant_step());
        if score_paeth {
            scores.paeth = lossy_txb_score(scores.paeth, paeth_sse, self.quant_step());
        }
        scores
    }

    fn smooth_txb_scores_for_score(
        &self,
        plane: Av2LossyPlane,
        x0: usize,
        y0: usize,
        context: Av2LossyLeafPredictorContext<'_>,
        kind: Av2CoefficientProxyKind,
    ) -> Av2LossyIntraTxbScores {
        let (smooth_above, smooth_left) = self.smooth_edges_for_score(plane, x0, y0, context);
        let mut source = [0; TX4X4_SAMPLES];
        let mut smooth_pred = [0; TX4X4_SAMPLES];
        let mut smooth_vertical_pred = [0; TX4X4_SAMPLES];
        let mut smooth_horizontal_pred = [0; TX4X4_SAMPLES];
        let mut scores = Av2LossyIntraTxbScores {
            dc: 0,
            horizontal: 0,
            vertical: 0,
            paeth: 0,
            smooth: 16,
            smooth_vertical: 16,
            smooth_horizontal: 16,
        };
        let mut smooth_sum = 0i32;
        let mut smooth_vertical_sum = 0i32;
        let mut smooth_horizontal_sum = 0i32;
        let magnitude_scale = residual_sample_proxy_magnitude_scale(kind);
        for local_y in 0..TX4X4_SIZE {
            for local_x in 0..TX4X4_SIZE {
                let index = local_y * TX4X4_SIZE + local_x;
                let sample = i32::from(self.source_sample(plane, x0 + local_x, y0 + local_y));
                source[index] = sample as Av2Sample;
                let (smooth, smooth_v, smooth_h) = av2_highbd_smooth_intra_predictor_set(
                    smooth_above,
                    smooth_left,
                    local_x,
                    local_y,
                    self.bit_depth,
                );
                smooth_pred[index] = smooth;
                smooth_vertical_pred[index] = smooth_v;
                smooth_horizontal_pred[index] = smooth_h;
                let smooth_diff = sample - i32::from(smooth);
                let smooth_vertical_diff = sample - i32::from(smooth_v);
                let smooth_horizontal_diff = sample - i32::from(smooth_h);
                smooth_sum += smooth_diff;
                smooth_vertical_sum += smooth_vertical_diff;
                smooth_horizontal_sum += smooth_horizontal_diff;
                add_residual_sample_proxy_score(&mut scores.smooth, smooth_diff, magnitude_scale);
                add_residual_sample_proxy_score(
                    &mut scores.smooth_vertical,
                    smooth_vertical_diff,
                    magnitude_scale,
                );
                add_residual_sample_proxy_score(
                    &mut scores.smooth_horizontal,
                    smooth_horizontal_diff,
                    magnitude_scale,
                );
            }
        }

        let max_delta = i32::from(self.bit_depth.max_sample());
        let smooth_delta = quantize_i32_to_step(
            round_div_i32(smooth_sum, TX4X4_SAMPLES as i32),
            self.quant_step(),
        )
        .clamp(-max_delta, max_delta);
        let smooth_vertical_delta = quantize_i32_to_step(
            round_div_i32(smooth_vertical_sum, TX4X4_SAMPLES as i32),
            self.quant_step(),
        )
        .clamp(-max_delta, max_delta);
        let smooth_horizontal_delta = quantize_i32_to_step(
            round_div_i32(smooth_horizontal_sum, TX4X4_SAMPLES as i32),
            self.quant_step(),
        )
        .clamp(-max_delta, max_delta);

        let mut smooth_sse = 0usize;
        let mut smooth_vertical_sse = 0usize;
        let mut smooth_horizontal_sse = 0usize;
        let max_sample = i32::from(self.bit_depth.max_sample());
        for index in 0..TX4X4_SAMPLES {
            let source = i32::from(source[index]);
            let smooth_recon = (i32::from(smooth_pred[index]) + smooth_delta).clamp(0, max_sample);
            let smooth_vertical_recon =
                (i32::from(smooth_vertical_pred[index]) + smooth_vertical_delta)
                    .clamp(0, max_sample);
            let smooth_horizontal_recon =
                (i32::from(smooth_horizontal_pred[index]) + smooth_horizontal_delta)
                    .clamp(0, max_sample);
            let smooth_diff = source - smooth_recon;
            let smooth_vertical_diff = source - smooth_vertical_recon;
            let smooth_horizontal_diff = source - smooth_horizontal_recon;
            smooth_sse += (smooth_diff * smooth_diff) as usize;
            smooth_vertical_sse += (smooth_vertical_diff * smooth_vertical_diff) as usize;
            smooth_horizontal_sse += (smooth_horizontal_diff * smooth_horizontal_diff) as usize;
        }

        scores.smooth = lossy_txb_score(scores.smooth, smooth_sse, self.quant_step());
        scores.smooth_vertical =
            lossy_txb_score(scores.smooth_vertical, smooth_vertical_sse, self.quant_step());
        scores.smooth_horizontal = lossy_txb_score(
            scores.smooth_horizontal,
            smooth_horizontal_sse,
            self.quant_step(),
        );
        scores
    }

    fn dc_predictor_for_score(
        &self,
        plane: Av2LossyPlane,
        x0: usize,
        y0: usize,
        context: Av2LossyLeafPredictorContext<'_>,
    ) -> Av2Sample {
        let (tile_origin_x, tile_origin_y) = self.plane_origin(plane);
        let have_left = x0 > tile_origin_x;
        let have_top = y0 > tile_origin_y;
        if !have_left && !have_top {
            return av2_lossless_dc_predictor(self.bit_depth);
        }

        let mut sum = 0u32;
        let mut count = 0u32;
        if have_top {
            for x in x0..(x0 + TX4X4_SIZE) {
                sum += u32::from(self.neighbor_sample_for_score(plane, x, y0 - 1, context));
                count += 1;
            }
        }
        if have_left {
            for y in y0..(y0 + TX4X4_SIZE) {
                sum += u32::from(self.neighbor_sample_for_score(plane, x0 - 1, y, context));
                count += 1;
            }
        }
        ((sum + count / 2) / count) as Av2Sample
    }

    fn h_predictor_for_score(
        &self,
        plane: Av2LossyPlane,
        x0: usize,
        y0: usize,
        local_y: usize,
        context: Av2LossyLeafPredictorContext<'_>,
    ) -> Av2Sample {
        let (tile_origin_x, tile_origin_y) = self.plane_origin(plane);
        if x0 > tile_origin_x {
            self.neighbor_sample_for_score(plane, x0 - 1, y0 + local_y, context)
        } else if y0 > tile_origin_y {
            self.neighbor_sample_for_score(plane, x0, y0 - 1, context)
        } else {
            av2_lossless_h_pred_left_edge(self.bit_depth)
        }
    }

    fn v_predictor_for_score(
        &self,
        plane: Av2LossyPlane,
        x0: usize,
        y0: usize,
        local_x: usize,
        context: Av2LossyLeafPredictorContext<'_>,
    ) -> Av2Sample {
        let (tile_origin_x, tile_origin_y) = self.plane_origin(plane);
        if y0 > tile_origin_y {
            self.neighbor_sample_for_score(plane, x0 + local_x, y0 - 1, context)
        } else if x0 > tile_origin_x {
            self.neighbor_sample_for_score(plane, x0 - 1, y0, context)
        } else {
            av2_lossless_v_pred_above_edge(self.bit_depth)
        }
    }

    fn above_left_predictor_for_score(
        &self,
        plane: Av2LossyPlane,
        x0: usize,
        y0: usize,
        context: Av2LossyLeafPredictorContext<'_>,
    ) -> Av2Sample {
        let (tile_origin_x, tile_origin_y) = self.plane_origin(plane);
        let have_left = x0 > tile_origin_x;
        let have_top = y0 > tile_origin_y;
        if have_left && have_top {
            self.neighbor_sample_for_score(plane, x0 - 1, y0 - 1, context)
        } else if have_top {
            self.neighbor_sample_for_score(plane, x0, y0 - 1, context)
        } else if have_left {
            self.neighbor_sample_for_score(plane, x0 - 1, y0, context)
        } else {
            av2_lossless_dc_predictor(self.bit_depth)
        }
    }

    fn smooth_edges(
        &self,
        plane: Av2LossyPlane,
        x0: usize,
        y0: usize,
        context: Av2LossyLeafPredictorContext<'_>,
    ) -> ([Av2Sample; TX4X4_SIZE + 1], [Av2Sample; TX4X4_SIZE + 1]) {
        let edge_sample = |plane, x, y| self.recon_sample(plane, x, y);
        self.smooth_edges_with(plane, x0, y0, context, &edge_sample)
    }

    fn smooth_edges_for_score(
        &self,
        plane: Av2LossyPlane,
        x0: usize,
        y0: usize,
        context: Av2LossyLeafPredictorContext<'_>,
    ) -> ([Av2Sample; TX4X4_SIZE + 1], [Av2Sample; TX4X4_SIZE + 1]) {
        let edge_sample = |plane, x, y| self.neighbor_sample_for_score(plane, x, y, context);
        self.smooth_edges_with(plane, x0, y0, context, &edge_sample)
    }

    fn smooth_edges_with<EdgeSample>(
        &self,
        plane: Av2LossyPlane,
        x0: usize,
        y0: usize,
        context: Av2LossyLeafPredictorContext<'_>,
        edge_sample: &EdgeSample,
    ) -> ([Av2Sample; TX4X4_SIZE + 1], [Av2Sample; TX4X4_SIZE + 1])
    where
        EdgeSample: Fn(Av2LossyPlane, usize, usize) -> Av2Sample,
    {
        let (tile_origin_x, tile_origin_y) = self.plane_origin(plane);
        let (plane_width, plane_height) = self.plane_geometry(plane);
        let (sub_x, sub_y) = self.plane_subsampling(plane);
        let have_top = y0 > tile_origin_y;
        let have_left = x0 > tile_origin_x;
        let mut above = [av2_lossless_v_pred_above_edge(self.bit_depth); TX4X4_SIZE + 1];
        let mut left = [av2_lossless_h_pred_left_edge(self.bit_depth); TX4X4_SIZE + 1];

        if have_top {
            for local_x in 0..TX4X4_SIZE {
                above[local_x] = edge_sample(plane, x0 + local_x, y0 - 1);
            }
        } else if have_left {
            above[..TX4X4_SIZE].fill(edge_sample(plane, x0 - 1, y0));
        }

        if have_left {
            for local_y in 0..TX4X4_SIZE {
                left[local_y] = edge_sample(plane, x0 - 1, y0 + local_y);
            }
        } else if have_top {
            left[..TX4X4_SIZE].fill(edge_sample(plane, x0, y0 - 1));
        }

        let plane_sb_width = MVP_SUPERBLOCK_SIZE / sub_x;
        let plane_sb_height = MVP_SUPERBLOCK_SIZE / sub_y;
        let sb_origin_x = (x0 / plane_sb_width) * plane_sb_width;
        let sb_right = (sb_origin_x + plane_sb_width).min(plane_width);
        let top_right_x = x0 + TX4X4_SIZE;
        let superblock_top_row = y0 % plane_sb_height == 0;
        let external_top_right_coded =
            have_top && y0 == context.leaf_y0 && top_right_x < plane_width && {
                let (row_mi, col_mi) = self.coded_mi_for_plane_sample(plane, top_right_x, y0 - 1);
                superblock_top_row
                    || (top_right_x < sb_right && context.coded_mi_context.is_coded(row_mi, col_mi))
            };
        if have_top
            && top_right_x < plane_width
            && (top_right_x < context.leaf_x0 + context.leaf_width || external_top_right_coded)
        {
            above[TX4X4_SIZE] = edge_sample(plane, top_right_x, y0 - 1);
        } else {
            above[TX4X4_SIZE] = above[TX4X4_SIZE - 1];
        }

        let sb_origin_y = (y0 / plane_sb_height) * plane_sb_height;
        let sb_bottom = (sb_origin_y + plane_sb_height).min(plane_height);
        let bottom_left_y = y0 + TX4X4_SIZE;
        let superblock_left_col = x0 % plane_sb_width == 0;
        let external_bottom_left_coded =
            have_left && x0 == context.leaf_x0 && bottom_left_y < sb_bottom && {
                let (row_mi, col_mi) =
                    self.coded_mi_for_plane_sample(plane, x0 - 1, bottom_left_y);
                superblock_left_col || context.coded_mi_context.is_coded(row_mi, col_mi)
            };
        if have_left
            && x0 == context.leaf_x0
            && bottom_left_y < plane_height
            && (bottom_left_y < context.leaf_y0 + context.leaf_height
                || external_bottom_left_coded)
        {
            left[TX4X4_SIZE] = edge_sample(plane, x0 - 1, bottom_left_y);
        } else {
            left[TX4X4_SIZE] = left[TX4X4_SIZE - 1];
        }

        (above, left)
    }

    fn neighbor_sample_for_score(
        &self,
        plane: Av2LossyPlane,
        x: usize,
        y: usize,
        context: Av2LossyLeafPredictorContext<'_>,
    ) -> Av2Sample {
        if x >= context.leaf_x0
            && x < context.leaf_x0 + context.leaf_width
            && y >= context.leaf_y0
            && y < context.leaf_y0 + context.leaf_height
        {
            self.source_sample(plane, x, y)
        } else {
            self.recon_sample(plane, x, y)
        }
    }

    fn quant_step(&self) -> i32 {
        i32::from(self.qp) << u32::from(self.bit_depth.bits() - 8)
    }
}

fn txb_dc_recon_sse(
    source: &[Av2Sample; TX4X4_SAMPLES],
    predictor: &[Av2Sample; TX4X4_SAMPLES],
    delta: i16,
    bit_depth: SampleBitDepth,
) -> usize {
    let mut sse = 0usize;
    let max_sample = i32::from(bit_depth.max_sample());
    for index in 0..TX4X4_SAMPLES {
        let recon = (i32::from(predictor[index]) + i32::from(delta)).clamp(0, max_sample);
        let diff = i32::from(source[index]) - recon;
        sse += (diff * diff) as usize;
    }
    sse
}

#[inline(always)]
fn read_lossy_planar_sample(
    input: &[u8],
    sample_index: usize,
    bit_depth: SampleBitDepth,
) -> Av2Sample {
    let offset = sample_index * bit_depth.bytes_per_sample();
    if bit_depth.bits() <= 8 {
        u16::from(input[offset])
    } else {
        u16::from_le_bytes([input[offset], input[offset + 1]])
    }
}

#[inline(always)]
fn write_lossy_planar_sample(
    output: &mut [u8],
    sample_index: usize,
    sample: Av2Sample,
    bit_depth: SampleBitDepth,
) {
    let offset = sample_index * bit_depth.bytes_per_sample();
    if bit_depth.bits() <= 8 {
        output[offset] = sample as u8;
    } else {
        let bytes = sample.to_le_bytes();
        output[offset] = bytes[0];
        output[offset + 1] = bytes[1];
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Av2LossyPlane {
    Y,
    U,
    V,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Av2LossySubsampledModeDecision {
    luma_intra_mode: Av2LumaIntraMode,
    chroma_intra_mode: Av2ChromaIntraMode,
}

impl Default for Av2LossySubsampledModeDecision {
    fn default() -> Self {
        Self {
            luma_intra_mode: Av2LumaIntraMode::Dc,
            chroma_intra_mode: Av2ChromaIntraMode::Horizontal,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct Av2LossyIntraTxbScores {
    dc: usize,
    horizontal: usize,
    vertical: usize,
    paeth: usize,
    smooth: usize,
    smooth_vertical: usize,
    smooth_horizontal: usize,
}

impl Av2LossyIntraTxbScores {
    fn add_assign(&mut self, other: Self) {
        self.dc += other.dc;
        self.horizontal += other.horizontal;
        self.vertical += other.vertical;
        self.paeth += other.paeth;
        self.smooth += other.smooth;
        self.smooth_vertical += other.smooth_vertical;
        self.smooth_horizontal += other.smooth_horizontal;
    }

    fn scaled_to_txb_count(self, total_txbs: usize, sampled_txbs: usize) -> Self {
        if sampled_txbs == 0 || sampled_txbs == total_txbs {
            return self;
        }
        Self {
            dc: self.dc.saturating_mul(total_txbs) / sampled_txbs,
            horizontal: self.horizontal.saturating_mul(total_txbs) / sampled_txbs,
            vertical: self.vertical.saturating_mul(total_txbs) / sampled_txbs,
            paeth: self.paeth.saturating_mul(total_txbs) / sampled_txbs,
            smooth: self.smooth.saturating_mul(total_txbs) / sampled_txbs,
            smooth_vertical: self.smooth_vertical.saturating_mul(total_txbs) / sampled_txbs,
            smooth_horizontal: self.smooth_horizontal.saturating_mul(total_txbs) / sampled_txbs,
        }
    }
}

fn lossy_mode_search_samples_txb(row: usize, col: usize, width: usize, height: usize) -> bool {
    const FULL_SEARCH_TXB_LIMIT: usize = 64;
    if width * height <= FULL_SEARCH_TXB_LIMIT {
        return true;
    }
    (row % 2 == 0 && col % 2 == 0) || row + 1 == height || col + 1 == width
}

fn lossy_luma_smooth_search_allowed(scores: Av2LossyIntraTxbScores, total_txbs: usize) -> bool {
    let txb_count = total_txbs.max(1);
    let best_directional = scores
        .horizontal
        .min(scores.vertical)
        .min(scores.paeth);
    let best_basic = scores.dc.min(best_directional);
    let per_txb_residual = best_basic / txb_count;
    if per_txb_residual < 1024 {
        return false;
    }

    // Smooth predictors are most useful on low-frequency ramps where DC is
    // still competitive with the axis/Paeth modes. Hard screen edges usually
    // make H/V/Paeth dominate DC; skip those leaves and keep the search budget
    // for gradient-like regions.
    let directional_advantage = scores.dc.saturating_sub(best_directional);
    let max_directional_advantage = (scores.dc / 4).max(txb_count * 64);
    directional_advantage <= max_directional_advantage
}

fn lossy_luma_mode_syntax_penalty(
    mode: Av2LumaIntraMode,
    syntax: Av2LumaModeSyntax,
) -> usize {
    match mode {
        Av2LumaIntraMode::Dc => 0,
        Av2LumaIntraMode::Horizontal | Av2LumaIntraMode::Vertical => {
            let index = usize::from(syntax.index_for(mode));
            32 + index.saturating_sub(6) * 128
        }
        Av2LumaIntraMode::Paeth => 128,
        _ => unreachable!("AV2 lossy luma syntax penalty handles scored modes"),
    }
}

fn lossy_chroma_mode_syntax_penalty(
    luma_mode: Av2LumaIntraMode,
    chroma_mode: Av2ChromaIntraMode,
) -> usize {
    let index = chroma_uv_mode_index(luma_mode, chroma_mode);
    index.min(7) * 32 + usize::from(index >= 7) * 64
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Av2LossyTxbChoice {
    DcDelta(i16),
    QuantizedResidual([i32; TX4X4_SAMPLES]),
    Exact,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Av2LossyTxbAnalysis {
    source: [Av2Sample; TX4X4_SAMPLES],
    predictor: [Av2Sample; TX4X4_SAMPLES],
    residual: [i32; TX4X4_SAMPLES],
    delta: i16,
    dc_sse: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Av2LosslessSubsampledModeDecision {
    luma_intra_mode: Av2LumaIntraMode,
    luma_bdpcm_horz: Option<bool>,
    chroma_use_bdpcm: bool,
    chroma_intra_mode: Av2ChromaIntraMode,
    use_luma_palette: bool,
    use_fsc: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Av2LosslessSubsampledModeSearch {
    Exhaustive,
    FastScreenContent,
}

impl Default for Av2LosslessSubsampledModeDecision {
    fn default() -> Self {
        Self {
            luma_intra_mode: Av2LumaIntraMode::Dc,
            luma_bdpcm_horz: None,
            chroma_use_bdpcm: false,
            chroma_intra_mode: Av2ChromaIntraMode::Horizontal,
            use_luma_palette: false,
            use_fsc: false,
        }
    }
}

impl Av2LosslessSubsampledModeDecision {
    fn coded_luma_mode(self) -> Av2LumaIntraMode {
        match self.luma_bdpcm_horz {
            Some(true) => Av2LumaIntraMode::Horizontal,
            Some(false) => Av2LumaIntraMode::Vertical,
            None => self.luma_intra_mode,
        }
    }
}

fn chroma_mode_for_luma_mode(mode: Av2LumaIntraMode) -> Av2ChromaIntraMode {
    match mode {
        Av2LumaIntraMode::Dc => Av2ChromaIntraMode::Dc,
        Av2LumaIntraMode::Smooth => Av2ChromaIntraMode::Smooth,
        Av2LumaIntraMode::SmoothVertical => Av2ChromaIntraMode::SmoothVertical,
        Av2LumaIntraMode::SmoothHorizontal => Av2ChromaIntraMode::SmoothHorizontal,
        Av2LumaIntraMode::Paeth => Av2ChromaIntraMode::Paeth,
        Av2LumaIntraMode::Directional45 => Av2ChromaIntraMode::Directional45,
        Av2LumaIntraMode::Directional67 => Av2ChromaIntraMode::Directional67,
        Av2LumaIntraMode::Vertical => Av2ChromaIntraMode::Vertical,
        Av2LumaIntraMode::Directional113 => Av2ChromaIntraMode::Directional113,
        Av2LumaIntraMode::Directional135 => Av2ChromaIntraMode::Directional135,
        Av2LumaIntraMode::Directional157 => Av2ChromaIntraMode::Directional157,
        Av2LumaIntraMode::Horizontal => Av2ChromaIntraMode::Horizontal,
        Av2LumaIntraMode::Directional203 => Av2ChromaIntraMode::Directional203,
        Av2LumaIntraMode::DirectionalDelta { base, .. } => base.chroma_mode(),
    }
}
