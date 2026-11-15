//! `GARDEN DEF` — tower defense «грядка проти зомбі» (у стилі Plants vs Zombies).
//!
//! Це **чиста логіка**: без `std`, `alloc` і `esp-hal`, тому файл компілюється
//! звичайним `rustc --test` на хості (`tools/test-host.sh`).
//!
//! Модель світу:
//! - грядка 9×5 клітинок по 32×36 px (288×180), HUD зверху;
//! - один ресурс — **сонце**: падає з неба, його дають соняшники, збирається тапом;
//! - 5 рослин: соняшник, горохостріл, волоський горіх, крижаний горох, вишнева бомба;
//! - 4 зомбі: ходячий, конусник, бакетник, бігун; 12 хвиль, далі — перемога;
//! - горизонтальна позиція зомбі — у **міліклітинках** (`XM = 1000`): 0 — лівий край
//!   грядки, `XM_RIGHT` — права лінія появи. Так логіка не залежить від пікселів;
//! - 1 тік = 50 мс (20 Гц), RNG — детермінований xorshift32 (сід із `Board::rng`);
//! - косарка в кожному ряду — один безкоштовний порятунок ряду, далі поразка.

#![allow(dead_code)]

// ── Геометрія ────────────────────────────────────────────────────────────────

/// Рядів на грядці (ряди = «доріжки» зомбі).
pub const LANES: usize = 5;
/// Стовпців на грядці.
pub const COLS: usize = 9;
/// Усього клітинок.
pub const CELLS: usize = LANES * COLS;

/// Ширина клітинки, px.
pub const CELL_W: i32 = 32;
/// Висота клітинки, px.
pub const CELL_H: i32 = 36;
/// Ширина грядки, px (288).
pub const PLAY_W: i32 = COLS as i32 * CELL_W;
/// Висота грядки, px (180).
pub const PLAY_H: i32 = LANES as i32 * CELL_H;

/// Один тік симуляції, мс.
pub const TICK_MS: u32 = 50;
/// Тіків за секунду.
pub const TICKS_PER_SEC: u32 = 20;

/// Міліклітинок в одній клітинці (позиції зомбі/горошин).
pub const XM: i32 = 1000;
/// Права межа грядки в міліклітинках — лінія появи зомбі.
pub const XM_RIGHT: i32 = COLS as i32 * XM;
/// Швидкість горошини, міліклітинок за тік (12 клітинок/с).
pub const PEA_SPEED: i32 = 240;
/// Швидкість косарки, міліклітинок за тік (6 клітинок/с).
pub const MOWER_SPEED: i32 = 60;
/// За скільки міліклітинок до лівого краю косарка стартує сама.
pub const MOWER_TRIGGER: i32 = 320;

// ── Економіка ────────────────────────────────────────────────────────────────

/// Сонце на старті.
pub const START_SUN: u16 = 175;
/// Скільки дає одна сонячна кулька.
pub const SUN_VALUE: u16 = 25;
/// Період появи сонця з неба, тіків (20 с).
pub const SKY_SUN_TICKS: u16 = 300;
/// Скільки тіків кулька сонця лежить на землі (24 с).
pub const SUN_TTL: u16 = 480;

// ── Бій ──────────────────────────────────────────────────────────────────────

/// Урон звичайної горошини.
pub const PEA_DMG: u16 = 25;
/// Урон крижаної горошини.
pub const ICE_DMG: u16 = 25;
/// Скільки тіків діє сповільнення від крижаної горошини (4 с).
pub const ICE_SLOW: u16 = 80;
/// Радіус ураження вишневої бомби, міліклітинок (1.4 клітинки).
pub const CHERRY_RADIUS: i32 = 1400;
/// Урон вишневої бомби — вбиває все, крім бакетника з повним HP.
pub const CHERRY_DMG: u16 = 1200;
/// Запал вишневої бомби, тіків.
pub const CHERRY_FUSE: u16 = 8;
/// Радіус попадання горошини, міліклітинок.
pub const PEA_HIT: i32 = 260;

pub const MAX_ZOMBIES: usize = 48;
pub const MAX_PEAS: usize = 40;
pub const MAX_SUNS: usize = 16;
pub const MAX_FX: usize = 24;

// ── Види рослин ──────────────────────────────────────────────────────────────

pub const P_SUNFLOWER: u8 = 0;
pub const P_PEASHOOTER: u8 = 1;
pub const P_WALLNUT: u8 = 2;
pub const P_ICEPEA: u8 = 3;
pub const P_CHERRY: u8 = 4;
pub const PLANT_KINDS: usize = 5;

/// Характеристики рослини.
pub struct PlantStats {
    /// Ціна в сонці.
    pub cost: u16,
    /// Запас HP.
    pub hp: u16,
    /// Перезарядка пакета насіння, тіків.
    pub recharge: u16,
    /// Період дії (постріл / сонце / запал), тіків; 0 — дії немає.
    pub period: u16,
}

pub const fn plant_stats(kind: u8) -> PlantStats {
    match kind {
        P_PEASHOOTER => PlantStats { cost: 100, hp: 120, recharge: 140, period: 28 },
        P_WALLNUT => PlantStats { cost: 50, hp: 600, recharge: 380, period: 0 },
        P_ICEPEA => PlantStats { cost: 175, hp: 120, recharge: 300, period: 30 },
        P_CHERRY => PlantStats { cost: 150, hp: 40, recharge: 480, period: CHERRY_FUSE },
        // соняшник
        _ => PlantStats { cost: 50, hp: 120, recharge: 140, period: 400 },
    }
}

#[derive(Clone, Copy)]
pub struct Plant {
    pub alive: bool,
    pub kind: u8,
    pub col: u8,
    pub row: u8,
    pub hp: u16,
    /// Лічильник дії (постріл/сонце/запал).
    pub t: u16,
    /// Спалах від укусу (для рендера), тіків.
    pub flash: u8,
}

impl Plant {
    pub const EMPTY: Self = Self { alive: false, kind: 0, col: 0, row: 0, hp: 0, t: 0, flash: 0 };
}

