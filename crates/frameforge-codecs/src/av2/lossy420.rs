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

    fn luma_dc_predictor(&self, plane: Av2LossyPlane, x0: usize, y0: usize) -> Av2Sample {
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

    fn chroma_h_predictor(
        &self,
        plane: Av2LossyPlane,
        x0: usize,
        y0: usize,
        local_y: usize,
    ) -> Av2Sample {
        let (tile_origin_x, tile_origin_y) = self.plane_origin(plane);
        if x0 > tile_origin_x {
            self.recon_sample(plane, x0 - 1, y0 + local_y)
        } else if y0 > tile_origin_y {
            self.recon_sample(plane, x0, y0 - 1)
        } else {
            av2_lossless_h_pred_left_edge(self.bit_depth)
        }
    }

    fn predictor_sample(
        &self,
        plane: Av2LossyPlane,
        x0: usize,
        y0: usize,
        _local_x: usize,
        local_y: usize,
    ) -> Av2Sample {
        match plane {
            Av2LossyPlane::Y => self.luma_dc_predictor(plane, x0, y0),
            Av2LossyPlane::U | Av2LossyPlane::V => {
                self.chroma_h_predictor(plane, x0, y0, local_y)
            }
        }
    }

    fn analyze_txb(&self, plane: Av2LossyPlane, x0: usize, y0: usize) -> Av2LossyTxbAnalysis {
        let mut source = [0; TX4X4_SAMPLES];
        let mut predictor = [0; TX4X4_SAMPLES];
        let mut residual = [0i32; TX4X4_SAMPLES];
        let mut sum = 0i32;
        for local_y in 0..TX4X4_SIZE {
            for local_x in 0..TX4X4_SIZE {
                let index = local_y * TX4X4_SIZE + local_x;
                let predictor_sample = self.predictor_sample(plane, x0, y0, local_x, local_y);
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
