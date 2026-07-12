fn chroma_directional_angle_for_mode(mode: Av2LosslessSubsampledModeDecision) -> Option<i16> {
    if let Some((base, delta)) = mode.luma_intra_mode.directional() {
        if base.chroma_mode() == mode.chroma_intra_mode {
            return Some(base.angle(delta));
        }
    }
    av2_chroma_directional_angle(mode.chroma_intra_mode)
}

#[derive(Debug, Clone, Copy, Default)]
struct Av2DcHvBdpcmTxbScores {
    dc: usize,
    horizontal: usize,
    vertical: usize,
    bdpcm_horizontal: usize,
    bdpcm_vertical: usize,
}

impl Av2DcHvBdpcmTxbScores {
    fn add_assign(&mut self, other: Self) {
        self.dc += other.dc;
        self.horizontal += other.horizontal;
        self.vertical += other.vertical;
        self.bdpcm_horizontal += other.bdpcm_horizontal;
        self.bdpcm_vertical += other.bdpcm_vertical;
    }
}

fn residual_sample_proxy_score(
    residual: &[i32; TX4X4_SAMPLES],
    kind: Av2CoefficientProxyKind,
) -> usize {
    let magnitude_scale = match kind {
        Av2CoefficientProxyKind::LumaIdtx => 4,
        Av2CoefficientProxyKind::LumaTransform => 4,
        Av2CoefficientProxyKind::ChromaTransform => 3,
    };
    let mut score = 16usize;
    for &delta in residual {
        let level = delta.unsigned_abs() as usize;
        if level == 0 {
            continue;
        }
        score += 80 + level.min(255) * magnitude_scale;
    }
    score
}

struct Av2LosslessSubsampledTileState<'a> {
    geometry: Av2VideoGeometry,
    region: Av2TileRegion,
    chroma_format: Av2ChromaFormat,
    bit_depth: SampleBitDepth,
    mode_search: Av2LosslessSubsampledModeSearch,
    source: &'a [u8],
    recon: &'a mut [u8],
    y_len: usize,
    c_width: usize,
    c_height: usize,
    c_len: usize,
}

impl<'a> Av2LosslessSubsampledTileState<'a> {
    fn new(
        geometry: Av2VideoGeometry,
        region: Av2TileRegion,
        chroma_format: Av2ChromaFormat,
        bit_depth: SampleBitDepth,
        mode_search: Av2LosslessSubsampledModeSearch,
        source: &'a [u8],
        recon: &'a mut [u8],
    ) -> Self {
        assert!(
            matches!(
                chroma_format,
                Av2ChromaFormat::Yuv420 | Av2ChromaFormat::Yuv422
            ),
            "AV2 subsampled lossless state expects 4:2:0 or 4:2:2 input"
        );
        let y_len = geometry.width * geometry.height;
        let c_width = geometry.width / chroma_subsample_x(chroma_format);
        let c_height = geometry.height / chroma_subsample_y(chroma_format);
        let c_len = c_width * c_height;
        let expected_len = (y_len + 2 * c_len) * bit_depth.bytes_per_sample();
        assert_eq!(
            source.len(),
            expected_len,
            "AV2 subsampled lossless source length must match geometry"
        );
        assert_eq!(
            recon.len(),
            source.len(),
            "AV2 subsampled lossless reconstruction length must match source"
        );
        Self {
            geometry,
            region,
            chroma_format,
            bit_depth,
            mode_search,
            source,
            recon,
            y_len,
            c_width,
            c_height,
            c_len,
        }
    }

    fn plane_geometry(&self, plane: Av2LosslessPlane) -> (usize, usize) {
        match plane {
            Av2LosslessPlane::Y => (self.geometry.width, self.geometry.height),
            Av2LosslessPlane::U | Av2LosslessPlane::V => (self.c_width, self.c_height),
        }
    }

    fn plane_origin(&self, plane: Av2LosslessPlane) -> (usize, usize) {
        match plane {
            Av2LosslessPlane::Y => (self.region.origin_x, self.region.origin_y),
            Av2LosslessPlane::U | Av2LosslessPlane::V => (
                self.region.origin_x / chroma_subsample_x(self.chroma_format),
                self.region.origin_y / chroma_subsample_y(self.chroma_format),
            ),
        }
    }

    fn plane_subsampling(&self, plane: Av2LosslessPlane) -> (usize, usize) {
        match plane {
            Av2LosslessPlane::Y => (1, 1),
            Av2LosslessPlane::U | Av2LosslessPlane::V => (
                chroma_subsample_x(self.chroma_format),
                chroma_subsample_y(self.chroma_format),
            ),
        }
    }

    fn coded_mi_for_plane_sample(
        &self,
        plane: Av2LosslessPlane,
        x: usize,
        y: usize,
    ) -> (usize, usize) {
        let (sub_x, sub_y) = self.plane_subsampling(plane);
        ((y * sub_y) / MI_SIZE, (x * sub_x) / MI_SIZE)
    }

    fn txb_origin(&self, plane: Av2LosslessPlane, col: usize, row: usize) -> (usize, usize) {
        let (origin_x, origin_y) = self.plane_origin(plane);
        (origin_x + col * TX4X4_SIZE, origin_y + row * TX4X4_SIZE)
    }

    fn source_block4x4(&self, plane: Av2LosslessPlane, x0: usize, y0: usize) -> [i32; 16] {
        let mut block = [0i32; TX4X4_SAMPLES];
        for local_y in 0..TX4X4_SIZE {
            for local_x in 0..TX4X4_SIZE {
                block[local_y * TX4X4_SIZE + local_x] =
                    i32::from(self.source_sample(plane, x0 + local_x, y0 + local_y));
            }
        }
        block
    }

    fn offset(&self, plane: Av2LosslessPlane, x: usize, y: usize) -> usize {
        match plane {
            Av2LosslessPlane::Y => y * self.geometry.width + x,
            Av2LosslessPlane::U => self.y_len + y * self.c_width + x,
            Av2LosslessPlane::V => self.y_len + self.c_len + y * self.c_width + x,
        }
    }

    fn source_sample(&self, plane: Av2LosslessPlane, x: usize, y: usize) -> Av2Sample {
        read_validated_planar_sample(self.source, self.offset(plane, x, y), self.bit_depth)
    }

    fn recon_sample(&self, plane: Av2LosslessPlane, x: usize, y: usize) -> Av2Sample {
        read_validated_planar_sample(self.recon, self.offset(plane, x, y), self.bit_depth)
    }

    fn dc_predictor(&self, plane: Av2LosslessPlane, x0: usize, y0: usize) -> Av2Sample {
        let edge_sample = |plane, x, y| self.recon_sample(plane, x, y);
        self.dc_predictor_with(plane, x0, y0, &edge_sample)
    }

