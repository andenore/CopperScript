//! Raw DEFLATE with zlib's choices at its default level, byte for byte.
//!
//! Python's `zipfile` compresses with `zlib.compressobj(-1, DEFLATED, -15)`:
//! level 6 (`deflate_slow`: lazy matching, good 8, lazy 16, nice 128, chain
//! 128), a 32 KiB window, `memLevel` 8 (15-bit hash, 16383 symbols a block)
//! and the default strategy. Any valid DEFLATE stream would decompress the
//! same, but the archives are compared byte for byte, so this follows zlib
//! 1.3 `deflate.c` and `trees.c` step for step, including the window bytes
//! a match may compare beyond the input (zeroed or left over from a slide).

const MIN_MATCH: usize = 3;
const MAX_MATCH: usize = 258;
const W_SIZE: usize = 1 << 15;
const W_MASK: usize = W_SIZE - 1;
const WINDOW_SIZE: usize = 2 * W_SIZE;
const HASH_BITS: u32 = 15;
const HASH_SIZE: usize = 1 << HASH_BITS;
const HASH_MASK: usize = HASH_SIZE - 1;
const HASH_SHIFT: u32 = HASH_BITS.div_ceil(MIN_MATCH as u32);
const MIN_LOOKAHEAD: usize = MAX_MATCH + MIN_MATCH + 1;
const MAX_DIST: usize = W_SIZE - MIN_LOOKAHEAD;
const WIN_INIT: usize = MAX_MATCH;
const TOO_FAR: usize = 4096;
const LIT_BUFSIZE: usize = 1 << (8 + 6);
const SYM_END: usize = LIT_BUFSIZE - 1;

const GOOD_MATCH: usize = 8;
const MAX_LAZY: usize = 16;
const NICE_MATCH: usize = 128;
const MAX_CHAIN: usize = 128;

const LENGTH_CODES: usize = 29;
const LITERALS: usize = 256;
const L_CODES: usize = LITERALS + 1 + LENGTH_CODES;
const D_CODES: usize = 30;
const BL_CODES: usize = 19;
const HEAP_SIZE: usize = 2 * L_CODES + 1;
const MAX_BITS: usize = 15;
const MAX_BL_BITS: usize = 7;
const END_BLOCK: usize = 256;
const REP_3_6: usize = 16;
const REPZ_3_10: usize = 17;
const REPZ_11_138: usize = 18;

const EXTRA_LBITS: [u32; LENGTH_CODES] =
    [0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3, 4, 4, 4, 4, 5, 5, 5, 5, 0];
const EXTRA_DBITS: [u32; D_CODES] =
    [0, 0, 0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7, 8, 8, 9, 9, 10, 10, 11, 11, 12, 12, 13, 13];
const EXTRA_BLBITS: [u32; BL_CODES] = [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 2, 3, 7];
const BL_ORDER: [usize; BL_CODES] = [16, 17, 18, 0, 8, 7, 9, 6, 10, 5, 11, 4, 12, 3, 13, 2, 14, 1, 15];

/// `ct_data`: `fc` holds the frequency, later the code; `dl` the parent,
/// later the bit length.
#[derive(Clone, Copy, Default)]
struct Ct {
    fc: u16,
    dl: u16,
}

struct Tables {
    length_code: [u8; 256],
    dist_code: [u8; 512],
    base_length: [u32; LENGTH_CODES],
    base_dist: [u32; D_CODES],
    static_ltree: [Ct; L_CODES + 2],
    static_dtree: [Ct; D_CODES],
}

fn bi_reverse(mut code: u32, mut length: u32) -> u32 {
    let mut result = 0;
    loop {
        result |= code & 1;
        code >>= 1;
        result <<= 1;
        length -= 1;
        if length == 0 {
            return result >> 1;
        }
    }
}

/// `gen_codes`: canonical codes from the bit lengths.
fn gen_codes(tree: &mut [Ct], max_code: usize, bl_count: &[u16; MAX_BITS + 1]) {
    let mut next_code = [0u32; MAX_BITS + 1];
    let mut code = 0u32;
    for bits in 1..=MAX_BITS {
        code = (code + bl_count[bits - 1] as u32) << 1;
        next_code[bits] = code;
    }
    for item in tree.iter_mut().take(max_code + 1) {
        let length = item.dl as usize;
        if length == 0 {
            continue;
        }
        item.fc = bi_reverse(next_code[length], length as u32) as u16;
        next_code[length] += 1;
    }
}

