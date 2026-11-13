//! Повний рушій класичного Судоку 9×9 (генератор, солвер, правила гри).
//!
//! Без `std`, без `alloc`, без сторонніх залежностей — компілюється і як частина прошивки ESP32-C5,
//! і окремо на хості (`rustc --edition 2024 --test games/sudoku/src/game.rs -O`).
//!
//! Що вміє:
//! - **генерація** повністю випадкової коректної сітки 9×9 (рандомізований backtracking із MRV);
//! - **віднімання цифр** із перевіркою ЄДИНОСТІ розв'язку (лічильник розв'язків із відсіканням на 2),
//!   тому кожна видана головоломка має рівно один розв'язок;
//! - три рівні складності за кількістю підказок (`Easy` 40, `Medium` 32, `Hard` 26);
//! - **солвер** `solve()` (повний розв'язок) і `count_solutions(limit)` для перевірок;
//! - перевірка конфліктів (однакові цифри в рядку / стовпці / квадраті 3×3) як бітова маска 81;
//! - гра: заповнення клітинки, помилки, підказки, підрахунок очок, `is_solved()`;
//! - `hint_square()` / `apply_hint()` і `demo_step()` — повне автозаповнення для режиму `DEMO`.

pub const N: usize = 9;
pub const CELLS: usize = 81;
pub const ALL_DIGITS: u16 = 0b0000_0011_1111_1110; // біти 1..9

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Difficulty {
    Easy,
    Medium,
    Hard,
}

impl Difficulty {
    pub const fn next(self) -> Self {
        match self {
            Self::Easy => Self::Medium,
            Self::Medium => Self::Hard,
            Self::Hard => Self::Easy,
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::Easy => "EASY",
            Self::Medium => "MEDIUM",
            Self::Hard => "HARD",
        }
    }

    /// Скільки цифр лишається в головоломці як підказки.
    pub const fn givens(self) -> usize {
        match self {
            Self::Easy => 40,
            Self::Medium => 32,
            Self::Hard => 26,
        }
    }

    /// Бонус за повністю розв'язану головоломку.
    pub const fn bonus(self) -> u32 {
        match self {
            Self::Easy => 200,
            Self::Medium => 400,
            Self::Hard => 700,
        }
    }
}

#[inline]
pub const fn row_of(sq: usize) -> usize {
    sq / 9
}

#[inline]
pub const fn col_of(sq: usize) -> usize {
    sq % 9
}

#[inline]
pub const fn box_of(sq: usize) -> usize {
    (sq / 27) * 3 + (sq % 9) / 3
}

/// Чи є в позиції хоч один конфлікт (однакова цифра у рядку / стовпці / квадраті)?
pub fn has_conflicts(cells: &[u8; CELLS]) -> bool {
    conflicts_mask(cells) != 0
}

/// Бітова маска 81: біт `i` = клітинка `i` конфліктує з якоюсь іншою.
pub fn conflicts_mask(cells: &[u8; CELLS]) -> u128 {
    let mut mask = 0u128;

    for i in 0..CELLS {
        let v = cells[i];
        if v == 0 {
            continue;
        }
        let (r, c, b) = (row_of(i), col_of(i), box_of(i));
        for j in (i + 1)..CELLS {
            if cells[j] != v {
                continue;
            }
            if row_of(j) == r || col_of(j) == c || box_of(j) == b {
                mask |= (1u128 << i) | (1u128 << j);
            }
        }
    }
    mask
}

#[inline]
fn digit_bit(v: u8) -> u16 {
    1u16 << v
}

/// Маска вже використаних цифр у рядку, стовпці та квадраті клітинки `sq`.
fn used_mask(cells: &[u8; CELLS], sq: usize) -> u16 {
    let (r, c, b) = (row_of(sq), col_of(sq), box_of(sq));
    let mut used = 0u16;
    for i in 0..9 {
        used |= digit_bit(cells[r * 9 + i]);
        used |= digit_bit(cells[i * 9 + c]);
    }
    let br = (b / 3) * 3;
    let bc = (b % 3) * 3;
    for dr in 0..3 {
        for dc in 0..3 {
            used |= digit_bit(cells[(br + dr) * 9 + bc + dc]);
        }
    }
    used
}

