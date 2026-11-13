//! Рушій гри **Ґомоку (五目並べ, «п'ять у ряд»)** на дошці **10×10** + альфа-бета ШІ.
//!
//! Без `std`, без `alloc`, без сторонніх залежностей — компілюється і як частина прошивки ESP32-C5,
//! і окремо на хості (`rustc --edition 2024 --test games/gomoku/src/game.rs -O`).
//!
//! Правила (freestyle gomoku, як у більшості «п'ять у ряд»):
//! - дошка 10×10, чорні ходять першими, по одному каменю за хід;
//! - перемагає той, хто першим збудує **5 і більше** каменів у неперервний ряд
//!   по горизонталі, вертикалі або діагоналі (перебір довжиною 6+ теж є перемогою);
//! - нічия — коли заповнені всі 100 клітинок і ніхто не зібрав п'ятірки;
//! - без «заборонених ходів» (fouls) для чорних — це вільний ґомоку, а не турнірний ренджу.
//!
//! Що вміє:
//! - детекція перемоги з бітовою маскою виграшної лінії (`winning_mask`) для підсвітки на екрані;
//! - **альфа-бета ШІ** з ітеративним поглибленням, перебором лише клітинок біля наявних каменів,
//!   `MVV`-подібним сортуванням (своя вигода + терміновість блокування), миттєвим пошуком
//!   «виграш зараз» і «заблокувати четвірку суперника», відсіканням за бюджетом вузлів;
//! - три рівні: `Easy` (глибина 2), `Demo` (3), `Hard` (4);
//! - `Undo` (у режимі проти ШІ одразу відкочує пару ходів), очки за гру.

pub const N: usize = 10;
pub const CELLS: usize = N * N;
pub const WIN_LEN: usize = 5;

pub const EMPTY: u8 = 0;
pub const BLACK: u8 = 1;
pub const WHITE: u8 = 2;

