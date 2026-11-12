//! Логіка гри **Angry Birds** (стиль slingshot-фізики) — чистий `no_std` Rust,
//! без `f32`/`f64`, без алокатора й без залежностей від `esp-hal`.
//!
//! Поле `240 × 240` (панель HUD — окремо, праворуч 80 px), координати у фіксованій точці
//! `FP = 64` (`1/64` пікселя), вісь `y` вниз. Один тік = один кадр (~30 мс).
//!
//! Що є в моделі:
//! - рогатка: тягнеш птаха (або тапаєш ціль) → натяг обмежений `MAX_PULL`, політ за параболою;
//! - конструкції з трьох матеріалів (дерево/лід/камінь) з міцністю, руйнуванням і **падінням**
//!   блоків, що втратили опору (вертикальна фізика, без обертань);
//! - свині: гинуть від прямого влучання птаха, від падаючого блока і від власного падіння;
//! - частинки, спливаючі `+500`, рахунок, рекорд сесії, бонуси за невитрачених птахів;
//! - 3 рукотворні рівні, далі цикли з міцнішими блоками/свинями;
//! - бот `find_demo_shot` (сітка точок прицілювання + швидка симуляція) для `--features demo`
//!   і для самотестів «на кожному рівні є постріл, що влучає».
//!
//! Файл тестується на хості:
//! ```text
//! rustc --edition 2024 --test src/game.rs -o /tmp/birds_tests && /tmp/birds_tests
//! ```

/// Масштаб фіксованої точки: `1 px = 64` одиниці.
pub const FP: i32 = 64;
pub const FIELD_W: i32 = 240;
pub const FIELD_H: i32 = 240;
/// Верх трави (лінія землі).
pub const GROUND_Y: i32 = 216;

/// Точка спокою птаха на рогатці.
pub const SLING_X: i32 = 54;
pub const SLING_Y: i32 = 172;
pub const BIRD_R: i32 = 8;
/// Максимальний натяг у пікселях (обмежує швидкість пострілу).
pub const MAX_PULL: i32 = 46;

pub const MAX_BLOCKS: usize = 48;
pub const MAX_PIGS: usize = 12;
pub const MAX_PARTICLES: usize = 72;
pub const MAX_POPUPS: usize = 10;

pub const KIND_WOOD: u8 = 0;
pub const KIND_ICE: u8 = 1;
pub const KIND_STONE: u8 = 2;

pub const PIG_SCORE: u32 = 500;
pub const BIRD_BONUS: u32 = 1000;

/// Гравітація: прискорення за тік, у одиницях FP (`19/64 ≈ 0.30 px/тік²`).
pub const GRAVITY: i32 = 19;
/// Вертикальне обмеження натягу, щоб птах не пірнав під землю ще на рогатці.
const MAX_PULL_UP: i32 = 46;
const MAX_PULL_DOWN: i32 = GROUND_Y - SLING_Y - BIRD_R; // 36
/// Швидкість = натяг / 4 → 11.5 px/тік на повному натягу.
const LAUNCH_DIV: i32 = 4;
const SUBSTEPS: i32 = 4;

const SETTLE_TICKS: u16 = 16;
const MAX_FLIGHT_TICKS: u16 = 420;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum State {
    /// Птах на рогатці, можна цілитися.
    #[default]
    Aim,
    /// Птах у польоті.
    Flying,
    /// Птах спинився, світ досипається (блоки/свині падають).
    Settle,
    /// Усі свині знищені — картка «LEVEL CLEAR».
    LevelClear,
    /// Птахи скінчилися, а свині живі.
    GameOver,
}

#[derive(Clone, Copy, Default)]
pub struct Block {
    pub x: i32,   // лівий край, px
    pub y_fp: i32, // верхній край, FP
    pub w: i32,
    pub h: i32,
    pub kind: u8,
    pub hp: i32,
    pub alive: bool,
    pub falling: bool,
    pub vy_fp: i32,
    pub flash: u8,
}

impl Block {
    pub const EMPTY: Self = Self {
        x: 0,
        y_fp: 0,
        w: 0,
        h: 0,
        kind: KIND_WOOD,
        hp: 0,
        alive: false,
        falling: false,
        vy_fp: 0,
        flash: 0,
    };

    #[inline]
    pub fn bottom_fp(&self) -> i32 {
        self.y_fp + self.h * FP
    }
    #[inline]
    pub fn top_px(&self) -> i32 {
        self.y_fp / FP
    }
    #[inline]
    pub fn cx_px(&self) -> i32 {
        self.x + self.w / 2
    }
}

#[derive(Clone, Copy, Default)]
pub struct Pig {
    pub x: i32,     // центр, px
    pub y_fp: i32,  // центр, FP
    pub r: i32,
    pub hp: i32,
    pub alive: bool,
    pub falling: bool,
    pub vy_fp: i32,
    pub flash: u8,
}

impl Pig {
    pub const EMPTY: Self = Self {
        x: 0,
        y_fp: 0,
        r: 0,
        hp: 0,
        alive: false,
        falling: false,
        vy_fp: 0,
        flash: 0,
    };
    #[inline]
    pub fn y_px(&self) -> i32 {
        self.y_fp / FP
    }
}

#[derive(Clone, Copy, Default)]
pub struct Particle {
    pub x_fp: i32,
    pub y_fp: i32,
    pub vx_fp: i32,
    pub vy_fp: i32,
    pub ttl: u8,
    pub max_ttl: u8,
    pub color: u8,
    pub size: u8,
    pub alive: bool,
}

#[derive(Clone, Copy, Default)]
pub struct Popup {
    pub x: i16,
    pub y: i16,
    pub value: i16,
    pub ttl: u8,
    pub kind: u8, // 0 = очки за руйнування, 1 = свиня, 2 = бонус
}

#[derive(Clone, Copy, Default)]
pub struct Bird {
    pub active: bool,
    pub x_fp: i32,
    pub y_fp: i32,
    pub vx_fp: i32,
    pub vy_fp: i32,
}

#[derive(Clone, Copy, Default)]
pub struct PreviewPt {
    pub x: i16,
    pub y: i16,
}

