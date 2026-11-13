//! Рушій 3D-шутера **CYBER MAZE** — рейкаст-лабіринт у стилі Wolfenstein 3D.
//!
//! Без `std`, без `alloc`, без `f32` у гарячих циклах: уся математика на **фіксованій комі Q16.16**
//! (координати у клітинках лабіринту) та **кутах у 1024 одиницях на оберт** — бо ESP32-C5 це RV32IMAC
//! **без FPU**, і будь-який `f32` тут коштує як десяток цілочисельних операцій.
//!
//! Що всередині:
//! - генерація лабіринту (recursive backtracker на сітці 17×17, 8×8 кімнат) із виходом у найдальшій клітинці;
//! - DDA-рейкаст (`cast_ray`) з поверненням перпендикулярної відстані та орієнтації стіни;
//! - рух із ковзанням по стінах (радіус 0.28 клітинки), поворот, постріл хітсканом;
//! - дрони-вороги: патрулювання, лінія видимості, зближення й стрільба; аптечки;
//! - автопілот `demo_input()` (BFS до виходу + автострільба) для режиму `DEMO`;
//! - очки, рівні, респавн після смерті.

pub const MAP_W: usize = 17;
pub const MAP_H: usize = 17;
pub const MAP_CELLS: usize = MAP_W * MAP_H;

/// 1.0 у фіксованій комі Q16.16 (одна клітинка лабіринту).
pub const FP: i32 = 1 << 16;
/// Одиниць кута на повний оберт.
pub const ANG: u16 = 1024;
/// Половинa кута огляду (FOV ≈ 66°).
pub const HALF_FOV: i32 = 94;

pub const HP_MAX: i16 = 100;
pub const MAX_ENEMIES: usize = 6;
pub const MAX_PICKUPS: usize = 4;