// ── Види зомбі ───────────────────────────────────────────────────────────────

pub const Z_BASIC: u8 = 0;
pub const Z_CONE: u8 = 1;
pub const Z_BUCKET: u8 = 2;
pub const Z_RUNNER: u8 = 3;
pub const ZOMBIE_KINDS: usize = 4;

/// Характеристики зомбі.
pub struct ZombieStats {
    /// Запас HP.
    pub hp: u16,
    /// Швидкість, міліклітинок за тік.
    pub speed: i32,
    /// Урон рослині за тік укусу.
    pub bite: u16,
    /// Очки за вбивство.
    pub score: u32,
}

pub const fn zombie_stats(kind: u8) -> ZombieStats {
    match kind {
        Z_CONE => ZombieStats { hp: 560, speed: 11, bite: 2, score: 20 },
        Z_BUCKET => ZombieStats { hp: 1200, speed: 10, bite: 2, score: 40 },
        Z_RUNNER => ZombieStats { hp: 220, speed: 18, bite: 2, score: 25 },
        // ходячий
        _ => ZombieStats { hp: 200, speed: 10, bite: 2, score: 10 },
    }
}

#[derive(Clone, Copy)]
pub struct Zombie {
    pub alive: bool,
    pub kind: u8,
    pub row: u8,
    /// Позиція в міліклітинках (центр зомбі).
    pub xm: i32,
    pub hp: u16,
    pub max_hp: u16,
    /// Сповільнення від крижаної горошини, тіків.
    pub slow_t: u16,
    /// Анімація укусу, тіків.
    pub bite_t: u8,
    /// Спалах від попадання, тіків.
    pub flash: u8,
    /// Фаза кроку для рендера.
    pub anim: u8,
}

impl Zombie {
    pub const EMPTY: Self = Self {
        alive: false,
        kind: 0,
        row: 0,
        xm: 0,
        hp: 0,
        max_hp: 0,
        slow_t: 0,
        bite_t: 0,
        flash: 0,
        anim: 0,
    };
}

#[derive(Clone, Copy)]
pub struct Pea {
    pub alive: bool,
    pub row: u8,
    pub xm: i32,
    pub dmg: u16,
    pub ice: bool,
}

impl Pea {
    pub const EMPTY: Self = Self { alive: false, row: 0, xm: 0, dmg: 0, ice: false };
}

/// Кулька сонця (координати — у пікселях грядки, не екрана).
#[derive(Clone, Copy)]
pub struct Sun {
    pub alive: bool,
    pub x: i32,
    pub y: i32,
    /// Куди падає.
    pub target_y: i32,
    pub ttl: u16,
    pub value: u16,
}

impl Sun {
    pub const EMPTY: Self = Self { alive: false, x: 0, y: 0, target_y: 0, ttl: 0, value: 0 };
}

pub const FX_HIT: u8 = 0;
pub const FX_BOOM: u8 = 1;
pub const FX_SUN: u8 = 2;
pub const FX_MOW: u8 = 3;

#[derive(Clone, Copy)]
pub struct Fx {
    pub alive: bool,
    pub x: i32,
    pub y: i32,
    pub ttl: u8,
    pub kind: u8,
}

impl Fx {
    pub const EMPTY: Self = Self { alive: false, x: 0, y: 0, ttl: 0, kind: 0 };
}

// ── Косарки ──────────────────────────────────────────────────────────────────

pub const MOWER_READY: u8 = 0;
pub const MOWER_RUN: u8 = 1;
pub const MOWER_USED: u8 = 2;

// ── Хвилі ────────────────────────────────────────────────────────────────────

/// Скільки хвиль треба пережити для перемоги.
pub const WAVES: u16 = 12;
/// Перша хвиля — через 30 с (час поставити соняшники).
pub const FIRST_WAVE_TICKS: u32 = 600;

/// Скільки зомбі в хвилі `w` (1..=WAVES).
pub const fn wave_count(w: u16) -> u16 {
    2 + w
}

/// Пауза між хвилями, тіків.
pub const fn wave_interval(w: u16) -> u32 {
    let v = 900u32.saturating_sub(40 * w as u32);
    if v < 420 { 420 } else { v }
}

/// Інтервал між зомбі всередині хвилі, тіків.
pub const fn wave_spread(w: u16) -> u16 {
    let v = 900 / wave_count(w) as u32;
    if v < 14 { 14 } else { v as u16 }
}

/// Маска видів зомбі, які з'являються у хвилі `w`.
pub const fn wave_kinds(w: u16) -> u16 {
    let mut m = 1u16 << Z_BASIC;
    if w >= 3 {
        m |= 1 << Z_CONE;
    }
    if w >= 4 {
        m |= 1 << Z_RUNNER;
    }
    if w >= 6 {
        m |= 1 << Z_BUCKET;
    }
    m
}

/// Стан партії: 0 — гра, 1 — перемога, 2 — поразка.
pub const OVER_NONE: u8 = 0;
pub const OVER_WIN: u8 = 1;
pub const OVER_LOSE: u8 = 2;

// ── Гра ──────────────────────────────────────────────────────────────────────

pub struct Game {
    pub sun: u16,
    pub plants: [Plant; CELLS],
    pub zombies: [Zombie; MAX_ZOMBIES],
    pub peas: [Pea; MAX_PEAS],
    pub suns: [Sun; MAX_SUNS],
    pub fx: [Fx; MAX_FX],
    /// Стан косарки в кожному ряду.
    pub mower: [u8; LANES],
    /// Позиція косарки, що їде, міліклітинок.
    pub mower_xm: [i32; LANES],
    /// Перезарядка пакетів насіння, тіків.
    pub cd: [u16; PLANT_KINDS],
    pub tick_n: u32,
    /// Поточна хвиля (0 — ще не було).
    pub wave: u16,
    /// Тіків до початку наступної хвилі.
    pub wave_t: u32,
    /// Скільки зомбі ще не з'явилося в поточній хвилі.
    pub spawn_left: u16,
    /// Тіків до появи наступного зомбі хвилі.
    pub spawn_t: u16,
    /// Маска видів зомбі поточної хвилі.
    pub spawn_kinds: u16,
    /// Лічильник появи сонця з неба.
    pub sky_t: u16,
    pub kills: u32,
    pub score: u32,
    pub over: u8,
    /// Тік, на якому партія завершилася.
    pub over_tick: u32,
    pub seed: u32,
}

