// SPDX-License-Identifier: GPL-3.0-or-later

//! The accumulator update and the output layer written against two safe SIMD libraries, to be timed
//! against the portable loops and checked against them bit for bit; no search calls any of it.

use cadence_core::features::feature_index;
use cadence_core::position::Board;
use cadence_core::{Colour, DirtyPieces};

use crate::accumulator::Accumulator;
use crate::net::{Network, QA, QB, SCALE};

const COLOURS: [Colour; 2] = [Colour::White, Colour::Black];

/// The activation's ceiling at the lanes' own width.
const CEILING: i16 = QA as i16;

/// The portable form's order: every subtraction before any addition, perspective by perspective.
#[inline(always)]
fn apply<const H: usize>(
    net: &Network<H>,
    acc: &mut Accumulator<H>,
    dirty: &DirtyPieces,
    sub: impl Fn(&mut [i16; H], &[i16; H]),
    add: impl Fn(&mut [i16; H], &[i16; H]),
) {
    for persp in COLOURS {
        let sums = &mut acc.sums[persp.index()];
        for d in dirty.as_slice() {
            if let Some(from) = d.from.get() {
                sub(
                    &mut *sums,
                    &net.weights[feature_index(persp, d.piece, from)],
                );
            }
        }
        for d in dirty.as_slice() {
            if let Some(to) = d.to.get() {
                add(&mut *sums, &net.weights[feature_index(persp, d.piece, to)]);
            }
        }
    }
}

/// Every piece added to the biases through the given column addition.
#[inline(always)]
fn refresh_with<const H: usize>(
    net: &Network<H>,
    board: &Board,
    add: impl Fn(&mut [i16; H], &[i16; H]),
) -> Accumulator<H> {
    let mut acc = Accumulator {
        sums: [net.bias; 2],
    };
    for sq in board.occupied() {
        if let Some(piece) = board.piece_at(sq) {
            for persp in COLOURS {
                add(
                    &mut acc.sums[persp.index()],
                    &net.weights[feature_index(persp, piece, sq)],
                );
            }
        }
    }
    acc
}

/// The output layer's lanes one at a time, for the tail a vector does not cover.
fn scalar_layer(sums: &[i16], weights: &[i16]) -> i32 {
    sums.iter().zip(weights).fold(0_i32, |acc, (&s, &w)| {
        let c = s.clamp(0, CEILING);
        acc.wrapping_add(i32::from(c.wrapping_mul(w)) * i32::from(c))
    })
}

/// The portable form's final scaling, unchanged.
fn finish(output_bias: i16, sum: i64) -> i32 {
    let value = round_div(
        (sum / i64::from(QA) + i64::from(output_bias)) * i64::from(SCALE),
        i64::from(QA * QB),
    );
    i32::try_from(value).unwrap_or_else(|_| unreachable!("the load check bounds the sum"))
}

fn round_div(n: i64, d: i64) -> i64 {
    let half = d / 2;
    if n >= 0 {
        (n + half) / d
    } else {
        (n - half) / d
    }
}

#[cfg(feature = "simd-wide")]
mod via_wide {
    use wide::{i16x16, i32x8};

    use super::{CEILING, scalar_layer};

    #[inline(always)]
    pub(super) fn add<const H: usize>(sums: &mut [i16; H], column: &[i16; H]) {
        let (s, s_tail) = sums.as_chunks_mut::<16>();
        let (c, c_tail) = column.as_chunks::<16>();
        for (s, c) in s.iter_mut().zip(c) {
            *s = (i16x16::new(*s) + i16x16::new(*c)).to_array();
        }
        for (s, &c) in s_tail.iter_mut().zip(c_tail) {
            *s = s.wrapping_add(c);
        }
    }