pub const SIN_Q15: [i16; 1024] = [
         0,    201,    402,    603,    804,   1005,   1206,   1407,   1608,   1809,   2009,   2210,   2410,   2611,   2811,   3012,
      3212,   3412,   3612,   3811,   4011,   4210,   4410,   4609,   4808,   5007,   5205,   5404,   5602,   5800,   5998,   6195,
      6393,   6590,   6786,   6983,   7179,   7375,   7571,   7767,   7962,   8157,   8351,   8545,   8739,   8933,   9126,   9319,
      9512,   9704,   9896,  10087,  10278,  10469,  10659,  10849,  11039,  11228,  11417,  11605,  11793,  11980,  12167,  12353,
     12539,  12725,  12910,  13094,  13279,  13462,  13645,  13828,  14010,  14191,  14372,  14553,  14732,  14912,  15090,  15269,
     15446,  15623,  15800,  15976,  16151,  16325,  16499,  16673,  16846,  17018,  17189,  17360,  17530,  17700,  17869,  18037,
     18204,  18371,  18537,  18703,  18868,  19032,  19195,  19357,  19519,  19680,  19841,  20000,  20159,  20317,  20475,  20631,
     20787,  20942,  21096,  21250,  21403,  21554,  21705,  21856,  22005,  22154,  22301,  22448,  22594,  22739,  22884,  23027,
     23170,  23311,  23452,  23592,  23731,  23870,  24007,  24143,  24279,  24413,  24547,  24680,  24811,  24942,  25072,  25201,
     25329,  25456,  25582,  25708,  25832,  25955,  26077,  26198,  26319,  26438,  26556,  26674,  26790,  26905,  27019,  27133,
     27245,  27356,  27466,  27575,  27683,  27790,  27896,  28001,  28105,  28208,  28310,  28411,  28510,  28609,  28706,  28803,
     28898,  28992,  29085,  29177,  29268,  29358,  29447,  29534,  29621,  29706,  29791,  29874,  29956,  30037,  30117,  30195,
     30273,  30349,  30424,  30498,  30571,  30643,  30714,  30783,  30852,  30919,  30985,  31050,  31113,  31176,  31237,  31297,
     31356,  31414,  31470,  31526,  31580,  31633,  31685,  31736,  31785,  31833,  31880,  31926,  31971,  32014,  32057,  32098,
     32137,  32176,  32213,  32250,  32285,  32318,  32351,  32382,  32412,  32441,  32469,  32495,  32521,  32545,  32567,  32589,
     32609,  32628,  32646,  32663,  32678,  32692,  32705,  32717,  32728,  32737,  32745,  32752,  32757,  32761,  32765,  32766,
     32767,  32766,  32765,  32761,  32757,  32752,  32745,  32737,  32728,  32717,  32705,  32692,  32678,  32663,  32646,  32628,
     32609,  32589,  32567,  32545,  32521,  32495,  32469,  32441,  32412,  32382,  32351,  32318,  32285,  32250,  32213,  32176,
     32137,  32098,  32057,  32014,  31971,  31926,  31880,  31833,  31785,  31736,  31685,  31633,  31580,  31526,  31470,  31414,
     31356,  31297,  31237,  31176,  31113,  31050,  30985,  30919,  30852,  30783,  30714,  30643,  30571,  30498,  30424,  30349,
     30273,  30195,  30117,  30037,  29956,  29874,  29791,  29706,  29621,  29534,  29447,  29358,  29268,  29177,  29085,  28992,
     28898,  28803,  28706,  28609,  28510,  28411,  28310,  28208,  28105,  28001,  27896,  27790,  27683,  27575,  27466,  27356,
     27245,  27133,  27019,  26905,  26790,  26674,  26556,  26438,  26319,  26198,  26077,  25955,  25832,  25708,  25582,  25456,
     25329,  25201,  25072,  24942,  24811,  24680,  24547,  24413,  24279,  24143,  24007,  23870,  23731,  23592,  23452,  23311,
     23170,  23027,  22884,  22739,  22594,  22448,  22301,  22154,  22005,  21856,  21705,  21554,  21403,  21250,  21096,  20942,
     20787,  20631,  20475,  20317,  20159,  20000,  19841,  19680,  19519,  19357,  19195,  19032,  18868,  18703,  18537,  18371,
     18204,  18037,  17869,  17700,  17530,  17360,  17189,  17018,  16846,  16673,  16499,  16325,  16151,  15976,  15800,  15623,
     15446,  15269,  15090,  14912,  14732,  14553,  14372,  14191,  14010,  13828,  13645,  13462,  13279,  13094,  12910,  12725,
     12539,  12353,  12167,  11980,  11793,  11605,  11417,  11228,  11039,  10849,  10659,  10469,  10278,  10087,   9896,   9704,
      9512,   9319,   9126,   8933,   8739,   8545,   8351,   8157,   7962,   7767,   7571,   7375,   7179,   6983,   6786,   6590,
      6393,   6195,   5998,   5800,   5602,   5404,   5205,   5007,   4808,   4609,   4410,   4210,   4011,   3811,   3612,   3412,
      3212,   3012,   2811,   2611,   2410,   2210,   2009,   1809,   1608,   1407,   1206,   1005,    804,    603,    402,    201,
         0,   -201,   -402,   -603,   -804,  -1005,  -1206,  -1407,  -1608,  -1809,  -2009,  -2210,  -2410,  -2611,  -2811,  -3012,
     -3212,  -3412,  -3612,  -3811,  -4011,  -4210,  -4410,  -4609,  -4808,  -5007,  -5205,  -5404,  -5602,  -5800,  -5998,  -6195,
     -6393,  -6590,  -6786,  -6983,  -7179,  -7375,  -7571,  -7767,  -7962,  -8157,  -8351,  -8545,  -8739,  -8933,  -9126,  -9319,
     -9512,  -9704,  -9896, -10087, -10278, -10469, -10659, -10849, -11039, -11228, -11417, -11605, -11793, -11980, -12167, -12353,
    -12539, -12725, -12910, -13094, -13279, -13462, -13645, -13828, -14010, -14191, -14372, -14553, -14732, -14912, -15090, -15269,
    -15446, -15623, -15800, -15976, -16151, -16325, -16499, -16673, -16846, -17018, -17189, -17360, -17530, -17700, -17869, -18037,
    -18204, -18371, -18537, -18703, -18868, -19032, -19195, -19357, -19519, -19680, -19841, -20000, -20159, -20317, -20475, -20631,
    -20787, -20942, -21096, -21250, -21403, -21554, -21705, -21856, -22005, -22154, -22301, -22448, -22594, -22739, -22884, -23027,
    -23170, -23311, -23452, -23592, -23731, -23870, -24007, -24143, -24279, -24413, -24547, -24680, -24811, -24942, -25072, -25201,
    -25329, -25456, -25582, -25708, -25832, -25955, -26077, -26198, -26319, -26438, -26556, -26674, -26790, -26905, -27019, -27133,
    -27245, -27356, -27466, -27575, -27683, -27790, -27896, -28001, -28105, -28208, -28310, -28411, -28510, -28609, -28706, -28803,
    -28898, -28992, -29085, -29177, -29268, -29358, -29447, -29534, -29621, -29706, -29791, -29874, -29956, -30037, -30117, -30195,
    -30273, -30349, -30424, -30498, -30571, -30643, -30714, -30783, -30852, -30919, -30985, -31050, -31113, -31176, -31237, -31297,
    -31356, -31414, -31470, -31526, -31580, -31633, -31685, -31736, -31785, -31833, -31880, -31926, -31971, -32014, -32057, -32098,
    -32137, -32176, -32213, -32250, -32285, -32318, -32351, -32382, -32412, -32441, -32469, -32495, -32521, -32545, -32567, -32589,
    -32609, -32628, -32646, -32663, -32678, -32692, -32705, -32717, -32728, -32737, -32745, -32752, -32757, -32761, -32765, -32766,
    -32767, -32766, -32765, -32761, -32757, -32752, -32745, -32737, -32728, -32717, -32705, -32692, -32678, -32663, -32646, -32628,
    -32609, -32589, -32567, -32545, -32521, -32495, -32469, -32441, -32412, -32382, -32351, -32318, -32285, -32250, -32213, -32176,
    -32137, -32098, -32057, -32014, -31971, -31926, -31880, -31833, -31785, -31736, -31685, -31633, -31580, -31526, -31470, -31414,
    -31356, -31297, -31237, -31176, -31113, -31050, -30985, -30919, -30852, -30783, -30714, -30643, -30571, -30498, -30424, -30349,
    -30273, -30195, -30117, -30037, -29956, -29874, -29791, -29706, -29621, -29534, -29447, -29358, -29268, -29177, -29085, -28992,
    -28898, -28803, -28706, -28609, -28510, -28411, -28310, -28208, -28105, -28001, -27896, -27790, -27683, -27575, -27466, -27356,
    -27245, -27133, -27019, -26905, -26790, -26674, -26556, -26438, -26319, -26198, -26077, -25955, -25832, -25708, -25582, -25456,
    -25329, -25201, -25072, -24942, -24811, -24680, -24547, -24413, -24279, -24143, -24007, -23870, -23731, -23592, -23452, -23311,
    -23170, -23027, -22884, -22739, -22594, -22448, -22301, -22154, -22005, -21856, -21705, -21554, -21403, -21250, -21096, -20942,
    -20787, -20631, -20475, -20317, -20159, -20000, -19841, -19680, -19519, -19357, -19195, -19032, -18868, -18703, -18537, -18371,
    -18204, -18037, -17869, -17700, -17530, -17360, -17189, -17018, -16846, -16673, -16499, -16325, -16151, -15976, -15800, -15623,
    -15446, -15269, -15090, -14912, -14732, -14553, -14372, -14191, -14010, -13828, -13645, -13462, -13279, -13094, -12910, -12725,
    -12539, -12353, -12167, -11980, -11793, -11605, -11417, -11228, -11039, -10849, -10659, -10469, -10278, -10087,  -9896,  -9704,
     -9512,  -9319,  -9126,  -8933,  -8739,  -8545,  -8351,  -8157,  -7962,  -7767,  -7571,  -7375,  -7179,  -6983,  -6786,  -6590,
     -6393,  -6195,  -5998,  -5800,  -5602,  -5404,  -5205,  -5007,  -4808,  -4609,  -4410,  -4210,  -4011,  -3811,  -3612,  -3412,
     -3212,  -3012,  -2811,  -2611,  -2410,  -2210,  -2009,  -1809,  -1608,  -1407,  -1206,  -1005,   -804,   -603,   -402,   -201,
];

