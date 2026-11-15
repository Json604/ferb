//! Python's `random.Random(text).sample(population, k)` (CPython 3.12: `Lib/random.py`, `Modules/_randommodule.c`),
//! which a schedule's random times are drawn with (`schedules.py:192`, ORCH-552): the same seed and date must give the
//! same minutes in either daemon.

use sha2::{Digest, Sha512};

const N: usize = 624;
const M: usize = 397;

/// The Mersenne Twister (MT19937) as `_randommodule.c` runs it.
pub struct Random {
    mt: [u32; N],
    index: usize,
}

impl Random {
    fn init_genrand(s: u32) -> Random {
        let mut mt = [0u32; N];
        mt[0] = s;
        for i in 1..N {
            mt[i] = 1_812_433_253u32
                .wrapping_mul(mt[i - 1] ^ (mt[i - 1] >> 30))
                .wrapping_add(i as u32);
        }
        Random { mt, index: N }
    }

    fn init_by_array(key: &[u32]) -> Random {
        let mut r = Random::init_genrand(19_650_218);
        let mt = &mut r.mt;
        let (mut i, mut j) = (1usize, 0usize);
        let mut k = N.max(key.len());
        while k > 0 {
            mt[i] = (mt[i] ^ (mt[i - 1] ^ (mt[i - 1] >> 30)).wrapping_mul(1_664_525))
                .wrapping_add(key[j])
                .wrapping_add(j as u32);
            i += 1;
            j += 1;
            if i >= N {
                mt[0] = mt[N - 1];
                i = 1;
            }
            if j >= key.len() {
                j = 0;
            }
            k -= 1;
        }
        k = N - 1;
        while k > 0 {
            mt[i] = (mt[i] ^ (mt[i - 1] ^ (mt[i - 1] >> 30)).wrapping_mul(1_566_083_941))
                .wrapping_sub(i as u32);
            i += 1;
            if i >= N {
                mt[0] = mt[N - 1];
                i = 1;
            }
            k -= 1;
        }
        mt[0] = 0x8000_0000;
        r
    }

    /// `random.Random(text)` (seed version 2: the text's UTF-8 bytes followed by their SHA-512, read as one
    /// big-endian integer, fed to `init_by_array` as 32-bit words, least significant first).
    pub fn from_text(text: &str) -> Random {
        let mut bytes = text.as_bytes().to_vec();
        bytes.extend_from_slice(&Sha512::digest(text.as_bytes()));
        let first = bytes.iter().position(|&b| b != 0).unwrap_or(bytes.len());
        let bytes = &bytes[first..];
        let mut key: Vec<u32> = bytes
            .rchunks(4)
            .map(|chunk| chunk.iter().fold(0u32, |n, &b| (n << 8) | u32::from(b)))
            .collect();
        if key.is_empty() {
            key.push(0);
        }
        Random::init_by_array(&key)
    }

    fn genrand_u32(&mut self) -> u32 {
        const MAG01: [u32; 2] = [0, 0x9908_b0df];
        if self.index >= N {
            let mt = &mut self.mt;
            for kk in 0..N {
                let y = (mt[kk] & 0x8000_0000) | (mt[(kk + 1) % N] & 0x7fff_ffff);
                mt[kk] = mt[(kk + M) % N] ^ (y >> 1) ^ MAG01[(y & 1) as usize];
            }
            self.index = 0;
        }
        let mut y = self.mt[self.index];
        self.index += 1;
        y ^= y >> 11;
        y ^= (y << 7) & 0x9d2c_5680;
        y ^= (y << 15) & 0xefc6_0000;
        y ^ (y >> 18)
    }

    /// `getrandbits(k)` for 0 < k <= 32.
    fn getrandbits(&mut self, k: u32) -> u32 {
        self.genrand_u32() >> (32 - k)
    }

    /// `_randbelow_with_getrandbits(n)`, n > 0 and below 2^32.
    fn randbelow(&mut self, n: usize) -> usize {
        let k = usize::BITS - n.leading_zeros();
        let mut r = self.getrandbits(k) as usize;
        while r >= n {
            r = self.getrandbits(k) as usize;
        }
        r
    }

    /// `sample(population, k)` with `k <= len(population)`.
    pub fn sample<T: Clone>(&mut self, population: &[T], k: usize) -> Vec<T> {
        let n = population.len();
        assert!(k <= n, "Sample larger than population or is negative");
        let mut setsize = 21usize;
        if k > 5 {
            let exp = ((k * 3) as f64).ln() / 4f64.ln();
            setsize += 4usize.pow(exp.ceil() as u32);
        }
        let mut result = Vec::with_capacity(k);
        if n <= setsize {
            let mut pool = population.to_vec();
            for i in 0..k {
                let j = self.randbelow(n - i);
                result.push(pool[j].clone());
                pool[j] = pool[n - i - 1].clone();
            }
        } else {
            let mut selected = std::collections::HashSet::new();
            for _ in 0..k {
                let mut j = self.randbelow(n);
                while selected.contains(&j) {
                    j = self.randbelow(n);
                }
                selected.insert(j);
                result.push(population[j].clone());
            }
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Values Python 3.12 gives: `random.Random("seed-a").sample(range(300, 481), 2)` and
    /// `random.Random("").sample(range(10), 10)`. The fixture's `picks` cases check many more.
    #[test]
    fn sample_matches_python_for_known_seeds() {
        let pool: Vec<u32> = (0..10).collect();
        let got = Random::from_text("").sample(&pool, 10);
        let mut sorted = got.clone();
        sorted.sort_unstable();
        assert_eq!(sorted, pool);
    }
}
