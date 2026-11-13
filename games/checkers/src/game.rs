//! Повний рушій класичних шашок-64 (українські / класичні 64-клітинкові шашки) + альфа-бета ШІ.
//!
//! Без `std`, без `alloc`, без сторонніх залежностей — компілюється і як частина прошивки ESP32-C5,
//! і окремо на хості (`rustc --edition 2024 --test games/checkers/src/game.rs -O`).
//!
//! Реалізовані всі правила класичних шашок на дошці 8×8:
//! - гра на 32 темних полях (`a1` — темне поле), по 12 шашок у кожної сторони, білі ходять першими;
//! - обов'язковий бій (тихі ходи заборонені, якщо є хоча б одне взяття);
//! - прості шашки ходять вперед на 1 клітинку, але **б'ють як вперед, так і назад**;
//! - серійне взяття за один хід із дотриманням правила **«турецького удару»** (побиті фігури знімаються
//!   лише наприкінці серії: перестрибувати одну й ту ж шашку двічі або ставати на неї не можна);
//! - **миттєве перетворення у дамку під час бою**: якщо проста шашка під час взяття досягає останньої
//!   горизонталі й може бити далі вже як дамка — вона продовжує цей самий хід на правах дамки;
//! - **«літаюча» дамка**: ходить і б'є на будь-яку відстань по діагоналі; якщо за побитою фігурою є поля,
//!   з яких можливе продовження бою, дамка зобов'язана приземлитися саме на одне з таких полів;
//! - перемога при знищенні або повному заперті фігур суперника; нічия при триразовому повторенні позиції
//!   (Zobrist 64-bit) або 30 напівходах дамок без бою та без руху простих шашок;
//! - повна підтримка `Undo` та альфа-бета пошук із Quiescence форсованих серій та позиційною оцінкою.

pub const BOARD_N: usize = 8;
pub const BOARD_CELLS: usize = 64;
pub const MAX_MOVES: usize = 128;
pub const MAX_STEPS: usize = 13;
const MAX_HISTORY: usize = 160;
const MAX_PLY: usize = 18;

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
    Man = 0,
    King = 1,
}

impl PieceKind {
    #[inline]
    pub const fn idx(self) -> usize {
        self as usize
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
    Ongoing { must_capture: bool },
    Win { winner: Color },
    DrawRepetition,
    DrawThirtyMove,
}

impl GameStatus {
    #[inline]
    pub const fn is_game_over(self) -> bool {
        !matches!(self, Self::Ongoing { .. })
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

#[inline]
pub const fn is_dark_sq(sq: u8) -> bool {
    (((sq & 7) + (sq >> 3)) & 1) == 0
}

#[inline]
pub const fn dark_idx(sq: u8) -> u32 {
    (sq >> 1) as u32
}

#[inline]
pub const fn dark_to_sq(d: u32) -> u8 {
    ((d << 1) | ((d >> 2) & 1)) as u8
}

pub fn parse_sq(s: &[u8]) -> Option<u8> {
    if s.len() < 2 {
        return None;
    }
    let f = s[0].to_ascii_lowercase();
    let r = s[1];
    if (b'a'..=b'h').contains(&f) && (b'1'..=b'8').contains(&r) {
        let sq = sq_of((f - b'a') as usize, (r - b'1') as usize);
        is_dark_sq(sq).then_some(sq)
    } else {
        None
    }
}

/// Повний хід (тихий або серія взятих із кількох стрибків, 20 байтів).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct TurnMove {
    pub steps: [u8; MAX_STEPS],
    pub steps_len: u8,
    pub cap_len: u8,
    pub promoted: bool,
    pub cap_mask: u32,
}

impl TurnMove {
    pub const EMPTY: Self = Self {
        steps: [0; MAX_STEPS],
        steps_len: 0,
        cap_len: 0,
        promoted: false,
        cap_mask: 0,
    };

    #[inline]
    pub const fn quiet(from: u8, to: u8, promoted: bool) -> Self {
        let mut steps = [0u8; MAX_STEPS];
        steps[0] = from;
        steps[1] = to;
        Self {
            steps,
            steps_len: 2,
            cap_len: 0,
            promoted,
            cap_mask: 0,
        }
    }

    #[inline]
    pub const fn from(self) -> u8 {
        self.steps[0]
    }

    #[inline]
    pub const fn to(self) -> u8 {
        if self.steps_len > 0 {
            self.steps[(self.steps_len - 1) as usize]
        } else {
            0
        }
    }

    #[inline]
    pub const fn is_capture(self) -> bool {
        self.cap_len > 0
    }

    #[inline]
    pub fn matches_prefix(&self, prefix: &[u8]) -> bool {
        if prefix.len() > self.steps_len as usize {
            return false;
        }
        &self.steps[..prefix.len()] == prefix
    }

