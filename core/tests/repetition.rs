// SPDX-License-Identifier: GPL-3.0-or-later

//! Repetition detection: twofold inside the search tree, threefold against
//! the game history, bounded by the null-move counter.
//!
//! The positions are kings alone, because every king move is reversible and
//! a king can triangulate (e1-d1-d2-e1: three moves, back where it started),
//! which is what makes a line with an odd number of real moves reach the
//! same placement -- the shape a null move needs in order to produce a false
//! repetition, and therefore the shape that shows the bound working.
//!
//! "Before the root" is built with `play`, as the UCI `position` handler
//! builds it; "inside the tree" with `make_move` and `make_null_move`.

mod support;

use cadence_core::position::Board;
use cadence_core::{Move, generate_legal};

/// Kings on e1 and e8, White to move, no rights. `P0` throughout.
const P0: &str = "4k3/8/8/8/8/8/8/4K3 w - - 0 1";

fn mv(board: &Board, uci: &str) -> Move {
    let legal = generate_legal(board);
    cadence_core::parse_uci(&legal, uci)
        .unwrap_or_else(|| panic!("{uci} is not legal in {board:?}"))
}

/// `P0` with `moves` played as game moves.
fn after(moves: &str) -> Board {
    let mut board = Board::from_fen(P0).expect("P0 parses");
    for m in moves.split_whitespace() {
        let m = mv(&board, m);
        board.play(m);
    }
    board
}

/// Make `moves` inside the tree; `-` is a null move.
fn line(board: &mut Board, moves: &str) -> Vec<Option<Move>> {
    let mut made = Vec::new();
    for tok in moves.split_whitespace() {
        if tok == "-" {
            board.make_null_move();
            made.push(None);
        } else {
            let m = mv(board, tok);
            board.make_move(m);
            made.push(Some(m));
        }
    }
    made
}

fn unwind(board: &mut Board, made: &[Option<Move>]) {
    for m in made.iter().rev() {
        match m {
            Some(m) => board.unmake_move(*m),
            None => board.unmake_null_move(),
        }
    }
}

// ---------------------------------------------------------------------------
// The counter
// ---------------------------------------------------------------------------

#[test]
fn plies_from_null_counts_real_moves_since_the_last_null_and_is_restored_by_unmake() {
    let mut board = Board::from_fen(P0).expect("P0 parses");
    assert_eq!(board.plies_from_null(), 0, "zero at setup");

    let made = line(&mut board, "e1e2 e8e7");
    assert_eq!(board.plies_from_null(), 2);
    board.make_null_move();
    assert_eq!(board.plies_from_null(), 0, "reset by a null move");
    let more = line(&mut board, "e7e8 e2e1 e8d8");
    assert_eq!(board.plies_from_null(), 3, "counts from the null move");
    unwind(&mut board, &more);
    assert_eq!(board.plies_from_null(), 0, "unmake restores the snapshot");
    board.unmake_null_move();
    assert_eq!(board.plies_from_null(), 2);
    unwind(&mut board, &made);
    assert_eq!(board.plies_from_null(), 0);

    // Game moves count too: the root after n game moves has seen n plies
    // since setup with no null among them.
    let board = after("e1e2 e8e7 e2e1 e7e8");
    assert_eq!(board.plies_from_null(), 4);
    assert_eq!(board.duplicate().plies_from_null(), 4);
}

#[test]
fn the_counter_fits_in_the_padding() {
    // The field sits in what was padding; the struct is still one cache line.
    assert_eq!(std::mem::size_of::<cadence_core::StateInfo>(), 64);
}

// ---------------------------------------------------------------------------
// Twofold in the tree
// ---------------------------------------------------------------------------

#[test]
fn returning_to_the_root_inside_the_tree_is_a_repetition() {
    let mut board = Board::from_fen(P0).expect("P0 parses");
    assert!(
        !board.is_repetition(),
        "a fresh position is not a repetition"
    );
    let made = line(&mut board, "e1e2 e8e7 e2e1");
    assert!(!board.is_repetition(), "not yet");
    let last = line(&mut board, "e7e8");
    assert!(board.is_repetition(), "P0 at ply 4 repeats P0 at the root");
    unwind(&mut board, &last);
    assert!(!board.is_repetition());
    unwind(&mut board, &made);
    assert!(!board.is_repetition());
}

