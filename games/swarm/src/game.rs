//! `SWARM` — RTS-lite на ESP32-C5: карта, юніти, накази, економіка, бій, AI.
//!
//! Це **чиста логіка**: без `std`, `alloc` і `esp-hal`, тому файл компілюється
//! звичайним `rustc --test` на хості (`tools/test-host.sh`).
//!
//! Модель світу:
//! - карта 20×13 тайлів по 16 px (320×208) — уся на екрані, без скролу;
//! - 2 сторони (`SIDE_BLUE` — гравець, `SIDE_RED` — AI), по 3 типи юнітів і 2 типи будівель;
//! - один ресурс (мінерали), безперервні родовища;
//! - один тік = 50 мс (20 Гц); усе детерміноване — немає RNG і `Instant`.

#![allow(dead_code)]

// ── Геометрія ────────────────────────────────────────────────────────────────

/// Сторона тайла, px.
pub const TILE: i32 = 16;
/// Карта в тайлах: 20×13 = 320×208 px, решта 32 px — HUD.
pub const MAP_W: usize = 20;
pub const MAP_H: usize = 13;
pub const MAP_PX_W: i32 = MAP_W as i32 * TILE;
pub const MAP_PX_H: i32 = MAP_H as i32 * TILE;
pub const HUD_H: i32 = 32;
/// Кількість тайлів на карті.
pub const N_TILES: usize = MAP_W * MAP_H;
/// Спрайт юніта, px (квадрат).
pub const UNIT_SIZE: i32 = 10;
/// Один тік симуляції, мс.
pub const TICK_MS: u32 = 50;

pub const SIDE_BLUE: u8 = 0;
pub const SIDE_RED: u8 = 1;

pub const MAX_UNITS: usize = 24;
pub const MAX_BUILDINGS: usize = 8;
pub const MAX_EFFECTS: usize = 16;
/// Максимум кроків у збереженому шляху (тайлів).
pub const PATH_MAX: usize = 48;
/// Ліміт армії на сторону (workers + army).
pub const SUPPLY_MAX: u16 = 12;

// ── Тайли ────────────────────────────────────────────────────────────────────

pub const T_GRASS: u8 = 0;
pub const T_SAND: u8 = 1;
pub const T_ROCK: u8 = 2;
pub const T_MINERAL: u8 = 3;

/// Чи можна ходити по тайлу (мінерали й скелі — ні).
#[inline]
pub fn is_walkable(t: u8) -> bool {
    matches!(t, T_GRASS | T_SAND)
}

/// Карта як текст: `.` трава, `,` пісок, `#` скеля, `*` мінерали.
pub const MAP_TXT: [&[u8]; MAP_H] = [
    b"....................",
    b"...####......####...",
    b"...#..#......#..#...",
    b"...####......####...",
    b"..,,*...####...*,,..",
    b"....*..........*....",
    b"..,,,,,,,,,,,,,,,,..",
    b"....*..........*....",
    b"..,,*...####...*,,..",
    b"...####......####...",
    b"...#..#......#..#...",
    b"...####......####...",
    b"....................",
];

const fn parse_row(s: &[u8]) -> [u8; MAP_W] {
    let mut out = [T_GRASS; MAP_W];
    let mut i = 0;
    while i < MAP_W && i < s.len() {
        out[i] = match s[i] {
            b'#' => T_ROCK,
            b'*' => T_MINERAL,
            b',' => T_SAND,
            _ => T_GRASS,
        };
        i += 1;
    }
    out
}

const fn build_map() -> [[u8; MAP_W]; MAP_H] {
    let mut m = [[T_GRASS; MAP_W]; MAP_H];
    let mut r = 0;
    while r < MAP_H {
        m[r] = parse_row(MAP_TXT[r]);
        r += 1;
    }
    m
}

/// Карта тайлів: `MAP[y][x]`.
pub const MAP: [[u8; MAP_W]; MAP_H] = build_map();

/// Тип тайла в пікселях (поза картою — скеля).
#[inline]
pub fn tile_at_px(x: i32, y: i32) -> u8 {
    if x < 0 || y < 0 || x >= MAP_PX_W || y >= MAP_PX_H {
        return T_ROCK;
    }
    MAP[(y / TILE) as usize][(x / TILE) as usize]
}

#[inline]
pub fn tile_index(tx: i32, ty: i32) -> usize {
    ty as usize * MAP_W + tx as usize
}

#[inline]
pub fn idx_tx(i: usize) -> i32 {
    (i % MAP_W) as i32
}

#[inline]
pub fn idx_ty(i: usize) -> i32 {
    (i / MAP_W) as i32
}

/// Лівий верхній кут спрайта, щоб він стояв по центру тайла.
#[inline]
pub fn tile_sprite_xy(tx: i32, ty: i32) -> (i32, i32) {
    let off = (TILE - UNIT_SIZE) / 2;
    (tx * TILE + off, ty * TILE + off)
}

// ── Типи юнітів і будівель ───────────────────────────────────────────────────

pub const U_WORKER: u8 = 0;
pub const U_GRUNT: u8 = 1;
pub const U_HEAVY: u8 = 2;

pub struct UStats {
    pub hp: u16,
    pub dmg: u16,
    /// Дальність пострілу, px (між центрами).
    pub range: i32,
    /// Перезарядка, тіків.
    pub cd: u16,
    /// Скільки тіків між кроками (1 = швидкий, 2 = важкий).
    pub move_interval: u8,
    pub cost: u16,
    pub train_ticks: u16,
}

pub fn unit_stats(kind: u8) -> UStats {
    match kind {
        U_WORKER => UStats {
            hp: 40,
            dmg: 0,
            range: 0,
            cd: 0,
            move_interval: 1,
            cost: 50,
            train_ticks: 60,
        },
        U_GRUNT => UStats {
            hp: 60,
            dmg: 6,
            range: 26,
            cd: 20,
            move_interval: 1,
            cost: 75,
            train_ticks: 70,
        },
        _ => UStats {
            hp: 140,
            dmg: 14,
            range: 30,
            cd: 30,
            move_interval: 2,
            cost: 125,
            train_ticks: 100,
        },
    }
}

pub const B_HQ: u8 = 0;
pub const B_BARRACKS: u8 = 1;

pub struct BStats {
    pub w: i32,
    pub h: i32,
    pub hp: u16,
    pub cost: u16,
    pub build_ticks: u16,
}

pub fn bld_stats(kind: u8) -> BStats {
    match kind {
        B_HQ => BStats { w: 3, h: 3, hp: 900, cost: 0, build_ticks: 1 },
        _ => BStats { w: 2, h: 2, hp: 300, cost: 100, build_ticks: 120 },
    }
}

// ── Накази ───────────────────────────────────────────────────────────────────

pub const O_IDLE: u8 = 0;
pub const O_MOVE: u8 = 1;
pub const O_ATTACK: u8 = 2;
pub const O_ATTACK_MOVE: u8 = 3;
pub const O_GATHER: u8 = 4;
pub const O_BUILD: u8 = 5;

/// Ціль-юніт: 0..MAX_UNITS-1; ціль-будівля: 200+b; 255 = немає.
pub const TGT_NONE: u8 = 255;
pub const TGT_BLD: u8 = 200;

#[inline]
pub fn tgt_is_bld(t: u8) -> bool {
    t >= TGT_BLD
}

// ── Стан юніта / будівлі / ефекту ────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Unit {
    pub alive: bool,
    pub kind: u8,
    pub side: u8,
    /// Лівий верхній кут спрайта, px.
    pub x: i32,
    pub y: i32,
    pub hp: u16,
    pub order: u8,
    /// Ціль (`TGT_NONE`, індекс юніта або `TGT_BLD+n`).
    pub target: u8,
    /// Накази: кінцевий тайл наказу.
    pub goal: u16,
    /// Будівля-ціль наказу (`255` — немає; тоді діє `goal`).
    pub goal_bld: u8,
    pub path: [u16; PATH_MAX],
    pub path_len: u8,
    pub path_i: u8,
    /// Лічильник перезарядки атаки.
    pub cd: u16,
    /// Лічильник «важкий робить крок раз на 2 тіки».
    pub slow: u8,
    /// Фаза робітника: 0 — до родовища, 1 — видобуває, 2 — до бази.
    pub phase: u8,
    pub carry: u16,
    pub mine_t: u16,
    /// Індекс будівлі, яку будує (`255` — ні).
    pub bld: u8,
    /// Скільки тіків показувати спалах пострілу.
    pub flash: u8,
}