impl Game {
    pub fn new(seed: u32) -> Self {
        let s = if seed == 0 { 0x9E37_79B9 } else { seed };
        Self {
            sun: START_SUN,
            plants: [Plant::EMPTY; CELLS],
            zombies: [Zombie::EMPTY; MAX_ZOMBIES],
            peas: [Pea::EMPTY; MAX_PEAS],
            suns: [Sun::EMPTY; MAX_SUNS],
            fx: [Fx::EMPTY; MAX_FX],
            mower: [MOWER_READY; LANES],
            mower_xm: [0; LANES],
            cd: [0; PLANT_KINDS],
            tick_n: 0,
            wave: 0,
            wave_t: FIRST_WAVE_TICKS,
            spawn_left: 0,
            spawn_t: 0,
            spawn_kinds: 1 << Z_BASIC,
            sky_t: SKY_SUN_TICKS - 220,
            kills: 0,
            score: 0,
            over: OVER_NONE,
            over_tick: 0,
            seed: s,
        }
    }

    #[inline]
    pub fn rng(&mut self) -> u32 {
        let mut x = self.seed;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.seed = x;
        x
    }

    // ── Доступ до клітинок ──────────────────────────────────────────────────

    pub fn cell_index(col: i32, row: i32) -> Option<usize> {
        if col < 0 || row < 0 || col >= COLS as i32 || row >= LANES as i32 {
            None
        } else {
            Some(row as usize * COLS + col as usize)
        }
    }

    #[inline]
    pub fn plant_at(&self, col: i32, row: i32) -> Option<usize> {
        let i = Self::cell_index(col, row)?;
        if self.plants[i].alive { Some(i) } else { None }
    }

    /// Центр клітинки в пікселях грядки.
    pub fn cell_center_px(col: i32, row: i32) -> (i32, i32) {
        (
            col * CELL_W + CELL_W / 2,
            row * CELL_H + CELL_H / 2,
        )
    }

    // ── Економіка / посадка ─────────────────────────────────────────────────

    #[inline]
    pub fn packet_ready(&self, kind: u8) -> bool {
        self.cd[kind as usize] == 0
    }

    #[inline]
    pub fn afford(&self, kind: u8) -> bool {
        (kind as usize) < PLANT_KINDS
            && self.cd[kind as usize] == 0
            && self.sun >= plant_stats(kind).cost
    }

    /// Посадити рослину. Повертає `true`, якщо посаджено.
    pub fn place(&mut self, col: i32, row: i32, kind: u8) -> bool {
        if kind as usize >= PLANT_KINDS {
            return false;
        }
        let Some(i) = Self::cell_index(col, row) else {
            return false;
        };
        if self.plants[i].alive || !self.afford(kind) {
            return false;
        }
        let st = plant_stats(kind);
        self.sun -= st.cost;
        self.cd[kind as usize] = st.recharge;
        self.plants[i] = Plant {
            alive: true,
            kind,
            col: col as u8,
            row: row as u8,
            hp: st.hp,
            t: 0,
            flash: 0,
        };
        true
    }

    /// Прибрати рослину (без повернення сонця) — для тестів і «лопати».
    pub fn remove(&mut self, col: i32, row: i32) -> bool {
        match self.plant_at(col, row) {
            Some(i) => {
                self.plants[i].alive = false;
                true
            }
            None => false,
        }
    }

    fn spawn_sun(&mut self, x: i32, y: i32, target_y: i32, value: u16) {
        for s in self.suns.iter_mut() {
            if !s.alive {
                *s = Sun { alive: true, x, y, target_y, ttl: SUN_TTL, value };
                return;
            }
        }
    }

    /// Індекс кульки сонця під точкою (координати грядки).
    pub fn sun_at(&self, px: i32, py: i32) -> Option<usize> {
        for (i, s) in self.suns.iter().enumerate() {
            if !s.alive {
                continue;
            }
            let dx = s.x - px;
            let dy = s.y - py;
            if dx * dx + dy * dy <= 22 * 22 {
                return Some(i);
            }
        }
        None
    }

    /// Зібрати сонце під пальцем. Повертає `true`, якщо щось зібрано.
    pub fn collect_sun(&mut self, px: i32, py: i32) -> bool {
        match self.sun_at(px, py) {
            Some(i) => {
                let s = self.suns[i];
                self.sun = self.sun.saturating_add(s.value);
                self.suns[i].alive = false;
                self.spawn_fx(s.x, s.y, FX_SUN);
                true
            }
            None => false,
        }
    }

    /// Зібрати всі кульки (автопілот / UART-команда).
    pub fn collect_all_suns(&mut self) -> u16 {
        let mut n = 0;
        for i in 0..MAX_SUNS {
            if self.suns[i].alive {
                let s = self.suns[i];
                self.sun = self.sun.saturating_add(s.value);
                self.suns[i].alive = false;
                self.spawn_fx(s.x, s.y, FX_SUN);
                n += 1;
            }
        }
        n
    }

    fn spawn_fx(&mut self, x: i32, y: i32, kind: u8) {
        for f in self.fx.iter_mut() {
            if !f.alive {
                *f = Fx { alive: true, x, y, ttl: 12, kind };
                return;
            }
        }
    }

    // ── Зомбі ───────────────────────────────────────────────────────────────

    pub fn spawn_zombie(&mut self, kind: u8, row: usize) -> bool {
        if kind as usize >= ZOMBIE_KINDS || row >= LANES {
            return false;
        }
        let st = zombie_stats(kind);
        for i in 0..MAX_ZOMBIES {
            if !self.zombies[i].alive {
                self.zombies[i] = Zombie {
                    alive: true,
                    kind,
                    row: row as u8,
                    xm: XM_RIGHT,
                    hp: st.hp,
                    max_hp: st.hp,
                    slow_t: 0,
                    bite_t: 0,
                    flash: 0,
                    anim: (self.rng() & 7) as u8,
                };
                return true;
            }
        }
        false
    }