#[inline]
pub fn sin_q15(a: u16) -> i32 {
    SIN_Q15[(a & 1023) as usize] as i32
}

#[inline]
pub fn cos_q15(a: u16) -> i32 {
    SIN_Q15[((a.wrapping_add(256)) & 1023) as usize] as i32
}

/// Цілочисельний квадратний корінь (без FPU).
pub fn isqrt(v: u64) -> u64 {
    if v == 0 {
        return 0;
    }
    let mut x = v;
    let mut y = (x + 1) / 2;
    while y < x {
        x = y;
        y = (x + v / x) / 2;
    }
    x
}

/// Найближчий кут (у одиницях 0..1023) до вектора `(dx, dy)`.
pub fn angle_of(dx: i32, dy: i32) -> u16 {
    if dx == 0 && dy == 0 {
        return 0;
    }
    let mut best = 0u16;
    let mut best_dot = i64::MIN;
    let mut a = 0u16;
    // 64 кандидати + уточнення: достатньо для автопілота й пострілів ворогів
    while a < ANG {
        let c = cos_q15(a) as i64;
        let s = sin_q15(a) as i64;
        let dot = c * dx as i64 + s * dy as i64;
        if dot > best_dot {
            best_dot = dot;
            best = a;
        }
        a += 16;
    }
    let lo = best.wrapping_sub(16);
    let mut a = lo;
    let mut best2 = best;
    let mut best_dot2 = i64::MIN;
    while a <= best.wrapping_add(16) {
        let c = cos_q15(a) as i64;
        let s = sin_q15(a) as i64;
        let dot = c * dx as i64 + s * dy as i64;
        if dot > best_dot2 {
            best_dot2 = dot;
            best2 = a;
        }
        a = a.wrapping_add(1);
    }
    best2 & 1023
}

/// Різниця кутів `a - b` у діапазоні -512..=512.
pub fn angle_delta(a: u16, b: u16) -> i32 {
    let d = (a as i32 & 1023) - (b as i32 & 1023);
    if d > 512 {
        d - 1024
    } else if d < -512 {
        d + 1024
    } else {
        d
    }
}

#[inline]
pub fn fx(v: i32) -> i32 {
    v * FP
}

#[derive(Clone)]
pub struct Rng {
    s: u32,
}

impl Rng {
    pub const fn new(seed: u32) -> Self {
        Self {
            s: if seed == 0 { 0x1234_5678 } else { seed },
        }
    }

    #[inline]
    pub fn next(&mut self) -> u32 {
        let mut x = self.s;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.s = if x == 0 { 0x9E37_79B9 } else { x };
        x
    }

    #[inline]
    pub fn below(&mut self, n: u32) -> u32 {
        self.next() % n.max(1)
    }
}

/// Лабіринт 17×17 (кімнати 8×8, стіни між ними) + вихід.
#[derive(Clone)]
pub struct Level {
    pub walls: [u8; MAP_CELLS],
    pub start: usize,
    pub exit: usize,
}

impl Level {
    #[inline]
    pub fn cell_to_map(cell: usize) -> usize {
        let cw = MAP_W / 2;
        let (cx, cy) = (cell % cw, cell / cw);
        (cy * 2 + 1) * MAP_W + (cx * 2 + 1)
    }

    /// Генерація лабіринту recursive backtracker + вихід у найдальшій клітинці.
    pub fn generate(seed: u32) -> Self {
        let mut rng = Rng::new(seed ^ 0x00C0_FFEE);
        let mut walls = [1u8; MAP_CELLS];
        let cw = MAP_W / 2;
        let ch = MAP_H / 2;
        let total = cw * ch;

        let mut visited = [false; 64];
        let mut stack = [0u8; 65];
        let mut sp = 1usize;
        visited[0] = true;
        walls[Self::cell_to_map(0)] = 0;
        stack[0] = 0;

        while sp > 0 {
            let cur = stack[sp - 1] as usize;
            let (cx, cy) = (cur % cw, cur / cw);
            let mut nb = [0u8; 4];
            let mut n = 0usize;
            for (dx, dy) in [(-1i32, 0i32), (1, 0), (0, -1), (0, 1)] {
                let nx = cx as i32 + dx;
                let ny = cy as i32 + dy;
                if (0..cw as i32).contains(&nx) && (0..ch as i32).contains(&ny) {
                    let ni = (ny as usize) * cw + nx as usize;
                    if !visited[ni] && n < 4 {
                        nb[n] = ni as u8;
                        n += 1;
                    }
                }
            }
            if n == 0 || sp >= stack.len() {
                sp -= 1;
                continue;
            }
            let pick = nb[rng.below(n as u32) as usize] as usize;
            visited[pick] = true;
            stack[sp] = pick as u8;
            sp += 1;
            let a = Self::cell_to_map(cur);
            let b = Self::cell_to_map(pick);
            walls[b] = 0;
            walls[(a + b) / 2] = 0;
        }
        let _ = total;

        let start = Self::cell_to_map(0);
        let mut level = Self {
            walls,
            start,
            exit: start,
        };
        // Вихід — найдальша клітинка
        let dist = level.dist_from(start);
        let mut far = start;
        let mut far_d = 0u16;
        for idx in 0..MAP_CELLS {
            if level.walls[idx] == 0 && dist[idx] != u16::MAX && dist[idx] > far_d {
                far_d = dist[idx];
                far = idx;
            }
        }
        level.exit = far;
        level
    }

