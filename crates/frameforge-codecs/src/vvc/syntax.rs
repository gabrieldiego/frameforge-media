use crate::bitstream::{SyntaxBitWriter, SyntaxField};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VvcSyntaxCode {
    Flag,
    U,
    Ue,
    Se,
    ByteAlignZero,
    CabacToken,
    RbspTrailingBits,
}

pub type VvcSyntaxField = SyntaxField<VvcSyntaxCode>;

#[derive(Debug, Default, Clone)]
pub struct VvcSyntaxWriter {
    writer: SyntaxBitWriter<VvcSyntaxCode>,
}

impl VvcSyntaxWriter {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn write_flag(&mut self, name: &'static str, value: bool) {
        self.writer
            .write_field_bool(name, VvcSyntaxCode::Flag, value);
    }

    pub fn write_u(&mut self, name: &'static str, value: u64, bit_count: u8) {
        assert!(bit_count <= 64, "u(n) cannot write more than 64 bits");
        if bit_count < 64 {
            assert!(
                value < (1u64 << bit_count),
                "value does not fit in u({bit_count})"
            );
        }
        self.writer
            .write_field_bits(name, VvcSyntaxCode::U, value, bit_count);
    }

    pub fn write_ue(&mut self, name: &'static str, value: u32) {
        let code_num = value as u64 + 1;
        self.write_exp_golomb_code(name, VvcSyntaxCode::Ue, code_num);
    }

    pub fn write_se(&mut self, name: &'static str, value: i32) {
        let code_num = if value > 0 {
            (value as u64) * 2
        } else {
            (value.unsigned_abs() as u64 * 2) + 1
        };
        self.write_exp_golomb_code(name, VvcSyntaxCode::Se, code_num);
    }

    fn write_exp_golomb_code(&mut self, name: &'static str, code: VvcSyntaxCode, code_num: u64) {
        debug_assert!(code_num > 0);
        let bit_count = 64 - code_num.leading_zeros() as u8;
        let leading_zero_bits = bit_count - 1;
        let total_bits = (leading_zero_bits * 2) + 1;
        self.writer.record_field(name, code, total_bits as usize);
        for _ in 0..leading_zero_bits {
            self.writer.write_bit(false);
        }
        self.writer.write_bits(code_num, bit_count);
    }

    pub fn write_cabac_token(&mut self, name: &'static str, value: u64, bit_count: u8) {
        assert!(
            bit_count <= 64,
            "CABAC token cannot write more than 64 bits"
        );
        self.writer
            .write_field_bits(name, VvcSyntaxCode::CabacToken, value, bit_count);
    }

    pub fn write_cabac_bits(&mut self, name: &'static str, bits: &[bool]) {
        self.writer
            .write_field_bit_slice(name, VvcSyntaxCode::CabacToken, bits);
    }

    pub fn byte_align_zero(&mut self, name: &'static str) {
        self.writer
            .write_byte_align_zero(name, VvcSyntaxCode::ByteAlignZero);
    }

    pub fn rbsp_trailing_bits(&mut self) {
        self.writer
            .write_trailing_one_zero_bits("rbsp_trailing_bits", VvcSyntaxCode::RbspTrailingBits);
    }

    pub fn is_byte_aligned(&self) -> bool {
        self.writer.is_byte_aligned()
    }

    pub fn fields(&self) -> &[VvcSyntaxField] {
        self.writer.fields()
    }

    pub fn into_bytes(self) -> Vec<u8> {
        self.writer.into_bytes()
    }

    pub fn finish(self) -> VvcSyntaxRbsp {
        let (bytes, fields) = self.writer.into_parts();
        VvcSyntaxRbsp { bytes, fields }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VvcSyntaxRbsp {
    pub bytes: Vec<u8>,
    pub fields: Vec<VvcSyntaxField>,
}
