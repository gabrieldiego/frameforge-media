use super::{
    av2_lossless_dc_predictor, av2_lossless_h_pred_left_edge, av2_lossless_v_pred_above_edge,
    Av2Sample, Av2VideoGeometry,
};
use crate::picture::{Picture, PixelFormat, SampleBitDepth};
use frameforge_core::read_planar_sample;
use std::cmp::Reverse;

pub(crate) const AV2_LUMA_PALETTE_MIN_COLORS: usize = 2;
pub(crate) const AV2_LUMA_PALETTE_MAX_COLORS: usize = 8;
pub(crate) const AV2_LUMA_PALETTE_BLOCK_SIZE: usize = 8;
const AV2_LUMA_PALETTE_SOFT_MAX_COLORS: usize = 6;
const AV2_LUMA_INTRA_MODE_SWITCH_SAD_MARGIN: usize = 64;
const AV2_LUMA_DPCM_NONZERO_COST: usize = 124;
const AV2_LUMA_DPCM_LEVEL_SCALE: usize = 20000;
const AV2_LUMA_DPCM_SCORE_MARGIN: usize = 1024;
const AV2_LUMA_DPCM_PALETTE_SYNTAX_BONUS: usize = 3072;
const AV2_CHROMA_BDPCM_NONZERO_COST: usize = 124;
const AV2_CHROMA_BDPCM_LEVEL_SCALE: usize = 20000;
const AV2_ENABLE_LUMA_DPCM_444: bool = true;
const AV2_LUMA_JOINT_MODE_V: usize = 22;
const AV2_LUMA_JOINT_MODE_H: usize = 50;
const AV2_LUMA_PALETTE_BLOCK_SAMPLES: usize =
    AV2_LUMA_PALETTE_BLOCK_SIZE * AV2_LUMA_PALETTE_BLOCK_SIZE;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Av2LumaIntraMode {
    Dc,
    Vertical,
    Horizontal,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Av2ChromaIntraMode {
    Dc,
    Vertical,
    Horizontal,
    Directional45,
    Directional67,
    Directional135,
    Directional113,
    Directional157,
    Directional203,
    Smooth,
    SmoothVertical,
    SmoothHorizontal,
    Paeth,
}

impl Av2ChromaIntraMode {
    pub(crate) fn is_horizontal(self) -> bool {
        matches!(self, Self::Horizontal)
    }
}

impl Av2LumaIntraMode {
    pub(crate) fn mode_index(self) -> usize {
        match self {
            Self::Dc => 0,
            Self::Vertical => 5,
            Self::Horizontal => 6,
        }
    }

    fn is_directional(self) -> bool {
        matches!(self, Self::Vertical | Self::Horizontal)
    }

    fn joint_mode(self) -> usize {
        match self {
            Self::Dc => 0,
            Self::Vertical => AV2_LUMA_JOINT_MODE_V,
            Self::Horizontal => AV2_LUMA_JOINT_MODE_H,
        }
    }

    pub(crate) fn symbol_name(self) -> &'static str {
        match self {
            Self::Dc => "tile.intra.y_mode_idx_dc",
            Self::Vertical => "tile.intra.y_mode_idx_v",
            Self::Horizontal => "tile.intra.y_mode_idx_h",
        }
    }
}

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

impl Av2LumaPalette444 {
    pub(crate) fn bit_depth(&self) -> SampleBitDepth {
        self.bit_depth
    }

    pub(crate) fn colors_for_block(&self, x0: usize, y0: usize) -> &[Av2Sample] {
        &self.block_for_origin(x0, y0).colors
    }