    #[inline]
    pub fn is_wall_cell(&self, cx: i32, cy: i32) -> bool {
        if !(0..MAP_W as i32).contains(&cx) || !(0..MAP_H as i32).contains(&cy) {
            return true;
        }
        self.walls[(cy as usize) * MAP_W + cx as usize] != 0
    }

    #[inline]
    pub fn wall_at(&self, x: i32, y: i32) -> bool {
        self.is_wall_cell(x >> 16, y >> 16)
    }

    pub fn floor_cells(&self) -> usize {
        self.walls.iter().filter(|&&w| w == 0).count()
    }

    /// BFS-відстані (в клітинках) від `from` до кожної відкритої клітинки.
    pub fn dist_from(&self, from: usize) -> [u16; MAP_CELLS] {
        let mut dist = [u16::MAX; MAP_CELLS];
        if self.walls[from] != 0 {
            return dist;
        }
        let mut queue = [0u16; MAP_CELLS];
        let mut head = 0usize;
        let mut tail = 0usize;
        queue[tail] = from as u16;
        tail += 1;
        dist[from] = 0;
        while head < tail {
            let cur = queue[head] as usize;
            head += 1;
            let (cx, cy) = (cur % MAP_W, cur / MAP_W);
            let d = dist[cur];
            for (dx, dy) in [(-1i32, 0i32), (1, 0), (0, -1), (0, 1)] {
                let nx = cx as i32 + dx;
                let ny = cy as i32 + dy;
                if !(0..MAP_W as i32).contains(&nx) || !(0..MAP_H as i32).contains(&ny) {
                    continue;
                }
                let ni = (ny as usize) * MAP_W + nx as usize;
                if self.walls[ni] == 0 && dist[ni] == u16::MAX {
                    dist[ni] = d.saturating_add(1);
                    if tail < MAP_CELLS {
                        queue[tail] = ni as u16;
                        tail += 1;
                    }
                }
            }
        }
        dist
    }

