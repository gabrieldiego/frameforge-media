use super::super::{
    VvcCabacContext, VvcCabacContexts, VvcCabacEncoder, VvcLastSigCoeffPrefixCtxInput,
};
use super::{VvcResidualComponent, VVC_CHROMA_AC_COEFFS_PER_TU, VVC_LUMA_AC_COEFFS_PER_TU};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::vvc) struct VvcResidualCabacOptions {
    pub(in crate::vvc) transform_skip_enabled: bool,
    pub(in crate::vvc) explicit_mts_intra_enabled: bool,
    pub(in crate::vvc) dependent_quantization_enabled: bool,
    pub(in crate::vvc) sign_data_hiding_enabled: bool,
    pub(in crate::vvc) lfnst_enabled: bool,
    pub(in crate::vvc) sbt_enabled: bool,
}

pub(in crate::vvc) struct VvcResidualCabacEncoder<'a> {
    contexts: &'a mut VvcCabacContexts,
    options: VvcResidualCabacOptions,
}

impl<'a> VvcResidualCabacEncoder<'a> {
    pub(in crate::vvc) fn new(
        contexts: &'a mut VvcCabacContexts,
        options: VvcResidualCabacOptions,
    ) -> Self {
        Self { contexts, options }
    }

    #[cfg(test)]
    fn emit_residual_symbol(
        &mut self,
        cabac: &mut VvcCabacEncoder,
        state: &VvcResidualPass1State,
        symbol: VvcResidualCabacSymbol,
    ) {
        match symbol {
            VvcResidualCabacSymbol::LastSigCoeffXPrefix { bin_idx, bin } => {
                self.emit_last_sig_coeff_prefix_bin(
                    cabac,
                    state.config.component,
                    true,
                    state.config.log2_zo_tb_width,
                    bin_idx,
                    bin,
                );
            }
            VvcResidualCabacSymbol::LastSigCoeffXSuffix { bits, count } => {
                cabac.encode_bins_ep(bits, u32::from(count));
            }
            VvcResidualCabacSymbol::LastSigCoeffYPrefix { bin_idx, bin } => {
                self.emit_last_sig_coeff_prefix_bin(
                    cabac,
                    state.config.component,
                    false,
                    state.config.log2_zo_tb_height,
                    bin_idx,
                    bin,
                );
            }
            VvcResidualCabacSymbol::LastSigCoeffYSuffix { bits, count } => {
                cabac.encode_bins_ep(bits, u32::from(count));
            }
            VvcResidualCabacSymbol::SbCodedFlag { x_s, y_s, coded } => {
                self.emit_sb_coded_flag(cabac, state, x_s, y_s, coded);
            }
            VvcResidualCabacSymbol::SigCoeffFlag { x, y, significant } => {
                self.emit_sig_coeff_flag(cabac, state, x, y, significant);
            }
            VvcResidualCabacSymbol::ParLevelFlag { x, y, par_level } => {
                self.emit_par_level_flag(cabac, state, x, y, par_level);
            }
            VvcResidualCabacSymbol::AbsLevelGtxFlag {
                x,
                y,
                gtx_idx,
                greater_than,
            } => {
                self.emit_abs_level_gtx_flag(cabac, state, x, y, gtx_idx, greater_than);
            }
            VvcResidualCabacSymbol::AbsRemainder {
                value, rice_param, ..
            } => {
                cabac.encode_rem_abs_ep(value, u32::from(rice_param));
            }
            VvcResidualCabacSymbol::BypassAbsLevel {
                value, rice_param, ..
            } => {
                cabac.encode_rem_abs_ep(value, u32::from(rice_param));
            }
            VvcResidualCabacSymbol::CoeffSignPattern { bits, count } => {
                cabac.encode_bins_ep(bits, u32::from(count));
            }
        }
    }

    fn emit_last_sig_coeff_prefix_bin(
        &mut self,
        cabac: &mut VvcCabacEncoder,
        component: VvcResidualComponent,
        x_prefix: bool,
        log2_tb_size: u8,
        bin_idx: u8,
        bin: bool,
    ) {
        let ctx_inc = VvcLastSigCoeffPrefixCtxInput {
            is_luma: component == VvcResidualComponent::Luma,
            log2_tb_size,
            bin_idx,
        }
        .ctx_inc();
        let ctx = if x_prefix {
            VvcCabacContext::LastSigCoeffXPrefix(ctx_inc)
        } else {
            VvcCabacContext::LastSigCoeffYPrefix(ctx_inc)
        };
        self.contexts.encode(cabac, ctx, bin);
    }

    #[cfg(test)]
    pub(in crate::vvc) fn emit_last_sig_coeff_prefixes_4x4(
        &mut self,
        cabac: &mut VvcCabacEncoder,
        component: VvcResidualComponent,
        last_x: u8,
        last_y: u8,
    ) {
        debug_assert!(last_x < 4);
        debug_assert!(last_y < 4);
        Self::append_last_sig_coeff_prefix_4x4(self, cabac, component, true, last_x);
        Self::append_last_sig_coeff_prefix_4x4(self, cabac, component, false, last_y);
    }

    #[cfg(test)]
    fn append_last_sig_coeff_prefix_4x4(
        encoder: &mut Self,
        cabac: &mut VvcCabacEncoder,
        component: VvcResidualComponent,
        x_prefix: bool,
        prefix: u8,
    ) {
        for bin_idx in 0..prefix {
            encoder.emit_last_sig_coeff_prefix_bin(cabac, component, x_prefix, 2, bin_idx, true);
        }
        if prefix < 3 {
            encoder.emit_last_sig_coeff_prefix_bin(cabac, component, x_prefix, 2, prefix, false);
        }
    }

    pub(in crate::vvc) fn emit_sb_coded_flag(
        &mut self,
        cabac: &mut VvcCabacEncoder,
        state: &VvcResidualPass1State,
        x_s: u8,
        y_s: u8,
        coded: bool,
    ) {
        let ctx_inc = state.sb_coded_flag_ctx_inc(x_s, y_s);
        self.contexts
            .encode(cabac, VvcCabacContext::SbCodedFlag(ctx_inc), coded);
    }

    pub(in crate::vvc) fn emit_sig_coeff_flag(
        &mut self,
        cabac: &mut VvcCabacEncoder,
        state: &VvcResidualPass1State,
        x: u8,
        y: u8,
        significant: bool,
    ) {
        let ctx_inc = state.sig_coeff_flag_ctx_inc(x, y);
        self.contexts
            .encode(cabac, VvcCabacContext::SigCoeffFlag(ctx_inc), significant);
    }

    pub(in crate::vvc) fn emit_par_level_flag(
        &mut self,
        cabac: &mut VvcCabacEncoder,
        state: &VvcResidualPass1State,
        x: u8,
        y: u8,
        par_level: bool,
    ) {
        let ctx_inc = state.par_level_flag_ctx_inc(x, y);
        self.contexts
            .encode(cabac, VvcCabacContext::ParLevelFlag(ctx_inc), par_level);
    }

    pub(in crate::vvc) fn emit_abs_level_gtx_flag(
        &mut self,
        cabac: &mut VvcCabacEncoder,
        state: &VvcResidualPass1State,
        x: u8,
        y: u8,
        gtx_idx: u8,
        greater_than: bool,
    ) {
        let ctx_inc = state.abs_level_gtx_flag_ctx_inc(x, y, gtx_idx);
        debug_assert!(
            ctx_inc < 72,
            "cached abs_level_gtx_flag table currently covers ctxInc 0..71"
        );
        self.contexts.encode(
            cabac,
            VvcCabacContext::AbsLevelGtxFlag(ctx_inc),
            greater_than,
        );
    }

    pub(in crate::vvc) fn emit_default_tool_control_hooks(
        &mut self,
        cabac: &mut VvcCabacEncoder,
        state: &VvcResidualPass1State,
    ) {
        self.emit_transform_skip_flag(
            cabac,
            state.config.component,
            state.config.transform_skip,
            state.config.bdpcm,
        );
        self.observe_future_chroma_defaults();
        self.observe_current_disabled_tool_defaults();
        #[cfg(test)]
        let _default_sb_symbol = VvcResidualCabacSymbol::SbCodedFlag {
            x_s: 0,
            y_s: 0,
            coded: false,
        };
        let _default_sb_ctx = state.sb_coded_flag_ctx_inc(0, 0);
    }

    fn emit_transform_skip_flag(
        &mut self,
        cabac: &mut VvcCabacEncoder,
        component: VvcResidualComponent,
        transform_skip: bool,
        bdpcm: bool,
    ) {
        if bdpcm {
            // H.266 7.4.12.11: intra_bdpcm_*_flag=1 infers
            // transform_skip_flag to 1 instead of coding it.
            debug_assert!(transform_skip);
            return;
        }
        if !self.options.transform_skip_enabled {
            debug_assert!(!transform_skip);
            return;
        }

        self.contexts.encode(
            cabac,
            VvcCabacContext::TransformSkipFlag(component.transform_skip_ctx_inc()),
            transform_skip,
        );
    }

    fn observe_future_chroma_defaults(&self) {
        let _default_chroma_transform_skip_contexts = (
            VvcResidualComponent::ChromaCb.transform_skip_ctx_inc(),
            VvcResidualComponent::ChromaCr.transform_skip_ctx_inc(),
        );
    }

