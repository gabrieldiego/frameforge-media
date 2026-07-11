#[derive(Debug, Clone, Copy)]
pub(crate) struct Av2LumaModeSyntax {
    pub(crate) context: u8,
    mode_indices: [u8; AV2_LUMA_NON_DIRECTIONAL_MODE_COUNT + AV2_LUMA_DIRECTIONAL_MODE_COUNT],
}

impl Av2LumaModeSyntax {
    pub(crate) fn index_for(self, mode: Av2LumaIntraMode) -> u8 {
        if mode.directional().is_none() {
            return mode.mode_index() as u8;
        }
        self.mode_indices[mode.joint_mode()]
    }
}

pub(crate) fn av2_luma_mode_syntax_for_block(
    bottom_left_mode: Option<Av2LumaIntraMode>,
    above_right_mode: Option<Av2LumaIntraMode>,
    large_block: bool,
) -> Av2LumaModeSyntax {
    let left_directional = bottom_left_mode.filter(|mode| mode.is_directional());
    let above_right_directional = above_right_mode.filter(|mode| mode.is_directional());
    let context =
        u8::from(left_directional.is_some()) + u8::from(above_right_directional.is_some());

    // AV2 v1.0.0 get_y_mode_idx_ctx()/get_y_intra_mode_set(), mirrored from
    // AVM reconintra.c: the entropy context counts directional bottom-left and
    // above-right modes, and the mode list appends bottom-left first. Large
    // blocks also insert derived directional neighbors before the default
    // directional list, so V/H cannot be represented by fixed indices.
    let mut selected =
        [false; AV2_LUMA_NON_DIRECTIONAL_MODE_COUNT + AV2_LUMA_DIRECTIONAL_MODE_COUNT];
    let mut mode_indices =
        [0u8; AV2_LUMA_NON_DIRECTIONAL_MODE_COUNT + AV2_LUMA_DIRECTIONAL_MODE_COUNT];
    for (index, entry) in selected
        .iter_mut()
        .take(AV2_LUMA_NON_DIRECTIONAL_MODE_COUNT)
        .enumerate()
    {
        *entry = true;
        mode_indices[index] = index as u8;
    }
    let mut mode_index = AV2_LUMA_NON_DIRECTIONAL_MODE_COUNT;
    let mut add_mode = |joint_mode: usize, mode_index: &mut usize| {
        if selected[joint_mode] {
            return;
        }
        selected[joint_mode] = true;
        mode_indices[joint_mode] = *mode_index as u8;
        *mode_index += 1;
    };

    let mut neighbor_joint_modes = [
        left_directional.map(|mode| mode.joint_mode()),
        above_right_directional.map(|mode| mode.joint_mode()),
    ];
    let mut directional_count = usize::from(neighbor_joint_modes[0].is_some())
        + usize::from(neighbor_joint_modes[1].is_some());
    if directional_count == 2 && neighbor_joint_modes[0] == neighbor_joint_modes[1] {
        directional_count = 1;
    }
    if directional_count == 1 && neighbor_joint_modes[0].is_none() {
        neighbor_joint_modes[0] = neighbor_joint_modes[1];
    }

    for joint_mode in neighbor_joint_modes
        .iter()
        .copied()
        .take(directional_count)
        .flatten()
    {
        add_mode(joint_mode, &mut mode_index);
    }

    if large_block {
        for offset in 0..4 {
            for joint_mode in neighbor_joint_modes
                .iter()
                .copied()
                .take(directional_count)
                .flatten()
            {
                let left_derived = (joint_mode - offset
                    + (AV2_LUMA_DIRECTIONAL_MODE_COUNT - AV2_LUMA_NON_DIRECTIONAL_MODE_COUNT - 1))
                    % AV2_LUMA_DIRECTIONAL_MODE_COUNT
                    + AV2_LUMA_NON_DIRECTIONAL_MODE_COUNT;
                add_mode(left_derived, &mut mode_index);
                let right_derived = (joint_mode + offset
                    - (AV2_LUMA_NON_DIRECTIONAL_MODE_COUNT - 1))
                    % AV2_LUMA_DIRECTIONAL_MODE_COUNT
                    + AV2_LUMA_NON_DIRECTIONAL_MODE_COUNT;
                add_mode(right_derived, &mut mode_index);
            }
        }
    }

    for joint_mode in AV2_LUMA_DEFAULT_DIRECTIONAL_MODE_LIST {
        add_mode(joint_mode, &mut mode_index);
    }

    debug_assert!(selected.iter().all(|selected| *selected));

    Av2LumaModeSyntax {
        context,
        mode_indices,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn av2_luma_mode_syntax_preserves_non_directional_indices() {
        let syntax = av2_luma_mode_syntax_for_block(None, None, false);

        assert_eq!(syntax.index_for(Av2LumaIntraMode::Dc), 0);
        assert_eq!(syntax.index_for(Av2LumaIntraMode::Smooth), 1);
        assert_eq!(syntax.index_for(Av2LumaIntraMode::SmoothVertical), 2);
        assert_eq!(syntax.index_for(Av2LumaIntraMode::SmoothHorizontal), 3);
        assert_eq!(syntax.index_for(Av2LumaIntraMode::Paeth), 4);
    }
}
