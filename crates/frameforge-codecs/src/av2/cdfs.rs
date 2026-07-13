
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
const DEFAULT_MV_JOINT_SHELL_SET_CDF: [u16; 6] = avm_cdf2(31579, -1, 0, 0);
const DEFAULT_MV_JOINT_SHELL_CLASS0_ONE_PEL_CDF: [u16; 11] =
    avm_cdf7(8680, 13723, 18208, 22686, 26722, 30020, 0, -1, 0);
const DEFAULT_MV_JOINT_SHELL_CLASS1_ONE_PEL_CDF: [u16; 11] =
    avm_cdf7(19978, 30160, 32564, 32732, 32736, 32740, 0, 0, -1);
const DEFAULT_MV_SHELL_OFFSET_LOW_CLASS_CDFS: [[u16; 6]; 2] =
    [avm_cdf2(14587, -1, -2, -1), avm_cdf2(20966, 1, 0, 0)];
const DEFAULT_MV_SHELL_OFFSET_CLASS2_CDF: [u16; 6] = avm_cdf2(13189, 0, 0, 0);
const DEFAULT_MV_SHELL_OFFSET_OTHER_CLASS_CDFS: [[u16; 6]; 16] = [
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
const DEFAULT_MV_COL_MV_GREATER_FLAGS_CDFS: [[u16; 6]; 2] =
    [avm_cdf2(5663, -1, 0, 0), avm_cdf2(4856, 1, 1, 0)];
const DEFAULT_MV_COL_MV_INDEX_CDFS: [[u16; 6]; 4] = [
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
const DEFAULT_INTRA_INTER_CDFS: [[u16; 6]; 4] = [
    avm_cdf2(1522, 0, 0, -1),
    avm_cdf2(14381, 0, 0, 0),
    avm_cdf2(10455, -1, 0, 0),
    avm_cdf2(27796, 0, 0, 0),
];
const DEFAULT_SINGLE_REF_CDFS: [[[u16; 6]; 6]; 3] = [
    [
        avm_cdf2(26469, 0, 0, 0),
        avm_cdf2(28870, -1, -1, 0),
        avm_cdf2(29662, 0, 0, -1),
        avm_cdf2(29867, 0, -1, -1),
        avm_cdf2(29772, 0, -1, -1),
        avm_cdf2(29776, -1, 0, -1),
    ],
    [
        avm_cdf2(13631, 0, -1, -1),
        avm_cdf2(18185, -1, -2, -2),
        avm_cdf2(19992, -1, -1, -2),
        avm_cdf2(18462, -2, -2, -2),
        avm_cdf2(17451, -1, -2, -2),
        avm_cdf2(11578, -2, -2, -2),
    ],
    [
        avm_cdf2(2599, 0, 0, 0),
        avm_cdf2(5203, -1, -1, -1),
        avm_cdf2(5185, -1, -1, -1),
        avm_cdf2(3671, -1, -1, -1),
        avm_cdf2(3954, 0, -1, -1),
        avm_cdf2(1633, 0, -1, 0),
    ],
];
const DEFAULT_INTER_SINGLE_MODE_CDFS: [[u16; 7]; 5] = [
    avm_cdf3(10043, 11100, 0, -1, -1),
    avm_cdf3(21561, 21758, 0, 0, -1),
    avm_cdf3(25411, 25714, 0, 0, 0),
    avm_cdf3(14117, 14341, 0, 0, 0),
    avm_cdf3(18288, 18577, 0, 0, 0),
];
const DEFAULT_DRL_CDFS: [[[u16; 6]; 5]; 3] = [
    [
        avm_cdf2(15721, 1, 1, 0),
        avm_cdf2(21115, 0, 0, 0),
        avm_cdf2(19567, 0, 0, -1),
        avm_cdf2(17602, 1, 1, 1),
        avm_cdf2(13319, 1, 1, 1),
    ],
    [
        avm_cdf2(18692, 1, 1, 1),
        avm_cdf2(19343, 1, 1, 0),
        avm_cdf2(18207, 1, 1, 0),
        avm_cdf2(17908, 1, 1, 1),
        avm_cdf2(18304, 1, 1, 1),
    ],
    [
        avm_cdf2(22157, 1, 1, 0),
        avm_cdf2(23233, 1, 1, 0),
        avm_cdf2(22782, 1, 1, 0),
        avm_cdf2(22353, 1, 1, 1),
        avm_cdf2(22457, 1, 1, 1),
    ],
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
const DEFAULT_FSC_MODE_CDFS: [[[u16; 6]; 6]; 4] = [
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
    [
        avm_cdf2(16384, 0, 0, 0),
        avm_cdf2(16384, 0, 0, 0),
        avm_cdf2(32016, 0, 1, 0),
        avm_cdf2(32403, 1, 1, 1),
        avm_cdf2(32583, 0, 1, 0),
        avm_cdf2(32683, 1, 0, -1),
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
const AV2_LUMA_FIRST_MODE_COUNT: usize = 13;
const AV2_LUMA_SECOND_MODE_COUNT: usize = 16;
const AV2_LUMA_MODE_SET_COUNT: usize = 4;
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
const DEFAULT_Y_MODE_IDX_OFFSET_CDFS: [[u16; 10]; 3] = [
    avm_cdf6(12743, 18172, 20194, 23648, 26419, 0, -1, -1),
    avm_cdf6(8976, 16084, 20827, 24595, 28496, 1, 0, 0),
    avm_cdf6(8784, 14556, 19710, 24903, 28724, 1, 0, 0),
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
