// SPDX-License-Identifier: GPL-3.0-or-later

//! A divide says which root move a wrong count sits under. At depths 1 and 2 the value is the root
//! move list, which localises a missing or spurious root move before any recursion.

mod support;

macro_rules! divide_tests {
    ($( $name:ident => $pos:literal, $depth:literal; )*) => { $(
        #[test]
        fn $name() {
            let expected = support::divide($pos, $depth);
            let fen = support::standard($pos).fen;
            let label = concat!("divide ", $pos, " d", stringify!($depth));

            let mut board = cadence_core::Board::from_fen(&fen)
                .unwrap_or_else(|e| panic!("{label}: FEN rejected ({e:?})\n  {fen}"));
            let mut got = cadence_core::perft_divide(&mut board, $depth);
            got.sort();

            let want_moves: Vec<String> = expected.iter().map(|(m, _)| m.clone()).collect();
            let got_moves: Vec<String> = got.iter().map(|(m, _)| m.clone()).collect();
            support::assert_move_list(label, &want_moves, &got_moves);

            let mismatches: Vec<String> = expected
                .iter()
                .zip(got.iter())
                .filter(|((_, w), (_, g))| w != g)
                .map(|((m, w), (_, g))| format!("    {m}: expected {w}, got {g}"))
                .collect();
            assert!(
                mismatches.is_empty(),
                "{label}\n  {fen}\n{}",
                mismatches.join("\n")
            );

            let total: u64 = got.iter().map(|(_, n)| n).sum();
            let want_total: u64 = expected.iter().map(|(_, n)| n).sum();
            assert_eq!(total, want_total, "{label}: divide does not sum to perft");
        }
    )* };
}

divide_tests! {
    divide_startpos_depth_1 => "startpos", 1;
    divide_startpos_depth_2 => "startpos", 2;
    divide_kiwipete_depth_1 => "kiwipete", 1;
}
