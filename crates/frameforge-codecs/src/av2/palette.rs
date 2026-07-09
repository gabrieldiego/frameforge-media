use super::Av2VideoGeometry;
use crate::picture::{Picture, PixelFormat};

pub(crate) const AV2_LUMA_PALETTE_MIN_COLORS: usize = 2;
pub(crate) const AV2_LUMA_PALETTE_MAX_COLORS: usize = 8;
pub(crate) const AV2_LUMA_PALETTE_BLOCK_SIZE: usize = 8;
const AV2_LUMA_INTRA_TILE_SIZE: usize = 64;
const AV2_LUMA_INTRA_MODE_SWITCH_SAD_MARGIN: usize = 64;
const AV2_CHROMA_BDPCM_NONZERO_COST: usize = 160;
const AV2_ENABLE_LUMA_DPCM_444: bool = false;
const LOSSLESS_DC_PREDICTOR: u8 = 128;
const LOSSLESS_H_PRED_LEFT_EDGE: u8 = 129;
const LOSSLESS_V_PRED_ABOVE_EDGE: u8 = 127;
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
    colors: Vec<u8>,
    indices: [u8; AV2_LUMA_PALETTE_BLOCK_SAMPLES],
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Av2LumaPalette444 {
    blocks: Vec<Av2LumaPaletteBlock444>,
    luma_modes: Vec<Av2LumaIntraMode>,
    luma_bdpcm_horz: Vec<Option<bool>>,
    chroma_use_bdpcm: Vec<bool>,
    chroma_intra_modes: Vec<Av2ChromaIntraMode>,
    y_plane: Vec<u8>,
    luma_prediction: Vec<u8>,
    u_plane: Vec<u8>,
    v_plane: Vec<u8>,
    reconstruction: Vec<u8>,
    width: usize,
    height: usize,
    blocks_wide: usize,
    blocks_high: usize,
}

