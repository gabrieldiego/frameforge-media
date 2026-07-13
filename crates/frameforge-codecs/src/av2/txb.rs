const AV2_STATIC_CDF_TXB_SKIP_Y_BASE: usize = 0;
const AV2_STATIC_CDF_TXB_SKIP_Y_FSC_BASE: usize = 16;
const AV2_STATIC_CDF_TXB_SKIP_U_BASE: usize = 32;
const AV2_STATIC_CDF_TXB_SKIP_U_FSC_BASE: usize = 48;
const AV2_STATIC_CDF_TXB_SKIP_V_BASE: usize = 64;
const AV2_STATIC_CDF_EOB_Y: usize = 100;
const AV2_STATIC_CDF_EOB_UV: usize = 101;
const AV2_STATIC_CDF_EOB_EXTRA: usize = 102;
const AV2_STATIC_CDF_COEFF_Y_BASE_LF_EOB_BASE: usize = 110;
const AV2_STATIC_CDF_COEFF_Y_BASE_EOB_BASE: usize = 130;
const AV2_STATIC_CDF_COEFF_Y_BASE_LF_BASE: usize = 160;
const AV2_STATIC_CDF_COEFF_Y_BASE_BASE: usize = 190;
const AV2_STATIC_CDF_COEFF_Y_BR_LF_BASE: usize = 220;
const AV2_STATIC_CDF_COEFF_Y_BR_BASE: usize = 240;
const AV2_STATIC_CDF_COEFF_UV_BASE_LF_EOB_BASE: usize = 260;
const AV2_STATIC_CDF_COEFF_UV_BASE_EOB_BASE: usize = 280;
const AV2_STATIC_CDF_COEFF_UV_BASE_LF_BASE: usize = 300;
const AV2_STATIC_CDF_COEFF_UV_BASE_BASE: usize = 320;
const AV2_STATIC_CDF_COEFF_UV_BR_BASE: usize = 340;
const AV2_STATIC_CDF_COEFF_Y_DC_BASE_LF_EOB_CTX0: usize = 360;
const AV2_STATIC_CDF_COEFF_Y_DC_LOW_RANGE_LF_CTX0: usize = 361;
const AV2_STATIC_CDF_COEFF_UV_DC_BASE_LF_EOB_CTX0: usize = 362;
const AV2_STATIC_CDF_COEFF_Y_DC_SIGN_BASE: usize = 370;

fn y_txb_skip_static_cdf_key(skip_ctx: u8) -> usize {
    AV2_STATIC_CDF_TXB_SKIP_Y_BASE + usize::from(skip_ctx)
}

fn y_fsc_txb_skip_static_cdf_key(skip_ctx: u8) -> usize {
    AV2_STATIC_CDF_TXB_SKIP_Y_FSC_BASE + usize::from(skip_ctx)
}

fn u_txb_skip_static_cdf_key(skip_ctx: u8, use_fsc: bool) -> usize {
    let base = if use_fsc {
        AV2_STATIC_CDF_TXB_SKIP_U_FSC_BASE
    } else {
        AV2_STATIC_CDF_TXB_SKIP_U_BASE
    };
    base + usize::from(skip_ctx)
}

fn v_txb_skip_static_cdf_key(skip_ctx: u8) -> usize {
    AV2_STATIC_CDF_TXB_SKIP_V_BASE + usize::from(skip_ctx)
}

fn tx4x4_coefficients_from_residual(
    residual: &[i32; TX4X4_SAMPLES],
    use_fsc: bool,
) -> [i32; TX4X4_SAMPLES] {
    if use_fsc {
        idtx4x4_coefficients(residual)
    } else {
        av2_fwht4x4(residual)
    }
}

fn tx4x4_residual_is_zero(residual: &[i32; TX4X4_SAMPLES]) -> bool {
    residual.iter().all(|&sample| sample == 0)
}

fn av2_fwht4x4(input: &[i32; TX4X4_SAMPLES]) -> [i32; TX4X4_SAMPLES] {
    // AV2 v1.0.0 lossless TX_4X4 uses AVM av2_fwht4x4_c() before coefficient
    // coding. The final UNIT_QUANT_FACTOR multiply is preserved so coefficient
    // levels below divide by eight, matching qindex 0 dequantization.
    let mut output = [0i32; TX4X4_SAMPLES];
    for i in 0..TX4X4_SIZE {
        let mut a1 = input[i];
        let mut b1 = input[TX4X4_SIZE + i];
        let mut c1 = input[2 * TX4X4_SIZE + i];
        let mut d1 = input[3 * TX4X4_SIZE + i];

        a1 += b1;
        d1 -= c1;
        let e1 = (a1 - d1) >> 1;
        b1 = e1 - b1;
        c1 = e1 - c1;
        a1 -= c1;
        d1 += b1;

        output[i] = a1;
        output[TX4X4_SIZE + i] = c1;
        output[2 * TX4X4_SIZE + i] = d1;
        output[3 * TX4X4_SIZE + i] = b1;
    }

    let pass0 = output;
    for i in 0..TX4X4_SIZE {
        let mut a1 = pass0[i * TX4X4_SIZE];
        let mut b1 = pass0[i * TX4X4_SIZE + 1];
        let mut c1 = pass0[i * TX4X4_SIZE + 2];
        let mut d1 = pass0[i * TX4X4_SIZE + 3];

        a1 += b1;
        d1 -= c1;
        let e1 = (a1 - d1) >> 1;
        b1 = e1 - b1;
        c1 = e1 - c1;
        a1 -= c1;
        d1 += b1;

        output[i * TX4X4_SIZE] = a1 * 8;
        output[i * TX4X4_SIZE + 1] = c1 * 8;
        output[i * TX4X4_SIZE + 2] = d1 * 8;
        output[i * TX4X4_SIZE + 3] = b1 * 8;
    }
    output
}

fn write_luma_palette_residual_txb(
    writer: &mut Av2EntropyWriter,
    skip_ctx: u8,
    dc_sign_ctx: u8,
    coefficients: &[i32; TX4X4_SAMPLES],
) -> (u8, bool) {
    let (levels, bounds) = lossless_coefficient_levels_and_bounds(coefficients);
    let Some((_, eob)) = bounds else {
        write_y_txb_all_zero(writer, skip_ctx);
        return (0, false);
    };

    write_y_txb_nonzero(writer, skip_ctx);
    write_eob_y(writer, eob);

    for scan_index in (1..eob).rev() {
        let pos = TX4X4_SCAN[scan_index];
        let level = levels[pos];
        let coeff_ctx = luma_nz_map_context(&levels, pos, scan_index, scan_index + 1 == eob);
        write_luma_coefficient_level(
            writer,
            &levels,
            pos,
            scan_index + 1 == eob,
            coeff_ctx,
            level,
        );
    }

    let dc_level = levels[0];
    let dc_ctx = luma_nz_map_context(&levels, 0, 0, eob == 1);
    write_luma_coefficient_level(writer, &levels, 0, eob == 1, dc_ctx, dc_level);

    let mut cul_level = 0u32;
    let mut dc_val = 0i32;
    let mut hr_level_avg = 0u32;
    for scan_index in (0..eob).rev() {
        let pos = TX4X4_SCAN[scan_index];
        let level = levels[pos];
        if level == 0 {
            continue;
        }
        let negative = coefficients[pos] < 0;
        if scan_index == 0 {
            write_y_dc_sign(writer, negative, dc_sign_ctx);
            dc_val = if negative {
                -(level as i32)
            } else {
                level as i32
            };
        } else {
            writer.write_literal_bit("tile.coeff.y.ac_sign_negative", negative);
        }
        write_luma_high_range(writer, pos, level, &mut hr_level_avg);
        cul_level += level;
    }

    (lossless_entropy_context(cul_level, dc_val), true)
}

