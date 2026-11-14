// SPDX-License-Identifier: GPL-3.0-or-later

//! A packed chunk: a 512-byte header, one record per game in index order, then a trailer, all
//! little-endian. A record's bytes depend only on its game, so a regenerated chunk's records match.

use cadence_core::Move;
use cadence_core::mv::MoveList;

use crate::crc32::crc32;

pub const MAGIC: [u8; 4] = *b"CDG\0";
pub const TRAILER_MAGIC: [u8; 4] = *b"CDT\0";
pub const FORMAT_VERSION: u16 = 1;
pub const HEADER_LEN: usize = 512;
pub const TRAILER_LEN: usize = 20;

/// A move whose search did not finish depth 1.
pub const NO_SCORE: i16 = i16::MIN;

/// Every real score lies within it, so none meets [`NO_SCORE`].
pub const SCORE_LIMIT: i16 = 32_000;

/// Both arrays drawn uniformly.
pub const KIND_UNIFORM: u8 = 0;

/// The standard array for both sides, by the run's share.
pub const KIND_STANDARD: u8 = 1;

/// The standard start's array number.
pub const STANDARD_ARRAY: u16 = 518;

/// The record's fields before its plies, its length field included.
const FIXED: usize = 26;

/// From bits 0 to 5, to bits 6 to 11, promotion bits 12 to 14 and castling bit 15, a castle
/// spelled king-takes-rook. Its own encoding, so a change to the engine's `Move` cannot move it.
#[must_use]
pub fn encode_move(m: Move) -> u16 {
    let promotion = m.promotion_piece().map_or(0, |p| p.index() as u16 + 1);
    m.from_sq().index() as u16
        | (m.to_sq().index() as u16) << 6
        | promotion << 12
        | u16::from(m.is_castle()) << 15
}

/// The legal move these bits name, so a record that decodes is a game the rules allow.
#[must_use]
pub fn decode_move(legal: &MoveList, bits: u16) -> Option<Move> {
    legal.iter().find(|&m| encode_move(m) == bits)
}

/// `<run seed, 16 hex>-<chunk index, 6 digits>.cdg`.
#[must_use]
pub fn chunk_name(run_seed: u64, chunk_index: u32) -> String {
    format!("{run_seed:016x}-{chunk_index:06}.cdg")
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
    /// Of the whole `cfg` feature string, which the 64-byte field cannot hold.
    pub cfg_sha256: [u8; 32],
}

/// Where each string field starts and how long it is.
const VERSION_FIELD: (usize, usize) = (28, 32);
const TARGET_FIELD: (usize, usize) = (60, 32);
const FEATURES_FIELD: (usize, usize) = (92, 64);
const HOST_FIELD: (usize, usize) = (186, 32);
const MODIFIED_AT: usize = 258;
const CFG_AT: usize = 259;
const RESERVED_AT: usize = 291;

impl Header {
    /// # Errors
    ///
    /// A string too long for its field or not ASCII.
    pub fn encode(&self) -> Result<[u8; HEADER_LEN], String> {
        let mut b = [0_u8; HEADER_LEN];
        b[0..4].copy_from_slice(&MAGIC);
        b[4..6].copy_from_slice(&FORMAT_VERSION.to_le_bytes());
        b[6..8].copy_from_slice(&(HEADER_LEN as u16).to_le_bytes());
        b[8..28].copy_from_slice(&self.commit);
        put_text(&mut b, VERSION_FIELD, "version", &self.version)?;
        put_text(&mut b, TARGET_FIELD, "target", &self.target)?;
        put_text(&mut b, FEATURES_FIELD, "features", &self.features)?;
        b[156..160].copy_from_slice(&self.nodes.to_le_bytes());
        b[160..164].copy_from_slice(&self.screen_nodes.to_le_bytes());
        b[164..166].copy_from_slice(&self.hash_mb.to_le_bytes());
        b[166] = self.random_plies;
        b[167..169].copy_from_slice(&self.window.to_le_bytes());
        b[169] = self.standard_share;
        b[170..178].copy_from_slice(&self.run_seed.to_le_bytes());
        b[178..182].copy_from_slice(&self.chunk_index.to_le_bytes());
        b[182..186].copy_from_slice(&self.games_planned.to_le_bytes());
        put_text(&mut b, HOST_FIELD, "host", &self.host)?;
        b[218..226].copy_from_slice(&self.start_time.to_le_bytes());
        b[226..258].copy_from_slice(&self.net_sha256);
        b[MODIFIED_AT] = self.modified;
        b[CFG_AT..RESERVED_AT].copy_from_slice(&self.cfg_sha256);
        Ok(b)
    }