    pub fn alive_zombies(&self) -> usize {
        self.zombies.iter().filter(|z| z.alive).count()
    }

    pub fn zombies_in_lane(&self, row: usize) -> usize {
        self.zombies.iter().filter(|z| z.alive && z.row as usize == row).count()
    }

    /// Найближчий до лівого краю зомбі в ряду.
    pub fn front_zombie(&self, row: usize) -> Option<Zombie> {
        let mut best: Option<Zombie> = None;
        for z in self.zombies.iter() {
            if z.alive && z.row as usize == row && best.is_none_or(|b| z.xm < b.xm) {
                best = Some(*z);
            }
        }
        best
    }

    fn damage_zombie(&mut self, zi: usize, dmg: u16, slow: bool) {
        if !self.zombies[zi].alive {
            return;
        }
        let hp = self.zombies[zi].hp.saturating_sub(dmg);
        self.zombies[zi].hp = hp;
        self.zombies[zi].flash = 4;
        if slow {
            self.zombies[zi].slow_t = ICE_SLOW;
        }
        if hp == 0 {
            let z = self.zombies[zi];
            self.zombies[zi].alive = false;
            self.kills += 1;
            self.score += zombie_stats(z.kind).score;
            let x = z.xm * CELL_W / XM;
            let y = z.row as i32 * CELL_H + CELL_H / 2;
            self.spawn_fx(x, y, FX_HIT);
        }
    }

    fn spawn_pea(&mut self, col: i32, row: i32, ice: bool) {
        for p in self.peas.iter_mut() {
            if !p.alive {
                *p = Pea {
                    alive: true,
                    row: row as u8,
                    xm: col * XM + XM * 3 / 4,
                    dmg: if ice { ICE_DMG } else { PEA_DMG },
                    ice,
                };
                return;
            }
        }
    }

    /// Чи є в ряду зомбі праворуч від `px_m` (тобто є куди стріляти).
    fn lane_has_target(&self, row: u8, px_m: i32) -> bool {
        self.zombies
            .iter()
            .any(|z| z.alive && z.row == row && z.xm >= px_m - 200)
    }

    // ── Вибух вишні ─────────────────────────────────────────────────────────

    fn detonate(&mut self, i: usize) {
        let p = self.plants[i];
        let (cx, cy) = Self::cell_center_px(p.col as i32, p.row as i32);
        self.plants[i].alive = false;
        self.spawn_fx(cx, cy, FX_BOOM);
        let px_m = p.col as i32 * XM + XM / 2;
        for zi in 0..MAX_ZOMBIES {
            let z = self.zombies[zi];
            if !z.alive {
                continue;
            }
            let dr = (z.row as i32 - p.row as i32).abs();
            if dr <= 1 && (z.xm - px_m).abs() <= CHERRY_RADIUS {
                self.damage_zombie(zi, CHERRY_DMG, false);
            }
        }
    }

    // ── Головний тік ────────────────────────────────────────────────────────

    pub fn tick(&mut self) {
        if self.over != OVER_NONE {
            self.over_tick = self.over_tick.saturating_add(1);
            return;
        }
        self.tick_n += 1;

        for c in self.cd.iter_mut() {
            if *c > 0 {
                *c -= 1;
            }
        }

        self.tick_sky_sun();
        self.tick_plants();
        self.tick_peas();
        self.tick_suns();
        self.tick_zombies();
        if self.over != OVER_NONE {
            return;
        }
        self.tick_mowers();
        self.tick_fx();
        self.tick_waves();
    }

    fn tick_sky_sun(&mut self) {
        self.sky_t += 1;
        if self.sky_t >= SKY_SUN_TICKS {
            self.sky_t = 0;
            let x = 20 + (self.rng() % (PLAY_W as u32 - 40)) as i32;
            let ty = 30 + (self.rng() % (PLAY_H as u32 - 50)) as i32;
            self.spawn_sun(x, -12, ty, SUN_VALUE);
        }
    }

    fn tick_plants(&mut self) {
        for i in 0..CELLS {
            let p = self.plants[i];
            if !p.alive {
                continue;
            }
            let t = p.t.saturating_add(1);
            self.plants[i].t = t;
            if self.plants[i].flash > 0 {
                self.plants[i].flash -= 1;
            }
            let st = plant_stats(p.kind);
            match p.kind {
                P_CHERRY => {
                    if t >= st.period {
                        self.detonate(i);
                    }
                }
                P_SUNFLOWER => {
                    if t >= st.period {
                        self.plants[i].t = 0;
                        let (cx, cy) = Self::cell_center_px(p.col as i32, p.row as i32);
                        self.spawn_sun(cx, cy, cy, SUN_VALUE);
                    }
                }
                P_PEASHOOTER | P_ICEPEA => {
                    if t >= st.period && self.lane_has_target(p.row, p.col as i32 * XM + XM / 2) {
                        self.plants[i].t = 0;
                        self.spawn_pea(p.col as i32, p.row as i32, p.kind == P_ICEPEA);
                    }
                }
                _ => {}
            }
        }
    }

    fn tick_peas(&mut self) {
        for i in 0..MAX_PEAS {
            let pea = self.peas[i];
            if !pea.alive {
                continue;
            }
            let xm = pea.xm + PEA_SPEED;
            self.peas[i].xm = xm;
            if xm > XM_RIGHT + XM {
                self.peas[i].alive = false;
                continue;
            }
            // найлівіший зомбі в зоні попадання
            let mut hit: Option<usize> = None;
            for zi in 0..MAX_ZOMBIES {
                let z = self.zombies[zi];
                if !z.alive || z.row != pea.row {
                    continue;
                }
                if (z.xm - xm).abs() <= PEA_HIT && hit.is_none_or(|h| z.xm < self.zombies[h].xm) {
                    hit = Some(zi);
                }
            }
            if let Some(zi) = hit {
                self.peas[i].alive = false;
                self.damage_zombie(zi, pea.dmg, pea.ice);
                let x = xm * CELL_W / XM;
                let y = pea.row as i32 * CELL_H + CELL_H / 2;
                self.spawn_fx(x, y, FX_HIT);
            }
        }
    }

