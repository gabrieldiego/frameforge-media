use crate::bitstream::{SyntaxBitWriter, SyntaxField};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Av2SyntaxCode {
    Flag,
    Literal,
    Uvlc,
    RiceGolomb,
    Quniform,
    TrailingBits,
    ByteAlignZero,
    TileEntropyPayload,
}

pub type Av2SyntaxField = SyntaxField<Av2SyntaxCode>;

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Av2SyntaxPayload {
    pub bytes: Vec<u8>,
    pub fields: Vec<Av2SyntaxField>,
}

#[derive(Debug, Default, Clone)]
pub struct Av2SyntaxWriter {
    writer: SyntaxBitWriter<Av2SyntaxCode>,
}

impl Av2SyntaxWriter {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn write_flag(&mut self, name: &'static str, value: bool) {
        self.writer
            .write_field_bool(name, Av2SyntaxCode::Flag, value);
    }

    pub fn write_literal(&mut self, name: &'static str, value: u64, bit_count: u8) {
        assert!(bit_count <= 64, "literal cannot write more than 64 bits");
        if bit_count < 64 {
            assert!(
                value < (1u64 << bit_count),
                "value does not fit in literal({bit_count})"
            );
        }
        self.writer
            .write_field_bits(name, Av2SyntaxCode::Literal, value, bit_count);
    }

    pub fn write_uvlc(&mut self, name: &'static str, value: u32) {
        assert!(value != u32::MAX, "uvlc cannot encode UINT32_MAX");
        let code_num = value as u64 + 1;
        let bit_count = 64 - code_num.leading_zeros() as u8;
        let leading_zero_bits = bit_count - 1;
        let total_bits = (leading_zero_bits * 2) + 1;
        self.writer
            .record_field(name, Av2SyntaxCode::Uvlc, total_bits as usize);
        for _ in 0..leading_zero_bits {
            self.writer.write_bit(false);
        }
        self.writer.write_bits(code_num, bit_count);
    }

    pub fn write_rice_golomb(&mut self, name: &'static str, value: u32, k: u8) {
        assert!(k <= 26, "AV2 Rice-Golomb k must be at most 26");
        let quotient = value >> k;
        assert!(
            quotient < 32,
            "AV2 Rice-Golomb quotient must be lower than 32"
        );
        let bit_count = quotient as usize + 1 + k as usize;
        self.writer
            .record_field(name, Av2SyntaxCode::RiceGolomb, bit_count);
        for _ in 0..quotient {
            self.writer.write_bit(true);
        }
        self.writer.write_bit(false);
        self.writer
            .write_bits(u64::from(value & ((1u32 << k) - 1)), k);
    }

    pub fn write_quniform(&mut self, name: &'static str, n: u16, value: u16) {
        if n <= 1 {
            return;
        }
        assert!(value < n, "quniform value must be lower than n");
        let l = 16 - n.leading_zeros() as u8;
        let m = (1u16 << l) - n;
        let bit_count = if value < m { l - 1 } else { l };
        self.writer
            .record_field(name, Av2SyntaxCode::Quniform, bit_count as usize);
        if value < m {
            self.writer.write_bits(value as u64, l - 1);
        } else {
            self.writer
                .write_bits((m + ((value - m) >> 1)) as u64, l - 1);
            self.writer.write_bit(((value - m) & 1) != 0);
        }
    }

    pub fn trailing_bits(&mut self) {
        self.writer
            .write_trailing_one_zero_bits("trailing_bits", Av2SyntaxCode::TrailingBits);
    }

    pub fn byte_align_zero(&mut self, name: &'static str) {
        self.writer
            .write_byte_align_zero(name, Av2SyntaxCode::ByteAlignZero);
    }

    pub fn finish(self) -> Av2SyntaxPayload {
        let (bytes, fields) = self.writer.into_parts();
        Av2SyntaxPayload { bytes, fields }
    }
}