fn write_luma_palette_fsc_txb(
    writer: &mut Av2EntropyWriter,
    coefficients: &[i32; TX4X4_SAMPLES],
) -> (u8, bool) {
    let (levels, bounds) = lossless_coefficient_levels_and_bounds(coefficients);
    let Some((bob, _)) = bounds else {
        write_y_fsc_txb_all_zero(writer);
        return (0, false);
    };

    write_y_fsc_txb_nonzero(writer);
    write_eob_y(writer, TX4X4_SAMPLES - bob);

    for scan_index in bob..TX4X4_SAMPLES {
        let pos = TX4X4_SCAN[scan_index];
        let level = levels[pos];
        if scan_index == bob {
            let coeff_ctx = idtx_bob_context(scan_index);
            let mut cdf = DEFAULT_COEFF_BASE_BOB_IDTX_CDFS[coeff_ctx];
            writer.write_symbol(
                "tile.coeff.y.idtx_base_bob",
                level.min(3) as usize - 1,
                &mut cdf,
                3,
                false,
            );
        } else {
            let coeff_ctx = idtx_upper_levels_context(&levels, pos);
            let mut cdf = DEFAULT_COEFF_BASE_IDTX_CDFS[coeff_ctx];
            writer.write_symbol(
                "tile.coeff.y.idtx_base",
                level.min(3) as usize,
                &mut cdf,
                4,
                false,
            );
        }
        if level > 2 {
            write_idtx_low_range(writer, &levels, pos, level);
        }
    }

    let mut cul_level = 0u32;
    let mut dc_val = 0i32;
    let mut hr_level_avg = 0u32;
    for scan_index in 0..TX4X4_SAMPLES {
        let pos = TX4X4_SCAN[scan_index];
        let level = levels[pos];
        if level == 0 {
            continue;
        }
        let negative = coefficients[pos] < 0;
        let sign_ctx = idtx_sign_context(&levels, coefficients, pos);
        let mut cdf = DEFAULT_IDTX_SIGN_CDFS[sign_ctx];
        writer.write_symbol(
            "tile.coeff.y.idtx_sign_negative",
            usize::from(negative),
            &mut cdf,
            2,
            false,
        );
        write_idtx_high_range(writer, level, &mut hr_level_avg);
        if scan_index == 0 {
            dc_val = if negative {
                -(level as i32)
            } else {
                level as i32
            };
        }
        cul_level += level;
    }

    (lossless_entropy_context(cul_level, dc_val), true)
}

fn write_chroma_bdpcm_txb(
    writer: &mut Av2EntropyWriter,
    plane: Av2ChromaPlane,
    skip_ctx: u8,
    coefficients: &[i32; TX4X4_SAMPLES],
    use_fsc: bool,
) -> (u8, bool) {
    let (levels, bounds) = lossless_coefficient_levels_and_bounds(coefficients);
    let Some((_, eob)) = bounds else {
        match plane {
            Av2ChromaPlane::U => write_u_txb_all_zero(writer, skip_ctx, use_fsc),
            Av2ChromaPlane::V => write_v_txb_all_zero(writer, skip_ctx),
        }
        return (0, false);
    };

    match plane {
        Av2ChromaPlane::U => write_u_txb_nonzero(writer, skip_ctx, use_fsc),
        Av2ChromaPlane::V => write_v_txb_nonzero(writer, skip_ctx),
    }
    write_eob_uv(writer, eob);

    for scan_index in (1..eob).rev() {
        let pos = TX4X4_SCAN[scan_index];
        let level = levels[pos];
        let coeff_ctx =
            chroma_nz_map_context(&levels, pos, scan_index, scan_index + 1 == eob, plane);
        write_chroma_coefficient_level(
            writer,
            &levels,
            pos,
            scan_index + 1 == eob,
            coeff_ctx,
            level,
        );
    }

    let dc_level = levels[0];
    let dc_ctx = chroma_nz_map_context(&levels, 0, 0, eob == 1, plane);
    write_chroma_coefficient_level(writer, &levels, 0, eob == 1, dc_ctx, dc_level);

    let mut cul_level = 0u32;
    let mut dc_val = 0i32;
    let mut hr_level_avg = 0u32;
    for scan_index in (0..eob).rev() {
        let pos = TX4X4_SCAN[scan_index];
        let level = levels[pos];
        if level == 0 {
            continue;
        }
        let negative = coefficients[pos] < 0;
        let sign_name = match plane {
            Av2ChromaPlane::U if scan_index == 0 => "tile.coeff.u.dc_sign_negative",
            Av2ChromaPlane::V if scan_index == 0 => "tile.coeff.v.dc_sign_negative",
            Av2ChromaPlane::U => "tile.coeff.u.ac_sign_negative",
            Av2ChromaPlane::V => "tile.coeff.v.ac_sign_negative",
        };
        writer.write_literal_bit(sign_name, negative);
        write_chroma_high_range(writer, plane, pos, level, &mut hr_level_avg);
        if scan_index == 0 {
            dc_val = if negative {
                -(level as i32)
            } else {
                level as i32
            };
        }
        cul_level += level;
    }

    (lossless_entropy_context(cul_level, dc_val), true)
}

fn lossless_coefficient_levels_and_bounds(
    coefficients: &[i32; TX4X4_SAMPLES],
) -> ([u32; TX4X4_SAMPLES], Option<(usize, usize)>) {
    let mut levels = [0u32; TX4X4_SAMPLES];
    let mut first = None;
    let mut eob = 0usize;
    for (scan_index, &index) in TX4X4_SCAN.iter().enumerate() {
        let coefficient = coefficients[index];
        debug_assert_eq!(
            coefficient % 8,
            0,
            "AV2 lossless WHT coefficient must be divisible by UNIT_QUANT_FACTOR"
        );
        let level = coefficient.unsigned_abs() / 8;
        levels[index] = level;
        if level != 0 {
            first.get_or_insert(scan_index);
            eob = scan_index + 1;
        }
    }
    (levels, first.map(|first| (first, eob)))
}

fn write_eob_y(writer: &mut Av2EntropyWriter, eob: usize) {
    let (eob_pt, eob_extra) = eob_pos_token(eob);
    let mut cdf = DEFAULT_EOB_MULTI16_Y_CTX0_CDF;
    writer.write_symbol_with_static_cdf_key(
        "tile.coeff.y.eob_pt_tx4x4",
        AV2_STATIC_CDF_EOB_Y,
        eob_pt - 1,
        &mut cdf,
        5,
        false,
    );

    let eob_offset_bits = eob_offset_bits(eob_pt);
    if eob_offset_bits > 0 {
        let eob_shift = eob_offset_bits - 1;
        let bit = (eob_extra & (1 << eob_shift)) != 0;
        let mut extra_cdf = DEFAULT_EOB_EXTRA_CDF;
        writer.write_symbol_with_static_cdf_key(
            "tile.coeff.eob_extra_bit",
            AV2_STATIC_CDF_EOB_EXTRA,
            usize::from(bit),
            &mut extra_cdf,
            2,
            false,
        );
        let low_bits = eob_extra & ((1 << eob_shift) - 1);
        writer.write_literal("tile.coeff.y.eob_extra", low_bits as u32, eob_shift as u8);
    }
}

