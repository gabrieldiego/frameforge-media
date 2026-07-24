fn write_luma_palette_mode_info(
    writer: &mut Av2EntropyWriter,
    decision: Av2TileDecision,
    palette: &Av2LumaPalette444,
    cache_context: &mut Av2PaletteColorCacheContext,
    tile_origin_x: usize,
    tile_origin_y: usize,
) {
    assert!(
        decision.block_size.width >= AV2_LUMA_PALETTE_BLOCK_SIZE
            && decision.block_size.height >= AV2_LUMA_PALETTE_BLOCK_SIZE,
        "AV2 palette leaves must be at least 8x8 blocks"
    );
    let x0 = tile_origin_x + decision.col * MI_SIZE;
    let y0 = tile_origin_y + decision.row * MI_SIZE;
    let region =
        palette.syntax_region_palette(x0, y0, decision.block_size.width, decision.block_size.height);
    let colors = region.colors();
    assert!(
        (AV2_LUMA_PALETTE_MIN_COLORS..=AV2_LUMA_PALETTE_MAX_COLORS).contains(&colors.len()),
        "AV2 palette size must be within the spec range"
    );
    let mut mode_cdf = DEFAULT_PALETTE_Y_MODE_CDF;
    // AV2 v1.0.0 Section 5.20.8.1 palette_mode_info(): DC_PRED luma blocks
    // signal whether a luma palette is present before palette size and color
    // literals.
    writer.write_symbol("tile.palette.y_mode_present", 1, &mut mode_cdf, 2, false);

    let mut size_cdf = DEFAULT_PALETTE_Y_SIZE_CDF;
    writer.write_symbol(
        "tile.palette.y_size_minus2",
        colors.len() - AV2_LUMA_PALETTE_MIN_COLORS,
        &mut size_cdf,
        7,
        false,
    );
    let cache = cache_context.cache(decision.row, decision.col);
    write_luma_palette_colors(writer, colors, &cache, palette.bit_depth());
    cache_context.update_leaf(decision.row, decision.col, decision.block_size, colors);
}

fn write_luma_palette_absent_mode_info(
    writer: &mut Av2EntropyWriter,
    decision: Av2TileDecision,
    cache_context: &mut Av2PaletteColorCacheContext,
) {
    let mut mode_cdf = DEFAULT_PALETTE_Y_MODE_CDF;
    writer.write_symbol("tile.palette.y_mode_present", 0, &mut mode_cdf, 2, false);
    cache_context.clear_leaf(decision.row, decision.col, decision.block_size);
}

fn write_luma_palette_colors(
    writer: &mut Av2EntropyWriter,
    colors: &[Av2Sample],
    cache: &[Av2Sample],
    bit_depth: SampleBitDepth,
) {
    assert!(colors.windows(2).all(|pair| pair[0] < pair[1]));
    let (cache_found, out_cache_colors) = index_luma_palette_color_cache(colors, cache);
    let mut colors_in_cache = 0usize;
    for found in cache_found {
        // AV2 v1.0.0 Section 5.20.8.1 palette_mode_info(), mirrored from
        // AVM write_palette_colors_y(): signal cache entries until every
        // palette color is accounted for, then delta-code only misses.
        writer.write_literal_bit("tile.palette.y_color_cache", found);
        colors_in_cache += usize::from(found);
        if colors_in_cache == colors.len() {
            break;
        }
    }

    delta_encode_luma_palette_colors(writer, &out_cache_colors, bit_depth);
}

fn index_luma_palette_color_cache(
    colors: &[Av2Sample],
    cache: &[Av2Sample],
) -> (Vec<bool>, Vec<Av2Sample>) {
    if cache.is_empty() {
        return (Vec::new(), colors.to_vec());
    }
    let mut cache_found = vec![false; cache.len()];
    let mut color_hit = vec![false; colors.len()];
    let mut colors_in_cache = 0usize;
    for (cache_index, cache_color) in cache.iter().enumerate() {
        if cache_index >= PALETTE_CACHE_PROBE_LIMIT {
            continue;
        }
        // AV2 v1.0.0 Section 5.20.8.1 palette color-cache signaling permits
        // marking any cached neighbor color that appears in the current
        // palette. Keep the scan bounded by PALETTE_CACHE_PROBE_LIMIT so the
        // matching RTL remains a fixed 8x16 compare network.
        if let Some(color_index) = colors.iter().enumerate().find_map(|(color_index, color)| {
            (!color_hit[color_index] && *color == *cache_color).then_some(color_index)
        }) {
            cache_found[cache_index] = true;
            color_hit[color_index] = true;
            colors_in_cache += 1;
            if colors_in_cache == colors.len() {
                break;
            }
        }
    }
    let out_cache_colors = colors
        .iter()
        .zip(color_hit.iter())
        .filter_map(|(color, hit)| (!*hit).then_some(*color))
        .collect();
    (cache_found, out_cache_colors)
}

