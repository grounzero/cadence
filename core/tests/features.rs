// SPDX-License-Identifier: GPL-3.0-or-later

//! `feature_index` must compute exactly this, each perspective a bijection onto `0..768` and
//! Black's view the colour-swapped mirror of White's:
//!
//! ```text
//! rel_colour = piece.colour ^ perspective       (0 = us)
//! rel_sq     = sq ^ (perspective == Black ? 56 : 0)
//! index      = rel_colour * 384 + piece_type * 64 + rel_sq
//! ```

use cadence_core::types::{Colour, File, Piece, PieceType, Rank, Square};
use cadence_core::{
    KING_BUCKET, KING_BUCKETS, NUM_INPUTS, bucketed_feature_index, feature_index, king_bucket,
    mirrored_feature_index, mirrors,
};

fn stated(persp: Colour, piece: Piece, sq: Square) -> usize {
    let rel_colour = piece.colour().index() ^ persp.index();
    let flip = if persp == Colour::Black { 56 } else { 0 };
    let rel_sq = sq.index() ^ flip;
    rel_colour * 384 + piece.piece_type().index() * 64 + rel_sq
}

#[test]
fn feature_index_is_the_stated_ordering_for_all_1536_inputs() {
    for persp in Colour::ALL {
        for piece in Piece::ALL {
            for sq in Square::all() {
                let got = feature_index(persp, piece, sq);
                assert_eq!(got, stated(persp, piece, sq), "{persp:?} {piece:?} {sq}");
                assert!(got < NUM_INPUTS);
            }
        }
    }
    assert_eq!(NUM_INPUTS, 768);
}

#[test]
fn each_perspective_is_a_bijection_onto_the_input_range() {
    for persp in Colour::ALL {
        let mut seen = vec![false; NUM_INPUTS];
        for piece in Piece::ALL {
            for sq in Square::all() {
                let i = feature_index(persp, piece, sq);
                assert!(!seen[i], "{persp:?}: index {i} used twice ({piece:?} {sq})");
                seen[i] = true;
            }
        }
        assert!(seen.iter().all(|s| *s), "{persp:?}: an input is never used");
    }
}

/// The perspective flip in one line.
#[test]
fn black_perspective_is_the_colour_swapped_mirror_of_white() {
    for piece in Piece::ALL {
        let swapped = Piece::new(piece.colour().flip(), piece.piece_type());
        for sq in Square::all() {
            assert_eq!(
                feature_index(Colour::Black, piece, sq),
                feature_index(Colour::White, swapped, sq.flip_vertical()),
                "{piece:?} {sq}"
            );
        }
    }
}

/// The same values the crate pins at compile time, restated so a report names them.
#[test]
fn named_pins() {
    assert_eq!(feature_index(Colour::White, Piece::WPawn, Square::A1), 0);
    assert_eq!(feature_index(Colour::White, Piece::WPawn, Square::H8), 63);
    assert_eq!(feature_index(Colour::White, Piece::WKnight, Square::A1), 64);
    assert_eq!(feature_index(Colour::White, Piece::WKing, Square::H8), 383);
    assert_eq!(feature_index(Colour::White, Piece::BPawn, Square::A1), 384);
    assert_eq!(feature_index(Colour::White, Piece::BKing, Square::H8), 767);
    // Black's own pawn on its relative a1 (the a8 square) is input 0.
    assert_eq!(feature_index(Colour::Black, Piece::BPawn, Square::A8), 0);
    assert_eq!(feature_index(Colour::Black, Piece::BKing, Square::H1), 383);
    assert_eq!(feature_index(Colour::Black, Piece::WPawn, Square::A8), 384);
    assert_eq!(
        feature_index(Colour::Black, Piece::WKing, Square::E1),
        384 + 5 * 64 + 60
    );
    // A piece type block is 64 wide, colour block 384.
    for pt in PieceType::ALL {
        assert_eq!(
            feature_index(Colour::White, Piece::new(Colour::White, pt), Square::A1),
            pt.index() * 64
        );
    }
}

/// The square on the same rank with its file read from the other edge.
fn across(sq: Square) -> Square {
    Square::new((sq.index() ^ 7) as u8)
}

/// The bucket stated without the table's indexing: the rank from the perspective's own back rank,
/// and on that back rank the distance from the nearer edge.
fn stated_bucket(persp: Colour, king: Square) -> usize {
    let rank = match persp {
        Colour::White => king.rank().index(),
        Colour::Black => 7 - king.rank().index(),
    };
    let edge = king.file().index().min(7 - king.file().index());
    match (rank, edge) {
        (0, 0 | 1) => 0,
        (0, _) => 1,
        (1, _) => 2,
        _ => 3,
    }
}