    #[inline(always)]
    pub(super) fn sub<const H: usize>(sums: &mut [i16; H], column: &[i16; H]) {
        let (s, s_tail) = sums.as_chunks_mut::<16>();
        let (c, c_tail) = column.as_chunks::<16>();
        for (s, c) in s.iter_mut().zip(c) {
            *s = (i16x16::new(*s) - i16x16::new(*c)).to_array();
        }
        for (s, &c) in s_tail.iter_mut().zip(c_tail) {
            *s = s.wrapping_sub(c);
        }
    }

    /// Clamp, the product in `i16`, then the second product widened and summed in pairs.
    #[inline(always)]
    pub(super) fn layer<const H: usize>(sums: &[i16; H], weights: &[i16; H]) -> i32 {
        let (s, s_tail) = sums.as_chunks::<16>();
        let (w, w_tail) = weights.as_chunks::<16>();
        let zero = i16x16::ZERO;
        let ceiling = i16x16::splat(CEILING);
        let mut acc = i32x8::ZERO;
        for (s, w) in s.iter().zip(w) {
            let c = i16x16::new(*s).max(zero).min(ceiling);
            let cw = c * i16x16::new(*w);
            acc += cw.dot(c);
        }
        acc.reduce_add().wrapping_add(scalar_layer(s_tail, w_tail))
    }
}

#[cfg(feature = "simd-pulp")]
mod via_pulp {
    use pulp::{Arch, Simd, WithSimd};

    use super::{Accumulator, DirtyPieces, Network, apply, scalar_layer};

    #[inline(always)]
    pub(super) fn add<S: Simd, const H: usize>(simd: S, sums: &mut [i16; H], column: &[i16; H]) {
        let (s, s_tail) = S::as_mut_simd_i16s(sums);
        let (c, c_tail) = S::as_simd_i16s(column);
        for (s, &c) in s.iter_mut().zip(c) {
            *s = simd.add_i16s(*s, c);
        }
        for (s, &c) in s_tail.iter_mut().zip(c_tail) {
            *s = s.wrapping_add(c);
        }
    }

    #[inline(always)]
    pub(super) fn sub<S: Simd, const H: usize>(simd: S, sums: &mut [i16; H], column: &[i16; H]) {
        let (s, s_tail) = S::as_mut_simd_i16s(sums);
        let (c, c_tail) = S::as_simd_i16s(column);
        for (s, &c) in s.iter_mut().zip(c) {
            *s = simd.sub_i16s(*s, c);
        }
        for (s, &c) in s_tail.iter_mut().zip(c_tail) {
            *s = s.wrapping_sub(c);
        }
    }