fn write_eob_uv(writer: &mut Av2EntropyWriter, eob: usize) {
    let (eob_pt, eob_extra) = eob_pos_token(eob);
    let mut cdf = DEFAULT_EOB_MULTI16_UV_CTX2_CDF;
    writer.write_symbol_with_static_cdf_key(
        "tile.coeff.uv.eob_pt_tx4x4",
        AV2_STATIC_CDF_EOB_UV,
        eob_pt - 1,
        &mut cdf,
        5,
        false,
    );

    let eob_offset_bits = eob_offset_bits(eob_pt);
    if eob_offset_bits > 0 {
        let eob_shift = eob_offset_bits - 1;
        let bit = (eob_extra & (1 << eob_shift)) != 0;
        let mut extra_cdf = DEFAULT_EOB_EXTRA_CDF;
        writer.write_symbol_with_static_cdf_key(
            "tile.coeff.eob_extra_bit",
            AV2_STATIC_CDF_EOB_EXTRA,
            usize::from(bit),
            &mut extra_cdf,
            2,
            false,
        );
        let low_bits = eob_extra & ((1 << eob_shift) - 1);
        writer.write_literal("tile.coeff.uv.eob_extra", low_bits as u32, eob_shift as u8);
    }
}

fn eob_pos_token(eob: usize) -> (usize, usize) {
    const EOB_TO_POS_SMALL: [usize; 33] = [
        0, 1, 2, 3, 3, 4, 4, 4, 4, 5, 5, 5, 5, 5, 5, 5, 5, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6,
        6, 6, 6,
    ];
    const EOB_GROUP_START: [usize; 12] = [0, 1, 2, 3, 5, 9, 17, 33, 65, 129, 257, 513];
    assert!((1..=TX4X4_SAMPLES).contains(&eob));
    let token = EOB_TO_POS_SMALL[eob];
    (token, eob - EOB_GROUP_START[token])
}

fn eob_offset_bits(eob_pt: usize) -> usize {
    const EOB_OFFSET_BITS: [usize; 12] = [0, 0, 0, 1, 2, 3, 4, 5, 6, 7, 8, 9];
    EOB_OFFSET_BITS[eob_pt]
}

fn write_chroma_coefficient_level(
    writer: &mut Av2EntropyWriter,
    levels: &[u32; TX4X4_SAMPLES],
    pos: usize,
    is_eob_coefficient: bool,
    coeff_ctx: usize,
    level: u32,
) {
    let limits = chroma_lf_limits(pos);
    if is_eob_coefficient {
        assert!(level > 0, "AV2 EOB coefficient must be non-zero");
        if limits {
            let mut cdf = DEFAULT_COEFF_BASE_LF_EOB_UV_CDFS[coeff_ctx];
            writer.write_symbol_with_static_cdf_key(
                "tile.coeff.uv.base_lf_eob",
                AV2_STATIC_CDF_COEFF_UV_BASE_LF_EOB_BASE + coeff_ctx,
                level.min(5) as usize - 1,
                &mut cdf,
                5,
                false,
            );
        } else {
            let mut cdf = DEFAULT_COEFF_BASE_EOB_UV_CDFS[coeff_ctx];
            writer.write_symbol_with_static_cdf_key(
                "tile.coeff.uv.base_eob",
                AV2_STATIC_CDF_COEFF_UV_BASE_EOB_BASE + coeff_ctx,
                level.min(3) as usize - 1,
                &mut cdf,
                3,
                false,
            );
            if level > 2 {
                write_chroma_low_range(writer, levels, pos, level - 3);
            }
        }
    } else if limits {
        let mut cdf = DEFAULT_COEFF_BASE_LF_UV_CDFS[coeff_ctx];
        writer.write_symbol_with_static_cdf_key(
            "tile.coeff.uv.base_lf",
            AV2_STATIC_CDF_COEFF_UV_BASE_LF_BASE + coeff_ctx,
            level.min(5) as usize,
            &mut cdf,
            6,
            false,
        );
    } else {
        let mut cdf = DEFAULT_COEFF_BASE_UV_CDFS[coeff_ctx];
        writer.write_symbol_with_static_cdf_key(
            "tile.coeff.uv.base",
            AV2_STATIC_CDF_COEFF_UV_BASE_BASE + coeff_ctx,
            level.min(3) as usize,
            &mut cdf,
            4,
            false,
        );
        if level > 2 {
            write_chroma_low_range(writer, levels, pos, level - 3);
        }
    }
}

fn write_luma_coefficient_level(
    writer: &mut Av2EntropyWriter,
    levels: &[u32; TX4X4_SAMPLES],
    pos: usize,
    is_eob_coefficient: bool,
    coeff_ctx: usize,
    level: u32,
) {
    let limits = luma_lf_limits(pos);
    if is_eob_coefficient {
        assert!(level > 0, "AV2 EOB coefficient must be non-zero");
        if limits {
            let mut cdf = DEFAULT_COEFF_BASE_LF_EOB_Y_CDFS[coeff_ctx];
            writer.write_symbol_with_static_cdf_key(
                "tile.coeff.y.base_lf_eob",
                AV2_STATIC_CDF_COEFF_Y_BASE_LF_EOB_BASE + coeff_ctx,
                level.min(5) as usize - 1,
                &mut cdf,
                5,
                false,
            );
            if level > 4 {
                write_luma_low_range(writer, levels, pos, true, level - 5);
            }
        } else {
            let mut cdf = DEFAULT_COEFF_BASE_EOB_Y_CDFS[coeff_ctx];
            writer.write_symbol_with_static_cdf_key(
                "tile.coeff.y.base_eob",
                AV2_STATIC_CDF_COEFF_Y_BASE_EOB_BASE + coeff_ctx,
                level.min(3) as usize - 1,
                &mut cdf,
                3,
                false,
            );
            if level > 2 {
                write_luma_low_range(writer, levels, pos, false, level - 3);
            }
        }
    } else if limits {
        let mut cdf = DEFAULT_COEFF_BASE_LF_Y_CDFS[coeff_ctx];
        writer.write_symbol_with_static_cdf_key(
            "tile.coeff.y.base_lf",
            AV2_STATIC_CDF_COEFF_Y_BASE_LF_BASE + coeff_ctx,
            level.min(5) as usize,
            &mut cdf,
            6,
            false,
        );
        if level > 4 {
            write_luma_low_range(writer, levels, pos, true, level - 5);
        }
    } else {
        let mut cdf = DEFAULT_COEFF_BASE_Y_CDFS[coeff_ctx];
        writer.write_symbol_with_static_cdf_key(
            "tile.coeff.y.base",
            AV2_STATIC_CDF_COEFF_Y_BASE_BASE + coeff_ctx,
            level.min(3) as usize,
            &mut cdf,
            4,
            false,
        );
        if level > 2 {
            write_luma_low_range(writer, levels, pos, false, level - 3);
        }
    }
}

