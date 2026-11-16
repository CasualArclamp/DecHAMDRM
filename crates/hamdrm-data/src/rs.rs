//! Reed–Solomon RS(255, k) over GF(256) with errors-and-erasures decoding, for the
//! two file-coding schemes in use (`docs/spec/app.md` §3):
//!
//! - EasyPal / QSSTV: field 0x11D, first root α¹ (Phil Karn's 1996 `rs.c`, as in QSSTV
//!   `utils/rs.cpp`);
//! - EasyDRF: field 0x187, first root α¹²⁰ (schifra, EasyDRF `common/RS/RS-coder.cpp`).
//!
//! Internally a codeword is `c[j]` = coefficient of xʲ. Decoding: syndromes,
//! Berlekamp–Massey seeded with the erasure locator, Chien search, Forney.

// Index loops over GF tables and polynomial coefficients read like the algebra.
#![allow(clippy::needless_range_loop)]

/// GF(256) with log/antilog tables.
#[derive(Debug, Clone)]
pub struct Gf {
    exp: [u8; 512],
    log: [u16; 256],
}

impl Gf {
    /// Field from its primitive polynomial (e.g. 0x11D), α = x.
    pub fn new(poly: u16) -> Self {
        let mut exp = [0u8; 512];
        let mut log = [0u16; 256];
        let mut x: u16 = 1;
        for i in 0..255 {
            exp[i] = x as u8;
            log[x as usize] = i as u16;
            x <<= 1;
            if x & 0x100 != 0 {
                x ^= poly;
            }
        }
        for i in 255..512 {
            exp[i] = exp[i - 255];
        }
        Self { exp, log }
    }

    pub fn mul(&self, a: u8, b: u8) -> u8 {
        if a == 0 || b == 0 {
            0
        } else {
            self.exp[self.log[a as usize] as usize + self.log[b as usize] as usize]
        }
    }

    pub fn div(&self, a: u8, b: u8) -> u8 {
        assert!(b != 0, "GF division by zero");
        if a == 0 {
            0
        } else {
            self.exp[(self.log[a as usize] as usize + 255 - self.log[b as usize] as usize) % 255]
        }
    }

    /// αⁿ for any integer n.
    pub fn pow_a(&self, n: i64) -> u8 {
        self.exp[n.rem_euclid(255) as usize]
    }

    pub fn inv(&self, a: u8) -> u8 {
        self.div(1, a)
    }
}

/// RS(255, k) code: field, first consecutive root α^fcr, k data symbols.
#[derive(Debug, Clone)]
pub struct RsCode {
    pub gf: Gf,
    pub fcr: i64,
    pub k: usize,
    /// Generator polynomial, `gen[j]` = coefficient of xʲ (monic, degree 255 − k).
    gen_poly: Vec<u8>,
}

pub const N: usize = 255;

impl RsCode {
    pub fn new(poly: u16, fcr: i64, k: usize) -> Self {
        let gf = Gf::new(poly);
        let mut g = vec![1u8];
        for i in 0..(N - k) as i64 {
            // g(x) ← g(x)·(x + α^(fcr+i))
            let r = gf.pow_a(fcr + i);
            let mut ng = vec![0u8; g.len() + 1];
            for (j, &c) in g.iter().enumerate() {
                ng[j + 1] ^= c;
                ng[j] ^= gf.mul(c, r);
            }
            g = ng;
        }
        Self {
            gf,
            fcr,
            k,
            gen_poly: g,
        }
    }

    pub fn parity_len(&self) -> usize {
        N - self.k
    }

    /// Parity of the message m(x) = Σ msg[i]·x^(i + 255 − k): p(x) = m(x) mod g(x),
    /// returned as `p[j]` = coefficient of xʲ.
    pub fn parity(&self, msg: &[u8]) -> Vec<u8> {
        assert_eq!(msg.len(), self.k);
        let np = self.parity_len();
        let mut rem = vec![0u8; np];
        // LFSR division, highest power first (Karn's encode_rs).
        for i in (0..self.k).rev() {
            let fb = msg[i] ^ rem[np - 1];
            for j in (1..np).rev() {
                rem[j] = rem[j - 1] ^ self.gf.mul(self.gen_poly[j], fb);
            }
            rem[0] = self.gf.mul(self.gen_poly[0], fb);
        }
        rem
    }

