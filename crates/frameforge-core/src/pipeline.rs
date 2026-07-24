use crate::error::Result;
use crate::{Frame, Packet};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct FilterPipelineStats {
    pub input_frames: usize,
    pub output_frames: usize,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct EncodePipelineStats {
    pub input_frames: usize,
    pub encoded_frames: usize,
    pub output_packets: usize,
}

pub trait Source {
    type Output;

    fn pull(&mut self) -> Result<Option<Self::Output>>;
}

pub trait Sink<I> {
    fn push(&mut self, input: I) -> Result<()>;

    fn finish(&mut self) -> Result<()> {
        Ok(())
    }
}

pub trait Decoder {
    fn decode(&mut self, packet: Packet) -> Result<Vec<Frame>>;

    fn finish(&mut self) -> Result<Vec<Frame>> {
        Ok(Vec::new())
    }
}

pub trait Filter {
    fn process(&mut self, frame: Frame) -> Result<Vec<Frame>>;

    fn finish(&mut self) -> Result<Vec<Frame>> {
        Ok(Vec::new())
    }
}

pub trait Encoder {
    fn encode(&mut self, frame: Frame) -> Result<Vec<Packet>>;

    fn finish(&mut self) -> Result<Vec<Packet>> {
        Ok(Vec::new())
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct IdentityFilter;

impl Filter for IdentityFilter {
    fn process(&mut self, frame: Frame) -> Result<Vec<Frame>> {
        Ok(vec![frame])
    }
}

pub fn run_frame_filter_pipeline(
    source: &mut dyn Source<Output = Frame>,
    filters: &mut [&mut dyn Filter],
    sink: &mut dyn Sink<Frame>,
) -> Result<FilterPipelineStats> {
    let mut stats = FilterPipelineStats::default();
    while let Some(frame) = source.pull()? {
        stats.input_frames += 1;
        let frames = process_frames_from(vec![frame], 0, filters)?;
        stats.output_frames += push_frames(frames, sink)?;
    }

    for filter_index in 0..filters.len() {
        let frames = filters[filter_index].finish()?;
        let frames = process_frames_from(frames, filter_index + 1, filters)?;
        stats.output_frames += push_frames(frames, sink)?;
    }

    sink.finish()?;
    Ok(stats)
}

pub fn run_frame_encode_pipeline(
    source: &mut dyn Source<Output = Frame>,
    filters: &mut [&mut dyn Filter],
    encoder: &mut dyn Encoder,
    sink: &mut dyn Sink<Packet>,
) -> Result<EncodePipelineStats> {
    let mut stats = EncodePipelineStats::default();
    while let Some(frame) = source.pull()? {
        stats.input_frames += 1;
        let frames = process_frames_from(vec![frame], 0, filters)?;
        stats.encoded_frames += encode_frames(frames, encoder, sink, &mut stats)?;
    }

    for filter_index in 0..filters.len() {
        let frames = filters[filter_index].finish()?;
        let frames = process_frames_from(frames, filter_index + 1, filters)?;
        stats.encoded_frames += encode_frames(frames, encoder, sink, &mut stats)?;
    }

    for packet in encoder.finish()? {
        stats.output_packets += 1;
        sink.push(packet)?;
    }
    sink.finish()?;
    Ok(stats)
}

fn process_frames_from(
    mut frames: Vec<Frame>,
    start: usize,
    filters: &mut [&mut dyn Filter],
) -> Result<Vec<Frame>> {
    for filter in filters.iter_mut().skip(start) {
        let mut next = Vec::new();
        for frame in frames {
            next.extend(filter.process(frame)?);
        }
        frames = next;
    }
    Ok(frames)
}

fn push_frames(frames: Vec<Frame>, sink: &mut dyn Sink<Frame>) -> Result<usize> {
    let count = frames.len();
    for frame in frames {
        sink.push(frame)?;
    }
    Ok(count)
}

fn encode_frames(
    frames: Vec<Frame>,
    encoder: &mut dyn Encoder,
    sink: &mut dyn Sink<Packet>,
    stats: &mut EncodePipelineStats,
) -> Result<usize> {
    let frame_count = frames.len();
    for frame in frames {
        for packet in encoder.encode(frame)? {
            stats.output_packets += 1;
            sink.push(packet)?;
        }
    }
    Ok(frame_count)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{FrameInfo, MediaError, PixelFormat, StreamId};

    struct VecFrameSource {
        frames: std::vec::IntoIter<Frame>,
    }

    impl VecFrameSource {
        fn new(frames: Vec<Frame>) -> Self {
            Self {
                frames: frames.into_iter(),
            }
        }
    }

    impl Source for VecFrameSource {
        type Output = Frame;

        fn pull(&mut self) -> Result<Option<Self::Output>> {
            Ok(self.frames.next())
        }
    }

    #[derive(Default)]
    struct VecFrameSink {
        frames: Vec<Frame>,
        finished: bool,
    }

    impl Sink<Frame> for VecFrameSink {
        fn push(&mut self, input: Frame) -> Result<()> {
            self.frames.push(input);
            Ok(())
        }

        fn finish(&mut self) -> Result<()> {
            self.finished = true;
            Ok(())
        }
    }

    #[derive(Default)]
    struct VecPacketSink {
        packets: Vec<Packet>,
        finished: bool,
    }

    impl Sink<Packet> for VecPacketSink {
        fn push(&mut self, input: Packet) -> Result<()> {
            self.packets.push(input);
            Ok(())
        }

        fn finish(&mut self) -> Result<()> {
            self.finished = true;
            Ok(())
        }
    }

    struct DuplicateFilter;

    impl Filter for DuplicateFilter {
        fn process(&mut self, frame: Frame) -> Result<Vec<Frame>> {
            Ok(vec![frame.clone(), frame])
        }
    }

    struct FlushingFilter {
        info: FrameInfo,
    }

    impl Filter for FlushingFilter {
        fn process(&mut self, frame: Frame) -> Result<Vec<Frame>> {
            Ok(vec![frame])
        }

        fn finish(&mut self) -> Result<Vec<Frame>> {
            Ok(vec![Frame::blank(self.info)])
        }
    }

    struct PacketizingEncoder {
        next_pts: i64,
    }

    impl Encoder for PacketizingEncoder {
        fn encode(&mut self, frame: Frame) -> Result<Vec<Packet>> {
            let packet = Packet::new(
                StreamId(0),
                Some(crate::Timestamp(self.next_pts)),
                frame.into_data(),
            );
            self.next_pts += 1;
            Ok(vec![packet])
        }

        fn finish(&mut self) -> Result<Vec<Packet>> {
            Ok(vec![Packet::new(StreamId(0), None, b"eos".to_vec())])
        }
    }

    fn test_frame(fill: u8) -> Frame {
        let info = FrameInfo::new(2, 2, PixelFormat::Rgb24).unwrap();
        Frame::new(info, vec![fill; info.expected_len()]).unwrap()
    }

    #[test]
    fn filter_pipeline_runs_filters_and_flushes_in_order() {
        let info = FrameInfo::new(2, 2, PixelFormat::Rgb24).unwrap();
        let mut source = VecFrameSource::new(vec![test_frame(7)]);
        let mut duplicate = DuplicateFilter;
        let mut flush = FlushingFilter { info };
        let mut filters: Vec<&mut dyn Filter> = vec![&mut duplicate, &mut flush];
        let mut sink = VecFrameSink::default();

        let stats =
            run_frame_filter_pipeline(&mut source, filters.as_mut_slice(), &mut sink).unwrap();

        assert_eq!(
            stats,
            FilterPipelineStats {
                input_frames: 1,
                output_frames: 3,
            }
        );
        assert_eq!(sink.frames.len(), 3);
        assert!(sink.finished);
        assert_eq!(sink.frames[0].data(), &[7; 12]);
        assert_eq!(sink.frames[2].data(), &[0; 12]);
    }

    #[test]
    fn encode_pipeline_pushes_encoder_packets_and_finish_packet() {
        let mut source = VecFrameSource::new(vec![test_frame(1), test_frame(2)]);
        let mut identity = IdentityFilter;
        let mut filters: Vec<&mut dyn Filter> = vec![&mut identity];
        let mut encoder = PacketizingEncoder { next_pts: 0 };
        let mut sink = VecPacketSink::default();

        let stats =
            run_frame_encode_pipeline(&mut source, filters.as_mut_slice(), &mut encoder, &mut sink)
                .unwrap();

        assert_eq!(
            stats,
            EncodePipelineStats {
                input_frames: 2,
                encoded_frames: 2,
                output_packets: 3,
            }
        );
        assert_eq!(sink.packets.len(), 3);
        assert!(sink.finished);
        assert_eq!(sink.packets[0].pts, Some(crate::Timestamp(0)));
        assert_eq!(sink.packets[2].data, b"eos");
    }

    #[test]
    fn identity_filter_preserves_frame() {
        let frame = test_frame(9);
        let mut filter = IdentityFilter;
        let out = filter.process(frame.clone()).unwrap();
        assert_eq!(out, vec![frame]);
    }

    #[test]
    fn source_errors_stop_pipeline_before_sink_finish() {
        struct FailingSource;

        impl Source for FailingSource {
            type Output = Frame;

            fn pull(&mut self) -> Result<Option<Self::Output>> {
                Err(MediaError::Message("source failed".to_string()))
            }
        }

        let mut source = FailingSource;
        let mut sink = VecFrameSink::default();
        let err = run_frame_filter_pipeline(&mut source, &mut [], &mut sink).unwrap_err();

        assert_eq!(err.to_string(), "source failed");
        assert!(!sink.finished);
    }
}
