#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-or-later
"""Recompute the perft corpus's count columns with python-chess, its oracle, and compare each with the
fixture. A count above --max-nodes is reported as skipped rather than compared, since the deepest take
minutes each even split across processes.

    python3 tests/fixtures/perft-corpus-generator.py tests/fixtures/perft-corpus.txt [--max-nodes N]
"""
import argparse
import re
import sys

import chess

STANDARD = {
    "startpos": chess.STARTING_FEN,
    "kiwipete": "r3k2r/p1ppqpb1/bn2pnp1/3PN3/1p2P3/2N2Q1p/PPPBBPPP/R3K2R w KQkq - 0 1",
    "pos3": "8/2p5/3p4/KP5r/1R3p1k/8/4P1P1/8 w - - 0 1",
    "pos4": "r3k2r/Pppp1ppp/1b3nbN/nP6/BBP1P3/q4N2/Pp1P2PP/R2Q1RK1 w kq - 0 1",
    "pos5": "rnbq1k1r/pp1Pbppp/2p5/8/2B5/8/PPP1NnPP/RNBQK2R w KQ - 1 8",
    "pos6": "r4rk1/1pp1qppp/p1np1n2/2b1p1B1/2B1P1b1/P1NP1N2/1PP1QPPP/R4RK1 w - - 0 10",
}
# Section: the FEN's column, the depth-1 count's column and how many depths follow it.
DEPTHS = {"dfrc-arrays": (2, 3, 5), "immediate-castles": (3, 5, 4), "castling-legality": (2, 3, 3),
          "edge-cases": (0, 1, 4), "castling-rights-capture": (0, 4, 4), "fen-notation": (0, 2, 4),
          "move-capacity": (0, 1, 4)}


def perft(board, depth):
    if depth == 1:
        return board.legal_moves.count()
    total = 0
    for move in list(board.legal_moves):
        board.push(move)
        total += perft(board, depth - 1)
        board.pop()
    return total


def cases(text):
    """Every (label, FEN, chess960, moves before counting, depth, expected count) the fixture holds."""
    for name, body in re.findall(r"```tsv ([a-z0-9-]+)\n(.*?)```", text, re.S):
        for row in (line.split("\t") for line in body.strip().split("\n")):
            if name in DEPTHS:
                fen_col, first, depths = DEPTHS[name]
                for d in range(1, depths + 1):
                    cell = row[first + d - 1].strip() if first + d - 1 < len(row) else ""
                    if cell:
                        yield f"{name} {row[fen_col]}", row[fen_col], True, [], d, int(cell)
            elif name == "standard-perft":
                yield f"{name} {row[0]}", STANDARD[row[0]], False, [], int(row[1]), int(row[2])
            elif name == "perft-divide" and int(row[1]) > 1:
                yield f"{name} {row[0]} {row[2]}", STANDARD[row[0]], False, [row[2]], int(row[1]) - 1, int(row[3])
            elif name == "perft-divide":
                yield f"{name} {row[0]} {row[2]}", STANDARD[row[0]], False, [row[2]], 0, int(row[3])


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("fixture")
    ap.add_argument("--max-nodes", type=int, default=20_000_000)
    a = ap.parse_args()
    compared = skipped = bad = 0
    for label, fen, chess960, moves, depth, expected in cases(open(a.fixture).read()):
        if expected > a.max_nodes:
            skipped += 1
            continue
        board = chess.Board(fen, chess960=chess960)
        for uci in moves:
            board.push_uci(uci)
        got = perft(board, depth) if depth else 1
        compared += 1
        if got != expected:
            bad += 1
            print(f"MISMATCH {label} depth {depth}: fixture {expected}, python-chess {got}")
    print(f"{compared} counts recomputed and compared, {bad} mismatched, {skipped} above {a.max_nodes:,} nodes skipped")
    return 1 if bad else 0


if __name__ == "__main__":
    sys.exit(main())