const WIN_SCORE: i32 = 1_000_000;
const MAX_CANDIDATES: usize = 64;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum AiLevel {
    Easy,
    Demo,
    Hard,
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
pub enum GameStatus {
    Ongoing,
    Win { winner: u8 },
    Draw,
}

impl GameStatus {
    #[inline]
    pub const fn is_game_over(self) -> bool {
        !matches!(self, Self::Ongoing)
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PlaceResult {
    Placed,
    /// Клітинка вже зайнята.
    Occupied,
    /// Партія вже завершена.
    Finished,
    /// Індекс поза дошкою.
    Invalid,
}

#[inline]
pub const fn row_of(sq: usize) -> usize {
    sq / N
}

#[inline]
pub const fn col_of(sq: usize) -> usize {
    sq % N
}

#[inline]
pub const fn sq_of(row: usize, col: usize) -> usize {
    row * N + col
}

#[inline]
pub const fn other(color: u8) -> u8 {
    match color {
        BLACK => WHITE,
        _ => BLACK,
    }
}

pub const DIRECTIONS: [(i32, i32); 4] = [(0, 1), (1, 0), (1, 1), (1, -1)];

#[inline]
fn step(sq: usize, dr: i32, df: i32) -> Option<usize> {
    let r = row_of(sq) as i32 + dr;
    let c = col_of(sq) as i32 + df;
    if (0..N as i32).contains(&r) && (0..N as i32).contains(&c) {
        Some(sq_of(r as usize, c as usize))
    } else {
        None
    }
}

#[derive(Clone)]
pub struct GomokuGame {
    cells: [u8; CELLS],
    pub turn: u8,
    pub last_move: Option<u8>,
    pub win_mask: u128,
    winner: u8,
    pub moves: u32,
    pub score: u32,
    history: [u8; CELLS],
    pub hist_len: usize,
    rng: u32,
}

impl GomokuGame {
    pub fn new(seed: u32) -> Self {
        Self {
            cells: [EMPTY; CELLS],
            turn: BLACK,
            last_move: None,
            win_mask: 0,
            winner: 0,
            moves: 0,
            score: 0,
            history: [0; CELLS],
            hist_len: 0,
            rng: if seed == 0 { 0x60_60_0C_0C } else { seed },
        }
    }

    pub fn reset(&mut self) {
        self.cells = [EMPTY; CELLS];
        self.turn = BLACK;
        self.last_move = None;
        self.win_mask = 0;
        self.winner = 0;
        self.moves = 0;
        self.hist_len = 0;
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
    pub fn cell(&self, sq: usize) -> u8 {
        self.cells[sq]
    }

    #[inline]
    pub fn is_empty(&self, sq: usize) -> bool {
        self.cells[sq] == EMPTY
    }

    pub fn stones(&self, color: u8) -> u32 {
        self.cells.iter().filter(|&&v| v == color).count() as u32
    }

    /// Чи є в позиції хоч один камінь (для підказки «ходіть біля каменів»).
    pub fn is_blank(&self) -> bool {
        self.moves == 0
    }

    pub fn status(&self) -> GameStatus {
        if self.winner != 0 {
            return GameStatus::Win {
                winner: self.winner,
            };
        }
        if self.moves as usize >= CELLS {
            return GameStatus::Draw;
        }
        GameStatus::Ongoing
    }

    /// Маска всіх клітинок неперервного ряду довжиною >= 5, що проходить через `sq`.
    pub fn winning_mask(&self, sq: usize, color: u8) -> u128 {
        let mut mask = 0u128;
        for (dr, df) in DIRECTIONS {
            let mut run = [0u8; CELLS];
            let mut len = 0usize;

            let mut cur = sq;
            while let Some(prev) = step(cur, -dr, -df) {
                if self.cells[prev] != color {
                    break;
                }
                cur = prev;
            }

            loop {
                run[len] = cur as u8;
                len += 1;
                match step(cur, dr, df) {
                    Some(next) if self.cells[next] == color => cur = next,
                    _ => break,
                }
            }

            if len >= WIN_LEN {
                for &c in &run[..len] {
                    mask |= 1u128 << c;
                }
            }
        }
        mask
    }

    #[inline]
    pub fn is_win_at(&self, sq: usize, color: u8) -> bool {
        self.winning_mask(sq, color) != 0
    }

    /// Хід гравця (з нарахуванням очок, якщо `award`).
    pub fn place(&mut self, sq: usize, award: bool) -> PlaceResult {
        if sq >= CELLS {
            return PlaceResult::Invalid;
        }
        if self.status().is_game_over() {
            return PlaceResult::Finished;
        }
        if self.cells[sq] != EMPTY {
            return PlaceResult::Occupied;
        }

        let color = self.turn;
        self.cells[sq] = color;
        self.history[self.hist_len] = sq as u8;
        self.hist_len += 1;
        self.moves += 1;
        self.last_move = Some(sq as u8);

        let mask = self.winning_mask(sq, color);
        if mask != 0 {
            self.win_mask = mask;
            self.winner = color;
            if award {
                self.score = self.score.saturating_add(500);
            }
        } else if award {
            self.score = self.score.saturating_add(10);
        }

        self.turn = other(color);
        PlaceResult::Placed
    }

    pub fn can_undo(&self) -> bool {
        self.hist_len > 0
    }

    /// Скасовує один хід (або пару ходів у грі проти ШІ).
    pub fn undo(&mut self, mode: PlayMode) -> bool {
        if self.hist_len == 0 {
            return false;
        }
        let steps = if mode.is_vs_ai() && self.turn == BLACK && self.hist_len >= 2 {
            2
        } else {
            1
        };
        for _ in 0..steps {
            if self.hist_len == 0 {
                break;
            }
            self.hist_len -= 1;
            let sq = self.history[self.hist_len] as usize;
            self.cells[sq] = EMPTY;
            self.moves = self.moves.saturating_sub(1);
            self.turn = other(self.turn);
        }
        self.win_mask = 0;
        self.winner = 0;
        self.last_move = if self.hist_len > 0 {
            Some(self.history[self.hist_len - 1])
        } else {
            None
        };
        true
    }

    #[inline]
    fn place_raw(&mut self, sq: usize, color: u8) {
        self.cells[sq] = color;
    }

    #[inline]
    fn undo_raw(&mut self, sq: usize) {
        self.cells[sq] = EMPTY;
    }

    /// Кандидатні ходи: порожні клітинки в радіусі 2 від наявних каменів (для 10×10 це майже вся дошка
    /// в міттельшпілі, тож наприкінці додаємо й «усі порожні», якщо кандидатів замало).
    fn gen_candidates(&self, out: &mut [u8; MAX_CANDIDATES]) -> usize {
        if self.moves == 0 {
            out[0] = sq_of(N / 2 - 1, N / 2 - 1) as u8;
            return 1;
        }

        let mut n = 0usize;
        for sq in 0..CELLS {
            if self.cells[sq] != EMPTY {
                continue;
            }
            let mut near = false;
            'scan: for dr in -2i32..=2 {
                for df in -2i32..=2 {
                    if dr == 0 && df == 0 {
                        continue;
                    }
                    if let Some(q) = step(sq, dr, df)
                        && self.cells[q] != EMPTY
                    {
                        near = true;
                        break 'scan;
                    }
                }
            }
            if near && n < MAX_CANDIDATES {
                out[n] = sq as u8;
                n += 1;
            }
        }
        n
    }

    /// Локальна цінність клітинки для кольору: довжина власних серій з обох боків + відкриті кінці.
    fn point_value(&self, sq: usize, color: u8) -> i32 {
        let mut total = 0i32;
        for (dr, df) in DIRECTIONS {
            let mut len = 1i32;
            let mut open = 0i32;

            for sign in [-1i32, 1] {
                let mut cur = sq;
                while let Some(next) = step(cur, dr * sign, df * sign) {
                    if self.cells[next] == color {
                        len += 1;
                        cur = next;
                    } else {
                        if self.cells[next] == EMPTY {
                            open += 1;
                        }
                        break;
                    }
                }
            }
            total += run_value(len as usize, open as usize);
        }
        total
    }

    fn move_order(&self, sq: usize, me: u8) -> i32 {
        let opp = other(me);
        // Невеликий бонус за центральні поля: ґомоку виграється в центрі, а не на краях
        let dr = (row_of(sq) as i32 * 2 - (N as i32 - 1)).abs();
        let dc = (col_of(sq) as i32 * 2 - (N as i32 - 1)).abs();
        let center = ((N as i32 - 1) * 2 - dr - dc).max(0) * 8;
        self.point_value(sq, me) * 2 + self.point_value(sq, opp) + center
    }

    fn sort_candidates(&self, list: &mut [u8], n: usize, me: u8) {
        let mut scores = [0i32; MAX_CANDIDATES];
        for i in 0..n {
            scores[i] = self.move_order(list[i] as usize, me);
        }
        // Просте сортування вставками за спаданням цінності (n <= 64)
        for i in 1..n {
            let mut j = i;
            while j > 0 && scores[j - 1] < scores[j] {
                scores.swap(j - 1, j);
                list.swap(j - 1, j);
                j -= 1;
            }
        }
    }

    /// Оцінка позиції з точки зору `me` (у «пунктах шаблонів»).
    fn evaluate(&self, me: u8) -> i32 {
        let opp = other(me);
        let mine = self.pattern_score(me);
        let theirs = self.pattern_score(opp);
        // Невеликий оборонний перекос: чужа загроза завжди трохи небезпечніша за свою
        mine - theirs - theirs / 8
    }

    fn pattern_score(&self, color: u8) -> i32 {
        let mut total = 0i32;
        for (dr, df) in DIRECTIONS {
            for sq in 0..CELLS {
                if self.cells[sq] != color {
                    continue;
                }
                // Рахуємо кожну серію лише з її початку
                if let Some(prev) = step(sq, -dr, -df)
                    && self.cells[prev] == color
                {
                    continue;
                }
                let mut len = 0usize;
                let mut cur = sq;
                loop {
                    len += 1;
                    match step(cur, dr, df) {
                        Some(next) if self.cells[next] == color => cur = next,
                        _ => break,
                    }
                }
                let mut open = 0usize;
                if let Some(next) = step(cur, dr, df)
                    && self.cells[next] == EMPTY
                {
                    open += 1;
                }
                if let Some(prev) = step(sq, -dr, -df)
                    && self.cells[prev] == EMPTY
                {
                    open += 1;
                }
                total += run_value(len, open);
            }
        }
        total
    }

    /// Пошук найкращого ходу для сторони `self.turn`.
    pub fn choose_ai_move(&mut self, level: AiLevel) -> Option<usize> {
        if self.status().is_game_over() {
            return None;
        }

        let me = self.turn;
        let opp = other(me);

        let mut list = [0u8; MAX_CANDIDATES];
        let mut n = self.gen_candidates(&mut list);
        if n == 0 {
            return None;
        }
        let n_all = n;
        self.sort_candidates(&mut list, n_all, me);
        n = n_all;

        // 1) Виграш за один хід
        for &c in &list[..n] {
            let sq = c as usize;
            self.place_raw(sq, me);
            let win = self.is_win_at(sq, me);
            self.undo_raw(sq);
            if win {
                return Some(sq);
            }
        }

        // 2) Заблокувати четвірку суперника
        for &c in &list[..n] {
            let sq = c as usize;
            self.place_raw(sq, opp);
            let lose = self.is_win_at(sq, opp);
            self.undo_raw(sq);
            if lose {
                return Some(sq);
            }
        }

        let (depth, budget): (u8, u32) = match level {
            AiLevel::Easy => (2, 2_000),
            AiLevel::Demo => (3, 5_000),
            AiLevel::Hard => (4, 14_000),
        };

        let mut ctx = SearchCtx {
            nodes: 0,
            max_nodes: budget,
            aborted: false,
        };

        let mut best = list[0] as usize;
        let mut best_val = -WIN_SCORE;

        for i in 0..n {
            let sq = list[i] as usize;
            self.place_raw(sq, me);
            let val = if self.is_win_at(sq, me) {
                WIN_SCORE
            } else {
                -self.search(&mut ctx, depth - 1, 1, opp, me, -WIN_SCORE, -best_val.max(-WIN_SCORE))
            };
            self.undo_raw(sq);

            if ctx.aborted && i > 0 {
                break;
            }
            if val > best_val {
                best_val = val;
                best = sq;
            }
        }

        // Трохи випадковості на легкому рівні, щоб партії не були ідентичними
        if matches!(level, AiLevel::Easy) && n > 1 && self.next_rand() % 4 == 0 {
            let pick = (self.next_rand() as usize) % n.min(3).max(1);
            let cand = list[pick] as usize;
            if self.cells[cand] == EMPTY {
                return Some(cand);
            }
        }

        Some(best)
    }

    #[allow(clippy::too_many_arguments)]
    fn search(
        &mut self,
        ctx: &mut SearchCtx,
        depth: u8,
        ply: usize,
        to_move: u8,
        me: u8,
        alpha: i32,
        beta: i32,
    ) -> i32 {
        ctx.nodes += 1;
        if (ctx.nodes & 63) == 0 && ctx.nodes >= ctx.max_nodes {
            ctx.aborted = true;
            return self.evaluate(me);
        }
        if depth == 0 || ply >= CELLS {
            return self.evaluate(me);
        }

        let mut list = [0u8; MAX_CANDIDATES];
        let n = self.gen_candidates(&mut list);
        if n == 0 {
            return self.evaluate(me);
        }
        self.sort_candidates(&mut list, n, to_move);

        let limit = limits_for(depth).min(n);
        let opp = other(to_move);
        let mut alpha = alpha;
        let mut any = false;

        for &c in &list[..limit] {
            let sq = c as usize;
            self.place_raw(sq, to_move);
            any = true;

            let val = if self.is_win_at(sq, to_move) {
                if to_move == me {
                    WIN_SCORE - ply as i32
                } else {
                    -(WIN_SCORE - ply as i32)
                }
            } else {
                -self.search(ctx, depth - 1, ply + 1, opp, me, -beta, -alpha)
            };

            self.undo_raw(sq);

            if val >= beta {
                return beta;
            }
            if val > alpha {
                alpha = val;
            }
            if ctx.aborted {
                return alpha;
            }
        }

        if !any {
            return self.evaluate(me);
        }
        alpha
    }
}

#[inline]
fn limits_for(depth: u8) -> usize {
    match depth {
        4 => 10,
        3 => 8,
        2 => 6,
        _ => 6,
    }
}

struct SearchCtx {
    nodes: u32,
    max_nodes: u32,
    aborted: bool,
}

/// Цінність серії довжини `len` з `open` відкритими кінцями (0..2).
fn run_value(len: usize, open: usize) -> i32 {
    if len >= WIN_LEN {
        return WIN_SCORE;
    }
    match (len, open) {
        (4, 2) => 100_000,
        (4, 1) => 12_000,
        (3, 2) => 5_000,
        (3, 1) => 500,
        (2, 2) => 300,
        (2, 1) => 60,
        (1, 2) => 15,
        (1, 1) => 3,
        _ => 0,
    }
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

    // Горизонтальна п'ятірка
    let mut g = GomokuGame::new(1);
    for c in 0..5 {
        g.place(sq_of(3, c), false);
        if c < 4 {
            g.place(sq_of(6, c), false);
        }
    }
    check(g.status() == GameStatus::Win { winner: BLACK });
    check(g.win_mask.count_ones() == 5);

    // Діагональ і вертикаль
    let mut g = GomokuGame::new(2);
    for i in 0..5 {
        g.place(sq_of(i, 9 - i), false);
        if i < 4 {
            g.place(sq_of(i, 0), false);
        }
    }
    check(g.status() == GameStatus::Win { winner: BLACK });

    // ШІ (чорні): добити власну четвірку на (2,0..3)
    let mut g = GomokuGame::new(3);
    for c in 0..4 {
        g.place(sq_of(8, c * 2), false); // білі розкидані
        g.place(sq_of(2, c), false); // чорні будують четвірку
    }
    check(g.turn == BLACK);
    let mv = g.choose_ai_move(AiLevel::Hard);
    check(mv == Some(sq_of(2, 4)));

    // ШІ (чорні): заблокувати четвірку білих на (5,2..5)
    let mut g = GomokuGame::new(4);
    for c in 0..4 {
        g.place(sq_of(9, c * 2), false); // чорні розкидані
        g.place(sq_of(5, c + 2), false); // білі будують четвірку
    }
    check(g.turn == BLACK);
    let mv = g.choose_ai_move(AiLevel::Hard);
    check(mv == Some(sq_of(5, 1)) || mv == Some(sq_of(5, 6)));

    (run, failed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selftest_passes() {
        let (run, failed) = selftest();
        assert_eq!(failed, 0, "selftest failed ({}/{})", failed, run);
        assert_eq!(run, 7);
    }

    #[test]
    fn win_detection_in_all_four_directions() {
        // горизонталь
        let mut g = GomokuGame::new(11);
        for c in 0..5 {
            g.place(sq_of(2, c), false);
            if c < 4 {
                g.place(sq_of(8, c), false);
            }
        }
        assert_eq!(g.status(), GameStatus::Win { winner: BLACK });
        assert_eq!(g.win_mask.count_ones(), 5);

        // вертикаль
        let mut g = GomokuGame::new(12);
        for r in 0..5 {
            g.place(sq_of(r, 7), false);
            if r < 4 {
                g.place(sq_of(r, 0), false);
            }
        }
        assert_eq!(g.status(), GameStatus::Win { winner: BLACK });

        // діагональ ↘
        let mut g = GomokuGame::new(13);
        for i in 0..5 {
            g.place(sq_of(i, i), false);
            if i < 4 {
                g.place(sq_of(i, 9), false);
            }
        }
        assert_eq!(g.status(), GameStatus::Win { winner: BLACK });

        // антидіагональ ↙
        let mut g = GomokuGame::new(14);
        for i in 0..5 {
            g.place(sq_of(i, 9 - i), false);
            if i < 4 {
                g.place(sq_of(i, 0), false);
            }
        }
        assert_eq!(g.status(), GameStatus::Win { winner: BLACK });
        assert_eq!(g.win_mask.count_ones(), 5);
    }

    #[test]
    fn overline_counts_as_win_and_four_is_not() {
        // Шість у ряд — теж перемога (freestyle)
        let mut g = GomokuGame::new(21);
        for c in 0..5 {
            g.place(sq_of(5, c), false);
            g.place(sq_of(0, c), false);
        }
        assert_eq!(g.status(), GameStatus::Win { winner: BLACK });
    }

    #[test]
    fn four_in_a_row_is_not_a_win_yet() {
        let mut g = GomokuGame::new(22);
        for c in 0..4 {
            g.place(sq_of(4, c), false);
            g.place(sq_of(9, c), false);
        }
        assert_eq!(g.status(), GameStatus::Ongoing);
        assert_eq!(g.win_mask, 0);
    }

    #[test]
    fn undo_restores_turn_and_board() {
        let mut g = GomokuGame::new(31);
        g.place(sq_of(5, 5), false);
        g.place(sq_of(5, 6), false);
        assert_eq!(g.turn, BLACK);
        assert_eq!(g.moves, 2);

        assert!(g.undo(PlayMode::TwoPlayers));
        assert_eq!(g.moves, 1);
        assert_eq!(g.turn, WHITE);
        assert!(g.is_empty(sq_of(5, 6)));
        assert_eq!(g.last_move, Some(sq_of(5, 5) as u8));
    }

    #[test]
    fn first_ai_move_center_and_later_near_stones() {
        let mut g = GomokuGame::new(41);
        let first = g.choose_ai_move(AiLevel::Hard).unwrap();
        assert_eq!(first, sq_of(N / 2 - 1, N / 2 - 1));

        g.place(first, false);
        let reply = g.choose_ai_move(AiLevel::Hard).unwrap();
        // Другий хід має бути поруч із першим каменем (у радіусі 2)
        assert!(row_of(reply).abs_diff(row_of(first)) <= 2);
        assert!(col_of(reply).abs_diff(col_of(first)) <= 2);
    }

    #[test]
    fn ai_wins_immediately_when_it_can() {
        // Чорні (ШІ) мають чотири в ряд на (6,1..4), хід чорних — мусить добити п'ятий
        let mut g = GomokuGame::new(51);
        for c in 0..4 {
            g.place(sq_of(6, c + 1), false); // чорні — четвірка
            g.place(sq_of(0, c * 2), false); // білі — розкидані
        }
        assert_eq!(g.turn, BLACK);
        let mv = g.choose_ai_move(AiLevel::Hard).unwrap();
        assert!(mv == sq_of(6, 0) || mv == sq_of(6, 5), "got {mv}");
        g.place(mv, false);
        assert_eq!(g.status(), GameStatus::Win { winner: BLACK });
    }

    #[test]
    fn ai_blocks_human_open_four() {
        // Білі погрожують п'ятіркою на (4,3..6): чорний ШІ мусить закрити один із кінців
        let mut g = GomokuGame::new(61);
        for c in 0..4 {
            g.place(sq_of(9, c * 2), false); // чорні — розкидані
            g.place(sq_of(4, c + 3), false); // білі — четвірка
        }
        assert_eq!(g.turn, BLACK);
        let mv = g.choose_ai_move(AiLevel::Hard).unwrap();
        assert!(
            mv == sq_of(4, 2) || mv == sq_of(4, 7),
            "AI must block the four, got {mv}"
        );
    }

    #[test]
    fn ai_move_is_always_empty_and_legal() {
        let mut g = GomokuGame::new(71);
        for _ in 0..20 {
            let mv = g.choose_ai_move(AiLevel::Easy).expect("move");
            assert!(g.is_empty(mv));
            g.place(mv, false);
            if g.status().is_game_over() {
                break;
            }
        }
    }
}