    fn dc_predictor_with<EdgeSample>(
        &self,
        plane: Av2LosslessPlane,
        x0: usize,
        y0: usize,
        edge_sample: &EdgeSample,
    ) -> Av2Sample
    where
        EdgeSample: Fn(Av2LosslessPlane, usize, usize) -> Av2Sample,
    {
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
                sum += u32::from(edge_sample(plane, x, y0 - 1));
                count += 1;
            }
        }
        if have_left {
            for y in y0..(y0 + TX4X4_SIZE) {
                sum += u32::from(edge_sample(plane, x0 - 1, y));
                count += 1;
            }
        }
        ((sum + count / 2) / count) as Av2Sample
    }

    fn h_predictor(
        &self,
        plane: Av2LosslessPlane,
        x0: usize,
        y0: usize,
        local_y: usize,
    ) -> Av2Sample {
        let edge_sample = |plane, x, y| self.recon_sample(plane, x, y);
        self.h_predictor_with(plane, x0, y0, local_y, &edge_sample)
    }

    fn h_predictor_with<EdgeSample>(
        &self,
        plane: Av2LosslessPlane,
        x0: usize,
        y0: usize,
        local_y: usize,
        edge_sample: &EdgeSample,
    ) -> Av2Sample
    where
        EdgeSample: Fn(Av2LosslessPlane, usize, usize) -> Av2Sample,
    {
        let (tile_origin_x, tile_origin_y) = self.plane_origin(plane);
        if x0 > tile_origin_x {
            edge_sample(plane, x0 - 1, y0 + local_y)
        } else if y0 > tile_origin_y {
            edge_sample(plane, x0, y0 - 1)
        } else {
            av2_lossless_h_pred_left_edge(self.bit_depth)
        }
    }

    fn v_predictor(
        &self,
        plane: Av2LosslessPlane,
        x0: usize,
        y0: usize,
        local_x: usize,
    ) -> Av2Sample {
        let edge_sample = |plane, x, y| self.recon_sample(plane, x, y);
        self.v_predictor_with(plane, x0, y0, local_x, &edge_sample)
    }

    fn v_predictor_with<EdgeSample>(
        &self,
        plane: Av2LosslessPlane,
        x0: usize,
        y0: usize,
        local_x: usize,
        edge_sample: &EdgeSample,
    ) -> Av2Sample
    where
        EdgeSample: Fn(Av2LosslessPlane, usize, usize) -> Av2Sample,
    {
        let (tile_origin_x, tile_origin_y) = self.plane_origin(plane);
        if y0 > tile_origin_y {
            edge_sample(plane, x0 + local_x, y0 - 1)
        } else if x0 > tile_origin_x {
            edge_sample(plane, x0 - 1, y0)
        } else {
            av2_lossless_v_pred_above_edge(self.bit_depth)
        }
    }

    #[cfg(test)]
    fn tx4x4_coefficients(
        &self,
        plane: Av2LosslessPlane,
        x0: usize,
        y0: usize,
    ) -> [i32; TX4X4_SAMPLES] {
        self.tx4x4_coefficients_for_mode(
            plane,
            x0,
            y0,
            Av2LosslessSubsampledModeDecision::default(),
            x0,
            y0,
            TX4X4_SIZE,
            TX4X4_SIZE,
            &Av2CodedMiContext::new(
                self.geometry.height.div_ceil(MI_SIZE),
                self.geometry.width.div_ceil(MI_SIZE),
            ),
        )
    }

    fn tx4x4_coefficients_for_mode(
        &self,
        plane: Av2LosslessPlane,
        x0: usize,
        y0: usize,
        mode: Av2LosslessSubsampledModeDecision,
        leaf_x0: usize,
        leaf_y0: usize,
        leaf_width: usize,
        leaf_height: usize,
        coded_mi_context: &Av2CodedMiContext,
    ) -> [i32; TX4X4_SAMPLES] {
        let residual = match plane {
            Av2LosslessPlane::Y => {
                if let Some(horz) = mode.luma_bdpcm_horz {
                    self.dpcm_residual4x4(plane, x0, y0, horz)
                } else {
                    self.luma_intra_residual4x4(
                        x0,
                        y0,
                        mode.luma_intra_mode,
                        leaf_x0,
                        leaf_y0,
                        leaf_width,
                        leaf_height,
                        coded_mi_context,
                    )
                }
            }
            Av2LosslessPlane::U | Av2LosslessPlane::V => {
                if mode.chroma_use_bdpcm {
                    self.dpcm_residual4x4(plane, x0, y0, mode.chroma_intra_mode.is_horizontal())
                } else {
                    self.intra_residual4x4(
                        plane,
                        x0,
                        y0,
                        mode.chroma_intra_mode,
                        chroma_directional_angle_for_mode(mode),
                        leaf_x0,
                        leaf_y0,
                        leaf_width,
                        leaf_height,
                        coded_mi_context,
                    )
                }
            }
        };
        if mode.use_fsc {
            idtx4x4_coefficients(&residual)
        } else {
            av2_fwht4x4(&residual)
        }
    }

    fn tx4x4_coefficients_for_mode_score(
        &self,
        plane: Av2LosslessPlane,
        x0: usize,
        y0: usize,
        mode: Av2LosslessSubsampledModeDecision,
        leaf_x0: usize,
        leaf_y0: usize,
        leaf_width: usize,
        leaf_height: usize,
        coded_mi_context: &Av2CodedMiContext,
    ) -> [i32; TX4X4_SAMPLES] {
        let residual = match plane {
            Av2LosslessPlane::Y => {
                if let Some(horz) = mode.luma_bdpcm_horz {
                    self.dpcm_residual4x4_for_score(plane, x0, y0, horz, leaf_x0, leaf_y0)
                } else {
                    self.luma_intra_residual4x4_for_score(
                        x0,
                        y0,
                        mode.luma_intra_mode,
                        leaf_x0,
                        leaf_y0,
                        leaf_width,
                        leaf_height,
                        coded_mi_context,
                    )
                }
            }
            Av2LosslessPlane::U | Av2LosslessPlane::V => {
                if mode.chroma_use_bdpcm {
                    self.dpcm_residual4x4_for_score(
                        plane,
                        x0,
                        y0,
                        mode.chroma_intra_mode.is_horizontal(),
                        leaf_x0,
                        leaf_y0,
                    )
                } else {
                    self.intra_residual4x4_for_score(
                        plane,
                        x0,
                        y0,
                        mode.chroma_intra_mode,
                        chroma_directional_angle_for_mode(mode),
                        leaf_x0,
                        leaf_y0,
                        leaf_width,
                        leaf_height,
                        coded_mi_context,
                    )
                }
            }
        };
        if mode.use_fsc {
            idtx4x4_coefficients(&residual)
        } else {
            av2_fwht4x4(&residual)
        }
    }

    fn luma_intra_residual4x4(
        &self,
        x0: usize,
        y0: usize,
        mode: Av2LumaIntraMode,
        leaf_x0: usize,
        leaf_y0: usize,
        leaf_width: usize,
        leaf_height: usize,
        coded_mi_context: &Av2CodedMiContext,
    ) -> [i32; TX4X4_SAMPLES] {
        if let Some((base, delta)) = mode.directional() {
            let angle = base.angle(delta);
            if angle != 90 && angle != 180 {
                return self.luma_directional_idif_residual4x4(
                    x0,
                    y0,
                    angle,
                    leaf_x0,
                    leaf_y0,
                    leaf_width,
                    leaf_height,
                    coded_mi_context,
                );
            }
        }
        self.intra_residual4x4(
            Av2LosslessPlane::Y,
            x0,
            y0,
            chroma_mode_for_luma_mode(mode),
            None,
            leaf_x0,
            leaf_y0,
            leaf_width,
            leaf_height,
            coded_mi_context,
        )
    }

    fn luma_intra_residual4x4_for_score(
        &self,
        x0: usize,
        y0: usize,
        mode: Av2LumaIntraMode,
        leaf_x0: usize,
        leaf_y0: usize,
        leaf_width: usize,
        leaf_height: usize,
        coded_mi_context: &Av2CodedMiContext,
    ) -> [i32; TX4X4_SAMPLES] {
        if let Some((base, delta)) = mode.directional() {
            let angle = base.angle(delta);
            if angle != 90 && angle != 180 {
                return self.luma_directional_idif_residual4x4_for_score(
                    x0,
                    y0,
                    angle,
                    leaf_x0,
                    leaf_y0,
                    leaf_width,
                    leaf_height,
                    coded_mi_context,
                );
            }
        }
        self.intra_residual4x4_for_score(
            Av2LosslessPlane::Y,
            x0,
            y0,
            chroma_mode_for_luma_mode(mode),
            None,
            leaf_x0,
            leaf_y0,
            leaf_width,
            leaf_height,
            coded_mi_context,
        )
    }

    fn intra_residual4x4(
        &self,
        plane: Av2LosslessPlane,
        x0: usize,
        y0: usize,
        mode: Av2ChromaIntraMode,
        directional_angle: Option<i16>,
        leaf_x0: usize,
        leaf_y0: usize,
        leaf_width: usize,
        leaf_height: usize,
        coded_mi_context: &Av2CodedMiContext,
    ) -> [i32; TX4X4_SAMPLES] {
        if plane == Av2LosslessPlane::Y {
            if let Some(angle) = av2_chroma_directional_angle(mode) {
                if angle != 90 && angle != 180 {
                    return self.luma_directional_idif_residual4x4(
                        x0,
                        y0,
                        angle,
                        leaf_x0,
                        leaf_y0,
                        leaf_width,
                        leaf_height,
                        coded_mi_context,
                    );
                }
            }
        }
        av2_intra_residual4x4(
            mode,
            directional_angle,
            self.bit_depth,
            |local_x, local_y| self.source_sample(plane, x0 + local_x, y0 + local_y),
            || self.dc_predictor(plane, x0, y0),
            |local_y| self.h_predictor(plane, x0, y0, local_y),
            |local_x| self.v_predictor(plane, x0, y0, local_x),
            || self.above_left_predictor(plane, x0, y0),
            |angle, local_x, local_y| {
                self.directional_predictor(
                    plane,
                    x0,
                    y0,
                    angle,
                    local_x,
                    local_y,
                    leaf_x0,
                    leaf_y0,
                    leaf_width,
                    leaf_height,
                    coded_mi_context,
                )
            },
            || {
                self.smooth_edges(
                    plane,
                    x0,
                    y0,
                    leaf_x0,
                    leaf_y0,
                    leaf_width,
                    leaf_height,
                    coded_mi_context,
                )
            },
        )
    }

    fn directional_predictor(
        &self,
        plane: Av2LosslessPlane,
        x0: usize,
        y0: usize,
        angle: i16,
        local_x: usize,
        local_y: usize,
        leaf_x0: usize,
        leaf_y0: usize,
        leaf_width: usize,
        leaf_height: usize,
        coded_mi_context: &Av2CodedMiContext,
    ) -> Av2Sample {
        let edge_sample = |plane, x, y| self.recon_sample(plane, x, y);
        self.directional_predictor_with(
            plane,
            x0,
            y0,
            angle,
            local_x,
            local_y,
            leaf_x0,
            leaf_y0,
            leaf_width,
            leaf_height,
            coded_mi_context,
            &edge_sample,
        )
    }

    fn directional_predictor_for_score(
        &self,
        plane: Av2LosslessPlane,
        x0: usize,
        y0: usize,
        angle: i16,
        local_x: usize,
        local_y: usize,
        leaf_x0: usize,
        leaf_y0: usize,
        leaf_width: usize,
        leaf_height: usize,
        coded_mi_context: &Av2CodedMiContext,
    ) -> Av2Sample {
        let edge_sample =
            |plane, x, y| self.neighbor_sample_for_score(plane, x, y, leaf_x0, leaf_y0);
        self.directional_predictor_with(
            plane,
            x0,
            y0,
            angle,
            local_x,
            local_y,
            leaf_x0,
            leaf_y0,
            leaf_width,
            leaf_height,
            coded_mi_context,
            &edge_sample,
        )
    }

    fn directional_predictor_with<EdgeSample>(
        &self,
        plane: Av2LosslessPlane,
        x0: usize,
        y0: usize,
        angle: i16,
        local_x: usize,
        local_y: usize,
        leaf_x0: usize,
        leaf_y0: usize,
        leaf_width: usize,
        leaf_height: usize,
        coded_mi_context: &Av2CodedMiContext,
        edge_sample: &EdgeSample,
    ) -> Av2Sample
    where
        EdgeSample: Fn(Av2LosslessPlane, usize, usize) -> Av2Sample,
    {
        if angle > 0 && angle < 90 {
            let above = self.directional_above_edge_with(
                plane,
                x0,
                y0,
                leaf_x0,
                leaf_y0,
                leaf_width,
                coded_mi_context,
                edge_sample,
            );
            return directional_interpolate_with_delta(
                above,
                av2_directional_dx(angle),
                local_x,
                local_y,
            );
        }
        if angle > 90 && angle < 180 {
            let above = self.directional_above_edge_with(
                plane,
                x0,
                y0,
                leaf_x0,
                leaf_y0,
                leaf_width,
                coded_mi_context,
                edge_sample,
            );
            let left = self.directional_left_edge_with(
                plane,
                x0,
                y0,
                leaf_x0,
                leaf_y0,
                leaf_height,
                coded_mi_context,
                edge_sample,
            );
            let edges = ChromaD135Edges {
                above_left: self.above_left_predictor_with(plane, x0, y0, edge_sample),
                above: [above[0], above[1], above[2], above[3]],
                left: [left[0], left[1], left[2], left[3]],
            };
            return zone2_directional_predictor(
                edges,
                av2_directional_dx(angle),
                av2_directional_dy(angle),
                local_x,
                local_y,
            );
        }
        if angle > 180 && angle < 270 {
            let left = self.directional_left_edge_with(
                plane,
                x0,
                y0,
                leaf_x0,
                leaf_y0,
                leaf_height,
                coded_mi_context,
                edge_sample,
            );
            return directional_interpolate_with_delta(
                left,
                av2_directional_dy(angle),
                local_y,
                local_x,
            );
        }
        unreachable!("generic directional predictor expects a non-cardinal angle")
    }

    fn dpcm_residual4x4(
        &self,
        plane: Av2LosslessPlane,
        x0: usize,
        y0: usize,
        horz: bool,
    ) -> [i32; TX4X4_SAMPLES] {
        let mut residual = [0i32; TX4X4_SAMPLES];
        for local_y in 0..TX4X4_SIZE {
            for local_x in 0..TX4X4_SIZE {
                let x = x0 + local_x;
                let y = y0 + local_y;
                let sample = i32::from(self.source_sample(plane, x, y));
                let predicted_delta = if horz {
                    if local_x == 0 {
                        sample - i32::from(self.h_predictor(plane, x0, y0, local_y))
                    } else {
                        sample - i32::from(self.source_sample(plane, x - 1, y))
                    }
                } else if local_y == 0 {
                    sample - i32::from(self.v_predictor(plane, x0, y0, local_x))
                } else {
                    sample - i32::from(self.source_sample(plane, x, y - 1))
                };
                residual[local_y * TX4X4_SIZE + local_x] = predicted_delta;
            }
        }
        residual
    }

    fn intra_residual4x4_for_score(
        &self,
        plane: Av2LosslessPlane,
        x0: usize,
        y0: usize,
        mode: Av2ChromaIntraMode,
        directional_angle: Option<i16>,
        leaf_x0: usize,
        leaf_y0: usize,
        leaf_width: usize,
        leaf_height: usize,
        coded_mi_context: &Av2CodedMiContext,
    ) -> [i32; TX4X4_SAMPLES] {
        if plane == Av2LosslessPlane::Y {
            if let Some(angle) = av2_chroma_directional_angle(mode) {
                if angle != 90 && angle != 180 {
                    return self.luma_directional_idif_residual4x4_for_score(
                        x0,
                        y0,
                        angle,
                        leaf_x0,
                        leaf_y0,
                        leaf_width,
                        leaf_height,
                        coded_mi_context,
                    );
                }
            }
        }
        av2_intra_residual4x4(
            mode,
            directional_angle,
            self.bit_depth,
            |local_x, local_y| self.source_sample(plane, x0 + local_x, y0 + local_y),
            || self.dc_predictor_for_score(plane, x0, y0, leaf_x0, leaf_y0),
            |local_y| self.h_predictor_for_score(plane, x0, y0, local_y, leaf_x0, leaf_y0),
            |local_x| self.v_predictor_for_score(plane, x0, y0, local_x, leaf_x0, leaf_y0),
            || self.above_left_predictor_for_score(plane, x0, y0, leaf_x0, leaf_y0),
            |angle, local_x, local_y| {
                self.directional_predictor_for_score(
                    plane,
                    x0,
                    y0,
                    angle,
                    local_x,
                    local_y,
                    leaf_x0,
                    leaf_y0,
                    leaf_width,
                    leaf_height,
                    coded_mi_context,
                )
            },
            || {
                self.smooth_edges_for_score(
                    plane,
                    x0,
                    y0,
                    leaf_x0,
                    leaf_y0,
                    leaf_width,
                    leaf_height,
                    coded_mi_context,
                )
            },
        )
    }

    fn dpcm_residual4x4_for_score(
        &self,
        plane: Av2LosslessPlane,
        x0: usize,
        y0: usize,
        horz: bool,
        leaf_x0: usize,
        leaf_y0: usize,
    ) -> [i32; TX4X4_SAMPLES] {
        let mut residual = [0i32; TX4X4_SAMPLES];
        for local_y in 0..TX4X4_SIZE {
            for local_x in 0..TX4X4_SIZE {
                let x = x0 + local_x;
                let y = y0 + local_y;
                let sample = i32::from(self.source_sample(plane, x, y));
                let predicted_delta = if horz {
                    if local_x == 0 {
                        sample
                            - i32::from(
                                self.h_predictor_for_score(
                                    plane, x0, y0, local_y, leaf_x0, leaf_y0,
                                ),
                            )
                    } else {
                        sample - i32::from(self.source_sample(plane, x - 1, y))
                    }
                } else if local_y == 0 {
                    sample
                        - i32::from(
                            self.v_predictor_for_score(plane, x0, y0, local_x, leaf_x0, leaf_y0),
                        )
                } else {
                    sample - i32::from(self.source_sample(plane, x, y - 1))
                };
                residual[local_y * TX4X4_SIZE + local_x] = predicted_delta;
            }
        }
        residual
    }

    fn dc_h_v_bdpcm_txb_scores_for_score(
        &self,
        plane: Av2LosslessPlane,
        x0: usize,
        y0: usize,
        leaf_x0: usize,
        leaf_y0: usize,
        kind: Av2CoefficientProxyKind,
    ) -> Av2DcHvBdpcmTxbScores {
        let source = self.source_block4x4(plane, x0, y0);
        let dc = i32::from(self.dc_predictor_for_score(plane, x0, y0, leaf_x0, leaf_y0));
        let mut h_pred = [0i32; TX4X4_SIZE];
        let mut v_pred = [0i32; TX4X4_SIZE];
        for index in 0..TX4X4_SIZE {
            h_pred[index] = i32::from(self.h_predictor_for_score(
                plane, x0, y0, index, leaf_x0, leaf_y0,
            ));
            v_pred[index] = i32::from(self.v_predictor_for_score(
                plane, x0, y0, index, leaf_x0, leaf_y0,
            ));
        }

        let mut dc_residual = [0i32; TX4X4_SAMPLES];
        let mut horizontal_residual = [0i32; TX4X4_SAMPLES];
        let mut vertical_residual = [0i32; TX4X4_SAMPLES];
        let mut bdpcm_horizontal_residual = [0i32; TX4X4_SAMPLES];
        let mut bdpcm_vertical_residual = [0i32; TX4X4_SAMPLES];
        for local_y in 0..TX4X4_SIZE {
            for local_x in 0..TX4X4_SIZE {
                let pos = local_y * TX4X4_SIZE + local_x;
                let sample = source[pos];
                dc_residual[pos] = sample - dc;
                horizontal_residual[pos] = sample - h_pred[local_y];
                vertical_residual[pos] = sample - v_pred[local_x];
                bdpcm_horizontal_residual[pos] = if local_x == 0 {
                    sample - h_pred[local_y]
                } else {
                    sample - source[pos - 1]
                };
                bdpcm_vertical_residual[pos] = if local_y == 0 {
                    sample - v_pred[local_x]
                } else {
                    sample - source[pos - TX4X4_SIZE]
                };
            }
        }

        Av2DcHvBdpcmTxbScores {
            dc: residual_sample_proxy_score(&dc_residual, kind),
            horizontal: residual_sample_proxy_score(&horizontal_residual, kind),
            vertical: residual_sample_proxy_score(&vertical_residual, kind),
            bdpcm_horizontal: residual_sample_proxy_score(&bdpcm_horizontal_residual, kind),
            bdpcm_vertical: residual_sample_proxy_score(&bdpcm_vertical_residual, kind),
        }
    }

    fn dc_predictor_for_score(
        &self,
        plane: Av2LosslessPlane,
        x0: usize,
        y0: usize,
        leaf_x0: usize,
        leaf_y0: usize,
    ) -> Av2Sample {
        let edge_sample =
            |plane, x, y| self.neighbor_sample_for_score(plane, x, y, leaf_x0, leaf_y0);
        self.dc_predictor_with(plane, x0, y0, &edge_sample)
    }

    fn h_predictor_for_score(
        &self,
        plane: Av2LosslessPlane,
        x0: usize,
        y0: usize,
        local_y: usize,
        leaf_x0: usize,
        leaf_y0: usize,
    ) -> Av2Sample {
        let edge_sample =
            |plane, x, y| self.neighbor_sample_for_score(plane, x, y, leaf_x0, leaf_y0);
        self.h_predictor_with(plane, x0, y0, local_y, &edge_sample)
    }

    fn v_predictor_for_score(
        &self,
        plane: Av2LosslessPlane,
        x0: usize,
        y0: usize,
        local_x: usize,
        leaf_x0: usize,
        leaf_y0: usize,
    ) -> Av2Sample {
        let edge_sample =
            |plane, x, y| self.neighbor_sample_for_score(plane, x, y, leaf_x0, leaf_y0);
        self.v_predictor_with(plane, x0, y0, local_x, &edge_sample)
    }

    fn luma_directional_idif_residual4x4(
        &self,
        x0: usize,
        y0: usize,
        angle: i16,
        leaf_x0: usize,
        leaf_y0: usize,
        leaf_width: usize,
        leaf_height: usize,
        coded_mi_context: &Av2CodedMiContext,
    ) -> [i32; TX4X4_SAMPLES] {
        let edge_sample = |plane, x, y| self.recon_sample(plane, x, y);
        let source_sample = |plane, x, y| self.source_sample(plane, x, y);
        self.luma_directional_idif_residual4x4_with(
            x0,
            y0,
            angle,
            leaf_x0,
            leaf_y0,
            leaf_width,
            leaf_height,
            coded_mi_context,
            edge_sample,
            source_sample,
        )
    }

    fn luma_directional_idif_residual4x4_for_score(
        &self,
        x0: usize,
        y0: usize,
        angle: i16,
        leaf_x0: usize,
        leaf_y0: usize,
        leaf_width: usize,
        leaf_height: usize,
        coded_mi_context: &Av2CodedMiContext,
    ) -> [i32; TX4X4_SAMPLES] {
        let edge_sample =
            |plane, x, y| self.neighbor_sample_for_score(plane, x, y, leaf_x0, leaf_y0);
        let source_sample = |plane, x, y| self.source_sample(plane, x, y);
        self.luma_directional_idif_residual4x4_with(
            x0,
            y0,
            angle,
            leaf_x0,
            leaf_y0,
            leaf_width,
            leaf_height,
            coded_mi_context,
            edge_sample,
            source_sample,
        )
    }

    fn luma_directional_idif_residual4x4_with<EdgeSample, SourceSample>(
        &self,
        x0: usize,
        y0: usize,
        angle: i16,
        leaf_x0: usize,
        leaf_y0: usize,
        leaf_width: usize,
        leaf_height: usize,
        coded_mi_context: &Av2CodedMiContext,
        edge_sample: EdgeSample,
        source_sample: SourceSample,
    ) -> [i32; TX4X4_SAMPLES]
    where
        EdgeSample: Fn(Av2LosslessPlane, usize, usize) -> Av2Sample,
        SourceSample: Fn(Av2LosslessPlane, usize, usize) -> Av2Sample,
    {
        let (tile_origin_x, tile_origin_y) = self.plane_origin(Av2LosslessPlane::Y);
        let have_top = y0 > tile_origin_y;
        let have_left = x0 > tile_origin_x;
        let base = av2_lossless_dc_predictor(self.bit_depth);

        let constant_predictor = match angle {
            1..=89 if !have_top => Some(if have_left {
                edge_sample(Av2LosslessPlane::Y, x0 - 1, y0)
            } else {
                base.saturating_sub(1)
            }),
            181..=269 if !have_left => Some(if have_top {
                edge_sample(Av2LosslessPlane::Y, x0, y0 - 1)
            } else {
                base.saturating_add(1)
            }),
            _ => None,
        };

        let edges = constant_predictor.is_none().then(|| {
            self.luma_directional_idif_edges_with(
                x0,
                y0,
                angle,
                leaf_x0,
                leaf_y0,
                leaf_width,
                leaf_height,
                coded_mi_context,
                &edge_sample,
            )
        });

        let mut residual = [0i32; TX4X4_SAMPLES];
        for local_y in 0..TX4X4_SIZE {
            for local_x in 0..TX4X4_SIZE {
                let predictor = constant_predictor.unwrap_or_else(|| {
                    luma_directional_idif_predictor(
                        angle,
                        edges.expect("IDIF edges are precomputed"),
                        local_x,
                        local_y,
                        self.bit_depth,
                    )
                });
                residual[local_y * TX4X4_SIZE + local_x] = i32::from(source_sample(
                    Av2LosslessPlane::Y,
                    x0 + local_x,
                    y0 + local_y,
                )) - i32::from(predictor);
            }
        }
        residual
    }

    fn luma_directional_idif_edges_with<EdgeSample>(
        &self,
        x0: usize,
        y0: usize,
        angle: i16,
        leaf_x0: usize,
        leaf_y0: usize,
        leaf_width: usize,
        leaf_height: usize,
        coded_mi_context: &Av2CodedMiContext,
        edge_sample: &EdgeSample,
    ) -> DirectionalIdifEdges
    where
        EdgeSample: Fn(Av2LosslessPlane, usize, usize) -> Av2Sample,
    {
        let above_core = self.directional_above_edge_with(
            Av2LosslessPlane::Y,
            x0,
            y0,
            leaf_x0,
            leaf_y0,
            leaf_width,
            coded_mi_context,
            edge_sample,
        );
        let left_core = self.directional_left_edge_with(
            Av2LosslessPlane::Y,
            x0,
            y0,
            leaf_x0,
            leaf_y0,
            leaf_height,
            coded_mi_context,
            edge_sample,
        );
        let above_left = self.above_left_predictor_with(Av2LosslessPlane::Y, x0, y0, edge_sample);
        let mut edges = DirectionalIdifEdges::new(self.bit_depth);
        edges.set_above(-2, above_left);
        edges.set_above(-1, above_left);
        edges.set_left(-2, above_left);
        edges.set_left(-1, above_left);
        for index in 0..8 {
            edges.set_above(index as i32, above_core[index]);
            edges.set_left(index as i32, left_core[index]);
        }
        if angle > 90 && angle < 180 {
            for index in TX4X4_SIZE..8 {
                edges.set_above(index as i32, above_core[TX4X4_SIZE - 1]);
                edges.set_left(index as i32, left_core[TX4X4_SIZE - 1]);
            }
        }
        edges.set_above(8, edges.above(7));
        edges.set_above(9, edges.above(7));
        edges.set_left(8, edges.left(7));
        edges.set_left(9, edges.left(7));
        edges
    }

    fn above_left_predictor_with<EdgeSample>(
        &self,
        plane: Av2LosslessPlane,
        x0: usize,
        y0: usize,
        edge_sample: &EdgeSample,
    ) -> Av2Sample
    where
        EdgeSample: Fn(Av2LosslessPlane, usize, usize) -> Av2Sample,
    {
        let (tile_origin_x, tile_origin_y) = self.plane_origin(plane);
        let have_left = x0 > tile_origin_x;
        let have_top = y0 > tile_origin_y;
        if have_left && have_top {
            edge_sample(plane, x0 - 1, y0 - 1)
        } else if have_top {
            edge_sample(plane, x0, y0 - 1)
        } else if have_left {
            edge_sample(plane, x0 - 1, y0)
        } else {
            av2_lossless_dc_predictor(self.bit_depth)
        }
    }

    fn directional_above_edge_with<EdgeSample>(
        &self,
        plane: Av2LosslessPlane,
        x0: usize,
        y0: usize,
        leaf_x0: usize,
        leaf_y0: usize,
        leaf_width: usize,
        coded_mi_context: &Av2CodedMiContext,
        edge_sample: &EdgeSample,
    ) -> [Av2Sample; 8]
    where
        EdgeSample: Fn(Av2LosslessPlane, usize, usize) -> Av2Sample,
    {
        let (tile_origin_x, tile_origin_y) = self.plane_origin(plane);
        let (plane_width, _) = self.plane_geometry(plane);
        let (sub_x, sub_y) = self.plane_subsampling(plane);
        let have_top = y0 > tile_origin_y;
        let have_left = x0 > tile_origin_x;
        let mut above = [av2_lossless_v_pred_above_edge(self.bit_depth); 8];
        if have_top {
            let plane_sb_width = MVP_SUPERBLOCK_SIZE / sub_x;
            let plane_sb_height = MVP_SUPERBLOCK_SIZE / sub_y;
            let sb_origin_x = (x0 / plane_sb_width) * plane_sb_width;
            let sb_right = (sb_origin_x + plane_sb_width).min(plane_width);
            let superblock_top_row = y0 % plane_sb_height == 0;
            for index in 0..above.len() {
                let x = x0 + index;
                let overhang = index >= TX4X4_SIZE;
                let external_top_right_coded = overhang && y0 == leaf_y0 && x < plane_width && {
                    let (row_mi, col_mi) = self.coded_mi_for_plane_sample(plane, x, y0 - 1);
                    superblock_top_row
                        || (x < sb_right && coded_mi_context.is_coded(row_mi, col_mi))
                };
                if x < plane_width
                    && (!overhang || x < leaf_x0 + leaf_width || external_top_right_coded)
                {
                    above[index] = edge_sample(plane, x, y0 - 1);
                } else if index > 0 {
                    above[index] = above[index - 1];
                }
            }
        } else if have_left {
            above.fill(edge_sample(plane, x0 - 1, y0));
        }
        above
    }

    fn directional_left_edge_with<EdgeSample>(
        &self,
        plane: Av2LosslessPlane,
        x0: usize,
        y0: usize,
        leaf_x0: usize,
        leaf_y0: usize,
        leaf_height: usize,
        coded_mi_context: &Av2CodedMiContext,
        edge_sample: &EdgeSample,
    ) -> [Av2Sample; 8]
    where
        EdgeSample: Fn(Av2LosslessPlane, usize, usize) -> Av2Sample,
    {
        let (tile_origin_x, tile_origin_y) = self.plane_origin(plane);
        let (_, plane_height) = self.plane_geometry(plane);
        let (sub_x, sub_y) = self.plane_subsampling(plane);
        let have_top = y0 > tile_origin_y;
        let have_left = x0 > tile_origin_x;
        let mut left = [av2_lossless_h_pred_left_edge(self.bit_depth); 8];
        if have_left {
            let plane_sb_width = MVP_SUPERBLOCK_SIZE / sub_x;
            let plane_sb_height = MVP_SUPERBLOCK_SIZE / sub_y;
            let sb_origin_y = (y0 / plane_sb_height) * plane_sb_height;
            let sb_bottom = (sb_origin_y + plane_sb_height).min(plane_height);
            let superblock_left_col = x0 % plane_sb_width == 0;
            for index in 0..left.len() {
                let y = y0 + index;
                let overhang = index >= TX4X4_SIZE;
                let external_bottom_left_coded = overhang && x0 == leaf_x0 && y < sb_bottom && {
                    let (row_mi, col_mi) = self.coded_mi_for_plane_sample(plane, x0 - 1, y);
                    superblock_left_col || coded_mi_context.is_coded(row_mi, col_mi)
                };
                if y < plane_height
                    && (!overhang
                        || (x0 == leaf_x0
                            && (y < leaf_y0 + leaf_height || external_bottom_left_coded)))
                {
                    left[index] = edge_sample(plane, x0 - 1, y);
                } else if index > 0 {
                    left[index] = left[index - 1];
                }
            }
        } else if have_top {
            left.fill(edge_sample(plane, x0, y0 - 1));
        }
        left
    }

    fn smooth_edges(
        &self,
        plane: Av2LosslessPlane,
        x0: usize,
        y0: usize,
        leaf_x0: usize,
        leaf_y0: usize,
        leaf_width: usize,
        leaf_height: usize,
        coded_mi_context: &Av2CodedMiContext,
    ) -> ([Av2Sample; TX4X4_SIZE + 1], [Av2Sample; TX4X4_SIZE + 1]) {
        let edge_sample = |plane, x, y| self.recon_sample(plane, x, y);
        self.smooth_edges_with(
            plane,
            x0,
            y0,
            leaf_x0,
            leaf_y0,
            leaf_width,
            leaf_height,
            coded_mi_context,
            &edge_sample,
        )
    }

    fn smooth_edges_for_score(
        &self,
        plane: Av2LosslessPlane,
        x0: usize,
        y0: usize,
        leaf_x0: usize,
        leaf_y0: usize,
        leaf_width: usize,
        leaf_height: usize,
        coded_mi_context: &Av2CodedMiContext,
    ) -> ([Av2Sample; TX4X4_SIZE + 1], [Av2Sample; TX4X4_SIZE + 1]) {
        let edge_sample =
            |plane, x, y| self.neighbor_sample_for_score(plane, x, y, leaf_x0, leaf_y0);
        self.smooth_edges_with(
            plane,
            x0,
            y0,
            leaf_x0,
            leaf_y0,
            leaf_width,
            leaf_height,
            coded_mi_context,
            &edge_sample,
        )
    }

    fn smooth_edges_with<EdgeSample>(
        &self,
        plane: Av2LosslessPlane,
        x0: usize,
        y0: usize,
        leaf_x0: usize,
        leaf_y0: usize,
        leaf_width: usize,
        leaf_height: usize,
        coded_mi_context: &Av2CodedMiContext,
        edge_sample: &EdgeSample,
    ) -> ([Av2Sample; TX4X4_SIZE + 1], [Av2Sample; TX4X4_SIZE + 1])
    where
        EdgeSample: Fn(Av2LosslessPlane, usize, usize) -> Av2Sample,
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
        let external_top_right_coded = have_top && y0 == leaf_y0 && top_right_x < plane_width && {
            let (row_mi, col_mi) = self.coded_mi_for_plane_sample(plane, top_right_x, y0 - 1);
            superblock_top_row
                || (top_right_x < sb_right && coded_mi_context.is_coded(row_mi, col_mi))
        };
        if have_top
            && top_right_x < plane_width
            && (top_right_x < leaf_x0 + leaf_width || external_top_right_coded)
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
            have_left && x0 == leaf_x0 && bottom_left_y < sb_bottom && {
                let (row_mi, col_mi) = self.coded_mi_for_plane_sample(plane, x0 - 1, bottom_left_y);
                superblock_left_col || coded_mi_context.is_coded(row_mi, col_mi)
            };
        if have_left
            && x0 == leaf_x0
            && bottom_left_y < plane_height
            && (bottom_left_y < leaf_y0 + leaf_height || external_bottom_left_coded)
        {
            left[TX4X4_SIZE] = edge_sample(plane, x0 - 1, bottom_left_y);
        } else {
            left[TX4X4_SIZE] = left[TX4X4_SIZE - 1];
        }

        (above, left)
    }

    fn above_left_predictor(&self, plane: Av2LosslessPlane, x0: usize, y0: usize) -> Av2Sample {
        let edge_sample = |plane, x, y| self.recon_sample(plane, x, y);
        self.above_left_predictor_with(plane, x0, y0, &edge_sample)
    }

    fn above_left_predictor_for_score(
        &self,
        plane: Av2LosslessPlane,
        x0: usize,
        y0: usize,
        leaf_x0: usize,
        leaf_y0: usize,
    ) -> Av2Sample {
        let edge_sample =
            |plane, x, y| self.neighbor_sample_for_score(plane, x, y, leaf_x0, leaf_y0);
        self.above_left_predictor_with(plane, x0, y0, &edge_sample)
    }

    fn neighbor_sample_for_score(
        &self,
        plane: Av2LosslessPlane,
        x: usize,
        y: usize,
        leaf_x0: usize,
        leaf_y0: usize,
    ) -> Av2Sample {
        if x >= leaf_x0 && y >= leaf_y0 {
            self.source_sample(plane, x, y)
        } else {
            self.recon_sample(plane, x, y)
        }
    }

    fn mode_decision_for_leaf(
        &self,
        decision: Av2TileDecision,
        visible_rows_mi: usize,
        visible_cols_mi: usize,
        coded_mi_context: &Av2CodedMiContext,
    ) -> Av2LosslessSubsampledModeDecision {
        if self.mode_search == Av2LosslessSubsampledModeSearch::FastScreenContent {
            return self.fast_mode_decision_for_leaf(
                decision,
                visible_rows_mi,
                visible_cols_mi,
                coded_mi_context,
            );
        }
        let txb_width = decision
            .block_size
            .tx4x4_width()
            .min(visible_cols_mi.saturating_sub(decision.col));
        let txb_height = decision
            .block_size
            .tx4x4_height()
            .min(visible_rows_mi.saturating_sub(decision.row));
        let chroma_span = chroma_tx4x4_span(
            decision,
            visible_rows_mi,
            visible_cols_mi,
            self.chroma_format,
        );
        // The AVM path validates 32-wide/high transform-coded leaves, but
        // 32-wide/high FSC/IDTX leaves can corrupt natural-content tiles with
        // the current coefficient writer. Keep FSC to smaller leaves until the
        // 32xN IDTX path is audited end to end.
        let fsc_allowed = decision.block_size.fsc_size_group().is_some()
            && decision.block_size.width < 32
            && decision.block_size.height < 32;
        let mut best = (Av2LosslessSubsampledModeDecision::default(), usize::MAX);

        for use_fsc in [false, true] {
            if use_fsc && !fsc_allowed {
                continue;
            }
            let luma_candidates = [
                (Av2LumaIntraMode::Dc, None, 0usize),
                (Av2LumaIntraMode::Smooth, None, 192usize),
                (Av2LumaIntraMode::SmoothVertical, None, 192usize),
                (Av2LumaIntraMode::SmoothHorizontal, None, 192usize),
                (Av2LumaIntraMode::Paeth, None, 128usize),
                (Av2LumaIntraMode::Directional45, None, 192usize),
                (Av2LumaIntraMode::Directional67, None, 192usize),
                (Av2LumaIntraMode::Horizontal, None, 32usize),
                (Av2LumaIntraMode::Vertical, None, 32usize),
                (Av2LumaIntraMode::Directional113, None, 192usize),
                (Av2LumaIntraMode::Directional135, None, 192usize),
                (Av2LumaIntraMode::Directional157, None, 192usize),
                (Av2LumaIntraMode::Directional203, None, 192usize),
                (Av2LumaIntraMode::Horizontal, Some(true), 64usize),
                (Av2LumaIntraMode::Vertical, Some(false), 64usize),
            ];
            // Chroma BDPCM is reference-clean in the transform-coded base
            // mode search. FSC/IDTX pairings and luma directional-delta
            // pairings diverge from AVM on natural 4:2:0 content.
            let chroma_bdpcm_allowed = !use_fsc;
            let chroma_candidates = [
                (false, Av2ChromaIntraMode::Horizontal, 0usize),
                (false, Av2ChromaIntraMode::Vertical, 0usize),
                (false, Av2ChromaIntraMode::Dc, 0usize),
                (false, Av2ChromaIntraMode::Directional45, 192usize),
                (false, Av2ChromaIntraMode::Directional135, 192usize),
                (false, Av2ChromaIntraMode::Directional67, 192usize),
                (false, Av2ChromaIntraMode::Directional203, 192usize),
                (false, Av2ChromaIntraMode::Directional113, 192usize),
                (false, Av2ChromaIntraMode::Directional157, 192usize),
                (false, Av2ChromaIntraMode::Smooth, 192usize),
                (false, Av2ChromaIntraMode::SmoothVertical, 192usize),
                (false, Av2ChromaIntraMode::SmoothHorizontal, 192usize),
                (false, Av2ChromaIntraMode::Paeth, 128usize),
                (true, Av2ChromaIntraMode::Horizontal, 64usize),
                (true, Av2ChromaIntraMode::Vertical, 64usize),
            ];

            for (luma_intra_mode, luma_bdpcm_horz, luma_syntax_penalty) in luma_candidates {
                for (chroma_use_bdpcm, chroma_intra_mode, chroma_syntax_penalty) in
                    chroma_candidates
                {
                    if chroma_use_bdpcm && !chroma_bdpcm_allowed {
                        continue;
                    }
                    let mode = Av2LosslessSubsampledModeDecision {
                        luma_intra_mode,
                        luma_bdpcm_horz,
                        chroma_use_bdpcm,
                        chroma_intra_mode,
                        use_fsc,
                    };
                    let fsc_syntax_penalty = usize::from(use_fsc) * 96;
                    let score =
                        self.luma_leaf_coefficient_score(
                            decision,
                            txb_width,
                            txb_height,
                            mode,
                            coded_mi_context,
                        ) + self.chroma_leaf_coefficient_score(chroma_span, mode, coded_mi_context)
                            + luma_syntax_penalty
                            + chroma_syntax_penalty
                            + fsc_syntax_penalty;
                    if score < best.1 {
                        best = (mode, score);
                    }
                }
            }

            for base in [
                Av2LumaDirectionalMode::Directional45,
                Av2LumaDirectionalMode::Directional67,
                Av2LumaDirectionalMode::Vertical,
                Av2LumaDirectionalMode::Directional113,
                Av2LumaDirectionalMode::Directional135,
                Av2LumaDirectionalMode::Directional157,
                Av2LumaDirectionalMode::Horizontal,
                Av2LumaDirectionalMode::Directional203,
            ] {
                for delta in [-1i8, 1, -2, 2, -3, 3] {
                    let luma_intra_mode = Av2LumaIntraMode::DirectionalDelta { base, delta };
                    let luma_syntax_penalty = 224usize + usize::from(delta.unsigned_abs()) * 48;
                    for (chroma_use_bdpcm, chroma_intra_mode, chroma_syntax_penalty) in
                        chroma_candidates
                    {
                        if chroma_use_bdpcm && !chroma_bdpcm_allowed {
                            continue;
                        }
                        // See the chroma_bdpcm_allowed comment above.
                        if chroma_use_bdpcm {
                            continue;
                        }
                        let mode = Av2LosslessSubsampledModeDecision {
                            luma_intra_mode,
                            luma_bdpcm_horz: None,
                            chroma_use_bdpcm,
                            chroma_intra_mode,
                            use_fsc,
                        };
                        let fsc_syntax_penalty = usize::from(use_fsc) * 96;
                        let score = self.luma_leaf_coefficient_score(
                            decision,
                            txb_width,
                            txb_height,
                            mode,
                            coded_mi_context,
                        ) + self.chroma_leaf_coefficient_score(
                            chroma_span,
                            mode,
                            coded_mi_context,
                        ) + luma_syntax_penalty
                            + chroma_syntax_penalty
                            + fsc_syntax_penalty;
                        if score < best.1 {
                            best = (mode, score);
                        }
                    }
                }
            }
        }

        best.0
    }

    fn fast_mode_decision_for_leaf(
        &self,
        decision: Av2TileDecision,
        visible_rows_mi: usize,
        visible_cols_mi: usize,
        _coded_mi_context: &Av2CodedMiContext,
    ) -> Av2LosslessSubsampledModeDecision {
        let txb_width = decision
            .block_size
            .tx4x4_width()
            .min(visible_cols_mi.saturating_sub(decision.col));
        let txb_height = decision
            .block_size
            .tx4x4_height()
            .min(visible_rows_mi.saturating_sub(decision.row));
        let chroma_span = chroma_tx4x4_span(
            decision,
            visible_rows_mi,
            visible_cols_mi,
            self.chroma_format,
        );
        let mut mode = Av2LosslessSubsampledModeDecision::default();

        let luma_candidates = [
            (Av2LumaIntraMode::Dc, None, 0usize),
            (Av2LumaIntraMode::Horizontal, None, 32usize),
            (Av2LumaIntraMode::Vertical, None, 32usize),
            (Av2LumaIntraMode::Horizontal, Some(true), 64usize),
            (Av2LumaIntraMode::Vertical, Some(false), 64usize),
        ];
        let luma_scores =
            self.fast_luma_leaf_sampled_dc_h_v_bdpcm_scores(decision, txb_width, txb_height);
        let mut best_luma = (mode.luma_intra_mode, mode.luma_bdpcm_horz, usize::MAX);
        for (luma_intra_mode, luma_bdpcm_horz, syntax_penalty) in luma_candidates {
            let score = match (luma_intra_mode, luma_bdpcm_horz) {
                (Av2LumaIntraMode::Dc, None) => luma_scores.dc,
                (Av2LumaIntraMode::Horizontal, None) => luma_scores.horizontal,
                (Av2LumaIntraMode::Vertical, None) => luma_scores.vertical,
                (Av2LumaIntraMode::Horizontal, Some(true)) => luma_scores.bdpcm_horizontal,
                (Av2LumaIntraMode::Vertical, Some(false)) => luma_scores.bdpcm_vertical,
                _ => unreachable!("fast luma mode search only scores DC/H/V and BDPCM"),
            } + syntax_penalty;
            if score < best_luma.2 {
                best_luma = (luma_intra_mode, luma_bdpcm_horz, score);
            }
        }
        mode.luma_intra_mode = best_luma.0;
        mode.luma_bdpcm_horz = best_luma.1;

        let chroma_candidates = [
            (false, Av2ChromaIntraMode::Horizontal, 0usize),
            (false, Av2ChromaIntraMode::Vertical, 0usize),
            (false, Av2ChromaIntraMode::Dc, 0usize),
            (true, Av2ChromaIntraMode::Horizontal, 64usize),
            (true, Av2ChromaIntraMode::Vertical, 64usize),
        ];
        let chroma_scores =
            self.fast_chroma_leaf_sampled_dc_h_v_bdpcm_scores(chroma_span);
        let mut best_chroma = (mode.chroma_use_bdpcm, mode.chroma_intra_mode, usize::MAX);
        for (chroma_use_bdpcm, chroma_intra_mode, syntax_penalty) in chroma_candidates {
            let score = match (chroma_use_bdpcm, chroma_intra_mode) {
                (false, Av2ChromaIntraMode::Horizontal) => chroma_scores.horizontal,
                (false, Av2ChromaIntraMode::Vertical) => chroma_scores.vertical,
                (false, Av2ChromaIntraMode::Dc) => chroma_scores.dc,
                (true, Av2ChromaIntraMode::Horizontal) => chroma_scores.bdpcm_horizontal,
                (true, Av2ChromaIntraMode::Vertical) => chroma_scores.bdpcm_vertical,
                _ => unreachable!("fast chroma mode search only scores DC/H/V and BDPCM"),
            } + syntax_penalty;
            if score < best_chroma.2 {
                best_chroma = (chroma_use_bdpcm, chroma_intra_mode, score);
            }
        }
        mode.chroma_use_bdpcm = best_chroma.0;
        mode.chroma_intra_mode = best_chroma.1;
        mode
    }

    fn fast_luma_leaf_sampled_dc_h_v_bdpcm_scores(
        &self,
        decision: Av2TileDecision,
        txb_width: usize,
        txb_height: usize,
    ) -> Av2DcHvBdpcmTxbScores {
        let mut scores = Av2DcHvBdpcmTxbScores::default();
        let (leaf_x0, leaf_y0) = self.txb_origin(Av2LosslessPlane::Y, decision.col, decision.row);
        let row_step = fast_leaf_sample_step(txb_height, AV2_FAST_LUMA_SAMPLE_GRID);
        let col_step = fast_leaf_sample_step(txb_width, AV2_FAST_LUMA_SAMPLE_GRID);
        for row in (0..txb_height).step_by(row_step) {
            let abs_row = decision.row + row;
            for col in (0..txb_width).step_by(col_step) {
                let abs_col = decision.col + col;
                let (x0, y0) = self.txb_origin(Av2LosslessPlane::Y, abs_col, abs_row);
                scores.add_assign(self.dc_h_v_bdpcm_txb_scores_for_score(
                    Av2LosslessPlane::Y,
                    x0,
                    y0,
                    leaf_x0,
                    leaf_y0,
                    Av2CoefficientProxyKind::LumaTransform,
                ));
            }
        }
        scores
    }

    fn fast_chroma_leaf_sampled_dc_h_v_bdpcm_scores(
        &self,
        chroma_span: Av2ChromaTx4x4Span,
    ) -> Av2DcHvBdpcmTxbScores {
        let mut scores = Av2DcHvBdpcmTxbScores::default();
        let row_step = fast_leaf_sample_step(chroma_span.height, AV2_FAST_CHROMA_SAMPLE_GRID);
        let col_step = fast_leaf_sample_step(chroma_span.width, AV2_FAST_CHROMA_SAMPLE_GRID);
        for plane in [Av2LosslessPlane::U, Av2LosslessPlane::V] {
            let (leaf_x0, leaf_y0) = self.txb_origin(plane, chroma_span.col, chroma_span.row);
            for row in (0..chroma_span.height).step_by(row_step) {
                let abs_row = chroma_span.row + row;
                for col in (0..chroma_span.width).step_by(col_step) {
                    let abs_col = chroma_span.col + col;
                    let (x0, y0) = self.txb_origin(plane, abs_col, abs_row);
                    scores.add_assign(self.dc_h_v_bdpcm_txb_scores_for_score(
                        plane,
                        x0,
                        y0,
                        leaf_x0,
                        leaf_y0,
                        Av2CoefficientProxyKind::ChromaTransform,
                    ));
                }
            }
        }
        scores
    }

    fn luma_leaf_coefficient_score(
        &self,
        decision: Av2TileDecision,
        txb_width: usize,
        txb_height: usize,
        mode: Av2LosslessSubsampledModeDecision,
        coded_mi_context: &Av2CodedMiContext,
    ) -> usize {
        let mut score = 0usize;
        let (leaf_x0, leaf_y0) = self.txb_origin(Av2LosslessPlane::Y, decision.col, decision.row);
        let leaf_width = txb_width * TX4X4_SIZE;
        let leaf_height = txb_height * TX4X4_SIZE;
        for row in 0..txb_height {
            let abs_row = decision.row + row;
            for col in 0..txb_width {
                let abs_col = decision.col + col;
                let (x0, y0) = self.txb_origin(Av2LosslessPlane::Y, abs_col, abs_row);
                let coefficients = self.tx4x4_coefficients_for_mode_score(
                    Av2LosslessPlane::Y,
                    x0,
                    y0,
                    mode,
                    leaf_x0,
                    leaf_y0,
                    leaf_width,
                    leaf_height,
                    coded_mi_context,
                );
                let kind = if mode.use_fsc {
                    Av2CoefficientProxyKind::LumaIdtx
                } else {
                    Av2CoefficientProxyKind::LumaTransform
                };
                score += coefficient_proxy_score(&coefficients, kind);
            }
        }
        score
    }

    fn chroma_leaf_coefficient_score(
        &self,
        chroma_span: Av2ChromaTx4x4Span,
        mode: Av2LosslessSubsampledModeDecision,
        coded_mi_context: &Av2CodedMiContext,
    ) -> usize {
        let mut score = 0usize;
        for plane in [Av2LosslessPlane::U, Av2LosslessPlane::V] {
            let (leaf_x0, leaf_y0) = self.txb_origin(plane, chroma_span.col, chroma_span.row);
            let leaf_width = chroma_span.width * TX4X4_SIZE;
            let leaf_height = chroma_span.height * TX4X4_SIZE;
            for row in 0..chroma_span.height {
                let abs_row = chroma_span.row + row;
                for col in 0..chroma_span.width {
                    let abs_col = chroma_span.col + col;
                    let (x0, y0) = self.txb_origin(plane, abs_col, abs_row);
                    let coefficients = self.tx4x4_coefficients_for_mode_score(
                        plane,
                        x0,
                        y0,
                        mode,
                        leaf_x0,
                        leaf_y0,
                        leaf_width,
                        leaf_height,
                        coded_mi_context,
                    );
                    score += coefficient_proxy_score(
                        &coefficients,
                        Av2CoefficientProxyKind::ChromaTransform,
                    );
                }
            }
        }
        score
    }

    fn copy_source_to_recon_txb(&mut self, plane: Av2LosslessPlane, x0: usize, y0: usize) {
        let (plane_width, plane_height) = self.plane_geometry(plane);
        let bytes_per_sample = self.bit_depth.bytes_per_sample();
        for local_y in 0..TX4X4_SIZE {
            let y = y0 + local_y;
            if y >= plane_height {
                continue;
            }
            let row_samples = TX4X4_SIZE.min(plane_width.saturating_sub(x0));
            let offset = self.offset(plane, x0, y) * bytes_per_sample;
            let row_bytes = row_samples * bytes_per_sample;
            self.recon[offset..offset + row_bytes]
                .copy_from_slice(&self.source[offset..offset + row_bytes]);
        }
    }

    fn copy_source_to_recon_leaf(
        &mut self,
        decision: Av2TileDecision,
        visible_rows_mi: usize,
        visible_cols_mi: usize,
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
                let (x0, y0) = self.txb_origin(Av2LosslessPlane::Y, abs_col, abs_row);
                self.copy_source_to_recon_txb(Av2LosslessPlane::Y, x0, y0);
            }
        }

        let chroma_span = chroma_tx4x4_span(
            decision,
            visible_rows_mi,
            visible_cols_mi,
            self.chroma_format,
        );
        for plane in [Av2LosslessPlane::U, Av2LosslessPlane::V] {
            for row in 0..chroma_span.height {
                let abs_row = chroma_span.row + row;
                for col in 0..chroma_span.width {
                    let abs_col = chroma_span.col + col;
                    let (x0, y0) = self.txb_origin(plane, abs_col, abs_row);
                    self.copy_source_to_recon_txb(plane, x0, y0);
                }
            }
        }
    }
}

