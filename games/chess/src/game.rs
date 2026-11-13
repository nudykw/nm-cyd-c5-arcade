//! Повний рушій класичних шахів (FIDE) + альфа-бета ШІ.
//!
//! Без `std`, без `alloc`, без сторонніх залежностей — компілюється і як частина прошивки ESP32-C5,
//! і окремо на хості (`rustc --edition 2024 --test games/chess/src/game.rs -O`).
//!
//! Реалізовані всі правила класичних шахів:
//! - усі ходи фігур, подвійний крок пішака, взяття на проході (*en passant*);
//! - коротка та довга рокіровки (`O-O`, `O-O-O`) з перевіркою битих полів і втрати прав при русі/взятті тури;
//! - перетворення пішака у ферзя, туру, слона або коня (`Q`, `R`, `B`, `N`);
//! - шах, мат, пат, нічия за правилом 50 ходів, триразовим повторенням позиції (Zobrist 64-bit)
//!   та недостатністю матеріалу (`K-K`, `K+N-K`, `K+B-K`, `K+B-K+B` однопольні);
//! - скасування ходів (`Undo`) та альфа-бета пошук із Quiescence, MVV-LVA, Killer-ходами й PST.

pub const BOARD_N: usize = 8;
pub const BOARD_CELLS: usize = 64;
pub const MAX_MOVES: usize = 218;
const MAX_CAPS: usize = 64;
const MAX_HISTORY: usize = 160;
const MAX_PLY: usize = 16;

// Прапорці прав на рокіровку
pub const CASTLE_WK: u8 = 1 << 0;
pub const CASTLE_WQ: u8 = 1 << 1;
pub const CASTLE_BK: u8 = 1 << 2;
pub const CASTLE_BQ: u8 = 1 << 3;
pub const CASTLE_ALL: u8 = CASTLE_WK | CASTLE_WQ | CASTLE_BK | CASTLE_BQ;

// Прапорці ходу
pub const FLAG_NONE: u8 = 0;
pub const FLAG_CAPTURE: u8 = 1 << 0;
pub const FLAG_DOUBLE_PAWN: u8 = 1 << 1;
pub const FLAG_EN_PASSANT: u8 = 1 << 2;
pub const FLAG_CASTLE_KING: u8 = 1 << 3;
pub const FLAG_CASTLE_QUEEN: u8 = 1 << 4;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum Color {
    White = 0,
    Black = 1,
}

impl Color {
    #[inline]
    pub const fn opponent(self) -> Self {
        match self {
            Self::White => Self::Black,
            Self::Black => Self::White,
        }
    }

