#[derive(Debug, Default, Clone)]
pub struct BitWriter {
    bytes: Vec<u8>,
    current_byte: u8,
    bits_filled: u8,
}

impl BitWriter {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn write_bit(&mut self, bit: bool) {
        self.current_byte <<= 1;
        if bit {
            self.current_byte |= 1;
        }
        self.bits_filled += 1;

        if self.bits_filled == 8 {
            self.bytes.push(self.current_byte);
            self.current_byte = 0;
            self.bits_filled = 0;
        }
    }

    pub fn write_bool(&mut self, value: bool) {
        self.write_bit(value);
    }

    pub fn write_bits(&mut self, value: u64, bit_count: u8) {
        assert!(bit_count <= 64, "cannot write more than 64 bits at once");
        for bit_index in (0..bit_count).rev() {
            self.write_bit(((value >> bit_index) & 1) != 0);
        }
    }

    pub fn byte_align_zero(&mut self) {
        if self.bits_filled == 0 {
            return;
        }
        while self.bits_filled != 0 {
            self.write_bit(false);
        }
    }

    pub fn into_bytes(mut self) -> Vec<u8> {
        self.byte_align_zero();
        self.bytes
    }

    pub fn is_byte_aligned(&self) -> bool {
        self.bits_filled == 0
    }
}