#[derive(Clone, Copy, Default)]
pub struct TickEvents {
    pub hud_dirty: bool,
    pub block_broken: u8,
    pub pig_killed: u8,
    pub stage_clear: bool,
    pub game_over: bool,
    pub impact: bool,
}

/* ---------- цілочисельна математика ---------- */

/// Точний цілочисельний `floor(sqrt(n))`.
pub const fn isqrt(n: u64) -> u32 {
    if n < 2 {
        return n as u32;
    }
    let mut x = n;
    let mut y = (x + 1) >> 1;
    while y < x {
        x = y;
        y = (x + n / x) >> 1;
    }
    x as u32
}

#[inline]
fn clamp_i32(v: i32, lo: i32, hi: i32) -> i32 {
    if v < lo {
        lo
    } else if v > hi {
        hi
    } else {
        v
    }
}

/* ---------- дані рівнів ---------- */

#[derive(Clone, Copy)]
pub struct StageBlock {
    pub x: i16,
    pub y: i16,
    pub w: i16,
    pub h: i16,
    pub kind: u8,
}

#[derive(Clone, Copy)]
pub struct StagePig {
    pub x: i16,
    pub y: i16,
    pub r: i16,
}

pub struct StageDef {
    pub blocks: &'static [StageBlock],
    pub pigs: &'static [StagePig],
    pub birds: u8,
}

const fn sb(x: i16, y: i16, w: i16, h: i16, kind: u8) -> StageBlock {
    StageBlock { x, y, w, h, kind }
}

const fn sp(x: i16, y: i16, r: i16) -> StagePig {
    StagePig { x, y, r }
}

// Рівень 1 — «перший поверх»: дві дерев'яні стійки, балка, дві свині.
static STAGE1_BLOCKS: [StageBlock; 4] = [
    sb(168, 180, 12, 36, KIND_WOOD),
    sb(204, 180, 12, 36, KIND_WOOD),
    sb(164, 168, 56, 12, KIND_WOOD),
    sb(168, 156, 12, 12, KIND_ICE),
];
static STAGE1_PIGS: [StagePig; 2] = [sp(192, 207, 9), sp(204, 159, 9)];

// Рівень 2 — «вежа»: камінь + два дерев'яні поверхи, три свині.
static STAGE2_BLOCKS: [StageBlock; 7] = [
    sb(156, 180, 12, 36, KIND_STONE),
    sb(212, 180, 12, 36, KIND_STONE),
    sb(152, 168, 72, 12, KIND_WOOD),
    sb(162, 132, 12, 36, KIND_WOOD),
    sb(206, 132, 12, 36, KIND_WOOD),
    sb(158, 120, 68, 12, KIND_WOOD),
    sb(170, 108, 12, 12, KIND_ICE),
];
static STAGE2_PIGS: [StagePig; 3] = [sp(184, 207, 9), sp(184, 159, 9), sp(208, 112, 8)];

// Рівень 3 — «фортеця»: кам'яні стіни, лід, чотири свині.
static STAGE3_BLOCKS: [StageBlock; 11] = [
    sb(130, 180, 12, 36, KIND_STONE),
    sb(222, 180, 12, 36, KIND_STONE),
    sb(126, 168, 112, 12, KIND_WOOD),
    sb(144, 132, 12, 36, KIND_WOOD),
    sb(206, 132, 12, 36, KIND_WOOD),
    sb(138, 120, 80, 12, KIND_WOOD),
    sb(128, 156, 12, 12, KIND_ICE),
    sb(156, 156, 12, 12, KIND_ICE),
    sb(174, 108, 12, 12, KIND_STONE),
    sb(140, 96, 12, 24, KIND_ICE),
    sb(210, 96, 12, 24, KIND_ICE),
];
static STAGE3_PIGS: [StagePig; 4] = [
    sp(176, 207, 9),
    sp(205, 207, 9),
    sp(180, 159, 9),
    sp(196, 112, 8),
];

pub const STAGES: [StageDef; 3] = [
    StageDef {
        blocks: &STAGE1_BLOCKS,
        pigs: &STAGE1_PIGS,
        birds: 4,
    },
    StageDef {
        blocks: &STAGE2_BLOCKS,
        pigs: &STAGE2_PIGS,
        birds: 4,
    },
    StageDef {
        blocks: &STAGE3_BLOCKS,
        pigs: &STAGE3_PIGS,
        birds: 5,
    },
];

pub fn stage_count() -> u8 {
    STAGES.len() as u8
}

fn base_block_hp(kind: u8) -> i32 {
    match kind {
        KIND_ICE => 2,
        KIND_STONE => 8,
        _ => 4,
    }
}

fn block_score(kind: u8) -> u32 {
    match kind {
        KIND_ICE => 30,
        KIND_STONE => 80,
        _ => 50,
    }
}

/* ---------- стан гри ---------- */

pub struct Game {
    pub stage: u8,
    pub score: u32,
    pub best: u32,
    pub birds_left: u8,
    pub birds_total: u8,
    pub bird: Bird,
    pub blocks: [Block; MAX_BLOCKS],
    pub block_count: u8,
    pub pigs: [Pig; MAX_PIGS],
    pub pig_count: u8,
    pub particles: [Particle; MAX_PARTICLES],
    pub popups: [Popup; MAX_POPUPS],
    pub state: State,
    pub tick_no: u32,
    pub pull_x: i32, // натяг у пікселях (птах зміщений на цей вектор)
    pub pull_y: i32,
    pub aiming: bool,
    pub shots: u16,
    pub cleared: bool,
    pub stage_base_score: u32,
    settle_ticks: u16,
    flight_ticks: u16,
    rest_ticks: u16,
    clear_bonus: u32,
}

impl Game {
    pub const fn empty() -> Self {
        Self {
            stage: 1,
            score: 0,
            best: 0,
            birds_left: 0,
            birds_total: 0,
            bird: Bird {
                active: false,
                x_fp: 0,
                y_fp: 0,
                vx_fp: 0,
                vy_fp: 0,
            },
            blocks: [Block::EMPTY; MAX_BLOCKS],
            block_count: 0,
            pigs: [Pig::EMPTY; MAX_PIGS],
            pig_count: 0,
            particles: [Particle {
                x_fp: 0,
                y_fp: 0,
                vx_fp: 0,
                vy_fp: 0,
                ttl: 0,
                max_ttl: 0,
                color: 0,
                size: 0,
                alive: false,
            }; MAX_PARTICLES],
            popups: [Popup {
                x: 0,
                y: 0,
                value: 0,
                ttl: 0,
                kind: 0,
            }; MAX_POPUPS],
            state: State::Aim,
            tick_no: 0,
            pull_x: 0,
            pull_y: 0,
            aiming: false,
            shots: 0,
            cleared: false,
            stage_base_score: 0,
            settle_ticks: 0,
            flight_ticks: 0,
            rest_ticks: 0,
            clear_bonus: 0,
        }
    }