impl Tables {
    /// `tr_static_init`.
    fn new() -> Tables {
        let mut tables = Tables {
            length_code: [0; 256],
            dist_code: [0; 512],
            base_length: [0; LENGTH_CODES],
            base_dist: [0; D_CODES],
            static_ltree: [Ct::default(); L_CODES + 2],
            static_dtree: [Ct::default(); D_CODES],
        };
        let mut length = 0usize;
        let mut code = 0;
        while code < LENGTH_CODES - 1 {
            tables.base_length[code] = length as u32;
            for _ in 0..(1 << EXTRA_LBITS[code]) {
                tables.length_code[length] = code as u8;
                length += 1;
            }
            code += 1;
        }
        tables.length_code[length - 1] = code as u8;
        let mut dist = 0usize;
        code = 0;
        while code < 16 {
            tables.base_dist[code] = dist as u32;
            for _ in 0..(1 << EXTRA_DBITS[code]) {
                tables.dist_code[dist] = code as u8;
                dist += 1;
            }
            code += 1;
        }
        dist >>= 7;
        while code < D_CODES {
            tables.base_dist[code] = (dist << 7) as u32;
            for _ in 0..(1 << (EXTRA_DBITS[code] - 7)) {
                tables.dist_code[256 + dist] = code as u8;
                dist += 1;
            }
            code += 1;
        }
        let mut bl_count = [0u16; MAX_BITS + 1];
        for (n, item) in tables.static_ltree.iter_mut().enumerate() {
            let bits = match n {
                0..=143 => 8,
                144..=255 => 9,
                256..=279 => 7,
                _ => 8,
            };
            item.dl = bits;
            bl_count[bits as usize] += 1;
        }
        gen_codes(&mut tables.static_ltree, L_CODES + 1, &bl_count);
        for (n, item) in tables.static_dtree.iter_mut().enumerate() {
            item.dl = 5;
            item.fc = bi_reverse(n as u32, 5) as u16;
        }
        tables
    }

