// SPDX-License-Identifier: GPL-3.0-or-later

//! The UCI options, the values a search reads and the `cadence spsa` block all come from
//! [`PARAMS`]. Only the UCI session moves a search off [`Tunables::DEFAULT`], so `bench` reads the
//! compiled-in values by construction.

use std::io::Write;
use std::process::ExitCode;

use crate::search::{LMP_MULTIPLIER, REVERSE_FUTILITY_MARGIN};

/// The search reads integers only, so a float is converted once, here.
pub const MILLI: i32 = 1000;

/// The discriminant indexes [`PARAMS`] and [`Tunables`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tunable {
    ReverseFutilityMargin,
    LmpMultiplier,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// A UCI `spin` and an SPSA `int`, stored as given.
    Int,
    /// A UCI `string` and an SPSA `float`, stored in thousandths of the declared unit.
    Float,
}

/// `default` is the compiled-in constant, and the range is where a tune may move it.
#[derive(Clone, Copy, Debug)]
pub struct Param {
    pub tunable: Tunable,
    pub name: &'static str,
    pub kind: Kind,
    pub default: i32,
    pub min: i32,
    pub max: i32,
    /// In declared units, printed for the tuner and read by nothing else.
    pub c_end: f64,
    pub r_end: f64,
}

/// A row is a parameter's whole declaration, so the UCI option and the tune input cannot disagree.
pub const PARAMS: &[Param] = &[
    Param {
        tunable: Tunable::ReverseFutilityMargin,
        name: "reverse_futility_margin",
        kind: Kind::Int,
        default: REVERSE_FUTILITY_MARGIN,
        min: 50,
        max: 300,
        c_end: 12.5,
        r_end: 0.002,
    },
    Param {
        tunable: Tunable::LmpMultiplier,
        name: "lmp_multiplier",
        kind: Kind::Float,
        default: LMP_MULTIPLIER,
        min: 0,
        max: 2 * MILLI,
        c_end: 0.1,
        r_end: 0.002,
    },
];

const _: () = {
    let mut i = 0;
    while i < PARAMS.len() {
        let p = &PARAMS[i];
        assert!(p.tunable as usize == i, "PARAMS is in Tunable order");
        assert!(
            p.min <= p.default && p.default <= p.max,
            "a default outside its range"
        );
        assert!(
            p.c_end > 0.0 && p.r_end > 0.0,
            "a step size that never moves"
        );
        i += 1;
    }
};

/// Only [`Param::set`] writes one, and it clamps, so every value is in range.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Tunables([i32; PARAMS.len()]);

impl Tunables {
    /// What `bench`, every test and a GUI that sets nothing search with.
    pub const DEFAULT: Tunables = {
        let mut values = [0; PARAMS.len()];
        let mut i = 0;
        while i < PARAMS.len() {
            values[i] = PARAMS[i].default;
            i += 1;
        }
        Tunables(values)
    };

    #[inline]
    #[must_use]
    pub fn get(&self, which: Tunable) -> i32 {
        self.0[which as usize]
    }
}

impl Default for Tunables {
    fn default() -> Self {
        Self::DEFAULT
    }
}

impl Param {
    /// Clamped; `None` where `text` is not a number of this kind, and nothing here panics.
    #[must_use]
    pub fn parse(&self, text: &str) -> Option<i32> {
        let text = text.trim();
        let stored = match self.kind {
            Kind::Int => text
                .parse::<i64>()
                .ok()?
                .clamp(i64::from(self.min), i64::from(self.max)),
            // Every spelling a tuner's float formatting produces parses, exponents included;
            // infinities and NaN are refused.
            Kind::Float => {
                let declared = text.parse::<f64>().ok().filter(|v| v.is_finite())?;
                (declared * f64::from(MILLI))
                    .round()
                    .clamp(f64::from(self.min), f64::from(self.max)) as i64
            }
        };
        i32::try_from(stored).ok()
    }

    pub fn set(&self, tunables: &mut Tunables, value: i32) {
        tunables.0[self.tunable as usize] = value.clamp(self.min, self.max);
    }

    #[must_use]
    pub fn spell(&self, value: i32) -> String {
        match self.kind {
            Kind::Int => value.to_string(),
            Kind::Float => {
                let sign = if value < 0 { "-" } else { "" };
                let (v, unit) = (value.unsigned_abs(), MILLI.unsigned_abs());
                format!("{sign}{}.{:03}", v / unit, v % unit)
            }
        }
    }

    #[must_use]
    pub fn uci_option(&self) -> String {
        match self.kind {
            Kind::Int => format!(
                "option name {} type spin default {} min {} max {}",
                self.name, self.default, self.min, self.max
            ),
            Kind::Float => format!(
                "option name {} type string default {}",
                self.name,
                self.spell(self.default)
            ),
        }
    }

    /// Name, type, current, minimum, maximum, `C_end`, `R_end`.
    #[must_use]
    pub fn spsa_line(&self) -> String {
        let kind = match self.kind {
            Kind::Int => "int",
            Kind::Float => "float",
        };
        format!(
            "{}, {kind}, {}, {}, {}, {}, {}",
            self.name,
            self.spell(self.default),
            self.spell(self.min),
            self.spell(self.max),
            self.c_end,
            self.r_end
        )
    }
}

/// Case-insensitive, as UCI asks.
#[must_use]
pub fn find(name: &str) -> Option<&'static Param> {
    PARAMS.iter().find(|p| p.name.eq_ignore_ascii_case(name))
}

/// No final newline: the tuner's form reads the empty piece after one as a malformed parameter.
#[must_use]
pub fn run(args: &[String]) -> ExitCode {
    if !args.is_empty() {
        eprintln!("cadence spsa takes no arguments: the parameters are declared in the source");
        return ExitCode::from(2);
    }
    let block: Vec<String> = PARAMS.iter().map(Param::spsa_line).collect();
    let mut out = std::io::stdout().lock();
    let _ = write!(out, "{}", block.join("\n"));
    let _ = out.flush();
    ExitCode::SUCCESS
}
