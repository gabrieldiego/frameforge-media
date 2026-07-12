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
) -> Av2LumaPaletteBlock444 {
    let stats = LumaPaletteBlockStats::from_samples(samples);

    let unique_colors = stats.len;
    let target_colors = unique_colors
        .clamp(AV2_LUMA_PALETTE_MIN_COLORS, AV2_LUMA_PALETTE_MAX_COLORS)
        .min(AV2_LUMA_PALETTE_SOFT_MAX_COLORS);

    let mut colors = if unique_colors > target_colors {
        quantized_luma_palette_values_compact(
            stats.values(),
            stats.counts(),
            stats.first_positions(),
            target_colors,
        )
    } else {
        stats.values().to_vec()
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

struct LumaPaletteBlockStats {
    values: [Av2Sample; AV2_LUMA_PALETTE_BLOCK_SAMPLES],
    counts: [usize; AV2_LUMA_PALETTE_BLOCK_SAMPLES],
    first_positions: [usize; AV2_LUMA_PALETTE_BLOCK_SAMPLES],
    len: usize,
}

impl LumaPaletteBlockStats {
    fn from_samples(samples: &[Av2Sample; AV2_LUMA_PALETTE_BLOCK_SAMPLES]) -> Self {
        let mut stats = Self {
            values: [0; AV2_LUMA_PALETTE_BLOCK_SAMPLES],
            counts: [0; AV2_LUMA_PALETTE_BLOCK_SAMPLES],
            first_positions: [usize::MAX; AV2_LUMA_PALETTE_BLOCK_SAMPLES],
            len: 0,
        };
        for (sample_index, &sample) in samples.iter().enumerate() {
            if let Some(value_index) = stats.values[..stats.len]
                .iter()
                .position(|&value| value == sample)
            {
                stats.counts[value_index] += 1;
                stats.first_positions[value_index] =
                    stats.first_positions[value_index].min(sample_index);
            } else {
                stats.values[stats.len] = sample;
                stats.counts[stats.len] = 1;
                stats.first_positions[stats.len] = sample_index;
                stats.len += 1;
            }
        }
        stats.sort_by_value();
        stats
    }

    fn sort_by_value(&mut self) {
        for index in 1..self.len {
            let value = self.values[index];
            let count = self.counts[index];
            let first_position = self.first_positions[index];
            let mut insertion = index;
            while insertion > 0 && self.values[insertion - 1] > value {
                self.values[insertion] = self.values[insertion - 1];
                self.counts[insertion] = self.counts[insertion - 1];
                self.first_positions[insertion] = self.first_positions[insertion - 1];
                insertion -= 1;
            }
            self.values[insertion] = value;
            self.counts[insertion] = count;
            self.first_positions[insertion] = first_position;
        }
    }

    fn values(&self) -> &[Av2Sample] {
        &self.values[..self.len]
    }

    fn counts(&self) -> &[usize] {
        &self.counts[..self.len]
    }

    fn first_positions(&self) -> &[usize] {
        &self.first_positions[..self.len]
    }
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

fn quantized_luma_palette_values_compact(
    values: &[Av2Sample],
    counts: &[usize],
    first_positions: &[usize],
    target_colors: usize,
) -> Vec<Av2Sample> {
    debug_assert_eq!(values.len(), counts.len());
    debug_assert_eq!(values.len(), first_positions.len());
    if values.len() <= target_colors {
        return values.to_vec();
    }

    let n = values.len();
    let mut prefix_counts = [0usize; AV2_LUMA_PALETTE_BLOCK_SAMPLES + 1];
    let mut prefix_sums = [0usize; AV2_LUMA_PALETTE_BLOCK_SAMPLES + 1];
    for (index, (&value, &count)) in values.iter().zip(counts).enumerate() {
        prefix_counts[index + 1] = prefix_counts[index] + count;
        prefix_sums[index + 1] = prefix_sums[index] + usize::from(value) * count;
    }

    let mut segment_cost =
        [0usize; AV2_LUMA_PALETTE_BLOCK_SAMPLES * AV2_LUMA_PALETTE_BLOCK_SAMPLES];
    let mut segment_value =
        [0; AV2_LUMA_PALETTE_BLOCK_SAMPLES * AV2_LUMA_PALETTE_BLOCK_SAMPLES];
    for start in 0..n {
        let mut median_index = start;
        for end in start..n {
            let total_count = prefix_counts[end + 1] - prefix_counts[start];
            let median_threshold = total_count.div_ceil(2);
            while prefix_counts[median_index + 1] - prefix_counts[start] < median_threshold {
                median_index += 1;
            }
            let median = values[median_index];
            let median_value = usize::from(median);
            let left_count = prefix_counts[median_index + 1] - prefix_counts[start];
            let left_sum = prefix_sums[median_index + 1] - prefix_sums[start];
            let right_count = prefix_counts[end + 1] - prefix_counts[median_index + 1];
            let right_sum = prefix_sums[end + 1] - prefix_sums[median_index + 1];
            let segment_index = start * AV2_LUMA_PALETTE_BLOCK_SAMPLES + end;
            segment_value[segment_index] = median;
            segment_cost[segment_index] =
                median_value * left_count - left_sum + right_sum - median_value * right_count;
        }
    }

    const DP_ROWS: usize = AV2_LUMA_PALETTE_MAX_COLORS + 1;
    const DP_COLS: usize = AV2_LUMA_PALETTE_BLOCK_SAMPLES + 1;
    let mut dp = [usize::MAX; DP_ROWS * DP_COLS];
    let mut split = [0usize; DP_ROWS * DP_COLS];
    dp[0] = 0;
    for colors in 1..=target_colors {
        for end in colors..=n {
            let dp_index = colors * DP_COLS + end;
            for start in (colors - 1)..end {
                let previous_index = (colors - 1) * DP_COLS + start;
                let segment_index = start * AV2_LUMA_PALETTE_BLOCK_SAMPLES + end - 1;
                let Some(cost) = dp[previous_index].checked_add(segment_cost[segment_index])
                else {
                    continue;
                };
                if cost < dp[dp_index] {
                    dp[dp_index] = cost;
                    split[dp_index] = start;
                }
            }
        }
    }

    let mut colors = Vec::with_capacity(target_colors);
    let mut end = n;
    for color_count in (1..=target_colors).rev() {
        let start = split[color_count * DP_COLS + end];
        colors.push(segment_value[start * AV2_LUMA_PALETTE_BLOCK_SAMPLES + end - 1]);
        end = start;
    }
    colors.reverse();
    colors.sort_unstable();
    colors.dedup();

    if colors.len() < target_colors {
        let mut frequent: Vec<usize> = (0..n).collect();
        frequent.sort_by_key(|&index| (Reverse(counts[index]), first_positions[index], values[index]));
        for index in frequent {
            if colors.len() == target_colors {
                break;
            }
            let value = values[index];
            if !colors.contains(&value) {
                colors.push(value);
            }
        }
        colors.sort_unstable();
    }

    colors
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
    let mut prefix_counts = vec![0usize; n + 1];
    let mut prefix_sums = vec![0usize; n + 1];
    for (index, &value) in values.iter().enumerate() {
        let count = counts[usize::from(value)];
        prefix_counts[index + 1] = prefix_counts[index] + count;
        prefix_sums[index + 1] = prefix_sums[index] + usize::from(value) * count;
    }

    let mut segment_cost = vec![vec![0usize; n]; n];
    let mut segment_value = vec![vec![0; n]; n];
    for start in 0..n {
        let mut median_index = start;
        for end in start..n {
            let total_count = prefix_counts[end + 1] - prefix_counts[start];
            let median_threshold = total_count.div_ceil(2);
            while prefix_counts[median_index + 1] - prefix_counts[start] < median_threshold {
                median_index += 1;
            }
            let median = values[median_index];
            let median_value = usize::from(median);
            let left_count = prefix_counts[median_index + 1] - prefix_counts[start];
            let left_sum = prefix_sums[median_index + 1] - prefix_sums[start];
            let right_count = prefix_counts[end + 1] - prefix_counts[median_index + 1];
            let right_sum = prefix_sums[end + 1] - prefix_sums[median_index + 1];
            segment_value[start][end] = median;
            segment_cost[start][end] =
                median_value * left_count - left_sum + right_sum - median_value * right_count;
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