    /// Decode in place a codeword in x-power order (`c[j]` = coefficient of xʲ) with
    /// erasures at the given positions. Returns the number of corrected symbols, or
    /// `None` if uncorrectable (then `c` is unchanged).
    pub fn decode(&self, c: &mut [u8], erasures: &[usize]) -> Option<usize> {
        assert_eq!(c.len(), N);
        let gf = &self.gf;
        let nsym = self.parity_len();
        if erasures.len() > nsym {
            return None;
        }
        // Syndromes S_i = c(α^(fcr+i)).
        let mut s = vec![0u8; nsym];
        let mut any = false;
        for (i, si) in s.iter_mut().enumerate() {
            let x = gf.pow_a(self.fcr + i as i64);
            let mut acc = 0u8;
            for j in (0..N).rev() {
                acc = gf.mul(acc, x) ^ c[j];
            }
            *si = acc;
            any |= acc != 0;
        }
        if !any {
            return Some(0);
        }
        // Erasure locator Γ(x) = Π (1 + α^pos·x).
        let mut lambda = vec![1u8];
        for &p in erasures {
            let xp = gf.pow_a(p as i64);
            let mut nl = vec![0u8; lambda.len() + 1];
            for (j, &v) in lambda.iter().enumerate() {
                nl[j] ^= v;
                nl[j + 1] ^= gf.mul(v, xp);
            }
            lambda = nl;
        }
        // Berlekamp–Massey from r = ρ (number of erasures).
        let rho = erasures.len();
        let mut b = lambda.clone();
        let mut l = rho;
        for r in rho..nsym {
            // discrepancy Δ = Σ λ_j S_(r−j)
            let mut d = 0u8;
            for j in 0..lambda.len() {
                if j <= r {
                    d ^= gf.mul(lambda[j], s[r - j]);
                }
            }
            // b ← x·b
            b.insert(0, 0);
            if d != 0 {
                let t: Vec<u8> = {
                    let mut t = lambda.clone();
                    if t.len() < b.len() {
                        t.resize(b.len(), 0);
                    }
                    for j in 0..b.len() {
                        t[j] ^= gf.mul(d, b[j]);
                    }
                    t
                };
                if 2 * l <= r + rho {
                    l = r + 1 + rho - l;
                    let dinv = gf.inv(d);
                    b = lambda.iter().map(|&v| gf.mul(v, dinv)).collect();
                }
                lambda = t;
            }
        }
        while lambda.len() > 1 && *lambda.last().unwrap() == 0 {
            lambda.pop();
        }
        let deg = lambda.len() - 1;
        if deg == 0 || deg > nsym {
            return None;
        }
        // Chien search: roots X⁻¹ = α^(−j) give error positions j.
        let mut pos = Vec::new();
        for j in 0..N {
            let xinv = gf.pow_a(-(j as i64));
            let mut acc = 0u8;
            for &v in lambda.iter().rev() {
                acc = gf.mul(acc, xinv) ^ v;
            }
            if acc == 0 {
                pos.push(j);
            }
        }
        if pos.len() != deg {
            return None;
        }
        // Ω(x) = S(x)·Λ(x) mod x^nsym.
        let mut omega = vec![0u8; nsym];
        for i in 0..nsym {
            for j in 0..lambda.len().min(i + 1) {
                omega[i] ^= gf.mul(lambda[j], s[i - j]);
            }
        }
        // Forney: e = X^(1−fcr)·Ω(X⁻¹) / Λ'(X⁻¹).
        let mut fixed = c.to_vec();
        for &j in &pos {
            let xinv = gf.pow_a(-(j as i64));
            let mut num = 0u8;
            for &v in omega.iter().rev() {
                num = gf.mul(num, xinv) ^ v;
            }
            let mut den = 0u8;
            for (i, &v) in lambda.iter().enumerate().skip(1).step_by(2) {
                den ^= gf.mul(v, gf.pow_a(-((j * (i - 1)) as i64)));
            }
            if den == 0 {
                return None;
            }
            let e = gf.mul(gf.div(num, den), gf.pow_a(j as i64 * (1 - self.fcr)));
            fixed[j] ^= e;
        }
        // Verify.
        for i in 0..nsym {
            let x = gf.pow_a(self.fcr + i as i64);
            let mut acc = 0u8;
            for j in (0..N).rev() {
                acc = gf.mul(acc, x) ^ fixed[j];
            }
            if acc != 0 {
                return None;
            }
        }
        c.copy_from_slice(&fixed);
        Some(pos.len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lcg(seed: &mut u32) -> u32 {
        *seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
        *seed >> 8
    }

    /// Codeword in x-power order with data at x^(n−k..n) and parity at x^(0..n−k).
    fn codeword(code: &RsCode, msg: &[u8]) -> Vec<u8> {
        let mut c = code.parity(msg);
        c.extend_from_slice(msg);
        c
    }

    #[test]
    fn errors_and_erasures() {
        for (poly, fcr) in [(0x11D, 1), (0x187, 120)] {
            for k in [224, 192, 160, 128] {
                let code = RsCode::new(poly, fcr, k);
                let np = N - k;
                let mut seed = k as u32;
                for trial in 0..20 {
                    let msg: Vec<u8> = (0..k).map(|_| lcg(&mut seed) as u8).collect();
                    let good = codeword(&code, &msg);
                    let mut c = good.clone();
                    // e errors + f erasures with 2e + f ≤ np
                    let f = (trial * 7) % (np + 1);
                    let e = (np - f) / 2;
                    let mut used = std::collections::BTreeSet::new();
                    while used.len() < e + f {
                        used.insert(lcg(&mut seed) as usize % N);
                    }
                    let used: Vec<usize> = used.into_iter().collect();
                    for &p in &used {
                        c[p] ^= (lcg(&mut seed) as u8) | 1;
                    }
                    let er = &used[..f];
                    assert!(
                        code.decode(&mut c, er).is_some(),
                        "poly {poly:x} k {k} e {e} f {f}"
                    );
                    assert_eq!(c, good);
                }
            }
        }
    }
}
