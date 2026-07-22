use super::tile::Av2TileRegion;
use super::{Av2ChromaFormat, Av2VideoGeometry};
use crate::picture::{
    chroma_subsample_x as planar_chroma_subsample_x,
    chroma_subsample_y as planar_chroma_subsample_y, PlanarYuvFrameLayout, PlanarYuvPlane,
    SampleBitDepth,
};

const AV2_PLANAR_HASH_PRIME: u64 = 0x1000_0000_01b3;
const AV2_PLANAR_MI_SIZE: usize = 4;
const AV2_PLANAR_TX4X4_SIZE: usize = 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Av2PlanarYuvLayout {
    geometry: Av2VideoGeometry,
    layout: PlanarYuvFrameLayout,
}

impl Av2PlanarYuvLayout {
    pub(crate) fn new(
        geometry: Av2VideoGeometry,
        chroma_format: Av2ChromaFormat,
        bit_depth: SampleBitDepth,
    ) -> Result<Self, String> {
        if !matches!(
            chroma_format,
            Av2ChromaFormat::Yuv420 | Av2ChromaFormat::Yuv422 | Av2ChromaFormat::Yuv444
        ) {
            return Err(format!("AV2 planar YUV does not support {chroma_format:?}"));
        }
        let layout = PlanarYuvFrameLayout::new(
            geometry.width,
            geometry.height,
            chroma_format.chroma_sampling(),
            bit_depth,
        )?;
        Ok(Self { geometry, layout })
    }

    #[cfg(test)]
    pub(crate) fn expected_len(self) -> usize {
        self.layout.frame_len()
    }

    #[cfg(test)]
    fn luma_byte_len(self) -> usize {
        self.layout.luma_byte_len()
    }

    #[allow(dead_code)]
    pub(crate) fn validate_frame_len(self, frame: &[u8], label: &str) -> Result<(), String> {
        if frame.len() != self.layout.frame_len() {
            return Err(format!(
                "AV2 planar YUV {label} length mismatch: expected {} byte(s), got {}",
                self.layout.frame_len(),
                frame.len()
            ));
        }
        Ok(())
    }

    pub(crate) fn region_equal_in_frame(
        self,
        frame: &[u8],
        x0: usize,
        y0: usize,
        ref_x0: usize,
        ref_y0: usize,
        width: usize,
        height: usize,
    ) -> bool {
        self.layout
            .region_equal_in_frame(frame, x0, y0, ref_x0, ref_y0, width, height)
    }

    pub(crate) fn regions_equal_between(
        self,
        current: &[u8],
        x0: usize,
        y0: usize,
        reference: &[u8],
        ref_x0: usize,
        ref_y0: usize,
        width: usize,
        height: usize,
    ) -> bool {
        self.layout
            .regions_equal_between(current, x0, y0, reference, ref_x0, ref_y0, width, height)
    }

    pub(crate) fn copy_region_between(
        self,
        dst: &mut [u8],
        x0: usize,
        y0: usize,
        src: &[u8],
        ref_x0: usize,
        ref_y0: usize,
        width: usize,
        height: usize,
    ) -> bool {
        self.layout
            .copy_region_between(dst, x0, y0, src, ref_x0, ref_y0, width, height)
    }

