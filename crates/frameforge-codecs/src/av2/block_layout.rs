#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Av2MvpBlockSize {
    width: usize,
    height: usize,
}

impl Av2MvpBlockSize {
    const BLOCK_64X64: Self = Self {
        width: 64,
        height: 64,
    };

    fn new(width: usize, height: usize) -> Self {
        assert!(
            is_supported_mvp_block_size(width, height),
            "unsupported AV2 MVP block size {width}x{height}"
        );
        Self { width, height }
    }

    fn mi_width(self) -> usize {
        self.width / MI_SIZE
    }

    fn mi_height(self) -> usize {
        self.height / MI_SIZE
    }

    fn tx4x4_width(self) -> usize {
        self.width / 4
    }

    fn tx4x4_height(self) -> usize {
        self.height / 4
    }

    fn is_square(self) -> bool {
        self.width == self.height
    }

    fn is_tall(self) -> bool {
        self.height > self.width
    }

    fn is_wide(self) -> bool {
        self.width > self.height
    }

    fn is_partition_point(self) -> bool {
        // AVM is_partition_point() returns false for BLOCK_8X64 and
        // BLOCK_64X8 because they live past BLOCK_SIZES in the conversion
        // tables. The MVP path never creates 4xN leaves.
        !matches!((self.width, self.height), (8, 64) | (64, 8))
    }

    fn bsize_map(self) -> usize {
        match (self.width, self.height) {
            (8, 8) => 0,
            (8, 16) | (16, 8) | (16, 16) => 1,
            (16, 32) | (32, 16) | (32, 32) => 2,
            (32, 64) | (64, 32) | (64, 64) => 3,
            (8, 32) => 12,
            (32, 8) => 13,
            (16, 64) => 14,
            (64, 16) => 15,
            (8, 64) | (64, 8) => {
                panic!("AV2 8:1 leaves are not partition context points")
            }
            _ => unreachable!("unsupported AV2 MVP block size"),
        }
    }

    fn bsize_rect_map(self) -> usize {
        match (self.width, self.height) {
            (8, 8) | (16, 16) => 0,
            (8, 16) | (16, 32) => 1,
            (16, 8) | (32, 16) => 2,
            (32, 32) => 3,
            (32, 64) => 4,
            (64, 32) => 5,
            (64, 64) => 6,
            (8, 32) | (16, 64) => 13,
            (32, 8) | (64, 16) => 14,
            (8, 64) | (64, 8) => {
                panic!("AV2 8:1 leaves are not partition context points")
            }
            _ => unreachable!("unsupported AV2 MVP block size"),
        }
    }

    fn fsc_size_group(self) -> Option<usize> {
        // AV2 v1.0.0 allow_fsc_intra() permits intra FSC signalling when
        // enable_idtx_intra is active and both block dimensions are 4..=32.
        if self.width > 32 || self.height > 32 {
            return None;
        }
        Some(match (self.width, self.height) {
            (8, 8) => 2,
            (8, 16) | (16, 8) => 3,
            (16, 16) | (8, 32) | (32, 8) => 4,
            (16, 32) | (32, 16) | (32, 32) => 5,
            _ => unreachable!("unsupported AV2 MVP FSC block size"),
        })
    }

    fn lossless_tx_size_group(self) -> usize {
        match (self.width, self.height) {
            (8, 8) | (8, 16) | (16, 8) | (8, 32) | (32, 8) => 1,
            (16, 16) | (16, 32) | (32, 16) => 2,
            (32, 32) => 3,
            _ => 0,
        }
    }

    fn subsize(self, partition: Av2MvpPartition) -> Option<Self> {
        let (width, height) = self.subsize_dims(partition)?;
        is_supported_mvp_block_size(width, height).then(|| Self::new(width, height))
    }

    fn subsize_dims(self, partition: Av2MvpPartition) -> Option<(usize, usize)> {
        if !self.is_partition_point() {
            return (partition == Av2MvpPartition::None).then_some((self.width, self.height));
        }
        match partition {
            Av2MvpPartition::None => Some((self.width, self.height)),
            Av2MvpPartition::Horz if self.height >= 8 => Some((self.width, self.height / 2)),
            Av2MvpPartition::Vert if self.width >= 8 => Some((self.width / 2, self.height)),
            _ => None,
        }
    }
}