    fn tick_suns(&mut self) {
        for i in 0..MAX_SUNS {
            let s = self.suns[i];
            if !s.alive {
                continue;
            }
            if self.suns[i].y < s.target_y {
                self.suns[i].y = (self.suns[i].y + 6).min(s.target_y);
            }
            if self.suns[i].ttl > 0 {
                self.suns[i].ttl -= 1;
            } else {
                self.suns[i].alive = false;
            }
        }
    }

    fn tick_zombies(&mut self) {
        let mut lost = false;
        for zi in 0..MAX_ZOMBIES {
            let z = self.zombies[zi];
            if !z.alive {
                continue;
            }
            let st = zombie_stats(z.kind);
            let slow = self.zombies[zi].slow_t > 0;
            if slow {
                self.zombies[zi].slow_t -= 1;
            }
            if self.zombies[zi].flash > 0 {
                self.zombies[zi].flash -= 1;
            }
            if self.zombies[zi].bite_t > 0 {
                self.zombies[zi].bite_t -= 1;
            }

            // їсти рослину в клітинці, де стоїть
            let cell = z.xm.div_euclid(XM);
            let mut eating = false;
            if let Some(pi) = Self::cell_index(cell, z.row as i32) {
                if self.plants[pi].alive {
                    eating = true;
                    self.zombies[zi].bite_t = 8;
                    let hp = self.plants[pi].hp.saturating_sub(st.bite);
                    self.plants[pi].hp = hp;
                    self.plants[pi].flash = 6;
                    if hp == 0 {
                        self.plants[pi].alive = false;
                        let (cx, cy) = Self::cell_center_px(cell, z.row as i32);
                        self.spawn_fx(cx, cy, FX_HIT);
                    }
                }
            }

            if !eating {
                let step = if slow { (st.speed / 2).max(1) } else { st.speed };
                self.zombies[zi].xm -= step;
                self.zombies[zi].anim = self.zombies[zi].anim.wrapping_add(1);
            }

            let lane = z.row as usize;
            let zxm = self.zombies[zi].xm;
            if zxm <= 0 {
                // зомбі дійшов до будинку
                if self.mower[lane] == MOWER_READY {
                    self.start_mower(lane);
                } else if self.mower[lane] != MOWER_RUN {
                    lost = true;
                }
            } else if zxm <= MOWER_TRIGGER && self.mower[lane] == MOWER_READY {
                self.start_mower(lane);
            }
        }
        if lost {
            self.over = OVER_LOSE;
            self.over_tick = self.tick_n;
        }
    }

    fn start_mower(&mut self, lane: usize) {
        self.mower[lane] = MOWER_RUN;
        self.mower_xm[lane] = 0;
    }

    fn tick_mowers(&mut self) {
        for lane in 0..LANES {
            if self.mower[lane] != MOWER_RUN {
                continue;
            }
            self.mower_xm[lane] += MOWER_SPEED;
            let mx = self.mower_xm[lane];
            for zi in 0..MAX_ZOMBIES {
                let z = self.zombies[zi];
                if z.alive && z.row as usize == lane && (z.xm - mx).abs() <= 500 {
                    self.damage_zombie(zi, 5000, false);
                }
            }
            if mx > XM_RIGHT + XM / 2 {
                self.mower[lane] = MOWER_USED;
            }
        }
    }

    fn tick_fx(&mut self) {
        for f in self.fx.iter_mut() {
            if !f.alive {
                continue;
            }
            if f.ttl > 0 {
                f.ttl -= 1;
            } else {
                f.alive = false;
            }
        }
    }

    fn tick_waves(&mut self) {
        if self.spawn_left > 0 {
            if self.spawn_t > 0 {
                self.spawn_t -= 1;
            } else {
                self.spawn_wave_zombie();
                self.spawn_left -= 1;
                self.spawn_t = wave_spread(self.wave);
            }
            return;
        }

        if self.wave >= WAVES {
            if self.alive_zombies() == 0 {
                self.over = OVER_WIN;
                self.over_tick = self.tick_n;
            }
            return;
        }

        if self.wave_t > 0 {
            self.wave_t -= 1;
        }
        if self.wave_t == 0 {
            self.begin_wave();
        }
    }

    fn begin_wave(&mut self) {
        self.wave += 1;
        self.spawn_left = wave_count(self.wave);
        self.spawn_kinds = wave_kinds(self.wave);
        self.spawn_t = 0;
        self.wave_t = wave_interval(self.wave);
    }

    fn spawn_wave_zombie(&mut self) {
        let row = (self.rng() % LANES as u32) as usize;
        let roll = (self.rng() % 100) as u16;
        let kind = if self.spawn_kinds & (1 << Z_BUCKET) != 0 && roll < 12 {
            Z_BUCKET
        } else if self.spawn_kinds & (1 << Z_CONE) != 0 && roll < 45 {
            Z_CONE
        } else if self.spawn_kinds & (1 << Z_RUNNER) != 0 && roll < 65 {
            Z_RUNNER
        } else {
            Z_BASIC
        };
        // шанс підвищити рівень зомбі у пізніх хвилях
        let kind = if kind == Z_BASIC && self.spawn_kinds & (1 << Z_CONE) != 0 && roll > 85 {
            Z_CONE
        } else {
            kind
        };
        self.spawn_zombie(kind, row);
    }

    /// Хвиля, що триває, у відсотках (0..100) — для HUD.
    pub fn wave_progress_pct(&self) -> u8 {
        if self.wave == 0 {
            // підготовка: скільки лишилося до першої хвилі
            let left = self.wave_t.min(FIRST_WAVE_TICKS);
            return (100 - (left * 100 / FIRST_WAVE_TICKS)) as u8;
        }
        if self.wave >= WAVES && self.spawn_left == 0 {
            return 100;
        }
        let total = wave_count(self.wave) as u32;
        let left = self.spawn_left as u32;
        (100 - left * 100 / total.max(1)) as u8
    }

