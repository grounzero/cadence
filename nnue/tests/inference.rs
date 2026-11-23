// SPDX-License-Identifier: GPL-3.0-or-later

//! The inference against its own oracles: the file round trip and its refusals, every update
//! against a refresh, the fast output against the reference, a position against its mirror, and
//! one digest every target's build must print, at each of the two widths a header may name.

use cadence_core::chess960::dfrc_fen;
use cadence_core::features::{NUM_INPUTS, mirrored_feature_index, mirrors};
use cadence_core::mv::parse_uci;
use cadence_core::position::Board;
use cadence_core::rng::Rng;
use cadence_core::{Colour, FenStyle, Move, generate_legal};
use cadence_nnue::net::{EMBEDDED, HEADER_LEN, PAYLOAD_LEN, payload_len};
use cadence_nnue::{
    AccumulatorStack, AnyNetwork, HIDDEN, Inputs, LoadError, Network, Provenance, WIDE,
};

/// The digest of `corpus_outputs`, the same on every target, or a build computes another number.
const DIGEST: u64 = 0xc222_483e_5258_62ef;
/// The same corpus scored by the wide seeded net, read alike on every CI leg.
const WIDE_DIGEST: u64 = 0x8fc8_6648_262e_67a0;

fn net() -> Network {
    Network::seeded(0xc0de, Provenance::SelfPlay)
}