    /// # Errors
    ///
    /// A magic, format version or header length not this format's, a string field that is not
    /// NUL-padded ASCII, or a reserved byte that is not zero.
    pub fn decode(b: &[u8; HEADER_LEN]) -> Result<Header, String> {
        if b[0..4] != MAGIC {
            return Err("not a chunk: the magic is wrong".to_string());
        }
        let version = u16_at(b, 4);
        if version != FORMAT_VERSION {
            return Err(format!("format version {version}, not {FORMAT_VERSION}"));
        }
        let length = u16_at(b, 6);
        if usize::from(length) != HEADER_LEN {
            return Err(format!("header length {length}, not {HEADER_LEN}"));
        }
        if b[RESERVED_AT..].iter().any(|&byte| byte != 0) {
            return Err("a reserved header byte is not zero".to_string());
        }
        let mut commit = [0; 20];
        commit.copy_from_slice(&b[8..28]);
        let mut net_sha256 = [0; 32];
        net_sha256.copy_from_slice(&b[226..258]);
        let mut cfg_sha256 = [0; 32];
        cfg_sha256.copy_from_slice(&b[CFG_AT..RESERVED_AT]);
        Ok(Header {
            commit,
            version: get_text(b, VERSION_FIELD, "version")?,
            target: get_text(b, TARGET_FIELD, "target")?,
            features: get_text(b, FEATURES_FIELD, "features")?,
            nodes: u32_at(b, 156),
            screen_nodes: u32_at(b, 160),
            hash_mb: u16_at(b, 164),
            random_plies: b[166],
            window: i16::from_le_bytes([b[167], b[168]]),
            standard_share: b[169],
            run_seed: u64_at(b, 170),
            chunk_index: u32_at(b, 178),
            games_planned: u32_at(b, 182),
            host: get_text(b, HOST_FIELD, "host")?,
            start_time: u64_at(b, 218),
            net_sha256,
            modified: b[MODIFIED_AT],
            cfg_sha256,
        })
    }
}

fn put_text(b: &mut [u8], (at, len): (usize, usize), name: &str, text: &str) -> Result<(), String> {
    if !text.is_ascii() || text.len() > len || text.bytes().any(|c| c == 0) {
        return Err(format!(
            "the {name} `{text}` is not ASCII of at most {len} bytes"
        ));
    }
    b[at..at + text.len()].copy_from_slice(text.as_bytes());
    Ok(())
}

/// NUL-padded: the text ends at the first NUL and only NULs follow it.
fn get_text(b: &[u8], (at, len): (usize, usize), name: &str) -> Result<String, String> {
    let field = &b[at..at + len];
    let end = field.iter().position(|&c| c == 0).unwrap_or(len);
    if !field[..end].is_ascii() || field[end..].iter().any(|&c| c != 0) {
        return Err(format!("the {name} field is not NUL-padded ASCII"));
    }
    Ok(String::from_utf8_lossy(&field[..end]).into_owned())
}

fn u16_at(b: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([b[at], b[at + 1]])
}

fn u32_at(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]])
}

fn u64_at(b: &[u8], at: usize) -> u64 {
    let mut word = [0; 8];
    word.copy_from_slice(&b[at..at + 8]);
    u64::from_le_bytes(word)
}

/// One game: what replays it and labels its positions.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GameRecord {
    pub index: u32,
    pub seed: u64,
    pub kind: u8,
    pub white: u16,
    pub black: u16,
    /// Encoded by [`encode_move`].
    pub plies: Vec<u16>,
    /// Each move encoded and its score from the side to move's view, or [`NO_SCORE`].
    pub moves: Vec<(u16, i16)>,
    /// 0 White, 1 Black, 2 draw.
    pub outcome: u8,
    /// 0 mate, 1 stalemate, 2 repetition, 3 fifty, 4 material.
    pub ending: u8,
    /// Saturating.
    pub refused: u16,
}

