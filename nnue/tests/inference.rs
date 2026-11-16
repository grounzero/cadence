// SPDX-License-Identifier: GPL-3.0-or-later

//! The inference against its own oracles: the file round trip and its refusals, every update
//! against a refresh, the fast output against the reference, a position against its mirror, and
//! one digest every target's build must print.

use cadence_core::chess960::dfrc_fen;
use cadence_core::position::Board;
use cadence_core::rng::Rng;
use cadence_core::{FenStyle, Move, generate_legal};
use cadence_nnue::net::{HEADER_LEN, PAYLOAD_LEN};
use cadence_nnue::{AccumulatorStack, LoadError, Network, Provenance};

/// The digest of `corpus_outputs`, the same on every target, or a build computes another number.
const DIGEST: u64 = 0xc222_483e_5258_62ef;

fn net() -> Network {
    Network::seeded(0xc0de, Provenance::SelfPlay)
}

/// Random games from random arrays, with the moves each made, so DFRC castles and promotions come.
fn games(seed: u64, count: usize, length: usize) -> Vec<(Board, Vec<Move>)> {
    let mut rng = Rng::new(seed);
    (0..count)
        .map(|_| {
            let white = u32::try_from(rng.below(960)).unwrap_or(518);
            let black = u32::try_from(rng.below(960)).unwrap_or(518);
            let start = Board::from_fen(&dfrc_fen(white, black)).expect("a start");
            let mut board = start.duplicate();
            let mut moves = Vec::new();
            for _ in 0..length {
                let legal = generate_legal(&board);
                let Some(&m) = legal.as_slice().get(rng.below(legal.len().max(1))) else {
                    break;
                };
                moves.push(m);
                let _ = board.make_move(m);
            }
            (start, moves)
        })
        .collect()
}

#[test]
fn a_net_round_trips_through_its_file() {
    let net = net();
    let bytes = net.to_bytes();
    assert_eq!(bytes.len(), HEADER_LEN + PAYLOAD_LEN);
    let back = Network::from_bytes(&bytes).expect("loads");
    assert_eq!(back.to_bytes(), bytes);
    assert_eq!(*back.provenance(), Provenance::SelfPlay);
    assert_eq!(back.trainer(), "0".repeat(40));
    assert_eq!(back.manifest(), [0; 32]);
    let external = Provenance::External("lichess-big3-resolved".to_string());
    let bytes = Network::seeded(1, external.clone()).to_bytes();
    let back = Network::from_bytes(&bytes).expect("an external net loads");
    assert_eq!(*back.provenance(), external);
    assert_eq!(back.to_bytes(), bytes);
}

/// One byte changed in each field the header checks, and each refusal names its field.
#[test]
fn each_header_fault_is_refused() {
    let good = net().to_bytes();
    let edit = |at: usize, value: u8| {
        let mut b = good.clone();
        b[at] = value;
        Network::from_bytes(&b).err()
    };
    let architecture = |field, file, build| Some(LoadError::Architecture { field, file, build });
    assert!(matches!(
        Network::from_bytes(&good[..good.len() - 1]),
        Err(LoadError::Length { .. })
    ));
    let mut long = good.clone();
    long.push(0);
    assert!(matches!(
        Network::from_bytes(&long),
        Err(LoadError::Length { .. })
    ));
    assert_eq!(edit(0, b'X'), Some(LoadError::Magic));
    assert_eq!(edit(4, 2), Some(LoadError::Format(2)));
    assert_eq!(edit(7, 2), architecture("inputs", 512, 768));
    assert_eq!(edit(9, 0), architecture("hidden width", 0, 256));
    assert_eq!(edit(10, 1), architecture("activation", 1, 0));
    assert_eq!(edit(12, 0), architecture("QA", 0, 255));
    assert_eq!(edit(14, 32), architecture("QB", 32, 64));
    assert_eq!(edit(16, 0), architecture("scale", 256, 400));
    assert_eq!(edit(20, 1), Some(LoadError::Reserved));
    assert_eq!(edit(24, b'G'), Some(LoadError::Trainer));
    assert_eq!(edit(63, b'A'), Some(LoadError::Trainer));
    assert_eq!(edit(11, 0), Some(LoadError::Provenance));
    assert_eq!(edit(11, 33), Some(LoadError::Provenance));
    assert_eq!(edit(96, b' '), Some(LoadError::Provenance));
    assert_eq!(edit(127, b'x'), Some(LoadError::Provenance));
    assert_eq!(edit(64, 0xff), None, "the manifest is any 32 bytes");
}

