//! Decision logic: one call per observation, returns the action as JSON.
use crate::nav::Nav;
use crate::world::World;
use serde_json::{json, Map, Value};
use std::collections::HashMap;
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
    pos: [f32; 3],
    vel: [f32; 3],
    aim: [f32; 2],
    hp: f32,
    max_hp: f32,
    cubes: f32,
    weapons: Vec<String>,
    seen: f32,
    hurt_me_t: f32,
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

pub struct Bot {
    w: World,
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
}

impl Bot {
    pub fn new(hello: &Value) -> Bot {
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
        let w = match World::load(&dir, &wf, &tf) {
            Ok(w) => w,
            Err(e) => {
                eprintln!("map load failed ({}), using flat world", e);
                World::flat()
            }
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
    fn find_cover(&self, me: &Me, threats: &[Enemy], z: &Zone) -> Option<(f32, f32)> {
        let w = &self.w;
        let mut best: Option<(f32, (f32, f32))> = None;
        let (cx, cy) = ((me.pos[0] * 0.5).round() * 2.0, (me.pos[1] * 0.5).round() * 2.0);
        // Lattice-aligned candidates (world grid origin is integral).
        let ox = (w.min_x.rem_euclid(2.0)) - (cx.rem_euclid(2.0));
        let oy = (w.min_y.rem_euclid(2.0)) - (cy.rem_euclid(2.0));
        let n = 23i32;
        for j in -n..=n {
            for i in -n..=n {
                let x = cx + ox + 2.0 * i as f32;
                let y = cy + oy + 2.0 * j as f32;
                let d = (x - me.pos[0]).hypot(y - me.pos[1]);
                if d > 46.0 {
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
                for e in threats {
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
                    !w.los(eye, [x, y, s + 0.5 * me.height]) && !w.los(eye, [x, y, s + me.height - 0.2])
                });
                if hidden {
                    best = Some((cost, (x, y)));
                }
            }
        }
        best.map(|b| b.1)
    }

    fn follow(&mut self, me: &Me, goal: (f32, f32), t: f32) -> [f32; 2] {
        let here = (me.pos[0], me.pos[1]);
        let need = self.path.is_empty()
            || (goal.0 - self.path_goal.0).hypot(goal.1 - self.path_goal.1) > 4.0
            || t - self.path_t > 1.5
            || self.path_i >= self.path.len();
        if need {
            self.path = self.nav.path(&self.w, me.pos, goal, 25_000);
            self.path_goal = goal;
            self.path_t = t;
            self.path_i = 0;
        }
        if self.path.is_empty() {
            return norm2([goal.0 - here.0, goal.1 - here.1]);
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
        if d < 0.6 {
            return [0.0, 0.0];
        }
        let s = if d < 2.0 && self.path_i + 1 >= self.path.len() { 0.5 } else { 1.0 };
        let n = norm2([tgt.0 - here.0, tgt.1 - here.1]);
        [n[0] * s, n[1] * s]
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
        if let Some(ms) = obs["mechs"].as_array() {
            for m in ms {
                if m["kind"].as_str() != Some("mech") {
                    continue;
                }
                let id = match m["id"].as_i64() {
                    Some(i) => i,
                    None => continue,
                };
                let hurt = self.enemies.get(&id).map_or(-100.0, |e| e.hurt_me_t);
                let weapons = m["weapons"].as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(|s| s.to_string())).collect()).unwrap_or_default();
                self.enemies.insert(
                    id,
                    Enemy {
                        pos: v3(&m["pos"]),
                        vel: v3(&m["vel"]),
                        aim: [f(&m["aim"][0]), f(&m["aim"][1])],
                        hp: m["hp"].as_f64().unwrap_or(1400.0) as f32,
                        max_hp: m["max_hp"].as_f64().unwrap_or(1400.0) as f32,
                        cubes: f(&m["cubes"]),
                        weapons,
                        seen: t,
                        hurt_me_t: hurt,
                    },
                );
            }
        }
        if let Some(evs) = obs["events"].as_array() {
            for e in evs {
                if e["type"].as_str() == Some("damaged") {
                    if let Some(by) = e["by"].as_i64() {
                        self.last_hurt_t = t;
                        if let Some(b) = e["bearing"].as_f64() {
                            self.hurt_bearing = b as f32;
                        }
                        if let Some(en) = self.enemies.get_mut(&by) {
                            en.hurt_me_t = t;
                        }
                    }
                }
            }
        }
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

        let threats: Vec<Enemy> = self.enemies.values().filter(|e| e.seen >= t - 0.01).cloned().collect();

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
            if d < 260.0 && self.w.los(muzzle, e.center()) {
                let score = d + 0.08 * e.hp - if t - e.hurt_me_t < 3.0 { 40.0 } else { 0.0 };
                if score < best {
                    best = score;
                    target = Some(e.clone());
                }
            }
        }
        let finishable = target.as_ref().map_or(false, |e| e.hp <= 180.0);

        // ---- retreat hysteresis
        if !self.retreating {
            if !threats.is_empty() && !finishable && (hp_frac < 0.40 || (attackers >= 2 && hp_frac < 0.75)) {
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

        // ---- weapons I hold
        let slot_of = |k: &str| -> Option<usize> { (0..2).find(|&i| me.wpn[i].as_ref().map_or(false, |w| w.kind == k)) };
        let gun_s = slot_of("gun");
        let laser_s = slot_of("laser");
        let light_s = slot_of("lightning");

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
                    let upgrade = (w1.kind == "mortar") || (w1.kind == "lightning" && kind == "laser" && threats.is_empty());
                    if upgrade && w1.charge <= 0.0 {
                        take = p["id"].as_i64();
                        drop = Some(1);
                    }
                }
                if take.is_some() {
                    break;
                }
            }
        }

