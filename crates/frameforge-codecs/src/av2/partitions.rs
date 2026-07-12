fn choose_partition(
    row_mi: usize,
    col_mi: usize,
    block_size: Av2MvpBlockSize,
    visible_rows_mi: usize,
    visible_cols_mi: usize,
) -> Av2MvpPartition {
    choose_8x8_leaf_partition(row_mi, col_mi, block_size, visible_rows_mi, visible_cols_mi)
}

fn choose_largest_lossless_partition(
    row_mi: usize,
    col_mi: usize,
    block_size: Av2MvpBlockSize,
    visible_rows_mi: usize,
    visible_cols_mi: usize,
) -> Av2MvpPartition {
    if !block_size.is_partition_point() {
        return Av2MvpPartition::None;
    }

    let allowed = allowed_partitions(row_mi, col_mi, block_size, visible_rows_mi, visible_cols_mi);
    if let Some(forced) =
        forced_boundary_partition(row_mi, col_mi, block_size, visible_rows_mi, visible_cols_mi)
    {
        if allowed.contains(forced) {
            return forced;
        }
    }
    if let Some(only_allowed) = allowed.only() {
        return only_allowed;
    }
    if allowed.none {
        return Av2MvpPartition::None;
    }

    choose_8x8_leaf_partition(row_mi, col_mi, block_size, visible_rows_mi, visible_cols_mi)
}

fn choose_lossless_leaf_limit_partition(
    row_mi: usize,
    col_mi: usize,
    block_size: Av2MvpBlockSize,
    visible_rows_mi: usize,
    visible_cols_mi: usize,
    max_size: usize,
) -> Av2MvpPartition {
    assert!(
        matches!(max_size, 8 | 16 | 32),
        "AV2 lossless leaf limits are expected to be 8, 16, or 32"
    );
    if !block_size.is_partition_point() {
        return Av2MvpPartition::None;
    }

    let allowed = allowed_partitions(row_mi, col_mi, block_size, visible_rows_mi, visible_cols_mi);
    if let Some(forced) =
        forced_boundary_partition(row_mi, col_mi, block_size, visible_rows_mi, visible_cols_mi)
    {
        if allowed.contains(forced) {
            return forced;
        }
    }
    if let Some(only_allowed) = allowed.only() {
        return only_allowed;
    }
    if block_size.width <= max_size && block_size.height <= max_size && allowed.none {
        return Av2MvpPartition::None;
    }

    if block_size.width == block_size.height {
        if block_size.height > max_size && allowed.horz {
            return Av2MvpPartition::Horz;
        }
        if block_size.width > max_size && allowed.vert {
            return Av2MvpPartition::Vert;
        }
    } else if block_size.width > block_size.height {
        if block_size.width > max_size && allowed.vert {
            return Av2MvpPartition::Vert;
        }
        if block_size.height > max_size && allowed.horz {
            return Av2MvpPartition::Horz;
        }
    } else {
        if block_size.height > max_size && allowed.horz {
            return Av2MvpPartition::Horz;
        }
        if block_size.width > max_size && allowed.vert {
            return Av2MvpPartition::Vert;
        }
    }

    if allowed.none {
        Av2MvpPartition::None
    } else if allowed.horz {
        Av2MvpPartition::Horz
    } else if allowed.vert {
        Av2MvpPartition::Vert
    } else {
        Av2MvpPartition::None
    }
}
fn choose_luma_palette_partition(
    row_mi: usize,
    col_mi: usize,
    block_size: Av2MvpBlockSize,
    visible_rows_mi: usize,
    visible_cols_mi: usize,
    partition_policy: Av2PartitionPolicy,
    palette: Option<&Av2LumaPalette444>,
) -> Av2MvpPartition {
    if block_size.is_partition_point() {
        let allowed =
            allowed_partitions(row_mi, col_mi, block_size, visible_rows_mi, visible_cols_mi);
        if let Some(forced) =
            forced_boundary_partition(row_mi, col_mi, block_size, visible_rows_mi, visible_cols_mi)
        {
            if allowed.contains(forced) {
                return forced;
            }
        }
        let block_inside_visible = row_mi + block_size.mi_height() <= visible_rows_mi
            && col_mi + block_size.mi_width() <= visible_cols_mi;
        if block_inside_visible
            && allowed.none
            && luma_palette_partition_policy_allows_leaf(partition_policy, block_size)
            && palette.is_some()
            && luma_palette_region_mergeable(block_size)
        {
            return Av2MvpPartition::None;
        }
    }
    choose_8x8_leaf_partition(row_mi, col_mi, block_size, visible_rows_mi, visible_cols_mi)
}