    pub fn new(stage: u8, _seed: u32) -> Self {
        let mut g = Self::empty();
        g.init_stage(stage);
        g
    }

    pub fn start_new_game(&mut self, start_stage: u8, seed: u32) {
        self.score = 0;
        self.shots = 0;
        self.clear_bonus = 0;
        let _ = seed;
        self.init_stage(start_stage.max(1));
    }

    pub fn init_stage(&mut self, stage: u8) {
        let stage = stage.max(1);
        self.stage = stage;
        let idx = ((stage - 1) as usize) % STAGES.len();
        let cycle = (stage - 1) as i32 / STAGES.len() as i32;
        let def = &STAGES[idx];

        self.particles = [Particle::default(); MAX_PARTICLES];
        self.popups = [Popup::default(); MAX_POPUPS];

        self.block_count = def.blocks.len() as u8;
        for (i, d) in def.blocks.iter().enumerate() {
            self.blocks[i] = Block {
                x: d.x as i32,
                y_fp: d.y as i32 * FP,
                w: d.w as i32,
                h: d.h as i32,
                kind: d.kind,
                hp: base_block_hp(d.kind) + cycle,
                alive: true,
                falling: false,
                vy_fp: 0,
                flash: 0,
            };
        }
        for i in def.blocks.len()..MAX_BLOCKS {
            self.blocks[i] = Block::EMPTY;
        }

        self.pig_count = def.pigs.len() as u8;
        for (i, d) in def.pigs.iter().enumerate() {
            self.pigs[i] = Pig {
                x: d.x as i32,
                y_fp: d.y as i32 * FP,
                r: d.r as i32,
                hp: 2 + cycle,
                alive: true,
                falling: false,
                vy_fp: 0,
                flash: 0,
            };
        }
        for i in def.pigs.len()..MAX_PIGS {
            self.pigs[i] = Pig::EMPTY;
        }

        self.stage_base_score = self.score;
        self.tick_no = 0;
        self.birds_total = def.birds;
        self.birds_left = def.birds;
        self.bird = Bird::default();
        self.state = State::Aim;
        self.pull_x = 0;
        self.pull_y = 0;
        self.aiming = false;
        self.cleared = false;
        self.settle_ticks = 0;
        self.flight_ticks = 0;
        self.rest_ticks = 0;
        self.clear_bonus = 0;
    }

    pub fn next_stage(&mut self) {
        self.init_stage(self.stage.saturating_add(1));
    }

    pub fn pigs_left(&self) -> u8 {
        let mut n = 0;
        for i in 0..self.pig_count as usize {
            if self.pigs[i].alive {
                n += 1;
            }
        }
        n
    }

    pub fn blocks_left(&self) -> u8 {
        let mut n = 0;
        for i in 0..self.block_count as usize {
            if self.blocks[i].alive {
                n += 1;
            }
        }
        n
    }

    pub fn power_pct(&self) -> u8 {
        let d = isqrt((self.pull_x as i64 * self.pull_x as i64
            + self.pull_y as i64 * self.pull_y as i64) as u64) as i32;
        ((d * 100) / MAX_PULL).clamp(0, 100) as u8
    }

    pub fn clear_bonus(&self) -> u32 {
        self.clear_bonus
    }

    /* ---------- прицілювання ---------- */

    /// Натяг напряму (px): птах зміщується на цей вектор, політ — у протилежний бік.
    pub fn set_pull(&mut self, px: i32, py: i32) -> bool {
        if self.state != State::Aim || self.birds_left == 0 {
            return false;
        }
        let (px, py) = clamp_pull(px, py);
        self.pull_x = px;
        self.pull_y = py;
        self.aiming = true;
        true
    }

    /// Режим «тягнемо птаха»: точка пальця стає позицією птаха.
    pub fn aim_drag(&mut self, tx: i32, ty: i32) -> bool {
        self.set_pull(tx - SLING_X, ty - SLING_Y)
    }

    /// Режим «тап по цілі»: напрямок — на палець, сила — за відстанню.
    pub fn aim_point(&mut self, tx: i32, ty: i32) -> bool {
        let (px, py) = aim_vector_for(tx, ty);
        self.set_pull(px, py)
    }

    pub fn cancel_aim(&mut self) {
        self.pull_x = 0;
        self.pull_y = 0;
        self.aiming = false;
    }

    pub fn launch(&mut self) -> bool {
        if self.state != State::Aim || self.birds_left == 0 {
            return false;
        }
        let d = isqrt((self.pull_x as i64 * self.pull_x as i64
            + self.pull_y as i64 * self.pull_y as i64) as u64) as i32;
        if d < 6 {
            return false;
        }
        self.bird = Bird {
            active: true,
            x_fp: (SLING_X + self.pull_x) * FP,
            y_fp: (SLING_Y + self.pull_y) * FP,
            vx_fp: -self.pull_x * FP / LAUNCH_DIV,
            vy_fp: -self.pull_y * FP / LAUNCH_DIV,
        };
        self.birds_left = self.birds_left.saturating_sub(1);
        self.state = State::Flying;
        self.flight_ticks = 0;
        self.rest_ticks = 0;
        self.aiming = false;
        self.shots = self.shots.saturating_add(1);
        true
    }

    /* ---------- геометрія ---------- */

    fn block_supported(&self, idx: usize) -> bool {
        let b = self.blocks[idx];
        let bottom = b.bottom_fp();
        if bottom >= GROUND_Y * FP - FP / 2 {
            return true;
        }
        for j in 0..self.block_count as usize {
            let o = self.blocks[j];
            if j == idx || !o.alive || o.falling {
                continue;
            }
            if o.x + o.w <= b.x || b.x + b.w <= o.x {
                continue;
            }
            if o.y_fp >= bottom - 2 * FP && o.y_fp <= bottom + FP / 2 {
                return true;
            }
        }
        false
    }

