//! Deterministic randomness: integer hashes and a small xorshift generator.

pub(super) fn hash(mut x: u32) -> u32 {
    x ^= x >> 16;
    x = x.wrapping_mul(0x7feb_352d);
    x ^= x >> 15;
    x = x.wrapping_mul(0x846c_a68b);
    x ^= x >> 16;
    x
}

/// Stateless noise in [0, 1].
pub(super) fn hf(a: u32, b: u32) -> f32 {
    (hash(a.wrapping_mul(0x9E37_79B1) ^ hash(b)) & 0xFFFF) as f32 / 65535.0
}

/// Stateless integer in `0..n` for slot `k` of a schedule keyed by `seed` and `salt`.
pub(super) fn slot(seed: u32, salt: u32, k: u64, n: u32) -> u64 {
    u64::from(hash(hash(seed ^ salt) ^ (k as u32) ^ hash((k >> 32) as u32)) % n.max(1))
}

pub(super) struct Rng(u64);

impl Rng {
    /// Generator for one simulation tick: the same (seed, tick) always yields the same stream.
    pub(super) fn for_tick(seed: u32, tick: u64) -> Self {
        let s = (u64::from(hash(seed)) << 32) ^ tick;
        Rng(s.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1)
    }

    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }

    pub(super) fn f(&mut self) -> f32 {
        (self.next() >> 40) as f32 / (1u64 << 24) as f32
    }

    pub(super) fn range(&mut self, a: f32, b: f32) -> f32 {
        a + (b - a) * self.f()
    }

    pub(super) fn chance(&mut self, p: f32) -> bool {
        self.f() < p
    }

    pub(super) fn int(&mut self, a: u32, b: u32) -> u32 {
        a + (self.next() % u64::from(b - a + 1)) as u32
    }
}