fn write_luma_low_range(
    writer: &mut Av2EntropyWriter,
    levels: &[u32; TX4X4_SAMPLES],
    pos: usize,
    lf: bool,
    base_range: u32,
) {
    if lf {
        let br_ctx = luma_br_lf_context(levels, pos);
        let mut cdf = DEFAULT_COEFF_BR_LF_Y_CDFS[br_ctx];
        writer.write_symbol_with_static_cdf_key(
            "tile.coeff.y.low_range_lf",
            AV2_STATIC_CDF_COEFF_Y_BR_LF_BASE + br_ctx,
            base_range.min(3) as usize,
            &mut cdf,
            4,
            false,
        );
    } else {
        let br_ctx = luma_br_context(levels, pos);
        let mut cdf = DEFAULT_COEFF_BR_Y_CDFS[br_ctx];
        writer.write_symbol_with_static_cdf_key(
            "tile.coeff.y.low_range",
            AV2_STATIC_CDF_COEFF_Y_BR_BASE + br_ctx,
            base_range.min(3) as usize,
            &mut cdf,
            4,
            false,
        );
    }
}

fn write_chroma_low_range(
    writer: &mut Av2EntropyWriter,
    levels: &[u32; TX4X4_SAMPLES],
    pos: usize,
    base_range: u32,
) {
    let br_ctx = chroma_br_context(levels, pos);
    let mut cdf = DEFAULT_COEFF_BR_UV_CDFS[br_ctx];
    writer.write_symbol_with_static_cdf_key(
        "tile.coeff.uv.low_range",
        AV2_STATIC_CDF_COEFF_UV_BR_BASE + br_ctx,
        base_range.min(3) as usize,
        &mut cdf,
        4,
        false,
    );
}

fn write_idtx_low_range(
    writer: &mut Av2EntropyWriter,
    levels: &[u32; TX4X4_SAMPLES],
    pos: usize,
    level: u32,
) {
    let br_ctx = idtx_br_context(levels, pos);
    let mut cdf = DEFAULT_COEFF_BR_IDTX_CDFS[br_ctx];
    writer.write_symbol(
        "tile.coeff.y.idtx_low_range",
        (level - 3).min(3) as usize,
        &mut cdf,
        4,
        false,
    );
}

fn write_luma_high_range(
    writer: &mut Av2EntropyWriter,
    pos: usize,
    level: u32,
    hr_level_avg: &mut u32,
) {
    let limits = luma_lf_limits(pos);
    let threshold = if limits { 7 } else { 5 };
    if level <= threshold {
        return;
    }
    let decoded_base = threshold + 1;
    let high_range = level.saturating_sub(decoded_base);
    write_adaptive_high_range_with_context(
        writer,
        "tile.coeff.y.high_range",
        high_range,
        *hr_level_avg,
    );
    *hr_level_avg = (*hr_level_avg + high_range) >> 1;
}

fn write_idtx_high_range(writer: &mut Av2EntropyWriter, level: u32, hr_level_avg: &mut u32) {
    if level <= 5 {
        return;
    }
    let high_range = level - 6;
    write_adaptive_high_range_with_context(
        writer,
        "tile.coeff.y.idtx_high_range",
        high_range,
        *hr_level_avg,
    );
    *hr_level_avg = (*hr_level_avg + high_range) >> 1;
}

fn write_chroma_high_range(
    writer: &mut Av2EntropyWriter,
    plane: Av2ChromaPlane,
    pos: usize,
    level: u32,
    hr_level_avg: &mut u32,
) {
    let limits = chroma_lf_limits(pos);
    let threshold = if limits { 4 } else { 5 };
    if level <= threshold {
        return;
    }
    let decoded_base = if limits { 5 } else { 6 };
    let high_range = level.saturating_sub(decoded_base);
    let name = match plane {
        Av2ChromaPlane::U => "tile.coeff.u.high_range",
        Av2ChromaPlane::V => "tile.coeff.v.high_range",
    };
    write_adaptive_high_range_with_context(writer, name, high_range, *hr_level_avg);
    *hr_level_avg = (*hr_level_avg + high_range) >> 1;
}

fn chroma_nz_map_context(
    levels: &[u32; TX4X4_SAMPLES],
    pos: usize,
    scan_index: usize,
    is_eob_coefficient: bool,
    plane: Av2ChromaPlane,
) -> usize {
    if is_eob_coefficient {
        return get_lower_levels_ctx_eob(scan_index);
    }
    if chroma_lf_limits(pos) {
        return chroma_lower_levels_lf_context(levels, pos, plane);
    }
    chroma_lower_levels_context(levels, pos, plane)
}

fn luma_nz_map_context(
    levels: &[u32; TX4X4_SAMPLES],
    pos: usize,
    scan_index: usize,
    is_eob_coefficient: bool,
) -> usize {
    if is_eob_coefficient {
        return get_lower_levels_ctx_eob(scan_index);
    }
    if luma_lf_limits(pos) {
        return luma_lower_levels_lf_context(levels, pos);
    }
    luma_lower_levels_context(levels, pos)
}

fn get_lower_levels_ctx_eob(scan_index: usize) -> usize {
    if scan_index == 0 {
        0
    } else if scan_index <= TX4X4_SAMPLES / 8 {
        1
    } else if scan_index <= TX4X4_SAMPLES / 4 {
        2
    } else {
        3
    }
}

fn luma_lower_levels_lf_context(levels: &[u32; TX4X4_SAMPLES], pos: usize) -> usize {
    let mag = tx4x4_level_at(levels, pos, 0, 1).min(5)
        + tx4x4_level_at(levels, pos, 1, 0).min(5)
        + tx4x4_level_at(levels, pos, 1, 1).min(5)
        + tx4x4_level_at(levels, pos, 0, 2).min(5)
        + tx4x4_level_at(levels, pos, 2, 0).min(5);
    let row = pos / TX4X4_SIZE;
    let col = pos % TX4X4_SIZE;
    let ctx = (mag + 1) >> 1;
    if pos == 0 {
        return ctx.min(8) as usize;
    }
    if row + col < 2 {
        return ctx.min(6) as usize + 9;
    }
    ctx.min(4) as usize + 16
}

fn luma_lower_levels_context(levels: &[u32; TX4X4_SAMPLES], pos: usize) -> usize {
    if pos == 0 {
        return 0;
    }
    let mag = tx4x4_level_at(levels, pos, 0, 1).min(3)
        + tx4x4_level_at(levels, pos, 1, 0).min(3)
        + tx4x4_level_at(levels, pos, 1, 1).min(3)
        + tx4x4_level_at(levels, pos, 0, 2).min(3)
        + tx4x4_level_at(levels, pos, 2, 0).min(3);
    let row = pos / TX4X4_SIZE;
    let col = pos % TX4X4_SIZE;
    let ctx = ((mag + 1) >> 1).min(4) as usize;
    if row + col < 6 {
        ctx
    } else if row + col < 8 {
        ctx + 5
    } else {
        ctx + 10
    }
}