    fn observe_current_disabled_tool_defaults(&self) {
        let _disabled_tool_defaults = (
            self.options.dependent_quantization_enabled,
            self.options.sign_data_hiding_enabled,
            self.options.lfnst_enabled,
            self.options.sbt_enabled,
        );
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::vvc) struct VvcResidualCtxConfig {
    pub(in crate::vvc) component: VvcResidualComponent,
    pub(in crate::vvc) log2_zo_tb_width: u8,
    pub(in crate::vvc) log2_zo_tb_height: u8,
    pub(in crate::vvc) q_state: u8,
    pub(in crate::vvc) transform_skip: bool,
    pub(in crate::vvc) ts_residual_coding_disabled: bool,
    pub(in crate::vvc) bdpcm: bool,
    pub(in crate::vvc) mts_index: u8,
    pub(in crate::vvc) last_significant_x: u8,
    pub(in crate::vvc) last_significant_y: u8,
}

impl VvcResidualCtxConfig {
    #[cfg(test)]
    pub(in crate::vvc) fn luma_4x4_subset(last_significant_x: u8, last_significant_y: u8) -> Self {
        Self::luma_subset(2, 2, last_significant_x, last_significant_y)
    }

    #[cfg(test)]
    pub(in crate::vvc) fn luma_subset(
        log2_zo_tb_width: u8,
        log2_zo_tb_height: u8,
        last_significant_x: u8,
        last_significant_y: u8,
    ) -> Self {
        Self::subset(
            VvcResidualComponent::Luma,
            log2_zo_tb_width,
            log2_zo_tb_height,
            last_significant_x,
            last_significant_y,
        )
    }

    pub(in crate::vvc) fn subset(
        component: VvcResidualComponent,
        log2_zo_tb_width: u8,
        log2_zo_tb_height: u8,
        last_significant_x: u8,
        last_significant_y: u8,
    ) -> Self {
        debug_assert!((2..=6).contains(&log2_zo_tb_width));
        debug_assert!((2..=6).contains(&log2_zo_tb_height));
        Self {
            component,
            log2_zo_tb_width,
            log2_zo_tb_height,
            q_state: 0,
            transform_skip: false,
            ts_residual_coding_disabled: true,
            bdpcm: false,
            mts_index: 0,
            last_significant_x,
            last_significant_y,
        }
    }

    fn is_luma(self) -> bool {
        self.component == VvcResidualComponent::Luma
    }

    fn transform_skip_residual_enabled(self) -> bool {
        self.transform_skip && !self.ts_residual_coding_disabled
    }

    fn tb_width(self) -> usize {
        1usize << self.log2_zo_tb_width
    }

    fn tb_height(self) -> usize {
        1usize << self.log2_zo_tb_height
    }

    fn log2_sb_width(self) -> u8 {
        let mut log2_sb_width = if self.log2_zo_tb_width.min(self.log2_zo_tb_height) < 2 {
            1
        } else {
            2
        };
        if self.log2_zo_tb_width < 2 && self.is_luma() {
            log2_sb_width = self.log2_zo_tb_width;
        } else if self.log2_zo_tb_height < 2 && self.is_luma() {
            log2_sb_width = 4 - self.log2_zo_tb_height;
        }
        log2_sb_width
    }

    fn log2_sb_height(self) -> u8 {
        let mut log2_sb_height = if self.log2_zo_tb_width.min(self.log2_zo_tb_height) < 2 {
            1
        } else {
            2
        };
        if self.log2_zo_tb_width < 2 && self.is_luma() {
            log2_sb_height = 4 - self.log2_zo_tb_width;
        } else if self.log2_zo_tb_height < 2 && self.is_luma() {
            log2_sb_height = self.log2_zo_tb_height;
        }
        log2_sb_height
    }

    fn subblocks_wide(self) -> usize {
        1usize << (self.log2_zo_tb_width - self.log2_sb_width())
    }

    fn subblocks_high(self) -> usize {
        1usize << (self.log2_zo_tb_height - self.log2_sb_height())
    }

    fn subblock_count(self) -> usize {
        self.subblocks_wide() * self.subblocks_high()
    }

    fn subblock_index(self, x_s: u8, y_s: u8) -> usize {
        assert!((x_s as usize) < self.subblocks_wide());
        assert!((y_s as usize) < self.subblocks_high());
        y_s as usize * self.subblocks_wide() + x_s as usize
    }
}

// Current production residual TUs are at most 8x8 luma and 4x4 chroma. Raise
// this with the transform-size selector when larger emitted TUs are enabled.
const VVC_RESIDUAL_CONTEXT_COEFFS: usize = 64;
const VVC_MAX_RESIDUAL_SUBBLOCKS: usize = 256;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::vvc) struct VvcResidualPass1State {
    pub(in crate::vvc) config: VvcResidualCtxConfig,
    pub(in crate::vvc) sig_coeff: [bool; VVC_RESIDUAL_CONTEXT_COEFFS],
    pub(in crate::vvc) abs_level_pass1: [u8; VVC_RESIDUAL_CONTEXT_COEFFS],
    rice_abs_level: [u16; VVC_RESIDUAL_CONTEXT_COEFFS],
    pub(in crate::vvc) sb_coded: [bool; VVC_MAX_RESIDUAL_SUBBLOCKS],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::vvc) struct VvcResidualLocalStats {
    pub(in crate::vvc) loc_num_sig: u8,
    pub(in crate::vvc) loc_sum_abs_pass1: u8,
}

#[cfg(test)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::vvc) enum VvcResidualCabacSymbol {
    LastSigCoeffXPrefix {
        bin_idx: u8,
        bin: bool,
    },
    LastSigCoeffXSuffix {
        bits: u32,
        count: u8,
    },
    LastSigCoeffYPrefix {
        bin_idx: u8,
        bin: bool,
    },
    LastSigCoeffYSuffix {
        bits: u32,
        count: u8,
    },
    SbCodedFlag {
        x_s: u8,
        y_s: u8,
        coded: bool,
    },
    SigCoeffFlag {
        x: u8,
        y: u8,
        significant: bool,
    },
    ParLevelFlag {
        x: u8,
        y: u8,
        par_level: bool,
    },
    AbsLevelGtxFlag {
        x: u8,
        y: u8,
        gtx_idx: u8,
        greater_than: bool,
    },
    AbsRemainder {
        x: u8,
        y: u8,
        value: u32,
        rice_param: u8,
    },
    BypassAbsLevel {
        x: u8,
        y: u8,
        value: u32,
        rice_param: u8,
    },
    CoeffSignPattern {
        bits: u32,
        count: u8,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum VvcDelayedResidualCabacSymbol {
    AbsRemainder { value: u32, rice_param: u8 },
    BypassAbsLevel { value: u32, rice_param: u8 },
}

#[cfg(test)]
trait VvcResidualSymbolSink {
    fn last_sig_coeff_x_prefix(&mut self, bin_idx: u8, bin: bool);
    fn last_sig_coeff_x_suffix(&mut self, bits: u32, count: u8);
    fn last_sig_coeff_y_prefix(&mut self, bin_idx: u8, bin: bool);
    fn last_sig_coeff_y_suffix(&mut self, bits: u32, count: u8);
    fn sb_coded_flag(&mut self, x_s: u8, y_s: u8, coded: bool);
    fn sig_coeff_flag(&mut self, x: u8, y: u8, significant: bool);
    fn par_level_flag(&mut self, x: u8, y: u8, par_level: bool);
    fn abs_level_gtx_flag(&mut self, x: u8, y: u8, gtx_idx: u8, greater_than: bool);
    fn abs_remainder(&mut self, x: u8, y: u8, value: u32, rice_param: u8);
    fn bypass_abs_level(&mut self, x: u8, y: u8, value: u32, rice_param: u8);
    fn coeff_sign_pattern(&mut self, bits: u32, count: u8);
}

#[cfg(test)]
impl VvcResidualSymbolSink for Vec<VvcResidualCabacSymbol> {
    fn last_sig_coeff_x_prefix(&mut self, bin_idx: u8, bin: bool) {
        self.push(VvcResidualCabacSymbol::LastSigCoeffXPrefix { bin_idx, bin });
    }

    fn last_sig_coeff_x_suffix(&mut self, bits: u32, count: u8) {
        self.push(VvcResidualCabacSymbol::LastSigCoeffXSuffix { bits, count });
    }

    fn last_sig_coeff_y_prefix(&mut self, bin_idx: u8, bin: bool) {
        self.push(VvcResidualCabacSymbol::LastSigCoeffYPrefix { bin_idx, bin });
    }

    fn last_sig_coeff_y_suffix(&mut self, bits: u32, count: u8) {
        self.push(VvcResidualCabacSymbol::LastSigCoeffYSuffix { bits, count });
    }

    fn sb_coded_flag(&mut self, x_s: u8, y_s: u8, coded: bool) {
        self.push(VvcResidualCabacSymbol::SbCodedFlag { x_s, y_s, coded });
    }

    fn sig_coeff_flag(&mut self, x: u8, y: u8, significant: bool) {
        self.push(VvcResidualCabacSymbol::SigCoeffFlag { x, y, significant });
    }

    fn par_level_flag(&mut self, x: u8, y: u8, par_level: bool) {
        self.push(VvcResidualCabacSymbol::ParLevelFlag { x, y, par_level });
    }

    fn abs_level_gtx_flag(&mut self, x: u8, y: u8, gtx_idx: u8, greater_than: bool) {
        self.push(VvcResidualCabacSymbol::AbsLevelGtxFlag {
            x,
            y,
            gtx_idx,
            greater_than,
        });
    }

    fn abs_remainder(&mut self, x: u8, y: u8, value: u32, rice_param: u8) {
        self.push(VvcResidualCabacSymbol::AbsRemainder {
            x,
            y,
            value,
            rice_param,
        });
    }

    fn bypass_abs_level(&mut self, x: u8, y: u8, value: u32, rice_param: u8) {
        self.push(VvcResidualCabacSymbol::BypassAbsLevel {
            x,
            y,
            value,
            rice_param,
        });
    }

    fn coeff_sign_pattern(&mut self, bits: u32, count: u8) {
        self.push(VvcResidualCabacSymbol::CoeffSignPattern { bits, count });
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::vvc) struct VvcResidualCabacSymbolStream {
    #[cfg(test)]
    pub(in crate::vvc) config: VvcResidualCtxConfig,
    #[cfg(test)]
    pub(in crate::vvc) pass1_state: VvcResidualPass1State,
    #[cfg(test)]
    pub(in crate::vvc) symbols: Vec<VvcResidualCabacSymbol>,
}

struct VvcResidualCoefficientPlan {
    #[cfg(test)]
    config: VvcResidualCtxConfig,
    pass1_state: VvcResidualPass1State,
    scan: VvcScanPlan,
    last_scan_pos: usize,
}

trait VvcCoeffAccessor {
    fn width(&self) -> usize;
    fn height(&self) -> usize;
    fn level_at(&self, x: usize, y: usize) -> i16;
}

struct VvcRasterCoeffAccessor<'a> {
    coeff_levels: &'a [i16],
    width: usize,
    height: usize,
}

impl<'a> VvcRasterCoeffAccessor<'a> {
    fn new(coeff_levels: &'a [i16], width: usize, height: usize) -> Self {
        assert_eq!(coeff_levels.len(), width * height);
        Self {
            coeff_levels,
            width,
            height,
        }
    }
}

impl VvcCoeffAccessor for VvcRasterCoeffAccessor<'_> {
    fn width(&self) -> usize {
        self.width
    }

    fn height(&self) -> usize {
        self.height
    }

    fn level_at(&self, x: usize, y: usize) -> i16 {
        self.coeff_levels[y * self.width + x]
    }
}