/// Допустимі цифри (бітова маска) для клітинки; для заповненої — 0.
pub fn candidates(cells: &[u8; CELLS], sq: usize) -> u16 {
    if cells[sq] != 0 {
        return 0;
    }
    ALL_DIGITS & !used_mask(cells, sq)
}

struct Rng32(u32);

impl Rng32 {
    #[inline]
    fn next(&mut self) -> u32 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.0 = if x == 0 { 0x9E37_79B9 } else { x };
        x
    }

    #[inline]
    fn below(&mut self, n: u32) -> u32 {
        if n == 0 { 0 } else { self.next() % n }
    }
}

fn digits_of_mask(mask: u16, out: &mut [u8; 9]) -> usize {
    let mut n = 0usize;
    for d in 1..=9u8 {
        if mask & digit_bit(d) != 0 {
            out[n] = d;
            n += 1;
        }
    }
    n
}

/// Знаходить порожню клітинку з найменшою кількістю кандидатів (MRV).
fn best_empty(cells: &[u8; CELLS]) -> Option<(usize, u16)> {
    let mut best: Option<(usize, u16)> = None;
    let mut best_count = 10u32;
    for sq in 0..CELLS {
        if cells[sq] != 0 {
            continue;
        }
        let mask = candidates(cells, sq);
        let count = mask.count_ones();
        if count == 0 {
            return Some((sq, 0)); // глухий кут — нехай солвер одразу відсічеться
        }
        if count < best_count {
            best_count = count;
            best = Some((sq, mask));
            if count == 1 {
                break;
            }
        }
    }
    best
}

/// Рекурсивний пошук розв'язків. `limit` — скільки максимум розв'язків шукати.
fn solve_rec(
    cells: &mut [u8; CELLS],
    limit: u8,
    found: &mut u8,
    first: &mut Option<[u8; CELLS]>,
    mut rng: Option<&mut Rng32>,
) {
    if *found >= limit {
        return;
    }
    let Some((sq, mask)) = best_empty(cells) else {
        *found += 1;
        if first.is_none() {
            *first = Some(*cells);
        }
        return;
    };
    if mask == 0 {
        return;
    }

    let mut digits = [0u8; 9];
    let n = digits_of_mask(mask, &mut digits);

    // Для генерації перебираємо цифри у випадковому порядку (щоб сітка була різноманітною)
    if let Some(r) = rng.as_mut() {
        for i in (1..n).rev() {
            let j = (*r).below((i + 1) as u32) as usize;
            digits.swap(i, j);
        }
    }

    for &d in &digits[..n] {
        cells[sq] = d;
        solve_rec(cells, limit, found, first, rng.as_deref_mut());
        cells[sq] = 0;
        if *found >= limit {
            return;
        }
    }
}

/// Скільки розв'язків має позиція (не більше `limit`).
pub fn count_solutions(cells: &[u8; CELLS], limit: u8) -> u8 {
    let mut work = *cells;
    let mut found = 0u8;
    let mut first = None;
    solve_rec(&mut work, limit.max(1), &mut found, &mut first, None);
    found
}

/// Повний розв'язок позиції (перший знайдений) або `None`, якщо він не існує.
pub fn solve(cells: &[u8; CELLS]) -> Option<[u8; CELLS]> {
    let mut work = *cells;
    let mut found = 0u8;
    let mut first = None;
    solve_rec(&mut work, 1, &mut found, &mut first, None);
    first
}

/// Рівень складності, який реально вийшов (кількість підказок після віднімання).
pub struct Generated {
    pub puzzle: [u8; CELLS],
    pub solution: [u8; CELLS],
    pub givens: usize,
}