fn chroma_lower_levels_lf_context(
    levels: &[u32; TX4X4_SAMPLES],
    pos: usize,
    plane: Av2ChromaPlane,
) -> usize {
    let mag = tx4x4_level_at(levels, pos, 0, 1).min(5)
        + tx4x4_level_at(levels, pos, 1, 0).min(5)
        + tx4x4_level_at(levels, pos, 1, 1).min(5);
    let ctx = ((mag + 1) >> 1).min(3) as usize;
    chroma_context_with_plane_offset(ctx, plane)
}

fn chroma_lower_levels_context(
    levels: &[u32; TX4X4_SAMPLES],
    pos: usize,
    plane: Av2ChromaPlane,
) -> usize {
    let mag = tx4x4_level_at(levels, pos, 0, 1).min(3)
        + tx4x4_level_at(levels, pos, 1, 0).min(3)
        + tx4x4_level_at(levels, pos, 1, 1).min(3);
    let ctx = ((mag + 1) >> 1).min(3) as usize;
    chroma_context_with_plane_offset(ctx, plane)
}

fn chroma_context_with_plane_offset(ctx: usize, plane: Av2ChromaPlane) -> usize {
    match plane {
        Av2ChromaPlane::U => ctx,
        Av2ChromaPlane::V => ctx + 4,
    }
}

fn chroma_br_context(levels: &[u32; TX4X4_SAMPLES], pos: usize) -> usize {
    let mag = tx4x4_level_at(levels, pos, 0, 1)
        + tx4x4_level_at(levels, pos, 1, 0)
        + tx4x4_level_at(levels, pos, 1, 1);
    ((mag + 1) >> 1).min(3) as usize
}

fn luma_br_lf_context(levels: &[u32; TX4X4_SAMPLES], pos: usize) -> usize {
    let mag = tx4x4_level_at(levels, pos, 0, 1).min(5)
        + tx4x4_level_at(levels, pos, 1, 0).min(5)
        + tx4x4_level_at(levels, pos, 1, 1).min(5);
    let mag = ((mag + 1) >> 1).min(6) as usize;
    if pos == 0 {
        mag
    } else {
        mag + 7
    }
}

fn luma_br_context(levels: &[u32; TX4X4_SAMPLES], pos: usize) -> usize {
    let mag = tx4x4_level_at(levels, pos, 0, 1).min(5)
        + tx4x4_level_at(levels, pos, 1, 0).min(5)
        + tx4x4_level_at(levels, pos, 1, 1).min(5);
    ((mag + 1) >> 1).min(6) as usize
}

fn idtx_bob_context(scan_index: usize) -> usize {
    if scan_index <= TX4X4_SAMPLES / 8 {
        0
    } else if scan_index <= TX4X4_SAMPLES / 4 {
        1
    } else {
        2
    }
}

fn idtx_upper_levels_context(levels: &[u32; TX4X4_SAMPLES], pos: usize) -> usize {
    let mag = idtx_left_level(levels, pos).min(3) + idtx_above_level(levels, pos).min(3);
    mag.min(6) as usize
}

fn idtx_br_context(levels: &[u32; TX4X4_SAMPLES], pos: usize) -> usize {
    let mag = idtx_left_level(levels, pos).min(5) + idtx_above_level(levels, pos).min(5);
    mag.min(6) as usize
}

fn idtx_sign_context(
    levels: &[u32; TX4X4_SAMPLES],
    coefficients: &[i32; TX4X4_SAMPLES],
    pos: usize,
) -> usize {
    let mut sign_sum = 0i32;
    if let Some(left) = idtx_left_pos(pos).filter(|&left| levels[left] != 0) {
        sign_sum += idtx_sign_value(coefficients[left]);
    }
    if let Some(above) = idtx_above_pos(pos).filter(|&above| levels[above] != 0) {
        sign_sum += idtx_sign_value(coefficients[above]);
    }
    if let Some(above_left) = idtx_above_left_pos(pos).filter(|&above_left| levels[above_left] != 0)
    {
        sign_sum += idtx_sign_value(coefficients[above_left]);
    }
    let mut ctx = if sign_sum > 2 {
        5
    } else if sign_sum < -2 {
        6
    } else if sign_sum > 0 {
        1
    } else if sign_sum < 0 {
        2
    } else {
        0
    };
    if levels[pos] > 3 && ctx != 0 {
        ctx += 2;
    }
    ctx
}

fn idtx_sign_value(coefficient: i32) -> i32 {
    if coefficient < 0 {
        -1
    } else {
        1
    }
}

fn idtx_left_level(levels: &[u32; TX4X4_SAMPLES], pos: usize) -> u32 {
    idtx_left_pos(pos).map_or(0, |left| levels[left].min(127))
}

fn idtx_above_level(levels: &[u32; TX4X4_SAMPLES], pos: usize) -> u32 {
    idtx_above_pos(pos).map_or(0, |above| levels[above].min(127))
}

fn idtx_left_pos(pos: usize) -> Option<usize> {
    if pos % TX4X4_SIZE != 0 {
        Some(pos - 1)
    } else {
        None
    }
}

fn idtx_above_pos(pos: usize) -> Option<usize> {
    if pos >= TX4X4_SIZE {
        Some(pos - TX4X4_SIZE)
    } else {
        None
    }
}

fn idtx_above_left_pos(pos: usize) -> Option<usize> {
    if pos % TX4X4_SIZE != 0 && pos >= TX4X4_SIZE {
        Some(pos - TX4X4_SIZE - 1)
    } else {
        None
    }
}

fn tx4x4_level_at(
    levels: &[u32; TX4X4_SAMPLES],
    pos: usize,
    row_delta: usize,
    col_delta: usize,
) -> u32 {
    let row = pos / TX4X4_SIZE + row_delta;
    let col = pos % TX4X4_SIZE + col_delta;
    if row < TX4X4_SIZE && col < TX4X4_SIZE {
        levels[row * TX4X4_SIZE + col].min(127)
    } else {
        0
    }
}

fn chroma_lf_limits(pos: usize) -> bool {
    let row = pos / TX4X4_SIZE;
    let col = pos % TX4X4_SIZE;
    row + col < 1
}

fn luma_lf_limits(pos: usize) -> bool {
    let row = pos / TX4X4_SIZE;
    let col = pos % TX4X4_SIZE;
    row + col < 4
}

fn lossless_entropy_context(cul_level: u32, dc_val: i32) -> u8 {
    let mut context = cul_level.min(7) as u8;
    if dc_val < 0 {
        context |= 1 << 3;
    } else if dc_val > 0 {
        context += 2 << 3;
    }
    context
}

fn lossless_dc_level_for_sample(sample: u8) -> (u16, bool) {
    let delta = i16::from(sample) - i16::from(LOSSLESS_DC_PREDICTOR);
    let level = delta.unsigned_abs() * 4;
    debug_assert!(level > 0);
    (level, delta < 0)
}

fn nonzero_dc_entropy_context(negative: bool) -> u8 {
    if negative {
        NONZERO_NEGATIVE_DC_ENTROPY_CONTEXT
    } else {
        NONZERO_POSITIVE_DC_ENTROPY_CONTEXT
    }
}