/// A trainer's weights behind a header built for them load as the file would, and a commit or a
/// provenance the header cannot hold is refused.
#[test]
fn a_payload_loads_behind_the_header_it_is_given() {
    let file = net().to_bytes();
    let payload = &file[HEADER_LEN..];
    let trainer = "0123456789abcdef0123456789abcdef01234567";
    let loaded = Network::from_payload(payload, Provenance::SelfPlay, trainer, [7; 32]);
    let loaded = loaded.expect("a payload loads");
    assert_eq!(&loaded.to_bytes()[HEADER_LEN..], payload);
    assert_eq!(loaded.trainer(), trainer);
    assert_eq!(loaded.manifest(), [7; 32]);
    let refused = |payload: &[u8], provenance, trainer| {
        Network::from_payload(payload, provenance, trainer, [0; 32]).err()
    };
    assert_eq!(
        refused(payload, Provenance::SelfPlay, "abc"),
        Some(LoadError::Trainer)
    );
    let long = Provenance::External("x".repeat(33));
    assert_eq!(refused(payload, long, trainer), Some(LoadError::Provenance));
    assert!(matches!(
        refused(&payload[1..], Provenance::SelfPlay, trainer),
        Some(LoadError::Length { .. })
    ));
}

/// A canary net is trained on self-play data and still names itself, so it is never taken for the
/// net it rehearses, and it may not generate data.
#[test]
fn a_canary_net_names_itself_and_may_not_generate_data() {
    let bytes = Network::seeded(1, Provenance::Canary).to_bytes();
    let back = Network::from_bytes(&bytes).expect("a canary net loads");
    assert_eq!(*back.provenance(), Provenance::Canary);
    assert_eq!(back.provenance().to_string(), "canary");
    assert_eq!(Network::for_datagen(&bytes).err(), Some(LoadError::Canary));
}

/// The loader every data generator uses takes a self-play net and refuses any other, even one a
/// letter away from the self-play spelling.
#[test]
fn only_a_self_play_net_may_generate_data() {
    assert!(Network::for_datagen(&net().to_bytes()).is_ok());
    let validation = Provenance::External("lichess-big3-resolved".to_string());
    let bytes = Network::seeded(1, validation).to_bytes();
    assert_eq!(
        Network::for_datagen(&bytes).err(),
        Some(LoadError::NotSelfPlay("lichess-big3-resolved".to_string()))
    );
    let mut near = net().to_bytes();
    near[96 + 8] = b'x';
    assert!(Network::from_bytes(&near).is_ok());
    assert_eq!(
        Network::for_datagen(&near).err(),
        Some(LoadError::NotSelfPlay("self-plax".to_string()))
    );
}

/// Each bound the load check holds is refused one past it: a lane whose 32 largest weights and
/// bias could leave `i16`, an output weight of 128, and an output sum past `i32`.
#[test]
fn a_net_that_could_overflow_is_refused() {
    let good = net().to_bytes();
    let word_at = |i: usize| HEADER_LEN + 2 * i;
    let set = |bytes: &mut Vec<u8>, i: usize, w: i16| {
        bytes[word_at(i)..word_at(i) + 2].copy_from_slice(&w.to_le_bytes());
    };
    let hidden = cadence_nnue::HIDDEN;
    let features = PAYLOAD_LEN / 2 - hidden - 2 * hidden - 1;
    let mut lane = good.clone();
    for f in 0..32 {
        set(&mut lane, f * hidden, 1_100);
    }
    assert!(matches!(
        Network::from_bytes(&lane),
        Err(LoadError::Lane { lane: 0, .. })
    ));
    let output = features + hidden;
    let mut weight = good.clone();
    set(&mut weight, output + 3, 128);
    assert_eq!(
        Network::from_bytes(&weight).err(),
        Some(LoadError::OutputWeight {
            index: 3,
            weight: 128
        })
    );
    let mut sum = good;
    for i in 0..2 * hidden {
        set(&mut sum, output + i, 127);
    }
    assert!(matches!(
        Network::from_bytes(&sum),
        Err(LoadError::OutputSum { .. })
    ));
}

