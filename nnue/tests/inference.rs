// SPDX-License-Identifier: GPL-3.0-or-later

//! The inference against its own oracles: the file round trip and its refusals, every update
//! against a refresh, the fast output against the reference, a position against its mirror, and
//! one digest every target's build must print, at each of the two widths a header may name and with
//! output buckets.

use cadence_core::chess960::dfrc_fen;
use cadence_core::features::NUM_INPUTS;
use cadence_core::mv::parse_uci;
use cadence_core::position::Board;
use cadence_core::rng::Rng;
use cadence_core::{FenStyle, Move, generate_legal};
use cadence_nnue::net::{EMBEDDED, HEADER_LEN, PAYLOAD_LEN, bucket_of, payload_len};
use cadence_nnue::{
    AccumulatorStack, AnyNetwork, HIDDEN, LoadError, Network, OUTPUT_BUCKETS, Provenance, WIDE,
};

/// The digest of `corpus_outputs`, the same on every target, or a build computes another number.
const DIGEST: u64 = 0xc222_483e_5258_62ef;
/// The same corpus scored by the wide seeded net, read alike on every CI leg.
const WIDE_DIGEST: u64 = 0x8fc8_6648_262e_67a0;
/// The same corpus scored by the narrow seeded net of eight output buckets.
const BUCKETED_DIGEST: u64 = 0x511d_bc0c_29e3_dd5c;
/// `file_digest` of each width's seeded file as the build before output buckets wrote it.
const NARROW_FILE: u64 = 0x17f0_80ed_298a_b817;
const WIDE_FILE: u64 = 0x4961_9e86_015c_6a7a;

fn net() -> Network {
    Network::seeded(0xc0de, Provenance::SelfPlay)
}

fn wide() -> Network<WIDE> {
    Network::<WIDE>::generate(0xc0de, Provenance::SelfPlay)
}

