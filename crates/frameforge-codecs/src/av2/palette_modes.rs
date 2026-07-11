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
const AV2_LUMA_NON_DIRECTIONAL_MODE_COUNT: usize = 5;
const AV2_LUMA_DIRECTIONAL_MODE_COUNT: usize = 56;
const AV2_LUMA_JOINT_MODE_D45: usize = 8;
const AV2_LUMA_JOINT_MODE_D67: usize = 15;
const AV2_LUMA_JOINT_MODE_V: usize = 22;
const AV2_LUMA_JOINT_MODE_D113: usize = 29;
const AV2_LUMA_JOINT_MODE_D135: usize = 36;
const AV2_LUMA_JOINT_MODE_D157: usize = 43;
const AV2_LUMA_JOINT_MODE_H: usize = 50;
const AV2_LUMA_JOINT_MODE_D203: usize = 57;
const AV2_LUMA_DEFAULT_DIRECTIONAL_MODE_LIST: [usize; AV2_LUMA_DIRECTIONAL_MODE_COUNT] = [
    22, 50, 8, 15, 29, 36, 43, 57, 20, 24, 48, 52, 6, 10, 13, 17, 27, 31, 34, 38, 41, 45, 55, 59,
    21, 23, 49, 51, 7, 9, 14, 16, 28, 30, 35, 37, 42, 44, 56, 58, 19, 25, 47, 53, 5, 11, 12, 18,
    26, 32, 33, 39, 40, 46, 54, 60,
];
const AV2_LUMA_PALETTE_BLOCK_SAMPLES: usize =
    AV2_LUMA_PALETTE_BLOCK_SIZE * AV2_LUMA_PALETTE_BLOCK_SIZE;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Av2LumaDirectionalMode {
    Directional45,
    Directional67,
    Vertical,
    Directional113,
    Directional135,
    Directional157,
    Horizontal,
    Directional203,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Av2LumaIntraMode {
    Dc,
    Smooth,
    SmoothVertical,
    SmoothHorizontal,
    Paeth,
    Directional45,
    Directional67,
    Vertical,
    Directional113,
    Directional135,
    Directional157,
    Horizontal,
    Directional203,
    DirectionalDelta {
        base: Av2LumaDirectionalMode,
        delta: i8,
    },
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
impl Av2LumaDirectionalMode {
    pub(crate) fn base_joint_mode(self) -> usize {
        match self {
            Self::Directional45 => AV2_LUMA_JOINT_MODE_D45,
            Self::Directional67 => AV2_LUMA_JOINT_MODE_D67,
            Self::Vertical => AV2_LUMA_JOINT_MODE_V,
            Self::Directional113 => AV2_LUMA_JOINT_MODE_D113,
            Self::Directional135 => AV2_LUMA_JOINT_MODE_D135,
            Self::Directional157 => AV2_LUMA_JOINT_MODE_D157,
            Self::Horizontal => AV2_LUMA_JOINT_MODE_H,
            Self::Directional203 => AV2_LUMA_JOINT_MODE_D203,
        }
    }

    fn mode_index(self) -> usize {
        match self {
            Self::Directional45 => 5,
            Self::Directional67 => 6,
            Self::Vertical => 7,
            Self::Directional113 => 8,
            Self::Directional135 => 9,
            Self::Directional157 => 10,
            Self::Horizontal => 11,
            Self::Directional203 => 12,
        }
    }

    pub(crate) fn angle(self, delta: i8) -> i16 {
        let base = match self {
            Self::Directional45 => 45,
            Self::Directional67 => 67,
            Self::Vertical => 90,
            Self::Directional113 => 113,
            Self::Directional135 => 135,
            Self::Directional157 => 157,
            Self::Horizontal => 180,
            Self::Directional203 => 203,
        };
        base + i16::from(delta) * 3
    }

    pub(crate) fn chroma_mode(self) -> Av2ChromaIntraMode {
        match self {
            Self::Directional45 => Av2ChromaIntraMode::Directional45,
            Self::Directional67 => Av2ChromaIntraMode::Directional67,
            Self::Vertical => Av2ChromaIntraMode::Vertical,
            Self::Directional113 => Av2ChromaIntraMode::Directional113,
            Self::Directional135 => Av2ChromaIntraMode::Directional135,
            Self::Directional157 => Av2ChromaIntraMode::Directional157,
            Self::Horizontal => Av2ChromaIntraMode::Horizontal,
            Self::Directional203 => Av2ChromaIntraMode::Directional203,
        }
    }
}

impl Av2LumaIntraMode {
    pub(crate) fn mode_index(self) -> usize {
        match self {
            Self::Dc => 0,
            Self::Smooth => 1,
            Self::SmoothVertical => 2,
            Self::SmoothHorizontal => 3,
            Self::Paeth => 4,
            Self::Directional45 => Av2LumaDirectionalMode::Directional45.mode_index(),
            Self::Directional67 => Av2LumaDirectionalMode::Directional67.mode_index(),
            Self::Vertical => Av2LumaDirectionalMode::Vertical.mode_index(),
            Self::Directional113 => Av2LumaDirectionalMode::Directional113.mode_index(),
            Self::Directional135 => Av2LumaDirectionalMode::Directional135.mode_index(),
            Self::Directional157 => Av2LumaDirectionalMode::Directional157.mode_index(),
            Self::Horizontal => Av2LumaDirectionalMode::Horizontal.mode_index(),
            Self::Directional203 => Av2LumaDirectionalMode::Directional203.mode_index(),
            Self::DirectionalDelta { base, .. } => base.mode_index(),
        }
    }

    pub(crate) fn directional(self) -> Option<(Av2LumaDirectionalMode, i8)> {
        match self {
            Self::Directional45 => Some((Av2LumaDirectionalMode::Directional45, 0)),
            Self::Directional67 => Some((Av2LumaDirectionalMode::Directional67, 0)),
            Self::Vertical => Some((Av2LumaDirectionalMode::Vertical, 0)),
            Self::Directional113 => Some((Av2LumaDirectionalMode::Directional113, 0)),
            Self::Directional135 => Some((Av2LumaDirectionalMode::Directional135, 0)),
            Self::Directional157 => Some((Av2LumaDirectionalMode::Directional157, 0)),
            Self::Horizontal => Some((Av2LumaDirectionalMode::Horizontal, 0)),
            Self::Directional203 => Some((Av2LumaDirectionalMode::Directional203, 0)),
            Self::DirectionalDelta { base, delta } => Some((base, delta)),
            Self::Dc
            | Self::Smooth
            | Self::SmoothVertical
            | Self::SmoothHorizontal
            | Self::Paeth => None,
        }
    }

    pub(crate) fn is_directional(self) -> bool {
        self.directional().is_some()
    }

    fn joint_mode(self) -> usize {
        if let Some((base, delta)) = self.directional() {
            debug_assert!((-3..=3).contains(&delta));
            return (base.base_joint_mode() as isize + isize::from(delta)) as usize;
        }
        match self {
            Self::Dc => 0,
            Self::Smooth | Self::SmoothVertical | Self::SmoothHorizontal => 0,
            Self::Paeth => 0,
            Self::Directional45
            | Self::Directional67
            | Self::Vertical
            | Self::Directional113
            | Self::Directional135
            | Self::Directional157
            | Self::Horizontal
            | Self::Directional203
            | Self::DirectionalDelta { .. } => unreachable!("directional modes returned above"),
        }
    }

    pub(crate) fn symbol_name(self) -> &'static str {
        match self {
            Self::Dc => "tile.intra.y_mode_idx_dc",
            Self::Smooth => "tile.intra.y_mode_idx_smooth",
            Self::SmoothVertical => "tile.intra.y_mode_idx_smooth_v",
            Self::SmoothHorizontal => "tile.intra.y_mode_idx_smooth_h",
            Self::Paeth => "tile.intra.y_mode_idx_paeth",
            Self::Directional45 => "tile.intra.y_mode_idx_d45",
            Self::Directional67 => "tile.intra.y_mode_idx_d67",
            Self::Vertical => "tile.intra.y_mode_idx_v",
            Self::Directional113 => "tile.intra.y_mode_idx_d113",
            Self::Directional135 => "tile.intra.y_mode_idx_d135",
            Self::Directional157 => "tile.intra.y_mode_idx_d157",
            Self::Horizontal => "tile.intra.y_mode_idx_h",
            Self::Directional203 => "tile.intra.y_mode_idx_d203",
            Self::DirectionalDelta { base, .. } => match base {
                Av2LumaDirectionalMode::Directional45 => "tile.intra.y_mode_idx_d45_delta",
                Av2LumaDirectionalMode::Directional67 => "tile.intra.y_mode_idx_d67_delta",
                Av2LumaDirectionalMode::Vertical => "tile.intra.y_mode_idx_v_delta",
                Av2LumaDirectionalMode::Directional113 => "tile.intra.y_mode_idx_d113_delta",
                Av2LumaDirectionalMode::Directional135 => "tile.intra.y_mode_idx_d135_delta",
                Av2LumaDirectionalMode::Directional157 => "tile.intra.y_mode_idx_d157_delta",
                Av2LumaDirectionalMode::Horizontal => "tile.intra.y_mode_idx_h_delta",
                Av2LumaDirectionalMode::Directional203 => "tile.intra.y_mode_idx_d203_delta",
            },
        }
    }
}
