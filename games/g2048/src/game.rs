//! Правила гри «2048» — без `std`, без залежностей, без динамічної пам'яті.
//!
//! Навмисно окремий файл: його можна зібрати й **на хості** (див. README):
//! ```text
//! rustc --edition 2021 --test src/game.rs -o /tmp/game_tests && /tmp/game_tests
//! ```
//! Тому вся логіка перевіряється звичайними `#[test]`, а на платі той самий
//! `selftest_checks()` проганяється при старті й друкує кількість перевірок.

pub const N: usize = 4;
pub const CELLS: usize = N * N;
pub const WIN: u32 = 2048;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Dir {
    Left,
    Right,
    Up,
    Down,
}

/// Одне переміщення плитки — для анімації (звідки, куди, яке значення, чи злиття).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Step {
    pub from: u8,
    pub to: u8,
    pub value: u32,
    pub merged: bool,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Steps {
    n: u8,
    s: [Step; CELLS],
}

impl Steps {
    pub fn len(&self) -> usize {
        self.n as usize
    }

    pub fn is_empty(&self) -> bool {
        self.n == 0
    }

    pub fn get(&self, i: usize) -> &Step {
        &self.s[i]
    }

    pub fn iter(&self) -> impl Iterator<Item = &Step> {
        self.s[..self.n as usize].iter()
    }

    fn push(&mut self, st: Step) {
        if (self.n as usize) < CELLS {
            self.s[self.n as usize] = st;
            self.n += 1;
        }
    }
}

#[derive(Clone, Copy)]
pub struct Game {
    pub cell: [u32; CELLS],
    pub score: u32,
    pub moves: u32,
    rng: u32,
}

impl Game {
    pub fn new(seed: u32) -> Self {
        let mut g = Self {
            cell: [0; CELLS],
            score: 0,
            moves: 0,
            rng: if seed == 0 { 0x1234_5677 } else { seed },
        };
        g.spawn();
        g.spawn();
        g
    }

    fn rng_next(&mut self) -> u32 {
        let mut x = self.rng;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.rng = x;
        x
    }

    /// Поява плитки: 90 % — 2, 10 % — 4. `None`, якщо вільного місця немає.
    pub fn spawn(&mut self) -> Option<(u8, u32)> {
        let mut empty = [0u8; CELLS];
        let mut n = 0;
        for (i, v) in self.cell.iter().enumerate() {
            if *v == 0 {
                empty[n] = i as u8;
                n += 1;
            }
        }
        if n == 0 {
            return None;
        }
        let r = self.rng_next();
        let at = empty[(r as usize) % n];
        let value = if (r >> 8).is_multiple_of(10) { 4 } else { 2 };
        self.cell[at as usize] = value;
        Some((at, value))
    }

    /// Хід. `None`, якщо нічого не змінилося (поле лишається як було).
    pub fn try_move(&mut self, dir: Dir) -> Option<Steps> {
        let old = self.cell;
        let mut new = [0u32; CELLS];
        let mut st = Steps::default();
        let mut changed = false;
        let mut gained = 0u32;

        for line in 0..N {
            let mut src = [0usize; N];
            let mut val = [0u32; N];
            let mut n = 0;

            for pos in 0..N {
                let (r, c) = order(dir, line, pos);
                let i = idx(r, c);
                if old[i] != 0 {
                    src[n] = i;
                    val[n] = old[i];
                    n += 1;
                }
            }

            let mut t = 0;
            let mut i = 0;
            while i < n {
                let (r, c) = order(dir, line, t);
                let to = idx(r, c);
                let merge = i + 1 < n && val[i] == val[i + 1];

                if merge {
                    st.push(Step { from: src[i] as u8, to: to as u8, value: val[i], merged: false });
                    st.push(Step { from: src[i + 1] as u8, to: to as u8, value: val[i], merged: true });
                    new[to] = val[i] * 2;
                    gained += val[i] * 2;
                    changed = true;
                    i += 2;
                } else {
                    st.push(Step { from: src[i] as u8, to: to as u8, value: val[i], merged: false });
                    new[to] = val[i];
                    if src[i] != to {
                        changed = true;
                    }
                    i += 1;
                }
                t += 1;
            }
        }

        if !changed {
            return None;
        }
        self.cell = new;
        self.score += gained;
        self.moves += 1;
        Some(st)
    }