    #[allow(dead_code)]
    pub(crate) fn hash_region(
        self,
        frame: &[u8],
        x0: usize,
        y0: usize,
        width: usize,
        height: usize,
    ) -> u64 {
        debug_assert_eq!(frame.len(), self.layout.frame_len());
        debug_assert!(x0
            .checked_add(width)
            .is_some_and(|x1| x1 <= self.geometry.width));
        debug_assert!(y0
            .checked_add(height)
            .is_some_and(|y1| y1 <= self.geometry.height));
        let (y_plane, u_plane, v_plane) = self.layout.plane_slices(frame);
        let mut hash = 0xcbf2_9ce4_8422_2325u64;
        hash = hash_plane_region(
            y_plane,
            hash,
            0,
            self.layout.plane_stride(PlanarYuvPlane::Y),
            x0,
            y0,
            width,
            height,
            self.layout.bytes_per_sample(),
        );
        let (sub_x, sub_y) = self.layout.plane_subsampling(PlanarYuvPlane::U);
        debug_assert_eq!(width % sub_x, 0);
        debug_assert_eq!(height % sub_y, 0);
        let chroma_width = width / sub_x;
        let chroma_height = height / sub_y;
        for (plane, plane_data) in [(1, u_plane), (2, v_plane)] {
            hash = hash_plane_region(
                plane_data,
                hash,
                plane,
                self.layout.plane_stride(PlanarYuvPlane::U),
                x0 / sub_x,
                y0 / sub_y,
                chroma_width,
                chroma_height,
                self.layout.bytes_per_sample(),
            );
        }
        hash
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Av2PlanarPlane {
    Y,
    U,
    V,
}

impl Av2PlanarPlane {
    fn yuv(self) -> PlanarYuvPlane {
        match self {
            Self::Y => PlanarYuvPlane::Y,
            Self::U => PlanarYuvPlane::U,
            Self::V => PlanarYuvPlane::V,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Av2PlanarTileLayout {
    region: Av2TileRegion,
    frame: PlanarYuvFrameLayout,
}

impl Av2PlanarTileLayout {
    pub(crate) fn for_validated_shape(
        geometry: Av2VideoGeometry,
        region: Av2TileRegion,
        chroma_format: Av2ChromaFormat,
        bit_depth: SampleBitDepth,
    ) -> Self {
        debug_assert!(
            matches!(
                chroma_format,
                Av2ChromaFormat::Yuv420 | Av2ChromaFormat::Yuv422 | Av2ChromaFormat::Yuv444
            ),
            "AV2 planar tile layout expects 4:2:0, 4:2:2, or 4:4:4 input"
        );
        let frame = PlanarYuvFrameLayout::for_validated_shape(
            geometry.width,
            geometry.height,
            chroma_format.chroma_sampling(),
            bit_depth,
        );
        Self { region, frame }
    }

    pub(crate) fn frame_len(self) -> usize {
        self.frame.frame_len()
    }

    pub(crate) fn plane_geometry(self, plane: Av2PlanarPlane) -> (usize, usize) {
        self.frame.plane_dimensions(plane.yuv())
    }

    pub(crate) fn plane_stride(self, plane: Av2PlanarPlane) -> usize {
        self.frame.plane_stride(plane.yuv())
    }

    pub(crate) fn plane_origin(self, plane: Av2PlanarPlane) -> (usize, usize) {
        let (sub_x, sub_y) = self.plane_subsampling(plane);
        (self.region.origin_x / sub_x, self.region.origin_y / sub_y)
    }

    pub(crate) fn plane_region_limit(self, plane: Av2PlanarPlane) -> (usize, usize) {
        let (origin_x, origin_y) = self.plane_origin(plane);
        let (sub_x, sub_y) = self.plane_subsampling(plane);
        (
            origin_x + self.region.width / sub_x,
            origin_y + self.region.height / sub_y,
        )
    }

    pub(crate) fn clipped_plane_region_limit(self, plane: Av2PlanarPlane) -> (usize, usize) {
        let (plane_width, plane_height) = self.plane_geometry(plane);
        let (sub_x, sub_y) = self.plane_subsampling(plane);
        (
            ((self.region.origin_x + self.region.width).div_ceil(sub_x)).min(plane_width),
            ((self.region.origin_y + self.region.height).div_ceil(sub_y)).min(plane_height),
        )
    }

    pub(crate) fn plane_subsampling(self, plane: Av2PlanarPlane) -> (usize, usize) {
        self.frame.plane_subsampling(plane.yuv())
    }

    pub(crate) fn coded_mi_for_plane_sample(
        self,
        plane: Av2PlanarPlane,
        x: usize,
        y: usize,
    ) -> (usize, usize) {
        let (sub_x, sub_y) = self.plane_subsampling(plane);
        let luma_x = x * sub_x;
        let luma_y = y * sub_y;
        (
            luma_y.saturating_sub(self.region.origin_y) / AV2_PLANAR_MI_SIZE,
            luma_x.saturating_sub(self.region.origin_x) / AV2_PLANAR_MI_SIZE,
        )
    }

    pub(crate) fn txb_origin(
        self,
        plane: Av2PlanarPlane,
        col: usize,
        row: usize,
    ) -> (usize, usize) {
        let (origin_x, origin_y) = self.plane_origin(plane);
        (
            origin_x + col * AV2_PLANAR_TX4X4_SIZE,
            origin_y + row * AV2_PLANAR_TX4X4_SIZE,
        )
    }

    pub(crate) fn offset(self, plane: Av2PlanarPlane, x: usize, y: usize) -> usize {
        self.frame.sample_offset(plane.yuv(), x, y)
    }
}

pub(crate) fn chroma_subsample_x(chroma_format: Av2ChromaFormat) -> usize {
    planar_chroma_subsample_x(chroma_format.chroma_sampling())
}

pub(crate) fn chroma_subsample_y(chroma_format: Av2ChromaFormat) -> usize {
    planar_chroma_subsample_y(chroma_format.chroma_sampling())
}

#[allow(dead_code)]
fn hash_plane_region(
    plane_data: &[u8],
    mut hash: u64,
    plane: u64,
    stride: usize,
    x0: usize,
    y0: usize,
    width: usize,
    height: usize,
    bytes_per_sample: usize,
) -> u64 {
    hash ^= plane.wrapping_mul(0x9e37_79b9_7f4a_7c15);
    hash = hash.wrapping_mul(AV2_PLANAR_HASH_PRIME);
    let row_bytes = width * bytes_per_sample;
    for local_y in 0..height {
        let row_start = ((y0 + local_y) * stride + x0) * bytes_per_sample;
        hash = hash_bytes(hash, &plane_data[row_start..row_start + row_bytes]);
    }
    hash
}

fn hash_bytes(mut hash: u64, bytes: &[u8]) -> u64 {
    let mut chunks = bytes.chunks_exact(8);
    for chunk in &mut chunks {
        let word = u64::from_le_bytes(chunk.try_into().expect("8-byte chunk"));
        hash ^= word;
        hash = hash.wrapping_mul(AV2_PLANAR_HASH_PRIME);
    }
    for byte in chunks.remainder() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(AV2_PLANAR_HASH_PRIME);
    }
    hash
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compares_420_regions_across_frames() {
        let geometry = Av2VideoGeometry {
            width: 16,
            height: 16,
        };
        let bit_depth = SampleBitDepth::new(8).expect("8-bit depth is supported");
        let layout = Av2PlanarYuvLayout::new(geometry, Av2ChromaFormat::Yuv420, bit_depth).unwrap();
        let mut a = vec![0u8; layout.expected_len()];
        let mut b = vec![0u8; layout.expected_len()];
        for (index, sample) in a.iter_mut().enumerate() {
            *sample = (index % 251) as u8;
        }
        b.copy_from_slice(&a);
        b[0] ^= 1;

        assert!(layout.regions_equal_between(&a, 8, 0, &b, 8, 0, 8, 8));
        assert!(!layout.regions_equal_between(&a, 0, 0, &b, 0, 0, 8, 8));
    }

    #[test]
    fn hashes_change_with_chroma_bytes() {
        let geometry = Av2VideoGeometry {
            width: 16,
            height: 16,
        };
        let bit_depth = SampleBitDepth::new(10).expect("10-bit depth is supported");
        let layout = Av2PlanarYuvLayout::new(geometry, Av2ChromaFormat::Yuv444, bit_depth).unwrap();
        let mut a = vec![0u8; layout.expected_len()];
        let mut b = vec![0u8; layout.expected_len()];
        b[layout.luma_byte_len()] = 1;

        assert_ne!(
            layout.hash_region(&a, 0, 0, 8, 8),
            layout.hash_region(&b, 0, 0, 8, 8)
        );
        a[layout.luma_byte_len()] = 1;
        assert_eq!(
            layout.hash_region(&a, 0, 0, 8, 8),
            layout.hash_region(&b, 0, 0, 8, 8)
        );
    }
}