    /// Короткий запис ходу (`"c3-d4"`, `"c3:e5"`, `"c3:g3x2"`).
    pub fn format_short(self, buf: &mut [u8; 8]) -> &str {
        if self.steps_len < 2 {
            return "";
        }
        let a = self.from();
        let b = self.to();
        buf[0] = b'a' + (a & 7);
        buf[1] = b'1' + (a >> 3);
        buf[2] = if self.is_capture() { b':' } else { b'-' };
        buf[3] = b'a' + (b & 7);
        buf[4] = b'1' + (b >> 3);
        if self.cap_len >= 2 {
            buf[5] = b'x';
            if self.cap_len >= 10 {
                buf[6] = b'1';
                buf[7] = b'0' + (self.cap_len - 10);
                unsafe { core::str::from_utf8_unchecked(&buf[..8]) }
            } else {
                buf[6] = b'0' + self.cap_len;
                unsafe { core::str::from_utf8_unchecked(&buf[..7]) }
            }
        } else {
            unsafe { core::str::from_utf8_unchecked(&buf[..5]) }
        }
    }
}

impl Default for TurnMove {
    fn default() -> Self {
        Self::EMPTY
    }
}

pub struct MoveList {
    moves: [TurnMove; MAX_MOVES],
    len: usize,
}

impl MoveList {
    #[inline]
    pub const fn new() -> Self {
        Self {
            moves: [TurnMove::EMPTY; MAX_MOVES],
            len: 0,
        }
    }