struct VvcStoredCoeffAccessor<'a, const AC_COEFFS: usize> {
    width: usize,
    height: usize,
    dc_level: i16,
    ac_levels: &'a [i16; AC_COEFFS],
    coeff_stride: usize,
}

impl<'a, const AC_COEFFS: usize> VvcStoredCoeffAccessor<'a, AC_COEFFS> {
    fn new(
        width: usize,
        height: usize,
        dc_level: i16,
        ac_levels: &'a [i16; AC_COEFFS],
        has_ac: bool,
        coeff_stride: usize,
    ) -> Self {
        debug_assert_eq!(has_ac, ac_levels.iter().any(|level| *level != 0));
        debug_assert!(coeff_stride > 0);
        Self {
            width,
            height,
            dc_level,
            ac_levels,
            coeff_stride,
        }
    }
}

impl<const AC_COEFFS: usize> VvcCoeffAccessor for VvcStoredCoeffAccessor<'_, AC_COEFFS> {
    fn width(&self) -> usize {
        self.width
    }

    fn height(&self) -> usize {
        self.height
    }

    fn level_at(&self, x: usize, y: usize) -> i16 {
        if x >= self.width || y >= self.height {
            return 0;
        }
        if x == 0 && y == 0 {
            self.dc_level
        } else {
            if x >= self.coeff_stride {
                return 0;
            }
            let compact_idx = y * self.coeff_stride + x;
            if compact_idx == 0 || compact_idx > self.ac_levels.len() {
                return 0;
            }
            self.ac_levels[compact_idx - 1]
        }
    }
}

impl VvcResidualPass1State {
    pub(in crate::vvc) fn new(config: VvcResidualCtxConfig) -> Self {
        debug_assert!(config.subblock_count() <= VVC_MAX_RESIDUAL_SUBBLOCKS);
        Self {
            config,
            sig_coeff: [false; VVC_RESIDUAL_CONTEXT_COEFFS],
            abs_level_pass1: [0; VVC_RESIDUAL_CONTEXT_COEFFS],
            rice_abs_level: [0; VVC_RESIDUAL_CONTEXT_COEFFS],
            sb_coded: [false; VVC_MAX_RESIDUAL_SUBBLOCKS],
        }
    }

    pub(in crate::vvc) fn set_pass1_coeff(
        &mut self,
        x: u8,
        y: u8,
        abs_level: u16,
        _negative: bool,
    ) {
        let index = self
            .coefficient_index(x, y)
            .expect("VVC residual pass-1 coefficient is outside the tracked transform block");
        self.sig_coeff[index] = abs_level != 0;
        // VTM CoeffCodingContext::sigCtxIdAbs uses
        // min(4 + (absLevel & 1), absLevel) for the local template sum and
        // then reuses sumAbs - numPos for the par/gt context offset.
        // Keep that exact template magnitude here instead of an artificial
        // pass-1 clip so AC contexts track H.266 9.3.4.2.8/9.
        let pass1_level = template_abs_sum_level(abs_level);
        self.abs_level_pass1[index] = pass1_level;
        self.rice_abs_level[index] = u16::from(pass1_level);
    }

    fn set_rice_abs_level(&mut self, x: u8, y: u8, abs_level: u16) {
        let index = self
            .coefficient_index(x, y)
            .expect("VVC residual rice coefficient is outside the tracked transform block");
        self.rice_abs_level[index] = abs_level;
    }

    pub(in crate::vvc) fn set_sb_coded(&mut self, x_s: u8, y_s: u8, coded: bool) {
        let index = self.config.subblock_index(x_s, y_s);
        debug_assert!(index < VVC_MAX_RESIDUAL_SUBBLOCKS);
        if index < VVC_MAX_RESIDUAL_SUBBLOCKS {
            self.sb_coded[index] = coded;
        }
    }

    pub(in crate::vvc) fn sb_coded_flag_ctx_inc(&self, x_s: u8, y_s: u8) -> u8 {
        // VVC 9.3.4.2.6. Keep transform-skip and regular residual paths
        // separate because future screen-content tools will use both.
        let mut csbf_ctx = 0;
        if self.config.transform_skip_residual_enabled() {
            if x_s > 0 && self.sb_coded_at(x_s - 1, y_s) {
                csbf_ctx += 1;
            }
            if y_s > 0 && self.sb_coded_at(x_s, y_s - 1) {
                csbf_ctx += 1;
            }
            4 + csbf_ctx
        } else {
            if (x_s as usize) + 1 < self.config.subblocks_wide() && self.sb_coded_at(x_s + 1, y_s) {
                csbf_ctx += 1;
            }
            if (y_s as usize) + 1 < self.config.subblocks_high() && self.sb_coded_at(x_s, y_s + 1) {
                csbf_ctx += 1;
            }
            if self.config.is_luma() {
                csbf_ctx.min(1)
            } else {
                2 + csbf_ctx.min(1)
            }
        }
    }

    pub(in crate::vvc) fn sig_coeff_flag_ctx_inc(&self, x: u8, y: u8) -> u8 {
        // VVC 9.3.4.2.8. QState is kept explicit even though the current
        // subset initializes it to zero for the simple residual path.
        let stats = self.local_stats(x, y);
        if self.config.transform_skip_residual_enabled() {
            60 + stats.loc_num_sig
        } else {
            let d = x + y;
            let sum_bucket = ((stats.loc_sum_abs_pass1 + 1) >> 1).min(3);
            let q_bucket = 12 * self.config.q_state.saturating_sub(1);
            if self.config.is_luma() {
                q_bucket
                    + sum_bucket
                    + if d < 2 {
                        8
                    } else if d < 5 {
                        4
                    } else {
                        0
                    }
            } else {
                36 + (8 * self.config.q_state.saturating_sub(1))
                    + sum_bucket
                    + if d < 2 { 4 } else { 0 }
            }
        }
    }

    pub(in crate::vvc) fn par_level_flag_ctx_inc(&self, x: u8, y: u8) -> u8 {
        self.par_or_abs_level_ctx_inc(x, y, false, 0)
    }

    pub(in crate::vvc) fn abs_level_gtx_flag_ctx_inc(&self, x: u8, y: u8, gtx_idx: u8) -> u8 {
        self.par_or_abs_level_ctx_inc(x, y, true, gtx_idx)
    }

    pub(in crate::vvc) fn local_stats(&self, x: u8, y: u8) -> VvcResidualLocalStats {
        // VVC 9.3.4.2.7. The regular transform path looks forward in raster
        // coordinates because coefficients are scanned in reverse order.
        let mut loc_num_sig = 0;
        let mut loc_sum_abs_pass1 = 0;
        if self.config.transform_skip_residual_enabled() {
            if x > 0 {
                self.accumulate_local(x - 1, y, &mut loc_num_sig, &mut loc_sum_abs_pass1);
            }
            if y > 0 {
                self.accumulate_local(x, y - 1, &mut loc_num_sig, &mut loc_sum_abs_pass1);
            }
        } else {
            if (x as usize) + 1 < self.config.tb_width() {
                self.accumulate_local(x + 1, y, &mut loc_num_sig, &mut loc_sum_abs_pass1);
                if (x as usize) + 2 < self.config.tb_width() {
                    self.accumulate_local(x + 2, y, &mut loc_num_sig, &mut loc_sum_abs_pass1);
                }
                if (y as usize) + 1 < self.config.tb_height() {
                    self.accumulate_local(x + 1, y + 1, &mut loc_num_sig, &mut loc_sum_abs_pass1);
                }
            }
            if (y as usize) + 1 < self.config.tb_height() {
                self.accumulate_local(x, y + 1, &mut loc_num_sig, &mut loc_sum_abs_pass1);
                if (y as usize) + 2 < self.config.tb_height() {
                    self.accumulate_local(x, y + 2, &mut loc_num_sig, &mut loc_sum_abs_pass1);
                }
            }
        }
        VvcResidualLocalStats {
            loc_num_sig,
            loc_sum_abs_pass1,
        }
    }