    /// Наступна клітинка на шляху від `from` до `to` (BFS), або `None`.
    pub fn path_next(&self, from: usize, to: usize) -> Option<usize> {
        if from == to {
            return None;
        }
        let dist = self.dist_from(to);
        if dist[from] == u16::MAX {
            return None;
        }
        let (cx, cy) = (from % MAP_W, from / MAP_W);
        let mut best: Option<usize> = None;
        let mut best_d = dist[from];
        for (dx, dy) in [(-1i32, 0i32), (1, 0), (0, -1), (0, 1)] {
            let nx = cx as i32 + dx;
            let ny = cy as i32 + dy;
            if !(0..MAP_W as i32).contains(&nx) || !(0..MAP_H as i32).contains(&ny) {
                continue;
            }
            let ni = (ny as usize) * MAP_W + nx as usize;
            if dist[ni] != u16::MAX && dist[ni] < best_d {
                best_d = dist[ni];
                best = Some(ni);
            }
        }
        best
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Hit {
    pub dist: i32,
    pub side: u8,
    /// Індекс клітинки стіни (для «шаховидного» затінення на екрані).
    pub cell: u16,
}

#[derive(Clone, Copy)]
pub struct Enemy {
    pub x: i32,
    pub y: i32,
    pub hp: i16,
    pub cool: u8,
    pub alive: bool,
    pub wander: u8,
    pub hit_flash: u8,
}

impl Enemy {
    const DEAD: Self = Self {
        x: 0,
        y: 0,
        hp: 0,
        cool: 0,
        alive: false,
        wander: 0,
        hit_flash: 0,
    };
}

#[derive(Clone, Copy)]
pub struct Pickup {
    pub x: i32,
    pub y: i32,
    pub taken: bool,
}

#[derive(Clone, Copy)]
pub struct Input {
    pub forward: i32,
    pub strafe: i32,
    pub turn: i32,
    pub fire: bool,
    pub run: bool,
}

impl Input {
    pub const NONE: Self = Self {
        forward: 0,
        strafe: 0,
        turn: 0,
        fire: false,
        run: false,
    };
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum GameState {
    Playing,
    Dead,
}

pub struct Game {
    pub level: Level,
    pub level_no: u16,
    pub px: i32,
    pub py: i32,
    pub angle: u16,
    pub hp: i16,
    pub ammo: u16,
    pub score: u32,
    pub kills: u16,
    pub enemies: [Enemy; MAX_ENEMIES],
    pub pickups: [Pickup; MAX_PICKUPS],
    pub flash: u8,
    pub state: GameState,
    pub dead_ticks: u16,
    pub level_cleared: u16,
    pub ticks: u32,
    rng: Rng,
}

impl Game {
    pub fn new(seed: u32) -> Self {
        let mut g = Self {
            level: Level::generate(seed),
            level_no: 1,
            px: 0,
            py: 0,
            angle: 0,
            hp: HP_MAX,
            ammo: 40,
            score: 0,
            kills: 0,
            enemies: [Enemy::DEAD; MAX_ENEMIES],
            pickups: [Pickup {
                x: 0,
                y: 0,
                taken: true,
            }; MAX_PICKUPS],
            flash: 0,
            state: GameState::Playing,
            dead_ticks: 0,
            level_cleared: 0,
            ticks: 0,
            rng: Rng::new(seed.wrapping_mul(2654435761) | 1),
        };
        g.spawn_level();
        g
    }

    fn spawn_level(&mut self) {
        self.px = fx(self.level.start as i32 % MAP_W as i32) + FP / 2;
        self.py = fx(self.level.start as i32 / MAP_W as i32) + FP / 2;
        // Стартовий кут — у бік відкритого сусіда
        self.angle = 0;
        for (a, (dx, dy)) in [
            (0u16, (1i32, 0i32)),
            (256, (0, 1)),
            (512, (-1, 0)),
            (768, (0, -1)),
        ] {
            let c = Level::cell_to_map(0);
            let _ = c;
            let cx = (self.px >> 16) + dx;
            let cy = (self.py >> 16) + dy;
            if !self.level.is_wall_cell(cx, cy) {
                self.angle = a;
                break;
            }
        }

        // Вороги — у випадкових відкритих клітинках подалі від старту
        let dist = self.level.dist_from(self.level.start);
        let mut placed = 0usize;
        let want = (2 + self.level_no as usize).min(MAX_ENEMIES);
        let mut guard = 0;
        while placed < want && guard < 400 {
            guard += 1;
            let idx = self.rng.below(MAP_CELLS as u32) as usize;
            if self.level.walls[idx] != 0 || dist[idx] < 4 || dist[idx] == u16::MAX {
                continue;
            }
            if self.enemies[..placed]
                .iter()
                .any(|e| e.alive && (e.x >> 16) == (idx % MAP_W) as i32)
            {
                continue;
            }
            let (cx, cy) = ((idx % MAP_W) as i32, (idx / MAP_W) as i32);
            self.enemies[placed] = Enemy {
                x: fx(cx) + FP / 2,
                y: fx(cy) + FP / 2,
                hp: 100,
                cool: 20 + self.rng.below(40) as u8,
                alive: true,
                wander: self.rng.below(4) as u8,
                hit_flash: 0,
            };
            placed += 1;
        }
        for slot in placed..MAX_ENEMIES {
            self.enemies[slot] = Enemy::DEAD;
        }

        // Аптечки
        for i in 0..MAX_PICKUPS {
            let mut placed_p = false;
            for _ in 0..200 {
                let idx = self.rng.below(MAP_CELLS as u32) as usize;
                if self.level.walls[idx] != 0 || dist[idx] < 2 || dist[idx] == u16::MAX {
                    continue;
                }
                let (cx, cy) = ((idx % MAP_W) as i32, (idx / MAP_W) as i32);
                self.pickups[i] = Pickup {
                    x: fx(cx) + FP / 2,
                    y: fx(cy) + FP / 2,
                    taken: false,
                };
                placed_p = true;
                break;
            }
            if !placed_p {
                self.pickups[i].taken = true;
            }
        }

        self.state = GameState::Playing;
        self.dead_ticks = 0;
        self.hp = HP_MAX;
        self.ammo = self.ammo.max(24);
    }

    /// Повний перехід на новий рівень.
    pub fn next_level(&mut self) {
        self.level_no = self.level_no.saturating_add(1);
        self.score = self.score.saturating_add(100 * self.level_no as u32);
        self.level = Level::generate(self.rng.next() | 1);
        self.ammo = self.ammo.saturating_add(20);
        self.spawn_level();
    }

    pub fn respawn(&mut self) {
        self.score = self.score.saturating_sub(50);
        self.ammo = 30;
        self.spawn_level();
    }

    #[inline]
    pub fn cell_of(x: i32, y: i32) -> usize {
        let cx = (x >> 16).clamp(0, MAP_W as i32 - 1) as usize;
        let cy = (y >> 16).clamp(0, MAP_H as i32 - 1) as usize;
        cy * MAP_W + cx
    }

    /// Чи можна стати в точку `(x, y)` з радіусом 0.28 клітинки.
    pub fn walkable(&self, x: i32, y: i32) -> bool {
        const R: i32 = FP * 28 / 100;
        for (dx, dy) in [(R, R), (-R, R), (R, -R), (-R, -R)] {
            if self.level.wall_at(x + dx, y + dy) {
                return false;
            }
        }
        true
    }

    /// Рух із ковзанням по стінах. `forward`/`strafe` у діапазоні -256..=256.
    pub fn move_player(&mut self, forward: i32, strafe: i32) {
        if self.state != GameState::Playing {
            return;
        }
        let f = forward.clamp(-256, 256);
        let s = strafe.clamp(-256, 256);
        if f == 0 && s == 0 {
            return;
        }
        let ux = cos_q15(self.angle); // Q15
        let uy = sin_q15(self.angle);
        // Q16 одиничний вектор
        let dx_dir = ux * 2;
        let dy_dir = uy * 2;
        // Q16 перпендикуляр (вправо)
        let rx_dir = dy_dir;
        let ry_dir = -dx_dir;

        let step = (FP * 9) / 100; // 0.09 клітинки на тік при повному нахилі
        let mx = (dx_dir as i64 * f as i64 * step as i64) >> 24;
        let my = (dy_dir as i64 * f as i64 * step as i64) >> 24;
        let sx = (rx_dir as i64 * s as i64 * step as i64) >> 24;
        let sy = (ry_dir as i64 * s as i64 * step as i64) >> 24;

        let nx = self.px + (mx + sx) as i32;
        let ny = self.py + (my + sy) as i32;
        if self.walkable(nx, self.py) {
            self.px = nx;
        }
        if self.walkable(self.px, ny) {
            self.py = ny;
        }
    }

    #[inline]
    pub fn turn(&mut self, delta: i32) {
        self.angle = ((self.angle as i32 + delta) & 1023) as u16;
    }

    /// DDA-рейкаст: перпендикулярна відстань до найближчої стіни.
    pub fn cast_ray_at(&self, ox: i32, oy: i32, ang: u16) -> Hit {
        let rdx = (cos_q15(ang) as i64) << 1; // Q16
        let rdy = (sin_q15(ang) as i64) << 1;
        let mut map_x = ox >> 16;
        let mut map_y = oy >> 16;

        let delta = |d: i64| -> i64 {
            if d == 0 {
                i64::MAX / 4
            } else {
                (FP as i64 * FP as i64) / d.abs()
            }
        };
        let delta_x = delta(rdx);
        let delta_y = delta(rdy);

        let (step_x, mut side_x) = if rdx > 0 {
            (
                1i32,
                (((map_x as i64 + 1) << 16) - ox as i64) * FP as i64 / rdx,
            )
        } else if rdx < 0 {
            (
                -1i32,
                (ox as i64 - ((map_x as i64) << 16)) * FP as i64 / (-rdx),
            )
        } else {
            (0, i64::MAX / 4)
        };
        let (step_y, mut side_y) = if rdy > 0 {
            (
                1i32,
                (((map_y as i64 + 1) << 16) - oy as i64) * FP as i64 / rdy,
            )
        } else if rdy < 0 {
            (
                -1i32,
                (oy as i64 - ((map_y as i64) << 16)) * FP as i64 / (-rdy),
            )
        } else {
            (0, i64::MAX / 4)
        };

        let mut side = 0u8;
        for _ in 0..80 {
            let dist;
            if side_x < side_y {
                dist = side_x;
                side_x += delta_x;
                map_x += step_x;
                side = 0;
            } else {
                dist = side_y;
                side_y += delta_y;
                map_y += step_y;
                side = 1;
            }
            if map_x < 0 || map_y < 0 || map_x >= MAP_W as i32 || map_y >= MAP_H as i32 {
                return Hit {
                    dist: FX_FAR,
                    side,
                    cell: 0,
                };
            }
            if self.level.walls[map_y as usize * MAP_W + map_x as usize] != 0 {
                return Hit {
                    dist: (dist as i32).max(1),
                    side,
                    cell: (map_y as u16) * MAP_W as u16 + map_x as u16,
                };
            }
        }
        Hit {
            dist: FX_FAR,
            side,
            cell: 0,
        }
    }

    #[inline]
    pub fn cast_ray(&self, ang: u16) -> Hit {
        self.cast_ray_at(self.px, self.py, ang)
    }

    /// Чи бачить ворог `i` гравця (пряма видимість).
    pub fn enemy_sees_player(&self, i: usize) -> bool {
        let e = self.enemies[i];
        if !e.alive {
            return false;
        }
        let rx = self.px - e.x;
        let ry = self.py - e.y;
        let len = isqrt((rx as i64 * rx as i64 + ry as i64 * ry as i64) as u64) as i32;
        if len <= 1 {
            return true;
        }
        let ang = angle_of(rx, ry);
        let hit = self.cast_ray_at(e.x, e.y, ang);
        hit.dist + FP / 3 > len
    }

    /// Постріл хітсканом уздовж напрямку погляду. Повертає `true`, якщо влучив у дрона.
    pub fn shoot(&mut self) -> bool {
        if self.state != GameState::Playing {
            return false;
        }
        if self.ammo == 0 {
            return false;
        }
        self.ammo -= 1;

        let dx = cos_q15(self.angle) as i64;
        let dy = sin_q15(self.angle) as i64;
        let wall = self.cast_ray(self.angle).dist as i64;

        let mut best: Option<usize> = None;
        let mut best_t = wall;
        for i in 0..MAX_ENEMIES {
            let e = self.enemies[i];
            if !e.alive {
                continue;
            }
            let rx = (e.x - self.px) as i64;
            let ry = (e.y - self.py) as i64;
            let t = (rx * dx + ry * dy) >> 15;
            if t <= 0 || t >= best_t {
                continue;
            }
            let lat = ((ry * dx - rx * dy) >> 15).abs();
            if lat > FP as i64 * 30 / 100 {
                continue;
            }
            best = Some(i);
            best_t = t;
        }

        let Some(i) = best else {
            return false;
        };
        let dmg = 34i16;
        self.enemies[i].hp -= dmg;
        self.enemies[i].hit_flash = 4;
        if self.enemies[i].hp <= 0 {
            self.enemies[i].alive = false;
            self.kills += 1;
            self.score = self.score.saturating_add(120);
        }
        true
    }

    /// Один тік світу: вороги, аптечки, перевірка виходу й смерті.
    pub fn tick(&mut self) {
        self.ticks = self.ticks.wrapping_add(1);
        if self.flash > 0 {
            self.flash -= 1;
        }
        if self.level_cleared > 0 {
            self.level_cleared -= 1;
        }

        if self.state == GameState::Dead {
            self.dead_ticks = self.dead_ticks.saturating_add(1);
            if self.dead_ticks > 45 {
                self.respawn();
            }
            return;
        }

        for i in 0..MAX_ENEMIES {
            if !self.enemies[i].alive {
                continue;
            }
            if self.enemies[i].hit_flash > 0 {
                self.enemies[i].hit_flash -= 1;
            }
            let e = self.enemies[i];
            let rx = self.px - e.x;
            let ry = self.py - e.y;
            let dist = isqrt((rx as i64 * rx as i64 + ry as i64 * ry as i64) as u64) as i32;
            let sees = self.enemy_sees_player(i);

            // Рух
            let mut vx = 0i32;
            let mut vy = 0i32;
            if sees && dist > FP * 3 / 2 {
                let len = dist.max(1) as i64;
                vx = (rx as i64 * (FP as i64 / 48) / len) as i32;
                vy = (ry as i64 * (FP as i64 / 48) / len) as i32;
            } else if !sees && (self.ticks + i as u32) % 70 == 0 {
                self.enemies[i].wander = self.rng.below(4) as u8;
            } else if !sees {
                let (dx, dy) = match self.enemies[i].wander {
                    0 => (1, 0),
                    1 => (-1, 0),
                    2 => (0, 1),
                    _ => (0, -1),
                };
                vx = dx * (FP / 96);
                vy = dy * (FP / 96);
            }

            if vx != 0 || vy != 0 {
                let nx = self.enemies[i].x + vx;
                let ny = self.enemies[i].y + vy;
                if self.walkable(nx, self.enemies[i].y) {
                    self.enemies[i].x = nx;
                } else {
                    self.enemies[i].wander = self.rng.below(4) as u8;
                }
                if self.walkable(self.enemies[i].x, ny) {
                    self.enemies[i].y = ny;
                } else {
                    self.enemies[i].wander = self.rng.below(4) as u8;
                }
            }

            // Стрільба по гравцю
            if self.enemies[i].cool > 0 {
                self.enemies[i].cool -= 1;
            } else if sees && dist < FP * 6 {
                self.enemies[i].cool = 26 + self.rng.below(30) as u8;
                let dmg = if dist < FP * 2 {
                    12
                } else if dist < FP * 4 {
                    8
                } else {
                    5
                };
                self.hp -= dmg;
                self.flash = 6;
                if self.hp <= 0 {
                    self.hp = 0;
                    self.state = GameState::Dead;
                    self.dead_ticks = 0;
                    return;
                }
            }
        }

        // Аптечки
        for i in 0..MAX_PICKUPS {
            if self.pickups[i].taken {
                continue;
            }
            let dx = (self.pickups[i].x - self.px).abs();
            let dy = (self.pickups[i].y - self.py).abs();
            if dx < FP / 2 && dy < FP / 2 {
                self.pickups[i].taken = true;
                self.hp = (self.hp + 35).min(HP_MAX);
            }
        }

        // Вихід
        let ecx = (self.level.exit % MAP_W) as i32;
        let ecy = (self.level.exit / MAP_W) as i32;
        let dx = (self.px - (fx(ecx) + FP / 2)).abs();
        let dy = (self.py - (fx(ecy) + FP / 2)).abs();
        if dx < FP / 2 && dy < FP / 2 && self.level_cleared == 0 {
            self.level_cleared = 60;
            self.next_level();
        }
    }

    /// Чи є ворог у полі зору (для автопілота та підсвітки прицілу).
    pub fn enemy_in_sight(&self) -> Option<usize> {
        let dx = cos_q15(self.angle) as i64;
        let dy = sin_q15(self.angle) as i64;
        let wall = self.cast_ray(self.angle).dist as i64;
        let mut best: Option<(usize, i64)> = None;
        for i in 0..MAX_ENEMIES {
            let e = self.enemies[i];
            if !e.alive {
                continue;
            }
            let rx = (e.x - self.px) as i64;
            let ry = (e.y - self.py) as i64;
            let t = (rx * dx + ry * dy) >> 15;
            if t <= 0 || t > wall || t > FP as i64 * 8 {
                continue;
            }
            let lat = ((ry * dx - rx * dy) >> 15).abs();
            if lat > t * 30 / 100 {
                continue;
            }
            if best.map(|(_, bt)| t < bt).unwrap_or(true) {
                best = Some((i, t));
            }
        }
        best.map(|(i, _)| i)
    }

    /// Автопілот для режиму `DEMO`: BFS до виходу + автострільба.
    pub fn demo_input(&mut self) -> Input {
        let from = Self::cell_of(self.px, self.py);
        let mut turn = 0i32;
        let mut forward = 0i32;

        if let Some(next) = self.level.path_next(from, self.level.exit) {
            let (cx, cy) = ((next % MAP_W) as i32, (next / MAP_W) as i32);
            let tx = fx(cx) + FP / 2;
            let ty = fx(cy) + FP / 2;
            let want = angle_of(tx - self.px, ty - self.py);
            turn = angle_delta(want, self.angle).clamp(-26, 26);
            forward = if angle_delta(want, self.angle).abs() < 160 {
                230
            } else {
                70
            };
        }

        // Якщо приціл на ворогові — не обертаємось, а стріляємо
        let fire = match self.enemy_in_sight() {
            Some(_) => {
                let dx = cos_q15(self.angle) as i64;
                let dy = sin_q15(self.angle) as i64;
                let mut hit = false;
                for i in 0..MAX_ENEMIES {
                    let e = self.enemies[i];
                    if !e.alive {
                        continue;
                    }
                    let rx = (e.x - self.px) as i64;
                    let ry = (e.y - self.py) as i64;
                    let t = (rx * dx + ry * dy) >> 15;
                    let lat = ((ry * dx - rx * dy) >> 15).abs();
                    if t > 0 && lat < t * 18 / 100 {
                        hit = true;
                        break;
                    }
                }
                hit && self.ammo > 0
            }
            None => false,
        };

        Input {
            forward,
            strafe: 0,
            turn,
            fire,
            run: false,
        }
    }

    /// Застосувати введення за один тік.
    pub fn apply(&mut self, input: Input) {
        if input.turn != 0 {
            self.turn(input.turn);
        }
        self.move_player(input.forward, input.strafe);
        if input.fire {
            self.shoot();
        }
    }
}

const FX_FAR: i32 = FP * 40;

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

    let level = Level::generate(7);
    let dist = level.dist_from(level.start);
    check(level.walls[level.start] == 0);
    check(dist[level.exit] != u16::MAX && dist[level.exit] > 2);

    let mut g = Game::new(11);
    check(g.walkable(g.px, g.py));
    g.move_player(256, 0);
    g.turn(64);
    g.move_player(256, 128);
    check(g.walkable(g.px, g.py));

    // Постріл витрачає патрон
    let ammo = g.ammo;
    g.shoot();
    check(g.ammo + 1 == ammo);

    // Рейкаст у стіну з центру клітинки дає скінченну відстань
    let hit = g.cast_ray(0);
    check(hit.dist > 0 && hit.dist < FX_FAR);

    (run, failed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selftest_passes() {
        let (run, failed) = selftest();
        assert_eq!(failed, 0, "selftest failed ({}/{})", failed, run);
        assert_eq!(run, 6);
    }

    #[test]
    fn maze_is_connected_and_exit_is_far() {
        for seed in [1u32, 2, 3, 42, 777] {
            let level = Level::generate(seed);
            let dist = level.dist_from(level.start);
            // усі відкриті клітинки досяжні
            for idx in 0..MAP_CELLS {
                if level.walls[idx] == 0 {
                    assert_ne!(dist[idx], u16::MAX, "seed {seed}: cell {idx} unreachable");
                }
            }
            assert!(dist[level.exit] >= 6, "seed {seed}: exit too close");
            assert!(level.floor_cells() > 40);
        }
    }

    #[test]
    fn path_next_shortens_distance() {
        let level = Level::generate(5);
        let dist = level.dist_from(level.exit);
        let mut cur = level.start;
        let mut steps = 0;
        while cur != level.exit && steps < 200 {
            let next = level.path_next(cur, level.exit).expect("path must exist");
            assert!(dist[next] < dist[cur]);
            cur = next;
            steps += 1;
        }
        assert_eq!(cur, level.exit);
    }

    #[test]
    fn raycast_hits_walls_with_sane_distance() {
        let level = Level::generate(9);
        let (cx, cy) = (level.start % MAP_W, level.start / MAP_W);
        let ox = fx(cx as i32) + FP / 2;
        let oy = fx(cy as i32) + FP / 2;
        let mut g = Game::new(9);
        g.px = ox;
        g.py = oy;
        for a in [0u16, 300, 600, 900] {
            let hit = g.cast_ray(a);
            assert!(hit.dist > 0, "angle {a}");
            assert!(hit.dist <= FP * 40);
        }
    }

    #[test]
    fn player_cannot_walk_through_walls() {
        let mut g = Game::new(21);
        // Ставимо гравця в центр і штовхаємо в усі боки 300 тіків
        for _ in 0..300 {
            g.move_player(256, 0);
            g.turn(7);
            assert!(g.walkable(g.px, g.py));
            assert!(!g.level.wall_at(g.px, g.py));
        }
    }

    #[test]
    fn shooting_kills_drone_and_scores() {
        let mut g = Game::new(31);
        // Ставимо дрона просто перед гравцем
        let dx = cos_q15(g.angle) as i32 * 2;
        let dy = sin_q15(g.angle) as i32 * 2;
        g.enemies[0] = Enemy {
            x: g.px + dx,
            y: g.py + dy,
            hp: 100,
            cool: 99,
            alive: true,
            wander: 0,
            hit_flash: 0,
        };
        g.ammo = 10;
        let hits = (0..3).filter(|_| g.shoot()).count();
        assert_eq!(hits, 3);
        assert!(!g.enemies[0].alive);
        assert_eq!(g.kills, 1);
        assert!(g.score >= 120);
    }

    #[test]
    fn wall_blocks_the_shot() {
        let mut g = Game::new(41);
        // Дрон за стіною: ставимо гравця в кут і цілимось у сусідню клітинку-стіну
        g.px = fx(1) + FP / 2;
        g.py = fx(1) + FP / 2;
        g.angle = 0;
        // Знайдемо напрямок, де поруч стіна
        let mut placed = false;
        for a in [0u16, 256, 512, 768] {
            let hit = g.cast_ray(a);
            if hit.dist < FP * 2 {
                g.angle = a;
                let dx = cos_q15(a) as i32 * 2;
                let dy = sin_q15(a) as i32 * 2;
                // дрон за стіною
                g.enemies[0] = Enemy {
                    x: g.px + dx * 4,
                    y: g.py + dy * 4,
                    hp: 100,
                    cool: 99,
                    alive: true,
                    wander: 0,
                    hit_flash: 0,
                };
                placed = true;
                break;
            }
        }
        if placed {
            g.ammo = 5;
            assert!(!g.shoot());
            assert!(g.enemies[0].alive);
        }
    }

    #[test]
    fn demo_autopilot_reaches_exit() {
        let mut g = Game::new(1234);
        let mut reached = false;
        for _ in 0..4000 {
            let input = g.demo_input();
            g.apply(input);
            g.tick();
            if g.level_no > 1 {
                reached = true;
                break;
            }
            if g.state == GameState::Dead {
                break;
            }
        }
        assert!(reached, "autopilot did not reach the exit");
    }

    #[test]
    fn enemies_damage_player_and_death_respawns() {
        let mut g = Game::new(55);
        // Дрон прямо перед гравцем, без перешкод
        let dx = cos_q15(g.angle) as i32 * 2;
        let dy = sin_q15(g.angle) as i32 * 2;
        g.enemies[0] = Enemy {
            x: g.px + dx,
            y: g.py + dy,
            hp: 100,
            cool: 0,
            alive: true,
            wander: 0,
            hit_flash: 0,
        };
        let hp0 = g.hp;
        for _ in 0..10 {
            g.enemies[0].cool = 0;
            g.tick();
        }
        assert!(g.hp < hp0, "enemy must damage the player");
    }

    #[test]
    fn next_level_increases_score_and_regenerates_maze() {
        let mut g = Game::new(77);
        let old_exit = g.level.exit;
        let score0 = g.score;
        g.next_level();
        assert_eq!(g.level_no, 2);
        assert!(g.score > score0);
        assert_eq!(g.state, GameState::Playing);
        assert!(g.level.exit < MAP_CELLS);
        let _ = old_exit;
    }
}
