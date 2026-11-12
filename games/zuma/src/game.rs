//! Логіка гри **Zuma** — чистий `#![no_std]` Rust, без `f32`/`f64`, без алокатора.
//!
//! Координати поля `240 × 240` зберігаються у фіксованій точці `FP = 64` (`1/64` пікселя).
//! Кожна траса параметризована натуральною довжиною дуги `s` (`0..=total_s`), тому дві сусідні
//! кулі торкаються **рівно тоді**, коли `s_front - s_back == BALL_STEP` — незалежно від кривини.
//!
//! Підтримано всю класичну механіку Palm OS / Astraware Zuma:
//! - 3 різні храмові траси (спіраль, зміїний каньйон з нижньою туреллю, ацтекський лабіринт);
//! - швидкий викот початкового поїзда на старті рівня;
//! - постріл з точним входженням кулі **перед** або **за** ураженою кулею (за знаком дотичної);
//! - розриви ланцюга: передній відрізок за розривом зупиняється, а при однакових кольорах на краях
//!   **магнітом розганяється назад**, б'ється, робить каскадний вибух `×2, ×3…` і відштовхує поїзд;
//! - бонуси на кулях: **Slow** (сповільнення), **Reverse** (відкат назад), **Bomb** (вибух по площі);
//! - шкала **ZUMA!**: коли заповнена — нові кулі більше не виходять із тунелю;
//! - жабка генерує лише кольори, що реально залишилися на полі;
//! - коли голова поїзда дістається черепа — весь ланцюг швидко затягується в пащу (`State::Draining`).
//!
//! Файл не залежить від `esp-hal` і тестується на хості:
//! ```text
//! rustc --edition 2024 --test src/game.rs -o /tmp/zuma_tests && /tmp/zuma_tests
//! ```

/// Масштаб фіксованої точки: `1 px = 64` одиниці.
pub const FP: i32 = 64;
pub const FIELD_W: i32 = 240;
pub const FIELD_H: i32 = 240;

/// Радіус та діаметр кулі в пікселях і в одиницях дуги `s`.
pub const BALL_R: i32 = 7;
pub const BALL_D: i32 = 14;
pub const BALL_STEP: i32 = BALL_D * FP; // 896

pub const BULLET_R: i32 = 6;
const HIT_DIST_FP: i32 = (BALL_R + BULLET_R - 1) * FP; // 12 px
const HIT_DIST_SQ: i64 = (HIT_DIST_FP as i64) * (HIT_DIST_FP as i64);
const BULLET_SPEED: i32 = 10 * FP; // 10 px / кадр
const BULLET_SUBSTEPS: i32 = 3;

const BOMB_RADIUS_FP: i32 = 38 * FP;
const BOMB_RADIUS_SQ: i64 = (BOMB_RADIUS_FP as i64) * (BOMB_RADIUS_FP as i64);

pub const MAX_COLORS: u8 = 6;
pub const MAX_PTS: usize = 720;
pub const MAX_BALLS: usize = 128;
pub const MAX_BULLETS: usize = 2;
pub const MAX_FX: usize = 14;
pub const NUM_TRACKS: u8 = 3;

/// Точка полілінії траси з накопиченою довжиною дуги `s`.
#[derive(Clone, Copy, Debug, Default)]
pub struct TrackPt {
    pub x_fp: i16,
    pub y_fp: i16,
    pub s: i32,
}