    // ── Автопілот (DEMO) ────────────────────────────────────────────────────

    fn count_plants(&self, kind: u8) -> usize {
        self.plants.iter().filter(|p| p.alive && p.kind == kind).count()
    }

    fn plants_in_cell_count(&self) -> usize {
        self.plants.iter().filter(|p| p.alive).count()
    }

    /// Скільки зомбі в ряду ще праворуч від 2-ї клітинки (тих, що реально загрожують).
    fn lane_pressure(&self, row: usize) -> usize {
        self.zombies
            .iter()
            .filter(|z| z.alive && z.row as usize == row && z.xm > 2 * XM)
            .count()
    }

    /// Чи є в ряду стрілець (горохостріл або крижаний) у стовпці `col`.
    fn has_shooter_at(&self, col: i32, row: i32) -> bool {
        match self.plant_at(col, row) {
            Some(i) => matches!(self.plants[i].kind, P_PEASHOOTER | P_ICEPEA),
            None => false,
        }
    }

    fn free_in_col(&self, col: i32, rows: &[i32]) -> Option<i32> {
        for &r in rows {
            if self.plant_at(col, r).is_none() {
                return Some(r);
            }
        }
        None
    }

    /// Один крок автопілота: зібрати сонце, посадити щось корисне.
    pub fn ai_step(&mut self) {
        if self.over != OVER_NONE {
            return;
        }
        self.collect_all_suns();

        // 1. Вишня по скупченню зомбі (3+ у радіусі 1.5 клітинки).
        if self.afford(P_CHERRY) {
            let mut best: Option<(i32, i32, usize)> = None;
            for row in 0..LANES as i32 {
                for col in 1..COLS as i32 {
                    if self.plant_at(col, row).is_some() {
                        continue;
                    }
                    let (cx, _) = Self::cell_center_px(col, row);
                    let n = self
                        .zombies
                        .iter()
                        .filter(|z| {
                            z.alive
                                && (z.row as i32 - row).abs() <= 1
                                && (z.xm - cx * XM / CELL_W).abs() <= CHERRY_RADIUS
                        })
                        .count();
                    if n >= 3 && best.is_none_or(|b| n > b.2) {
                        best = Some((col, row, n));
                    }
                }
            }
            if let Some((col, row, _)) = best {
                self.place(col, row, P_CHERRY);
                return;
            }
        }

        // 2. Економіка: спершу 3 соняшники — далі все інше.
        let sunflowers = self.count_plants(P_SUNFLOWER);
        if sunflowers < 3 && self.afford(P_SUNFLOWER) {
            if let Some((col, row)) = self.pick_sunflower() {
                self.place(col, row, P_SUNFLOWER);
                return;
            }
        }

        // 3. Хоч по одному стрільцю в кожен ряд (стовпці 2..4).
        let shooters = self.count_plants(P_PEASHOOTER) + self.count_plants(P_ICEPEA);
        if shooters < LANES as usize + 2 && self.afford(P_PEASHOOTER) {
            if let Some((col, row)) = self.pick_shooter() {
                self.place(col, row, P_PEASHOOTER);
                return;
            }
        }

        // 3b. Другий-третій соняшник після першого кола оборони.
        if sunflowers < 6 && self.afford(P_SUNFLOWER) {
            if let Some((col, row)) = self.pick_sunflower() {
                self.place(col, row, P_SUNFLOWER);
                return;
            }
        }

        // 4. Стіни перед стрілками.
        let walls = self.count_plants(P_WALLNUT);
        if walls < LANES && self.afford(P_WALLNUT) {
            let mut best_row = None;
            let mut best_n = 0usize;
            for row in 0..LANES {
                let n = self.lane_pressure(row);
                if n > best_n && (0..COLS as i32).all(|c| self.plant_at(c, row as i32).map(|i| self.plants[i].kind) != Some(P_WALLNUT)) {
                    best_n = n;
                    best_row = Some(row);
                }
            }
            if let Some(row) = best_row {
                if self.plant_at(6, row as i32).is_none() {
                    self.place(6, row as i32, P_WALLNUT);
                    return;
                }
            }
        }

        // 5. Крижаний горох у найгарячіший ряд.
        if self.wave >= 4 && self.afford(P_ICEPEA) {
            let mut best_row = 0usize;
            let mut best_n = 0usize;
            for row in 0..LANES {
                let n = self.lane_pressure(row);
                if n > best_n {
                    best_n = n;
                    best_row = row;
                }
            }
            if best_n > 0 {
                for col in 2..=4 {
                    if self.has_shooter_at(col, best_row as i32) {
                        continue;
                    }
                    if self.plant_at(col, best_row as i32).is_none() {
                        self.place(col, best_row as i32, P_ICEPEA);
                        return;
                    }
                }
            }
        }

        // 6. Далі — просто забиваємо вільні клітинки соняшниками.
        if sunflowers < 8 && self.afford(P_SUNFLOWER) {
            if let Some((col, row)) = self.pick_sunflower() {
                self.place(col, row, P_SUNFLOWER);
            }
        }
    }

    /// Вільна клітинка під соняшник: стовпці 0..2, ряд із найменшою кількістю соняшників.
    fn pick_sunflower(&self) -> Option<(i32, i32)> {
        let mut best: Option<(i32, i32, usize)> = None;
        for col in 0..3 {
            for row in 0..LANES as i32 {
                if self.plant_at(col, row).is_some() {
                    continue;
                }
                let n = self
                    .plants
                    .iter()
                    .filter(|p| p.alive && p.kind == P_SUNFLOWER && p.row as i32 == row)
                    .count();
                let pref = if col == 0 { 0 } else { 1 };
                if best.is_none_or(|b| (n, pref) < (b.2, 0)) {
                    best = Some((col, row, n));
                }
            }
        }
        best.map(|(c, r, _)| (c, r))
    }