impl GameRecord {
    /// # Errors
    ///
    /// More random plies than a byte counts, or more moves than fit a record's length field.
    pub fn encode(&self) -> Result<Vec<u8>, String> {
        let plies = u8::try_from(self.plies.len()).map_err(|_| "too many random plies")?;
        let moves = u16::try_from(self.moves.len()).map_err(|_| "too many moves")?;
        let length = FIXED - 2 + 2 * self.plies.len() + 4 * self.moves.len() + 4;
        let length = u16::try_from(length).map_err(|_| "a record too long for its length field")?;
        let mut b = Vec::with_capacity(2 + usize::from(length));
        b.extend_from_slice(&length.to_le_bytes());
        b.extend_from_slice(&self.index.to_le_bytes());
        b.extend_from_slice(&self.seed.to_le_bytes());
        b.push(self.kind);
        b.extend_from_slice(&self.white.to_le_bytes());
        b.extend_from_slice(&self.black.to_le_bytes());
        b.push(plies);
        b.extend_from_slice(&moves.to_le_bytes());
        b.push(self.outcome);
        b.push(self.ending);
        b.extend_from_slice(&self.refused.to_le_bytes());
        for ply in &self.plies {
            b.extend_from_slice(&ply.to_le_bytes());
        }
        for (m, score) in &self.moves {
            b.extend_from_slice(&m.to_le_bytes());
            b.extend_from_slice(&score.to_le_bytes());
        }
        let crc = crc32(&b);
        b.extend_from_slice(&crc.to_le_bytes());
        Ok(b)
    }

    /// The record at the start of `b`, and how many bytes it took.
    ///
    /// # Errors
    ///
    /// A length past the end of `b`, a CRC that does not hold, counts that disagree with the
    /// length, or a field out of its range.
    pub fn decode(b: &[u8]) -> Result<(GameRecord, usize), String> {
        if b.len() < 2 {
            return Err("a record's length runs past the end".to_string());
        }
        let total = 2 + usize::from(u16_at(b, 0));
        if total > b.len() {
            return Err("a record's length runs past the end".to_string());
        }
        if total < FIXED + 4 {
            return Err("a record shorter than its fixed fields".to_string());
        }
        let stored = u32_at(b, total - 4);
        if crc32(&b[..total - 4]) != stored {
            return Err("a record whose CRC does not hold".to_string());
        }
        let (plies, moves) = (usize::from(b[19]), usize::from(u16_at(b, 20)));
        if FIXED + 2 * plies + 4 * moves + 4 != total {
            return Err("a record whose counts disagree with its length".to_string());
        }
        let mut record = GameRecord {
            index: u32_at(b, 2),
            seed: u64_at(b, 6),
            kind: b[14],
            white: u16_at(b, 15),
            black: u16_at(b, 17),
            plies: Vec::with_capacity(plies),
            moves: Vec::with_capacity(moves),
            outcome: b[22],
            ending: b[23],
            refused: u16_at(b, 24),
        };
        let mut at = FIXED;
        for _ in 0..plies {
            record.plies.push(u16_at(b, at));
            at += 2;
        }
        for _ in 0..moves {
            let score = i16::from_le_bytes([b[at + 2], b[at + 3]]);
            record.moves.push((u16_at(b, at), score));
            at += 4;
        }
        record.check()?;
        Ok((record, total))
    }