#[derive(Clone, Copy)]
pub struct Track {
    pub pts: [TrackPt; MAX_PTS],
    pub len: usize,
    pub total_s: i32,
    pub frog_x: i32, // у пікселях (0..240)
    pub frog_y: i32,
    pub kind: u8,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Bonus {
    #[default]
    None,
    Slow,
    Reverse,
    Bomb,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Ball {
    pub color: u8,
    pub s: i32,
    pub vel: i32,
    pub bonus: Bonus,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Bullet {
    pub active: bool,
    pub color: u8,
    pub x_fp: i32,
    pub y_fp: i32,
    pub vx_fp: i32,
    pub vy_fp: i32,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Fx {
    pub active: bool,
    pub x: i16,
    pub y: i16,
    pub color: u8,
    pub ttl: u8,
    pub max_ttl: u8,
    pub combo: u8,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    Playing,
    /// Перша куля зайшла в череп — поїзд стрімко затягується в лунку.
    Draining,
    StageClear,
    GameOver,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct TickEvents {
    pub popped: u16,
    pub gap_combo: u8,
    pub zuma_triggered: bool,
    pub bonus_triggered: Option<Bonus>,
    pub hud_dirty: bool,
}

/* ---------- цілочисельна математика ---------- */

/// Точний цілочисельний квадратний корінь `floor(sqrt(n))`.
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

/// Таблиця `round(sin(deg) * 4096)` для `0..=90` градусів.
const SIN91: [i16; 91] = [
    0, 71, 143, 214, 286, 357, 428, 499, 570, 641, 711, 781, 852, 921, 991, 1060, 1129, 1197,
    1266, 1334, 1401, 1468, 1534, 1600, 1666, 1731, 1796, 1860, 1923, 1986, 2048, 2109, 2170,
    2230, 2290, 2349, 2408, 2465, 2522, 2578, 2633, 2687, 2741, 2794, 2845, 2896, 2946, 2996,
    3044, 3091, 3138, 3183, 3228, 3272, 3314, 3355, 3396, 3435, 3474, 3511, 3547, 3582, 3617,
    3650, 3681, 3712, 3742, 3770, 3798, 3824, 3849, 3873, 3896, 3917, 3937, 3956, 3974, 3991,
    4006, 4020, 4033, 4045, 4056, 4065, 4073, 4080, 4086, 4090, 4093, 4095, 4096,
];

/// `sin(deg) * 4096` для довільного цілого кута в градусах.
pub fn sin_deg(deg: i32) -> i32 {
    let a = deg.rem_euclid(360);
    match a {
        0..=90 => SIN91[a as usize] as i32,
        91..=180 => SIN91[(180 - a) as usize] as i32,
        181..=270 => -(SIN91[(a - 180) as usize] as i32),
        _ => -(SIN91[(360 - a) as usize] as i32),
    }
}

pub fn cos_deg(deg: i32) -> i32 {
    sin_deg(deg + 90)
}

/* ---------- побудова та запити до траси ---------- */

impl Track {
    pub const fn empty() -> Self {
        Self {
            pts: [TrackPt {
                x_fp: 0,
                y_fp: 0,
                s: 0,
            }; MAX_PTS],
            len: 0,
            total_s: 0,
            frog_x: 120,
            frog_y: 120,
            kind: 0,
        }
    }

    fn clear(&mut self) {
        self.len = 0;
        self.total_s = 0;
    }

    /// Додати вершину `(x_fp, y_fp)`; якщо крок завеликий — автоматично розбити на рівні підкроки
    /// (~3 px), щоб інтерполяція та візуальне малювання жолоба були гладкими.
    fn push_fp(&mut self, x_fp: i32, y_fp: i32) {
        let x_fp = x_fp.clamp(8 * FP, (FIELD_W - 8) * FP);
        let y_fp = y_fp.clamp(8 * FP, (FIELD_H - 8) * FP);
        if self.len == 0 {
            self.pts[0] = TrackPt {
                x_fp: x_fp as i16,
                y_fp: y_fp as i16,
                s: 0,
            };
            self.len = 1;
            return;
        }
        let prev = self.pts[self.len - 1];
        let dx = x_fp - prev.x_fp as i32;
        let dy = y_fp - prev.y_fp as i32;
        let dist = isqrt((dx as i64 * dx as i64 + dy as i64 * dy as i64) as u64) as i32;
        if dist < FP {
            return; // менше 1 px — пропускаємо дубль
        }
        let max_seg = 3 * FP; // ~3 px між точками таблиці
        let steps = ((dist + max_seg - 1) / max_seg).max(1);
        for k in 1..=steps {
            if self.len >= MAX_PTS {
                break;
            }
            let nx = prev.x_fp as i32 + dx * k / steps;
            let ny = prev.y_fp as i32 + dy * k / steps;
            let p0 = self.pts[self.len - 1];
            let sdx = nx - p0.x_fp as i32;
            let sdy = ny - p0.y_fp as i32;
            let seg = isqrt((sdx as i64 * sdx as i64 + sdy as i64 * sdy as i64) as u64) as i32;
            if seg == 0 {
                continue;
            }
            self.total_s += seg;
            self.pts[self.len] = TrackPt {
                x_fp: nx as i16,
                y_fp: ny as i16,
                s: self.total_s,
            };
            self.len += 1;
        }
    }

    fn push_arc_deg(&mut self, cx_px: i32, cy_px: i32, r_px: i32, deg_start: i32, deg_end: i32) {
        let span = (deg_end - deg_start).abs();
        let steps = (span / 5).max(4);
        for k in 0..=steps {
            let deg = deg_start + (deg_end - deg_start) * k / steps;
            let x_fp = cx_px * FP + (r_px * FP * cos_deg(deg)) / 4096;
            let y_fp = cy_px * FP + (r_px * FP * sin_deg(deg)) / 4096;
            self.push_fp(x_fp, y_fp);
        }
    }

    /// Згенерувати одну з 3 храмових трас за індексом `kind_idx` (`0..3`).
    pub fn build(&mut self, kind_idx: u8) {
        self.clear();
        let k = kind_idx % NUM_TRACKS;
        self.kind = k;
        match k {
            0 => {
                // Траса 0: «Храмова спіраль» — жабка в центрі (120, 120),
                // плавний вхід зверху + 2.2 оберту за годинниковою стрілкою (крок витка 28 px).
                self.frog_x = 120;
                self.frog_y = 120;
                self.push_fp(14 * FP, 15 * FP);
                self.push_fp(120 * FP, 15 * FP);
                // r(d) зменшується від 105 px до 43 px за 792 градуси (28 px / 360°)
                let total_deg = 792;
                let steps = total_deg / 5;
                for i in 1..=steps {
                    let d = i * total_deg / steps;
                    let deg = -90 + d;
                    let r_fp = 105 * FP - (28 * FP * d) / 360;
                    let x_fp = 120 * FP + (r_fp * cos_deg(deg)) / 4096;
                    let y_fp = 120 * FP + (r_fp * sin_deg(deg)) / 4096;
                    self.push_fp(x_fp, y_fp);
                }
            }
            1 => {
                // Траса 1: «Зміїний каньйон» — жабка знизу (120, 206), 4 горизонтальні коридори.
                self.frog_x = 120;
                self.frog_y = 206;
                self.push_fp(14 * FP, 22 * FP);
                self.push_arc_deg(203, 41, 19, -90, 90);
                self.push_arc_deg(37, 79, 19, -90, -270);
                self.push_arc_deg(203, 117, 19, -90, 90);
                self.push_fp(34 * FP, 136 * FP);
            }
            _ => {
                // Траса 2: «Ацтекський лабіринт» — прямокутна спіраль із заокругленими кутами (r=16),
                // крок між коридорами рівно 28 px, жабка в центрі (120, 120).
                self.frog_x = 120;
                self.frog_y = 120;
                let r = 16;
                self.push_fp(16 * FP, 18 * FP);
                // Зовнішній контур: y=18 -> x=222 -> y=222 -> x=18 -> y=46
                self.push_arc_deg(222 - r, 18 + r, r, -90, 0);
                self.push_arc_deg(222 - r, 222 - r, r, 0, 90);
                self.push_arc_deg(18 + r, 222 - r, r, 90, 180);
                self.push_arc_deg(18 + r, 46 + r, r, 180, 270);
                // Середній виток: y=46 -> x=194 -> y=194 -> x=46 -> y=74
                self.push_arc_deg(194 - r, 46 + r, r, -90, 0);
                self.push_arc_deg(194 - r, 194 - r, r, 0, 90);
                self.push_arc_deg(46 + r, 194 - r, r, 90, 180);
                self.push_arc_deg(46 + r, 74 + r, r, 180, 270);
                // Внутрішній виток: y=74 -> x=166 -> вниз до черепа (166, 154)
                self.push_arc_deg(166 - r, 74 + r, r, -90, 0);
                self.push_fp(166 * FP, 154 * FP);
            }
        }
    }

    /// Позиція `(x_fp, y_fp)` на дузі `s` (`0..=total_s`), лінійно інтерпольована по таблиці.
    pub fn pos_fp(&self, s: i32) -> (i32, i32) {
        if self.len == 0 {
            return (120 * FP, 120 * FP);
        }
        if s <= 0 {
            let p = self.pts[0];
            return (p.x_fp as i32, p.y_fp as i32);
        }
        if s >= self.total_s || self.len == 1 {
            let p = self.pts[self.len - 1];
            return (p.x_fp as i32, p.y_fp as i32);
        }
        // Двійковий пошук: найбільше `lo`, де `pts[lo].s <= s`.
        let mut lo = 0usize;
        let mut hi = self.len - 1;
        while lo + 1 < hi {
            let mid = (lo + hi) >> 1;
            if self.pts[mid].s <= s {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        let a = self.pts[lo];
        let b = self.pts[hi];
        let ds = (b.s - a.s).max(1);
        let t = s - a.s;
        let x = a.x_fp as i32 + (b.x_fp as i32 - a.x_fp as i32) * t / ds;
        let y = a.y_fp as i32 + (b.y_fp as i32 - a.y_fp as i32) * t / ds;
        (x, y)
    }

    /// Позиція `(x_px, y_px)` у пікселях.
    pub fn pos_px(&self, s: i32) -> (i32, i32) {
        let (xf, yf) = self.pos_fp(s);
        ((xf + FP / 2) / FP, (yf + FP / 2) / FP)
    }

    /// Дотичний вектор у точці `s` (напрямок руху до черепа).
    pub fn tangent_fp(&self, s: i32) -> (i32, i32) {
        let (x0, y0) = self.pos_fp((s - 2 * FP).max(0));
        let (x1, y1) = self.pos_fp((s + 2 * FP).min(self.total_s));
        (x1 - x0, y1 - y0)
    }

    pub fn skull_px(&self) -> (i32, i32) {
        self.pos_px(self.total_s)
    }

    pub fn start_px(&self) -> (i32, i32) {
        self.pos_px(0)
    }
}

/* ---------- головний стан гри ---------- */

pub struct Game {
    pub track: Track,
    pub balls: [Ball; MAX_BALLS],
    pub ball_count: usize,
    pub bullets: [Bullet; MAX_BULLETS],
    pub fx: [Fx; MAX_FX],

    pub stage: u8,
    pub num_colors: u8,
    pub score: u32,
    pub best: u32,

    pub zuma_meter: u16,
    pub zuma_target: u16,
    pub zuma_achieved: bool,

    pub state: State,
    /// Напрямок прицілу жабки, нормований до довжини `FP` (`64`).
    pub frog_dir: (i32, i32),
    pub cur_color: u8,
    pub next_color: u8,
    pub fire_cd: u8,

    pub chain_combo: u8,
    pub last_combo: u8,
    pub combo_ttl: u8,

    pub slow_ticks: u16,
    pub recoil_ticks: u16,
    pub roll_in_ticks: u16,
    pub bonus_timer: u16,
    pub tick_no: u32,

    rng: u32,
}

impl Game {
    pub const fn empty() -> Self {
        Self {
            track: Track::empty(),
            balls: [Ball {
                color: 1,
                s: 0,
                vel: 0,
                bonus: Bonus::None,
            }; MAX_BALLS],
            ball_count: 0,
            bullets: [Bullet {
                active: false,
                color: 1,
                x_fp: 0,
                y_fp: 0,
                vx_fp: 0,
                vy_fp: 0,
            }; MAX_BULLETS],
            fx: [Fx {
                active: false,
                x: 0,
                y: 0,
                color: 1,
                ttl: 0,
                max_ttl: 1,
                combo: 0,
            }; MAX_FX],
            stage: 1,
            num_colors: 4,
            score: 0,
            best: 0,
            zuma_meter: 0,
            zuma_target: 28,
            zuma_achieved: false,
            state: State::Playing,
            frog_dir: (0, -FP),
            cur_color: 1,
            next_color: 2,
            fire_cd: 0,
            chain_combo: 1,
            last_combo: 1,
            combo_ttl: 0,
            slow_ticks: 0,
            recoil_ticks: 0,
            roll_in_ticks: 0,
            bonus_timer: 220,
            tick_no: 0,
            rng: 0xA341_316C,
        }
    }

    pub fn new(stage: u8, seed: u32) -> Self {
        let mut g = Self::empty();
        g.start_new_game(stage, seed);
        g
    }

    fn rand(&mut self) -> u32 {
        let mut x = if self.rng == 0 { 0x9E37_79B9 } else { self.rng };
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.rng = x;
        x
    }

    fn rand_color_any(&mut self) -> u8 {
        1 + (self.rand() % self.num_colors as u32) as u8
    }

    /// Які кольори зараз присутні на полі (бітова маска бітів `1..=6`).
    pub fn active_color_mask(&self) -> u8 {
        let mut m = 0u8;
        for i in 0..self.ball_count {
            let c = self.balls[i].color;
            if (1..=MAX_COLORS).contains(&c) {
                m |= 1 << c;
            }
        }
        m
    }

    /// Вибрати колір для жабки серед тих, що реально є в ланцюгу (щоб не блокувати фінал рівня).
    fn pick_frog_color(&mut self) -> u8 {
        let mask = self.active_color_mask();
        if mask == 0 {
            return self.rand_color_any();
        }
        let mut list = [0u8; MAX_COLORS as usize];
        let mut n = 0usize;
        for c in 1..=self.num_colors {
            if (mask & (1 << c)) != 0 {
                list[n] = c;
                n += 1;
            }
        }
        if n == 0 {
            self.rand_color_any()
        } else {
            list[(self.rand() as usize) % n]
        }
    }

    /// Якщо поточний або наступний колір зник з поля — оновити кулю в роті жабки.
    fn sync_frog_colors(&mut self) {
        let mask = self.active_color_mask();
        if mask == 0 {
            return;
        }
        if (mask & (1 << self.cur_color)) == 0 {
            self.cur_color = self.pick_frog_color();
        }
        if (mask & (1 << self.next_color)) == 0 {
            self.next_color = self.pick_frog_color();
        }
    }

    /// Створити нову кулю на вході в тунель: ~42 % шанс повторити попередній колір (природні пари,
    /// як в оригінальній Zuma), але ніколи не давати 3 однакові поспіль автоматично.
    fn pick_spawn_color(&mut self) -> u8 {
        if self.ball_count > 0 {
            let prev = self.balls[0].color;
            let two_same = self.ball_count >= 2 && self.balls[1].color == prev;
            if !two_same && (self.rand() % 100) < 42 {
                return prev;
            }
            if two_same {
                for _ in 0..8 {
                    let c = self.rand_color_any();
                    if c != prev {
                        return c;
                    }
                }
                return 1 + (prev % self.num_colors);
            }
        }
        self.rand_color_any()
    }

    pub fn start_new_game(&mut self, start_stage: u8, seed: u32) {
        self.rng = if seed == 0 { 0xCAFE_BABE } else { seed };
        self.score = 0;
        let stg = start_stage.max(1);
        self.init_stage(stg);
    }

    pub fn init_stage(&mut self, stage: u8) {
        let stg = stage.max(1);
        self.stage = stg;
        // Плавне наростання складності: рівень 1 — 4 кольори, рівень 2 — 5, від 3-го — усі 6.
        self.num_colors = match stg {
            1 => 4,
            2 => 5,
            _ => 6,
        };
        self.track.build(stg - 1);
        self.ball_count = 0;
        self.bullets = [Bullet::default(); MAX_BULLETS];
        self.fx = [Fx::default(); MAX_FX];

        self.zuma_meter = 0;
        self.zuma_target = 28 + (stg as u16 - 1) * 8;
        self.zuma_achieved = false;
        self.state = State::Playing;

        self.frog_dir = (0, -FP);
        self.fire_cd = 0;
        self.chain_combo = 1;
        self.last_combo = 1;
        self.combo_ttl = 0;
        self.slow_ticks = 0;
        self.recoil_ticks = 0;
        self.roll_in_ticks = 80;
        self.bonus_timer = 180;
        self.tick_no = 0;

        // Початковий поїзд із 18 куль готовий на вході й бадьоро викочується на поле.
        let init_n = 18usize;
        for _ in 0..init_n {
            self.spawn_tail_ball();
        }
        self.cur_color = self.pick_frog_color();
        self.next_color = self.pick_frog_color();
    }

    /// Вставити одну нову кулю в хвіст (`balls[0]`).
    fn spawn_tail_ball(&mut self) {
        if self.ball_count >= MAX_BALLS {
            return;
        }
        let s_new = if self.ball_count == 0 {
            0
        } else {
            self.balls[0].s - BALL_STEP
        };
        let c = self.pick_spawn_color();
        for j in (1..=self.ball_count).rev() {
            self.balls[j] = self.balls[j - 1];
        }
        self.balls[0] = Ball {
            color: c,
            s: s_new,
            vel: 0,
            bonus: Bonus::None,
        };
        self.ball_count += 1;
    }

    /// Базова швидкість натиску з тунелю (в одиницях `s` за тік).
    fn current_push_speed(&self) -> i32 {
        let cruise = (22 + (self.stage as i32 - 1) * 4).min(46); // ~0.34..0.72 px/кадр
        if self.slow_ticks > 0 {
            return (cruise / 3).max(8);
        }
        // Швидкий викот на старті рівня, поки голова не пройде 28 % траси.
        let roll_limit = self.track.total_s * 28 / 100;
        let head_s = if self.ball_count > 0 {
            self.balls[self.ball_count - 1].s
        } else {
            0
        };
        if self.roll_in_ticks > 0 && head_s < roll_limit {
            let rem = roll_limit - head_s;
            let boost = (rem * 180 / roll_limit.max(1)).clamp(0, 180);
            return cruise + boost;
        }
        // Якщо гравець розчистив майже все поле до ZUMA — підкотити свіжу хвилю жвавіше.
        if !self.zuma_achieved && head_s < BALL_STEP * 4 {
            return cruise * 3;
        }
        cruise
    }

    /// Рівень небезпеки `0..=100` (наскільки близько голова поїзда до пащі черепа).
    pub fn danger_pct(&self) -> u8 {
        if self.ball_count == 0 || self.track.total_s <= 0 {
            return 0;
        }
        let head = self.balls[self.ball_count - 1].s;
        let warn_start = self.track.total_s * 72 / 100;
        if head <= warn_start {
            0
        } else {
            (((head - warn_start) * 100) / (self.track.total_s - warn_start).max(1)).clamp(0, 100)
                as u8
        }
    }

    /// Навести жабку на піксель `(px, py)` поля.
    pub fn aim_at(&mut self, px: i32, py: i32) {
        let dx = px * FP - self.track.frog_x * FP;
        let dy = py * FP - self.track.frog_y * FP;
        let len = isqrt((dx as i64 * dx as i64 + dy as i64 * dy as i64) as u64) as i32;
        if len >= 4 * FP {
            let ux = (dx * FP) / len;
            let uy = (dy * FP) / len;
            if ux != 0 || uy != 0 {
                self.frog_dir = (ux, uy);
            }
        }
    }

    /// Промінь прицілу від рота жабки до першої кулі або краю поля `240×240` (у пікселях).
    pub fn raycast_aim(&self) -> (i32, i32) {
        let (ux, uy) = self.frog_dir;
        let mut x = self.track.frog_x * FP + ux * 14;
        let mut y = self.track.frog_y * FP + uy * 14;
        let step_x = ux * 3;
        let step_y = uy * 3;
        for _ in 0..90 {
            x += step_x;
            y += step_y;
            if !(4 * FP..(FIELD_W - 4) * FP).contains(&x)
                || !(4 * FP..(FIELD_H - 4) * FP).contains(&y)
            {
                break;
            }
            for i in 0..self.ball_count {
                let s = self.balls[i].s;
                if s < 0 || s > self.track.total_s {
                    continue;
                }
                let (bx, by) = self.track.pos_fp(s);
                let dx = (x - bx) as i64;
                let dy = (y - by) as i64;
                if dx * dx + dy * dy <= HIT_DIST_SQ {
                    return ((x + FP / 2) / FP, (y + FP / 2) / FP);
                }
            }
        }
        (
            ((x + FP / 2) / FP).clamp(2, FIELD_W - 3),
            ((y + FP / 2) / FP).clamp(2, FIELD_H - 3),
        )
    }

    /// Поміняти місцями поточну й наступну кулю в жабці.
    pub fn swap_balls(&mut self) -> bool {
        if !matches!(self.state, State::Playing) {
            return false;
        }
        core::mem::swap(&mut self.cur_color, &mut self.next_color);
        true
    }

    /// Постріл уздовж поточного `frog_dir`.
    pub fn shoot(&mut self) -> bool {
        if !matches!(self.state, State::Playing) || self.fire_cd > 0 {
            return false;
        }
        let Some(slot) = self.bullets.iter().position(|b| !b.active) else {
            return false;
        };
        let (ux, uy) = self.frog_dir;
        let start_x = self.track.frog_x * FP + ux * 12;
        let start_y = self.track.frog_y * FP + uy * 12;
        self.bullets[slot] = Bullet {
            active: true,
            color: self.cur_color,
            x_fp: start_x,
            y_fp: start_y,
            vx_fp: (ux * BULLET_SPEED) / FP,
            vy_fp: (uy * BULLET_SPEED) / FP,
        };
        self.cur_color = self.next_color;
        self.next_color = self.pick_frog_color();
        self.fire_cd = 6;
        true
    }

    fn spawn_fx(&mut self, x_px: i32, y_px: i32, color: u8, combo: u8) {
        if let Some(slot) = self.fx.iter_mut().find(|f| !f.active) {
            *slot = Fx {
                active: true,
                x: x_px as i16,
                y: y_px as i16,
                color,
                ttl: 8,
                max_ttl: 8,
                combo,
            };
        }
    }

    #[inline]
    fn touching(&self, left: usize, right: usize) -> bool {
        self.balls[right].s - self.balls[left].s <= BALL_STEP + 4
    }

    /// Знайти межі суцільного дотичного ланцюжка однакового кольору навколо індексу `pivot`.
    fn color_run_around(&self, pivot: usize) -> (usize, usize) {
        let c = self.balls[pivot].color;
        let mut l = pivot;
        while l > 0 && self.touching(l - 1, l) && self.balls[l - 1].color == c {
            l -= 1;
        }
        let mut r = pivot;
        while r + 1 < self.ball_count && self.touching(r, r + 1) && self.balls[r + 1].color == c {
            r += 1;
        }
        (l, r)
    }

    /// Видалити групу `l..=r` (плюс радіус вибуху, якщо всередині була `Bonus::Bomb`),
    /// нарахувати очки, шкалу ZUMA!, візуальні спалахи та активувати бонуси.
    fn pop_range(&mut self, l: usize, r: usize, combo: u8, ev: &mut TickEvents) {
        let mut mask = [false; MAX_BALLS];
        for m in mask.iter_mut().take(r + 1).skip(l) {
            *m = true;
        }

        // Якщо серед підірваних є Bomb — розширити маску на всі кулі в радіусі 38 px (каскадно).
        loop {
            let mut expanded = false;
            for i in 0..self.ball_count {
                if mask[i] && self.balls[i].bonus == Bonus::Bomb {
                    let (bx, by) = self.track.pos_fp(self.balls[i].s);
                    for (j, marked_j) in mask.iter_mut().enumerate().take(self.ball_count) {
                        if !*marked_j && self.balls[j].s >= 0 {
                            let (jx, jy) = self.track.pos_fp(self.balls[j].s);
                            let dx = (bx - jx) as i64;
                            let dy = (by - jy) as i64;
                            if dx * dx + dy * dy <= BOMB_RADIUS_SQ {
                                *marked_j = true;
                                expanded = true;
                            }
                        }
                    }
                    // Щоб не крутити ту саму бомбу повторно
                    self.balls[i].bonus = Bonus::None;
                    ev.bonus_triggered = Some(Bonus::Bomb);
                }
            }
            if !expanded {
                break;
            }
        }

        let mut removed = 0u32;
        let mut fx_emitted = 0u8;
        let mid = (l + r) / 2;
        for (i, &marked) in mask.iter().enumerate().take(self.ball_count) {
            if marked {
                removed += 1;
                match self.balls[i].bonus {
                    Bonus::Slow => {
                        self.slow_ticks = 175;
                        ev.bonus_triggered = Some(Bonus::Slow);
                    }
                    Bonus::Reverse => {
                        self.recoil_ticks = self.recoil_ticks.max(55);
                        ev.bonus_triggered = Some(Bonus::Reverse);
                    }
                    Bonus::Bomb | Bonus::None => {}
                }
                if fx_emitted < 5 && self.balls[i].s >= 0 {
                    let (px, py) = self.track.pos_px(self.balls[i].s);
                    let tag = if i == mid && combo > 1 { combo } else { 0 };
                    self.spawn_fx(px, py, self.balls[i].color, tag);
                    fx_emitted += 1;
                }
            }
        }

        // Стискання масиву куль без порушення порядку й інваріанта s[j+1] - s[j] >= BALL_STEP.
        let mut w = 0usize;
        for (i, &marked) in mask.iter().enumerate().take(self.ball_count) {
            if !marked {
                self.balls[w] = self.balls[i];
                w += 1;
            }
        }
        self.ball_count = w;

        let pts = removed * 10 * (combo as u32)
            + if combo > 1 {
                (combo as u32 - 1) * 100
            } else {
                0
            };
        self.score += pts;
        if self.score > self.best {
            self.best = self.score;
        }
        ev.popped = ev.popped.saturating_add(removed as u16);
        ev.hud_dirty = true;
        if combo > 1 {
            ev.gap_combo = ev.gap_combo.max(combo);
            self.last_combo = combo;
            self.combo_ttl = 40;
        }

        if !self.zuma_achieved {
            self.zuma_meter = self.zuma_meter.saturating_add(removed as u16);
            if self.zuma_meter >= self.zuma_target {
                self.zuma_meter = self.zuma_target;
                self.zuma_achieved = true;
                ev.zuma_triggered = true;
                // Класичний урочистий поштовх поїзда назад при «ZUMA!»
                self.recoil_ticks = self.recoil_ticks.max(32);
            }
        }

        self.sync_frog_colors();
    }

    /// Вставити снаряд кольору `color`, який влучив у `balls[hit_idx]` у точці `(bx_fp, by_fp)`.
    fn insert_bullet_at(&mut self, hit_idx: usize, bx_fp: i32, by_fp: i32, color: u8, ev: &mut TickEvents) {
        if self.ball_count == 0 || self.ball_count >= MAX_BALLS {
            return;
        }
        let s_hit = self.balls[hit_idx].s;
        let (cx, cy) = self.track.pos_fp(s_hit);
        let (tx, ty) = self.track.tangent_fp(s_hit);
        let dot = (bx_fp - cx) as i64 * (tx as i64) + (by_fp - cy) as i64 * (ty as i64);
        let ahead = dot >= 0;

        let (ins, s_new) = if ahead {
            (hit_idx + 1, s_hit + BALL_STEP)
        } else if hit_idx > 0 && self.balls[hit_idx].s - self.balls[hit_idx - 1].s > BALL_STEP {
            let s_cand = (s_hit - BALL_STEP).max(self.balls[hit_idx - 1].s + BALL_STEP);
            (hit_idx, s_cand)
        } else {
            (hit_idx, s_hit)
        };

        for j in (ins + 1..=self.ball_count).rev() {
            self.balls[j] = self.balls[j - 1];
        }
        self.balls[ins] = Ball {
            color,
            s: s_new,
            vel: 0,
            bonus: Bonus::None,
        };
        self.ball_count += 1;

        // Плавно посунути передніх сусідів уперед, якщо їм не вистачає місця (розрив попереду поглине зсув).
        for j in (ins + 1)..self.ball_count {
            let min_s = self.balls[j - 1].s + BALL_STEP;
            if self.balls[j].s < min_s {
                self.balls[j].s = min_s;
            } else {
                break;
            }
        }

        let (l, r) = self.color_run_around(ins);
        if r + 1 - l >= 3 {
            self.chain_combo = 1;
            self.pop_range(l, r, 1, ev);
        } else {
            self.sync_frog_colors();
        }
    }

    /// Один кадр симуляції (~30 мс).
    pub fn tick(&mut self) -> TickEvents {
        let mut ev = TickEvents::default();
        self.tick_no = self.tick_no.wrapping_add(1);

        // Оновити таймери візуальних ефектів
        for f in self.fx.iter_mut() {
            if f.active {
                if f.ttl <= 1 {
                    f.active = false;
                } else {
                    f.ttl -= 1;
                }
            }
        }
        if self.combo_ttl > 0 {
            self.combo_ttl -= 1;
            if self.combo_ttl == 0 {
                ev.hud_dirty = true;
            }
        }
        if self.fire_cd > 0 {
            self.fire_cd -= 1;
        }
        if self.roll_in_ticks > 0 {
            self.roll_in_ticks -= 1;
        }
        if self.slow_ticks > 0 {
            self.slow_ticks -= 1;
            if self.slow_ticks == 0 {
                ev.hud_dirty = true;
            }
        }

        match self.state {
            State::StageClear | State::GameOver => return ev,
            State::Draining => {
                // Поїзд стрімко всмоктується в череп
                let drain_v = 9 * FP;
                for i in 0..self.ball_count {
                    self.balls[i].s += drain_v;
                }
                while self.ball_count > 0 && self.balls[self.ball_count - 1].s >= self.track.total_s
                {
                    let (sx, sy) = self.track.skull_px();
                    let c = self.balls[self.ball_count - 1].color;
                    self.ball_count -= 1;
                    if self.ball_count.is_multiple_of(4) {
                        self.spawn_fx(sx, sy, c, 0);
                    }
                }
                if self.ball_count == 0 {
                    self.state = State::GameOver;
                    ev.hud_dirty = true;
                }
                return ev;
            }
            State::Playing => {}
        }

        // Періодично запалювати бонус на випадковій видимій кулі
        if self.bonus_timer > 0 {
            self.bonus_timer -= 1;
        } else {
            self.bonus_timer = 260;
            if self.ball_count >= 6 {
                let idx = (self.rand() as usize) % self.ball_count;
                if self.balls[idx].s > 4 * BALL_STEP && self.balls[idx].bonus == Bonus::None {
                    self.balls[idx].bonus = match self.rand() % 3 {
                        0 => Bonus::Slow,
                        1 => Bonus::Reverse,
                        _ => Bonus::Bomb,
                    };
                }
            }
        }

        // 1. Політ куль-снарядів (у кілька підкроків, щоб унеможливити тунелювання)
        for b_idx in 0..MAX_BULLETS {
            if !self.bullets[b_idx].active {
                continue;
            }
            let vx = self.bullets[b_idx].vx_fp / BULLET_SUBSTEPS;
            let vy = self.bullets[b_idx].vy_fp / BULLET_SUBSTEPS;
            let b_color = self.bullets[b_idx].color;
            for _ in 0..BULLET_SUBSTEPS {
                self.bullets[b_idx].x_fp += vx;
                self.bullets[b_idx].y_fp += vy;
                let bx = self.bullets[b_idx].x_fp;
                let by = self.bullets[b_idx].y_fp;

                if !(-8 * FP..=(FIELD_W + 8) * FP).contains(&bx)
                    || !(-8 * FP..=(FIELD_H + 8) * FP).contains(&by)
                {
                    self.bullets[b_idx].active = false;
                    break;
                }

                // Перевірка зіткнення з кулями ланцюга (найближча в межах HIT_DIST_SQ)
                let mut best_hit: Option<(usize, i64)> = None;
                for i in 0..self.ball_count {
                    let s = self.balls[i].s;
                    if s < -BALL_STEP / 2 || s > self.track.total_s {
                        continue;
                    }
                    let (cx, cy) = self.track.pos_fp(s.max(0));
                    let dx = (bx - cx) as i64;
                    let dy = (by - cy) as i64;
                    let d2 = dx * dx + dy * dy;
                    if d2 <= HIT_DIST_SQ && best_hit.is_none_or(|(_, bd)| d2 < bd) {
                        best_hit = Some((i, d2));
                    }
                }
                if let Some((hit_idx, _)) = best_hit {
                    self.bullets[b_idx].active = false;
                    self.insert_bullet_at(hit_idx, bx, by, b_color, &mut ev);
                    break;
                }
            }
        }

        // 2. Додати нові кулі з тунелю, якщо шкала ZUMA ще не заповнена
        if !self.zuma_achieved && self.recoil_ticks == 0 {
            while (self.ball_count == 0 || self.balls[0].s >= BALL_STEP)
                && self.ball_count < MAX_BALLS - 6
            {
                self.spawn_tail_ball();
            }
        }

        if self.ball_count == 0 {
            if self.zuma_achieved {
                self.score += 500 * self.stage as u32;
                if self.score > self.best {
                    self.best = self.score;
                }
                self.state = State::StageClear;
                ev.hud_dirty = true;
            }
            return ev;
        }

        // 3. Зафіксувати, між якими сусідами ДО руху був розрив, і розрахувати швидкості сегментів
        let mut was_gap = [false; MAX_BALLS];
        for (i, gap_slot) in was_gap
            .iter_mut()
            .enumerate()
            .take(self.ball_count.saturating_sub(1))
        {
            *gap_slot = !self.touching(i, i + 1);
        }

        let push_v = if self.recoil_ticks > 0 {
            self.recoil_ticks -= 1;
            if self.recoil_ticks == 0 {
                ev.hud_dirty = true;
            }
            -145
        } else {
            self.current_push_speed()
        };

        let mut seg_start = 0usize;
        let mut is_rear_seg = true;
        while seg_start < self.ball_count {
            let mut seg_end = seg_start + 1;
            while seg_end < self.ball_count && !was_gap[seg_end - 1] {
                seg_end += 1;
            }

            let seg_v = if is_rear_seg {
                push_v
            } else {
                let left_c = self.balls[seg_start - 1].color;
                let right_c = self.balls[seg_start].color;
                let prev_v = self.balls[seg_start].vel;
                if left_c == right_c {
                    // Магнітне притягання однакових кольорів через розрив!
                    (prev_v.min(-80) - 28).max(-320)
                } else if prev_v < 0 {
                    // Затухання інерції відкату
                    (prev_v + 28).min(0)
                } else {
                    // Ізольований відрізок попереду розриву чекає на місці
                    0
                }
            };

            for b in self.balls.iter_mut().take(seg_end).skip(seg_start) {
                b.vel = seg_v;
                b.s += seg_v;
            }

            is_rear_seg = false;
            seg_start = seg_end;
        }

        // 4. Не дозволяти хвосту відкочуватися занадто глибоко в тунель
        if self.balls[0].s < -3 * BALL_STEP {
            let shift = -3 * BALL_STEP - self.balls[0].s;
            for j in 0..self.ball_count {
                if j > 0 && was_gap[j - 1] {
                    break;
                }
                self.balls[j].s += shift;
                self.balls[j].vel = 0;
            }
        }

        // 5. Зіткнення сегментів (схлопування розривів) та каскадні вибухи!
        for i in 0..self.ball_count.saturating_sub(1) {
            if self.balls[i + 1].s <= self.balls[i].s + BALL_STEP {
                let shift = (self.balls[i].s + BALL_STEP) - self.balls[i + 1].s;
                if shift > 0 {
                    // Вирівняти передній сегмент у точний контакт
                    for j in (i + 1)..self.ball_count {
                        if j > i + 1 && was_gap[j - 1] && self.balls[j].s > self.balls[j - 1].s + BALL_STEP {
                            break;
                        }
                        self.balls[j].s += shift;
                        self.balls[j].vel = 0;
                    }
                }
                if was_gap[i] && self.balls[i].color == self.balls[i + 1].color {
                    let (l, r) = self.color_run_around(i);
                    if r + 1 - l >= 3 {
                        self.chain_combo = self.chain_combo.saturating_add(1).max(2);
                        let combo = self.chain_combo;
                        self.pop_range(l, r, combo, &mut ev);
                        self.recoil_ticks = self.recoil_ticks.max(14 + 5 * combo as u16);
                        break;
                    }
                }
            }
        }

        // 6. Перевірка фінішу рівня або потрапляння голови поїзда в череп
        if self.ball_count == 0 {
            if self.zuma_achieved {
                self.score += 500 * self.stage as u32;
                if self.score > self.best {
                    self.best = self.score;
                }
                self.state = State::StageClear;
                ev.hud_dirty = true;
            }
        } else if self.balls[self.ball_count - 1].s >= self.track.total_s {
            self.state = State::Draining;
            self.bullets = [Bullet::default(); MAX_BULLETS];
            ev.hud_dirty = true;
        }

        ev
    }

    /// Перевірити, чи прямий постріл з жабки в кулю `target_idx` не перекритий іншим витком траси.
    fn has_clear_line_of_sight(&self, target_idx: usize) -> bool {
        let s_t = self.balls[target_idx].s;
        if s_t < BALL_STEP / 2 || s_t > self.track.total_s - BALL_STEP / 2 {
            return false;
        }
        let (tx, ty) = self.track.pos_fp(s_t);
        let fx = self.track.frog_x * FP;
        let fy = self.track.frog_y * FP;
        let dx = tx - fx;
        let dy = ty - fy;
        let dist = isqrt((dx as i64 * dx as i64 + dy as i64 * dy as i64) as u64) as i32;
        if dist < 16 * FP {
            return false;
        }
        let ux = (dx * FP) / dist;
        let uy = (dy * FP) / dist;
        let mut x = fx + ux * 12;
        let mut y = fy + uy * 12;
        let steps = (dist / (3 * FP)).max(1);
        for _ in 0..=steps {
            x += ux * 3;
            y += uy * 3;
            for i in 0..self.ball_count {
                let s = self.balls[i].s;
                if s < 0 || s > self.track.total_s {
                    continue;
                }
                let (bx, by) = self.track.pos_fp(s);
                let ddx = (x - bx) as i64;
                let ddy = (y - by) as i64;
                if ddx * ddx + ddy * ddy <= HIT_DIST_SQ {
                    // Дозволяємо влучити в саму ціль або сусідню кулю тієї самої одноколірної групи
                    return i == target_idx || self.balls[i].color == self.balls[target_idx].color;
                }
            }
        }
        true
    }

    /// Автопілот для `--features demo`: шукає відкриту групу свого кольору (перевага парам та голові поїзда);
    /// якщо для `cur_color` влучного пострілу немає, а для `next_color` є — пропонує `swap`.
    /// Повертає `(need_swap, aim_x_px, aim_y_px)`.
    pub fn find_demo_shot(&self) -> Option<(bool, i32, i32)> {
        if !matches!(self.state, State::Playing) || self.ball_count == 0 {
            return None;
        }
        for (try_swap, want_c) in [(false, self.cur_color), (true, self.next_color)] {
            let mut best: Option<(i32, i32, i32)> = None; // (оцінка, x_px, y_px)
            for i in 0..self.ball_count {
                if self.balls[i].color != want_c {
                    continue;
                }
                if !self.has_clear_line_of_sight(i) {
                    continue;
                }
                let (l, r) = self.color_run_around(i);
                let run_len = (r + 1 - l) as i32;
                // Високий пріоритет: готові пари (run_len >= 2 -> миттєвий вибух!) + близькість до черепа
                let bonus_w = if self.balls[i].bonus != Bonus::None { 80 } else { 0 };
                let prio = run_len * 200 + (i as i32) * 3 + bonus_w;
                let (px, py) = self.track.pos_px(self.balls[i].s);
                if best.is_none_or(|(bp, _, _)| prio > bp) {
                    best = Some((prio, px, py));
                }
            }
            if let Some((_, px, py)) = best {
                return Some((try_swap, px, py));
            }
        }
        // Резерв: стріляти у будь-яку видиму кулю ближче до голови
        for i in (0..self.ball_count).rev() {
            if self.has_clear_line_of_sight(i) {
                let (px, py) = self.track.pos_px(self.balls[i].s);
                return Some((false, px, py));
            }
        }
        None
    }
}

/* ---------- самотести (спільні для хоста й плати) ---------- */

/// Перевірити строгий інваріант ланцюга: кольори валідні, координати `s` строго зростають з кроком `>= BALL_STEP`.
pub fn check_chain_invariants(g: &Game) -> bool {
    if g.ball_count > MAX_BALLS {
        return false;
    }
    for i in 0..g.ball_count {
        let c = g.balls[i].color;
        if !(1..=g.num_colors).contains(&c) {
            return false;
        }
        if i > 0 && g.balls[i].s - g.balls[i - 1].s < BALL_STEP - 1 {
            return false;
        }
    }
    true
}

/// Повний набір детермінованих самотестів + фаззинг (приймає `&mut Game`, щоб не класти велику структуру на стек МК).
pub fn selftest_checks(g: &mut Game) -> (u32, u32) {
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

    // 1. Перевірка геометрії всіх 3 трас: межі поля, кліренс до жабки, відсутність самоперетинів!
    for k in 0..NUM_TRACKS {
        g.track.build(k);
        chk!(g.track.len > 40 && g.track.len < MAX_PTS);
        chk!(g.track.total_s > 600 * FP);
        let mut s = 0;
        while s <= g.track.total_s {
            let (px, py) = g.track.pos_px(s);
            chk!((8..=FIELD_W - 8).contains(&px));
            chk!((8..=FIELD_H - 8).contains(&py));
            // Дистанція від траси до центру жабки має бути >= 36 px
            let dx = px - g.track.frog_x;
            let dy = py - g.track.frog_y;
            chk!(dx * dx + dy * dy >= 36 * 36);
            s += 12 * FP;
        }
        // Відсутність накладання сусідніх витків: точки, далекі по дузі (> 65 px), мають бути >= 22 px у 2D
        let mut s1 = 0;
        while s1 <= g.track.total_s {
            let (x1, y1) = g.track.pos_px(s1);
            let mut s2 = s1 + 65 * FP;
            while s2 <= g.track.total_s {
                let (x2, y2) = g.track.pos_px(s2);
                let dx = x1 - x2;
                let dy = y1 - y2;
                chk!(dx * dx + dy * dy >= 22 * 22);
                s2 += 16 * FP;
            }
            s1 += 16 * FP;
        }
    }

    // 2. Тест влучання й миттєвого зникнення трійки
    g.start_new_game(1, 42);
    g.zuma_achieved = true; // зафіксувати набір без спавну з тунелю
    g.ball_count = 4;
    let base_s = 200 * FP;
    g.balls[0] = Ball { color: 1, s: base_s, vel: 0, bonus: Bonus::None };
    g.balls[1] = Ball { color: 2, s: base_s + BALL_STEP, vel: 0, bonus: Bonus::None };
    g.balls[2] = Ball { color: 2, s: base_s + 2 * BALL_STEP, vel: 0, bonus: Bonus::None };
    g.balls[3] = Ball { color: 3, s: base_s + 3 * BALL_STEP, vel: 0, bonus: Bonus::None };
    let mut ev = TickEvents::default();
    let (hx, hy) = g.track.pos_fp(g.balls[1].s);
    g.insert_bullet_at(1, hx, hy, 2, &mut ev);
    chk!(ev.popped == 3);
    chk!(g.ball_count == 2);
    chk!(g.balls[0].color == 1 && g.balls[1].color == 3);
    chk!(g.balls[1].s - g.balls[0].s > 2 * BALL_STEP); // між 1 і 3 утворився чесний розрив!
    chk!(check_chain_invariants(g));

    // 3. Тест магнітного схлопування розриву з каскадним вибухом COMBO x2!
    g.start_new_game(1, 77);
    g.zuma_achieved = true;
    g.roll_in_ticks = 0;
    g.ball_count = 6;
    let s0 = 240 * FP;
    // Ланцюг: R R G G R B -> постріл G прибирає G G G, лишає R R ... R B -> магніт -> вибух R R R (combo 2)!
    let cols = [1u8, 1, 2, 2, 1, 3];
    for (i, &c) in cols.iter().enumerate() {
        g.balls[i] = Ball {
            color: c,
            s: s0 + (i as i32) * BALL_STEP,
            vel: 0,
            bonus: Bonus::None,
        };
    }
    let mut ev0 = TickEvents::default();
    let (gx, gy) = g.track.pos_fp(g.balls[2].s);
    g.insert_bullet_at(2, gx, gy, 2, &mut ev0);
    chk!(ev0.popped == 3);
    chk!(g.ball_count == 4); // R, R, [розрив], R, B
    let mut saw_combo = false;
    for _ in 0..40 {
        let e = g.tick();
        chk!(check_chain_invariants(g));
        if e.gap_combo >= 2 {
            saw_combo = true;
            break;
        }
    }
    chk!(saw_combo);
    chk!(g.ball_count == 1 && g.balls[0].color == 3); // залишилась лише самотня B!

    // 4. Бонус Bomb підриває сусідів і зберігає строгий інваріант
    g.start_new_game(2, 123);
    g.zuma_achieved = true;
    g.ball_count = 6;
    for i in 0..6 {
        g.balls[i] = Ball {
            color: if (2..=3).contains(&i) { 4 } else { 1 + (i as u8 % 3) },
            s: 220 * FP + (i as i32) * BALL_STEP,
            vel: 0,
            bonus: if i == 2 { Bonus::Bomb } else { Bonus::None },
        };
    }
    let mut ev_b = TickEvents::default();
    let (bx, by) = g.track.pos_fp(g.balls[2].s);
    g.insert_bullet_at(2, bx, by, 4, &mut ev_b);
    chk!(ev_b.bonus_triggered == Some(Bonus::Bomb));
    chk!(ev_b.popped > 3);
    chk!(check_chain_invariants(g));

    // 5. Стрес-фаззинг: 600 тіків живої гри бота на всіх 3 рівнях з перевіркою інваріантів на кожному кроці
    for stg in 1..=3 {
        g.start_new_game(stg, 1000 + stg as u32 * 37);
        for t in 0..220 {
            if t % 9 == 0
                && let Some((need_swap, ax, ay)) = g.find_demo_shot()
            {
                if need_swap {
                    g.swap_balls();
                }
                g.aim_at(ax, ay);
                g.shoot();
            }
            let _ = g.tick();
            chk!(check_chain_invariants(g));
        }
        chk!(g.score > 0);
    }

    (run, failed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selftest_all_pass() {
        let mut g = Game::empty();
        let (run, failed) = selftest_checks(&mut g);
        assert!(run > 1000, "замало перевірок: {run}");
        assert_eq!(failed, 0, "провалено {failed} з {run} перевірок");
    }

    #[test]
    fn demo_bot_clears_all_three_tracks() {
        for stg in 1..=3 {
            let mut g = Game::new(stg, 2020 + stg as u32);
            let mut cleared = false;
            for t in 0..6000 {
                if t % 8 == 0
                    && let Some((swap, ax, ay)) = g.find_demo_shot()
                {
                    if swap {
                        g.swap_balls();
                    }
                    g.aim_at(ax, ay);
                    g.shoot();
                }
                g.tick();
                assert!(check_chain_invariants(&g));
                if g.state == State::StageClear {
                    cleared = true;
                    break;
                }
                if g.state == State::GameOver {
                    break;
                }
            }
            assert!(
                cleared,
                "бот мав пройти рівень {stg}, state={:?}, score={}",
                g.state, g.score
            );
        }
    }

    #[test]
    fn idle_player_triggers_skull_drain_and_game_over() {
        let mut g = Game::new(1, 99);
        let mut saw_drain = false;
        for _ in 0..5000 {
            g.tick();
            if g.state == State::Draining {
                saw_drain = true;
            } else if g.state == State::GameOver {
                break;
            } else {
                assert!(check_chain_invariants(&g));
            }
        }
        assert!(saw_drain, "поїзд мав увійти у фазу Draining");
        assert_eq!(g.state, State::GameOver);
        assert_eq!(g.ball_count, 0);
    }
}
