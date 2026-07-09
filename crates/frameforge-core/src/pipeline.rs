use crate::error::Result;
use crate::{Frame, Packet};

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
