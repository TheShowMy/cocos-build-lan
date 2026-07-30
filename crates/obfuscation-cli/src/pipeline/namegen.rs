use std::collections::HashSet;

use anyhow::{Result, bail};
use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};

#[derive(Debug, Clone)]
pub struct NameGenerator {
    min_len: usize,
    max_len: usize,
    rng: StdRng,
    used: HashSet<String>,
    blocked: HashSet<String>,
}

impl NameGenerator {
    pub fn new(
        min_len: usize,
        max_len: usize,
        seed: Option<u64>,
        blocked: HashSet<String>,
    ) -> Result<Self> {
        if min_len == 0 || max_len < min_len {
            bail!("invalid name length range: {}..={}", min_len, max_len);
        }
        let rng = match seed {
            Some(s) => StdRng::seed_from_u64(s),
            None => {
                let mut trng = rand::rng();
                StdRng::from_rng(&mut trng)
            }
        };

        Ok(Self {
            min_len,
            max_len,
            rng,
            used: HashSet::new(),
            blocked,
        })
    }

    pub fn next_name(&mut self) -> Result<String> {
        const MAX_RETRY: usize = 10_000;
        for _ in 0..MAX_RETRY {
            let len = self.rng.random_range(self.min_len..=self.max_len);
            let mut s = String::with_capacity(len);
            // 首字符限定为字母，兼容常见 JS 标识符规则。
            s.push(random_letter(&mut self.rng));
            for _ in 1..len {
                s.push(random_alnum(&mut self.rng));
            }
            if self.blocked.contains(&s) || self.used.contains(&s) {
                continue;
            }
            self.used.insert(s.clone());
            return Ok(s);
        }
        bail!("unable to allocate unique obfuscated name after retries")
    }
}

fn random_letter(rng: &mut StdRng) -> char {
    const LETTERS: &[u8] = b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ";
    LETTERS[rng.random_range(0..LETTERS.len())] as char
}

fn random_alnum(rng: &mut StdRng) -> char {
    const ALNUM: &[u8] = b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";
    ALNUM[rng.random_range(0..ALNUM.len())] as char
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_names_match_constraints() {
        let mut g = NameGenerator::new(8, 14, Some(42), HashSet::new()).unwrap();
        for _ in 0..1000 {
            let s = g.next_name().unwrap();
            assert!((8..=14).contains(&s.len()));
            assert!(s.chars().next().unwrap().is_ascii_alphabetic());
            assert!(s.chars().all(|c| c.is_ascii_alphanumeric()));
        }
    }
}