pub(crate) fn av2_mvp_8x8_leaf_order_for_region(
    visible_width: usize,
    visible_height: usize,
) -> Vec<(usize, usize)> {
    assert!(visible_width <= MVP_SUPERBLOCK_SIZE);
    assert!(visible_height <= MVP_SUPERBLOCK_SIZE);
    assert_eq!(visible_width % MVP_LEAF_BLOCK_SIZE, 0);
    assert_eq!(visible_height % MVP_LEAF_BLOCK_SIZE, 0);

    let mut order = Vec::with_capacity(
        (visible_width / MVP_LEAF_BLOCK_SIZE) * (visible_height / MVP_LEAF_BLOCK_SIZE),
    );
    append_8x8_leaf_order(
        0,
        0,
        Av2MvpBlockSize::BLOCK_64X64,
        visible_height / MI_SIZE,
        visible_width / MI_SIZE,
        &mut order,
    );
    order
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Av2MvpLeafRegion {
    pub(crate) x: usize,
    pub(crate) y: usize,
    pub(crate) width: usize,
    pub(crate) height: usize,
}

pub(crate) fn av2_luma_palette_leaf_order_for_region(
    tile_origin_x: usize,
    tile_origin_y: usize,
    visible_width: usize,
    visible_height: usize,
    palette: &Av2LumaPalette444,
) -> Vec<Av2MvpLeafRegion> {
    assert!(visible_width <= MVP_SUPERBLOCK_SIZE);
    assert!(visible_height <= MVP_SUPERBLOCK_SIZE);
    assert_eq!(visible_width % MVP_LEAF_BLOCK_SIZE, 0);
    assert_eq!(visible_height % MVP_LEAF_BLOCK_SIZE, 0);

    let mut order = Vec::new();
    append_luma_palette_leaf_order(
        0,
        0,
        Av2MvpBlockSize::BLOCK_64X64,
        visible_height / MI_SIZE,
        visible_width / MI_SIZE,
        tile_origin_x,
        tile_origin_y,
        palette,
        &mut order,
    );
    order
}

fn append_luma_palette_leaf_order(
    row_mi: usize,
    col_mi: usize,
    block_size: Av2MvpBlockSize,
    visible_rows_mi: usize,
    visible_cols_mi: usize,
    tile_origin_x: usize,
    tile_origin_y: usize,
    palette: &Av2LumaPalette444,
    order: &mut Vec<Av2MvpLeafRegion>,
) {
    if row_mi >= visible_rows_mi || col_mi >= visible_cols_mi {
        return;
    }

    let partition = choose_luma_palette_partition(
        row_mi,
        col_mi,
        block_size,
        visible_rows_mi,
        visible_cols_mi,
        Av2PartitionPolicy::Fixed8x8Leaves,
        Some(palette),
    );
    match partition {
        Av2MvpPartition::None => {
            let x = col_mi * MI_SIZE;
            let y = row_mi * MI_SIZE;
            order.push(Av2MvpLeafRegion {
                x,
                y,
                width: block_size.width.min(visible_cols_mi * MI_SIZE - x),
                height: block_size.height.min(visible_rows_mi * MI_SIZE - y),
            });
        }
        Av2MvpPartition::Horz => {
            let subsize = block_size
                .subsize(partition)
                .expect("AV2 MVP horizontal partition must have a subsize");
            append_luma_palette_leaf_order(
                row_mi,
                col_mi,
                subsize,
                visible_rows_mi,
                visible_cols_mi,
                tile_origin_x,
                tile_origin_y,
                palette,
                order,
            );
            append_luma_palette_leaf_order(
                row_mi + block_size.mi_height() / 2,
                col_mi,
                subsize,
                visible_rows_mi,
                visible_cols_mi,
                tile_origin_x,
                tile_origin_y,
                palette,
                order,
            );
        }
        Av2MvpPartition::Vert => {
            let subsize = block_size
                .subsize(partition)
                .expect("AV2 MVP vertical partition must have a subsize");
            append_luma_palette_leaf_order(
                row_mi,
                col_mi,
                subsize,
                visible_rows_mi,
                visible_cols_mi,
                tile_origin_x,
                tile_origin_y,
                palette,
                order,
            );
            append_luma_palette_leaf_order(
                row_mi,
                col_mi + block_size.mi_width() / 2,
                subsize,
                visible_rows_mi,
                visible_cols_mi,
                tile_origin_x,
                tile_origin_y,
                palette,
                order,
            );
        }
    }
}

fn append_8x8_leaf_order(
    row_mi: usize,
    col_mi: usize,
    block_size: Av2MvpBlockSize,
    visible_rows_mi: usize,
    visible_cols_mi: usize,
    order: &mut Vec<(usize, usize)>,
) {
    if row_mi >= visible_rows_mi || col_mi >= visible_cols_mi {
        return;
    }

    let partition =
        choose_8x8_leaf_partition(row_mi, col_mi, block_size, visible_rows_mi, visible_cols_mi);
    match partition {
        Av2MvpPartition::None => {
            assert_eq!(
                block_size.width, MVP_LEAF_BLOCK_SIZE,
                "AV2 MVP leaf order is only defined for fixed 8x8 leaves"
            );
            assert_eq!(
                block_size.height, MVP_LEAF_BLOCK_SIZE,
                "AV2 MVP leaf order is only defined for fixed 8x8 leaves"
            );
            order.push((col_mi * MI_SIZE, row_mi * MI_SIZE));
        }
        Av2MvpPartition::Horz => {
            let subsize = block_size
                .subsize(partition)
                .expect("AV2 MVP horizontal partition must have a subsize");
            append_8x8_leaf_order(
                row_mi,
                col_mi,
                subsize,
                visible_rows_mi,
                visible_cols_mi,
                order,
            );
            append_8x8_leaf_order(
                row_mi + block_size.mi_height() / 2,
                col_mi,
                subsize,
                visible_rows_mi,
                visible_cols_mi,
                order,
            );
        }
        Av2MvpPartition::Vert => {
            let subsize = block_size
                .subsize(partition)
                .expect("AV2 MVP vertical partition must have a subsize");
            append_8x8_leaf_order(
                row_mi,
                col_mi,
                subsize,
                visible_rows_mi,
                visible_cols_mi,
                order,
            );
            append_8x8_leaf_order(
                row_mi,
                col_mi + block_size.mi_width() / 2,
                subsize,
                visible_rows_mi,
                visible_cols_mi,
                order,
            );
        }
    }
}

fn is_supported_mvp_block_size(width: usize, height: usize) -> bool {
    matches!(
        (width, height),
        (8, 8)
            | (8, 16)
            | (16, 8)
            | (16, 16)
            | (16, 32)
            | (32, 16)
            | (32, 32)
            | (32, 64)
            | (64, 32)
            | (64, 64)
            | (8, 32)
            | (32, 8)
            | (16, 64)
            | (64, 16)
            | (8, 64)
            | (64, 8)
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Av2MvpPartition {
    None,
    Horz,
    Vert,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Av2TileDecisionKind {
    Partition(Av2MvpPartition),
    IntrabcFlag(bool),
    IntrabcCopy {
        drl_idx: u8,
        explicit_dv: Option<Av2IntrabcExplicitDv>,
    },
    IntraLumaMode {
        mode: Av2LumaIntraMode,
        use_dpcm_y: bool,
        dpcm_horz: bool,
        use_fsc: bool,
    },
    IntraChromaMode {
        use_bdpcm_uv: bool,
        luma_mode: Av2LumaIntraMode,
        chroma_intra_mode: Av2ChromaIntraMode,
    },
    LumaPaletteModeInfo,
    LumaPaletteColorMap,
    BlackDcResidualCoefficients,
    LumaPaletteResidualCoefficients {
        luma_bdpcm_horz: Option<bool>,
        chroma_use_bdpcm: bool,
        chroma_intra_mode: Av2ChromaIntraMode,
        use_fsc: bool,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Av2ChromaPlane {
    U,
    V,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Av2TileDecision {
    kind: Av2TileDecisionKind,
    row: usize,
    col: usize,
    block_size: Av2MvpBlockSize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Av2TileRegion {
    pub(crate) origin_x: usize,
    pub(crate) origin_y: usize,
    pub(crate) width: usize,
    pub(crate) height: usize,
}

impl Av2TileRegion {
    #[cfg(test)]
    pub(crate) fn root(geometry: Av2VideoGeometry) -> Self {
        Self {
            origin_x: 0,
            origin_y: 0,
            width: geometry.width,
            height: geometry.height,
        }
    }

    fn geometry(self) -> Av2VideoGeometry {
        Av2VideoGeometry {
            width: self.width,
            height: self.height,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Av2Black444TilePlan {
    decisions: Vec<Av2TileDecision>,
    origin_x: usize,
    origin_y: usize,
    chroma_format: Av2ChromaFormat,
    partition_policy: Av2PartitionPolicy,
    visible_rows_mi: usize,
    visible_cols_mi: usize,
    luma_palette: bool,
    allow_intrabc: bool,
    max_ref_bv_count: usize,
    lossless_partition_features: Option<Av2LosslessPartitionFeatures>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Av2PartitionPolicy {
    Fixed8x8Leaves,
    LargestLosslessLeaves,
    LosslessLeafLimit { max_size: usize },
    LosslessAdaptive32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Av2LosslessPartitionFeatures {
    simple_32x32: Vec<bool>,
    cols_32x32: usize,
}

impl Av2LosslessPartitionFeatures {
    fn allows_larger_leaf(
        &self,
        row_mi: usize,
        col_mi: usize,
        block_size: Av2MvpBlockSize,
    ) -> bool {
        if block_size.width <= 16 && block_size.height <= 16 {
            return true;
        }
        if block_size.width > 32 || block_size.height > 32 {
            return false;
        }

        let x0 = col_mi * MI_SIZE;
        let y0 = row_mi * MI_SIZE;
        let x1 = x0 + block_size.width;
        let y1 = y0 + block_size.height;
        let col0 = x0 / 32;
        let row0 = y0 / 32;
        let col1 = (x1 - 1) / 32;
        let row1 = (y1 - 1) / 32;
        for row in row0..=row1 {
            for col in col0..=col1 {
                let index = row * self.cols_32x32 + col;
                if !self.simple_32x32.get(index).copied().unwrap_or(false) {
                    return false;
                }
            }
        }
        true
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Av2PartitionContext {
    above: Vec<u8>,
    left: Vec<u8>,
}

impl Av2PartitionContext {
    fn new(visible_rows_mi: usize, visible_cols_mi: usize) -> Self {
        Self {
            above: vec![0; visible_cols_mi],
            left: vec![0; visible_rows_mi],
        }
    }

    fn raw_context(&self, row_mi: usize, col_mi: usize, block_size: Av2MvpBlockSize) -> usize {
        let above_shift = block_size.mi_width().ilog2().saturating_sub(1);
        let left_shift = block_size.mi_height().ilog2().saturating_sub(1);
        let above = (self.above[col_mi] >> above_shift) & 1;
        let left = (self.left[row_mi] >> left_shift) & 1;
        usize::from(left * 2 + above)
    }

    fn split_context(&self, row_mi: usize, col_mi: usize, block_size: Av2MvpBlockSize) -> usize {
        self.raw_context(row_mi, col_mi, block_size) + block_size.bsize_map() * 4
    }

    fn rect_context(&self, row_mi: usize, col_mi: usize, block_size: Av2MvpBlockSize) -> usize {
        self.raw_context(row_mi, col_mi, block_size) + block_size.bsize_rect_map() * 4
    }

    fn update_leaf(&mut self, row_mi: usize, col_mi: usize, block_size: Av2MvpBlockSize) {
        // AV2 v1.0.0 Section 9.3 partition context conversion tables, mirrored
        // from AVM partition_context_lookup[] and update_partition_context().
        let (above, left) = partition_context_lookup(block_size);
        for index in col_mi..(col_mi + block_size.mi_width()).min(self.above.len()) {
            self.above[index] = above;
        }
        for index in row_mi..(row_mi + block_size.mi_height()).min(self.left.len()) {
            self.left[index] = left;
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Av2CodedMiContext {
    coded: Vec<bool>,
    rows: usize,
    cols: usize,
}

impl Av2CodedMiContext {
    fn new(visible_rows_mi: usize, visible_cols_mi: usize) -> Self {
        Self {
            coded: vec![false; visible_rows_mi * visible_cols_mi],
            rows: visible_rows_mi,
            cols: visible_cols_mi,
        }
    }

    fn is_coded(&self, row_mi: usize, col_mi: usize) -> bool {
        if row_mi >= self.rows || col_mi >= self.cols {
            return false;
        }
        self.coded[row_mi * self.cols + col_mi]
    }

    fn update_leaf(&mut self, row_mi: usize, col_mi: usize, block_size: Av2MvpBlockSize) {
        for row in row_mi..(row_mi + block_size.mi_height()).min(self.rows) {
            for col in col_mi..(col_mi + block_size.mi_width()).min(self.cols) {
                self.coded[row * self.cols + col] = true;
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Av2PaletteColorCacheContext {
    above: Vec<Option<Vec<Av2Sample>>>,
    left: Vec<Option<Vec<Av2Sample>>>,
}

impl Av2PaletteColorCacheContext {
    fn new(visible_rows_mi: usize, visible_cols_mi: usize) -> Self {
        Self {
            above: vec![None; visible_cols_mi],
            left: vec![None; visible_rows_mi],
        }
    }

    fn cache(&self, row_mi: usize, col_mi: usize) -> Vec<Av2Sample> {
        let above = if row_mi > 0 && row_mi % PARTITION_CONTEXT_DIM != 0 {
            self.above.get(col_mi).and_then(|entry| entry.as_deref())
        } else {
            None
        };
        let left = if col_mi > 0 {
            self.left.get(row_mi).and_then(|entry| entry.as_deref())
        } else {
            None
        };
        av2_palette_cache_from_neighbors(above, left)
    }

    fn update_leaf(
        &mut self,
        row_mi: usize,
        col_mi: usize,
        block_size: Av2MvpBlockSize,
        colors: &[Av2Sample],
    ) {
        let colors = Some(colors.to_vec());
        for col in col_mi..(col_mi + block_size.mi_width()).min(self.above.len()) {
            self.above[col] = colors.clone();
        }
        for row in row_mi..(row_mi + block_size.mi_height()).min(self.left.len()) {
            self.left[row] = colors.clone();
        }
    }

    fn clear_leaf(&mut self, row_mi: usize, col_mi: usize, block_size: Av2MvpBlockSize) {
        // AV2 v1.0.0 palette cache derives from neighboring MB_MODE_INFO
        // palette sizes. IntraBC leaves return before palette_mode_info(), so
        // their palette size is zero for subsequent above/left cache lookups.
        for col in col_mi..(col_mi + block_size.mi_width()).min(self.above.len()) {
            self.above[col] = None;
        }
        for row in row_mi..(row_mi + block_size.mi_height()).min(self.left.len()) {
            self.left[row] = None;
        }
    }
}

fn av2_palette_cache_from_neighbors(
    above: Option<&[Av2Sample]>,
    left: Option<&[Av2Sample]>,
) -> Vec<Av2Sample> {
    let mut cache = Vec::with_capacity(2 * AV2_LUMA_PALETTE_MAX_COLORS);
    let above = above.unwrap_or(&[]);
    let left = left.unwrap_or(&[]);
    let mut above_index = 0usize;
    let mut left_index = 0usize;
    while above_index < above.len() && left_index < left.len() {
        cache.push(above[above_index]);
        above_index += 1;
        cache.push(left[left_index]);
        left_index += 1;
    }
    while above_index < above.len() {
        cache.push(above[above_index]);
        above_index += 1;
    }
    while left_index < left.len() {
        cache.push(left[left_index]);
        left_index += 1;
    }
    cache
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Av2LumaModeContext {
    modes: Vec<Option<Av2LumaIntraMode>>,
    blocks_wide: usize,
    blocks_high: usize,
}

impl Av2LumaModeContext {
    fn new(visible_rows_mi: usize, visible_cols_mi: usize) -> Self {
        let blocks_wide = visible_cols_mi / (MVP_LEAF_BLOCK_SIZE / MI_SIZE);
        let blocks_high = visible_rows_mi / (MVP_LEAF_BLOCK_SIZE / MI_SIZE);
        Self {
            modes: vec![None; blocks_wide * blocks_high],
            blocks_wide,
            blocks_high,
        }
    }

    fn syntax_for_leaf(
        &self,
        row_mi: usize,
        col_mi: usize,
        block_size: Av2MvpBlockSize,
    ) -> Av2LumaModeSyntax {
        let bottom_left_mode = col_mi.checked_sub(1).and_then(|col| {
            self.mode_at_mi(row_mi + block_size.mi_height().saturating_sub(1), col)
        });
        let above_right_mode = row_mi
            .checked_sub(1)
            .and_then(|row| self.mode_at_mi(row, col_mi + block_size.mi_width().saturating_sub(1)));
        av2_luma_mode_syntax_for_block(
            bottom_left_mode,
            above_right_mode,
            block_size.width * block_size.height > 64,
        )
    }

    fn mode_at_mi(&self, row_mi: usize, col_mi: usize) -> Option<Av2LumaIntraMode> {
        let block_row = row_mi / (MVP_LEAF_BLOCK_SIZE / MI_SIZE);
        let block_col = col_mi / (MVP_LEAF_BLOCK_SIZE / MI_SIZE);
        if block_row >= self.blocks_high || block_col >= self.blocks_wide {
            return None;
        }
        self.modes[block_row * self.blocks_wide + block_col]
    }

    fn update_leaf(
        &mut self,
        row_mi: usize,
        col_mi: usize,
        block_size: Av2MvpBlockSize,
        mode: Av2LumaIntraMode,
    ) {
        let block_row = row_mi / (MVP_LEAF_BLOCK_SIZE / MI_SIZE);
        let block_col = col_mi / (MVP_LEAF_BLOCK_SIZE / MI_SIZE);
        let block_rows = block_size.mi_height() / (MVP_LEAF_BLOCK_SIZE / MI_SIZE);
        let block_cols = block_size.mi_width() / (MVP_LEAF_BLOCK_SIZE / MI_SIZE);
        for row in block_row..(block_row + block_rows).min(self.blocks_high) {
            for col in block_col..(block_col + block_cols).min(self.blocks_wide) {
                self.modes[row * self.blocks_wide + col] = Some(mode);
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Av2FscModeContext {
    coded: Vec<bool>,
    use_fsc: Vec<bool>,
    rows: usize,
    cols: usize,
}

impl Av2FscModeContext {
    fn new(visible_rows_mi: usize, visible_cols_mi: usize) -> Self {
        Self {
            coded: vec![false; visible_rows_mi * visible_cols_mi],
            use_fsc: vec![false; visible_rows_mi * visible_cols_mi],
            rows: visible_rows_mi,
            cols: visible_cols_mi,
        }
    }

    fn context(&self, row_mi: usize, col_mi: usize, block_size: Av2MvpBlockSize) -> usize {
        let not_at_sb_top_boundary = row_mi % PARTITION_CONTEXT_DIM != 0;
        let mut count = 0usize;
        let mut sum = 0usize;

        let mut push = |use_fsc: Option<bool>| {
            if count >= 2 {
                return;
            }
            if let Some(use_fsc) = use_fsc {
                sum += usize::from(use_fsc);
                count += 1;
            }
        };

        push(self.bottom_left_state(row_mi, col_mi, block_size));
        if not_at_sb_top_boundary {
            push(self.above_right_state(row_mi, col_mi, block_size));
        }
        push(self.left_state(row_mi, col_mi));
        if not_at_sb_top_boundary {
            push(self.above_state(row_mi, col_mi));
        }
        sum
    }

    fn update_leaf(
        &mut self,
        row_mi: usize,
        col_mi: usize,
        block_size: Av2MvpBlockSize,
        use_fsc: bool,
    ) {
        for row in row_mi..(row_mi + block_size.mi_height()).min(self.rows) {
            for col in col_mi..(col_mi + block_size.mi_width()).min(self.cols) {
                let index = row * self.cols + col;
                self.coded[index] = true;
                self.use_fsc[index] = use_fsc;
            }
        }
    }

    fn state_at(&self, row_mi: usize, col_mi: usize) -> Option<bool> {
        if row_mi >= self.rows || col_mi >= self.cols {
            return None;
        }
        let index = row_mi * self.cols + col_mi;
        self.coded[index].then_some(self.use_fsc[index])
    }

    fn bottom_left_state(
        &self,
        row_mi: usize,
        col_mi: usize,
        block_size: Av2MvpBlockSize,
    ) -> Option<bool> {
        col_mi
            .checked_sub(1)
            .and_then(|col| self.state_at(row_mi + block_size.mi_height().saturating_sub(1), col))
    }

    fn above_right_state(
        &self,
        row_mi: usize,
        col_mi: usize,
        block_size: Av2MvpBlockSize,
    ) -> Option<bool> {
        row_mi
            .checked_sub(1)
            .and_then(|row| self.state_at(row, col_mi + block_size.mi_width().saturating_sub(1)))
    }

    fn left_state(&self, row_mi: usize, col_mi: usize) -> Option<bool> {
        col_mi
            .checked_sub(1)
            .and_then(|col| self.state_at(row_mi, col))
    }

    fn above_state(&self, row_mi: usize, col_mi: usize) -> Option<bool> {
        row_mi
            .checked_sub(1)
            .and_then(|row| self.state_at(row, col_mi))
    }
}