    struct Update<'a, const H: usize> {
        net: &'a Network<H>,
        acc: &'a mut Accumulator<H>,
        dirty: &'a DirtyPieces,
    }

    impl<const H: usize> WithSimd for Update<'_, H> {
        type Output = ();

        #[inline(always)]
        fn with_simd<S: Simd>(self, simd: S) -> Self::Output {
            apply(
                self.net,
                self.acc,
                self.dirty,
                |s, c| sub(simd, s, c),
                |s, c| add(simd, s, c),
            );
        }
    }

    /// One dispatch a move, as a search holding no SIMD token of its own would pay.
    pub(super) fn update<const H: usize>(
        net: &Network<H>,
        parent: &Accumulator<H>,
        dirty: &DirtyPieces,
    ) -> Accumulator<H> {
        let mut acc = *parent;
        Arch::new().dispatch(Update {
            net,
            acc: &mut acc,
            dirty,
        });
        acc
    }

    struct Refresh<'a, const H: usize> {
        net: &'a Network<H>,
        board: &'a cadence_core::position::Board,
    }

    impl<const H: usize> WithSimd for Refresh<'_, H> {
        type Output = Accumulator<H>;

        #[inline(always)]
        fn with_simd<S: Simd>(self, simd: S) -> Accumulator<H> {
            super::refresh_with(self.net, self.board, |s, c| add(simd, s, c))
        }
    }

    pub(super) fn refresh<const H: usize>(
        net: &Network<H>,
        board: &cadence_core::position::Board,
    ) -> Accumulator<H> {
        Arch::new().dispatch(Refresh { net, board })
    }

    /// Both perspectives' layers under one dispatch, the widening step from the target's own
    /// safe intrinsics because the portable trait has no widening multiply for `i16`.
    pub(super) fn layers<const H: usize>(
        us: &[i16; H],
        w_us: &[i16; H],
        them: &[i16; H],
        w_them: &[i16; H],
    ) -> (i32, i32) {
        arch_layers(us, w_us, them, w_them)
            .unwrap_or_else(|| (scalar_layer(us, w_us), scalar_layer(them, w_them)))
    }

    /// Which paths the two kernels take on this machine.
    pub(super) fn path() -> String {
        format!("dispatch {:?}, output layer {}", Arch::new(), arch_name())
    }

    struct Layers<'a, T, const H: usize> {
        simd: T,
        us: &'a [i16; H],
        w_us: &'a [i16; H],
        them: &'a [i16; H],
        w_them: &'a [i16; H],
    }

    #[cfg(target_arch = "x86_64")]
    fn arch_name() -> &'static str {
        if pulp::x86::V3::try_new().is_some() {
            "V3 (AVX2, runtime-detected)"
        } else {
            "scalar"
        }
    }

    #[cfg(target_arch = "x86_64")]
    fn arch_layers<const H: usize>(
        us: &[i16; H],
        w_us: &[i16; H],
        them: &[i16; H],
        w_them: &[i16; H],
    ) -> Option<(i32, i32)> {
        let simd = pulp::x86::V3::try_new()?;
        Some(simd.vectorize(Layers {
            simd,
            us,
            w_us,
            them,
            w_them,
        }))
    }

    #[cfg(target_arch = "x86_64")]
    impl<const H: usize> pulp::NullaryFnOnce for Layers<'_, pulp::x86::V3, H> {
        type Output = (i32, i32);

        #[inline(always)]
        fn call(self) -> (i32, i32) {
            (
                layer_v3(self.simd, self.us, self.w_us),
                layer_v3(self.simd, self.them, self.w_them),
            )
        }
    }

    #[cfg(target_arch = "x86_64")]
    #[inline(always)]
    fn layer_v3<const H: usize>(simd: pulp::x86::V3, sums: &[i16; H], weights: &[i16; H]) -> i32 {
        use pulp::x86::V3;
        let (s, s_tail) = V3::as_simd_i16s(sums);
        let (w, w_tail) = V3::as_simd_i16s(weights);
        let zero = simd.splat_i16s(0);
        let ceiling = simd.splat_i16s(super::CEILING);
        let mut acc = simd.splat_i32s(0);
        for (&s, &w) in s.iter().zip(w) {
            let c = simd.min_i16s(simd.max_i16s(s, zero), ceiling);
            let cw = simd.mul_i16s(c, w);
            acc = simd.add_i32s(acc, simd.multiply_wrapping_add_adjacent_i16x16(cw, c));
        }
        let lanes: [i32; 8] = pulp::cast(acc);
        lanes
            .iter()
            .fold(scalar_layer(s_tail, w_tail), |a, &b| a.wrapping_add(b))
    }

    #[cfg(target_arch = "aarch64")]
    fn arch_name() -> &'static str {
        if pulp::aarch64::Neon::try_new().is_some() {
            "NEON"
        } else {
            "scalar"
        }
    }

    #[cfg(target_arch = "aarch64")]
    fn arch_layers<const H: usize>(
        us: &[i16; H],
        w_us: &[i16; H],
        them: &[i16; H],
        w_them: &[i16; H],
    ) -> Option<(i32, i32)> {
        let simd = pulp::aarch64::Neon::try_new()?;
        Some(simd.vectorize(Layers {
            simd,
            us,
            w_us,
            them,
            w_them,
        }))
    }

    #[cfg(target_arch = "aarch64")]
    impl<const H: usize> pulp::NullaryFnOnce for Layers<'_, pulp::aarch64::Neon, H> {
        type Output = (i32, i32);

        #[inline(always)]
        fn call(self) -> (i32, i32) {
            (
                layer_neon(self.simd, self.us, self.w_us),
                layer_neon(self.simd, self.them, self.w_them),
            )
        }
    }

    #[cfg(target_arch = "aarch64")]
    #[inline(always)]
    fn layer_neon<const H: usize>(
        simd: pulp::aarch64::Neon,
        sums: &[i16; H],
        weights: &[i16; H],
    ) -> i32 {
        use core::arch::aarch64::int16x8_t;
        use pulp::aarch64::Neon;
        let n = simd.neon;
        let (s, s_tail) = Neon::as_simd_i16s(sums);
        let (w, w_tail) = Neon::as_simd_i16s(weights);
        let zero = simd.splat_i16s(0);
        let ceiling = simd.splat_i16s(super::CEILING);
        let mut lo = n.vdupq_n_s32(0);
        let mut hi = n.vdupq_n_s32(0);
        for (&s, &w) in s.iter().zip(w) {
            let c = simd.min_i16s(simd.max_i16s(s, zero), ceiling);
            let cw = simd.mul_i16s(c, w);
            let c: int16x8_t = pulp::cast(c);
            let cw: int16x8_t = pulp::cast(cw);
            lo = n.vmlal_s16(lo, n.vget_low_s16(cw), n.vget_low_s16(c));
            hi = n.vmlal_high_s16(hi, cw, c);
        }
        n.vaddvq_s32(n.vaddq_s32(lo, hi))
            .wrapping_add(scalar_layer(s_tail, w_tail))
    }

    #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
    fn arch_name() -> &'static str {
        "scalar"
    }

    #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
    fn arch_layers<const H: usize>(
        _: &[i16; H],
        _: &[i16; H],
        _: &[i16; H],
        _: &[i16; H],
    ) -> Option<(i32, i32)> {
        None
    }
}

