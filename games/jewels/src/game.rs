//! Логіка match-3 (у стилі Bejeweled) — без `std`, без залежностей, без динамічної пам'яті.
//!
//! Поле 8×8, шість видів каменів. Гравець міняє два сусідні камені; якщо після цього
//! утворюється лінія з 3+ однакових — вони зникають, решта падає вниз, зверху досипаються
//! нові (каскад із множником).
//!
//! Файл навмисно не залежить від `esp-hal`, тому його можна перевірити на хості:
//! ```text
//! rustc --edition 2021 --test src/game.rs -o /tmp/jewels_tests && /tmp/jewels_tests
//! ```

pub const N: usize = 8;
pub const CELLS: usize = N * N;
pub const KINDS: u8 = 6;
/// Очки за один камінь у лінії (далі множиться на номер каскаду).
pub const POINT_PER_GEM: u32 = 30;

pub const fn idx(r: usize, c: usize) -> usize {
    r * N + c
}

/// Один камінь, що падає (або щойно з'явився над полем: `from_r` < 0).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct FallMove {
    pub value: u8,
    pub from_r: i8,
    pub from_c: i8,
    pub to_r: i8,
    pub to_c: i8,
}

#[derive(Clone, Copy, Debug)]
pub struct Fall {
    pub n: u8,
    pub mv: [FallMove; 128],
}

impl Fall {
    pub fn iter(&self) -> impl Iterator<Item = &FallMove> {
        self.mv[..self.n as usize].iter()
    }
    fn push(&mut self, m: FallMove) {
        if (self.n as usize) < self.mv.len() {
            self.mv[self.n as usize] = m;
            self.n += 1;
        }
    }
}

#[derive(Clone, Copy)]
pub struct Board {
    /// 0 = порожньо (тільки під час каскаду), 1..=KINDS — вид каменя.
    pub cell: [u8; CELLS],
    rng: u32,
}

impl Board {
    /// Нове поле: без готових ліній і з гарантованим хоча б одним ходом.
    pub fn new(seed: u32) -> Self {
        let mut b = Self {
            cell: [0; CELLS],
            rng: if seed == 0 { 0x1234_5677 } else { seed },
        };
        loop {
            for r in 0..N {
                for c in 0..N {
                    let v = loop {
                        let v = 1 + (b.next_rng() % KINDS as u32) as u8;
                        let same_h = c >= 2
                            && b.cell[idx(r, c - 1)] == v
                            && b.cell[idx(r, c - 2)] == v;
                        let same_v = r >= 2
                            && b.cell[idx(r - 1, c)] == v
                            && b.cell[idx(r - 2, c)] == v;
                        if !same_h && !same_v {
                            break v;
                        }
                    };
                    b.cell[idx(r, c)] = v;
                }
            }
            if !b.has_match() && b.has_move() {
                return b;
            }
        }
    }

    fn next_rng(&mut self) -> u32 {
        let mut x = self.rng;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.rng = x;
        x
    }

    pub fn random_u32(&mut self) -> u32 {
        self.next_rng()
    }

    pub fn at(&self, r: usize, c: usize) -> u8 {
        self.cell[idx(r, c)]
    }

    pub fn swap_cells(&mut self, a: usize, b: usize) {
        self.cell.swap(a, b);
    }

    pub fn adjacent(a: usize, b: usize) -> bool {
        let (r1, c1) = (a / N, a % N);
        let (r2, c2) = (b / N, b % N);
        (r1 == r2 && c1.abs_diff(c2) == 1) || (c1 == c2 && r1.abs_diff(r2) == 1)
    }

