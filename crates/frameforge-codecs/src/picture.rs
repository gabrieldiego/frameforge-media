use std::io::{ErrorKind, Read};

pub(crate) use frameforge_core::{ChromaSampling, PixelFormat, SampleBitDepth};

pub(crate) struct Picture;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PlanarYuvGeometry {
    bytes_per_sample: usize,
    luma_samples: usize,
    chroma_width: usize,
    chroma_height: usize,
    chroma_samples: usize,
    frame_len: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PlanarYuvPlane {
    Y,
    U,
    V,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PlanarYuvFrameLayout {
    width: usize,
    height: usize,
    chroma_sampling: ChromaSampling,
    geometry: PlanarYuvGeometry,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FrameLimit {
    UntilEof,
    Exact(usize),
}

impl FrameLimit {
    pub(crate) fn from_frame_count(frames: usize) -> Self {
        if frames == 0 {
            Self::UntilEof
        } else {
            Self::Exact(frames)
        }
    }

    pub(crate) fn should_read(self, frame_index: usize) -> bool {
        match self {
            Self::UntilEof => true,
            Self::Exact(frames) => frame_index < frames,
        }
    }

    pub(crate) fn metric_count(self) -> usize {
        match self {
            Self::UntilEof => 0,
            Self::Exact(frames) => frames,
        }
    }
}

impl Picture {
    pub(crate) fn expected_len(width: usize, height: usize, format: PixelFormat) -> usize {
        Self::checked_len(width, height, format).expect("picture dimensions overflow usize")
    }

    pub(crate) fn checked_len(width: usize, height: usize, format: PixelFormat) -> Option<usize> {
        format.frame_len(width, height)
    }

    pub(crate) fn validate_shape(
        width: usize,
        height: usize,
        format: PixelFormat,
    ) -> Result<(), String> {
        format
            .validate_geometry(width, height)
            .map_err(|err| err.to_string())
    }

    pub(crate) fn validate_format_shape<T>(
        width: usize,
        height: usize,
        format: PixelFormat,
        validate_format: impl FnOnce(PixelFormat) -> Result<T, String>,
    ) -> Result<T, String> {
        let validated = validate_format(format)?;
        Self::validate_shape(width, height, format)?;
        Ok(validated)
    }
}

impl PlanarYuvGeometry {
    pub(crate) fn new(
        width: usize,
        height: usize,
        chroma_sampling: ChromaSampling,
        bit_depth: SampleBitDepth,
    ) -> Result<Self, String> {
        if chroma_sampling == ChromaSampling::Monochrome {
            return Err("planar YUV geometry expects chroma planes".to_string());
        }
        let format = PixelFormat::planar_yuv(chroma_sampling, bit_depth);
        Picture::validate_shape(width, height, format)?;
        Ok(Self::for_validated_shape(
            width,
            height,
            chroma_sampling,
            bit_depth,
        ))
    }

    pub(crate) fn for_validated_shape(
        width: usize,
        height: usize,
        chroma_sampling: ChromaSampling,
        bit_depth: SampleBitDepth,
    ) -> Self {
        debug_assert_ne!(chroma_sampling, ChromaSampling::Monochrome);
        debug_assert!(
            Picture::validate_shape(
                width,
                height,
                PixelFormat::planar_yuv(chroma_sampling, bit_depth)
            )
            .is_ok(),
            "planar YUV geometry expects validated dimensions"
        );
        Self::from_parts(width, height, chroma_sampling, bit_depth)
    }

    fn from_parts(
        width: usize,
        height: usize,
        chroma_sampling: ChromaSampling,
        bit_depth: SampleBitDepth,
    ) -> Self {
        let format = PixelFormat::planar_yuv(chroma_sampling, bit_depth);
        let luma_samples = width
            .checked_mul(height)
            .expect("validated planar YUV luma sample count fits usize");
        let sub_x = chroma_subsample_x(chroma_sampling);
        let sub_y = chroma_subsample_y(chroma_sampling);
        let chroma_width = width / sub_x;
        let chroma_height = height / sub_y;
        let chroma_samples = chroma_width
            .checked_mul(chroma_height)
            .expect("validated planar YUV chroma sample count fits usize");
        let frame_len = Picture::expected_len(width, height, format);
        Self {
            bytes_per_sample: bit_depth.bytes_per_sample(),
            luma_samples,
            chroma_width,
            chroma_height,
            chroma_samples,
            frame_len,
        }
    }

    pub(crate) fn bytes_per_sample(self) -> usize {
        self.bytes_per_sample
    }

    pub(crate) fn luma_samples(self) -> usize {
        self.luma_samples
    }

    #[cfg(any(feature = "av2", test))]
    pub(crate) fn chroma_width(self) -> usize {
        self.chroma_width
    }

    #[cfg(test)]
    pub(crate) fn chroma_height(self) -> usize {
        self.chroma_height
    }

    pub(crate) fn chroma_samples(self) -> usize {
        self.chroma_samples
    }

    pub(crate) fn frame_len(self) -> usize {
        self.frame_len
    }
}

impl PlanarYuvFrameLayout {
    pub(crate) fn new(
        width: usize,
        height: usize,
        chroma_sampling: ChromaSampling,
        bit_depth: SampleBitDepth,
    ) -> Result<Self, String> {
        let geometry = PlanarYuvGeometry::new(width, height, chroma_sampling, bit_depth)?;
        Ok(Self::from_geometry(
            width,
            height,
            chroma_sampling,
            geometry,
        ))
    }

    pub(crate) fn for_validated_shape(
        width: usize,
        height: usize,
        chroma_sampling: ChromaSampling,
        bit_depth: SampleBitDepth,
    ) -> Self {
        let geometry =
            PlanarYuvGeometry::for_validated_shape(width, height, chroma_sampling, bit_depth);
        Self::from_geometry(width, height, chroma_sampling, geometry)
    }

    fn from_geometry(
        width: usize,
        height: usize,
        chroma_sampling: ChromaSampling,
        geometry: PlanarYuvGeometry,
    ) -> Self {
        Self {
            width,
            height,
            chroma_sampling,
            geometry,
        }
    }

    pub(crate) fn bytes_per_sample(self) -> usize {
        self.geometry.bytes_per_sample
    }

    pub(crate) fn frame_len(self) -> usize {
        self.geometry.frame_len
    }

    pub(crate) fn luma_samples(self) -> usize {
        self.geometry.luma_samples
    }

    pub(crate) fn chroma_samples(self) -> usize {
        self.geometry.chroma_samples
    }

    pub(crate) fn luma_byte_len(self) -> usize {
        self.geometry.luma_samples * self.geometry.bytes_per_sample
    }

    pub(crate) fn chroma_byte_len(self) -> usize {
        self.geometry.chroma_samples * self.geometry.bytes_per_sample
    }

    pub(crate) fn plane_slices<'a>(self, frame: &'a [u8]) -> (&'a [u8], &'a [u8], &'a [u8]) {
        debug_assert_eq!(frame.len(), self.frame_len());
        let y_len = self.luma_byte_len();
        let c_len = self.chroma_byte_len();
        let y = &frame[..y_len];
        let u_start = y_len;
        let v_start = y_len + c_len;
        let u = &frame[u_start..v_start];
        let v = &frame[v_start..v_start + c_len];
        (y, u, v)
    }

    pub(crate) fn plane_slices_mut<'a>(
        self,
        frame: &'a mut [u8],
    ) -> (&'a mut [u8], &'a mut [u8], &'a mut [u8]) {
        debug_assert_eq!(frame.len(), self.frame_len());
        let y_len = self.luma_byte_len();
        let c_len = self.chroma_byte_len();
        let (y, chroma) = frame.split_at_mut(y_len);
        let (u, v) = chroma.split_at_mut(c_len);
        (y, u, &mut v[..c_len])
    }

    pub(crate) fn plane_dimensions(self, plane: PlanarYuvPlane) -> (usize, usize) {
        match plane {
            PlanarYuvPlane::Y => (self.width, self.height),
            PlanarYuvPlane::U | PlanarYuvPlane::V => {
                (self.geometry.chroma_width, self.geometry.chroma_height)
            }
        }
    }

    pub(crate) fn plane_stride(self, plane: PlanarYuvPlane) -> usize {
        self.plane_dimensions(plane).0
    }

    pub(crate) fn plane_subsampling(self, plane: PlanarYuvPlane) -> (usize, usize) {
        match plane {
            PlanarYuvPlane::Y => (1, 1),
            PlanarYuvPlane::U | PlanarYuvPlane::V => (
                chroma_subsample_x(self.chroma_sampling),
                chroma_subsample_y(self.chroma_sampling),
            ),
        }
    }

    pub(crate) fn sample_offset(self, plane: PlanarYuvPlane, x: usize, y: usize) -> usize {
        match plane {
            PlanarYuvPlane::Y => y * self.width + x,
            PlanarYuvPlane::U => self.geometry.luma_samples + y * self.geometry.chroma_width + x,
            PlanarYuvPlane::V => {
                self.geometry.luma_samples
                    + self.geometry.chroma_samples
                    + y * self.geometry.chroma_width
                    + x
            }
        }
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
        if current.len() != self.frame_len() || reference.len() != self.frame_len() {
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
            self.width,
            x0,
            y0,
            reference_y,
            self.width,
            ref_x0,
            ref_y0,
            width,
            height,
            self.geometry.bytes_per_sample,
        ) {
            return false;
        }

        let (sub_x, sub_y) = self.plane_subsampling(PlanarYuvPlane::U);
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
                self.geometry.chroma_width,
                x0 / sub_x,
                y0 / sub_y,
                reference_plane,
                self.geometry.chroma_width,
                ref_x0 / sub_x,
                ref_y0 / sub_y,
                chroma_width,
                chroma_height,
                self.geometry.bytes_per_sample,
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
        if dst.len() != self.frame_len() || src.len() != self.frame_len() {
            return false;
        }
        if !self.luma_region_in_bounds(x0, y0, width, height)
            || !self.luma_region_in_bounds(ref_x0, ref_y0, width, height)
        {
            return false;
        }

        let (sub_x, sub_y) = self.plane_subsampling(PlanarYuvPlane::U);
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
            self.width,
            x0,
            y0,
            src_y,
            self.width,
            ref_x0,
            ref_y0,
            width,
            height,
            self.geometry.bytes_per_sample,
        );

        let chroma_width = width / sub_x;
        let chroma_height = height / sub_y;
        for (dst_plane, src_plane) in [(dst_u, src_u), (dst_v, src_v)] {
            copy_plane_region_between(
                dst_plane,
                self.geometry.chroma_width,
                x0 / sub_x,
                y0 / sub_y,
                src_plane,
                self.geometry.chroma_width,
                ref_x0 / sub_x,
                ref_y0 / sub_y,
                chroma_width,
                chroma_height,
                self.geometry.bytes_per_sample,
            );
        }
        true
    }