fn luma_palette_partition_policy_allows_leaf(
    partition_policy: Av2PartitionPolicy,
    block_size: Av2MvpBlockSize,
) -> bool {
    match partition_policy {
        Av2PartitionPolicy::Fixed8x8Leaves => {
            block_size.width == MVP_LEAF_BLOCK_SIZE && block_size.height == MVP_LEAF_BLOCK_SIZE
        }
        Av2PartitionPolicy::LargestLosslessLeaves => true,
        Av2PartitionPolicy::LosslessLeafLimit { max_size } => {
            block_size.width <= max_size && block_size.height <= max_size
        }
        Av2PartitionPolicy::LosslessAdaptive32 => false,
    }
}

fn luma_palette_region_mergeable(block_size: Av2MvpBlockSize) -> bool {
    block_size.width >= MVP_LEAF_BLOCK_SIZE && block_size.height >= MVP_LEAF_BLOCK_SIZE
}

fn choose_8x8_leaf_partition(
    row_mi: usize,
    col_mi: usize,
    block_size: Av2MvpBlockSize,
    visible_rows_mi: usize,
    visible_cols_mi: usize,
) -> Av2MvpPartition {
    // AV2 v1.0.0 Section 5.20.3 partition syntax permits recursive binary
    // splits. FrameForge's current AV2 MVP fixes the coding leaf to 8x8; any
    // TX_4X4 symbols later in the residual path are transform blocks only.
    if block_size.width == MVP_LEAF_BLOCK_SIZE && block_size.height == MVP_LEAF_BLOCK_SIZE {
        return Av2MvpPartition::None;
    }
    if !block_size.is_partition_point() {
        return Av2MvpPartition::None;
    }

    let allowed = allowed_partitions(row_mi, col_mi, block_size, visible_rows_mi, visible_cols_mi);
    if let Some(forced) =
        forced_boundary_partition(row_mi, col_mi, block_size, visible_rows_mi, visible_cols_mi)
    {
        if allowed.contains(forced) {
            return forced;
        }
    }
    if let Some(only_allowed) = allowed.only() {
        return only_allowed;
    }

    if block_size.width == block_size.height {
        if block_size.height > MVP_LEAF_BLOCK_SIZE && allowed.horz {
            return Av2MvpPartition::Horz;
        }
        if block_size.width > MVP_LEAF_BLOCK_SIZE && allowed.vert {
            return Av2MvpPartition::Vert;
        }
    } else if block_size.width > block_size.height {
        if block_size.width > MVP_LEAF_BLOCK_SIZE && allowed.vert {
            return Av2MvpPartition::Vert;
        }
        if block_size.height > MVP_LEAF_BLOCK_SIZE && allowed.horz {
            return Av2MvpPartition::Horz;
        }
    } else {
        if block_size.height > MVP_LEAF_BLOCK_SIZE && allowed.horz {
            return Av2MvpPartition::Horz;
        }
        if block_size.width > MVP_LEAF_BLOCK_SIZE && allowed.vert {
            return Av2MvpPartition::Vert;
        }
    }

    if allowed.none {
        Av2MvpPartition::None
    } else if allowed.horz {
        Av2MvpPartition::Horz
    } else if allowed.vert {
        Av2MvpPartition::Vert
    } else {
        Av2MvpPartition::None
    }
}