    fn pig_supported(&self, idx: usize) -> bool {
        let p = self.pigs[idx];
        let bottom = p.y_fp + p.r * FP;
        if bottom >= GROUND_Y * FP - FP / 2 {
            return true;
        }
        for j in 0..self.block_count as usize {
            let o = self.blocks[j];
            if !o.alive {
                continue;
            }
            if p.x + p.r <= o.x || p.x - p.r >= o.x + o.w {
                continue;
            }
            if o.y_fp >= bottom - 2 * FP && o.y_fp <= bottom + FP / 2 {
                return true;
            }
        }
        false
    }

    fn any_falling(&self) -> bool {
        for i in 0..self.block_count as usize {
            if self.blocks[i].alive && self.blocks[i].falling {
                return true;
            }
        }
        for i in 0..self.pig_count as usize {
            if self.pigs[i].alive && self.pigs[i].falling {
                return true;
            }
        }
        false
    }

    /* ---------- траєкторія ---------- */

    /// Точки попереднього перегляду польоту (кожні 2 тіки) до землі/першої перешкоди.
    pub fn preview(&self, out: &mut [PreviewPt; 72]) -> usize {
        if self.state != State::Aim || self.birds_left == 0 {
            return 0;
        }
        let mut x = (SLING_X + self.pull_x) * FP;
        let mut y = (SLING_Y + self.pull_y) * FP;
        let vx = -self.pull_x * FP / LAUNCH_DIV;
        let mut vy = -self.pull_y * FP / LAUNCH_DIV;
        let mut n = 0usize;
        let mut tick = 0u32;
        while tick < 240 && n < out.len() {
            x += vx;
            y += vy;
            vy += GRAVITY;
            tick += 1;
            let hit = y + BIRD_R * FP >= GROUND_Y * FP
                || x < -4 * FP
                || x > (FIELD_W + 4) * FP
                || self.point_hits(x, y);
            if hit {
                out[n] = PreviewPt {
                    x: clamp_i16(x / FP, 0, FIELD_W - 1),
                    y: clamp_i16(y / FP, 0, FIELD_H - 1),
                };
                n += 1;
                break;
            }
            if tick % 2 == 0 {
                out[n] = PreviewPt {
                    x: (x / FP) as i16,
                    y: (y / FP) as i16,
                };
                n += 1;
            }
        }
        n
    }

    fn point_hits(&self, x_fp: i32, y_fp: i32) -> bool {
        for i in 0..self.block_count as usize {
            let b = self.blocks[i];
            if b.alive && circle_aabb(x_fp, y_fp, BIRD_R * FP, &b) {
                return true;
            }
        }
        for i in 0..self.pig_count as usize {
            let p = self.pigs[i];
            if p.alive && circle_circle(x_fp, y_fp, BIRD_R * FP, p.x * FP, p.y_fp, p.r * FP) {
                return true;
            }
        }
        false
    }

    /* ---------- бот ---------- */

    /// Підібрати постріл сіткою точок прицілювання. Повертає вектор натягу (px).
    pub fn find_demo_shot(&self) -> Option<(i32, i32)> {
        if self.state != State::Aim || self.birds_left == 0 {
            return None;
        }
        let mut best = None;
        let mut best_score = 0u32;
        let mut ty = 40i32;
        while ty <= 168 {
            let mut tx = 92i32;
            while tx <= 236 {
                let (px, py) = aim_vector_for(tx, ty);
                let s = self.simulate_score(px, py);
                if s > best_score {
                    best_score = s;
                    best = Some((px, py));
                }
                tx += 18;
            }
            ty += 18;
        }
        best
    }

    fn simulate_score(&self, px: i32, py: i32) -> u32 {
        let mut x = (SLING_X + px) * FP;
        let mut y = (SLING_Y + py) * FP;
        let vx = -px * FP / LAUNCH_DIV;
        let mut vy = -py * FP / LAUNCH_DIV;
        for _ in 0..220 {
            x += vx;
            y += vy;
            vy += GRAVITY;
            if y + BIRD_R * FP >= GROUND_Y * FP || x < -20 * FP || x > (FIELD_W + 20) * FP {
                return 0;
            }
            for i in 0..self.block_count as usize {
                let b = self.blocks[i];
                if b.alive && circle_aabb(x, y, BIRD_R * FP, &b) {
                    let height_bonus = (GROUND_Y - b.top_px()).max(0) as u32;
                    let base = match b.kind {
                        KIND_ICE => 60,
                        KIND_STONE => 120,
                        _ => 90,
                    };
                    return base + height_bonus;
                }
            }
            for i in 0..self.pig_count as usize {
                let p = self.pigs[i];
                if p.alive
                    && circle_circle(x, y, BIRD_R * FP, p.x * FP, p.y_fp, p.r * FP)
                {
                    return 1000 + (GROUND_Y - p.y_px()).max(0) as u32;
                }
            }
        }
        0
    }

    /* ---------- тік ---------- */

    pub fn tick(&mut self) -> TickEvents {
        let mut ev = TickEvents::default();
        self.tick_no = self.tick_no.wrapping_add(1);

        self.update_particles();
        self.update_popups();
        for i in 0..self.block_count as usize {
            self.blocks[i].flash = self.blocks[i].flash.saturating_sub(1);
        }
        for i in 0..self.pig_count as usize {
            self.pigs[i].flash = self.pigs[i].flash.saturating_sub(1);
        }

        match self.state {
            State::Aim => {
                self.update_falling(&mut ev);
            }
            State::Flying => {
                self.flight_ticks += 1;
                self.step_bird(&mut ev);
                self.update_falling(&mut ev);
                if self.pigs_left() == 0 {
                    self.finish_level(&mut ev);
                } else if !self.bird.active
                    || self.flight_ticks > MAX_FLIGHT_TICKS
                    || self.bird.y_fp > (FIELD_H + 60) * FP
                {
                    self.bird.active = false;
                    self.state = State::Settle;
                    self.settle_ticks = 0;
                    ev.hud_dirty = true;
                }
            }
            State::Settle => {
                // птах ще може летіти, якщо рівень закрився в повітрі — даємо йому долетіти
                if self.bird.active {
                    self.step_bird(&mut ev);
                }
                self.update_falling(&mut ev);
                self.settle_ticks += 1;
                if self.pigs_left() == 0 {
                    self.finish_level(&mut ev);
                } else if !self.any_falling() && self.settle_ticks >= SETTLE_TICKS {
                    if self.birds_left > 0 {
                        self.state = State::Aim;
                        self.aiming = false;
                        self.pull_x = 0;
                        self.pull_y = 0;
                    } else {
                        self.state = State::GameOver;
                        ev.game_over = true;
                        if self.score > self.best {
                            self.best = self.score;
                        }
                    }
                    ev.hud_dirty = true;
                }
            }
            State::LevelClear => {
                self.update_falling(&mut ev);
                if !self.any_falling() {
                    self.state = State::LevelClear;
                }
            }
            State::GameOver => {
                self.update_falling(&mut ev);
            }
        }
        ev
    }

