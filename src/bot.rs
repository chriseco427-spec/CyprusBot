//! Decision logic: one call per observation, returns the action as JSON.
use crate::nav::Nav;
use crate::world::World;
use serde_json::{json, Map, Value};
use std::collections::HashMap;
use std::sync::Arc;
use std::f32::consts::{PI, TAU};

fn f(v: &Value) -> f32 {
    v.as_f64().unwrap_or(0.0) as f32
}
fn v3(v: &Value) -> [f32; 3] {
    [f(&v[0]), f(&v[1]), f(&v[2])]
}
fn wrap(a: f32) -> f32 {
    let mut a = a % TAU;
    if a > PI {
        a -= TAU;
    }
    if a < -PI {
        a += TAU;
    }
    a
}
fn dxy(a: &[f32], b: &[f32]) -> f32 {
    (a[0] - b[0]).hypot(a[1] - b[1])
}
fn d3(a: [f32; 3], b: [f32; 3]) -> f32 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
}
fn aim_to(from: [f32; 3], to: [f32; 3]) -> [f32; 2] {
    let (dx, dy, dz) = (to[0] - from[0], to[1] - from[1], to[2] - from[2]);
    [dy.atan2(dx), dz.atan2(dx.hypot(dy))]
}
fn norm2(v: [f32; 2]) -> [f32; 2] {
    let l = v[0].hypot(v[1]);
    if l < 1e-6 {
        [0.0, 0.0]
    } else {
        [v[0] / l, v[1] / l]
    }
}

#[derive(Clone, Default)]
struct Wpn {
    kind: String,
    cooldown: f32,
    ammo: f32,
    reload: f32,
    charge: f32,
    speed: f32,
    range: f32,
    mag: f32,
    full_charge: f32,
    energy: f32,
}

struct Me {
    pos: [f32; 3],
    aim: [f32; 2],
    hp: f32,
    max_hp: f32,
    energy: f32,
    max_energy: f32,
    cubes: f32,
    grounded: bool,
    in_zone: bool,
    walk: f32,
    muzzle_h: f32,
    eye_h: f32,
    height: f32,
    wpn: [Option<Wpn>; 2],
}

#[derive(Clone)]
struct Enemy {
    id: i64,
    pos: [f32; 3],
    vel: [f32; 3],
    aim: [f32; 2],
    hp: f32,
    max_hp: f32,
    cubes: f32,
    weapons: Vec<String>,
    seen: f32,
    hurt_me_t: f32,
    hostile_until: f32,
    charging: bool,
    facing_since: f32,
    hit_by_me: bool,
}

impl Enemy {
    fn height(&self) -> f32 {
        9.0 * (1.0 + 0.05 * self.cubes)
    }
    fn center(&self) -> [f32; 3] {
        [self.pos[0], self.pos[1], self.pos[2] + 0.5 * self.height()]
    }
    fn eye(&self) -> [f32; 3] {
        [self.pos[0], self.pos[1], self.pos[2] + 8.0 * (1.0 + 0.05 * self.cubes)]
    }
    fn has(&self, k: &str) -> bool {
        self.weapons.iter().any(|w| w == k)
    }
}

#[derive(Clone)]
struct Item {
    kind: String,
    pos: [f32; 3],
    seen: f32,
}

struct Zone {
    c: [f32; 2],
    r: f32,
    nc: [f32; 2],
    nr: f32,
    ss: f32,
}

enum Mv {
    Goto((f32, f32), bool),
    Direct([f32; 2], bool),
    Hold,
}

/// Behaviour switches. Defaults are what ships; the simulator overrides them per bot.
#[derive(Clone)]
pub struct Cfg {
    /// 0 = detect, 1 = always treat the match as house-squad practice, 2 = always free-for-all.
    pub mode: u8,
    /// 0 = shoot anything, 1 = only mechs that threaten us, 2 = also unaware mechs when safe.
    pub policy: u8,
    /// range, retreat hp, retreat hp when outnumbered, retreat in squad mode,
    /// hostile bonus, hp weight, strafe base, strafe spread
    pub tune: [f32; 16],
}

impl Cfg {
    /// range, retreat hp, retreat hp when outnumbered, retreat in squad mode, hostile bonus,
    /// hp weight, strafe base, strafe spread, power ratio needed to accept a duel when cautious,
    /// seconds before assuming practice, cornered distance, the same duel ratio while the match type is unknown
    pub const TUNE: [f32; 16] = [55.0, 0.40, 0.75, 0.0, 40.0, 0.08, 0.45, 0.5, 1.3, 8.0, 40.0, 1.3, 0.0, 0.0, 0.0, 0.0];
    pub fn from_env() -> Cfg {
        let mut tune = Self::TUNE;
        if let Ok(e) = std::env::var("MB_TUNE") {
            for (i, x) in e.split(',').enumerate().take(16) {
                tune[i] = x.parse().unwrap_or(tune[i]);
            }
        }
        let num = |k: &str| std::env::var(k).ok().and_then(|v| v.parse().ok()).unwrap_or(0u8);
        Cfg { mode: num("MB_MODE"), policy: num("MB_POLICY"), tune }
    }
}

pub struct Bot {
    w: Arc<World>,
    nav: Nav,
    rng: u64,
    pickup_radius: f32,
    enemies: HashMap<i64, Enemy>,
    pickups: HashMap<i64, Item>,
    crates: HashMap<i64, Item>,
    cubes: HashMap<i64, Item>,
    path: Vec<(f32, f32)>,
    path_i: usize,
    path_goal: (f32, f32),
    path_t: f32,
    strafe_dir: f32,
    strafe_until: f32,
    last_hurt_t: f32,
    hurt_bearing: f32,
    retreating: bool,
    cover: Option<(f32, f32)>,
    cover_t: f32,
    stuck_pos: [f32; 3],
    stuck_t: f32,
    unstick_until: f32,
    unstick_dir: f32,
    wander: Option<(f32, f32)>,
    wander_t: f32,
    last_aim: [f32; 2],
    climb_until: f32,
    climb_goal: (f32, f32),
    blacklist: HashMap<i64, f32>,
    policy: u8,
    tune: [f32; 16],
    kills: i64,
    ghost_t: f32,
    t0: f32,
    /// Drones seen this match: last position and when.
    npc_mem: HashMap<i64, ([f32; 3], f32)>,
    npc_hurt_t: f32,
    /// Recent (time, position, velocity) per enemy, to judge how predictable its movement is.
    hist: HashMap<i64, Vec<(f32, [f32; 3], [f32; 3])>>,
    foreign_since: f32,
    i_hit: Vec<i64>,
    attackers_seen: Vec<i64>,
    hide: Option<(f32, f32)>,
    hide_t: f32,
    hide_arrived: f32,
    pub debug: bool,
    pub dbg: String,
    alive0: i64,
    cfg_mode: u8,
    /// 0 = not known yet, 1 = house-squad practice, 2 = free-for-all.
    mode: u8,
}

impl Bot {
    pub fn new(hello: &Value) -> Bot {
        Self::with_cfg(hello, Cfg::from_env(), None)
    }

    pub fn load_world(hello: &Value) -> World {
        let dir = hello["map"]["dir"].as_str().unwrap_or("/arena").to_string();
        let mut wf = "v2/world.json".to_string();
        let mut tf = "v2/terrain.i16".to_string();
        if let Some(files) = hello["map"]["files"].as_array() {
            for x in files {
                if let Some(s) = x.as_str() {
                    if s.ends_with("world.json") {
                        wf = s.to_string();
                    } else if s.ends_with(".i16") {
                        tf = s.to_string();
                    }
                }
            }
        }
        match World::load(&dir, &wf, &tf) {
            Ok(w) => w,
            Err(e) => {
                eprintln!("map load failed ({}), using flat world", e);
                World::flat()
            }
        }
    }