fn forced_boundary_partition(
    row_mi: usize,
    col_mi: usize,
    block_size: Av2MvpBlockSize,
    visible_rows_mi: usize,
    visible_cols_mi: usize,
) -> Option<Av2MvpPartition> {
    if !block_size.is_partition_point() {
        return Some(Av2MvpPartition::None);
    }

    let hbs_w = block_size.mi_width() / 2;
    let hbs_h = block_size.mi_height() / 2;
    let has_rows = row_mi + hbs_h < visible_rows_mi;
    let has_cols = col_mi + hbs_w < visible_cols_mi;
    if has_rows && has_cols {
        return None;
    }

    // AV2 v1.0.0 partition() boundary derivation, mirrored from AVM
    // av2_get_normative_forced_partition_type() and
    // is_partition_implied_at_boundary().
    if block_size.is_square() {
        Some(if has_rows && !has_cols {
            Av2MvpPartition::Vert
        } else {
            Av2MvpPartition::Horz
        })
    } else if block_size.is_tall() {
        if !has_rows {
            Some(Av2MvpPartition::Horz)
        } else {
            let sub_has_cols = col_mi + block_size.mi_width() / 4 < visible_cols_mi;
            (block_size.mi_width() >= 4 && !sub_has_cols).then_some(Av2MvpPartition::Horz)
        }
    } else {
        assert!(block_size.is_wide());
        if !has_cols {
            Some(Av2MvpPartition::Vert)
        } else {
            let sub_has_rows = row_mi + block_size.mi_height() / 4 < visible_rows_mi;
            (block_size.mi_height() >= 4 && !sub_has_rows).then_some(Av2MvpPartition::Vert)
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Av2AllowedPartitions {
    none: bool,
    horz: bool,
    vert: bool,
}

impl Av2AllowedPartitions {
    fn contains(self, partition: Av2MvpPartition) -> bool {
        match partition {
            Av2MvpPartition::None => self.none,
            Av2MvpPartition::Horz => self.horz,
            Av2MvpPartition::Vert => self.vert,
        }
    }

    fn only(self) -> Option<Av2MvpPartition> {
        let mut count = 0usize;
        let mut partition = Av2MvpPartition::None;
        for candidate in [
            Av2MvpPartition::None,
            Av2MvpPartition::Horz,
            Av2MvpPartition::Vert,
        ] {
            if self.contains(candidate) {
                count += 1;
                partition = candidate;
            }
        }
        (count == 1).then_some(partition)
    }
}

fn allowed_partitions(
    row_mi: usize,
    col_mi: usize,
    block_size: Av2MvpBlockSize,
    visible_rows_mi: usize,
    visible_cols_mi: usize,
) -> Av2AllowedPartitions {
    let has_rows = row_mi + block_size.mi_height() / 2 < visible_rows_mi;
    let has_cols = col_mi + block_size.mi_width() / 2 < visible_cols_mi;
    let mut allowed = Av2AllowedPartitions {
        none: has_rows && has_cols && partition_aspect_allowed(block_size, Av2MvpPartition::None),
        horz: block_size.subsize_dims(Av2MvpPartition::Horz).is_some()
            && rect_type_implied_by_bsize(block_size) != Some(Av2MvpPartition::Vert)
            && partition_aspect_allowed(block_size, Av2MvpPartition::Horz),
        vert: block_size.subsize_dims(Av2MvpPartition::Vert).is_some()
            && rect_type_implied_by_bsize(block_size) != Some(Av2MvpPartition::Horz)
            && partition_aspect_allowed(block_size, Av2MvpPartition::Vert),
    };
    if !allowed.none && !allowed.horz && !allowed.vert {
        allowed.none = true;
    }
    allowed
}

fn rect_type_implied_by_bsize(block_size: Av2MvpBlockSize) -> Option<Av2MvpPartition> {
    match (block_size.width, block_size.height) {
        (8, 32) | (16, 64) | (8, 64) => Some(Av2MvpPartition::Horz),
        (32, 8) | (64, 16) | (64, 8) => Some(Av2MvpPartition::Vert),
        _ => None,
    }
}

fn partition_aspect_allowed(block_size: Av2MvpBlockSize, partition: Av2MvpPartition) -> bool {
    let Some((width, height)) = block_size.subsize_dims(partition) else {
        return false;
    };
    let max_aspect_ratio = 8usize;
    if width > height * max_aspect_ratio || height > width * max_aspect_ratio {
        if partition == Av2MvpPartition::None {
            return false;
        }
        if width >= height * 8 || height >= width * 8 {
            return false;
        }
    }
    true
}

fn write_partition(
    writer: &mut Av2EntropyWriter,
    decision: Av2TileDecision,
    partition: Av2MvpPartition,
    partition_context: &Av2PartitionContext,
    visible_rows_mi: usize,
    visible_cols_mi: usize,
) {
    let allowed = allowed_partitions(
        decision.row,
        decision.col,
        decision.block_size,
        visible_rows_mi,
        visible_cols_mi,
    );
    if forced_boundary_partition(
        decision.row,
        decision.col,
        decision.block_size,
        visible_rows_mi,
        visible_cols_mi,
    )
    .is_some_and(|forced| forced == partition && allowed.contains(forced))
        || allowed.only().is_some()
    {
        return;
    }

    let do_split = partition != Av2MvpPartition::None;
    if allowed.none {
        let ctx = partition_context.split_context(decision.row, decision.col, decision.block_size);
        let mut cdf = DEFAULT_DO_SPLIT_CDFS[ctx];
        writer.write_symbol(
            "tile.partition.do_split",
            usize::from(do_split),
            &mut cdf,
            2,
            false,
        );
    } else {
        assert!(
            do_split,
            "AV2 do_split is implied when PARTITION_NONE is disallowed"
        );
    }
    if !do_split {
        return;
    }

    if allowed.horz && allowed.vert && rect_type_implied_by_bsize(decision.block_size).is_none() {
        let ctx = partition_context.rect_context(decision.row, decision.col, decision.block_size);
        let mut cdf = DEFAULT_RECT_TYPE_CDFS[ctx];
        writer.write_symbol(
            "tile.partition.rect_type",
            usize::from(partition == Av2MvpPartition::Vert),
            &mut cdf,
            2,
            false,
        );
    }
}

fn write_intrabc_flag(
    writer: &mut Av2EntropyWriter,
    decision: Av2TileDecision,
    context: &Av2IntrabcContext,
    use_intrabc: bool,
) {
    // AV2 v1.0.0 intra-frame mode syntax, mirrored from AVM
    // write_mb_modes_kf()/read_intra_frame_mode_info(): when allow_intrabc is
    // set, each non-chroma leaf signals use_intrabc before normal intra modes.
    let ctx = context.intrabc_ctx(decision.row, decision.col, decision.block_size);
    let mut cdf = DEFAULT_INTRABC_CDFS[ctx];
    writer.write_symbol_with_key(
        "tile.intrabc.use_intrabc",
        ctx,
        usize::from(use_intrabc),
        &mut cdf,
        2,
        false,
    );
}

fn write_intrabc_copy(
    writer: &mut Av2EntropyWriter,
    decision: Av2TileDecision,
    context: &Av2IntrabcContext,
    max_ref_bv_count: usize,
    drl_idx: u8,
    explicit_dv: Option<Av2IntrabcExplicitDv>,
) {
    assert!(
        max_ref_bv_count >= 4,
        "AV2 local IntraBC uses default BV candidates 2 and 3"
    );
    assert!(
        usize::from(drl_idx) < max_ref_bv_count,
        "AV2 local IntraBC DRL index is outside the BVP stack"
    );
    let skip_ctx = context.skip_txfm_ctx(decision.row, decision.col, decision.block_size);
    let mut skip_cdf = DEFAULT_SKIP_TXFM_CDFS[skip_ctx];
    writer.write_symbol_with_key(
        "tile.intrabc.skip_txfm",
        skip_ctx,
        1,
        &mut skip_cdf,
        2,
        false,
    );

    // AV2 v1.0.0 read_intrabc_info()/write_intrabc_info(): intrabc_mode=1
    // copies the selected reference BV directly. intrabc_mode=0 reads a
    // differential BV with av2_encode_dv()/ndvc contexts. FrameForge uses
    // mode 0 for exact hash hits when the implicit BVP stack is not yet
    // modeled tightly enough for direct mode.
    let mut mode_cdf = DEFAULT_INTRABC_MODE_CDF;
    writer.write_symbol(
        "tile.intrabc.mode",
        usize::from(explicit_dv.is_none()),
        &mut mode_cdf,
        2,
        false,
    );
    for idx in 0..(max_ref_bv_count - 1) {
        let bit = usize::from(usize::from(drl_idx) != idx);
        writer.write_literal("tile.intrabc.drl_idx", bit as u32, 1);
        if usize::from(drl_idx) == idx {
            break;
        }
    }
    if let Some(dv) = explicit_dv {
        assert_eq!(
            dv.drl_idx, drl_idx,
            "AV2 explicit IntraBC DV and DRL syntax must select the same reference"
        );
        write_intrabc_explicit_dv(writer, dv);
    }
}

fn write_intrabc_explicit_dv(writer: &mut Av2EntropyWriter, dv: Av2IntrabcExplicitDv) {
    // AVM av2_encode_dv() writes a magnitude-only shell vector, then
    // write_intrabc_info() appends row/col sign bits. The frame header forces
    // integer block-vector precision for this MVP path, so no
    // intrabc_bv_precision symbol is present. FrameForge stores IBC vectors
    // in pixel units; AVM stores MV values in eighth-pel units, subtracts the
    // reference there, and then right-shifts the magnitude to one-pel units
    // for the shell syntax.
    let mv_row = i32::from(dv.mv_row) * 8;
    let mv_col = i32::from(dv.mv_col) * 8;
    let ref_row = i32::from(dv.ref_row) * 8;
    let ref_col = i32::from(dv.ref_col) * 8;
    let diff_row = mv_row - ref_row;
    let diff_col = mv_col - ref_col;
    let scaled_row = (diff_row.unsigned_abs() >> 3) as usize;
    let scaled_col = (diff_col.unsigned_abs() >> 3) as usize;
    write_intrabc_dv_magnitude(writer, scaled_row, scaled_col);
    if diff_row != 0 {
        writer.write_literal("tile.intrabc.dv.sign", u32::from(diff_row < 0), 1);
    }
    if diff_col != 0 {
        writer.write_literal("tile.intrabc.dv.sign", u32::from(diff_col < 0), 1);
    }
}

fn write_intrabc_dv_magnitude(writer: &mut Av2EntropyWriter, scaled_row: usize, scaled_col: usize) {
    let shell_index = scaled_row + scaled_col;
    let (shell_class, shell_offset) = if shell_index < 2 {
        (0usize, shell_index)
    } else {
        let class = usize::BITS as usize - 1 - shell_index.leading_zeros() as usize;
        (class, shell_index - (1usize << class))
    };
    let num_shell_classes = 14usize;
    let num_class0 = num_shell_classes >> 1;
    let num_class1 = num_shell_classes - num_class0;

    let mut set_cdf = DEFAULT_NDVC_JOINT_SHELL_SET_CDF;
    if shell_class < num_class0 {
        writer.write_symbol("tile.intrabc.dv.shell_set", 0, &mut set_cdf, 2, false);
        let mut class_cdf = DEFAULT_NDVC_JOINT_SHELL_CLASS0_ONE_PEL_CDF;
        writer.write_symbol(
            "tile.intrabc.dv.shell_class0",
            shell_class,
            &mut class_cdf,
            num_class0,
            false,
        );
    } else {
        writer.write_symbol("tile.intrabc.dv.shell_set", 1, &mut set_cdf, 2, false);
        let mut class_cdf = DEFAULT_NDVC_JOINT_SHELL_CLASS1_ONE_PEL_CDF;
        writer.write_symbol(
            "tile.intrabc.dv.shell_class1",
            shell_class - num_class0,
            &mut class_cdf,
            num_class1,
            false,
        );
    }

    if shell_class < 2 {
        let mut offset_cdf = DEFAULT_NDVC_SHELL_OFFSET_LOW_CLASS_CDFS[shell_class];
        writer.write_symbol(
            "tile.intrabc.dv.shell_offset_low",
            shell_offset,
            &mut offset_cdf,
            2,
            false,
        );
    } else if shell_class == 2 {
        write_intrabc_dv_truncated_unary(writer, 3, shell_offset);
    } else {
        for bit_idx in 0..shell_class {
            let mut offset_cdf = DEFAULT_NDVC_SHELL_OFFSET_OTHER_CLASS_CDFS[bit_idx];
            writer.write_symbol(
                "tile.intrabc.dv.shell_offset",
                (shell_offset >> bit_idx) & 1,
                &mut offset_cdf,
                2,
                false,
            );
        }
    }

    if shell_index > 0 {
        write_intrabc_dv_col_index(writer, shell_class, shell_index, scaled_col);
    }
}

fn write_intrabc_dv_truncated_unary(
    writer: &mut Av2EntropyWriter,
    max_coded_value: usize,
    coded_value: usize,
) {
    for bit_idx in 0..max_coded_value {
        let bit = usize::from(coded_value != bit_idx);
        if bit_idx == 0 {
            let mut cdf = DEFAULT_NDVC_SHELL_OFFSET_CLASS2_CDF;
            writer.write_symbol(
                "tile.intrabc.dv.shell_offset_class2",
                bit,
                &mut cdf,
                2,
                false,
            );
        } else {
            writer.write_literal("tile.intrabc.dv.shell_offset_class2", bit as u32, 1);
        }
        if coded_value == bit_idx {
            break;
        }
    }
}

fn write_intrabc_dv_col_index(
    writer: &mut Av2EntropyWriter,
    shell_class: usize,
    shell_index: usize,
    scaled_col: usize,
) {
    let maximum_pair_index = shell_index >> 1;
    let this_pair_index = if scaled_col <= maximum_pair_index {
        scaled_col
    } else {
        shell_index - scaled_col
    };
    if maximum_pair_index > 0 {
        write_intrabc_dv_col_pair_index(writer, maximum_pair_index, this_pair_index);
    }
    let skip_col_bit = this_pair_index == maximum_pair_index && (shell_index % 2 == 0);
    if !skip_col_bit {
        let context = shell_class.min(3);
        let mut cdf = DEFAULT_NDVC_COL_MV_INDEX_CDFS[context];
        writer.write_symbol(
            "tile.intrabc.dv.col_index",
            usize::from(scaled_col > maximum_pair_index),
            &mut cdf,
            2,
            false,
        );
    }
}

fn write_intrabc_dv_col_pair_index(
    writer: &mut Av2EntropyWriter,
    maximum_pair_index: usize,
    this_pair_index: usize,
) {
    let max_trunc_unary_value = 2usize;
    let max_idx_bits = maximum_pair_index.min(max_trunc_unary_value);
    let coded_col = this_pair_index.min(max_trunc_unary_value);
    for bit_idx in 0..max_idx_bits {
        let context = bit_idx.min(1);
        let mut cdf = DEFAULT_NDVC_COL_MV_GREATER_FLAGS_CDFS[context];
        writer.write_symbol(
            "tile.intrabc.dv.col_gt",
            usize::from(coded_col != bit_idx),
            &mut cdf,
            2,
            false,
        );
        if coded_col == bit_idx {
            break;
        }
    }
    if maximum_pair_index > max_trunc_unary_value && this_pair_index >= max_trunc_unary_value {
        let remainder = this_pair_index - max_trunc_unary_value;
        let remainder_max = maximum_pair_index - max_trunc_unary_value;
        writer.write_uniform(
            "tile.intrabc.dv.col_remainder",
            (remainder_max + 1) as u32,
            remainder as u32,
        );
    }
}

fn write_intra_luma_mode(
    writer: &mut Av2EntropyWriter,
    decision: Av2TileDecision,
    mode: Av2LumaIntraMode,
    mode_context: u8,
    mode_index: u8,
    use_dpcm_y: bool,
    dpcm_horz: bool,
    use_fsc: bool,
    fsc_context: usize,
) {
    let mut dpcm_cdf = DEFAULT_DPCM_CDF;
    // AV2 v1.0.0 Section 5.20.5.5 read_intra_y_mode(): lossless
    // intra blocks signal DPCM usage before luma mode. If selected, AVM maps
    // dpcm_horz=0 to V_PRED and dpcm_horz=1 to H_PRED and skips y_mode_idx.
    writer.write_symbol(
        "tile.intra.use_dpcm_y",
        usize::from(use_dpcm_y),
        &mut dpcm_cdf,
        2,
        false,
    );
    if use_dpcm_y {
        let mut dpcm_direction_cdf = DEFAULT_DPCM_CDF;
        writer.write_symbol(
            "tile.intra.dpcm_y_horz",
            usize::from(dpcm_horz),
            &mut dpcm_direction_cdf,
            2,
            false,
        );
        if let Some(size_group) = decision.block_size.fsc_size_group() {
            let mut fsc_cdf = DEFAULT_FSC_MODE_CDFS[fsc_context.min(2)][size_group];
            writer.write_symbol(
                "tile.intra.fsc_mode",
                usize::from(use_fsc),
                &mut fsc_cdf,
                2,
                false,
            );
        }
        return;
    }

    // AV2 v1.0.0 write_intra_luma_mode()/read_intra_luma_mode() calls
    // get_y_mode_idx_ctx()/get_y_intra_mode_set() before mapping y_mode_idx
    // to a predictor. Large blocks may move H/V beyond the first mode set
    // after neighbor-derived directional modes are inserted.
    let mode_index = usize::from(mode_index);
    let mode_set_index = if mode_index < AV2_LUMA_FIRST_MODE_COUNT {
        0
    } else {
        1 + (mode_index - AV2_LUMA_FIRST_MODE_COUNT) / AV2_LUMA_SECOND_MODE_COUNT
    };
    let mut mode_set_cdf = DEFAULT_Y_MODE_SET_CDF;
    writer.write_symbol(
        "tile.intra.y_mode_set_index",
        mode_set_index,
        &mut mode_set_cdf,
        AV2_LUMA_MODE_SET_COUNT,
        false,
    );
    if mode_set_index != 0 {
        writer.write_literal(
            mode.symbol_name(),
            (mode_index
                - AV2_LUMA_FIRST_MODE_COUNT
                - (mode_set_index - 1) * AV2_LUMA_SECOND_MODE_COUNT) as u32,
            4,
        );
        if let Some(size_group) = decision.block_size.fsc_size_group() {
            let mut fsc_cdf = DEFAULT_FSC_MODE_CDFS[fsc_context.min(2)][size_group];
            writer.write_symbol(
                "tile.intra.fsc_mode",
                usize::from(use_fsc),
                &mut fsc_cdf,
                2,
                false,
            );
        }
        return;
    }

    let mode_context = mode_context.min(2);
    let mode_set_low = mode_index.min(7);
    let mut mode_idx_cdf = DEFAULT_Y_MODE_IDX_CDFS[usize::from(mode_context)];
    writer.write_symbol_with_cdf_key(
        mode.symbol_name(),
        "tile.intra.y_mode_idx",
        usize::from(mode_context),
        mode_set_low,
        &mut mode_idx_cdf,
        8,
        false,
    );
    if mode_set_low == 7 {
        let mut offset_cdf = DEFAULT_Y_MODE_IDX_OFFSET_CDFS[usize::from(mode_context)];
        writer.write_symbol_with_cdf_key(
            mode.symbol_name(),
            "tile.intra.y_mode_idx_offset",
            usize::from(mode_context),
            mode_index - mode_set_low,
            &mut offset_cdf,
            6,
            false,
        );
    }

    if let Some(size_group) = decision.block_size.fsc_size_group() {
        let mut fsc_cdf = DEFAULT_FSC_MODE_CDFS[fsc_context.min(2)][size_group];
        writer.write_symbol(
            "tile.intra.fsc_mode",
            usize::from(use_fsc),
            &mut fsc_cdf,
            2,
            false,
        );
    }
}

fn write_intra_chroma_mode(
    writer: &mut Av2EntropyWriter,
    _decision: Av2TileDecision,
    use_bdpcm_uv: bool,
    luma_mode: Av2LumaIntraMode,
    chroma_intra_mode: Av2ChromaIntraMode,
) {
    let mut dpcm_uv_cdf = DEFAULT_DPCM_CDF;
    // AV2 v1.0.0 Section 5.20.5.6 read_intra_uv_mode() signals chroma DPCM
    // in lossless shared tree blocks. When DPCM is disabled, the same
    // direction flag selects the normal H/V chroma intra mode used by the
    // matching residual predictor.
    writer.write_symbol(
        "tile.intra.use_dpcm_uv",
        usize::from(use_bdpcm_uv),
        &mut dpcm_uv_cdf,
        2,
        false,
    );

    if use_bdpcm_uv {
        let mut dpcm_uv_direction_cdf = DEFAULT_DPCM_CDF;
        writer.write_symbol(
            "tile.intra.dpcm_uv_horz",
            usize::from(chroma_intra_mode.is_horizontal()),
            &mut dpcm_uv_direction_cdf,
            2,
            false,
        );
        return;
    }

    let uv_mode_context = usize::from(luma_mode_is_directional(luma_mode));
    let mut uv_mode_cdf = if uv_mode_context != 0 {
        DEFAULT_UV_MODE_CTX1_CDF
    } else {
        DEFAULT_UV_MODE_CTX0_CDF
    };
    let (name, index) = chroma_uv_mode_symbol(luma_mode, chroma_intra_mode);
    writer.write_symbol_with_cdf_key(
        name,
        "tile.intra.uv_mode_idx",
        uv_mode_context,
        index.min(7),
        &mut uv_mode_cdf,
        8,
        false,
    );
    if index >= 7 {
        writer.write_literal("tile.intra.uv_mode_idx_ext", (index - 7) as u32, 3);
    }
}

fn write_lossless_tx_size_4x4(writer: &mut Av2EntropyWriter, block_size: Av2MvpBlockSize) {
    let bsize_group = block_size.lossless_tx_size_group();
    let mut cdf = DEFAULT_LOSSLESS_TX_SIZE_CDFS[bsize_group];
    writer.write_symbol("tile.lossless_tx_size_4x4", 0, &mut cdf, 2, false);
}

fn luma_mode_is_directional(mode: Av2LumaIntraMode) -> bool {
    mode.is_directional()
}

fn chroma_uv_mode_symbol(
    luma_mode: Av2LumaIntraMode,
    chroma_mode: Av2ChromaIntraMode,
) -> (&'static str, usize) {
    let name = match chroma_mode {
        Av2ChromaIntraMode::Dc => "tile.intra.uv_mode_idx_dc",
        Av2ChromaIntraMode::Vertical => "tile.intra.uv_mode_idx_v",
        Av2ChromaIntraMode::Horizontal => "tile.intra.uv_mode_idx_h",
        Av2ChromaIntraMode::Directional45 => "tile.intra.uv_mode_idx_d45",
        Av2ChromaIntraMode::Directional67 => "tile.intra.uv_mode_idx_d67",
        Av2ChromaIntraMode::Directional135 => "tile.intra.uv_mode_idx_d135",
        Av2ChromaIntraMode::Directional113 => "tile.intra.uv_mode_idx_d113",
        Av2ChromaIntraMode::Directional157 => "tile.intra.uv_mode_idx_d157",
        Av2ChromaIntraMode::Directional203 => "tile.intra.uv_mode_idx_d203",
        Av2ChromaIntraMode::Smooth => "tile.intra.uv_mode_idx_smooth",
        Av2ChromaIntraMode::SmoothVertical => "tile.intra.uv_mode_idx_smooth_v",
        Av2ChromaIntraMode::SmoothHorizontal => "tile.intra.uv_mode_idx_smooth_h",
        Av2ChromaIntraMode::Paeth => "tile.intra.uv_mode_idx_paeth",
    };
    (name, chroma_uv_mode_index(luma_mode, chroma_mode))
}

fn chroma_uv_mode_index(luma_mode: Av2LumaIntraMode, chroma_mode: Av2ChromaIntraMode) -> usize {
    let target = chroma_uv_mode_id(chroma_mode);
    let mut index = 0usize;
    let luma_directional = luma_mode
        .directional()
        .map(|(base, _)| chroma_uv_mode_id(base.chroma_mode()));
    if let Some(mode_id) = luma_directional {
        if target == mode_id {
            return index;
        }
        index += 1;
    }

    for mode_id in [0usize, 9, 10, 11, 12] {
        if target == mode_id {
            return index;
        }
        index += 1;
    }

    for mode_id in DEFAULT_UV_DIRECTIONAL_MODE_LIST {
        if Some(mode_id) == luma_directional {
            continue;
        }
        if target == mode_id {
            return index;
        }
        index += 1;
    }

    unreachable!("supported chroma intra mode must appear in AVM UV mode list")
}

fn chroma_uv_mode_id(mode: Av2ChromaIntraMode) -> usize {
    match mode {
        Av2ChromaIntraMode::Dc => 0,
        Av2ChromaIntraMode::Vertical => 1,
        Av2ChromaIntraMode::Horizontal => 2,
        Av2ChromaIntraMode::Directional45 => 3,
        Av2ChromaIntraMode::Directional67 => 8,
        Av2ChromaIntraMode::Directional135 => 4,
        Av2ChromaIntraMode::Directional113 => 5,
        Av2ChromaIntraMode::Directional157 => 6,
        Av2ChromaIntraMode::Directional203 => 7,
        Av2ChromaIntraMode::Smooth => 9,
        Av2ChromaIntraMode::SmoothVertical => 10,
        Av2ChromaIntraMode::SmoothHorizontal => 11,
        Av2ChromaIntraMode::Paeth => 12,
    }
}