/// `net`'s fast output for `board`, refreshed, at the bucket its pieces pick.
fn score<const H: usize>(net: &Network<H>, board: &Board) -> i32 {
    net.evaluate(
        &net.refresh(board),
        board.side_to_move(),
        board.occupied().count(),
    )
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
    assert_eq!(edit(18, 3), Some(LoadError::Buckets(3)));
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
    let loaded = Network::from_payload(payload, 1, Provenance::SelfPlay, trainer, [7; 32]);
    let loaded = loaded.expect("a payload loads");
    assert_eq!(&loaded.to_bytes()[HEADER_LEN..], payload);
    assert_eq!(loaded.trainer(), trainer);
    assert_eq!(loaded.manifest(), [7; 32]);
    let refused = |payload: &[u8], provenance, trainer| {
        Network::from_payload(payload, 1, provenance, trainer, [0; 32]).err()
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
        Some(Ok(shipped)) => {
            assert_eq!(shipped.to_bytes(), EMBEDDED);
            assert_eq!(shipped.buckets(), 1);
        }
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
    let (stm, pieces) = (board.side_to_move(), board.occupied().count());
    assert_eq!(
        widest.evaluate(stack.current(), stm, pieces),
        widest.evaluate_reference(stack.current(), stm, pieces)
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
        let updated = net.update(&parent, &dirty);
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
            let (stm, pieces) = (board.side_to_move(), board.occupied().count());
            assert_eq!(
                net.evaluate(&acc, stm, pieces),
                net.evaluate_reference(&acc, stm, pieces)
            );
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
            let a = score(&net, &board);
            let b = score(&net, &flipped);
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
            let value = score(&net, &board);
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
    assert_eq!(bytes.len(), HEADER_LEN + payload_len(WIDE, 1));
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
    Network::<WIDE>::load_payload(&payload, 1, Provenance::SelfPlay, &"0".repeat(40), [0; 32])
}

/// At the wide width a perspective's sum fits `i32` only for output weights of 64 or less: every
/// output weight at 64 with every lane at its ceiling loads and scores as the reference does, and 65
/// is refused.
#[test]
fn a_wide_net_whose_output_could_overflow_is_refused() {
    let widest = wide_extreme(64).expect("every output weight at 64 loads");
    let board = Board::from_fen(&dfrc_fen(518, 518)).expect("the start");
    let stack = AccumulatorStack::new(&widest, &board);
    let (stm, pieces) = (board.side_to_move(), board.occupied().count());
    assert_eq!(
        widest.evaluate(stack.current(), stm, pieces),
        widest.evaluate_reference(stack.current(), stm, pieces)
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
            stack.push(&net, &dirty);
            let fresh = net.refresh(&board);
            assert_eq!(*stack.current(), fresh, "after {m:?}");
            let (stm, pieces) = (board.side_to_move(), board.occupied().count());
            assert_eq!(
                net.evaluate(&fresh, stm, pieces),
                net.evaluate_reference(&fresh, stm, pieces)
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
            let a = score(&net, &board);
            let b = score(&net, &flipped);
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
            let value = score(&net, &board);
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

fn bucketed() -> Network {
    Network::<HIDDEN>::generate_bucketed(0xc0de, Provenance::SelfPlay)
}

/// A file's bytes folded eight at a time, the last word padded with zeros.
fn file_digest(bytes: &[u8]) -> u64 {
    bytes.chunks(8).fold(0, |digest, chunk| {
        let mut word = [0; 8];
        word[..chunk.len()].copy_from_slice(chunk);
        cadence_core::rng::splitmix64(digest ^ u64::from_le_bytes(word)).0
    })
}

/// A one-bucket net writes the file the build before buckets wrote, byte for byte at each width,
/// its bucket byte zero.
#[test]
fn a_one_bucket_file_is_the_file_written_before_buckets() {
    let narrow = net().to_bytes();
    let wide = wide().to_bytes();
    assert_eq!((narrow[18], wide[18]), (0, 0));
    assert_eq!(
        file_digest(&narrow),
        NARROW_FILE,
        "{:#018x}",
        file_digest(&narrow)
    );
    assert_eq!(
        file_digest(&wide),
        WIDE_FILE,
        "{:#018x}",
        file_digest(&wide)
    );
    assert_eq!(net().buckets(), 1);
    assert_eq!(AnyNetwork::from_bytes(&wide).map(|n| n.buckets()), Ok(1));
}

/// A net of eight output buckets round trips through its file at either width, its header naming the
/// count, and the loader of either width reads it by its header.
#[test]
fn a_bucketed_net_round_trips_at_each_width() {
    let narrow = bucketed().to_bytes();
    assert_eq!(
        narrow.len(),
        HEADER_LEN + payload_len(HIDDEN, OUTPUT_BUCKETS)
    );
    assert_eq!(usize::from(narrow[18]), OUTPUT_BUCKETS);
    let back = Network::from_bytes(&narrow).expect("loads");
    assert_eq!(back.buckets(), OUTPUT_BUCKETS);
    assert_eq!(back.to_bytes(), narrow);
    let any = AnyNetwork::from_bytes(&narrow).expect("loads by its header");
    assert_eq!((any.hidden(), any.buckets()), (HIDDEN, OUTPUT_BUCKETS));
    assert_eq!(any.to_bytes(), narrow);
    let wide = Network::<WIDE>::generate_bucketed(0xc0de, Provenance::SelfPlay).to_bytes();
    assert_eq!(wide.len(), HEADER_LEN + payload_len(WIDE, OUTPUT_BUCKETS));
    let back = Network::<WIDE>::load(&wide).expect("loads");
    assert_eq!(back.to_bytes(), wide);
    let any = AnyNetwork::from_bytes(&wide).expect("loads by its header");
    assert_eq!((any.hidden(), any.buckets()), (WIDE, OUTPUT_BUCKETS));
    assert_eq!(any.to_bytes(), wide);
    let payload = &narrow[HEADER_LEN..];
    let trainer = "0".repeat(40);
    let from =
        |buckets| Network::from_payload(payload, buckets, Provenance::SelfPlay, &trainer, [0; 32]);
    assert_eq!(
        from(OUTPUT_BUCKETS).map(|n| n.to_bytes()),
        Ok(narrow.clone())
    );
    assert!(matches!(from(1), Err(LoadError::Length { .. })));
    assert_eq!(from(3).err(), Some(LoadError::Buckets(3)));
    assert_eq!(from(0).err(), Some(LoadError::Buckets(u8::MAX)));
}

/// A bucket byte the file's length does not match, either way, is refused as a length; any byte
/// but 0 or 8 is refused by name.
#[test]
fn a_bucket_count_the_payload_does_not_hold_is_refused() {
    let one = net().to_bytes();
    let mut claims_eight = one.clone();
    claims_eight[18] = 8;
    assert_eq!(
        Network::from_bytes(&claims_eight).err(),
        Some(LoadError::Length {
            want: HEADER_LEN + payload_len(HIDDEN, OUTPUT_BUCKETS),
            got: one.len()
        })
    );
    let mut claims_one = bucketed().to_bytes();
    claims_one[18] = 0;
    assert!(matches!(
        Network::from_bytes(&claims_one),
        Err(LoadError::Length { .. })
    ));
    for byte in [1, 2, 3, 7, 9, 16, 255] {
        let mut other = one.clone();
        other[18] = byte;
        assert_eq!(
            Network::from_bytes(&other).err(),
            Some(LoadError::Buckets(byte))
        );
        assert_eq!(
            AnyNetwork::from_bytes(&other).err(),
            Some(LoadError::Buckets(byte))
        );
    }
}

/// The trainer's material count: 2 to 32 pieces, kings included, four to a bucket of eight, and
/// always the one bucket of a one-bucket net.
#[test]
fn the_bucket_is_the_trainers_material_count() {
    let eight = bucketed();
    let one = net();
    for pieces in 2..=32_u32 {
        let want = (pieces as usize - 2) / 4;
        assert_eq!(bucket_of(pieces, OUTPUT_BUCKETS), want, "{pieces} pieces");
        assert_eq!(eight.bucket(pieces), want, "{pieces} pieces");
        assert_eq!((bucket_of(pieces, 1), one.bucket(pieces)), (0, 0));
    }
    let ends: Vec<usize> = [2, 5, 6, 9, 10, 29, 30, 32]
        .iter()
        .map(|&p| eight.bucket(p))
        .collect();
    assert_eq!(ends, [0, 0, 1, 1, 2, 6, 7, 7]);
    assert_eq!((eight.bucket(0), eight.bucket(33)), (0, 7));
}

/// Over long random games, played rather than made so they outlast the search's stack, the fast
/// output equals the reference at each position's own bucket and at others, at both widths; the
/// games reach every bucket.
#[test]
fn a_bucketed_net_scores_as_its_reference_in_every_bucket() {
    let narrow = bucketed();
    let wide = Network::<WIDE>::generate_bucketed(0xc0de, Provenance::SelfPlay);
    let mut reached = [false; OUTPUT_BUCKETS];
    let mut rng = Rng::new(7);
    for _ in 0..40 {
        let white = u32::try_from(rng.below(960)).unwrap_or(518);
        let black = u32::try_from(rng.below(960)).unwrap_or(518);
        let mut board = Board::from_fen(&dfrc_fen(white, black)).expect("a start");
        for _ in 0..600 {
            let legal = generate_legal(&board);
            let Some(&m) = legal.as_slice().get(rng.below(legal.len().max(1))) else {
                break;
            };
            board.play(m);
            let (stm, pieces) = (board.side_to_move(), board.occupied().count());
            reached[narrow.bucket(pieces)] = true;
            let (acc, wide_acc) = (narrow.refresh(&board), wide.refresh(&board));
            assert_eq!(
                narrow.evaluate(&acc, stm, pieces),
                narrow.evaluate_reference(&acc, stm, pieces)
            );
            for other in (2..=32).step_by(3) {
                assert_eq!(
                    narrow.evaluate(&acc, stm, other),
                    narrow.evaluate_reference(&acc, stm, other)
                );
                assert_eq!(
                    wide.evaluate(&wide_acc, stm, other),
                    wide.evaluate_reference(&wide_acc, stm, other)
                );
            }
        }
    }
    let count = reached.iter().filter(|&&r| r).count();
    assert_eq!(
        count, OUTPUT_BUCKETS,
        "the games reached {count} buckets: {reached:?}"
    );
}

/// Each bucket's layer is its own: one accumulator scored at a piece count in each bucket gives
/// eight different values, where a one-bucket net gives one.
#[test]
fn different_buckets_give_different_values() {
    let board = Board::from_fen(&dfrc_fen(518, 518)).expect("the start");
    let stm = board.side_to_move();
    let eight = bucketed();
    let acc = eight.refresh(&board);
    let mut values: Vec<i32> = (0..8)
        .map(|b| eight.evaluate(&acc, stm, 2 + 4 * b))
        .collect();
    values.sort_unstable();
    values.dedup();
    assert_eq!(values.len(), OUTPUT_BUCKETS, "{values:?}");
    let one = net();
    let acc = one.refresh(&board);
    let first = one.evaluate(&acc, stm, 2);
    assert!((3..=32).all(|p| one.evaluate(&acc, stm, p) == first));
}

/// Every lane at the activation's ceiling, every bucket's output weights at 64 but the last's at
/// `last`, at the wide width with eight buckets.
fn wide_bucketed_extreme(last: i16) -> Result<Network<WIDE>, LoadError> {
    let words: Vec<i16> = std::iter::repeat_n(0, NUM_INPUTS * WIDE)
        .chain(std::iter::repeat_n(255, WIDE))
        .chain(std::iter::repeat_n(64, 2 * WIDE * (OUTPUT_BUCKETS - 1)))
        .chain(std::iter::repeat_n(last, 2 * WIDE))
        .chain(std::iter::repeat_n(0, OUTPUT_BUCKETS))
        .collect();
    let payload: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
    let trainer = "0".repeat(40);
    Network::<WIDE>::load_payload(
        &payload,
        OUTPUT_BUCKETS,
        Provenance::SelfPlay,
        &trainer,
        [0; 32],
    )
}

/// The load check holds each bucket to the bounds one bucket is held to: a weight of 128 in the last
/// bucket is refused at its index across every bucket's rows, a wide bucket whose sum could leave
/// `i32` is refused, and one at the bound scores as the reference does in every bucket.
#[test]
fn every_bucket_is_held_to_the_load_check() {
    let good = bucketed().to_bytes();
    let output = NUM_INPUTS * HIDDEN + HIDDEN;
    let index = (OUTPUT_BUCKETS - 1) * 2 * HIDDEN + 3;
    let mut weight = good.clone();
    let at = HEADER_LEN + 2 * (output + index);
    weight[at..at + 2].copy_from_slice(&128_i16.to_le_bytes());
    assert_eq!(
        Network::from_bytes(&weight).err(),
        Some(LoadError::OutputWeight { index, weight: 128 })
    );
    assert_eq!(
        wide_bucketed_extreme(65).err(),
        Some(LoadError::OutputSum {
            bound: 512 * 65 * 255 * 255
        })
    );
    let widest = wide_bucketed_extreme(64).expect("every output weight at 64 loads");
    let board = Board::from_fen(&dfrc_fen(518, 518)).expect("the start");
    let stack = AccumulatorStack::new(&widest, &board);
    let stm = board.side_to_move();
    for pieces in 2..=32 {
        assert_eq!(
            widest.evaluate(stack.current(), stm, pieces),
            widest.evaluate_reference(stack.current(), stm, pieces)
        );
    }
}

/// `corpus_digest`'s corpus scored by the narrow seeded net of eight buckets, each position at its
/// own; every CI leg must print it as it prints the others.
#[test]
fn every_target_scores_the_corpus_alike_with_buckets() {
    let net = bucketed();
    let mut digest = 0_u64;
    for (start, moves) in games(4, 50, 120) {
        let mut board = start.duplicate();
        for &m in &moves {
            let _ = board.make_move(m);
            let word = u64::from(score(&net, &board).cast_unsigned());
            digest = cadence_core::rng::splitmix64(digest ^ word).0;
        }
    }
    assert_eq!(digest, BUCKETED_DIGEST, "digest {digest:#018x}");
}