    /// Every field inside its range, a score inside the limit or no score at all.
    fn check(&self) -> Result<(), String> {
        let index = self.index;
        if self.kind > KIND_STANDARD {
            return Err(format!("game {index}: start kind {}", self.kind));
        }
        if self.white >= 960 || self.black >= 960 {
            return Err(format!(
                "game {index}: arrays {} {}",
                self.white, self.black
            ));
        }
        if self.kind == KIND_STANDARD
            && (self.white, self.black) != (STANDARD_ARRAY, STANDARD_ARRAY)
        {
            return Err(format!(
                "game {index}: a standard start at arrays {} {}",
                self.white, self.black
            ));
        }
        if self.outcome > 2 || self.ending > 4 {
            return Err(format!(
                "game {index}: outcome {} ending {}",
                self.outcome, self.ending
            ));
        }
        if let Some((_, score)) = self
            .moves
            .iter()
            .find(|&&(_, s)| s != NO_SCORE && !(-SCORE_LIMIT..=SCORE_LIMIT).contains(&s))
        {
            return Err(format!("game {index}: a score of {score}"));
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Trailer {
    pub games: u32,
    /// The moves of the game proper, summed over the chunk.
    pub plies: u64,
    pub header_crc: u32,
}

impl Trailer {
    #[must_use]
    pub fn encode(&self) -> [u8; TRAILER_LEN] {
        let mut b = [0; TRAILER_LEN];
        b[0..4].copy_from_slice(&TRAILER_MAGIC);
        b[4..8].copy_from_slice(&self.games.to_le_bytes());
        b[8..16].copy_from_slice(&self.plies.to_le_bytes());
        b[16..20].copy_from_slice(&self.header_crc.to_le_bytes());
        b
    }

    /// # Errors
    ///
    /// A trailer magic that is not this format's.
    pub fn decode(b: &[u8; TRAILER_LEN]) -> Result<Trailer, String> {
        if b[0..4] != TRAILER_MAGIC {
            return Err("no trailer: the trailer magic is wrong".to_string());
        }
        Ok(Trailer {
            games: u32_at(b, 4),
            plies: u64_at(b, 8),
            header_crc: u32_at(b, 16),
        })
    }
}

/// A complete chunk, read and held together.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Chunk {
    pub header: Header,
    pub records: Vec<GameRecord>,
    pub trailer: Trailer,
}

impl Chunk {
    /// The header's bytes, then the records, then the trailer naming their counts and the header's
    /// CRC.
    ///
    /// # Errors
    ///
    /// A header or a record that cannot be encoded.
    pub fn bytes(header: &Header, records: &[GameRecord]) -> Result<Vec<u8>, String> {
        let head = header.encode()?;
        let mut out = head.to_vec();
        let mut plies = 0_u64;
        for record in records {
            out.extend_from_slice(&record.encode()?);
            plies += record.moves.len() as u64;
        }
        let games = u32::try_from(records.len()).map_err(|_| "too many games")?;
        let trailer = Trailer {
            games,
            plies,
            header_crc: crc32(&head),
        };
        out.extend_from_slice(&trailer.encode());
        Ok(out)
    }

    /// # Errors
    ///
    /// Any part that does not decode, a trailer whose counts or header CRC disagree with what it
    /// closes, more games than the header planned, indices not `0..count` in order, a record whose
    /// random plies are not the header's count, or a byte after the trailer.
    pub fn read(b: &[u8]) -> Result<Chunk, String> {
        if b.len() < HEADER_LEN + TRAILER_LEN {
            return Err("shorter than a header and a trailer".to_string());
        }
        let head: &[u8; HEADER_LEN] = b[..HEADER_LEN].try_into().map_err(|_| "no header")?;
        let header = Header::decode(head)?;
        let tail: &[u8; TRAILER_LEN] = b[b.len() - TRAILER_LEN..]
            .try_into()
            .map_err(|_| "no trailer")?;
        let trailer = Trailer::decode(tail)?;
        if trailer.header_crc != crc32(head) {
            return Err("the header's CRC does not hold".to_string());
        }
        let body = &b[HEADER_LEN..b.len() - TRAILER_LEN];
        let records = read_records(&header, body)?;
        let plies: u64 = records.iter().map(|r| r.moves.len() as u64).sum();
        if u64::from(trailer.games) != records.len() as u64 || trailer.plies != plies {
            return Err(format!(
                "the trailer counts {} games and {} plies, and the records {} and {plies}",
                trailer.games,
                trailer.plies,
                records.len()
            ));
        }
        if trailer.games > header.games_planned {
            return Err(format!(
                "{} games where the header plans {}",
                trailer.games, header.games_planned
            ));
        }
        Ok(Chunk {
            header,
            records,
            trailer,
        })
    }
}

/// Records end to end with nothing after the last, held to the header.
fn read_records(header: &Header, mut body: &[u8]) -> Result<Vec<GameRecord>, String> {
    let mut records = Vec::new();
    while !body.is_empty() {
        let (record, used) = GameRecord::decode(body)?;
        let due = records.len();
        if record.index as usize != due {
            return Err(format!("game {} where game {due} is due", record.index));
        }
        if record.plies.len() != usize::from(header.random_plies) {
            return Err(format!(
                "game {due}: {} random plies where the header names {}",
                record.plies.len(),
                header.random_plies
            ));
        }
        records.push(record);
        body = &body[used..];
    }
    Ok(records)
}

/// The records a `.tmp` holds whole: each read while its length fits and its CRC holds, and the
/// byte count they take, where a torn last record is cut.
#[must_use]
pub fn whole_records(body: &[u8]) -> (Vec<GameRecord>, usize) {
    let mut records = Vec::new();
    let mut at = 0;
    while let Ok((record, used)) = GameRecord::decode(&body[at..]) {
        if record.index as usize != records.len() {
            break;
        }
        records.push(record);
        at += used;
    }
    (records, at)
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