    /// Вільна клітинка під стрілка: ряд без стрільців (або з найбільшим тиском), стовпці 2..4.
    fn pick_shooter(&self) -> Option<(i32, i32)> {
        let mut best: Option<(i32, i32, (usize, usize))> = None;
        for row in 0..LANES {
            let n = self
                .plants
                .iter()
                .filter(|p| p.alive && matches!(p.kind, P_PEASHOOTER | P_ICEPEA) && p.row as usize == row)
                .count();
            let press = self.lane_pressure(row);
            for col in 2..=4 {
                if self.plant_at(col, row as i32).is_some() {
                    continue;
                }
                // менше стрільців у ряду → більше тиску → лівий стовпець
                let key = (n, usize::MAX - press);
                if best.is_none_or(|b| key < b.2) {
                    best = Some((col, row as i32, key));
                }
            }
        }
        best.map(|(c, r, _)| (c, r))
    }

    /// Скільки клітинок зайнято (для HUD/тестів).
    pub fn plants_count(&self) -> usize {
        self.plants_in_cell_count()
    }

    // ── Самотест на залізі ──────────────────────────────────────────────────

    /// Швидка перевірка інваріантів прошивки; повертає `(виконано, провалено)`.
    pub fn selftest() -> (u32, u32) {
        let mut run = 0u32;
        let mut failed = 0u32;
        macro_rules! check {
            ($cond:expr) => {{
                run += 1;
                if !$cond {
                    failed += 1;
                }
            }};
        }

        check!(PLAY_W + 0 == 288);
        check!(PLAY_H == 180);
        check!(wave_count(1) == 3 && wave_count(WAVES) == 14);
        check!(wave_kinds(1) & (1 << Z_CONE) == 0);
        check!(wave_kinds(WAVES) & (1 << Z_BUCKET) != 0);
        check!(zombie_stats(Z_BUCKET).hp > zombie_stats(Z_CONE).hp);
        check!(plant_stats(P_ICEPEA).cost > plant_stats(P_PEASHOOTER).cost);
        check!(START_SUN >= plant_stats(P_PEASHOOTER).cost);
        check!(MOWER_TRIGGER < XM);

        let mut g = Game::new(1);
        check!(!g.place(-1, 0, P_SUNFLOWER));
        check!(!g.place(0, 5, P_SUNFLOWER));
        check!(g.place(0, 0, P_SUNFLOWER));
        check!(!g.place(0, 0, P_SUNFLOWER));
        check!(g.sun == START_SUN - plant_stats(P_SUNFLOWER).cost);

        let mut g2 = Game::new(2);
        check!(g2.place(2, 2, P_PEASHOOTER));
        check!(g2.spawn_zombie(Z_BASIC, 2));
        let mut killed = false;
        for _ in 0..600 {
            g2.tick();
            if g2.kills > 0 {
                killed = true;
                break;
            }
        }
        check!(killed);

        let mut g3 = Game::new(3);
        g3.mower[0] = MOWER_USED;
        g3.spawn_zombie(Z_BASIC, 0);
        g3.zombies[0].xm = 40;
        for _ in 0..4 {
            g3.tick();
        }
        check!(g3.over == OVER_LOSE);

        let mut g4 = Game::new(4);
        g4.spawn_zombie(Z_BASIC, 1);
        g4.zombies[0].xm = 60;
        g4.tick();
        check!(g4.mower[1] == MOWER_RUN && g4.over == OVER_NONE);
        for _ in 0..200 {
            g4.tick();
        }
        check!(g4.kills >= 1 && g4.mower[1] == MOWER_USED);

        (run, failed)
    }
}

