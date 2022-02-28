//! This module implements the [Goldwasser-Goldreich-Micali
//! PRF](https://crypto.stanford.edu/pbc/notes/crypto/ggm.html), along
//! with extended functionality that allows puncturing inputs from
//! secret keys.

use std::fmt;

use super::PPRF;
use bitvec::prelude::*;
use ring::{
    hmac,
    rand::{SecureRandom, SystemRandom},
};

#[derive(Debug)]
enum GGMError {
    NoPrefixFound,
    AlreadyPunctured,
}

#[derive(Clone, Eq, PartialEq)]
struct Prefix {
    bits: BitVec<bitvec::order::Lsb0, usize>,
}

impl Prefix {
    fn new(bits: BitVec<bitvec::order::Lsb0, usize>) -> Self {
        Prefix { bits }
    }

    fn len(&self) -> usize {
        self.bits.len()
    }
}

impl fmt::Debug for Prefix {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Prefix").field("bits", &self.bits).finish()
    }
}

#[derive(Clone)]
struct GGMPseudorandomGenerator {
    key: ring::hmac::Key,
}

impl GGMPseudorandomGenerator {
    fn setup() -> Self {
        let secret = sample_secret();
        let s_key = hmac::Key::new(hmac::HMAC_SHA256, secret.as_ref());
        GGMPseudorandomGenerator { key: s_key }
    }

    fn eval(&self, input: &[u8], output: &mut [u8]) {
        let tag = hmac::sign(&self.key, input);
        output.copy_from_slice(tag.as_ref());
    }
}

#[derive(Clone)]
struct GGMPuncturableKey {
    prgs: Vec<GGMPseudorandomGenerator>,
    prefixes: Vec<(Prefix, Vec<u8>)>,
    punctured: Vec<Prefix>,
}

// TODO: remove copies/clones
impl GGMPuncturableKey {
    fn new() -> Self {
        let secret = sample_secret();
        // Setup PRGs and initial tree
        let prg0 = GGMPseudorandomGenerator::setup();
        let mut out0 = vec![0u8; 32];
        prg0.eval(&secret, &mut out0);
        let prg1 = GGMPseudorandomGenerator::setup();
        let mut out1 = vec![0u8; 32];
        prg1.eval(&secret, &mut out1);
        GGMPuncturableKey {
            prgs: vec![prg0, prg1],
            prefixes: vec![
                (Prefix::new(bits![0].to_bitvec()), out0),
                (Prefix::new(bits![1].to_bitvec()), out1),
            ],
            punctured: vec![],
        }
    }

    fn find_prefix(&self, bv: &BitVec) -> Result<(Prefix, Vec<u8>), GGMError> {
        let key_prefixes = self.prefixes.clone();
        for prefix in key_prefixes {
            let bits = &prefix.0.bits;
            if bv.starts_with(bits) {
                return Ok(prefix);
            }
        }
        Err(GGMError::NoPrefixFound)
    }

    fn puncture(
        &mut self,
        pfx: &Prefix,
        to_punc: &Prefix,
        new_prefixes: Vec<(Prefix, Vec<u8>)>,
    ) -> Result<(), GGMError> {
        if self.punctured.iter().any(|p| p.bits == pfx.bits) {
            return Err(GGMError::AlreadyPunctured);
        }
        if let Some(index) = self.prefixes.iter().position(|p| p.0.bits == pfx.bits) {
            self.prefixes.remove(index);
            if !new_prefixes.is_empty() {
                self.prefixes.extend(new_prefixes);
            }
            self.punctured.push(to_punc.clone());
            return Ok(());
        }
        Err(GGMError::NoPrefixFound)
    }
}

#[derive(Clone)]
pub struct GGM {
    inp_len: usize,
    key: GGMPuncturableKey,
}

impl GGM {
    fn bit_eval(&self, bits: &BitVec, prg_inp: &[u8], output: &mut [u8]) {
        let mut eval = prg_inp.to_vec();
        for bit in bits {
            let prg = if *bit {
                &self.key.prgs[1]
            } else {
                &self.key.prgs[0]
            };
            prg.eval(&eval.clone(), &mut eval);
        }
        output.copy_from_slice(&eval);
    }

    fn partial_eval(&self, input_bits: &mut BitVec, output: &mut [u8]) -> Result<(), GGMError> {
        let res = self.key.find_prefix(input_bits);
        if let Ok(pfx) = res {
            let tail = pfx.1;
            let (_, right) = input_bits.split_at(pfx.0.bits.len());
            self.bit_eval(&right.to_bitvec(), &tail, output);
            return Ok(());
        }
        Err(GGMError::NoPrefixFound)
    }
}

impl PPRF for GGM {
    fn setup() -> Self {
        GGM {
            inp_len: 1,
            key: GGMPuncturableKey::new(),
        }
    }

    fn eval(&self, input: &[u8], output: &mut [u8]) {
        if input.len() != self.inp_len {
            panic!(
                "Input length ({}) does not match input param ({})",
                input.len(),
                self.inp_len
            );
        }
        let mut input_bits = bvcast_u8_to_usize(&BitVec::<Lsb0, _>::from_slice(input).unwrap());
        if let Err(e) = self.partial_eval(&mut input_bits, output) {
            panic!("Error occurred for {:?}: {:?}", input, e);
        }
    }

