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

struct Av2Lossy420TileState<'a> {
    geometry: Av2VideoGeometry,
    region: Av2TileRegion,
    bit_depth: SampleBitDepth,
    source: &'a [u8],
    recon: &'a mut [u8],
    y_len: usize,
    c_width: usize,
    c_height: usize,
    c_len: usize,
}

impl<'a> Av2Lossy420TileState<'a> {
    fn new(
        geometry: Av2VideoGeometry,
        region: Av2TileRegion,
        bit_depth: SampleBitDepth,
        source: &'a [u8],
        recon: &'a mut [u8],
    ) -> Self {
        let y_len = geometry.width * geometry.height;
        let c_width = geometry.width / 2;
        let c_height = geometry.height / 2;
        let c_len = c_width * c_height;
        let expected_len = (y_len + 2 * c_len) * bit_depth.bytes_per_sample();
        assert_eq!(
            source.len(),
            expected_len,
            "AV2 4:2:0 residual source length must match geometry"
        );
        assert_eq!(
            recon.len(),
            source.len(),
            "AV2 4:2:0 residual reconstruction length must match source"
        );
        Self {
            geometry,
            region,
            bit_depth,
            source,
            recon,
            y_len,
            c_width,
            c_height,
            c_len,
        }
    }

    fn plane_geometry(&self, plane: Av2Lossy420Plane) -> (usize, usize) {
        match plane {
            Av2Lossy420Plane::Y => (self.geometry.width, self.geometry.height),
            Av2Lossy420Plane::U | Av2Lossy420Plane::V => (self.c_width, self.c_height),
        }
    }

    fn plane_origin(&self, plane: Av2Lossy420Plane) -> (usize, usize) {
        match plane {
            Av2Lossy420Plane::Y => (self.region.origin_x, self.region.origin_y),
            Av2Lossy420Plane::U | Av2Lossy420Plane::V => {
                (self.region.origin_x / 2, self.region.origin_y / 2)
            }
        }
    }

    fn txb_origin(&self, plane: Av2Lossy420Plane, col: usize, row: usize) -> (usize, usize) {
        let (origin_x, origin_y) = self.plane_origin(plane);
        (origin_x + col * TX4X4_SIZE, origin_y + row * TX4X4_SIZE)
    }

    fn offset(&self, plane: Av2Lossy420Plane, x: usize, y: usize) -> usize {
        match plane {
            Av2Lossy420Plane::Y => y * self.geometry.width + x,
            Av2Lossy420Plane::U => self.y_len + y * self.c_width + x,
            Av2Lossy420Plane::V => self.y_len + self.c_len + y * self.c_width + x,
        }
    }

    fn source_sample(&self, plane: Av2Lossy420Plane, x: usize, y: usize) -> Av2Sample {
        read_planar_sample(self.source, self.offset(plane, x, y), self.bit_depth)
            .expect("validated AV2 4:2:0 source must contain every sample")
    }

    fn recon_sample(&self, plane: Av2Lossy420Plane, x: usize, y: usize) -> Av2Sample {
        read_planar_sample(self.recon, self.offset(plane, x, y), self.bit_depth)
            .expect("validated AV2 4:2:0 reconstruction must contain every sample")
    }

    fn set_recon_sample(&mut self, plane: Av2Lossy420Plane, x: usize, y: usize, sample: Av2Sample) {
        let offset = self.offset(plane, x, y);
        write_planar_sample(self.recon, offset, sample, self.bit_depth)
            .expect("validated AV2 4:2:0 reconstruction must contain every sample");
    }

    fn luma_dc_predictor(&self, plane: Av2Lossy420Plane, x0: usize, y0: usize) -> Av2Sample {
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

    fn chroma_h_predictor(&self, plane: Av2Lossy420Plane, x0: usize, y0: usize) -> Av2Sample {
        let (tile_origin_x, tile_origin_y) = self.plane_origin(plane);
        // read_intra_uv_mode() currently emits the normal horizontal chroma
        // predictor for 4:2:0 leaves. AVM's H_PRED falls back to above[0] when
        // the left edge is unavailable, then to base+1 at the tile corner.
        if x0 > tile_origin_x {
            self.recon_sample(plane, x0 - 1, y0)
        } else if y0 > tile_origin_y {
            self.recon_sample(plane, x0, y0 - 1)
        } else {
            av2_lossless_h_pred_left_edge(self.bit_depth)
        }
    }

    fn predictor(&self, plane: Av2Lossy420Plane, x0: usize, y0: usize) -> Av2Sample {
        match plane {
            Av2Lossy420Plane::Y => self.luma_dc_predictor(plane, x0, y0),
            Av2Lossy420Plane::U | Av2Lossy420Plane::V => self.chroma_h_predictor(plane, x0, y0),
        }
    }

    fn quantized_dc_delta(&self, plane: Av2Lossy420Plane, x0: usize, y0: usize) -> i16 {
        let predictor = i32::from(self.predictor(plane, x0, y0));
        let mut sum = 0i32;
        for local_y in 0..TX4X4_SIZE {
            for local_x in 0..TX4X4_SIZE {
                sum += i32::from(self.source_sample(plane, x0 + local_x, y0 + local_y)) - predictor;
            }
        }
        let average = round_div_i32(sum, TX4X4_SAMPLES as i32);
        let max_delta = i32::from(self.bit_depth.max_sample());
        quantize_i32_to_step(average, self.quant_step()).clamp(-max_delta, max_delta) as i16
    }

    fn fill_recon_txb(&mut self, plane: Av2Lossy420Plane, x0: usize, y0: usize, delta: i16) {
        let predictor = i32::from(self.predictor(plane, x0, y0));
        let sample = (predictor + i32::from(delta)).clamp(0, i32::from(self.bit_depth.max_sample()))
            as Av2Sample;
        let (plane_width, plane_height) = self.plane_geometry(plane);
        for local_y in 0..TX4X4_SIZE {
            let y = y0 + local_y;
            if y >= plane_height {
                continue;
            }
            for local_x in 0..TX4X4_SIZE {
                let x = x0 + local_x;
                if x < plane_width {
                    self.set_recon_sample(plane, x, y, sample);
                }
            }
        }
    }

    fn quant_step(&self) -> i32 {
        AV2_LOSSY_420_DC_QUANT_STEP << u32::from(self.bit_depth.bits() - 8)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Av2Lossy420Plane {
    Y,
    U,
    V,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Av2LosslessSubsampledModeDecision {
    luma_intra_mode: Av2LumaIntraMode,
    luma_bdpcm_horz: Option<bool>,
    chroma_use_bdpcm: bool,
    chroma_intra_mode: Av2ChromaIntraMode,
    use_fsc: bool,
}

impl Default for Av2LosslessSubsampledModeDecision {
    fn default() -> Self {
        Self {
            luma_intra_mode: Av2LumaIntraMode::Dc,
            luma_bdpcm_horz: None,
            chroma_use_bdpcm: false,
            chroma_intra_mode: Av2ChromaIntraMode::Horizontal,
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
