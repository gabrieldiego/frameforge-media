use super::{
    av2_lossless_dc_predictor, av2_lossless_h_pred_left_edge, av2_lossless_v_pred_above_edge,
    Av2Black444MvpProfile, Av2ChromaFormat, Av2Sample, Av2VideoGeometry,
};
use crate::av2::decision::{decide_leaf_prediction, Av2LeafPredictionMode, Av2LeafResidualMode};
use crate::av2::entropy::{Av2EntropyPayload, Av2EntropyWriter};
use crate::av2::ibc::{Av2IntrabcExplicitDv, Av2LocalIbc444};
use crate::av2::palette::{
    av2_highbd_smooth_intra_predictor, av2_luma_mode_syntax_for_block, Av2ChromaIntraMode,
    Av2LumaIntraMode, Av2LumaModeSyntax, Av2LumaPalette444, AV2_LUMA_PALETTE_BLOCK_SIZE,
    AV2_LUMA_PALETTE_MAX_COLORS, AV2_LUMA_PALETTE_MIN_COLORS,
};
use crate::picture::SampleBitDepth;
use frameforge_core::{read_planar_sample, write_planar_sample};

const MVP_SUPERBLOCK_SIZE: usize = 64;
const MVP_LEAF_BLOCK_SIZE: usize = AV2_LUMA_PALETTE_BLOCK_SIZE;
const MI_SIZE: usize = 4;
const PARTITION_CONTEXT_DIM: usize = MVP_SUPERBLOCK_SIZE / MI_SIZE;
const TX4X4_SIZE: usize = 4;
const TX4X4_SAMPLES: usize = TX4X4_SIZE * TX4X4_SIZE;
const TX4X4_SCAN: [usize; TX4X4_SAMPLES] = [0, 4, 1, 8, 5, 2, 12, 9, 6, 3, 13, 10, 7, 14, 11, 15];
const AVM_CDF_PROB_TOP: u16 = 32768;
const LOSSLESS_DC_PREDICTOR: u8 = 128;
const BLACK_LOSSLESS_DC_LEVEL: u16 = 512;
const NONZERO_NEGATIVE_DC_ENTROPY_CONTEXT: u8 = 15;
const NONZERO_POSITIVE_DC_ENTROPY_CONTEXT: u8 = 23;
// Region merging saves partition syntax, but the current merge test does not
// estimate the larger leaf's palette-map and residual cost. Keep it disabled
// for lossless screenshot content until merge decisions are rate modeled.
const AV2_ENABLE_LUMA_PALETTE_REGION_MERGE: bool = false;

const fn avm_cdf2(a0: u16, p0: i16, p1: i16, p2: i16) -> [u16; 6] {
    [
        AVM_CDF_PROB_TOP - a0,
        0,
        0,
        (p0 + 2) as u16,
        (p1 + 3) as u16,
        (p2 + 4) as u16,
    ]
}

const fn avm_cdf3(a0: u16, a1: u16, p0: i16, p1: i16, p2: i16) -> [u16; 7] {
    [
        AVM_CDF_PROB_TOP - a0,
        AVM_CDF_PROB_TOP - a1,
        0,
        0,
        (p0 + 2) as u16,
        (p1 + 3) as u16,
        (p2 + 4) as u16,
    ]
}

const fn avm_cdf4(a0: u16, a1: u16, a2: u16, p0: i16, p1: i16, p2: i16) -> [u16; 8] {
    [
        AVM_CDF_PROB_TOP - a0,
        AVM_CDF_PROB_TOP - a1,
        AVM_CDF_PROB_TOP - a2,
        0,
        0,
        (p0 + 3) as u16,
        (p1 + 4) as u16,
        (p2 + 5) as u16,
    ]
}

const fn avm_cdf5(a0: u16, a1: u16, a2: u16, a3: u16, p0: i16, p1: i16, p2: i16) -> [u16; 9] {
    [
        AVM_CDF_PROB_TOP - a0,
        AVM_CDF_PROB_TOP - a1,
        AVM_CDF_PROB_TOP - a2,
        AVM_CDF_PROB_TOP - a3,
        0,
        0,
        (p0 + 3) as u16,
        (p1 + 4) as u16,
        (p2 + 5) as u16,
    ]
}

const fn avm_cdf6(
    a0: u16,
    a1: u16,
    a2: u16,
    a3: u16,
    a4: u16,
    p0: i16,
    p1: i16,
    p2: i16,
) -> [u16; 10] {
    [
        AVM_CDF_PROB_TOP - a0,
        AVM_CDF_PROB_TOP - a1,
        AVM_CDF_PROB_TOP - a2,
        AVM_CDF_PROB_TOP - a3,
        AVM_CDF_PROB_TOP - a4,
        0,
        0,
        (p0 + 3) as u16,
        (p1 + 4) as u16,
        (p2 + 5) as u16,
    ]
}

const fn avm_cdf7(
    a0: u16,
    a1: u16,
    a2: u16,
    a3: u16,
    a4: u16,
    a5: u16,
    p0: i16,
    p1: i16,
    p2: i16,
) -> [u16; 11] {
    [
        AVM_CDF_PROB_TOP - a0,
        AVM_CDF_PROB_TOP - a1,
        AVM_CDF_PROB_TOP - a2,
        AVM_CDF_PROB_TOP - a3,
        AVM_CDF_PROB_TOP - a4,
        AVM_CDF_PROB_TOP - a5,
        0,
        0,
        (p0 + 3) as u16,
        (p1 + 4) as u16,
        (p2 + 5) as u16,
    ]
}

const fn avm_cdf2_padded(a0: u16, p0: i16, p1: i16, p2: i16) -> [u16; 12] {
    let mut cdf = [0; 12];
    cdf[0] = AVM_CDF_PROB_TOP - a0;
    cdf[3] = (p0 + 2) as u16;
    cdf[4] = (p1 + 3) as u16;
    cdf[5] = (p2 + 4) as u16;
    cdf
}

const fn avm_cdf3_padded(a0: u16, a1: u16, p0: i16, p1: i16, p2: i16) -> [u16; 12] {
    let mut cdf = [0; 12];
    cdf[0] = AVM_CDF_PROB_TOP - a0;
    cdf[1] = AVM_CDF_PROB_TOP - a1;
    cdf[4] = (p0 + 2) as u16;
    cdf[5] = (p1 + 3) as u16;
    cdf[6] = (p2 + 4) as u16;
    cdf
}

const fn avm_cdf4_padded(a0: u16, a1: u16, a2: u16, p0: i16, p1: i16, p2: i16) -> [u16; 12] {
    let mut cdf = [0; 12];
    cdf[0] = AVM_CDF_PROB_TOP - a0;
    cdf[1] = AVM_CDF_PROB_TOP - a1;
    cdf[2] = AVM_CDF_PROB_TOP - a2;
    cdf[5] = (p0 + 3) as u16;
    cdf[6] = (p1 + 4) as u16;
    cdf[7] = (p2 + 5) as u16;
    cdf
}

const fn avm_cdf5_padded(
    a0: u16,
    a1: u16,
    a2: u16,
    a3: u16,
    p0: i16,
    p1: i16,
    p2: i16,
) -> [u16; 12] {
    let mut cdf = [0; 12];
    cdf[0] = AVM_CDF_PROB_TOP - a0;
    cdf[1] = AVM_CDF_PROB_TOP - a1;
    cdf[2] = AVM_CDF_PROB_TOP - a2;
    cdf[3] = AVM_CDF_PROB_TOP - a3;
    cdf[6] = (p0 + 3) as u16;
    cdf[7] = (p1 + 4) as u16;
    cdf[8] = (p2 + 5) as u16;
    cdf
}

const fn avm_cdf6_padded(
    a0: u16,
    a1: u16,
    a2: u16,
    a3: u16,
    a4: u16,
    p0: i16,
    p1: i16,
    p2: i16,
) -> [u16; 12] {
    let mut cdf = [0; 12];
    cdf[0] = AVM_CDF_PROB_TOP - a0;
    cdf[1] = AVM_CDF_PROB_TOP - a1;
    cdf[2] = AVM_CDF_PROB_TOP - a2;
    cdf[3] = AVM_CDF_PROB_TOP - a3;
    cdf[4] = AVM_CDF_PROB_TOP - a4;
    cdf[7] = (p0 + 3) as u16;
    cdf[8] = (p1 + 4) as u16;
    cdf[9] = (p2 + 5) as u16;
    cdf
}

const fn avm_cdf7_padded(
    a0: u16,
    a1: u16,
    a2: u16,
    a3: u16,
    a4: u16,
    a5: u16,
    p0: i16,
    p1: i16,
    p2: i16,
) -> [u16; 12] {
    let mut cdf = [0; 12];
    cdf[0] = AVM_CDF_PROB_TOP - a0;
    cdf[1] = AVM_CDF_PROB_TOP - a1;
    cdf[2] = AVM_CDF_PROB_TOP - a2;
    cdf[3] = AVM_CDF_PROB_TOP - a3;
    cdf[4] = AVM_CDF_PROB_TOP - a4;
    cdf[5] = AVM_CDF_PROB_TOP - a5;
    cdf[8] = (p0 + 3) as u16;
    cdf[9] = (p1 + 4) as u16;
    cdf[10] = (p2 + 5) as u16;
    cdf
}

const fn avm_cdf8_padded(
    a0: u16,
    a1: u16,
    a2: u16,
    a3: u16,
    a4: u16,
    a5: u16,
    a6: u16,
    p0: i16,
    p1: i16,
    p2: i16,
) -> [u16; 12] {
    let mut cdf = [0; 12];
    cdf[0] = AVM_CDF_PROB_TOP - a0;
    cdf[1] = AVM_CDF_PROB_TOP - a1;
    cdf[2] = AVM_CDF_PROB_TOP - a2;
    cdf[3] = AVM_CDF_PROB_TOP - a3;
    cdf[4] = AVM_CDF_PROB_TOP - a4;
    cdf[5] = AVM_CDF_PROB_TOP - a5;
    cdf[6] = AVM_CDF_PROB_TOP - a6;
    cdf[9] = (p0 + 3) as u16;
    cdf[10] = (p1 + 4) as u16;
    cdf[11] = (p2 + 5) as u16;
    cdf
}

const DEFAULT_DPCM_CDF: [u16; 6] = [16384, 0, 0, 2, 3, 4];
const DEFAULT_INTRABC_CDFS: [[u16; 6]; 3] = [
    avm_cdf2(32085, 0, -1, 0),
    avm_cdf2(15172, -1, -1, 0),
    avm_cdf2(4503, 0, 0, 0),
];
const DEFAULT_INTRABC_MODE_CDF: [u16; 6] = avm_cdf2(29993, 0, -1, -1);
const DEFAULT_NDVC_JOINT_SHELL_SET_CDF: [u16; 6] = avm_cdf2(31579, -1, 0, 0);
const DEFAULT_NDVC_JOINT_SHELL_CLASS0_ONE_PEL_CDF: [u16; 11] =
    avm_cdf7(8680, 13723, 18208, 22686, 26722, 30020, 0, -1, 0);
const DEFAULT_NDVC_JOINT_SHELL_CLASS1_ONE_PEL_CDF: [u16; 11] =
    avm_cdf7(19978, 30160, 32564, 32732, 32736, 32740, 0, 0, -1);
const DEFAULT_NDVC_SHELL_OFFSET_LOW_CLASS_CDFS: [[u16; 6]; 2] =
    [avm_cdf2(14587, -1, -2, -1), avm_cdf2(20966, 1, 0, 0)];
const DEFAULT_NDVC_SHELL_OFFSET_CLASS2_CDF: [u16; 6] = avm_cdf2(13189, 0, 0, 0);
const DEFAULT_NDVC_SHELL_OFFSET_OTHER_CLASS_CDFS: [[u16; 6]; 16] = [
    avm_cdf2(17943, 1, 1, 1),
    avm_cdf2(18934, 1, 1, 1),
    avm_cdf2(18928, 1, 1, 1),
    avm_cdf2(18696, 1, 1, 1),
    avm_cdf2(19044, 1, 1, 1),
    avm_cdf2(20362, 1, 1, 1),
    avm_cdf2(20426, 1, 1, 1),
    avm_cdf2(22563, 1, 1, 1),
    avm_cdf2(22190, 1, 1, 1),
    avm_cdf2(23458, 1, 1, 0),
    avm_cdf2(26227, 0, 0, -2),
    avm_cdf2(30765, -2, 0, 0),
    avm_cdf2(16384, 0, 0, 0),
    avm_cdf2(16384, 0, 0, 0),
    avm_cdf2(16384, 0, 0, 0),
    avm_cdf2(16384, 0, 0, 0),
];
const DEFAULT_NDVC_COL_MV_GREATER_FLAGS_CDFS: [[u16; 6]; 2] =
    [avm_cdf2(5663, -1, 0, 0), avm_cdf2(4856, 1, 1, 0)];
const DEFAULT_NDVC_COL_MV_INDEX_CDFS: [[u16; 6]; 4] = [
    avm_cdf2(13445, 0, 0, -1),
    avm_cdf2(13541, 0, 0, -1),
    avm_cdf2(14045, 0, 0, -1),
    avm_cdf2(12888, -1, -1, -1),
];
const DEFAULT_SKIP_TXFM_CDFS: [[u16; 6]; 6] = [
    avm_cdf2(25865, -1, 0, 0),
    avm_cdf2(14316, 0, 0, 0),
    avm_cdf2(4598, 0, 0, 0),
    avm_cdf2(25612, 0, -1, -1),
    avm_cdf2(12366, 0, 0, -1),
    avm_cdf2(3320, 1, 1, 0),
];
const DEFAULT_LOSSLESS_TX_SIZE_CDFS: [[u16; 6]; 4] = [
    avm_cdf2(16384, 0, 0, -1),
    avm_cdf2(16384, 1, 0, 0),
    avm_cdf2(16384, 1, 0, 0),
    avm_cdf2(16384, 1, 0, 0),
];
const DEFAULT_FSC_MODE_CTX0_CDFS: [[u16; 6]; 6] = [
    avm_cdf2(30503, 0, 0, 1),
    avm_cdf2(31244, 0, 0, 1),
    avm_cdf2(32254, 1, 0, 1),
    avm_cdf2(32324, 1, 1, 1),
    avm_cdf2(32582, 1, 1, 1),
    avm_cdf2(32691, 1, 1, 1),
];
const DEFAULT_FSC_MODE_CDFS: [[[u16; 6]; 6]; 3] = [
    DEFAULT_FSC_MODE_CTX0_CDFS,
    [
        avm_cdf2(27437, 0, 0, 0),
        avm_cdf2(27242, 1, 1, 0),
        avm_cdf2(28040, 1, 0, -1),
        avm_cdf2(27589, 1, 0, -1),
        avm_cdf2(27234, 0, -1, -2),
        avm_cdf2(23583, -2, -2, -2),
    ],
    [
        avm_cdf2(26068, 1, 0, 0),
        avm_cdf2(22635, 1, 0, 0),
        avm_cdf2(22069, 0, -1, -1),
        avm_cdf2(19218, -1, -1, -2),
        avm_cdf2(13701, -1, -1, -1),
        avm_cdf2(4636, -1, -2, 1),
    ],
];
const DEFAULT_DO_SPLIT_CDFS: [[u16; 6]; 64] = [
    avm_cdf2(28084, 0, 0, 1),
    avm_cdf2(23755, 1, 1, 1),
    avm_cdf2(23634, 1, 1, 1),
    avm_cdf2(19368, 0, 0, 1),
    avm_cdf2(24961, 0, 0, 0),
    avm_cdf2(14941, 0, 0, -1),
    avm_cdf2(16154, 0, 0, -1),
    avm_cdf2(5905, 0, 0, 0),
    avm_cdf2(21934, 0, 0, 0),
    avm_cdf2(10440, -1, 0, -1),
    avm_cdf2(11984, -1, -1, -1),
    avm_cdf2(3474, 0, 0, 0),
    avm_cdf2(20492, 0, 1, -1),
    avm_cdf2(6963, 0, -1, -1),
    avm_cdf2(8099, -1, 0, -1),
    avm_cdf2(1529, 0, 0, 0),
    avm_cdf2(24117, 1, 1, -2),
    avm_cdf2(7871, 0, -2, 0),
    avm_cdf2(23604, 0, 0, -2),
    avm_cdf2(8429, -1, -1, 0),
    avm_cdf2(27356, 0, 0, -2),
    avm_cdf2(22441, 0, -1, -2),
    avm_cdf2(8897, -1, -1, -1),
    avm_cdf2(6811, -2, -2, -1),
    avm_cdf2(17592, 0, 1, -1),
    avm_cdf2(5648, -1, -1, -2),
    avm_cdf2(5339, -1, 0, -1),
    avm_cdf2(1082, -1, 0, -1),
    avm_cdf2(26143, 1, 0, -2),
    avm_cdf2(11379, 1, -2, 0),
    avm_cdf2(20142, 1, 1, 1),
    avm_cdf2(7401, 0, -1, 1),
    avm_cdf2(26235, 1, -1, -2),
    avm_cdf2(23674, 1, 0, 1),
    avm_cdf2(12441, 1, 0, -2),
    avm_cdf2(10482, 1, 0, 0),
    avm_cdf2(20663, 0, 0, 0),
    avm_cdf2(4192, -1, 0, -2),
    avm_cdf2(5274, -1, -1, 1),
    avm_cdf2(713, 0, 0, -1),
    avm_cdf2(28255, 1, 0, 0),
    avm_cdf2(27370, 1, 0, 0),
    avm_cdf2(23527, 0, 0, 0),
    avm_cdf2(20990, 0, 0, -1),
    avm_cdf2(26727, 0, 0, 0),
    avm_cdf2(21187, 0, 0, 0),
    avm_cdf2(25324, 0, 0, 0),
    avm_cdf2(17838, 0, 0, 0),
    avm_cdf2(26136, 0, 0, 0),
    avm_cdf2(16591, 0, -1, -1),
    avm_cdf2(19838, 0, 0, -1),
    avm_cdf2(10605, -1, -1, -1),
    avm_cdf2(22914, 0, 0, -1),
    avm_cdf2(12609, -1, -1, -1),
    avm_cdf2(11341, 0, 0, 0),
    avm_cdf2(4556, 0, 0, 0),
    avm_cdf2(24218, 0, 0, -1),
    avm_cdf2(13059, 0, -1, -2),
    avm_cdf2(15378, -1, -1, -2),
    avm_cdf2(5858, -1, -1, -2),
    avm_cdf2(21644, -1, -1, -2),
    avm_cdf2(7767, -1, -1, -1),
    avm_cdf2(8309, 0, -1, -1),
    avm_cdf2(1687, 0, 0, 0),
];
const DEFAULT_RECT_TYPE_CDFS: [[u16; 6]; 64] = [
    avm_cdf2(14644, 0, 0, 0),
    avm_cdf2(10173, 1, 0, 0),
    avm_cdf2(18529, 0, 0, 0),
    avm_cdf2(16071, 1, 1, 0),
    avm_cdf2(20263, 0, 0, -1),
    avm_cdf2(12813, 0, 0, -1),
    avm_cdf2(26612, 0, 0, 0),
    avm_cdf2(23277, 0, 0, -1),
    avm_cdf2(10594, 1, 0, -1),
    avm_cdf2(7000, 1, 0, 0),
    avm_cdf2(20002, 0, 0, -1),
    avm_cdf2(12889, 0, 0, -2),
    avm_cdf2(13854, 1, 0, -1),
    avm_cdf2(10750, 0, 0, -1),
    avm_cdf2(18380, 0, 0, -1),
    avm_cdf2(17505, 0, -1, -1),
    avm_cdf2(14430, 0, -1, -2),
    avm_cdf2(11554, 0, 0, -2),
    avm_cdf2(20078, 0, 0, -1),
    avm_cdf2(19097, 1, 0, -1),
    avm_cdf2(15278, 0, 0, -2),
    avm_cdf2(10137, 0, 0, -1),
    avm_cdf2(21921, 0, -1, -2),
    avm_cdf2(14621, 0, -1, -1),
    avm_cdf2(19330, 0, 0, -2),
    avm_cdf2(15921, 0, 0, -1),
    avm_cdf2(26218, 0, 0, -1),
    avm_cdf2(24318, 0, 0, -1),
    avm_cdf2(16384, 0, 0, 0),
    avm_cdf2(16384, 0, 0, 0),
    avm_cdf2(16384, 0, 0, 0),
    avm_cdf2(16384, 0, 0, 0),
    avm_cdf2(16384, 0, 0, 0),
    avm_cdf2(16384, 0, 0, 0),
    avm_cdf2(16384, 0, 0, 0),
    avm_cdf2(16384, 0, 0, 0),
    avm_cdf2(16066, 1, 0, 1),
    avm_cdf2(9225, 0, 0, -2),
    avm_cdf2(22849, -1, -1, -1),
    avm_cdf2(14817, 0, -2, -1),
    avm_cdf2(16384, 0, 0, 0),
    avm_cdf2(16384, 0, 0, 0),
    avm_cdf2(16384, 0, 0, 0),
    avm_cdf2(16384, 0, 0, 0),
    avm_cdf2(16384, 0, 0, 0),
    avm_cdf2(16384, 0, 0, 0),
    avm_cdf2(16384, 0, 0, 0),
    avm_cdf2(16384, 0, 0, 0),
    avm_cdf2(18543, 1, 0, 0),
    avm_cdf2(13210, 0, -2, 0),
    avm_cdf2(24367, -1, -1, -2),
    avm_cdf2(18417, -1, 0, 0),
    avm_cdf2(24701, 0, -1, -1),
    avm_cdf2(18911, 0, -1, -2),
    avm_cdf2(29590, 0, 0, -1),
    avm_cdf2(27778, 0, -1, -2),
    avm_cdf2(3400, 0, 0, -1),
    avm_cdf2(935, 1, 1, 0),
    avm_cdf2(10365, -1, -1, -2),
    avm_cdf2(1723, 0, 0, -1),
    avm_cdf2(16384, 0, 0, 0),
    avm_cdf2(16384, 0, 0, 0),
    avm_cdf2(16384, 0, 0, 0),
    avm_cdf2(16384, 0, 0, 0),
];
const DEFAULT_Y_MODE_SET_CDF: [u16; 8] = [
    AVM_CDF_PROB_TOP - 28863,
    AVM_CDF_PROB_TOP - 31022,
    AVM_CDF_PROB_TOP - 31724,
    0,
    0,
    4, // AVM_PARA4(1, 1, 1)
    5,
    6,
];
const DEFAULT_Y_MODE_IDX_CTX0_CDF: [u16; 12] = [
    AVM_CDF_PROB_TOP - 15175,
    AVM_CDF_PROB_TOP - 20075,
    AVM_CDF_PROB_TOP - 21728,
    AVM_CDF_PROB_TOP - 24098,
    AVM_CDF_PROB_TOP - 26405,
    AVM_CDF_PROB_TOP - 27655,
    AVM_CDF_PROB_TOP - 28860,
    0,
    0,
    3, // AVM_PARA8(0, -1, 0)
    3,
    5,
];
const DEFAULT_Y_MODE_IDX_CDFS: [[u16; 12]; 3] = [
    DEFAULT_Y_MODE_IDX_CTX0_CDF,
    [
        AVM_CDF_PROB_TOP - 10114,
        AVM_CDF_PROB_TOP - 14957,
        AVM_CDF_PROB_TOP - 16815,
        AVM_CDF_PROB_TOP - 19127,
        AVM_CDF_PROB_TOP - 20147,
        AVM_CDF_PROB_TOP - 25583,
        AVM_CDF_PROB_TOP - 27169,
        0,
        0,
        3, // AVM_PARA8(0, 0, 0)
        4,
        5,
    ],
    [
        AVM_CDF_PROB_TOP - 5636,
        AVM_CDF_PROB_TOP - 9004,
        AVM_CDF_PROB_TOP - 10456,
        AVM_CDF_PROB_TOP - 12122,
        AVM_CDF_PROB_TOP - 12744,
        AVM_CDF_PROB_TOP - 20325,
        AVM_CDF_PROB_TOP - 25607,
        0,
        0,
        3, // AVM_PARA8(0, 0, 0)
        4,
        5,
    ],
];
const DEFAULT_UV_MODE_CTX0_CDF: [u16; 12] = [
    AVM_CDF_PROB_TOP - 9363,
    AVM_CDF_PROB_TOP - 20957,
    AVM_CDF_PROB_TOP - 22865,
    AVM_CDF_PROB_TOP - 24753,
    AVM_CDF_PROB_TOP - 26411,
    AVM_CDF_PROB_TOP - 27983,
    AVM_CDF_PROB_TOP - 30428,
    0,
    0,
    2, // AVM_PARA8(-1, -1, -1)
    3,
    4,
];
const DEFAULT_UV_MODE_CTX1_CDF: [u16; 12] = [
    AVM_CDF_PROB_TOP - 21282,
    AVM_CDF_PROB_TOP - 23610,
    AVM_CDF_PROB_TOP - 28208,
    AVM_CDF_PROB_TOP - 29311,
    AVM_CDF_PROB_TOP - 30348,
    AVM_CDF_PROB_TOP - 31158,
    AVM_CDF_PROB_TOP - 31491,
    0,
    0,
    2, // AVM_PARA8(-1, -1, 0)
    3,
    5,
];
const DEFAULT_UV_DIRECTIONAL_MODE_LIST: [usize; 8] = [1, 2, 3, 4, 8, 5, 6, 7];
const DEFAULT_TXB_SKIP_Y_TX4X4_CTX1_CDF: [u16; 6] = [
    AVM_CDF_PROB_TOP - 1099,
    0,
    0,
    3, // AVM_PARA2(1, 1, 1)
    4,
    5,
];
const DEFAULT_TXB_SKIP_Y_TX4X4_CTX2_CDF: [u16; 6] = avm_cdf2(2762, 0, -1, 0);
const DEFAULT_TXB_SKIP_Y_TX4X4_CTX3_CDF: [u16; 6] = avm_cdf2(7944, -1, 0, -1);
const DEFAULT_TXB_SKIP_Y_TX4X4_CTX4_CDF: [u16; 6] = avm_cdf2(16230, 0, -1, -1);
const DEFAULT_TXB_SKIP_Y_TX4X4_CTX5_CDF: [u16; 6] = avm_cdf2(29076, -1, -1, -1);
const DEFAULT_TXB_SKIP_U_TX4X4_CTX6_CDF: [u16; 6] = [
    AVM_CDF_PROB_TOP - 8898,
    0,
    0,
    2, // AVM_PARA2(0, 0, -1)
    3,
    3,
];
const DEFAULT_TXB_SKIP_U_TX4X4_CTX7_CDF: [u16; 6] = avm_cdf2(13655, 0, 0, -1);
const DEFAULT_TXB_SKIP_U_TX4X4_CTX8_CDF: [u16; 6] = avm_cdf2(22348, 0, 0, 0);
const DEFAULT_TXB_SKIP_U_FSC_TX4X4_CTX6_CDF: [u16; 6] = avm_cdf2(5437, -1, -2, -2);
const DEFAULT_TXB_SKIP_U_FSC_TX4X4_CTX7_CDF: [u16; 6] = avm_cdf2(17819, -1, 0, -1);
const DEFAULT_TXB_SKIP_U_FSC_TX4X4_CTX8_CDF: [u16; 6] = avm_cdf2(28074, 0, -1, 1);
const DEFAULT_TXB_SKIP_Y_FSC_TX4X4_CTX9_CDF: [u16; 6] = avm_cdf2(30432, 1, -1, 0);
const DEFAULT_V_TXB_SKIP_TX4X4_CTX0_CDF: [u16; 6] = avm_cdf2(1439, 1, 0, 1);
const DEFAULT_V_TXB_SKIP_TX4X4_CTX1_CDF: [u16; 6] = avm_cdf2(6191, 0, 0, 0);
const DEFAULT_V_TXB_SKIP_TX4X4_CTX2_CDF: [u16; 6] = avm_cdf2(14610, 0, 0, -1);
const DEFAULT_V_TXB_SKIP_TX4X4_CTX3_CDF: [u16; 6] = avm_cdf2(180, -2, 0, 0);
const DEFAULT_V_TXB_SKIP_TX4X4_CTX4_CDF: [u16; 6] = avm_cdf2(16384, 0, 0, 0);
const DEFAULT_V_TXB_SKIP_TX4X4_CTX5_CDF: [u16; 6] = avm_cdf2(16384, 0, 0, 0);
const DEFAULT_V_TXB_SKIP_TX4X4_CTX6_CDF: [u16; 6] = avm_cdf2(7648, 1, 1, 0);
const DEFAULT_V_TXB_SKIP_TX4X4_CTX7_CDF: [u16; 6] = avm_cdf2(16148, 1, 1, 0);
const DEFAULT_V_TXB_SKIP_TX4X4_CTX8_CDF: [u16; 6] = avm_cdf2(24565, 1, 1, 0);
const DEFAULT_V_TXB_SKIP_TX4X4_CTX9_CDF: [u16; 6] = avm_cdf2(16384, 0, 0, 0);
const DEFAULT_V_TXB_SKIP_TX4X4_CTX10_CDF: [u16; 6] = avm_cdf2(16384, 0, 0, 0);
const DEFAULT_V_TXB_SKIP_TX4X4_CTX11_CDF: [u16; 6] = avm_cdf2(16384, 0, 0, 0);
const DEFAULT_EOB_MULTI16_Y_CTX0_CDF: [u16; 9] = avm_cdf5(1946, 3059, 6834, 15123, 0, -1, -1);
const DEFAULT_EOB_MULTI16_UV_CTX2_CDF: [u16; 9] = avm_cdf5(8000, 10366, 14466, 19569, -1, -1, -1);
const DEFAULT_COEFF_BASE_LF_EOB_Y_TX4X4_CTX0_CDF: [u16; 9] =
    avm_cdf5(27486, 31140, 31779, 32064, 0, -1, -2);
