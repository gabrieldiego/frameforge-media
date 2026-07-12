#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Av2LumaPaletteBlock444 {
    colors: Vec<Av2Sample>,
    indices: [u8; AV2_LUMA_PALETTE_BLOCK_SAMPLES],
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Av2LumaPalette444 {
    blocks: Vec<Av2LumaPaletteBlock444>,
    luma_modes: Vec<Av2LumaIntraMode>,
    luma_bdpcm_horz: Vec<Option<bool>>,
    chroma_use_bdpcm: Vec<bool>,
    chroma_intra_modes: Vec<Av2ChromaIntraMode>,
    bit_depth: SampleBitDepth,
    y_plane: Vec<Av2Sample>,
    luma_prediction: Vec<Av2Sample>,
    u_plane: Vec<Av2Sample>,
    v_plane: Vec<Av2Sample>,
    reconstruction: Vec<u8>,
    width: usize,
    height: usize,
    blocks_wide: usize,
    blocks_high: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Av2LumaPaletteRegion {
    colors: Vec<Av2Sample>,
    x0: usize,
    y0: usize,
    width: usize,
    height: usize,
    indices: Vec<u8>,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct Av2ChromaModeTxbScores {
    bdpcm_horz: usize,
    bdpcm_vert: usize,
    intra_dc: usize,
    intra_horz: usize,
    intra_vert: usize,
    intra_smooth: usize,
    intra_smooth_v: usize,
    intra_smooth_h: usize,
    intra_paeth: usize,
}

fn chroma_sample_prediction_score(sample: Av2Sample, predictor: Av2Sample) -> usize {
    chroma_idtx_sample_score(i32::from(sample) - i32::from(predictor))
}

impl Av2LumaPaletteRegion {
    fn from_block(x0: usize, y0: usize, block: &Av2LumaPaletteBlock444) -> Self {
        Self {
            colors: block.colors.clone(),
            x0,
            y0,
            width: AV2_LUMA_PALETTE_BLOCK_SIZE,
            height: AV2_LUMA_PALETTE_BLOCK_SIZE,
            indices: block.indices.to_vec(),
        }
    }

    pub(crate) fn colors(&self) -> &[Av2Sample] {
        &self.colors
    }

    pub(crate) fn color_count(&self) -> usize {
        self.colors.len()
    }

    pub(crate) fn index_at(&self, x: usize, y: usize) -> u8 {
        debug_assert!(x >= self.x0 && x < self.x0 + self.width);
        debug_assert!(y >= self.y0 && y < self.y0 + self.height);
        self.indices[(y - self.y0) * self.width + (x - self.x0)]
    }

    pub(crate) fn prediction_at(&self, x: usize, y: usize) -> Av2Sample {
        self.colors[usize::from(self.index_at(x, y))]
    }
}

impl Av2LumaPalette444 {
    pub(crate) fn bit_depth(&self) -> SampleBitDepth {
        self.bit_depth
    }

    fn quantized_region_palette(
        &self,
        x0: usize,
        y0: usize,
        width: usize,
        height: usize,
    ) -> Av2LumaPaletteRegion {
        assert!(x0 + width <= self.width && y0 + height <= self.height);
        let mut colors = Vec::with_capacity(AV2_LUMA_PALETTE_MAX_COLORS);
        let value_count = usize::from(self.bit_depth.max_sample()) + 1;
        let mut counts = vec![0usize; value_count];
        let mut first_positions = vec![usize::MAX; value_count];
        let mut sample_index = 0usize;
        for y in y0..y0 + height {
            for x in x0..x0 + width {
                let sample = self.y_sample(x, y);
                let sample_index_by_value = usize::from(sample);
                counts[sample_index_by_value] += 1;
                first_positions[sample_index_by_value] =
                    first_positions[sample_index_by_value].min(sample_index);
                if !colors.contains(&sample) && colors.len() < AV2_LUMA_PALETTE_MAX_COLORS {
                    colors.push(sample);
                }
                sample_index += 1;
            }
        }
        if colors.is_empty() {
            colors.push(0);
        }

        let unique_colors = counts.iter().filter(|&&count| count != 0).count();
        let target_colors = unique_colors
            .clamp(AV2_LUMA_PALETTE_MIN_COLORS, AV2_LUMA_PALETTE_MAX_COLORS)
            .min(AV2_LUMA_PALETTE_SOFT_MAX_COLORS);

        let mut colors = if unique_colors > target_colors {
            quantized_luma_palette_values(&counts, &first_positions, target_colors)
        } else {
            colors
        };
        while colors.len() < target_colors {
            let filler = (0..=self.bit_depth.max_sample())
                .find(|sample| !colors.contains(sample))
                .expect("AV2 bit depth range must contain at least two samples");
            colors.push(filler);
        }
        colors.sort_unstable();

        let mut indices = Vec::with_capacity(width * height);
        for y in y0..y0 + height {
            for x in x0..x0 + width {
                indices.push(palette_index_for_sample(&colors, self.y_sample(x, y)));
            }
        }
        Av2LumaPaletteRegion {
            colors,
            x0,
            y0,
            width,
            height,
            indices,
        }
    }

    pub(crate) fn syntax_region_palette(
        &self,
        x0: usize,
        y0: usize,
        width: usize,
        height: usize,
    ) -> Av2LumaPaletteRegion {
        if width == AV2_LUMA_PALETTE_BLOCK_SIZE
            && height == AV2_LUMA_PALETTE_BLOCK_SIZE
            && !self.blocks.is_empty()
        {
            Av2LumaPaletteRegion::from_block(x0, y0, self.block_for_origin(x0, y0))
        } else {
            self.quantized_region_palette(x0, y0, width, height)
        }
    }

    pub(crate) fn luma_mode_for_block(&self, x0: usize, y0: usize) -> Av2LumaIntraMode {
        self.luma_modes[self.block_index_for_origin(x0, y0)]
    }

    pub(crate) fn luma_bdpcm_horz_for_block(&self, x0: usize, y0: usize) -> Option<bool> {
        self.luma_bdpcm_horz[self.block_index_for_origin(x0, y0)]
    }

    pub(crate) fn chroma_intra_mode_for_block(&self, x0: usize, y0: usize) -> Av2ChromaIntraMode {
        self.chroma_intra_modes[self.block_index_for_origin(x0, y0)]
    }

    pub(crate) fn chroma_use_bdpcm_for_block(&self, x0: usize, y0: usize) -> bool {
        self.chroma_use_bdpcm[self.block_index_for_origin(x0, y0)]
    }

    fn chroma_mode_decision_for_block(&self, x0: usize, y0: usize) -> (bool, Av2ChromaIntraMode) {
        let mut bdpcm_horz_score = 0usize;
        let mut bdpcm_vert_score = 0usize;
        let mut intra_dc_score = 0usize;
        let mut intra_horz_score = 0usize;
        let mut intra_vert_score = 0usize;
        let mut intra_d45_score = 0usize;
        let mut intra_d67_score = 0usize;
        let mut intra_d135_score = 0usize;
        let mut intra_d113_score = 0usize;
        let mut intra_d157_score = 0usize;
        let mut intra_d203_score = 0usize;
        let mut intra_smooth_score = 0usize;
        let mut intra_smooth_v_score = 0usize;
        let mut intra_smooth_h_score = 0usize;
        let mut intra_paeth_score = 0usize;
        // The local coefficient proxy currently over-selects directional
        // chroma for screenshot content. Keep the search to basic, smooth, and
        // Paeth families until the mode cost model includes syntax context.
        let directional_allowed = false;
        for plane in [&self.u_plane, &self.v_plane] {
            for txb_y in (0..AV2_LUMA_PALETTE_BLOCK_SIZE).step_by(4) {
                for txb_x in (0..AV2_LUMA_PALETTE_BLOCK_SIZE).step_by(4) {
                    let txb_x0 = x0 + txb_x;
                    let txb_y0 = y0 + txb_y;
                    let scores = self.chroma_mode_scores_for_txb(
                        plane,
                        txb_x0,
                        txb_y0,
                        x0,
                        y0,
                        AV2_LUMA_PALETTE_BLOCK_SIZE,
                        AV2_LUMA_PALETTE_BLOCK_SIZE,
                    );
                    let intra_d45_residual = directional_allowed.then(|| {
                        self.chroma_intra_residuals(
                            plane,
                            txb_x0,
                            txb_y0,
                            x0,
                            y0,
                            AV2_LUMA_PALETTE_BLOCK_SIZE,
                            AV2_LUMA_PALETTE_BLOCK_SIZE,
                            Av2ChromaIntraMode::Directional45,
                        )
                    });
                    let intra_d135_residual = directional_allowed.then(|| {
                        self.chroma_intra_residuals(
                            plane,
                            txb_x0,
                            txb_y0,
                            x0,
                            y0,
                            AV2_LUMA_PALETTE_BLOCK_SIZE,
                            AV2_LUMA_PALETTE_BLOCK_SIZE,
                            Av2ChromaIntraMode::Directional135,
                        )
                    });
                    let intra_d67_residual = directional_allowed.then(|| {
                        self.chroma_intra_residuals(
                            plane,
                            txb_x0,
                            txb_y0,
                            x0,
                            y0,
                            AV2_LUMA_PALETTE_BLOCK_SIZE,
                            AV2_LUMA_PALETTE_BLOCK_SIZE,
                            Av2ChromaIntraMode::Directional67,
                        )
                    });
                    let intra_d113_residual = directional_allowed.then(|| {
                        self.chroma_intra_residuals(
                            plane,
                            txb_x0,
                            txb_y0,
                            x0,
                            y0,
                            AV2_LUMA_PALETTE_BLOCK_SIZE,
                            AV2_LUMA_PALETTE_BLOCK_SIZE,
                            Av2ChromaIntraMode::Directional113,
                        )
                    });
                    let intra_d157_residual = directional_allowed.then(|| {
                        self.chroma_intra_residuals(
                            plane,
                            txb_x0,
                            txb_y0,
                            x0,
                            y0,
                            AV2_LUMA_PALETTE_BLOCK_SIZE,
                            AV2_LUMA_PALETTE_BLOCK_SIZE,
                            Av2ChromaIntraMode::Directional157,
                        )
                    });
                    let intra_d203_residual = directional_allowed.then(|| {
                        self.chroma_intra_residuals(
                            plane,
                            txb_x0,
                            txb_y0,
                            x0,
                            y0,
                            AV2_LUMA_PALETTE_BLOCK_SIZE,
                            AV2_LUMA_PALETTE_BLOCK_SIZE,
                            Av2ChromaIntraMode::Directional203,
                        )
                    });
                    bdpcm_horz_score += scores.bdpcm_horz;
                    bdpcm_vert_score += scores.bdpcm_vert;
                    intra_dc_score += scores.intra_dc;
                    intra_horz_score += scores.intra_horz;
                    intra_vert_score += scores.intra_vert;
                    if let Some(residual) = intra_d45_residual {
                        intra_d45_score += chroma_idtx_coeff_score(&residual);
                    }
                    if let Some(residual) = intra_d67_residual {
                        intra_d67_score += chroma_idtx_coeff_score(&residual);
                    }
                    if let Some(residual) = intra_d135_residual {
                        intra_d135_score += chroma_idtx_coeff_score(&residual);
                    }
                    if let Some(residual) = intra_d113_residual {
                        intra_d113_score += chroma_idtx_coeff_score(&residual);
                    }
                    if let Some(residual) = intra_d157_residual {
                        intra_d157_score += chroma_idtx_coeff_score(&residual);
                    }
                    if let Some(residual) = intra_d203_residual {
                        intra_d203_score += chroma_idtx_coeff_score(&residual);
                    }
                    intra_smooth_score += scores.intra_smooth;
                    intra_smooth_v_score += scores.intra_smooth_v;
                    intra_smooth_h_score += scores.intra_smooth_h;
                    intra_paeth_score += scores.intra_paeth;
                }
            }
        }
        let candidates = [
            (true, Av2ChromaIntraMode::Horizontal, bdpcm_horz_score),
            (true, Av2ChromaIntraMode::Vertical, bdpcm_vert_score),
            (false, Av2ChromaIntraMode::Dc, intra_dc_score),
            (false, Av2ChromaIntraMode::Horizontal, intra_horz_score),
            (false, Av2ChromaIntraMode::Vertical, intra_vert_score),
            (
                false,
                Av2ChromaIntraMode::Directional45,
                if directional_allowed {
                    intra_d45_score
                } else {
                    usize::MAX
                },
            ),
            (
                false,
                Av2ChromaIntraMode::Directional135,
                if directional_allowed {
                    intra_d135_score
                } else {
                    usize::MAX
                },
            ),
            (
                false,
                Av2ChromaIntraMode::Directional67,
                if directional_allowed {
                    intra_d67_score
                } else {
                    usize::MAX
                },
            ),
            (
                false,
                Av2ChromaIntraMode::Directional203,
                if directional_allowed {
                    intra_d203_score
                } else {
                    usize::MAX
                },
            ),
            (
                false,
                Av2ChromaIntraMode::Directional113,
                if directional_allowed {
                    intra_d113_score
                } else {
                    usize::MAX
                },
            ),
            (
                false,
                Av2ChromaIntraMode::Directional157,
                if directional_allowed {
                    intra_d157_score
                } else {
                    usize::MAX
                },
            ),
            (false, Av2ChromaIntraMode::Smooth, intra_smooth_score),
            (
                false,
                Av2ChromaIntraMode::SmoothVertical,
                intra_smooth_v_score,
            ),
            (
                false,
                Av2ChromaIntraMode::SmoothHorizontal,
                intra_smooth_h_score,
            ),
            (false, Av2ChromaIntraMode::Paeth, intra_paeth_score),
        ];
        let &(use_bdpcm, mode, _) = candidates
            .iter()
            .min_by_key(|(_, _, score)| *score)
            .expect("AV2 chroma mode scorer has fixed candidates");
        // AV2 v1.0.0 read_intra_uv_mode() permits normal DC/H/V/Paeth chroma
        // prediction and, in lossless blocks, H/V DPCM. Lossless CfL is only
        // legal for 4x4 chroma blocks; this MVP palette path codes 8x8 leaves.
        // Large screen-content crops frequently have chroma fills and flat runs
        // where a block-local family choice is much cheaper than always using
        // DPCM.
        (use_bdpcm, mode)
    }

    fn chroma_mode_scores_for_txb(
        &self,
        plane: &[Av2Sample],
        txb_x0: usize,
        txb_y0: usize,
        leaf_x0: usize,
        leaf_y0: usize,
        leaf_width: usize,
        leaf_height: usize,
    ) -> Av2ChromaModeTxbScores {
        let tile_x0 = 0;
        let tile_y0 = 0;
        let dc = self.chroma_dc_predictor(plane, txb_x0, txb_y0);
        let above_left = {
            let have_left = txb_x0 != tile_x0;
            let have_top = txb_y0 != tile_y0;
            if have_left && have_top {
                self.chroma_sample(plane, txb_x0 - 1, txb_y0 - 1)
            } else if have_top {
                self.chroma_sample(plane, txb_x0, txb_y0 - 1)
            } else if have_left {
                self.chroma_sample(plane, txb_x0 - 1, txb_y0)
            } else {
                self.dc_predictor()
            }
        };
        let (smooth_above, smooth_left) =
            self.chroma_smooth_edges(plane, txb_x0, txb_y0, leaf_x0, leaf_y0, leaf_width, leaf_height);
        let mut h_edge = [self.h_pred_left_edge(); 4];
        let mut v_edge = [self.v_pred_above_edge(); 4];
        for local_y in 0..4 {
            h_edge[local_y] = if txb_x0 != tile_x0 {
                self.chroma_sample(plane, txb_x0 - 1, txb_y0 + local_y)
            } else if txb_y0 != tile_y0 {
                self.chroma_sample(plane, txb_x0, txb_y0 - 1)
            } else {
                self.h_pred_left_edge()
            };
        }
        for local_x in 0..4 {
            v_edge[local_x] = if txb_y0 != tile_y0 {
                self.chroma_sample(plane, txb_x0 + local_x, txb_y0 - 1)
            } else if txb_x0 != tile_x0 {
                self.chroma_sample(plane, txb_x0 - 1, txb_y0)
            } else {
                self.v_pred_above_edge()
            };
        }

        let mut scores = Av2ChromaModeTxbScores::default();
        for local_y in 0..4 {
            let y = txb_y0 + local_y;
            for local_x in 0..4 {
                let x = txb_x0 + local_x;
                let sample = self.chroma_sample(plane, x, y);
                let bdpcm_horz = if local_x != 0 {
                    self.chroma_sample(plane, x - 1, y)
                } else if txb_x0 != tile_x0 {
                    self.chroma_sample(plane, txb_x0 - 1, y)
                } else if txb_y0 != tile_y0 {
                    self.chroma_sample(plane, txb_x0, txb_y0 - 1)
                } else {
                    self.h_pred_left_edge()
                };
                let bdpcm_vert = if local_y != 0 {
                    self.chroma_sample(plane, x, y - 1)
                } else if txb_y0 != tile_y0 {
                    self.chroma_sample(plane, x, txb_y0 - 1)
                } else if txb_x0 != tile_x0 {
                    self.chroma_sample(plane, txb_x0 - 1, txb_y0)
                } else {
                    self.v_pred_above_edge()
                };
                scores.bdpcm_horz += chroma_sample_prediction_score(sample, bdpcm_horz);
                scores.bdpcm_vert += chroma_sample_prediction_score(sample, bdpcm_vert);
                scores.intra_dc += chroma_sample_prediction_score(sample, dc);
                scores.intra_horz += chroma_sample_prediction_score(sample, h_edge[local_y]);
                scores.intra_vert += chroma_sample_prediction_score(sample, v_edge[local_x]);
                let (smooth, smooth_v, smooth_h) = av2_highbd_smooth_intra_predictor_set(
                    smooth_above,
                    smooth_left,
                    local_x,
                    local_y,
                    self.bit_depth,
                );
                scores.intra_smooth += chroma_sample_prediction_score(sample, smooth);
                scores.intra_smooth_v += chroma_sample_prediction_score(sample, smooth_v);
                scores.intra_smooth_h += chroma_sample_prediction_score(sample, smooth_h);
                scores.intra_paeth += chroma_sample_prediction_score(
                    sample,
                    paeth_predictor(h_edge[local_y], v_edge[local_x], above_left),
                );
            }
        }
        scores
    }

    fn luma_bdpcm_horz_decision_for_block(&self, x0: usize, y0: usize) -> Option<bool> {
        let palette_score = self.luma_palette_coeff_score_for_block(x0, y0);
        let vert_score = self.luma_bdpcm_coeff_score_for_block(x0, y0, false);
        let horz_score = self.luma_bdpcm_coeff_score_for_block(x0, y0, true);
        let (horz, dpcm_score) = if horz_score < vert_score {
            (true, horz_score)
        } else {
            (false, vert_score)
        };

        (dpcm_score + AV2_LUMA_DPCM_SCORE_MARGIN
            < palette_score + AV2_LUMA_DPCM_PALETTE_SYNTAX_BONUS)
            .then_some(horz)
    }

    fn luma_palette_coeff_score_for_block(&self, x0: usize, y0: usize) -> usize {
        let mut score = 0usize;
        for txb_y in 0..2 {
            for txb_x in 0..2 {
                let residual = self.luma_palette_residual4x4(x0 + txb_x * 4, y0 + txb_y * 4);
                score += luma_coeff_score(&av2_fwht4x4_for_score(&residual));
            }
        }
        score
    }

    fn luma_bdpcm_coeff_score_for_block(&self, x0: usize, y0: usize, horz: bool) -> usize {
        let tile_origin_x = 0;
        let tile_origin_y = 0;
        let mut score = 0usize;
        for txb_y in 0..2 {
            for txb_x in 0..2 {
                let residual = self.luma_bdpcm_residual4x4(
                    x0 + txb_x * 4,
                    y0 + txb_y * 4,
                    tile_origin_x,
                    tile_origin_y,
                    horz,
                );
                score += luma_coeff_score(&av2_fwht4x4_for_score(&residual));
            }
        }
        score
    }

    fn luma_palette_residual4x4(&self, x0: usize, y0: usize) -> [i32; 16] {
        let mut residual = [0i32; 16];
        for local_y in 0..4 {
            let y = y0 + local_y;
            for local_x in 0..4 {
                let x = x0 + local_x;
                residual[local_y * 4 + local_x] =
                    i32::from(self.y_sample(x, y)) - i32::from(self.luma_prediction_sample(x, y));
            }
        }
        residual
    }

    fn luma_bdpcm_residual4x4(
        &self,
        x0: usize,
        y0: usize,
        tile_origin_x: usize,
        tile_origin_y: usize,
        horz: bool,
    ) -> [i32; 16] {
        let mut residual = [0i32; 16];
        for local_y in 0..4 {
            let y = y0 + local_y;
            for local_x in 0..4 {
                let x = x0 + local_x;
                residual[local_y * 4 + local_x] = if horz {
                    let row_predictor = i32::from(self.luma_h_predictor(
                        x0,
                        y0,
                        local_y,
                        tile_origin_x,
                        tile_origin_y,
                    ));
                    if local_x == 0 {
                        i32::from(self.y_sample(x, y)) - row_predictor
                    } else {
                        i32::from(self.y_sample(x, y)) - i32::from(self.y_sample(x - 1, y))
                    }
                } else if local_y == 0 {
                    let col_predictor = i32::from(self.luma_v_predictor(
                        x0,
                        y0,
                        local_x,
                        tile_origin_x,
                        tile_origin_y,
                    ));
                    i32::from(self.y_sample(x, y)) - col_predictor
                } else {
                    i32::from(self.y_sample(x, y)) - i32::from(self.y_sample(x, y - 1))
                };
            }
        }
        residual
    }

    fn luma_h_predictor(
        &self,
        x0: usize,
        y0: usize,
        local_y: usize,
        tile_origin_x: usize,
        tile_origin_y: usize,
    ) -> Av2Sample {
        if x0 > tile_origin_x {
            self.y_sample(x0 - 1, y0 + local_y)
        } else if y0 > tile_origin_y {
            self.y_sample(x0, y0 - 1)
        } else {
            self.h_pred_left_edge()
        }
    }

    fn luma_v_predictor(
        &self,
        x0: usize,
        y0: usize,
        local_x: usize,
        tile_origin_x: usize,
        tile_origin_y: usize,
    ) -> Av2Sample {
        if y0 > tile_origin_y {
            self.y_sample(x0 + local_x, y0 - 1)
        } else if x0 > tile_origin_x {
            self.y_sample(x0 - 1, y0)
        } else {
            self.v_pred_above_edge()
        }
    }

    pub(crate) fn region_index_at(
        &self,
        region: &Av2LumaPaletteRegion,
        x: usize,
        y: usize,
    ) -> u8 {
        region.index_at(x, y)
    }

    pub(crate) fn region_prediction_sample(
        &self,
        region: &Av2LumaPaletteRegion,
        x: usize,
        y: usize,
    ) -> Av2Sample {
        region.prediction_at(x, y)
    }

    pub(crate) fn y_sample(&self, x: usize, y: usize) -> Av2Sample {
        self.luma_sample(&self.y_plane, x, y)
    }

    pub(crate) fn luma_prediction_sample(&self, x: usize, y: usize) -> Av2Sample {
        self.luma_sample(&self.luma_prediction, x, y)
    }

    pub(crate) fn reconstruction(&self) -> &[u8] {
        &self.reconstruction
    }

    pub(crate) fn width(&self) -> usize {
        self.width
    }

    pub(crate) fn height(&self) -> usize {
        self.height
    }

    pub(crate) fn u_sample(&self, x: usize, y: usize) -> Av2Sample {
        self.chroma_sample(&self.u_plane, x, y)
    }

    pub(crate) fn v_sample(&self, x: usize, y: usize) -> Av2Sample {
        self.chroma_sample(&self.v_plane, x, y)
    }

    fn luma_sample(&self, plane: &[Av2Sample], x: usize, y: usize) -> Av2Sample {
        assert!(x < self.width && y < self.height);
        plane[y * self.width + x]
    }

    fn chroma_sample(&self, plane: &[Av2Sample], x: usize, y: usize) -> Av2Sample {
        assert!(x < self.width && y < self.height);
        plane[y * self.width + x]
    }

    fn dc_predictor(&self) -> Av2Sample {
        av2_lossless_dc_predictor(self.bit_depth)
    }

    fn h_pred_left_edge(&self) -> Av2Sample {
        av2_lossless_h_pred_left_edge(self.bit_depth)
    }

    fn v_pred_above_edge(&self) -> Av2Sample {
        av2_lossless_v_pred_above_edge(self.bit_depth)
    }

    fn chroma_intra_residuals(
        &self,
        plane: &[Av2Sample],
        txb_x0: usize,
        txb_y0: usize,
        leaf_x0: usize,
        leaf_y0: usize,
        leaf_width: usize,
        leaf_height: usize,
        mode: Av2ChromaIntraMode,
    ) -> [i32; 16] {
        let tile_x0 = 0;
        let tile_y0 = 0;
        av2_intra_residual4x4(
            mode,
            None,
            self.bit_depth,
            |local_x, local_y| self.chroma_sample(plane, txb_x0 + local_x, txb_y0 + local_y),
            || self.chroma_dc_predictor(plane, txb_x0, txb_y0),
            |local_y| {
                if txb_x0 != tile_x0 {
                    self.chroma_sample(plane, txb_x0 - 1, txb_y0 + local_y)
                } else if txb_y0 != tile_y0 {
                    self.chroma_sample(plane, txb_x0, txb_y0 - 1)
                } else {
                    self.h_pred_left_edge()
                }
            },
            |local_x| {
                if txb_y0 != tile_y0 {
                    self.chroma_sample(plane, txb_x0 + local_x, txb_y0 - 1)
                } else if txb_x0 != tile_x0 {
                    self.chroma_sample(plane, txb_x0 - 1, txb_y0)
                } else {
                    self.v_pred_above_edge()
                }
            },
            || {
                let have_left = txb_x0 != tile_x0;
                let have_top = txb_y0 != tile_y0;
                if have_left && have_top {
                    self.chroma_sample(plane, txb_x0 - 1, txb_y0 - 1)
                } else if have_top {
                    self.chroma_sample(plane, txb_x0, txb_y0 - 1)
                } else if have_left {
                    self.chroma_sample(plane, txb_x0 - 1, txb_y0)
                } else {
                    self.dc_predictor()
                }
            },
            |_angle, local_x, local_y| match mode {
                Av2ChromaIntraMode::Directional45 => {
                    let above = self.chroma_d45_above_edge(
                        plane, txb_x0, txb_y0, leaf_x0, leaf_y0, leaf_width,
                    );
                    above[local_y + local_x + 1]
                }
                Av2ChromaIntraMode::Directional67 => {
                    let above = self.chroma_d45_above_edge(
                        plane, txb_x0, txb_y0, leaf_x0, leaf_y0, leaf_width,
                    );
                    directional_interpolate(above, local_x, local_y)
                }
                Av2ChromaIntraMode::Directional135 => {
                    let edges = self.chroma_d135_edges(plane, txb_x0, txb_y0, tile_x0, tile_y0);
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
                    let edges = self.chroma_d135_edges(plane, txb_x0, txb_y0, tile_x0, tile_y0);
                    zone2_directional_predictor(edges, 24, 170, local_x, local_y)
                }
                Av2ChromaIntraMode::Directional157 => {
                    let edges = self.chroma_d135_edges(plane, txb_x0, txb_y0, tile_x0, tile_y0);
                    zone2_directional_predictor(edges, 170, 24, local_x, local_y)
                }
                Av2ChromaIntraMode::Directional203 => {
                    let left = self.chroma_d203_left_edge(
                        plane,
                        txb_x0,
                        txb_y0,
                        leaf_x0,
                        leaf_y0,
                        leaf_height,
                    );
                    directional_interpolate(left, local_y, local_x)
                }
                _ => unreachable!("4:4:4 palette path only dispatches directional modes here"),
            },
            || {
                self.chroma_smooth_edges(
                    plane,
                    txb_x0,
                    txb_y0,
                    leaf_x0,
                    leaf_y0,
                    leaf_width,
                    leaf_height,
                )
            },
        )
    }

    fn chroma_d45_above_edge(
        &self,
        plane: &[Av2Sample],
        txb_x0: usize,
        txb_y0: usize,
        leaf_x0: usize,
        leaf_y0: usize,
        leaf_width: usize,
    ) -> [Av2Sample; 8] {
        let tile_x0 = 0;
        let tile_y0 = 0;
        let tile_right = self.width;
        let have_top = txb_y0 > tile_y0;
        let have_left = txb_x0 > tile_x0;
        let mut above = [self.v_pred_above_edge(); 8];
        if have_top {
            for index in 0..above.len() {
                let x = txb_x0 + index;
                if x < leaf_x0 + leaf_width || (txb_y0 == leaf_y0 && x < tile_right) {
                    above[index] = self.chroma_sample(plane, x, txb_y0 - 1);
                } else if index > 0 {
                    above[index] = above[index - 1];
                }
            }
        } else if have_left {
            above.fill(self.chroma_sample(plane, txb_x0 - 1, txb_y0));
        }
        above
    }

    fn chroma_d135_edges(
        &self,
        plane: &[Av2Sample],
        txb_x0: usize,
        txb_y0: usize,
        tile_x0: usize,
        tile_y0: usize,
    ) -> ChromaD135Edges {
        let have_top = txb_y0 > tile_y0;
        let have_left = txb_x0 > tile_x0;
        let mut above = [self.v_pred_above_edge(); 4];
        let mut left = [self.h_pred_left_edge(); 4];
        if have_top {
            for local_x in 0..4 {
                above[local_x] = self.chroma_sample(plane, txb_x0 + local_x, txb_y0 - 1);
            }
        } else if have_left {
            above.fill(self.chroma_sample(plane, txb_x0 - 1, txb_y0));
        }
        if have_left {
            for local_y in 0..4 {
                left[local_y] = self.chroma_sample(plane, txb_x0 - 1, txb_y0 + local_y);
            }
        } else if have_top {
            left.fill(self.chroma_sample(plane, txb_x0, txb_y0 - 1));
        }
        let above_left = if have_top && have_left {
            self.chroma_sample(plane, txb_x0 - 1, txb_y0 - 1)
        } else if have_top {
            above[0]
        } else if have_left {
            left[0]
        } else {
            self.dc_predictor()
        };
        ChromaD135Edges {
            above_left,
            above,
            left,
        }
    }

    fn chroma_d203_left_edge(
        &self,
        plane: &[Av2Sample],
        txb_x0: usize,
        txb_y0: usize,
        leaf_x0: usize,
        leaf_y0: usize,
        leaf_height: usize,
    ) -> [Av2Sample; 8] {
        let tile_x0 = 0;
        let tile_y0 = 0;
        let tile_bottom = self.height;
        let have_top = txb_y0 > tile_y0;
        let have_left = txb_x0 > tile_x0;
        let mut left = [self.h_pred_left_edge(); 8];
        if have_left {
            for index in 0..left.len() {
                let y = txb_y0 + index;
                // Match AVM has_bottom_left(): only TXBs on the leaf's left
                // edge may use D203 bottom-left overhang samples.
                if y < txb_y0 + AV2_LUMA_PALETTE_BLOCK_SIZE / 2
                    || (txb_x0 == leaf_x0 && (y < leaf_y0 + leaf_height || y < tile_bottom))
                {
                    left[index] = self.chroma_sample(plane, txb_x0 - 1, y);
                } else if index > 0 {
                    left[index] = left[index - 1];
                }
            }
        } else if have_top {
            left.fill(self.chroma_sample(plane, txb_x0, txb_y0 - 1));
        }
        left
    }

    fn chroma_smooth_edges(
        &self,
        plane: &[Av2Sample],
        txb_x0: usize,
        txb_y0: usize,
        leaf_x0: usize,
        leaf_y0: usize,
        leaf_width: usize,
        leaf_height: usize,
    ) -> ([Av2Sample; 5], [Av2Sample; 5]) {
        debug_assert!(txb_x0 >= leaf_x0 && txb_y0 >= leaf_y0);
        debug_assert!(txb_x0 + 4 <= leaf_x0 + leaf_width);
        debug_assert!(txb_y0 + 4 <= leaf_y0 + leaf_height);
        let tile_x0 = 0;
        let tile_y0 = 0;
        let have_top = txb_y0 > tile_y0;
        let have_left = txb_x0 > tile_x0;
        let mut above = [self.v_pred_above_edge(); 5];
        let mut left = [self.h_pred_left_edge(); 5];

        if have_top {
            for local_x in 0..4 {
                above[local_x] = self.chroma_sample(plane, txb_x0 + local_x, txb_y0 - 1);
            }
        } else if have_left {
            above[..4].fill(self.chroma_sample(plane, txb_x0 - 1, txb_y0));
        }

        if have_left {
            for local_y in 0..4 {
                left[local_y] = self.chroma_sample(plane, txb_x0 - 1, txb_y0 + local_y);
            }
        } else if have_top {
            left[..4].fill(self.chroma_sample(plane, txb_x0, txb_y0 - 1));
        }

        let tile_right = self.width;
        if have_top
            && (txb_x0 + 4 < leaf_x0 + leaf_width || (txb_y0 == leaf_y0 && txb_x0 + 4 < tile_right))
        {
            above[4] = self.chroma_sample(plane, txb_x0 + 4, txb_y0 - 1);
        } else {
            above[4] = above[3];
        }

        if have_left
            && txb_x0 == leaf_x0
            && txb_y0 + 4 < leaf_y0 + leaf_height
            && txb_y0 + 4 < self.height
        {
            left[4] = self.chroma_sample(plane, txb_x0 - 1, txb_y0 + 4);
        } else {
            left[4] = left[3];
        }

        (above, left)
    }

    fn chroma_dc_predictor(&self, plane: &[Av2Sample], x0: usize, y0: usize) -> Av2Sample {
        let tile_x0 = 0;
        let tile_y0 = 0;
        let have_left = x0 != tile_x0;
        let have_top = y0 != tile_y0;
        if !have_left && !have_top {
            return self.dc_predictor();
        }

        let mut sum = 0u32;
        let mut count = 0u32;
        if have_top {
            for local_x in 0..4 {
                sum += u32::from(self.chroma_sample(plane, x0 + local_x, y0 - 1));
                count += 1;
            }
        }
        if have_left {
            for local_y in 0..4 {
                sum += u32::from(self.chroma_sample(plane, x0 - 1, y0 + local_y));
                count += 1;
            }
        }
        ((sum + count / 2) / count) as Av2Sample
    }

    fn block_for_origin(&self, x0: usize, y0: usize) -> &Av2LumaPaletteBlock444 {
        &self.blocks[self.block_index_for_origin(x0, y0)]
    }

    fn block_index_for_origin(&self, x0: usize, y0: usize) -> usize {
        assert!(x0 < self.width && y0 < self.height);
        assert_eq!(x0 % AV2_LUMA_PALETTE_BLOCK_SIZE, 0);
        assert_eq!(y0 % AV2_LUMA_PALETTE_BLOCK_SIZE, 0);
        let block_x = x0 / AV2_LUMA_PALETTE_BLOCK_SIZE;
        let block_y = y0 / AV2_LUMA_PALETTE_BLOCK_SIZE;
        assert!(block_x < self.blocks_wide && block_y < self.blocks_high);
        block_y * self.blocks_wide + block_x
    }
}