        // ---- movement
        let mut shoot_crate: Option<[f32; 3]> = None;
        let mv: Mv;
        let cornered = target.as_ref().map_or(false, |e| d3(muzzle, e.center()) < 40.0);
        if t < self.unstick_until {
            mv = Mv::Direct([self.unstick_dir.cos(), self.unstick_dir.sin()], true);
        } else if zone_urgent {
            mv = Mv::Goto(zone_point, !me.in_zone || z.ss - t < travel + 4.0);
        } else if self.retreating && !cornered {
            if threats.is_empty() {
                mv = Mv::Hold;
            } else {
                let exposed = match self.cover {
                    Some(c) => {
                        let s = self.w.surf_at(c.0, c.1);
                        threats.iter().any(|e| self.w.los(e.eye(), [c.0, c.1, s + 0.5 * me.height]))
                    }
                    None => true,
                };
                if exposed && t - self.cover_t > 0.5 {
                    self.cover = self.find_cover(&me, &threats, &z);
                    self.cover_t = t;
                }
                match self.cover {
                    Some(c) => {
                        if (c.0 - here.0).hypot(c.1 - here.1) < 1.5 {
                            mv = Mv::Hold;
                        } else {
                            mv = Mv::Goto(c, me.energy > 12.0);
                        }
                    }
                    None => {
                        // No cover in reach: run from the nearest threat, staying in the zone.
                        let e = threats.iter().min_by(|a, b| dxy(&a.pos, &me.pos).partial_cmp(&dxy(&b.pos, &me.pos)).unwrap()).unwrap();
                        let away = norm2([here.0 - e.pos[0], here.1 - e.pos[1]]);
                        let p = [here.0 + away[0] * 40.0, here.1 + away[1] * 40.0];
                        if in_bounds(&p) {
                            mv = Mv::Goto((p[0], p[1]), me.energy > 12.0);
                        } else {
                            mv = Mv::Goto(zone_point, me.energy > 12.0);
                        }
                    }
                }
            }
        } else if let Some(e) = &target {
            // Fight: strafe around the target at the range our weapons like.
            let to = [e.pos[0] - here.0, e.pos[1] - here.1];
            let d = to[0].hypot(to[1]).max(0.1);
            let dir = [to[0] / d, to[1] / d];
            let mut range = 55.0;
            if light_s.is_some() && me.energy > 25.0 {
                range = 30.0;
            } else if e.has("lightning") {
                range = 65.0;
            }
            if t >= self.strafe_until {
                if self.rand() < 0.6 {
                    self.strafe_dir = -self.strafe_dir;
                }
                self.strafe_until = t + 0.4 + 0.9 * self.rand();
            }
            let radial = ((d - range) / 20.0).clamp(-1.0, 1.0) * 0.8;
            let mut out = [0.0, 0.0];
            for attempt in 0..2 {
                let s = self.strafe_dir;
                let v = norm2([-dir[1] * s + dir[0] * radial, dir[0] * s + dir[1] * radial]);
                if self.w.walkable_line(here, (here.0 + v[0] * 4.0, here.1 + v[1] * 4.0)) && in_bounds(&[here.0 + v[0] * 4.0, here.1 + v[1] * 4.0]) {
                    out = v;
                    break;
                }
                if attempt == 0 {
                    self.strafe_dir = -self.strafe_dir;
                }
            }
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
        } else {
            // Idle: supply drop, cubes, weapons, crates, then explore inside the next circle.
            let mut goal: Option<(f32, (f32, f32))> = None;
            let consider = |score: f32, p: &[f32; 3], goal: &mut Option<(f32, (f32, f32))>| {
                if goal.map_or(true, |g| score < g.0) {
                    *goal = Some((score, (p[0], p[1])));
                }
            };
            if let Some(ds) = obs["drops"].as_array() {
                for d in ds {
                    let p = v3(&d["pos"]);
                    let dist = dxy(&p, &me.pos);
                    if in_bounds(&p) && dist < 300.0 && f(&d["lands_in"]) < dist / me.walk + 10.0 {
                        consider(dist * 0.4, &p, &mut goal);
                    }
                }
            }
            for it in self.cubes.values() {
                if in_bounds(&it.pos) && (it.pos[2] - me.pos[2]).abs() < 6.0 {
                    consider(dxy(&it.pos, &me.pos) * 0.5, &it.pos, &mut goal);
                }
            }
            let want_laser = laser_s.is_none();
            let slot_free = me.wpn[1].is_none() || me.wpn[0].is_none();
            for it in self.pickups.values() {
                let wanted = (slot_free && (it.kind == "laser" || it.kind == "lightning") && slot_of(&it.kind).is_none())
                    || (!slot_free && want_laser && it.kind == "laser" && light_s.is_some());
                if wanted && in_bounds(&it.pos) && (it.pos[2] - self.w.surf_at(it.pos[0], it.pos[1])).abs() < 2.0 {
                    let pref = if it.kind == "laser" { 0.6 } else { 0.8 };
                    consider(dxy(&it.pos, &me.pos) * pref, &it.pos, &mut goal);
                }
            }
            if hp_frac > 0.85 {
                for it in self.crates.values() {
                    if in_bounds(&it.pos) {
                        consider(dxy(&it.pos, &me.pos) * 1.0 + 10.0, &it.pos, &mut goal);
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
                    goal = Some((0.0, here));
                }
            }
            match goal {
                Some((_, g)) if g == here => mv = Mv::Hold,
                Some((_, g)) => mv = Mv::Goto(g, false),
                None => {
                    if !inside_next {
                        mv = Mv::Goto(zone_point, false);
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

        let (mut mvec, mut boost) = match mv {
            Mv::Goto(g, b) => (self.follow(&me, g, t), b),
            Mv::Direct(v, b) => (v, b),
            Mv::Hold => ([0.0, 0.0], false),
        };
        let mut jump = false;

        // ---- stuck detection (only when we mean to travel)
        let travelling = matches!(mv, Mv::Goto(..)) && mvec[0].hypot(mvec[1]) > 0.6;
        if !travelling || t < self.unstick_until {
            self.stuck_pos = me.pos;
            self.stuck_t = t;
        } else if t - self.stuck_t >= 1.0 {
            if d3(me.pos, self.stuck_pos) < 0.8 {
                self.unstick_until = t + 0.7;
                self.unstick_dir = self.rand() * TAU;
                self.path.clear();
                jump = true;
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
            let mut p = c0;
            for _ in 0..4 {
                // Plain linear lead: it scored best against recorded opponent movement at every range.
                let tt = d3(muzzle, p) / speed + 0.05;
                p = [c0[0] + e.vel[0] * tt, c0[1] + e.vel[1] * tt, c0[2] + (e.vel[2] * tt).clamp(-2.0, 2.0)];
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
        } else if let Some(c) = shoot_crate {
            aim = aim_to(muzzle, c);
            let dd = dxy(&c, &muzzle).max(1.0);
            let on = wrap(aim[0] - me.aim[0]).abs() * dd < 1.0 && (aim[1] - me.aim[1]).abs() * dd < 0.9;
            if let Some(s) = gun_s {
                let w = me.wpn[s].as_ref().unwrap();
                if on && w.ammo > 0.0 && w.reload <= 0.0 {
                    fire[s] = true;
                }
            }
        } else {
            // Scan: sweep around the direction of travel, or spin when standing.
            if mvec[0].hypot(mvec[1]) > 0.3 {
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