/// Генерує нову головоломку з єдиним розв'язком.
pub fn generate(seed: u32, difficulty: Difficulty) -> Generated {
    let mut rng = Rng32(if seed == 0 { 0x5EED_5EED } else { seed });

    // 1. Повністю випадкова валідна сітка
    let mut solution = [0u8; CELLS];
    let mut found = 0u8;
    let mut first = None;
    solve_rec(&mut solution, 1, &mut found, &mut first, Some(&mut rng));
    let solution = first.unwrap_or_else(fallback_solution);

    // 2. Віднімаємо цифри, доки розв'язок лишається єдиним і підказок більше за ціль
    let mut puzzle = solution;
    let target = difficulty.givens();
    let mut givens = CELLS;

    let mut order = [0u8; CELLS];
    for (i, slot) in order.iter_mut().enumerate() {
        *slot = i as u8;
    }
    for i in (1..CELLS).rev() {
        let j = rng.below((i + 1) as u32) as usize;
        order.swap(i, j);
    }

    for &sq in order.iter() {
        if givens <= target {
            break;
        }
        let sq = sq as usize;
        let saved = puzzle[sq];
        if saved == 0 {
            continue;
        }
        puzzle[sq] = 0;
        if count_solutions(&puzzle, 2) == 1 {
            givens -= 1;
        } else {
            puzzle[sq] = saved;
        }
    }

    Generated {
        puzzle,
        solution,
        givens,
    }
}

/// Класична тестова головоломка (єдиний розв'язок — `FALLBACK_SOLUTION`).
pub const TEST_PUZZLE: [u8; CELLS] = [
    5, 3, 0, 0, 7, 0, 0, 0, 0, //
    6, 0, 0, 1, 9, 5, 0, 0, 0, //
    0, 9, 8, 0, 0, 0, 0, 6, 0, //
    8, 0, 0, 0, 6, 0, 0, 0, 3, //
    4, 0, 0, 8, 0, 3, 0, 0, 1, //
    7, 0, 0, 0, 2, 0, 0, 0, 6, //
    0, 6, 0, 0, 0, 0, 2, 8, 0, //
    0, 0, 0, 4, 1, 9, 0, 0, 5, //
    0, 0, 0, 0, 8, 0, 0, 7, 9, //
];

/// Резервна валідна сітка (на випадок неможливого збою генерації).
pub const FALLBACK_SOLUTION: [u8; CELLS] = [
    5, 3, 4, 6, 7, 8, 9, 1, 2, //
    6, 7, 2, 1, 9, 5, 3, 4, 8, //
    1, 9, 8, 3, 4, 2, 5, 6, 7, //
    8, 5, 9, 7, 6, 1, 4, 2, 3, //
    4, 2, 6, 8, 5, 3, 7, 9, 1, //
    7, 1, 3, 9, 2, 4, 8, 5, 6, //
    9, 6, 1, 5, 3, 7, 2, 8, 4, //
    2, 8, 7, 4, 1, 9, 6, 3, 5, //
    3, 4, 5, 2, 8, 6, 1, 7, 9, //
];