/// After every move and every unmake of random DFRC games, the incremental stack equals a full
/// refresh of the board, castles and promotions included.
#[test]
fn incremental_updates_equal_a_refresh() {
    let net = net();
    let mut castles = 0;
    let mut promotions = 0;
    for (start, moves) in games(1, 60, 160) {
        let mut board = start.duplicate();
        let mut stack = AccumulatorStack::new(&net, &board);
        let mut boards = vec![board.duplicate()];
        for &m in &moves {
            castles += usize::from(m.is_castle());
            promotions += usize::from(m.is_promotion());
            let dirty = board.make_move(m);
            stack.push(&net, &dirty);
            assert_eq!(*stack.current(), net.refresh(&board), "after {m:?}");
            boards.push(board.duplicate());
        }
        for &m in moves.iter().rev() {
            board.unmake_move(m);
            stack.pop();
            boards.pop();
            let parent = boards.last().expect("the root stays");
            assert_eq!(
                *stack.current(),
                net.refresh(parent),
                "after unmaking {m:?}"
            );
        }
    }
    assert!(
        castles > 0 && promotions > 0,
        "{castles} castles and {promotions} promotions"
    );
}

#[test]
fn the_fast_output_equals_the_reference() {
    let net = net();
    for (start, moves) in games(2, 40, 120) {
        let mut board = start.duplicate();
        for &m in &moves {
            let _ = board.make_move(m);
            let acc = net.refresh(&board);
            let stm = board.side_to_move();
            assert_eq!(net.evaluate(&acc, stm), net.evaluate_reference(&acc, stm));
        }
    }
}

/// Ranks reversed, colours swapped and the side to move flipped, in a Shredder FEN.
fn mirror(fen: &str) -> String {
    let fields: Vec<&str> = fen.split(' ').collect();
    let swap = |c: char| {
        if c.is_ascii_uppercase() {
            c.to_ascii_lowercase()
        } else {
            c.to_ascii_uppercase()
        }
    };
    let ranks: Vec<String> = fields[0]
        .split('/')
        .rev()
        .map(|r| r.chars().map(swap).collect())
        .collect();
    let side = if fields[1] == "w" { "b" } else { "w" };
    let castling = if fields[2] == "-" {
        "-".to_string()
    } else {
        fields[2].chars().map(swap).collect()
    };
    let ep = if fields[3] == "-" {
        "-".to_string()
    } else {
        let rank = if &fields[3][1..] == "3" { "6" } else { "3" };
        format!("{}{rank}", &fields[3][..1])
    };
    format!(
        "{} {side} {castling} {ep} {} {}",
        ranks.join("/"),
        fields[4],
        fields[5]
    )
}

/// A position and its colour-flipped mirror look the same from the side to move, so they score
/// the same, bit for bit.
#[test]
fn a_position_and_its_mirror_score_alike() {
    let net = net();
    for (start, moves) in games(3, 30, 100) {
        let mut board = start.duplicate();
        for &m in &moves {
            let _ = board.make_move(m);
            let fen = board.to_fen(FenStyle::Shredder);
            let flipped = Board::from_fen(&mirror(&fen)).expect("a mirrored position");
            let a = net.evaluate(&net.refresh(&board), board.side_to_move());
            let b = net.evaluate(&net.refresh(&flipped), flipped.side_to_move());
            assert_eq!(a, b, "{fen} against {}", mirror(&fen));
        }
    }
}

/// Every position of a fixed corpus scored by the seeded net, folded into one number.
fn corpus_digest() -> u64 {
    let net = net();
    let mut digest = 0_u64;
    for (start, moves) in games(4, 50, 120) {
        let mut board = start.duplicate();
        for &m in &moves {
            let _ = board.make_move(m);
            let value = net.evaluate(&net.refresh(&board), board.side_to_move());
            let word = u64::from(value.cast_unsigned());
            digest = cadence_core::rng::splitmix64(digest ^ word).0;
        }
    }
    digest
}

/// One portable source compiled for each target: every CI leg must print this digest, which is
/// how the scalar, NEON and AVX2 builds are shown bit-identical.
#[test]
fn every_target_scores_the_corpus_alike() {
    assert_eq!(corpus_digest(), DIGEST, "digest {:#018x}", corpus_digest());
}
