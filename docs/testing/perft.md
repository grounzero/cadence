# Cadence verified perft corpus

Reference data for move-generation verification. Every number in this corpus was
**computed**, not quoted from a published table. Tests read the accompanying
[machine fixture](../../tests/fixtures/perft-corpus.txt); this document records
the provenance, conventions and diagnostic meaning of those values.

---

## Provenance

| | |
|---|---|
| **Generated** | 2026-08-18 |
| **Host** | Apple M5 Max, macOS (Darwin 25.6.0), aarch64 |
| **Oracle** | `python-chess` 1.11.2, installed into a throwaway directory, used solely to *compute* node counts and legal-move lists |
| **Method** | Recursive perft with bulk counting at depth 1 (`legal_moves.count()`), which yields identical leaf totals to full recursion. Deep values were computed with an 8-process root split. |
| **Cross-check** | Every standard-suite value below independently reproduces the long-published figures for those positions. The DFRC row `518/518` reproduces the standard start position exactly, which confirms the Chess960 code path and the standard path agree. |

### Castling convention assumed: read this before comparing numbers

Chess960 castling legality is **evaluated with BOTH the king and the castling
rook lifted from the occupancy** before testing whether any square of the
king's path is attacked.

This is not a stylistic choice; it changes node counts. Two positions in
section 3 (`4r3/6k1/8/8/8/8/8/3KR3 w E` and `5r2/4k3/8/8/8/8/8/4KR2 w F`) are
legal under a naive implementation that leaves the rook on the board, and
illegal under this convention. If Cadence ever disagrees with this corpus on a
DFRC position, check this rule before suspecting the magics.

The rest of the rule set, stated so it is unambiguous:

- King path = the closed segment `[king_from, king_to]`, **inclusive of both
  endpoints**. Every square of it must be unattacked (this folds "out of
  check", "through check" and "into check" into one test).
- King path ∪ rook path must be **empty**, excluding the king's and the
  rook's own origin squares (the castling pieces do not block themselves).
- The **rook's** path must be empty but **may be attacked**.
- Destinations are fixed by the rules and never derived from direction:
  kingside → king `g1`, rook `f1`; queenside → king `c1`, rook `d1`
  (mirrored on rank 8). "Kingside" means the rook stands on a **higher file**
  than the king.

### The two halves of this corpus do NOT have the same authority

This is the most important caveat in the document.

**Section 1 (standard suite) is cross-checked.** Those positions and node
counts have been published, recomputed and argued over for decades. The values
here were recomputed independently and agree with the long-established figures.
If Cadence disagrees with section 1, Cadence is wrong.

**Section 2 (DFRC) is not cross-checked against anything.** `python-chess` is
the **sole oracle** for every DFRC number in the fixture. Nothing here has been
compared against a second implementation, and there is no decades-old published
table for double-Fischer-random start arrays to fall back on. The same is true
of section 3 and of the constructed positions in section 4.

Therefore: **when Cadence disagrees with this corpus on a DFRC position, "the
corpus is wrong" is a live hypothesis** and must be investigated as seriously as
"the engine is wrong". Specifically:

- A disagreement in section 2 or 3 is roughly as likely to be a corpus bug, a
  FEN transcription error, or a castling-convention mismatch as it is a movegen
  bug.
- The convention below is the most probable source of a systematic disagreement,
  because it is a genuine implementation choice that changes node counts.
- Before debugging movegen against a disagreement in section 2 or 3, regenerate
  the disputed position from a **second independent implementation**. A corpus
  and an engine that share a bug agree perfectly, and this corpus has never
  been given the chance to disagree with anyone.

The generating scripts are not checked in: they are ~30 lines of
`python-chess` driving the recursion.

---

## Section 1. Standard perft suite

All values below were recomputed for this file.

| # | Position | FEN |
|---|---|---|
| 1 | startpos | `rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR w KQkq - 0 1` |
| 2 | Kiwipete | `r3k2r/p1ppqpb1/bn2pnp1/3PN3/1p2P3/2N2Q1p/PPPBBPPP/R3K2R w KQkq - 0 1` |
| 3 | pos3 | `8/2p5/3p4/KP5r/1R3p1k/8/4P1P1/8 w - - 0 1` |
| 4 | pos4 | `r3k2r/Pppp1ppp/1b3nbN/nP6/BBP1P3/q4N2/Pp1P2PP/R2Q1RK1 w kq - 0 1` |
| 5 | pos5 | `rnbq1k1r/pp1Pbppp/2p5/8/2B5/8/PPP1NnPP/RNBQK2R w KQ - 1 8` |
| 6 | pos6 | `r4rk1/1pp1qppp/p1np1n2/2b1p1B1/2B1P1b1/P1NP1N2/1PP1QPPP/R4RK1 w - - 0 10` |

| Position | d1 | d2 | d3 | d4 | d5 | d6 | d7 |
|---|---|---|---|---|---|---|---|
| startpos | 20 | 400 | 8,902 | 197,281 | 4,865,609 | 119,060,324 | 3,195,901,860 |
| Kiwipete | 48 | 2,039 | 97,862 | 4,085,603 | 193,690,690 | 8,031,647,685 |   |
| pos3 | 14 | 191 | 2,812 | 43,238 | 674,624 | 11,030,083 | 178,633,661 |
| pos4 | 6 | 264 | 9,467 | 422,333 | 15,833,292 | 706,045,033 |   |
| pos5 | 44 | 1,486 | 62,379 | 2,103,487 | 89,941,194 | 3,048,196,529 |   |
| pos6 | 46 | 2,079 | 89,890 | 3,894,594 | 164,075,551 | 6,923,051,137 |   |

An empty cell means no value is recorded at that depth, not that the value is
zero. Only the start position is carried to depth 7 here; the four rows that
stop at depth 6 stop because the reference run that produced this file stopped
there. The same table is repeated as machine data in
`tests/fixtures/perft-corpus.txt`, and the corpus integrity tests hold the two
copies to each other value by value, so a cell that is empty in one and filled
in the other fails the suite.

Machine-parseable, `name<TAB>depth<TAB>nodes`:

Machine-readable data: [`standard-perft`](../../tests/fixtures/perft-corpus.txt).


### Cost, measured

Using 8 worker processes on the M5 Max: `startpos` d7 took 748 s, `kiwipete`
d6 702 s, `pos6` d6 507 s, `pos5` d6 260 s. Those costs are why the deepest
values are an explicitly requested test tier rather than part of every run.

The reference set includes the start position to depth 7, and Kiwipete plus
positions 3-6 to depth 6.

---

## Section 2. DFRC start arrays

20 double-Fischer-random start positions. Castling rights are in **Shredder
notation** (rook files, uppercase for White) and **both sides hold both
rights** in every row.

The `wid/bid` column gives the Scharnagl index of White's and Black's back
rank independently: `518/518` is the standard array, and everything else
mixes two different shuffles, which is what makes this DFRC rather than FRC.

Schema: `wid<TAB>bid<TAB>fen<TAB>d1<TAB>d2<TAB>d3<TAB>d4<TAB>d5`

Machine-readable data: [`dfrc-arrays`](../../tests/fixtures/perft-corpus.txt).


### Why these 20, and the trap this section exists to expose

**A DFRC start array does not usually test castling.** In most arrays the king
and rook have one to three pieces between them, so no castle is reachable
until depth 5 or deeper. A DFRC corpus run to depth 4 will happily pass while
the castling code is completely broken.

Four of these arrays permit a castle **immediately, with no preparatory
move**, because the king and rook are already adjacent in the right
configuration. They were selected deliberately:

| wid/bid | Side | Castle available at that side's first move | Shape |
|---|---|---|---|
| `600/199` | White | `f1g1` | Kf1, Rg1: pure swap |
| `800/3` | White | `d1c1` | Kd1, Rc1: queenside, king lands on c1, rook to d1 |
| `800/3` | Black | `f8g8` | Kf8, Rg8: pure swap |
| `400/55` | Black | `f8g8` | Kf8, Rg8: pure swap |
| `911/88` | Black | `f8g8` | Kf8, Rg8: pure swap |

Evidence that this matters: adding Black's castling rights to array `400/55`
changed its d2 count from 380 to 400: twenty extra replies, all of them the
single castling move, at ply 2.

The remaining rows cover: the standard array (`518/518`, a control), both
extremes (`0/0`, `959/959`), both cross-products of the extremes (`0/959`,
`959/0`) which is the DFRC-specific case of asymmetric back ranks, and a
spread across the index space including several arrays with corner rooks and
several with the king on the b- or g-file.

Machine-checkable form of the table above. `colour` is the side whose castle
is being claimed; the Black rows are the position after `1. a3`, which is a
reachable position rather than a contrived side-to-move flip.

Schema: `wid<TAB>bid<TAB>colour<TAB>fen<TAB>castling<TAB>d1<TAB>d2<TAB>d3<TAB>d4`

Machine-readable data: [`immediate-castles`](../../tests/fixtures/perft-corpus.txt).


**These depths do not replace section 3.** Even with the four immediate-castle
arrays, depth 5 from a start array exercises only a thin slice of castling
geometry. The purpose-built positions in section 3 are where the degenerate
cases actually live.

---

## Section 3. DFRC castling legality

Each position isolates one rule. `verdict` is whether **any** castling move
is legal; `castling` gives the move in king-takes-rook notation.

Schema: `verdict<TAB>castling<TAB>fen<TAB>d1<TAB>d2<TAB>d3<TAB>reason`

Machine-readable data: [`castling-legality`](../../tests/fixtures/perft-corpus.txt).


### The ambiguity proof, in full

Machine-readable data: [`ambiguity-proof`](../../tests/fixtures/perft-corpus.txt).


`f1g1` already means something else. King-takes-rook is injective by
construction: the destination always holds a friendly rook, and no
non-castling move can ever land on one.

---

## Section 4. Check, evasion, promotion and en-passant edge cases

These are the cases that silently corrupt perft at depth 4+ while shallow
depths stay green. Between them they cover checks and evasions, promotions,
en passant and their interactions with pins and occupancy.

Schema: `fen<TAB>d1<TAB>d2<TAB>d3<TAB>d4<TAB>reason`

Machine-readable data: [`edge-cases`](../../tests/fixtures/perft-corpus.txt).


Full expected move lists at depth 1:

Machine-readable data: [`edge-case-moves`](../../tests/fixtures/perft-corpus.txt).


The evasion geometry, stated as data rather than as prose. `mask` is the
target set an evasion generator built as `checker | BETWEEN[king][checker]`
would search; the point of the row is that the legal move's destination is
**not in it**.

Schema: `fen<TAB>move<TAB>checker<TAB>mask<TAB>reason`

Machine-readable data: [`ep-evasion`](../../tests/fixtures/perft-corpus.txt).


### The rule the horizontal case forces

The en-passant capture removes **two** pieces from the same rank at once, so
neither pawn is individually pinned and no pin mask can express it. Verify
every generated en-passant capture with an explicit occupancy-modified test.

There is at most one ep *square* per position but **at most two ep captures**:
both pawns flanking the pushed pawn may take it. An earlier draft of this
line said "at most one such capture", which is false and is the premise an
implementer optimises against: written to it, the natural shape finds the ep
square and generates a single move from it. The cost conclusion is unaffected;
two occupancy tests per node is still nil:

Machine-readable data: [`ep-rule`](../../tests/fixtures/perft-corpus.txt).


---

## Section 5. Degenerate castling frequency

Exhaustive enumeration over all 960 Chess960 back-rank arrays, both castling
sides, 1,920 castling moves in total:

| Property | Result |
|---|---|
| Arrays where the king stands strictly between both rooks | **960 / 960** |
| Castles where exactly one piece moves (`DirtyPieces` len 1) | **552** |
| Castles where both pieces move (len 2) | **1,368** |
| Castles where neither piece moves (len 0) | **0** |

Two consequences that belong in the implementation, not in a comment:

1. Because the king is strictly between the rooks in every legal array,
   `castle_side` is safely **derived** from `rook_file > king_file` and must
   never be stored as a separate bit that could disagree with the squares.
2. **28.7% of DFRC castling moves are degenerate**: one of the two pieces
   does not move. This is not a corner case to handle later.

`len == 0` is unreachable: kingside would require `rook_from > king_from`
while the destinations satisfy `rook_to = f1 < g1 = king_to`. Therefore
`DirtyPieces::is_empty()` holds **iff** the move is a null move.

---

## Section 6. Castling-rights removal by capture

`update_mask[from] & update_mask[to]` is claimed to cover king moves, rook
moves, rook captures and rook-takes-rook in one branchless line. The first two
are exercised constantly by any perft run. The capture cases are not, and
under DFRC they are where the arbitrary rook files bite.

`before` and `after` are the castling field in **Shredder** notation, so the
assertion is on the emitted FEN rather than on an internal representation.

Schema: `fen<TAB>move<TAB>before<TAB>after<TAB>d1<TAB>d2<TAB>d3<TAB>d4<TAB>reason`

Machine-readable data: [`castling-rights-capture`](../../tests/fixtures/perft-corpus.txt).


---

## Section 7. FEN notation: X-FEN versus Shredder

These are two different notations and conflating them is a silent parse error,
not a cosmetic one.

- **Shredder-FEN** always names the castling rook's **file**: `HAha`.
- **X-FEN** uses `KQkq`, where the letter denotes the **outermost** rook on
  that side of the king, and falls back to the file letter **only** when that
  would be ambiguous: that is, when another rook of the same colour stands
  outside the castling rook on the same side.

For the standard start array the two agree, which is why every position in
section 1 passes under either reading. They diverge the moment the castling
rook is not on the a- or h-file, and that is what the rows below pin.

Both spellings of a row denote **the same position**, so both must parse to the
same rights and the same node counts.

Schema: `shredder<TAB>xfen<TAB>d1<TAB>d2<TAB>d3<TAB>d4<TAB>reason`

Machine-readable data: [`fen-notation`](../../tests/fixtures/perft-corpus.txt).


---

## Section 8. Move-list capacity

Nothing else in this corpus comes close to the 218-move bound, so `MAX_MOVES`
and the width of `MoveList`'s length field are otherwise untested, including
the `u8`-to-`u16` correction, which is unverified by every other position here.
A `u8` length wraps to zero at a capacity of 256 and would report **no legal
moves at all**.

Schema: `fen<TAB>d1<TAB>d2<TAB>d3<TAB>d4<TAB>reason`

Machine-readable data: [`move-capacity`](../../tests/fixtures/perft-corpus.txt).


Full expected move list at depth 1:

Machine-readable data: [`move-capacity-moves`](../../tests/fixtures/perft-corpus.txt).


---

## Section 9. Perft divide reference

The only tool for bisecting a perft mismatch is a divide, and a divide is only
useful against a reference. Depth 1 and depth 2 divides are uniform in their
counts, so their value is in the **root move list**, not the numbers: they
localise a wrong or missing root move immediately, before any recursion is
involved.

Moves are in king-takes-rook notation.

Schema: `name<TAB>depth<TAB>move<TAB>nodes`

Machine-readable data: [`perft-divide`](../../tests/fixtures/perft-corpus.txt).