fn fallback_solution() -> [u8; CELLS] {
    FALLBACK_SOLUTION
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PlaceResult {
    /// Цифра правильна (збігається з єдиним розв'язком).
    Correct,
    /// Цифра неправильна — залишається на дошці, доки гравець її не зітре.
    Wrong,
    /// Клітинка — підказка головоломки, її не можна змінювати.
    Given,
    /// Позиція вже розв'язана.
    Finished,
    /// Цифра поза діапазоном 1..9.
    Invalid,
}

pub struct SudokuGame {
    pub puzzle: [u8; CELLS],
    pub solution: [u8; CELLS],
    pub cells: [u8; CELLS],
    pub hinted: [bool; CELLS],
    pub difficulty: Difficulty,
    pub mistakes: u32,
    pub hints: u32,
    pub score: u32,
    pub started_move: u32,
    rng: u32,
}

impl SudokuGame {
    pub fn new(seed: u32, difficulty: Difficulty) -> Self {
        let generated = generate(seed, difficulty);
        Self {
            puzzle: generated.puzzle,
            solution: generated.solution,
            cells: generated.puzzle,
            hinted: [false; CELLS],
            difficulty,
            mistakes: 0,
            hints: 0,
            score: 0,
            started_move: 0,
            rng: if seed == 0 { 0xC0FF_EE11 } else { seed },
        }
    }

    /// Нова головоломка тієї ж складності.
    pub fn regenerate(&mut self) {
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 17;
        self.rng ^= self.rng << 5;
        *self = Self::new(self.rng, self.difficulty);
    }

    /// Нова головоломка іншої складності.
    pub fn new_with_difficulty(&mut self, difficulty: Difficulty) {
        self.difficulty = difficulty;
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 17;
        self.rng ^= self.rng << 5;
        *self = Self::new(self.rng, difficulty);
    }

    #[inline]
    pub fn value(&self, sq: usize) -> u8 {
        self.cells[sq]
    }

    #[inline]
    pub fn is_given(&self, sq: usize) -> bool {
        self.puzzle[sq] != 0
    }

    #[inline]
    pub fn is_empty(&self, sq: usize) -> bool {
        self.cells[sq] == 0
    }

    pub fn givens(&self) -> u32 {
        self.puzzle.iter().filter(|&&v| v != 0).count() as u32
    }

    pub fn empty_count(&self) -> u32 {
        self.cells.iter().filter(|&&v| v == 0).count() as u32
    }

    pub fn filled_count(&self) -> u32 {
        (CELLS as u32) - self.empty_count()
    }

    /// Скільком клітинкам ще можна поставити цифру `d` (для «залишилось» на кнопках).
    pub fn remaining_for(&self, d: u8) -> u32 {
        let placed = self.cells.iter().filter(|&&v| v == d).count() as u32;
        9u32.saturating_sub(placed)
    }

    #[inline]
    pub fn conflict_mask(&self) -> u128 {
        conflicts_mask(&self.cells)
    }

    pub fn is_solved(&self) -> bool {
        self.cells == self.solution
    }

    pub fn place(&mut self, sq: usize, digit: u8) -> PlaceResult {
        if sq >= CELLS || !(1..=9).contains(&digit) {
            return PlaceResult::Invalid;
        }
        if self.is_solved() {
            return PlaceResult::Finished;
        }
        if self.is_given(sq) {
            return PlaceResult::Given;
        }

        self.cells[sq] = digit;
        self.started_move = self.started_move.saturating_add(1);

        if digit == self.solution[sq] {
            self.score = self.score.saturating_add(20);
            if self.is_solved() {
                self.score = self.score.saturating_add(self.difficulty.bonus());
            }
            PlaceResult::Correct
        } else {
            self.mistakes = self.mistakes.saturating_add(1);
            self.score = self.score.saturating_sub(10);
            PlaceResult::Wrong
        }
    }

    pub fn clear(&mut self, sq: usize) -> bool {
        if sq >= CELLS || self.is_given(sq) || self.cells[sq] == 0 {
            return false;
        }
        self.cells[sq] = 0;
        self.hinted[sq] = false;
        true
    }

    /// Перша порожня клітинка (або вказана, якщо вона порожня) — куди поставити підказку.
    pub fn hint_square(&self, preferred: Option<usize>) -> Option<usize> {
        if let Some(sq) = preferred
            && sq < CELLS
            && self.cells[sq] == 0
        {
            return Some(sq);
        }
        (0..CELLS).find(|&sq| self.cells[sq] == 0)
    }

    /// Ставить правильну цифру в клітинку (підказка). Повертає заповнену клітинку.
    pub fn apply_hint(&mut self, preferred: Option<usize>) -> Option<usize> {
        let sq = self.hint_square(preferred)?;
        self.cells[sq] = self.solution[sq];
        self.hinted[sq] = true;
        self.hints = self.hints.saturating_add(1);
        self.score = self.score.saturating_sub(15);
        if self.is_solved() {
            self.score = self.score.saturating_add(self.difficulty.bonus() / 2);
        }
        Some(sq)
    }

    /// Один крок автогру для режиму `DEMO`: заповнює одну клітинку.
    pub fn demo_step(&mut self) -> Option<usize> {
        let sq = self.hint_square(None)?;
        self.cells[sq] = self.solution[sq];
        self.hinted[sq] = false;
        Some(sq)
    }

    /// Скидає всі помилки гравця (кнопка `ERASE` із довгим тапом не потрібна — просто перемальовуємо дошку).
    pub fn clear_wrong_entries(&mut self) -> usize {
        let mut cleared = 0usize;
        for sq in 0..CELLS {
            if !self.is_given(sq) && self.cells[sq] != 0 && self.cells[sq] != self.solution[sq] {
                self.cells[sq] = 0;
                self.hinted[sq] = false;
                cleared += 1;
            }
        }
        cleared
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

    // Резервна сітка валідна й не має конфліктів
    check(!has_conflicts(&FALLBACK_SOLUTION));

    // Солвер розв'язує класичну головоломку
    check(!has_conflicts(&TEST_PUZZLE));
    match solve(&TEST_PUZZLE) {
        Some(sol) => {
            check(sol == FALLBACK_SOLUTION);
            check(count_solutions(&TEST_PUZZLE, 2) == 1);
        }
        None => {
            check(false);
            check(false);
        }
    }

    // Генерація: єдиний розв'язок, коректна кількість підказок
    let generated = generate(12345, Difficulty::Medium);
    let givens = generated.puzzle.iter().filter(|&&v| v != 0).count();
    check(givens <= Difficulty::Medium.givens());
    check(count_solutions(&generated.puzzle, 2) == 1);
    check(!has_conflicts(&generated.puzzle));
    check(solve(&generated.puzzle) == Some(generated.solution));

    (run, failed)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn all_valid(cells: &[u8; CELLS]) -> bool {
        if cells.iter().any(|&v| !(1..=9).contains(&v)) {
            return false;
        }
        for i in 0..9 {
            let mut row = [0u8; 10];
            let mut col = [0u8; 10];
            for j in 0..9 {
                row[cells[i * 9 + j] as usize] += 1;
                col[cells[j * 9 + i] as usize] += 1;
            }
            if row[1..].iter().any(|&c| c != 1) || col[1..].iter().any(|&c| c != 1) {
                return false;
            }
        }
        for b in 0..9 {
            let mut cnt = [0u8; 10];
            let (br, bc) = ((b / 3) * 3, (b % 3) * 3);
            for dr in 0..3 {
                for dc in 0..3 {
                    cnt[cells[(br + dr) * 9 + bc + dc] as usize] += 1;
                }
            }
            if cnt[1..].iter().any(|&c| c != 1) {
                return false;
            }
        }
        true
    }

    #[test]
    fn selftest_passes() {
        let (run, failed) = selftest();
        assert_eq!(failed, 0, "selftest failed ({}/{})", failed, run);
        assert!(run >= 8);
    }

    #[test]
    fn solver_and_solution_grid_are_valid() {
        assert!(all_valid(&FALLBACK_SOLUTION));
        assert!(!has_conflicts(&FALLBACK_SOLUTION));
        assert_eq!(solve(&TEST_PUZZLE), Some(FALLBACK_SOLUTION));

        // Головоломка з кількома розв'язками (порожня сітка) — лічильник зупиняється на ліміті
        let empty = [0u8; CELLS];
        assert_eq!(count_solutions(&empty, 2), 2);
        assert!(solve(&empty).is_some());
        assert!(all_valid(&solve(&empty).unwrap()));
    }

    #[test]
    fn generation_is_unique_and_matches_difficulty() {
        for (seed, diff) in [
            (1u32, Difficulty::Easy),
            (7u32, Difficulty::Medium),
            (99u32, Difficulty::Hard),
        ] {
            let g = generate(seed, diff);
            assert!(all_valid(&g.solution));
            assert!(all_valid(&g.puzzle) || g.puzzle.iter().any(|&v| v == 0));
            assert!(!has_conflicts(&g.puzzle));
            assert_eq!(count_solutions(&g.puzzle, 2), 1, "seed {seed} must be unique");
            assert_eq!(solve(&g.puzzle), Some(g.solution));
            let givens = g.puzzle.iter().filter(|&&v| v != 0).count();
            assert!(
                givens <= diff.givens(),
                "seed {seed}: {givens} givens > {}",
                diff.givens()
            );
            // Підказки — підмножина розв'язку
            for sq in 0..CELLS {
                if g.puzzle[sq] != 0 {
                    assert_eq!(g.puzzle[sq], g.solution[sq]);
                }
            }
        }
    }

    #[test]
    fn generation_differs_between_seeds() {
        let a = generate(11, Difficulty::Easy);
        let b = generate(12, Difficulty::Easy);
        assert_ne!(a.solution, b.solution);
    }

    #[test]
    fn rules_place_clear_hint_and_conflicts() {
        let mut g = SudokuGame::new(2026, Difficulty::Easy);
        assert!(!g.is_solved());
        assert_eq!(g.mistakes, 0);

        let empty = (0..CELLS).find(|&sq| g.is_empty(sq)).unwrap();
        let correct = g.solution[empty];
        assert_eq!(g.place(empty, correct), PlaceResult::Correct);
        assert_eq!(g.value(empty), correct);
        assert!(g.score >= 20);
        assert!(g.clear(empty));
        assert!(g.is_empty(empty));

        let wrong = if correct == 9 { 1 } else { correct + 1 };
        assert_eq!(g.place(empty, wrong), PlaceResult::Wrong);
        assert_eq!(g.mistakes, 1);
        assert_eq!(g.clear_wrong_entries(), 1);
        assert!(g.is_empty(empty));

        // Підказки не змінюються
        let given = (0..CELLS).find(|&sq| g.is_given(sq)).unwrap();
        assert_eq!(g.place(given, 5), PlaceResult::Given);

        let sq = g.apply_hint(None).unwrap();
        assert!(g.hinted[sq]);
        assert_eq!(g.value(sq), g.solution[sq]);
        assert_eq!(g.hints, 1);
        assert_eq!(g.place(CELLS, 1), PlaceResult::Invalid);
    }

    #[test]
    fn hint_prefers_selected_empty_cell_and_finishes_puzzle() {
        let mut g = SudokuGame::new(424242, Difficulty::Hard);
        let target = (0..CELLS).find(|&sq| g.is_empty(sq)).unwrap();
        assert_eq!(g.apply_hint(Some(target)), Some(target));

        // Дозаповнюємо решту підказками → задача розв'язана, нараховано бонус
        while g.empty_count() > 0 {
            g.apply_hint(None).unwrap();
        }
        assert!(g.is_solved());
        assert!(g.score > 0);
    }

    #[test]
    fn conflicts_detected_in_row_col_and_box() {
        let mut cells = [0u8; CELLS];
        cells[0] = 5;
        cells[1] = 5; // той самий рядок
        assert!(has_conflicts(&cells));

        let mut cells = [0u8; CELLS];
        cells[0] = 7;
        cells[9] = 7; // той самий стовпець
        assert!(has_conflicts(&cells));

        let mut cells = [0u8; CELLS];
        cells[0] = 3;
        cells[10] = 3; // той самий квадрат 3×3
        assert!(has_conflicts(&cells));

        let mut cells = [0u8; CELLS];
        cells[0] = 3;
        cells[9] = 4;
        cells[1] = 0;
        assert!(!has_conflicts(&cells));
        assert_eq!(candidates(&cells, 1) & digit_bit(3), 0);
        assert_eq!(candidates(&cells, 1) & digit_bit(4), 0);
        assert_ne!(candidates(&cells, 1) & digit_bit(1), 0);
        assert_eq!(candidates(&cells, 5) & digit_bit(1), digit_bit(1));
    }

    #[test]
    fn demo_mode_solves_whole_board() {
        let mut g = SudokuGame::new(777, Difficulty::Medium);
        let mut steps = 0;
        while let Some(_sq) = g.demo_step() {
            steps += 1;
            assert!(steps <= CELLS);
        }
        assert!(g.is_solved());
        assert_eq!(g.mistakes, 0);
    }
}