    /// Маска клітинок, що входять у лінії з 3+ однакових.
    pub fn find_matches(&self) -> [bool; CELLS] {
        let mut m = [false; CELLS];
        for r in 0..N {
            let mut c = 0;
            while c < N {
                let v = self.cell[idx(r, c)];
                let mut len = 1;
                while c + len < N && self.cell[idx(r, c + len)] == v && v != 0 {
                    len += 1;
                }
                if v != 0 && len >= 3 {
                    for k in 0..len {
                        m[idx(r, c + k)] = true;
                    }
                }
                c += len;
            }
        }
        for c in 0..N {
            let mut r = 0;
            while r < N {
                let v = self.cell[idx(r, c)];
                let mut len = 1;
                while r + len < N && self.cell[idx(r + len, c)] == v && v != 0 {
                    len += 1;
                }
                if v != 0 && len >= 3 {
                    for k in 0..len {
                        m[idx(r + k, c)] = true;
                    }
                }
                r += len;
            }
        }
        m
    }

    pub fn has_match(&self) -> bool {
        self.find_matches().iter().any(|x| *x)
    }

    /// Прибрати помічені камені; повертає, скільки прибрано.
    pub fn clear_matches(&mut self, mask: &[bool; CELLS]) -> u32 {
        let mut n = 0;
        for (cell, &marked) in self.cell.iter_mut().zip(mask.iter()) {
            if marked && *cell != 0 {
                *cell = 0;
                n += 1;
            }
        }
        n
    }

    /// Гравітація + досипання нових каменів. Після цього порожніх клітинок немає.
    pub fn collapse(&mut self) -> Fall {
        let mut fall = Fall { n: 0, mv: [FallMove::default(); 128] };
        for c in 0..N {
            /* знизу вгору збираємо те, що лишилося */
            let mut keep: [(u8, usize); N] = [(0, 0); N];
            let mut n = 0;
            for r in (0..N).rev() {
                let v = self.cell[idx(r, c)];
                if v != 0 {
                    keep[n] = (v, r);
                    n += 1;
                }
            }
            /* розставляємо знизу */
            for (i, (v, from_r)) in keep[..n].iter().enumerate() {
                let to_r = N - 1 - i;
                self.cell[idx(to_r, c)] = *v;
                if *from_r != to_r {
                    fall.push(FallMove {
                        value: *v,
                        from_r: *from_r as i8,
                        from_c: c as i8,
                        to_r: to_r as i8,
                        to_c: c as i8,
                    });
                }
            }
            /* порожні зверху — нові камені, «падають» з-за верхнього краю */
            let empty = N - n;
            for i in 0..empty {
                let to_r = empty - 1 - i;
                let v = 1 + (self.next_rng() % KINDS as u32) as u8;
                self.cell[idx(to_r, c)] = v;
                fall.push(FallMove {
                    value: v,
                    from_r: -(i as i8 + 1),
                    from_c: c as i8,
                    to_r: to_r as i8,
                    to_c: c as i8,
                });
            }
        }
        fall
    }

    /// Чи є хоч один хід, що дає лінію.
    pub fn find_valid_move(&self) -> Option<(usize, usize)> {
        for r in 0..N {
            for c in 0..N {
                for (dr, dc) in [(0usize, 1usize), (1, 0)] {
                    if r + dr >= N || c + dc >= N {
                        continue;
                    }
                    let (a, b) = (idx(r, c), idx(r + dr, c + dc));
                    let mut t = *self;
                    t.cell.swap(a, b);
                    if t.has_match() {
                        return Some((a, b));
                    }
                }
            }
        }
        None
    }

    pub fn has_move(&self) -> bool {
        self.find_valid_move().is_some()
    }

    /// Перемішати наявні камені (без зміни набору) так, щоб був хід і не було готових ліній.
    /// Якщо не вдалося за 50 спроб — згенерувати нове поле.
    pub fn shuffle(&mut self) {
        let mut pool = self.cell;
        for _ in 0..50 {
            /* Fisher–Yates */
            for i in (1..CELLS).rev() {
                let j = (self.next_rng() as usize) % (i + 1);
                pool.swap(i, j);
            }
            self.cell = pool;
            if !self.has_match() && self.has_move() {
                return;
            }
        }
        *self = Board::new(self.next_rng());
    }