impl Av2LumaPalette444 {
    pub(crate) fn colors_for_block(&self, x0: usize, y0: usize) -> &[u8] {
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
        let mut intra_paeth_score = 0usize;
        for plane in [&self.u_plane, &self.v_plane] {
            for txb_y in (0..AV2_LUMA_PALETTE_BLOCK_SIZE).step_by(4) {
                for txb_x in (0..AV2_LUMA_PALETTE_BLOCK_SIZE).step_by(4) {
                    let txb_x0 = x0 + txb_x;
                    let txb_y0 = y0 + txb_y;
                    let bdpcm_horz_residual =
                        self.chroma_bdpcm_residuals(plane, txb_x0, txb_y0, true);
                    let bdpcm_vert_residual =
                        self.chroma_bdpcm_residuals(plane, txb_x0, txb_y0, false);
                    let intra_dc_residual =
                        self.chroma_intra_residuals(plane, txb_x0, txb_y0, Av2ChromaIntraMode::Dc);
                    let intra_horz_residual = self.chroma_intra_residuals(
                        plane,
                        txb_x0,
                        txb_y0,
                        Av2ChromaIntraMode::Horizontal,
                    );
                    let intra_vert_residual = self.chroma_intra_residuals(
                        plane,
                        txb_x0,
                        txb_y0,
                        Av2ChromaIntraMode::Vertical,
                    );
                    let intra_paeth_residual = self.chroma_intra_residuals(
                        plane,
                        txb_x0,
                        txb_y0,
                        Av2ChromaIntraMode::Paeth,
                    );
                    bdpcm_horz_score += chroma_bdpcm_coeff_score(&bdpcm_horz_residual);
                    bdpcm_vert_score += chroma_bdpcm_coeff_score(&bdpcm_vert_residual);
                    intra_dc_score += chroma_bdpcm_coeff_score(&intra_dc_residual);
                    intra_horz_score += chroma_bdpcm_coeff_score(&intra_horz_residual);
                    intra_vert_score += chroma_bdpcm_coeff_score(&intra_vert_residual);
                    intra_paeth_score += chroma_bdpcm_coeff_score(&intra_paeth_residual);
                }
            }
        }
        let candidates = [
            (true, Av2ChromaIntraMode::Horizontal, bdpcm_horz_score),
            (true, Av2ChromaIntraMode::Vertical, bdpcm_vert_score),
            (false, Av2ChromaIntraMode::Dc, intra_dc_score),
            (false, Av2ChromaIntraMode::Horizontal, intra_horz_score),
            (false, Av2ChromaIntraMode::Vertical, intra_vert_score),
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

    fn luma_bdpcm_horz_direction_for_block(&self, _x0: usize, _y0: usize) -> bool {
        // Keep the first luma-DPCM step hardware-cheap: two-color 64x64
        // tiles use vertical DPCM only, avoiding a per-block direction-cost
        // scorer and additional predictor-edge storage in RTL.
        false
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

    pub(crate) fn y_sample(&self, x: usize, y: usize) -> u8 {
        self.luma_sample(&self.y_plane, x, y)
    }

    pub(crate) fn luma_prediction_sample(&self, x: usize, y: usize) -> u8 {
        self.luma_sample(&self.luma_prediction, x, y)
    }

    pub(crate) fn reconstruction(&self) -> &[u8] {
        &self.reconstruction
    }

    pub(crate) fn u_sample(&self, x: usize, y: usize) -> u8 {
        self.chroma_sample(&self.u_plane, x, y)
    }

    pub(crate) fn v_sample(&self, x: usize, y: usize) -> u8 {
        self.chroma_sample(&self.v_plane, x, y)
    }

    fn luma_sample(&self, plane: &[u8], x: usize, y: usize) -> u8 {
        assert!(x < self.width && y < self.height);
        plane[y * self.width + x]
    }

    fn chroma_sample(&self, plane: &[u8], x: usize, y: usize) -> u8 {
        assert!(x < self.width && y < self.height);
        plane[y * self.width + x]
    }

    fn chroma_bdpcm_residuals(&self, plane: &[u8], x0: usize, y0: usize, horz: bool) -> [i32; 16] {
        let tile_x0 = (x0 / AV2_LUMA_INTRA_TILE_SIZE) * AV2_LUMA_INTRA_TILE_SIZE;
        let tile_y0 = (y0 / AV2_LUMA_INTRA_TILE_SIZE) * AV2_LUMA_INTRA_TILE_SIZE;
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
                        LOSSLESS_H_PRED_LEFT_EDGE
                    }
                } else if local_y != 0 {
                    self.chroma_sample(plane, x, y - 1)
                } else if y0 != tile_y0 {
                    self.chroma_sample(plane, x, y0 - 1)
                } else if x0 != tile_x0 {
                    self.chroma_sample(plane, x0 - 1, y0)
                } else {
                    LOSSLESS_V_PRED_ABOVE_EDGE
                };
                residual[local_y * 4 + local_x] = sample - i32::from(predictor);
            }
        }
        residual
    }

    fn chroma_intra_residuals(
        &self,
        plane: &[u8],
        x0: usize,
        y0: usize,
        mode: Av2ChromaIntraMode,
    ) -> [i32; 16] {
        let tile_x0 = (x0 / AV2_LUMA_INTRA_TILE_SIZE) * AV2_LUMA_INTRA_TILE_SIZE;
        let tile_y0 = (y0 / AV2_LUMA_INTRA_TILE_SIZE) * AV2_LUMA_INTRA_TILE_SIZE;
        let dc_predictor =
            (mode == Av2ChromaIntraMode::Dc).then(|| self.chroma_dc_predictor(plane, x0, y0));
        let mut residual = [0i32; 16];
        for local_y in 0..4 {
            let y = y0 + local_y;
            for local_x in 0..4 {
                let x = x0 + local_x;
                let sample = i32::from(self.chroma_sample(plane, x, y));
                let predictor = match mode {
                    Av2ChromaIntraMode::Dc => dc_predictor.expect("DC predictor is precomputed"),
                    Av2ChromaIntraMode::Horizontal => {
                        if x0 != tile_x0 {
                            self.chroma_sample(plane, x0 - 1, y)
                        } else if y0 != tile_y0 {
                            self.chroma_sample(plane, x0, y0 - 1)
                        } else {
                            LOSSLESS_H_PRED_LEFT_EDGE
                        }
                    }
                    Av2ChromaIntraMode::Vertical => {
                        if y0 != tile_y0 {
                            self.chroma_sample(plane, x, y0 - 1)
                        } else if x0 != tile_x0 {
                            self.chroma_sample(plane, x0 - 1, y0)
                        } else {
                            LOSSLESS_V_PRED_ABOVE_EDGE
                        }
                    }
                    Av2ChromaIntraMode::Paeth => {
                        let have_left = x0 != tile_x0;
                        let have_top = y0 != tile_y0;
                        let left = if have_left {
                            self.chroma_sample(plane, x0 - 1, y)
                        } else if have_top {
                            self.chroma_sample(plane, x0, y0 - 1)
                        } else {
                            LOSSLESS_H_PRED_LEFT_EDGE
                        };
                        let above = if have_top {
                            self.chroma_sample(plane, x, y0 - 1)
                        } else if have_left {
                            self.chroma_sample(plane, x0 - 1, y0)
                        } else {
                            LOSSLESS_V_PRED_ABOVE_EDGE
                        };
                        let above_left = if have_left && have_top {
                            self.chroma_sample(plane, x0 - 1, y0 - 1)
                        } else if have_top {
                            self.chroma_sample(plane, x0, y0 - 1)
                        } else if have_left {
                            self.chroma_sample(plane, x0 - 1, y0)
                        } else {
                            LOSSLESS_DC_PREDICTOR
                        };
                        paeth_predictor(left, above, above_left)
                    }
                };
                residual[local_y * 4 + local_x] = sample - i32::from(predictor);
            }
        }
        residual
    }

    fn chroma_dc_predictor(&self, plane: &[u8], x0: usize, y0: usize) -> u8 {
        let tile_x0 = (x0 / AV2_LUMA_INTRA_TILE_SIZE) * AV2_LUMA_INTRA_TILE_SIZE;
        let tile_y0 = (y0 / AV2_LUMA_INTRA_TILE_SIZE) * AV2_LUMA_INTRA_TILE_SIZE;
        let have_left = x0 != tile_x0;
        let have_top = y0 != tile_y0;
        if !have_left && !have_top {
            return LOSSLESS_DC_PREDICTOR;
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
        ((sum + count / 2) / count) as u8
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

fn paeth_predictor(left: u8, above: u8, above_left: u8) -> u8 {
    let left = i32::from(left);
    let above = i32::from(above);
    let above_left = i32::from(above_left);
    let base = left + above - above_left;
    let p_left = (base - left).abs();
    let p_above = (base - above).abs();
    let p_above_left = (base - above_left).abs();
    if p_left <= p_above && p_left <= p_above_left {
        left as u8
    } else if p_above <= p_above_left {
        above as u8
    } else {
        above_left as u8
    }
}

fn chroma_bdpcm_coeff_score(residual: &[i32; 16]) -> usize {
    // Encoder-only mode decision proxy. The syntax/reconstruction path writes
    // exact lossless FWHT coefficients, so score the same coefficient domain
    // when choosing between legal chroma prediction families.
    let coefficients = av2_fwht4x4_for_score(residual);
    coefficients.iter().fold(0usize, |score, coefficient| {
        debug_assert_eq!(coefficient % 8, 0);
        let level = (coefficient.unsigned_abs() / 8) as usize;
        if level == 0 {
            score
        } else {
            score + AV2_CHROMA_BDPCM_NONZERO_COST + level.min(255) + level.saturating_sub(5) / 4
        }
    })
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
) -> Result<Av2LumaPalette444, String> {
    let expected_len =
        Picture::expected_len(geometry.width, geometry.height, PixelFormat::Yuv444p8);
    if frame.len() != expected_len {
        return Err(format!(
            "AV2 yuv444p8 input length mismatch: expected {expected_len} byte(s), got {}",
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
    let y_plane = &frame[..plane_len];
    let u_plane = &frame[plane_len..2 * plane_len];
    let v_plane = &frame[2 * plane_len..3 * plane_len];
    let blocks_wide = geometry.width / AV2_LUMA_PALETTE_BLOCK_SIZE;
    let blocks_high = geometry.height / AV2_LUMA_PALETTE_BLOCK_SIZE;
    let mut blocks = Vec::with_capacity(blocks_wide * blocks_high);
    let mut luma_modes = Vec::with_capacity(blocks_wide * blocks_high);
    let mut luma_prediction = vec![0; plane_len];

    for block_y in 0..blocks_high {
        for block_x in 0..blocks_wide {
            let x0 = block_x * AV2_LUMA_PALETTE_BLOCK_SIZE;
            let y0 = block_y * AV2_LUMA_PALETTE_BLOCK_SIZE;
            let mut samples = [0u8; AV2_LUMA_PALETTE_BLOCK_SAMPLES];
            for local_y in 0..AV2_LUMA_PALETTE_BLOCK_SIZE {
                for local_x in 0..AV2_LUMA_PALETTE_BLOCK_SIZE {
                    let src_index = (y0 + local_y) * geometry.width + x0 + local_x;
                    samples[local_y * AV2_LUMA_PALETTE_BLOCK_SIZE + local_x] = y_plane[src_index];
                }
            }

            let block = build_luma_palette_block(&samples);
            let mode = choose_luma_intra_mode(
                y_plane,
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
                        y_plane,
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
        y_plane: y_plane.to_vec(),
        luma_prediction,
        u_plane: u_plane.to_vec(),
        v_plane: v_plane.to_vec(),
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
                if palette.blocks[block_index].colors.len() <= 2 {
                    let x0 = block_x * AV2_LUMA_PALETTE_BLOCK_SIZE;
                    let y0 = block_y * AV2_LUMA_PALETTE_BLOCK_SIZE;
                    palette.luma_bdpcm_horz[block_index] =
                        Some(palette.luma_bdpcm_horz_direction_for_block(x0, y0));
                }
            }
        }
    }

    Ok(palette)
}

fn choose_luma_intra_mode(
    y_plane: &[u8],
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

    // AV2 tiles are independent in this MVP path. Do not borrow predictors
    // across 64x64 tile boundaries; the decoder has no reconstructed neighbor
    // there.
    let above_mode = (y0 % AV2_LUMA_INTRA_TILE_SIZE != 0)
        .then(|| previous_modes[(block_y - 1) * blocks_wide + block_x]);
    let left_mode = (x0 % AV2_LUMA_INTRA_TILE_SIZE != 0)
        .then(|| previous_modes[block_y * blocks_wide + block_x - 1]);

    // AV2 v1.0.0 Sections 5.20.5.5 and 5.20.5.6, implemented in AVM as
    // get_y_mode_idx_ctx()/get_y_intra_mode_set(), derive the y_mode_idx
    // context and mode list from above-right and bottom-left directional
    // neighbors. The current RTL entropy mux only implements the
    // non-directional-neighbor context, so H/V remains restricted to a terminal
    // 8x8 tile leaf that cannot seed a later block's directional context.
    let fixed_mode_ctx0 = above_mode.map_or(true, |mode| mode == Av2LumaIntraMode::Dc)
        && left_mode.map_or(true, |mode| mode == Av2LumaIntraMode::Dc);
    let terminal_tile_leaf = (block_x + 1 == blocks_wide
        || (x0 + AV2_LUMA_PALETTE_BLOCK_SIZE) % AV2_LUMA_INTRA_TILE_SIZE == 0)
        && (block_y + 1 == blocks_high
            || (y0 + AV2_LUMA_PALETTE_BLOCK_SIZE) % AV2_LUMA_INTRA_TILE_SIZE == 0);

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
    y_plane: &[u8],
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
    y_plane: &[u8],
    width: usize,
    x0: usize,
    y0: usize,
    local_x: usize,
    local_y: usize,
    block: &Av2LumaPaletteBlock444,
    mode: Av2LumaIntraMode,
) -> u8 {
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
    samples: &[u8; AV2_LUMA_PALETTE_BLOCK_SAMPLES],
) -> Av2LumaPaletteBlock444 {
    let mut collected = Vec::with_capacity(AV2_LUMA_PALETTE_MAX_COLORS);
    for &sample in samples {
        if !collected.contains(&sample) && collected.len() < AV2_LUMA_PALETTE_MAX_COLORS {
            collected.push(sample);
        }
    }
    if collected.is_empty() {
        collected.push(0);
    }

    let target_colors = if collected.len() <= 2 {
        2
    } else if collected.len() <= 4 {
        4
    } else {
        AV2_LUMA_PALETTE_MAX_COLORS
    };

    let mut colors = collected;
    let mut candidate = 0u16;
    while colors.len() < target_colors {
        let sample = candidate as u8;
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
                        let delta = i16::from(sample) - i16::from(color);
                        delta.abs()
                    })
                    .map(|(index, _)| index)
                    .expect("AV2 palette always has at least one color")
            });
        indices[sample_index] = index as u8;
    }

    Av2LumaPaletteBlock444 { colors, indices }
}