const AV2_FAST_LUMA_SAMPLE_GRID: usize = 8;
const AV2_FAST_CHROMA_SAMPLE_GRID: usize = 4;

fn fast_leaf_sample_step(txb_count: usize, sample_grid: usize) -> usize {
    if txb_count <= 2 {
        return txb_count.max(1);
    }
    txb_count.div_ceil(sample_grid).max(1)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Av2LosslessPlane {
    Y,
    U,
    V,
}

fn chroma_subsample_x(chroma_format: Av2ChromaFormat) -> usize {
    match chroma_format {
        Av2ChromaFormat::Yuv420 | Av2ChromaFormat::Yuv422 => 2,
        Av2ChromaFormat::Yuv444 => 1,
    }
}

fn chroma_subsample_y(chroma_format: Av2ChromaFormat) -> usize {
    match chroma_format {
        Av2ChromaFormat::Yuv420 => 2,
        Av2ChromaFormat::Yuv422 | Av2ChromaFormat::Yuv444 => 1,
    }
}

fn read_validated_planar_sample(
    buffer: &[u8],
    sample_index: usize,
    bit_depth: SampleBitDepth,
) -> Av2Sample {
    if bit_depth.bits() <= 8 {
        debug_assert!(sample_index < buffer.len());
        return Av2Sample::from(buffer[sample_index]);
    }

    let offset = sample_index * 2;
    debug_assert!(offset + 1 < buffer.len());
    Av2Sample::from_le_bytes([buffer[offset], buffer[offset + 1]])
}