// ── Хост-тести (`rustc --test games/td/src/game.rs`) ─────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn fresh() -> Game {
        Game::new(12345)
    }

    #[test]
    fn geometry_matches_screen() {
        assert_eq!(PLAY_W, 288);
        assert_eq!(PLAY_H, 180);
        assert_eq!(CELLS, 45);
        assert_eq!(Game::cell_index(8, 4), Some(44));
        assert_eq!(Game::cell_index(9, 0), None);
        assert_eq!(Game::cell_center_px(0, 0), (16, 18));
    }

    #[test]
    fn start_state() {
        let g = fresh();
        assert_eq!(g.sun, START_SUN);
        assert_eq!(g.wave, 0);
        assert_eq!(g.over, OVER_NONE);
        assert!(g.alive_zombies() == 0);
        assert_eq!(g.mower, [MOWER_READY; LANES]);
    }

    #[test]
    fn planting_costs_and_blocks() {
        let mut g = fresh();
        assert!(g.place(0, 0, P_SUNFLOWER));
        assert_eq!(g.sun, START_SUN - 50);
        assert!(!g.place(0, 0, P_PEASHOOTER)); // клітинка зайнята
        assert!(!g.afford(P_SUNFLOWER)); // пакет на перезарядці
        assert!(g.afford(P_WALLNUT));
        g.sun = 10;
        assert!(!g.place(1, 1, P_WALLNUT));
    }

    #[test]
    fn sunflower_produces_sun() {
        let mut g = fresh();
        assert!(g.place(0, 0, P_SUNFLOWER));
        let mut got = 0;
        for _ in 0..1000 {
            g.tick();
            let before = g.suns.iter().filter(|s| s.alive).count();
            if before > 0 {
                let s = g.suns.iter().find(|s| s.alive).copied().unwrap();
                assert_eq!(s.value, SUN_VALUE);
                got += 1;
                g.suns.iter_mut().for_each(|s| s.alive = false);
            }
        }
        assert!(got >= 2, "соняшник дав {got} кульок за 50 с");
    }

    #[test]
    fn sky_sun_falls_and_expires() {
        let mut g = fresh();
        let mut seen = false;
        let mut fell = false;
        for _ in 0..(SKY_SUN_TICKS as u32 + 60) {
            g.tick();
            for s in g.suns.iter().filter(|s| s.alive) {
                seen = true;
                if s.y == s.target_y && s.target_y > 0 {
                    fell = true;
                }
            }
        }
        assert!(seen, "сонце з неба не з'явилося");
        assert!(fell, "сонце не долетіло до землі");

        // кулька, яку ніхто не зібрав, зникає за SUN_TTL
        g.spawn_sun(1, 1, 1, SUN_VALUE);
        assert!(g.suns.iter().any(|s| s.alive && s.x == 1));
        for _ in 0..(SUN_TTL + 2) {
            g.tick();
        }
        assert!(
            !g.suns.iter().any(|s| s.alive && s.x == 1),
            "сонце не зникло за ttl"
        );
    }

    #[test]
    fn collect_sun_by_tap() {
        let mut g = fresh();
        g.collect_all_suns();
        g.spawn_sun(100, 50, 50, SUN_VALUE);
        assert!(!g.collect_sun(0, 0));
        let before = g.sun;
        assert!(g.collect_sun(101, 52));
        assert_eq!(g.sun, before + SUN_VALUE);
        assert_eq!(g.suns.iter().filter(|s| s.alive).count(), 0);
    }

    #[test]
    fn peashooter_kills_basic_zombie() {
        let mut g = fresh();
        g.sun = 1000;
        assert!(g.place(2, 2, P_PEASHOOTER));
        assert!(g.spawn_zombie(Z_BASIC, 2));
        let mut ticks = 0;
        while g.zombies[0].alive && ticks < 400 {
            g.tick();
            ticks += 1;
        }
        assert!(!g.zombies[0].alive);
        assert_eq!(g.kills, 1);
        assert_eq!(g.score, zombie_stats(Z_BASIC).score);
        assert!(g.plants.iter().any(|p| p.alive)); // рослина жива
    }

    #[test]
    fn zombie_eats_plant_before_it() {
        let mut g = fresh();
        g.sun = 1000;
        assert!(g.place(6, 1, P_WALLNUT));
        assert!(g.spawn_zombie(Z_BASIC, 1));
        let mut eaten = false;
        for _ in 0..2000 {
            g.tick();
            if !g.plants[1 * COLS + 6].alive {
                eaten = true;
                break;
            }
        }
        assert!(eaten, "зомбі мав з'їсти горіх");
        assert_eq!(g.kills, 0);
    }

    #[test]
    fn ice_pea_slows_zombie() {
        let mut g = fresh();
        g.sun = 1000;
        assert!(g.place(2, 0, P_ICEPEA));
        assert!(g.spawn_zombie(Z_BUCKET, 0));
        let mut slowed = false;
        for _ in 0..120 {
            g.tick();
            if g.zombies[0].slow_t > 0 {
                slowed = true;
                break;
            }
        }
        assert!(slowed);
    }

    #[test]
    fn cherry_bomb_clears_area() {
        let mut g = fresh();
        g.sun = 1000;
        for lane in 0..3 {
            assert!(g.spawn_zombie(Z_BASIC, lane));
            g.zombies[lane].xm = 4 * XM + 300;
        }
        assert!(g.place(4, 1, P_CHERRY));
        for _ in 0..CHERRY_FUSE + 2 {
            g.tick();
        }
        assert_eq!(g.kills, 3);
        assert!(!g.plants[1 * COLS + 4].alive);
    }

    #[test]
    fn mower_saves_lane_once() {
        let mut g = fresh();
        assert!(g.spawn_zombie(Z_BASIC, 3));
        g.zombies[0].xm = 200;
        g.tick();
        assert_eq!(g.mower[3], MOWER_RUN);
        for _ in 0..200 {
            g.tick();
        }
        assert_eq!(g.kills, 1);
        assert_eq!(g.mower[3], MOWER_USED);

        // друга спроба в цьому ж ряду — поразка
        assert!(g.spawn_zombie(Z_BASIC, 3));
        let n = g.zombies.iter().position(|z| z.alive).unwrap();
        g.zombies[n].xm = 100;
        for _ in 0..40 {
            g.tick();
        }
        assert_eq!(g.over, OVER_LOSE);
    }

    #[test]
    fn waves_start_and_grow() {
        let mut g = fresh();
        for _ in 0..FIRST_WAVE_TICKS + 10 {
            g.tick();
        }
        assert_eq!(g.wave, 1);
        assert!(g.alive_zombies() > 0 || g.spawn_left > 0);
        assert!(wave_interval(12) < wave_interval(1));
        assert!(wave_count(12) > wave_count(1));
    }

    #[test]
    fn win_after_all_waves() {
        let mut g = fresh();
        g.wave = WAVES;
        g.spawn_left = 0;
        g.wave_t = 0;
        g.tick();
        assert_eq!(g.over, OVER_WIN);
    }

    #[test]
    fn demo_ai_makes_progress() {
        let mut g = fresh();
        let mut max_wave = 0;
        for _ in 0..(20 * 60 * 6) {
            g.tick();
            if g.tick_n % 4 == 0 {
                g.ai_step();
            }
            max_wave = max_wave.max(g.wave);
        }
        println!(
            "DEMO: over={} t={}s wave={} kills={} score={} plants={}",
            g.over,
            g.tick_n / TICKS_PER_SEC,
            g.wave,
            g.kills,
            g.score,
            g.plants_count()
        );
        assert!(max_wave >= 4, "автопілот дійшов лише до хвилі {max_wave}");
        assert!(g.kills >= 10, "автопілот убив лише {}", g.kills);
    }

    #[test]
    fn deterministic_for_same_seed() {
        let mut a = Game::new(777);
        let mut b = Game::new(777);
        for _ in 0..600 {
            a.tick();
            b.tick();
        }
        assert_eq!(a.kills, b.kills);
        assert_eq!(a.wave, b.wave);
        assert_eq!(a.alive_zombies(), b.alive_zombies());
        assert_eq!(a.suns.iter().filter(|s| s.alive).count(), b.suns.iter().filter(|s| s.alive).count());
    }

    #[test]
    fn selftest_passes() {
        let (run, failed) = Game::selftest();
        assert!(run >= 20);
        assert_eq!(failed, 0);
    }
}