    pub fn with_cfg(hello: &Value, cfg: Cfg, world: Option<Arc<World>>) -> Bot {
        let w = match world {
            Some(w) => w,
            None => Arc::new(Self::load_world(hello)),
        };
        let nav = Nav::new(&w);
        let seed = hello["seed"].as_u64().unwrap_or(1) | 1;
        let pr = hello["config"]["loot"]["pickup_radius"].as_f64().unwrap_or(4.0) as f32;
        Bot {
            w,
            nav,
            rng: seed.wrapping_mul(0x9E3779B97F4A7C15) | 1,
            pickup_radius: pr,
            enemies: HashMap::new(),
            pickups: HashMap::new(),
            crates: HashMap::new(),
            cubes: HashMap::new(),
            path: Vec::new(),
            path_i: 0,
            path_goal: (0.0, 0.0),
            path_t: -100.0,
            strafe_dir: 1.0,
            strafe_until: 0.0,
            last_hurt_t: -100.0,
            hurt_bearing: 0.0,
            retreating: false,
            cover: None,
            cover_t: -100.0,
            stuck_pos: [0.0; 3],
            stuck_t: 0.0,
            unstick_until: -1.0,
            unstick_dir: 0.0,
            wander: None,
            wander_t: -100.0,
            last_aim: [0.0, 0.0],
            climb_until: -1.0,
            climb_goal: (0.0, 0.0),
            blacklist: HashMap::new(),
            policy: cfg.policy,
            tune: cfg.tune,
            cfg_mode: cfg.mode,
            mode: if cfg.mode == 0 { 0 } else { cfg.mode },
            kills: 0,
            ghost_t: -100.0,
            t0: -1.0,
            npc_mem: HashMap::new(),
            npc_hurt_t: -100.0,
            hist: HashMap::new(),
            foreign_since: -1.0,
            i_hit: Vec::new(),
            attackers_seen: Vec::new(),
            hide: None,
            hide_t: -100.0,
            hide_arrived: -1.0,
            debug: false,
            dbg: String::new(),
            alive0: -1,
        }
    }

    fn rand(&mut self) -> f32 {
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 7;
        self.rng ^= self.rng << 17;
        ((self.rng >> 40) as f32) / ((1u64 << 24) as f32)
    }

    fn parse_me(s: &Value) -> Me {
        let st = &s["stats"];
        let g = |k: &str, d: f32| st[k].as_f64().map(|x| x as f32).unwrap_or(d);
        let mut wpn: [Option<Wpn>; 2] = [None, None];
        for i in 0..2 {
            let w = &s["weapons"][i];
            if w.is_object() {
                let ws = &w["stats"];
                wpn[i] = Some(Wpn {
                    kind: w["kind"].as_str().unwrap_or("").to_string(),
                    cooldown: f(&w["cooldown"]),
                    ammo: w["ammo"].as_f64().unwrap_or(99.0) as f32,
                    reload: f(&w["reload"]),
                    charge: f(&w["charge"]),
                    speed: ws["speed"].as_f64().unwrap_or(252.0) as f32,
                    range: ws["range"].as_f64().unwrap_or(200.0) as f32,
                    mag: ws["mag"].as_f64().unwrap_or(12.0) as f32,
                    full_charge: ws["charge"].as_f64().unwrap_or(0.75) as f32,
                    energy: ws["energy"].as_f64().unwrap_or(3.0) as f32,
                });
            }
        }
        let aim = &s["aim"];
        Me {
            pos: v3(&s["pos"]),
            aim: [f(&aim[0]), f(&aim[1])],
            hp: f(&s["hp"]),
            max_hp: f(&s["max_hp"]).max(1.0),
            energy: f(&s["energy"]),
            max_energy: s["max_energy"].as_f64().unwrap_or(100.0) as f32,
            cubes: f(&s["cubes"]),
            grounded: s["grounded"].as_bool().unwrap_or(true),
            in_zone: s["in_zone"].as_bool().unwrap_or(true),
            walk: g("walk_speed", 11.7),
            muzzle_h: g("muzzle_height", 7.0),
            eye_h: g("eye_height", 8.0),
            height: g("height", 9.0),
            wpn,
        }
    }

    fn remember(map: &mut HashMap<i64, Item>, list: &Value, t: f32) {
        if let Some(a) = list.as_array() {
            for it in a {
                if let Some(id) = it["id"].as_i64() {
                    map.insert(id, Item { kind: it["kind"].as_str().unwrap_or("").to_string(), pos: v3(&it["pos"]), seen: t });
                }
            }
        }
    }

    fn forget(&mut self, me: &Me, t: f32) {
        let eye = [me.pos[0], me.pos[1], me.pos[2] + me.eye_h];
        let w = &self.w;
        let gone = |it: &Item| it.seen < t && dxy(&it.pos, &me.pos) < 38.0 && w.los(eye, [it.pos[0], it.pos[1], it.pos[2] + 1.0]);
        self.pickups.retain(|_, it| !gone(it));
        self.crates.retain(|_, it| !gone(it));
        self.cubes.retain(|_, it| !gone(it) && t - it.seen < 40.0);
    }

    /// Nearest standable spot hidden from every threat's eyes.
    fn find_cover(&self, me: &Me, threats: &[Enemy], z: &Zone, strict: bool, radius: f32, step: f32) -> Option<(f32, f32)> {
        let w = &self.w;
        let mut best: Option<(f32, (f32, f32))> = None;
        let mut threats: Vec<&Enemy> = threats.iter().filter(|e| dxy(&e.pos, &me.pos) < 260.0).collect();
        threats.sort_by(|a, b| dxy(&a.pos, &me.pos).partial_cmp(&dxy(&b.pos, &me.pos)).unwrap_or(std::cmp::Ordering::Equal));
        threats.truncate(3);
        let (cx, cy) = ((me.pos[0] * 0.5).round() * 2.0, (me.pos[1] * 0.5).round() * 2.0);
        // Lattice-aligned candidates (world grid origin is integral).
        let ox = (w.min_x.rem_euclid(2.0)) - (cx.rem_euclid(2.0));
        let oy = (w.min_y.rem_euclid(2.0)) - (cy.rem_euclid(2.0));
        let n = (radius / step) as i32;
        // Rings outward from our feet, so the search stops as soon as nothing farther can win.
        for ring in 0..=n {
            if best.map_or(false, |b| b.0 < (ring as f32 - 1.0) * step) {
                break;
            }
            for j in -ring..=ring {
                for i in -ring..=ring {
                if i.abs().max(j.abs()) != ring {
                    continue;
                }
                let x = cx + ox + step * i as f32;
                let y = cy + oy + step * j as f32;
                let d = (x - me.pos[0]).hypot(y - me.pos[1]);
                if d > radius {
                    continue;
                }
                let c = w.cell(x, y);
                if !w.free(c) {
                    continue;
                }
                let s = w.surf[c];
                if (s - me.pos[2]).abs() > 2.5 {
                    continue;
                }
                if (x - z.c[0]).hypot(y - z.c[1]) > z.r - 6.0 {
                    continue;
                }
                let mut cost = d;
                let mut near = f32::INFINITY;
                for e in &threats {
                    near = near.min((x - e.pos[0]).hypot(y - e.pos[1]));
                }
                if near < 25.0 {
                    cost += 25.0 - near;
                }
                if best.map_or(false, |b| cost >= b.0) {
                    continue;
                }
                let hidden = threats.iter().all(|e| {
                    let eye = e.eye();
                    if w.los(eye, [x, y, s + 0.5 * me.height]) || w.los(eye, [x, y, s + me.height - 0.2]) {
                        return false;
                    }
                    if strict {
                        // Still hidden if the enemy sidesteps: cover that one step defeats is no cover.
                        let to = norm2([x - e.pos[0], y - e.pos[1]]);
                        for sgn in [-1.0f32, 1.0] {
                            let (px, py) = (e.pos[0] - to[1] * 18.0 * sgn, e.pos[1] + to[0] * 18.0 * sgn);
                            let pc = w.cell(px, py);
                            if w.prism[pc] {
                                continue;
                            }
                            if w.los([px, py, w.surf[pc] + 8.0], [x, y, s + 0.5 * me.height]) {
                                return false;
                            }
                        }
                    }
                    true
                });
                if hidden {
                    best = Some((cost, (x, y)));
                }
                }
            }
        }
        best.map(|b| b.1)
    }