    fn par_or_abs_level_ctx_inc(&self, x: u8, y: u8, abs_level_gtx: bool, gtx_idx: u8) -> u8 {
        // VVC 9.3.4.2.9. Only abs_level_gtx_flag[n][0] is wired to the cached
        // context table today; gtx_idx > 0 is labelled here for the upcoming
        // larger residual-level implementation.
        if self.config.transform_skip_residual_enabled() {
            if !abs_level_gtx {
                return 32;
            }
            if gtx_idx > 0 {
                return 67 + gtx_idx;
            }
            if self.config.bdpcm {
                return 67;
            }
            return 64
                + if x > 0 && self.sig_coeff_at(x - 1, y) {
                    1
                } else {
                    0
                }
                + if y > 0 && self.sig_coeff_at(x, y - 1) {
                    1
                } else {
                    0
                };
        }

        let base = if x == self.config.last_significant_x && y == self.config.last_significant_y {
            if self.config.is_luma() {
                0
            } else {
                21
            }
        } else {
            let stats = self.local_stats(x, y);
            let ctx_offset = stats
                .loc_sum_abs_pass1
                .saturating_sub(stats.loc_num_sig)
                .min(4);
            let d = x + y;
            if self.config.is_luma() {
                1 + ctx_offset
                    + if d == 0 {
                        15
                    } else if d < 3 {
                        10
                    } else if d < 10 {
                        5
                    } else {
                        0
                    }
            } else {
                22 + ctx_offset + if d == 0 { 5 } else { 0 }
            }
        };
        base + if abs_level_gtx && gtx_idx == 1 { 32 } else { 0 }
    }

    fn accumulate_local(&self, x: u8, y: u8, loc_num_sig: &mut u8, loc_sum_abs_pass1: &mut u8) {
        if self.sig_coeff_at(x, y) {
            *loc_num_sig += 1;
        }
        *loc_sum_abs_pass1 = loc_sum_abs_pass1.saturating_add(self.abs_level_pass1_at(x, y));
    }

    pub(in crate::vvc) fn sig_coeff_at(&self, x: u8, y: u8) -> bool {
        self.coefficient_index(x, y)
            .is_some_and(|index| self.sig_coeff[index])
    }

    pub(in crate::vvc) fn abs_level_pass1_at(&self, x: u8, y: u8) -> u8 {
        self.coefficient_index(x, y)
            .map_or(0, |index| self.abs_level_pass1[index])
    }

    fn rice_abs_level_at(&self, x: u8, y: u8) -> u16 {
        self.coefficient_index(x, y)
            .map_or(0, |index| self.rice_abs_level[index])
    }

    pub(in crate::vvc) fn sb_coded_at(&self, x_s: u8, y_s: u8) -> bool {
        let index = self.config.subblock_index(x_s, y_s);
        index < VVC_MAX_RESIDUAL_SUBBLOCKS && self.sb_coded[index]
    }

    fn coefficient_index(&self, x: u8, y: u8) -> Option<usize> {
        let x = usize::from(x);
        let y = usize::from(y);
        if x >= self.config.tb_width() || y >= self.config.tb_height() {
            return None;
        }
        let index = y * self.config.tb_width() + x;
        (index < VVC_RESIDUAL_CONTEXT_COEFFS).then_some(index)
    }
}

impl VvcResidualCabacSymbolStream {
    #[cfg(test)]
    pub(in crate::vvc) fn luma_coefficients(
        log2_tb_width: u8,
        log2_tb_height: u8,
        coeff_levels: &[i16],
    ) -> Self {
        Self::coefficients(
            VvcResidualComponent::Luma,
            log2_tb_width,
            log2_tb_height,
            coeff_levels,
        )
    }

    #[cfg(test)]
    pub(in crate::vvc) fn luma_transform_skip_coefficients(
        log2_tb_width: u8,
        log2_tb_height: u8,
        coeff_levels: &[i16],
    ) -> Self {
        Self::coefficients_with_transform_skip(
            VvcResidualComponent::Luma,
            log2_tb_width,
            log2_tb_height,
            coeff_levels,
            true,
        )
    }

    #[cfg(test)]
    #[allow(dead_code)]
    pub(in crate::vvc) fn luma_bdpcm_transform_skip_coefficients(
        log2_tb_width: u8,
        log2_tb_height: u8,
        coeff_levels: &[i16],
    ) -> Self {
        Self::coefficients_with_tool_flags(
            VvcResidualComponent::Luma,
            log2_tb_width,
            log2_tb_height,
            coeff_levels,
            true,
            true,
        )
    }

    #[cfg(test)]
    #[allow(dead_code)]
    pub(in crate::vvc) fn chroma_coefficients(
        component: VvcResidualComponent,
        log2_tb_width: u8,
        log2_tb_height: u8,
        coeff_levels: &[i16],
    ) -> Self {
        debug_assert!(matches!(
            component,
            VvcResidualComponent::ChromaCb | VvcResidualComponent::ChromaCr
        ));
        Self::coefficients(component, log2_tb_width, log2_tb_height, coeff_levels)
    }

    #[cfg(test)]
    #[allow(dead_code)]
    pub(in crate::vvc) fn chroma_transform_skip_coefficients(
        component: VvcResidualComponent,
        log2_tb_width: u8,
        log2_tb_height: u8,
        coeff_levels: &[i16],
    ) -> Self {
        debug_assert!(matches!(
            component,
            VvcResidualComponent::ChromaCb | VvcResidualComponent::ChromaCr
        ));
        Self::coefficients_with_transform_skip(
            component,
            log2_tb_width,
            log2_tb_height,
            coeff_levels,
            true,
        )
    }

    #[cfg(test)]
    #[allow(dead_code)]
    pub(in crate::vvc) fn chroma_bdpcm_transform_skip_coefficients(
        component: VvcResidualComponent,
        log2_tb_width: u8,
        log2_tb_height: u8,
        coeff_levels: &[i16],
    ) -> Self {
        debug_assert!(matches!(
            component,
            VvcResidualComponent::ChromaCb | VvcResidualComponent::ChromaCr
        ));
        Self::coefficients_with_tool_flags(
            component,
            log2_tb_width,
            log2_tb_height,
            coeff_levels,
            true,
            true,
        )
    }

    pub(in crate::vvc) fn emit_luma_transform_skip_coefficients(
        log2_tb_width: u8,
        log2_tb_height: u8,
        coeff_levels: &[i16],
        encoder: &mut VvcResidualCabacEncoder<'_>,
        cabac: &mut VvcCabacEncoder,
    ) {
        Self::emit_coefficients_with_transform_skip(
            VvcResidualComponent::Luma,
            log2_tb_width,
            log2_tb_height,
            coeff_levels,
            true,
            encoder,
            cabac,
        );
    }

    pub(in crate::vvc) fn emit_luma_stored_coefficients(
        log2_tb_width: u8,
        log2_tb_height: u8,
        dc_level: i16,
        ac_levels: &[i16; VVC_LUMA_AC_COEFFS_PER_TU],
        has_ac: bool,
        transform_skip: bool,
        mts_index: u8,
        encoder: &mut VvcResidualCabacEncoder<'_>,
        cabac: &mut VvcCabacEncoder,
    ) {
        if transform_skip {
            debug_assert_eq!(mts_index, 0);
        }
        Self::emit_stored_coefficients_with_tool_flags(
            VvcResidualComponent::Luma,
            log2_tb_width,
            log2_tb_height,
            dc_level,
            ac_levels,
            has_ac,
            luma_stored_coeff_stride(log2_tb_width, log2_tb_height),
            transform_skip,
            false,
            if transform_skip { 0 } else { mts_index },
            encoder,
            cabac,
        );
    }

    pub(in crate::vvc) fn emit_luma_bdpcm_transform_skip_coefficients(
        log2_tb_width: u8,
        log2_tb_height: u8,
        coeff_levels: &[i16],
        encoder: &mut VvcResidualCabacEncoder<'_>,
        cabac: &mut VvcCabacEncoder,
    ) {
        Self::emit_coefficients_with_tool_flags(
            VvcResidualComponent::Luma,
            log2_tb_width,
            log2_tb_height,
            coeff_levels,
            true,
            true,
            encoder,
            cabac,
        );
    }

    pub(in crate::vvc) fn emit_chroma_transform_skip_coefficients(
        component: VvcResidualComponent,
        log2_tb_width: u8,
        log2_tb_height: u8,
        coeff_levels: &[i16],
        encoder: &mut VvcResidualCabacEncoder<'_>,
        cabac: &mut VvcCabacEncoder,
    ) {
        debug_assert!(matches!(
            component,
            VvcResidualComponent::ChromaCb | VvcResidualComponent::ChromaCr
        ));
        Self::emit_coefficients_with_transform_skip(
            component,
            log2_tb_width,
            log2_tb_height,
            coeff_levels,
            true,
            encoder,
            cabac,
        );
    }

