const CDF_PROB_TOP: u32 = 1 << 15;
const EC_PROB_SHIFT: u32 = 7;
const AV2_ADAPTIVE_CDF_LOOKUP_CACHE_SIZE: usize = 512;
const AV2_ADAPTIVE_CDF_NAME_LOOKUP_CACHE_SIZE: usize = 256;
const AV2_STATIC_CDF_LOOKUP_SIZE: usize = 512;
// The forward AV2 pre-carry finalizer delays output while a future carry could
// still change pending bytes. Each pending word is a 9-bit byte-plus-carry
// value, so 32 words map to a small 288-bit RTL queue. This is intentionally
// slightly wider than a 256-bit guard against pathological all-carry runs; if
// this ever overflows on real streams, treat it as an encoder bug and revisit
// the entropy finalizer instead of reinstating a full payload buffer.
const AV2_PRE_CARRY_PENDING_LIMIT: usize = 32;
const AV2_PRE_CARRY_FLUSH_THRESHOLD: usize = 8;

const PROB_INC: [[i32; 16]; 15] = [
    [8, 0, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1],
    [10, 5, 0, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1],
    [12, 8, 4, 0, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1],
    [12, 9, 6, 3, 0, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1],
    [13, 10, 8, 5, 2, 0, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1],
    [13, 11, 9, 6, 4, 2, 0, -1, -1, -1, -1, -1, -1, -1, -1, -1],
    [14, 12, 10, 8, 6, 4, 2, 0, -1, -1, -1, -1, -1, -1, -1, -1],
    [14, 12, 10, 8, 7, 5, 3, 1, 0, -1, -1, -1, -1, -1, -1, -1],
    [14, 12, 11, 9, 8, 6, 4, 3, 1, 0, -1, -1, -1, -1, -1, -1],
    [14, 13, 11, 10, 8, 7, 5, 4, 2, 1, 0, -1, -1, -1, -1, -1],
    [14, 13, 12, 10, 9, 8, 6, 5, 4, 2, 1, 0, -1, -1, -1, -1],
    [14, 13, 12, 11, 9, 8, 7, 6, 4, 3, 2, 1, 0, -1, -1, -1],
    [14, 13, 12, 11, 10, 9, 8, 6, 5, 4, 3, 2, 1, 0, -1, -1],
    [14, 13, 12, 11, 10, 9, 8, 7, 6, 5, 4, 3, 2, 1, 0, -1],
    [15, 14, 13, 12, 11, 10, 9, 8, 7, 6, 5, 4, 3, 2, 1, 0],
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Av2EntropyCode {
    Literal,
    Symbol,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Av2EntropyField {
    pub name: &'static str,
    pub code: Av2EntropyCode,
    pub symbol_offset: usize,
    pub bit_count: usize,
    pub symbol: Option<usize>,
    pub literal_value: Option<u32>,
    pub fl: Option<u32>,
    pub fh: Option<u32>,
    pub fl_inc: Option<i32>,
    pub fh_inc: Option<i32>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Av2EntropyPayload {
    pub bytes: Vec<u8>,
    pub fields: Vec<Av2EntropyField>,
    pub symbol_bits: usize,
}

#[derive(Debug, Clone)]
pub struct Av2EntropyWriter {
    low: u64,
    rng: u32,
    cnt: i32,
    precarry: Av2PrecarryForwardFinalizer,
    #[cfg(debug_assertions)]
    reverse_precarry: Vec<u16>,
    fields: Vec<Av2EntropyField>,
    symbol_bits: usize,
    adaptive_cdf_updates: bool,
    adaptive_cdfs: Vec<Av2AdaptiveCdf>,
    adaptive_cdf_names: Vec<&'static str>,
    adaptive_cdf_name_ptrs: Vec<Av2StaticNameCacheEntry>,
    adaptive_cdf_name_lookup_cache: [usize; AV2_ADAPTIVE_CDF_NAME_LOOKUP_CACHE_SIZE],
    last_adaptive_cdf_name: Option<Av2StaticNameCacheEntry>,
    last_adaptive_cdf_index: Option<usize>,
    adaptive_cdf_lookup_cache: [Option<usize>; AV2_ADAPTIVE_CDF_LOOKUP_CACHE_SIZE],
    adaptive_static_cdf_indices: [Option<usize>; AV2_STATIC_CDF_LOOKUP_SIZE],
    record_fields: bool,
}

#[derive(Debug, Clone)]
struct Av2PrecarryForwardFinalizer {
    pending: Vec<u16>,
    bytes: Vec<u8>,
    max_pending_words: usize,
}

#[derive(Debug, Clone)]
struct Av2AdaptiveCdf {
    name_index: usize,
    key: usize,
    nsymbs: usize,
    initial: Av2CdfSignature,
    cdf: Vec<u16>,
}

#[derive(Debug, Clone, Copy)]
struct Av2CdfSignature {
    len: usize,
    words: [u64; 5],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Av2StaticNameCacheEntry {
    ptr: usize,
    len: usize,
    index: usize,
}

impl Av2AdaptiveCdf {
    #[inline(always)]
    fn matches(
        &self,
        name_index: usize,
        key: usize,
        nsymbs: usize,
        initial: Av2CdfSignature,
    ) -> bool {
        self.name_index == name_index
            && self.key == key
            && self.nsymbs == nsymbs
            && self.initial.matches(initial)
    }
}

impl Av2EntropyWriter {
    pub fn new() -> Self {
        Self::with_cdf_updates(false)
    }

    pub fn with_cdf_updates(adaptive_cdf_updates: bool) -> Self {
        Self::with_cdf_updates_and_fields(adaptive_cdf_updates, true)
    }

    pub fn with_cdf_updates_and_fields(adaptive_cdf_updates: bool, record_fields: bool) -> Self {
        Self {
            low: 0,
            rng: 0x8000,
            cnt: -9,
            precarry: Av2PrecarryForwardFinalizer::new(),
            #[cfg(debug_assertions)]
            reverse_precarry: Vec::new(),
            fields: Vec::new(),
            symbol_bits: 0,
            adaptive_cdf_updates,
            adaptive_cdfs: Vec::new(),
            adaptive_cdf_names: Vec::new(),
            adaptive_cdf_name_ptrs: Vec::new(),
            adaptive_cdf_name_lookup_cache: [usize::MAX; AV2_ADAPTIVE_CDF_NAME_LOOKUP_CACHE_SIZE],
            last_adaptive_cdf_name: None,
            last_adaptive_cdf_index: None,
            adaptive_cdf_lookup_cache: [None; AV2_ADAPTIVE_CDF_LOOKUP_CACHE_SIZE],
            adaptive_static_cdf_indices: [None; AV2_STATIC_CDF_LOOKUP_SIZE],
            record_fields,
        }
    }

    pub fn write_literal(&mut self, name: &'static str, mut value: u32, mut bits: u8) {
        assert!(
            bits <= 32,
            "AV2 literal helper currently supports up to 32 bits"
        );
        if bits == 0 {
            return;
        }
        // AV2 v1.0.0 Section 4.11.3: L(n) consumes n literal bits through the
        // arithmetic decoder. Encoder-side this mirrors AVM avm_write_literal().
        if self.record_fields {
            self.fields.push(Av2EntropyField {
                name,
                code: Av2EntropyCode::Literal,
                symbol_offset: self.symbol_bits,
                bit_count: bits as usize,
                symbol: None,
                literal_value: Some(value),
                fl: None,
                fh: None,
                fl_inc: None,
                fh_inc: None,
            });
        }
        self.symbol_bits += bits as usize;
        while bits > 0 {
            let n = bits.min(31);
            let shift = bits - n;
            let chunk = (value >> shift) & ((1u32 << n) - 1);
            self.encode_literal_bypass(chunk, n);
            bits -= n;
            value &= (1u32 << bits) - 1;
        }
    }

    #[inline(always)]
    pub fn write_literal_bit(&mut self, name: &'static str, value: bool) {
        if self.record_fields {
            self.fields.push(Av2EntropyField {
                name,
                code: Av2EntropyCode::Literal,
                symbol_offset: self.symbol_bits,
                bit_count: 1,
                symbol: None,
                literal_value: Some(u32::from(value)),
                fl: None,
                fh: None,
                fl_inc: None,
                fh_inc: None,
            });
        }
        self.symbol_bits += 1;
        self.encode_literal_bypass(u32::from(value), 1);
    }

    pub fn write_uniform(&mut self, name: &'static str, n: u32, value: u32) {
        assert!(n > 0, "AV2 uniform helper expects a positive range");
        assert!(value < n, "AV2 uniform value is outside the coding range");
        // AV2 v1.0.0 Section 4.11.3 uses the same arithmetic-literal path as
        // AVM write_uniform() for palette map first tokens.
        let l = 32 - n.leading_zeros();
        let m = (1u32 << l) - n;
        if l == 0 {
            return;
        }
        if value < m {
            self.write_literal(name, value, (l - 1) as u8);
        } else {
            self.write_literal(name, m + ((value - m) >> 1), (l - 1) as u8);
            self.write_literal_bit(name, ((value - m) & 1) != 0);
        }
    }

    pub fn write_symbol(
        &mut self,
        name: &'static str,
        symbol: usize,
        cdf: &mut [u16],
        nsymbs: usize,
        update_cdf: bool,
    ) {
        self.write_symbol_with_cdf_key(name, name, 0, symbol, cdf, nsymbs, update_cdf);
    }

    pub fn write_symbol_with_key(
        &mut self,
        name: &'static str,
        cdf_key: usize,
        symbol: usize,
        cdf: &mut [u16],
        nsymbs: usize,
        update_cdf: bool,
    ) {
        self.write_symbol_with_cdf_key(name, name, cdf_key, symbol, cdf, nsymbs, update_cdf);
    }

    pub fn write_symbol_with_cdf_key(
        &mut self,
        name: &'static str,
        cdf_name: &'static str,
        cdf_key: usize,
        symbol: usize,
        cdf: &mut [u16],
        nsymbs: usize,
        update_cdf: bool,
    ) {
        let adaptive_index = if self.adaptive_cdf_updates {
            Some(self.adaptive_cdf_index(cdf_name, cdf_key, cdf, nsymbs))
        } else {
            None
        };
        self.write_symbol_with_resolved_cdf(name, symbol, cdf, nsymbs, update_cdf, adaptive_index);
    }

    #[inline(always)]
    pub fn write_symbol_with_static_cdf_key(
        &mut self,
        name: &'static str,
        static_cdf_key: usize,
        symbol: usize,
        cdf: &mut [u16],
        nsymbs: usize,
        update_cdf: bool,
    ) {
        let adaptive_index = if self.adaptive_cdf_updates {
            Some(self.static_adaptive_cdf_index(static_cdf_key, cdf, nsymbs))
        } else {
            None
        };
        self.write_symbol_with_resolved_cdf(name, symbol, cdf, nsymbs, update_cdf, adaptive_index);
    }

    #[inline(always)]
    fn write_symbol_with_resolved_cdf(
        &mut self,
        name: &'static str,
        symbol: usize,
        cdf: &mut [u16],
        nsymbs: usize,
        update_cdf: bool,
        adaptive_index: Option<usize>,
    ) {
        assert!((2..=16).contains(&nsymbs), "AV2 CDF symbols must be 2..=16");
        assert!(symbol < nsymbs, "symbol out of CDF range");
        assert!(
            cdf.len() >= nsymbs + 4,
            "CDF must include adaptation entries"
        );
        // AV2 v1.0.0 Sections 4.11.2 and 8.3: S() reads a symbol using the
        // active CDF selected by the syntax process. Encoder-side this mirrors
        // AVM avm_write_symbol().
        let (fl, fh) = {
            let active_cdf = if let Some(index) = adaptive_index {
                self.adaptive_cdfs[index].cdf.as_slice()
            } else {
                &*cdf
            };
            assert_eq!(
                active_cdf[nsymbs - 1],
                0,
                "last AV2 inverse CDF entry must be zero"
            );
            let fl = if symbol > 0 {
                active_cdf[symbol - 1] as u32
            } else {
                CDF_PROB_TOP
            };
            let fh = active_cdf[symbol] as u32;
            (fl, fh)
        };
        if self.record_fields {
            let fl_inc = if fl < CDF_PROB_TOP {
                PROB_INC[nsymbs - 2][symbol.saturating_sub(1)]
            } else {
                0
            };
            let fh_inc = PROB_INC[nsymbs - 2][symbol];
            self.fields.push(Av2EntropyField {
                name,
                code: Av2EntropyCode::Symbol,
                symbol_offset: self.symbol_bits,
                bit_count: 1,
                symbol: Some(symbol),
                literal_value: None,
                fl: Some(fl),
                fh: Some(fh),
                fl_inc: Some(fl_inc),
                fh_inc: Some(fh_inc),
            });
        }
        self.symbol_bits += 1;
        self.encode_q15(fl, fh, symbol, nsymbs);
        if update_cdf || self.adaptive_cdf_updates {
            if let Some(index) = adaptive_index {
                update_cdf_counts(&mut self.adaptive_cdfs[index].cdf, symbol, nsymbs);
            } else {
                update_cdf_counts(cdf, symbol, nsymbs);
            }
        }
    }

    pub fn finish(mut self) -> Av2EntropyPayload {
        let mut e = ((self.low + 0x3fff) & !0x3fff) | 0x4000;
        let mut c = self.cnt;
        let mut s = c + 10;
        if s > 0 {
            let mut n = mask(c + 16);
            while s > 0 {
                self.push_precarry((e >> (c + 16)) as u16);
                e &= n;
                s -= 8;
                c -= 8;
                n >>= 8;
            }
        }

        #[cfg(debug_assertions)]
        let max_pending_words = self.precarry.max_pending_words;
        let out = self.precarry.finish();
        #[cfg(debug_assertions)]
        {
            let reverse = finalize_precarry_reverse(&self.reverse_precarry);
            debug_assert_eq!(
                out, reverse,
                "AV2 forward pre-carry finalizer diverged from reverse AVM carry propagation"
            );
            if std::env::var_os("FRAMEFORGE_AV2_PRE_CARRY_STATS").is_some() {
                eprintln!(
                    "FRAMEFORGE_AV2_PRE_CARRY_STATS words={} max_pending_words={}",
                    self.reverse_precarry.len(),
                    max_pending_words
                );
            }
        }

        Av2EntropyPayload {
            bytes: out,
            fields: self.fields,
            symbol_bits: self.symbol_bits,
        }
    }

    fn encode_literal_bypass(&mut self, value: u32, bits: u8) {
        assert!(
            bits <= 31,
            "AV2 bypass literal chunks are limited to 31 bits"
        );
        let low = (self.low << bits) + (self.rng as u64 * value as u64);
        self.normalize(low, self.rng, bits as i32);
    }

    fn adaptive_cdf_index(
        &mut self,
        name: &'static str,
        key: usize,
        cdf: &[u16],
        nsymbs: usize,
    ) -> usize {
        let initial_len = nsymbs + 4;
        let initial = Av2CdfSignature::from_cdf(&cdf[..initial_len]);
        let name_index = self.adaptive_cdf_name_index(name);
        let cache_slot = av2_adaptive_cdf_lookup_cache_slot(name_index, key, nsymbs, initial);
        if let Some(index) = self.last_adaptive_cdf_index {
            let entry = &self.adaptive_cdfs[index];
            if entry.matches(name_index, key, nsymbs, initial) {
                self.adaptive_cdf_lookup_cache[cache_slot] = Some(index);
                return index;
            }
        }
        if let Some(index) = self.adaptive_cdf_lookup_cache[cache_slot] {
            let entry = &self.adaptive_cdfs[index];
            if entry.matches(name_index, key, nsymbs, initial) {
                self.last_adaptive_cdf_index = Some(index);
                return index;
            }
        }
        if let Some(index) = self
            .adaptive_cdfs
            .iter()
            .position(|entry| entry.matches(name_index, key, nsymbs, initial))
        {
            self.last_adaptive_cdf_index = Some(index);
            self.adaptive_cdf_lookup_cache[cache_slot] = Some(index);
            return index;
        }

        self.adaptive_cdfs.push(Av2AdaptiveCdf {
            name_index,
            key,
            nsymbs,
            initial,
            cdf: cdf[..initial_len].to_vec(),
        });
        let index = self.adaptive_cdfs.len() - 1;
        self.last_adaptive_cdf_index = Some(index);
        self.adaptive_cdf_lookup_cache[cache_slot] = Some(index);
        index
    }

    #[inline(always)]
    fn static_adaptive_cdf_index(
        &mut self,
        static_key: usize,
        cdf: &[u16],
        nsymbs: usize,
    ) -> usize {
        assert!(
            static_key < AV2_STATIC_CDF_LOOKUP_SIZE,
            "static AV2 CDF key {static_key} exceeds direct lookup table"
        );
        if let Some(index) = self.adaptive_static_cdf_indices[static_key] {
            #[cfg(debug_assertions)]
            {
                let initial_len = nsymbs + 4;
                let initial = Av2CdfSignature::from_cdf(&cdf[..initial_len]);
                let entry = &self.adaptive_cdfs[index];
                debug_assert_eq!(entry.nsymbs, nsymbs);
                debug_assert!(
                    entry.initial.matches(initial),
                    "static AV2 CDF key {static_key} reused for a different default CDF"
                );
            }
            return index;
        }

        let initial_len = nsymbs + 4;
        let initial = Av2CdfSignature::from_cdf(&cdf[..initial_len]);
        self.adaptive_cdfs.push(Av2AdaptiveCdf {
            name_index: usize::MAX,
            key: static_key,
            nsymbs,
            initial,
            cdf: cdf[..initial_len].to_vec(),
        });
        let index = self.adaptive_cdfs.len() - 1;
        self.adaptive_static_cdf_indices[static_key] = Some(index);
        index
    }

    fn adaptive_cdf_name_index(&mut self, name: &'static str) -> usize {
        let ptr = name.as_ptr() as usize;
        let len = name.len();
        if let Some(entry) = self.last_adaptive_cdf_name {
            if entry.ptr == ptr && entry.len == len {
                return entry.index;
            }
        }
        let cache_slot = av2_adaptive_cdf_name_lookup_cache_slot(ptr, len);
        let cached_ptr_index = self.adaptive_cdf_name_lookup_cache[cache_slot];
        if let Some(entry) = (cached_ptr_index != usize::MAX)
            .then(|| self.adaptive_cdf_name_ptrs.get(cached_ptr_index))
            .flatten()
            .copied()
        {
            if entry.ptr == ptr && entry.len == len {
                self.last_adaptive_cdf_name = Some(entry);
                return entry.index;
            }
        }
        if let Some((ptr_index, entry)) = self
            .adaptive_cdf_name_ptrs
            .iter()
            .enumerate()
            .find(|(_, entry)| entry.ptr == ptr && entry.len == len)
        {
            let entry = *entry;
            self.last_adaptive_cdf_name = Some(entry);
            self.adaptive_cdf_name_lookup_cache[cache_slot] = ptr_index;
            return entry.index;
        }

        let index = if let Some(index) = self
            .adaptive_cdf_names
            .iter()
            .position(|existing| *existing == name)
        {
            index
        } else {
            self.adaptive_cdf_names.push(name);
            self.adaptive_cdf_names.len() - 1
        };
        let entry = Av2StaticNameCacheEntry { ptr, len, index };
        let ptr_index = self.adaptive_cdf_name_ptrs.len();
        self.adaptive_cdf_name_ptrs.push(entry);
        self.adaptive_cdf_name_lookup_cache[cache_slot] = ptr_index;
        self.last_adaptive_cdf_name = Some(entry);
        index
    }

    fn push_precarry(&mut self, word: u16) {
        // AVM stores pre-carry values as uint16_t, but the AV2 range coder
        // normalize()/exit_symbol() process is expected to emit a 9-bit
        // byte-plus-carry value. Keeping this invariant explicit is what makes
        // a VVC-style forward delayed-carry finalizer possible.
        assert!(
            word <= 0x01ff,
            "AV2 pre-carry word exceeded the 9-bit forward-carry invariant: {word:#06x}"
        );
        self.precarry.push(word);
        #[cfg(debug_assertions)]
        self.reverse_precarry.push(word);
    }

    fn encode_q15(&mut self, fl: u32, fh: u32, symbol: usize, nsymbs: usize) {
        assert!(fh <= fl, "AV2 inverse CDF must be monotonic");
        assert!(fl <= CDF_PROB_TOP, "AV2 inverse CDF exceeds Q15 top");
        let mut low = self.low;
        let mut rng = self.rng;
        if fl < CDF_PROB_TOP {
            let u = prob_scale(fl, rng, symbol.saturating_sub(1), nsymbs);
            let v = prob_scale(fh, rng, symbol, nsymbs);
            low += (rng - u) as u64;
            rng = u - v;
        } else {
            let v = prob_scale(fh, rng, symbol, nsymbs);
            rng -= v;
        }
        self.normalize(low, rng, 0);
    }

    fn normalize(&mut self, mut low: u64, rng: u32, bypass_bits: i32) {
        assert!(rng <= 65535, "AV2 range must fit 16 bits before normalize");
        let mut c = self.cnt;
        let d = if bypass_bits > 0 {
            c += bypass_bits;
            0
        } else {
            16 - ilog_nz(rng)
        };
        let mut s = c + d;
        if s >= 0 {
            c += 16;
            let mut m = mask(c);
            if s >= 8 {
                self.push_precarry((low >> c) as u16);
                low &= m;
                c -= 8;
                m >>= 8;
            }
            self.push_precarry((low >> c) as u16);
            s = c + d - 24;
            low &= m;
        }
        self.low = low << d;
        self.rng = rng << d;
        self.cnt = s;
    }
}

impl Av2CdfSignature {
    #[inline(always)]
    fn matches(self, other: Self) -> bool {
        self.len == other.len
            && self.words[0] == other.words[0]
            && self.words[1] == other.words[1]
            && self.words[2] == other.words[2]
            && self.words[3] == other.words[3]
            && self.words[4] == other.words[4]
    }

    fn from_cdf(cdf: &[u16]) -> Self {
        debug_assert!(
            cdf.len() <= 20,
            "AV2 adaptive CDF signature expects nsymbs + 4 <= 20 entries"
        );
        let mut words = [0u64; 5];
        for (index, &value) in cdf.iter().enumerate() {
            words[index / 4] |= u64::from(value) << ((index % 4) * 16);
        }
        Self {
            len: cdf.len(),
            words,
        }
    }
}

fn av2_adaptive_cdf_lookup_cache_slot(
    name_index: usize,
    key: usize,
    nsymbs: usize,
    initial: Av2CdfSignature,
) -> usize {
    let mut hash = (name_index as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15);
    hash ^= (key as u64).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    hash ^= (nsymbs as u64).wrapping_mul(0x94d0_49bb_1331_11eb);
    hash ^= (initial.len as u64).wrapping_mul(0xd6e8_feb8_6659_fd93);
    for word in initial.words {
        hash ^= word;
        hash = hash.rotate_left(13).wrapping_mul(0x9e37_79b9_7f4a_7c15);
    }
    (hash as usize) & (AV2_ADAPTIVE_CDF_LOOKUP_CACHE_SIZE - 1)
}

fn av2_adaptive_cdf_name_lookup_cache_slot(ptr: usize, len: usize) -> usize {
    let mut hash = ptr.wrapping_mul(0x9e37_79b9_7f4a_7c15usize);
    hash ^= len.wrapping_mul(0xbf58_476d_1ce4_e5b9usize);
    hash ^= hash >> 33;
    hash = hash.wrapping_mul(0xff51_afd7_ed55_8ccdusize);
    hash ^= hash >> 33;
    hash & (AV2_ADAPTIVE_CDF_NAME_LOOKUP_CACHE_SIZE - 1)
}

impl Av2PrecarryForwardFinalizer {
    fn new() -> Self {
        Self {
            pending: Vec::new(),
            bytes: Vec::new(),
            max_pending_words: 0,
        }
    }

    fn push(&mut self, word: u16) {
        self.pending.push(word);
        assert!(
            self.pending.len() <= AV2_PRE_CARRY_PENDING_LIMIT,
            "AV2 forward pre-carry pending buffer exceeded {AV2_PRE_CARRY_PENDING_LIMIT} words"
        );
        self.max_pending_words = self.max_pending_words.max(self.pending.len());
        if self.pending.len() >= AV2_PRE_CARRY_FLUSH_THRESHOLD {
            self.flush_common_prefix();
        }
    }

    fn finish(mut self) -> Vec<u8> {
        let (_, tail) = eval_precarry_suffix(&self.pending, 0);
        self.bytes.extend(tail);
        self.pending.clear();
        self.bytes
    }

    fn flush_common_prefix(&mut self) {
        if self.pending.is_empty() {
            return;
        }

        let len = self.pending.len();
        let mut carry0_bytes = [0u8; AV2_PRE_CARRY_PENDING_LIMIT];
        let mut carry1_bytes = [0u8; AV2_PRE_CARRY_PENDING_LIMIT];
        let mut carry2_bytes = [0u8; AV2_PRE_CARRY_PENDING_LIMIT];
        eval_precarry_suffix_into(&self.pending, 0, &mut carry0_bytes[..len]);
        eval_precarry_suffix_into(&self.pending, 1, &mut carry1_bytes[..len]);
        eval_precarry_suffix_into(&self.pending, 2, &mut carry2_bytes[..len]);
        let mut common = 0usize;
        while common < len
            && carry0_bytes[common] == carry1_bytes[common]
            && carry0_bytes[common] == carry2_bytes[common]
        {
            common += 1;
        }

        if common != 0 {
            self.bytes.extend_from_slice(&carry0_bytes[..common]);
            self.pending.drain(..common);
        }
    }
}

#[cfg(any(debug_assertions, test))]
fn finalize_precarry_reverse(precarry: &[u16]) -> Vec<u8> {
    let mut out = vec![0; precarry.len()];
    let mut carry = 0u16;
    for index in (0..precarry.len()).rev() {
        carry += precarry[index];
        out[index] = carry as u8;
        carry >>= 8;
    }
    out
}

#[cfg(test)]
fn finalize_precarry_forward(precarry: &[u16]) -> (Vec<u8>, usize) {
    let mut finalizer = Av2PrecarryForwardFinalizer::new();
    for &word in precarry {
        finalizer.push(word);
    }
    let max_pending = finalizer.max_pending_words;
    (finalizer.finish(), max_pending)
}

fn eval_precarry_suffix(precarry: &[u16], terminal_carry: u16) -> (u16, Vec<u8>) {
    let mut out = vec![0; precarry.len()];
    let carry = eval_precarry_suffix_into(precarry, terminal_carry, &mut out);
    (carry, out)
}

fn eval_precarry_suffix_into(precarry: &[u16], terminal_carry: u16, out: &mut [u8]) -> u16 {
    assert!(
        out.len() >= precarry.len(),
        "AV2 pre-carry output scratch must cover the pending suffix"
    );
    let mut carry = terminal_carry;
    for index in (0..precarry.len()).rev() {
        carry += precarry[index];
        out[index] = carry as u8;
        carry >>= 8;
    }
    carry
}

impl Default for Av2EntropyWriter {
    fn default() -> Self {
        Self::new()
    }
}

pub fn av2_empty_tile_entropy_payload() -> Av2EntropyPayload {
    // AV2 v1.0.0 Section 5.20.1 calls init_symbol(tileSize) before
    // decode_tile() and exit_symbol() afterward. This is the smallest possible
    // generated payload: no syntax decisions are emitted yet, but the range
    // writer still emits the exit-symbol terminating bit pattern.
    Av2EntropyWriter::new().finish()
}

pub fn av2_uniform_icdf(nsymbs: usize) -> Vec<u16> {
    assert!((2..=16).contains(&nsymbs), "AV2 CDF symbols must be 2..=16");
    let mut cdf = vec![0; nsymbs + 4];
    for index in 1..nsymbs {
        let cumulative = ((CDF_PROB_TOP as usize * index) / nsymbs) as u32;
        cdf[index - 1] = (CDF_PROB_TOP - cumulative) as u16;
    }
    cdf[nsymbs - 1] = 0;
    cdf
}

fn update_cdf_counts(cdf: &mut [u16], symbol: usize, nsymbs: usize) {
    let time_interval = if cdf[nsymbs] > 31 {
        2
    } else if cdf[nsymbs] > 15 {
        1
    } else {
        0
    };
    let rate = 2 + cdf[nsymbs + 1 + time_interval] as u32;
    let mut tmp = CDF_PROB_TOP as i32;
    for (index, value) in cdf.iter_mut().take(nsymbs - 1).enumerate() {
        if index == symbol {
            tmp = 0;
        }
        let current = *value as i32;
        if tmp < current {
            *value -= ((current - tmp) >> rate) as u16;
        } else {
            *value += ((tmp - current) >> rate) as u16;
        }
    }
    if cdf[nsymbs] < 32 {
        cdf[nsymbs] += 1;
    }
}

fn prob_scale(p: u32, rng: u32, symbol: usize, nsymbs: usize) -> u32 {
    let rr = rng >> 8;
    let mut pp = ((p >> EC_PROB_SHIFT) << 4) as i32;
    pp += PROB_INC[nsymbs - 2][symbol];
    (((rr as i32 * pp) >> 7) as u32) << 3
}

fn ilog_nz(value: u32) -> i32 {
    debug_assert!(value != 0);
    (u32::BITS - value.leading_zeros()) as i32
}

fn mask(bits: i32) -> u64 {
    if bits <= 0 {
        0
    } else if bits >= 64 {
        u64::MAX
    } else {
        (1u64 << bits) - 1
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn av2_entropy_writer_finalizes_empty_tile_payload() {
        let payload = av2_empty_tile_entropy_payload();

        assert_eq!(payload.bytes, vec![0x80]);
        assert_eq!(payload.symbol_bits, 0);
        assert!(payload.fields.is_empty());
    }

    #[test]
    fn av2_entropy_writer_encodes_literals_through_range_coder() {
        let mut writer = Av2EntropyWriter::new();
        writer.write_literal("tile.test_literal", 0b1010_0101, 8);
        let payload = writer.finish();

        assert!(!payload.bytes.is_empty());
        assert_eq!(payload.symbol_bits, 8);
        assert_eq!(payload.fields[0].name, "tile.test_literal");
        assert_eq!(payload.fields[0].code, Av2EntropyCode::Literal);
    }

    #[test]
    fn av2_entropy_writer_encodes_cdf_symbols_and_updates_counts() {
        let mut cdf = av2_uniform_icdf(2);
        let mut writer = Av2EntropyWriter::new();
        writer.write_symbol("tile.test_symbol", 0, &mut cdf, 2, true);
        let payload = writer.finish();

        assert!(!payload.bytes.is_empty());
        assert_eq!(payload.symbol_bits, 1);
        assert_eq!(payload.fields[0].code, Av2EntropyCode::Symbol);
        assert_eq!(cdf[2], 1);
    }

    #[test]
    fn av2_forward_precarry_matches_reverse_for_edge_patterns() {
        let patterns: &[&[u16]] = &[
            &[0x000],
            &[0x0ff],
            &[0x100],
            &[0x1ff],
            &[0x000, 0x1ff, 0x100],
            &[0x1ff, 0x1ff, 0x1ff, 0x100],
            &[0x0ff, 0x0ff, 0x100, 0x1ff],
        ];

        for pattern in patterns {
            let (forward, _) = finalize_precarry_forward(pattern);
            assert_eq!(forward, finalize_precarry_reverse(pattern), "{pattern:#x?}");
        }
    }

    #[test]
    fn av2_forward_precarry_matches_reverse_for_representative_words() {
        let values = [
            0x000, 0x001, 0x07f, 0x0fe, 0x0ff, 0x100, 0x101, 0x1fe, 0x1ff,
        ];
        for &a in &values {
            for &b in &values {
                for &c in &values {
                    for &d in &values {
                        let words = [a, b, c, d];
                        let (forward, _) = finalize_precarry_forward(&words);
                        assert_eq!(forward, finalize_precarry_reverse(&words), "{words:#x?}");
                    }
                }
            }
        }
    }

    #[test]
    fn av2_forward_precarry_matches_reverse_for_deterministic_random_words() {
        let mut seed = 0x1234_5678_u32;
        let mut max_pending = 0usize;
        for len in [1usize, 2, 3, 4, 8, 16, 64, 256] {
            for _ in 0..128 {
                let mut words = Vec::with_capacity(len);
                for _ in 0..len {
                    seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                    words.push(((seed >> 7) & 0x1ff) as u16);
                }
                let (forward, pending) = finalize_precarry_forward(&words);
                max_pending = max_pending.max(pending);
                assert_eq!(forward, finalize_precarry_reverse(&words));
            }
        }
        assert!(max_pending > 0);
    }

    #[test]
    fn av2_forward_precarry_handles_long_max_carry_run_with_small_pending_buffer() {
        let words = vec![0x1ff; 4096];
        let (forward, max_pending) = finalize_precarry_forward(&words);

        assert_eq!(forward, finalize_precarry_reverse(&words));
        assert!(
            max_pending <= AV2_PRE_CARRY_PENDING_LIMIT,
            "max pending {max_pending} exceeded limit"
        );
    }
}
