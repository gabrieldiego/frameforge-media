use super::palette::{Av2ChromaIntraMode, Av2LumaIntraMode};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Av2LeafPredictionMode {
    IntrabcCopy {
        drl_idx: u8,
    },
    Intra {
        luma_mode: Av2LumaIntraMode,
        use_luma_palette: bool,
        use_dpcm_y: bool,
        luma_bdpcm_horz: bool,
        use_bdpcm_uv: bool,
        chroma_intra_mode: Av2ChromaIntraMode,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Av2LeafResidualMode {
    None,
    BlackDc,
    LumaPalette {
        luma_bdpcm_horz: Option<bool>,
        chroma_use_bdpcm: bool,
        chroma_intra_mode: Av2ChromaIntraMode,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Av2LeafPredictionDecision {
    pub(crate) intrabc_flag: bool,
    pub(crate) prediction: Av2LeafPredictionMode,
    pub(crate) residual: Av2LeafResidualMode,
}

pub(crate) fn decide_leaf_prediction(
    allow_intrabc: bool,
    ibc_drl_idx: Option<u8>,
    luma_palette_enabled: bool,
    luma_mode: Av2LumaIntraMode,
    luma_bdpcm_horz: Option<bool>,
    chroma_use_bdpcm: bool,
    chroma_intra_mode: Av2ChromaIntraMode,
) -> Av2LeafPredictionDecision {
    // AV2 v1.0.0 Sections 5.20.5.1 and 5.20.8: IntraBC returns from
    // intra-frame mode parsing before palette/intra residual syntax. Keep that
    // priority explicit here so later rate decisions do not accidentally layer
    // residual syntax onto copied blocks.
    if allow_intrabc {
        if let Some(drl_idx) = ibc_drl_idx {
            return Av2LeafPredictionDecision {
                intrabc_flag: true,
                prediction: Av2LeafPredictionMode::IntrabcCopy { drl_idx },
                residual: Av2LeafResidualMode::None,
            };
        }
    }

    let use_dpcm_y = luma_bdpcm_horz.is_some();
    let use_luma_palette = luma_palette_enabled && luma_mode == Av2LumaIntraMode::Dc && !use_dpcm_y;
    let residual = if luma_palette_enabled {
        Av2LeafResidualMode::LumaPalette {
            luma_bdpcm_horz,
            chroma_use_bdpcm,
            chroma_intra_mode,
        }
    } else {
        Av2LeafResidualMode::BlackDc
    };

    Av2LeafPredictionDecision {
        intrabc_flag: false,
        prediction: Av2LeafPredictionMode::Intra {
            luma_mode,
            use_luma_palette,
            use_dpcm_y,
            luma_bdpcm_horz: luma_bdpcm_horz.unwrap_or(false),
            use_bdpcm_uv: luma_palette_enabled && chroma_use_bdpcm,
            chroma_intra_mode,
        },
        residual,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn intrabc_copy_suppresses_intra_residual() {
        let decision = decide_leaf_prediction(
            true,
            Some(1),
            true,
            Av2LumaIntraMode::Dc,
            None,
            true,
            Av2ChromaIntraMode::Horizontal,
        );

        assert_eq!(
            decision.prediction,
            Av2LeafPredictionMode::IntrabcCopy { drl_idx: 1 }
        );
        assert_eq!(decision.residual, Av2LeafResidualMode::None);
        assert!(decision.intrabc_flag);
    }

    #[test]
    fn dc_palette_intra_enables_palette_residual() {
        let decision = decide_leaf_prediction(
            true,
            None,
            true,
            Av2LumaIntraMode::Dc,
            None,
            true,
            Av2ChromaIntraMode::Vertical,
        );

        assert_eq!(
            decision.prediction,
            Av2LeafPredictionMode::Intra {
                luma_mode: Av2LumaIntraMode::Dc,
                use_luma_palette: true,
                use_dpcm_y: false,
                luma_bdpcm_horz: false,
                use_bdpcm_uv: true,
                chroma_intra_mode: Av2ChromaIntraMode::Vertical,
            }
        );
        assert_eq!(
            decision.residual,
            Av2LeafResidualMode::LumaPalette {
                luma_bdpcm_horz: None,
                chroma_use_bdpcm: true,
                chroma_intra_mode: Av2ChromaIntraMode::Vertical
            }
        );
        assert!(!decision.intrabc_flag);
    }

    #[test]
    fn non_palette_path_keeps_black_dc_residual() {
        let decision = decide_leaf_prediction(
            false,
            Some(0),
            false,
            Av2LumaIntraMode::Horizontal,
            None,
            false,
            Av2ChromaIntraMode::Horizontal,
        );

        assert_eq!(
            decision.prediction,
            Av2LeafPredictionMode::Intra {
                luma_mode: Av2LumaIntraMode::Horizontal,
                use_luma_palette: false,
                use_dpcm_y: false,
                luma_bdpcm_horz: false,
                use_bdpcm_uv: false,
                chroma_intra_mode: Av2ChromaIntraMode::Horizontal,
            }
        );
        assert_eq!(decision.residual, Av2LeafResidualMode::BlackDc);
        assert!(!decision.intrabc_flag);
    }
}