fn write_y_txb_all_zero(writer: &mut Av2EntropyWriter, skip_ctx: u8) {
    let (name, mut cdf) = match skip_ctx {
        1 => (
            "tile.coeff.y.txb_all_zero_tx4x4_ctx1",
            DEFAULT_TXB_SKIP_Y_TX4X4_CTX1_CDF,
        ),
        2 => (
            "tile.coeff.y.txb_all_zero_tx4x4_ctx2",
            DEFAULT_TXB_SKIP_Y_TX4X4_CTX2_CDF,
        ),
        3 => (
            "tile.coeff.y.txb_all_zero_tx4x4_ctx3",
            DEFAULT_TXB_SKIP_Y_TX4X4_CTX3_CDF,
        ),
        4 => (
            "tile.coeff.y.txb_all_zero_tx4x4_ctx4",
            DEFAULT_TXB_SKIP_Y_TX4X4_CTX4_CDF,
        ),
        5 => (
            "tile.coeff.y.txb_all_zero_tx4x4_ctx5",
            DEFAULT_TXB_SKIP_Y_TX4X4_CTX5_CDF,
        ),
        _ => panic!("unsupported AV2 luma TXB skip context {skip_ctx}"),
    };
    writer.write_symbol_with_static_cdf_key(
        name,
        y_txb_skip_static_cdf_key(skip_ctx),
        1,
        &mut cdf,
        2,
        false,
    );
}

fn write_y_txb_nonzero(writer: &mut Av2EntropyWriter, skip_ctx: u8) {
    let (name, mut cdf) = match skip_ctx {
        1 => (
            "tile.coeff.y.txb_nonzero_tx4x4_ctx1",
            DEFAULT_TXB_SKIP_Y_TX4X4_CTX1_CDF,
        ),
        2 => (
            "tile.coeff.y.txb_nonzero_tx4x4_ctx2",
            DEFAULT_TXB_SKIP_Y_TX4X4_CTX2_CDF,
        ),
        3 => (
            "tile.coeff.y.txb_nonzero_tx4x4_ctx3",
            DEFAULT_TXB_SKIP_Y_TX4X4_CTX3_CDF,
        ),
        4 => (
            "tile.coeff.y.txb_nonzero_tx4x4_ctx4",
            DEFAULT_TXB_SKIP_Y_TX4X4_CTX4_CDF,
        ),
        5 => (
            "tile.coeff.y.txb_nonzero_tx4x4_ctx5",
            DEFAULT_TXB_SKIP_Y_TX4X4_CTX5_CDF,
        ),
        _ => panic!("unsupported AV2 luma TXB skip context {skip_ctx}"),
    };
    writer.write_symbol_with_static_cdf_key(
        name,
        y_txb_skip_static_cdf_key(skip_ctx),
        0,
        &mut cdf,
        2,
        false,
    );
}

fn write_y_fsc_txb_all_zero(writer: &mut Av2EntropyWriter) {
    let mut cdf = DEFAULT_TXB_SKIP_Y_FSC_TX4X4_CTX9_CDF;
    writer.write_symbol_with_static_cdf_key(
        "tile.coeff.y.txb_all_zero_fsc_tx4x4_ctx9",
        y_fsc_txb_skip_static_cdf_key(9),
        1,
        &mut cdf,
        2,
        false,
    );
}

fn write_y_fsc_txb_nonzero(writer: &mut Av2EntropyWriter) {
    let mut cdf = DEFAULT_TXB_SKIP_Y_FSC_TX4X4_CTX9_CDF;
    writer.write_symbol_with_static_cdf_key(
        "tile.coeff.y.txb_nonzero_fsc_tx4x4_ctx9",
        y_fsc_txb_skip_static_cdf_key(9),
        0,
        &mut cdf,
        2,
        false,
    );
}

fn write_u_txb_nonzero(writer: &mut Av2EntropyWriter, skip_ctx: u8, use_fsc: bool) {
    let (name, mut cdf) = match skip_ctx {
        6 if use_fsc => (
            "tile.coeff.u.txb_nonzero_fsc_tx4x4_ctx6",
            DEFAULT_TXB_SKIP_U_FSC_TX4X4_CTX6_CDF,
        ),
        6 => (
            "tile.coeff.u.txb_nonzero_tx4x4_ctx6",
            DEFAULT_TXB_SKIP_U_TX4X4_CTX6_CDF,
        ),
        7 if use_fsc => (
            "tile.coeff.u.txb_nonzero_fsc_tx4x4_ctx7",
            DEFAULT_TXB_SKIP_U_FSC_TX4X4_CTX7_CDF,
        ),
        7 => (
            "tile.coeff.u.txb_nonzero_tx4x4_ctx7",
            DEFAULT_TXB_SKIP_U_TX4X4_CTX7_CDF,
        ),
        8 if use_fsc => (
            "tile.coeff.u.txb_nonzero_fsc_tx4x4_ctx8",
            DEFAULT_TXB_SKIP_U_FSC_TX4X4_CTX8_CDF,
        ),
        8 => (
            "tile.coeff.u.txb_nonzero_tx4x4_ctx8",
            DEFAULT_TXB_SKIP_U_TX4X4_CTX8_CDF,
        ),
        _ => panic!("unsupported AV2 U TXB skip context {skip_ctx}"),
    };
    writer.write_symbol_with_static_cdf_key(
        name,
        u_txb_skip_static_cdf_key(skip_ctx, use_fsc),
        0,
        &mut cdf,
        2,
        false,
    );
}

fn write_v_txb_nonzero(writer: &mut Av2EntropyWriter, skip_ctx: u8) {
    let (name, mut cdf) = match skip_ctx {
        0 => (
            "tile.coeff.v.txb_nonzero_tx4x4_ctx0",
            DEFAULT_V_TXB_SKIP_TX4X4_CTX0_CDF,
        ),
        1 => (
            "tile.coeff.v.txb_nonzero_tx4x4_ctx1",
            DEFAULT_V_TXB_SKIP_TX4X4_CTX1_CDF,
        ),
        2 => (
            "tile.coeff.v.txb_nonzero_tx4x4_ctx2",
            DEFAULT_V_TXB_SKIP_TX4X4_CTX2_CDF,
        ),
        3 => (
            "tile.coeff.v.txb_nonzero_tx4x4_ctx3",
            DEFAULT_V_TXB_SKIP_TX4X4_CTX3_CDF,
        ),
        4 => (
            "tile.coeff.v.txb_nonzero_tx4x4_ctx4",
            DEFAULT_V_TXB_SKIP_TX4X4_CTX4_CDF,
        ),
        5 => (
            "tile.coeff.v.txb_nonzero_tx4x4_ctx5",
            DEFAULT_V_TXB_SKIP_TX4X4_CTX5_CDF,
        ),
        6 => (
            "tile.coeff.v.txb_nonzero_tx4x4_ctx6",
            DEFAULT_V_TXB_SKIP_TX4X4_CTX6_CDF,
        ),
        7 => (
            "tile.coeff.v.txb_nonzero_tx4x4_ctx7",
            DEFAULT_V_TXB_SKIP_TX4X4_CTX7_CDF,
        ),
        8 => (
            "tile.coeff.v.txb_nonzero_tx4x4_ctx8",
            DEFAULT_V_TXB_SKIP_TX4X4_CTX8_CDF,
        ),
        9 => (
            "tile.coeff.v.txb_nonzero_tx4x4_ctx9",
            DEFAULT_V_TXB_SKIP_TX4X4_CTX9_CDF,
        ),
        10 => (
            "tile.coeff.v.txb_nonzero_tx4x4_ctx10",
            DEFAULT_V_TXB_SKIP_TX4X4_CTX10_CDF,
        ),
        11 => (
            "tile.coeff.v.txb_nonzero_tx4x4_ctx11",
            DEFAULT_V_TXB_SKIP_TX4X4_CTX11_CDF,
        ),
        _ => panic!("unsupported AV2 V TXB skip context {skip_ctx}"),
    };
    writer.write_symbol_with_static_cdf_key(
        name,
        v_txb_skip_static_cdf_key(skip_ctx),
        0,
        &mut cdf,
        2,
        false,
    );
}