#[test]
fn returning_to_an_interior_node_inside_the_tree_is_a_repetition() {
    let mut board = Board::from_fen(P0).expect("P0 parses");
    // Ply 2 (Ke2, Ke7, White to move) recurs at ply 6.
    line(&mut board, "e1e2 e8e7 e2d2 e7d7 d2e2 d7e7");
    assert!(board.is_repetition());
    assert_eq!(
        board.game_history().len(),
        0,
        "nothing before the root was needed"
    );
}

#[test]
fn a_different_side_to_move_is_a_different_position() {
    // The kings triangulate back to P0's placement in an odd number of plies:
    // same squares, other side to move. Not a repetition.
    let mut board = Board::from_fen(P0).expect("P0 parses");
    line(&mut board, "e1d1 e8d8 d1d2 d8d7 d2e1");
    assert!(!board.is_repetition());
    // One more black move and it is P0 itself.
    line(&mut board, "d7e8");
    assert!(board.is_repetition());
}

// ---------------------------------------------------------------------------
// Threefold against the history
// ---------------------------------------------------------------------------

/// Two occurrences before the root and one inside the tree: three in all,
/// and the scan has to cross from the state stack into the history to see
/// it. This is the case a scan bounded at the root cannot detect.
#[test]
fn two_occurrences_before_the_root_and_one_in_the_tree_is_threefold() {
    // history = [P0, A, B, C, P0]; the root is A (Ke2, Black to move).
    let mut board = after("e1e2 e8e7 e2e1 e7e8 e1e2");
    assert_eq!(board.game_history().len(), 5);
    assert!(
        !board.is_repetition(),
        "the root A has occurred once before only"
    );
    line(&mut board, "e8e7 e2e1");
    assert!(!board.is_repetition());
    line(&mut board, "e7e8");
    assert!(
        board.is_repetition(),
        "P0 at ply 3: once here, twice in the history = threefold"
    );
}

/// One occurrence before the root is only a twofold, which is not a draw.
/// The second occurrence inside the tree then makes it one -- by the in-tree
/// rule, which needs nothing from the history.
#[test]
fn one_occurrence_before_the_root_is_not_yet_a_repetition() {
    // history = [P0]; the root is A.
    let mut board = after("e1e2");
    line(&mut board, "e8e7 e2e1 e7e8");
    assert!(
        !board.is_repetition(),
        "P0: once before the root, once here -- twofold"
    );
    line(&mut board, "e1e2 e8e7 e2e1 e7e8");
    assert!(board.is_repetition(), "P0 again: the tree repeats itself");
}

#[test]
fn the_root_itself_can_be_the_third_occurrence() {
    // history = [P0, A, B, C, P0, A, B, C]; the root is P0 for the third time.
    let board = after("e1e2 e8e7 e2e1 e7e8 e1e2 e8e7 e2e1 e7e8");
    assert_eq!(board.game_history().len(), 8);
    assert_eq!(board.ply(), 0);
    assert!(
        board.is_repetition(),
        "threefold entirely within the history"
    );
    // And the second occurrence was not.
    assert!(!after("e1e2 e8e7 e2e1 e7e8").is_repetition());
}

// ---------------------------------------------------------------------------
// The null-move bound
// ---------------------------------------------------------------------------

/// The same two occurrences before the root as above, reached inside the
/// tree through a null move: NOT a repetition. A null move lets one side
/// move twice running, so the line Kd8, Ke1, Kd7, (null), Ke8 lands on P0's
/// placement with White to move after five plies -- the keys agree, and
/// nothing about the game does.
#[test]
fn occurrences_separated_by_a_null_move_are_not_a_repetition() {
    let mut board = after("e1e2 e8e7 e2e1 e7e8 e1e2");
    line(&mut board, "e8d8 e2e1 d8d7 - d7e8");
    assert_eq!(board.plies_from_null(), 1);
    let p0 = Board::from_fen(P0).expect("P0 parses");
    assert_eq!(board.key(), p0.key(), "the keys agree -- that is the point");
    assert!(
        !board.is_repetition(),
        "a null move stands between this position and both earlier ones"
    );
}