    pub(in crate::vvc) fn emit_chroma_stored_coefficients(
        component: VvcResidualComponent,
        log2_tb_width: u8,
        log2_tb_height: u8,
        dc_level: i16,
        ac_levels: &[i16; VVC_CHROMA_AC_COEFFS_PER_TU],
        has_ac: bool,
        transform_skip: bool,
        encoder: &mut VvcResidualCabacEncoder<'_>,
        cabac: &mut VvcCabacEncoder,
    ) {
        debug_assert!(matches!(
            component,
            VvcResidualComponent::ChromaCb | VvcResidualComponent::ChromaCr
        ));
        Self::emit_stored_coefficients_with_tool_flags(
            component,
            log2_tb_width,
            log2_tb_height,
            dc_level,
            ac_levels,
            has_ac,
            4,
            transform_skip,
            false,
            0,
            encoder,
            cabac,
        );
    }

    pub(in crate::vvc) fn emit_chroma_bdpcm_transform_skip_coefficients(
        component: VvcResidualComponent,
        log2_tb_width: u8,
        log2_tb_height: u8,
        coeff_levels: &[i16],
        encoder: &mut VvcResidualCabacEncoder<'_>,
        cabac: &mut VvcCabacEncoder,
    ) {
        debug_assert!(matches!(
            component,
            VvcResidualComponent::ChromaCb | VvcResidualComponent::ChromaCr
        ));
        Self::emit_coefficients_with_tool_flags(
            component,
            log2_tb_width,
            log2_tb_height,
            coeff_levels,
            true,
            true,
            encoder,
            cabac,
        );
    }

    #[cfg(test)]
    fn coefficients(
        component: VvcResidualComponent,
        log2_tb_width: u8,
        log2_tb_height: u8,
        coeff_levels: &[i16],
    ) -> Self {
        Self::coefficients_with_transform_skip(
            component,
            log2_tb_width,
            log2_tb_height,
            coeff_levels,
            false,
        )
    }

    #[cfg(test)]
    fn coefficients_with_transform_skip(
        component: VvcResidualComponent,
        log2_tb_width: u8,
        log2_tb_height: u8,
        coeff_levels: &[i16],
        transform_skip: bool,
    ) -> Self {
        Self::coefficients_with_tool_flags(
            component,
            log2_tb_width,
            log2_tb_height,
            coeff_levels,
            transform_skip,
            false,
        )
    }

    fn emit_coefficients_with_transform_skip(
        component: VvcResidualComponent,
        log2_tb_width: u8,
        log2_tb_height: u8,
        coeff_levels: &[i16],
        transform_skip: bool,
        encoder: &mut VvcResidualCabacEncoder<'_>,
        cabac: &mut VvcCabacEncoder,
    ) {
        Self::emit_coefficients_with_tool_flags(
            component,
            log2_tb_width,
            log2_tb_height,
            coeff_levels,
            transform_skip,
            false,
            encoder,
            cabac,
        );
    }

    fn emit_coefficients_with_tool_flags(
        component: VvcResidualComponent,
        log2_tb_width: u8,
        log2_tb_height: u8,
        coeff_levels: &[i16],
        transform_skip: bool,
        bdpcm: bool,
        encoder: &mut VvcResidualCabacEncoder<'_>,
        cabac: &mut VvcCabacEncoder,
    ) {
        let width = 1usize << log2_tb_width;
        let height = 1usize << log2_tb_height;
        let coeffs = VvcRasterCoeffAccessor::new(coeff_levels, width, height);
        Self::emit_coefficients_from_accessor(
            component,
            log2_tb_width,
            log2_tb_height,
            &coeffs,
            transform_skip,
            bdpcm,
            0,
            encoder,
            cabac,
        );
    }

    fn emit_stored_coefficients_with_tool_flags<const AC_COEFFS: usize>(
        component: VvcResidualComponent,
        log2_tb_width: u8,
        log2_tb_height: u8,
        dc_level: i16,
        ac_levels: &[i16; AC_COEFFS],
        has_ac: bool,
        coeff_stride: usize,
        transform_skip: bool,
        bdpcm: bool,
        mts_index: u8,
        encoder: &mut VvcResidualCabacEncoder<'_>,
        cabac: &mut VvcCabacEncoder,
    ) {
        let width = 1usize << log2_tb_width;
        let height = 1usize << log2_tb_height;
        let coeffs =
            VvcStoredCoeffAccessor::new(width, height, dc_level, ac_levels, has_ac, coeff_stride);
        Self::emit_coefficients_from_accessor(
            component,
            log2_tb_width,
            log2_tb_height,
            &coeffs,
            transform_skip,
            bdpcm,
            mts_index,
            encoder,
            cabac,
        );
    }

    fn emit_coefficients_from_accessor(
        component: VvcResidualComponent,
        log2_tb_width: u8,
        log2_tb_height: u8,
        coeffs: &impl VvcCoeffAccessor,
        transform_skip: bool,
        bdpcm: bool,
        mts_index: u8,
        encoder: &mut VvcResidualCabacEncoder<'_>,
        cabac: &mut VvcCabacEncoder,
    ) {
        let plan = Self::coefficient_plan_for_accessor(
            component,
            log2_tb_width,
            log2_tb_height,
            coeffs,
            transform_skip,
            bdpcm,
            mts_index,
        );
        encoder.emit_default_tool_control_hooks(cabac, &plan.pass1_state);
        let mut progressive_state = VvcResidualPass1State::new(plan.pass1_state.config);
        progressive_state.sb_coded = plan.pass1_state.sb_coded;
        Self::emit_coefficient_symbols_direct(
            encoder,
            cabac,
            &mut progressive_state,
            coeffs,
            log2_tb_width,
            log2_tb_height,
            plan.scan.as_slice(),
            plan.last_scan_pos,
        );
    }

    fn emit_coefficient_symbols_direct(
        encoder: &mut VvcResidualCabacEncoder<'_>,
        cabac: &mut VvcCabacEncoder,
        state: &mut VvcResidualPass1State,
        coeffs: &impl VvcCoeffAccessor,
        log2_tb_width: u8,
        log2_tb_height: u8,
        scan: &[VvcScanPosition],
        last_scan_pos: usize,
    ) {
        let width = coeffs.width();
        let height = coeffs.height();
        let last_x = scan[last_scan_pos].x as u8;
        let last_y = scan[last_scan_pos].y as u8;
        Self::emit_last_sig_coeff_position_direct(
            encoder,
            cabac,
            state.config.component,
            true,
            log2_tb_width,
            last_x,
        );
        Self::emit_last_sig_coeff_position_direct(
            encoder,
            cabac,
            state.config.component,
            false,
            log2_tb_height,
            last_y,
        );

        let last_subset = last_scan_pos / 16;
        let mut residual_state = 0u8;
        let mut rem_reg_bins = regular_bin_limit(width, height);
        for subset in (0..=last_subset).rev() {
            let min_scan_pos = subset * 16;
            let max_scan_pos = (min_scan_pos + 15).min(scan.len() - 1);
            let is_last = subset == last_subset;
            let is_not_first = subset != 0;
            let first_scan_pos = if is_last { last_scan_pos } else { max_scan_pos };
            let subblock = scan[min_scan_pos];
            let x_s = (subblock.x / 4) as u8;
            let y_s = (subblock.y / 4) as u8;
            let subset_coded = state.sb_coded_at(x_s, y_s);
            if !is_last && is_not_first {
                encoder.emit_sb_coded_flag(cabac, state, x_s, y_s, subset_coded);
                if !subset_coded {
                    continue;
                }
            }
            Self::emit_coefficient_subblock_symbols_direct(
                encoder,
                cabac,
                state,
                coeffs,
                scan,
                min_scan_pos,
                first_scan_pos,
                last_scan_pos,
                is_not_first,
                &mut rem_reg_bins,
                &mut residual_state,
            );
        }
    }