fn write_u_txb_all_zero(writer: &mut Av2EntropyWriter, skip_ctx: u8, use_fsc: bool) {
    let (name, mut cdf) = match skip_ctx {
        6 if use_fsc => (
            "tile.coeff.u.txb_all_zero_fsc_tx4x4_ctx6",
            DEFAULT_TXB_SKIP_U_FSC_TX4X4_CTX6_CDF,
        ),
        6 => (
            "tile.coeff.u.txb_all_zero_tx4x4_ctx6",
            DEFAULT_TXB_SKIP_U_TX4X4_CTX6_CDF,
        ),
        7 if use_fsc => (
            "tile.coeff.u.txb_all_zero_fsc_tx4x4_ctx7",
            DEFAULT_TXB_SKIP_U_FSC_TX4X4_CTX7_CDF,
        ),
        7 => (
            "tile.coeff.u.txb_all_zero_tx4x4_ctx7",
            DEFAULT_TXB_SKIP_U_TX4X4_CTX7_CDF,
        ),
        8 if use_fsc => (
            "tile.coeff.u.txb_all_zero_fsc_tx4x4_ctx8",
            DEFAULT_TXB_SKIP_U_FSC_TX4X4_CTX8_CDF,
        ),
        8 => (
            "tile.coeff.u.txb_all_zero_tx4x4_ctx8",
            DEFAULT_TXB_SKIP_U_TX4X4_CTX8_CDF,
        ),
        _ => panic!("unsupported AV2 U TXB skip context {skip_ctx}"),
    };
    writer.write_symbol_with_static_cdf_key(
        name,
        u_txb_skip_static_cdf_key(skip_ctx, use_fsc),
        1,
        &mut cdf,
        2,
        false,
    );
}

fn write_v_txb_all_zero(writer: &mut Av2EntropyWriter, skip_ctx: u8) {
    let (name, mut cdf) = match skip_ctx {
        0 => (
            "tile.coeff.v.txb_all_zero_tx4x4_ctx0",
            DEFAULT_V_TXB_SKIP_TX4X4_CTX0_CDF,
        ),
        1 => (
            "tile.coeff.v.txb_all_zero_tx4x4_ctx1",
            DEFAULT_V_TXB_SKIP_TX4X4_CTX1_CDF,
        ),
        2 => (
            "tile.coeff.v.txb_all_zero_tx4x4_ctx2",
            DEFAULT_V_TXB_SKIP_TX4X4_CTX2_CDF,
        ),
        3 => (
            "tile.coeff.v.txb_all_zero_tx4x4_ctx3",
            DEFAULT_V_TXB_SKIP_TX4X4_CTX3_CDF,
        ),
        4 => (
            "tile.coeff.v.txb_all_zero_tx4x4_ctx4",
            DEFAULT_V_TXB_SKIP_TX4X4_CTX4_CDF,
        ),
        5 => (
            "tile.coeff.v.txb_all_zero_tx4x4_ctx5",
            DEFAULT_V_TXB_SKIP_TX4X4_CTX5_CDF,
        ),
        6 => (
            "tile.coeff.v.txb_all_zero_tx4x4_ctx6",
            DEFAULT_V_TXB_SKIP_TX4X4_CTX6_CDF,
        ),
        7 => (
            "tile.coeff.v.txb_all_zero_tx4x4_ctx7",
            DEFAULT_V_TXB_SKIP_TX4X4_CTX7_CDF,
        ),
        8 => (
            "tile.coeff.v.txb_all_zero_tx4x4_ctx8",
            DEFAULT_V_TXB_SKIP_TX4X4_CTX8_CDF,
        ),
        9 => (
            "tile.coeff.v.txb_all_zero_tx4x4_ctx9",
            DEFAULT_V_TXB_SKIP_TX4X4_CTX9_CDF,
        ),
        10 => (
            "tile.coeff.v.txb_all_zero_tx4x4_ctx10",
            DEFAULT_V_TXB_SKIP_TX4X4_CTX10_CDF,
        ),
        11 => (
            "tile.coeff.v.txb_all_zero_tx4x4_ctx11",
            DEFAULT_V_TXB_SKIP_TX4X4_CTX11_CDF,
        ),
        _ => panic!("unsupported AV2 V TXB skip context {skip_ctx}"),
    };
    writer.write_symbol_with_static_cdf_key(
        name,
        v_txb_skip_static_cdf_key(skip_ctx),
        1,
        &mut cdf,
        2,
        false,
    );
}

fn write_eob_one_y(writer: &mut Av2EntropyWriter) {
    write_eob_y(writer, 1);
}

fn write_eob_one_uv(writer: &mut Av2EntropyWriter) {
    write_eob_uv(writer, 1);
}

fn write_y_dc_level(writer: &mut Av2EntropyWriter, level: u16) {
    let mut base_cdf = DEFAULT_COEFF_BASE_LF_EOB_Y_TX4X4_CTX0_CDF;
    let base_symbol = usize::from(level.min(5) - 1);
    writer.write_symbol_with_static_cdf_key(
        "tile.coeff.y.dc_base_lf_eob_ctx0",
        AV2_STATIC_CDF_COEFF_Y_DC_BASE_LF_EOB_CTX0,
        base_symbol,
        &mut base_cdf,
        5,
        false,
    );

    if level > 4 {
        let mut low_cdf = DEFAULT_COEFF_LPS_LF_CTX0_CDF;
        let low_symbol = usize::from((level - 1 - 4).min(3));
        writer.write_symbol_with_static_cdf_key(
            "tile.coeff.y.dc_low_range_lf_ctx0",
            AV2_STATIC_CDF_COEFF_Y_DC_LOW_RANGE_LF_CTX0,
            low_symbol,
            &mut low_cdf,
            4,
            false,
        );
    }
}

fn write_uv_dc_level(writer: &mut Av2EntropyWriter, level: u16) {
    let mut base_cdf = DEFAULT_COEFF_BASE_LF_EOB_UV_CTX0_CDF;
    let base_symbol = usize::from(level.min(5) - 1);
    writer.write_symbol_with_static_cdf_key(
        "tile.coeff.uv.dc_base_lf_eob_ctx0",
        AV2_STATIC_CDF_COEFF_UV_DC_BASE_LF_EOB_CTX0,
        base_symbol,
        &mut base_cdf,
        5,
        false,
    );
}

