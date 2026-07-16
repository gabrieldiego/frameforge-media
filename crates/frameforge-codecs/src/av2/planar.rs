use super::{Av2ChromaFormat, Av2VideoGeometry};
use crate::picture::{Picture, PixelFormat, SampleBitDepth};

const AV2_PLANAR_HASH_PRIME: u64 = 0x1000_0000_01b3;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Av2PlanarYuvLayout {
    geometry: Av2VideoGeometry,
    chroma_format: Av2ChromaFormat,
    chroma_width: usize,
    bytes_per_sample: usize,
    y_bytes: usize,
    c_bytes: usize,
    expected_len: usize,
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
        let sub_x = chroma_subsample_x(chroma_format);
        let sub_y = chroma_subsample_y(chroma_format);
        if geometry.width % sub_x != 0 || geometry.height % sub_y != 0 {
            return Err(format!(
                "AV2 {:?} dimensions must be divisible by {}x{}, got {}x{}",
                chroma_format, sub_x, sub_y, geometry.width, geometry.height
            ));
        }
        let chroma_width = geometry.width / sub_x;
        let chroma_height = geometry.height / sub_y;
        let bytes_per_sample = bit_depth.bytes_per_sample();
        let y_bytes = geometry.width * geometry.height * bytes_per_sample;
        let c_bytes = chroma_width * chroma_height * bytes_per_sample;
        let expected_len = Picture::expected_len(
            geometry.width,
            geometry.height,
            PixelFormat::planar_yuv(chroma_format.chroma_sampling(), bit_depth),
        );
        Ok(Self {
            geometry,
            chroma_format,
            chroma_width,
            bytes_per_sample,
            y_bytes,
            c_bytes,
            expected_len,
        })
    }

    #[cfg(test)]
    pub(crate) fn expected_len(self) -> usize {
        self.expected_len
    }

    #[allow(dead_code)]
    pub(crate) fn validate_frame_len(self, frame: &[u8], label: &str) -> Result<(), String> {
        if frame.len() != self.expected_len {
            return Err(format!(
                "AV2 planar YUV {label} length mismatch: expected {} byte(s), got {}",
                self.expected_len,
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
        self.regions_equal_between(frame, x0, y0, frame, ref_x0, ref_y0, width, height)
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
        if current.len() != self.expected_len || reference.len() != self.expected_len {
            return false;
        }
        if !self.luma_region_in_bounds(x0, y0, width, height)
            || !self.luma_region_in_bounds(ref_x0, ref_y0, width, height)
        {
            return false;
        }

        let (current_y, current_u, current_v) = self.plane_slices(current);
        let (reference_y, reference_u, reference_v) = self.plane_slices(reference);
        if !plane_regions_equal_between(
            current_y,
            self.geometry.width,
            x0,
            y0,
            reference_y,
            self.geometry.width,
            ref_x0,
            ref_y0,
            width,
            height,
            self.bytes_per_sample,
        ) {
            return false;
        }

        let sub_x = chroma_subsample_x(self.chroma_format);
        let sub_y = chroma_subsample_y(self.chroma_format);
        if width % sub_x != 0
            || height % sub_y != 0
            || x0 % sub_x != 0
            || y0 % sub_y != 0
            || ref_x0 % sub_x != 0
            || ref_y0 % sub_y != 0
        {
            return false;
        }
        let chroma_width = width / sub_x;
        let chroma_height = height / sub_y;
        for (current_plane, reference_plane) in [(current_u, reference_u), (current_v, reference_v)]
        {
            if !plane_regions_equal_between(
                current_plane,
                self.chroma_width,
                x0 / sub_x,
                y0 / sub_y,
                reference_plane,
                self.chroma_width,
                ref_x0 / sub_x,
                ref_y0 / sub_y,
                chroma_width,
                chroma_height,
                self.bytes_per_sample,
            ) {
                return false;
            }
        }
        true
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
        if dst.len() != self.expected_len || src.len() != self.expected_len {
            return false;
        }
        if !self.luma_region_in_bounds(x0, y0, width, height)
            || !self.luma_region_in_bounds(ref_x0, ref_y0, width, height)
        {
            return false;
        }

        let sub_x = chroma_subsample_x(self.chroma_format);
        let sub_y = chroma_subsample_y(self.chroma_format);
        if width % sub_x != 0
            || height % sub_y != 0
            || x0 % sub_x != 0
            || y0 % sub_y != 0
            || ref_x0 % sub_x != 0
            || ref_y0 % sub_y != 0
        {
            return false;
        }

        let (dst_y, dst_u, dst_v) = self.plane_slices_mut(dst);
        let (src_y, src_u, src_v) = self.plane_slices(src);
        copy_plane_region_between(
            dst_y,
            self.geometry.width,
            x0,
            y0,
            src_y,
            self.geometry.width,
            ref_x0,
            ref_y0,
            width,
            height,
            self.bytes_per_sample,
        );

        let chroma_width = width / sub_x;
        let chroma_height = height / sub_y;
        for (dst_plane, src_plane) in [(dst_u, src_u), (dst_v, src_v)] {
            copy_plane_region_between(
                dst_plane,
                self.chroma_width,
                x0 / sub_x,
                y0 / sub_y,
                src_plane,
                self.chroma_width,
                ref_x0 / sub_x,
                ref_y0 / sub_y,
                chroma_width,
                chroma_height,
                self.bytes_per_sample,
            );
        }
        true
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
        debug_assert_eq!(frame.len(), self.expected_len);
        debug_assert!(self.luma_region_in_bounds(x0, y0, width, height));
        let (y_plane, u_plane, v_plane) = self.plane_slices(frame);
        let mut hash = 0xcbf2_9ce4_8422_2325u64;
        hash = hash_plane_region(
            y_plane,
            hash,
            0,
            self.geometry.width,
            x0,
            y0,
            width,
            height,
            self.bytes_per_sample,
        );
        let sub_x = chroma_subsample_x(self.chroma_format);
        let sub_y = chroma_subsample_y(self.chroma_format);
        debug_assert_eq!(width % sub_x, 0);
        debug_assert_eq!(height % sub_y, 0);
        let chroma_width = width / sub_x;
        let chroma_height = height / sub_y;
        for (plane, plane_data) in [(1, u_plane), (2, v_plane)] {
            hash = hash_plane_region(
                plane_data,
                hash,
                plane,
                self.chroma_width,
                x0 / sub_x,
                y0 / sub_y,
                chroma_width,
                chroma_height,
                self.bytes_per_sample,
            );
        }
        hash
    }

    fn plane_slices<'a>(self, frame: &'a [u8]) -> (&'a [u8], &'a [u8], &'a [u8]) {
        let y = &frame[..self.y_bytes];
        let u_start = self.y_bytes;
        let v_start = self.y_bytes + self.c_bytes;
        let u = &frame[u_start..v_start];
        let v = &frame[v_start..v_start + self.c_bytes];
        (y, u, v)
    }

    fn plane_slices_mut<'a>(
        self,
        frame: &'a mut [u8],
    ) -> (&'a mut [u8], &'a mut [u8], &'a mut [u8]) {
        let (y, chroma) = frame.split_at_mut(self.y_bytes);
        let (u, v) = chroma.split_at_mut(self.c_bytes);
        (y, u, &mut v[..self.c_bytes])
    }

    fn luma_region_in_bounds(self, x0: usize, y0: usize, width: usize, height: usize) -> bool {
        x0.checked_add(width)
            .is_some_and(|x1| x1 <= self.geometry.width)
            && y0
                .checked_add(height)
                .is_some_and(|y1| y1 <= self.geometry.height)
    }
}