    fn finish_level(&mut self, ev: &mut TickEvents) {
        if self.cleared {
            if !self.any_falling() && !self.bird.active {
                self.state = State::LevelClear;
            }
            return;
        }
        self.cleared = true;
        self.clear_bonus = self.birds_left as u32 * BIRD_BONUS + 500;
        self.score += self.clear_bonus;
        if self.score > self.best {
            self.best = self.score;
        }
        ev.stage_clear = true;
        ev.hud_dirty = true;
        self.state = State::Settle;
        self.settle_ticks = SETTLE_TICKS;
    }

    /* ---------- політ птаха ---------- */

    fn step_bird(&mut self, ev: &mut TickEvents) {
        if !self.bird.active {
            return;
        }
        let sub = SUBSTEPS;
        for _ in 0..sub {
            self.bird.x_fp += self.bird.vx_fp / sub;
            self.bird.y_fp += self.bird.vy_fp / sub;
            self.collide_blocks(ev);
            self.collide_pigs(ev);
            self.collide_ground(ev);
        }
        self.bird.vy_fp += GRAVITY;
        self.bird.vx_fp -= self.bird.vx_fp / 320; // легкий опір повітря

        // Птах зупинився на землі — віддаємо хід світу.
        let sp = self.bird.vx_fp.abs() + self.bird.vy_fp.abs();
        let on_ground = self.bird.y_fp + BIRD_R * FP >= GROUND_Y * FP - FP;
        if on_ground && sp < 2 * FP {
            self.rest_ticks += 1;
            if self.rest_ticks > 10 {
                self.bird.active = false;
            }
        } else {
            self.rest_ticks = 0;
        }

        if self.bird.x_fp < -40 * FP
            || self.bird.x_fp > (FIELD_W + 40) * FP
            || self.bird.y_fp > (FIELD_H + 40) * FP
        {
            self.bird.active = false;
        }
    }

    fn collide_blocks(&mut self, ev: &mut TickEvents) {
        for i in 0..self.block_count as usize {
            let b = self.blocks[i];
            if !b.alive {
                continue;
            }
            let bx0 = b.x * FP;
            let by0 = b.y_fp;
            if !circle_aabb(self.bird.x_fp, self.bird.y_fp, BIRD_R * FP, &b) {
                continue;
            }
            let bx1 = (b.x + b.w) * FP;
            let by1 = b.y_fp + b.h * FP;
            let cx = clamp_i32(self.bird.x_fp, bx0, bx1);
            let cy = clamp_i32(self.bird.y_fp, by0, by1);
            let dx = self.bird.x_fp - cx;
            let dy = self.bird.y_fp - cy;
            let d = isqrt((dx as i64 * dx as i64 + dy as i64 * dy as i64) as u64) as i32;
            let (nx, ny) = if d > 0 {
                ((dx * FP) / d, (dy * FP) / d)
            } else {
                (0, -FP)
            };
            // виштовхнути птаха на поверхню
            self.bird.x_fp = cx + nx * BIRD_R / FP;
            self.bird.y_fp = cy + ny * BIRD_R / FP;
            let vn = (self.bird.vx_fp * nx + self.bird.vy_fp * ny) / FP;
            if vn < 0 {
                let imp = -vn;
                // пружність 0.5
                self.bird.vx_fp -= nx * imp * 3 / (2 * FP);
                self.bird.vy_fp -= ny * imp * 3 / (2 * FP);
                let speed_px = imp / FP;
                if speed_px >= 1 && b.flash == 0 {
                    let dmg = clamp_i32(speed_px * 4 / 3, 1, 14);
                    self.damage_block(i, dmg, ev);
                    ev.impact = true;
                    self.spawn_dust(cx / FP, cy / FP, 3, b.kind);
                }
            }
        }
    }

    fn collide_pigs(&mut self, ev: &mut TickEvents) {
        for i in 0..self.pig_count as usize {
            let p = self.pigs[i];
            if !p.alive {
                continue;
            }
            if !circle_circle(
                self.bird.x_fp,
                self.bird.y_fp,
                BIRD_R * FP,
                p.x * FP,
                p.y_fp,
                p.r * FP,
            ) {
                continue;
            }
            let dx = self.bird.x_fp - p.x * FP;
            let dy = self.bird.y_fp - p.y_fp;
            let d = isqrt((dx as i64 * dx as i64 + dy as i64 * dy as i64) as u64) as i32;
            let (nx, ny) = if d > 0 {
                ((dx * FP) / d, (dy * FP) / d)
            } else {
                (0, -FP)
            };
            let push = (BIRD_R + p.r) * FP;
            self.bird.x_fp = p.x * FP + nx * push / FP;
            self.bird.y_fp = p.y_fp + ny * push / FP;
            self.bird.vx_fp = self.bird.vx_fp * 6 / 10;
            self.bird.vy_fp = self.bird.vy_fp * 6 / 10;
            self.kill_pig(i, ev);
        }
    }