    /// A low-exposure spot to wait in, between rmin and rmax metres away.
    fn pick_hide(&self, me: &Me, z: &Zone, t: f32, rmin: f32, rmax: f32) -> Option<(f32, f32)> {
        let w = &self.w;
        let mut best: Option<(f32, (f32, f32))> = None;
        let n = (rmax / 6.0) as i32;
        for j in -n..=n {
            for i in -n..=n {
                let d = 6.0 * ((i * i + j * j) as f32).sqrt();
                if d < rmin || d > rmax || best.map_or(false, |b| 0.35 * d + 3.0 >= b.0) {
                    continue;
                }
                let (x, y) = (me.pos[0] + 6.0 * i as f32, me.pos[1] + 6.0 * j as f32);
                let c = w.cell(x, y);
                if !w.free(c) || w.prism[c] || (w.surf[c] - me.pos[2]).abs() > 6.0 {
                    continue;
                }
                if (x - z.nc[0]).hypot(y - z.nc[1]) > z.nr - 12.0 || (x - z.c[0]).hypot(y - z.c[1]) > z.r - 12.0 {
                    continue;
                }
                if self.enemies.values().any(|e| t - e.seen < 15.0 && (e.pos[0] - x).hypot(e.pos[1] - y) < 80.0) {
                    continue;
                }
                if self.npc_mem.values().any(|n| (n.0[0] - x).hypot(n.0[1] - y) < 58.0) {
                    continue;
                }
                let score = w.openness(x, y) + 0.35 * d;
                if best.map_or(true, |b| score < b.0) {
                    best = Some((score, (x, y)));
                }
            }
        }
        best.map(|b| b.1)
    }

    fn follow(&mut self, me: &Me, goal: (f32, f32), t: f32) -> ([f32; 2], bool) {
        let here = (me.pos[0], me.pos[1]);
        let need = self.path.is_empty()
            || (goal.0 - self.path_goal.0).hypot(goal.1 - self.path_goal.1) > 4.0
            || t - self.path_t > 1.5
            || self.path_i >= self.path.len();
        if need {
            self.path = self.nav.path(&self.w, me.pos, goal, 12_000);
            self.path_goal = goal;
            self.path_t = t;
            self.path_i = 0;
        }
        if self.path.is_empty() {
            return (norm2([goal.0 - here.0, goal.1 - here.1]), false);
        }
        while self.path_i + 1 < self.path.len() && (self.path[self.path_i].0 - here.0).hypot(self.path[self.path_i].1 - here.1) < 2.4 {
            self.path_i += 1;
        }
        let mut tgt = self.path[self.path_i];
        let hi = (self.path_i + 10).min(self.path.len() - 1);
        let mut k = hi;
        while k > self.path_i {
            if self.w.walkable_line(here, self.path[k]) {
                tgt = self.path[k];
                break;
            }
            k -= 1;
        }
        let d = (tgt.0 - here.0).hypot(tgt.1 - here.1);
        let last = self.path[self.path.len() - 1];
        let at_end = self.path_i + 1 >= self.path.len() && (last.0 - here.0).hypot(last.1 - here.1) < 2.5;
        if d < 0.6 {
            return ([0.0, 0.0], at_end);
        }
        let s = if d < 2.0 && self.path_i + 1 >= self.path.len() { 0.5 } else { 1.0 };
        let n = norm2([tgt.0 - here.0, tgt.1 - here.1]);
        ([n[0] * s, n[1] * s], at_end)
    }

    /// How much of the target's velocity to lead by. Plain linear lead is best against most
    /// movement, but a mech that reverses every half second is closer to where it was than to
    /// where its velocity points; its own recent track tells us which.
    fn lead_factor(&self, id: i64, tof: f32, t: f32) -> f32 {
        let h = match self.hist.get(&id) {
            Some(h) if h.len() >= 12 => h,
            _ => return 1.0,
        };
        const KS: [f32; 4] = [1.0, 0.7, 0.4, 0.0];
        let mut hits = [0i32; 4];
        let mut n = 0;
        for i in 0..h.len() {
            let tt = h[i].0 + tof;
            if tt > t + 0.01 || t - h[i].0 > 2.6 {
                continue;
            }
            let mut j = i;
            while j + 1 < h.len() && h[j].0 < tt - 0.05 {
                j += 1;
            }
            if (h[j].0 - tt).abs() > 0.12 {
                continue;
            }
            for (q, k) in KS.iter().enumerate() {
                let px = h[i].1[0] + h[i].2[0] * tof * k;
                let py = h[i].1[1] + h[i].2[1] * tof * k;
                if (px - h[j].1[0]).hypot(py - h[j].1[1]) < 2.4 {
                    hits[q] += 1;
                }
            }
            n += 1;
        }
        if n < 8 {
            return 1.0;
        }
        let mut best = 0;
        for q in 1..4 {
            if hits[q] > hits[best] + 1 {
                best = q;
            }
        }
        KS[best]
    }

    /// Nothing taller than a step within a few metres this way (walls only; drops are fine).
    fn open_dir(&self, here: (f32, f32), z: f32, v: [f32; 2]) -> bool {
        for s in [1.5f32, 3.0, 4.5] {
            if self.w.surf_at(here.0 + v[0] * s, here.1 + v[1] * s) > z + 1.2 {
                return false;
            }
        }
        true
    }

    /// Sideways movement around a point at about `range`, reversing unpredictably and never
    /// stopping while any direction is open. `dir` points at the target, `d` is its distance.
    fn strafe_vec(&mut self, here: (f32, f32), z: f32, dir: [f32; 2], d: f32, range: f32, t: f32, zone: &Zone) -> [f32; 2] {
        if t >= self.strafe_until {
            if self.rand() < 0.85 {
                self.strafe_dir = -self.strafe_dir;
            }
            self.strafe_until = t + self.tune[6] + self.tune[7] * self.rand();
        }
        let radial = ((d - range) / 20.0).clamp(-1.0, 1.0) * 0.8;
        let sd = self.strafe_dir;
        let px = [-dir[1] * sd, dir[0] * sd];
        let cands = [
            [px[0] + dir[0] * radial, px[1] + dir[1] * radial],
            [-px[0] + dir[0] * radial, -px[1] + dir[1] * radial],
            px,
            [-px[0], -px[1]],
            [px[0] - dir[0] * 0.7, px[1] - dir[1] * 0.7],
            [-px[0] - dir[0] * 0.7, -px[1] - dir[1] * 0.7],
            [-dir[0], -dir[1]],
            dir,
        ];
        for (k, c) in cands.iter().enumerate() {
            let v = norm2(*c);
            let ahead = [here.0 + v[0] * 4.0, here.1 + v[1] * 4.0];
            if self.open_dir(here, z, v) && dxy(&ahead, &zone.c) < zone.r - 8.0 {
                if k == 1 || k == 3 || k == 5 {
                    self.strafe_dir = -sd;
                }
                return v;
            }
        }
        [0.0, 0.0]
    }

    fn upgrade_score(id: &str) -> i32 {
        match id {
            "plating" => 100,
            "armor" => 95,
            "nanites" => 80,
            "gun_damage" => 78,
            "laser_damage" => 77,
            "gun_rate" => 75,
            "laser_charge" => 70,
            "lightning_efficiency" => 68,
            "servos" => 60,
            "gun_mag" => 55,
            "reactor" => 50,
            "capacitor" => 45,
            "gyros" => 40,
            "lightning_range" => 38,
            "gun_velocity" => 36,
            "sensors" => 30,
            "lightning_chain" => 20,
            "laser_pierce" => 15,
            "jets" => 10,
            _ => 5,
        }
    }

