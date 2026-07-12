fn chroma_idtx_coeff_score(residual: &[i32; 16]) -> usize {
    // Most screen-content palette leaves use FSC, which writes chroma through
    // IDTX coefficients. Score the sample-domain residuals used by that path
    // instead of the FWHT domain so mode selection matches the coded syntax.
    let mut score = 0usize;
    for &sample_delta in residual {
        score += chroma_idtx_sample_score(sample_delta);
    }
    score
}

fn chroma_idtx_sample_score(sample_delta: i32) -> usize {
    let level = sample_delta.unsigned_abs() as usize;
    if level == 0 {
        return 0;
    }
    AV2_CHROMA_BDPCM_NONZERO_COST + (level.min(255) * AV2_CHROMA_BDPCM_LEVEL_SCALE) / 100
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