    pub fn can_move(&self) -> bool {
        if self.cell.contains(&0) {
            return true;
        }
        for r in 0..N {
            for c in 0..N {
                let v = self.cell[idx(r, c)];
                if c + 1 < N && self.cell[idx(r, c + 1)] == v {
                    return true;
                }
                if r + 1 < N && self.cell[idx(r + 1, c)] == v {
                    return true;
                }
            }
        }
        false
    }

    pub fn won(&self) -> bool {
        self.cell.iter().any(|v| *v >= WIN)
    }

    pub fn max_tile(&self) -> u32 {
        self.cell.iter().copied().max().unwrap_or(0)
    }
}

fn idx(r: usize, c: usize) -> usize {
    r * N + c
}

/// Під індексом `pos` (0 = «до краю, куди рухаємось») повертає рядок/стовпець.
fn order(dir: Dir, line: usize, pos: usize) -> (usize, usize) {
    match dir {
        Dir::Left => (line, pos),
        Dir::Right => (line, N - 1 - pos),
        Dir::Up => (pos, line),
        Dir::Down => (N - 1 - pos, line),
    }
}

/* ---------- самотести (спільні для плати й хоста) ---------- */

/// Повертає (скільки перевірок виконано, скільки провалено).
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

    /* побудувати поле з першого рядка = cells, решта нулі */
    fn row(cells: [u32; N]) -> Game {
        let mut g = Game::new(1);
        g.cell = [0; CELLS];
        g.cell[..N].copy_from_slice(&cells);
        g.score = 0;
        g.moves = 0;
        g
    }

    /* --- 1. старт: рівно дві плитки, тільки 2/4, рахунок 0 --- */
    for seed in 1..20u32 {
        let g = Game::new(seed);
        let n = g.cell.iter().filter(|v| **v != 0).count();
        chk!(n == 2);
        chk!(g.cell.iter().all(|v| *v == 0 || *v == 2 || *v == 4));
        chk!(g.score == 0 && g.moves == 0);
    }
    let a = Game::new(777);
    let b = Game::new(777);
    chk!(a.cell == b.cell); // детермінованість за seed

    /* --- 2. злиття й рахунок --- */
    let mut g = row([2, 2, 0, 0]);
    let st = g.try_move(Dir::Left).unwrap();
    chk!(g.cell[..4] == [4, 0, 0, 0]);
    chk!(g.score == 4);
    chk!(st.len() == 2 && st.get(0).to == 0 && st.get(1).to == 0);
    chk!(!st.get(0).merged && st.get(1).merged);

    let mut g = row([2, 2, 2, 2]);
    g.try_move(Dir::Left);
    chk!(g.cell[..4] == [4, 4, 0, 0]);
    chk!(g.score == 8);

    let mut g = row([4, 4, 4, 0]);
    g.try_move(Dir::Left);
    chk!(g.cell[..4] == [8, 4, 0, 0]);

    let mut g = row([4, 2, 2, 0]);
    g.try_move(Dir::Left);
    chk!(g.cell[..4] == [4, 4, 0, 0]); // не 8!

    let mut g = row([2, 0, 2, 4]);
    g.try_move(Dir::Left);
    chk!(g.cell[..4] == [4, 4, 0, 0]);

    /* --- 3. напрямки --- */
    let mut g = row([2, 0, 2, 0]);
    g.try_move(Dir::Right);
    chk!(g.cell[..4] == [0, 0, 0, 4]);

    let mut g = Game::new(1);
    g.cell = [0; CELLS];
    g.cell[1] = 2;
    g.cell[idx(1, 1)] = 2;
    g.cell[idx(2, 1)] = 4;
    g.score = 0;
    g.try_move(Dir::Up);
    chk!(g.cell[1] == 4 && g.cell[idx(1, 1)] == 4 && g.cell[idx(2, 1)] == 0);

    let mut g = Game::new(1);
    g.cell = [0; CELLS];
    g.cell[1] = 2;
    g.cell[idx(1, 1)] = 2;
    g.cell[idx(2, 1)] = 4;
    g.score = 0;
    g.try_move(Dir::Down);
    chk!(g.cell[idx(2, 1)] == 4 && g.cell[idx(3, 1)] == 4 && g.cell[1] == 0);

    /* --- 4. «нічого не змінилось» не псує поле --- */
    let full = [
        2, 4, 2, 4, //
        4, 2, 4, 2, //
        2, 4, 2, 4, //
        4, 2, 4, 2,
    ];
    for dir in [Dir::Left, Dir::Right, Dir::Up, Dir::Down] {
        let mut g = Game::new(1);
        g.cell = full;
        g.score = 0;
        g.moves = 0;
        chk!(g.try_move(dir).is_none());
        chk!(g.cell == full);
        chk!(g.score == 0 && g.moves == 0);
    }
    let mut g = Game::new(1);
    g.cell = [2, 2, 4, 8, 4, 8, 16, 32, 2, 4, 2, 4, 4, 2, 4, 2];
    chk!(g.can_move()); // є пара 2,2 у першому рядку
    let mut g = Game::new(1);
    g.cell = full;
    chk!(!g.can_move());

    /* --- 5. перемога й поява плитки --- */
    let mut g = Game::new(1);
    g.cell = [0; CELLS];
    g.cell[0] = 1024;
    g.cell[1] = 1024;
    g.score = 0;
    chk!(!g.won());
    g.try_move(Dir::Left);
    chk!(g.won());
    chk!(g.max_tile() == 2048);
    chk!(g.score == 2048);

    let mut g = Game::new(1);
    g.cell = full;
    chk!(g.spawn().is_none());
    chk!(g.cell == full);

    let mut g = Game::new(1);
    g.cell = [0; CELLS];
    g.cell[..15].copy_from_slice(&full[..15]);
    let spawn = g.spawn();
    chk!(spawn.is_some());
    if let Some((at, v)) = spawn {
        chk!(at == 15);
        chk!(v == 2 || v == 4);
        chk!(g.cell[15] == v);
    }

    /* --- 6. випадкова гра: інваріанти --- */
    let mut g = Game::new(2024);
    for i in 0..2000u32 {
        let dir = match (i * 7 + (g.rng_next() >> 3)) % 4 {
            0 => Dir::Left,
            1 => Dir::Right,
            2 => Dir::Up,
            _ => Dir::Down,
        };
        if g.try_move(dir).is_some() {
            g.spawn();
        }
        let mut nonempty = 0;
        for v in g.cell.iter() {
            if *v != 0 {
                chk!(*v >= 2 && (*v & (*v - 1)) == 0); // степінь двійки
                nonempty += 1;
            }
        }
        chk!((1..=CELLS).contains(&nonempty));
        if !g.can_move() {
            break;
        }
    }

    (run, failed)
}

#[cfg(test)]
mod tests {
    #[test]
    fn selftest_passes() {
        let (run, failed) = super::selftest_checks();
        assert!(run > 300, "занадто мало перевірок: {run}");
        assert_eq!(failed, 0, "провалено {failed} з {run} перевірок");
    }

    #[test]
    fn merge_twice_in_a_row_is_not_allowed() {
        let mut g = super::Game::new(1);
        g.cell = [0; super::CELLS];
        g.cell[0] = 2;
        g.cell[1] = 2;
        g.cell[2] = 4;
        g.score = 0;
        g.try_move(super::Dir::Left);
        // [2,2,4] -> [4,4], а не [8]
        assert_eq!(g.cell[..3], [4, 4, 0]);
        assert_eq!(g.score, 4);
    }
}