    fn emit_coefficient_subblock_symbols_direct(
        encoder: &mut VvcResidualCabacEncoder<'_>,
        cabac: &mut VvcCabacEncoder,
        state: &mut VvcResidualPass1State,
        coeffs: &impl VvcCoeffAccessor,
        scan: &[VvcScanPosition],
        min_scan_pos: usize,
        first_scan_pos: usize,
        last_scan_pos: usize,
        is_not_first: bool,
        rem_reg_bins: &mut i32,
        residual_state: &mut u8,
    ) {
        let mut remainder_symbols = [None; 16];
        let mut remainder_count = 0usize;
        let mut bypass_symbols = [None; 16];
        let mut bypass_count = 0usize;
        let mut sign_bits = 0u32;
        let mut sign_count = 0u8;
        let mut first_pos_2nd_pass: Option<usize> = None;
        let mut next_scan_pos = first_scan_pos as isize;
        let infer_sig_pos = if first_scan_pos != last_scan_pos {
            is_not_first.then_some(min_scan_pos)
        } else {
            Some(first_scan_pos)
        };
        let mut num_nonzero = 0usize;

        while next_scan_pos >= min_scan_pos as isize && *rem_reg_bins >= 4 {
            let scan_pos = next_scan_pos as usize;
            let pos = scan[scan_pos];
            let x = pos.x as u8;
            let y = pos.y as u8;
            let level = coeffs.level_at(pos.x, pos.y);
            let abs_level = level.unsigned_abs();
            let significant = abs_level != 0;
            if num_nonzero != 0 || Some(scan_pos) != infer_sig_pos {
                encoder.emit_sig_coeff_flag(cabac, state, x, y, significant);
                *rem_reg_bins -= 1;
            }
            if significant {
                num_nonzero += 1;
                Self::emit_regular_level_symbols_direct(encoder, cabac, state, x, y, abs_level);
                *rem_reg_bins -= regular_level_bin_count(abs_level);
                if abs_level > 3 {
                    first_pos_2nd_pass =
                        Some(first_pos_2nd_pass.map_or(scan_pos, |first| first.max(scan_pos)));
                }
                append_sign_bit(&mut sign_bits, &mut sign_count, level < 0);
                state.set_pass1_coeff(x, y, abs_level, level < 0);
            }
            *residual_state = disabled_dep_quant_state_transition(*residual_state, abs_level);
            next_scan_pos -= 1;
        }

        let min_pos_2nd_pass = next_scan_pos;
        if let Some(first_pos_2nd_pass) = first_pos_2nd_pass {
            for scan_pos in ((min_pos_2nd_pass + 1) as usize..=first_pos_2nd_pass).rev() {
                let pos = scan[scan_pos];
                let abs_level = coeffs.level_at(pos.x, pos.y).unsigned_abs();
                if abs_level >= 4 {
                    debug_assert!(remainder_count < remainder_symbols.len());
                    remainder_symbols[remainder_count] =
                        Some(VvcDelayedResidualCabacSymbol::AbsRemainder {
                            value: u32::from((abs_level - 4) >> 1),
                            rice_param: derive_rice_param_from_state(scan_pos, state, scan, 4),
                        });
                    remainder_count += 1;
                    state.set_rice_abs_level(pos.x as u8, pos.y as u8, abs_level);
                }
            }
        }

        if min_pos_2nd_pass >= 0 {
            for scan_pos in (min_scan_pos..=min_pos_2nd_pass as usize).rev() {
                let pos = scan[scan_pos];
                let level = coeffs.level_at(pos.x, pos.y);
                let abs_level = level.unsigned_abs();
                let rice_param = derive_rice_param_from_state(scan_pos, state, scan, 0);
                let zero_pos = go_rice_zero_position(*residual_state, rice_param);
                let rem_value = if abs_level == 0 {
                    zero_pos
                } else if u32::from(abs_level) <= zero_pos {
                    u32::from(abs_level - 1)
                } else {
                    u32::from(abs_level)
                };
                debug_assert!(bypass_count < bypass_symbols.len());
                bypass_symbols[bypass_count] =
                    Some(VvcDelayedResidualCabacSymbol::BypassAbsLevel {
                        value: rem_value,
                        rice_param,
                    });
                bypass_count += 1;
                *residual_state = disabled_dep_quant_state_transition(*residual_state, abs_level);
                if abs_level != 0 {
                    append_sign_bit(&mut sign_bits, &mut sign_count, level < 0);
                    state.set_pass1_coeff(pos.x as u8, pos.y as u8, abs_level, level < 0);
                    state.set_rice_abs_level(pos.x as u8, pos.y as u8, abs_level);
                }
            }
        }

        for symbol in remainder_symbols.iter().take(remainder_count).flatten() {
            Self::emit_delayed_symbol_direct(cabac, *symbol);
        }
        for symbol in bypass_symbols.iter().take(bypass_count).flatten() {
            Self::emit_delayed_symbol_direct(cabac, *symbol);
        }
        if sign_count > 0 {
            cabac.encode_bins_ep(sign_bits, u32::from(sign_count));
        }
    }

    fn emit_last_sig_coeff_position_direct(
        encoder: &mut VvcResidualCabacEncoder<'_>,
        cabac: &mut VvcCabacEncoder,
        component: VvcResidualComponent,
        x_prefix: bool,
        log2_tb_size: u8,
        position: u8,
    ) {
        let group_idx = last_sig_coeff_group_index(position);
        let max_group_idx = last_sig_coeff_group_index((1u8 << log2_tb_size) - 1);
        for bin_idx in 0..group_idx {
            encoder.emit_last_sig_coeff_prefix_bin(
                cabac,
                component,
                x_prefix,
                log2_tb_size,
                bin_idx,
                true,
            );
        }
        if group_idx < max_group_idx {
            encoder.emit_last_sig_coeff_prefix_bin(
                cabac,
                component,
                x_prefix,
                log2_tb_size,
                group_idx,
                false,
            );
        }
        if group_idx > 3 {
            let suffix_len = (group_idx - 2) >> 1;
            let suffix = u32::from(position - last_sig_coeff_group_min(group_idx));
            cabac.encode_bins_ep(suffix, u32::from(suffix_len));
        }
    }

    fn emit_regular_level_symbols_direct(
        encoder: &mut VvcResidualCabacEncoder<'_>,
        cabac: &mut VvcCabacEncoder,
        state: &VvcResidualPass1State,
        x: u8,
        y: u8,
        abs_level: u16,
    ) {
        encoder.emit_abs_level_gtx_flag(cabac, state, x, y, 0, abs_level > 1);
        if abs_level > 1 {
            encoder.emit_par_level_flag(cabac, state, x, y, (abs_level & 1) != 0);
            encoder.emit_abs_level_gtx_flag(cabac, state, x, y, 1, abs_level > 3);
        }
    }

    fn emit_delayed_symbol_direct(
        cabac: &mut VvcCabacEncoder,
        symbol: VvcDelayedResidualCabacSymbol,
    ) {
        match symbol {
            VvcDelayedResidualCabacSymbol::AbsRemainder { value, rice_param }
            | VvcDelayedResidualCabacSymbol::BypassAbsLevel { value, rice_param } => {
                cabac.encode_rem_abs_ep(value, u32::from(rice_param));
            }
        }
    }

    #[cfg(test)]
    fn coefficients_with_tool_flags(
        component: VvcResidualComponent,
        log2_tb_width: u8,
        log2_tb_height: u8,
        coeff_levels: &[i16],
        transform_skip: bool,
        bdpcm: bool,
    ) -> Self {
        let plan = Self::coefficient_plan_with_tool_flags(
            component,
            log2_tb_width,
            log2_tb_height,
            coeff_levels,
            transform_skip,
            bdpcm,
        );
        let mut symbols = Vec::new();
        let width = 1usize << log2_tb_width;
        let height = 1usize << log2_tb_height;
        let coeffs = VvcRasterCoeffAccessor::new(coeff_levels, width, height);
        let mut progressive_state = VvcResidualPass1State::new(plan.pass1_state.config);
        progressive_state.sb_coded = plan.pass1_state.sb_coded;
        Self::append_coefficient_symbols(
            &mut symbols,
            &mut progressive_state,
            &coeffs,
            log2_tb_width,
            log2_tb_height,
            plan.scan.as_slice(),
            plan.last_scan_pos,
        );

        Self {
            config: plan.config,
            pass1_state: plan.pass1_state,
            symbols,
        }
    }

    #[cfg(test)]
    fn coefficient_plan_with_tool_flags(
        component: VvcResidualComponent,
        log2_tb_width: u8,
        log2_tb_height: u8,
        coeff_levels: &[i16],
        transform_skip: bool,
        bdpcm: bool,
    ) -> VvcResidualCoefficientPlan {
        let width = 1usize << log2_tb_width;
        let height = 1usize << log2_tb_height;
        let coeffs = VvcRasterCoeffAccessor::new(coeff_levels, width, height);
        Self::coefficient_plan_for_accessor(
            component,
            log2_tb_width,
            log2_tb_height,
            &coeffs,
            transform_skip,
            bdpcm,
            0,
        )
    }

    fn coefficient_plan_for_accessor(
        component: VvcResidualComponent,
        log2_tb_width: u8,
        log2_tb_height: u8,
        coeffs: &impl VvcCoeffAccessor,
        transform_skip: bool,
        bdpcm: bool,
        mts_index: u8,
    ) -> VvcResidualCoefficientPlan {
        // H.266 7.3.11.11 residual_coding() first codes the last significant
        // coefficient position and then walks earlier scan positions with
        // sig_coeff_flag and level/sign syntax. VTM's CoeffCodingContext uses
        // SCAN_GROUPED_4x4 with diagonal scan (CommonLib/Rom.cpp). Current
        // production TUs cover up to 8x8 luma and 4x4 chroma, so this supports
        // four luma coefficient groups and one chroma coefficient group.
        //
        // H.266 7.3.11.11 still uses this residual_coding() syntax for
        // transform-skipped TUs when sh_ts_residual_coding_disabled_flag is 1.
        // The 4:4:4 screen-content residual subset relies on that normative
        // switch so transform_skip_flag affects reconstruction without adding
        // residual_codingTS()'s separate sign-context and neighbour-modulated
        // level syntax yet.
        let width = coeffs.width();
        let height = coeffs.height();
        debug_assert_eq!(width, 1usize << log2_tb_width);
        debug_assert_eq!(height, 1usize << log2_tb_height);

        let scan = if width == 8 && height == 8 {
            VvcScanPlan::grouped_8x8()
        } else {
            VvcScanPlan::first4x4()
        };
        let scan_slice = scan.as_slice();
        let last_scan_pos = scan
            .as_slice()
            .iter()
            .rposition(|pos| coeffs.level_at(pos.x, pos.y) != 0)
            .unwrap_or(0);
        let last_x = scan_slice[last_scan_pos].x as u8;
        let last_y = scan_slice[last_scan_pos].y as u8;

        let mut config =
            VvcResidualCtxConfig::subset(component, log2_tb_width, log2_tb_height, last_x, last_y);
        config.transform_skip = transform_skip;
        config.ts_residual_coding_disabled = true;
        config.bdpcm = bdpcm;
        config.mts_index = mts_index;
        let mut pass1_state = VvcResidualPass1State::new(config);
        for pos in scan_slice.iter().take(last_scan_pos + 1) {
            let level = coeffs.level_at(pos.x, pos.y);
            let x = pos.x as u8;
            let y = pos.y as u8;
            let abs_level = level.unsigned_abs();
            pass1_state.set_pass1_coeff(x, y, abs_level, level < 0);
        }
        for subset_start in (0..=last_scan_pos).step_by(16) {
            let subset_end = (subset_start + 15).min(scan_slice.len() - 1);
            let subset_coded = scan_slice[subset_start..=subset_end]
                .iter()
                .any(|pos| coeffs.level_at(pos.x, pos.y) != 0);
            let subblock = scan_slice[subset_start];
            pass1_state.set_sb_coded((subblock.x / 4) as u8, (subblock.y / 4) as u8, subset_coded);
        }

        VvcResidualCoefficientPlan {
            #[cfg(test)]
            config,
            pass1_state,
            scan,
            last_scan_pos,
        }
    }