    #[inline]
    pub const fn idx(self) -> usize {
        self as usize
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum PieceKind {
    Pawn = 0,
    Knight = 1,
    Bishop = 2,
    Rook = 3,
    Queen = 4,
    King = 5,
}

impl PieceKind {
    #[inline]
    pub const fn idx(self) -> usize {
        self as usize
    }

    #[inline]
    pub const fn value_cp(self) -> i32 {
        match self {
            Self::Pawn => 100,
            Self::Knight => 320,
            Self::Bishop => 330,
            Self::Rook => 500,
            Self::Queen => 900,
            Self::King => 20_000,
        }
    }

    #[inline]
    pub const fn capture_points(self) -> u32 {
        match self {
            Self::Pawn => 10,
            Self::Knight | Self::Bishop => 30,
            Self::Rook => 50,
            Self::Queen => 90,
            Self::King => 0,
        }
    }

    #[inline]
    pub const fn promo_code(self) -> u8 {
        match self {
            Self::Knight => 1,
            Self::Bishop => 2,
            Self::Rook => 3,
            Self::Queen => 4,
            _ => 0,
        }
    }

    #[inline]
    pub const fn from_promo_code(c: u8) -> Option<Self> {
        match c {
            1 => Some(Self::Knight),
            2 => Some(Self::Bishop),
            3 => Some(Self::Rook),
            4 => Some(Self::Queen),
            _ => None,
        }
    }

    #[inline]
    pub const fn promo_char(self) -> u8 {
        match self {
            Self::Knight => b'n',
            Self::Bishop => b'b',
            Self::Rook => b'r',
            Self::Queen => b'q',
            _ => b' ',
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Piece {
    pub color: Color,
    pub kind: PieceKind,
}

impl Piece {
    #[inline]
    pub const fn new(color: Color, kind: PieceKind) -> Self {
        Self { color, kind }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PlayMode {
    AiHard,
    AiEasy,
    TwoPlayers,
}

impl PlayMode {
    pub const fn next(self) -> Self {
        match self {
            Self::AiHard => Self::AiEasy,
            Self::AiEasy => Self::TwoPlayers,
            Self::TwoPlayers => Self::AiHard,
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::AiHard => "AI: HARD",
            Self::AiEasy => "AI: EASY",
            Self::TwoPlayers => "2 PLAYR",
        }
    }

    pub const fn is_vs_ai(self) -> bool {
        !matches!(self, Self::TwoPlayers)
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum AiLevel {
    Easy,
    Demo,
    Hard,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum GameStatus {
    Ongoing { in_check: bool },
    Checkmate { winner: Color },
    Stalemate,
    DrawFiftyMove,
    DrawRepetition,
    DrawInsufficientMaterial,
}

impl GameStatus {
    #[inline]
    pub const fn is_game_over(self) -> bool {
        !matches!(self, Self::Ongoing { .. })
    }
}

/// Компактне представлення ходу (4 байти).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Move {
    pub from: u8,
    pub to: u8,
    promo_raw: u8,
    pub flags: u8,
}

impl Move {
    #[inline]
    pub const fn new(from: u8, to: u8, promo: Option<PieceKind>, flags: u8) -> Self {
        let promo_raw = match promo {
            Some(pk) => pk.promo_code(),
            None => 0,
        };
        Self {
            from,
            to,
            promo_raw,
            flags,
        }
    }

    #[inline]
    pub const fn promo(self) -> Option<PieceKind> {
        PieceKind::from_promo_code(self.promo_raw)
    }

    #[inline]
    pub const fn is_capture(self) -> bool {
        (self.flags & FLAG_CAPTURE) != 0
    }

    #[inline]
    pub const fn is_en_passant(self) -> bool {
        (self.flags & FLAG_EN_PASSANT) != 0
    }

    #[inline]
    pub const fn is_castle(self) -> bool {
        (self.flags & (FLAG_CASTLE_KING | FLAG_CASTLE_QUEEN)) != 0
    }

    /// Записує хід у читабельному вигляді (`"e2e4"`, `"e7e8Q"`, `"O-O"`, `"O-O-O"`).
    pub fn format_short(self, buf: &mut [u8; 6]) -> &str {
        if (self.flags & FLAG_CASTLE_KING) != 0 {
            buf[..3].copy_from_slice(b"O-O");
            return unsafe { core::str::from_utf8_unchecked(&buf[..3]) };
        }
        if (self.flags & FLAG_CASTLE_QUEEN) != 0 {
            buf[..5].copy_from_slice(b"O-O-O");
            return unsafe { core::str::from_utf8_unchecked(&buf[..5]) };
        }
        buf[0] = b'a' + (self.from & 7);
        buf[1] = b'1' + (self.from >> 3);
        buf[2] = b'a' + (self.to & 7);
        buf[3] = b'1' + (self.to >> 3);
        if let Some(pk) = self.promo() {
            buf[4] = pk.promo_char().to_ascii_uppercase();
            unsafe { core::str::from_utf8_unchecked(&buf[..5]) }
        } else {
            unsafe { core::str::from_utf8_unchecked(&buf[..4]) }
        }
    }
}

pub struct MoveList {
    moves: [Move; MAX_MOVES],
    len: usize,
}

impl MoveList {
    #[inline]
    pub const fn new() -> Self {
        Self {
            moves: [Move {
                from: 0,
                to: 0,
                promo_raw: 0,
                flags: 0,
            }; MAX_MOVES],
            len: 0,
        }
    }

    #[inline]
    fn push(&mut self, mv: Move) {
        if self.len < MAX_MOVES {
            self.moves[self.len] = mv;
            self.len += 1;
        }
    }

    #[inline]
    pub fn len(&self) -> usize {
        self.len
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    #[inline]
    pub fn as_slice(&self) -> &[Move] {
        &self.moves[..self.len]
    }

    #[inline]
    pub fn as_mut_slice(&mut self) -> &mut [Move] {
        &mut self.moves[..self.len]
    }
}

impl Default for MoveList {
    fn default() -> Self {
        Self::new()
    }
}

#[inline]
pub const fn sq_of(file: usize, rank: usize) -> u8 {
    ((rank << 3) | file) as u8
}

#[inline]
pub const fn file_of(sq: u8) -> usize {
    (sq & 7) as usize
}

#[inline]
pub const fn rank_of(sq: u8) -> usize {
    (sq >> 3) as usize
}

pub fn parse_sq(s: &[u8]) -> Option<u8> {
    if s.len() < 2 {
        return None;
    }
    let f = s[0].to_ascii_lowercase();
    let r = s[1];
    if (b'a'..=b'h').contains(&f) && (b'1'..=b'8').contains(&r) {
        Some(sq_of((f - b'a') as usize, (r - b'1') as usize))
    } else {
        None
    }
}

// Маска скидання прав рокіровки при ході з поля або взятті на полі.
const CASTLE_SPOIL: [u8; 64] = {
    let mut t = [CASTLE_ALL; 64];
    t[0] = CASTLE_ALL & !CASTLE_WQ;
    t[4] = CASTLE_ALL & !(CASTLE_WK | CASTLE_WQ);
    t[7] = CASTLE_ALL & !CASTLE_WK;
    t[56] = CASTLE_ALL & !CASTLE_BQ;
    t[60] = CASTLE_ALL & !(CASTLE_BK | CASTLE_BQ);
    t[63] = CASTLE_ALL & !CASTLE_BK;
    t
};

// Детерміновані константи Zobrist (SplitMix64 під час компіляції)
struct ZobristTable {
    pieces: [[[u64; 64]; 6]; 2],
    castling: [u64; 16],
    ep_file: [u64; 8],
    side_black: u64,
}

const fn splitmix64(mut x: u64) -> (u64, u64) {
    x = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = x;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z = z ^ (z >> 31);
    (x, z)
}

const ZOBRIST: ZobristTable = {
    let mut seed = 0xC5E5_5A11_2026_1112u64;
    let mut pieces = [[[0u64; 64]; 6]; 2];
    let mut c = 0;
    while c < 2 {
        let mut k = 0;
        while k < 6 {
            let mut s = 0;
            while s < 64 {
                let (ns, v) = splitmix64(seed);
                seed = ns;
                pieces[c][k][s] = v;
                s += 1;
            }
            k += 1;
        }
        c += 1;
    }
    let mut castling = [0u64; 16];
    let mut i = 0;
    while i < 16 {
        let (ns, v) = splitmix64(seed);
        seed = ns;
        castling[i] = v;
        i += 1;
    }
    let mut ep_file = [0u64; 8];
    let mut f = 0;
    while f < 8 {
        let (ns, v) = splitmix64(seed);
        seed = ns;
        ep_file[f] = v;
        f += 1;
    }
    let (_, side_black) = splitmix64(seed);
    ZobristTable {
        pieces,
        castling,
        ep_file,
        side_black,
    }
};

// Позиційні таблиці (орієнтовані зверху вниз: рядок 0 = 8-ма горизонталь для білих, `sq ^ 56`)
#[rustfmt::skip]
const PST_PAWN: [i16; 64] = [
     0,  0,  0,  0,  0,  0,  0,  0,
    50, 50, 50, 50, 50, 50, 50, 50,
    12, 12, 22, 32, 32, 22, 12, 12,
     6,  6, 12, 26, 26, 12,  6,  6,
     0,  0,  2, 22, 22,  2,  0,  0,
     5, -4, -8,  0,  0, -8, -4,  5,
     5, 10, 10,-20,-20, 10, 10,  5,
     0,  0,  0,  0,  0,  0,  0,  0,
];

#[rustfmt::skip]
const PST_KNIGHT: [i16; 64] = [
   -50,-40,-30,-30,-30,-30,-40,-50,
   -40,-20,  0,  0,  0,  0,-20,-40,
   -30,  0, 12, 16, 16, 12,  0,-30,
   -30,  6, 16, 22, 22, 16,  6,-30,
   -30,  0, 16, 22, 22, 16,  0,-30,
   -30,  6, 12, 16, 16, 12,  6,-30,
   -40,-20,  0,  5,  5,  0,-20,-40,
   -50,-40,-30,-30,-30,-30,-40,-50,
];

#[rustfmt::skip]
const PST_BISHOP: [i16; 64] = [
   -20,-10,-10,-10,-10,-10,-10,-20,
   -10,  0,  0,  0,  0,  0,  0,-10,
   -10,  0,  6, 12, 12,  6,  0,-10,
   -10,  6,  6, 12, 12,  6,  6,-10,
   -10,  0, 12, 12, 12, 12,  0,-10,
   -10, 10, 10, 12, 12, 10, 10,-10,
   -10,  6,  0,  0,  0,  0,  6,-10,
   -20,-10,-10,-10,-10,-10,-10,-20,
];

#[rustfmt::skip]
const PST_ROOK: [i16; 64] = [
     0,  0,  0,  2,  2,  0,  0,  0,
     6, 12, 12, 12, 12, 12, 12,  6,
    -5,  0,  0,  0,  0,  0,  0, -5,
    -5,  0,  0,  0,  0,  0,  0, -5,
    -5,  0,  0,  0,  0,  0,  0, -5,
    -5,  0,  0,  0,  0,  0,  0, -5,
    -5,  0,  0,  0,  0,  0,  0, -5,
     0,  0,  2,  8,  8,  2,  0,  0,
];

#[rustfmt::skip]
const PST_QUEEN: [i16; 64] = [
   -20,-10,-10, -5, -5,-10,-10,-20,
   -10,  0,  0,  0,  0,  0,  0,-10,
   -10,  0,  5,  5,  5,  5,  0,-10,
    -5,  0,  5,  6,  6,  5,  0, -5,
     0,  0,  5,  6,  6,  5,  0, -5,
   -10,  5,  5,  5,  5,  5,  0,-10,
   -10,  0,  5,  0,  0,  0,  0,-10,
   -20,-10,-10, -5, -5,-10,-10,-20,
];

#[rustfmt::skip]
const PST_KING_MG: [i16; 64] = [
   -30,-40,-40,-50,-50,-40,-40,-30,
   -30,-40,-40,-50,-50,-40,-40,-30,
   -30,-40,-40,-50,-50,-40,-40,-30,
   -30,-40,-40,-50,-50,-40,-40,-30,
   -20,-30,-30,-40,-40,-30,-30,-20,
   -10,-20,-20,-20,-20,-20,-20,-10,
    20, 20,  0,  0,  0,  0, 20, 20,
    20, 32, 12,  0,  0, 10, 32, 20,
];

#[rustfmt::skip]
const PST_KING_EG: [i16; 64] = [
   -50,-40,-30,-20,-20,-30,-40,-50,
   -30,-20,-10,  0,  0,-10,-20,-30,
   -30,-10, 20, 30, 30, 20,-10,-30,
   -30,-10, 30, 40, 40, 30,-10,-30,
   -30,-10, 30, 40, 40, 30,-10,-30,
   -30,-10, 20, 30, 30, 20,-10,-30,
   -30,-30,  0,  0,  0,  0,-30,-30,
   -50,-30,-30,-30,-30,-30,-30,-50,
];

#[derive(Clone, Copy)]
pub struct UndoEntry {
    pub mv: Move,
    pub captured: Option<Piece>,
    pub prev_castling: u8,
    pub prev_ep: Option<u8>,
    pub prev_halfmove: u16,
    pub prev_hash: u64,
    pub score_delta: u32,
}

impl UndoEntry {
    const EMPTY: Self = Self {
        mv: Move {
            from: 0,
            to: 0,
            promo_raw: 0,
            flags: 0,
        },
        captured: None,
        prev_castling: 0,
        prev_ep: None,
        prev_halfmove: 0,
        prev_hash: 0,
        score_delta: 0,
    };
}

#[derive(Clone)]
pub struct ChessGame {
    squares: [Option<Piece>; 64],
    king_sq: [u8; 2],
    pub turn: Color,
    pub castling: u8,
    pub ep_square: Option<u8>,
    pub halfmove_clock: u16,
    pub fullmove_number: u16,
    pub hash: u64,
    history: [UndoEntry; MAX_HISTORY],
    hist_len: usize,
    pub score: u32,
    rng: u32,
}

impl ChessGame {
    pub fn new(seed: u32) -> Self {
        let mut g = Self {
            squares: [None; 64],
            king_sq: [4, 60],
            turn: Color::White,
            castling: CASTLE_ALL,
            ep_square: None,
            halfmove_clock: 0,
            fullmove_number: 1,
            hash: 0,
            history: [UndoEntry::EMPTY; MAX_HISTORY],
            hist_len: 0,
            score: 0,
            rng: if seed == 0 { 0xA5A5_1234 } else { seed },
        };
        g.reset_standard();
        g
    }

    pub fn reset_standard(&mut self) {
        self.squares = [None; 64];
        let back = [
            PieceKind::Rook,
            PieceKind::Knight,
            PieceKind::Bishop,
            PieceKind::Queen,
            PieceKind::King,
            PieceKind::Bishop,
            PieceKind::Knight,
            PieceKind::Rook,
        ];
        for (f, &k) in back.iter().enumerate() {
            self.squares[f] = Some(Piece::new(Color::White, k));
            self.squares[8 + f] = Some(Piece::new(Color::White, PieceKind::Pawn));
            self.squares[48 + f] = Some(Piece::new(Color::Black, PieceKind::Pawn));
            self.squares[56 + f] = Some(Piece::new(Color::Black, k));
        }
        self.king_sq = [4, 60];
        self.turn = Color::White;
        self.castling = CASTLE_ALL;
        self.ep_square = None;
        self.halfmove_clock = 0;
        self.fullmove_number = 1;
        self.hist_len = 0;
        self.score = 0;
        self.hash = self.compute_hash();
    }

    /// Завантаження позиції з рядка FEN (зручно для тестів та етюдів).
    pub fn from_fen(fen: &str, seed: u32) -> Option<Self> {
        let mut g = Self {
            squares: [None; 64],
            king_sq: [255, 255],
            turn: Color::White,
            castling: 0,
            ep_square: None,
            halfmove_clock: 0,
            fullmove_number: 1,
            hash: 0,
            history: [UndoEntry::EMPTY; MAX_HISTORY],
            hist_len: 0,
            score: 0,
            rng: if seed == 0 { 0x5EED_CAFE } else { seed },
        };

        let mut parts = fen.split_whitespace();
        let board_part = parts.next()?;
        let mut rank: i32 = 7;
        let mut file: usize = 0;

        for ch in board_part.bytes() {
            if ch == b'/' {
                if file != 8 || rank == 0 {
                    return None;
                }
                rank -= 1;
                file = 0;
            } else if (b'1'..=b'8').contains(&ch) {
                file += (ch - b'0') as usize;
                if file > 8 {
                    return None;
                }
            } else {
                if file >= 8 {
                    return None;
                }
                let color = if ch.is_ascii_uppercase() {
                    Color::White
                } else {
                    Color::Black
                };
                let kind = match ch.to_ascii_lowercase() {
                    b'p' => PieceKind::Pawn,
                    b'n' => PieceKind::Knight,
                    b'b' => PieceKind::Bishop,
                    b'r' => PieceKind::Rook,
                    b'q' => PieceKind::Queen,
                    b'k' => PieceKind::King,
                    _ => return None,
                };
                let sq = sq_of(file, rank as usize);
                g.squares[sq as usize] = Some(Piece::new(color, kind));
                if kind == PieceKind::King {
                    g.king_sq[color.idx()] = sq;
                }
                file += 1;
            }
        }
        if rank != 0 || file != 8 || g.king_sq[0] >= 64 || g.king_sq[1] >= 64 {
            return None;
        }

        if let Some(side) = parts.next() {
            g.turn = match side {
                "w" => Color::White,
                "b" => Color::Black,
                _ => return None,
            };
        }
        if let Some(cst) = parts.next() {
            if cst != "-" {
                for b in cst.bytes() {
                    match b {
                        b'K' => g.castling |= CASTLE_WK,
                        b'Q' => g.castling |= CASTLE_WQ,
                        b'k' => g.castling |= CASTLE_BK,
                        b'q' => g.castling |= CASTLE_BQ,
                        _ => return None,
                    }
                }
            }
        }
        if let Some(ep) = parts.next()
            && ep != "-"
        {
            g.ep_square = Some(parse_sq(ep.as_bytes())?);
        }
        if let Some(hm) = parts.next() {
            g.halfmove_clock = parse_u16(hm).unwrap_or(0);
        }
        if let Some(fm) = parts.next() {
            g.fullmove_number = parse_u16(fm).unwrap_or(1).max(1);
        }

        g.hash = g.compute_hash();
        Some(g)
    }

    #[inline]
    fn next_rand(&mut self) -> u32 {
        let mut x = self.rng;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.rng = x;
        x
    }

    #[inline]
    pub fn piece_at(&self, sq: u8) -> Option<Piece> {
        self.squares[sq as usize]
    }

    #[inline]
    pub fn king_square(&self, color: Color) -> u8 {
        self.king_sq[color.idx()]
    }

    #[inline]
    pub fn last_move(&self) -> Option<Move> {
        if self.hist_len > 0 {
            Some(self.history[self.hist_len - 1].mv)
        } else {
            None
        }
    }

    #[inline]
    pub fn can_undo(&self) -> bool {
        self.hist_len > 0
    }

    /// Різниця матеріалу у пішакових одиницях (додатна = перевага білих).
    pub fn material_balance_pawns(&self) -> i32 {
        let mut diff = 0i32;
        for p in self.squares.iter().flatten() {
            let v = match p.kind {
                PieceKind::Pawn => 1,
                PieceKind::Knight | PieceKind::Bishop => 3,
                PieceKind::Rook => 5,
                PieceKind::Queen => 9,
                PieceKind::King => 0,
            };
            if p.color == Color::White {
                diff += v;
            } else {
                diff -= v;
            }
        }
        diff
    }

    pub fn compute_hash(&self) -> u64 {
        let mut h = 0u64;
        for (sq, cell) in self.squares.iter().enumerate() {
            if let Some(p) = cell {
                h ^= ZOBRIST.pieces[p.color.idx()][p.kind.idx()][sq];
            }
        }
        h ^= ZOBRIST.castling[(self.castling & 0x0F) as usize];
        if let Some(ep) = self.ep_square {
            h ^= ZOBRIST.ep_file[(ep & 7) as usize];
        }
        if self.turn == Color::Black {
            h ^= ZOBRIST.side_black;
        }
        h
    }

    /// Чи атаковане поле `sq` фігурами сторони `by`?
    pub fn is_square_attacked(&self, sq: u8, by: Color) -> bool {
        let r = rank_of(sq) as i32;
        let f = file_of(sq) as i32;

        // 1. Пішаки
        let pawn_src_r = if by == Color::White { r - 1 } else { r + 1 };
        if (0..8).contains(&pawn_src_r) {
            for df in [-1i32, 1] {
                let pf = f + df;
                if (0..8).contains(&pf)
                    && let Some(p) = self.squares[((pawn_src_r as usize) << 3) | (pf as usize)]
                    && p.color == by
                    && p.kind == PieceKind::Pawn
                {
                    return true;
                }
            }
        }

        // 2. Коні
        const KNIGHT_D: [(i32, i32); 8] = [
            (-2, -1),
            (-2, 1),
            (-1, -2),
            (-1, 2),
            (1, -2),
            (1, 2),
            (2, -1),
            (2, 1),
        ];
        for (dr, df) in KNIGHT_D {
            let nr = r + dr;
            let nf = f + df;
            if (0..8).contains(&nr)
                && (0..8).contains(&nf)
                && let Some(p) = self.squares[((nr as usize) << 3) | (nf as usize)]
                && p.color == by
                && p.kind == PieceKind::Knight
            {
                return true;
            }
        }

        // 3. Король
        for dr in -1i32..=1 {
            for df in -1i32..=1 {
                if dr == 0 && df == 0 {
                    continue;
                }
                let nr = r + dr;
                let nf = f + df;
                if (0..8).contains(&nr)
                    && (0..8).contains(&nf)
                    && let Some(p) = self.squares[((nr as usize) << 3) | (nf as usize)]
                    && p.color == by
                    && p.kind == PieceKind::King
                {
                    return true;
                }
            }
        }

        // 4. Тури / ферзі (ортогоналі)
        const ORTHO_D: [(i32, i32); 4] = [(1, 0), (-1, 0), (0, 1), (0, -1)];
        for (dr, df) in ORTHO_D {
            let mut nr = r + dr;
            let mut nf = f + df;
            while (0..8).contains(&nr) && (0..8).contains(&nf) {
                if let Some(p) = self.squares[((nr as usize) << 3) | (nf as usize)] {
                    if p.color == by && matches!(p.kind, PieceKind::Rook | PieceKind::Queen) {
                        return true;
                    }
                    break;
                }
                nr += dr;
                nf += df;
            }
        }

        // 5. Слони / ферзі (діагоналі)
        const DIAG_D: [(i32, i32); 4] = [(1, 1), (1, -1), (-1, 1), (-1, -1)];
        for (dr, df) in DIAG_D {
            let mut nr = r + dr;
            let mut nf = f + df;
            while (0..8).contains(&nr) && (0..8).contains(&nf) {
                if let Some(p) = self.squares[((nr as usize) << 3) | (nf as usize)] {
                    if p.color == by && matches!(p.kind, PieceKind::Bishop | PieceKind::Queen) {
                        return true;
                    }
                    break;
                }
                nr += dr;
                nf += df;
            }
        }

        false
    }

    #[inline]
    pub fn in_check(&self, color: Color) -> bool {
        self.is_square_attacked(self.king_sq[color.idx()], color.opponent())
    }

    fn make_move_raw(&mut self, mv: Move, score_delta: u32) -> UndoEntry {
        let undo = UndoEntry {
            mv,
            captured: None,
            prev_castling: self.castling,
            prev_ep: self.ep_square,
            prev_halfmove: self.halfmove_clock,
            prev_hash: self.hash,
            score_delta,
        };
        let mut undo = undo;

        let us = self.turn;
        let them = us.opponent();
        let moving = self.squares[mv.from as usize].expect("missing moving piece");

        // Знімаємо старий стан кастінгу й en passant із хешу
        self.hash ^= ZOBRIST.castling[(self.castling & 0x0F) as usize];
        if let Some(ep) = self.ep_square {
            self.hash ^= ZOBRIST.ep_file[(ep & 7) as usize];
        }

        // Прибираємо фігуру з початкового поля
        self.squares[mv.from as usize] = None;
        self.hash ^= ZOBRIST.pieces[us.idx()][moving.kind.idx()][mv.from as usize];

        // Обробка взяття (включно з en passant)
        if mv.is_en_passant() {
            let cap_sq = sq_of(file_of(mv.to), rank_of(mv.from));
            let cap = self.squares[cap_sq as usize].take();
            debug_assert!(cap.is_some());
            if let Some(cp) = cap {
                self.hash ^= ZOBRIST.pieces[them.idx()][cp.kind.idx()][cap_sq as usize];
            }
            undo.captured = cap;
        } else if let Some(cp) = self.squares[mv.to as usize].take() {
            self.hash ^= ZOBRIST.pieces[them.idx()][cp.kind.idx()][mv.to as usize];
            undo.captured = Some(cp);
        }

        // Ставимо фігуру (або перетворену фігуру) на цільове поле
        let placed_kind = mv.promo().unwrap_or(moving.kind);
        self.squares[mv.to as usize] = Some(Piece::new(us, placed_kind));
        self.hash ^= ZOBRIST.pieces[us.idx()][placed_kind.idx()][mv.to as usize];

        if moving.kind == PieceKind::King {
            self.king_sq[us.idx()] = mv.to;
        }

        // Переміщення тури при рокіровці
        if (mv.flags & FLAG_CASTLE_KING) != 0 {
            let (r_from, r_to) = if us == Color::White {
                (7usize, 5usize)
            } else {
                (63usize, 61usize)
            };
            let rook = self.squares[r_from].take();
            self.squares[r_to] = rook;
            self.hash ^= ZOBRIST.pieces[us.idx()][PieceKind::Rook.idx()][r_from];
            self.hash ^= ZOBRIST.pieces[us.idx()][PieceKind::Rook.idx()][r_to];
        } else if (mv.flags & FLAG_CASTLE_QUEEN) != 0 {
            let (r_from, r_to) = if us == Color::White {
                (0usize, 3usize)
            } else {
                (56usize, 59usize)
            };
            let rook = self.squares[r_from].take();
            self.squares[r_to] = rook;
            self.hash ^= ZOBRIST.pieces[us.idx()][PieceKind::Rook.idx()][r_from];
            self.hash ^= ZOBRIST.pieces[us.idx()][PieceKind::Rook.idx()][r_to];
        }

        // Оновлення прав рокіровки та поля en passant
        self.castling &= CASTLE_SPOIL[mv.from as usize] & CASTLE_SPOIL[mv.to as usize];
        self.ep_square = if (mv.flags & FLAG_DOUBLE_PAWN) != 0 {
            Some((mv.from + mv.to) / 2)
        } else {
            None
        };

        self.hash ^= ZOBRIST.castling[(self.castling & 0x0F) as usize];
        if let Some(ep) = self.ep_square {
            self.hash ^= ZOBRIST.ep_file[(ep & 7) as usize];
        }

        if moving.kind == PieceKind::Pawn || undo.captured.is_some() {
            self.halfmove_clock = 0;
        } else {
            self.halfmove_clock = self.halfmove_clock.saturating_add(1);
        }
        if us == Color::Black {
            self.fullmove_number = self.fullmove_number.saturating_add(1);
        }

        self.turn = them;
        self.hash ^= ZOBRIST.side_black;

        undo
    }

    fn unmake_move_raw(&mut self, undo: UndoEntry) {
        let us = self.turn.opponent();
        let mv = undo.mv;

        if us == Color::Black {
            self.fullmove_number = self.fullmove_number.saturating_sub(1).max(1);
        }
        self.turn = us;
        self.castling = undo.prev_castling;
        self.ep_square = undo.prev_ep;
        self.halfmove_clock = undo.prev_halfmove;
        self.hash = undo.prev_hash;

        // Повертаємо туру при рокіровці
        if (mv.flags & FLAG_CASTLE_KING) != 0 {
            let (r_from, r_to) = if us == Color::White {
                (7usize, 5usize)
            } else {
                (63usize, 61usize)
            };
            self.squares[r_from] = self.squares[r_to].take();
        } else if (mv.flags & FLAG_CASTLE_QUEEN) != 0 {
            let (r_from, r_to) = if us == Color::White {
                (0usize, 3usize)
            } else {
                (56usize, 59usize)
            };
            self.squares[r_from] = self.squares[r_to].take();
        }

        let placed = self.squares[mv.to as usize].take().expect("missing moved piece");
        let orig_kind = if mv.promo().is_some() {
            PieceKind::Pawn
        } else {
            placed.kind
        };
        self.squares[mv.from as usize] = Some(Piece::new(us, orig_kind));
        if orig_kind == PieceKind::King {
            self.king_sq[us.idx()] = mv.from;
        }

        if mv.is_en_passant() {
            let cap_sq = sq_of(file_of(mv.to), rank_of(mv.from));
            self.squares[cap_sq as usize] = undo.captured;
        } else {
            self.squares[mv.to as usize] = undo.captured;
        }
    }

    fn push_history(&mut self, undo: UndoEntry) {
        if self.hist_len == MAX_HISTORY {
            // Зсуваємо на 2 напівходи, щоб не порушувати парність і зберегти вікно >100 ходів
            self.history.copy_within(2..MAX_HISTORY, 0);
            self.hist_len -= 2;
        }
        self.history[self.hist_len] = undo;
        self.hist_len += 1;
    }

    fn pop_history(&mut self) -> Option<UndoEntry> {
        if self.hist_len == 0 {
            None
        } else {
            self.hist_len -= 1;
            Some(self.history[self.hist_len])
        }
    }

    /// Скільки разів поточна позиція вже зустрічалася у партії (включно з поточною = щонайменше 1).
    pub fn repetition_count(&self) -> usize {
        let mut count = 1usize;
        let max_back = (self.halfmove_clock as usize).min(self.hist_len);
        let mut back = 2usize;
        while back <= max_back {
            let idx = self.hist_len - back;
            if self.history[idx].prev_hash == self.hash {
                count += 1;
            }
            back += 2;
        }
        count
    }

    #[inline]
    fn is_repeated_once(&self) -> bool {
        let max_back = (self.halfmove_clock as usize).min(self.hist_len);
        let mut back = 2usize;
        while back <= max_back {
            if self.history[self.hist_len - back].prev_hash == self.hash {
                return true;
            }
            back += 2;
        }
        false
    }

    /// Недостатність матеріалу за правилами FIDE (`K-K`, `K+N-K`, `K+B-K`, `K+B-K+B` однопольні).
    pub fn is_insufficient_material(&self) -> bool {
        let mut knights = 0u8;
        let mut light_bishops = 0u8;
        let mut dark_bishops = 0u8;

        for (sq, cell) in self.squares.iter().enumerate() {
            let Some(p) = cell else { continue };
            match p.kind {
                PieceKind::Pawn | PieceKind::Rook | PieceKind::Queen => return false,
                PieceKind::King => {}
                PieceKind::Knight => knights += 1,
                PieceKind::Bishop => {
                    let is_light = ((sq & 7) + (sq >> 3)) & 1 != 0;
                    if is_light {
                        light_bishops += 1;
                    } else {
                        dark_bishops += 1;
                    }
                }
            }
        }

        let bishops = light_bishops + dark_bishops;
        if knights == 0 && bishops == 0 {
            return true;
        }
        if knights == 1 && bishops == 0 {
            return true;
        }
        if knights == 0 && (light_bishops == 0 || dark_bishops == 0) {
            // Усі наявні слони стоять на полях одного кольору (включно з 1 слоном або K+B проти K+B)
            return true;
        }
        false
    }

    fn gen_pseudo_moves(&self, out: &mut MoveList, captures_only: bool) {
        let us = self.turn;
        let them = us.opponent();

        for sq_idx in 0..64usize {
            let Some(p) = self.squares[sq_idx] else {
                continue;
            };
            if p.color != us {
                continue;
            }
            let from = sq_idx as u8;
            let r = (sq_idx >> 3) as i32;
            let f = (sq_idx & 7) as i32;

            match p.kind {
                PieceKind::Pawn => {
                    let (dir, start_r, promo_r) = if us == Color::White {
                        (1i32, 1i32, 7i32)
                    } else {
                        (-1i32, 6i32, 0i32)
                    };
                    let nr = r + dir;
                    if (0..8).contains(&nr) {
                        // Ходи вперед
                        let one_sq = sq_of(f as usize, nr as usize);
                        if self.squares[one_sq as usize].is_none() {
                            if nr == promo_r {
                                for pk in [
                                    PieceKind::Queen,
                                    PieceKind::Rook,
                                    PieceKind::Bishop,
                                    PieceKind::Knight,
                                ] {
                                    out.push(Move::new(from, one_sq, Some(pk), FLAG_NONE));
                                }
                            } else if !captures_only {
                                out.push(Move::new(from, one_sq, None, FLAG_NONE));
                                if r == start_r {
                                    let two_sq = sq_of(f as usize, (r + 2 * dir) as usize);
                                    if self.squares[two_sq as usize].is_none() {
                                        out.push(Move::new(
                                            from,
                                            two_sq,
                                            None,
                                            FLAG_DOUBLE_PAWN,
                                        ));
                                    }
                                }
                            }
                        }

                        // Діагональні взяття та en passant
                        for df in [-1i32, 1] {
                            let nf = f + df;
                            if !(0..8).contains(&nf) {
                                continue;
                            }
                            let to = sq_of(nf as usize, nr as usize);
                            if let Some(tp) = self.squares[to as usize] {
                                if tp.color == them {
                                    if nr == promo_r {
                                        for pk in [
                                            PieceKind::Queen,
                                            PieceKind::Rook,
                                            PieceKind::Bishop,
                                            PieceKind::Knight,
                                        ] {
                                            out.push(Move::new(
                                                from,
                                                to,
                                                Some(pk),
                                                FLAG_CAPTURE,
                                            ));
                                        }
                                    } else {
                                        out.push(Move::new(from, to, None, FLAG_CAPTURE));
                                    }
                                }
                            } else if self.ep_square == Some(to) {
                                out.push(Move::new(
                                    from,
                                    to,
                                    None,
                                    FLAG_CAPTURE | FLAG_EN_PASSANT,
                                ));
                            }
                        }
                    }
                }
                PieceKind::Knight => {
                    const D: [(i32, i32); 8] = [
                        (-2, -1),
                        (-2, 1),
                        (-1, -2),
                        (-1, 2),
                        (1, -2),
                        (1, 2),
                        (2, -1),
                        (2, 1),
                    ];
                    for (dr, df) in D {
                        let nr = r + dr;
                        let nf = f + df;
                        if (0..8).contains(&nr) && (0..8).contains(&nf) {
                            let to = sq_of(nf as usize, nr as usize);
                            match self.squares[to as usize] {
                                None if !captures_only => {
                                    out.push(Move::new(from, to, None, FLAG_NONE));
                                }
                                Some(tp) if tp.color == them => {
                                    out.push(Move::new(from, to, None, FLAG_CAPTURE));
                                }
                                _ => {}
                            }
                        }
                    }
                }
                PieceKind::Bishop | PieceKind::Rook | PieceKind::Queen => {
                    const DIAG: [(i32, i32); 4] = [(1, 1), (1, -1), (-1, 1), (-1, -1)];
                    const ORTH: [(i32, i32); 4] = [(1, 0), (-1, 0), (0, 1), (0, -1)];
                    let dirs: &[(i32, i32)] = match p.kind {
                        PieceKind::Bishop => &DIAG,
                        PieceKind::Rook => &ORTH,
                        _ => &[
                            (1, 1),
                            (1, -1),
                            (-1, 1),
                            (-1, -1),
                            (1, 0),
                            (-1, 0),
                            (0, 1),
                            (0, -1),
                        ],
                    };
                    for &(dr, df) in dirs {
                        let mut nr = r + dr;
                        let mut nf = f + df;
                        while (0..8).contains(&nr) && (0..8).contains(&nf) {
                            let to = sq_of(nf as usize, nr as usize);
                            match self.squares[to as usize] {
                                None => {
                                    if !captures_only {
                                        out.push(Move::new(from, to, None, FLAG_NONE));
                                    }
                                }
                                Some(tp) => {
                                    if tp.color == them {
                                        out.push(Move::new(from, to, None, FLAG_CAPTURE));
                                    }
                                    break;
                                }
                            }
                            nr += dr;
                            nf += df;
                        }
                    }
                }
                PieceKind::King => {
                    for dr in -1i32..=1 {
                        for df in -1i32..=1 {
                            if dr == 0 && df == 0 {
                                continue;
                            }
                            let nr = r + dr;
                            let nf = f + df;
                            if (0..8).contains(&nr) && (0..8).contains(&nf) {
                                let to = sq_of(nf as usize, nr as usize);
                                match self.squares[to as usize] {
                                    None if !captures_only => {
                                        out.push(Move::new(from, to, None, FLAG_NONE));
                                    }
                                    Some(tp) if tp.color == them => {
                                        out.push(Move::new(from, to, None, FLAG_CAPTURE));
                                    }
                                    _ => {}
                                }
                            }
                        }
                    }

                    // Рокіровка (лише якщо це не пошук суто форсованих взятих у Quiescence)
                    if !captures_only {
                        if us == Color::White && from == 4 {
                            if (self.castling & CASTLE_WK) != 0
                                && self.squares[5].is_none()
                                && self.squares[6].is_none()
                                && !self.is_square_attacked(4, them)
                                && !self.is_square_attacked(5, them)
                            {
                                out.push(Move::new(4, 6, None, FLAG_CASTLE_KING));
                            }
                            if (self.castling & CASTLE_WQ) != 0
                                && self.squares[1].is_none()
                                && self.squares[2].is_none()
                                && self.squares[3].is_none()
                                && !self.is_square_attacked(4, them)
                                && !self.is_square_attacked(3, them)
                            {
                                out.push(Move::new(4, 2, None, FLAG_CASTLE_QUEEN));
                            }
                        } else if us == Color::Black && from == 60 {
                            if (self.castling & CASTLE_BK) != 0
                                && self.squares[61].is_none()
                                && self.squares[62].is_none()
                                && !self.is_square_attacked(60, them)
                                && !self.is_square_attacked(61, them)
                            {
                                out.push(Move::new(60, 62, None, FLAG_CASTLE_KING));
                            }
                            if (self.castling & CASTLE_BQ) != 0
                                && self.squares[57].is_none()
                                && self.squares[58].is_none()
                                && self.squares[59].is_none()
                                && !self.is_square_attacked(60, them)
                                && !self.is_square_attacked(59, them)
                            {
                                out.push(Move::new(60, 58, None, FLAG_CASTLE_QUEEN));
                            }
                        }
                    }
                }
            }
        }
    }

    /// Повний список легальних ходів у поточній позиції.
    pub fn legal_moves(&mut self) -> MoveList {
        let mut pseudo = MoveList::new();
        self.gen_pseudo_moves(&mut pseudo, false);
        let mut legal = MoveList::new();
        let us = self.turn;
        let them = us.opponent();
        for &mv in pseudo.as_slice() {
            let u = self.make_move_raw(mv, 0);
            let ok = !self.is_square_attacked(self.king_sq[us.idx()], them);
            self.unmake_move_raw(u);
            if ok {
                legal.push(mv);
            }
        }
        legal
    }

    /// Усі легальні ходи вибраної фігури з клітинки `from_sq` (для підсвітки клітинок у UI).
    pub fn legal_moves_from(&mut self, from_sq: u8, out: &mut [Move; 32]) -> usize {
        let list = self.legal_moves();
        let mut n = 0usize;
        for &mv in list.as_slice() {
            if mv.from == from_sq && n < out.len() {
                out[n] = mv;
                n += 1;
            }
        }
        n
    }

    /// Чи вимагає хід `from -> to` вибору фігури перетворення пішака?
    pub fn is_promotion_move(&mut self, from: u8, to: u8) -> bool {
        let list = self.legal_moves();
        list.as_slice()
            .iter()
            .any(|m| m.from == from && m.to == to && m.promo().is_some())
    }

    pub fn find_legal_move(
        &mut self,
        from: u8,
        to: u8,
        promo: Option<PieceKind>,
    ) -> Option<Move> {
        let list = self.legal_moves();
        for &mv in list.as_slice() {
            if mv.from == from && mv.to == to {
                if mv.promo().is_some() {
                    let wanted = promo.unwrap_or(PieceKind::Queen);
                    if mv.promo() == Some(wanted) {
                        return Some(mv);
                    }
                } else if promo.is_none() {
                    return Some(mv);
                }
            }
        }
        None
    }

    /// Розбір текстового UCI-ходу (`"e2e4"`, `"e7e8q"`).
    pub fn parse_uci_move(&mut self, text: &str) -> Option<Move> {
        let b = text.trim().as_bytes();
        if b.len() < 4 {
            return None;
        }
        let from = parse_sq(&b[0..2])?;
        let to = parse_sq(&b[2..4])?;
        let promo = if b.len() >= 5 {
            match b[4].to_ascii_lowercase() {
                b'q' => Some(PieceKind::Queen),
                b'r' => Some(PieceKind::Rook),
                b'b' => Some(PieceKind::Bishop),
                b'n' => Some(PieceKind::Knight),
                _ => None,
            }
        } else {
            None
        };
        self.find_legal_move(from, to, promo)
    }

    /// Поточний статус партії за правилами FIDE.
    pub fn status(&mut self) -> GameStatus {
        let moves = self.legal_moves();
        let chk = self.in_check(self.turn);
        if moves.is_empty() {
            if chk {
                return GameStatus::Checkmate {
                    winner: self.turn.opponent(),
                };
            }
            return GameStatus::Stalemate;
        }
        if self.repetition_count() >= 3 {
            return GameStatus::DrawRepetition;
        }
        if self.halfmove_clock >= 100 {
            return GameStatus::DrawFiftyMove;
        }
        if self.is_insufficient_material() {
            return GameStatus::DrawInsufficientMaterial;
        }
        GameStatus::Ongoing { in_check: chk }
    }

    /// Виконує легальний хід у партії, нараховуючи очки (якщо `award_for` збігається зі стороною ходу).
    pub fn play_move(&mut self, mv: Move, award_for: Option<Color>, mode: PlayMode) -> GameStatus {
        let mover = self.turn;
        let cap_pts = if mv.is_en_passant() {
            PieceKind::Pawn.capture_points()
        } else {
            self.squares[mv.to as usize]
                .map(|p| p.kind.capture_points())
                .unwrap_or(0)
        };
        let promo_pts = if mv.promo().is_some() { 80 } else { 0 };

        let mut u = self.make_move_raw(mv, 0);
        let st = self.status();

        let mut gained = 0u32;
        if award_for == Some(mover) {
            gained += cap_pts + promo_pts;
            match st {
                GameStatus::Ongoing { in_check: true } => gained += 5,
                GameStatus::Checkmate { winner } if winner == mover => {
                    gained += match mode {
                        PlayMode::AiHard => 1000,
                        PlayMode::AiEasy => 500,
                        PlayMode::TwoPlayers => 0,
                    };
                }
                _ => {}
            }
            self.score = self.score.saturating_add(gained);
        }
        u.score_delta = gained;
        self.push_history(u);
        st
    }

    /// Скасовує останній напівхід (або пару ходів у грі проти ШІ).
    pub fn undo(&mut self, mode: PlayMode) -> bool {
        if self.hist_len == 0 {
            return false;
        }
        let steps = if mode.is_vs_ai() && self.turn == Color::White && self.hist_len >= 2 {
            2
        } else {
            1
        };
        for _ in 0..steps {
            if let Some(u) = self.pop_history() {
                self.score = self.score.saturating_sub(u.score_delta);
                self.unmake_move_raw(u);
            }
        }
        true
    }

    /// Статична оцінка позиції в сантипішаках з точки зору `self.turn`.
    pub fn evaluate(&self) -> i32 {
        let mut w_score = 0i32;
        let mut b_score = 0i32;
        let mut phase = 0i32; // 0 = ендшпіль, 24 = повний комплект фігур
        let mut w_bishops = 0u8;
        let mut b_bishops = 0u8;
        let mut w_pawn_files = 0u8;
        let mut b_pawn_files = 0u8;

        for (sq, cell) in self.squares.iter().enumerate() {
            if let Some(p) = cell
                && p.kind == PieceKind::Pawn
            {
                let bit = 1u8 << (sq & 7);
                if p.color == Color::White {
                    w_pawn_files |= bit;
                } else {
                    b_pawn_files |= bit;
                }
            }
        }

        for (sq, cell) in self.squares.iter().enumerate() {
            let Some(p) = cell else { continue };
            let idx = if p.color == Color::White {
                sq ^ 56
            } else {
                sq
            };
            let pst = match p.kind {
                PieceKind::Pawn => PST_PAWN[idx] as i32,
                PieceKind::Knight => {
                    phase += 1;
                    PST_KNIGHT[idx] as i32
                }
                PieceKind::Bishop => {
                    phase += 1;
                    if p.color == Color::White {
                        w_bishops += 1;
                    } else {
                        b_bishops += 1;
                    }
                    PST_BISHOP[idx] as i32
                }
                PieceKind::Rook => {
                    phase += 2;
                    PST_ROOK[idx] as i32
                }
                PieceKind::Queen => {
                    phase += 4;
                    PST_QUEEN[idx] as i32
                }
                PieceKind::King => 0,
            };

            let mut val = if p.kind == PieceKind::King {
                0
            } else {
                p.kind.value_cp() + pst
            };

            // Бонус за прохідного пішака
            if p.kind == PieceKind::Pawn {
                let f = sq & 7;
                let adj_mask = (1u8 << f)
                    | (if f > 0 { 1u8 << (f - 1) } else { 0 })
                    | (if f < 7 { 1u8 << (f + 1) } else { 0 });
                let enemy_files = if p.color == Color::White {
                    b_pawn_files
                } else {
                    w_pawn_files
                };
                if (enemy_files & adj_mask) == 0 {
                    let adv = if p.color == Color::White {
                        sq >> 3
                    } else {
                        7 - (sq >> 3)
                    };
                    const PASSED_BONUS: [i32; 8] = [0, 8, 14, 24, 42, 70, 110, 0];
                    val += PASSED_BONUS[adv];
                }
            }

            if p.color == Color::White {
                w_score += val;
            } else {
                b_score += val;
            }
        }

        if w_bishops >= 2 {
            w_score += 30;
        }
        if b_bishops >= 2 {
            b_score += 30;
        }

        let mg_weight = phase.min(24);
        let eg_weight = 24 - mg_weight;

        let wk_idx = (self.king_sq[0] as usize) ^ 56;
        let bk_idx = self.king_sq[1] as usize;
        w_score += ((PST_KING_MG[wk_idx] as i32) * mg_weight
            + (PST_KING_EG[wk_idx] as i32) * eg_weight)
            / 24;
        b_score += ((PST_KING_MG[bk_idx] as i32) * mg_weight
            + (PST_KING_EG[bk_idx] as i32) * eg_weight)
            / 24;

        // Mop-up оцінка в глибокому ендшпілі: допомагає ставити мат самотньому королю
        if eg_weight >= 16 {
            let mat_diff = w_score - b_score;
            if mat_diff.abs() >= 300 {
                let wk_r = rank_of(self.king_sq[0]) as i32;
                let wk_f = file_of(self.king_sq[0]) as i32;
                let bk_r = rank_of(self.king_sq[1]) as i32;
                let bk_f = file_of(self.king_sq[1]) as i32;
                let dist_kings = (wk_r - bk_r).abs() + (wk_f - bk_f).abs();
                if mat_diff > 0 {
                    let center_d = (bk_r * 2 - 7).abs() + (bk_f * 2 - 7).abs();
                    w_score += center_d * 6 + (14 - dist_kings) * 4;
                } else {
                    let center_d = (wk_r * 2 - 7).abs() + (wk_f * 2 - 7).abs();
                    b_score += center_d * 6 + (14 - dist_kings) * 4;
                }
            }
        }

        let diff = w_score - b_score;
        if self.turn == Color::White {
            diff
        } else {
            -diff
        }
    }

    #[inline]
    fn move_order_score(&self, mv: Move, tt_best: Option<Move>, killers: [Move; 2]) -> i32 {
        if tt_best == Some(mv) {
            return 100_000;
        }
        let mut s = 0i32;
        if let Some(pk) = mv.promo() {
            s += 20_000 + pk.value_cp();
        }
        if mv.is_capture() {
            let victim = if mv.is_en_passant() {
                100
            } else {
                self.squares[mv.to as usize]
                    .map(|p| p.kind.value_cp())
                    .unwrap_or(100)
            };
            let attacker = self.squares[mv.from as usize]
                .map(|p| p.kind.value_cp())
                .unwrap_or(100);
            s += 10_000 + victim * 10 - attacker;
        } else if mv == killers[0] {
            s += 5_000;
        } else if mv == killers[1] {
            s += 4_000;
        } else if mv.is_castle() {
            s += 1_200;
        }
        s
    }

    fn pick_next_move(moves: &mut [Move], scores: &mut [i32], idx: usize) -> Move {
        let mut best_i = idx;
        let mut best_s = scores[idx];
        for j in (idx + 1)..moves.len() {
            if scores[j] > best_s {
                best_s = scores[j];
                best_i = j;
            }
        }
        if best_i != idx {
            moves.swap(idx, best_i);
            scores.swap(idx, best_i);
        }
        moves[idx]
    }

    /// Вибирає найкращий хід для поточної сторони (`Easy`, `Demo`, `Hard`).
    pub fn choose_ai_move(&mut self, level: AiLevel) -> Option<Move> {
        let mut root_moves = self.legal_moves();
        if root_moves.is_empty() {
            return None;
        }
        if root_moves.len() == 1 {
            return Some(root_moves.as_slice()[0]);
        }

        let (max_depth, node_budget, jitter_cp): (u8, u32, i32) = match level {
            AiLevel::Easy => (2, 2_200, 22),
            AiLevel::Demo => (3, 9_500, 8),
            AiLevel::Hard => (4, 22_000, if self.fullmove_number <= 3 { 4 } else { 0 }),
        };

        let mut ctx = SearchCtx {
            nodes: 0,
            max_nodes: node_budget,
            aborted: false,
            killers: [[Move::default(); 2]; MAX_PLY],
        };

        let mut root_scores = [0i32; MAX_MOVES];
        for (i, &mv) in root_moves.as_slice().iter().enumerate() {
            root_scores[i] = self.move_order_score(mv, None, [Move::default(); 2]);
        }

        let mut best_move = root_moves.as_slice()[0];

        for depth in 1..=max_depth {
            let mut best_val = -30_000i32;
            let beta = 30_000i32;
            let mut iter_best = best_move;
            let mut completed_any = false;

            let n = root_moves.len();
            for i in 0..n {
                let mv = Self::pick_next_move(
                    root_moves.as_mut_slice(),
                    &mut root_scores[..n],
                    i,
                );
                let search_alpha = if completed_any {
                    (best_val - jitter_cp * 2 - 1).max(-30_000)
                } else {
                    -30_000
                };

                let u = self.make_move_raw(mv, 0);
                self.push_history(u);

                let raw_val = -self.alphabeta(&mut ctx, depth - 1, 1, -beta, -search_alpha);

                let popped = self.pop_history().unwrap();
                self.unmake_move_raw(popped);

                if ctx.aborted && depth > 1 {
                    break;
                }

                // Якщо хід відсікся за верхньою межею `search_alpha`, не додаємо випадковий шум!
                let mut val = raw_val;
                if jitter_cp > 0 && raw_val > search_alpha && raw_val.abs() < 25_000 {
                    let r = (self.next_rand() % ((jitter_cp as u32) * 2 + 1)) as i32 - jitter_cp;
                    val += r;
                }

                root_scores[i] = 50_000 + raw_val;
                if !completed_any || val > best_val {
                    best_val = val;
                    iter_best = mv;
                    completed_any = true;
                }
            }

            if completed_any {
                best_move = iter_best;
            }
            if ctx.aborted || best_val >= 28_000 {
                break;
            }
        }

        Some(best_move)
    }

    fn alphabeta(
        &mut self,
        ctx: &mut SearchCtx,
        mut depth: u8,
        ply: usize,
        mut alpha: i32,
        beta: i32,
    ) -> i32 {
        ctx.nodes += 1;
        if (ctx.nodes & 63) == 0 && ctx.nodes >= ctx.max_nodes {
            ctx.aborted = true;
            return self.evaluate();
        }

        if ply > 0 {
            if self.halfmove_clock >= 100 || self.is_repeated_once() || self.is_insufficient_material()
            {
                return 0;
            }
        }
        if ply >= MAX_PLY - 1 {
            return self.evaluate();
        }

        let in_chk = self.in_check(self.turn);
        if in_chk && ply < MAX_PLY - 2 && depth < 5 {
            depth += 1; // Check extension
        }

        if depth == 0 {
            return self.quiescence(ctx, ply, alpha, beta);
        }

        let mut pseudo = MoveList::new();
        self.gen_pseudo_moves(&mut pseudo, false);
        let n = pseudo.len();
        let mut scores = [0i32; MAX_MOVES];
        let killers = ctx.killers[ply];
        for (i, &mv) in pseudo.as_slice().iter().enumerate() {
            scores[i] = self.move_order_score(mv, None, killers);
        }

        let us = self.turn;
        let them = us.opponent();
        let mut legal_count = 0usize;

        for i in 0..n {
            let mv = Self::pick_next_move(pseudo.as_mut_slice(), &mut scores[..n], i);
            let u = self.make_move_raw(mv, 0);
            if self.is_square_attacked(self.king_sq[us.idx()], them) {
                self.unmake_move_raw(u);
                continue;
            }
            legal_count += 1;
            self.push_history(u);

            let score = -self.alphabeta(ctx, depth - 1, ply + 1, -beta, -alpha);

            let popped = self.pop_history().unwrap();
            self.unmake_move_raw(popped);

            if ctx.aborted {
                return alpha;
            }

            if score >= beta {
                if !mv.is_capture() && mv.promo().is_none() && ctx.killers[ply][0] != mv {
                    ctx.killers[ply][1] = ctx.killers[ply][0];
                    ctx.killers[ply][0] = mv;
                }
                return beta;
            }
            if score > alpha {
                alpha = score;
            }
        }

        if legal_count == 0 {
            if in_chk {
                return -29_000 + (ply as i32);
            }
            return 0;
        }

        alpha
    }

    fn quiescence(&mut self, ctx: &mut SearchCtx, ply: usize, mut alpha: i32, beta: i32) -> i32 {
        ctx.nodes += 1;
        if (ctx.nodes & 63) == 0 && ctx.nodes >= ctx.max_nodes {
            ctx.aborted = true;
            return self.evaluate();
        }

        let stand_pat = self.evaluate();
        if stand_pat >= beta {
            return beta;
        }
        if stand_pat > alpha {
            alpha = stand_pat;
        }
        if ply >= MAX_PLY - 1 {
            return alpha;
        }

        let mut pseudo = MoveList::new();
        self.gen_pseudo_moves(&mut pseudo, true);

        let mut caps = [Move::default(); MAX_CAPS];
        let mut scores = [0i32; MAX_CAPS];
        let mut n = 0usize;
        for &mv in pseudo.as_slice() {
            if n < MAX_CAPS {
                caps[n] = mv;
                scores[n] = self.move_order_score(mv, None, [Move::default(); 2]);
                n += 1;
            }
        }

        let us = self.turn;
        let them = us.opponent();

        for i in 0..n {
            let mv = Self::pick_next_move(&mut caps[..n], &mut scores[..n], i);
            let u = self.make_move_raw(mv, 0);
            if self.is_square_attacked(self.king_sq[us.idx()], them) {
                self.unmake_move_raw(u);
                continue;
            }
            let score = -self.quiescence(ctx, ply + 1, -beta, -alpha);
            self.unmake_move_raw(u);

            if ctx.aborted {
                return alpha;
            }
            if score >= beta {
                return beta;
            }
            if score > alpha {
                alpha = score;
            }
        }

        alpha
    }

    /// Класичний `perft(depth)` для математичної перевірки генератора ходів.
    pub fn perft(&mut self, depth: u8) -> u64 {
        if depth == 0 {
            return 1;
        }
        let mut pseudo = MoveList::new();
        self.gen_pseudo_moves(&mut pseudo, false);
        let us = self.turn;
        let them = us.opponent();
        let mut nodes = 0u64;

        for &mv in pseudo.as_slice() {
            let u = self.make_move_raw(mv, 0);
            if !self.is_square_attacked(self.king_sq[us.idx()], them) {
                nodes += if depth == 1 {
                    1
                } else {
                    self.perft(depth - 1)
                };
            }
            self.unmake_move_raw(u);
        }
        nodes
    }
}

struct SearchCtx {
    nodes: u32,
    max_nodes: u32,
    aborted: bool,
    killers: [[Move; 2]; MAX_PLY],
}

fn parse_u16(s: &str) -> Option<u16> {
    let mut v: u16 = 0;
    if s.is_empty() {
        return None;
    }
    for b in s.bytes() {
        if !b.is_ascii_digit() {
            return None;
        }
        v = v.checked_mul(10)?.checked_add((b - b'0') as u16)?;
    }
    Some(v)
}

/// Швидка самоперевірка рушія при старті на платі.
pub fn selftest() -> (usize, usize) {
    let mut run = 0usize;
    let mut failed = 0usize;

    let mut check = |ok: bool| {
        run += 1;
        if !ok {
            failed += 1;
        }
    };

    let mut g = ChessGame::new(1);
    check(g.perft(1) == 20);
    check(g.perft(2) == 400);

    // Дитячий мат: 1. e4 e5 2. Qh5 Nc6 3. Bc4 Nf6 4. Qxf7#
    for uci in ["e2e4", "e7e5", "d1h5", "b8c6", "f1c4", "g8f6", "h5f7"] {
        if let Some(mv) = g.parse_uci_move(uci) {
            g.play_move(mv, None, PlayMode::TwoPlayers);
        } else {
            check(false);
        }
    }
    check(
        g.status()
            == GameStatus::Checkmate {
                winner: Color::White,
            },
    );

    // Перевірка мату в 1 хід для ШІ
    if let Some(mut m1) = ChessGame::from_fen("6k1/5ppp/8/8/8/8/5PPP/4R1K1 w - - 0 1", 7) {
        let best = m1.choose_ai_move(AiLevel::Hard);
        let mut buf = [0u8; 6];
        let s = best.map(|m| m.format_short(&mut buf)).unwrap_or("");
        check(s == "e1e8");
    } else {
        check(false);
    }

    (run, failed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selftest_passes() {
        let (run, failed) = selftest();
        assert_eq!(failed, 0, "selftest failed ({}/{})", failed, run);
        assert!(run >= 4);
    }

    #[test]
    fn startpos_perft_depth_3() {
        let mut g = ChessGame::new(42);
        assert_eq!(g.perft(1), 20);
        assert_eq!(g.perft(2), 400);
        assert_eq!(g.perft(3), 8_902);
        assert_eq!(g.hash, g.compute_hash());
    }

    #[test]
    fn kiwipete_perft_castling_ep_promotions() {
        // Класична тестова позиція Kiwipete (рокіровки, взяття на проході, перетворення, зв'язки)
        let mut g = ChessGame::from_fen(
            "r3k2r/p1ppqpb1/bn2pnp1/3PN3/1p2P3/2N2Q1p/PPPBBPPP/R3K2R w KQkq - 0 1",
            1,
        )
        .unwrap();
        assert_eq!(g.perft(1), 48);
        assert_eq!(g.perft(2), 2_039);
        assert_eq!(g.hash, g.compute_hash());
    }

    #[test]
    fn en_passant_horizontal_pin_disallowed() {
        // Позиція CPW Pos 3: горизонтальна зв'язка пішака турою — фальшиве en passant заборонене!
        let mut g = ChessGame::from_fen("8/2p5/3p4/KP5r/1R3p1k/8/4P1P1/8 w - - 0 1", 1).unwrap();
        assert_eq!(g.perft(1), 14);
        assert_eq!(g.perft(2), 191);
        assert_eq!(g.perft(3), 2_812);
    }

    #[test]
    fn castling_and_rook_capture_rights() {
        let mut g =
            ChessGame::from_fen("r3k2r/8/8/8/8/8/8/R3K2R w KQkq - 0 1", 1).unwrap();
        let mv_oo = g.parse_uci_move("e1g1").expect("O-O must be legal");
        assert!(mv_oo.is_castle());
        g.play_move(mv_oo, None, PlayMode::TwoPlayers);
        assert_eq!(g.piece_at(sq_of(6, 0)), Some(Piece::new(Color::White, PieceKind::King)));
        assert_eq!(g.piece_at(sq_of(5, 0)), Some(Piece::new(Color::White, PieceKind::Rook)));
        assert_eq!(g.castling & (CASTLE_WK | CASTLE_WQ), 0);
        assert_eq!(g.hash, g.compute_hash());

        // Якщо біла тура бере чорну туру на h8 — чорні втрачають право на коротку рокіровку!
        let mut g2 =
            ChessGame::from_fen("r3k2r/8/8/8/8/8/8/4K2R w Kkq - 0 1", 1).unwrap();
        let cap = g2.parse_uci_move("h1h8").unwrap();
        g2.play_move(cap, None, PlayMode::TwoPlayers);
        assert_eq!(g2.castling & CASTLE_BK, 0);
        assert_ne!(g2.castling & CASTLE_BQ, 0);
    }

    #[test]
    fn underpromotion_and_undo_restores_exact_state() {
        let mut g =
            ChessGame::from_fen("8/5P1k/8/8/8/8/P7/4K3 w - - 0 1", 9).unwrap();
        let orig_hash = g.hash;
        assert!(g.is_promotion_move(sq_of(5, 6), sq_of(5, 7)));

        // Перетворення у коня з шахом!
        let mv_n = g.parse_uci_move("f7f8n").expect("underpromotion to knight");
        let st = g.play_move(mv_n, Some(Color::White), PlayMode::AiHard);
        assert_eq!(st, GameStatus::Ongoing { in_check: true });
        assert_eq!(g.piece_at(sq_of(5, 7)), Some(Piece::new(Color::White, PieceKind::Knight)));
        assert_eq!(g.hash, g.compute_hash());

        assert!(g.undo(PlayMode::TwoPlayers));
        assert_eq!(g.hash, orig_hash);
        assert_eq!(g.hash, g.compute_hash());
        assert_eq!(g.piece_at(sq_of(5, 6)), Some(Piece::new(Color::White, PieceKind::Pawn)));
        assert_eq!(g.piece_at(sq_of(5, 7)), None);
        assert_eq!(g.score, 0);
    }

    #[test]
    fn stalemate_repetition_fifty_and_insufficient() {
        // Пат
        let mut st = ChessGame::from_fen("7k/5Q2/6K1/8/8/8/8/8 b - - 0 1", 1).unwrap();
        assert_eq!(st.status(), GameStatus::Stalemate);

        // Недостатність матеріалу: K+B проти K
        let mut ins = ChessGame::from_fen("7k/8/6K1/4B3/8/8/8/8 w - - 0 1", 1).unwrap();
        assert_eq!(ins.status(), GameStatus::DrawInsufficientMaterial);

        // Правило 50 ходів
        let mut fifty = ChessGame::from_fen("7k/8/6K1/8/8/8/4R3/8 w - - 100 80", 1).unwrap();
        assert_eq!(fifty.status(), GameStatus::DrawFiftyMove);

        // Триразове повторення позиції
        let mut rep = ChessGame::new(1);
        for _ in 0..2 {
            for uci in ["g1f3", "g8f6", "f3g1", "f6g8"] {
                let mv = rep.parse_uci_move(uci).unwrap();
                rep.play_move(mv, None, PlayMode::TwoPlayers);
                assert_eq!(rep.hash, rep.compute_hash());
            }
        }
        assert_eq!(rep.repetition_count(), 3);
        assert_eq!(rep.status(), GameStatus::DrawRepetition);
    }

    #[test]
    fn ai_finds_mate_in_two_and_captures_hanging_queen() {
        // Класичний мат у 2 ходи з жертвою ферзя: 1. Qe8+! Rxe8 2. Rxe8#
        let mut g =
            ChessGame::from_fen("3r2k1/5ppp/8/8/8/8/4QPPP/4R1K1 w - - 0 1", 3).unwrap();
        let mv = g.choose_ai_move(AiLevel::Hard).unwrap();
        let mut b1 = [0u8; 6];
        assert_eq!(mv.format_short(&mut b1), "e2e8");
        g.play_move(mv, None, PlayMode::TwoPlayers);

        let reply = g.choose_ai_move(AiLevel::Hard).unwrap();
        g.play_move(reply, None, PlayMode::TwoPlayers);

        let finish = g.choose_ai_move(AiLevel::Hard).unwrap();
        let st = g.play_move(finish, None, PlayMode::TwoPlayers);
        assert_eq!(
            st,
            GameStatus::Checkmate {
                winner: Color::White
            }
        );

        // Незахищений чорний ферзь на d5
        let mut g2 =
            ChessGame::from_fen("rnb1kbnr/pppppppp/8/3q4/4P3/8/PPPP1PPP/RNBQKBNR w KQkq - 0 2", 5)
                .unwrap();
        let best = g2.choose_ai_move(AiLevel::Hard).unwrap();
        let mut buf = [0u8; 6];
        let s = best.format_short(&mut buf);
        assert_eq!(s, "e4d5", "fullmove={} eval_before={}", g2.fullmove_number, g2.evaluate());
    }
}