/// The paths the pulp variant takes on this machine, for the measurement's header.
#[cfg(feature = "simd-pulp")]
#[must_use]
pub fn pulp_path() -> String {
    via_pulp::path()
}

#[cfg(feature = "simd-wide")]
impl<const H: usize> Network<H> {
    #[must_use]
    pub fn refresh_wide(&self, board: &Board) -> Accumulator<H> {
        refresh_with(self, board, |s, c| via_wide::add(s, c))
    }

    #[must_use]
    pub fn update_wide(&self, parent: &Accumulator<H>, dirty: &DirtyPieces) -> Accumulator<H> {
        let mut acc = *parent;
        apply(
            self,
            &mut acc,
            dirty,
            |s, c| via_wide::sub(s, c),
            |s, c| via_wide::add(s, c),
        );
        acc
    }

    #[must_use]
    pub fn evaluate_wide(&self, acc: &Accumulator<H>, stm: Colour) -> i32 {
        let us = &acc.sums[stm.index()];
        let them = &acc.sums[stm.flip().index()];
        let sum = i64::from(via_wide::layer(us, &self.output[0]))
            + i64::from(via_wide::layer(them, &self.output[1]));
        finish(self.output_bias, sum)
    }
}

#[cfg(feature = "simd-pulp")]
impl<const H: usize> Network<H> {
    #[must_use]
    pub fn refresh_pulp(&self, board: &Board) -> Accumulator<H> {
        via_pulp::refresh(self, board)
    }

    #[must_use]
    pub fn update_pulp(&self, parent: &Accumulator<H>, dirty: &DirtyPieces) -> Accumulator<H> {
        via_pulp::update(self, parent, dirty)
    }

    #[must_use]
    pub fn evaluate_pulp(&self, acc: &Accumulator<H>, stm: Colour) -> i32 {
        let (us, them) = via_pulp::layers(
            &acc.sums[stm.index()],
            &self.output[0],
            &acc.sums[stm.flip().index()],
            &self.output[1],
        );
        finish(self.output_bias, i64::from(us) + i64::from(them))
    }
}