/// In-tree twofold across a null move: the king triangulates on one side
/// of the null and not on the other, returning to the root's placement and
/// side to move in six plies. Without the bound the scan sees the root at
/// distance six and calls it a repetition; with it, the scan stops at the
/// null move.
#[test]
fn a_return_to_the_root_across_a_null_move_is_not_a_repetition() {
    let mut board = Board::from_fen(P0).expect("P0 parses");
    line(&mut board, "e1d1 - d1d2 e8d8 d2e1 d8e8");
    let p0 = Board::from_fen(P0).expect("P0 parses");
    assert_eq!(board.key(), p0.key());
    assert_eq!(board.plies_from_null(), 4);
    assert!(!board.is_repetition());

    // The control: the same return with real moves on both sides is one.
    let mut board = Board::from_fen(P0).expect("P0 parses");
    line(&mut board, "e1d1 e8d8 d1d2 d8d7 d2e1 d7e8");
    assert_eq!(board.key(), p0.key());
    assert!(board.is_repetition());
}

/// After the null move is unmade, the positions on its near side are
/// scanned again as normal.
#[test]
fn unmaking_the_null_move_restores_the_scan() {
    let mut board = after("e1e2 e8e7 e2e1 e7e8 e1e2");
    let made = line(&mut board, "e8e7 e2e1 - ");
    assert!(!board.is_repetition());
    unwind(&mut board, &made[2..]);
    line(&mut board, "e7e8");
    assert!(
        board.is_repetition(),
        "P0 three times, no null move in the way"
    );
}

// ---------------------------------------------------------------------------
// Through the walk: no false positives
// ---------------------------------------------------------------------------

/// Along random games from the corpus positions, `is_repetition` agrees with
/// a literal count over the full key sequence: true iff the current key
/// occurs at or after the root among the earlier keys, or at least twice
/// before the root -- restricted to the reversible tail. The walk makes
/// no null moves, so the counter never binds.
///
/// Random moves almost never repeat: a return needs both sides to undo, and
/// a uniform walk drifts. So three times in four a side plays the reverse
/// of its own last move when that is legal, and both sides oscillate. The
/// coverage is asserted, per branch, not assumed.
#[test]
fn the_scan_agrees_with_a_literal_count_along_random_games() {
    let mut rng = support::generative::Rng::new(0x5EED);
    let mut by_tree = 0;
    let mut by_history = 0;
    let irreversible = |board: &Board, m: Move| {
        m.is_capture()
            || board
                .piece_at(m.from_sq())
                .is_some_and(|p| p.piece_type() == cadence_core::PieceType::Pawn)
    };
    for fen in support::generative::walk_seeds() {
        let mut board = Board::from_fen(&fen).expect("seed parses");
        let mut keys = vec![board.key()];
        let mut irreversible_at = 0usize;
        let mut last: [Option<Move>; 2] = [None, None];
        let mut pick = |board: &Board, rng: &mut support::generative::Rng| {
            let legal = generate_legal(board);
            if legal.is_empty() {
                return None;
            }
            let us = board.side_to_move().index();
            let reverse = last[us].map(|m| Move::new_quiet(m.to_sq(), m.from_sq()));
            let m = match reverse {
                Some(r) if rng.below(4) != 0 && legal.contains(r) => r,
                _ => legal.as_slice()[rng.below(legal.len())],
            };
            last[us] = Some(m);
            Some(m)
        };
        // Some game moves first, so the history is populated.
        for _ in 0..30 {
            let Some(m) = pick(&board, &mut rng) else {
                break;
            };
            if irreversible(&board, m) {
                irreversible_at = keys.len();
            }
            board.play(m);
            keys.push(board.key());
        }
        let root = keys.len() - 1;
        // Then the tree.
        let mut made = 0;
        for _ in 0..40 {
            let Some(m) = pick(&board, &mut rng) else {
                break;
            };
            if irreversible(&board, m) {
                irreversible_at = keys.len();
            }
            board.make_move(m);
            made += 1;
            keys.push(board.key());
            let cur = *keys.last().expect("pushed");
            let current = keys.len() - 1;
            let mut in_tree = false;
            let mut before = 0;
            for (i, k) in keys.iter().enumerate().take(current).skip(irreversible_at) {
                if *k == cur {
                    if i >= root {
                        in_tree = true;
                    } else {
                        before += 1;
                    }
                }
            }
            let want = in_tree || before >= 2;
            assert_eq!(board.is_repetition(), want, "ply {made} from {fen}");
            by_tree += usize::from(in_tree);
            by_history += usize::from(!in_tree && before >= 2);
        }
    }
    println!("repetitions by branch: tree {by_tree}, history {by_history}");
    assert!(by_tree > 0, "the walks never repeated inside the tree");
    assert!(
        by_history > 0,
        "the walks never reached a position twice before the root and once in the tree; \
         the history branch went untested"
    );
}
