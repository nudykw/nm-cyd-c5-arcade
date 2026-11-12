//! Логіка та мінімакс-ШІ гри «Хрестики-нулики» (без залежностей від заліза, тестується на хості).

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Cell {
    Empty,
    X,
    O,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum AiMode {
    Hard, // Непереможний мінімакс
    Easy, // Простий ШІ (дає шанс обіграти себе вилкою / неуважністю)
    Off,  // Два гравці
}

impl AiMode {
    pub fn next(self) -> Self {
        match self {
            Self::Hard => Self::Easy,
            Self::Easy => Self::Off,
            Self::Off => Self::Hard,
        }
    }

    pub fn is_ai(self) -> bool {
        !matches!(self, Self::Off)
    }
}

pub const LINES: [[usize; 3]; 8] = [
    [0, 1, 2],
    [3, 4, 5],
    [6, 7, 8],
    [0, 3, 6],
    [1, 4, 7],
    [2, 5, 8],
    [0, 4, 8],
    [2, 4, 6],
];

pub struct Game {
    pub b: [Cell; 9],
    pub turn_x: bool,
    pub over: bool,
    pub ai_mode: AiMode,
    pub wins_x: u32,
    pub wins_o: u32,
    pub draws: u32,
    rng: u32,
}

impl Game {
    pub const fn new(seed: u32) -> Self {
        Self {
            b: [Cell::Empty; 9],
            turn_x: true,
            over: false,
            ai_mode: AiMode::Hard,
            wins_x: 0,
            wins_o: 0,
            draws: 0,
            rng: if seed == 0 { 0xA5A5_5A5A } else { seed },
        }
    }

    pub fn next_rand(&mut self) -> u32 {
        let mut x = self.rng;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.rng = x;
        x
    }

    pub fn reset(&mut self) {
        self.b = [Cell::Empty; 9];
        self.turn_x = true;
        self.over = false;
    }

    /// 0 — гра триває, 1 — переміг X, 2 — переміг O, 3 — нічия.
    pub fn winner(&self) -> u8 {
        winner_of(&self.b)
    }

    pub fn winning_line(&self) -> Option<[usize; 3]> {
        for l in LINES {
            let a = self.b[l[0]];
            if a != Cell::Empty && a == self.b[l[1]] && a == self.b[l[2]] {
                return Some(l);
            }
        }
        None
    }

    pub fn ai_choose_move(&mut self) -> Option<usize> {
        match self.ai_mode {
            AiMode::Off => None,
            AiMode::Hard => best_move(&self.b, 2),
            AiMode::Easy => {
                // 1. Якщо O може виграти в 1 хід — завершує.
                for i in 0..9 {
                    if self.b[i] == Cell::Empty {
                        let mut t = self.b;
                        t[i] = Cell::O;
                        if winner_of(&t) == 2 {
                            return Some(i);
                        }
                    }
                }
                // 2. Із імовірністю 50% блокує пряму погрозу X, інакше робить випадковий хід.
                if (self.next_rand() & 1) == 0 {
                    for i in 0..9 {
                        if self.b[i] == Cell::Empty {
                            let mut t = self.b;
                            t[i] = Cell::X;
                            if winner_of(&t) == 1 {
                                return Some(i);
                            }
                        }
                    }
                }
                let mut empties = [0usize; 9];
                let mut n = 0usize;
                for i in 0..9 {
                    if self.b[i] == Cell::Empty {
                        empties[n] = i;
                        n += 1;
                    }
                }
                if n == 0 {
                    None
                } else {
                    Some(empties[(self.next_rand() as usize) % n])
                }
            }
        }
    }
}

pub fn winner_of(b: &[Cell; 9]) -> u8 {
    for l in LINES {
        let a = b[l[0]];
        if a != Cell::Empty && a == b[l[1]] && a == b[l[2]] {
            return if a == Cell::X { 1 } else { 2 };
        }
    }
    if b.iter().all(|&c| c != Cell::Empty) {
        3
    } else {
        0
    }
}

fn mm(b: &mut [Cell; 9], turn: u8, me: u8, depth: i32) -> i32 {
    match winner_of(b) {
        w if w == me => return 10 - depth,
        3 => return 0,
        1 | 2 => return depth - 10,
        _ => {}
    }
    let mut best = if turn == me { -1000 } else { 1000 };
    for i in 0..9 {
        if b[i] != Cell::Empty {
            continue;
        }
        b[i] = if turn == 1 { Cell::X } else { Cell::O };
        let v = mm(b, 3 - turn, me, depth + 1);
        b[i] = Cell::Empty;
        if turn == me {
            best = best.max(v);
        } else {
            best = best.min(v);
        }
    }
    best
}

fn move_rank(i: usize) -> u8 {
    if i == 4 {
        0
    } else if i % 2 == 0 {
        1
    } else {
        2
    }
}

pub fn best_move(b: &[Cell; 9], me: u8) -> Option<usize> {
    let mut t = *b;
    let mut best = -1000;
    let mut cand = [0usize; 9];
    let mut ncand = 0usize;
    for i in 0..9 {
        if t[i] != Cell::Empty {
            continue;
        }
        t[i] = if me == 1 { Cell::X } else { Cell::O };
        let v = mm(&mut t, 3 - me, me, 1);
        t[i] = Cell::Empty;
        if v > best {
            best = v;
            ncand = 0;
            cand[ncand] = i;
            ncand += 1;
        } else if v == best {
            cand[ncand] = i;
            ncand += 1;
        }
    }
    if ncand == 0 {
        return None;
    }
    let mut best_rank = 3;
    for &c in &cand[..ncand] {
        best_rank = best_rank.min(move_rank(c));
    }
    for &c in &cand[..ncand] {
        if move_rank(c) == best_rank {
            return Some(c);
        }
    }
    Some(cand[0])
}

pub fn selftest() -> usize {
    let empty = [Cell::Empty; 9];
    assert_eq!(best_move(&empty, 2), Some(4));

    let mut b = [Cell::Empty; 9];
    b[0] = Cell::X;
    b[1] = Cell::X;
    assert_eq!(best_move(&b, 2), Some(2));

    let mut b2 = [Cell::Empty; 9];
    b2[0] = Cell::X;
    b2[1] = Cell::X;
    b2[3] = Cell::O;
    b2[4] = Cell::O;
    assert_eq!(best_move(&b2, 2), Some(5));
    3
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn minimax_never_loses() {
        assert_eq!(selftest(), 3);
    }
}