    #[cfg(test)]
    fn append_coefficient_symbols<S: VvcResidualSymbolSink>(
        symbols: &mut S,
        state: &mut VvcResidualPass1State,
        coeffs: &impl VvcCoeffAccessor,
        log2_tb_width: u8,
        log2_tb_height: u8,
        scan: &[VvcScanPosition],
        last_scan_pos: usize,
    ) {
        let width = coeffs.width();
        let height = coeffs.height();
        let last_x = scan[last_scan_pos].x as u8;
        let last_y = scan[last_scan_pos].y as u8;
        Self::append_last_sig_coeff_position(symbols, true, log2_tb_width, last_x);
        Self::append_last_sig_coeff_position(symbols, false, log2_tb_height, last_y);

        let last_subset = last_scan_pos / 16;
        let mut residual_state = 0u8;
        let mut rem_reg_bins = regular_bin_limit(width, height);
        for subset in (0..=last_subset).rev() {
            let min_scan_pos = subset * 16;
            let max_scan_pos = (min_scan_pos + 15).min(scan.len() - 1);
            let is_last = subset == last_subset;
            let is_not_first = subset != 0;
            let first_scan_pos = if is_last { last_scan_pos } else { max_scan_pos };
            let subblock = scan[min_scan_pos];
            let x_s = (subblock.x / 4) as u8;
            let y_s = (subblock.y / 4) as u8;
            let subset_coded = state.sb_coded_at(x_s, y_s);
            if !is_last && is_not_first {
                symbols.sb_coded_flag(x_s, y_s, subset_coded);
                if !subset_coded {
                    continue;
                }
            }
            Self::append_coefficient_subblock_symbols(
                symbols,
                state,
                coeffs,
                scan,
                min_scan_pos,
                first_scan_pos,
                last_scan_pos,
                is_not_first,
                &mut rem_reg_bins,
                &mut residual_state,
            );
        }
    }

    #[cfg(test)]
    fn append_coefficient_subblock_symbols<S: VvcResidualSymbolSink>(
        symbols: &mut S,
        state: &mut VvcResidualPass1State,
        coeffs: &impl VvcCoeffAccessor,
        scan: &[VvcScanPosition],
        min_scan_pos: usize,
        first_scan_pos: usize,
        last_scan_pos: usize,
        is_not_first: bool,
        rem_reg_bins: &mut i32,
        residual_state: &mut u8,
    ) {
        let mut remainder_symbols = [None; 16];
        let mut remainder_count = 0usize;
        let mut bypass_symbols = [None; 16];
        let mut bypass_count = 0usize;
        let mut sign_bits = 0u32;
        let mut sign_count = 0u8;
        let mut first_pos_2nd_pass: Option<usize> = None;
        let mut next_scan_pos = first_scan_pos as isize;
        let infer_sig_pos = if first_scan_pos != last_scan_pos {
            is_not_first.then_some(min_scan_pos)
        } else {
            Some(first_scan_pos)
        };
        let mut num_nonzero = 0usize;

        while next_scan_pos >= min_scan_pos as isize && *rem_reg_bins >= 4 {
            let scan_pos = next_scan_pos as usize;
            let pos = scan[scan_pos];
            let x = pos.x as u8;
            let y = pos.y as u8;
            let level = coeffs.level_at(pos.x, pos.y);
            let abs_level = level.unsigned_abs();
            let significant = abs_level != 0;
            if num_nonzero != 0 || Some(scan_pos) != infer_sig_pos {
                symbols.sig_coeff_flag(x, y, significant);
                *rem_reg_bins -= 1;
            }
            if significant {
                num_nonzero += 1;
                Self::append_regular_level_symbols(symbols, x, y, abs_level);
                *rem_reg_bins -= regular_level_bin_count(abs_level);
                if abs_level > 3 {
                    first_pos_2nd_pass =
                        Some(first_pos_2nd_pass.map_or(scan_pos, |first| first.max(scan_pos)));
                }
                append_sign_bit(&mut sign_bits, &mut sign_count, level < 0);
                state.set_pass1_coeff(x, y, abs_level, level < 0);
            }
            *residual_state = disabled_dep_quant_state_transition(*residual_state, abs_level);
            next_scan_pos -= 1;
        }

        let min_pos_2nd_pass = next_scan_pos;
        if let Some(first_pos_2nd_pass) = first_pos_2nd_pass {
            for scan_pos in ((min_pos_2nd_pass + 1) as usize..=first_pos_2nd_pass).rev() {
                let pos = scan[scan_pos];
                let abs_level = coeffs.level_at(pos.x, pos.y).unsigned_abs();
                if abs_level >= 4 {
                    let x = pos.x as u8;
                    let y = pos.y as u8;
                    debug_assert!(remainder_count < remainder_symbols.len());
                    remainder_symbols[remainder_count] =
                        Some(VvcResidualCabacSymbol::AbsRemainder {
                            x,
                            y,
                            value: u32::from((abs_level - 4) >> 1),
                            rice_param: derive_rice_param_from_state(scan_pos, state, scan, 4),
                        });
                    remainder_count += 1;
                    state.set_rice_abs_level(x, y, abs_level);
                }
            }
        }

        if min_pos_2nd_pass >= 0 {
            for scan_pos in (min_scan_pos..=min_pos_2nd_pass as usize).rev() {
                let pos = scan[scan_pos];
                let level = coeffs.level_at(pos.x, pos.y);
                let abs_level = level.unsigned_abs();
                let rice_param = derive_rice_param_from_state(scan_pos, state, scan, 0);
                let zero_pos = go_rice_zero_position(*residual_state, rice_param);
                let rem_value = if abs_level == 0 {
                    zero_pos
                } else if u32::from(abs_level) <= zero_pos {
                    u32::from(abs_level - 1)
                } else {
                    u32::from(abs_level)
                };
                debug_assert!(bypass_count < bypass_symbols.len());
                bypass_symbols[bypass_count] = Some(VvcResidualCabacSymbol::BypassAbsLevel {
                    x: pos.x as u8,
                    y: pos.y as u8,
                    value: rem_value,
                    rice_param,
                });
                bypass_count += 1;
                *residual_state = disabled_dep_quant_state_transition(*residual_state, abs_level);
                if abs_level != 0 {
                    append_sign_bit(&mut sign_bits, &mut sign_count, level < 0);
                    state.set_pass1_coeff(pos.x as u8, pos.y as u8, abs_level, level < 0);
                    state.set_rice_abs_level(pos.x as u8, pos.y as u8, abs_level);
                }
            }
        }
        // H.266 7.3.11.11 / residual_coding_subblock(): Go-Rice remainders
        // are emitted in a second pass after all regular significant/gt/par
        // bins for the subblock. If the regular-bin budget is exhausted, the
        // remaining dec_abs_level values are bypass-coded before the grouped
        // coefficient signs. See VTM CABACWriter::residual_coding_subblock().
        for symbol in remainder_symbols.iter().take(remainder_count).flatten() {
            Self::append_delayed_symbol(symbols, *symbol);
        }
        for symbol in bypass_symbols.iter().take(bypass_count).flatten() {
            Self::append_delayed_symbol(symbols, *symbol);
        }
        if sign_count > 0 {
            symbols.coeff_sign_pattern(sign_bits, sign_count);
        }
    }

    #[cfg(test)]
    fn append_last_sig_coeff_position<S: VvcResidualSymbolSink>(
        symbols: &mut S,
        x_prefix: bool,
        log2_tb_size: u8,
        position: u8,
    ) {
        let group_idx = last_sig_coeff_group_index(position);
        let max_group_idx = last_sig_coeff_group_index((1u8 << log2_tb_size) - 1);
        for bin_idx in 0..group_idx {
            if x_prefix {
                symbols.last_sig_coeff_x_prefix(bin_idx, true);
            } else {
                symbols.last_sig_coeff_y_prefix(bin_idx, true);
            }
        }
        if group_idx < max_group_idx {
            if x_prefix {
                symbols.last_sig_coeff_x_prefix(group_idx, false);
            } else {
                symbols.last_sig_coeff_y_prefix(group_idx, false);
            }
        }
        if group_idx > 3 {
            let suffix_len = (group_idx - 2) >> 1;
            let suffix = u32::from(position - last_sig_coeff_group_min(group_idx));
            if x_prefix {
                symbols.last_sig_coeff_x_suffix(suffix, suffix_len);
            } else {
                symbols.last_sig_coeff_y_suffix(suffix, suffix_len);
            }
        }
    }