const DEFAULT_COEFF_BASE_EOB_Y_CDFS: [[u16; 7]; 4] = [
    avm_cdf3(10923, 21845, 0, 0, 0),
    avm_cdf3(10923, 21845, 0, 0, 0),
    avm_cdf3(10923, 21845, 0, 0, 0),
    avm_cdf3(25475, 29789, 1, 0, 0),
];
const DEFAULT_COEFF_BASE_LF_EOB_Y_CDFS: [[u16; 9]; 4] = [
    avm_cdf5(27486, 31140, 31779, 32064, 0, -1, -2),
    avm_cdf5(28263, 31142, 31813, 32057, 0, -1, -1),
    avm_cdf5(27578, 30405, 31202, 31448, 0, -1, -1),
    avm_cdf5(29800, 32145, 32589, 32665, 1, 1, 1),
];
const DEFAULT_COEFF_BASE_LF_EOB_UV_CTX0_CDF: [u16; 9] =
    avm_cdf5(28950, 31443, 32009, 32257, 1, 0, 0);
const DEFAULT_EOB_EXTRA_CDF: [u16; 6] = avm_cdf2(16391, 0, 0, 0);
const DEFAULT_COEFF_BASE_EOB_UV_CDFS: [[u16; 7]; 4] = [
    avm_cdf3(10923, 21845, 0, 0, 0),
    avm_cdf3(31214, 32437, 1, 1, 1),
    avm_cdf3(31888, 32447, 1, 0, 1),
    avm_cdf3(30612, 32073, 1, 1, 1),
];
const DEFAULT_COEFF_BASE_LF_EOB_UV_CDFS: [[u16; 9]; 4] = [
    avm_cdf5(28950, 31443, 32009, 32257, 1, 0, 0),
    avm_cdf5(29916, 31919, 32224, 32441, 0, -1, -1),
    avm_cdf5(28902, 30805, 31579, 31816, 0, 0, -2),
    avm_cdf5(6554, 13107, 19661, 26214, 0, 0, 0),
];
const DEFAULT_COEFF_BASE_UV_CDFS: [[u16; 8]; 12] = [
    avm_cdf4(26904, 32102, 32598, 0, 0, 0),
    avm_cdf4(15749, 28898, 31610, 1, 1, 0),
    avm_cdf4(9106, 21329, 26962, 1, 1, 0),
    avm_cdf4(4828, 12923, 18983, 1, 0, 0),
    avm_cdf4(27779, 32406, 32689, 1, 1, 0),
    avm_cdf4(17414, 30077, 32025, 1, 1, 0),
    avm_cdf4(9228, 22296, 27767, 1, -1, -1),
    avm_cdf4(4564, 12734, 19144, 1, 1, 0),
    avm_cdf4(29238, 32489, 32693, 1, -1, 0),
    avm_cdf4(19819, 30853, 32222, -1, 0, 0),
    avm_cdf4(9314, 19318, 25346, 0, -1, -1),
    avm_cdf4(3060, 10265, 16088, 0, -1, 0),
];
const DEFAULT_COEFF_BASE_LF_UV_CDFS: [[u16; 10]; 12] = [
    avm_cdf6(14076, 26464, 29938, 31308, 31828, 0, -1, -1),
    avm_cdf6(7520, 21227, 27766, 30312, 31477, 1, 0, 0),
    avm_cdf6(4377, 13290, 19811, 24220, 27064, 1, 1, 0),
    avm_cdf6(1682, 5139, 8601, 11973, 15046, 1, 1, 0),
    avm_cdf6(15235, 28605, 31367, 32151, 32451, 0, -1, -1),
    avm_cdf6(10256, 24586, 29775, 31465, 32137, 1, 1, 1),
    avm_cdf6(5918, 15629, 22317, 26602, 29101, 1, 1, 0),
    avm_cdf6(2015, 5704, 9835, 13705, 17299, 1, 0, -1),
    avm_cdf6(26420, 31955, 32312, 32430, 32526, 1, 0, 0),
    avm_cdf6(16374, 29560, 31531, 32023, 32291, -1, -1, 0),
    avm_cdf6(7197, 15954, 20986, 24934, 27737, 0, -1, -1),
    avm_cdf6(4820, 9488, 11701, 14065, 16248, 0, -2, -1),
];
const DEFAULT_COEFF_BASE_Y_CDFS: [[u16; 8]; 20] = [
    avm_cdf4(12360, 26392, 29943, -1, 0, -1),
    avm_cdf4(7246, 19496, 26530, 1, 0, 0),
    avm_cdf4(4008, 12605, 18928, 1, 1, 1),
    avm_cdf4(3148, 9393, 14900, 1, 1, 1),
    avm_cdf4(2543, 7526, 12021, 1, 1, 1),
    avm_cdf4(8192, 16384, 24576, 0, 0, 0),
    avm_cdf4(8192, 16384, 24576, 0, 0, 0),
    avm_cdf4(8192, 16384, 24576, 0, 0, 0),
    avm_cdf4(8192, 16384, 24576, 0, 0, 0),
    avm_cdf4(8192, 16384, 24576, 0, 0, 0),
    avm_cdf4(8192, 16384, 24576, 0, 0, 0),
    avm_cdf4(8192, 16384, 24576, 0, 0, 0),
    avm_cdf4(8192, 16384, 24576, 0, 0, 0),
    avm_cdf4(8192, 16384, 24576, 0, 0, 0),
    avm_cdf4(8192, 16384, 24576, 0, 0, 0),
    avm_cdf4(28014, 31534, 32060, 0, -1, 0),
    avm_cdf4(13135, 23487, 28599, 0, 0, 0),
    avm_cdf4(7049, 15368, 20768, 1, 1, 1),
    avm_cdf4(3109, 8054, 12383, 1, 0, 0),
    avm_cdf4(8192, 16384, 24576, 0, 0, 0),
];
const DEFAULT_COEFF_BASE_LF_Y_CDFS: [[u16; 10]; 21] = [
    avm_cdf6(5461, 10923, 16384, 21845, 27307, 0, 0, 0),
    avm_cdf6(1828, 16851, 24012, 28649, 30422, -1, 1, 1),
    avm_cdf6(6923, 16016, 21706, 27149, 29436, 0, 0, 1),
    avm_cdf6(5490, 8820, 15814, 20244, 24021, 1, 1, -2),
    avm_cdf6(3032, 8030, 13087, 17462, 21741, 0, 0, 0),
    avm_cdf6(2261, 6418, 9159, 11973, 15591, 1, 0, 1),
    avm_cdf6(2300, 5287, 8547, 12143, 15837, 1, 1, 0),
    avm_cdf6(1698, 5197, 8275, 11449, 12212, 0, 1, 1),
    avm_cdf6(588, 2906, 4192, 5998, 7090, 1, 1, 1),
    avm_cdf6(12754, 29010, 31539, 32136, 32523, 1, 0, 0),
    avm_cdf6(7974, 23312, 28743, 31187, 32129, 1, 0, 1),
    avm_cdf6(6004, 17753, 25489, 28906, 30692, 1, 0, 1),
    avm_cdf6(5318, 12906, 20831, 25848, 28911, 1, 1, 1),
    avm_cdf6(3337, 10161, 16413, 20903, 24729, 1, 1, 1),
    avm_cdf6(2632, 8256, 13389, 18349, 22057, 1, 1, 1),
    avm_cdf6(1647, 4981, 8018, 10713, 12930, 1, 0, 1),
    avm_cdf6(17458, 29871, 32000, 32546, 32702, 1, 1, 1),
    avm_cdf6(10512, 24503, 29646, 31529, 32218, 1, 1, 1),
    avm_cdf6(6509, 17436, 24062, 28298, 30439, 1, 1, 1),
    avm_cdf6(4334, 12843, 19639, 24807, 27809, 1, 1, 0),
    avm_cdf6(2763, 7942, 12551, 16873, 20575, 1, 1, 1),
];
const DEFAULT_COEFF_BR_UV_CDFS: [[u16; 8]; 4] = [
    avm_cdf4(20014, 26541, 29552, 0, -1, -2),
    avm_cdf4(20674, 27680, 30329, 1, 0, 1),
    avm_cdf4(16228, 24293, 28314, 1, 0, 0),
    avm_cdf4(9580, 16283, 20959, 1, 0, 0),
];
const DEFAULT_COEFF_BASE_BOB_IDTX_CDFS: [[u16; 7]; 3] = [
    avm_cdf3(9917, 17060, 1, 1, 0),
    avm_cdf3(13841, 21928, 1, 1, -1),
    avm_cdf3(11228, 19107, 1, 1, 0),
];
const DEFAULT_COEFF_BASE_IDTX_CDFS: [[u16; 8]; 7] = [
    avm_cdf4(28343, 29890, 30977, 1, 1, 1),
    avm_cdf4(20601, 26193, 28764, 1, 1, 1),
    avm_cdf4(19490, 23791, 27048, 1, 0, 0),
    avm_cdf4(16423, 19493, 22007, 1, 0, 1),
    avm_cdf4(12176, 17688, 21070, 0, 0, -1),
    avm_cdf4(11254, 15066, 18960, 0, 0, -1),
    avm_cdf4(7135, 9594, 11748, 0, 0, -1),
];
const DEFAULT_COEFF_BR_IDTX_CDFS: [[u16; 8]; 7] = [
    avm_cdf4(10358, 16536, 21006, 0, 1, 0),
    avm_cdf4(10820, 18219, 22881, 1, 1, -1),
    avm_cdf4(10100, 15687, 20193, 1, 0, 0),
    avm_cdf4(10388, 15552, 19869, 1, 0, 0),
    avm_cdf4(7467, 14671, 18379, 1, 1, 0),
    avm_cdf4(5068, 8607, 12235, 0, 1, 0),
    avm_cdf4(3545, 6569, 9269, 1, 1, 1),
];
const DEFAULT_IDTX_SIGN_CDFS: [[u16; 6]; 9] = [
    avm_cdf2(15560, 1, 1, 1),
    avm_cdf2(24775, 1, 1, 1),
    avm_cdf2(7540, 1, 1, 1),
    avm_cdf2(27844, 0, 0, 0),
    avm_cdf2(3545, 1, 0, 0),
    avm_cdf2(28880, 1, -1, -2),
    avm_cdf2(4886, 1, -1, 0),
    avm_cdf2(32178, 0, -1, -1),
    avm_cdf2(1204, 1, 0, 0),
];
const DEFAULT_COEFF_BR_Y_CDFS: [[u16; 8]; 7] = [
    avm_cdf4(22305, 28743, 30345, 0, -1, -1),
    avm_cdf4(22663, 29948, 31320, 1, 0, 1),
    avm_cdf4(19776, 28658, 30435, 1, 0, 1),
    avm_cdf4(15436, 25313, 28181, 1, 0, 1),
    avm_cdf4(11214, 20671, 24854, 1, 0, 1),
    avm_cdf4(8548, 16982, 21766, 1, 0, 1),
    avm_cdf4(5729, 11993, 17176, 1, 0, 1),
];
const DEFAULT_COEFF_BR_LF_Y_CDFS: [[u16; 8]; 14] = [
    avm_cdf4(7943, 14193, 20775, -1, -1, -2),
    avm_cdf4(14297, 22400, 26238, 1, 1, -1),
    avm_cdf4(10557, 18683, 22550, 1, 1, 0),
    avm_cdf4(8289, 16068, 18454, 1, 1, -2),
    avm_cdf4(5258, 10730, 13709, 1, 1, 0),
    avm_cdf4(3933, 8166, 10680, 1, 1, 0),
    avm_cdf4(2465, 5325, 6625, 1, 1, 0),
    avm_cdf4(10865, 16430, 19691, 0, -1, -1),
    avm_cdf4(14571, 22733, 26106, 0, 1, 0),
    avm_cdf4(14072, 23021, 25971, 1, 0, 0),
    avm_cdf4(11558, 20253, 23235, 1, 0, 1),
    avm_cdf4(8603, 16200, 19466, 1, 1, 1),
    avm_cdf4(6641, 13086, 16612, 1, 0, 1),
    avm_cdf4(4240, 9043, 11946, 1, 1, 1),
];
const DEFAULT_COEFF_LPS_LF_CTX0_CDF: [u16; 8] = avm_cdf4(7943, 14193, 20775, -1, -1, -2);
const DEFAULT_DC_SIGN_Y_CTX0_CDF: [u16; 6] = avm_cdf2(15831, 1, 1, 1);
const DEFAULT_DC_SIGN_Y_CTX1_CDF: [u16; 6] = avm_cdf2(13632, 1, 0, 0);
const DEFAULT_DC_SIGN_Y_CTX2_CDF: [u16; 6] = avm_cdf2(19041, 1, 0, 0);
const DEFAULT_PALETTE_Y_MODE_CDF: [u16; 6] = avm_cdf2(30045, -2, -2, -2);
const DEFAULT_PALETTE_Y_SIZE_CDF: [u16; 11] =
    avm_cdf7(8779, 15095, 20777, 24903, 27923, 30403, -1, -1, -2);