    fn collide_ground(&mut self, ev: &mut TickEvents) {
        let floor = GROUND_Y * FP - BIRD_R * FP;
        if self.bird.y_fp > floor {
            self.bird.y_fp = floor;
            if self.bird.vy_fp > 0 {
                if self.bird.vy_fp > 3 * FP {
                    self.spawn_dust(self.bird.x_fp / FP, GROUND_Y, 2, KIND_WOOD);
                    ev.impact = true;
                }
                self.bird.vy_fp = -self.bird.vy_fp * 3 / 10;
                self.bird.vx_fp = self.bird.vx_fp * 7 / 10;
            }
            if self.bird.vy_fp.abs() < FP / 3 {
                self.bird.vy_fp = 0;
            }
        }
    }

    /* ---------- падіння блоків і свиней ---------- */

    fn update_falling(&mut self, ev: &mut TickEvents) {
        for i in 0..self.block_count as usize {
            let b = self.blocks[i];
            if !b.alive {
                continue;
            }
            if !b.falling {
                if self.block_supported(i) {
                    continue;
                }
                self.blocks[i].falling = true;
                self.blocks[i].vy_fp = 0;
            }
            let b = self.blocks[i];
            let old_bottom = b.bottom_fp();
            let vy = (b.vy_fp + GRAVITY).min(16 * FP);
            let mut y_fp = b.y_fp + vy;
            let new_bottom = y_fp + b.h * FP;

            // поверхня під блоком: земля або верх іншого блока
            let mut land = GROUND_Y * FP;
            let mut land_on: i32 = -1;
            for j in 0..self.block_count as usize {
                if j == i {
                    continue;
                }
                let o = self.blocks[j];
                if !o.alive || o.falling {
                    continue;
                }
                if o.x + o.w <= b.x || b.x + b.w <= o.x {
                    continue;
                }
                if o.y_fp >= old_bottom - 2 * FP && o.y_fp < land {
                    land = o.y_fp;
                    land_on = j as i32;
                }
            }

            if new_bottom >= land {
                y_fp = land - b.h * FP;
                self.blocks[i].y_fp = y_fp;
                self.blocks[i].falling = false;
                self.blocks[i].vy_fp = 0;
                self.blocks[i].flash = 6;
                if vy > 4 * FP {
                    ev.impact = true;
                    self.spawn_dust(b.x + b.w / 2, land / FP, 4, KIND_WOOD);
                    if land_on >= 0 {
                        self.damage_block(land_on as usize, 3, ev);
                    } else if vy > 9 * FP {
                        self.damage_block(i, 1, ev);
                    }
                }
            } else {
                self.blocks[i].y_fp = y_fp;
                self.blocks[i].vy_fp = vy;
            }

            // падаючий блок розчавлює свиню, що опинилася під ним
            let b = self.blocks[i];
            for pi in 0..self.pig_count as usize {
                let p = self.pigs[pi];
                if p.alive
                    && circle_aabb(p.x * FP, p.y_fp, p.r * FP, &b)
                {
                    self.kill_pig(pi, ev);
                }
            }
        }

        // свині теж падають, якщо під ними зник блок
        for i in 0..self.pig_count as usize {
            let p = self.pigs[i];
            if !p.alive {
                continue;
            }
            if !p.falling {
                if self.pig_supported(i) {
                    continue;
                }
                self.pigs[i].falling = true;
                self.pigs[i].vy_fp = 0;
            }
            let p = self.pigs[i];
            let old_bottom = p.y_fp + p.r * FP;
            let vy = (p.vy_fp + GRAVITY).min(16 * FP);
            let mut y_fp = p.y_fp + vy;
            let new_bottom = y_fp + p.r * FP;
            let mut land = GROUND_Y * FP;
            for j in 0..self.block_count as usize {
                let o = self.blocks[j];
                if !o.alive || o.falling {
                    continue;
                }
                if p.x + p.r <= o.x || p.x - p.r >= o.x + o.w {
                    continue;
                }
                if o.y_fp >= old_bottom - 2 * FP && o.y_fp < land {
                    land = o.y_fp;
                }
            }
            if new_bottom >= land {
                y_fp = land - p.r * FP;
                self.pigs[i].y_fp = y_fp;
                self.pigs[i].falling = false;
                self.pigs[i].vy_fp = 0;
                if vy > 6 * FP {
                    // впала з висоти — луснула
                    self.kill_pig(i, ev);
                }
            } else {
                self.pigs[i].y_fp = y_fp;
                self.pigs[i].vy_fp = vy;
            }
        }
    }

    /* ---------- руйнування ---------- */

    fn damage_block(&mut self, i: usize, dmg: i32, ev: &mut TickEvents) {
        let b = self.blocks[i];
        if !b.alive {
            return;
        }
        self.blocks[i].hp = b.hp - dmg;
        self.blocks[i].flash = 6;
        if self.blocks[i].hp > 0 {
            return;
        }
        self.blocks[i].alive = false;
        self.blocks[i].falling = false;
        let value = block_score(b.kind);
        self.score += value;
        if self.score > self.best {
            self.best = self.score;
        }
        ev.hud_dirty = true;
        ev.block_broken = ev.block_broken.saturating_add(1);
        let cx = b.cx_px();
        let cy = (b.y_fp + b.h * FP / 2) / FP;
        self.spawn_debris(cx, cy, b.kind);
        self.add_popup(cx, cy, value as i16, 0);
    }

    fn kill_pig(&mut self, i: usize, ev: &mut TickEvents) {
        let p = self.pigs[i];
        if !p.alive {
            return;
        }
        self.pigs[i].alive = false;
        self.pigs[i].falling = false;
        self.score += PIG_SCORE;
        if self.score > self.best {
            self.best = self.score;
        }
        ev.hud_dirty = true;
        ev.pig_killed = ev.pig_killed.saturating_add(1);
        let cx = p.x;
        let cy = p.y_px();
        for k in 0..10 {
            let ang = k as i32 * 4096 / 10;
            let (dx, dy) = unit_from_angle(ang);
            self.spawn_particle(
                cx,
                cy,
                dx * 3,
                dy * 3 - FP,
                18 + (k % 4) as u8 * 3,
                4, // зелений «свинячий» колір
                2 + (k % 2) as u8,
            );
        }
        self.add_popup(cx, cy - 4, PIG_SCORE as i16, 1);
    }

    /* ---------- частинки та написи ---------- */