    #[cfg(test)]
    fn append_regular_level_symbols<S: VvcResidualSymbolSink>(
        symbols: &mut S,
        x: u8,
        y: u8,
        abs_level: u16,
    ) {
        // H.266 7.3.11.11 residual_coding_subblock regular-pass order: gt1,
        // parity, gt2. Remainder and sign bypass bins are collected and
        // emitted after this pass for the whole subblock.
        symbols.abs_level_gtx_flag(x, y, 0, abs_level > 1);
        if abs_level > 1 {
            symbols.par_level_flag(x, y, (abs_level & 1) != 0);
            symbols.abs_level_gtx_flag(x, y, 1, abs_level > 3);
        }
    }

    #[cfg(test)]
    fn append_delayed_symbol<S: VvcResidualSymbolSink>(
        symbols: &mut S,
        symbol: VvcResidualCabacSymbol,
    ) {
        match symbol {
            VvcResidualCabacSymbol::LastSigCoeffXPrefix { bin_idx, bin } => {
                symbols.last_sig_coeff_x_prefix(bin_idx, bin);
            }
            VvcResidualCabacSymbol::LastSigCoeffXSuffix { bits, count } => {
                symbols.last_sig_coeff_x_suffix(bits, count);
            }
            VvcResidualCabacSymbol::LastSigCoeffYPrefix { bin_idx, bin } => {
                symbols.last_sig_coeff_y_prefix(bin_idx, bin);
            }
            VvcResidualCabacSymbol::LastSigCoeffYSuffix { bits, count } => {
                symbols.last_sig_coeff_y_suffix(bits, count);
            }
            VvcResidualCabacSymbol::SbCodedFlag { x_s, y_s, coded } => {
                symbols.sb_coded_flag(x_s, y_s, coded);
            }
            VvcResidualCabacSymbol::SigCoeffFlag { x, y, significant } => {
                symbols.sig_coeff_flag(x, y, significant);
            }
            VvcResidualCabacSymbol::ParLevelFlag { x, y, par_level } => {
                symbols.par_level_flag(x, y, par_level);
            }
            VvcResidualCabacSymbol::AbsLevelGtxFlag {
                x,
                y,
                gtx_idx,
                greater_than,
            } => {
                symbols.abs_level_gtx_flag(x, y, gtx_idx, greater_than);
            }
            VvcResidualCabacSymbol::AbsRemainder {
                x,
                y,
                value,
                rice_param,
            } => {
                symbols.abs_remainder(x, y, value, rice_param);
            }
            VvcResidualCabacSymbol::BypassAbsLevel {
                x,
                y,
                value,
                rice_param,
            } => {
                symbols.bypass_abs_level(x, y, value, rice_param);
            }
            VvcResidualCabacSymbol::CoeffSignPattern { bits, count } => {
                symbols.coeff_sign_pattern(bits, count);
            }
        }
    }

    #[cfg(test)]
    pub(in crate::vvc) fn emit(
        &self,
        encoder: &mut VvcResidualCabacEncoder<'_>,
        cabac: &mut VvcCabacEncoder,
    ) {
        debug_assert_eq!(self.config, self.pass1_state.config);
        encoder.emit_default_tool_control_hooks(cabac, &self.pass1_state);
        for symbol in &self.symbols {
            encoder.emit_residual_symbol(cabac, &self.pass1_state, *symbol);
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct VvcScanPosition {
    x: usize,
    y: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct VvcScanPlan {
    positions: [VvcScanPosition; VVC_RESIDUAL_CONTEXT_COEFFS],
    len: usize,
}

impl VvcScanPlan {
    fn first4x4() -> Self {
        let mut plan = Self::empty();
        for pos in VVC_FIRST_4X4_DIAG_SCAN {
            plan.push(pos);
        }
        plan
    }

    fn grouped_8x8() -> Self {
        let mut plan = Self::empty();
        for (group_x, group_y) in [(0, 0), (0, 1), (1, 0), (1, 1)] {
            for pos in VVC_FIRST_4X4_DIAG_SCAN {
                plan.push(VvcScanPosition {
                    x: group_x * 4 + pos.x,
                    y: group_y * 4 + pos.y,
                });
            }
        }
        plan
    }

    fn empty() -> Self {
        Self {
            positions: [VvcScanPosition { x: 0, y: 0 }; VVC_RESIDUAL_CONTEXT_COEFFS],
            len: 0,
        }
    }

    fn push(&mut self, pos: VvcScanPosition) {
        debug_assert!(self.len < self.positions.len());
        self.positions[self.len] = pos;
        self.len += 1;
    }

    fn as_slice(&self) -> &[VvcScanPosition] {
        &self.positions[..self.len]
    }
}

const VVC_FIRST_4X4_DIAG_SCAN: [VvcScanPosition; 16] = [
    VvcScanPosition { x: 0, y: 0 },
    VvcScanPosition { x: 0, y: 1 },
    VvcScanPosition { x: 1, y: 0 },
    VvcScanPosition { x: 0, y: 2 },
    VvcScanPosition { x: 1, y: 1 },
    VvcScanPosition { x: 2, y: 0 },
    VvcScanPosition { x: 0, y: 3 },
    VvcScanPosition { x: 1, y: 2 },
    VvcScanPosition { x: 2, y: 1 },
    VvcScanPosition { x: 3, y: 0 },
    VvcScanPosition { x: 1, y: 3 },
    VvcScanPosition { x: 2, y: 2 },
    VvcScanPosition { x: 3, y: 1 },
    VvcScanPosition { x: 2, y: 3 },
    VvcScanPosition { x: 3, y: 2 },
    VvcScanPosition { x: 3, y: 3 },
];

fn template_abs_sum_level(abs_level: u16) -> u8 {
    abs_level.min(4 + (abs_level & 1)) as u8
}

fn last_sig_coeff_group_index(position: u8) -> u8 {
    match position {
        0..=3 => position,
        4..=5 => 4,
        6..=7 => 5,
        8..=11 => 6,
        12..=15 => 7,
        16..=23 => 8,
        24..=31 => 9,
        32..=47 => 10,
        48..=63 => 11,
        _ => unimplemented!("VVC last coefficient groups above 64 samples are not wired yet"),
    }
}

fn last_sig_coeff_group_min(group_idx: u8) -> u8 {
    match group_idx {
        0..=4 => group_idx,
        5 => 6,
        6 => 8,
        7 => 12,
        8 => 16,
        9 => 24,
        10 => 32,
        11 => 48,
        _ => unimplemented!("VVC last coefficient group minima above 64 samples are not wired yet"),
    }
}

fn luma_stored_coeff_stride(log2_tb_width: u8, log2_tb_height: u8) -> usize {
    if log2_tb_width == 3 && log2_tb_height == 3 {
        8
    } else {
        (1usize << log2_tb_width).min(4)
    }
}

fn regular_bin_limit(width: usize, height: usize) -> i32 {
    // H.266 7.3.11.11 residual_coding() initializes remBinsPass1 as
    // ((1 << (Log2ZoTbWidth + Log2ZoTbHeight)) * 7) >> 2. VTM expresses the
    // same value as (TbAreaAfterCoefZeroOut * MAX_TU_LEVEL_CTX_CODED_BIN_*) >>
    // 4; VTM 24.0 sets both luma and chroma constraints to 28.
    ((width * height * 28) >> 4) as i32
}

fn regular_level_bin_count(abs_level: u16) -> i32 {
    if abs_level > 1 {
        3
    } else {
        1
    }
}

fn append_sign_bit(sign_bits: &mut u32, sign_count: &mut u8, negative: bool) {
    if *sign_count != 0 {
        *sign_bits <<= 1;
    }
    *sign_bits |= u32::from(negative);
    *sign_count += 1;
}

fn disabled_dep_quant_state_transition(_state: u8, _abs_level: u16) -> u8 {
    // VTM passes stateTransTable = 0 when dependent quantization is disabled,
    // so the residual state remains zero for both regular and bypass passes.
    0
}

fn derive_rice_param_from_state(
    scan_pos: usize,
    state: &VvcResidualPass1State,
    scan: &[VvcScanPosition],
    base_level: i32,
) -> u8 {
    rice_param_from_template_abs_sum(
        rice_template_abs_sum_from_state(scan_pos, state, scan),
        base_level,
    )
}

fn rice_param_from_template_abs_sum(sum_abs: i32, base_level: i32) -> u8 {
    const GO_RICE_PARS_COEFF: [u8; 32] = [
        0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 1, 1, 1, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 3, 3,
        3, 3,
    ];
    let clipped = (sum_abs - 5 * base_level).clamp(0, 31) as usize;
    GO_RICE_PARS_COEFF[clipped]
}

fn rice_template_abs_sum_from_state(
    scan_pos: usize,
    state: &VvcResidualPass1State,
    scan: &[VvcScanPosition],
) -> i32 {
    let pos = scan[scan_pos];
    let width = state.config.tb_width();
    let height = state.config.tb_height();
    let x = pos.x;
    let y = pos.y;
    let mut sum = 0i32;
    if x + 1 < width {
        sum += state_rice_abs_at(state, x + 1, y);
        if x + 2 < width {
            sum += state_rice_abs_at(state, x + 2, y);
        }
        if y + 1 < height {
            sum += state_rice_abs_at(state, x + 1, y + 1);
        }
    }
    if y + 1 < height {
        sum += state_rice_abs_at(state, x, y + 1);
        if y + 2 < height {
            sum += state_rice_abs_at(state, x, y + 2);
        }
    }
    sum
}

fn state_rice_abs_at(state: &VvcResidualPass1State, x: usize, y: usize) -> i32 {
    state.rice_abs_level_at(x as u8, y as u8) as i32
}

fn go_rice_zero_position(state: u8, rice_param: u8) -> u32 {
    u32::from(if state < 2 { 1u8 } else { 2u8 }) << rice_param
}