    fn d_code(&self, dist: usize) -> usize {
        if dist < 256 { self.dist_code[dist] as usize } else { self.dist_code[256 + (dist >> 7)] as usize }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum TreeKind {
    Literal,
    Distance,
    BitLength,
}

struct Bits {
    out: Vec<u8>,
    buffer: u64,
    valid: u32,
}

impl Bits {
    fn send(&mut self, value: u32, length: u32) {
        self.buffer |= (value as u64) << self.valid;
        self.valid += length;
        while self.valid >= 8 {
            self.out.push(self.buffer as u8);
            self.buffer >>= 8;
            self.valid -= 8;
        }
    }

    /// `bi_windup`: pad to a byte boundary.
    fn windup(&mut self) {
        if self.valid > 0 {
            self.out.push(self.buffer as u8);
        }
        self.buffer = 0;
        self.valid = 0;
    }
}

struct State {
    tables: Tables,
    window: Vec<u8>,
    prev: Vec<u16>,
    head: Vec<u16>,
    ins_h: usize,
    high_water: usize,
    strstart: usize,
    block_start: i64,
    lookahead: usize,
    insert: usize,
    match_start: usize,
    match_length: usize,
    prev_length: usize,
    prev_match: usize,
    match_available: bool,
    dyn_ltree: [Ct; HEAP_SIZE],
    dyn_dtree: [Ct; 2 * D_CODES + 1],
    bl_tree: [Ct; 2 * BL_CODES + 1],
    l_max_code: usize,
    d_max_code: usize,
    bl_count: [u16; MAX_BITS + 1],
    heap: [usize; HEAP_SIZE],
    heap_len: usize,
    heap_max: usize,
    depth: [u8; HEAP_SIZE],
    symbols: Vec<(u16, u8)>,
    opt_len: u64,
    static_len: u64,
    bits: Bits,
}

/// Compress `data` like `compressobj(-1, DEFLATED, -15)` followed by `flush()`.
pub fn deflate_raw(data: &[u8]) -> Vec<u8> {
    let mut state = State {
        tables: Tables::new(),
        window: vec![0; WINDOW_SIZE],
        prev: vec![0; W_SIZE],
        head: vec![0; HASH_SIZE],
        ins_h: 0,
        high_water: 0,
        strstart: 0,
        block_start: 0,
        lookahead: 0,
        insert: 0,
        match_start: 0,
        match_length: MIN_MATCH - 1,
        prev_length: MIN_MATCH - 1,
        prev_match: 0,
        match_available: false,
        dyn_ltree: [Ct::default(); HEAP_SIZE],
        dyn_dtree: [Ct::default(); 2 * D_CODES + 1],
        bl_tree: [Ct::default(); 2 * BL_CODES + 1],
        l_max_code: 0,
        d_max_code: 0,
        bl_count: [0; MAX_BITS + 1],
        heap: [0; HEAP_SIZE],
        heap_len: 0,
        heap_max: 0,
        depth: [0; HEAP_SIZE],
        symbols: Vec::with_capacity(SYM_END),
        opt_len: 0,
        static_len: 0,
        bits: Bits { out: Vec::new(), buffer: 0, valid: 0 },
    };
    state.init_block();
    let mut input = data;
    state.deflate_slow(&mut input);
    state.bits.out
}

impl State {
    fn update_hash(&mut self, byte: u8) {
        self.ins_h = ((self.ins_h << HASH_SHIFT) ^ byte as usize) & HASH_MASK;
    }

    /// `INSERT_STRING`: the previous head of the string's hash chain.
    fn insert_string(&mut self, position: usize) -> usize {
        self.update_hash(self.window[position + MIN_MATCH - 1]);
        let head = self.head[self.ins_h];
        self.prev[position & W_MASK] = head;
        self.head[self.ins_h] = position as u16;
        head as usize
    }

    fn slide_hash(&mut self) {
        for entry in self.head.iter_mut().chain(self.prev.iter_mut()) {
            *entry = if *entry as usize >= W_SIZE { *entry - W_SIZE as u16 } else { 0 };
        }
    }

    fn fill_window(&mut self, input: &mut &[u8]) {
        loop {
            let mut more = WINDOW_SIZE - self.lookahead - self.strstart;
            if self.strstart >= W_SIZE + MAX_DIST {
                self.window.copy_within(W_SIZE..W_SIZE + (W_SIZE - more), 0);
                self.match_start = self.match_start.wrapping_sub(W_SIZE);
                self.strstart -= W_SIZE;
                self.block_start -= W_SIZE as i64;
                if self.insert > self.strstart {
                    self.insert = self.strstart;
                }
                self.slide_hash();
                more += W_SIZE;
            }
            if input.is_empty() {
                break;
            }
            let count = more.min(input.len());
            let at = self.strstart + self.lookahead;
            self.window[at..at + count].copy_from_slice(&input[..count]);
            *input = &input[count..];
            self.lookahead += count;
            if self.lookahead + self.insert >= MIN_MATCH {
                let mut position = self.strstart - self.insert;
                self.ins_h = self.window[position] as usize;
                self.update_hash(self.window[position + 1]);
                while self.insert > 0 {
                    self.update_hash(self.window[position + MIN_MATCH - 1]);
                    self.prev[position & W_MASK] = self.head[self.ins_h];
                    self.head[self.ins_h] = position as u16;
                    position += 1;
                    self.insert -= 1;
                    if self.lookahead + self.insert < MIN_MATCH {
                        break;
                    }
                }
            }
            if self.lookahead >= MIN_LOOKAHEAD || input.is_empty() {
                break;
            }
        }
        if self.high_water < WINDOW_SIZE {
            let current = self.strstart + self.lookahead;
            if self.high_water < current {
                let init = (WINDOW_SIZE - current).min(WIN_INIT);
                self.window[current..current + init].fill(0);
                self.high_water = current + init;
            } else if self.high_water < current + WIN_INIT {
                let init = (current + WIN_INIT - self.high_water).min(WINDOW_SIZE - self.high_water);
                self.window[self.high_water..self.high_water + init].fill(0);
                self.high_water += init;
            }
        }
    }

    fn longest_match(&mut self, mut cur_match: usize) -> usize {
        let mut chain_length = MAX_CHAIN;
        let scan = self.strstart;
        let mut best_len = self.prev_length;
        let mut nice_match = NICE_MATCH;
        let limit = self.strstart.saturating_sub(MAX_DIST);
        let strend = scan + MAX_MATCH;
        let window = &self.window;
        let mut scan_end1 = window[scan + best_len - 1];
        let mut scan_end = window[scan + best_len];
        if self.prev_length >= GOOD_MATCH {
            chain_length >>= 2;
        }
        if nice_match > self.lookahead {
            nice_match = self.lookahead;
        }
        loop {
            let found = cur_match;
            if window[found + best_len] == scan_end
                && window[found + best_len - 1] == scan_end1
                && window[found] == window[scan]
                && window[found + 1] == window[scan + 1]
            {
                // The unrolled compare stops at `strend`, which is reached
                // exactly, so the plain loop compares the same bytes.
                let mut s = scan + 2;
                let mut m = found + 2;
                loop {
                    s += 1;
                    m += 1;
                    if window[s] != window[m] || s >= strend {
                        break;
                    }
                }
                let length = MAX_MATCH - (strend - s);
                if length > best_len {
                    self.match_start = found;
                    best_len = length;
                    if length >= nice_match {
                        break;
                    }
                    scan_end1 = window[scan + best_len - 1];
                    scan_end = window[scan + best_len];
                }
            }
            cur_match = self.prev[cur_match & W_MASK] as usize;
            chain_length -= 1;
            if cur_match <= limit || chain_length == 0 {
                break;
            }
        }
        best_len.min(self.lookahead)
    }

    fn tally_literal(&mut self, byte: u8) -> bool {
        self.symbols.push((0, byte));
        self.dyn_ltree[byte as usize].fc += 1;
        self.symbols.len() == SYM_END
    }

    fn tally_distance(&mut self, distance: usize, length: usize) -> bool {
        self.symbols.push((distance as u16, length as u8));
        self.dyn_ltree[self.tables.length_code[length] as usize + LITERALS + 1].fc += 1;
        let code = self.tables.d_code(distance - 1);
        self.dyn_dtree[code].fc += 1;
        self.symbols.len() == SYM_END
    }

    fn flush_block(&mut self, last: bool) {
        let stored = if self.block_start >= 0 {
            Some((self.block_start as usize, self.strstart - self.block_start as usize))
        } else {
            None
        };
        let stored_len = (self.strstart as i64 - self.block_start) as u64;
        self.tr_flush_block(stored, stored_len, last);
        self.block_start = self.strstart as i64;
    }

    fn deflate_slow(&mut self, input: &mut &[u8]) {
        loop {
            if self.lookahead < MIN_LOOKAHEAD {
                self.fill_window(input);
                if self.lookahead == 0 {
                    break;
                }
            }
            let mut hash_head = 0;
            if self.lookahead >= MIN_MATCH {
                hash_head = self.insert_string(self.strstart);
            }
            self.prev_length = self.match_length;
            self.prev_match = self.match_start;
            self.match_length = MIN_MATCH - 1;
            if hash_head != 0 && self.prev_length < MAX_LAZY && self.strstart - hash_head <= MAX_DIST {
                self.match_length = self.longest_match(hash_head);
                if self.match_length <= 5
                    && self.match_length == MIN_MATCH
                    && self.strstart - self.match_start > TOO_FAR
                {
                    self.match_length = MIN_MATCH - 1;
                }
            }
            if self.prev_length >= MIN_MATCH && self.match_length <= self.prev_length {
                let max_insert = self.strstart + self.lookahead - MIN_MATCH;
                let flush = self.tally_distance(self.strstart - 1 - self.prev_match, self.prev_length - MIN_MATCH);
                self.lookahead -= self.prev_length - 1;
                self.prev_length -= 2;
                loop {
                    self.strstart += 1;
                    if self.strstart <= max_insert {
                        self.insert_string(self.strstart);
                    }
                    self.prev_length -= 1;
                    if self.prev_length == 0 {
                        break;
                    }
                }
                self.match_available = false;
                self.match_length = MIN_MATCH - 1;
                self.strstart += 1;
                if flush {
                    self.flush_block(false);
                }
            } else if self.match_available {
                if self.tally_literal(self.window[self.strstart - 1]) {
                    self.flush_block(false);
                }
                self.strstart += 1;
                self.lookahead -= 1;
            } else {
                self.match_available = true;
                self.strstart += 1;
                self.lookahead -= 1;
            }
        }
        if self.match_available {
            self.tally_literal(self.window[self.strstart - 1]);
            self.match_available = false;
        }
        self.flush_block(true);
    }

    fn init_block(&mut self) {
        for item in &mut self.dyn_ltree[..L_CODES] {
            item.fc = 0;
        }
        for item in &mut self.dyn_dtree[..D_CODES] {
            item.fc = 0;
        }
        for item in &mut self.bl_tree[..BL_CODES] {
            item.fc = 0;
        }
        self.dyn_ltree[END_BLOCK].fc = 1;
        self.opt_len = 0;
        self.static_len = 0;
        self.symbols.clear();
    }

    fn tree(&mut self, kind: TreeKind) -> &mut [Ct] {
        match kind {
            TreeKind::Literal => &mut self.dyn_ltree,
            TreeKind::Distance => &mut self.dyn_dtree,
            TreeKind::BitLength => &mut self.bl_tree,
        }
    }

    fn smaller(tree: &[Ct], depth: &[u8], n: usize, m: usize) -> bool {
        tree[n].fc < tree[m].fc || (tree[n].fc == tree[m].fc && depth[n] <= depth[m])
    }

    fn pqdownheap(&mut self, kind: TreeKind, mut k: usize) {
        let heap_len = self.heap_len;
        let v = self.heap[k];
        let mut j = k << 1;
        let (heap, depth) = (&mut self.heap, &self.depth);
        let tree: &[Ct] = match kind {
            TreeKind::Literal => &self.dyn_ltree,
            TreeKind::Distance => &self.dyn_dtree,
            TreeKind::BitLength => &self.bl_tree,
        };
        while j <= heap_len {
            if j < heap_len && Self::smaller(tree, depth, heap[j + 1], heap[j]) {
                j += 1;
            }
            if Self::smaller(tree, depth, v, heap[j]) {
                break;
            }
            heap[k] = heap[j];
            k = j;
            j <<= 1;
        }
        heap[k] = v;
    }

    fn build_tree(&mut self, kind: TreeKind) {
        let (elems, static_tree): (usize, Option<&[Ct]>) = match kind {
            TreeKind::Literal => (L_CODES, Some(&self.tables.static_ltree[..])),
            TreeKind::Distance => (D_CODES, Some(&self.tables.static_dtree[..])),
            TreeKind::BitLength => (BL_CODES, None),
        };
        let static_lengths: Option<Vec<u16>> = static_tree.map(|tree| tree.iter().map(|item| item.dl).collect());
        let mut max_code: i64 = -1;
        self.heap_len = 0;
        self.heap_max = HEAP_SIZE;
        for n in 0..elems {
            if self.tree(kind)[n].fc != 0 {
                self.heap_len += 1;
                self.heap[self.heap_len] = n;
                max_code = n as i64;
                self.depth[n] = 0;
            } else {
                self.tree(kind)[n].dl = 0;
            }
        }
        while self.heap_len < 2 {
            let node = if max_code < 2 {
                max_code += 1;
                max_code as usize
            } else {
                0
            };
            self.heap_len += 1;
            self.heap[self.heap_len] = node;
            self.tree(kind)[node].fc = 1;
            self.depth[node] = 0;
            self.opt_len = self.opt_len.wrapping_sub(1);
            if let Some(lengths) = &static_lengths {
                self.static_len = self.static_len.wrapping_sub(lengths[node] as u64);
            }
        }
        let max_code = max_code as usize;
        match kind {
            TreeKind::Literal => self.l_max_code = max_code,
            TreeKind::Distance => self.d_max_code = max_code,
            TreeKind::BitLength => {}
        }
        for n in (1..=self.heap_len / 2).rev() {
            self.pqdownheap(kind, n);
        }
        let mut node = elems;
        loop {
            let n = self.heap[1];
            self.heap[1] = self.heap[self.heap_len];
            self.heap_len -= 1;
            self.pqdownheap(kind, 1);
            let m = self.heap[1];
            self.heap_max -= 1;
            self.heap[self.heap_max] = n;
            self.heap_max -= 1;
            self.heap[self.heap_max] = m;
            let depth = self.depth[n].max(self.depth[m]) + 1;
            let tree = self.tree(kind);
            tree[node].fc = tree[n].fc + tree[m].fc;
            tree[n].dl = node as u16;
            tree[m].dl = node as u16;
            self.depth[node] = depth;
            self.heap[1] = node;
            node += 1;
            self.pqdownheap(kind, 1);
            if self.heap_len < 2 {
                break;
            }
        }
        self.heap_max -= 1;
        self.heap[self.heap_max] = self.heap[1];
        self.gen_bitlen(kind, max_code, static_lengths.as_deref());
        let bl_count = self.bl_count;
        gen_codes(self.tree(kind), max_code, &bl_count);
    }

    fn gen_bitlen(&mut self, kind: TreeKind, max_code: usize, static_lengths: Option<&[u16]>) {
        let (extra, base, max_length): (&[u32], usize, usize) = match kind {
            TreeKind::Literal => (&EXTRA_LBITS, LITERALS + 1, MAX_BITS),
            TreeKind::Distance => (&EXTRA_DBITS, 0, MAX_BITS),
            TreeKind::BitLength => (&EXTRA_BLBITS, 0, MAX_BL_BITS),
        };
        self.bl_count = [0; MAX_BITS + 1];
        let root = self.heap[self.heap_max];
        self.tree(kind)[root].dl = 0;
        let mut overflow = 0i64;
        let mut h = self.heap_max + 1;
        while h < HEAP_SIZE {
            let n = self.heap[h];
            let tree = self.tree(kind);
            let mut bits = tree[tree[n].dl as usize].dl as usize + 1;
            if bits > max_length {
                bits = max_length;
                overflow += 1;
            }
            tree[n].dl = bits as u16;
            let frequency = tree[n].fc as u64;
            h += 1;
            if n > max_code {
                continue;
            }
            self.bl_count[bits] += 1;
            let xbits = if n >= base { extra[n - base] as u64 } else { 0 };
            self.opt_len = self.opt_len.wrapping_add(frequency * (bits as u64 + xbits));
            if let Some(lengths) = static_lengths {
                self.static_len = self.static_len.wrapping_add(frequency * (lengths[n] as u64 + xbits));
            }
        }
        if overflow == 0 {
            return;
        }
        loop {
            let mut bits = max_length - 1;
            while self.bl_count[bits] == 0 {
                bits -= 1;
            }
            self.bl_count[bits] -= 1;
            self.bl_count[bits + 1] += 2;
            self.bl_count[max_length] -= 1;
            overflow -= 2;
            if overflow <= 0 {
                break;
            }
        }
        h = HEAP_SIZE;
        for bits in (1..=max_length).rev() {
            let mut n = self.bl_count[bits];
            while n != 0 {
                h -= 1;
                let m = self.heap[h];
                if m > max_code {
                    continue;
                }
                let tree = self.tree(kind);
                if tree[m].dl as usize != bits {
                    let delta = (bits as u64).wrapping_sub(tree[m].dl as u64).wrapping_mul(tree[m].fc as u64);
                    self.opt_len = self.opt_len.wrapping_add(delta);
                    self.tree(kind)[m].dl = bits as u16;
                }
                n -= 1;
            }
        }
    }

    fn scan_tree(&mut self, kind: TreeKind, max_code: usize) {
        let mut prevlen: i64 = -1;
        let mut nextlen = self.tree(kind)[0].dl as i64;
        let mut count = 0;
        let (mut max_count, mut min_count) = if nextlen == 0 { (138, 3) } else { (7, 4) };
        self.tree(kind)[max_code + 1].dl = 0xffff;
        for n in 0..=max_code {
            let curlen = nextlen;
            nextlen = self.tree(kind)[n + 1].dl as i64;
            count += 1;
            if count < max_count && curlen == nextlen {
                continue;
            } else if count < min_count {
                self.bl_tree[curlen as usize].fc += count as u16;
            } else if curlen != 0 {
                if curlen != prevlen {
                    self.bl_tree[curlen as usize].fc += 1;
                }
                self.bl_tree[REP_3_6].fc += 1;
            } else if count <= 10 {
                self.bl_tree[REPZ_3_10].fc += 1;
            } else {
                self.bl_tree[REPZ_11_138].fc += 1;
            }
            count = 0;
            prevlen = curlen;
            (max_count, min_count) = if nextlen == 0 {
                (138, 3)
            } else if curlen == nextlen {
                (6, 3)
            } else {
                (7, 4)
            };
        }
    }

    fn send_code(&mut self, code: usize, kind: TreeKind) {
        let item = match kind {
            TreeKind::Literal => self.dyn_ltree[code],
            TreeKind::Distance => self.dyn_dtree[code],
            TreeKind::BitLength => self.bl_tree[code],
        };
        self.bits.send(item.fc as u32, item.dl as u32);
    }

    fn send_tree(&mut self, kind: TreeKind, max_code: usize) {
        let mut prevlen: i64 = -1;
        let mut nextlen = self.tree(kind)[0].dl as i64;
        let mut count: u32 = 0;
        let (mut max_count, mut min_count) = if nextlen == 0 { (138, 3) } else { (7, 4) };
        for n in 0..=max_code {
            let curlen = nextlen;
            nextlen = self.tree(kind)[n + 1].dl as i64;
            count += 1;
            if count < max_count && curlen == nextlen {
                continue;
            } else if count < min_count {
                while count != 0 {
                    self.send_code(curlen as usize, TreeKind::BitLength);
                    count -= 1;
                }
            } else if curlen != 0 {
                if curlen != prevlen {
                    self.send_code(curlen as usize, TreeKind::BitLength);
                    count -= 1;
                }
                self.send_code(REP_3_6, TreeKind::BitLength);
                self.bits.send(count - 3, 2);
            } else if count <= 10 {
                self.send_code(REPZ_3_10, TreeKind::BitLength);
                self.bits.send(count - 3, 3);
            } else {
                self.send_code(REPZ_11_138, TreeKind::BitLength);
                self.bits.send(count - 11, 7);
            }
            count = 0;
            prevlen = curlen;
            (max_count, min_count) = if nextlen == 0 {
                (138, 3)
            } else if curlen == nextlen {
                (6, 3)
            } else {
                (7, 4)
            };
        }
    }

    fn build_bl_tree(&mut self) -> usize {
        self.scan_tree(TreeKind::Literal, self.l_max_code);
        self.scan_tree(TreeKind::Distance, self.d_max_code);
        self.build_tree(TreeKind::BitLength);
        let mut max_blindex = BL_CODES - 1;
        while max_blindex >= 3 {
            if self.bl_tree[BL_ORDER[max_blindex]].dl != 0 {
                break;
            }
            max_blindex -= 1;
        }
        self.opt_len = self.opt_len.wrapping_add(3 * (max_blindex as u64 + 1) + 5 + 5 + 4);
        max_blindex
    }

    fn compress_block(&mut self, dynamic: bool) {
        let symbols = std::mem::take(&mut self.symbols);
        let code_of = |state: &State, kind: TreeKind, code: usize| -> Ct {
            match (dynamic, kind) {
                (true, TreeKind::Literal) => state.dyn_ltree[code],
                (true, _) => state.dyn_dtree[code],
                (false, TreeKind::Literal) => state.tables.static_ltree[code],
                (false, _) => state.tables.static_dtree[code],
            }
        };
        for &(distance, length) in &symbols {
            if distance == 0 {
                let item = code_of(self, TreeKind::Literal, length as usize);
                self.bits.send(item.fc as u32, item.dl as u32);
                continue;
            }
            let mut lc = length as u32;
            let code = self.tables.length_code[lc as usize] as usize;
            let item = code_of(self, TreeKind::Literal, code + LITERALS + 1);
            self.bits.send(item.fc as u32, item.dl as u32);
            let extra = EXTRA_LBITS[code];
            if extra != 0 {
                lc -= self.tables.base_length[code];
                self.bits.send(lc, extra);
            }
            let mut dist = distance as u32 - 1;
            let code = self.tables.d_code(dist as usize);
            let item = code_of(self, TreeKind::Distance, code);
            self.bits.send(item.fc as u32, item.dl as u32);
            let extra = EXTRA_DBITS[code];
            if extra != 0 {
                dist -= self.tables.base_dist[code];
                self.bits.send(dist, extra);
            }
        }
        let item = code_of(self, TreeKind::Literal, END_BLOCK);
        self.bits.send(item.fc as u32, item.dl as u32);
        self.symbols = symbols;
    }

    fn tr_flush_block(&mut self, stored: Option<(usize, usize)>, stored_len: u64, last: bool) {
        self.build_tree(TreeKind::Literal);
        self.build_tree(TreeKind::Distance);
        let max_blindex = self.build_bl_tree();
        let mut opt_lenb = self.opt_len.wrapping_add(3 + 7) >> 3;
        let static_lenb = self.static_len.wrapping_add(3 + 7) >> 3;
        if static_lenb <= opt_lenb {
            opt_lenb = static_lenb;
        }
        let last_bit = u32::from(last);
        match stored {
            Some((start, length)) if stored_len + 4 <= opt_lenb => {
                self.bits.send(last_bit, 3);
                self.bits.windup();
                let length16 = length as u16;
                self.bits.out.extend_from_slice(&length16.to_le_bytes());
                self.bits.out.extend_from_slice(&(!length16).to_le_bytes());
                self.bits.out.extend_from_slice(&self.window[start..start + length]);
            }
            _ if static_lenb == opt_lenb => {
                self.bits.send((1 << 1) + last_bit, 3);
                self.compress_block(false);
            }
            _ => {
                self.bits.send((2 << 1) + last_bit, 3);
                let (lcodes, dcodes, blcodes) = (self.l_max_code + 1, self.d_max_code + 1, max_blindex + 1);
                self.bits.send(lcodes as u32 - 257, 5);
                self.bits.send(dcodes as u32 - 1, 5);
                self.bits.send(blcodes as u32 - 4, 4);
                for rank in BL_ORDER.iter().take(blcodes) {
                    self.bits.send(self.bl_tree[*rank].dl as u32, 3);
                }
                self.send_tree(TreeKind::Literal, lcodes - 1);
                self.send_tree(TreeKind::Distance, dcodes - 1);
                self.compress_block(true);
            }
        }
        self.init_block();
        if last {
            self.bits.windup();
        }
    }
}

/// The CRC-32 of `data` (`zlib.crc32`).
pub fn crc32(data: &[u8]) -> u32 {
    let mut table = [0u32; 256];
    for (n, entry) in table.iter_mut().enumerate() {
        let mut c = n as u32;
        for _ in 0..8 {
            c = if c & 1 != 0 { 0xedb8_8320 ^ (c >> 1) } else { c >> 1 };
        }
        *entry = c;
    }
    let mut crc = 0xffff_ffffu32;
    for byte in data {
        crc = table[((crc ^ *byte as u32) & 0xff) as usize] ^ (crc >> 8);
    }
    crc ^ 0xffff_ffff
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sha256(data: &[u8]) -> String {
        crate::files::sha256_hex(data)
    }

    /// Recorded from CPython 3.13's `zlib` (zlib 1.3).
    #[test]
    fn matches_a_recorded_zlib_stream() {
        let data: Vec<u8> = (0..3000)
            .flat_map(|i: u32| {
                format!("line {} of the deflate fixture, repeated words words words\n", i * i % 97).into_bytes()
            })
            .collect();
        let compressed = deflate_raw(&data);
        assert_eq!(compressed.len(), 2029);
        assert_eq!(sha256(&compressed), "dfbacc2e27209655896ef6719e45d2b1565caecf8be253d7e882beca7332ab66");
    }

    /// Recorded from `zipfile` on Linux (`create_system` 3).
    #[cfg(not(windows))]
    #[test]
    fn matches_a_recorded_zipfile_archive() {
        let entries = [
            ("gerbers/a-F_Cu.gbr".to_owned(), b"G04 test*\n%FSLAX46Y46*%\n".repeat(50)),
            ("drill/b\u{e9}.drl".to_owned(), b"M48\nT1C0.3\n".repeat(20)),
            ("SHA256SUMS".to_owned(), Vec::new()),
        ];
        let archive = crate::zip::archive(&entries);
        assert_eq!(
            (archive.len(), sha256(&archive).as_str()),
            (387, "bdc82b87812a8ee72cb73ee6f16176891f0e86458130fe7fb5afb4b282a53901")
        );
    }

    /// Compare with streams `zlib.compressobj(-1, DEFLATED, -15)` wrote for
    /// the inputs in `$COPPER_DEFLATE_CORPUS` (`N.bin` and `N.def`).
    #[test]
    fn matches_zlib_streams() {
        let Ok(directory) = std::env::var("COPPER_DEFLATE_CORPUS") else { return };
        let mut index = 0;
        while let Ok(data) = std::fs::read(format!("{directory}/{index}.bin")) {
            let expected = std::fs::read(format!("{directory}/{index}.def")).unwrap();
            assert!(deflate_raw(&data) == expected, "case {index} ({} bytes)", data.len());
            index += 1;
        }
        assert!(index > 0);
    }
}