    pub fn spawn_particle(
        &mut self,
        x: i32,
        y: i32,
        vx_px: i32,
        vy_px: i32,
        ttl: u8,
        color: u8,
        size: u8,
    ) {
        let slot = self
            .particles
            .iter()
            .position(|p| !p.alive)
            .unwrap_or((self.tick_no as usize) % MAX_PARTICLES);
        self.particles[slot] = Particle {
            x_fp: x * FP,
            y_fp: y * FP,
            vx_fp: vx_px * FP,
            vy_fp: vy_px * FP,
            ttl,
            max_ttl: ttl,
            color,
            size,
            alive: true,
        };
    }

    fn spawn_dust(&mut self, x: i32, y: i32, n: u8, _kind: u8) {
        for k in 0..n {
            let ang = k as i32 * 4096 / (n.max(1) as i32) + 512;
            let (dx, dy) = unit_from_angle(ang);
            self.spawn_particle(x, y, dx / 2, dy / 2 - FP / 2, 12, 0, 2);
        }
    }

    fn spawn_debris(&mut self, x: i32, y: i32, kind: u8) {
        for k in 0..6 {
            let ang = k as i32 * 4096 / 6;
            let (dx, dy) = unit_from_angle(ang);
            let color = match kind {
                KIND_ICE => 1,
                KIND_STONE => 2,
                _ => 3,
            };
            self.spawn_particle(x, y, dx * 2, dy * 2 - FP, 16, color, 2);
        }
    }

    fn add_popup(&mut self, x: i32, y: i32, value: i16, kind: u8) {
        let slot = self
            .popups
            .iter()
            .position(|p| p.ttl == 0)
            .unwrap_or((self.tick_no as usize) % MAX_POPUPS);
        self.popups[slot] = Popup {
            x: clamp_i16(x - 10, 2, FIELD_W - 24),
            y: clamp_i16(y - 8, 2, FIELD_H - 12),
            value,
            ttl: 40,
            kind,
        };
    }

    fn update_particles(&mut self) {
        for p in self.particles.iter_mut() {
            if !p.alive {
                continue;
            }
            p.x_fp += p.vx_fp;
            p.y_fp += p.vy_fp;
            p.vy_fp += GRAVITY / 2;
            if p.y_fp > GROUND_Y * FP {
                p.y_fp = GROUND_Y * FP;
                p.vy_fp = -p.vy_fp / 3;
                p.vx_fp = p.vx_fp * 2 / 3;
            }
            if p.ttl > 0 {
                p.ttl -= 1;
            } else {
                p.alive = false;
            }
        }
    }

    fn update_popups(&mut self) {
        for p in self.popups.iter_mut() {
            if p.ttl == 0 {
                continue;
            }
            p.ttl -= 1;
            if p.ttl % 8 == 0 {
                p.y -= 1;
            }
        }
    }
}

/* ---------- допоміжна геометрія ---------- */

fn clamp_i16(v: i32, lo: i32, hi: i32) -> i16 {
    clamp_i32(v, lo, hi) as i16
}

#[inline]
fn circle_aabb(cx: i32, cy: i32, r_fp: i32, b: &Block) -> bool {
    let x0 = b.x * FP;
    let x1 = (b.x + b.w) * FP;
    let y0 = b.y_fp;
    let y1 = b.y_fp + b.h * FP;
    let px = clamp_i32(cx, x0, x1);
    let py = clamp_i32(cy, y0, y1);
    let dx = (cx - px) as i64;
    let dy = (cy - py) as i64;
    dx * dx + dy * dy < (r_fp as i64) * (r_fp as i64)
}

#[inline]
fn circle_circle(ax: i32, ay: i32, ar: i32, bx: i32, by: i32, br: i32) -> bool {
    let dx = (ax - bx) as i64;
    let dy = (ay - by) as i64;
    let r = (ar + br) as i64;
    dx * dx + dy * dy < r * r
}

fn rects_overlap(a: &Block, b: &Block) -> bool {
    if !a.alive || !b.alive {
        return false;
    }
    a.x < b.x + b.w && b.x < a.x + a.w && a.y_fp < b.bottom_fp() && b.y_fp < a.bottom_fp()
}

/// Одиничний вектор (у FP) з кута `0..4096` (повне коло).
fn unit_from_angle(ang: i32) -> (i32, i32) {
    // невеликий наближений синус без таблиць: 4 квадрати, парабола
    let a = ang.rem_euclid(4096);
    let (sign_x, sign_y) = match a {
        0..=1023 => (1, 1),
        1024..=2047 => (-1, 1),
        2048..=3071 => (-1, -1),
        _ => (1, -1),
    };
    let t = match a {
        0..=1023 => a,
        1024..=2047 => a - 1024,
        2048..=3071 => a - 2048,
        _ => a - 3072,
    };
    let u = t as i64 * FP as i64 / 1024; // 0..FP
    let v = FP as i64 - u;
    let s = (u * v) / FP as i64; // ~0..FP/4
    let x = sign_x as i64 * (s * 4);
    let y = sign_y as i64 * ((FP as i64 - s * 4) * v / FP as i64);
    (x as i32, y as i32)
}

fn clamp_pull(px: i32, py: i32) -> (i32, i32) {
    let px = clamp_i32(px, -MAX_PULL, MAX_PULL);
    let py = clamp_i32(py, -MAX_PULL_UP, MAX_PULL_DOWN);
    let d = isqrt((px as i64 * px as i64 + py as i64 * py as i64) as u64) as i32;
    if d > MAX_PULL {
        ((px * MAX_PULL) / d, (py * MAX_PULL) / d)
    } else {
        (px, py)
    }
}

fn aim_vector_for(tx: i32, ty: i32) -> (i32, i32) {
    let dx = tx - SLING_X;
    let dy = ty - SLING_Y;
    let d = isqrt((dx as i64 * dx as i64 + dy as i64 * dy as i64) as u64) as i32;
    if d < 1 {
        return (0, 0);
    }
    let len = ((d * MAX_PULL) / 150).clamp(18, MAX_PULL);
    clamp_pull(-(dx * len) / d, -(dy * len) / d)
}

/* ---------- самотести ---------- */