impl Unit {
    const fn empty() -> Self {
        Self {
            alive: false,
            kind: U_WORKER,
            side: SIDE_BLUE,
            x: 0,
            y: 0,
            hp: 0,
            order: O_IDLE,
            target: TGT_NONE,
            goal: 0,
            goal_bld: 255,
            path: [0; PATH_MAX],
            path_len: 0,
            path_i: 0,
            cd: 0,
            slow: 0,
            phase: 0,
            carry: 0,
            mine_t: 0,
            bld: 255,
            flash: 0,
        }
    }

    #[inline]
    pub fn cx(&self) -> i32 {
        self.x + UNIT_SIZE / 2
    }

    #[inline]
    pub fn cy(&self) -> i32 {
        self.y + UNIT_SIZE / 2
    }

    #[inline]
    pub fn tile(&self) -> (i32, i32) {
        ((self.x + UNIT_SIZE / 2) / TILE, (self.y + UNIT_SIZE / 2) / TILE)
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Building {
    pub alive: bool,
    pub kind: u8,
    pub side: u8,
    pub tx: u8,
    pub ty: u8,
    pub hp: u16,
    pub done: bool,
    pub progress: u16,
    pub queue_kind: u8,
    pub queue_t: u16,
}

impl Building {
    const fn empty() -> Self {
        Self {
            alive: false,
            kind: B_HQ,
            side: SIDE_BLUE,
            tx: 0,
            ty: 0,
            hp: 0,
            done: false,
            progress: 0,
            queue_kind: 255,
            queue_t: 0,
        }
    }

    #[inline]
    pub fn w(&self) -> i32 {
        bld_stats(self.kind).w
    }

    #[inline]
    pub fn h(&self) -> i32 {
        bld_stats(self.kind).h
    }

    #[inline]
    pub fn px(&self) -> (i32, i32) {
        (self.tx as i32 * TILE, self.ty as i32 * TILE)
    }

    #[inline]
    pub fn cx(&self) -> i32 {
        self.tx as i32 * TILE + self.w() * TILE / 2
    }

    #[inline]
    pub fn cy(&self) -> i32 {
        self.ty as i32 * TILE + self.h() * TILE / 2
    }

    #[inline]
    pub fn contains_px(&self, x: i32, y: i32) -> bool {
        let (px, py) = self.px();
        x >= px && y >= py && x < px + self.w() * TILE && y < py + self.h() * TILE
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Effect {
    pub alive: bool,
    pub x: i32,
    pub y: i32,
    pub ttl: u8,
    pub kind: u8,
}

pub const FX_HIT: u8 = 0;
pub const FX_DEATH: u8 = 1;
pub const FX_BUILD: u8 = 2;

impl Effect {
    const fn empty() -> Self {
        Self { alive: false, x: 0, y: 0, ttl: 0, kind: 0 }
    }
}

// ── Світ ─────────────────────────────────────────────────────────────────────

pub struct Game {
    pub units: [Unit; MAX_UNITS],
    pub buildings: [Building; MAX_BUILDINGS],
    pub effects: [Effect; MAX_EFFECTS],
    pub minerals: [u32; SIDE_RED as usize + 1],
    pub miner_tiles: [u16; 16],
    pub miner_count: u8,
    pub tick_n: u32,
    /// 0 — гра триває, 1 — перемога синіх, 2 — перемога червоних.
    pub over: u8,
    /// Скільки тіків триває партія на момент завершення.
    pub over_tick: u32,
}

const DIR_DX: [i32; 4] = [0, 1, 0, -1];
const DIR_DY: [i32; 4] = [-1, 0, 1, 0];

/// Скільки тіків робітник видобуває одну партію.
const MINE_TICKS: u16 = 40;
/// Скільки мінералів приносить один рейс.
const CARRY_AMOUNT: u16 = 8;

impl Game {
    pub fn new() -> Self {
        let mut g = Self {
            units: [Unit::empty(); MAX_UNITS],
            buildings: [Building::empty(); MAX_BUILDINGS],
            effects: [Effect::empty(); MAX_EFFECTS],
            minerals: [0; SIDE_RED as usize + 1],
            miner_tiles: [0; 16],
            miner_count: 0,
            tick_n: 0,
            over: 0,
            over_tick: 0,
        };
        // родовища
        for y in 0..MAP_H {
            for x in 0..MAP_W {
                if MAP[y][x] == T_MINERAL && (g.miner_count as usize) < g.miner_tiles.len() {
                    g.miner_tiles[g.miner_count as usize] = tile_index(x as i32, y as i32) as u16;
                    g.miner_count += 1;
                }
            }
        }
        // стартові бази: сині в лівому верхньому куті, червоні — дзеркально
        g.buildings[0] = Building {
            alive: true,
            kind: B_HQ,
            side: SIDE_BLUE,
            tx: 0,
            ty: 4,
            hp: 900,
            done: true,
            progress: 0,
            queue_kind: 255,
            queue_t: 0,
        };
        g.buildings[1] = Building {
            alive: true,
            kind: B_HQ,
            side: SIDE_RED,
            tx: 17,
            ty: 4,
            hp: 900,
            done: true,
            progress: 0,
            queue_kind: 255,
            queue_t: 0,
        };
        // 4 робітники на сторону — біля бази, з наказом «видобувати»
        for side in [SIDE_BLUE, SIDE_RED] {
            for k in 0..4 {
                let (tx, ty) = if side == SIDE_BLUE { (k, 7) } else { (19 - k, 7) };
                let idx = g.free_unit_slot();
                if idx == MAX_UNITS {
                    break;
                }
                g.spawn_unit(side, U_WORKER, tx as i32, ty as i32, idx);
                g.units[idx].order = O_GATHER;
                g.units[idx].phase = 0;
                let mt = g.nearest_mineral(tx as i32 * TILE, ty as i32 * TILE, side);
                g.units[idx].goal = mt;
                g.units[idx].path_len = 0;
            }
        }
        g
    }

    // ── Дрібні запити ────────────────────────────────────────────────────────

    pub fn free_unit_slot(&self) -> usize {
        for i in 0..MAX_UNITS {
            if !self.units[i].alive {
                return i;
            }
        }
        MAX_UNITS
    }

    pub fn free_bld_slot(&self) -> usize {
        for i in 0..MAX_BUILDINGS {
            if !self.buildings[i].alive {
                return i;
            }
        }
        MAX_BUILDINGS
    }

    pub fn count_units(&self, side: u8) -> u16 {
        let mut n = 0;
        for i in 0..MAX_UNITS {
            let u = &self.units[i];
            if u.alive && u.side == side {
                n += 1;
            }
        }
        n
    }

    pub fn count_kind(&self, side: u8, kind: u8) -> u16 {
        let mut n = 0;
        for i in 0..MAX_UNITS {
            let u = &self.units[i];
            if u.alive && u.side == side && u.kind == kind {
                n += 1;
            }
        }
        n
    }

    pub fn army_count(&self, side: u8) -> u16 {
        self.count_kind(side, U_GRUNT) + self.count_kind(side, U_HEAVY)
    }

    pub fn hq(&self, side: u8) -> Option<usize> {
        for i in 0..MAX_BUILDINGS {
            let b = &self.buildings[i];
            if b.alive && b.side == side && b.kind == B_HQ {
                return Some(i);
            }
        }
        None
    }

    pub fn barracks(&self, side: u8) -> Option<usize> {
        for i in 0..MAX_BUILDINGS {
            let b = &self.buildings[i];
            if b.alive && b.side == side && b.kind == B_BARRACKS && b.done {
                return Some(i);
            }
        }
        None
    }

    /// Юніт під точкою екрана (для тапу).
    pub fn unit_at(&self, x: i32, y: i32) -> Option<usize> {
        for i in 0..MAX_UNITS {
            let u = &self.units[i];
            if u.alive && x >= u.x && y >= u.y && x < u.x + UNIT_SIZE && y < u.y + UNIT_SIZE {
                return Some(i);
            }
        }
        None
    }

    /// Будівля під точкою екрана.
    pub fn building_at(&self, x: i32, y: i32) -> Option<usize> {
        for i in 0..MAX_BUILDINGS {
            let b = &self.buildings[i];
            if b.alive && b.contains_px(x, y) {
                return Some(i);
            }
        }
        None
    }

    /// Маска юнітів у прямокутнику (рамка виділення) для однієї сторони.
    pub fn units_in_rect(&self, side: u8, x0: i32, y0: i32, x1: i32, y1: i32) -> u32 {
        let (x0, x1) = (x0.min(x1), x0.max(x1));
        let (y0, y1) = (y0.min(y1), y0.max(y1));
        let mut m = 0u32;
        for i in 0..MAX_UNITS {
            let u = &self.units[i];
            if !u.alive || u.side != side {
                continue;
            }
            let ux = u.x + UNIT_SIZE / 2;
            let uy = u.y + UNIT_SIZE / 2;
            if ux >= x0 && ux <= x1 && uy >= y0 && uy <= y1 {
                m |= 1 << i;
            }
        }
        m
    }

    pub fn mask_of_kind(&self, side: u8, kind: u8) -> u32 {
        let mut m = 0u32;
        for i in 0..MAX_UNITS {
            let u = &self.units[i];
            if u.alive && u.side == side && u.kind == kind {
                m |= 1 << i;
            }
        }
        m
    }

    /// Скільки юнітів у масці.
    pub fn mask_count(mask: u32) -> u16 {
        mask.count_ones() as u16
    }

    // ── Створення сутностей ──────────────────────────────────────────────────

    pub fn spawn_unit(&mut self, side: u8, kind: u8, tx: i32, ty: i32, slot: usize) {
        if slot >= MAX_UNITS {
            return;
        }
        let (x, y) = tile_sprite_xy(tx, ty);
        self.units[slot] = Unit {
            alive: true,
            kind,
            side,
            x,
            y,
            hp: unit_stats(kind).hp,
            ..Unit::empty()
        };
    }

    fn add_effect(&mut self, x: i32, y: i32, kind: u8, ttl: u8) {
        for i in 0..MAX_EFFECTS {
            if !self.effects[i].alive {
                self.effects[i] = Effect { alive: true, x, y, ttl, kind };
                return;
            }
        }
        // усі зайняті — перезаписуємо найстаріший
        self.effects[0] = Effect { alive: true, x, y, ttl, kind };
    }

    // ── Перешкоди й пошук шляху ──────────────────────────────────────────────

    /// Карта непрохідності: скелі, мінерали, усі будівлі.
    pub fn blocked_map(&self) -> [bool; N_TILES] {
        let mut b = [false; N_TILES];
        for y in 0..MAP_H {
            for x in 0..MAP_W {
                b[tile_index(x as i32, y as i32)] = !is_walkable(MAP[y][x]);
            }
        }
        for i in 0..MAX_BUILDINGS {
            let bl = &self.buildings[i];
            if !bl.alive {
                continue;
            }
            for dy in 0..bl.h() {
                for dx in 0..bl.w() {
                    let tx = bl.tx as i32 + dx;
                    let ty = bl.ty as i32 + dy;
                    if tx >= 0 && ty >= 0 && tx < MAP_W as i32 && ty < MAP_H as i32 {
                        b[tile_index(tx, ty)] = true;
                    }
                }
            }
        }
        b
    }

    /// Позначити в `want` клітинки, досягнення яких завершує шлях для цілі-тайла:
    /// сам тайл (якщо прохідний) або будь-який його вільний сусід.
    fn want_tile(block: &[bool; N_TILES], goal: usize, want: &mut [bool; N_TILES]) {
        if !block[goal] {
            want[goal] = true;
            return;
        }
        Self::want_ring(block, idx_tx(goal), idx_ty(goal), 1, 1, want);
    }

    /// Вільні сусіди прямокутника `w×h` у тайлах (для будівель: усі вільні клітинки навколо).
    fn want_ring(block: &[bool; N_TILES], tx: i32, ty: i32, w: i32, h: i32, want: &mut [bool; N_TILES]) {
        for dy in 0..h {
            for dx in 0..w {
                let cx = tx + dx;
                let cy = ty + dy;
                for d in 0..4usize {
                    let nx = cx + DIR_DX[d];
                    let ny = cy + DIR_DY[d];
                    if nx < 0 || ny < 0 || nx >= MAP_W as i32 || ny >= MAP_H as i32 {
                        continue;
                    }
                    let ni = tile_index(nx, ny);
                    if !block[ni] {
                        want[ni] = true;
                    }
                }
            }
        }
    }

    /// Найкоротший шлях (BFS/Дейкстра по 4 сусідах) від `start` до будь-якої клітинки з `want`.
    /// Карта маленька (260 тайлів), тому без евристики: пошук завжди оптимальний.
    fn find_path(block: &[bool; N_TILES], start: usize, want: &[bool; N_TILES]) -> ([u16; PATH_MAX], u8) {
        let mut out = ([0u16; PATH_MAX], 0u8);
        if want[start] {
            return out;
        }
        const INF: u16 = u16::MAX;
        let mut g = [INF; N_TILES];
        let mut from = [u8::MAX; N_TILES];
        let mut closed = [false; N_TILES];
        g[start] = 0;
        let mut end = usize::MAX;
        loop {
            let mut best = usize::MAX;
            let mut bg = INF;
            for i in 0..N_TILES {
                if !closed[i] && g[i] < bg {
                    bg = g[i];
                    best = i;
                }
            }
            if best == usize::MAX {
                break;
            }
            if want[best] {
                end = best;
                break;
            }
            closed[best] = true;
            let bx = idx_tx(best);
            let by = idx_ty(best);
            for d in 0..4usize {
                let nx = bx + DIR_DX[d];
                let ny = by + DIR_DY[d];
                if nx < 0 || ny < 0 || nx >= MAP_W as i32 || ny >= MAP_H as i32 {
                    continue;
                }
                let ni = tile_index(nx, ny);
                if block[ni] || closed[ni] {
                    continue;
                }
                let ng = g[best] + 1;
                if ng < g[ni] {
                    g[ni] = ng;
                    from[ni] = d as u8;
                }
            }
        }
        if end == usize::MAX {
            return out;
        }
        // зворотне відновлення
        let mut rev = [0u16; N_TILES];
        let mut n = 0usize;
        let mut cur = end;
        while cur != start {
            if n >= N_TILES {
                break;
            }
            rev[n] = cur as u16;
            n += 1;
            let d = from[cur] as usize;
            if d > 3 {
                break;
            }
            cur = (cur as i32 - DIR_DY[d] * MAP_W as i32 - DIR_DX[d]) as usize;
        }
        let take = n.min(PATH_MAX);
        for k in 0..take {
            out.0[k] = rev[n - 1 - k];
        }
        out.1 = take as u8;
        out
    }

    fn apply_path(&mut self, i: usize, block: &[bool; N_TILES], want: &[bool; N_TILES], goal: u16, goal_bld: u8) {
        let (tx, ty) = self.units[i].tile();
        let start = tile_index(tx.clamp(0, MAP_W as i32 - 1), ty.clamp(0, MAP_H as i32 - 1));
        let (p, n) = Self::find_path(block, start, want);
        self.units[i].path = p;
        self.units[i].path_len = n;
        self.units[i].path_i = 0;
        self.units[i].goal = goal;
        self.units[i].goal_bld = goal_bld;
    }

    /// Задати шлях юніту до тайла (тайл може бути непрохідним — тоді шлях
    /// закінчується на сусідньому тайлі).
    pub fn set_path_to(&mut self, i: usize, goal: usize) {
        let block = self.blocked_map();
        let mut want = [false; N_TILES];
        Self::want_tile(&block, goal, &mut want);
        self.apply_path(i, &block, &want, goal as u16, 255);
    }

    /// Задати шлях юніту до будівлі (до будь-якого вільного тайла навколо неї).
    pub fn set_path_to_bld(&mut self, i: usize, bi: usize) {
        let block = self.blocked_map();
        let b = self.buildings[bi];
        let mut want = [false; N_TILES];
        Self::want_ring(&block, b.tx as i32, b.ty as i32, b.w(), b.h(), &mut want);
        self.apply_path(i, &block, &want, tile_index(b.tx as i32, b.ty as i32) as u16, bi as u8);
    }

    /// Перерахувати шлях за збереженою ціллю юніта (після обрізання по `PATH_MAX`).
    fn repath(&mut self, i: usize) {
        let b = self.units[i].goal_bld;
        if b != 255 && (b as usize) < MAX_BUILDINGS && self.buildings[b as usize].alive {
            self.set_path_to_bld(i, b as usize);
        } else {
            self.set_path_to(i, self.units[i].goal as usize);
        }
    }

    /// Найближче до точки родовище (для робітника).
    pub fn nearest_mineral(&self, x: i32, y: i32, side: u8) -> u16 {
        let mut best = 0u16;
        let mut best_d = i64::MAX;
        let hx = if side == SIDE_BLUE { 0 } else { MAP_PX_W };
        for k in 0..self.miner_count as usize {
            let t = self.miner_tiles[k];
            let mx = idx_tx(t as usize) * TILE;
            let my = idx_ty(t as usize) * TILE;
            // спершу ближче до своєї половини карти
            let side_bias = ((mx - hx).abs() as i64) * 2;
            let d = ((mx - x).abs() + (my - y).abs()) as i64 + side_bias;
            if d < best_d {
                best_d = d;
                best = t;
            }
        }
        best
    }

    // ── Накази ───────────────────────────────────────────────────────────────

    /// Наказ «іди/атакуй сюди» для всіх юнітів маски.
    /// Повертає true, якщо під точкою був ворог (наказ-атака).
    pub fn order_at(&mut self, mask: u32, x: i32, y: i32, side: u8) -> bool {
        let mut attacked = false;
        let tgt_unit = self.unit_at(x, y).filter(|&i| self.units[i].side != side);
        let tgt_bld = self.building_at(x, y).filter(|&i| self.buildings[i].side != side);
        let goal_tile = tile_index(
            (x / TILE).clamp(0, MAP_W as i32 - 1),
            (y / TILE).clamp(0, MAP_H as i32 - 1),
        );
        for i in 0..MAX_UNITS {
            if mask & (1 << i) == 0 || !self.units[i].alive {
                continue;
            }
            if let Some(t) = tgt_unit {
                self.units[i].order = O_ATTACK;
                self.units[i].target = t as u8;
                self.units[i].path_len = 0;
                attacked = true;
            } else if let Some(b) = tgt_bld {
                self.units[i].order = O_ATTACK;
                self.units[i].target = TGT_BLD + b as u8;
                self.units[i].path_len = 0;
                attacked = true;
            } else {
                // робітник, що видобуває, отримує звичайний наказ «рух»
                self.units[i].order = O_ATTACK_MOVE;
                self.units[i].target = TGT_NONE;
                self.set_path_to(i, goal_tile);
            }
        }
        attacked
    }

    pub fn stop(&mut self, mask: u32) {
        for i in 0..MAX_UNITS {
            if mask & (1 << i) != 0 && self.units[i].alive {
                self.units[i].order = O_IDLE;
                self.units[i].path_len = 0;
                self.units[i].target = TGT_NONE;
                self.units[i].goal_bld = 255;
            }
        }
    }

    /// Надіслати робітника видобувати (використовує і AI).
    pub fn order_gather(&mut self, i: usize) {
        if i >= MAX_UNITS || !self.units[i].alive || self.units[i].kind != U_WORKER {
            return;
        }
        let side = self.units[i].side;
        let mx = self.nearest_mineral(self.units[i].x, self.units[i].y, side);
        self.units[i].order = O_GATHER;
        self.units[i].phase = 0;
        self.units[i].goal = mx;
        self.units[i].path_len = 0;
        self.units[i].target = TGT_NONE;
    }

    // ── Будівництво й виробництво ────────────────────────────────────────────

    pub fn can_place(&self, kind: u8, tx: i32, ty: i32) -> bool {
        let st = bld_stats(kind);
        if tx < 0 || ty < 0 || tx + st.w > MAP_W as i32 || ty + st.h > MAP_H as i32 {
            return false;
        }
        let block = self.blocked_map();
        for dy in 0..st.h {
            for dx in 0..st.w {
                let i = tile_index(tx + dx, ty + dy);
                if block[i] {
                    return false;
                }
            }
        }
        true
    }

    /// Поставити будівлю й відправити робітника будувати.
    pub fn place(&mut self, side: u8, kind: u8, tx: i32, ty: i32, worker: usize) -> bool {
        let st = bld_stats(kind);
        if !self.can_place(kind, tx, ty) {
            return false;
        }
        if self.minerals[side as usize] < st.cost as u32 {
            return false;
        }
        let Some(slot) = ({ let s = self.free_bld_slot(); if s == MAX_BUILDINGS { None } else { Some(s) } }) else {
            return false;
        };
        self.minerals[side as usize] -= st.cost as u32;
        self.buildings[slot] = Building {
            alive: true,
            kind,
            side,
            tx: tx as u8,
            ty: ty as u8,
            hp: st.hp / 5,
            done: false,
            progress: 0,
            queue_kind: 255,
            queue_t: 0,
        };
        if worker < MAX_UNITS && self.units[worker].alive {
            self.units[worker].order = O_BUILD;
            self.units[worker].bld = slot as u8;
            self.units[worker].target = TGT_NONE;
            let goal = tile_index(tx, ty);
            self.set_path_to(worker, goal);
        }
        true
    }

    /// Замовити юніта в будівлі. Повертає true, якщо замовлення прийняте.
    pub fn train(&mut self, bld: usize, kind: u8) -> bool {
        if bld >= MAX_BUILDINGS {
            return false;
        }
        let b = self.buildings[bld];
        if !b.alive || !b.done || b.queue_kind != 255 {
            return false;
        }
        let ok_kind = match b.kind {
            B_HQ => kind == U_WORKER,
            _ => kind == U_GRUNT || kind == U_HEAVY,
        };
        if !ok_kind {
            return false;
        }
        let st = unit_stats(kind);
        let side = b.side;
        if self.count_units(side) >= SUPPLY_MAX {
            return false;
        }
        if self.minerals[side as usize] < st.cost as u32 {
            return false;
        }
        self.minerals[side as usize] -= st.cost as u32;
        self.buildings[bld].queue_kind = kind;
        self.buildings[bld].queue_t = st.train_ticks;
        true
    }

    /// Вільний тайл навколо будівлі для появи юніта: найближчий до власного HQ
    /// (щоб поява була симетричною для обох сторін і юніт не народжувався в бою).
    fn spawn_tile_near(&self, b: &Building) -> Option<(i32, i32)> {
        let block = self.blocked_map();
        let cx = b.tx as i32;
        let cy = b.ty as i32;
        let (ax, ay) = match self.hq(b.side) {
            Some(h) => {
                let hq = self.buildings[h];
                (hq.tx as i32, hq.ty as i32)
            }
            None => (cx, cy),
        };
        let mut best: Option<(i32, i32, i32)> = None;
        for r in 1..=3i32 {
            for dy in -r..=r {
                for dx in -r..=r {
                    if dx.abs() != r && dy.abs() != r {
                        continue;
                    }
                    let nx = cx + dx;
                    let ny = cy + dy;
                    if nx < 0 || ny < 0 || nx >= MAP_W as i32 || ny >= MAP_H as i32 {
                        continue;
                    }
                    if block[tile_index(nx, ny)] {
                        continue;
                    }
                    let d = (nx - ax).abs() + (ny - ay).abs();
                    if best.is_none() || d < best.unwrap().2 {
                        best = Some((nx, ny, d));
                    }
                }
            }
        }
        best.map(|(x, y, _)| (x, y))
    }

    // ── Бій ──────────────────────────────────────────────────────────────────

    /// Позиція цілі: центр + піврозміри (щоб дальність до будівлі мірялась до її краю, а не центру).
    fn target_pos(&self, t: u8) -> Option<(i32, i32, i32, i32, u16)> {
        if t == TGT_NONE {
            return None;
        }
        if tgt_is_bld(t) {
            let b = &self.buildings[(t - TGT_BLD) as usize];
            if b.alive {
                return Some((b.cx(), b.cy(), b.w() * TILE / 2, b.h() * TILE / 2, b.hp));
            }
            return None;
        }
        let u = &self.units[t as usize];
        if u.alive {
            Some((u.cx(), u.cy(), UNIT_SIZE / 2, UNIT_SIZE / 2, u.hp))
        } else {
            None
        }
    }

    fn damage_target(&mut self, t: u8, dmg: u16) {
        if tgt_is_bld(t) {
            let bi = (t - TGT_BLD) as usize;
            let b = &mut self.buildings[bi];
            b.hp = b.hp.saturating_sub(dmg);
            if b.hp == 0 {
                b.alive = false;
                let (x, y) = b.px();
                self.add_effect(x + 8, y + 8, FX_DEATH, 12);
            }
        } else if t != TGT_NONE {
            let ti = t as usize;
            let u = &mut self.units[ti];
            u.hp = u.hp.saturating_sub(dmg);
            if u.hp == 0 {
                u.alive = false;
                u.path_len = 0;
                u.target = TGT_NONE;
                let (x, y) = (u.x, u.y);
                let side = u.side;
                let _ = side;
                self.add_effect(x + 5, y + 5, FX_DEATH, 12);
            }
        }
    }

    /// Найближчий ворог (юніт або будівля) у радіусі `max_d` px.
    fn nearest_enemy(&self, side: u8, x: i32, y: i32, max_d: i32) -> Option<u8> {
        let mut best: u8 = TGT_NONE;
        let mut best_d = max_d * max_d;
        for i in 0..MAX_UNITS {
            let u = &self.units[i];
            if !u.alive || u.side == side {
                continue;
            }
            let dx = u.cx() - x;
            let dy = u.cy() - y;
            let d = dx * dx + dy * dy;
            if d <= best_d {
                best_d = d;
                best = i as u8;
            }
        }
        if best != TGT_NONE {
            return Some(best);
        }
        for i in 0..MAX_BUILDINGS {
            let b = &self.buildings[i];
            if !b.alive || b.side == side {
                continue;
            }
            let dx = b.cx() - x;
            let dy = b.cy() - y;
            let d = dx * dx + dy * dy;
            if d <= best_d {
                best_d = d;
                best = TGT_BLD + i as u8;
            }
        }
        if best == TGT_NONE { None } else { Some(best) }
    }

    // ── Крок симуляції ───────────────────────────────────────────────────────

    pub fn tick(&mut self) {
        if self.over != 0 {
            return;
        }
        self.tick_n += 1;

        for i in 0..MAX_UNITS {
            if !self.units[i].alive {
                continue;
            }
            if self.units[i].flash > 0 {
                self.units[i].flash -= 1;
            }
            if self.units[i].cd > 0 {
                self.units[i].cd -= 1;
            }
            match self.units[i].order {
                O_GATHER => self.tick_gather(i),
                O_BUILD => self.tick_build(i),
                O_MOVE => self.tick_move(i, false),
                O_ATTACK_MOVE => self.tick_move(i, true),
                O_ATTACK => self.tick_attack(i),
                _ => {}
            }
        }

        self.tick_buildings();

        for e in self.effects.iter_mut() {
            if e.alive {
                e.ttl = e.ttl.saturating_sub(1);
                if e.ttl == 0 {
                    e.alive = false;
                }
            }
        }

        // перевірка завершення
        let blue_hq = self.hq(SIDE_BLUE).is_some();
        let red_hq = self.hq(SIDE_RED).is_some();
        if !red_hq {
            self.over = 1;
            self.over_tick = self.tick_n;
        } else if !blue_hq {
            self.over = 2;
            self.over_tick = self.tick_n;
        }
    }

    fn advance(&mut self, i: usize, gx: i32, gy: i32) -> bool {
        let mut u = self.units[i];
        if u.slow > 1 {
            u.slow -= 1;
            self.units[i] = u;
            return false;
        }
        u.slow = unit_stats(u.kind).move_interval;
        let dx = gx - u.x;
        let dy = gy - u.y;
        if dx != 0 {
            u.x += dx.signum();
        } else if dy != 0 {
            u.y += dy.signum();
        }
        let reached = u.x == gx && u.y == gy;
        self.units[i] = u;
        reached
    }

    /// Рух по шляху; повертає true, коли шлях закінчено.
    fn follow(&mut self, i: usize) -> bool {
        loop {
            let u = self.units[i];
            if u.path_i >= u.path_len {
                return true;
            }
            let t = u.path[u.path_i as usize] as usize;
            let (gx, gy) = tile_sprite_xy(idx_tx(t), idx_ty(t));
            if self.advance(i, gx, gy) {
                self.units[i].path_i += 1;
            } else {
                return false;
            }
        }
    }

    fn tick_move(&mut self, i: usize, attack_move: bool) {
        let u = self.units[i];
        let side = u.side;
        if attack_move {
            // шукаємо ворога поруч, якщо цілі немає
            let t = u.target;
            let valid = if t == TGT_NONE {
                false
            } else if tgt_is_bld(t) {
                self.buildings[(t - TGT_BLD) as usize].alive
            } else {
                self.units[t as usize].alive
            };
            if !valid {
                if let Some(e) = self.nearest_enemy(side, u.cx(), u.cy(), 5 * TILE) {
                    self.units[i].target = e;
                }
                if self.units[i].target != TGT_NONE {
                    self.units[i].order = O_ATTACK;
                    self.units[i].path_len = 0;
                    self.tick_attack(i);
                    return;
                }
            }
        }
        if self.follow(i) {
            // шлях закінчився
            let u = self.units[i];
            let truncated = u.path_len as usize == PATH_MAX;
            let (tx, ty) = u.tile();
            let at_goal = tile_index(tx, ty) == u.goal as usize;
            if truncated && !at_goal {
                // шлях обірвався по PATH_MAX — дорахувати від поточної позиції
                self.repath(i);
                if self.units[i].path_len > 0 {
                    self.units[i].order = if attack_move { O_ATTACK_MOVE } else { O_MOVE };
                    return;
                }
            }
            self.units[i].order = O_IDLE;
            self.units[i].path_len = 0;
        }
    }

    fn tick_attack(&mut self, i: usize) {
        let u = self.units[i];
        let Some((tx, ty, hw, hh, _)) = self.target_pos(u.target) else {
            // ціль зникла: бійці шукають нову, робітники — за роботою
            if u.kind == U_WORKER {
                self.order_gather(i);
            } else if let Some(e) = self.nearest_enemy(u.side, u.cx(), u.cy(), 7 * TILE) {
                self.units[i].target = e;
            } else {
                self.units[i].order = O_IDLE;
                self.units[i].target = TGT_NONE;
            }
            return;
        };
        let st = unit_stats(u.kind);
        if st.dmg == 0 {
            // робітник не воює — просто йде до цілі, якщо вона юніт
            if !tgt_is_bld(u.target) {
                let t = self.units[u.target as usize];
                let (gx, gy) = (t.x, t.y);
                let _ = self.advance(i, gx, gy);
            }
            return;
        }
        // відстань до краю цілі (для будівлі — до її прямокутника)
        let dx = ((tx - u.cx()).abs() - hw).max(0);
        let dy = ((ty - u.cy()).abs() - hh).max(0);
        let range = st.range + 8;
        if dx * dx + dy * dy <= range * range {
            self.units[i].path_len = 0;
            self.units[i].path_i = 0;
            if u.cd == 0 {
                self.units[i].cd = st.cd;
                self.units[i].flash = 3;
                self.damage_target(u.target, st.dmg);
                self.add_effect(tx, ty, FX_HIT, 4);
            }
        } else {
            // підійти: шлях до цілі (для будівлі — до будь-якого вільного тайла навколо);
            // якщо шлях вичерпано й ціль усе ще далеко — перерахувати (з рознесенням по тіках)
            let exhausted = self.units[i].path_i >= self.units[i].path_len;
            if exhausted && (self.tick_n.wrapping_add(i as u32)) % 10 == 0 {
                let t = u.target;
                if tgt_is_bld(t) {
                    let bi = (t - TGT_BLD) as usize;
                    self.units[i].goal_bld = bi as u8;
                    self.set_path_to_bld(i, bi);
                } else {
                    let tu = &self.units[t as usize];
                    let (ttx, tty) = tu.tile();
                    self.set_path_to(i, tile_index(ttx, tty));
                }
            }
            self.follow(i);
        }
    }

    fn tick_gather(&mut self, i: usize) {
        let u = self.units[i];
        let side = u.side;
        match u.phase {
            0 => {
                if u.path_len == 0 {
                    // родовище вибираємо від поточної позиції (goal міг лишитися від рейсу до бази)
                    let goal = self.nearest_mineral(u.x, u.y, side);
                    self.set_path_to(i, goal as usize);
                }
                if self.follow(i) {
                    self.units[i].phase = 1;
                    self.units[i].mine_t = MINE_TICKS;
                    self.units[i].path_len = 0;
                }
            }
            1 => {
                if self.units[i].mine_t > 0 {
                    self.units[i].mine_t -= 1;
                } else {
                    self.units[i].carry = CARRY_AMOUNT;
                    self.units[i].phase = 2;
                    self.units[i].path_len = 0;
                }
            }
            _ => {
                if u.path_len == 0 {
                    let Some(hq) = self.hq(side) else {
                        self.units[i].order = O_IDLE;
                        return;
                    };
                    self.set_path_to_bld(i, hq);
                }
                if self.follow(i) {
                    let carry = self.units[i].carry;
                    self.minerals[side as usize] += carry as u32;
                    self.units[i].carry = 0;
                    self.units[i].phase = 0;
                    self.units[i].path_len = 0;
                }
            }
        }
    }

    fn tick_build(&mut self, i: usize) {
        let u = self.units[i];
        let bi = u.bld as usize;
        if bi >= MAX_BUILDINGS || !self.buildings[bi].alive {
            self.units[i].order = O_GATHER;
            self.units[i].phase = 0;
            return;
        }
        if self.buildings[bi].done {
            self.order_gather(i);
            return;
        }
        if u.path_len == 0 && u.path_i >= u.path_len {
            self.set_path_to_bld(i, bi);
        }
        let reached = self.follow(i);
        if reached {
            let b = &mut self.buildings[bi];
            b.progress += 1;
            let total = bld_stats(b.kind).build_ticks;
            if b.progress >= total {
                b.done = true;
                b.hp = bld_stats(b.kind).hp;
                let (x, y) = b.px();
                self.add_effect(x + 8, y + 8, FX_BUILD, 8);
                self.order_gather(i);
            }
        }
    }

    fn tick_buildings(&mut self) {
        // недобудовані без будівельника > 300 тіків (15 с) — зносимо майданчик
        for bi in 0..MAX_BUILDINGS {
            if !self.buildings[bi].alive || self.buildings[bi].done {
                continue;
            }
            let mut has_builder = false;
            for i in 0..MAX_UNITS {
                let u = &self.units[i];
                if u.alive && u.order == O_BUILD && u.bld as usize == bi {
                    has_builder = true;
                    break;
                }
            }
            if has_builder {
                self.buildings[bi].queue_t = 0;
            } else {
                self.buildings[bi].queue_t += 1;
                if self.buildings[bi].queue_t > 300 {
                    self.buildings[bi].alive = false;
                }
            }
        }
        for bi in 0..MAX_BUILDINGS {
            if !self.buildings[bi].alive || !self.buildings[bi].done {
                continue;
            }
            if self.buildings[bi].queue_kind == 255 {
                continue;
            }
            if self.buildings[bi].queue_t > 0 {
                self.buildings[bi].queue_t -= 1;
                continue;
            }
            let kind = self.buildings[bi].queue_kind;
            let side = self.buildings[bi].side;
            let b = self.buildings[bi];
            let Some((tx, ty)) = self.spawn_tile_near(&b) else {
                continue;
            };
            let slot = self.free_unit_slot();
            if slot == MAX_UNITS {
                continue;
            }
            self.spawn_unit(side, kind, tx, ty, slot);
            if kind == U_WORKER {
                self.order_gather(slot);
            }
            self.buildings[bi].queue_kind = 255;
            self.buildings[bi].queue_t = 0;
        }
    }

    // ── AI ───────────────────────────────────────────────────────────────────

    /// Один крок AI для сторони (викликати раз на кілька тіків).
    pub fn ai_step(&mut self, side: u8) {
        if self.over != 0 {
            return;
        }
        let enemy = 1 - side;
        let minerals = self.minerals[side as usize];
        let army = self.army_count(side);
        let my_hq = self.hq(side);

        // 1. Робітники без наказу — на родовище.
        for i in 0..MAX_UNITS {
            let u = self.units[i];
            if u.alive && u.side == side && u.kind == U_WORKER && u.order == O_IDLE {
                self.order_gather(i);
            }
        }

        // 1b. Ворог біля бази — сховати робітників за HQ (інакше їх просто вирізають).
        if let Some(hq) = my_hq {
            let b = self.buildings[hq];
            if self.nearest_enemy(side, b.cx(), b.cy(), 7 * TILE).is_some() {
                for i in 0..MAX_UNITS {
                    let u = self.units[i];
                    if u.alive
                        && u.side == side
                        && u.kind == U_WORKER
                        && u.order == O_GATHER
                        && u.phase != 1
                    {
                        self.units[i].order = O_MOVE;
                        self.units[i].target = TGT_NONE;
                        self.set_path_to_bld(i, hq);
                    }
                }
            }
        }

        // 2. Барак: спершу один, другий — коли вже є армія.
        let workers = self.count_kind(side, U_WORKER);
        let barracks_total = self
            .buildings
            .iter()
            .filter(|b| b.alive && b.side == side && b.kind == B_BARRACKS)
            .count();
        let want_barracks = if barracks_total == 0 {
            workers >= 2 && minerals >= 100
        } else {
            barracks_total < 2 && army >= 3 && workers >= 3 && minerals >= 150
        };
        if want_barracks {
            let worker = (0..MAX_UNITS)
                .find(|&i| self.units[i].alive && self.units[i].side == side && self.units[i].kind == U_WORKER)
                .unwrap_or(MAX_UNITS);
            if worker < MAX_UNITS {
                if let Some((tx, ty)) = self.ai_build_spot(side) {
                    self.place(side, B_BARRACKS, tx, ty, worker);
                }
            }
        }

        // 3. Виробництво.
        let supply = self.count_units(side);
        if supply < SUPPLY_MAX {
            if let Some(hq) = self.hq(side) {
                if workers < 4 && self.minerals[side as usize] >= 50 {
                    self.train(hq, U_WORKER);
                }
            }
            if let Some(br) = self.barracks(side) {
                if self.buildings[br].queue_kind == 255 {
                    let heavies = self.count_kind(side, U_HEAVY);
                    if heavies < 1 && self.minerals[side as usize] >= 125 {
                        self.train(br, U_HEAVY);
                    } else if self.minerals[side as usize] >= 75 {
                        self.train(br, U_GRUNT);
                    }
                }
            }
        }

        // 4. Армія.
        let army = self.army_count(side);
        // 4a. Оборона: ворог близько до бази → атакувати його.
        if let Some(hq) = my_hq {
            let b = self.buildings[hq];
            if let Some(e) = self.nearest_enemy(side, b.cx(), b.cy(), 6 * TILE) {
                let mut mask = 0u32;
                for i in 0..MAX_UNITS {
                    let u = &self.units[i];
                    if u.alive && u.side == side && u.kind != U_WORKER && u.order != O_ATTACK {
                        mask |= 1 << i;
                        self.units[i].order = O_ATTACK;
                        self.units[i].target = e;
                        self.units[i].path_len = 0;
                    }
                }
                let _ = mask;
            }
        }
        // 4b. Атака: армія ≥ 7 і минуло 90 с (перша хвиля не на самому старті).
        if army >= 7 && self.tick_n > 1800 {
            let target_bld = self.hq(enemy);
            if let Some(tb) = target_bld {
                for i in 0..MAX_UNITS {
                    let u = self.units[i];
                    if u.alive && u.side == side && u.kind != U_WORKER && u.order == O_IDLE {
                        self.units[i].order = O_ATTACK_MOVE;
                        self.set_path_to_bld(i, tb);
                    }
                }
            }
        }
    }

    /// Місце під барак: найближче до своєї бази вільне місце 2×2 (детерміновано),
    /// але не впритул до HQ — інакше барак затикає власний вихід з бази.
    fn ai_build_spot(&self, side: u8) -> Option<(i32, i32)> {
        let hq = self.hq(side)?;
        let b = self.buildings[hq];
        let hx = b.tx as i32;
        let hy = b.ty as i32;
        let st = bld_stats(B_BARRACKS);
        // прямокутник HQ, розширений на 1 тайл навколо
        let zx0 = hx - 1;
        let zy0 = hy - 1;
        let zx1 = hx + b.w() + 1;
        let zy1 = hy + b.h() + 1;
        let mut best: Option<(i32, i32, i32)> = None;
        // 1) бажано в смузі головної дороги (ряди 6..8) — юніти швидко доходять до бою
        for band in [(7i32, 8i32, 1i32), (9, 11, 1), (0, MAP_H as i32 - st.h, 0)] {
            for ty in band.0..=band.1.min(MAP_H as i32 - st.h) {
                for tx in 0..(MAP_W as i32 - st.w) {
                    // не чіпати смугу навколо HQ
                    if tx < zx1 && zx0 < tx + st.w && ty < zy1 && zy0 < ty + st.h {
                        continue;
                    }
                    // не перекривати головну дорогу (рядок 6) — інакше власні юніти не пройдуть
                    if band.2 != 0 && ty == 6 {
                        continue;
                    }
                    if !self.can_place(B_BARRACKS, tx, ty) {
                        continue;
                    }
                    let d = (tx - hx).abs() + (ty - hy).abs();
                    if best.is_none() || d < best.unwrap().2 {
                        best = Some((tx, ty, d));
                    }
                }
            }
            if best.is_some() {
                break;
            }
        }
        best.map(|(tx, ty, _)| (tx, ty))
    }

    /// Дешевий хеш стану — для тестів на детермінізм і логів.
    pub fn state_hash(&self) -> u64 {
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        let mut mix = |v: u64| {
            h ^= v;
            h = h.wrapping_mul(0x100_0000_01b3);
        };
        mix(self.tick_n as u64);
        mix(self.minerals[0] as u64);
        mix(self.minerals[1] as u64);
        for i in 0..MAX_UNITS {
            let u = &self.units[i];
            mix(u.alive as u64 + ((u.kind as u64) << 1) + ((u.side as u64) << 3));
            mix(u.x as u64 * 31 + u.y as u64 * 17 + u.hp as u64);
            mix(u.order as u64 * 3 + u.path_i as u64);
        }
        for b in self.buildings.iter() {
            mix(b.alive as u64 + ((b.side as u64) << 1));
            mix(b.hp as u64 * 7 + b.progress as u64 + b.queue_kind as u64 * 13);
        }
        mix(self.over as u64);
        h
    }
}

// ── Самоперевірка ────────────────────────────────────────────────────────────

macro_rules! check {
    ($run:ident, $failed:ident, $cond:expr) => {
        $run += 1;
        if !$cond {
            $failed += 1;
        }
    };
}

pub fn selftest() -> (u32, u32) {
    let mut run = 0u32;
    let mut failed = 0u32;

    // карта
    for r in 0..MAP_H {
        check!(run, failed, MAP_TXT[r].len() == MAP_W);
    }
    let mut minerals = 0;
    let mut rocks = 0;
    for y in 0..MAP_H {
        for x in 0..MAP_W {
            match MAP[y][x] {
                T_ROCK => rocks += 1,
                T_MINERAL => minerals += 1,
                _ => {}
            }
        }
    }
    check!(run, failed, minerals >= 8);
    check!(run, failed, rocks >= 20);
    check!(run, failed, !is_walkable(T_MINERAL));
    check!(run, failed, !is_walkable(T_ROCK));
    check!(run, failed, is_walkable(T_GRASS));

    // стартовий стан
    let g = Game::new();
    check!(run, failed, g.miner_count == 8);
    check!(run, failed, g.count_units(SIDE_BLUE) == 4);
    check!(run, failed, g.count_units(SIDE_RED) == 4);
    check!(run, failed, g.hq(SIDE_BLUE).is_some());
    check!(run, failed, g.hq(SIDE_RED).is_some());
    check!(run, failed, g.over == 0);
    check!(run, failed, unit_stats(U_HEAVY).hp > unit_stats(U_GRUNT).hp);
    check!(run, failed, unit_stats(U_WORKER).dmg == 0);

    // шлях існує через увесь екран
    let mut g2 = Game::new();
    let slot = g2.free_unit_slot();
    g2.spawn_unit(SIDE_BLUE, U_GRUNT, 1, 12, slot);
    g2.set_path_to(slot, tile_index(18, 12));
    check!(run, failed, g2.units[slot].path_len > 0);

    // робітник добуває мінерали
    let mut g3 = Game::new();
    for _ in 0..900 {
        g3.tick();
    }
    check!(run, failed, g3.minerals[SIDE_BLUE as usize] > 0);

    // AI щось будує і виробляє
    let mut g4 = Game::new();
    for t in 0..4000 {
        g4.tick();
        if t % 4 == 0 {
            g4.ai_step(SIDE_BLUE);
            g4.ai_step(SIDE_RED);
        }
    }
    check!(run, failed, g4.army_count(SIDE_RED) + g4.army_count(SIDE_BLUE) > 0);

    (run, failed)
}

// ── Хост-тести ───────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn fresh() -> Game {
        Game::new()
    }

    #[test]
    fn selftest_all_green() {
        let (run, failed) = selftest();
        assert!(run > 20);
        assert_eq!(failed, 0, "{failed} з {run} перевірок впали");
    }

    #[test]
    fn map_matches_text_and_is_symmetric() {
        for y in 0..MAP_H {
            for x in 0..MAP_W {
                let expect = match MAP_TXT[y][x] {
                    b'#' => T_ROCK,
                    b'*' => T_MINERAL,
                    b',' => T_SAND,
                    b'.' => T_GRASS,
                    _ => panic!("невідомий символ"),
                };
                assert_eq!(MAP[y][x], expect, "тайл ({x},{y})");
                assert_eq!(MAP[y][x], MAP[y][MAP_W - 1 - x], "симетрія ({x},{y})");
            }
        }
    }

    #[test]
    fn new_game_has_two_bases_and_eight_workers() {
        let g = fresh();
        assert_eq!(g.count_units(SIDE_BLUE), 4);
        assert_eq!(g.count_units(SIDE_RED), 4);
        let b = g.buildings[g.hq(SIDE_BLUE).unwrap()];
        assert_eq!(b.kind, B_HQ);
        assert!(b.done);
        assert_eq!(b.hp, bld_stats(B_HQ).hp);
    }

    #[test]
    fn base_tiles_are_free_and_units_spawn_on_own_half() {
        let g = fresh();
        assert!(g.can_place(B_BARRACKS, 5, 7), "вільне місце поряд з базою");
        for i in 0..MAX_UNITS {
            let u = &g.units[i];
            if !u.alive {
                continue;
            }
            if u.side == SIDE_BLUE {
                assert!(u.x < MAP_PX_W / 2);
            } else {
                assert!(u.x >= MAP_PX_W / 2);
            }
            assert_eq!(is_walkable(tile_at_px(u.cx(), u.cy())), true);
        }
    }

    #[test]
    fn path_goes_around_rocks() {
        let mut g = fresh();
        let slot = g.free_unit_slot();
        g.spawn_unit(SIDE_BLUE, U_GRUNT, 2, 1, slot);
        // ціль за скельною стіною (рядок 1..3, стовпці 3..7)
        g.set_path_to(slot, tile_index(9, 1));
        g.units[slot].order = O_MOVE;
        assert!(g.units[slot].path_len > 0);
        let path = g.units[slot].path;
        for k in 0..g.units[slot].path_len as usize {
            let i = path[k] as usize;
            assert!(!g.blocked_map()[i], "шлях через перешкоду");
        }
        for _ in 0..600 {
            g.tick();
        }
        let (tx, ty) = g.units[slot].tile();
        assert_eq!((tx, ty), (9, 1), "юніт не дійшов");
    }

    #[test]
    fn move_order_crosses_map() {
        let mut g = fresh();
        let slot = g.free_unit_slot();
        g.spawn_unit(SIDE_BLUE, U_WORKER, 1, 12, slot);
        let mask = 1u32 << slot;
        g.order_at(mask, 18 * TILE + 8, 12 * TILE + 8, SIDE_BLUE);
        assert_eq!(g.units[slot].order, O_ATTACK_MOVE);
        for _ in 0..1200 {
            g.tick();
        }
        let (tx, _) = g.units[slot].tile();
        assert!(tx >= 16, "не дійшов: tx={tx}");
    }

    #[test]
    fn gatherer_brings_minerals_home() {
        let mut g = fresh();
        let before = g.minerals[SIDE_BLUE as usize];
        for _ in 0..1200 {
            g.tick();
        }
        assert!(g.minerals[SIDE_BLUE as usize] > before, "робітники не добувають");
        // усі четверо мають бути зайняті
        for i in 0..MAX_UNITS {
            let u = &g.units[i];
            if u.alive && u.side == SIDE_BLUE {
                assert_eq!(u.order, O_GATHER);
            }
        }
    }

    #[test]
    fn train_worker_costs_and_spawns() {
        let mut g = fresh();
        g.minerals[SIDE_BLUE as usize] = 200;
        let hq = g.hq(SIDE_BLUE).unwrap();
        let n0 = g.count_units(SIDE_BLUE);
        assert!(g.train(hq, U_WORKER));
        assert_eq!(g.minerals[SIDE_BLUE as usize], 150);
        for _ in 0..unit_stats(U_WORKER).train_ticks + 10 {
            g.tick();
        }
        assert_eq!(g.count_units(SIDE_BLUE), n0 + 1);
    }

    #[test]
    fn train_requires_minerals_and_correct_building() {
        let mut g = fresh();
        let hq = g.hq(SIDE_BLUE).unwrap();
        assert!(!g.train(hq, U_WORKER), "без мінералів не можна");
        g.minerals[SIDE_BLUE as usize] = 500;
        assert!(!g.train(hq, U_GRUNT), "HQ не робить ґрантів");
    }

    #[test]
    fn supply_cap_blocks_production() {
        let mut g = fresh();
        g.minerals[SIDE_BLUE as usize] = 5000;
        let hq = g.hq(SIDE_BLUE).unwrap();
        // добиваємо до ліміту
        for _ in 0..(SUPPLY_MAX as usize - g.count_units(SIDE_BLUE) as usize) {
            let slot = g.free_unit_slot();
            g.spawn_unit(SIDE_BLUE, U_WORKER, 3, 10, slot);
        }
        g.buildings[hq].queue_kind = 255;
        assert!(!g.train(hq, U_WORKER));
    }

    #[test]
    fn combat_kills_enemy_worker() {
        let mut g = fresh();
        // забираємо всіх, крім одного червоного робітника
        for i in 0..MAX_UNITS {
            g.units[i].alive = false;
        }
        let a = g.free_unit_slot();
        g.spawn_unit(SIDE_BLUE, U_HEAVY, 5, 6, a);
        let b = g.free_unit_slot();
        g.spawn_unit(SIDE_RED, U_WORKER, 6, 6, b);
        g.units[b].order = O_IDLE;
        g.units[a].order = O_ATTACK;
        g.units[a].target = b as u8;
        for _ in 0..200 {
            g.tick();
        }
        assert!(!g.units[b].alive, "ціль жива");
    }

    #[test]
    fn destroying_hq_wins_the_game() {
        let mut g = fresh();
        let red = g.hq(SIDE_RED).unwrap();
        g.buildings[red].hp = 5;
        for i in 0..MAX_UNITS {
            g.units[i].alive = false;
        }
        let a = g.free_unit_slot();
        g.spawn_unit(SIDE_BLUE, U_HEAVY, 15, 5, a);
        g.units[a].target = TGT_BLD + red as u8;
        g.units[a].order = O_ATTACK;
        for _ in 0..400 {
            g.tick();
        }
        assert_eq!(g.over, 1, "перемога не зарахована");
        assert!(g.over_tick > 0);
    }

    #[test]
    fn worker_builds_barracks() {
        let mut g = fresh();
        g.minerals[SIDE_BLUE as usize] = 500;
        let w = (0..MAX_UNITS)
            .find(|&i| g.units[i].alive && g.units[i].kind == U_WORKER && g.units[i].side == SIDE_BLUE)
            .unwrap();
        assert!(g.place(SIDE_BLUE, B_BARRACKS, 5, 7, w));
        let bi = (0..MAX_BUILDINGS)
            .find(|&i| g.buildings[i].alive && g.buildings[i].kind == B_BARRACKS)
            .unwrap();
        for _ in 0..bld_stats(B_BARRACKS).build_ticks + 400 {
            g.tick();
        }
        assert!(g.buildings[bi].done, "барак не добудований");
        assert_eq!(g.buildings[bi].hp, bld_stats(B_BARRACKS).hp);
    }

    #[test]
    fn cannot_place_on_rock_or_over_base() {
        let g = fresh();
        assert!(!g.can_place(B_BARRACKS, 3, 1), "скеля");
        assert!(!g.can_place(B_BARRACKS, 0, 4), "на базі");
        assert!(!g.can_place(B_BARRACKS, 19, 12), "вихід за карту");
    }

    #[test]
    fn ai_builds_barracks_and_army() {
        let mut g = fresh();
        for t in 0..6000 {
            g.tick();
            if t % 4 == 0 {
                g.ai_step(SIDE_RED);
            }
        }
        assert!(g.buildings.iter().any(|b| b.alive && b.side == SIDE_RED && b.kind == B_BARRACKS), "AI не збудував барак");
        assert!(g.army_count(SIDE_RED) >= 2, "AI не зробив армію");
    }

    #[test]
    fn ai_vs_ai_makes_progress() {
        let mut g = fresh();
        let mut hq_damaged = false;
        for t in 0..12000 {
            g.tick();
            if t % 4 == 0 {
                g.ai_step(SIDE_BLUE);
                g.ai_step(SIDE_RED);
            }
            for b in g.buildings.iter() {
                if b.alive && b.kind == B_HQ && b.hp < bld_stats(B_HQ).hp {
                    hq_damaged = true;
                }
            }
            if g.over != 0 {
                break;
            }
        }
        assert!(
            hq_damaged || g.over != 0 || g.army_count(SIDE_BLUE) + g.army_count(SIDE_RED) >= 4,
            "матч стоїть на місці"
        );
    }

    #[test]
    fn deterministic_for_same_script() {
        let mut a = fresh();
        let mut b = fresh();
        for t in 0..1500 {
            a.tick();
            b.tick();
            if t % 4 == 0 {
                a.ai_step(SIDE_RED);
                b.ai_step(SIDE_RED);
            }
        }
        assert_eq!(a.state_hash(), b.state_hash());
    }

    #[test]
    fn selection_helpers() {
        let g = fresh();
        let i = (0..MAX_UNITS)
            .find(|&i| g.units[i].alive && g.units[i].side == SIDE_BLUE)
            .unwrap();
        let u = g.units[i];
        assert_eq!(g.unit_at(u.x + 2, u.y + 2), Some(i));
        let mask = g.units_in_rect(SIDE_BLUE, 0, 100, 60, 130);
        assert!(mask & (1 << i) != 0);
        let wmask = g.mask_of_kind(SIDE_BLUE, U_WORKER);
        assert_eq!(Game::mask_count(wmask), 4);
        // будівля
        let hq = g.hq(SIDE_BLUE).unwrap();
        let b = g.buildings[hq];
        let (px, py) = b.px();
        assert_eq!(g.building_at(px + 4, py + 4), Some(hq));
    }

    #[test]
    fn stop_clears_orders() {
        let mut g = fresh();
        let i = (0..MAX_UNITS)
            .find(|&i| g.units[i].alive && g.units[i].side == SIDE_BLUE)
            .unwrap();
        g.order_at(1 << i, 5 * TILE, 12 * TILE, SIDE_BLUE);
        assert_ne!(g.units[i].order, O_IDLE);
        g.stop(1 << i);
        assert_eq!(g.units[i].order, O_IDLE);
        assert_eq!(g.units[i].path_len, 0);
    }

    #[test]
    fn attack_order_on_enemy_unit() {
        let mut g = fresh();
        let a = (0..MAX_UNITS)
            .find(|&i| g.units[i].alive && g.units[i].side == SIDE_BLUE)
            .unwrap();
        g.units[a].kind = U_GRUNT;
        let b = (0..MAX_UNITS)
            .find(|&i| g.units[i].alive && g.units[i].side == SIDE_RED)
            .unwrap();
        let (bx, by) = (g.units[b].x + 5, g.units[b].y + 5);
        let attacked = g.order_at(1 << a, bx, by, SIDE_BLUE);
        assert!(attacked);
        assert_eq!(g.units[a].order, O_ATTACK);
        assert_eq!(g.units[a].target, b as u8);
    }

    #[test]
    fn hud_fits_screen() {
        assert_eq!(MAP_PX_H + HUD_H, 240);
        assert_eq!(MAP_PX_W, 320);
        assert_eq!(MAX_UNITS, 24);
    }
}