pub(crate) fn chroma_subsample_x(chroma_format: Av2ChromaFormat) -> usize {
    match chroma_format {
        Av2ChromaFormat::Yuv420 | Av2ChromaFormat::Yuv422 => 2,
        Av2ChromaFormat::Yuv444 => 1,
    }
}

pub(crate) fn chroma_subsample_y(chroma_format: Av2ChromaFormat) -> usize {
    match chroma_format {
        Av2ChromaFormat::Yuv420 => 2,
        Av2ChromaFormat::Yuv422 | Av2ChromaFormat::Yuv444 => 1,
    }
}

fn plane_regions_equal_between(
    current_plane: &[u8],
    current_stride: usize,
    x0: usize,
    y0: usize,
    reference_plane: &[u8],
    reference_stride: usize,
    ref_x0: usize,
    ref_y0: usize,
    width: usize,
    height: usize,
    bytes_per_sample: usize,
) -> bool {
    let row_bytes = width * bytes_per_sample;
    for local_y in 0..height {
        let row_start = ((y0 + local_y) * current_stride + x0) * bytes_per_sample;
        let ref_row_start = ((ref_y0 + local_y) * reference_stride + ref_x0) * bytes_per_sample;
        if current_plane[row_start..row_start + row_bytes]
            != reference_plane[ref_row_start..ref_row_start + row_bytes]
        {
            return false;
        }
    }
    true
}

fn copy_plane_region_between(
    dst_plane: &mut [u8],
    dst_stride: usize,
    x0: usize,
    y0: usize,
    src_plane: &[u8],
    src_stride: usize,
    ref_x0: usize,
    ref_y0: usize,
    width: usize,
    height: usize,
    bytes_per_sample: usize,
) {
    let row_bytes = width * bytes_per_sample;
    for local_y in 0..height {
        let dst_row_start = ((y0 + local_y) * dst_stride + x0) * bytes_per_sample;
        let src_row_start = ((ref_y0 + local_y) * src_stride + ref_x0) * bytes_per_sample;
        dst_plane[dst_row_start..dst_row_start + row_bytes]
            .copy_from_slice(&src_plane[src_row_start..src_row_start + row_bytes]);
    }
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
        b[layout.y_bytes] = 1;

        assert_ne!(
            layout.hash_region(&a, 0, 0, 8, 8),
            layout.hash_region(&b, 0, 0, 8, 8)
        );
        a[layout.y_bytes] = 1;
        assert_eq!(
            layout.hash_region(&a, 0, 0, 8, 8),
            layout.hash_region(&b, 0, 0, 8, 8)
        );
    }
}