    /// Очки за прибрані камені з урахуванням каскаду (combo = 1, 2, 3, …).
    pub fn points(cleared: u32, combo: u32) -> u32 {
        cleared * POINT_PER_GEM * combo
    }
}

/* ---------- самотести (спільні для плати й хоста) ---------- */

/// Повертає (скільки перевірок виконано, скільки провалено).
#[allow(clippy::needless_range_loop)] // у тестах індекси читаються наочніше
pub fn selftest_checks() -> (u32, u32) {
    let mut run = 0u32;
    let mut failed = 0u32;
    macro_rules! chk {
        ($cond:expr) => {{
            run += 1;
            if !($cond) {
                failed += 1;
            }
        }};
    }

    fn from(rows: [[u8; N]; N]) -> Board {
        let mut b = Board::new(1);
        for r in 0..N {
            for c in 0..N {
                b.cell[idx(r, c)] = rows[r][c];
            }
        }
        b
    }

    /* --- 1. нове поле: без ліній, усі камені 1..=6, хід є --- */
    for seed in 1..40u32 {
        let b = Board::new(seed);
        chk!(!b.has_match());
        chk!(b.cell.iter().all(|v| *v >= 1 && *v <= KINDS));
        chk!(b.has_move());
    }
    chk!(Board::new(777).cell == Board::new(777).cell); // детермінованість

    /* --- 2. пошук ліній --- */
    let mut rows = [[1u8; N]; N];
    for r in 0..N {
        for c in 0..N {
            rows[r][c] = 1 + ((r + c) % 6) as u8; // «латинський квадрат»: без ліній
        }
    }
    let latin = from(rows);
    chk!(!latin.has_match());
    chk!(!latin.has_move()); // у латинському квадраті ходів немає — гарний «мертвий» тест

    let mut rows = [[1u8; N]; N];
    for r in 0..N {
        for c in 0..N {
            rows[r][c] = 1 + ((r + c) % 6) as u8;
        }
    }
    rows[3][2] = rows[3][1];
    rows[3][3] = rows[3][1];
    let b = from(rows);
    let m = b.find_matches();
    chk!(m[idx(3, 1)] && m[idx(3, 2)] && m[idx(3, 3)]);
    chk!(m.iter().filter(|x| **x).count() >= 3);

    /* вертикальна лінія */
    let mut rows = [[1u8; N]; N];
    for r in 0..N {
        for c in 0..N {
            rows[r][c] = 1 + ((r + c) % 6) as u8;
        }
    }
    rows[2][5] = rows[1][5];
    rows[3][5] = rows[1][5];
    let b = from(rows);
    let m = b.find_matches();
    chk!(m[idx(1, 5)] && m[idx(2, 5)] && m[idx(3, 5)]);

    /* --- 3. прибирання + очки --- */
    let mut b = from([[1; N]; N]); // усе однакове — 8 ліній по 8
    let m = b.find_matches();
    let cleared = b.clear_matches(&m);
    chk!(cleared == 64);
    chk!(b.cell.iter().all(|v| *v == 0));
    chk!(Board::points(3, 1) == 3 * POINT_PER_GEM);
    chk!(Board::points(4, 2) == 8 * POINT_PER_GEM);
    chk!(Board::points(5, 3) > Board::points(5, 2));

    /* --- 4. гравітація й досипання --- */
    let mut rows = [[1u8; N]; N];
    for r in 0..N {
        for c in 0..N {
            rows[r][c] = 1 + ((r + c) % 6) as u8;
        }
    }
    let mut b = from(rows);
    let mut orig = [0u8; N];
    for r in 0..N {
        orig[r] = b.cell[idx(r, 0)];
    }
    /* прибираємо два камені в стовпці 0 (верхній і середній) */
    b.cell[idx(0, 0)] = 0;
    b.cell[idx(2, 0)] = 0;
    let fall = b.collapse();
    chk!(b.cell.iter().all(|v| *v >= 1 && *v <= KINDS)); // порожніх не лишилось
    chk!(fall.n as usize >= 2);
    /* уцілілі камені зберегли порядок і з'їхали вниз */
    let mut expect = [0u8; N];
    let mut n = 0;
    for (r, v) in orig.iter().enumerate() {
        if r != 0 && r != 2 {
            expect[n] = *v;
            n += 1;
        }
    }
    chk!(n == N - 2);
    for k in 0..n {
        chk!(b.cell[idx(N - n + k, 0)] == expect[k]);
    }
    /* кожен рух «звідки → куди» відповідає значенню в цілі */
    for m in fall.iter() {
        if m.from_r >= 0 {
            chk!(m.value == b.cell[idx(m.to_r as usize, m.to_c as usize)]);
            chk!(m.from_r as usize != m.to_r as usize);
        } else {
            chk!(m.from_r < 0);
            chk!(m.value == b.cell[idx(m.to_r as usize, m.to_c as usize)]);
        }
    }

    /* --- 5. swap-хід --- */
    let mut rows = [[1u8; N]; N];
    for r in 0..N {
        for c in 0..N {
            rows[r][c] = 1 + ((r + c) % 6) as u8;
        }
    }
    rows[4][0] = rows[0][0]; // потім поміняємо з (0,0): отримаємо 2 однакових…
    let mut b = from(rows);
    let before = b.cell;
    let a = idx(0, 0);
    let bb = idx(1, 0);
    b.swap_cells(a, bb);
    /* у латинському квадраті обмін двох сусідніх не дає лінії — повертаємо назад */
    if !b.has_match() {
        b.swap_cells(a, bb);
        chk!(b.cell == before);
    }

    /* --- 6. випадкова гра: каскади тримають інваріанти --- */
    let mut b = Board::new(2024);
    let mut total_cleared = 0u32;
    for _ in 0..300 {
        let Some((a, c)) = b.find_valid_move() else {
            b.shuffle();
            chk!(b.has_move());
            continue;
        };
        b.swap_cells(a, c);
        chk!(b.has_match());
        let mut combo = 1u32;
        loop {
            let m = b.find_matches();
            if !m.iter().any(|x| *x) {
                break;
            }
            let cleared = b.clear_matches(&m);
            chk!(cleared >= 3);
            total_cleared += cleared;
            let _ = Board::points(cleared, combo);
            b.collapse();
            chk!(b.cell.iter().all(|v| *v >= 1 && *v <= KINDS));
            chk!(b.cell.iter().all(|v| *v <= KINDS));
            combo += 1;
            chk!(combo < 200); // захист від нескінченного каскаду
        }
        chk!(!b.has_match());
    }
    chk!(total_cleared > 300);

    /* --- 7. shuffle зберігає набір каменів --- */
    let mut b = Board::new(99);
    let mut before = b.cell;
    before.sort_unstable();
    b.shuffle();
    let mut after = b.cell;
    after.sort_unstable();
    chk!(before == after);
    chk!(!b.has_match() && b.has_move());

    (run, failed)
}

#[cfg(test)]
mod tests {
    #[test]
    fn selftest_passes() {
        let (run, failed) = super::selftest_checks();
        assert!(run > 200, "занадто мало перевірок: {run}");
        assert_eq!(failed, 0, "провалено {failed} з {run} перевірок");
    }

    #[test]
    fn latin_square_is_dead_but_playable_after_shuffle() {
        let mut b = super::Board::new(5);
        for r in 0..super::N {
            for c in 0..super::N {
                b.cell[super::idx(r, c)] = 1 + ((r + c) % 6) as u8;
            }
        }
        assert!(!b.has_match());
        assert!(!b.has_move());
        b.shuffle();
        assert!(b.has_move());
        assert!(!b.has_match());
    }
}