#[test]
fn the_bucket_table_is_the_stated_split_for_every_king() {
    assert_eq!(KING_BUCKETS, 4);
    assert_eq!(KING_BUCKET.iter().max(), Some(&(KING_BUCKETS - 1)));
    let mut kings = [0; KING_BUCKETS];
    for persp in Colour::ALL {
        for king in Square::all() {
            let got = king_bucket(persp, king);
            assert_eq!(got, stated_bucket(persp, king), "{persp:?} {king}");
            assert_eq!(got, king_bucket(persp, across(king)), "{persp:?} {king}");
            assert_eq!(
                got,
                king_bucket(persp.flip(), king.flip_vertical()),
                "{persp:?} {king}"
            );
            kings[got] += 1;
        }
    }
    assert_eq!(kings, [8, 8, 16, 96]);
}

/// Every perspective, king square, piece and square: 98,304 indices.
#[test]
fn the_bucketed_index_is_the_mirrored_index_in_its_buckets_block() {
    let mut compared = 0;
    for persp in Colour::ALL {
        for king in Square::all() {
            let block = king_bucket(persp, king) * NUM_INPUTS;
            for piece in Piece::ALL {
                for sq in Square::all() {
                    let got = bucketed_feature_index(persp, king, piece, sq);
                    assert!(got < KING_BUCKETS * NUM_INPUTS);
                    assert_eq!(
                        got,
                        block + mirrored_feature_index(persp, king, piece, sq),
                        "{persp:?} king {king} {piece:?} {sq}"
                    );
                    assert_eq!(
                        got,
                        bucketed_feature_index(persp, across(king), piece, across(sq)),
                        "{persp:?} king {king} {piece:?} {sq}"
                    );
                    compared += 1;
                }
            }
        }
    }
    assert_eq!(compared, 2 * 64 * 12 * 64);
}

#[test]
fn each_king_square_is_a_bijection_onto_its_buckets_block() {
    for persp in Colour::ALL {
        for king in Square::all() {
            let block = king_bucket(persp, king) * NUM_INPUTS;
            let mut seen = vec![false; NUM_INPUTS];
            for piece in Piece::ALL {
                for sq in Square::all() {
                    let i = bucketed_feature_index(persp, king, piece, sq) - block;
                    assert!(
                        !seen[i],
                        "{persp:?} king {king}: {i} twice ({piece:?} {sq})"
                    );
                    seen[i] = true;
                }
            }
            assert!(seen.iter().all(|s| *s), "{persp:?} king {king}");
        }
    }
}

/// A king that castles from each file Chess960 can start it on, b to g, to the g or the c file:
/// which starts change the bucket, and which of those leave the half as it was.
#[test]
fn a_castling_king_changes_bucket_where_the_table_says() {
    let lands = [File::ALL[6], File::ALL[2]];
    for persp in Colour::ALL {
        let back = Rank::One.relative(persp);
        let mut bucket_only = Vec::new();
        let mut both = Vec::new();
        for from in &File::ALL[1..7] {
            for to in lands {
                let (from, to) = (
                    Square::from_file_rank(*from, back),
                    Square::from_file_rank(to, back),
                );
                if king_bucket(persp, from) == king_bucket(persp, to) {
                    continue;
                }
                if mirrors(from) == mirrors(to) {
                    bucket_only.push((from.file().to_char(), to.file().to_char()));
                } else {
                    both.push((from.file().to_char(), to.file().to_char()));
                }
            }
        }
        assert_eq!(
            bucket_only,
            [('b', 'c'), ('e', 'g'), ('f', 'g')],
            "{persp:?}"
        );
        assert_eq!(both, [('c', 'g'), ('d', 'g'), ('g', 'c')], "{persp:?}");
    }
}

/// The same values the crate pins at compile time, restated so a report names them.
#[test]
fn named_bucket_pins() {
    assert_eq!(king_bucket(Colour::White, Square::B1), 0);
    assert_eq!(king_bucket(Colour::White, Square::C1), 1);
    assert_eq!(king_bucket(Colour::White, Square::G1), 0);
    assert_eq!(king_bucket(Colour::Black, Square::E8), 1);
    assert_eq!(king_bucket(Colour::Black, Square::E1), 3);
    assert_eq!(
        bucketed_feature_index(Colour::White, Square::E1, Piece::WPawn, Square::A1),
        NUM_INPUTS + 7
    );
    assert_eq!(
        bucketed_feature_index(Colour::Black, Square::E7, Piece::WPawn, Square::A8),
        2 * NUM_INPUTS + 384 + 7
    );
}