fn wide() -> Network<WIDE> {
    Network::<WIDE>::generate(0xc0de, Provenance::SelfPlay)
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

/// No bytes ship no net, and a self-play net ships exactly as its file loads.
#[test]
fn a_build_ships_a_self_play_net_or_none() {
    assert!(Network::shipped(&[]).is_none());
    let bytes = net().to_bytes();
    let shipped = Network::shipped(&bytes)
        .expect("bytes ship a net")
        .expect("a self-play net ships");
    assert_eq!(shipped.to_bytes(), bytes);
}

/// A canary net, or one trained on an external dataset, is refused as the net a build ships.
#[test]
fn only_a_self_play_net_ships() {
    let canary = Network::seeded(1, Provenance::Canary).to_bytes();
    assert_eq!(
        Network::shipped(&canary).map(Result::err),
        Some(Some(LoadError::Canary))
    );
    let validation = Provenance::External("lichess-big3-resolved".to_string());
    let external = Network::seeded(1, validation).to_bytes();
    assert_eq!(
        Network::shipped(&external).map(Result::err),
        Some(Some(LoadError::NotSelfPlay(
            "lichess-big3-resolved".to_string()
        )))
    );
}

/// The file this build reads in ships no net, or a self-play net of this build's architecture
/// whose weights pass the load check, so a refused one fails here before any binary ships it.
#[test]
fn the_embedded_net_is_none_or_a_self_play_net_this_build_loads() {
    match AnyNetwork::shipped(EMBEDDED) {
        None => assert_eq!(EMBEDDED, b""),
        Some(Ok(shipped)) => assert_eq!(shipped.to_bytes(), EMBEDDED),
        Some(Err(e)) => panic!("the embedded net is refused: {e}"),
    }
}

/// Each bound the load check holds is refused one past it, a lane whose 32 largest weights and
/// bias could leave `i16` and an output weight of 128; every output weight at 127 loads and scores
/// as the reference does, since the two perspectives add in `i64`.
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
    let widest = Network::from_bytes(&sum).expect("every output weight at 127 loads");
    let board = Board::from_fen(&dfrc_fen(518, 518)).expect("the start");
    let stack = AccumulatorStack::new(&widest, &board);
    let stm = board.side_to_move();
    assert_eq!(
        widest.evaluate(stack.current(), stm),
        widest.evaluate_reference(stack.current(), stm)
    );
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
            stack.push(&net, &dirty, &board);
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

/// A position, a move in it, and the kind of move it must be.
type Case = (&'static str, &'static str, fn(Move) -> bool);

/// A Chess960 castle whose king and rook swap squares, an en passant capture and a promotion, each
/// updated from its parent and compared with a refresh from both perspectives.
#[test]
fn a_swapping_castle_en_passant_and_a_promotion_update_as_a_refresh() {
    let net = net();
    let cases: [Case; 3] = [
        ("4k3/8/8/8/8/8/8/2RK3R w C - 0 1", "d1c1", Move::is_castle),
        (
            "4k3/8/8/3pP3/8/8/8/4K3 w - d6 0 1",
            "e5d6",
            Move::is_en_passant,
        ),
        (
            "4k3/P7/8/8/8/8/8/4K3 w - - 0 1",
            "a7a8q",
            Move::is_promotion,
        ),
    ];
    for (fen, uci, kind) in cases {
        let mut board = Board::from_fen(fen).unwrap_or_else(|e| panic!("{fen}: {e:?}"));
        let legal = generate_legal(&board);
        let m = parse_uci(&legal, uci).unwrap_or_else(|| panic!("{uci} is not legal in {fen}"));
        assert!(kind(m), "{uci} in {fen} is not the case it stands for");
        let parent = net.refresh(&board);
        let dirty = board.make_move(m);
        let updated = net.update(&parent, &dirty, &board);
        let fresh = net.refresh(&board);
        for (persp, side) in [(0, "White"), (1, "Black")] {
            assert_eq!(
                updated.sums[persp], fresh.sums[persp],
                "{uci} in {fen}, {side}'s perspective"
            );
        }
    }
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

/// A wide net round trips through its file, whose header names its width, and the loader of either
/// width picks each file's by that name.
#[test]
fn a_wide_net_round_trips_and_is_loaded_by_its_width() {
    let bytes = wide().to_bytes();
    assert_eq!(bytes.len(), HEADER_LEN + payload_len(WIDE));
    assert_eq!(u16::from_le_bytes([bytes[8], bytes[9]]), 512);
    let back = Network::<WIDE>::load(&bytes).expect("loads");
    assert_eq!(back.to_bytes(), bytes);
    let any = AnyNetwork::from_bytes(&bytes).expect("loads by its width");
    assert_eq!(any.hidden(), WIDE);
    assert_eq!(any.to_bytes(), bytes);
    let narrow = net().to_bytes();
    let any = AnyNetwork::from_bytes(&narrow).expect("loads by its width");
    assert_eq!(any.hidden(), HIDDEN);
    assert_eq!(any.to_bytes(), narrow);
}

/// A loader asked for one width refuses a file of the other, and a header naming a width this build
/// does not read is refused by name.
#[test]
fn a_width_is_read_only_by_its_own_loader() {
    let wide = wide().to_bytes();
    let narrow = net().to_bytes();
    assert_eq!(
        Network::from_bytes(&wide).err(),
        Some(LoadError::Length {
            want: HEADER_LEN + PAYLOAD_LEN,
            got: wide.len()
        })
    );
    assert!(matches!(
        Network::<WIDE>::load(&narrow),
        Err(LoadError::Length { .. })
    ));
    let mut other = narrow.clone();
    other[8..10].copy_from_slice(&384_u16.to_le_bytes());
    assert_eq!(
        AnyNetwork::from_bytes(&other).err(),
        Some(LoadError::Width(384))
    );
    let mut renamed = narrow;
    renamed[8..10].copy_from_slice(&512_u16.to_le_bytes());
    assert!(matches!(
        AnyNetwork::from_bytes(&renamed),
        Err(LoadError::Length { .. })
    ));
}

/// Every lane at the activation's ceiling and every output weight `weight`, at the wide width.
fn wide_extreme(weight: i16) -> Result<Network<WIDE>, LoadError> {
    let words: Vec<i16> = std::iter::repeat_n(0, NUM_INPUTS * WIDE)
        .chain(std::iter::repeat_n(255, WIDE))
        .chain(std::iter::repeat_n(weight, 2 * WIDE))
        .chain([0])
        .collect();
    let payload: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
    Network::<WIDE>::load_payload(&payload, Provenance::SelfPlay, &"0".repeat(40), [0; 32])
}

/// At the wide width a perspective's sum fits `i32` only for output weights of 64 or less: every
/// output weight at 64 with every lane at its ceiling loads and scores as the reference does, and 65
/// is refused.
#[test]
fn a_wide_net_whose_output_could_overflow_is_refused() {
    let widest = wide_extreme(64).expect("every output weight at 64 loads");
    let board = Board::from_fen(&dfrc_fen(518, 518)).expect("the start");
    let stack = AccumulatorStack::new(&widest, &board);
    let stm = board.side_to_move();
    assert_eq!(
        widest.evaluate(stack.current(), stm),
        widest.evaluate_reference(stack.current(), stm)
    );
    assert_eq!(
        wide_extreme(65).err(),
        Some(LoadError::OutputSum {
            bound: 512 * 65 * 255 * 255
        })
    );
}

/// The wide accumulators follow every move and unmake of random DFRC games as a refresh does, and
/// the fast output equals the reference at every position.
#[test]
fn a_wide_net_updates_and_scores_as_its_oracles() {
    let net = wide();
    for (start, moves) in games(5, 30, 120) {
        let mut board = start.duplicate();
        let mut stack = AccumulatorStack::new(&net, &board);
        for &m in &moves {
            let dirty = board.make_move(m);
            stack.push(&net, &dirty, &board);
            let fresh = net.refresh(&board);
            assert_eq!(*stack.current(), fresh, "after {m:?}");
            let stm = board.side_to_move();
            assert_eq!(
                net.evaluate(&fresh, stm),
                net.evaluate_reference(&fresh, stm)
            );
        }
        for &m in moves.iter().rev() {
            board.unmake_move(m);
            stack.pop();
            assert_eq!(
                *stack.current(),
                net.refresh(&board),
                "after unmaking {m:?}"
            );
        }
    }
}

/// A position and its mirror score alike at the wide width too.
#[test]
fn a_position_and_its_mirror_score_alike_at_the_wide_width() {
    let net = wide();
    for (start, moves) in games(6, 20, 100) {
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

/// `corpus_digest`'s corpus scored by the wide seeded net.
fn wide_corpus_digest() -> u64 {
    let net = wide();
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

/// The wide build's digest, which every CI leg must print as it prints the narrow one.
#[test]
fn every_target_scores_the_corpus_alike_at_the_wide_width() {
    let digest = wide_corpus_digest();
    assert_eq!(digest, WIDE_DIGEST, "digest {digest:#018x}");
}

/// The header byte naming the input layout.
const INPUTS_AT: usize = 18;

/// The seeded net of either width behind a header that names mirrored inputs.
fn mirrored<const H: usize>() -> Network<H> {
    let mut bytes = Network::<H>::generate(0xc0de, Provenance::SelfPlay).to_bytes();
    bytes[INPUTS_AT] = 1;
    Network::<H>::load(&bytes).expect("a header naming mirrored inputs loads")
}

/// The file's weights as words, the oracle's only view of a net.
fn words<const H: usize>(net: &Network<H>) -> Vec<i16> {
    net.to_bytes()[HEADER_LEN..]
        .as_chunks::<2>()
        .0
        .iter()
        .map(|&w| i16::from_le_bytes(w))
        .collect()
}

/// Both perspectives' sums written out from the file's words by the mirrored index, sharing
/// nothing with the accumulator but that index, which the trainer's own holds.
fn mirrored_sums<const H: usize>(words: &[i16], board: &Board) -> [[i16; H]; 2] {
    let mut sums = [[0_i16; H]; 2];
    for persp in [Colour::White, Colour::Black] {
        let king = board.king_square(persp);
        for (lane, sum) in sums[persp.index()].iter_mut().enumerate() {
            *sum = words[NUM_INPUTS * H + lane];
            for sq in board.occupied() {
                let piece = board.piece_at(sq).expect("an occupied square");
                *sum += words[mirrored_feature_index(persp, king, piece, sq) * H + lane];
            }
        }
    }
    sums
}

/// The header's layout byte is read as plain or mirrored, kept by a round trip, and refused at
/// any other value.
#[test]
fn the_header_names_plain_or_mirrored_inputs_and_nothing_else() {
    assert_eq!(net().inputs(), Inputs::Plain);
    assert_eq!(net().to_bytes()[INPUTS_AT], 0);
    let net = mirrored::<HIDDEN>();
    assert_eq!(net.inputs(), Inputs::Mirrored);
    assert_eq!(net.to_bytes()[INPUTS_AT], 1);
    assert_eq!(mirrored::<WIDE>().inputs(), Inputs::Mirrored);
    let mut other = net.to_bytes();
    other[INPUTS_AT] = 2;
    assert_eq!(
        Network::from_bytes(&other).err(),
        Some(LoadError::Inputs(2))
    );
}

/// Castling positions, Chess960's among them, each with the file White's king and then Black's
/// lands on: a king crosses the middle either way, stays on its half, stays on its square, and
/// trades squares with its rook.
const CASTLES: [(&str, [u8; 2]); 9] = [
    (
        "r3k2r/pppppppp/2n5/8/8/5N2/PPPPPPPP/R3K2R w KQkq - 0 1",
        *b"gc",
    ),
    (
        "r3k2r/pppppppp/2n5/8/8/5N2/PPPPPPPP/R3K2R w KQkq - 0 1",
        *b"cg",
    ),
    ("1k5r/pppppppp/8/8/8/8/PPPPPPPP/1K5R w Hh - 0 1", *b"gg"),
    ("r4k2/pppppppp/8/8/8/8/PPPPPPPP/R4K2 w Aa - 0 1", *b"cc"),
    ("6kr/pppppppp/8/8/8/8/PPPPPPPP/6KR w Hh - 0 1", *b"gg"),
    ("r1k5/pppppppp/8/8/8/8/PPPPPPPP/R1K5 w Aa - 0 1", *b"cc"),
    ("3k3r/pppppppp/8/8/8/8/PPPPPPPP/3K3R w Hh - 0 1", *b"gg"),
    ("r2k4/pppppppp/8/8/8/8/PPPPPPPP/R2K4 w Aa - 0 1", *b"cc"),
    ("2rk4/pppppppp/8/8/8/8/PPPPPPPP/2RK4 w Cc - 0 1", *b"cc"),
];

/// The side to move's castle that lands its king on `file`.
fn castle_to(board: &mut Board, file: u8) -> Move {
    let us = board.side_to_move();
    let mut found = Vec::new();
    for &m in generate_legal(board).as_slice() {
        if m.is_castle() {
            board.make_move(m);
            let lands = board.king_square(us).file().index();
            board.unmake_move(m);
            if lands == usize::from(file - b'a') {
                found.push(m);
            }
        }
    }
    assert_eq!(
        found.len(),
        1,
        "one castle to the {} file",
        char::from(file)
    );
    found[0]
}

/// Through each side's castle in every castling position, a mirrored net's stack is the refresh
/// and the refresh is the oracle's sums, at both widths; the kings cross the middle eight times.
fn castles_update_as_the_oracle<const H: usize>() -> usize {
    let net = mirrored::<H>();
    let words = words(&net);
    let mut crossings = 0;
    for (fen, files) in CASTLES {
        let mut board = Board::from_fen(fen).unwrap_or_else(|e| panic!("{fen}: {e:?}"));
        let mut stack = AccumulatorStack::new(&net, &board);
        assert_eq!(
            stack.current().sums,
            mirrored_sums::<H>(&words, &board),
            "{fen}"
        );
        let mut made = Vec::new();
        for file in files {
            let us = board.side_to_move();
            let before = mirrors(board.king_square(us));
            let m = castle_to(&mut board, file);
            let dirty = board.make_move(m);
            made.push(m);
            crossings += usize::from(before != mirrors(board.king_square(us)));
            stack.push(&net, &dirty, &board);
            let fresh = net.refresh(&board);
            assert_eq!(*stack.current(), fresh, "{fen}, after {m:?}");
            assert_eq!(
                fresh.sums,
                mirrored_sums::<H>(&words, &board),
                "{fen}, after {m:?}"
            );
        }
        for &m in made.iter().rev() {
            board.unmake_move(m);
            stack.pop();
            assert_eq!(
                *stack.current(),
                net.refresh(&board),
                "{fen}, unmaking {m:?}"
            );
        }
    }
    crossings
}

#[test]
fn a_mirrored_net_follows_each_castle_chess960s_among_them() {
    assert_eq!(castles_update_as_the_oracle::<HIDDEN>(), 8);
    assert_eq!(castles_update_as_the_oracle::<WIDE>(), 8);
}

/// After every move and unmake of random DFRC games a mirrored net's stack equals a refresh, the
/// refresh equals the oracle's sums, and the fast output equals the reference. Kings cross the
/// middle by a step and move within a half; a castle's crossing is the castling test's.
#[test]
fn a_mirrored_net_updates_as_a_refresh_through_every_crossing() {
    let net = mirrored::<HIDDEN>();
    let words = words(&net);
    let (mut steps, mut castles, mut stays) = (0, 0, 0);
    for (start, moves) in games(11, 60, 160) {
        let mut board = start.duplicate();
        let mut stack = AccumulatorStack::new(&net, &board);
        let mut boards = vec![board.duplicate()];
        for (ply, &m) in moves.iter().enumerate() {
            let us = board.side_to_move();
            let before = board.king_square(us);
            let dirty = board.make_move(m);
            let after = board.king_square(us);
            let crossed = mirrors(before) != mirrors(after);
            steps += usize::from(crossed && !m.is_castle());
            castles += usize::from(crossed && m.is_castle());
            stays += usize::from(!crossed && before != after);
            stack.push(&net, &dirty, &board);
            let fresh = net.refresh(&board);
            assert_eq!(*stack.current(), fresh, "after {m:?}");
            if ply % 8 == 0 || crossed {
                assert_eq!(
                    fresh.sums,
                    mirrored_sums::<HIDDEN>(&words, &board),
                    "after {m:?}"
                );
            }
            let stm = board.side_to_move();
            assert_eq!(
                net.evaluate(&fresh, stm),
                net.evaluate_reference(&fresh, stm)
            );
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
        steps > 0 && stays > 0,
        "{steps} crossings by a step, {castles} by a castle, {stays} king moves within a half"
    );
}

/// A board flipped left to right, with no castling rights or en passant square on either.
fn flipped_pair(board: &Board) -> (Board, Board) {
    let fen = board.to_fen(FenStyle::Shredder);
    let mut fields = fen.split(' ');
    let (placement, mover) = (
        fields.next().expect("placement"),
        fields.next().expect("mover"),
    );
    let across: Vec<String> = placement
        .split('/')
        .map(|rank| rank.chars().rev().collect())
        .collect();
    let read = |placement: &str| {
        let fen = format!("{placement} {mover} - - 0 1");
        Board::from_fen(&fen).unwrap_or_else(|e| panic!("{fen}: {e:?}"))
    };
    (read(placement), read(&across.join("/")))
}

/// A mirrored net scores a position and its left-to-right flip alike, since each perspective's
/// king changes half with the board; a plain net with the same weights does not.
#[test]
fn a_mirrored_net_scores_a_position_and_its_flip_alike() {
    let (net, plain) = (mirrored::<HIDDEN>(), net());
    let (mut compared, mut differing) = (0, 0);
    for (start, moves) in games(13, 20, 80) {
        let mut board = start.duplicate();
        for &m in &moves {
            board.make_move(m);
            let (here, across) = flipped_pair(&board);
            let stm = here.side_to_move();
            assert_eq!(
                net.evaluate(&net.refresh(&here), stm),
                net.evaluate(&net.refresh(&across), stm),
                "{}",
                here.to_fen(FenStyle::Shredder)
            );
            differing += usize::from(
                plain.evaluate(&plain.refresh(&here), stm)
                    != plain.evaluate(&plain.refresh(&across), stm),
            );
            compared += 1;
        }
    }
    assert!(
        compared > 1_000 && differing > compared / 2,
        "{differing} of {compared}"
    );
}