    fn puncture(&mut self, input: &[u8]) {
        if input.len() != self.inp_len {
            panic!(
                "Input length ({}) does not match input param ({})",
                input.len(),
                self.inp_len
            );
        }
        let bv = bvcast_u8_to_usize(&BitVec::<Lsb0, _>::from_slice(input).unwrap());
        if let Ok(pfx) = self.key.find_prefix(&bv) {
            let pfx_len = pfx.0.len();

            // If the prfix is smaller than the current input, then we
            // need to recompute some parts of the tree. Otherwise we
            // just remove the prefix entirely.
            let mut new_pfxs: Vec<(Prefix, Vec<u8>)> = Vec::new();
            if pfx_len != bv.len() {
                let mut iter_bv = bv.clone();
                for i in (0..bv.len()).rev() {
                    if let Some((last, rest)) = iter_bv.clone().split_last() {
                        let mut cbv = iter_bv.clone();
                        cbv.set(i, !*last);
                        let mut out = vec![0u8; 32];
                        let (_, split) = cbv.split_at(pfx_len);
                        self.bit_eval(&split.to_bitvec(), &pfx.1, &mut out);
                        new_pfxs.push((Prefix::new(cbv), out));
                        if rest.len() == pfx_len {
                            // we don't want to recompute any further
                            break;
                        }
                        iter_bv = rest.to_bitvec();
                    } else {
                        panic!("Unexpected end of input");
                    }
                }
            }

            if let Err(e) = self.key.puncture(&pfx.0, &Prefix::new(bv), new_pfxs) {
                panic!("Problem puncturing key: {:?}", e);
            }
        } else {
            panic!("No prefix found");
        }
    }
}

fn sample_secret() -> Vec<u8> {
    let rng = SystemRandom::new();
    let mut out = vec![0u8; 32];
    if let Err(e) = rng.fill(&mut out) {
        panic!("{}", e);
    }
    out
}

fn bvcast_u8_to_usize(
    bv_u8: &BitVec<bitvec::order::Lsb0, u8>,
) -> BitVec<bitvec::order::Lsb0, usize> {
    let mut bv_us = BitVec::with_capacity(bv_u8.len());
    for i in 0..bv_u8.len() {
        bv_us.push(bv_u8[i]);
    }
    bv_us
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn eval() {
        let ggm = GGM::setup();
        let x0 = [8u8];
        let x1 = [7u8];
        let mut out = [0u8; 32];
        ggm.eval(&x0, &mut out);
        ggm.eval(&x1, &mut out);
    }

    #[test]
    #[should_panic(expected = "NoPrefixFound")]
    fn puncture_fail_eval() {
        let mut ggm = GGM::setup();
        let x0 = [8u8];
        let mut out = [0u8; 32];
        ggm.eval(&x0, &mut out);
        ggm.puncture(&x0);
        // next step should panic
        ggm.eval(&x0, &mut out);
    }

    #[test]
    #[should_panic(expected = "NoPrefixFound")]
    fn mult_puncture_fail_eval() {
        let mut ggm = GGM::setup();
        let x0 = [0u8];
        let x1 = [1u8];
        ggm.puncture(&x0);
        ggm.puncture(&x1);
        // next step should panic
        ggm.eval(&x0, &mut [0u8; 32]);
    }

    #[test]
    fn puncture_eval_consistent() {
        let mut ggm = GGM::setup();
        let inputs = [[2u8], [4u8], [8u8], [16u8], [32u8], [64u8], [128u8]];
        let x0 = [0u8];
        let mut outputs_b4 = vec![vec![0u8; 1]; inputs.len()];
        let mut outputs_after = vec![vec![0u8; 1]; inputs.len()];
        for (i, x) in inputs.iter().enumerate() {
            let mut out = vec![0u8; 32];
            ggm.eval(x, &mut out);
            outputs_b4[i] = out;
        }
        ggm.puncture(&x0);
        for (i, x) in inputs.iter().enumerate() {
            let mut out = vec![0u8; 32];
            ggm.eval(x, &mut out);
            outputs_after[i] = out;
        }
        for (i, o) in outputs_b4.iter().enumerate() {
            assert_eq!(o, &outputs_after[i]);
        }
    }

    #[test]
    fn multiple_puncture() {
        let mut ggm = GGM::setup();
        let inputs = [[2u8], [4u8], [8u8], [16u8], [32u8], [64u8], [128u8]];
        let mut outputs_b4 = vec![vec![0u8; 1]; inputs.len()];
        let mut outputs_after = vec![vec![0u8; 1]; inputs.len()];
        for (i, x) in inputs.iter().enumerate() {
            let mut out = vec![0u8; 32];
            ggm.eval(x, &mut out);
            outputs_b4[i] = out;
        }
        let x0 = [0u8];
        let x1 = [1u8];
        ggm.puncture(&x0);
        for (i, x) in inputs.iter().enumerate() {
            let mut out = vec![0u8; 32];
            ggm.eval(x, &mut out);
            outputs_after[i] = out;
        }
        for (i, o) in outputs_b4.iter().enumerate() {
            assert_eq!(o, &outputs_after[i]);
        }
        ggm.puncture(&x1);
        for (i, x) in inputs.iter().enumerate() {
            let mut out = vec![0u8; 32];
            ggm.eval(x, &mut out);
            outputs_after[i] = out;
        }
        for (i, o) in outputs_b4.iter().enumerate() {
            assert_eq!(o, &outputs_after[i]);
        }
    }

    #[test]
    fn puncture_all() {
        let mut inputs = Vec::new();
        for i in 0..255 {
            inputs.push(vec![i as u8]);
        }
        let mut ggm = GGM::setup();
        for x in &inputs {
            ggm.puncture(x);
        }
    }

    #[test]
    fn casting() {
        let bv_0 = bits![0].to_bitvec();
        let bv_1 = bvcast_u8_to_usize(&BitVec::<Lsb0, _>::from_slice(&[4]).unwrap());
        assert_eq!(bv_0.len(), 1);
        assert_eq!(bv_1.len(), 8);
        assert!(bv_1.starts_with(&bv_0));
    }
}