fn delta_encode_luma_palette_colors(
    writer: &mut Av2EntropyWriter,
    colors: &[Av2Sample],
    bit_depth: SampleBitDepth,
) {
    if colors.is_empty() {
        return;
    }
    // AV2 v1.0.0 luma palette colors use AVM
    // delta_encode_palette_colors(..., min_val=1): first color is literal at
    // stream bit depth, followed by two bits selecting delta precision and
    // then deltas.
    writer.write_literal(
        "tile.palette.y_color_first",
        u32::from(colors[0]),
        bit_depth.bits(),
    );
    if colors.len() == 1 {
        return;
    }
    let mut deltas = Vec::with_capacity(colors.len() - 1);
    let mut max_delta = 0u32;
    for pair in colors.windows(2) {
        let delta = u32::from(pair[1] - pair[0]);
        assert!(delta >= 1, "AV2 palette deltas must be at least one");
        max_delta = max_delta.max(delta);
        deltas.push(delta);
    }
    let min_bits = bit_depth.bits().saturating_sub(3);
    let mut bits = ceil_log2(max_delta).max(u32::from(min_bits)) as u8;
    writer.write_literal(
        "tile.palette.y_delta_bits_minus_min",
        u32::from(bits - min_bits),
        2,
    );
    let mut range = (1u32 << bit_depth.bits()) - u32::from(colors[0]) - 1;
    for (delta_index, delta) in deltas.iter().enumerate() {
        writer.write_literal("tile.palette.y_color_delta_minus1", *delta - 1, bits);
        range -= *delta;
        if delta_index + 1 < deltas.len() {
            bits = bits.min(ceil_log2(range) as u8);
        }
    }
}

fn write_luma_palette_color_map(
    writer: &mut Av2EntropyWriter,
    decision: Av2TileDecision,
    palette: &Av2LumaPalette444,
    tile_origin_x: usize,
    tile_origin_y: usize,
) {
    let x0 = tile_origin_x + decision.col * MI_SIZE;
    let y0 = tile_origin_y + decision.row * MI_SIZE;
    let region =
        palette.syntax_region_palette(x0, y0, decision.block_size.width, decision.block_size.height);
    let colors = region.color_count();
    let vertical_scan = choose_luma_palette_map_vertical_for_region(
        palette,
        &region,
        x0,
        y0,
        decision.block_size.width,
        decision.block_size.height,
    );
    if decision.block_size.width < 64 && decision.block_size.height < 64 {
        // AV2 v1.0.0 Section 5.20.8.4 palette_tokens(): palette blocks
        // smaller than 64x64 signal a scan direction before the identity-axis
        // and color-index tokens. AVM pack_map_tokens() maps direction=1 to
        // a transposed column-major scan.
        writer.write_literal_bit("tile.palette.y_direction", vertical_scan);
    }
    let mut prev_identity_row_flag = 0usize;
    let outer_limit = if vertical_scan {
        decision.block_size.width
    } else {
        decision.block_size.height
    };
    let inner_limit = if vertical_scan {
        decision.block_size.height
    } else {
        decision.block_size.width
    };
    for outer in 0..outer_limit {
        let identity_row_flag = palette_identity_row_flag(
            palette,
            &region,
            x0,
            y0,
            vertical_scan,
            outer,
            inner_limit,
        );
        let ctx = if outer == 0 {
            3
        } else {
            prev_identity_row_flag
        };
        let mut cdf = DEFAULT_IDENTITY_ROW_CDF_Y[ctx];
        writer.write_symbol_with_key(
            "tile.palette.y_identity_row_flag",
            ctx,
            identity_row_flag,
            &mut cdf,
            3,
            false,
        );

        for inner in 0..inner_limit {
            let (row, col) = palette_map_coordinate(vertical_scan, outer, inner);
            if outer == 0 && inner == 0 {
                writer.write_uniform(
                    "tile.palette.y_color_index_first",
                    colors as u32,
                    u32::from(palette.region_index_at(&region, x0 + col, y0 + row)),
                );
            } else if identity_row_flag != 2 && (identity_row_flag != 1 || inner == 0) {
                let (color_ctx, color_token) = palette_color_index_context(
                    palette,
                    &region,
                    x0,
                    y0,
                    row,
                    col,
                    decision.block_size.width,
                );
                let mut color_cdf = DEFAULT_PALETTE_Y_COLOR_INDEX_CDFS
                    [colors - AV2_LUMA_PALETTE_MIN_COLORS][color_ctx];
                let cdf_key = (colors - AV2_LUMA_PALETTE_MIN_COLORS) * 5 + color_ctx;
                writer.write_symbol_with_key(
                    "tile.palette.y_color_index",
                    cdf_key,
                    color_token,
                    &mut color_cdf,
                    colors,
                    false,
                );
            }
        }
        prev_identity_row_flag = identity_row_flag;
    }
}

