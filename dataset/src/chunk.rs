// SPDX-License-Identifier: GPL-3.0-or-later

//! A packed chunk: a 512-byte header, one record per game in index order, then a trailer, all
//! little-endian. A record's bytes depend only on its game, so a regenerated chunk's records match.

use cadence_core::Move;
use cadence_core::mv::MoveList;

pub const MAGIC: [u8; 4] = *b"CDG\0";
pub const TRAILER_MAGIC: [u8; 4] = *b"CDT\0";
pub const FORMAT_VERSION: u16 = 1;
pub const HEADER_LEN: usize = 512;
pub const TRAILER_LEN: usize = 20;
pub const NO_SCORE: i16 = i16::MIN;
pub const SCORE_LIMIT: i16 = 32_000;
pub const KIND_UNIFORM: u8 = 0;
pub const KIND_STANDARD: u8 = 1;
pub const STANDARD_ARRAY: u16 = 518;

#[must_use]
pub fn encode_move(_m: Move) -> u16 {
    0
}

#[must_use]
pub fn decode_move(_legal: &MoveList, _bits: u16) -> Option<Move> {
    None
}

#[must_use]
pub fn chunk_name(_run_seed: u64, _chunk_index: u32) -> String {
    String::new()
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Header {
    pub commit: [u8; 20],
    pub version: String,
    pub target: String,
    pub features: String,
    pub nodes: u32,
    pub screen_nodes: u32,
    pub hash_mb: u16,
    pub random_plies: u8,
    pub window: i16,
    pub standard_share: u8,
    pub run_seed: u64,
    pub chunk_index: u32,
    pub games_planned: u32,
    pub host: String,
    pub start_time: u64,
    pub net_sha256: [u8; 32],
    pub modified: u8,
    pub cfg_sha256: [u8; 32],
}

impl Header {
    /// # Errors
    ///
    /// Not yet.
    pub fn encode(&self) -> Result<[u8; HEADER_LEN], String> {
        Ok([0; HEADER_LEN])
    }

    /// # Errors
    ///
    /// Not yet.
    pub fn decode(_b: &[u8; HEADER_LEN]) -> Result<Header, String> {
        Err("not built".to_string())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GameRecord {
    pub index: u32,
    pub seed: u64,
    pub kind: u8,
    pub white: u16,
    pub black: u16,
    pub plies: Vec<u16>,
    pub moves: Vec<(u16, i16)>,
    pub outcome: u8,
    pub ending: u8,
    pub refused: u16,
}

impl GameRecord {
    /// # Errors
    ///
    /// Not yet.
    pub fn encode(&self) -> Result<Vec<u8>, String> {
        Ok(Vec::new())
    }

    /// # Errors
    ///
    /// Not yet.
    pub fn decode(_b: &[u8]) -> Result<(GameRecord, usize), String> {
        Err("not built".to_string())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Trailer {
    pub games: u32,
    pub plies: u64,
    pub header_crc: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Chunk {
    pub header: Header,
    pub records: Vec<GameRecord>,
    pub trailer: Trailer,
}

impl Chunk {
    /// # Errors
    ///
    /// Not yet.
    pub fn bytes(_header: &Header, _records: &[GameRecord]) -> Result<Vec<u8>, String> {
        Ok(Vec::new())
    }

    /// # Errors
    ///
    /// Not yet.
    pub fn read(_b: &[u8]) -> Result<Chunk, String> {
        Err("not built".to_string())
    }
}

#[must_use]
pub fn whole_records(_body: &[u8]) -> (Vec<GameRecord>, usize) {
    (Vec::new(), 0)
}

#[cfg(test)]
mod tests {
    use cadence_core::types::{PromoPiece, Square};
    use cadence_core::{Move, START_FEN, generate_legal, position::Board};

    use super::{
        Chunk, GameRecord, Header, KIND_STANDARD, KIND_UNIFORM, NO_SCORE, decode_move, encode_move,
        whole_records,
    };
    use crate::sha256::hex;

    fn square(name: &str) -> Square {
        let b = name.as_bytes();
        Square::new((b[1] - b'1') * 8 + (b[0] - b'a'))
    }

    fn header() -> Header {
        Header {
            commit: [0xab; 20],
            version: "0.6.0-dev-abc1234".to_string(),
            target: "aarch64-apple-darwin".to_string(),
            features: "+neon".to_string(),
            nodes: 5_000,
            screen_nodes: 5_000,
            hash_mb: 16,
            random_plies: 2,
            window: 200,
            standard_share: 50,
            run_seed: 0x0123_4567_89ab_cdef,
            chunk_index: 3,
            games_planned: 4,
            host: "mac".to_string(),
            start_time: 1_794_600_000,
            net_sha256: [0; 32],
            modified: 0,
            cfg_sha256: [7; 32],
        }
    }

    /// A castle king-takes-rook, the four promotions, an en passant capture and a move with no
    /// score, built by hand so no search can move the bytes.
    fn hand_built() -> GameRecord {
        let castle = Move::new_castle(square("b1"), square("a1"));
        let promotions = [
            PromoPiece::Knight,
            PromoPiece::Bishop,
            PromoPiece::Rook,
            PromoPiece::Queen,
        ]
        .map(|p| Move::new_promotion(square("c7"), square("c8"), p));
        let passant = Move::new_en_passant(square("e5"), square("d6"));
        let mut moves = vec![(encode_move(castle), 12)];
        moves.extend(promotions.iter().map(|&m| (encode_move(m), -5)));
        moves.push((encode_move(passant), NO_SCORE));
        GameRecord {
            index: 0,
            seed: 0xfeed_beef_0000_0001,
            kind: KIND_UNIFORM,
            white: 17,
            black: 900,
            plies: vec![encode_move(Move::new_quiet(square("g1"), square("f3"))); 2],
            moves,
            outcome: 2,
            ending: 3,
            refused: 4,
        }
    }

    /// The format on disk, pinned: a change to the engine's `Move` or to the layout fails here.
    #[test]
    fn a_hand_built_record_encodes_to_its_pinned_bytes() {
        let castle = Move::new_castle(square("b1"), square("a1"));
        assert_eq!(
            encode_move(castle),
            0x8001,
            "b1 is 1, a1 is 0, the castle bit set"
        );
        let queen = Move::new_promotion(square("c7"), square("c8"), PromoPiece::Queen);
        assert_eq!(
            encode_move(queen),
            0x4eb2,
            "c7 is 50, c8 is 58, the queen 4"
        );
        let bytes = hand_built().encode().expect("encodes");
        assert_eq!(
            hex(&bytes),
            "38000000000001000000efbeedfe0011008403020600020304004605460501800c00b21efbffb22efbf\
             fb23efbffb24efbffe40a0080e350b385"
        );
        let (back, used) = GameRecord::decode(&bytes).expect("decodes");
        assert_eq!((back, used), (hand_built(), bytes.len()));
    }

    #[test]
    fn a_header_round_trips_and_refuses_what_it_cannot_hold() {
        let bytes = header().encode().expect("encodes");
        assert_eq!(Header::decode(&bytes).expect("decodes"), header());
        let mut long = header();
        long.host = "h".repeat(33);
        assert!(long.encode().is_err(), "a 33-byte host was taken");
        for (at, what) in [
            (0, "magic"),
            (4, "format version"),
            (6, "header length"),
            (400, "reserved"),
        ] {
            let mut bad = bytes;
            bad[at] ^= 1;
            assert!(Header::decode(&bad).is_err(), "a changed {what} was read");
        }
    }

    #[test]
    fn a_chunk_round_trips_and_refuses_each_planted_fault() {
        let mut second = hand_built();
        second.index = 1;
        second.kind = KIND_STANDARD;
        (second.white, second.black) = (518, 518);
        let records = vec![hand_built(), second];
        let bytes = Chunk::bytes(&header(), &records).expect("a chunk");
        let chunk = Chunk::read(&bytes).expect("reads");
        assert_eq!(chunk.records, records);
        assert_eq!(chunk.trailer.games, 2);

        let at_record = super::HEADER_LEN;
        let length = usize::from(u16::from_le_bytes([bytes[at_record], bytes[at_record + 1]]));
        let mut faults: Vec<(&str, Vec<u8>)> = Vec::new();
        let mut b = bytes.clone();
        b[at_record + 10] ^= 1;
        faults.push(("a record's byte flipped, its CRC not", b));
        let mut b = bytes.clone();
        b[at_record] = 0xff;
        b[at_record + 1] = 0xff;
        faults.push(("a record length past the end", b));
        let mut b = bytes.clone();
        b[3] = b'X';
        faults.push(("the magic", b));
        let mut b = bytes.clone();
        b[100] ^= 1;
        faults.push(("a header byte, its CRC in the trailer not", b));
        let mut b = bytes.clone();
        let games_at = b.len() - super::TRAILER_LEN + 4;
        b[games_at] = 3;
        faults.push(("a trailer counting one game more", b));
        let mut b = bytes.clone();
        b.push(0);
        faults.push(("a byte after the trailer", b));
        let mut b = bytes.clone();
        b.truncate(super::HEADER_LEN + 2 + length);
        b.extend_from_slice(&bytes[bytes.len() - super::TRAILER_LEN..]);
        faults.push(("a record missing before the trailer", b));
        for (what, b) in faults {
            assert!(Chunk::read(&b).is_err(), "{what} was read");
        }
    }

    #[test]
    fn a_score_past_the_limit_is_refused_and_no_score_is_not() {
        let mut record = hand_built();
        record.moves[0].1 = 32_500;
        let bytes = record.encode().expect("encodes");
        assert!(
            GameRecord::decode(&bytes).is_err(),
            "a score of 32,500 was read"
        );
        record.moves[0].1 = NO_SCORE;
        assert!(GameRecord::decode(&record.encode().expect("encodes")).is_ok());
        let mut standard = hand_built();
        standard.kind = KIND_STANDARD;
        assert!(GameRecord::decode(&standard.encode().expect("encodes")).is_err());
    }

    /// Every legal move from the start decodes back to itself, and bits naming no legal move
    /// decode to nothing.
    #[test]
    fn a_move_decodes_only_against_its_legal_list() {
        let board = Board::from_fen(START_FEN).expect("the start");
        let legal = generate_legal(&board);
        for m in legal.iter() {
            assert_eq!(decode_move(&legal, encode_move(m)), Some(m));
        }
        let rook = encode_move(Move::new_quiet(square("a1"), square("a5")));
        assert_eq!(decode_move(&legal, rook), None);
    }

    /// A torn `.tmp` keeps exactly the records whole before the cut, at every byte.
    #[test]
    fn whole_records_keep_exactly_what_is_complete_at_every_cut() {
        let mut second = hand_built();
        second.index = 1;
        let first = hand_built().encode().expect("encodes");
        let body = [first.clone(), second.encode().expect("encodes")].concat();
        for cut in 0..=body.len() {
            let (records, used) = whole_records(&body[..cut]);
            let want = if cut == body.len() {
                2
            } else {
                usize::from(cut >= first.len())
            };
            assert_eq!(records.len(), want, "cut at {cut}");
            assert_eq!(used, [0, first.len(), body.len()][want], "cut at {cut}");
        }
    }
}