    fn luma_region_in_bounds(self, x0: usize, y0: usize, width: usize, height: usize) -> bool {
        x0.checked_add(width).is_some_and(|x1| x1 <= self.width)
            && y0.checked_add(height).is_some_and(|y1| y1 <= self.height)
    }
}

pub(crate) fn chroma_subsample_x(chroma_sampling: ChromaSampling) -> usize {
    chroma_sampling.subsample_x()
}

pub(crate) fn chroma_subsample_y(chroma_sampling: ChromaSampling) -> usize {
    chroma_sampling.subsample_y()
}

#[inline(always)]
pub(crate) fn read_planar_sample(
    buffer: &[u8],
    sample_index: usize,
    bit_depth: SampleBitDepth,
) -> u16 {
    if bit_depth.bits() <= 8 {
        debug_assert!(sample_index < buffer.len());
        return u16::from(buffer[sample_index]);
    }

    let offset = sample_index * 2;
    debug_assert!(offset + 1 < buffer.len());
    u16::from_le_bytes([buffer[offset], buffer[offset + 1]])
}

#[inline(always)]
pub(crate) fn write_planar_sample(
    buffer: &mut [u8],
    sample_index: usize,
    sample: u16,
    bit_depth: SampleBitDepth,
) {
    debug_assert!(sample <= bit_depth.max_sample());
    if bit_depth.bits() <= 8 {
        debug_assert!(sample_index < buffer.len());
        buffer[sample_index] = sample as u8;
        return;
    }

    let offset = sample_index * 2;
    debug_assert!(offset + 1 < buffer.len());
    let bytes = sample.to_le_bytes();
    buffer[offset] = bytes[0];
    buffer[offset + 1] = bytes[1];
}