fn choose_luma_palette_map_vertical_for_region(
    palette: &Av2LumaPalette444,
    region: &Av2LumaPaletteRegion,
    x0: usize,
    y0: usize,
    width: usize,
    height: usize,
) -> bool {
    if width >= 64 || height >= 64 {
        return false;
    }

    let horizontal_rate =
        luma_palette_color_map_rate_q8(palette, region, x0, y0, width, height, false);
    let vertical_rate =
        luma_palette_color_map_rate_q8(palette, region, x0, y0, width, height, true);
    vertical_rate <= horizontal_rate
}

fn luma_palette_color_map_rate_q8(
    palette: &Av2LumaPalette444,
    region: &Av2LumaPaletteRegion,
    x0: usize,
    y0: usize,
    width: usize,
    height: usize,
    vertical_scan: bool,
) -> u32 {
    let colors = region.color_count();
    let mut rate = 0u32;
    let mut prev_identity_row_flag = 0usize;
    let outer_limit = if vertical_scan { width } else { height };
    let inner_limit = if vertical_scan { height } else { width };

    for outer in 0..outer_limit {
        let identity_row_flag =
            palette_identity_row_flag(palette, region, x0, y0, vertical_scan, outer, inner_limit);
        let ctx = if outer == 0 {
            3
        } else {
            prev_identity_row_flag
        };
        rate = rate.saturating_add(cdf_symbol_rate_q8(
            &DEFAULT_IDENTITY_ROW_CDF_Y[ctx],
            identity_row_flag,
            3,
        ));

        for inner in 0..inner_limit {
            if outer == 0 && inner == 0 {
                continue;
            }
            if identity_row_flag != 2 && (identity_row_flag != 1 || inner == 0) {
                let (row, col) = palette_map_coordinate(vertical_scan, outer, inner);
                let (color_ctx, color_token) =
                    palette_color_index_context(palette, region, x0, y0, row, col, width);
                rate = rate.saturating_add(cdf_symbol_rate_q8(
                    &DEFAULT_PALETTE_Y_COLOR_INDEX_CDFS[colors - AV2_LUMA_PALETTE_MIN_COLORS]
                        [color_ctx],
                    color_token,
                    colors,
                ));
            }
        }
        prev_identity_row_flag = identity_row_flag;
    }

    rate
}

fn cdf_symbol_rate_q8(cdf: &[u16], symbol: usize, nsymbs: usize) -> u32 {
    assert!((2..=16).contains(&nsymbs));
    assert!(symbol < nsymbs);
    let fl = if symbol > 0 {
        u32::from(cdf[symbol - 1])
    } else {
        1 << 15
    };
    let fh = u32::from(cdf[symbol]);
    let prob = fl.saturating_sub(fh).max(1);
    (((f64::from(1 << 15) / f64::from(prob)).log2() * 256.0).round()) as u32
}

fn palette_identity_row_flag(
    palette: &Av2LumaPalette444,
    region: &Av2LumaPaletteRegion,
    x0: usize,
    y0: usize,
    vertical_scan: bool,
    outer: usize,
    inner_limit: usize,
) -> usize {
    if outer > 0
        && (0..inner_limit).all(|inner| {
            let (row, col) = palette_map_coordinate(vertical_scan, outer, inner);
            let (prev_row, prev_col) = palette_map_coordinate(vertical_scan, outer - 1, inner);
            palette.region_index_at(region, x0 + col, y0 + row)
                == palette.region_index_at(region, x0 + prev_col, y0 + prev_row)
        })
    {
        return 2;
    }
    if (1..inner_limit).all(|inner| {
        let (row, col) = palette_map_coordinate(vertical_scan, outer, inner);
        let (prev_row, prev_col) = palette_map_coordinate(vertical_scan, outer, inner - 1);
        palette.region_index_at(region, x0 + col, y0 + row)
            == palette.region_index_at(region, x0 + prev_col, y0 + prev_row)
    }) {
        1
    } else {
        0
    }
}