fn write_y_negative_dc_sign(writer: &mut Av2EntropyWriter, dc_sign_ctx: u8) {
    write_y_dc_sign(writer, true, dc_sign_ctx);
}

fn write_y_dc_sign(writer: &mut Av2EntropyWriter, negative: bool, dc_sign_ctx: u8) {
    let (name, mut cdf) = match dc_sign_ctx {
        0 => (
            "tile.coeff.y.dc_sign_negative_ctx0",
            DEFAULT_DC_SIGN_Y_CTX0_CDF,
        ),
        1 => (
            "tile.coeff.y.dc_sign_negative_ctx1",
            DEFAULT_DC_SIGN_Y_CTX1_CDF,
        ),
        2 => (
            "tile.coeff.y.dc_sign_negative_ctx2",
            DEFAULT_DC_SIGN_Y_CTX2_CDF,
        ),
        _ => panic!("unsupported AV2 luma DC sign context {dc_sign_ctx}"),
    };
    writer.write_symbol_with_static_cdf_key(
        name,
        AV2_STATIC_CDF_COEFF_Y_DC_SIGN_BASE + usize::from(dc_sign_ctx),
        usize::from(negative),
        &mut cdf,
        2,
        false,
    );
}

fn write_y_dc_high_range(writer: &mut Av2EntropyWriter, level: u16) {
    if level > 7 {
        write_adaptive_high_range(writer, "tile.coeff.y.dc_high_range", u32::from(level - 8));
    }
}

fn write_uv_dc_high_range(writer: &mut Av2EntropyWriter, level: u16) {
    if level > 4 {
        write_adaptive_high_range(writer, "tile.coeff.uv.dc_high_range", u32::from(level - 5));
    }
}

fn write_adaptive_high_range(writer: &mut Av2EntropyWriter, name: &'static str, value: u32) {
    // AVM write_adaptive_hr() starts every TXB with hr_level_avg=0; the
    // resulting Rice parameter is m=1, k=2, cmax=5 for this DC-only path.
    write_adaptive_high_range_with_context(writer, name, value, 0);
}

fn write_adaptive_high_range_with_context(
    writer: &mut Av2EntropyWriter,
    name: &'static str,
    value: u32,
    context: u32,
) {
    // AV2 v1.0.0 high-range coefficient coding mirrors AVM
    // write_adaptive_hr(): derive Rice parameter m from hr_level_avg, then use
    // truncated Rice with Exp-Golomb order k=m+1 and cmax=min(m+4,6).
    let m = adaptive_high_range_rice_parameter(context);
    write_truncated_rice(writer, name, value, m, m + 1, (m + 4).min(6));
}

fn adaptive_high_range_rice_parameter(context: u32) -> u8 {
    if context < 4 {
        1
    } else if context < 8 {
        2
    } else if context < 16 {
        3
    } else if context < 32 {
        4
    } else if context < 64 {
        5
    } else {
        6
    }
}

fn write_truncated_rice(
    writer: &mut Av2EntropyWriter,
    name: &'static str,
    value: u32,
    m: u8,
    k: u8,
    cmax: u8,
) {
    let q = value >> m;
    if q >= u32::from(cmax) {
        writer.write_literal(name, 0, cmax);
        write_exp_golomb(writer, name, value - (u32::from(cmax) << m), k);
    } else {
        if q > 0 {
            writer.write_literal(name, 0, q as u8);
        }
        writer.write_literal_bit(name, true);
        if m > 0 {
            writer.write_literal(name, value & ((1u32 << m) - 1), m);
        }
    }
}

fn write_exp_golomb(writer: &mut Av2EntropyWriter, name: &'static str, value: u32, k: u8) {
    let x = value + (1u32 << k);
    let length = (u32::BITS - x.leading_zeros()) as u8;
    assert!(length > k, "AV2 Exp-Golomb length must exceed order");
    writer.write_literal(name, 0, length - 1 - k);
    writer.write_literal(name, x, length);
}

fn ceil_log2(value: u32) -> u32 {
    assert!(value > 0, "ceil_log2 expects a positive value");
    if value == 1 {
        0
    } else {
        u32::BITS - (value - 1).leading_zeros()
    }
}

fn luma_txb_skip_context(above: u8, left: u8) -> u8 {
    let top = (above & 7).min(4);
    let left = (left & 7).min(4);
    match (top, left) {
        (0, 0) => 1,
        (0, 1..=2) | (1..=2, 0) | (1, 1) => 2,
        (0, _) | (_, 0) | (1, 2..=3) | (2..=3, 1) | (2, 2) => 3,
        (1..=2, 4) | (4, 1..=2) | (2..=3, 3) | (3, 2..=3) => 4,
        _ => 5,
    }
}

fn chroma_txb_skip_base_context(above: u8, left: u8) -> u8 {
    u8::from(above != 0) + u8::from(left != 0)
}

fn v_txb_skip_context(above: u8, left: u8, last_u_txb_nonzero: bool) -> u8 {
    // AV2 v1.0.0 Section 5.20.7.23 read_tx_block(): AVM get_txb_ctx()
    // offsets V-plane TX_4X4 contexts by three when the 8x8 coding block is
    // larger than the transform block, then av2_read_sig_txtype() adds
    // V_TXB_SKIP_CONTEXT_OFFSET (6) if the retained U-plane EOB flag is set.
    chroma_txb_skip_base_context(above, left) + 3 + if last_u_txb_nonzero { 6 } else { 0 }
}

fn v_txb_skip_context_for_chroma_format(
    above: u8,
    left: u8,
    last_u_txb_nonzero: bool,
    chroma_format: Av2ChromaFormat,
    block_size: Av2MvpBlockSize,
) -> u8 {
    // AV2 v1.0.0 get_txb_ctx() adds half of V_TXB_SKIP_CONTEXT_OFFSET only
    // when the chroma coding block is larger than the TXB. 4:2:0 8x8 luma
    // leaves map to exactly one 4x4 chroma TXB, while larger lossless leaves
    // inherit the same +3 offset as 4:2:2/4:4:4.
    let chroma_block_width = block_size.width / chroma_subsample_x(chroma_format);
    let chroma_block_height = block_size.height / chroma_subsample_y(chroma_format);
    let block_larger_than_txb_offset =
        if chroma_block_width > TX4X4_SIZE || chroma_block_height > TX4X4_SIZE {
            3
        } else {
            0
        };
    chroma_txb_skip_base_context(above, left)
        + block_larger_than_txb_offset
        + if last_u_txb_nonzero { 6 } else { 0 }
}

fn dc_sign_context(above: u8, left: u8) -> u8 {
    let mut sign_sum = entropy_context_dc_sign(above) + entropy_context_dc_sign(left);
    sign_sum = sign_sum.clamp(-32, 32);
    match sign_sum {
        0 => 0,
        -32..=-1 => 1,
        1..=32 => 2,
        _ => unreachable!("AV2 DC sign sum was clamped before context lookup"),
    }
}

fn entropy_context_dc_sign(context: u8) -> i8 {
    match context >> 3 {
        0 => 0,
        1 => -1,
        2 => 1,
        _ => panic!("unsupported AV2 DC sign entropy context {context}"),
    }
}