    pub(crate) fn color_count_for_block(&self, x0: usize, y0: usize) -> usize {
        self.colors_for_block(x0, y0).len()
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
                    let bdpcm_horz_residual =
                        self.chroma_bdpcm_residuals(plane, txb_x0, txb_y0, true);
                    let bdpcm_vert_residual =
                        self.chroma_bdpcm_residuals(plane, txb_x0, txb_y0, false);
                    let intra_dc_residual = self.chroma_intra_residuals(
                        plane,
                        txb_x0,
                        txb_y0,
                        x0,
                        y0,
                        AV2_LUMA_PALETTE_BLOCK_SIZE,
                        AV2_LUMA_PALETTE_BLOCK_SIZE,
                        Av2ChromaIntraMode::Dc,
                    );
                    let intra_horz_residual = self.chroma_intra_residuals(
                        plane,
                        txb_x0,
                        txb_y0,
                        x0,
                        y0,
                        AV2_LUMA_PALETTE_BLOCK_SIZE,
                        AV2_LUMA_PALETTE_BLOCK_SIZE,
                        Av2ChromaIntraMode::Horizontal,
                    );
                    let intra_vert_residual = self.chroma_intra_residuals(
                        plane,
                        txb_x0,
                        txb_y0,
                        x0,
                        y0,
                        AV2_LUMA_PALETTE_BLOCK_SIZE,
                        AV2_LUMA_PALETTE_BLOCK_SIZE,
                        Av2ChromaIntraMode::Vertical,
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
                    let intra_smooth_residual = self.chroma_intra_residuals(
                        plane,
                        txb_x0,
                        txb_y0,
                        x0,
                        y0,
                        AV2_LUMA_PALETTE_BLOCK_SIZE,
                        AV2_LUMA_PALETTE_BLOCK_SIZE,
                        Av2ChromaIntraMode::Smooth,
                    );
                    let intra_smooth_v_residual = self.chroma_intra_residuals(
                        plane,
                        txb_x0,
                        txb_y0,
                        x0,
                        y0,
                        AV2_LUMA_PALETTE_BLOCK_SIZE,
                        AV2_LUMA_PALETTE_BLOCK_SIZE,
                        Av2ChromaIntraMode::SmoothVertical,
                    );
                    let intra_smooth_h_residual = self.chroma_intra_residuals(
                        plane,
                        txb_x0,
                        txb_y0,
                        x0,
                        y0,
                        AV2_LUMA_PALETTE_BLOCK_SIZE,
                        AV2_LUMA_PALETTE_BLOCK_SIZE,
                        Av2ChromaIntraMode::SmoothHorizontal,
                    );
                    let intra_paeth_residual = self.chroma_intra_residuals(
                        plane,
                        txb_x0,
                        txb_y0,
                        x0,
                        y0,
                        AV2_LUMA_PALETTE_BLOCK_SIZE,
                        AV2_LUMA_PALETTE_BLOCK_SIZE,
                        Av2ChromaIntraMode::Paeth,
                    );
                    bdpcm_horz_score += chroma_idtx_coeff_score(&bdpcm_horz_residual);
                    bdpcm_vert_score += chroma_idtx_coeff_score(&bdpcm_vert_residual);
                    intra_dc_score += chroma_idtx_coeff_score(&intra_dc_residual);
                    intra_horz_score += chroma_idtx_coeff_score(&intra_horz_residual);
                    intra_vert_score += chroma_idtx_coeff_score(&intra_vert_residual);
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
                    intra_smooth_score += chroma_idtx_coeff_score(&intra_smooth_residual);
                    intra_smooth_v_score += chroma_idtx_coeff_score(&intra_smooth_v_residual);
                    intra_smooth_h_score += chroma_idtx_coeff_score(&intra_smooth_h_residual);
                    intra_paeth_score += chroma_idtx_coeff_score(&intra_paeth_residual);
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

    pub(crate) fn index_at(&self, x: usize, y: usize) -> u8 {
        assert!(x < self.width && y < self.height);
        let block = self.block_for_origin(
            (x / AV2_LUMA_PALETTE_BLOCK_SIZE) * AV2_LUMA_PALETTE_BLOCK_SIZE,
            (y / AV2_LUMA_PALETTE_BLOCK_SIZE) * AV2_LUMA_PALETTE_BLOCK_SIZE,
        );
        let local_x = x % AV2_LUMA_PALETTE_BLOCK_SIZE;
        let local_y = y % AV2_LUMA_PALETTE_BLOCK_SIZE;
        block.indices[local_y * AV2_LUMA_PALETTE_BLOCK_SIZE + local_x]
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

    fn chroma_bdpcm_residuals(
        &self,
        plane: &[Av2Sample],
        x0: usize,
        y0: usize,
        horz: bool,
    ) -> [i32; 16] {
        let tile_x0 = 0;
        let tile_y0 = 0;
        let mut residual = [0i32; 16];
        for local_y in 0..4 {
            let y = y0 + local_y;
            for local_x in 0..4 {
                let x = x0 + local_x;
                let sample = i32::from(self.chroma_sample(plane, x, y));
                let predictor = if horz {
                    if local_x != 0 {
                        self.chroma_sample(plane, x - 1, y)
                    } else if x0 != tile_x0 {
                        self.chroma_sample(plane, x0 - 1, y)
                    } else if y0 != tile_y0 {
                        self.chroma_sample(plane, x0, y0 - 1)
                    } else {
                        self.h_pred_left_edge()
                    }
                } else if local_y != 0 {
                    self.chroma_sample(plane, x, y - 1)
                } else if y0 != tile_y0 {
                    self.chroma_sample(plane, x, y0 - 1)
                } else if x0 != tile_x0 {
                    self.chroma_sample(plane, x0 - 1, y0)
                } else {
                    self.v_pred_above_edge()
                };
                residual[local_y * 4 + local_x] = sample - i32::from(predictor);
            }
        }
        residual
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
        let dc_predictor = (mode == Av2ChromaIntraMode::Dc)
            .then(|| self.chroma_dc_predictor(plane, txb_x0, txb_y0));
        let smooth_edges = matches!(
            mode,
            Av2ChromaIntraMode::Smooth
                | Av2ChromaIntraMode::SmoothVertical
                | Av2ChromaIntraMode::SmoothHorizontal
        )
        .then(|| {
            self.chroma_smooth_edges(
                plane,
                txb_x0,
                txb_y0,
                leaf_x0,
                leaf_y0,
                leaf_width,
                leaf_height,
            )
        });
        let mut residual = [0i32; 16];
        for local_y in 0..4 {
            let y = txb_y0 + local_y;
            for local_x in 0..4 {
                let x = txb_x0 + local_x;
                let sample = i32::from(self.chroma_sample(plane, x, y));
                let predictor = match mode {
                    Av2ChromaIntraMode::Dc => dc_predictor.expect("DC predictor is precomputed"),
                    Av2ChromaIntraMode::Horizontal => {
                        if txb_x0 != tile_x0 {
                            self.chroma_sample(plane, txb_x0 - 1, y)
                        } else if txb_y0 != tile_y0 {
                            self.chroma_sample(plane, txb_x0, txb_y0 - 1)
                        } else {
                            self.h_pred_left_edge()
                        }
                    }
                    Av2ChromaIntraMode::Vertical => {
                        if txb_y0 != tile_y0 {
                            self.chroma_sample(plane, x, txb_y0 - 1)
                        } else if txb_x0 != tile_x0 {
                            self.chroma_sample(plane, txb_x0 - 1, txb_y0)
                        } else {
                            self.v_pred_above_edge()
                        }
                    }
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
                    Av2ChromaIntraMode::Smooth
                    | Av2ChromaIntraMode::SmoothVertical
                    | Av2ChromaIntraMode::SmoothHorizontal => {
                        let (above, left) =
                            smooth_edges.expect("smooth predictor edges are precomputed");
                        av2_highbd_smooth_intra_predictor(
                            mode,
                            above,
                            left,
                            local_x,
                            local_y,
                            self.bit_depth,
                        )
                    }
                    Av2ChromaIntraMode::Paeth => {
                        let have_left = txb_x0 != tile_x0;
                        let have_top = txb_y0 != tile_y0;
                        let left = if have_left {
                            self.chroma_sample(plane, txb_x0 - 1, y)
                        } else if have_top {
                            self.chroma_sample(plane, txb_x0, txb_y0 - 1)
                        } else {
                            self.h_pred_left_edge()
                        };
                        let above = if have_top {
                            self.chroma_sample(plane, x, txb_y0 - 1)
                        } else if have_left {
                            self.chroma_sample(plane, txb_x0 - 1, txb_y0)
                        } else {
                            self.v_pred_above_edge()
                        };
                        let above_left = if have_left && have_top {
                            self.chroma_sample(plane, txb_x0 - 1, txb_y0 - 1)
                        } else if have_top {
                            self.chroma_sample(plane, txb_x0, txb_y0 - 1)
                        } else if have_left {
                            self.chroma_sample(plane, txb_x0 - 1, txb_y0)
                        } else {
                            self.dc_predictor()
                        };
                        paeth_predictor(left, above, above_left)
                    }
                };
                residual[local_y * 4 + local_x] = sample - i32::from(predictor);
            }
        }
        residual
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

fn paeth_predictor(left: Av2Sample, above: Av2Sample, above_left: Av2Sample) -> Av2Sample {
    let left = i32::from(left);
    let above = i32::from(above);
    let above_left = i32::from(above_left);
    let base = left + above - above_left;
    let p_left = (base - left).abs();
    let p_above = (base - above).abs();
    let p_above_left = (base - above_left).abs();
    if p_left <= p_above && p_left <= p_above_left {
        left as Av2Sample
    } else if p_above <= p_above_left {
        above as Av2Sample
    } else {
        above_left as Av2Sample
    }
}

#[derive(Debug, Clone, Copy)]
struct ChromaD135Edges {
    above_left: Av2Sample,
    above: [Av2Sample; 4],
    left: [Av2Sample; 4],
}

fn directional_interpolate(edge: [Av2Sample; 8], along: usize, across: usize) -> Av2Sample {
    // AVM dr_intra_derivative[67], used by both D67 and D203.
    const DERIVATIVE_67_203: usize = 24;
    let projected = DERIVATIVE_67_203 * (across + 1);
    let base = (projected >> 6) + along;
    let shift = (projected & 0x3f) >> 1;
    let value =
        u32::from(edge[base]) * (32 - shift) as u32 + u32::from(edge[base + 1]) * shift as u32;
    ((value + 16) >> 5) as Av2Sample
}

fn zone2_directional_predictor(
    edges: ChromaD135Edges,
    dx: i32,
    dy: i32,
    local_x: usize,
    local_y: usize,
) -> Av2Sample {
    let projected_x = ((local_x as i32) << 6) - ((local_y as i32 + 1) * dx);
    let base_x = projected_x >> 6;
    if base_x >= -1 {
        let shift = ((projected_x & 0x3f) >> 1) as usize;
        return directional_weighted_sample(
            zone2_above_sample(edges, base_x),
            zone2_above_sample(edges, base_x + 1),
            shift,
        );
    }

    let projected_y = ((local_y as i32) << 6) - ((local_x as i32 + 1) * dy);
    let base_y = projected_y >> 6;
    debug_assert!(base_y >= -1);
    let shift = ((projected_y & 0x3f) >> 1) as usize;
    directional_weighted_sample(
        zone2_left_sample(edges, base_y),
        zone2_left_sample(edges, base_y + 1),
        shift,
    )
}

fn zone2_above_sample(edges: ChromaD135Edges, offset: i32) -> Av2Sample {
    if offset < 0 {
        edges.above_left
    } else {
        edges.above[offset as usize]
    }
}

fn zone2_left_sample(edges: ChromaD135Edges, offset: i32) -> Av2Sample {
    if offset < 0 {
        edges.above_left
    } else {
        edges.left[offset as usize]
    }
}

fn directional_weighted_sample(first: Av2Sample, second: Av2Sample, shift: usize) -> Av2Sample {
    let value = u32::from(first) * (32 - shift) as u32 + u32::from(second) * shift as u32;
    ((value + 16) >> 5) as Av2Sample
}

pub(crate) fn av2_highbd_smooth_intra_predictor(
    mode: Av2ChromaIntraMode,
    above: [Av2Sample; 5],
    left: [Av2Sample; 5],
    local_x: usize,
    local_y: usize,
    bit_depth: SampleBitDepth,
) -> Av2Sample {
    debug_assert!(local_x < 4 && local_y < 4);
    const BLEND_WEIGHT_MAX: i32 = 32;
    const BLEND_MAX_LOG2: u8 = 5;
    const TX_LOG2: u8 = 2;

    fn divide_round(value: i32, bits: u8) -> i32 {
        (value + (1 << (bits - 1))) >> bits
    }

    let top = i32::from(above[local_x]);
    let left_sample = i32::from(left[local_y]);
    let bottom_left = i32::from(left[4]);
    let top_right = i32::from(above[4]);
    let row_weight = BLEND_WEIGHT_MAX >> ((local_y << 1).min(6) as u8);
    let col_weight = BLEND_WEIGHT_MAX >> ((local_x << 1).min(6) as u8);
    let pred_v = bottom_left + divide_round((top - bottom_left) * (3 - local_y) as i32, TX_LOG2);
    let pred_h =
        top_right + divide_round((left_sample - top_right) * (3 - local_x) as i32, TX_LOG2);
    let pred_v = pred_v + divide_round((top - pred_v) * row_weight, BLEND_MAX_LOG2 + 1);
    let pred_h = pred_h + divide_round((left_sample - pred_h) * col_weight, BLEND_MAX_LOG2 + 1);
    let prediction = match mode {
        Av2ChromaIntraMode::Smooth => divide_round(pred_v + pred_h, 1),
        Av2ChromaIntraMode::SmoothVertical => pred_v,
        Av2ChromaIntraMode::SmoothHorizontal => pred_h,
        _ => unreachable!("smooth predictor only supports smooth chroma modes"),
    };
    prediction.clamp(0, i32::from(bit_depth.max_sample())) as Av2Sample
}

fn chroma_idtx_coeff_score(residual: &[i32; 16]) -> usize {
    // Most screen-content palette leaves use FSC, which writes chroma through
    // IDTX coefficients. Score the sample-domain residuals used by that path
    // instead of the FWHT domain so mode selection matches the coded syntax.
    let mut score = 0usize;
    for &sample_delta in residual {
        let level = sample_delta.unsigned_abs() as usize;
        if level == 0 {
            continue;
        }
        score +=
            AV2_CHROMA_BDPCM_NONZERO_COST + (level.min(255) * AV2_CHROMA_BDPCM_LEVEL_SCALE) / 100;
    }
    score
}

fn luma_coeff_score(coefficients: &[i32; 16]) -> usize {
    let mut score = 0usize;
    for &coefficient in coefficients {
        debug_assert_eq!(coefficient % 8, 0);
        let level = (coefficient.unsigned_abs() / 8) as usize;
        if level == 0 {
            continue;
        }
        score += AV2_LUMA_DPCM_NONZERO_COST + (level.min(255) * AV2_LUMA_DPCM_LEVEL_SCALE) / 100;
    }
    score
}

fn av2_fwht4x4_for_score(input: &[i32; 16]) -> [i32; 16] {
    let mut output = [0i32; 16];
    for i in 0..4 {
        let mut a1 = input[i];
        let mut b1 = input[4 + i];
        let mut c1 = input[8 + i];
        let mut d1 = input[12 + i];

        a1 += b1;
        d1 -= c1;
        let e1 = (a1 - d1) >> 1;
        b1 = e1 - b1;
        c1 = e1 - c1;
        a1 -= c1;
        d1 += b1;

        output[i] = a1;
        output[4 + i] = c1;
        output[8 + i] = d1;
        output[12 + i] = b1;
    }

    let pass0 = output;
    for i in 0..4 {
        let mut a1 = pass0[i * 4];
        let mut b1 = pass0[i * 4 + 1];
        let mut c1 = pass0[i * 4 + 2];
        let mut d1 = pass0[i * 4 + 3];

        a1 += b1;
        d1 -= c1;
        let e1 = (a1 - d1) >> 1;
        b1 = e1 - b1;
        c1 = e1 - c1;
        a1 -= c1;
        d1 += b1;

        output[i * 4] = a1 * 8;
        output[i * 4 + 1] = c1 * 8;
        output[i * 4 + 2] = d1 * 8;
        output[i * 4 + 3] = b1 * 8;
    }
    output
}

pub(crate) fn build_luma_palette_444(
    frame: &[u8],
    geometry: Av2VideoGeometry,
    bit_depth: SampleBitDepth,
) -> Result<Av2LumaPalette444, String> {
    let format = PixelFormat::yuv444(bit_depth.bits())
        .expect("validated AV2 bit depth must map to a YUV444 pixel format");
    let expected_len = Picture::expected_len(geometry.width, geometry.height, format);
    if frame.len() != expected_len {
        return Err(format!(
            "AV2 yuv444p{} input length mismatch: expected {expected_len} byte(s), got {}",
            bit_depth.bits(),
            frame.len()
        ));
    }
    if geometry.width % AV2_LUMA_PALETTE_BLOCK_SIZE != 0
        || geometry.height % AV2_LUMA_PALETTE_BLOCK_SIZE != 0
    {
        return Err(format!(
            "AV2 luma palette path expects dimensions in {}-pixel units, got {}x{}",
            AV2_LUMA_PALETTE_BLOCK_SIZE, geometry.width, geometry.height
        ));
    }

    let plane_len = geometry.width * geometry.height;
    let y_plane = decode_planar_samples(frame, 0, plane_len, bit_depth)?;
    let u_plane = decode_planar_samples(frame, plane_len, plane_len, bit_depth)?;
    let v_plane = decode_planar_samples(frame, 2 * plane_len, plane_len, bit_depth)?;
    let blocks_wide = geometry.width / AV2_LUMA_PALETTE_BLOCK_SIZE;
    let blocks_high = geometry.height / AV2_LUMA_PALETTE_BLOCK_SIZE;
    let mut blocks = Vec::with_capacity(blocks_wide * blocks_high);
    let mut luma_modes = Vec::with_capacity(blocks_wide * blocks_high);
    let mut luma_prediction = vec![0; plane_len];

    for block_y in 0..blocks_high {
        for block_x in 0..blocks_wide {
            let x0 = block_x * AV2_LUMA_PALETTE_BLOCK_SIZE;
            let y0 = block_y * AV2_LUMA_PALETTE_BLOCK_SIZE;
            let mut samples = [0; AV2_LUMA_PALETTE_BLOCK_SAMPLES];
            for local_y in 0..AV2_LUMA_PALETTE_BLOCK_SIZE {
                for local_x in 0..AV2_LUMA_PALETTE_BLOCK_SIZE {
                    let src_index = (y0 + local_y) * geometry.width + x0 + local_x;
                    samples[local_y * AV2_LUMA_PALETTE_BLOCK_SIZE + local_x] = y_plane[src_index];
                }
            }

            let block = build_luma_palette_block(&samples, bit_depth);
            let mode = choose_luma_intra_mode(
                &y_plane,
                geometry.width,
                x0,
                y0,
                block_x,
                block_y,
                blocks_wide,
                blocks_high,
                &luma_modes,
                &block,
            );
            for local_y in 0..AV2_LUMA_PALETTE_BLOCK_SIZE {
                for local_x in 0..AV2_LUMA_PALETTE_BLOCK_SIZE {
                    let dst_index = (y0 + local_y) * geometry.width + x0 + local_x;
                    luma_prediction[dst_index] = luma_intra_prediction_sample(
                        &y_plane,
                        geometry.width,
                        x0,
                        y0,
                        local_x,
                        local_y,
                        &block,
                        mode,
                    );
                }
            }
            luma_modes.push(mode);
            blocks.push(block);
        }
    }
    // AV2 v1.0.0 Sections 5.20.5.5 and 5.20.8.1 code the luma intra mode
    // before optional DC_PRED palette syntax. The residual coefficient path
    // corrects any samples that are not represented exactly by the selected
    // predictor. Keep both the predictor and final reconstruction explicit so
    // high-color screen blocks cannot silently become lossy.
    let reconstruction = frame.to_vec();

    let block_count = blocks_wide * blocks_high;
    let mut palette = Av2LumaPalette444 {
        blocks,
        luma_modes,
        luma_bdpcm_horz: vec![None; block_count],
        chroma_use_bdpcm: vec![true; block_count],
        chroma_intra_modes: vec![Av2ChromaIntraMode::Horizontal; block_count],
        bit_depth,
        y_plane,
        luma_prediction,
        u_plane,
        v_plane,
        reconstruction,
        width: geometry.width,
        height: geometry.height,
        blocks_wide,
        blocks_high,
    };

    for block_y in 0..blocks_high {
        for block_x in 0..blocks_wide {
            let block_index = block_y * blocks_wide + block_x;
            let x0 = block_x * AV2_LUMA_PALETTE_BLOCK_SIZE;
            let y0 = block_y * AV2_LUMA_PALETTE_BLOCK_SIZE;
            let (chroma_use_bdpcm, chroma_intra_mode) =
                palette.chroma_mode_decision_for_block(x0, y0);
            palette.chroma_use_bdpcm[block_index] = chroma_use_bdpcm;
            palette.chroma_intra_modes[block_index] = chroma_intra_mode;
        }
    }

    // AV2 read_intra_y_mode() supports lossless luma DPCM, and the entropy
    // writer/residual code below can emit it. Keep selection disabled until
    // the selector is block-local and REF-safe: tile-uniform selection delayed
    // entropy until the whole 64x64 tile was scanned, while naive block-local
    // selection can desynchronize AVM tile parsing.
    if AV2_ENABLE_LUMA_DPCM_444 {
        for block_y in 0..blocks_high {
            for block_x in 0..blocks_wide {
                let block_index = block_y * blocks_wide + block_x;
                let x0 = block_x * AV2_LUMA_PALETTE_BLOCK_SIZE;
                let y0 = block_y * AV2_LUMA_PALETTE_BLOCK_SIZE;
                palette.luma_bdpcm_horz[block_index] =
                    palette.luma_bdpcm_horz_decision_for_block(x0, y0);
            }
        }
    }

    Ok(palette)
}

fn decode_planar_samples(
    frame: &[u8],
    sample_start: usize,
    sample_count: usize,
    bit_depth: SampleBitDepth,
) -> Result<Vec<Av2Sample>, String> {
    let mut samples = Vec::with_capacity(sample_count);
    for sample_index in sample_start..sample_start + sample_count {
        let sample = read_planar_sample(frame, sample_index, bit_depth).ok_or_else(|| {
            format!(
                "AV2 yuv444p{} frame ended while reading sample {}",
                bit_depth.bits(),
                sample_index
            )
        })?;
        samples.push(sample.min(bit_depth.max_sample()));
    }
    Ok(samples)
}

fn choose_luma_intra_mode(
    y_plane: &[Av2Sample],
    width: usize,
    x0: usize,
    y0: usize,
    block_x: usize,
    block_y: usize,
    blocks_wide: usize,
    blocks_high: usize,
    previous_modes: &[Av2LumaIntraMode],
    block: &Av2LumaPaletteBlock444,
) -> Av2LumaIntraMode {
    let mut best_mode = Av2LumaIntraMode::Dc;
    let mut best_sad = luma_prediction_sad(y_plane, width, x0, y0, block, best_mode);

    let above_mode = (y0 != 0).then(|| previous_modes[(block_y - 1) * blocks_wide + block_x]);
    let left_mode = (x0 != 0).then(|| previous_modes[block_y * blocks_wide + block_x - 1]);

    // AV2 v1.0.0 Sections 5.20.5.5 and 5.20.5.6, implemented in AVM as
    // get_y_mode_idx_ctx()/get_y_intra_mode_set(), derive the y_mode_idx
    // context and mode list from above-right and bottom-left directional
    // neighbors. The current RTL entropy mux only implements the
    // non-directional-neighbor context, so H/V remains restricted to a terminal
    // 8x8 tile leaf that cannot seed a later block's directional context.
    let fixed_mode_ctx0 = above_mode.map_or(true, |mode| mode == Av2LumaIntraMode::Dc)
        && left_mode.map_or(true, |mode| mode == Av2LumaIntraMode::Dc);
    let terminal_tile_leaf = block_x + 1 == blocks_wide && block_y + 1 == blocks_high;

    if fixed_mode_ctx0 && terminal_tile_leaf && above_mode == Some(Av2LumaIntraMode::Dc) {
        let sad = luma_prediction_sad(y_plane, width, x0, y0, block, Av2LumaIntraMode::Vertical);
        if sad + AV2_LUMA_INTRA_MODE_SWITCH_SAD_MARGIN < best_sad {
            best_sad = sad;
            best_mode = Av2LumaIntraMode::Vertical;
        }
    }
    if fixed_mode_ctx0 && terminal_tile_leaf && left_mode == Some(Av2LumaIntraMode::Dc) {
        let sad = luma_prediction_sad(y_plane, width, x0, y0, block, Av2LumaIntraMode::Horizontal);
        if sad + AV2_LUMA_INTRA_MODE_SWITCH_SAD_MARGIN < best_sad {
            best_mode = Av2LumaIntraMode::Horizontal;
        }
    }

    best_mode
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct Av2LumaModeSyntax {
    pub(crate) context: u8,
    vertical_index: u8,
    horizontal_index: u8,
}

impl Av2LumaModeSyntax {
    pub(crate) fn index_for(self, mode: Av2LumaIntraMode) -> u8 {
        match mode {
            Av2LumaIntraMode::Dc => 0,
            Av2LumaIntraMode::Vertical => self.vertical_index,
            Av2LumaIntraMode::Horizontal => self.horizontal_index,
        }
    }
}

pub(crate) fn av2_luma_mode_syntax_for_block(
    bottom_left_mode: Option<Av2LumaIntraMode>,
    above_right_mode: Option<Av2LumaIntraMode>,
) -> Av2LumaModeSyntax {
    let left_directional = bottom_left_mode.filter(|mode| mode.is_directional());
    let above_right_directional = above_right_mode.filter(|mode| mode.is_directional());
    let context =
        u8::from(left_directional.is_some()) + u8::from(above_right_directional.is_some());

    // AV2 v1.0.0 get_y_mode_idx_ctx()/get_y_intra_mode_set(), mirrored from
    // AVM reconintra.c: the entropy context counts directional bottom-left and
    // above-right modes, and the mode list appends bottom-left first. For fixed
    // 8x8 leaves there are no large-block derived angles, so FrameForge's DC/V/H
    // subset only needs to swap V/H when H is the first directional neighbor.
    let first_directional = left_directional.or(above_right_directional);
    if first_directional.map_or(false, |mode| mode.joint_mode() == AV2_LUMA_JOINT_MODE_H) {
        Av2LumaModeSyntax {
            context,
            vertical_index: 6,
            horizontal_index: 5,
        }
    } else {
        Av2LumaModeSyntax {
            context,
            vertical_index: Av2LumaIntraMode::Vertical.mode_index() as u8,
            horizontal_index: Av2LumaIntraMode::Horizontal.mode_index() as u8,
        }
    }
}

fn luma_prediction_sad(
    y_plane: &[Av2Sample],
    width: usize,
    x0: usize,
    y0: usize,
    block: &Av2LumaPaletteBlock444,
    mode: Av2LumaIntraMode,
) -> usize {
    let mut sad = 0usize;
    for local_y in 0..AV2_LUMA_PALETTE_BLOCK_SIZE {
        for local_x in 0..AV2_LUMA_PALETTE_BLOCK_SIZE {
            let original = y_plane[(y0 + local_y) * width + x0 + local_x];
            let predicted =
                luma_intra_prediction_sample(y_plane, width, x0, y0, local_x, local_y, block, mode);
            sad += usize::from(original.abs_diff(predicted));
        }
    }
    sad
}

fn luma_intra_prediction_sample(
    y_plane: &[Av2Sample],
    width: usize,
    x0: usize,
    y0: usize,
    local_x: usize,
    local_y: usize,
    block: &Av2LumaPaletteBlock444,
    mode: Av2LumaIntraMode,
) -> Av2Sample {
    match mode {
        Av2LumaIntraMode::Dc => {
            let local_index = local_y * AV2_LUMA_PALETTE_BLOCK_SIZE + local_x;
            block.colors[usize::from(block.indices[local_index])]
        }
        // AV2 v1.0.0 Section 5.20.7 residual syntax uses 4x4 TXBs here, and
        // AVM calls av2_predict_intra_block() for each TXB. The second 4x4 in
        // an 8x8 H/V leaf therefore predicts from the reconstructed inner
        // edge of the first 4x4, which is exact in this lossless path.
        Av2LumaIntraMode::Vertical => {
            let predictor_y = if local_y >= 4 { y0 + 3 } else { y0 - 1 };
            y_plane[predictor_y * width + x0 + local_x]
        }
        Av2LumaIntraMode::Horizontal => {
            let predictor_x = if local_x >= 4 { x0 + 3 } else { x0 - 1 };
            y_plane[(y0 + local_y) * width + predictor_x]
        }
    }
}

fn build_luma_palette_block(
    samples: &[Av2Sample; AV2_LUMA_PALETTE_BLOCK_SAMPLES],
    bit_depth: SampleBitDepth,
) -> Av2LumaPaletteBlock444 {
    let mut collected = Vec::with_capacity(AV2_LUMA_PALETTE_MAX_COLORS);
    let value_count = usize::from(bit_depth.max_sample()) + 1;
    let mut counts = vec![0usize; value_count];
    let mut first_positions = vec![usize::MAX; value_count];
    for (sample_index, &sample) in samples.iter().enumerate() {
        let sample_index_by_value = usize::from(sample);
        counts[sample_index_by_value] += 1;
        first_positions[sample_index_by_value] =
            first_positions[sample_index_by_value].min(sample_index);
    }
    for &sample in samples {
        if !collected.contains(&sample) && collected.len() < AV2_LUMA_PALETTE_MAX_COLORS {
            collected.push(sample);
        }
    }
    if collected.is_empty() {
        collected.push(0);
    }

    let unique_colors = counts.iter().filter(|&&count| count != 0).count();
    let target_colors = unique_colors
        .clamp(AV2_LUMA_PALETTE_MIN_COLORS, AV2_LUMA_PALETTE_MAX_COLORS)
        .min(AV2_LUMA_PALETTE_SOFT_MAX_COLORS);

    let mut colors = if unique_colors > target_colors {
        quantized_luma_palette_values(&counts, &first_positions, target_colors)
    } else {
        collected
    };
    let mut candidate = 0;
    while colors.len() < target_colors {
        let sample = candidate as Av2Sample;
        if !colors.contains(&sample) {
            colors.push(sample);
        }
        candidate += 1;
    }
    colors.sort_unstable();

    let mut indices = [0u8; AV2_LUMA_PALETTE_BLOCK_SAMPLES];
    for (sample_index, &sample) in samples.iter().enumerate() {
        let index = colors
            .iter()
            .position(|&color| color == sample)
            .unwrap_or_else(|| {
                colors
                    .iter()
                    .enumerate()
                    .min_by_key(|(_, &color)| {
                        let delta = i32::from(sample) - i32::from(color);
                        delta.unsigned_abs()
                    })
                    .map(|(index, _)| index)
                    .expect("AV2 palette always has at least one color")
            });
        indices[sample_index] = index as u8;
    }

    Av2LumaPaletteBlock444 { colors, indices }
}

fn quantized_luma_palette_values(
    counts: &[usize],
    first_positions: &[usize],
    target_colors: usize,
) -> Vec<Av2Sample> {
    let values: Vec<Av2Sample> = (0..counts.len())
        .filter(|&value| counts[value as usize] != 0)
        .map(|value| value as Av2Sample)
        .collect();
    if values.len() <= target_colors {
        return values;
    }

    let n = values.len();
    // Minimize weighted absolute luma prediction error over sorted value
    // buckets. Residual coefficients still make reconstruction lossless.
    let mut segment_cost = vec![vec![0usize; n]; n];
    let mut segment_value = vec![vec![0; n]; n];
    for start in 0..n {
        for end in start..n {
            let total_count: usize = values[start..=end]
                .iter()
                .map(|&value| counts[usize::from(value)])
                .sum();
            let median_threshold = total_count.div_ceil(2);
            let mut cumulative = 0usize;
            let mut median = values[start];
            for &value in &values[start..=end] {
                cumulative += counts[usize::from(value)];
                if cumulative >= median_threshold {
                    median = value;
                    break;
                }
            }
            segment_value[start][end] = median;
            segment_cost[start][end] = values[start..=end]
                .iter()
                .map(|&value| usize::from(value.abs_diff(median)) * counts[usize::from(value)])
                .sum();
        }
    }

    let mut dp = vec![vec![usize::MAX; n + 1]; target_colors + 1];
    let mut split = vec![vec![0usize; n + 1]; target_colors + 1];
    dp[0][0] = 0;
    for colors in 1..=target_colors {
        for end in colors..=n {
            for start in (colors - 1)..end {
                let Some(cost) = dp[colors - 1][start].checked_add(segment_cost[start][end - 1])
                else {
                    continue;
                };
                if cost < dp[colors][end] {
                    dp[colors][end] = cost;
                    split[colors][end] = start;
                }
            }
        }
    }

    let mut colors = Vec::with_capacity(target_colors);
    let mut end = n;
    for color_count in (1..=target_colors).rev() {
        let start = split[color_count][end];
        colors.push(segment_value[start][end - 1]);
        end = start;
    }
    colors.reverse();
    colors.sort_unstable();
    colors.dedup();

    if colors.len() < target_colors {
        let mut frequent: Vec<Av2Sample> = values;
        frequent.sort_by_key(|&value| {
            let value_index = usize::from(value);
            (
                Reverse(counts[value_index]),
                first_positions[value_index],
                value,
            )
        });
        for value in frequent {
            if colors.len() == target_colors {
                break;
            }
            if !colors.contains(&value) {
                colors.push(value);
            }
        }
        colors.sort_unstable();
    }

    colors
}