fn palette_map_coordinate(vertical_scan: bool, outer: usize, inner: usize) -> (usize, usize) {
    if vertical_scan {
        (inner, outer)
    } else {
        (outer, inner)
    }
}

fn palette_color_index_context(
    palette: &Av2LumaPalette444,
    region: &Av2LumaPaletteRegion,
    x0: usize,
    y0: usize,
    row: usize,
    col: usize,
    stride: usize,
) -> (usize, usize) {
    assert!(row > 0 || col > 0);
    let mut color_order = [0u8, 1, 2, 3, 4, 5, 6, 7];
    let mut color_status = [false; 8];
    let mut color_count = 0usize;
    let color_index_ctx;

    if row > 0 && col > 0 {
        let left = palette.region_index_at(region, x0 + col - 1, y0 + row);
        let top_left = palette.region_index_at(region, x0 + col - 1, y0 + row - 1);
        let top = palette.region_index_at(region, x0 + col, y0 + row - 1);
        if left == top_left && left == top {
            color_index_ctx = 4;
            swap_palette_color_order(
                &mut color_order,
                &mut color_status,
                0,
                left,
                &mut color_count,
            );
        } else if left == top {
            color_index_ctx = 3;
            swap_palette_color_order(
                &mut color_order,
                &mut color_status,
                0,
                left,
                &mut color_count,
            );
            swap_palette_color_order(
                &mut color_order,
                &mut color_status,
                1,
                top_left,
                &mut color_count,
            );
        } else if left == top_left {
            color_index_ctx = 2;
            swap_palette_color_order(
                &mut color_order,
                &mut color_status,
                0,
                left,
                &mut color_count,
            );
            swap_palette_color_order(
                &mut color_order,
                &mut color_status,
                1,
                top,
                &mut color_count,
            );
        } else if top_left == top {
            color_index_ctx = 2;
            swap_palette_color_order(
                &mut color_order,
                &mut color_status,
                0,
                top,
                &mut color_count,
            );
            swap_palette_color_order(
                &mut color_order,
                &mut color_status,
                1,
                left,
                &mut color_count,
            );
        } else {
            color_index_ctx = 1;
            swap_palette_color_order(
                &mut color_order,
                &mut color_status,
                0,
                left,
                &mut color_count,
            );
            swap_palette_color_order(
                &mut color_order,
                &mut color_status,
                1,
                top,
                &mut color_count,
            );
            swap_palette_color_order(
                &mut color_order,
                &mut color_status,
                2,
                top_left,
                &mut color_count,
            );
        }
    } else {
        color_index_ctx = 0;
        let neighbor = if col == 0 {
            palette.region_index_at(region, x0 + col, y0 + row - 1)
        } else {
            palette.region_index_at(region, x0 + col - 1, y0 + row)
        };
        swap_palette_color_order(
            &mut color_order,
            &mut color_status,
            0,
            neighbor,
            &mut color_count,
        );
    }

    let mut write_idx = color_count;
    let color_count = region.color_count();
    for read_idx in 0..color_count {
        if !color_status[read_idx] {
            color_order[write_idx] = read_idx as u8;
            write_idx += 1;
        }
    }
    let current_color = palette.region_index_at(region, x0 + col, y0 + row);
    let color_token = color_order
        .iter()
        .take(color_count)
        .position(|&color| color == current_color)
        .unwrap_or_else(|| {
            debug_assert!(
                false,
                "palette color order missed color {} at ({}, {}) with stride {}",
                current_color, col, row, stride
            );
            0
        });
    (color_index_ctx, color_token)
}

fn swap_palette_color_order(
    color_order: &mut [u8; 8],
    color_status: &mut [bool; 8],
    switch_idx: usize,
    max_idx: u8,
    color_count: &mut usize,
) {
    color_order[switch_idx] = max_idx;
    color_status[usize::from(max_idx)] = true;
    *color_count += 1;
}
