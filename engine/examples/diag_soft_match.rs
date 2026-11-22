// SPDX-License-Identifier: GPL-3.0-or-later

//! A diagnostic: a match between two node settings for datagen's searches, played from datagen's
//! starts in colour-swapped pairs, each side with its own table and histories as the generator has.

use std::io::{self, Write};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::thread;

use cadence_core::rng::Rng;
use cadence_core::{Colour, generate_legal};
use cadence_dataset::chunk::KIND_STANDARD;
use cadence_engine::datagen::game::{self, Outcome};
use cadence_engine::datagen::packed::{chunk_seed, shipped_net, start_kind};
use cadence_engine::datagen::{self, HASH_MB, opening, spawn_search};
use cadence_engine::position::Position;
use cadence_engine::search::{Learned, Limits, Search};
use cadence_engine::tt::Table;
use cadence_nnue::AnyNetwork;

/// The packed writer's default share of standard starts.
const SHARE: u8 = 50;

fn limits(soft: u64, cap: u64) -> Limits {
    Limits {
        soft_nodes: (soft > 0).then_some(soft),
        nodes: Some(cap),
        ..Limits::default()
    }
}

/// The same start for both games of a pair: drawn again from the same seed, since a position is
/// not cloned.
fn start(seed: u64, pair: u64, net: &AnyNetwork, tt: &Table) -> Position {
    let mut rng = Rng::new(datagen::game_seed(seed, pair));
    let kind = start_kind(&mut rng, SHARE);
    let probe = |board: &mut _, tt: &Table| game::probe(board, opening::SCREEN_NODES, tt);
    let (opening, _) =
        opening::next_of_kind(&mut rng, tt, kind == KIND_STANDARD, &probe, Some(net));
    opening.board
}

/// A's score from 0 to 2 in half points.
fn game_score(
    board: &mut Position,
    a_white: bool,
    a: Limits,
    b: Limits,
    tables: &[Table; 2],
) -> u8 {
    let stop = AtomicBool::new(false);
    let (white_limits, black_limits) = if a_white { (a, b) } else { (b, a) };
    tables[0].clear();
    tables[1].clear();
    let mut white = Search::new(&stop, &tables[0]);
    let mut black = Search::new(&stop, &tables[1]);
    white.set_limits(white_limits);
    black.set_limits(black_limits);
    let (mut lw, mut lb) = (Learned::new(), Learned::new());
    white.swap_learned(&mut lw);
    black.swap_learned(&mut lb);
    let outcome = loop {
        if let Some((_, outcome)) = game::ending(board) {
            break outcome;
        }
        let side = if board.side_to_move() == Colour::White {
            &mut white
        } else {
            &mut black
        };
        let best = side.run(board, &mut io::sink());
        assert!(generate_legal(board).contains(best), "an illegal move");
        board.play(best);
    };
    white.swap_learned(&mut lw);
    black.swap_learned(&mut lb);
    match (outcome, a_white) {
        (Outcome::Draw, _) => 1,
        (Outcome::White, true) | (Outcome::Black, false) => 2,
        _ => 0,
    }
}

/// Elo from a mean score, and the 95% interval from the pairs' variance.
fn elo(penta: &[u64; 5]) -> (f64, f64, f64) {
    let n: u64 = penta.iter().sum();
    let n = n as f64;
    let mean = penta
        .iter()
        .enumerate()
        .map(|(i, &c)| i as f64 / 4.0 * c as f64)
        .sum::<f64>()
        / n;
    let var = penta
        .iter()
        .enumerate()
        .map(|(i, &c)| (i as f64 / 4.0 - mean).powi(2) * c as f64)
        .sum::<f64>()
        / n;
    let se = (var / n).sqrt();
    let to_elo = |s: f64| -400.0 * (1.0 / s.clamp(1e-9, 1.0 - 1e-9) - 1.0).log10();
    (
        to_elo(mean),
        to_elo(mean - 1.96 * se),
        to_elo(mean + 1.96 * se),
    )
}

fn main() {
    let args: Vec<u64> = std::env::args()
        .skip(1)
        .map(|a| {
            a.parse()
                .expect("numbers: pairs threads a-soft a-cap b-soft b-cap (soft 0 is hard)")
        })
        .collect();
    let (pairs, threads) = (args[0], args[1] as usize);
    let (a, b) = (limits(args[2], args[3]), limits(args[4], args[5]));
    let net = shipped_net()
        .expect("the shipped net")
        .expect("a shipped net");
    let seed = chunk_seed(0x6d61_7463_6800_0001, 0);
    println!(
        "pairs {pairs} threads {threads} A soft {} cap {} B soft {} cap {} share {SHARE} run seed 6d61746368000001",
        args[2], args[3], args[4], args[5]
    );
    let next = AtomicU64::new(0);
    let done = AtomicU64::new(0);
    let penta = Mutex::new([0_u64; 5]);
    thread::scope(|scope| {
        for _ in 0..threads {
            let (next, done, penta, net) = (&next, &done, &penta, &net);
            spawn_search(scope, move || {
                let probe_tt = Table::new(HASH_MB).expect("a table");
                let tables = [
                    Table::new(HASH_MB).expect("a table"),
                    Table::new(HASH_MB).expect("a table"),
                ];
                loop {
                    let pair = next.fetch_add(1, Ordering::Relaxed);
                    if pair >= pairs {
                        return;
                    }
                    let first =
                        game_score(&mut start(seed, pair, net, &probe_tt), true, a, b, &tables);
                    let second =
                        game_score(&mut start(seed, pair, net, &probe_tt), false, a, b, &tables);
                    let mut p = penta.lock().expect("the tally");
                    p[usize::from(first + second)] += 1;
                    let finished = done.fetch_add(1, Ordering::Relaxed) + 1;
                    if finished % 500 == 0 || finished == pairs {
                        let (e, lo, hi) = elo(&p);
                        println!("{finished} pairs {p:?} A {e:+.2} [{lo:+.2}, {hi:+.2}]");
                        let _ = io::stdout().flush();
                    }
                }
            })
            .expect("a search thread");
        }
    });
    let p = penta.into_inner().expect("the tally");
    let (e, lo, hi) = elo(&p);
    println!("final {pairs} pairs {p:?} A {e:+.2} Elo, 95% [{lo:+.2}, {hi:+.2}]");
}
