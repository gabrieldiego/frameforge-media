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