    pub fn decide(&mut self, obs: &Value) -> Value {
        let t = f(&obs["t"]);
        let me = Self::parse_me(&obs["self"]);
        let here = (me.pos[0], me.pos[1]);
        let muzzle = [me.pos[0], me.pos[1], me.pos[2] + me.muzzle_h];
        let my_center = [me.pos[0], me.pos[1], me.pos[2] + 0.5 * me.height];
        let hp_frac = me.hp / me.max_hp;

        // ---- memory
        // Drones in view: (centre, velocity, id).
        let mut npcs: Vec<([f32; 3], [f32; 3], i64)> = Vec::new();
        if let Some(ms) = obs["mechs"].as_array() {
            for m in ms {
                if m["kind"].as_str() == Some("npc") {
                    if let Some(id) = m["id"].as_i64() {
                        let p = v3(&m["pos"]);
                        self.npc_mem.insert(id, (p, t));
                        npcs.push(([p[0], p[1], p[2] + 1.75], v3(&m["vel"]), id));
                    }
                    continue;
                }
                if m["kind"].as_str() != Some("mech") {
                    continue;
                }
                let id = match m["id"].as_i64() {
                    Some(i) => i,
                    None => continue,
                };
                let hurt = self.enemies.get(&id).map_or(-100.0, |e| e.hurt_me_t);
                let hostile = self.enemies.get(&id).map_or(-100.0, |e| e.hostile_until);
                let facing_since = self.enemies.get(&id).map_or(-1.0, |e| e.facing_since);
                let hit_by_me = self.enemies.get(&id).map_or(false, |e| e.hit_by_me) || self.i_hit.contains(&id);
                let weapons = m["weapons"].as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(|s| s.to_string())).collect()).unwrap_or_default();
                self.enemies.insert(
                    id,
                    Enemy {
                        id,
                        pos: v3(&m["pos"]),
                        vel: v3(&m["vel"]),
                        aim: [f(&m["aim"][0]), f(&m["aim"][1])],
                        hp: m["hp"].as_f64().unwrap_or(1400.0) as f32,
                        max_hp: m["max_hp"].as_f64().unwrap_or(1400.0) as f32,
                        cubes: f(&m["cubes"]),
                        weapons,
                        seen: t,
                        hurt_me_t: hurt,
                        hostile_until: hostile,
                        charging: m["charging"].as_bool().unwrap_or(false),
                        facing_since,
                        hit_by_me,
                    },
                );
            }
        }
        if let Some(evs) = obs["events"].as_array() {
            for e in evs {
                if e["type"].as_str() == Some("kill") {
                    self.kills += 1;
                }
                if e["type"].as_str() == Some("hit") {
                    if let Some(id) = e["target"].as_i64() {
                        if !self.i_hit.contains(&id) {
                            self.i_hit.push(id);
                        }
                        if let Some(en) = self.enemies.get_mut(&id) {
                            en.hit_by_me = true;
                        }
                    }
                }
                if e["type"].as_str() == Some("damaged") && e["weapon"].as_str() == Some("npc") {
                    self.npc_hurt_t = t;
                } else if e["type"].as_str() == Some("damaged") {
                    if let Some(by) = e["by"].as_i64() {
                        if by < 100 && !self.attackers_seen.contains(&by) {
                            self.attackers_seen.push(by);
                        }
                        self.last_hurt_t = t;
                        if let Some(b) = e["bearing"].as_f64() {
                            self.hurt_bearing = b as f32;
                        }
                        if self.enemies.get(&by).map_or(true, |e| e.seen < t - 0.01) {
                            self.ghost_t = t;
                        }
                        if let Some(en) = self.enemies.get_mut(&by) {
                            en.hurt_me_t = t;
                            en.hostile_until = t + 30.0;
                        }
                    }
                }
            }
        }
        for (id, e) in &self.enemies {
            if e.seen >= t - 0.01 {
                let h = self.hist.entry(*id).or_default();
                h.push((t, e.pos, e.vel));
                if h.len() > 32 {
                    h.remove(0);
                }
            }
        }
        self.hist.retain(|_, h| h.last().map_or(false, |x| t - x.0 < 6.0));
        self.enemies.retain(|_, e| t - e.seen < 20.0);
        Self::remember(&mut self.pickups, &obs["pickups"], t);
        Self::remember(&mut self.crates, &obs["crates"], t);
        Self::remember(&mut self.cubes, &obs["cubes"], t);
        self.forget(&me, t);

        let zv = &obs["zone"];
        let z = Zone {
            c: [f(&zv["center"][0]), f(&zv["center"][1])],
            r: zv["radius"].as_f64().unwrap_or(1e4) as f32,
            nc: [f(&zv["next_center"][0]), f(&zv["next_center"][1])],
            nr: zv["next_radius"].as_f64().unwrap_or(1e4) as f32,
            ss: zv["shrink_start"].as_f64().unwrap_or(1e6) as f32,
        };

        {
            let w = &self.w;
            for e in self.enemies.values_mut() {
                if e.seen >= t - 0.01 {
                    let facing = dxy(&e.pos, &me.pos) < 230.0 && wrap(aim_to(e.eye(), my_center)[0] - e.aim[0]).abs() < 0.2 && w.los(e.eye(), my_center);
                    if facing {
                        if e.facing_since < 0.0 {
                            e.facing_since = t;
                        }
                        if t - e.facing_since >= 0.5 || e.charging {
                            e.hostile_until = e.hostile_until.max(t + 6.0);
                        }
                    } else {
                        e.facing_since = -1.0;
                    }
                }
            }
        }
        // ---- which kind of match is this? Mechs dying that we did not kill means a free-for-all.
        let alive = obs["alive"].as_i64().unwrap_or(16);
        if self.cfg_mode == 0 {
            if self.alive0 < 0 {
                self.alive0 = alive;
            }
            // A death we did not cause means a free-for-all. Wait a second before believing it, in
            // case the kill event for our own shot arrives an observation late.
            if self.alive0 - alive > self.kills {
                if self.foreign_since < 0.0 {
                    self.foreign_since = t;
                }
            } else {
                self.foreign_since = -1.0;
            }
            let foreign_death = self.foreign_since >= 0.0 && t - self.foreign_since >= 1.0;
            // House bots never hurt each other and hunt one at a time early, so a badly wounded mech
            // we never touched, or two attackers while we are still only defending, means the same.
            let wounded_by_others = t < 60.0 && self.enemies.values().any(|e| e.seen >= t - 0.01 && !e.hit_by_me && e.hp < e.max_hp - 350.0);
            let ganged = self.mode == 0 && self.kills == 0 && self.attackers_seen.len() >= 2;
            if foreign_death || (self.mode == 0 && (wounded_by_others || ganged)) {
                self.mode = 2;
            } else if self.mode == 0 && t >= self.tune[9] {
                self.mode = 1;
            }
        }
        let cautious = self.mode != 1;
        let slot_of = |k: &str| -> Option<usize> { (0..2).find(|&i| me.wpn[i].as_ref().map_or(false, |w| w.kind == k)) };
        let gun_s = slot_of("gun");
        let laser_s = slot_of("laser");
        let light_s = slot_of("lightning");
        let my_power = me.hp * (1.0 + 0.04 * me.cubes.min(10.0)) * if laser_s.is_some() { 1.3 } else { 1.0 };
        let power = |e: &Enemy| e.hp * (1.0 + 0.04 * e.cubes.min(10.0)) * if e.has("laser") { 1.3 } else { 1.0 };
        let threats: Vec<Enemy> = self.enemies.values().filter(|e| e.seen >= t - 0.01).cloned().collect();
        let any_hostile = threats.iter().any(|e| e.hostile_until > t);

        // ---- target: best enemy we have a line of fire to
        let mut target: Option<Enemy> = None;
        let mut best = f32::INFINITY;
        let mut attackers = 0;
        let mut nearest_threat = f32::INFINITY;
        for e in &threats {
            let d = d3(muzzle, e.center());
            nearest_threat = nearest_threat.min(d);
            let sees_me = self.w.los(e.eye(), my_center);
            let facing = wrap(aim_to(e.eye(), my_center)[0] - e.aim[0]).abs() < 0.3;
            if t - e.hurt_me_t < 3.0 || (sees_me && facing && d < 230.0) {
                attackers += 1;
            }
            let allowed = if cautious {
                // Free-for-all (or not sure yet): shoot back, finish the weak, bully only late.
                let prey = e.hp < e.max_hp * if alive <= 4 { 0.5 } else { 0.35 } && hp_frac > 0.6 && power(e) < 0.6 * my_power;
                // A wounded mech looking elsewhere is in someone else's fight: the cheapest cubes.
                let busy = self.tune[14] > 0.0
                    && e.hp < e.max_hp * self.tune[14]
                    && hp_frac > 0.8
                    && d < 120.0
                    && power(e) < 0.9 * my_power
                    && wrap(aim_to(e.eye(), my_center)[0] - e.aim[0]).abs() > 1.2;
                e.hostile_until > t || prey || busy || (alive <= 4 && my_power > 1.4 * power(e))
            } else {
                match self.policy {
                    1 => e.hostile_until > t,
                    2 => e.hostile_until > t || (!any_hostile && hp_frac > 0.9),
                    _ => true,
                }
            };
            if allowed && d < 260.0 && self.w.los(muzzle, e.center()) {
                let score = d + self.tune[5] * e.hp - if t - e.hurt_me_t < 3.0 || e.hostile_until > t { self.tune[4] } else { 0.0 };
                if score < best {
                    best = score;
                    target = Some(e.clone());
                }
            }
        }
        let finishable = target.as_ref().map_or(false, |e| e.hp <= 180.0);

        // ---- stand or run. Against the house squad nobody else thins the field, so running only
        // delays the same fight. In a free-for-all every second alive is worth points: fight one
        // attacker we can match, hide from everything else.
        let mut evade_from: Vec<Enemy> = threats.clone();
        if cautious {
            // Keep hiding from mechs seen in the last few seconds, and from a shooter we cannot see.
            for e in self.enemies.values() {
                if e.seen < t - 0.01 && t - e.seen < 10.0 {
                    evade_from.push(e.clone());
                }
            }
            if t - self.ghost_t < 3.0 && !threats.iter().any(|e| e.hostile_until > t) {
                let p = [me.pos[0] + 110.0 * self.hurt_bearing.cos(), me.pos[1] + 110.0 * self.hurt_bearing.sin()];
                let gz = self.w.surf_at(p[0], p[1]);
                evade_from.push(Enemy { id: -1, pos: [p[0], p[1], gz], vel: [0.0; 3], aim: [0.0, 0.0], hp: 1400.0, max_hp: 1400.0, cubes: 0.0, weapons: Vec::new(), seen: t, hurt_me_t: t, hostile_until: t + 1.0, charging: false, facing_since: -1.0, hit_by_me: false });
            }
        }
        if cautious {
            let hostiles: Vec<&Enemy> = threats.iter().filter(|e| e.hostile_until > t).collect();
            let engage_ok = match &target {
                None => false,
                Some(_) => {
                    let near = threats.iter().filter(|o| dxy(&o.pos, &me.pos) < 130.0).count();
                    if hostiles.is_empty() {
                        true
                    } else if hostiles.len() == 1 && near <= 1 {
                        my_power >= (if self.mode == 0 { self.tune[11] } else { self.tune[8] }) * power(hostiles[0]) || finishable
                    } else {
                        false
                    }
                }
            };
            let evade = !evade_from.is_empty() && !engage_ok;
            if evade && !self.retreating {
                self.cover = None;
            }
            self.retreating = evade;
        } else if self.tune[3] < 0.5 {
            self.retreating = false;
        } else if !self.retreating {
            if !threats.is_empty() && !finishable && (hp_frac < self.tune[1] || (attackers >= 2 && hp_frac < self.tune[2])) {
                self.retreating = true;
                self.cover = None;
            }
        } else if hp_frac >= 0.9 || (threats.is_empty() && hp_frac >= 0.7 && t - self.last_hurt_t > 6.0) {
            self.retreating = false;
        }

        // ---- zone
        let margin = (z.nr * 0.5).min(20.0);
        let d_next = dxy(&[here.0, here.1], &z.nc);
        let inside_next = d_next <= z.nr - margin;
        let zone_point = if d_next > 1e-3 {
            let k = ((z.nr - margin - 6.0).max(0.0) / d_next).min(1.0);
            (z.nc[0] + (here.0 - z.nc[0]) * k, z.nc[1] + (here.1 - z.nc[1]) * k)
        } else {
            (z.nc[0], z.nc[1])
        };
        let travel = (d_next - (z.nr - margin)).max(0.0) / me.walk;
        let near_edge = dxy(&[here.0, here.1], &z.c) > z.r - 10.0;
        let zone_urgent = !me.in_zone || (!inside_next && (z.ss - t < travel + 12.0 || near_edge));
        let in_bounds = |p: &[f32]| -> bool {
            dxy(p, &z.c) < z.r - 8.0 && (z.ss - t > 25.0 || dxy(p, &z.nc) < z.nr - 4.0)
        };


        // ---- loot: take what is in reach
        let mut take: Option<i64> = None;
        let mut drop: Option<i64> = None;
        if let Some(ps) = obs["pickups"].as_array() {
            for p in ps {
                let pos = v3(&p["pos"]);
                if dxy(&pos, &me.pos) > self.pickup_radius - 0.3 || (pos[2] - me.pos[2]).abs() > 3.5 {
                    continue;
                }
                let kind = p["kind"].as_str().unwrap_or("");
                if slot_of(kind).is_some() || !(kind == "laser" || kind == "lightning") {
                    continue;
                }
                let empty = (0..2).find(|&i| me.wpn[i].is_none());
                if empty.is_some() {
                    take = p["id"].as_i64();
                } else if let Some(w1) = &me.wpn[1] {
                    let upgrade = (w1.kind == "mortar") || (self.tune[12] < 0.5 && w1.kind == "lightning" && kind == "laser" && threats.is_empty());
                    if upgrade && w1.charge <= 0.0 {
                        take = p["id"].as_i64();
                        drop = Some(1);
                    } else if self.tune[12] > 0.5 {
                        if let Some(gs) = gun_s {
                            take = p["id"].as_i64();
                            drop = Some(gs as i64);
                        }
                    }
                }
                if take.is_some() {
                    break;
                }
            }
        }

        // Nearest cube we can walk to: worth grabbing even mid-fight (HP, damage, upgrades).
        let mut near_cube: Option<(f32, (f32, f32), i64)> = None;
        for (id, it) in &self.cubes {
            if self.blacklist.get(id).map_or(false, |u| t < *u) {
                continue;
            }
            let d = dxy(&it.pos, &me.pos);
            if d < 75.0 && in_bounds(&it.pos) && (it.pos[2] - me.pos[2]).abs() < 5.0 && near_cube.map_or(true, |c| d < c.0) {
                near_cube = Some((d, (it.pos[0], it.pos[1]), *id));
            }
        }

        // Nearest weapon worth having while slot 1 is empty.
        let mut near_gun: Option<(f32, (f32, f32), i64)> = None;
        if me.wpn[1].is_none() || (self.tune[12] > 0.5 && gun_s.is_some()) {
            for (id, it) in &self.pickups {
                if self.blacklist.get(id).map_or(false, |u| t < *u) || !(it.kind == "laser" || it.kind == "lightning") || slot_of(&it.kind).is_some() {
                    continue;
                }
                let d = dxy(&it.pos, &me.pos);
                if d < 60.0 && in_bounds(&it.pos) && (it.pos[2] - me.pos[2]).abs() < 5.0 && near_gun.map_or(true, |c| d < c.0) {
                    near_gun = Some((d, (it.pos[0], it.pos[1]), *id));
                }
            }
        }

        // Weapon rush: with only the gun, a laser or lightning we can reach before anyone else
        // comes first, ahead of hiding. The teams that win free-for-alls are armed within seconds.
        let mut rush: Option<(f32, (f32, f32), i64)> = None;
        if self.tune[13] < 0.5 && me.wpn[1].is_none() {
            for (id, it) in &self.pickups {
                if self.blacklist.get(id).map_or(false, |u| t < *u) || !(it.kind == "laser" || it.kind == "lightning") {
                    continue;
                }
                let d = dxy(&it.pos, &me.pos);
                let climb = (it.pos[2] - me.pos[2]).max(0.0);
                if d > 130.0 || climb > 14.0 || !in_bounds(&it.pos) {
                    continue;
                }
                // Someone else closer to it, or a shooter between us and a long run: leave it.
                if self.enemies.values().any(|e| t - e.seen < 6.0 && dxy(&e.pos, &it.pos) < d + 15.0) {
                    continue;
                }
                if d > 40.0 && threats.iter().any(|e| e.hostile_until > t && self.w.los(e.eye(), my_center)) {
                    continue;
                }
                let score = d * if it.kind == "laser" { 0.7 } else { 1.0 } + climb * 3.0;
                if rush.map_or(true, |r| score < r.0) {
                    rush = Some((score, (it.pos[0], it.pos[1]), *id));
                }
            }
        }

        // A drone worth shooting: one that is hurting us, or any in reach when things are quiet.
        // Each is 350 HP for two cubes, and its own bullets are slow.
        let quiet = threats.is_empty() && (!cautious || self.enemies.values().all(|e| t - e.seen > 8.0));
        let mut shoot_npc: Option<([f32; 3], [f32; 3])> = None;
        if self.tune[15] < 0.5 && target.is_none() && hp_frac > 0.35 && (t - self.npc_hurt_t < 4.0 || (quiet && hp_frac > 0.6)) {
            let mut bd = 85.0;
            for n in &npcs {
                let d = d3(muzzle, n.0);
                if d < bd && self.w.los(muzzle, n.0) {
                    bd = d;
                    shoot_npc = Some((n.0, n.1));
                }
            }
        }

        // ---- movement
        let mut shoot_crate: Option<[f32; 3]> = None;
        let mut goal_id: Option<i64> = None;
        let mv: Mv;
        let cornered = target.as_ref().map_or(false, |e| d3(muzzle, e.center()) < self.tune[10]);
        let mut flee_hold = false;
        let mut flee_goal: Option<(f32, f32)> = None;
        if self.retreating && !cornered && t >= self.unstick_until && !zone_urgent {
            if evade_from.is_empty() {
                flee_hold = true;
            } else {
                let exposed = match self.cover {
                    Some(c) => {
                        let s = self.w.surf_at(c.0, c.1);
                        evade_from.iter().any(|e| self.w.los(e.eye(), [c.0, c.1, s + 0.5 * me.height]))
                    }
                    None => true,
                };
                if exposed && t - self.cover_t > 0.5 {
                    self.cover = if cautious {
                        // Prefer cover that survives a sidestep; widen and relax if there is none.
                        let hostile_only: Vec<Enemy> = evade_from.iter().filter(|e| e.hostile_until > t).cloned().collect();
                        self.find_cover(&me, &evade_from, &z, true, 60.0, 4.0)
                            .or_else(|| self.find_cover(&me, &evade_from, &z, false, 96.0, 6.0))
                            .or_else(|| if hostile_only.is_empty() { None } else { self.find_cover(&me, &hostile_only, &z, false, 96.0, 6.0) })
                    } else {
                        self.find_cover(&me, &evade_from, &z, false, 46.0, 4.0)
                    };
                    self.cover_t = t;
                }
                match self.cover {
                    Some(c) => {
                        if (c.0 - here.0).hypot(c.1 - here.1) < 1.5 {
                            flee_hold = true;
                        } else {
                            flee_goal = Some(c);
                        }
                    }
                    None => {
                        if !cautious {
                            // No cover in reach: run from the nearest threat, staying in the zone.
                            let e = evade_from.iter().min_by(|a, b| dxy(&a.pos, &me.pos).partial_cmp(&dxy(&b.pos, &me.pos)).unwrap()).unwrap();
                            let away = norm2([here.0 - e.pos[0], here.1 - e.pos[1]]);
                            let p = [here.0 + away[0] * 40.0, here.1 + away[1] * 40.0];
                            flee_goal = Some(if in_bounds(&p) { (p[0], p[1]) } else { zone_point });
                        }
                        // Cautious with no cover: running from a shooter in the open only shows
                        // our back, so fall through and fight. If nobody is shooting yet, walk out
                        // of the nearest watcher's sight instead of standing in it.
                        if cautious && target.is_none() {
                            let watcher = threats.iter().filter(|e| self.w.los(e.eye(), my_center)).min_by(|a, b| dxy(&a.pos, &me.pos).partial_cmp(&dxy(&b.pos, &me.pos)).unwrap());
                            if let Some(e) = watcher {
                                let away = norm2([here.0 - e.pos[0], here.1 - e.pos[1]]);
                                for len in [40.0f32, 25.0, 12.0] {
                                    let p = [here.0 + away[0] * len, here.1 + away[1] * len];
                                    if in_bounds(&p) && self.w.free_at(p[0], p[1]) {
                                        flee_goal = Some((p[0], p[1]));
                                        break;
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
        if t < self.unstick_until {
            mv = Mv::Direct([self.unstick_dir.cos(), self.unstick_dir.sin()], true);
        } else if zone_urgent {
            mv = Mv::Goto(zone_point, !me.in_zone || z.ss - t < travel + 4.0);
        } else if let (Some(r), false) = (rush, cornered) {
            goal_id = Some(r.2);
            mv = Mv::Goto(r.1, me.energy > 15.0);
        } else if flee_hold {
            mv = Mv::Hold;
        } else if let Some(c) = flee_goal {
            mv = Mv::Goto(c, me.energy > 12.0);
        } else if let (Some(c), true) = (near_gun, !threats.is_empty() && !self.retreating) {
            goal_id = Some(c.2);
            mv = Mv::Goto(c.1, me.energy > 30.0 && c.0 > 15.0);
        } else if let (Some(c), true) = (near_cube, !threats.is_empty() && !self.retreating) {
            // Fighting or stalking with a cube in reach: collect it while we shoot.
            goal_id = Some(c.2);
            mv = Mv::Goto(c.1, me.energy > 30.0 && c.0 > 15.0);
        } else if let Some(e) = &target {
            // Fight: strafe around the target at the range our weapons like.
            let to = [e.pos[0] - here.0, e.pos[1] - here.1];
            let d = to[0].hypot(to[1]).max(0.1);
            let dir = [to[0] / d, to[1] / d];
            let mut range = self.tune[0];
            if light_s.is_some() && me.energy > 25.0 {
                range = 30.0;
            } else if e.has("lightning") {
                range = 65.0;
            }
            let out = self.strafe_vec(here, me.pos[2], dir, d, range, t, &z);
            let burst = me.energy > 55.0 && (t * 1.3).sin() > 0.3;
            mv = Mv::Direct(out, burst);
        } else if !threats.is_empty() && nearest_threat < 200.0 {
            // Known enemy without a line of fire: chase the weak, otherwise wait for it.
            let e = threats.iter().min_by(|a, b| dxy(&a.pos, &me.pos).partial_cmp(&dxy(&b.pos, &me.pos)).unwrap()).unwrap();
            if e.hp / e.max_hp.max(1.0) < 0.4 && hp_frac > 0.6 {
                mv = Mv::Goto((e.pos[0], e.pos[1]), false);
            } else {
                mv = Mv::Hold;
            }
        } else if let (Some(n), true) = (shoot_npc, threats.is_empty()) {
            // Circle the drone at a distance where its slow bullets miss a moving mech.
            let to = [n.0[0] - here.0, n.0[1] - here.1];
            let d = to[0].hypot(to[1]).max(0.1);
            let out = self.strafe_vec(here, me.pos[2], [to[0] / d, to[1] / d], d, 60.0, t, &z);
            mv = Mv::Direct(out, false);
        } else {
            // Idle: supply drop, cubes, weapons, crates, then explore inside the next circle.
            let mut goal: Option<(f32, (f32, f32), i64)> = None;
            let bl = &self.blacklist;
            let en = &self.enemies;
            let danger = |p: &[f32; 3]| cautious && en.values().any(|e| t - e.seen < 12.0 && dxy(&e.pos, p) < 90.0);
            let calm = !cautious || en.values().all(|e| t - e.seen > 8.0);
            let consider = |score: f32, p: &[f32; 3], id: i64, goal: &mut Option<(f32, (f32, f32), i64)>| {
                if bl.get(&id).map_or(false, |u| t < *u) {
                    return;
                }
                if goal.map_or(true, |g| score < g.0) {
                    *goal = Some((score, (p[0], p[1]), id));
                }
            };
            if let Some(ds) = obs["drops"].as_array() {
                for d in ds {
                    let p = v3(&d["pos"]);
                    let dist = dxy(&p, &me.pos);
                    if in_bounds(&p) && dist < 300.0 && f(&d["lands_in"]) < dist / me.walk + 10.0 && (!cautious || (calm && dist < 120.0 && !danger(&p))) {
                        consider(dist * 0.4, &p, d["id"].as_i64().unwrap_or(-1), &mut goal);
                    }
                }
            }
            for (id, it) in &self.cubes {
                if in_bounds(&it.pos) && (it.pos[2] - me.pos[2]).abs() < 6.0 && !danger(&it.pos) {
                    consider(dxy(&it.pos, &me.pos) * 0.5, &it.pos, *id, &mut goal);
                }
            }
            let want_laser = laser_s.is_none();
            let slot_free = me.wpn[1].is_none() || me.wpn[0].is_none();
            for (id, it) in &self.pickups {
                let wanted = ((slot_free || (self.tune[12] > 0.5 && gun_s.is_some())) && (it.kind == "laser" || it.kind == "lightning") && slot_of(&it.kind).is_none())
                    || (!slot_free && want_laser && it.kind == "laser" && light_s.is_some());
                if wanted && !danger(&it.pos) && in_bounds(&it.pos) && (it.pos[2] - self.w.surf_at(it.pos[0], it.pos[1])).abs() < 2.0 {
                    let pref = if it.kind == "laser" { 0.6 } else { 0.8 };
                    consider(dxy(&it.pos, &me.pos) * pref, &it.pos, *id, &mut goal);
                }
            }
            if hp_frac > 0.85 && calm {
                for (id, it) in &self.crates {
                    if in_bounds(&it.pos) && !danger(&it.pos) {
                        consider(dxy(&it.pos, &me.pos) * 1.0 + 10.0, &it.pos, *id, &mut goal);
                        let c = [it.pos[0], it.pos[1], it.pos[2] + 1.2];
                        let dd = d3(muzzle, c);
                        if dd < 70.0 && self.w.los(muzzle, c) && shoot_crate.map_or(true, |s| dd < d3(muzzle, s)) {
                            shoot_crate = Some(c);
                        }
                    }
                }
            }
            if let Some(c) = shoot_crate {
                // Close enough to shoot; keep walking in until 20 m so the cube is near.
                if d3(muzzle, c) < 20.0 {
                    goal = Some((0.0, here, -1));
                }
            }
            match goal {
                Some((_, g, _)) if g == here => mv = Mv::Hold,
                Some((_, g, id)) => {
                    goal_id = Some(id);
                    mv = Mv::Goto(g, false);
                }
                None => {
                    if !inside_next {
                        mv = Mv::Goto(zone_point, false);
                    } else if cautious {
                        // Nothing to collect: wait in an enclosed spot, and hop to a new one now and
                        // then so we see more of the map.
                        let stale = match self.hide {
                            Some(h) => {
                                let hp3 = [h.0, h.1, 0.0];
                                !in_bounds(&hp3) || danger(&hp3) || (self.hide_arrived >= 0.0 && t - self.hide_arrived > 14.0) || t - self.hide_t > 60.0
                            }
                            None => true,
                        };
                        if stale {
                            let first = self.hide.is_none();
                            self.hide = self.pick_hide(&me, &z, t, if first { 0.0 } else { 40.0 }, if first { 60.0 } else { 110.0 });
                            self.hide_t = t;
                            self.hide_arrived = -1.0;
                        }
                        mv = match self.hide {
                            Some(h) if (h.0 - here.0).hypot(h.1 - here.1) < 3.0 => {
                                if self.hide_arrived < 0.0 {
                                    self.hide_arrived = t;
                                }
                                Mv::Hold
                            }
                            Some(h) => Mv::Goto(h, false),
                            None => Mv::Hold,
                        };
                    } else {
                        let stale = match self.wander {
                            Some(wp) => t - self.wander_t > 30.0 || (wp.0 - here.0).hypot(wp.1 - here.1) < 6.0 || !in_bounds(&[wp.0, wp.1]),
                            None => true,
                        };
                        if stale {
                            let mut pick = None;
                            for _ in 0..12 {
                                let a = self.rand() * TAU;
                                let r = z.nr * 0.8 * self.rand().sqrt();
                                let p = (z.nc[0] + r * a.cos(), z.nc[1] + r * a.sin());
                                if self.w.free_at(p.0, p.1) && !self.w.prism[self.w.cell(p.0, p.1)] {
                                    pick = Some(p);
                                    break;
                                }
                            }
                            self.wander = pick;
                            self.wander_t = t;
                        }
                        mv = match self.wander {
                            Some(wp) => Mv::Goto(wp, false),
                            None => Mv::Hold,
                        };
                    }
                }
            }
        }

        let mut jump = false;
        let (mut mvec, mut boost) = match mv {
            Mv::Goto(g, b) => {
                let (v, at_end) = self.follow(&me, g, t);
                let mut out = (v, b);
                let gc = self.w.cell(g.0, g.1);
                let gs = self.w.surf[gc];
                let dgoal = (g.0 - here.0).hypot(g.1 - here.1);
                let roof_goal = self.w.prism[gc] && gs > me.pos[2] + 1.5;
                if at_end && dgoal > 2.5 && me.grounded {
                    if roof_goal && dgoal < 60.0 {
                        // The goal is on a roof we cannot walk onto: jet up from here.
                        self.climb_until = t + 9.0;
                        self.climb_goal = g;
                    } else if let Some(id) = goal_id {
                        self.blacklist.insert(id, t + 90.0);
                    }
                }
                if t < self.climb_until && (self.climb_goal.0 - g.0).hypot(self.climb_goal.1 - g.1) < 3.0 {
                    if me.grounded && !roof_goal {
                        self.climb_until = -1.0;
                    } else {
                        let dir = norm2([g.0 - here.0, g.1 - here.1]);
                        let mut wall_top = me.pos[2];
                        let reach = dgoal.min(8.0);
                        let mut s = 1.0;
                        while s <= reach {
                            wall_top = wall_top.max(self.w.surf_at(here.0 + dir[0] * s, here.1 + dir[1] * s));
                            s += 1.0;
                        }
                        if me.grounded {
                            let need = (wall_top - me.pos[2] - 2.0).max(0.0) * 1.7 + 8.0;
                            if need > me.max_energy {
                                self.climb_until = -1.0;
                                if let Some(id) = goal_id {
                                    self.blacklist.insert(id, t + 300.0);
                                }
                            } else if me.energy >= need {
                                jump = true;
                                out = (dir, false);
                            } else {
                                out = ([0.0, 0.0], false);
                            }
                        } else {
                            out = (dir, me.pos[2] < wall_top + 0.8);
                        }
                    }
                }
                out
            }
            Mv::Direct(v, b) => (v, b),
            Mv::Hold => ([0.0, 0.0], false),
        };

        // ---- stuck detection (only when we mean to travel)
        let travelling = matches!(mv, Mv::Goto(..)) && mvec[0].hypot(mvec[1]) > 0.6;
        if !travelling || t < self.unstick_until {
            self.stuck_pos = me.pos;
            self.stuck_t = t;
        } else if t - self.stuck_t >= 1.0 {
            if d3(me.pos, self.stuck_pos) < 0.8 {
                self.path.clear();
                if let (Some(id), false) = (goal_id, threats.is_empty()) {
                    // Blocked on the way to loot with enemies about: forget it, keep fighting.
                    self.blacklist.insert(id, t + 25.0);
                } else {
                    self.unstick_until = t + 0.7;
                    self.unstick_dir = self.rand() * TAU;
                    jump = true;
                }
            }
            self.stuck_pos = me.pos;
            self.stuck_t = t;
        }
        if t < self.unstick_until {
            boost = true;
            if me.grounded {
                jump = true;
            }
        }
        if boost && me.energy < 3.0 {
            boost = false;
        }

        // ---- aim and fire
        let mut fire = [false, false];
        let mut aim;
        let mut reload = false;
        if let Some(e) = &target {
            let c0 = e.center();
            let speed = gun_s.and_then(|s| me.wpn[s].as_ref()).map_or(252.0, |w| w.speed);
            let lead_k = self.lead_factor(e.id, d3(muzzle, c0) / speed + 0.05, t);
            let mut p = c0;
            for _ in 0..4 {
                let tt = d3(muzzle, p) / speed + 0.05;
                p = [c0[0] + e.vel[0] * tt * lead_k, c0[1] + e.vel[1] * tt * lead_k, c0[2] + (e.vel[2] * tt * lead_k).clamp(-2.0, 2.0)];
            }
            let direct = [c0[0] + e.vel[0] * 0.08, c0[1] + e.vel[1] * 0.08, c0[2]];
            let d = d3(muzzle, c0);
            let scale = 1.0 + 0.05 * e.cubes;
            let on = |pt: [f32; 3]| -> bool {
                let a = aim_to(muzzle, pt);
                let dd = dxy(&pt, &muzzle).max(1.0);
                wrap(a[0] - me.aim[0]).abs() * dd < 2.2 * scale && (a[1] - me.aim[1]).abs() * dd < 3.6 * scale
            };
            let mut aim_pt = p;
            // Laser: charge while we have the line, release at full charge when on target.
            if let Some(s) = laser_s {
                let w = me.wpn[s].as_ref().unwrap();
                if d < w.range - 5.0 && w.cooldown <= 0.1 {
                    let ready = w.charge >= w.full_charge - 0.06;
                    if w.charge >= w.full_charge - 0.3 {
                        aim_pt = direct;
                    }
                    fire[s] = !(ready && on(direct));
                }
            }
            if let Some(s) = gun_s {
                let w = me.wpn[s].as_ref().unwrap();
                if d < w.range - 3.0 && w.ammo > 0.0 && w.reload <= 0.0 && on(p) {
                    fire[s] = true;
                }
            }
            aim = aim_to(muzzle, aim_pt);
        } else if let Some(e) = threats.iter().min_by(|a, b| dxy(&a.pos, &me.pos).partial_cmp(&dxy(&b.pos, &me.pos)).unwrap()) {
            // Pre-aim where a tracked enemy will appear; keep a laser charged for it.
            aim = aim_to(muzzle, e.center());
            if let Some(s) = laser_s {
                let w = me.wpn[s].as_ref().unwrap();
                if !self.retreating && w.cooldown <= 0.1 && dxy(&e.pos, &me.pos) < 150.0 && w.charge < w.full_charge + 1.4 {
                    fire[s] = true;
                }
            }
        } else if t - self.last_hurt_t < 2.5 {
            aim = [self.hurt_bearing, 0.0];
        } else if let Some((c0, v)) = shoot_npc {
            let speed = gun_s.and_then(|s| me.wpn[s].as_ref()).map_or(252.0, |w| w.speed);
            let tt = d3(muzzle, c0) / speed + 0.05;
            let p = [c0[0] + v[0] * tt, c0[1] + v[1] * tt, c0[2]];
            aim = aim_to(muzzle, p);
            let dd = dxy(&p, &muzzle).max(1.0);
            let on = wrap(aim[0] - me.aim[0]).abs() * dd < 1.4 && (aim[1] - me.aim[1]).abs() * dd < 1.3;
            if let Some(s) = gun_s {
                let w = me.wpn[s].as_ref().unwrap();
                if on && w.ammo > 0.0 && w.reload <= 0.0 && d3(muzzle, c0) < w.range - 3.0 {
                    fire[s] = true;
                }
            }
            if let Some(s) = laser_s {
                let w = me.wpn[s].as_ref().unwrap();
                if w.cooldown <= 0.1 {
                    fire[s] = !(w.charge >= w.full_charge - 0.06 && on);
                }
            }
            if let Some(s) = light_s {
                let w = me.wpn[s].as_ref().unwrap();
                if d3(muzzle, c0) < w.range - 3.0 && wrap(aim[0] - me.aim[0]).abs() < 0.2 && me.energy > 30.0 {
                    fire[s] = true;
                }
            }
        } else if let Some(c) = shoot_crate {
            aim = aim_to(muzzle, c);
            let dd = dxy(&c, &muzzle).max(1.0);
            let on = wrap(aim[0] - me.aim[0]).abs() * dd < 1.0 && (aim[1] - me.aim[1]).abs() * dd < 0.9;
            if let Some(s) = gun_s {
                let w = me.wpn[s].as_ref().unwrap();
                if on && w.ammo > 0.0 && w.reload <= 0.0 {
                    fire[s] = true;
                }
            } else if let Some(s) = light_s {
                let w = me.wpn[s].as_ref().unwrap();
                if d3(muzzle, c) < w.range - 3.0 && wrap(aim[0] - me.aim[0]).abs() < 0.2 && me.energy > 30.0 {
                    fire[s] = true;
                }
            } else if let Some(s) = laser_s {
                // One full charge kills a crate: hold until charged and on it, then let go.
                let w = me.wpn[s].as_ref().unwrap();
                if w.cooldown <= 0.1 {
                    fire[s] = !(w.charge >= w.full_charge - 0.06 && on);
                }
            }
        } else {
            // Scan: a full turn at the start to see what is around the spawn, then sweep around
            // the direction of travel, or spin when standing.
            if self.t0 < 0.0 {
                self.t0 = t;
            }
            if t - self.t0 < 1.7 {
                aim = [wrap(self.last_aim[0] + 0.39), 0.0];
            } else if mvec[0].hypot(mvec[1]) > 0.3 {
                let base = mvec[1].atan2(mvec[0]);
                aim = [wrap(base + 1.5 * (t * 1.4).sin()), 0.0];
            } else {
                aim = [wrap(self.last_aim[0] + 0.33), 0.0];
            }
        }
        // Lightning: instant arc at anything hostile inside the cone.
        if let Some(s) = light_s {
            let w = me.wpn[s].as_ref().unwrap();
            if me.energy >= w.energy + 2.0 {
                for e in &threats {
                    let c = e.center();
                    if d3(muzzle, c) < w.range - 2.0 && wrap(aim_to(muzzle, c)[0] - me.aim[0]).abs() < 0.22 && self.w.los(muzzle, c) {
                        fire[s] = true;
                    }
                }
            }
        }
        // Reload in quiet moments.
        if let Some(s) = gun_s {
            let w = me.wpn[s].as_ref().unwrap();
            if w.reload <= 0.0 && w.ammo < w.mag && ((target.is_none() && w.ammo <= w.mag * 0.5) || (threats.is_empty() && shoot_crate.is_none())) {
                reload = true;
            }
        }
        aim[1] = aim[1].clamp(-0.7, 1.3);
        if !aim[0].is_finite() || !aim[1].is_finite() {
            aim = [0.0, 0.0];
        }
        if !mvec[0].is_finite() || !mvec[1].is_finite() {
            mvec = [0.0, 0.0];
        }
        self.last_aim = aim;

        if self.debug {
            let mvk = match mv {
                Mv::Goto(g, _) => format!("goto({:.0},{:.0})", g.0, g.1),
                Mv::Direct(..) => "direct".to_string(),
                Mv::Hold => "hold".to_string(),
            };
            let mut es = String::new();
            for e in &threats {
                es += &format!(" [d{:.0} hp{:.0} {}{}]", dxy(&e.pos, &me.pos), e.hp, if e.hostile_until > t { "H" } else { "n" }, if self.w.los(e.eye(), my_center) { "L" } else { "-" });
            }
            self.dbg = format!("mode{} hp{:.0} {} retreat={} cover={:?} tgt={} fire={:?} thr:{}", self.mode, me.hp, mvk, self.retreating, self.cover.map(|c| (c.0 as i32, c.1 as i32)), target.is_some(), fire, es);
        }
        let r3 = |x: f32| (x * 1000.0).round() / 1000.0;
        let mut a = Map::new();
        a.insert("tick".into(), obs["tick"].clone());
        a.insert("move".into(), json!([r3(mvec[0]), r3(mvec[1])]));
        a.insert("boost".into(), json!(boost));
        a.insert("jump".into(), json!(jump));
        a.insert("aim".into(), json!([r3(aim[0]), r3(aim[1])]));
        a.insert("fire".into(), json!(fire));
        a.insert("reload".into(), json!(reload));
        if let Some(ch) = obs["offer"]["choices"].as_array() {
            let mut bi = 0usize;
            let mut bs = i32::MIN;
            for (i, c) in ch.iter().enumerate() {
                let s = Self::upgrade_score(c["id"].as_str().unwrap_or(""));
                if s > bs {
                    bs = s;
                    bi = i;
                }
            }
            a.insert("pick".into(), json!(bi));
        }
        if let Some(id) = take {
            a.insert("take".into(), json!(id));
            if let Some(d) = drop {
                a.insert("drop".into(), json!(d));
            }
        }
        Value::Object(a)
    }
}
