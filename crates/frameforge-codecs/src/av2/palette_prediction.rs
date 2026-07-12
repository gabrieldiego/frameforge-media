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
        Av2LumaIntraMode::Smooth
        | Av2LumaIntraMode::SmoothVertical
        | Av2LumaIntraMode::SmoothHorizontal => {
            unreachable!("the 4:4:4 palette path does not select smooth luma prediction")
        }
        Av2LumaIntraMode::Paeth
        | Av2LumaIntraMode::Directional45
        | Av2LumaIntraMode::Directional67
        | Av2LumaIntraMode::Directional113
        | Av2LumaIntraMode::Directional135
        | Av2LumaIntraMode::Directional157
        | Av2LumaIntraMode::Directional203
        | Av2LumaIntraMode::DirectionalDelta { .. } => {
            unreachable!("the 4:4:4 palette path does not select this luma prediction mode")
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
        indices[sample_index] = palette_index_for_sample(&colors, sample);
    }

    Av2LumaPaletteBlock444 { colors, indices }
}

fn palette_index_for_sample(colors: &[Av2Sample], sample: Av2Sample) -> u8 {
    match colors.binary_search(&sample) {
        Ok(index) => index as u8,
        Err(0) => 0,
        Err(index) if index == colors.len() => (colors.len() - 1) as u8,
        Err(index) => {
            let previous_index = index - 1;
            let previous_delta = sample.abs_diff(colors[previous_index]);
            let next_delta = sample.abs_diff(colors[index]);
            if previous_delta <= next_delta {
                previous_index as u8
            } else {
                index as u8
            }
        }
    }
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