// The AV2 bitstream lets an encoder decline cache hits and delta-code those
// colors instead. In the software encoder, probing the full small neighbor
// cache is cheap and avoids repeatedly sending common screen-content colors.
const PALETTE_CACHE_PROBE_LIMIT: usize = 16;
const DEFAULT_IDENTITY_ROW_CDF_Y: [[u16; 7]; 4] = [
    avm_cdf3(22515, 25751, -1, 0, 0),
    avm_cdf3(4014, 5233, -1, -1, -1),
    avm_cdf3(3548, 4163, -1, -1, 1),
    avm_cdf3(12999, 32756, -2, -1, -1),
];
const DEFAULT_PALETTE_Y_COLOR_INDEX_CDFS: [[[u16; 12]; 5]; 7] = [
    [
        avm_cdf2_padded(28140, 1, 1, 0),
        avm_cdf2_padded(16384, 0, 0, 0),
        avm_cdf2_padded(8582, 0, -1, -1),
        avm_cdf2_padded(27413, -1, -1, -2),
        avm_cdf2_padded(30429, 1, 1, 1),
    ],
    [
        avm_cdf3_padded(25350, 29026, 1, 1, 0),
        avm_cdf3_padded(11363, 25273, 0, -1, -2),
        avm_cdf3_padded(6841, 28579, 0, 0, -1),
        avm_cdf3_padded(21350, 26012, 0, -1, -1),
        avm_cdf3_padded(30573, 31646, 1, 1, 1),
    ],
    [
        avm_cdf4_padded(23706, 26962, 29060, 0, 0, 0),
        avm_cdf4_padded(9976, 22516, 27382, 0, 0, -1),
        avm_cdf4_padded(6691, 25460, 29234, 0, -1, -1),
        avm_cdf4_padded(18909, 23925, 28403, -1, -1, -1),
        avm_cdf4_padded(30308, 31076, 31818, 1, 1, 1),
    ],
    [
        avm_cdf5_padded(24116, 26957, 28486, 29941, 0, 0, 0),
        avm_cdf5_padded(9568, 20472, 24294, 28942, 1, -1, -1),
        avm_cdf5_padded(5706, 25243, 28040, 30406, 1, 0, -1),
        avm_cdf5_padded(20105, 22982, 27024, 28911, -1, -1, -1),
        avm_cdf5_padded(30897, 31342, 31766, 32199, 1, 1, 1),
    ],
    [
        avm_cdf6_padded(20824, 24227, 25926, 27459, 29266, 1, 0, 0),
        avm_cdf6_padded(8141, 18989, 21599, 26182, 28576, 1, 0, 0),
        avm_cdf6_padded(5252, 24340, 26450, 28438, 30625, 1, 0, 0),
        avm_cdf6_padded(19519, 22695, 25587, 26972, 28423, 0, -1, -1),
        avm_cdf6_padded(30383, 30890, 31247, 31653, 32150, 1, 0, 1),
    ],
    [
        avm_cdf7_padded(21628, 24512, 25873, 27054, 28131, 29539, 1, -1, 0),
        avm_cdf7_padded(8028, 18264, 20613, 25424, 27112, 28906, 1, 1, 0),
        avm_cdf7_padded(6489, 22242, 24461, 26394, 28350, 30510, 1, 0, 0),
        avm_cdf7_padded(22048, 24429, 26990, 27944, 28417, 29574, 1, 0, -1),
        avm_cdf7_padded(30801, 31205, 31472, 31728, 32005, 32305, 1, 1, 1),
    ],
    [
        avm_cdf8_padded(22471, 25083, 25984, 26893, 27654, 28750, 29903, 1, 1, 1),
        avm_cdf8_padded(7542, 17057, 19151, 23550, 25459, 27066, 28804, 1, 1, 0),
        avm_cdf8_padded(7582, 20437, 22728, 24622, 26515, 28579, 30632, 1, 1, 0),
        avm_cdf8_padded(22102, 24144, 26916, 28151, 28846, 29212, 30153, 0, 0, 0),
        avm_cdf8_padded(30524, 30887, 31156, 31393, 31626, 31911, 32281, 1, 1, 1),
    ],
];

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
        tile_origin_x,
        tile_origin_y,
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
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Av2PartitionPolicy {
    Fixed8x8Leaves,
    LargestLosslessLeaves,
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
        av2_luma_mode_syntax_for_block(bottom_left_mode, above_right_mode)
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

#[cfg(test)]
pub(crate) fn av2_black_444_tile_entropy_payload(
    geometry: Av2VideoGeometry,
    profile: Av2Black444MvpProfile,
) -> Av2EntropyPayload {
    av2_black_444_tile_entropy_payload_for_region(Av2TileRegion::root(geometry), profile)
}

pub(crate) fn av2_black_444_tile_entropy_payload_for_region(
    region: Av2TileRegion,
    profile: Av2Black444MvpProfile,
) -> Av2EntropyPayload {
    av2_black_444_tile_entropy_payload_for_region_with_intrabc(region, profile, false)
}

pub(crate) fn av2_black_444_tile_entropy_payload_for_region_with_intrabc(
    region: Av2TileRegion,
    profile: Av2Black444MvpProfile,
    allow_intrabc: bool,
) -> Av2EntropyPayload {
    let plan = Av2Black444TilePlan::for_region(
        region,
        profile,
        Av2ChromaFormat::Yuv444,
        false,
        allow_intrabc,
        None,
        None,
    );
    let mut writer = Av2EntropyWriter::with_cdf_updates(!profile.disable_cdf_update);
    plan.write_entropy(&mut writer, None, None);
    writer.finish()
}

pub(crate) fn av2_black_tile_entropy_payload_for_region(
    region: Av2TileRegion,
    profile: Av2Black444MvpProfile,
    chroma_format: Av2ChromaFormat,
) -> Av2EntropyPayload {
    let plan =
        Av2Black444TilePlan::for_region(region, profile, chroma_format, false, false, None, None);
    let mut writer = Av2EntropyWriter::with_cdf_updates(!profile.disable_cdf_update);
    plan.write_entropy(&mut writer, None, None);
    writer.finish()
}

pub(crate) fn av2_luma_palette_444_tile_entropy_payload_for_region(
    region: Av2TileRegion,
    profile: Av2Black444MvpProfile,
    allow_intrabc: bool,
    palette: &Av2LumaPalette444,
    ibc: &Av2LocalIbc444,
) -> Av2EntropyPayload {
    let plan = Av2Black444TilePlan::for_region(
        region,
        profile,
        Av2ChromaFormat::Yuv444,
        true,
        allow_intrabc,
        Some(ibc),
        Some(palette),
    );
    let mut writer = Av2EntropyWriter::with_cdf_updates(!profile.disable_cdf_update);
    plan.write_entropy(&mut writer, Some(palette), Some(ibc));
    writer.finish()
}

pub(crate) fn av2_lossy_420_tile_entropy_payload_for_region(
    region: Av2TileRegion,
    profile: Av2Black444MvpProfile,
    geometry: Av2VideoGeometry,
    bit_depth: SampleBitDepth,
    source: &[u8],
    recon: &mut [u8],
) -> Av2EntropyPayload {
    let plan = Av2Black444TilePlan::for_region(
        region,
        profile,
        Av2ChromaFormat::Yuv420,
        false,
        false,
        None,
        None,
    );
    let mut writer = Av2EntropyWriter::with_cdf_updates(!profile.disable_cdf_update);
    let mut lossy = Av2Lossy420TileState::new(geometry, region, bit_depth, source, recon);
    plan.write_lossy_420_entropy(&mut writer, &mut lossy);
    writer.finish()
}

pub(crate) fn av2_lossless_subsampled_tile_entropy_payload_for_region(
    region: Av2TileRegion,
    profile: Av2Black444MvpProfile,
    geometry: Av2VideoGeometry,
    chroma_format: Av2ChromaFormat,
    bit_depth: SampleBitDepth,
    source: &[u8],
    recon: &mut [u8],
) -> Av2EntropyPayload {
    debug_assert!(matches!(
        chroma_format,
        Av2ChromaFormat::Yuv420 | Av2ChromaFormat::Yuv422
    ));
    let plan = Av2Black444TilePlan::for_region_with_partition_policy(
        region,
        profile,
        chroma_format,
        Av2PartitionPolicy::LargestLosslessLeaves,
        false,
        false,
        None,
        None,
    );
    let mut writer = Av2EntropyWriter::with_cdf_updates(!profile.disable_cdf_update);
    let mut lossless = Av2LosslessSubsampledTileState::new(
        geometry,
        region,
        chroma_format,
        bit_depth,
        source,
        recon,
    );
    plan.write_lossless_subsampled_entropy(&mut writer, &mut lossless);
    writer.finish()
}

impl Av2Black444TilePlan {
    fn for_region(
        region: Av2TileRegion,
        profile: Av2Black444MvpProfile,
        chroma_format: Av2ChromaFormat,
        luma_palette: bool,
        allow_intrabc: bool,
        ibc: Option<&Av2LocalIbc444>,
        palette: Option<&Av2LumaPalette444>,
    ) -> Self {
        Self::for_region_with_partition_policy(
            region,
            profile,
            chroma_format,
            Av2PartitionPolicy::Fixed8x8Leaves,
            luma_palette,
            allow_intrabc,
            ibc,
            palette,
        )
    }

    fn for_region_with_partition_policy(
        region: Av2TileRegion,
        profile: Av2Black444MvpProfile,
        chroma_format: Av2ChromaFormat,
        partition_policy: Av2PartitionPolicy,
        luma_palette: bool,
        allow_intrabc: bool,
        ibc: Option<&Av2LocalIbc444>,
        palette: Option<&Av2LumaPalette444>,
    ) -> Self {
        assert!(
            !profile.enable_sdp,
            "AV2 MVP tile plan expects a shared luma/chroma partition tree"
        );
        assert!(
            region.origin_x % MVP_SUPERBLOCK_SIZE == 0
                && region.origin_y % MVP_SUPERBLOCK_SIZE == 0,
            "AV2 MVP tiles are aligned to 64x64 superblock origins"
        );
        assert!(
            region.width % 8 == 0 && region.height % 8 == 0,
            "AV2 MVP tile plan expects visible dimensions in 8-pixel units"
        );
        let geometry = region.geometry();
        let visible_rows_mi = geometry.height / MI_SIZE;
        let visible_cols_mi = geometry.width / MI_SIZE;
        let max_ref_bv_count = usize::from(profile.def_max_bvp_drl_bits_minus_min) + 2;
        let mut plan = Self {
            decisions: Vec::new(),
            origin_x: region.origin_x,
            origin_y: region.origin_y,
            chroma_format,
            partition_policy,
            visible_rows_mi,
            visible_cols_mi,
            luma_palette,
            allow_intrabc,
            max_ref_bv_count,
        };
        let mut partition_context = Av2PartitionContext::new(visible_rows_mi, visible_cols_mi);
        for row_mi in (0..visible_rows_mi).step_by(PARTITION_CONTEXT_DIM) {
            for col_mi in (0..visible_cols_mi).step_by(PARTITION_CONTEXT_DIM) {
                plan.visit_block(
                    row_mi,
                    col_mi,
                    Av2MvpBlockSize::BLOCK_64X64,
                    visible_rows_mi,
                    visible_cols_mi,
                    &mut partition_context,
                    ibc,
                    palette,
                );
            }
        }
        plan
    }

    fn visit_block(
        &mut self,
        row_mi: usize,
        col_mi: usize,
        block_size: Av2MvpBlockSize,
        visible_rows_mi: usize,
        visible_cols_mi: usize,
        partition_context: &mut Av2PartitionContext,
        ibc: Option<&Av2LocalIbc444>,
        palette: Option<&Av2LumaPalette444>,
    ) {
        if row_mi >= visible_rows_mi || col_mi >= visible_cols_mi {
            return;
        }

        let partition = if self.luma_palette {
            choose_luma_palette_partition(
                row_mi,
                col_mi,
                block_size,
                visible_rows_mi,
                visible_cols_mi,
                self.origin_x,
                self.origin_y,
                palette,
            )
        } else {
            match self.partition_policy {
                Av2PartitionPolicy::Fixed8x8Leaves => {
                    choose_partition(row_mi, col_mi, block_size, visible_rows_mi, visible_cols_mi)
                }
                Av2PartitionPolicy::LargestLosslessLeaves => choose_largest_lossless_partition(
                    row_mi,
                    col_mi,
                    block_size,
                    visible_rows_mi,
                    visible_cols_mi,
                ),
            }
        };
        self.decisions.push(Av2TileDecision {
            kind: Av2TileDecisionKind::Partition(partition),
            row: row_mi,
            col: col_mi,
            block_size,
        });

        match partition {
            Av2MvpPartition::None => {
                self.visit_leaf(row_mi, col_mi, block_size, ibc, palette);
                partition_context.update_leaf(row_mi, col_mi, block_size);
            }
            Av2MvpPartition::Horz => {
                let subsize = block_size
                    .subsize(partition)
                    .expect("AV2 MVP horizontal partition must have a subsize");
                self.visit_block(
                    row_mi,
                    col_mi,
                    subsize,
                    visible_rows_mi,
                    visible_cols_mi,
                    partition_context,
                    ibc,
                    palette,
                );
                self.visit_block(
                    row_mi + block_size.mi_height() / 2,
                    col_mi,
                    subsize,
                    visible_rows_mi,
                    visible_cols_mi,
                    partition_context,
                    ibc,
                    palette,
                );
            }
            Av2MvpPartition::Vert => {
                let subsize = block_size
                    .subsize(partition)
                    .expect("AV2 MVP vertical partition must have a subsize");
                self.visit_block(
                    row_mi,
                    col_mi,
                    subsize,
                    visible_rows_mi,
                    visible_cols_mi,
                    partition_context,
                    ibc,
                    palette,
                );
                self.visit_block(
                    row_mi,
                    col_mi + block_size.mi_width() / 2,
                    subsize,
                    visible_rows_mi,
                    visible_cols_mi,
                    partition_context,
                    ibc,
                    palette,
                );
            }
        }
    }

    fn visit_leaf(
        &mut self,
        row_mi: usize,
        col_mi: usize,
        block_size: Av2MvpBlockSize,
        ibc: Option<&Av2LocalIbc444>,
        palette: Option<&Av2LumaPalette444>,
    ) {
        assert!(
            block_size.width >= MVP_LEAF_BLOCK_SIZE && block_size.height >= MVP_LEAF_BLOCK_SIZE,
            "AV2 MVP coding leaves must be at least 8x8 blocks"
        );
        let x0 = self.origin_x + col_mi * MI_SIZE;
        let y0 = self.origin_y + row_mi * MI_SIZE;
        let ibc_copy = ibc.and_then(|ibc| ibc.candidate_copy(x0, y0));
        let ibc_drl_idx = ibc_copy.map(|copy| copy.drl_idx());
        let luma_mode = palette
            .map(|palette| palette.luma_mode_for_block(x0, y0))
            .unwrap_or(Av2LumaIntraMode::Dc);
        let luma_bdpcm_horz = palette.and_then(|palette| palette.luma_bdpcm_horz_for_block(x0, y0));
        let chroma_intra_mode = palette
            .map(|palette| palette.chroma_intra_mode_for_block(x0, y0))
            .unwrap_or(Av2ChromaIntraMode::Horizontal);
        let chroma_use_bdpcm = palette
            .map(|palette| palette.chroma_use_bdpcm_for_block(x0, y0))
            .unwrap_or(false);
        let prediction = decide_leaf_prediction(
            self.allow_intrabc,
            ibc_drl_idx,
            self.luma_palette,
            luma_mode,
            luma_bdpcm_horz,
            chroma_use_bdpcm,
            chroma_intra_mode,
        );
        if self.allow_intrabc {
            self.decisions.push(Av2TileDecision {
                kind: Av2TileDecisionKind::IntrabcFlag(prediction.intrabc_flag),
                row: row_mi,
                col: col_mi,
                block_size,
            });
        }
        match prediction.prediction {
            Av2LeafPredictionMode::IntrabcCopy { drl_idx } => {
                self.decisions.push(Av2TileDecision {
                    kind: Av2TileDecisionKind::IntrabcCopy {
                        drl_idx,
                        explicit_dv: ibc_copy.and_then(|copy| copy.explicit_dv()),
                    },
                    row: row_mi,
                    col: col_mi,
                    block_size,
                });
            }
            Av2LeafPredictionMode::Intra {
                luma_mode,
                use_luma_palette,
                use_dpcm_y,
                luma_bdpcm_horz,
                use_bdpcm_uv,
                chroma_intra_mode,
            } => {
                let use_fsc = use_luma_palette
                    && block_size.width == AV2_LUMA_PALETTE_BLOCK_SIZE
                    && block_size.height == AV2_LUMA_PALETTE_BLOCK_SIZE
                    && !use_dpcm_y
                    && palette.is_some_and(|palette| {
                        luma_palette_fsc_is_rate_worthy(
                            palette,
                            x0,
                            y0,
                            self.origin_x,
                            self.origin_y,
                            chroma_use_bdpcm,
                            chroma_intra_mode,
                        )
                    });
                self.decisions.push(Av2TileDecision {
                    kind: Av2TileDecisionKind::IntraLumaMode {
                        mode: luma_mode,
                        use_dpcm_y,
                        dpcm_horz: luma_bdpcm_horz,
                        use_fsc,
                    },
                    row: row_mi,
                    col: col_mi,
                    block_size,
                });
                let coded_luma_mode = if use_dpcm_y {
                    if luma_bdpcm_horz {
                        Av2LumaIntraMode::Horizontal
                    } else {
                        Av2LumaIntraMode::Vertical
                    }
                } else {
                    luma_mode
                };
                self.decisions.push(Av2TileDecision {
                    kind: Av2TileDecisionKind::IntraChromaMode {
                        use_bdpcm_uv,
                        luma_mode: coded_luma_mode,
                        chroma_intra_mode,
                    },
                    row: row_mi,
                    col: col_mi,
                    block_size,
                });
                if use_luma_palette {
                    self.decisions.push(Av2TileDecision {
                        kind: Av2TileDecisionKind::LumaPaletteModeInfo,
                        row: row_mi,
                        col: col_mi,
                        block_size,
                    });
                    self.decisions.push(Av2TileDecision {
                        kind: Av2TileDecisionKind::LumaPaletteColorMap,
                        row: row_mi,
                        col: col_mi,
                        block_size,
                    });
                }
                match prediction.residual {
                    Av2LeafResidualMode::BlackDc => {
                        self.decisions.push(Av2TileDecision {
                            kind: Av2TileDecisionKind::BlackDcResidualCoefficients,
                            row: row_mi,
                            col: col_mi,
                            block_size,
                        });
                    }
                    Av2LeafResidualMode::LumaPalette {
                        luma_bdpcm_horz,
                        chroma_use_bdpcm,
                        chroma_intra_mode,
                    } => {
                        self.decisions.push(Av2TileDecision {
                            kind: Av2TileDecisionKind::LumaPaletteResidualCoefficients {
                                luma_bdpcm_horz,
                                chroma_use_bdpcm,
                                chroma_intra_mode,
                                use_fsc,
                            },
                            row: row_mi,
                            col: col_mi,
                            block_size,
                        });
                    }
                    Av2LeafResidualMode::None => {}
                }
            }
        }
    }

    fn write_entropy(
        &self,
        writer: &mut Av2EntropyWriter,
        palette: Option<&Av2LumaPalette444>,
        _ibc: Option<&Av2LocalIbc444>,
    ) {
        let mut partition_context =
            Av2PartitionContext::new(self.visible_rows_mi, self.visible_cols_mi);
        let mut txb_contexts =
            Av2TxbEntropyContexts::new(self.visible_rows_mi, self.visible_cols_mi);
        let mut intrabc_context =
            Av2IntrabcContext::new(self.visible_rows_mi, self.visible_cols_mi);
        let mut coded_mi_context =
            Av2CodedMiContext::new(self.visible_rows_mi, self.visible_cols_mi);
        let mut palette_cache_context =
            Av2PaletteColorCacheContext::new(self.visible_rows_mi, self.visible_cols_mi);
        let mut luma_mode_context =
            Av2LumaModeContext::new(self.visible_rows_mi, self.visible_cols_mi);
        let mut fsc_mode_context =
            Av2FscModeContext::new(self.visible_rows_mi, self.visible_cols_mi);
        for decision in &self.decisions {
            match decision.kind {
                Av2TileDecisionKind::Partition(partition) => {
                    write_partition(
                        writer,
                        *decision,
                        partition,
                        &partition_context,
                        self.visible_rows_mi,
                        self.visible_cols_mi,
                    );
                    if partition == Av2MvpPartition::None {
                        partition_context.update_leaf(
                            decision.row,
                            decision.col,
                            decision.block_size,
                        );
                    }
                }
                Av2TileDecisionKind::IntrabcFlag(use_intrabc) => {
                    write_intrabc_flag(writer, *decision, &intrabc_context, use_intrabc);
                }
                Av2TileDecisionKind::IntrabcCopy {
                    drl_idx,
                    explicit_dv,
                } => {
                    write_intrabc_copy(
                        writer,
                        *decision,
                        &intrabc_context,
                        self.profile_max_ref_bv_count(),
                        drl_idx,
                        explicit_dv,
                    );
                    intrabc_context.update_leaf(
                        decision.row,
                        decision.col,
                        decision.block_size,
                        true,
                        true,
                    );
                    txb_contexts.clear_leaf(
                        decision.row,
                        decision.col,
                        decision.block_size,
                        self.visible_rows_mi,
                        self.visible_cols_mi,
                    );
                    palette_cache_context.clear_leaf(
                        decision.row,
                        decision.col,
                        decision.block_size,
                    );
                    // AVM av2_get_joint_mode() reports DC_PRED for inter and
                    // IntraBC neighbors. Keep the luma-mode context tied to
                    // actual coded leaves rather than palette pre-analysis so
                    // enabling more IBC copies cannot desynchronize later
                    // intra-mode symbols.
                    luma_mode_context.update_leaf(
                        decision.row,
                        decision.col,
                        decision.block_size,
                        Av2LumaIntraMode::Dc,
                    );
                    fsc_mode_context.update_leaf(
                        decision.row,
                        decision.col,
                        decision.block_size,
                        false,
                    );
                    coded_mi_context.update_leaf(decision.row, decision.col, decision.block_size);
                }
                Av2TileDecisionKind::IntraLumaMode {
                    mode,
                    use_dpcm_y,
                    dpcm_horz,
                    use_fsc,
                } => {
                    let mode_syntax = luma_mode_context.syntax_for_leaf(
                        decision.row,
                        decision.col,
                        decision.block_size,
                    );
                    let mode_context = mode_syntax.context;
                    let mode_index = mode_syntax.index_for(mode);
                    let fsc_context =
                        fsc_mode_context.context(decision.row, decision.col, decision.block_size);
                    write_intra_luma_mode(
                        writer,
                        *decision,
                        mode,
                        mode_context,
                        mode_index,
                        use_dpcm_y,
                        dpcm_horz,
                        use_fsc,
                        fsc_context,
                    );
                    if mode != Av2LumaIntraMode::Dc || use_dpcm_y {
                        palette_cache_context.clear_leaf(
                            decision.row,
                            decision.col,
                            decision.block_size,
                        );
                    }
                    let coded_mode = if use_dpcm_y {
                        if dpcm_horz {
                            Av2LumaIntraMode::Horizontal
                        } else {
                            Av2LumaIntraMode::Vertical
                        }
                    } else {
                        mode
                    };
                    luma_mode_context.update_leaf(
                        decision.row,
                        decision.col,
                        decision.block_size,
                        coded_mode,
                    );
                    fsc_mode_context.update_leaf(
                        decision.row,
                        decision.col,
                        decision.block_size,
                        use_fsc,
                    );
                }
                Av2TileDecisionKind::IntraChromaMode {
                    use_bdpcm_uv,
                    luma_mode,
                    chroma_intra_mode,
                } => {
                    write_intra_chroma_mode(
                        writer,
                        *decision,
                        use_bdpcm_uv,
                        luma_mode,
                        chroma_intra_mode,
                    );
                }
                Av2TileDecisionKind::LumaPaletteModeInfo => {
                    write_luma_palette_mode_info(
                        writer,
                        *decision,
                        palette.expect("luma palette decision needs palette state"),
                        &mut palette_cache_context,
                        self.origin_x,
                        self.origin_y,
                    );
                }
                Av2TileDecisionKind::LumaPaletteColorMap => {
                    write_luma_palette_color_map(
                        writer,
                        *decision,
                        palette.expect("luma palette decision needs palette state"),
                        self.origin_x,
                        self.origin_y,
                    );
                }
                Av2TileDecisionKind::BlackDcResidualCoefficients => {
                    write_black_dc_residual_coefficients(
                        writer,
                        *decision,
                        self.visible_rows_mi,
                        self.visible_cols_mi,
                        self.chroma_format,
                        &mut txb_contexts,
                    );
                    intrabc_context.update_leaf(
                        decision.row,
                        decision.col,
                        decision.block_size,
                        false,
                        false,
                    );
                    coded_mi_context.update_leaf(decision.row, decision.col, decision.block_size);
                }
                Av2TileDecisionKind::LumaPaletteResidualCoefficients {
                    luma_bdpcm_horz,
                    chroma_use_bdpcm,
                    chroma_intra_mode,
                    use_fsc,
                } => {
                    write_luma_palette_residual_coefficients(
                        writer,
                        *decision,
                        self.visible_rows_mi,
                        self.visible_cols_mi,
                        palette.expect("luma palette residual needs palette state"),
                        &mut txb_contexts,
                        &coded_mi_context,
                        self.origin_x,
                        self.origin_y,
                        luma_bdpcm_horz,
                        chroma_use_bdpcm,
                        chroma_intra_mode,
                        use_fsc,
                    );
                    intrabc_context.update_leaf(
                        decision.row,
                        decision.col,
                        decision.block_size,
                        false,
                        false,
                    );
                    coded_mi_context.update_leaf(decision.row, decision.col, decision.block_size);
                }
            }
        }
    }

    fn profile_max_ref_bv_count(&self) -> usize {
        self.max_ref_bv_count
    }

    fn write_lossy_420_entropy(
        &self,
        writer: &mut Av2EntropyWriter,
        lossy: &mut Av2Lossy420TileState<'_>,
    ) {
        let mut partition_context =
            Av2PartitionContext::new(self.visible_rows_mi, self.visible_cols_mi);
        let mut txb_contexts =
            Av2TxbEntropyContexts::new(self.visible_rows_mi, self.visible_cols_mi);
        let mut intrabc_context =
            Av2IntrabcContext::new(self.visible_rows_mi, self.visible_cols_mi);
        for decision in &self.decisions {
            match decision.kind {
                Av2TileDecisionKind::Partition(partition) => {
                    write_partition(
                        writer,
                        *decision,
                        partition,
                        &partition_context,
                        self.visible_rows_mi,
                        self.visible_cols_mi,
                    );
                    if partition == Av2MvpPartition::None {
                        partition_context.update_leaf(
                            decision.row,
                            decision.col,
                            decision.block_size,
                        );
                    }
                }
                Av2TileDecisionKind::IntraLumaMode {
                    mode,
                    use_dpcm_y: _,
                    dpcm_horz: _,
                    use_fsc: _,
                } => {
                    write_intra_luma_mode(
                        writer,
                        *decision,
                        mode,
                        0,
                        mode.mode_index() as u8,
                        false,
                        false,
                        false,
                        0,
                    );
                }
                Av2TileDecisionKind::IntraChromaMode {
                    use_bdpcm_uv: _,
                    luma_mode,
                    chroma_intra_mode: _,
                } => {
                    write_intra_chroma_mode(
                        writer,
                        *decision,
                        false,
                        luma_mode,
                        Av2ChromaIntraMode::Horizontal,
                    );
                }
                Av2TileDecisionKind::BlackDcResidualCoefficients => {
                    write_lossy_420_residual_coefficients(
                        writer,
                        *decision,
                        self.visible_rows_mi,
                        self.visible_cols_mi,
                        &mut txb_contexts,
                        lossy,
                    );
                    intrabc_context.update_leaf(
                        decision.row,
                        decision.col,
                        decision.block_size,
                        false,
                        false,
                    );
                }
                Av2TileDecisionKind::IntrabcFlag(_)
                | Av2TileDecisionKind::IntrabcCopy { .. }
                | Av2TileDecisionKind::LumaPaletteModeInfo
                | Av2TileDecisionKind::LumaPaletteColorMap
                | Av2TileDecisionKind::LumaPaletteResidualCoefficients { .. } => {
                    unreachable!("AV2 4:2:0 residual path disables palette and IntraBC")
                }
            }
        }
    }

    fn write_lossless_subsampled_entropy(
        &self,
        writer: &mut Av2EntropyWriter,
        lossless: &mut Av2LosslessSubsampledTileState<'_>,
    ) {
        let mut partition_context =
            Av2PartitionContext::new(self.visible_rows_mi, self.visible_cols_mi);
        let mut txb_contexts =
            Av2TxbEntropyContexts::new(self.visible_rows_mi, self.visible_cols_mi);
        let mut intrabc_context =
            Av2IntrabcContext::new(self.visible_rows_mi, self.visible_cols_mi);
        let mut fsc_mode_context =
            Av2FscModeContext::new(self.visible_rows_mi, self.visible_cols_mi);
        for decision in &self.decisions {
            match decision.kind {
                Av2TileDecisionKind::Partition(partition) => {
                    write_partition(
                        writer,
                        *decision,
                        partition,
                        &partition_context,
                        self.visible_rows_mi,
                        self.visible_cols_mi,
                    );
                    if partition == Av2MvpPartition::None {
                        partition_context.update_leaf(
                            decision.row,
                            decision.col,
                            decision.block_size,
                        );
                    }
                }
                Av2TileDecisionKind::IntraLumaMode {
                    mode: _,
                    use_dpcm_y: _,
                    dpcm_horz: _,
                    use_fsc: _,
                } => {
                    let mode = lossless.mode_decision_for_leaf(
                        *decision,
                        self.visible_rows_mi,
                        self.visible_cols_mi,
                    );
                    let coded_luma_mode = mode.coded_luma_mode();
                    let fsc_context =
                        fsc_mode_context.context(decision.row, decision.col, decision.block_size);
                    write_intra_luma_mode(
                        writer,
                        *decision,
                        coded_luma_mode,
                        0,
                        coded_luma_mode.mode_index() as u8,
                        mode.luma_bdpcm_horz.is_some(),
                        mode.luma_bdpcm_horz.unwrap_or(false),
                        mode.use_fsc,
                        fsc_context,
                    );
                    fsc_mode_context.update_leaf(
                        decision.row,
                        decision.col,
                        decision.block_size,
                        mode.use_fsc,
                    );
                }
                Av2TileDecisionKind::IntraChromaMode {
                    use_bdpcm_uv: _,
                    luma_mode: _,
                    chroma_intra_mode: _,
                } => {
                    let mode = lossless.mode_decision_for_leaf(
                        *decision,
                        self.visible_rows_mi,
                        self.visible_cols_mi,
                    );
                    write_intra_chroma_mode(
                        writer,
                        *decision,
                        mode.chroma_use_bdpcm,
                        mode.coded_luma_mode(),
                        mode.chroma_intra_mode,
                    );
                }
                Av2TileDecisionKind::BlackDcResidualCoefficients => {
                    write_lossless_subsampled_residual_coefficients(
                        writer,
                        *decision,
                        self.visible_rows_mi,
                        self.visible_cols_mi,
                        &mut txb_contexts,
                        lossless,
                    );
                    intrabc_context.update_leaf(
                        decision.row,
                        decision.col,
                        decision.block_size,
                        false,
                        false,
                    );
                }
                Av2TileDecisionKind::IntrabcFlag(_)
                | Av2TileDecisionKind::IntrabcCopy { .. }
                | Av2TileDecisionKind::LumaPaletteModeInfo
                | Av2TileDecisionKind::LumaPaletteColorMap
                | Av2TileDecisionKind::LumaPaletteResidualCoefficients { .. } => {
                    unreachable!("AV2 subsampled lossless path disables palette and IntraBC")
                }
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Av2TxbEntropyContexts {
    y_above: Vec<u8>,
    y_left: Vec<u8>,
    u_above: Vec<u8>,
    u_left: Vec<u8>,
    v_above: Vec<u8>,
    v_left: Vec<u8>,
}

impl Av2TxbEntropyContexts {
    fn new(visible_rows_mi: usize, visible_cols_mi: usize) -> Self {
        Self {
            y_above: vec![0; visible_cols_mi],
            y_left: vec![0; visible_rows_mi],
            u_above: vec![0; visible_cols_mi],
            u_left: vec![0; visible_rows_mi],
            v_above: vec![0; visible_cols_mi],
            v_left: vec![0; visible_rows_mi],
        }
    }

    fn clear_leaf(
        &mut self,
        row_mi: usize,
        col_mi: usize,
        block_size: Av2MvpBlockSize,
        visible_rows_mi: usize,
        visible_cols_mi: usize,
    ) {
        let txb_width = block_size
            .tx4x4_width()
            .min(visible_cols_mi.saturating_sub(col_mi));
        let txb_height = block_size
            .tx4x4_height()
            .min(visible_rows_mi.saturating_sub(row_mi));
        for col in col_mi..(col_mi + txb_width).min(self.y_above.len()) {
            self.y_above[col] = 0;
            self.u_above[col] = 0;
            self.v_above[col] = 0;
        }
        for row in row_mi..(row_mi + txb_height).min(self.y_left.len()) {
            self.y_left[row] = 0;
            self.u_left[row] = 0;
            self.v_left[row] = 0;
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Av2IntrabcContext {
    coded: Vec<bool>,
    ibc: Vec<bool>,
    skip: Vec<bool>,
    rows: usize,
    cols: usize,
}

impl Av2IntrabcContext {
    fn new(visible_rows_mi: usize, visible_cols_mi: usize) -> Self {
        Self {
            coded: vec![false; visible_rows_mi * visible_cols_mi],
            ibc: vec![false; visible_rows_mi * visible_cols_mi],
            skip: vec![false; visible_rows_mi * visible_cols_mi],
            rows: visible_rows_mi,
            cols: visible_cols_mi,
        }
    }

    fn intrabc_ctx(&self, row_mi: usize, col_mi: usize, block_size: Av2MvpBlockSize) -> usize {
        // AV2 v1.0.0 read_intra_frame_mode_info()/get_intrabc_ctx(): the
        // context is derived from the first two available spatial neighbors
        // in AVM's bottom-left, above-right, left, above scan. At a 64x64 SB
        // top boundary AVM suppresses above/above-right for this context.
        self.neighbor_sum(row_mi, col_mi, block_size, true, |state| state.ibc)
    }

    fn skip_txfm_ctx(&self, row_mi: usize, col_mi: usize, block_size: Av2MvpBlockSize) -> usize {
        // AV2 v1.0.0 read_skip_txfm()/get_txb_ctx() uses neighboring
        // skip_txfm state from the same two-neighbor scan, but the line-buffer
        // variant keeps above/above-right available at SB top boundaries.
        self.neighbor_sum(row_mi, col_mi, block_size, false, |state| state.skip)
    }

    fn update_leaf(
        &mut self,
        row_mi: usize,
        col_mi: usize,
        block_size: Av2MvpBlockSize,
        use_intrabc: bool,
        skip_txfm: bool,
    ) {
        for row in row_mi..(row_mi + block_size.mi_height()).min(self.rows) {
            for col in col_mi..(col_mi + block_size.mi_width()).min(self.cols) {
                let index = row * self.cols + col;
                self.coded[index] = true;
                self.ibc[index] = use_intrabc;
                self.skip[index] = skip_txfm;
            }
        }
    }

    fn neighbor_sum(
        &self,
        row_mi: usize,
        col_mi: usize,
        block_size: Av2MvpBlockSize,
        suppress_above_at_sb_top: bool,
        value: impl Fn(Av2IntrabcNeighborState) -> bool,
    ) -> usize {
        let not_at_sb_top_boundary = row_mi % PARTITION_CONTEXT_DIM != 0;
        let include_above = !suppress_above_at_sb_top || not_at_sb_top_boundary;
        let mut count = 0usize;
        let mut sum = 0usize;

        let mut push = |state: Option<Av2IntrabcNeighborState>| {
            if count >= 2 {
                return;
            }
            if let Some(state) = state {
                sum += usize::from(value(state));
                count += 1;
            }
        };

        push(self.bottom_left_state(row_mi, col_mi, block_size));
        if include_above {
            push(self.above_right_state(row_mi, col_mi, block_size));
        }
        push(self.left_state(row_mi, col_mi));
        if include_above {
            push(self.above_state(row_mi, col_mi));
        }
        sum
    }

    fn state_at(&self, row_mi: usize, col_mi: usize) -> Option<Av2IntrabcNeighborState> {
        if row_mi >= self.rows || col_mi >= self.cols {
            return None;
        }
        let index = row_mi * self.cols + col_mi;
        self.coded[index].then_some(Av2IntrabcNeighborState {
            ibc: self.ibc[index],
            skip: self.skip[index],
        })
    }

    fn bottom_left_state(
        &self,
        row_mi: usize,
        col_mi: usize,
        block_size: Av2MvpBlockSize,
    ) -> Option<Av2IntrabcNeighborState> {
        col_mi
            .checked_sub(1)
            .and_then(|col| self.state_at(row_mi + block_size.mi_height().saturating_sub(1), col))
    }

    fn above_right_state(
        &self,
        row_mi: usize,
        col_mi: usize,
        block_size: Av2MvpBlockSize,
    ) -> Option<Av2IntrabcNeighborState> {
        row_mi
            .checked_sub(1)
            .and_then(|row| self.state_at(row, col_mi + block_size.mi_width().saturating_sub(1)))
    }

    fn left_state(&self, row_mi: usize, col_mi: usize) -> Option<Av2IntrabcNeighborState> {
        col_mi
            .checked_sub(1)
            .and_then(|col| self.state_at(row_mi, col))
    }

    fn above_state(&self, row_mi: usize, col_mi: usize) -> Option<Av2IntrabcNeighborState> {
        row_mi
            .checked_sub(1)
            .and_then(|row| self.state_at(row, col_mi))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Av2IntrabcNeighborState {
    ibc: bool,
    skip: bool,
}

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

fn choose_luma_palette_partition(
    row_mi: usize,
    col_mi: usize,
    block_size: Av2MvpBlockSize,
    visible_rows_mi: usize,
    visible_cols_mi: usize,
    tile_origin_x: usize,
    tile_origin_y: usize,
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
        if AV2_ENABLE_LUMA_PALETTE_REGION_MERGE
            && allowed.none
            && palette.is_some_and(|palette| {
                luma_palette_region_mergeable(
                    palette,
                    tile_origin_x + col_mi * MI_SIZE,
                    tile_origin_y + row_mi * MI_SIZE,
                    block_size,
                )
            })
        {
            return Av2MvpPartition::None;
        }
    }
    choose_8x8_leaf_partition(row_mi, col_mi, block_size, visible_rows_mi, visible_cols_mi)
}

fn luma_palette_region_mergeable(
    palette: &Av2LumaPalette444,
    x0: usize,
    y0: usize,
    block_size: Av2MvpBlockSize,
) -> bool {
    if block_size.width < MVP_LEAF_BLOCK_SIZE || block_size.height < MVP_LEAF_BLOCK_SIZE {
        return false;
    }
    if luma_palette_region_has_adjacent_copy(palette, x0, y0, block_size) {
        return false;
    }

    let base_colors = palette.colors_for_block(x0, y0);
    let base_chroma_use_bdpcm = palette.chroma_use_bdpcm_for_block(x0, y0);
    let base_chroma_mode = palette.chroma_intra_mode_for_block(x0, y0);
    for local_y in (0..block_size.height).step_by(MVP_LEAF_BLOCK_SIZE) {
        for local_x in (0..block_size.width).step_by(MVP_LEAF_BLOCK_SIZE) {
            let child_x = x0 + local_x;
            let child_y = y0 + local_y;
            if palette.luma_mode_for_block(child_x, child_y) != Av2LumaIntraMode::Dc
                || palette
                    .luma_bdpcm_horz_for_block(child_x, child_y)
                    .is_some()
                || palette.colors_for_block(child_x, child_y) != base_colors
                || palette.chroma_use_bdpcm_for_block(child_x, child_y) != base_chroma_use_bdpcm
                || palette.chroma_intra_mode_for_block(child_x, child_y) != base_chroma_mode
            {
                return false;
            }
        }
    }
    true
}

fn luma_palette_region_has_adjacent_copy(
    palette: &Av2LumaPalette444,
    x0: usize,
    y0: usize,
    block_size: Av2MvpBlockSize,
) -> bool {
    if block_size.width == MVP_LEAF_BLOCK_SIZE && block_size.height == MVP_LEAF_BLOCK_SIZE {
        return false;
    }

    for local_y in (0..block_size.height).step_by(MVP_LEAF_BLOCK_SIZE) {
        for local_x in (0..block_size.width).step_by(MVP_LEAF_BLOCK_SIZE) {
            let child_x = x0 + local_x;
            let child_y = y0 + local_y;
            let left_in_same_tile = child_x % MVP_SUPERBLOCK_SIZE != 0;
            if left_in_same_tile
                && luma_palette_8x8_blocks_match(
                    palette,
                    child_x,
                    child_y,
                    child_x - MVP_LEAF_BLOCK_SIZE,
                    child_y,
                )
            {
                return true;
            }
            let above_in_same_tile = child_y % MVP_SUPERBLOCK_SIZE != 0;
            if above_in_same_tile
                && luma_palette_8x8_blocks_match(
                    palette,
                    child_x,
                    child_y,
                    child_x,
                    child_y - MVP_LEAF_BLOCK_SIZE,
                )
            {
                return true;
            }
        }
    }
    false
}

fn luma_palette_8x8_blocks_match(
    palette: &Av2LumaPalette444,
    x0: usize,
    y0: usize,
    ref_x0: usize,
    ref_y0: usize,
) -> bool {
    for local_y in 0..MVP_LEAF_BLOCK_SIZE {
        for local_x in 0..MVP_LEAF_BLOCK_SIZE {
            let x = x0 + local_x;
            let y = y0 + local_y;
            let ref_x = ref_x0 + local_x;
            let ref_y = ref_y0 + local_y;
            if palette.y_sample(x, y) != palette.y_sample(ref_x, ref_y)
                || palette.u_sample(x, y) != palette.u_sample(ref_x, ref_y)
                || palette.v_sample(x, y) != palette.v_sample(ref_x, ref_y)
            {
                return false;
            }
        }
    }
    true
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
    writer.write_symbol(
        "tile.intrabc.use_intrabc",
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
    writer.write_symbol("tile.intrabc.skip_txfm", 1, &mut skip_cdf, 2, false);

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

    let mut mode_set_cdf = DEFAULT_Y_MODE_SET_CDF;
    // AV2 v1.0.0 write_intra_luma_mode()/read_intra_luma_mode() calls
    // get_y_mode_idx_ctx()/get_y_intra_mode_set() before mapping y_mode_idx
    // to a predictor. The palette analyzer stores the 8x8 mode-set index and
    // directional-neighbor context so H/V leaves can seed later contexts
    // without desynchronizing the reference decoder.
    writer.write_symbol(
        "tile.intra.y_mode_set_index",
        0,
        &mut mode_set_cdf,
        4,
        false,
    );

    let mode_context = mode_context.min(2);
    let mut mode_idx_cdf = DEFAULT_Y_MODE_IDX_CDFS[usize::from(mode_context)];
    writer.write_symbol_with_cdf_key(
        mode.symbol_name(),
        "tile.intra.y_mode_idx",
        usize::from(mode_context),
        usize::from(mode_index),
        &mut mode_idx_cdf,
        8,
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
    matches!(
        mode,
        Av2LumaIntraMode::Vertical | Av2LumaIntraMode::Horizontal
    )
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
    let luma_directional = match luma_mode {
        Av2LumaIntraMode::Vertical => Some(1usize),
        Av2LumaIntraMode::Horizontal => Some(2usize),
        Av2LumaIntraMode::Dc => None,
    };
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
    let colors = palette.colors_for_block(x0, y0);
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
        writer.write_literal("tile.palette.y_color_cache", u32::from(found), 1);
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
    let colors = palette.color_count_for_block(x0, y0);
    let vertical_scan = choose_luma_palette_map_vertical_for_region(
        palette,
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
        writer.write_literal("tile.palette.y_direction", u32::from(vertical_scan), 1);
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
        let identity_row_flag =
            palette_identity_row_flag(palette, x0, y0, vertical_scan, outer, inner_limit);
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
                    u32::from(palette.index_at(x0 + col, y0 + row)),
                );
            } else if identity_row_flag != 2 && (identity_row_flag != 1 || inner == 0) {
                let (color_ctx, color_token) = palette_color_index_context(
                    palette,
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
    x0: usize,
    y0: usize,
    width: usize,
    height: usize,
) -> bool {
    if width >= 64 || height >= 64 {
        return false;
    }

    let horizontal_rate = luma_palette_color_map_rate_q8(palette, x0, y0, width, height, false);
    let vertical_rate = luma_palette_color_map_rate_q8(palette, x0, y0, width, height, true);
    vertical_rate <= horizontal_rate
}

fn luma_palette_color_map_rate_q8(
    palette: &Av2LumaPalette444,
    x0: usize,
    y0: usize,
    width: usize,
    height: usize,
    vertical_scan: bool,
) -> u32 {
    let colors = palette.color_count_for_block(x0, y0);
    let mut rate = 0u32;
    let mut prev_identity_row_flag = 0usize;
    let outer_limit = if vertical_scan { width } else { height };
    let inner_limit = if vertical_scan { height } else { width };

    for outer in 0..outer_limit {
        let identity_row_flag =
            palette_identity_row_flag(palette, x0, y0, vertical_scan, outer, inner_limit);
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
                    palette_color_index_context(palette, x0, y0, row, col, width);
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
            palette.index_at(x0 + col, y0 + row) == palette.index_at(x0 + prev_col, y0 + prev_row)
        })
    {
        return 2;
    }
    if (1..inner_limit).all(|inner| {
        let (row, col) = palette_map_coordinate(vertical_scan, outer, inner);
        let (prev_row, prev_col) = palette_map_coordinate(vertical_scan, outer, inner - 1);
        palette.index_at(x0 + col, y0 + row) == palette.index_at(x0 + prev_col, y0 + prev_row)
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
        let left = palette.index_at(x0 + col - 1, y0 + row);
        let top_left = palette.index_at(x0 + col - 1, y0 + row - 1);
        let top = palette.index_at(x0 + col, y0 + row - 1);
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
            palette.index_at(x0 + col, y0 + row - 1)
        } else {
            palette.index_at(x0 + col - 1, y0 + row)
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
    let color_count = palette.color_count_for_block(x0, y0);
    for read_idx in 0..color_count {
        if !color_status[read_idx] {
            color_order[write_idx] = read_idx as u8;
            write_idx += 1;
        }
    }
    let current_color = palette.index_at(x0 + col, y0 + row);
    let color_token = color_order
        .iter()
        .take(color_count)
        .position(|&color| color == current_color)
        .unwrap_or_else(|| {
            panic!(
                "palette color order missed color {} at ({}, {}) with stride {}",
                current_color, col, row, stride
            )
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

fn write_black_dc_residual_coefficients(
    writer: &mut Av2EntropyWriter,
    decision: Av2TileDecision,
    visible_rows_mi: usize,
    visible_cols_mi: usize,
    chroma_format: Av2ChromaFormat,
    contexts: &mut Av2TxbEntropyContexts,
) {
    // AV2 v1.0.0 Section 5.20.7.23 residual() sets lossless residuals to
    // TX_4X4 transform blocks.
    // DC_PRED reconstructs 128 at frame/tile boundaries, so a black input
    // needs one negative DC coefficient per TXB. With qindex 0, dequant is 64
    // and the lossless 4x4 inverse WHT divides a DC-only coefficient by four;
    // level 512 therefore produces -128 at every sample after dequant.
    // AV2 v1.0.0 decoding clips residual visits to the visible frame edge;
    // AVM does this through max_block_wide()/max_block_high() after setting
    // the nominal partition block. Match that by emitting only visible TXBs.
    let txb_width = decision
        .block_size
        .tx4x4_width()
        .min(visible_cols_mi.saturating_sub(decision.col));
    let txb_height = decision
        .block_size
        .tx4x4_height()
        .min(visible_rows_mi.saturating_sub(decision.row));
    for row in 0..txb_height {
        let abs_row = decision.row + row;
        for col in 0..txb_width {
            let abs_col = decision.col + col;
            let skip_ctx =
                luma_txb_skip_context(contexts.y_above[abs_col], contexts.y_left[abs_row]);
            let dc_sign_ctx = dc_sign_context(contexts.y_above[abs_col], contexts.y_left[abs_row]);
            write_y_black_dc_txb(writer, skip_ctx, dc_sign_ctx);
            contexts.y_above[abs_col] = NONZERO_NEGATIVE_DC_ENTROPY_CONTEXT;
            contexts.y_left[abs_row] = NONZERO_NEGATIVE_DC_ENTROPY_CONTEXT;
        }
    }

    let chroma_span = chroma_tx4x4_span(decision, visible_rows_mi, visible_cols_mi, chroma_format);

    for row in 0..chroma_span.height {
        let abs_row = chroma_span.row + row;
        for col in 0..chroma_span.width {
            let abs_col = chroma_span.col + col;
            let skip_ctx =
                chroma_txb_skip_base_context(contexts.u_above[abs_col], contexts.u_left[abs_row])
                    + 6;
            write_u_black_dc_txb(writer, skip_ctx);
            contexts.u_above[abs_col] = NONZERO_NEGATIVE_DC_ENTROPY_CONTEXT;
            contexts.u_left[abs_row] = NONZERO_NEGATIVE_DC_ENTROPY_CONTEXT;
        }
    }

    let last_u_txb_nonzero = chroma_span.width != 0 && chroma_span.height != 0;
    for row in 0..chroma_span.height {
        let abs_row = chroma_span.row + row;
        for col in 0..chroma_span.width {
            let abs_col = chroma_span.col + col;
            let skip_ctx = v_txb_skip_context_for_chroma_format(
                contexts.v_above[abs_col],
                contexts.v_left[abs_row],
                last_u_txb_nonzero,
                chroma_format,
                decision.block_size,
            );
            write_v_black_dc_txb(writer, skip_ctx);
            contexts.v_above[abs_col] = NONZERO_NEGATIVE_DC_ENTROPY_CONTEXT;
            contexts.v_left[abs_row] = NONZERO_NEGATIVE_DC_ENTROPY_CONTEXT;
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Av2ChromaTx4x4Span {
    row: usize,
    col: usize,
    width: usize,
    height: usize,
}

fn chroma_tx4x4_span(
    decision: Av2TileDecision,
    visible_rows_mi: usize,
    visible_cols_mi: usize,
    chroma_format: Av2ChromaFormat,
) -> Av2ChromaTx4x4Span {
    match chroma_format {
        Av2ChromaFormat::Yuv444 => Av2ChromaTx4x4Span {
            row: decision.row,
            col: decision.col,
            width: decision
                .block_size
                .tx4x4_width()
                .min(visible_cols_mi.saturating_sub(decision.col)),
            height: decision
                .block_size
                .tx4x4_height()
                .min(visible_rows_mi.saturating_sub(decision.row)),
        },
        Av2ChromaFormat::Yuv422 => {
            // 4:2:2 chroma uses half-resolution columns and full-resolution
            // rows, so an 8x8 luma leaf maps to two vertical 4x4 chroma TXBs.
            let row = decision.row;
            let col = decision.col / 2;
            let visible_rows = visible_rows_mi;
            let visible_cols = visible_cols_mi / 2;
            Av2ChromaTx4x4Span {
                row,
                col,
                width: (decision.block_size.tx4x4_width() / 2)
                    .min(visible_cols.saturating_sub(col)),
                height: decision
                    .block_size
                    .tx4x4_height()
                    .min(visible_rows.saturating_sub(row)),
            }
        }
        Av2ChromaFormat::Yuv420 => {
            // AV2 v1.0.0 residual() uses chroma transform units in chroma
            // sample coordinates. FrameForge's first 4:2:0 milestone keeps
            // 8x8 luma leaves, so each leaf maps to one 4x4 U TXB and one
            // 4x4 V TXB.
            let row = decision.row / 2;
            let col = decision.col / 2;
            let visible_rows = visible_rows_mi / 2;
            let visible_cols = visible_cols_mi / 2;
            Av2ChromaTx4x4Span {
                row,
                col,
                width: (decision.block_size.tx4x4_width() / 2)
                    .min(visible_cols.saturating_sub(col)),
                height: (decision.block_size.tx4x4_height() / 2)
                    .min(visible_rows.saturating_sub(row)),
            }
        }
    }
}

struct Av2Lossy420TileState<'a> {
    geometry: Av2VideoGeometry,
    region: Av2TileRegion,
    bit_depth: SampleBitDepth,
    source: &'a [u8],
    recon: &'a mut [u8],
    y_len: usize,
    c_width: usize,
    c_height: usize,
    c_len: usize,
}

impl<'a> Av2Lossy420TileState<'a> {
    fn new(
        geometry: Av2VideoGeometry,
        region: Av2TileRegion,
        bit_depth: SampleBitDepth,
        source: &'a [u8],
        recon: &'a mut [u8],
    ) -> Self {
        let y_len = geometry.width * geometry.height;
        let c_width = geometry.width / 2;
        let c_height = geometry.height / 2;
        let c_len = c_width * c_height;
        let expected_len = (y_len + 2 * c_len) * bit_depth.bytes_per_sample();
        assert_eq!(
            source.len(),
            expected_len,
            "AV2 4:2:0 residual source length must match geometry"
        );
        assert_eq!(
            recon.len(),
            source.len(),
            "AV2 4:2:0 residual reconstruction length must match source"
        );
        Self {
            geometry,
            region,
            bit_depth,
            source,
            recon,
            y_len,
            c_width,
            c_height,
            c_len,
        }
    }

    fn plane_geometry(&self, plane: Av2Lossy420Plane) -> (usize, usize) {
        match plane {
            Av2Lossy420Plane::Y => (self.geometry.width, self.geometry.height),
            Av2Lossy420Plane::U | Av2Lossy420Plane::V => (self.c_width, self.c_height),
        }
    }

    fn plane_origin(&self, plane: Av2Lossy420Plane) -> (usize, usize) {
        match plane {
            Av2Lossy420Plane::Y => (self.region.origin_x, self.region.origin_y),
            Av2Lossy420Plane::U | Av2Lossy420Plane::V => {
                (self.region.origin_x / 2, self.region.origin_y / 2)
            }
        }
    }

    fn txb_origin(&self, plane: Av2Lossy420Plane, col: usize, row: usize) -> (usize, usize) {
        let (origin_x, origin_y) = self.plane_origin(plane);
        (origin_x + col * TX4X4_SIZE, origin_y + row * TX4X4_SIZE)
    }

    fn offset(&self, plane: Av2Lossy420Plane, x: usize, y: usize) -> usize {
        match plane {
            Av2Lossy420Plane::Y => y * self.geometry.width + x,
            Av2Lossy420Plane::U => self.y_len + y * self.c_width + x,
            Av2Lossy420Plane::V => self.y_len + self.c_len + y * self.c_width + x,
        }
    }

    fn source_sample(&self, plane: Av2Lossy420Plane, x: usize, y: usize) -> Av2Sample {
        read_planar_sample(self.source, self.offset(plane, x, y), self.bit_depth)
            .expect("validated AV2 4:2:0 source must contain every sample")
    }

    fn recon_sample(&self, plane: Av2Lossy420Plane, x: usize, y: usize) -> Av2Sample {
        read_planar_sample(self.recon, self.offset(plane, x, y), self.bit_depth)
            .expect("validated AV2 4:2:0 reconstruction must contain every sample")
    }

    fn set_recon_sample(&mut self, plane: Av2Lossy420Plane, x: usize, y: usize, sample: Av2Sample) {
        let offset = self.offset(plane, x, y);
        write_planar_sample(self.recon, offset, sample, self.bit_depth)
            .expect("validated AV2 4:2:0 reconstruction must contain every sample");
    }

    fn luma_dc_predictor(&self, plane: Av2Lossy420Plane, x0: usize, y0: usize) -> Av2Sample {
        let (tile_origin_x, tile_origin_y) = self.plane_origin(plane);
        let have_left = x0 > tile_origin_x;
        let have_top = y0 > tile_origin_y;
        if !have_left && !have_top {
            return av2_lossless_dc_predictor(self.bit_depth);
        }

        let mut sum = 0u32;
        let mut count = 0u32;
        if have_top {
            for x in x0..(x0 + TX4X4_SIZE) {
                sum += u32::from(self.recon_sample(plane, x, y0 - 1));
                count += 1;
            }
        }
        if have_left {
            for y in y0..(y0 + TX4X4_SIZE) {
                sum += u32::from(self.recon_sample(plane, x0 - 1, y));
                count += 1;
            }
        }
        ((sum + count / 2) / count) as Av2Sample
    }

    fn chroma_h_predictor(&self, plane: Av2Lossy420Plane, x0: usize, y0: usize) -> Av2Sample {
        let (tile_origin_x, tile_origin_y) = self.plane_origin(plane);
        // read_intra_uv_mode() currently emits the normal horizontal chroma
        // predictor for 4:2:0 leaves. AVM's H_PRED falls back to above[0] when
        // the left edge is unavailable, then to base+1 at the tile corner.
        if x0 > tile_origin_x {
            self.recon_sample(plane, x0 - 1, y0)
        } else if y0 > tile_origin_y {
            self.recon_sample(plane, x0, y0 - 1)
        } else {
            av2_lossless_h_pred_left_edge(self.bit_depth)
        }
    }

    fn predictor(&self, plane: Av2Lossy420Plane, x0: usize, y0: usize) -> Av2Sample {
        match plane {
            Av2Lossy420Plane::Y => self.luma_dc_predictor(plane, x0, y0),
            Av2Lossy420Plane::U | Av2Lossy420Plane::V => self.chroma_h_predictor(plane, x0, y0),
        }
    }

    fn quantized_dc_delta(&self, plane: Av2Lossy420Plane, x0: usize, y0: usize) -> i16 {
        let predictor = i32::from(self.predictor(plane, x0, y0));
        let mut sum = 0i32;
        for local_y in 0..TX4X4_SIZE {
            for local_x in 0..TX4X4_SIZE {
                sum += i32::from(self.source_sample(plane, x0 + local_x, y0 + local_y)) - predictor;
            }
        }
        let average = round_div_i32(sum, TX4X4_SAMPLES as i32);
        let max_delta = i32::from(self.bit_depth.max_sample());
        quantize_i32_to_step(average, self.quant_step()).clamp(-max_delta, max_delta) as i16
    }

    fn fill_recon_txb(&mut self, plane: Av2Lossy420Plane, x0: usize, y0: usize, delta: i16) {
        let predictor = i32::from(self.predictor(plane, x0, y0));
        let sample = (predictor + i32::from(delta)).clamp(0, i32::from(self.bit_depth.max_sample()))
            as Av2Sample;
        let (plane_width, plane_height) = self.plane_geometry(plane);
        for local_y in 0..TX4X4_SIZE {
            let y = y0 + local_y;
            if y >= plane_height {
                continue;
            }
            for local_x in 0..TX4X4_SIZE {
                let x = x0 + local_x;
                if x < plane_width {
                    self.set_recon_sample(plane, x, y, sample);
                }
            }
        }
    }

    fn quant_step(&self) -> i32 {
        AV2_LOSSY_420_DC_QUANT_STEP << u32::from(self.bit_depth.bits() - 8)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Av2Lossy420Plane {
    Y,
    U,
    V,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Av2LosslessSubsampledModeDecision {
    luma_intra_mode: Av2LumaIntraMode,
    luma_bdpcm_horz: Option<bool>,
    chroma_use_bdpcm: bool,
    chroma_intra_mode: Av2ChromaIntraMode,
    use_fsc: bool,
}

impl Default for Av2LosslessSubsampledModeDecision {
    fn default() -> Self {
        Self {
            luma_intra_mode: Av2LumaIntraMode::Dc,
            luma_bdpcm_horz: None,
            chroma_use_bdpcm: false,
            chroma_intra_mode: Av2ChromaIntraMode::Horizontal,
            use_fsc: false,
        }
    }
}

impl Av2LosslessSubsampledModeDecision {
    fn coded_luma_mode(self) -> Av2LumaIntraMode {
        match self.luma_bdpcm_horz {
            Some(true) => Av2LumaIntraMode::Horizontal,
            Some(false) => Av2LumaIntraMode::Vertical,
            None => self.luma_intra_mode,
        }
    }
}

fn chroma_mode_for_luma_mode(mode: Av2LumaIntraMode) -> Av2ChromaIntraMode {
    match mode {
        Av2LumaIntraMode::Dc => Av2ChromaIntraMode::Dc,
        Av2LumaIntraMode::Vertical => Av2ChromaIntraMode::Vertical,
        Av2LumaIntraMode::Horizontal => Av2ChromaIntraMode::Horizontal,
    }
}

struct Av2LosslessSubsampledTileState<'a> {
    geometry: Av2VideoGeometry,
    region: Av2TileRegion,
    chroma_format: Av2ChromaFormat,
    bit_depth: SampleBitDepth,
    source: &'a [u8],
    recon: &'a mut [u8],
    y_len: usize,
    c_width: usize,
    c_height: usize,
    c_len: usize,
}

impl<'a> Av2LosslessSubsampledTileState<'a> {
    fn new(
        geometry: Av2VideoGeometry,
        region: Av2TileRegion,
        chroma_format: Av2ChromaFormat,
        bit_depth: SampleBitDepth,
        source: &'a [u8],
        recon: &'a mut [u8],
    ) -> Self {
        assert!(
            matches!(
                chroma_format,
                Av2ChromaFormat::Yuv420 | Av2ChromaFormat::Yuv422
            ),
            "AV2 subsampled lossless state expects 4:2:0 or 4:2:2 input"
        );
        let y_len = geometry.width * geometry.height;
        let c_width = geometry.width / chroma_subsample_x(chroma_format);
        let c_height = geometry.height / chroma_subsample_y(chroma_format);
        let c_len = c_width * c_height;
        let expected_len = (y_len + 2 * c_len) * bit_depth.bytes_per_sample();
        assert_eq!(
            source.len(),
            expected_len,
            "AV2 subsampled lossless source length must match geometry"
        );
        assert_eq!(
            recon.len(),
            source.len(),
            "AV2 subsampled lossless reconstruction length must match source"
        );
        Self {
            geometry,
            region,
            chroma_format,
            bit_depth,
            source,
            recon,
            y_len,
            c_width,
            c_height,
            c_len,
        }
    }

    fn plane_geometry(&self, plane: Av2LosslessPlane) -> (usize, usize) {
        match plane {
            Av2LosslessPlane::Y => (self.geometry.width, self.geometry.height),
            Av2LosslessPlane::U | Av2LosslessPlane::V => (self.c_width, self.c_height),
        }
    }

    fn plane_origin(&self, plane: Av2LosslessPlane) -> (usize, usize) {
        match plane {
            Av2LosslessPlane::Y => (self.region.origin_x, self.region.origin_y),
            Av2LosslessPlane::U | Av2LosslessPlane::V => (
                self.region.origin_x / chroma_subsample_x(self.chroma_format),
                self.region.origin_y / chroma_subsample_y(self.chroma_format),
            ),
        }
    }

    fn txb_origin(&self, plane: Av2LosslessPlane, col: usize, row: usize) -> (usize, usize) {
        let (origin_x, origin_y) = self.plane_origin(plane);
        (origin_x + col * TX4X4_SIZE, origin_y + row * TX4X4_SIZE)
    }

    fn offset(&self, plane: Av2LosslessPlane, x: usize, y: usize) -> usize {
        match plane {
            Av2LosslessPlane::Y => y * self.geometry.width + x,
            Av2LosslessPlane::U => self.y_len + y * self.c_width + x,
            Av2LosslessPlane::V => self.y_len + self.c_len + y * self.c_width + x,
        }
    }

    fn source_sample(&self, plane: Av2LosslessPlane, x: usize, y: usize) -> Av2Sample {
        read_planar_sample(self.source, self.offset(plane, x, y), self.bit_depth)
            .expect("validated AV2 subsampled lossless source must contain every sample")
    }

    fn recon_sample(&self, plane: Av2LosslessPlane, x: usize, y: usize) -> Av2Sample {
        read_planar_sample(self.recon, self.offset(plane, x, y), self.bit_depth)
            .expect("validated AV2 subsampled lossless reconstruction must contain every sample")
    }

    fn set_recon_sample(&mut self, plane: Av2LosslessPlane, x: usize, y: usize, sample: Av2Sample) {
        let offset = self.offset(plane, x, y);
        write_planar_sample(self.recon, offset, sample, self.bit_depth)
            .expect("validated AV2 subsampled lossless reconstruction must contain every sample");
    }

    fn dc_predictor(&self, plane: Av2LosslessPlane, x0: usize, y0: usize) -> Av2Sample {
        let (tile_origin_x, tile_origin_y) = self.plane_origin(plane);
        let have_left = x0 > tile_origin_x;
        let have_top = y0 > tile_origin_y;
        if !have_left && !have_top {
            return av2_lossless_dc_predictor(self.bit_depth);
        }

        let mut sum = 0u32;
        let mut count = 0u32;
        if have_top {
            for x in x0..(x0 + TX4X4_SIZE) {
                sum += u32::from(self.recon_sample(plane, x, y0 - 1));
                count += 1;
            }
        }
        if have_left {
            for y in y0..(y0 + TX4X4_SIZE) {
                sum += u32::from(self.recon_sample(plane, x0 - 1, y));
                count += 1;
            }
        }
        ((sum + count / 2) / count) as Av2Sample
    }

    fn h_predictor(
        &self,
        plane: Av2LosslessPlane,
        x0: usize,
        y0: usize,
        local_y: usize,
    ) -> Av2Sample {
        let (tile_origin_x, tile_origin_y) = self.plane_origin(plane);
        if x0 > tile_origin_x {
            self.recon_sample(plane, x0 - 1, y0 + local_y)
        } else if y0 > tile_origin_y {
            self.recon_sample(plane, x0, y0 - 1)
        } else {
            av2_lossless_h_pred_left_edge(self.bit_depth)
        }
    }

    fn v_predictor(
        &self,
        plane: Av2LosslessPlane,
        x0: usize,
        y0: usize,
        local_x: usize,
    ) -> Av2Sample {
        let (tile_origin_x, tile_origin_y) = self.plane_origin(plane);
        if y0 > tile_origin_y {
            self.recon_sample(plane, x0 + local_x, y0 - 1)
        } else if x0 > tile_origin_x {
            self.recon_sample(plane, x0 - 1, y0)
        } else {
            av2_lossless_v_pred_above_edge(self.bit_depth)
        }
    }

    #[cfg(test)]
    fn tx4x4_coefficients(
        &self,
        plane: Av2LosslessPlane,
        x0: usize,
        y0: usize,
    ) -> [i32; TX4X4_SAMPLES] {
        self.tx4x4_coefficients_for_mode(
            plane,
            x0,
            y0,
            Av2LosslessSubsampledModeDecision::default(),
        )
    }

    fn tx4x4_coefficients_for_mode(
        &self,
        plane: Av2LosslessPlane,
        x0: usize,
        y0: usize,
        mode: Av2LosslessSubsampledModeDecision,
    ) -> [i32; TX4X4_SAMPLES] {
        let residual = match plane {
            Av2LosslessPlane::Y => {
                if let Some(horz) = mode.luma_bdpcm_horz {
                    self.dpcm_residual4x4(plane, x0, y0, horz)
                } else {
                    self.intra_residual4x4(
                        plane,
                        x0,
                        y0,
                        chroma_mode_for_luma_mode(mode.luma_intra_mode),
                    )
                }
            }
            Av2LosslessPlane::U | Av2LosslessPlane::V => {
                if mode.chroma_use_bdpcm {
                    self.dpcm_residual4x4(plane, x0, y0, mode.chroma_intra_mode.is_horizontal())
                } else {
                    self.intra_residual4x4(plane, x0, y0, mode.chroma_intra_mode)
                }
            }
        };
        if mode.use_fsc {
            idtx4x4_coefficients(&residual)
        } else {
            av2_fwht4x4(&residual)
        }
    }

    fn tx4x4_coefficients_for_mode_score(
        &self,
        plane: Av2LosslessPlane,
        x0: usize,
        y0: usize,
        mode: Av2LosslessSubsampledModeDecision,
        leaf_x0: usize,
        leaf_y0: usize,
    ) -> [i32; TX4X4_SAMPLES] {
        let residual = match plane {
            Av2LosslessPlane::Y => {
                if let Some(horz) = mode.luma_bdpcm_horz {
                    self.dpcm_residual4x4_for_score(plane, x0, y0, horz, leaf_x0, leaf_y0)
                } else {
                    self.intra_residual4x4_for_score(
                        plane,
                        x0,
                        y0,
                        chroma_mode_for_luma_mode(mode.luma_intra_mode),
                        leaf_x0,
                        leaf_y0,
                    )
                }
            }
            Av2LosslessPlane::U | Av2LosslessPlane::V => {
                if mode.chroma_use_bdpcm {
                    self.dpcm_residual4x4_for_score(
                        plane,
                        x0,
                        y0,
                        mode.chroma_intra_mode.is_horizontal(),
                        leaf_x0,
                        leaf_y0,
                    )
                } else {
                    self.intra_residual4x4_for_score(
                        plane,
                        x0,
                        y0,
                        mode.chroma_intra_mode,
                        leaf_x0,
                        leaf_y0,
                    )
                }
            }
        };
        if mode.use_fsc {
            idtx4x4_coefficients(&residual)
        } else {
            av2_fwht4x4(&residual)
        }
    }

    fn intra_residual4x4(
        &self,
        plane: Av2LosslessPlane,
        x0: usize,
        y0: usize,
        mode: Av2ChromaIntraMode,
    ) -> [i32; TX4X4_SAMPLES] {
        let dc_predictor =
            (mode == Av2ChromaIntraMode::Dc).then(|| self.dc_predictor(plane, x0, y0));
        let mut residual = [0i32; TX4X4_SAMPLES];
        for local_y in 0..TX4X4_SIZE {
            for local_x in 0..TX4X4_SIZE {
                let predictor = match mode {
                    Av2ChromaIntraMode::Dc => dc_predictor.expect("DC predictor is precomputed"),
                    Av2ChromaIntraMode::Horizontal => self.h_predictor(plane, x0, y0, local_y),
                    Av2ChromaIntraMode::Vertical => self.v_predictor(plane, x0, y0, local_x),
                    _ => unreachable!("subsampled lossless scorer only uses DC/H/V predictors"),
                };
                residual[local_y * TX4X4_SIZE + local_x] =
                    i32::from(self.source_sample(plane, x0 + local_x, y0 + local_y))
                        - i32::from(predictor);
            }
        }
        residual
    }

    fn dpcm_residual4x4(
        &self,
        plane: Av2LosslessPlane,
        x0: usize,
        y0: usize,
        horz: bool,
    ) -> [i32; TX4X4_SAMPLES] {
        let mut residual = [0i32; TX4X4_SAMPLES];
        for local_y in 0..TX4X4_SIZE {
            for local_x in 0..TX4X4_SIZE {
                let x = x0 + local_x;
                let y = y0 + local_y;
                let sample = i32::from(self.source_sample(plane, x, y));
                let predicted_delta = if horz {
                    if local_x == 0 {
                        sample - i32::from(self.h_predictor(plane, x0, y0, local_y))
                    } else {
                        sample - i32::from(self.source_sample(plane, x - 1, y))
                    }
                } else if local_y == 0 {
                    sample - i32::from(self.v_predictor(plane, x0, y0, local_x))
                } else {
                    sample - i32::from(self.source_sample(plane, x, y - 1))
                };
                residual[local_y * TX4X4_SIZE + local_x] = predicted_delta;
            }
        }
        residual
    }

    fn intra_residual4x4_for_score(
        &self,
        plane: Av2LosslessPlane,
        x0: usize,
        y0: usize,
        mode: Av2ChromaIntraMode,
        leaf_x0: usize,
        leaf_y0: usize,
    ) -> [i32; TX4X4_SAMPLES] {
        let dc_predictor = (mode == Av2ChromaIntraMode::Dc)
            .then(|| self.dc_predictor_for_score(plane, x0, y0, leaf_x0, leaf_y0));
        let mut residual = [0i32; TX4X4_SAMPLES];
        for local_y in 0..TX4X4_SIZE {
            for local_x in 0..TX4X4_SIZE {
                let predictor = match mode {
                    Av2ChromaIntraMode::Dc => dc_predictor.expect("DC predictor is precomputed"),
                    Av2ChromaIntraMode::Horizontal => {
                        self.h_predictor_for_score(plane, x0, y0, local_y, leaf_x0, leaf_y0)
                    }
                    Av2ChromaIntraMode::Vertical => {
                        self.v_predictor_for_score(plane, x0, y0, local_x, leaf_x0, leaf_y0)
                    }
                    _ => unreachable!("subsampled lossless scorer only uses DC/H/V predictors"),
                };
                residual[local_y * TX4X4_SIZE + local_x] =
                    i32::from(self.source_sample(plane, x0 + local_x, y0 + local_y))
                        - i32::from(predictor);
            }
        }
        residual
    }

    fn dpcm_residual4x4_for_score(
        &self,
        plane: Av2LosslessPlane,
        x0: usize,
        y0: usize,
        horz: bool,
        leaf_x0: usize,
        leaf_y0: usize,
    ) -> [i32; TX4X4_SAMPLES] {
        let mut residual = [0i32; TX4X4_SAMPLES];
        for local_y in 0..TX4X4_SIZE {
            for local_x in 0..TX4X4_SIZE {
                let x = x0 + local_x;
                let y = y0 + local_y;
                let sample = i32::from(self.source_sample(plane, x, y));
                let predicted_delta = if horz {
                    if local_x == 0 {
                        sample
                            - i32::from(
                                self.h_predictor_for_score(
                                    plane, x0, y0, local_y, leaf_x0, leaf_y0,
                                ),
                            )
                    } else {
                        sample - i32::from(self.source_sample(plane, x - 1, y))
                    }
                } else if local_y == 0 {
                    sample
                        - i32::from(
                            self.v_predictor_for_score(plane, x0, y0, local_x, leaf_x0, leaf_y0),
                        )
                } else {
                    sample - i32::from(self.source_sample(plane, x, y - 1))
                };
                residual[local_y * TX4X4_SIZE + local_x] = predicted_delta;
            }
        }
        residual
    }

    fn dc_predictor_for_score(
        &self,
        plane: Av2LosslessPlane,
        x0: usize,
        y0: usize,
        leaf_x0: usize,
        leaf_y0: usize,
    ) -> Av2Sample {
        let (tile_origin_x, tile_origin_y) = self.plane_origin(plane);
        let have_left = x0 > tile_origin_x;
        let have_top = y0 > tile_origin_y;
        if !have_left && !have_top {
            return av2_lossless_dc_predictor(self.bit_depth);
        }

        let mut sum = 0u32;
        let mut count = 0u32;
        if have_top {
            for x in x0..(x0 + TX4X4_SIZE) {
                sum +=
                    u32::from(self.neighbor_sample_for_score(plane, x, y0 - 1, leaf_x0, leaf_y0));
                count += 1;
            }
        }
        if have_left {
            for y in y0..(y0 + TX4X4_SIZE) {
                sum +=
                    u32::from(self.neighbor_sample_for_score(plane, x0 - 1, y, leaf_x0, leaf_y0));
                count += 1;
            }
        }
        ((sum + count / 2) / count) as Av2Sample
    }

    fn h_predictor_for_score(
        &self,
        plane: Av2LosslessPlane,
        x0: usize,
        y0: usize,
        local_y: usize,
        leaf_x0: usize,
        leaf_y0: usize,
    ) -> Av2Sample {
        let (tile_origin_x, tile_origin_y) = self.plane_origin(plane);
        if x0 > tile_origin_x {
            self.neighbor_sample_for_score(plane, x0 - 1, y0 + local_y, leaf_x0, leaf_y0)
        } else if y0 > tile_origin_y {
            self.neighbor_sample_for_score(plane, x0, y0 - 1, leaf_x0, leaf_y0)
        } else {
            av2_lossless_h_pred_left_edge(self.bit_depth)
        }
    }

    fn v_predictor_for_score(
        &self,
        plane: Av2LosslessPlane,
        x0: usize,
        y0: usize,
        local_x: usize,
        leaf_x0: usize,
        leaf_y0: usize,
    ) -> Av2Sample {
        let (tile_origin_x, tile_origin_y) = self.plane_origin(plane);
        if y0 > tile_origin_y {
            self.neighbor_sample_for_score(plane, x0 + local_x, y0 - 1, leaf_x0, leaf_y0)
        } else if x0 > tile_origin_x {
            self.neighbor_sample_for_score(plane, x0 - 1, y0, leaf_x0, leaf_y0)
        } else {
            av2_lossless_v_pred_above_edge(self.bit_depth)
        }
    }

    fn neighbor_sample_for_score(
        &self,
        plane: Av2LosslessPlane,
        x: usize,
        y: usize,
        leaf_x0: usize,
        leaf_y0: usize,
    ) -> Av2Sample {
        if x >= leaf_x0 && y >= leaf_y0 {
            self.source_sample(plane, x, y)
        } else {
            self.recon_sample(plane, x, y)
        }
    }

    fn mode_decision_for_leaf(
        &self,
        decision: Av2TileDecision,
        visible_rows_mi: usize,
        visible_cols_mi: usize,
    ) -> Av2LosslessSubsampledModeDecision {
        let txb_width = decision
            .block_size
            .tx4x4_width()
            .min(visible_cols_mi.saturating_sub(decision.col));
        let txb_height = decision
            .block_size
            .tx4x4_height()
            .min(visible_rows_mi.saturating_sub(decision.row));
        let chroma_span = chroma_tx4x4_span(
            decision,
            visible_rows_mi,
            visible_cols_mi,
            self.chroma_format,
        );
        let fsc_allowed = decision.block_size.fsc_size_group().is_some();
        let mut best = (Av2LosslessSubsampledModeDecision::default(), usize::MAX);

        for use_fsc in [false, true] {
            if use_fsc && !fsc_allowed {
                continue;
            }
            let luma_candidates = [
                (Av2LumaIntraMode::Dc, None, 0usize),
                (Av2LumaIntraMode::Horizontal, None, 32usize),
                (Av2LumaIntraMode::Vertical, None, 32usize),
                (Av2LumaIntraMode::Horizontal, Some(true), 64usize),
                (Av2LumaIntraMode::Vertical, Some(false), 64usize),
            ];
            let chroma_candidates = [
                (false, Av2ChromaIntraMode::Horizontal, 0usize),
                (false, Av2ChromaIntraMode::Vertical, 0usize),
                (false, Av2ChromaIntraMode::Dc, 0usize),
                (true, Av2ChromaIntraMode::Horizontal, 64usize),
                (true, Av2ChromaIntraMode::Vertical, 64usize),
            ];

            for (luma_intra_mode, luma_bdpcm_horz, luma_syntax_penalty) in luma_candidates {
                for (chroma_use_bdpcm, chroma_intra_mode, chroma_syntax_penalty) in
                    chroma_candidates
                {
                    let mode = Av2LosslessSubsampledModeDecision {
                        luma_intra_mode,
                        luma_bdpcm_horz,
                        chroma_use_bdpcm,
                        chroma_intra_mode,
                        use_fsc,
                    };
                    let fsc_syntax_penalty = usize::from(use_fsc) * 96;
                    let score = self
                        .luma_leaf_coefficient_score(decision, txb_width, txb_height, mode)
                        + self.chroma_leaf_coefficient_score(chroma_span, mode)
                        + luma_syntax_penalty
                        + chroma_syntax_penalty
                        + fsc_syntax_penalty;
                    if score < best.1 {
                        best = (mode, score);
                    }
                }
            }
        }

        best.0
    }

    fn luma_leaf_coefficient_score(
        &self,
        decision: Av2TileDecision,
        txb_width: usize,
        txb_height: usize,
        mode: Av2LosslessSubsampledModeDecision,
    ) -> usize {
        let mut score = 0usize;
        let (leaf_x0, leaf_y0) = self.txb_origin(Av2LosslessPlane::Y, decision.col, decision.row);
        for row in 0..txb_height {
            let abs_row = decision.row + row;
            for col in 0..txb_width {
                let abs_col = decision.col + col;
                let (x0, y0) = self.txb_origin(Av2LosslessPlane::Y, abs_col, abs_row);
                let coefficients = self.tx4x4_coefficients_for_mode_score(
                    Av2LosslessPlane::Y,
                    x0,
                    y0,
                    mode,
                    leaf_x0,
                    leaf_y0,
                );
                let kind = if mode.use_fsc {
                    Av2CoefficientProxyKind::LumaIdtx
                } else {
                    Av2CoefficientProxyKind::LumaTransform
                };
                score += coefficient_proxy_score(&coefficients, kind);
            }
        }
        score
    }

    fn chroma_leaf_coefficient_score(
        &self,
        chroma_span: Av2ChromaTx4x4Span,
        mode: Av2LosslessSubsampledModeDecision,
    ) -> usize {
        let mut score = 0usize;
        for plane in [Av2LosslessPlane::U, Av2LosslessPlane::V] {
            let (leaf_x0, leaf_y0) = self.txb_origin(plane, chroma_span.col, chroma_span.row);
            for row in 0..chroma_span.height {
                let abs_row = chroma_span.row + row;
                for col in 0..chroma_span.width {
                    let abs_col = chroma_span.col + col;
                    let (x0, y0) = self.txb_origin(plane, abs_col, abs_row);
                    let coefficients = self
                        .tx4x4_coefficients_for_mode_score(plane, x0, y0, mode, leaf_x0, leaf_y0);
                    score += coefficient_proxy_score(
                        &coefficients,
                        Av2CoefficientProxyKind::ChromaTransform,
                    );
                }
            }
        }
        score
    }

    fn copy_source_to_recon_txb(&mut self, plane: Av2LosslessPlane, x0: usize, y0: usize) {
        let (plane_width, plane_height) = self.plane_geometry(plane);
        for local_y in 0..TX4X4_SIZE {
            let y = y0 + local_y;
            if y >= plane_height {
                continue;
            }
            for local_x in 0..TX4X4_SIZE {
                let x = x0 + local_x;
                if x < plane_width {
                    let sample = self.source_sample(plane, x, y);
                    self.set_recon_sample(plane, x, y, sample);
                }
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Av2LosslessPlane {
    Y,
    U,
    V,
}

fn chroma_subsample_x(chroma_format: Av2ChromaFormat) -> usize {
    match chroma_format {
        Av2ChromaFormat::Yuv420 | Av2ChromaFormat::Yuv422 => 2,
        Av2ChromaFormat::Yuv444 => 1,
    }
}

fn chroma_subsample_y(chroma_format: Av2ChromaFormat) -> usize {
    match chroma_format {
        Av2ChromaFormat::Yuv420 => 2,
        Av2ChromaFormat::Yuv422 | Av2ChromaFormat::Yuv444 => 1,
    }
}

const AV2_LOSSY_420_DC_QUANT_STEP: i32 = 8;

fn round_div_i32(value: i32, divisor: i32) -> i32 {
    debug_assert!(divisor > 0);
    if value >= 0 {
        (value + divisor / 2) / divisor
    } else {
        -((-value + divisor / 2) / divisor)
    }
}

fn quantize_i32_to_step(value: i32, step: i32) -> i32 {
    debug_assert!(step > 0);
    round_div_i32(value, step) * step
}

fn write_lossy_420_residual_coefficients(
    writer: &mut Av2EntropyWriter,
    decision: Av2TileDecision,
    visible_rows_mi: usize,
    visible_cols_mi: usize,
    contexts: &mut Av2TxbEntropyContexts,
    lossy: &mut Av2Lossy420TileState<'_>,
) {
    let txb_width = decision
        .block_size
        .tx4x4_width()
        .min(visible_cols_mi.saturating_sub(decision.col));
    let txb_height = decision
        .block_size
        .tx4x4_height()
        .min(visible_rows_mi.saturating_sub(decision.row));
    for row in 0..txb_height {
        let abs_row = decision.row + row;
        for col in 0..txb_width {
            let abs_col = decision.col + col;
            let skip_ctx =
                luma_txb_skip_context(contexts.y_above[abs_col], contexts.y_left[abs_row]);
            let dc_sign_ctx = dc_sign_context(contexts.y_above[abs_col], contexts.y_left[abs_row]);
            let (x0, y0) = lossy.txb_origin(Av2Lossy420Plane::Y, abs_col, abs_row);
            let delta = lossy.quantized_dc_delta(Av2Lossy420Plane::Y, x0, y0);
            let context = write_y_dc_delta_txb(writer, skip_ctx, dc_sign_ctx, delta);
            lossy.fill_recon_txb(Av2Lossy420Plane::Y, x0, y0, delta);
            contexts.y_above[abs_col] = context;
            contexts.y_left[abs_row] = context;
        }
    }

    let chroma_span = chroma_tx4x4_span(
        decision,
        visible_rows_mi,
        visible_cols_mi,
        Av2ChromaFormat::Yuv420,
    );
    let mut last_u_txb_nonzero = false;
    for row in 0..chroma_span.height {
        let abs_row = chroma_span.row + row;
        for col in 0..chroma_span.width {
            let abs_col = chroma_span.col + col;
            let skip_ctx =
                chroma_txb_skip_base_context(contexts.u_above[abs_col], contexts.u_left[abs_row])
                    + 6;
            let (x0, y0) = lossy.txb_origin(Av2Lossy420Plane::U, abs_col, abs_row);
            let delta = lossy.quantized_dc_delta(Av2Lossy420Plane::U, x0, y0);
            let (context, nonzero) =
                write_chroma_dc_delta_txb(writer, Av2ChromaPlane::U, skip_ctx, delta);
            lossy.fill_recon_txb(Av2Lossy420Plane::U, x0, y0, delta);
            contexts.u_above[abs_col] = context;
            contexts.u_left[abs_row] = context;
            last_u_txb_nonzero = nonzero;
        }
    }

    for row in 0..chroma_span.height {
        let abs_row = chroma_span.row + row;
        for col in 0..chroma_span.width {
            let abs_col = chroma_span.col + col;
            let skip_ctx = v_txb_skip_context_for_chroma_format(
                contexts.v_above[abs_col],
                contexts.v_left[abs_row],
                last_u_txb_nonzero,
                Av2ChromaFormat::Yuv420,
                decision.block_size,
            );
            let (x0, y0) = lossy.txb_origin(Av2Lossy420Plane::V, abs_col, abs_row);
            let delta = lossy.quantized_dc_delta(Av2Lossy420Plane::V, x0, y0);
            let (context, _) =
                write_chroma_dc_delta_txb(writer, Av2ChromaPlane::V, skip_ctx, delta);
            lossy.fill_recon_txb(Av2Lossy420Plane::V, x0, y0, delta);
            contexts.v_above[abs_col] = context;
            contexts.v_left[abs_row] = context;
        }
    }
}

fn write_lossless_subsampled_residual_coefficients(
    writer: &mut Av2EntropyWriter,
    decision: Av2TileDecision,
    visible_rows_mi: usize,
    visible_cols_mi: usize,
    contexts: &mut Av2TxbEntropyContexts,
    lossless: &mut Av2LosslessSubsampledTileState<'_>,
) {
    let mode = lossless.mode_decision_for_leaf(decision, visible_rows_mi, visible_cols_mi);
    let txb_width = decision
        .block_size
        .tx4x4_width()
        .min(visible_cols_mi.saturating_sub(decision.col));
    let txb_height = decision
        .block_size
        .tx4x4_height()
        .min(visible_rows_mi.saturating_sub(decision.row));
    if mode.use_fsc {
        write_lossless_tx_size_4x4(writer, decision.block_size);
    }
    for row in 0..txb_height {
        let abs_row = decision.row + row;
        for col in 0..txb_width {
            let abs_col = decision.col + col;
            let skip_ctx =
                luma_txb_skip_context(contexts.y_above[abs_col], contexts.y_left[abs_row]);
            let dc_sign_ctx = dc_sign_context(contexts.y_above[abs_col], contexts.y_left[abs_row]);
            let (x0, y0) = lossless.txb_origin(Av2LosslessPlane::Y, abs_col, abs_row);
            let coefficients =
                lossless.tx4x4_coefficients_for_mode(Av2LosslessPlane::Y, x0, y0, mode);
            let (context, _) = if mode.use_fsc {
                write_luma_palette_fsc_txb(writer, &coefficients)
            } else {
                write_luma_palette_residual_txb(writer, skip_ctx, dc_sign_ctx, &coefficients)
            };
            lossless.copy_source_to_recon_txb(Av2LosslessPlane::Y, x0, y0);
            contexts.y_above[abs_col] = context;
            contexts.y_left[abs_row] = context;
        }
    }

    let chroma_span = chroma_tx4x4_span(
        decision,
        visible_rows_mi,
        visible_cols_mi,
        lossless.chroma_format,
    );
    let mut last_u_txb_nonzero = false;
    for row in 0..chroma_span.height {
        let abs_row = chroma_span.row + row;
        for col in 0..chroma_span.width {
            let abs_col = chroma_span.col + col;
            let skip_ctx =
                chroma_txb_skip_base_context(contexts.u_above[abs_col], contexts.u_left[abs_row])
                    + 6;
            let (x0, y0) = lossless.txb_origin(Av2LosslessPlane::U, abs_col, abs_row);
            let coefficients =
                lossless.tx4x4_coefficients_for_mode(Av2LosslessPlane::U, x0, y0, mode);
            let (context, nonzero) = write_chroma_bdpcm_txb(
                writer,
                Av2ChromaPlane::U,
                skip_ctx,
                &coefficients,
                mode.use_fsc,
            );
            lossless.copy_source_to_recon_txb(Av2LosslessPlane::U, x0, y0);
            contexts.u_above[abs_col] = context;
            contexts.u_left[abs_row] = context;
            last_u_txb_nonzero = nonzero;
        }
    }

    for row in 0..chroma_span.height {
        let abs_row = chroma_span.row + row;
        for col in 0..chroma_span.width {
            let abs_col = chroma_span.col + col;
            let skip_ctx = v_txb_skip_context_for_chroma_format(
                contexts.v_above[abs_col],
                contexts.v_left[abs_row],
                last_u_txb_nonzero,
                lossless.chroma_format,
                decision.block_size,
            );
            let (x0, y0) = lossless.txb_origin(Av2LosslessPlane::V, abs_col, abs_row);
            let coefficients =
                lossless.tx4x4_coefficients_for_mode(Av2LosslessPlane::V, x0, y0, mode);
            let (context, _) = write_chroma_bdpcm_txb(
                writer,
                Av2ChromaPlane::V,
                skip_ctx,
                &coefficients,
                mode.use_fsc,
            );
            lossless.copy_source_to_recon_txb(Av2LosslessPlane::V, x0, y0);
            contexts.v_above[abs_col] = context;
            contexts.v_left[abs_row] = context;
        }
    }
}

fn write_luma_palette_residual_coefficients(
    writer: &mut Av2EntropyWriter,
    decision: Av2TileDecision,
    visible_rows_mi: usize,
    visible_cols_mi: usize,
    palette: &Av2LumaPalette444,
    contexts: &mut Av2TxbEntropyContexts,
    coded_mi_context: &Av2CodedMiContext,
    tile_origin_x: usize,
    tile_origin_y: usize,
    luma_bdpcm_horz: Option<bool>,
    chroma_use_bdpcm: bool,
    chroma_intra_mode: Av2ChromaIntraMode,
    use_fsc: bool,
) {
    // AV2 v1.0.0 Sections 5.20.8.4 palette_tokens() and 5.20.7.27 coeffs():
    // palette supplies a luma predictor, not an escape-coded lossless sample
    // stream. Blocks with more than eight luma values therefore need normal
    // lossless TX_4X4 coefficients for original_y - palette_prediction_y.
    // Chroma palette is not legal in this AV2 branch: av2_allow_palette()
    // accepts PLANE_TYPE_Y only, and AVM keeps palette_size[1] at zero. Chroma
    // therefore remains on an allowed DPCM residual path even though the public
    // FrameForge leaf and input packet are 8x8.
    let txb_width = decision
        .block_size
        .tx4x4_width()
        .min(visible_cols_mi.saturating_sub(decision.col));
    let txb_height = decision
        .block_size
        .tx4x4_height()
        .min(visible_rows_mi.saturating_sub(decision.row));
    let leaf_x0 = tile_origin_x + decision.col * MI_SIZE;
    let leaf_y0 = tile_origin_y + decision.row * MI_SIZE;
    let leaf_width = decision.block_size.width;
    let leaf_height = decision.block_size.height;
    if use_fsc {
        write_lossless_tx_size_4x4(writer, decision.block_size);
    }
    for row in 0..txb_height {
        let abs_row = decision.row + row;
        for col in 0..txb_width {
            let abs_col = decision.col + col;
            let skip_ctx =
                luma_txb_skip_context(contexts.y_above[abs_col], contexts.y_left[abs_row]);
            let dc_sign_ctx = dc_sign_context(contexts.y_above[abs_col], contexts.y_left[abs_row]);
            let txb_x0 = tile_origin_x + abs_col * TX4X4_SIZE;
            let txb_y0 = tile_origin_y + abs_row * TX4X4_SIZE;
            let coefficients = if use_fsc {
                luma_palette_idtx4x4_coefficients(palette, txb_x0, txb_y0)
            } else if let Some(horz) = luma_bdpcm_horz {
                luma_bdpcm_tx4x4_coefficients(
                    palette,
                    txb_x0,
                    txb_y0,
                    tile_origin_x,
                    tile_origin_y,
                    horz,
                )
            } else {
                luma_palette_tx4x4_coefficients(palette, txb_x0, txb_y0)
            };
            let (context, _) = if use_fsc {
                write_luma_palette_fsc_txb(writer, &coefficients)
            } else {
                write_luma_palette_residual_txb(writer, skip_ctx, dc_sign_ctx, &coefficients)
            };
            contexts.y_above[abs_col] = context;
            contexts.y_left[abs_row] = context;
        }
    }

    let mut last_u_txb_nonzero = false;
    for row in 0..txb_height {
        let abs_row = decision.row + row;
        for col in 0..txb_width {
            let abs_col = decision.col + col;
            let skip_ctx =
                chroma_txb_skip_base_context(contexts.u_above[abs_col], contexts.u_left[abs_row])
                    + 6;
            let txb_x0 = tile_origin_x + abs_col * TX4X4_SIZE;
            let txb_y0 = tile_origin_y + abs_row * TX4X4_SIZE;
            let coefficients = if use_fsc {
                chroma_idtx4x4_coefficients(
                    palette,
                    Av2ChromaPlane::U,
                    txb_x0,
                    txb_y0,
                    tile_origin_x,
                    tile_origin_y,
                    leaf_x0,
                    leaf_y0,
                    leaf_width,
                    leaf_height,
                    coded_mi_context,
                    chroma_use_bdpcm,
                    chroma_intra_mode,
                )
            } else if chroma_use_bdpcm {
                chroma_bdpcm_tx4x4_coefficients(
                    palette,
                    Av2ChromaPlane::U,
                    txb_x0,
                    txb_y0,
                    tile_origin_x,
                    tile_origin_y,
                    chroma_intra_mode.is_horizontal(),
                )
            } else {
                chroma_intra_tx4x4_coefficients(
                    palette,
                    Av2ChromaPlane::U,
                    txb_x0,
                    txb_y0,
                    tile_origin_x,
                    tile_origin_y,
                    leaf_x0,
                    leaf_y0,
                    leaf_width,
                    leaf_height,
                    coded_mi_context,
                    chroma_intra_mode,
                )
            };
            let (context, nonzero) =
                write_chroma_bdpcm_txb(writer, Av2ChromaPlane::U, skip_ctx, &coefficients, use_fsc);
            contexts.u_above[abs_col] = context;
            contexts.u_left[abs_row] = context;
            last_u_txb_nonzero = nonzero;
        }
    }

    for row in 0..txb_height {
        let abs_row = decision.row + row;
        for col in 0..txb_width {
            let abs_col = decision.col + col;
            let skip_ctx = v_txb_skip_context(
                contexts.v_above[abs_col],
                contexts.v_left[abs_row],
                last_u_txb_nonzero,
            );
            let txb_x0 = tile_origin_x + abs_col * TX4X4_SIZE;
            let txb_y0 = tile_origin_y + abs_row * TX4X4_SIZE;
            let coefficients = if use_fsc {
                chroma_idtx4x4_coefficients(
                    palette,
                    Av2ChromaPlane::V,
                    txb_x0,
                    txb_y0,
                    tile_origin_x,
                    tile_origin_y,
                    leaf_x0,
                    leaf_y0,
                    leaf_width,
                    leaf_height,
                    coded_mi_context,
                    chroma_use_bdpcm,
                    chroma_intra_mode,
                )
            } else if chroma_use_bdpcm {
                chroma_bdpcm_tx4x4_coefficients(
                    palette,
                    Av2ChromaPlane::V,
                    txb_x0,
                    txb_y0,
                    tile_origin_x,
                    tile_origin_y,
                    chroma_intra_mode.is_horizontal(),
                )
            } else {
                chroma_intra_tx4x4_coefficients(
                    palette,
                    Av2ChromaPlane::V,
                    txb_x0,
                    txb_y0,
                    tile_origin_x,
                    tile_origin_y,
                    leaf_x0,
                    leaf_y0,
                    leaf_width,
                    leaf_height,
                    coded_mi_context,
                    chroma_intra_mode,
                )
            };
            let (context, _) =
                write_chroma_bdpcm_txb(writer, Av2ChromaPlane::V, skip_ctx, &coefficients, use_fsc);
            contexts.v_above[abs_col] = context;
            contexts.v_left[abs_row] = context;
        }
    }
}

fn write_y_black_dc_txb(writer: &mut Av2EntropyWriter, skip_ctx: u8, dc_sign_ctx: u8) {
    write_y_txb_nonzero(writer, skip_ctx);
    write_eob_one_y(writer);
    write_y_dc_level(writer, BLACK_LOSSLESS_DC_LEVEL);
    write_y_negative_dc_sign(writer, dc_sign_ctx);
    write_y_dc_high_range(writer, BLACK_LOSSLESS_DC_LEVEL);
}

fn write_y_dc_delta_txb(
    writer: &mut Av2EntropyWriter,
    skip_ctx: u8,
    dc_sign_ctx: u8,
    delta: i16,
) -> u8 {
    if delta == 0 {
        write_y_txb_all_zero(writer, skip_ctx);
        return 0;
    }
    let level = dc_delta_level(delta);
    write_y_txb_nonzero(writer, skip_ctx);
    write_eob_one_y(writer);
    write_y_dc_level(writer, level);
    write_y_dc_sign(writer, delta < 0, dc_sign_ctx);
    write_y_dc_high_range(writer, level);
    lossless_entropy_context(u32::from(level), i32::from(delta.signum()))
}

fn write_u_black_dc_txb(writer: &mut Av2EntropyWriter, skip_ctx: u8) {
    let context = write_u_lossless_dc_txb(writer, skip_ctx, 0);
    assert_eq!(context, NONZERO_NEGATIVE_DC_ENTROPY_CONTEXT);
}

fn write_v_black_dc_txb(writer: &mut Av2EntropyWriter, skip_ctx: u8) {
    let context = write_v_lossless_dc_txb(writer, skip_ctx, 0);
    assert_eq!(context, NONZERO_NEGATIVE_DC_ENTROPY_CONTEXT);
}

fn write_u_lossless_dc_txb(writer: &mut Av2EntropyWriter, skip_ctx: u8, sample: u8) -> u8 {
    if sample == LOSSLESS_DC_PREDICTOR {
        write_u_txb_all_zero(writer, skip_ctx, false);
        return 0;
    }
    let (level, negative) = lossless_dc_level_for_sample(sample);
    write_u_txb_nonzero(writer, skip_ctx, false);
    write_eob_one_uv(writer);
    write_uv_dc_level(writer, level);
    writer.write_literal("tile.coeff.u.dc_sign_negative", u32::from(negative), 1);
    write_uv_dc_high_range(writer, level);
    nonzero_dc_entropy_context(negative)
}

fn write_v_lossless_dc_txb(writer: &mut Av2EntropyWriter, skip_ctx: u8, sample: u8) -> u8 {
    if sample == LOSSLESS_DC_PREDICTOR {
        write_v_txb_all_zero(writer, skip_ctx);
        return 0;
    }
    let (level, negative) = lossless_dc_level_for_sample(sample);
    write_v_txb_nonzero(writer, skip_ctx);
    write_eob_one_uv(writer);
    write_uv_dc_level(writer, level);
    writer.write_literal("tile.coeff.v.dc_sign_negative", u32::from(negative), 1);
    write_uv_dc_high_range(writer, level);
    nonzero_dc_entropy_context(negative)
}

fn write_chroma_dc_delta_txb(
    writer: &mut Av2EntropyWriter,
    plane: Av2ChromaPlane,
    skip_ctx: u8,
    delta: i16,
) -> (u8, bool) {
    if delta == 0 {
        match plane {
            Av2ChromaPlane::U => write_u_txb_all_zero(writer, skip_ctx, false),
            Av2ChromaPlane::V => write_v_txb_all_zero(writer, skip_ctx),
        }
        return (0, false);
    }

    let level = dc_delta_level(delta);
    match plane {
        Av2ChromaPlane::U => write_u_txb_nonzero(writer, skip_ctx, false),
        Av2ChromaPlane::V => write_v_txb_nonzero(writer, skip_ctx),
    }
    write_eob_one_uv(writer);
    write_uv_dc_level(writer, level);
    let sign_name = match plane {
        Av2ChromaPlane::U => "tile.coeff.u.dc_sign_negative",
        Av2ChromaPlane::V => "tile.coeff.v.dc_sign_negative",
    };
    writer.write_literal(sign_name, u32::from(delta < 0), 1);
    write_uv_dc_high_range(writer, level);
    (
        lossless_entropy_context(u32::from(level), i32::from(delta.signum())),
        true,
    )
}

fn dc_delta_level(delta: i16) -> u16 {
    (i32::from(delta).unsigned_abs() as u16) * 4
}

fn luma_palette_tx4x4_coefficients(
    palette: &Av2LumaPalette444,
    x0: usize,
    y0: usize,
) -> [i32; TX4X4_SAMPLES] {
    let mut residual = [0i32; TX4X4_SAMPLES];
    for local_y in 0..TX4X4_SIZE {
        let y = y0 + local_y;
        for local_x in 0..TX4X4_SIZE {
            let x = x0 + local_x;
            let original = i32::from(palette.y_sample(x, y));
            let predicted = i32::from(palette.luma_prediction_sample(x, y));
            residual[local_y * TX4X4_SIZE + local_x] = original - predicted;
        }
    }

    av2_fwht4x4(&residual)
}

fn luma_palette_idtx4x4_coefficients(
    palette: &Av2LumaPalette444,
    x0: usize,
    y0: usize,
) -> [i32; TX4X4_SAMPLES] {
    let mut residual = [0i32; TX4X4_SAMPLES];
    for local_y in 0..TX4X4_SIZE {
        let y = y0 + local_y;
        for local_x in 0..TX4X4_SIZE {
            let x = x0 + local_x;
            let original = i32::from(palette.y_sample(x, y));
            let predicted = i32::from(palette.luma_prediction_sample(x, y));
            residual[local_y * TX4X4_SIZE + local_x] = original - predicted;
        }
    }

    idtx4x4_coefficients(&residual)
}

fn luma_palette_fsc_is_rate_worthy(
    palette: &Av2LumaPalette444,
    leaf_x0: usize,
    leaf_y0: usize,
    tile_origin_x: usize,
    tile_origin_y: usize,
    chroma_use_bdpcm: bool,
    chroma_intra_mode: Av2ChromaIntraMode,
) -> bool {
    let coded_mi_context = Av2CodedMiContext::new(PARTITION_CONTEXT_DIM, PARTITION_CONTEXT_DIM);
    let mut fsc_score = 96usize;
    let mut transform_score = 0usize;

    for row in 0..(AV2_LUMA_PALETTE_BLOCK_SIZE / TX4X4_SIZE) {
        for col in 0..(AV2_LUMA_PALETTE_BLOCK_SIZE / TX4X4_SIZE) {
            let txb_x0 = leaf_x0 + col * TX4X4_SIZE;
            let txb_y0 = leaf_y0 + row * TX4X4_SIZE;

            fsc_score += coefficient_proxy_score(
                &luma_palette_idtx4x4_coefficients(palette, txb_x0, txb_y0),
                Av2CoefficientProxyKind::LumaIdtx,
            );
            transform_score += coefficient_proxy_score(
                &luma_palette_tx4x4_coefficients(palette, txb_x0, txb_y0),
                Av2CoefficientProxyKind::LumaTransform,
            );

            for plane in [Av2ChromaPlane::U, Av2ChromaPlane::V] {
                fsc_score += coefficient_proxy_score(
                    &chroma_idtx4x4_coefficients(
                        palette,
                        plane,
                        txb_x0,
                        txb_y0,
                        tile_origin_x,
                        tile_origin_y,
                        leaf_x0,
                        leaf_y0,
                        AV2_LUMA_PALETTE_BLOCK_SIZE,
                        AV2_LUMA_PALETTE_BLOCK_SIZE,
                        &coded_mi_context,
                        chroma_use_bdpcm,
                        chroma_intra_mode,
                    ),
                    Av2CoefficientProxyKind::ChromaTransform,
                );
                let transform_coefficients = if chroma_use_bdpcm {
                    chroma_bdpcm_tx4x4_coefficients(
                        palette,
                        plane,
                        txb_x0,
                        txb_y0,
                        tile_origin_x,
                        tile_origin_y,
                        chroma_intra_mode.is_horizontal(),
                    )
                } else {
                    chroma_intra_tx4x4_coefficients(
                        palette,
                        plane,
                        txb_x0,
                        txb_y0,
                        tile_origin_x,
                        tile_origin_y,
                        leaf_x0,
                        leaf_y0,
                        AV2_LUMA_PALETTE_BLOCK_SIZE,
                        AV2_LUMA_PALETTE_BLOCK_SIZE,
                        &coded_mi_context,
                        chroma_intra_mode,
                    )
                };
                transform_score += coefficient_proxy_score(
                    &transform_coefficients,
                    Av2CoefficientProxyKind::ChromaTransform,
                );
            }
        }
    }

    fsc_score < transform_score
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Av2CoefficientProxyKind {
    LumaTransform,
    LumaIdtx,
    ChromaTransform,
}

fn coefficient_proxy_score(
    coefficients: &[i32; TX4X4_SAMPLES],
    kind: Av2CoefficientProxyKind,
) -> usize {
    let levels = lossless_coefficient_levels(coefficients);
    let Some((first, eob)) = tx4x4_nonzero_bounds(&levels) else {
        return 16;
    };

    let range = match kind {
        Av2CoefficientProxyKind::LumaIdtx => first..TX4X4_SAMPLES,
        Av2CoefficientProxyKind::LumaTransform | Av2CoefficientProxyKind::ChromaTransform => 0..eob,
    };
    let mut score = 96 + range.len() * 10;
    for scan_index in range {
        let pos = TX4X4_SCAN[scan_index];
        let level = levels[pos] as usize;
        if level == 0 {
            continue;
        }
        score += 80 + level.min(12) * 14;
    }

    let mut high_range_avg = 0u32;
    match kind {
        Av2CoefficientProxyKind::LumaIdtx => {
            for scan_index in 0..TX4X4_SAMPLES {
                score += coefficient_high_range_proxy_score(
                    &levels,
                    kind,
                    TX4X4_SCAN[scan_index],
                    &mut high_range_avg,
                );
            }
        }
        Av2CoefficientProxyKind::LumaTransform | Av2CoefficientProxyKind::ChromaTransform => {
            for scan_index in (0..eob).rev() {
                score += coefficient_high_range_proxy_score(
                    &levels,
                    kind,
                    TX4X4_SCAN[scan_index],
                    &mut high_range_avg,
                );
            }
        }
    }

    score
}

fn coefficient_high_range_proxy_score(
    levels: &[u32; TX4X4_SAMPLES],
    kind: Av2CoefficientProxyKind,
    pos: usize,
    high_range_avg: &mut u32,
) -> usize {
    let level = levels[pos];
    if level == 0 {
        return 0;
    }
    let (threshold, decoded_base) = match kind {
        Av2CoefficientProxyKind::LumaIdtx => (5, 6),
        Av2CoefficientProxyKind::LumaTransform if luma_lf_limits(pos) => (7, 8),
        Av2CoefficientProxyKind::LumaTransform => (5, 6),
        Av2CoefficientProxyKind::ChromaTransform if chroma_lf_limits(pos) => (4, 5),
        Av2CoefficientProxyKind::ChromaTransform => (5, 6),
    };
    if level <= threshold {
        return 0;
    }
    let high_range = level.saturating_sub(decoded_base);
    let score = adaptive_high_range_score_bits(high_range, *high_range_avg) * 64;
    *high_range_avg = (*high_range_avg + high_range) >> 1;
    score
}

fn tx4x4_nonzero_bounds(levels: &[u32; TX4X4_SAMPLES]) -> Option<(usize, usize)> {
    let first = TX4X4_SCAN.iter().position(|&pos| levels[pos] != 0)?;
    let eob = TX4X4_SCAN
        .iter()
        .rposition(|&pos| levels[pos] != 0)
        .map(|index| index + 1)
        .expect("first nonzero implies eob");
    Some((first, eob))
}

fn adaptive_high_range_score_bits(value: u32, context: u32) -> usize {
    let m = adaptive_high_range_rice_parameter(context);
    truncated_rice_score_bits(value, m, m + 1, (m + 4).min(6))
}

fn truncated_rice_score_bits(value: u32, m: u8, k: u8, cmax: u8) -> usize {
    let q = value >> m;
    if q >= u32::from(cmax) {
        usize::from(cmax) + exp_golomb_score_bits(value - (u32::from(cmax) << m), k)
    } else {
        q as usize + 1 + usize::from(m)
    }
}

fn exp_golomb_score_bits(value: u32, k: u8) -> usize {
    let x = value + (1u32 << k);
    let length = (u32::BITS - x.leading_zeros()) as u8;
    usize::from(length - 1 - k + length)
}

fn luma_bdpcm_tx4x4_coefficients(
    palette: &Av2LumaPalette444,
    x0: usize,
    y0: usize,
    tile_origin_x: usize,
    tile_origin_y: usize,
    horz: bool,
) -> [i32; TX4X4_SAMPLES] {
    let mut residual = [0i32; TX4X4_SAMPLES];
    for local_y in 0..TX4X4_SIZE {
        let y = y0 + local_y;
        for local_x in 0..TX4X4_SIZE {
            let x = x0 + local_x;
            let sample = i32::from(palette.y_sample(x, y));
            let predicted_delta = if horz {
                let row_predictor = i32::from(luma_h_predictor(
                    palette,
                    x0,
                    y0,
                    local_y,
                    tile_origin_x,
                    tile_origin_y,
                ));
                if local_x == 0 {
                    sample - row_predictor
                } else {
                    let previous = i32::from(palette.y_sample(x - 1, y));
                    sample - previous
                }
            } else if local_y == 0 {
                let col_predictor = i32::from(luma_v_predictor(
                    palette,
                    x0,
                    y0,
                    local_x,
                    tile_origin_x,
                    tile_origin_y,
                ));
                sample - col_predictor
            } else {
                let previous = i32::from(palette.y_sample(x, y - 1));
                sample - previous
            };
            residual[local_y * TX4X4_SIZE + local_x] = predicted_delta;
        }
    }

    av2_fwht4x4(&residual)
}

fn chroma_bdpcm_tx4x4_coefficients(
    palette: &Av2LumaPalette444,
    plane: Av2ChromaPlane,
    x0: usize,
    y0: usize,
    tile_origin_x: usize,
    tile_origin_y: usize,
    horz: bool,
) -> [i32; TX4X4_SAMPLES] {
    let residual =
        chroma_bdpcm_residual4x4(palette, plane, x0, y0, tile_origin_x, tile_origin_y, horz);
    av2_fwht4x4(&residual)
}

fn chroma_bdpcm_residual4x4(
    palette: &Av2LumaPalette444,
    plane: Av2ChromaPlane,
    x0: usize,
    y0: usize,
    tile_origin_x: usize,
    tile_origin_y: usize,
    horz: bool,
) -> [i32; TX4X4_SAMPLES] {
    let mut residual = [0i32; TX4X4_SAMPLES];
    for local_y in 0..TX4X4_SIZE {
        let y = y0 + local_y;
        for local_x in 0..TX4X4_SIZE {
            let x = x0 + local_x;
            let sample = i32::from(chroma_sample(palette, plane, x, y));
            let predicted_delta = if horz {
                let row_predictor = i32::from(chroma_h_predictor(
                    palette,
                    plane,
                    x0,
                    y0,
                    local_y,
                    tile_origin_x,
                    tile_origin_y,
                ));
                if local_x == 0 {
                    sample - row_predictor
                } else {
                    let previous = i32::from(chroma_sample(palette, plane, x - 1, y));
                    sample - previous
                }
            } else if local_y == 0 {
                let col_predictor = i32::from(chroma_v_predictor(
                    palette,
                    plane,
                    x0,
                    y0,
                    local_x,
                    tile_origin_x,
                    tile_origin_y,
                ));
                sample - col_predictor
            } else {
                let previous = i32::from(chroma_sample(palette, plane, x, y - 1));
                sample - previous
            };
            residual[local_y * TX4X4_SIZE + local_x] = predicted_delta;
        }
    }

    residual
}

fn chroma_intra_tx4x4_coefficients(
    palette: &Av2LumaPalette444,
    plane: Av2ChromaPlane,
    x0: usize,
    y0: usize,
    tile_origin_x: usize,
    tile_origin_y: usize,
    leaf_x0: usize,
    leaf_y0: usize,
    leaf_width: usize,
    leaf_height: usize,
    coded_mi_context: &Av2CodedMiContext,
    mode: Av2ChromaIntraMode,
) -> [i32; TX4X4_SAMPLES] {
    let residual = chroma_intra_residual4x4(
        palette,
        plane,
        x0,
        y0,
        tile_origin_x,
        tile_origin_y,
        leaf_x0,
        leaf_y0,
        leaf_width,
        leaf_height,
        coded_mi_context,
        mode,
    );
    av2_fwht4x4(&residual)
}

fn chroma_intra_residual4x4(
    palette: &Av2LumaPalette444,
    plane: Av2ChromaPlane,
    x0: usize,
    y0: usize,
    tile_origin_x: usize,
    tile_origin_y: usize,
    leaf_x0: usize,
    leaf_y0: usize,
    leaf_width: usize,
    leaf_height: usize,
    coded_mi_context: &Av2CodedMiContext,
    mode: Av2ChromaIntraMode,
) -> [i32; TX4X4_SAMPLES] {
    let dc_predictor =
        (mode == Av2ChromaIntraMode::Dc).then(|| chroma_dc_predictor(palette, plane, x0, y0));
    let smooth_edges = matches!(
        mode,
        Av2ChromaIntraMode::Smooth
            | Av2ChromaIntraMode::SmoothVertical
            | Av2ChromaIntraMode::SmoothHorizontal
    )
    .then(|| {
        chroma_smooth_edges(
            palette,
            plane,
            x0,
            y0,
            tile_origin_x,
            tile_origin_y,
            leaf_x0,
            leaf_y0,
            leaf_width,
            leaf_height,
            coded_mi_context,
        )
    });
    let mut residual = [0i32; TX4X4_SAMPLES];
    for local_y in 0..TX4X4_SIZE {
        let y = y0 + local_y;
        for local_x in 0..TX4X4_SIZE {
            let x = x0 + local_x;
            let sample = i32::from(chroma_sample(palette, plane, x, y));
            let predictor = match mode {
                Av2ChromaIntraMode::Dc => dc_predictor.expect("DC predictor is precomputed"),
                Av2ChromaIntraMode::Horizontal => {
                    if x0 != tile_origin_x {
                        chroma_sample(palette, plane, x0 - 1, y)
                    } else if y0 != tile_origin_y {
                        chroma_sample(palette, plane, x0, y0 - 1)
                    } else {
                        av2_lossless_h_pred_left_edge(palette.bit_depth())
                    }
                }
                Av2ChromaIntraMode::Vertical => {
                    if y0 != tile_origin_y {
                        chroma_sample(palette, plane, x, y0 - 1)
                    } else if x0 != tile_origin_x {
                        chroma_sample(palette, plane, x0 - 1, y0)
                    } else {
                        av2_lossless_v_pred_above_edge(palette.bit_depth())
                    }
                }
                Av2ChromaIntraMode::Directional45 => {
                    let above = chroma_d45_above_edge(
                        palette,
                        plane,
                        x0,
                        y0,
                        tile_origin_x,
                        tile_origin_y,
                        leaf_x0,
                        leaf_y0,
                        leaf_width,
                        coded_mi_context,
                    );
                    above[local_y + local_x + 1]
                }
                Av2ChromaIntraMode::Directional67 => {
                    let above = chroma_d45_above_edge(
                        palette,
                        plane,
                        x0,
                        y0,
                        tile_origin_x,
                        tile_origin_y,
                        leaf_x0,
                        leaf_y0,
                        leaf_width,
                        coded_mi_context,
                    );
                    directional_interpolate(above, local_x, local_y)
                }
                Av2ChromaIntraMode::Directional135 => {
                    let edges =
                        chroma_d135_edges(palette, plane, x0, y0, tile_origin_x, tile_origin_y);
                    if local_x >= local_y {
                        let offset = local_x - local_y;
                        if offset == 0 {
                            edges.above_left
                        } else {
                            edges.above[offset - 1]
                        }
                    } else {
                        edges.left[local_y - local_x - 1]
                    }
                }
                Av2ChromaIntraMode::Directional113 => {
                    let edges =
                        chroma_d135_edges(palette, plane, x0, y0, tile_origin_x, tile_origin_y);
                    zone2_directional_predictor(edges, 24, 170, local_x, local_y)
                }
                Av2ChromaIntraMode::Directional157 => {
                    let edges =
                        chroma_d135_edges(palette, plane, x0, y0, tile_origin_x, tile_origin_y);
                    zone2_directional_predictor(edges, 170, 24, local_x, local_y)
                }
                Av2ChromaIntraMode::Directional203 => {
                    let left = chroma_d203_left_edge(
                        palette,
                        plane,
                        x0,
                        y0,
                        tile_origin_x,
                        tile_origin_y,
                        leaf_x0,
                        leaf_y0,
                        leaf_height,
                        coded_mi_context,
                    );
                    directional_interpolate(left, local_y, local_x)
                }
                Av2ChromaIntraMode::Smooth
                | Av2ChromaIntraMode::SmoothVertical
                | Av2ChromaIntraMode::SmoothHorizontal => {
                    let (above, left) =
                        smooth_edges.expect("smooth predictor edges are precomputed");
                    av2_highbd_smooth_intra_predictor(
                        mode,
                        above,
                        left,
                        local_x,
                        local_y,
                        palette.bit_depth(),
                    )
                }
                Av2ChromaIntraMode::Paeth => {
                    let left = chroma_h_predictor(
                        palette,
                        plane,
                        x0,
                        y0,
                        local_y,
                        tile_origin_x,
                        tile_origin_y,
                    );
                    let above = chroma_v_predictor(
                        palette,
                        plane,
                        x0,
                        y0,
                        local_x,
                        tile_origin_x,
                        tile_origin_y,
                    );
                    let above_left = chroma_above_left_predictor(
                        palette,
                        plane,
                        x0,
                        y0,
                        tile_origin_x,
                        tile_origin_y,
                    );
                    paeth_predictor(left, above, above_left)
                }
            };
            residual[local_y * TX4X4_SIZE + local_x] = sample - i32::from(predictor);
        }
    }

    residual
}

fn chroma_idtx4x4_coefficients(
    palette: &Av2LumaPalette444,
    plane: Av2ChromaPlane,
    x0: usize,
    y0: usize,
    tile_origin_x: usize,
    tile_origin_y: usize,
    leaf_x0: usize,
    leaf_y0: usize,
    leaf_width: usize,
    leaf_height: usize,
    coded_mi_context: &Av2CodedMiContext,
    use_bdpcm: bool,
    mode: Av2ChromaIntraMode,
) -> [i32; TX4X4_SAMPLES] {
    let residual = if use_bdpcm {
        chroma_bdpcm_residual4x4(
            palette,
            plane,
            x0,
            y0,
            tile_origin_x,
            tile_origin_y,
            mode.is_horizontal(),
        )
    } else {
        chroma_intra_residual4x4(
            palette,
            plane,
            x0,
            y0,
            tile_origin_x,
            tile_origin_y,
            leaf_x0,
            leaf_y0,
            leaf_width,
            leaf_height,
            coded_mi_context,
            mode,
        )
    };
    idtx4x4_coefficients(&residual)
}

fn idtx4x4_coefficients(residual: &[i32; TX4X4_SAMPLES]) -> [i32; TX4X4_SAMPLES] {
    let mut coefficients = [0i32; TX4X4_SAMPLES];
    for (coefficient, residual) in coefficients.iter_mut().zip(residual.iter()) {
        *coefficient = *residual * 8;
    }
    coefficients
}

fn chroma_d45_above_edge(
    palette: &Av2LumaPalette444,
    plane: Av2ChromaPlane,
    txb_x0: usize,
    txb_y0: usize,
    tile_origin_x: usize,
    tile_origin_y: usize,
    leaf_x0: usize,
    leaf_y0: usize,
    leaf_width: usize,
    coded_mi_context: &Av2CodedMiContext,
) -> [Av2Sample; 8] {
    let sb_origin_x = (txb_x0 / MVP_SUPERBLOCK_SIZE) * MVP_SUPERBLOCK_SIZE;
    let sb_right = (sb_origin_x + MVP_SUPERBLOCK_SIZE).min(palette.width());
    let have_top = txb_y0 > tile_origin_y;
    let have_left = txb_x0 > tile_origin_x;
    let mut above = [av2_lossless_v_pred_above_edge(palette.bit_depth()); 8];
    if have_top {
        for index in 0..above.len() {
            let x = txb_x0 + index;
            let external_top_right_coded = txb_y0 == leaf_y0
                && x < sb_right
                && coded_mi_context.is_coded((txb_y0 - 1) / MI_SIZE, x / MI_SIZE);
            if x < leaf_x0 + leaf_width || external_top_right_coded {
                above[index] = chroma_sample(palette, plane, x, txb_y0 - 1);
            } else if index > 0 {
                above[index] = above[index - 1];
            }
        }
    } else if have_left {
        above.fill(chroma_sample(palette, plane, txb_x0 - 1, txb_y0));
    }
    above
}

#[derive(Debug, Clone, Copy)]
struct ChromaD135Edges {
    above_left: Av2Sample,
    above: [Av2Sample; 4],
    left: [Av2Sample; 4],
}

fn chroma_d135_edges(
    palette: &Av2LumaPalette444,
    plane: Av2ChromaPlane,
    txb_x0: usize,
    txb_y0: usize,
    tile_origin_x: usize,
    tile_origin_y: usize,
) -> ChromaD135Edges {
    let have_top = txb_y0 > tile_origin_y;
    let have_left = txb_x0 > tile_origin_x;
    let mut above = [av2_lossless_v_pred_above_edge(palette.bit_depth()); 4];
    let mut left = [av2_lossless_h_pred_left_edge(palette.bit_depth()); 4];
    if have_top {
        for local_x in 0..4 {
            above[local_x] = chroma_sample(palette, plane, txb_x0 + local_x, txb_y0 - 1);
        }
    } else if have_left {
        above.fill(chroma_sample(palette, plane, txb_x0 - 1, txb_y0));
    }
    if have_left {
        for local_y in 0..4 {
            left[local_y] = chroma_sample(palette, plane, txb_x0 - 1, txb_y0 + local_y);
        }
    } else if have_top {
        left.fill(chroma_sample(palette, plane, txb_x0, txb_y0 - 1));
    }
    let above_left = if have_top && have_left {
        chroma_sample(palette, plane, txb_x0 - 1, txb_y0 - 1)
    } else if have_top {
        above[0]
    } else if have_left {
        left[0]
    } else {
        av2_lossless_dc_predictor(palette.bit_depth())
    };
    ChromaD135Edges {
        above_left,
        above,
        left,
    }
}

fn chroma_d203_left_edge(
    palette: &Av2LumaPalette444,
    plane: Av2ChromaPlane,
    txb_x0: usize,
    txb_y0: usize,
    tile_origin_x: usize,
    tile_origin_y: usize,
    leaf_x0: usize,
    leaf_y0: usize,
    leaf_height: usize,
    coded_mi_context: &Av2CodedMiContext,
) -> [Av2Sample; 8] {
    let sb_origin_y = (txb_y0 / MVP_SUPERBLOCK_SIZE) * MVP_SUPERBLOCK_SIZE;
    let sb_bottom = (sb_origin_y + MVP_SUPERBLOCK_SIZE).min(palette.height());
    let have_top = txb_y0 > tile_origin_y;
    let have_left = txb_x0 > tile_origin_x;
    let mut left = [av2_lossless_h_pred_left_edge(palette.bit_depth()); 8];
    if have_left {
        for index in 0..left.len() {
            let y = txb_y0 + index;
            let external_bottom_left_coded = txb_x0 == leaf_x0
                && y < sb_bottom
                && coded_mi_context.is_coded(y / MI_SIZE, (txb_x0 - 1) / MI_SIZE);
            // Match AVM has_bottom_left(): only TXBs on the leaf's left edge
            // may use D203 bottom-left overhang samples.
            if y < txb_y0 + TX4X4_SIZE
                || (txb_x0 == leaf_x0 && (y < leaf_y0 + leaf_height || external_bottom_left_coded))
            {
                left[index] = chroma_sample(palette, plane, txb_x0 - 1, y);
            } else if index > 0 {
                left[index] = left[index - 1];
            }
        }
    } else if have_top {
        left.fill(chroma_sample(palette, plane, txb_x0, txb_y0 - 1));
    }
    left
}

fn directional_interpolate(edge: [Av2Sample; 8], along: usize, across: usize) -> Av2Sample {
    // AVM dr_intra_derivative[67], used by both D67 and D203.
    const DERIVATIVE_67_203: usize = 24;
    let projected = DERIVATIVE_67_203 * (across + 1);
    let base = (projected >> 6) + along;
    let shift = (projected & 0x3f) >> 1;
    let value =
        u32::from(edge[base]) * (32 - shift) as u32 + u32::from(edge[base + 1]) * shift as u32;
    ((value + 16) >> 5) as Av2Sample
}

fn zone2_directional_predictor(
    edges: ChromaD135Edges,
    dx: i32,
    dy: i32,
    local_x: usize,
    local_y: usize,
) -> Av2Sample {
    let projected_x = ((local_x as i32) << 6) - ((local_y as i32 + 1) * dx);
    let base_x = projected_x >> 6;
    if base_x >= -1 {
        let shift = ((projected_x & 0x3f) >> 1) as usize;
        return directional_weighted_sample(
            zone2_above_sample(edges, base_x),
            zone2_above_sample(edges, base_x + 1),
            shift,
        );
    }

    let projected_y = ((local_y as i32) << 6) - ((local_x as i32 + 1) * dy);
    let base_y = projected_y >> 6;
    debug_assert!(base_y >= -1);
    let shift = ((projected_y & 0x3f) >> 1) as usize;
    directional_weighted_sample(
        zone2_left_sample(edges, base_y),
        zone2_left_sample(edges, base_y + 1),
        shift,
    )
}

fn zone2_above_sample(edges: ChromaD135Edges, offset: i32) -> Av2Sample {
    if offset < 0 {
        edges.above_left
    } else {
        edges.above[offset as usize]
    }
}

fn zone2_left_sample(edges: ChromaD135Edges, offset: i32) -> Av2Sample {
    if offset < 0 {
        edges.above_left
    } else {
        edges.left[offset as usize]
    }
}

fn directional_weighted_sample(first: Av2Sample, second: Av2Sample, shift: usize) -> Av2Sample {
    let value = u32::from(first) * (32 - shift) as u32 + u32::from(second) * shift as u32;
    ((value + 16) >> 5) as Av2Sample
}

fn chroma_above_left_predictor(
    palette: &Av2LumaPalette444,
    plane: Av2ChromaPlane,
    x0: usize,
    y0: usize,
    tile_origin_x: usize,
    tile_origin_y: usize,
) -> Av2Sample {
    let have_left = x0 > tile_origin_x;
    let have_top = y0 > tile_origin_y;
    if have_left && have_top {
        chroma_sample(palette, plane, x0 - 1, y0 - 1)
    } else if have_top {
        chroma_sample(palette, plane, x0, y0 - 1)
    } else if have_left {
        chroma_sample(palette, plane, x0 - 1, y0)
    } else {
        av2_lossless_dc_predictor(palette.bit_depth())
    }
}

fn chroma_smooth_edges(
    palette: &Av2LumaPalette444,
    plane: Av2ChromaPlane,
    txb_x0: usize,
    txb_y0: usize,
    tile_origin_x: usize,
    tile_origin_y: usize,
    leaf_x0: usize,
    leaf_y0: usize,
    leaf_width: usize,
    leaf_height: usize,
    coded_mi_context: &Av2CodedMiContext,
) -> ([Av2Sample; 5], [Av2Sample; 5]) {
    debug_assert!(txb_x0 >= leaf_x0 && txb_y0 >= leaf_y0);
    debug_assert!(txb_x0 + TX4X4_SIZE <= leaf_x0 + leaf_width);
    debug_assert!(txb_y0 + TX4X4_SIZE <= leaf_y0 + leaf_height);
    let have_top = txb_y0 > tile_origin_y;
    let have_left = txb_x0 > tile_origin_x;
    let mut above = [av2_lossless_v_pred_above_edge(palette.bit_depth()); TX4X4_SIZE + 1];
    let mut left = [av2_lossless_h_pred_left_edge(palette.bit_depth()); TX4X4_SIZE + 1];

    if have_top {
        for local_x in 0..TX4X4_SIZE {
            above[local_x] = chroma_sample(palette, plane, txb_x0 + local_x, txb_y0 - 1);
        }
    } else if have_left {
        above[..TX4X4_SIZE].fill(chroma_sample(palette, plane, txb_x0 - 1, txb_y0));
    }

    if have_left {
        for local_y in 0..TX4X4_SIZE {
            left[local_y] = chroma_sample(palette, plane, txb_x0 - 1, txb_y0 + local_y);
        }
    } else if have_top {
        left[..TX4X4_SIZE].fill(chroma_sample(palette, plane, txb_x0, txb_y0 - 1));
    }

    let sb_origin_x = (txb_x0 / MVP_SUPERBLOCK_SIZE) * MVP_SUPERBLOCK_SIZE;
    let sb_right = (sb_origin_x + MVP_SUPERBLOCK_SIZE).min(palette.width());
    let external_top_right_coded = have_top
        && txb_y0 == leaf_y0
        && txb_x0 + TX4X4_SIZE < sb_right
        && coded_mi_context.is_coded((txb_y0 - 1) / MI_SIZE, (txb_x0 + TX4X4_SIZE) / MI_SIZE);
    if have_top && (txb_x0 + TX4X4_SIZE < leaf_x0 + leaf_width || external_top_right_coded) {
        above[TX4X4_SIZE] = chroma_sample(palette, plane, txb_x0 + TX4X4_SIZE, txb_y0 - 1);
    } else {
        above[TX4X4_SIZE] = above[TX4X4_SIZE - 1];
    }

    let sb_origin_y = (txb_y0 / MVP_SUPERBLOCK_SIZE) * MVP_SUPERBLOCK_SIZE;
    let sb_bottom = (sb_origin_y + MVP_SUPERBLOCK_SIZE).min(palette.height());
    let external_bottom_left_coded = have_left
        && txb_x0 == leaf_x0
        && txb_y0 + TX4X4_SIZE < sb_bottom
        && coded_mi_context.is_coded((txb_y0 + TX4X4_SIZE) / MI_SIZE, (txb_x0 - 1) / MI_SIZE);
    if have_left
        && txb_x0 == leaf_x0
        && (txb_y0 + TX4X4_SIZE < leaf_y0 + leaf_height || external_bottom_left_coded)
    {
        left[TX4X4_SIZE] = chroma_sample(palette, plane, txb_x0 - 1, txb_y0 + TX4X4_SIZE);
    } else {
        left[TX4X4_SIZE] = left[TX4X4_SIZE - 1];
    }

    (above, left)
}

fn paeth_predictor(left: Av2Sample, above: Av2Sample, above_left: Av2Sample) -> Av2Sample {
    let left = i32::from(left);
    let above = i32::from(above);
    let above_left = i32::from(above_left);
    let base = left + above - above_left;
    let p_left = (base - left).abs();
    let p_above = (base - above).abs();
    let p_above_left = (base - above_left).abs();
    if p_left <= p_above && p_left <= p_above_left {
        left as Av2Sample
    } else if p_above <= p_above_left {
        above as Av2Sample
    } else {
        above_left as Av2Sample
    }
}

fn chroma_dc_predictor(
    palette: &Av2LumaPalette444,
    plane: Av2ChromaPlane,
    x0: usize,
    y0: usize,
) -> Av2Sample {
    let tile_origin_x = (x0 / MVP_SUPERBLOCK_SIZE) * MVP_SUPERBLOCK_SIZE;
    let tile_origin_y = (y0 / MVP_SUPERBLOCK_SIZE) * MVP_SUPERBLOCK_SIZE;
    let have_left = x0 != tile_origin_x;
    let have_top = y0 != tile_origin_y;
    if !have_left && !have_top {
        return av2_lossless_dc_predictor(palette.bit_depth());
    }

    let mut sum = 0u32;
    let mut count = 0u32;
    if have_top {
        for local_x in 0..TX4X4_SIZE {
            sum += u32::from(chroma_sample(palette, plane, x0 + local_x, y0 - 1));
            count += 1;
        }
    }
    if have_left {
        for local_y in 0..TX4X4_SIZE {
            sum += u32::from(chroma_sample(palette, plane, x0 - 1, y0 + local_y));
            count += 1;
        }
    }
    ((sum + count / 2) / count) as Av2Sample
}

fn luma_h_predictor(
    palette: &Av2LumaPalette444,
    x0: usize,
    y0: usize,
    local_y: usize,
    tile_origin_x: usize,
    tile_origin_y: usize,
) -> Av2Sample {
    // AV2 v1.0.0 Section 7.11 H_PRED with lossless DPCM uses the same
    // intra-prediction edge as normal horizontal prediction before
    // avm_highbd_subtract_block_horz() differentials src-pred.
    if x0 > tile_origin_x {
        palette.y_sample(x0 - 1, y0 + local_y)
    } else if y0 > tile_origin_y {
        palette.y_sample(x0, y0 - 1)
    } else {
        av2_lossless_h_pred_left_edge(palette.bit_depth())
    }
}

fn luma_v_predictor(
    palette: &Av2LumaPalette444,
    x0: usize,
    y0: usize,
    local_x: usize,
    tile_origin_x: usize,
    tile_origin_y: usize,
) -> Av2Sample {
    // AV2 v1.0.0 Section 7.11 V_PRED with lossless DPCM uses the same
    // intra-prediction edge as normal vertical prediction before
    // avm_highbd_subtract_block_vert() differentials src-pred.
    if y0 > tile_origin_y {
        palette.y_sample(x0 + local_x, y0 - 1)
    } else if x0 > tile_origin_x {
        palette.y_sample(x0 - 1, y0)
    } else {
        av2_lossless_v_pred_above_edge(palette.bit_depth())
    }
}

fn chroma_h_predictor(
    palette: &Av2LumaPalette444,
    plane: Av2ChromaPlane,
    x0: usize,
    y0: usize,
    local_y: usize,
    tile_origin_x: usize,
    tile_origin_y: usize,
) -> Av2Sample {
    // AV2 v1.0.0 Section 7.11 intra prediction, mirrored from AVM
    // av2_build_intra_predictors_high(): H_PRED uses the left reference
    // column; if the left edge is unavailable, AVM falls back to above[0] when
    // available and to base+1 at the top-left tile/frame corner. Independent
    // 64x64 superblock tiles must not borrow the left/top predictor from the
    // previous tile even though the global frame coordinate is non-zero.
    if x0 > tile_origin_x {
        chroma_sample(palette, plane, x0 - 1, y0 + local_y)
    } else if y0 > tile_origin_y {
        chroma_sample(palette, plane, x0, y0 - 1)
    } else {
        av2_lossless_h_pred_left_edge(palette.bit_depth())
    }
}

fn chroma_v_predictor(
    palette: &Av2LumaPalette444,
    plane: Av2ChromaPlane,
    x0: usize,
    y0: usize,
    local_x: usize,
    tile_origin_x: usize,
    tile_origin_y: usize,
) -> Av2Sample {
    // AV2 v1.0.0 Section 7.11 intra prediction, implemented in AVM
    // reconintra.c: V_PRED uses the above reference row. If the above edge is
    // unavailable, AVM fills it from left[0] when left is available and from
    // base-1 at the tile top-left. Independent 64x64 tiles must not borrow
    // predictors across tile boundaries.
    if y0 > tile_origin_y {
        chroma_sample(palette, plane, x0 + local_x, y0 - 1)
    } else if x0 > tile_origin_x {
        chroma_sample(palette, plane, x0 - 1, y0)
    } else {
        av2_lossless_v_pred_above_edge(palette.bit_depth())
    }
}

fn chroma_sample(
    palette: &Av2LumaPalette444,
    plane: Av2ChromaPlane,
    x: usize,
    y: usize,
) -> Av2Sample {
    match plane {
        Av2ChromaPlane::U => palette.u_sample(x, y),
        Av2ChromaPlane::V => palette.v_sample(x, y),
    }
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
    let levels = lossless_coefficient_levels(coefficients);
    let Some(eob) = tx4x4_eob(&levels) else {
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
            writer.write_literal("tile.coeff.y.ac_sign_negative", u32::from(negative), 1);
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
    let levels = lossless_coefficient_levels(coefficients);
    let Some(bob) = TX4X4_SCAN.iter().position(|&pos| levels[pos] != 0) else {
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
    let levels = lossless_coefficient_levels(coefficients);
    let Some(eob) = tx4x4_eob(&levels) else {
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
        writer.write_literal(sign_name, u32::from(negative), 1);
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

fn lossless_coefficient_levels(coefficients: &[i32; TX4X4_SAMPLES]) -> [u32; TX4X4_SAMPLES] {
    let mut levels = [0u32; TX4X4_SAMPLES];
    for (index, &coefficient) in coefficients.iter().enumerate() {
        assert_eq!(
            coefficient % 8,
            0,
            "AV2 lossless WHT coefficient must be divisible by UNIT_QUANT_FACTOR"
        );
        levels[index] = coefficient.unsigned_abs() / 8;
    }
    levels
}

fn tx4x4_eob(levels: &[u32; TX4X4_SAMPLES]) -> Option<usize> {
    TX4X4_SCAN
        .iter()
        .position(|&pos| levels[pos] != 0)
        .and_then(|_| {
            TX4X4_SCAN
                .iter()
                .rposition(|&pos| levels[pos] != 0)
                .map(|index| index + 1)
        })
}

fn write_eob_y(writer: &mut Av2EntropyWriter, eob: usize) {
    let (eob_pt, eob_extra) = eob_pos_token(eob);
    let mut cdf = DEFAULT_EOB_MULTI16_Y_CTX0_CDF;
    writer.write_symbol("tile.coeff.y.eob_pt_tx4x4", eob_pt - 1, &mut cdf, 5, false);

    let eob_offset_bits = eob_offset_bits(eob_pt);
    if eob_offset_bits > 0 {
        let eob_shift = eob_offset_bits - 1;
        let bit = (eob_extra & (1 << eob_shift)) != 0;
        let mut extra_cdf = DEFAULT_EOB_EXTRA_CDF;
        writer.write_symbol(
            "tile.coeff.eob_extra_bit",
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
    writer.write_symbol("tile.coeff.uv.eob_pt_tx4x4", eob_pt - 1, &mut cdf, 5, false);

    let eob_offset_bits = eob_offset_bits(eob_pt);
    if eob_offset_bits > 0 {
        let eob_shift = eob_offset_bits - 1;
        let bit = (eob_extra & (1 << eob_shift)) != 0;
        let mut extra_cdf = DEFAULT_EOB_EXTRA_CDF;
        writer.write_symbol(
            "tile.coeff.eob_extra_bit",
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
            writer.write_symbol(
                "tile.coeff.uv.base_lf_eob",
                level.min(5) as usize - 1,
                &mut cdf,
                5,
                false,
            );
        } else {
            let mut cdf = DEFAULT_COEFF_BASE_EOB_UV_CDFS[coeff_ctx];
            writer.write_symbol(
                "tile.coeff.uv.base_eob",
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
        writer.write_symbol(
            "tile.coeff.uv.base_lf",
            level.min(5) as usize,
            &mut cdf,
            6,
            false,
        );
    } else {
        let mut cdf = DEFAULT_COEFF_BASE_UV_CDFS[coeff_ctx];
        writer.write_symbol(
            "tile.coeff.uv.base",
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
            writer.write_symbol(
                "tile.coeff.y.base_lf_eob",
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
            writer.write_symbol(
                "tile.coeff.y.base_eob",
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
        writer.write_symbol(
            "tile.coeff.y.base_lf",
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
        writer.write_symbol(
            "tile.coeff.y.base",
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
        writer.write_symbol(
            "tile.coeff.y.low_range_lf",
            base_range.min(3) as usize,
            &mut cdf,
            4,
            false,
        );
    } else {
        let br_ctx = luma_br_context(levels, pos);
        let mut cdf = DEFAULT_COEFF_BR_Y_CDFS[br_ctx];
        writer.write_symbol(
            "tile.coeff.y.low_range",
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
    writer.write_symbol(
        "tile.coeff.uv.low_range",
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
    writer.write_symbol_with_cdf_key(
        name,
        "tile.coeff.y.txb_skip_tx4x4",
        usize::from(skip_ctx),
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
    writer.write_symbol_with_cdf_key(
        name,
        "tile.coeff.y.txb_skip_tx4x4",
        usize::from(skip_ctx),
        0,
        &mut cdf,
        2,
        false,
    );
}

fn write_y_fsc_txb_all_zero(writer: &mut Av2EntropyWriter) {
    let mut cdf = DEFAULT_TXB_SKIP_Y_FSC_TX4X4_CTX9_CDF;
    writer.write_symbol_with_cdf_key(
        "tile.coeff.y.txb_all_zero_fsc_tx4x4_ctx9",
        "tile.coeff.y.txb_skip_fsc_tx4x4",
        9,
        1,
        &mut cdf,
        2,
        false,
    );
}

fn write_y_fsc_txb_nonzero(writer: &mut Av2EntropyWriter) {
    let mut cdf = DEFAULT_TXB_SKIP_Y_FSC_TX4X4_CTX9_CDF;
    writer.write_symbol_with_cdf_key(
        "tile.coeff.y.txb_nonzero_fsc_tx4x4_ctx9",
        "tile.coeff.y.txb_skip_fsc_tx4x4",
        9,
        0,
        &mut cdf,
        2,
        false,
    );
}

fn write_u_txb_nonzero(writer: &mut Av2EntropyWriter, skip_ctx: u8, use_fsc: bool) {
    let (name, cdf_name, mut cdf) = match skip_ctx {
        6 if use_fsc => (
            "tile.coeff.u.txb_nonzero_fsc_tx4x4_ctx6",
            "tile.coeff.u.txb_skip_fsc_tx4x4",
            DEFAULT_TXB_SKIP_U_FSC_TX4X4_CTX6_CDF,
        ),
        6 => (
            "tile.coeff.u.txb_nonzero_tx4x4_ctx6",
            "tile.coeff.u.txb_skip_tx4x4",
            DEFAULT_TXB_SKIP_U_TX4X4_CTX6_CDF,
        ),
        7 if use_fsc => (
            "tile.coeff.u.txb_nonzero_fsc_tx4x4_ctx7",
            "tile.coeff.u.txb_skip_fsc_tx4x4",
            DEFAULT_TXB_SKIP_U_FSC_TX4X4_CTX7_CDF,
        ),
        7 => (
            "tile.coeff.u.txb_nonzero_tx4x4_ctx7",
            "tile.coeff.u.txb_skip_tx4x4",
            DEFAULT_TXB_SKIP_U_TX4X4_CTX7_CDF,
        ),
        8 if use_fsc => (
            "tile.coeff.u.txb_nonzero_fsc_tx4x4_ctx8",
            "tile.coeff.u.txb_skip_fsc_tx4x4",
            DEFAULT_TXB_SKIP_U_FSC_TX4X4_CTX8_CDF,
        ),
        8 => (
            "tile.coeff.u.txb_nonzero_tx4x4_ctx8",
            "tile.coeff.u.txb_skip_tx4x4",
            DEFAULT_TXB_SKIP_U_TX4X4_CTX8_CDF,
        ),
        _ => panic!("unsupported AV2 U TXB skip context {skip_ctx}"),
    };
    writer.write_symbol_with_cdf_key(name, cdf_name, usize::from(skip_ctx), 0, &mut cdf, 2, false);
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
    writer.write_symbol_with_cdf_key(
        name,
        "tile.coeff.v.txb_skip_tx4x4",
        usize::from(skip_ctx),
        0,
        &mut cdf,
        2,
        false,
    );
}

fn write_u_txb_all_zero(writer: &mut Av2EntropyWriter, skip_ctx: u8, use_fsc: bool) {
    let (name, cdf_name, mut cdf) = match skip_ctx {
        6 if use_fsc => (
            "tile.coeff.u.txb_all_zero_fsc_tx4x4_ctx6",
            "tile.coeff.u.txb_skip_fsc_tx4x4",
            DEFAULT_TXB_SKIP_U_FSC_TX4X4_CTX6_CDF,
        ),
        6 => (
            "tile.coeff.u.txb_all_zero_tx4x4_ctx6",
            "tile.coeff.u.txb_skip_tx4x4",
            DEFAULT_TXB_SKIP_U_TX4X4_CTX6_CDF,
        ),
        7 if use_fsc => (
            "tile.coeff.u.txb_all_zero_fsc_tx4x4_ctx7",
            "tile.coeff.u.txb_skip_fsc_tx4x4",
            DEFAULT_TXB_SKIP_U_FSC_TX4X4_CTX7_CDF,
        ),
        7 => (
            "tile.coeff.u.txb_all_zero_tx4x4_ctx7",
            "tile.coeff.u.txb_skip_tx4x4",
            DEFAULT_TXB_SKIP_U_TX4X4_CTX7_CDF,
        ),
        8 if use_fsc => (
            "tile.coeff.u.txb_all_zero_fsc_tx4x4_ctx8",
            "tile.coeff.u.txb_skip_fsc_tx4x4",
            DEFAULT_TXB_SKIP_U_FSC_TX4X4_CTX8_CDF,
        ),
        8 => (
            "tile.coeff.u.txb_all_zero_tx4x4_ctx8",
            "tile.coeff.u.txb_skip_tx4x4",
            DEFAULT_TXB_SKIP_U_TX4X4_CTX8_CDF,
        ),
        _ => panic!("unsupported AV2 U TXB skip context {skip_ctx}"),
    };
    writer.write_symbol_with_cdf_key(name, cdf_name, usize::from(skip_ctx), 1, &mut cdf, 2, false);
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
    writer.write_symbol_with_cdf_key(
        name,
        "tile.coeff.v.txb_skip_tx4x4",
        usize::from(skip_ctx),
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
    let mut cdf = DEFAULT_EOB_MULTI16_UV_CTX2_CDF;
    writer.write_symbol("tile.coeff.uv.eob_pt_tx4x4_eob1", 0, &mut cdf, 5, false);
}

fn write_y_dc_level(writer: &mut Av2EntropyWriter, level: u16) {
    let mut base_cdf = DEFAULT_COEFF_BASE_LF_EOB_Y_TX4X4_CTX0_CDF;
    let base_symbol = usize::from(level.min(5) - 1);
    writer.write_symbol(
        "tile.coeff.y.dc_base_lf_eob_ctx0",
        base_symbol,
        &mut base_cdf,
        5,
        false,
    );

    if level > 4 {
        let mut low_cdf = DEFAULT_COEFF_LPS_LF_CTX0_CDF;
        let low_symbol = usize::from((level - 1 - 4).min(3));
        writer.write_symbol(
            "tile.coeff.y.dc_low_range_lf_ctx0",
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
    writer.write_symbol(
        "tile.coeff.uv.dc_base_lf_eob_ctx0",
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
    writer.write_symbol(name, usize::from(negative), &mut cdf, 2, false);
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
        writer.write_literal(name, 1, 1);
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

fn partition_context_lookup(block_size: Av2MvpBlockSize) -> (u8, u8) {
    match (block_size.width, block_size.height) {
        (8, 8) => (32 + 30, 32 + 30),
        (8, 16) => (32 + 30, 32 + 28),
        (16, 8) => (32 + 28, 32 + 30),
        (16, 16) => (32 + 28, 32 + 28),
        (16, 32) => (32 + 28, 32 + 24),
        (32, 16) => (32 + 24, 32 + 28),
        (32, 32) => (32 + 24, 32 + 24),
        (32, 64) => (32 + 24, 32 + 16),
        (64, 32) => (32 + 16, 32 + 24),
        (64, 64) => (32 + 16, 32 + 16),
        (8, 32) => (32 + 30, 32 + 24),
        (32, 8) => (32 + 24, 32 + 30),
        (16, 64) => (32 + 28, 32 + 16),
        (64, 16) => (32 + 16, 32 + 28),
        (8, 64) => (32 + 30, 32 + 16),
        (64, 8) => (32 + 16, 32 + 30),
        _ => unreachable!("unsupported AV2 MVP block size"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn av2_black_444_tile_plan_uses_8x8_leaves() {
        let plan = Av2Black444TilePlan::for_region(
            Av2TileRegion::root(Av2VideoGeometry {
                width: 64,
                height: 64,
            }),
            Av2Black444MvpProfile::current(),
            Av2ChromaFormat::Yuv444,
            false,
            false,
            None,
            None,
        );

        let partition_none_count = plan
            .decisions
            .iter()
            .filter(|decision| {
                decision.kind == Av2TileDecisionKind::Partition(Av2MvpPartition::None)
            })
            .count();
        let luma_leaf_count = plan
            .decisions
            .iter()
            .filter(|decision| {
                decision.kind
                    == Av2TileDecisionKind::IntraLumaMode {
                        mode: Av2LumaIntraMode::Dc,
                        use_dpcm_y: false,
                        dpcm_horz: false,
                        use_fsc: false,
                    }
            })
            .count();

        assert_eq!(partition_none_count, 64);
        assert_eq!(luma_leaf_count, 64);
        assert!(plan.decisions.iter().any(|decision| {
            decision.kind == Av2TileDecisionKind::BlackDcResidualCoefficients
                && decision.row == 0
                && decision.col == 0
                && decision.block_size == Av2MvpBlockSize::new(8, 8)
        }));
    }

    #[test]
    fn av2_lossless_subsampled_plan_keeps_larger_leaves() {
        let plan = Av2Black444TilePlan::for_region_with_partition_policy(
            Av2TileRegion::root(Av2VideoGeometry {
                width: 64,
                height: 64,
            }),
            Av2Black444MvpProfile::current(),
            Av2ChromaFormat::Yuv420,
            Av2PartitionPolicy::LargestLosslessLeaves,
            false,
            false,
            None,
            None,
        );

        let partition_none_count = plan
            .decisions
            .iter()
            .filter(|decision| {
                decision.kind == Av2TileDecisionKind::Partition(Av2MvpPartition::None)
            })
            .count();
        assert_eq!(partition_none_count, 1);
        assert!(plan.decisions.iter().any(|decision| {
            decision.kind == Av2TileDecisionKind::BlackDcResidualCoefficients
                && decision.row == 0
                && decision.col == 0
                && decision.block_size == Av2MvpBlockSize::new(64, 64)
        }));
    }

    #[test]
    fn av2_black_444_tile_payload_emits_root_partition_symbol() {
        let payload = av2_black_444_tile_entropy_payload(
            Av2VideoGeometry {
                width: 64,
                height: 64,
            },
            Av2Black444MvpProfile::current(),
        );

        for name in [
            "tile.partition.do_split",
            "tile.intra.use_dpcm_y",
            "tile.intra.y_mode_set_index",
            "tile.intra.y_mode_idx_dc",
            "tile.intra.use_dpcm_uv",
            "tile.coeff.y.txb_nonzero_tx4x4_ctx1",
            "tile.coeff.y.dc_base_lf_eob_ctx0",
            "tile.coeff.y.dc_sign_negative_ctx0",
            "tile.coeff.u.txb_nonzero_tx4x4_ctx6",
            "tile.coeff.u.dc_sign_negative",
            "tile.coeff.v.txb_nonzero_tx4x4_ctx9",
            "tile.coeff.v.dc_sign_negative",
        ] {
            assert!(
                payload.fields.iter().any(|field| field.name == name),
                "missing AV2 entropy field {name}"
            );
        }
        assert!(
            payload.fields.iter().any(|field| {
                field.name == "tile.intra.uv_mode_idx_dc"
                    || field.name == "tile.intra.uv_mode_idx_v"
                    || field.name == "tile.intra.uv_mode_idx_h"
            }),
            "missing AV2 entropy field for non-DPCM UV mode"
        );
        assert_eq!(
            payload
                .fields
                .iter()
                .filter(|field| field.name.starts_with("tile.coeff.y.txb_nonzero_tx4x4_ctx"))
                .count(),
            256
        );
        assert_eq!(
            payload
                .fields
                .iter()
                .filter(|field| field.name.starts_with("tile.coeff.u.txb_nonzero_tx4x4_ctx"))
                .count(),
            256
        );
        assert_eq!(
            payload
                .fields
                .iter()
                .filter(|field| field.name.starts_with("tile.coeff.v.txb_nonzero_tx4x4_ctx"))
                .count(),
            256
        );
        assert!(payload.symbol_bits > 0);
    }

    #[test]
    fn av2_black_444_tile_payload_supports_all_8_pixel_geometries() {
        for height in (8..=64).step_by(8) {
            for width in (8..=64).step_by(8) {
                let payload = av2_black_444_tile_entropy_payload(
                    Av2VideoGeometry { width, height },
                    Av2Black444MvpProfile::current(),
                );
                assert!(
                    payload
                        .fields
                        .iter()
                        .any(|field| field.name == "tile.intra.y_mode_idx_dc"),
                    "missing AV2 luma mode for {width}x{height}"
                );
                assert!(
                    payload
                        .fields
                        .iter()
                        .any(|field| field.name.starts_with("tile.coeff.y.txb_nonzero_tx4x4_ctx")),
                    "missing AV2 luma TXB residuals for {width}x{height}"
                );
            }
        }
    }

    #[test]
    fn av2_black_444_tile_payload_emits_boundary_partitions() {
        let payload = av2_black_444_tile_entropy_payload(
            Av2VideoGeometry {
                width: 16,
                height: 8,
            },
            Av2Black444MvpProfile::current(),
        );

        assert!(payload
            .fields
            .iter()
            .any(|field| field.name == "tile.partition.do_split"));
        assert!(payload.symbol_bits > 0);
    }

    #[test]
    fn av2_chroma_eob_supports_last_tx4x4_scan_position() {
        let mut levels = [0u32; TX4X4_SAMPLES];
        levels[*TX4X4_SCAN.last().expect("TX_4X4 scan is non-empty")] = 1;

        // AV2 v1.0.0 Section 5.20.7.27 coeffs(), mirrored by AVM coefficient
        // coding, permits EOB values up to the transform sample count. A
        // nonzero final scan coefficient must therefore signal eob=16, not
        // wrap to txb_skip=1 in narrower RTL state.
        assert_eq!(tx4x4_eob(&levels), Some(TX4X4_SAMPLES));
        assert_eq!(eob_pos_token(TX4X4_SAMPLES), (5, 7));
    }

    #[test]
    fn av2_lossless_422_chroma_h_predictor_uses_row_edges() {
        let geometry = Av2VideoGeometry {
            width: 16,
            height: 16,
        };
        let y_len = geometry.width * geometry.height;
        let c_width = geometry.width / 2;
        let c_len = c_width * geometry.height;
        let mut source = vec![128u8; y_len + 2 * c_len];
        let v_offset = y_len + c_len;
        for y in 0..geometry.height {
            for x in 0..c_width {
                source[v_offset + y * c_width + x] = 96 + (y as u8) * 4;
            }
        }
        let mut recon = source.clone();
        let lossless = Av2LosslessSubsampledTileState::new(
            geometry,
            Av2TileRegion::root(geometry),
            Av2ChromaFormat::Yuv422,
            SampleBitDepth::new(8).expect("valid bit depth"),
            &source,
            &mut recon,
        );

        assert_eq!(
            lossless.tx4x4_coefficients(Av2LosslessPlane::V, 4, 0),
            [0; TX4X4_SAMPLES]
        );
    }

    #[test]
    fn av2_v_txb_skip_context_matches_shared_tree_chroma_refs() {
        assert_eq!(
            v_txb_skip_context_for_chroma_format(
                0,
                0,
                false,
                Av2ChromaFormat::Yuv420,
                Av2MvpBlockSize::new(8, 8)
            ),
            chroma_txb_skip_base_context(0, 0)
        );
        assert_eq!(
            v_txb_skip_context_for_chroma_format(
                0,
                0,
                false,
                Av2ChromaFormat::Yuv420,
                Av2MvpBlockSize::new(16, 16)
            ),
            chroma_txb_skip_base_context(0, 0) + 3
        );
        assert_eq!(
            v_txb_skip_context_for_chroma_format(
                0,
                0,
                false,
                Av2ChromaFormat::Yuv422,
                Av2MvpBlockSize::new(8, 8)
            ),
            chroma_txb_skip_base_context(0, 0) + 3
        );
        assert_eq!(
            v_txb_skip_context_for_chroma_format(
                0,
                0,
                false,
                Av2ChromaFormat::Yuv444,
                Av2MvpBlockSize::new(8, 8)
            ),
            chroma_txb_skip_base_context(0, 0) + 3
        );
        assert_eq!(
            v_txb_skip_context_for_chroma_format(
                1,
                0,
                true,
                Av2ChromaFormat::Yuv422,
                Av2MvpBlockSize::new(8, 8)
            ),
            chroma_txb_skip_base_context(1, 0) + 3 + 6
        );
    }
}