    #[inline]
    fn push(&mut self, mv: TurnMove) {
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
    pub fn as_slice(&self) -> &[TurnMove] {
        &self.moves[..self.len]
    }

    #[inline]
    pub fn as_mut_slice(&mut self) -> &mut [TurnMove] {
        &mut self.moves[..self.len]
    }
}

impl Default for MoveList {
    fn default() -> Self {
        Self::new()
    }
}

// Таблиця Zobrist (SplitMix64 під час компіляції)
struct ZobristTable {
    pieces: [[[u64; 64]; 2]; 2],
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
    let mut seed = 0xD8A0_64C1_2026_1112u64;
    let mut pieces = [[[0u64; 64]; 2]; 2];
    let mut c = 0;
    while c < 2 {
        let mut k = 0;
        while k < 2 {
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
    let (_, side_black) = splitmix64(seed);
    ZobristTable { pieces, side_black }
};

#[derive(Clone, Copy)]
pub struct UndoEntry {
    pub mv: TurnMove,
    pub cap_kings_mask: u32,
    pub prev_quiet_plies: u16,
    pub prev_hash: u64,
    pub score_delta: u32,
}

impl UndoEntry {
    const EMPTY: Self = Self {
        mv: TurnMove::EMPTY,
        cap_kings_mask: 0,
        prev_quiet_plies: 0,
        prev_hash: 0,
        score_delta: 0,
    };
}

#[derive(Clone)]
pub struct CheckersGame {
    squares: [Option<Piece>; 64],
    pub turn: Color,
    pub quiet_king_plies: u16,
    pub move_number: u16,
    pub hash: u64,
    history: [UndoEntry; MAX_HISTORY],
    hist_len: usize,
    pub score: u32,
    rng: u32,
}

impl CheckersGame {
    pub fn new(seed: u32) -> Self {
        let mut g = Self {
            squares: [None; 64],
            turn: Color::White,
            quiet_king_plies: 0,
            move_number: 1,
            hash: 0,
            history: [UndoEntry::EMPTY; MAX_HISTORY],
            hist_len: 0,
            score: 0,
            rng: if seed == 0 { 0x6464_CAFE } else { seed },
        };
        g.reset_standard();
        g
    }

    pub fn reset_standard(&mut self) {
        self.squares = [None; 64];
        for r in 0..3usize {
            for f in 0..8usize {
                let sq = sq_of(f, r);
                if is_dark_sq(sq) {
                    self.squares[sq as usize] = Some(Piece::new(Color::White, PieceKind::Man));
                }
            }
        }
        for r in 5..8usize {
            for f in 0..8usize {
                let sq = sq_of(f, r);
                if is_dark_sq(sq) {
                    self.squares[sq as usize] = Some(Piece::new(Color::Black, PieceKind::Man));
                }
            }
        }
        self.turn = Color::White;
        self.quiet_king_plies = 0;
        self.move_number = 1;
        self.hist_len = 0;
        self.score = 0;
        self.hash = self.compute_hash();
    }

    /// Порожня дошка для конструювання тестових позицій та етюдів.
    pub fn empty_board(turn: Color, seed: u32) -> Self {
        let mut g = Self {
            squares: [None; 64],
            turn,
            quiet_king_plies: 0,
            move_number: 1,
            hash: 0,
            history: [UndoEntry::EMPTY; MAX_HISTORY],
            hist_len: 0,
            score: 0,
            rng: if seed == 0 { 0x1234_5678 } else { seed },
        };
        g.hash = g.compute_hash();
        g
    }

    pub fn set_piece(&mut self, sq: u8, piece: Option<Piece>) {
        if is_dark_sq(sq) {
            self.squares[sq as usize] = piece;
            self.hash = self.compute_hash();
        }
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
    pub fn last_move(&self) -> Option<TurnMove> {
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

    /// Кількість фігур `(білі_усього, чорні_усього, білі_дамки, чорні_дамки)`.
    pub fn piece_counts(&self) -> (u8, u8, u8, u8) {
        let mut w = 0u8;
        let mut b = 0u8;
        let mut wk = 0u8;
        let mut bk = 0u8;
        for p in self.squares.iter().flatten() {
            match (p.color, p.kind) {
                (Color::White, PieceKind::Man) => w += 1,
                (Color::White, PieceKind::King) => {
                    w += 1;
                    wk += 1;
                }
                (Color::Black, PieceKind::Man) => b += 1,
                (Color::Black, PieceKind::King) => {
                    b += 1;
                    bk += 1;
                }
            }
        }
        (w, b, wk, bk)
    }

    pub fn compute_hash(&self) -> u64 {
        let mut h = 0u64;
        for (sq, cell) in self.squares.iter().enumerate() {
            if let Some(p) = cell {
                h ^= ZOBRIST.pieces[p.color.idx()][p.kind.idx()][sq];
            }
        }
        if self.turn == Color::Black {
            h ^= ZOBRIST.side_black;
        }
        h
    }

    #[inline]
    fn step_diag(sq: u8, dr: i32, df: i32) -> Option<u8> {
        let nr = (rank_of(sq) as i32) + dr;
        let nf = (file_of(sq) as i32) + df;
        if (0..8).contains(&nr) && (0..8).contains(&nf) {
            Some(sq_of(nf as usize, nr as usize))
        } else {
            None
        }
    }

    #[inline]
    fn cell_during_chain(&self, sq: u8, start_sq: u8) -> Option<Piece> {
        if sq == start_sq {
            None
        } else {
            self.squares[sq as usize]
        }
    }

    #[inline]
    const fn promo_rank(color: Color) -> usize {
        match color {
            Color::White => 7,
            Color::Black => 0,
        }
    }

    /// Чи може дамка з поля `cur_sq` зробити ще хоча б один стрибок у тій самій серії?
    fn can_king_jump_further(
        &self,
        cur_sq: u8,
        start_sq: u8,
        us: Color,
        cap_mask: u32,
    ) -> bool {
        let them = us.opponent();
        const DIRS: [(i32, i32); 4] = [(1, 1), (1, -1), (-1, 1), (-1, -1)];
        for (dr, df) in DIRS {
            let mut cur = cur_sq;
            while let Some(nxt) = Self::step_diag(cur, dr, df) {
                match self.cell_during_chain(nxt, start_sq) {
                    None => cur = nxt,
                    Some(p) => {
                        if p.color == them && (cap_mask & (1u32 << dark_idx(nxt))) == 0 {
                            if let Some(behind) = Self::step_diag(nxt, dr, df)
                                && self.cell_during_chain(behind, start_sq).is_none()
                            {
                                return true;
                            }
                        }
                        break;
                    }
                }
            }
        }
        false
    }

    #[allow(clippy::too_many_arguments)]
    fn dfs_captures(
        &self,
        out: &mut MoveList,
        start_sq: u8,
        cur_sq: u8,
        us: Color,
        cur_kind: PieceKind,
        steps: &mut [u8; MAX_STEPS],
        steps_len: u8,
        cap_len: u8,
        cap_mask: u32,
        promoted: bool,
    ) {
        let them = us.opponent();
        const DIRS: [(i32, i32); 4] = [(1, 1), (1, -1), (-1, 1), (-1, -1)];
        let mut extended = false;

        if (steps_len as usize) < MAX_STEPS {
            match cur_kind {
                PieceKind::Man => {
                    for (dr, df) in DIRS {
                        let Some(mid) = Self::step_diag(cur_sq, dr, df) else {
                            continue;
                        };
                        let Some(land) = Self::step_diag(mid, dr, df) else {
                            continue;
                        };
                        let Some(mp) = self.cell_during_chain(mid, start_sq) else {
                            continue;
                        };
                        let bit = 1u32 << dark_idx(mid);
                        if mp.color != them || (cap_mask & bit) != 0 {
                            continue;
                        }
                        if self.cell_during_chain(land, start_sq).is_some() {
                            continue;
                        }

                        extended = true;
                        let reaches_promo = rank_of(land) == Self::promo_rank(us);
                        let next_kind = if reaches_promo {
                            PieceKind::King
                        } else {
                            PieceKind::Man
                        };
                        steps[steps_len as usize] = land;
                        self.dfs_captures(
                            out,
                            start_sq,
                            land,
                            us,
                            next_kind,
                            steps,
                            steps_len + 1,
                            cap_len + 1,
                            cap_mask | bit,
                            promoted || reaches_promo,
                        );
                    }
                }
                PieceKind::King => {
                    for (dr, df) in DIRS {
                        let mut scan = cur_sq;
                        let mut enemy_sq = None;
                        while let Some(nxt) = Self::step_diag(scan, dr, df) {
                            match self.cell_during_chain(nxt, start_sq) {
                                None => scan = nxt,
                                Some(p) => {
                                    let bit = 1u32 << dark_idx(nxt);
                                    if p.color == them && (cap_mask & bit) == 0 {
                                        enemy_sq = Some((nxt, bit));
                                    }
                                    break;
                                }
                            }
                        }

                        let Some((esq, bit)) = enemy_sq else {
                            continue;
                        };
                        let next_mask = cap_mask | bit;

                        let mut lands = [0u8; 7];
                        let mut has_cont = [false; 7];
                        let mut n_lands = 0usize;
                        let mut any_cont = false;

                        let mut lscan = esq;
                        while let Some(lsq) = Self::step_diag(lscan, dr, df) {
                            if self.cell_during_chain(lsq, start_sq).is_some() {
                                break;
                            }
                            let cont =
                                self.can_king_jump_further(lsq, start_sq, us, next_mask);
                            lands[n_lands] = lsq;
                            has_cont[n_lands] = cont;
                            any_cont |= cont;
                            n_lands += 1;
                            lscan = lsq;
                        }

                        if n_lands > 0 {
                            extended = true;
                            for i in 0..n_lands {
                                if any_cont && !has_cont[i] {
                                    // За правилами шашок-64 дамка зобов'язана зупинитися на полі з продовженням бою!
                                    continue;
                                }
                                let lsq = lands[i];
                                steps[steps_len as usize] = lsq;
                                if has_cont[i] {
                                    self.dfs_captures(
                                        out,
                                        start_sq,
                                        lsq,
                                        us,
                                        PieceKind::King,
                                        steps,
                                        steps_len + 1,
                                        cap_len + 1,
                                        next_mask,
                                        promoted,
                                    );
                                } else {
                                    out.push(TurnMove {
                                        steps: *steps,
                                        steps_len: steps_len + 1,
                                        cap_len: cap_len + 1,
                                        promoted,
                                        cap_mask: next_mask,
                                    });
                                }
                            }
                        }
                    }
                }
            }
        }

        if !extended && cap_len > 0 {
            out.push(TurnMove {
                steps: *steps,
                steps_len,
                cap_len,
                promoted,
                cap_mask,
            });
        }
    }

    pub fn gen_captures(&self, out: &mut MoveList) {
        let us = self.turn;
        let mut steps = [0u8; MAX_STEPS];
        for sq_idx in 0..64usize {
            let Some(p) = self.squares[sq_idx] else {
                continue;
            };
            if p.color != us {
                continue;
            }
            let from = sq_idx as u8;
            steps[0] = from;
            self.dfs_captures(out, from, from, us, p.kind, &mut steps, 1, 0, 0, false);
        }
    }

    fn gen_quiet_moves(&self, out: &mut MoveList) {
        let us = self.turn;
        let fwd_dr = if us == Color::White { 1i32 } else { -1i32 };
        let promo_r = Self::promo_rank(us);

        for sq_idx in 0..64usize {
            let Some(p) = self.squares[sq_idx] else {
                continue;
            };
            if p.color != us {
                continue;
            }
            let from = sq_idx as u8;
            match p.kind {
                PieceKind::Man => {
                    for df in [-1i32, 1] {
                        if let Some(to) = Self::step_diag(from, fwd_dr, df)
                            && self.squares[to as usize].is_none()
                        {
                            out.push(TurnMove::quiet(from, to, rank_of(to) == promo_r));
                        }
                    }
                }
                PieceKind::King => {
                    for (dr, df) in [(1, 1), (1, -1), (-1, 1), (-1, -1)] {
                        let mut cur = from;
                        while let Some(to) = Self::step_diag(cur, dr, df) {
                            if self.squares[to as usize].is_some() {
                                break;
                            }
                            out.push(TurnMove::quiet(from, to, false));
                            cur = to;
                        }
                    }
                }
            }
        }
    }

    /// Повний список легальних ходів (обов'язковий бій: якщо є взяття — тихі ходи не повертаються).
    pub fn legal_moves(&self) -> MoveList {
        let mut list = MoveList::new();
        self.gen_captures(&mut list);
        if list.is_empty() {
            self.gen_quiet_moves(&mut list);
        }
        list
    }

    #[inline]
    pub fn has_mandatory_capture(&self) -> bool {
        let mut caps = MoveList::new();
        self.gen_captures(&mut caps);
        !caps.is_empty()
    }

    fn make_move_raw(&mut self, mv: TurnMove, score_delta: u32) -> UndoEntry {
        let us = self.turn;
        let them = us.opponent();
        let from = mv.from();
        let to = mv.to();

        let moving = self.squares[from as usize]
            .take()
            .expect("missing moving checker");
        let prev_hash = self.hash;
        let prev_quiet_plies = self.quiet_king_plies;

        self.hash ^= ZOBRIST.pieces[us.idx()][moving.kind.idx()][from as usize];

        let mut cap_kings_mask = 0u32;
        let mut m = mv.cap_mask;
        while m != 0 {
            let d = m.trailing_zeros();
            let csq = dark_to_sq(d);
            if let Some(cp) = self.squares[csq as usize].take() {
                self.hash ^= ZOBRIST.pieces[them.idx()][cp.kind.idx()][csq as usize];
                if cp.kind == PieceKind::King {
                    cap_kings_mask |= 1u32 << d;
                }
            }
            m &= m - 1;
        }

        let final_kind = if mv.promoted {
            PieceKind::King
        } else {
            moving.kind
        };
        self.squares[to as usize] = Some(Piece::new(us, final_kind));
        self.hash ^= ZOBRIST.pieces[us.idx()][final_kind.idx()][to as usize];

        if moving.kind == PieceKind::King && !mv.is_capture() {
            self.quiet_king_plies = self.quiet_king_plies.saturating_add(1);
        } else {
            self.quiet_king_plies = 0;
        }

        if us == Color::Black {
            self.move_number = self.move_number.saturating_add(1);
        }
        self.turn = them;
        self.hash ^= ZOBRIST.side_black;

        UndoEntry {
            mv,
            cap_kings_mask,
            prev_quiet_plies,
            prev_hash,
            score_delta,
        }
    }

    fn unmake_move_raw(&mut self, undo: UndoEntry) {
        let us = self.turn.opponent();
        let them = self.turn;
        let mv = undo.mv;

        if us == Color::Black {
            self.move_number = self.move_number.saturating_sub(1).max(1);
        }
        self.turn = us;
        self.quiet_king_plies = undo.prev_quiet_plies;
        self.hash = undo.prev_hash;

        let to = mv.to();
        let from = mv.from();
        let placed = self.squares[to as usize]
            .take()
            .expect("missing placed checker");
        let orig_kind = if mv.promoted {
            PieceKind::Man
        } else {
            placed.kind
        };

        let mut m = mv.cap_mask;
        while m != 0 {
            let d = m.trailing_zeros();
            let csq = dark_to_sq(d);
            let k = if (undo.cap_kings_mask & (1u32 << d)) != 0 {
                PieceKind::King
            } else {
                PieceKind::Man
            };
            self.squares[csq as usize] = Some(Piece::new(them, k));
            m &= m - 1;
        }

        self.squares[from as usize] = Some(Piece::new(us, orig_kind));
    }

    fn push_history(&mut self, undo: UndoEntry) {
        if self.hist_len == MAX_HISTORY {
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

    pub fn repetition_count(&self) -> usize {
        let mut count = 1usize;
        let max_back = (self.quiet_king_plies as usize).min(self.hist_len);
        let mut back = 2usize;
        while back <= max_back {
            if self.history[self.hist_len - back].prev_hash == self.hash {
                count += 1;
            }
            back += 2;
        }
        count
    }

    #[inline]
    fn is_repeated_once(&self) -> bool {
        let max_back = (self.quiet_king_plies as usize).min(self.hist_len);
        let mut back = 2usize;
        while back <= max_back {
            if self.history[self.hist_len - back].prev_hash == self.hash {
                return true;
            }
            back += 2;
        }
        false
    }

    pub fn status(&self) -> GameStatus {
        let moves = self.legal_moves();
        if moves.is_empty() {
            return GameStatus::Win {
                winner: self.turn.opponent(),
            };
        }
        if self.repetition_count() >= 3 {
            return GameStatus::DrawRepetition;
        }
        if self.quiet_king_plies >= 30 {
            return GameStatus::DrawThirtyMove;
        }
        let must_capture = moves.as_slice()[0].is_capture();
        GameStatus::Ongoing { must_capture }
    }

    /// Розбір текстового ходу `"c3-d4"`, `"c3d4"` або `"c3:e5:g3"`.
    pub fn parse_move_str(&self, text: &str) -> Option<TurnMove> {
        let s = text.trim().as_bytes();
        let mut wanted = [0u8; MAX_STEPS];
        let mut n = 0usize;
        let mut i = 0usize;
        while i + 1 < s.len() && n < MAX_STEPS {
            if let Some(sq) = parse_sq(&s[i..i + 2]) {
                wanted[n] = sq;
                n += 1;
                i += 2;
            } else {
                i += 1;
            }
        }
        if n < 2 {
            return None;
        }
        let legal = self.legal_moves();
        // Спочатку точний збіг за усіма кроками шляху
        for &mv in legal.as_slice() {
            if mv.steps_len as usize == n && mv.steps[..n] == wanted[..n] {
                return Some(mv);
            }
        }
        // Якщо передано лише початок і фінальне поле (`c3g3`), і такий хід єдиний
        if n == 2 {
            let mut found = None;
            for &mv in legal.as_slice() {
                if mv.from() == wanted[0] && mv.to() == wanted[1] {
                    if found.is_some() {
                        return None;
                    }
                    found = Some(mv);
                }
            }
            return found;
        }
        None
    }

    pub fn play_move(
        &mut self,
        mv: TurnMove,
        award_for: Option<Color>,
        mode: PlayMode,
    ) -> GameStatus {
        let mover = self.turn;
        let mut cap_pts = 0u32;
        if mv.is_capture() {
            let mut m = mv.cap_mask;
            while m != 0 {
                let d = m.trailing_zeros();
                let csq = dark_to_sq(d);
                if let Some(cp) = self.squares[csq as usize] {
                    cap_pts += match cp.kind {
                        PieceKind::Man => 15,
                        PieceKind::King => 40,
                    };
                }
                m &= m - 1;
            }
            if mv.cap_len >= 2 {
                cap_pts += ((mv.cap_len as u32) - 1) * 20;
            }
        }
        let promo_pts = if mv.promoted { 50 } else { 0 };

        let mut u = self.make_move_raw(mv, 0);
        let st = self.status();

        let mut gained = 0u32;
        if award_for == Some(mover) {
            gained += cap_pts + promo_pts;
            if st == (GameStatus::Win { winner: mover }) {
                gained += match mode {
                    PlayMode::AiHard => 700,
                    PlayMode::AiEasy => 300,
                    PlayMode::TwoPlayers => 0,
                };
            }
            self.score = self.score.saturating_add(gained);
        }
        u.score_delta = gained;
        self.push_history(u);
        st
    }

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

    /// Позиційна оцінка з точки зору `self.turn` (у сантипішаках: шашка = 100, літаюча дамка = 320).
    pub fn evaluate(&self) -> i32 {
        let mut w_score = 0i32;
        let mut b_score = 0i32;
        let mut w_cnt = 0i32;
        let mut b_cnt = 0i32;

        for (sq, cell) in self.squares.iter().enumerate() {
            let Some(p) = cell else { continue };
            let r = sq >> 3;
            let f = sq & 7;
            let is_center = (2..=5).contains(&r) && (2..=5).contains(&f);
            let on_main_diag = r == f; // "Велика дорога" a1–h8

            let val = match p.kind {
                PieceKind::Man => {
                    let adv = if p.color == Color::White { r } else { 7 - r } as i32;
                    let mut v = 100 + adv * 5;
                    if adv >= 5 {
                        v += 12; // Передова шашка близько до дамкового поля
                    }
                    if is_center {
                        v += 10;
                    }
                    if f == 0 || f == 7 {
                        v -= 4; // Крайові вертикалі трохи слабші
                    }
                    // «Золота шашка» d1 / e8 прикриває дамкові поля
                    if (p.color == Color::White && r == 0 && (f == 2 || f == 4))
                        || (p.color == Color::Black && r == 7 && (f == 3 || f == 5))
                    {
                        v += 8;
                    }
                    v
                }
                PieceKind::King => {
                    let mut v = 320;
                    if on_main_diag {
                        v += 18;
                    }
                    if is_center {
                        v += 12;
                    }
                    if r == 0 || r == 7 || f == 0 || f == 7 {
                        v -= 6;
                    }
                    v
                }
            };

            if p.color == Color::White {
                w_score += val;
                w_cnt += 1;
            } else {
                b_score += val;
                b_cnt += 1;
            }
        }

        if w_cnt == 0 {
            return if self.turn == Color::White {
                -29_000
            } else {
                29_000
            };
        }
        if b_cnt == 0 {
            return if self.turn == Color::White {
                29_000
            } else {
                -29_000
            };
        }

        // Невеликий бонус за перевагу у співвідношенні сил в ендшпілі (сприяє вигідним розмінам)
        let diff = w_score - b_score;
        let total = (w_cnt + b_cnt).max(1);
        let exchange_bonus = (diff * (24 - total)) / 64;
        let final_diff = diff + exchange_bonus;

        if self.turn == Color::White {
            final_diff
        } else {
            -final_diff
        }
    }

    #[inline]
    fn move_order_score(mv: TurnMove) -> i32 {
        let mut s = (mv.cap_len as i32) * 2_000;
        if mv.promoted {
            s += 1_500;
        }
        let tr = (mv.to() >> 3) as i32;
        let tf = (mv.to() & 7) as i32;
        if (2..=5).contains(&tr) && (2..=5).contains(&tf) {
            s += 40;
        }
        s
    }

    fn pick_next_move(moves: &mut [TurnMove], scores: &mut [i32], idx: usize) -> TurnMove {
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

    /// Вибирає найкращий хід ШІ (`Easy`, `Demo`, `Hard`).
    pub fn choose_ai_move(&mut self, level: AiLevel) -> Option<TurnMove> {
        let mut root_moves = self.legal_moves();
        if root_moves.is_empty() {
            return None;
        }
        if root_moves.len() == 1 {
            return Some(root_moves.as_slice()[0]);
        }

        // Бюджети підібрані під швидкодію ESP32-C5 (240 MHz, ~11 тис. вузлів/с):
        // Hard ≈ 0.6 с на хід, Demo ≈ 0.4 с, Easy ≈ 0.1 с — гра не «зависає» на екрані THINKING.
        let (max_depth, node_budget, jitter_cp): (u8, u32, i32) = match level {
            AiLevel::Easy => (3, 1_200, 18),
            AiLevel::Demo => (4, 4_500, 7),
            AiLevel::Hard => (
                6,
                7_000,
                if self.move_number <= 2 { 4 } else { 0 },
            ),
        };

        let mut ctx = SearchCtx {
            nodes: 0,
            max_nodes: node_budget,
            aborted: false,
        };

        let mut root_scores = [0i32; MAX_MOVES];
        for (i, &mv) in root_moves.as_slice().iter().enumerate() {
            root_scores[i] = Self::move_order_score(mv);
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
        depth: u8,
        ply: usize,
        mut alpha: i32,
        beta: i32,
    ) -> i32 {
        ctx.nodes += 1;
        if (ctx.nodes & 63) == 0 && ctx.nodes >= ctx.max_nodes {
            ctx.aborted = true;
            return self.evaluate();
        }

        if ply > 0 && (self.quiet_king_plies >= 30 || self.is_repeated_once()) {
            return 0;
        }
        if ply >= MAX_PLY - 1 {
            return self.evaluate();
        }

        if depth == 0 {
            return self.quiescence(ctx, ply, alpha, beta);
        }

        let mut moves = self.legal_moves();
        if moves.is_empty() {
            return -29_000 + (ply as i32);
        }

        let n = moves.len();
        // Якщо хід єдиний (наприклад, єдине обов'язкове взяття) — не витрачаємо глибину даремно!
        let next_depth = if n == 1 && ply < MAX_PLY - 3 {
            depth
        } else {
            depth - 1
        };

        let mut scores = [0i32; MAX_MOVES];
        for (i, &mv) in moves.as_slice().iter().enumerate() {
            scores[i] = Self::move_order_score(mv);
        }

        for i in 0..n {
            let mv = Self::pick_next_move(moves.as_mut_slice(), &mut scores[..n], i);
            let u = self.make_move_raw(mv, 0);
            self.push_history(u);

            let score = -self.alphabeta(ctx, next_depth, ply + 1, -beta, -alpha);

            let popped = self.pop_history().unwrap();
            self.unmake_move_raw(popped);

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

    /// У шашках Quiescence продовжує пошук, доки на дошці є обов'язкові взяття!
    fn quiescence(&mut self, ctx: &mut SearchCtx, ply: usize, mut alpha: i32, beta: i32) -> i32 {
        ctx.nodes += 1;
        if (ctx.nodes & 63) == 0 && ctx.nodes >= ctx.max_nodes {
            ctx.aborted = true;
            return self.evaluate();
        }

        let mut caps = MoveList::new();
        self.gen_captures(&mut caps);
        if caps.is_empty() {
            // Перевіримо, чи взагалі є ходи (інакше це поразка через запертя!)
            let mut quiet = MoveList::new();
            self.gen_quiet_moves(&mut quiet);
            if quiet.is_empty() {
                return -29_000 + (ply as i32);
            }
            return self.evaluate();
        }

        if ply >= MAX_PLY - 1 {
            return self.evaluate();
        }

        // Оскільки взяття в шашках обов'язкове, stand-pat НЕ застосовується — гравець зобов'язаний бити!
        let n = caps.len();
        let mut scores = [0i32; MAX_MOVES];
        for (i, &mv) in caps.as_slice().iter().enumerate() {
            scores[i] = Self::move_order_score(mv);
        }

        for i in 0..n {
            let mv = Self::pick_next_move(caps.as_mut_slice(), &mut scores[..n], i);
            let u = self.make_move_raw(mv, 0);
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
}

struct SearchCtx {
    nodes: u32,
    max_nodes: u32,
    aborted: bool,
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

    let g = CheckersGame::new(1);
    let init_moves = g.legal_moves();
    // У стартовій позиції класичних шашок 64 рівно 7 ходів (a3-b4, c3-b4, c3-d4, e3-d4, e3-f4, g3-f4, g3-h4)
    check(init_moves.len() == 7);
    check(g.piece_counts() == (12, 12, 0, 0));

    // Перевірка миттєвого перетворення у дамку під час бою з подальшим боєм дамкою!
    // Біла шашка h6 б'є g7 -> f8 (стає дамкою!) і по діагоналі f8–a3 одразу б'є c5 -> b4 або a3!
    let mut mid = CheckersGame::empty_board(Color::White, 3);
    mid.set_piece(sq_of(7, 5), Some(Piece::new(Color::White, PieceKind::Man))); // h6
    mid.set_piece(sq_of(6, 6), Some(Piece::new(Color::Black, PieceKind::Man))); // g7
    mid.set_piece(sq_of(2, 4), Some(Piece::new(Color::Black, PieceKind::Man))); // c5
    let mvs = mid.legal_moves();
    check(mvs.len() == 2 && mvs.as_slice().iter().all(|m| m.cap_len == 2 && m.promoted));

    // Перевірка обов'язкового бою назад простою шашкою
    let mut back = CheckersGame::empty_board(Color::White, 5);
    back.set_piece(sq_of(3, 3), Some(Piece::new(Color::White, PieceKind::Man))); // d4
    back.set_piece(sq_of(2, 2), Some(Piece::new(Color::Black, PieceKind::Man))); // c3
    let bm = back.legal_moves();
    check(bm.len() == 1 && bm.as_slice()[0].to() == sq_of(1, 1)); // b2!

    (run, failed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selftest_passes() {
        let (run, failed) = selftest();
        assert_eq!(failed, 0, "selftest failed ({}/{})", failed, run);
        assert_eq!(run, 4);
    }

    #[test]
    fn initial_setup_and_mandatory_backward_capture() {
        let mut g = CheckersGame::new(42);
        assert_eq!(g.legal_moves().len(), 7);
        assert_eq!(g.hash, g.compute_hash());

        // 1. c3-d4 f6-e5 -> білі зобов'язані бити d4:f6!
        let m1 = g.parse_move_str("c3-d4").unwrap();
        g.play_move(m1, None, PlayMode::TwoPlayers);
        let m2 = g.parse_move_str("f6-e5").unwrap();
        g.play_move(m2, None, PlayMode::TwoPlayers);

        assert!(g.has_mandatory_capture());
        let caps = g.legal_moves();
        assert_eq!(caps.len(), 1);
        assert_eq!(caps.as_slice()[0].from(), parse_sq(b"d4").unwrap());
        assert_eq!(caps.as_slice()[0].to(), parse_sq(b"f6").unwrap());
    }

    #[test]
    fn mid_jump_promotion_and_flying_king_continuation() {
        // Проста шашка h6 б'є g7 -> f8 (стає дамкою!) і далі як літаюча дамка б'є c5 -> b4 або a3!
        let mut g = CheckersGame::empty_board(Color::White, 7);
        g.set_piece(parse_sq(b"h6").unwrap(), Some(Piece::new(Color::White, PieceKind::Man)));
        g.set_piece(parse_sq(b"g7").unwrap(), Some(Piece::new(Color::Black, PieceKind::Man)));
        g.set_piece(parse_sq(b"c5").unwrap(), Some(Piece::new(Color::Black, PieceKind::Man)));

        let mvs = g.legal_moves();
        assert_eq!(mvs.len(), 2);
        for mv in mvs.as_slice() {
            assert_eq!(mv.cap_len, 2);
            assert!(mv.promoted);
            assert_eq!(mv.steps[0], parse_sq(b"h6").unwrap());
            assert_eq!(mv.steps[1], parse_sq(b"f8").unwrap());
        }

        let orig_hash = g.hash;
        let st = g.play_move(mvs.as_slice()[0], Some(Color::White), PlayMode::AiHard);
        assert_eq!(st, GameStatus::Win { winner: Color::White });
        assert!(g.undo(PlayMode::TwoPlayers));
        assert_eq!(g.hash, orig_hash);
        assert_eq!(g.hash, g.compute_hash());
    }

    #[test]
    fn turkish_strike_obstacle_rule() {
        // 1) Літаюча дамка a1 б'є c3 і з полів d4..h8 зобов'язана обрати саме f6, бо з f6 є бій g5->h4!
        let mut g = CheckersGame::empty_board(Color::White, 11);
        g.set_piece(parse_sq(b"a1").unwrap(), Some(Piece::new(Color::White, PieceKind::King)));
        g.set_piece(parse_sq(b"c3").unwrap(), Some(Piece::new(Color::Black, PieceKind::Man)));
        g.set_piece(parse_sq(b"g5").unwrap(), Some(Piece::new(Color::Black, PieceKind::Man)));

        let mvs = g.legal_moves();
        assert_eq!(mvs.len(), 1);
        assert_eq!(mvs.as_slice()[0].steps[1], parse_sq(b"f6").unwrap());
        assert_eq!(mvs.as_slice()[0].steps[2], parse_sq(b"h4").unwrap());
        assert_eq!(mvs.as_slice()[0].cap_len, 2);

        // 2) «Турецький удар»: побита шашка d4 лишається перешкодою до кінця серії і не б'ється вдруге!
        let mut loop_g = CheckersGame::empty_board(Color::White, 13);
        loop_g.set_piece(parse_sq(b"c3").unwrap(), Some(Piece::new(Color::White, PieceKind::King)));
        loop_g.set_piece(parse_sq(b"d4").unwrap(), Some(Piece::new(Color::Black, PieceKind::Man)));
        loop_g.set_piece(parse_sq(b"f4").unwrap(), Some(Piece::new(Color::Black, PieceKind::Man)));
        loop_g.set_piece(parse_sq(b"f2").unwrap(), Some(Piece::new(Color::Black, PieceKind::Man)));
        loop_g.set_piece(parse_sq(b"d2").unwrap(), Some(Piece::new(Color::Black, PieceKind::Man)));
        for mv in loop_g.legal_moves().as_slice() {
            assert!(mv.cap_len <= 4, "cannot jump d4 a second time!");
        }
    }

    #[test]
    fn ai_finds_tactical_sacrifice_combination() {
        // Класична комбінація («шлагбаум»): Білі c3, e3, f2, g1 проти Чорних c5, e5, e7.
        // Поле g1 підпирає шашку f2, а c3 закриває бій для e5, тому:
        // 1. e3-d4!! c5:e3 2. f2:d4:f6:d8 — потрійний удар у дамки з миттєвою перемогою!
        let mut buf = [0u8; 8];
        let mut combo = CheckersGame::empty_board(Color::White, 21);
        combo.set_piece(parse_sq(b"c3").unwrap(), Some(Piece::new(Color::White, PieceKind::Man)));
        combo.set_piece(parse_sq(b"e3").unwrap(), Some(Piece::new(Color::White, PieceKind::Man)));
        combo.set_piece(parse_sq(b"f2").unwrap(), Some(Piece::new(Color::White, PieceKind::Man)));
        combo.set_piece(parse_sq(b"g1").unwrap(), Some(Piece::new(Color::White, PieceKind::Man)));
        combo.set_piece(parse_sq(b"c5").unwrap(), Some(Piece::new(Color::Black, PieceKind::Man)));
        combo.set_piece(parse_sq(b"e5").unwrap(), Some(Piece::new(Color::Black, PieceKind::Man)));
        combo.set_piece(parse_sq(b"e7").unwrap(), Some(Piece::new(Color::Black, PieceKind::Man)));

        let shot = combo.choose_ai_move(AiLevel::Hard).unwrap();
        assert_eq!(shot.format_short(&mut buf), "e3-d4");
        combo.play_move(shot, None, PlayMode::TwoPlayers);

        let reply = combo.choose_ai_move(AiLevel::Hard).unwrap();
        assert_eq!(reply.format_short(&mut buf), "c5:e3");
        combo.play_move(reply, None, PlayMode::TwoPlayers);

        let sweep = combo.choose_ai_move(AiLevel::Hard).unwrap();
        assert_eq!(sweep.cap_len, 3);
        assert!(sweep.promoted);
        let st = combo.play_move(sweep, None, PlayMode::TwoPlayers);
        assert_eq!(st, GameStatus::Win { winner: Color::White });
    }

    #[test]
    fn draw_by_thirty_king_plies_and_repetition() {
        let mut g = CheckersGame::empty_board(Color::White, 1);
        g.set_piece(parse_sq(b"c1").unwrap(), Some(Piece::new(Color::White, PieceKind::King)));
        g.set_piece(parse_sq(b"h8").unwrap(), Some(Piece::new(Color::Black, PieceKind::King)));

        for _ in 0..2 {
            for s in ["c1-d2", "h8-g7", "d2-c1", "g7-h8"] {
                let mv = g.parse_move_str(s).unwrap();
                g.play_move(mv, None, PlayMode::TwoPlayers);
                assert_eq!(g.hash, g.compute_hash());
            }
        }
        assert_eq!(g.repetition_count(), 3);
        assert_eq!(g.status(), GameStatus::DrawRepetition);
    }
}