/// Прогнати перевірки логіки (викликається і на хості, і на старті МК).
/// Повертає `(скільки_перевірок, скільки_провалено)`.
pub fn selftest_checks(g: &mut Game) -> (u32, u32) {
    let mut run = 0u32;
    let mut failed = 0u32;
    macro_rules! check {
        ($c:expr) => {{
            run += 1;
            if !($c) {
                failed += 1;
                #[cfg(test)]
                std::eprintln!("FAIL: line {}", line!());
            }
        }};
    }

    for s in 1..=stage_count() {
        g.init_stage(s);
        check!(g.block_count as usize <= MAX_BLOCKS);
        check!(g.pig_count as usize <= MAX_PIGS);
        check!(g.birds_left >= 1);
        check!(g.state == State::Aim);

        for i in 0..g.block_count as usize {
            let b = g.blocks[i];
            check!(b.alive);
            check!(b.x >= 0 && b.x + b.w <= FIELD_W);
            check!(b.y_fp >= 0);
            check!(b.bottom_fp() <= GROUND_Y * FP);
            check!(b.hp > 0);
            check!(g.block_supported(i));
        }
        for i in 0..g.block_count as usize {
            for j in (i + 1)..g.block_count as usize {
                check!(!rects_overlap(&g.blocks[i], &g.blocks[j]));
            }
        }
        for i in 0..g.pig_count as usize {
            let p = g.pigs[i];
            check!(p.alive);
            check!(p.x - p.r >= 0 && p.x + p.r <= FIELD_W);
            check!(p.y_fp - p.r * FP >= 0);
            check!(p.y_fp + p.r * FP <= GROUND_Y * FP);
            check!(g.pig_supported(i));
            for k in 0..g.block_count as usize {
                check!(!circle_aabb(p.x * FP, p.y_fp, p.r * FP, &g.blocks[k]));
            }
        }
        // на кожному рівні існує постріл, що влучає у споруду
        check!(g.find_demo_shot().is_some());
    }

    // Політ: птах не провалюється крізь землю і не «зависає» назавжди.
    g.init_stage(1);
    check!(g.set_pull(-38, 22));
    check!(g.launch());
    check!(g.state == State::Flying);
    let mut ticks = 0;
    while g.bird.active && ticks < 400 {
        g.tick();
        ticks += 1;
        check!(g.bird.y_fp <= GROUND_Y * FP + FP / 2);
        check!(g.bird.x_fp > -60 * FP);
    }
    check!(ticks < 400);

    // Натяг понад максимум обрізається до MAX_PULL.
    g.init_stage(1);
    g.set_pull(-300, 300);
    let d = isqrt((g.pull_x as i64 * g.pull_x as i64 + g.pull_y as i64 * g.pull_y as i64) as u64);
    check!(d as i32 <= MAX_PULL);
    check!(g.power_pct() <= 100);

    // Демо-постріл справді руйнує щось на кожному рівні.
    for s in 1..=stage_count() {
        g.init_stage(s);
        let before_pigs = g.pigs_left();
        let before_blocks = g.blocks_left();
        if let Some((px, py)) = g.find_demo_shot() {
            check!(g.set_pull(px, py));
            check!(g.launch());
            for _ in 0..420 {
                g.tick();
                if g.state == State::LevelClear {
                    break;
                }
            }
            check!(g.pigs_left() < before_pigs || g.blocks_left() < before_blocks);
        } else {
            check!(false);
        }
    }

    // Бонус за рівень нараховується рівно один раз.
    g.init_stage(1);
    let base = g.score;
    let mut ev = TickEvents::default();
    g.finish_level(&mut ev);
    check!(ev.stage_clear);
    check!(g.score > base);
    let after = g.score;
    let mut ev2 = TickEvents::default();
    g.finish_level(&mut ev2);
    check!(!ev2.stage_clear);
    check!(g.score == after);

    (run, failed)
}

/* ---------- тести на хості ---------- */

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selftests_pass() {
        let mut g = Game::empty();
        let (run, failed) = selftest_checks(&mut g);
        eprintln!("selftest: {run} перевірок, {failed} провалено");
        assert!(run > 200, "замало перевірок: {run}");
        assert_eq!(failed, 0, "провалено {failed} з {run} перевірок");
    }

    #[test]
    fn launched_bird_flies_and_lands() {
        let mut g = Game::new(1, 1);
        assert!(g.set_pull(-40, 20));
        assert!(g.launch());
        let mut max_up = 0;
        for _ in 0..300 {
            g.tick();
            max_up = max_up.max(GROUND_Y - g.bird.y_fp / FP);
        }
        assert!(max_up > 40, "птах не злетів вище 40 px");
    }

    #[test]
    fn blocks_fall_when_support_removed() {
        let mut g = Game::new(1, 1);
        // знищуємо обидві стійки
        for i in 0..g.block_count as usize {
            if g.blocks[i].h == 36 {
                g.blocks[i].alive = false;
            }
        }
        let beam = g
            .blocks
            .iter()
            .position(|b| b.alive && b.w == 56)
            .expect("балка");
        let y0 = g.blocks[beam].y_fp;
        for _ in 0..30 {
            g.tick();
        }
        assert!(g.blocks[beam].y_fp > y0, "балка не впала після втрати опори");
    }
}

#[cfg(test)]
mod playtest {
    use super::*;

    fn play(stage: u8) -> (bool, u8, u32) {
        let mut g = Game::new(stage, 7);
        let mut guard = 0;
        while g.state != State::LevelClear && g.state != State::GameOver && guard < 4000 {
            guard += 1;
            if g.state == State::Aim {
                if let Some((px, py)) = g.find_demo_shot() {
                    g.set_pull(px, py);
                    g.launch();
                } else {
                    break;
                }
            }
            g.tick();
        }
        (g.state == State::LevelClear, g.pigs_left(), g.score)
    }

    /// Бот (`find_demo_shot`) мусить проходити всі три рівні — це доказ, що вони взагалі проходимі
    /// наявними пострілами, а не «дизайн на папері».
    #[test]
    fn bot_clears_all_levels() {
        for s in 1..=3 {
            let (clear, pigs, score) = play(s);
            eprintln!("stage {s}: clear={clear} pigs_left={pigs} score={score}");
            assert!(clear, "бот не пройшов рівень {s}: лишилось свиней {pigs}");
        }
    }
}