pub fn rbsp_trailing_bits(writer: &mut BitWriter) {
    writer.write_bit(true);
    writer.byte_align_zero();
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyntaxField<C> {
    pub name: &'static str,
    pub code: C,
    pub bit_offset: usize,
    pub bit_count: usize,
}

#[derive(Debug, Clone)]
pub struct SyntaxBitWriter<C> {
    writer: BitWriter,
    fields: Vec<SyntaxField<C>>,
    bit_offset: usize,
}

impl<C> Default for SyntaxBitWriter<C> {
    fn default() -> Self {
        Self {
            writer: BitWriter::new(),
            fields: Vec::new(),
            bit_offset: 0,
        }
    }
}

impl<C: Copy> SyntaxBitWriter<C> {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn record_field(&mut self, name: &'static str, code: C, bit_count: usize) {
        self.fields.push(SyntaxField {
            name,
            code,
            bit_offset: self.bit_offset,
            bit_count,
        });
    }

    pub fn write_bit(&mut self, bit: bool) {
        self.writer.write_bit(bit);
        self.bit_offset += 1;
    }

    pub fn write_bool(&mut self, value: bool) {
        self.write_bit(value);
    }

    pub fn write_bits(&mut self, value: u64, bit_count: u8) {
        self.writer.write_bits(value, bit_count);
        self.bit_offset += bit_count as usize;
    }

    pub fn write_field_bool(&mut self, name: &'static str, code: C, value: bool) {
        self.record_field(name, code, 1);
        self.write_bool(value);
    }

    pub fn write_field_bits(&mut self, name: &'static str, code: C, value: u64, bit_count: u8) {
        self.record_field(name, code, bit_count as usize);
        self.write_bits(value, bit_count);
    }

    pub fn write_field_bit_slice(&mut self, name: &'static str, code: C, bits: &[bool]) {
        self.record_field(name, code, bits.len());
        for bit in bits {
            self.write_bit(*bit);
        }
    }

    pub fn write_byte_align_zero(&mut self, name: &'static str, code: C) {
        let remainder = self.bit_offset % 8;
        if remainder == 0 {
            return;
        }
        let bit_count = 8 - remainder;
        self.record_field(name, code, bit_count);
        self.writer.byte_align_zero();
        self.bit_offset += bit_count;
    }

    pub fn write_trailing_one_zero_bits(&mut self, name: &'static str, code: C) {
        let bit_count = if self.writer.is_byte_aligned() {
            8
        } else {
            8 - (self.bit_offset % 8)
        };
        self.record_field(name, code, bit_count);
        rbsp_trailing_bits(&mut self.writer);
        self.bit_offset += bit_count;
    }

    pub fn is_byte_aligned(&self) -> bool {
        self.writer.is_byte_aligned()
    }

    pub fn fields(&self) -> &[SyntaxField<C>] {
        &self.fields
    }

    pub fn into_bytes(self) -> Vec<u8> {
        self.writer.into_bytes()
    }

    pub fn into_parts(self) -> (Vec<u8>, Vec<SyntaxField<C>>) {
        (self.writer.into_bytes(), self.fields)
    }
}

pub fn insert_emulation_prevention_bytes(rbsp: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(rbsp.len());
    let mut zero_count = 0usize;

    for &byte in rbsp {
        if zero_count >= 2 && byte <= 0x03 {
            out.push(0x03);
            zero_count = 0;
        }

        out.push(byte);
        if byte == 0 {
            zero_count += 1;
        } else {
            zero_count = 0;
        }
    }

    out
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NalUnitKind {
    /// Generic coded-picture category for legacy/shared Annex-B tests.
    CodedPicture,
    /// Generic parameter-set category for legacy/shared Annex-B tests.
    ParameterSet,
    /// Non-codec project metadata payload, not a conforming video bitstream.
    FrameForgePlaceholder,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NalUnit {
    pub kind: NalUnitKind,
    pub temporal_id: u8,
    pub payload: Vec<u8>,
}

#[derive(Debug, Default)]
pub struct AnnexBWriter {
    units: Vec<NalUnit>,
}

impl AnnexBWriter {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, unit: NalUnit) {
        self.units.push(unit);
    }

    pub fn into_bytes(self) -> Vec<u8> {
        let mut out = Vec::new();
        for unit in self.units {
            out.extend_from_slice(&[0x00, 0x00, 0x00, 0x01]);
            out.extend_from_slice(&placeholder_nal_header(unit.kind, unit.temporal_id));
            out.extend_from_slice(&insert_emulation_prevention_bytes(&unit.payload));
        }
        out
    }
}

fn placeholder_nal_header(kind: NalUnitKind, temporal_id: u8) -> [u8; 2] {
    // This shared helper writes project-local placeholder headers. Codec
    // modules that need conforming NAL headers own their exact header packing.
    let kind_code = match kind {
        NalUnitKind::CodedPicture => 1,
        NalUnitKind::ParameterSet => 2,
        NalUnitKind::FrameForgePlaceholder => 63,
    };
    [kind_code, temporal_id & 0x07]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bit_packing_msb_first() {
        let mut writer = BitWriter::new();
        writer.write_bits(0b101, 3);
        writer.write_bits(0b10, 2);
        assert_eq!(writer.into_bytes(), vec![0b1011_0000]);
    }

    #[test]
    fn byte_alignment_writes_zero_padding() {
        let mut writer = BitWriter::new();
        writer.write_bits(0b1111, 4);
        writer.byte_align_zero();
        assert!(writer.is_byte_aligned());
        assert_eq!(writer.into_bytes(), vec![0b1111_0000]);
    }

    #[test]
    fn rbsp_trailing_bits_adds_stop_bit_and_padding() {
        let mut writer = BitWriter::new();
        writer.write_bits(0b1010, 4);
        rbsp_trailing_bits(&mut writer);
        assert_eq!(writer.into_bytes(), vec![0b1010_1000]);
    }

    #[test]
    fn syntax_bit_writer_records_field_offsets() {
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        enum Code {
            Flag,
            Bits,
            Align,
            Trailing,
        }

        let mut writer = SyntaxBitWriter::new();
        writer.write_field_bool("flag", Code::Flag, true);
        writer.write_field_bits("bits", Code::Bits, 0b010, 3);
        writer.write_byte_align_zero("align", Code::Align);
        writer.write_trailing_one_zero_bits("trailing", Code::Trailing);
        let (bytes, fields) = writer.into_parts();

        assert_eq!(bytes, vec![0b1010_0000, 0b1000_0000]);
        assert_eq!(
            fields,
            vec![
                SyntaxField {
                    name: "flag",
                    code: Code::Flag,
                    bit_offset: 0,
                    bit_count: 1,
                },
                SyntaxField {
                    name: "bits",
                    code: Code::Bits,
                    bit_offset: 1,
                    bit_count: 3,
                },
                SyntaxField {
                    name: "align",
                    code: Code::Align,
                    bit_offset: 4,
                    bit_count: 4,
                },
                SyntaxField {
                    name: "trailing",
                    code: Code::Trailing,
                    bit_offset: 8,
                    bit_count: 8,
                },
            ]
        );
    }

    #[test]
    fn emulation_prevention_inserts_after_two_zeroes() {
        let rbsp = [0x00, 0x00, 0x01, 0x11, 0x00, 0x00, 0x03];
        assert_eq!(
            insert_emulation_prevention_bytes(&rbsp),
            vec![0x00, 0x00, 0x03, 0x01, 0x11, 0x00, 0x00, 0x03, 0x03]
        );
    }
}
