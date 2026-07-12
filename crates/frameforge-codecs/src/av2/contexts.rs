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
        let mut coefficients = [0i32; TX4X4_SAMPLES];
        coefficients[*TX4X4_SCAN.last().expect("TX_4X4 scan is non-empty")] = 8;
        let (_, bounds) = lossless_coefficient_levels_and_bounds(&coefficients);

        // AV2 v1.0.0 Section 5.20.7.27 coeffs(), mirrored by AVM coefficient
        // coding, permits EOB values up to the transform sample count. A
        // nonzero final scan coefficient must therefore signal eob=16, not
        // wrap to txb_skip=1 in narrower RTL state.
        assert_eq!(bounds, Some((TX4X4_SAMPLES - 1, TX4X4_SAMPLES)));
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
            Av2LosslessSubsampledModeSearch::Exhaustive,
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