pub(crate) fn unpack_planar_samples(input: &[u8], output: &mut [u16], bit_depth: SampleBitDepth) {
    debug_assert_eq!(input.len(), output.len() * bit_depth.bytes_per_sample());
    if bit_depth.bits() <= 8 {
        for (dst, &sample) in output.iter_mut().zip(input) {
            *dst = u16::from(sample);
        }
    } else {
        let max_sample = bit_depth.max_sample();
        for (dst, sample) in output.iter_mut().zip(input.chunks_exact(2)) {
            *dst = u16::from_le_bytes([sample[0], sample[1]]).min(max_sample);
        }
    }
}

pub(crate) fn pack_planar_samples(samples: &[u16], output: &mut [u8], bit_depth: SampleBitDepth) {
    debug_assert_eq!(output.len(), samples.len() * bit_depth.bytes_per_sample());
    if bit_depth.bits() <= 8 {
        for (dst, &sample) in output.iter_mut().zip(samples) {
            debug_assert!(sample <= bit_depth.max_sample());
            *dst = sample as u8;
        }
    } else {
        for (dst, &sample) in output.chunks_exact_mut(2).zip(samples) {
            debug_assert!(sample <= bit_depth.max_sample());
            let bytes = sample.to_le_bytes();
            dst[0] = bytes[0];
            dst[1] = bytes[1];
        }
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

pub(crate) fn read_input_frame<R: Read + ?Sized>(
    input: &mut R,
    frame: &mut [u8],
    frame_index: usize,
    frame_limit: FrameLimit,
    context: &str,
) -> Result<bool, String> {
    if let FrameLimit::Exact(requested_frames) = frame_limit {
        input.read_exact(frame).map_err(|err| {
            format!(
                "failed to read {context} frame {} of {}: {err}",
                frame_index + 1,
                requested_frames
            )
        })?;
        return Ok(true);
    }

    let mut filled = 0usize;
    while filled < frame.len() {
        match input.read(&mut frame[filled..]) {
            Ok(0) if filled == 0 => return Ok(false),
            Ok(0) => {
                return Err(format!(
                    "failed to read complete {context} frame {}: EOF after {} of {} byte(s)",
                    frame_index + 1,
                    filled,
                    frame.len()
                ));
            }
            Ok(read) => filled += read,
            Err(err) if err.kind() == ErrorKind::Interrupted => {}
            Err(err) => {
                return Err(format!(
                    "failed to read {context} frame {}: {err}",
                    frame_index + 1
                ));
            }
        }
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn planar_yuv_geometry_derives_422_lengths() {
        let bit_depth = SampleBitDepth::new(10).unwrap();
        let geometry = PlanarYuvGeometry::new(16, 8, ChromaSampling::Cs422, bit_depth).unwrap();

        assert_eq!(geometry.bytes_per_sample(), 2);
        assert_eq!(geometry.luma_samples(), 128);
        assert_eq!(geometry.chroma_width(), 8);
        assert_eq!(geometry.chroma_height(), 8);
        assert_eq!(geometry.chroma_samples(), 64);
        assert_eq!(geometry.frame_len(), 512);
    }

    #[test]
    fn planar_yuv_geometry_rejects_incompatible_sampling_shape() {
        let bit_depth = SampleBitDepth::new(8).unwrap();

        assert!(
            PlanarYuvGeometry::new(15, 8, ChromaSampling::Cs422, bit_depth).is_err(),
            "4:2:2 chroma requires even luma width"
        );
        assert!(
            PlanarYuvGeometry::new(16, 8, ChromaSampling::Monochrome, bit_depth).is_err(),
            "planar YUV geometry requires chroma planes"
        );
    }
}
