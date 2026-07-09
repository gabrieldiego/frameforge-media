#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct StreamId(pub u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Timestamp(pub i64);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Packet {
    pub stream_id: StreamId,
    pub pts: Option<Timestamp>,
    pub data: Vec<u8>,
}

impl Packet {
    pub fn new(stream_id: StreamId, pts: Option<Timestamp>, data: Vec<u8>) -> Self {
        Self {
            stream_id,
            pts,
            data,
        }
    }
}
