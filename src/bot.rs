//! Decision logic: one call per observation, returns the action as JSON.
use crate::nav::Nav;
use crate::world::World;
use serde_json::{json, Map, Value};
use std::collections::HashMap;
use std::sync::Arc;
use std::f32::consts::{PI, TAU};

#[path = "practice.rs"]
mod practice;
#[path = "cover.rs"]
mod cover;
#[path = "salts.rs"]
mod salts;

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
    /// Damage at full charge after upgrades (laser), from the weapon's stats.
    damage: f32,
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
    /// A point in the upper body: 7.5 m up on a 9 m mech.
    fn upper(&self) -> [f32; 3] {
        [self.pos[0], self.pos[1], self.pos[2] + 0.83 * self.height()]
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
    pub tune: [f32; 24],
}

impl Cfg {
    /// range, retreat hp, retreat hp when outnumbered, retreat in squad mode, hostile bonus,
    /// hp weight, strafe base, strafe spread, power ratio needed to accept a duel when cautious,
    /// seconds before assuming practice, cornered distance, the same duel ratio while the match type is unknown
    pub const TUNE: [f32; 24] = [55.0, 0.40, 0.75, 0.0, 40.0, 0.08, 0.45, 0.5, 1.3, 8.0, 40.0, 1.3, 0.0, 0.0, 0.0, 0.0, 0.0, 14.0, 0.0, 0.0, 1.0, 0.7, 14.0, 0.0];
    pub fn from_env() -> Cfg {
        let mut tune = Self::TUNE;
        if let Ok(e) = std::env::var("MB_TUNE") {
            for (i, x) in e.split(',').enumerate().take(24) {
                tune[i] = x.parse().unwrap_or(tune[i]);
            }
        }
        let num = |k: &str| std::env::var(k).ok().and_then(|v| v.parse().ok()).unwrap_or(0u8);
        Cfg { mode: num("MB_MODE"), policy: num("MB_POLICY"), tune }
    }
}

/// Laser and lightning fix knobs. Defaults ship; `FX_TUNE=a,b,...` overrides them in the simulator.
/// 0: hold guard on (1) / off (0)   1: longest hold past full charge (s)   2: target in our line of
/// fire this long before release (s)   3: lightning energy reserve (no boost below this)
/// 4: range (m) of the nearest enemy that triggers the reserve   5: opening length (s)
/// 6: energy floor in the opening (free-for-all and unknown mode)   7: longest open-ground stretch (m)
/// a weapon rush may cross (0 = no limit)   8: dodge charges aimed at us   10: break away after a
/// laser hit from range (off: -0.06 in sim)   11: back off from lightning when we hold a laser
/// (both modes)   12: dodge and break away in practice too
/// 13: break-away time (s)   14: after a dodge, stay hidden until that charge is spent
/// 15: no weapon rush outside confirmed practice (1). Every showdown build with the free-for-all
///     rush (b788b9c on) did badly (5.0 to 5.8 points); the one without it scored 12.2.
/// Early laser release (all modes): let go as soon as the charge so far already does the target's
/// visible HP plus `LASER_EARLY_MARGIN`. Set to false to switch it off (or `MB_NOLASEREARLY=1`).
const LASER_EARLY: bool = true;
const LASER_EARLY_MARGIN: f32 = 1.10;
fn laser_early_on() -> bool {
    static K: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *K.get_or_init(|| LASER_EARLY && std::env::var("MB_NOLASEREARLY").is_err())
}

fn fx(i: usize) -> f32 {
    static K: std::sync::OnceLock<[f32; 16]> = std::sync::OnceLock::new();
    K.get_or_init(|| {
        let mut k = [1.0, 0.3, 0.3, 30.0, 60.0, 20.0, 60.0, 40.0, 1.0, 0.0, 0.0, 1.0, 0.0, 3.0, 1.0, 1.0];
        if let Ok(e) = std::env::var("FX_TUNE") {
            for (j, x) in e.split(',').enumerate().take(16) {
                k[j] = x.parse().unwrap_or(k[j]);
            }
        }
        k
    })[i]
}

/// Practice fixes: bug fixes and plain improvements that were first shipped for confirmed
/// free-for-alls only, so that the practice score would not be re-rolled. Each constant switches
/// one of them on outside mode 2 as well (modes 0 and 1); `false` restores the older practice
/// code path. They never change mode 2.
/// - timed hold: time a full laser charge ourselves (the server reports charge only up to full
///   and fires on its own 2 s later), let go after a short hold instead of crawling until then.
const PF_TIMED_HOLD: bool = true;
/// - jet out of an enclosed pocket when the zone point cannot be walked to.
const PF_JET_POCKET: bool = true;
/// - zone margin: stay 30 m inside the next circle (was 20) and treat the last 20 m before the
///   current edge as urgent (was 10).
const PF_ZONE_MARGIN: bool = true;
/// - a lone drone hitting us at 20% HP or less: walk out of its reach instead of holding still.
const PF_DRONE_FLEE: bool = true;
/// - cover inside a known drone's reach (50 m) is not cover.
const PF_DRONE_COVER: bool = true;
/// - cover test against a shooter on a roof: it sidesteps along its own roof, at its height.
const PF_ROOF_SIDE: bool = true;
/// - with a laser in hand, prefer a mortar carrier as the target (the laser does 60% more to it).
const PF_MORTAR_FIRST: bool = true;
/// - back out of a lightning holder's 45 m whenever we hold no lightning (practice did so only
///   while holding a laser). Off: it lost 0.10 points in simulated practice.
const PF_LIGHT_BACK: bool = false;
/// - step behind cover within reach when a laser charge is aimed at us. Off: no gain in
///   simulated practice.
const PF_DODGE: bool = false;
const PF: [bool; 9] = [PF_TIMED_HOLD, PF_JET_POCKET, PF_ZONE_MARGIN, PF_DRONE_FLEE, PF_DRONE_COVER, PF_ROOF_SIDE, PF_MORTAR_FIRST, PF_LIGHT_BACK, PF_DODGE];
/// Is practice fix `i` on? `MB_PF=<bit mask>` overrides the constants in the simulator (0 = all off).
pub(crate) fn pf(i: usize) -> bool {
    static K: std::sync::OnceLock<u32> = std::sync::OnceLock::new();
    let m = *K.get_or_init(|| {
        std::env::var("MB_PF").ok().and_then(|v| v.parse().ok()).unwrap_or_else(|| PF.iter().enumerate().fold(0u32, |a, (i, on)| a | ((*on as u32) << i)))
    });
    (m >> i) & 1 == 1
}

/// Fire rate (modes 0 and 1 only; a confirmed free-for-all is unchanged). Each constant removes one
/// thing that kept a ready weapon from firing; `false` restores the older behaviour.
/// - gun gate: fire when the aim is within 3.0 m of the lead point (was 2.2 m; the body is 2.5 m
///   wide and the gun was ready but silent in 5% of fight time for want of the last metre).
///   Off: 2% more shots, no more damage, and 0.13 points lower in simulated practice.
const FR_GUN_GATE: bool = false;
/// - upper body: when a mech's centre is behind cover but its upper body is in our line of fire
///   (a shooter back from a roof edge, a low wall), shoot at the upper body instead of not at all.
const FR_TOP_AIM: bool = true;
/// - late switch: move the aim from the gun's lead point to the laser's direct line 0.15 s before
///   the charge is full (was 0.3 s; the torso needs one decision to get there), so the gun keeps
///   firing longer. Off: slightly less damage and 0.05 points lower in simulated practice.
const FR_LATE_DIRECT: bool = false;
/// - long hold: keep a full charge for up to 1.5 s (was 0.3 s) while the mech it is meant for is
///   tracked behind cover, instead of firing it into the wall and waiting out the cooldown.
const FR_LONG_HOLD: bool = true;
const FR: [bool; 4] = [FR_GUN_GATE, FR_TOP_AIM, FR_LATE_DIRECT, FR_LONG_HOLD];
/// Is fire-rate change `i` on? `MB_FR=<bit mask>` overrides the constants in the simulator.
fn fr(i: usize) -> bool {
    static K: std::sync::OnceLock<u32> = std::sync::OnceLock::new();
    let m = *K.get_or_init(|| {
        std::env::var("MB_FR").ok().and_then(|v| v.parse().ok()).unwrap_or_else(|| FR.iter().enumerate().fold(0u32, |a, (i, on)| a | ((*on as u32) << i)))
    });
    (m >> i) & 1 == 1
}

/// How long a match that looks like practice is still treated as possibly a free-for-all.
pub(crate) fn p_lite_until() -> f32 {
    static K: std::sync::OnceLock<f32> = std::sync::OnceLock::new();
    *K.get_or_init(|| std::env::var("MB_PLITE").ok().and_then(|v| v.parse().ok()).unwrap_or(16.0))
}

fn first_shot_on() -> bool {
    static K: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *K.get_or_init(|| std::env::var("MB_NOFIRST").is_err())
}

fn regen_hold_on() -> bool {
    static K: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *K.get_or_init(|| std::env::var("MB_NOREGENHOLD").is_err())
}

/// Lightning against a laser carrier (confirmed free-for-all): close in instead of running.
fn arc_rush_on() -> bool {
    static K: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *K.get_or_init(|| std::env::var("MB_NOARC").is_err())
}

/// Arc-rush knobs (`MB_ARC=a,b,...` in the simulator). 0: longest run-in (m)   1: HP we want left
/// on arrival   2: their HP as a share of what one tank of lightning plus the gun can deal
/// 3: energy we want left on arrival
fn arc_knob(i: usize) -> f32 {
    static K: std::sync::OnceLock<[f32; 4]> = std::sync::OnceLock::new();
    K.get_or_init(|| {
        let mut k = [110.0, 450.0, 0.8, 30.0];
        if let Ok(e) = std::env::var("MB_ARC") {
            for (j, x) in e.split(',').enumerate().take(4) {
                k[j] = x.parse().unwrap_or(k[j]);
            }
        }
        k
    })[i]
}

/// Weapon fire heard from two directions before our first kill marks a free-for-all.
fn crossfire_on() -> bool {
    static K: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    // On, with the revert at `xf_revert_s` (18 s): a free-for-all set by gunfire alone goes back to
    // practice if nobody but us has killed anything by then. Practice scoring now uses fresh seeds,
    // so nothing is fitted to the old fixed matches. `MB_NOXFIRE=1` turns it off (simulator, bed).
    *K.get_or_init(|| std::env::var("MB_NOXFIRE").is_err())
}

/// Discovery risk of standing still at ground spots on an 8 m lattice, row by row from the
/// south-west corner: hundredths of mech-seconds per match (first 180 s) during which a mech in
/// recorded team-bot matches was within 45 m of the spot or had sight of it within 200 m.
/// 65535 marks a spot nobody can stand on. Built offline from the event's public replays.
static HIDE_RISK: &[u8] = include_bytes!("hide_risk.bin");
const HR_NX: usize = 132;
const HR_NY: usize = 89;
const HR_X0: f32 = -501.0;
const HR_Y0: f32 = -322.0;

fn hide_table_on() -> bool {
    static K: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *K.get_or_init(|| std::env::var("MB_NOHIDETAB").is_err())
}

/// Hide-table knobs (`MB_HIDE=a,b,c` in the simulator). 0: risk points one metre of travel costs
/// 1: seconds at a spot before looking for a better one   2: search radius (m)
fn hide_knob(i: usize) -> f32 {
    static K: std::sync::OnceLock<[f32; 3]> = std::sync::OnceLock::new();
    K.get_or_init(|| {
        let mut k = [0.2, 40.0, 150.0];
        if let Ok(e) = std::env::var("MB_HIDE") {
            for (j, x) in e.split(',').enumerate().take(3) {
                k[j] = x.parse().unwrap_or(k[j]);
            }
        }
        k
    })[i]
}

/// Fetch knobs (`MB_FETCH=a,b,c,d`): 0 reach (m), 1 open-street limit (m, 0 = none),
/// 2 earliest time (s), 3 contested margin (m).
fn fetch_knob(i: usize) -> f32 {
    static K: std::sync::OnceLock<[f32; 4]> = std::sync::OnceLock::new();
    K.get_or_init(|| {
        let mut k = [130.0, 40.0, 10.0, 0.0];
        if let Ok(e) = std::env::var("MB_FETCH") {
            for (j, x) in e.split(',').enumerate().take(4) {
                k[j] = x.parse().unwrap_or(k[j]);
            }
        }
        k
    })[i]
}

fn fetch_on() -> bool {
    static K: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *K.get_or_init(|| std::env::var("MB_NOFETCH").is_err())
}

fn wounded_hold_on() -> bool {
    static K: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *K.get_or_init(|| std::env::var("MB_NOWOUNDHOLD").is_err())
}

/// Opening safety (modes 0 and guarded 1): changes practice play, so it ships only on salted builds.
pub(crate) fn opening_safe_on() -> bool {
    static K: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *K.get_or_init(|| std::env::var("MB_NOOPENSAFE").is_err())
}

fn counter_brk_on() -> bool {
    static K: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *K.get_or_init(|| std::env::var("MB_NOCOUNTERBRK").is_err())
}

/// Free-for-all: no first shot at a snowballer (10+ cubes) that has not hit us (`MB_NOSNOW=1` turns it off).
fn snow_veto_on() -> bool {
    static K: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *K.get_or_init(|| std::env::var("MB_NOSNOW").is_err())
}

/// Free-for-all: strict cover also holds when a roof shooter sidesteps along its roof
/// (`MB_NOROOFSIDE=1` turns it off).
fn roof_side_on() -> bool {
    static K: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *K.get_or_init(|| std::env::var("MB_NOROOFSIDE").is_err())
}

pub(crate) fn finish_on() -> bool {
    static K: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *K.get_or_init(|| std::env::var("MB_NOFINISH").is_err())
}

/// Hold range in a fight we started (confirmed free-for-all only). On by default;
/// `MB_NOHOLDRANGE=1` switches it off.
fn hold_range_on() -> bool {
    static K: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *K.get_or_init(|| std::env::var("MB_NOHOLDRANGE").is_err())
}

/// Leave a gun fight against a laser holder that hit us first (confirmed free-for-all only).
/// On by default; `MB_NOLEAVE=1` switches it off.
fn leave_on() -> bool {
    static K: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *K.get_or_init(|| std::env::var("MB_NOLEAVE").is_err())
}

/// Test log of fights on stderr (`MB_FIGHTLOG=1`); changes no decision.
fn fight_log() -> bool {
    static K: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *K.get_or_init(|| std::env::var("MB_FIGHTLOG").is_ok())
}

/// `MB_XF2=sectors,seconds` (0 sectors = off, the default).
fn crossfire2() -> (usize, f32) {
    static K: std::sync::OnceLock<(usize, f32)> = std::sync::OnceLock::new();
    *K.get_or_init(|| {
        let v: Vec<f32> = std::env::var("MB_XF2").ok().map(|e| e.split(',').filter_map(|x| x.parse().ok()).collect()).unwrap_or_default();
        if v.len() == 2 { (v[0] as usize, v[1]) } else { (XF2_SECTORS, XF2_SECONDS) }
    })
}
const XF2_SECTORS: usize = 0;
const XF2_SECONDS: f32 = 12.0;

/// Free-for-all: keep 60 m from supply drops we are not taking (`MB_NODROPAWAY=1` turns it off).
fn drop_away_on() -> bool {
    static K: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *K.get_or_init(|| std::env::var("MB_NODROPAWAY").is_err())
}

/// Free-for-all: laser the mortar carrier first (`MB_NOMORTARFIRST=1` turns it off).
fn mortar_first_on() -> bool {
    static K: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *K.get_or_init(|| std::env::var("MB_NOMORTARFIRST").is_err())
}

/// Free-for-all: near a laser holder, move only along cover (`MB_NOCOVERRULE=1` turns it off).
fn cover_rule_on() -> bool {
    static K: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *K.get_or_init(|| std::env::var("MB_NOCOVERRULE").is_err())
}

/// Longest stretch (m) of the straight route a -> b with no building within 8 m.
fn open_run_on(w: &World, a: (f32, f32), b: (f32, f32)) -> f32 {
    let len = (b.0 - a.0).hypot(b.1 - a.1);
    let n = (len / 4.0).ceil().max(1.0) as i32;
    let (mut run, mut best) = (0.0f32, 0.0f32);
    for k in 0..=n {
        let f = k as f32 / n as f32;
        let (x, y) = (a.0 + (b.0 - a.0) * f, a.1 + (b.1 - a.1) * f);
        let mut near = false;
        'probe: for r in [0.0f32, 4.0, 8.0] {
            for q in 0..8 {
                let ang = q as f32 * std::f32::consts::FRAC_PI_4;
                if w.prism[w.cell(x + r * ang.cos(), y + r * ang.sin())] {
                    near = true;
                    break 'probe;
                }
                if r == 0.0 {
                    break;
                }
            }
        }
        if near {
            run = 0.0;
        } else {
            run += len / n as f32;
            best = best.max(run);
        }
    }
    best
}

/// Free-for-all rules from the recorded showdowns (each only in a confirmed free-for-all, mode 2;
/// set a constant to false to switch it off):
/// - supply drops: go when at most 2 tracked mechs are within 100 m of the drop and our HP is at
///   least 50% (instead of nobody within 100 m). Ring entries died within 15 s 8% of the time
///   against a 9-13% baseline; loitering 8-40 m away 25%.
/// - first-shot guard: no first shot at a mech holding a laser unless we hold a laser too (gun or
///   lightning against a laser: kills 23% against 22%, 22% against 17%).
/// - laser duel floor: when both of us hold a laser, no first shot from under 70 m (won 61% under
///   70 m, 71% at 70-100 m, 75% over 100 m).
const FFA_DROP_RULE: bool = true;
const FFA_LASER_GUARD: bool = true;
const FFA_LASER_FLOOR: bool = true;

/// First-shot knobs (`MB_FS=a,b,...`): 0 earliest time (s), 1 our HP share, 2 longest range (m),
/// 3 longest range without a ready laser (m), 4 their power as a multiple of ours, 5 how far off
/// us the target must be looking (rad).
fn first_shot_knobs() -> [f32; 6] {
    static K: std::sync::OnceLock<[f32; 6]> = std::sync::OnceLock::new();
    *K.get_or_init(|| {
        let mut k = [20.0, 0.75, 220.0, 150.0, 1.3, 1.3];
        if let Ok(e) = std::env::var("MB_FS") {
            for (j, x) in e.split(',').enumerate().take(6) {
                k[j] = x.parse().unwrap_or(k[j]);
            }
        }
        k
    })
}

/// Last-four rules in a free-for-all (`MB_NOLAST4=1` turns them off): no gun-against-laser trades,
/// an earlier move into the next circle, and a full look around while walking (three of our
/// last-four deaths were hits from behind).
fn last4_on() -> bool {
    static K: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *K.get_or_init(|| std::env::var("MB_NOLAST4").is_err())
}

/// Seconds after which a free-for-all guessed from gunfire alone is undone if nobody but
/// us has killed anything (`MB_XFREVERT`, 0 = never).
fn xf_revert_s() -> f32 {
    static K: std::sync::OnceLock<f32> = std::sync::OnceLock::new();
    *K.get_or_init(|| std::env::var("MB_XFREVERT").ok().and_then(|v| v.parse().ok()).unwrap_or(18.0))
}

/// Free-for-all evidence from damage seen on other mechs (`MB_FD=1` turns it on; off by default
/// until a practice submission repeats the pick's score exactly).
fn foreign_damage_on() -> bool {
    static K: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *K.get_or_init(|| std::env::var("MB_NOFD").is_err())
}

fn flip_slack() -> bool {
    static K: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *K.get_or_init(|| std::env::var("MB_NOFLIP").is_err())
}

/// Per-group practice switches (`salts::VARIANT`, one bit mask per salt group). All are practice
/// (mode 1) only. `MB_VARIANT=n` overrides the mask for every group (simulator and self-test);
/// the scorer sets no environment, so the masks in `salts.rs` are what runs there.
const V_LHUNT: u16 = 1;
const V_POLICY2: u16 = 2;
const V_HEAL: u16 = 4;
const V_RANGE100: u16 = 8;
/// Guarded opening off (practice mode 1 only; it also reaches the first seconds of a free-for-all
/// that is still read as practice).
const V_PLITE0: u16 = 16;
const V_RANGE40: u16 = 32;
const V_RANGE150: u16 = 64;
const V_FOLLOW55: u16 = 128;
const V_SHAREDUP: u16 = 256;
const V_HEAL_ANY: u16 = 512;
/// Regroup in the lulls: hurt, nobody has had a line on us for 3 s and nothing is hitting us:
/// go to cover and hold all fire until HP is back to 90% or a mech shows up.
const V_REGROUP: u16 = 1024;
/// Straight, boosted run to the nearest special weapon from the first second (modes 0 and 1:
/// it acts before the match type is known, so it also plays in free-for-alls of that group).
const V_RUN0: u16 = 2048;
/// Practice opening: wait for the first hunter at a low-exposure spot, not on a random walk.
const V_COVERSPOT: u16 = 4096;
/// Sidestep timing: reversal floor of the bullet's flight time, one reversal on a laser charge
/// aimed at us, no jump under fire.
const V_SIDESTEP: u16 = 8192;
/// Eager farming: crates and lone drones are shot whenever no hunter is in view, from 40% HP.
const V_EAGER: u16 = 16384;
/// Opening safety off (modes 0 and 1 only): no break-away after a laser hit from range and no limit on
/// weapon walks across open streets before the match type is known; the practice play of the builds
/// before the opening-safety commit. Never acts in confirmed free-for-all.
const V_OPEN_OFF: u16 = 32768;

/// Late-game practice switches, all off (practice mode only). `MB_PG=134` turns on 1, 3 and 4 in
/// the simulator.
/// 1 `PG_DROP_REPAIR`: HP below 80% and a supply drop landed or landing within 200 m inside the
///   zone: go and hold its ring for the capture before anything else, shooting as usual.
/// 2 `PG_REST_AFTER5`: from the fifth kill, nothing in view for 3 s and HP below 85%: cover, all
///   fire held, until HP is above 95% or a mech shows up.
/// 3 `PG_CUBE_STOP`: with ten cubes, no more walks to cubes, no crates, no drone farming.
/// 4 `PG_HOLD_FOR_HUNTER`: holding a laser, a hunter coming at us from beyond 150 m: wait near
///   cover and let it come, charging as it enters range.
const PG_DROP_REPAIR: bool = true;
const PG_REST_AFTER5: bool = true;
const PG_CUBE_STOP: bool = true;
const PG_HOLD_FOR_HUNTER: bool = false;
/// Kills needed before PG_REST_AFTER5 acts (5; `MB_PGK` overrides it in the simulator).
fn pg_kills() -> f32 {
    static K: std::sync::OnceLock<f32> = std::sync::OnceLock::new();
    *K.get_or_init(|| std::env::var("MB_PGK").ok().and_then(|v| v.parse().ok()).unwrap_or(5.0))
}
fn pg(i: usize) -> bool {
    static K: std::sync::OnceLock<[bool; 4]> = std::sync::OnceLock::new();
    K.get_or_init(|| {
        let e = std::env::var("MB_PG").unwrap_or_default();
        [PG_DROP_REPAIR || e.contains('1'), PG_REST_AFTER5 || e.contains('2'), PG_CUBE_STOP || e.contains('3'), PG_HOLD_FOR_HUNTER || e.contains('4')]
    })[i]
}

/// Practice design (private/FULL_SET_GAPS.md). 0 = the behaviour of the build before design B;
/// 1 = design B, one coherent practice design, practice (mode 1) only plus the opening weapon run:
/// 1 weapon run from second 0 (strong laser preference, short reach in a fight, 12 s give-up);
/// 2 fight range by weapon (gun 80 m, laser 130 m) with a finishing exemption;
/// 3 restraint: no first shot at a bot that has not attacked or approached us while we hold only
///   the gun (until 2 minutes), hunters first, the one hurting us, stay on it, the nearly dead;
/// 4 retreat and heal: below 40% HP, or below two thirds with two or more hunters on us, hold fire,
///   back at 80%; never while a fight is nearly won;
/// 5 errands between kills in a fixed order. (The early laser release is its own switch.)
/// `MB_DESIGN=1` switches it on in the simulator; `MB_DBOFF=34` switches items off (ablation);
/// `DESIGN_B_RETREAT` below (false) leaves item 4 out.
pub const PRACTICE_DESIGN: u8 = 0;
fn dboff(n: u8) -> bool {
    static K: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    (n == 4 && !DESIGN_B_RETREAT) || K.get_or_init(|| std::env::var("MB_DBOFF").unwrap_or_default()).contains((b'0' + n) as char)
}
/// Design B item 4 (retreat and heal), off inside design B: in the simulator it costs 0.3 to 0.6
/// points; true switches it on.
const DESIGN_B_RETREAT: bool = false;
/// Design B thresholds (`DB_TUNE=a,b,...` in the simulator): 0 leave below this HP fraction,
/// 1 leave below this with two hunters on us (0 = never), 2 return at this fraction, 3 restraint
/// ends at this time (s), 4 harmless-bot reach with a laser (m), 5 gun range, 6 laser range.
fn dbk(i: usize) -> f32 {
    static K: std::sync::OnceLock<[f32; 7]> = std::sync::OnceLock::new();
    K.get_or_init(|| {
        let mut k = [0.40, 0.67, 0.8, 120.0, 160.0, 80.0, 130.0];
        if let Ok(e) = std::env::var("DB_TUNE") {
            for (j, x) in e.split(',').enumerate().take(7) {
                k[j] = x.parse().unwrap_or(k[j]);
            }
        }
        k
    })[i]
}
fn design_b() -> bool {
    static K: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *K.get_or_init(|| PRACTICE_DESIGN == 1 || std::env::var("MB_DESIGN").map_or(false, |v| v == "1"))
}

fn variant_env() -> Option<u16> {
    static K: std::sync::OnceLock<Option<u16>> = std::sync::OnceLock::new();
    *K.get_or_init(|| std::env::var("MB_VARIANT").ok().and_then(|v| v.parse().ok()))
}

fn variant_log_on() -> bool {
    static K: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *K.get_or_init(|| std::env::var("MB_VARLOG").is_ok())
}

fn scav_on() -> bool {
    static K: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *K.get_or_init(|| std::env::var("MB_NOSCAV").is_err())
}

/// Mixed into the seed of our own random choices (strafe reversals, wander points). Zero keeps
/// the plain seed; other values only reshuffle those choices. This seeds the stream until the
/// first observation (no draws happen before it); from then on the match group's entry in
/// `salts::SALTS` does. Every entry equal to this value plays exactly like the single-salt build.
const RNG_SALT: u64 = 3;

fn rng_seed(seed: u64, salt: u64) -> u64 {
    (seed ^ salt.wrapping_mul(0xD1B54A32D192ED03)).wrapping_mul(0x9E3779B97F4A7C15) | 1
}

/// splitmix64 finaliser: a fixed, version-independent hash step for the match key.
fn mix(mut z: u64) -> u64 {
    z = z.wrapping_add(0x9E3779B97F4A7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58476D1CE4E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D049BB133111EB);
    z ^ (z >> 31)
}

pub struct Bot {
    w: Arc<World>,
    nav: Nav,
    rng: u64,
    /// `hello.seed`, `hello.slot`, whether the match group's salt is applied, its group, and how
    /// many random draws were made (to check that none come before the salt).
    seed0: u64,
    slot: i64,
    salted: bool,
    salt_group: i32,
    variant: u16,
    variant_seen: std::cell::Cell<u16>,
    regroup: bool,
    /// PG_HOLD_FOR_HUNTER: the waiting spot, when it was chosen, and for which hunter.
    hfh_cover: Option<(f32, f32)>,
    hfh_t: f32,
    hfh_id: i64,
    watched_t: f32,
    run0_id: i64,
    run0_t0: f32,
    side_id: i64,
    side_t: f32,
    strafe_floor: f32,
    /// Design B: when an enemy began closing on us, the target we stay on, whether no cover was
    /// found for the retreat, and the current errand.
    close_since: HashMap<i64, f32>,
    db_target: i64,
    db_nocover: bool,
    errand_id: i64,
    errand_t0: f32,
    rng_draws: u32,
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
    /// Jetting out of a spot we cannot walk out of (free-for-all), and until when.
    escape_until: f32,
    /// Replay marker (look straight up for 0.4 s once, on confirmed free-for-all): when it ran.
    marker_t: f32,
    /// Damage seen on other mechs that we did not deal (free-for-all evidence).
    fd_cand: Vec<(i64, f32)>,
    fd_small: Vec<f32>,
    fd_big: bool,
    fd_logged: bool,
    my_hit_t: HashMap<i64, f32>,
    /// Free-for-all set by the volume rule alone (when), and whether that was undone.
    xf_t: f32,
    xf_off: bool,
    /// Free-for-all: landed or held supply drops we are not taking (keep 60 m away), and the drop
    /// we are walking to, if any.
    drop_avoid: Vec<[f32; 2]>,
    drop_goal: i64,
    /// Free-for-all: longest stretch of open ground (m) a goal's route may cross; infinite unless
    /// a laser holder is within 140 m (LASER.md rule 2).
    cover_lim: f32,
    /// When our laser charge reached full (the reported charge stops at full, so how long it has
    /// been held must be timed here), or -1.
    full_since: f32,
    climb_goal: (f32, f32),
    blacklist: HashMap<i64, f32>,
    policy: u8,
    tune: [f32; 24],
    kills: i64,
    ghost_t: f32,
    t0: f32,
    /// Reload cover: until when, where to hide, where we were, and whom we hide from.
    peek_until: f32,
    peek_cover: Option<(f32, f32)>,
    peek_from: (f32, f32),
    peek_id: i64,
    peek_retry_t: f32,
    /// An ambush we started: see it through unless it goes badly.
    commit_id: i64,
    commit_until: f32,
    /// The laser carrier we are closing on with lightning, and until when.
    arc_id: i64,
    arc_until: f32,
    /// The mech we opened fire on (it had not hit us in the 30 s before we took aim), and until
    /// when the fight counts as one we started. Tracked in a confirmed free-for-all only.
    open_id: i64,
    open_until: f32,
    /// Laser holders that hit us first while we held only the gun, and when: no fight with them
    /// until we hold a laser or lightning, or they are wounded. Confirmed free-for-all only.
    shun: HashMap<i64, f32>,
    /// Test log (MB_FIGHTLOG): kind (0 fight we started, 1 hit first), mech, start time, whether
    /// the 5 s line is written, where we stood and the direction to the mech then; and a key that
    /// tells runs apart.
    flog: Vec<(u8, i64, f32, bool, [f32; 2], [f32; 2])>,
    flog_key: String,
    /// The target we have had a continuous line of fire to, since when, and when last.
    los_id: i64,
    los_since: f32,
    los_last: f32,
    /// Break-away from a laser that hit us first at range, and the last lightning that hit us.
    brk_id: i64,
    brk_until: f32,
    zapped_by: i64,
    zapped_t: f32,
    dodge_id: i64,
    dodge_until: f32,
    /// Places we failed to reach (stuck twice, or a climb that kept failing): keep away for a while.
    avoid: Vec<((f32, f32), f32)>,
    stuck_goal: (f32, f32),
    stuck_goal_t: f32,
    stuck_count: u8,
    climb_tries: u8,
    climb_try_goal: (f32, f32),
    climb_block_until: f32,
    /// Deaths we heard: estimated place and time. A death leaves cubes and weapons behind.
    scav: Vec<([f32; 2], f32)>,
    /// Bearing and time of weapon fire heard from mechs that are not on us.
    fire_heard: Vec<(f32, f32)>,
    scav_goal: Option<([f32; 2], f32)>,
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
            rng: rng_seed(seed, RNG_SALT),
            seed0: seed,
            slot: hello["slot"].as_i64().unwrap_or(-1),
            salted: false,
            salt_group: -1,
            variant: 0,
            variant_seen: std::cell::Cell::new(0),
            regroup: false,
            hfh_cover: None,
            hfh_t: -100.0,
            hfh_id: -1,
            watched_t: -100.0,
            run0_id: -1,
            run0_t0: -100.0,
            side_id: -1,
            side_t: -100.0,
            strafe_floor: 0.0,
            close_since: HashMap::new(),
            db_target: -1,
            db_nocover: false,
            errand_id: -1,
            errand_t0: -100.0,
            rng_draws: 0,
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
            escape_until: -1.0,
            marker_t: -1.0,
            fd_cand: Vec::new(),
            fd_small: Vec::new(),
            fd_big: false,
            fd_logged: false,
            my_hit_t: HashMap::new(),
            xf_t: -1.0,
            xf_off: false,
            drop_avoid: Vec::new(),
            drop_goal: -1,
            cover_lim: f32::INFINITY,
            full_since: -1.0,
            climb_goal: (0.0, 0.0),
            blacklist: HashMap::new(),
            policy: cfg.policy,
            tune: cfg.tune,
            cfg_mode: cfg.mode,
            mode: if cfg.mode == 0 { 0 } else { cfg.mode },
            kills: 0,
            ghost_t: -100.0,
            t0: -1.0,
            peek_until: -1.0,
            peek_cover: None,
            peek_from: (0.0, 0.0),
            peek_id: -1,
            peek_retry_t: -100.0,
            commit_id: -1,
            commit_until: -1.0,
            arc_id: -1,
            arc_until: -1.0,
            open_id: -1,
            open_until: -1.0,
            shun: HashMap::new(),
            flog: Vec::new(),
            flog_key: String::new(),
            los_id: -1,
            los_since: 0.0,
            los_last: -100.0,
            brk_id: -1,
            brk_until: -1.0,
            zapped_by: -1,
            zapped_t: -100.0,
            dodge_id: -1,
            dodge_until: -1.0,
            avoid: Vec::new(),
            stuck_goal: (0.0, 0.0),
            stuck_goal_t: -100.0,
            stuck_count: 0,
            climb_tries: 0,
            climb_try_goal: (0.0, 0.0),
            climb_block_until: -1.0,
            scav: Vec::new(),
            fire_heard: Vec::new(),
            scav_goal: None,
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

    /// Practice: per-match-group RNG salts. Practice matches are fixed and scored deterministically,
    /// so each group of matches can get its own salt (`salts::SALTS`, chosen by submission testing).
    /// The key uses only what the server fixes per match before we act: our slot and the first
    /// next-circle centre (rounded to 1 m). Runs once, on the first observation, before any draw.
    fn apply_salt(&mut self, obs: &Value) {
        self.salted = true;
        let nc = &obs["zone"]["next_center"];
        let key = match (nc[0].as_f64(), nc[1].as_f64()) {
            (Some(x), Some(y)) => mix(mix(mix(self.slot as u64) ^ x.round() as i64 as u64) ^ y.round() as i64 as u64),
            _ => mix(self.slot as u64),
        };
        let g = (key % salts::SALTS.len() as u64) as usize;
        self.salt_group = g as i32;
        self.variant = variant_env().unwrap_or(salts::VARIANT[g % 8]);
        self.rng = rng_seed(self.seed0, salts::SALTS[g]);
        eprintln!("salt group {} slot {} next centre {} draws before {}", g, self.slot, nc, self.rng_draws);
    }

    /// Is practice switch `bit` on for this match's salt group? Only in confirmed practice (mode 1).
    /// With `MB_VARLOG` set, the first read that is on is logged to stderr (self-test).
    fn vsw(&self, bit: u16) -> bool {
        self.vgate(bit, self.mode == 1)
    }

    /// Opening safety (see `opening_safe_on`) unless the group switched it off with V_OPEN_OFF.
    pub(crate) fn open_safe(&self) -> bool {
        opening_safe_on() && !self.vgate(V_OPEN_OFF, self.mode != 2)
    }

    /// Design B in confirmed practice.
    fn db(&self) -> bool {
        design_b() && self.mode == 1
    }

    /// Design B item `n` (see `PRACTICE_DESIGN`) in confirmed practice.
    fn dbi(&self, n: u8) -> bool {
        self.db() && !dboff(n)
    }

    /// Design B, also before the match type is known (the opening weapon run).
    fn db_open(&self) -> bool {
        design_b() && self.mode != 2
    }

    /// Sidestep timing: the V_SIDESTEP switch.
    fn side_on(&self) -> bool {
        self.vsw(V_SIDESTEP)
    }

    /// Same, with the caller deciding which modes the switch may act in (`ok`).
    fn vgate(&self, bit: u16, ok: bool) -> bool {
        let on = ok && self.variant & bit != 0;
        if on && variant_log_on() && self.variant_seen.get() & bit == 0 {
            self.variant_seen.set(self.variant_seen.get() | bit);
            eprintln!("VARIANT bit {} read as on (group {}, mask {})", bit, self.salt_group, self.variant);
        }
        on
    }

    /// How long a match that looks like practice stays guarded (see `p_lite_until`); 0 with V_PLITE0.
    pub(crate) fn p_lite_t(&self) -> f32 {
        if self.vsw(V_PLITE0) {
            0.0
        } else {
            p_lite_until()
        }
    }

    /// Practice break-and-heal on for this match (V_HEAL, V_HEAL_ANY, or the simulator knob 23).
    fn heal_on(&self) -> bool {
        self.vsw(V_HEAL_ANY) || self.vsw(V_HEAL) || (self.mode == 1 && practice::knob(23) > 0.5)
    }

    fn rand(&mut self) -> f32 {
        self.rng_draws = self.rng_draws.wrapping_add(1);
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
                    damage: ws["damage"].as_f64().unwrap_or(315.0) as f32,
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
                if self.avoid.iter().any(|a| (a.0 .0 - x).hypot(a.0 .1 - y) < 8.0) {
                    continue;
                }
                // Free-for-all: cover inside a known drone's reach is no cover (5 AM: we hid 24 m
                // from a drone and it killed us).
                if (self.mode == 2 || pf(4)) && self.npc_mem.values().any(|n| (n.0[0] - x).hypot(n.0[1] - y) < 50.0) {
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
                            // Free-for-all: a roof shooter sidesteps along its own roof, so test
                            // from there at its height (COUNTERS.md #5: cover picked against a
                            // ground-level eye often fails against a roof sniper).
                            let same_roof = (self.mode == 2 || pf(5)) && roof_side_on() && (w.surf[pc] - e.pos[2]).abs() < 2.0;
                            if w.prism[pc] && !same_roof {
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
    /// The hiding place with the least recorded traffic in reach: low risk in the table, a short
    /// walk, inside this circle and the next, and away from mechs and drones we know about.
    fn pick_hide_risk(&self, me: &Me, z: &Zone, t: f32, rmax: f32) -> Option<(f32, f32)> {
        let w = &self.w;
        if HIDE_RISK.len() < 2 * HR_NX * HR_NY {
            return None;
        }
        let on_roof = w.prism[w.cell(me.pos[0], me.pos[1])];
        let margin = 20f32.min(0.3 * z.nr);
        let mut best: Option<(f32, (f32, f32))> = None;
        for iy in 0..HR_NY {
            let y = HR_Y0 + 8.0 * iy as f32;
            if (y - me.pos[1]).abs() > rmax {
                continue;
            }
            for ix in 0..HR_NX {
                let x = HR_X0 + 8.0 * ix as f32;
                let k = iy * HR_NX + ix;
                let raw = u16::from_le_bytes([HIDE_RISK[2 * k], HIDE_RISK[2 * k + 1]]);
                let d = (x - me.pos[0]).hypot(y - me.pos[1]);
                if raw == u16::MAX || d > rmax {
                    continue;
                }
                // The inner part of the next circle draws the most traffic mid-match (hit risk 27 to
                // 29% against 16 to 18% at 0.5 to 1.25 of its radius), on top of what the table saw.
                let inner = z.nr > 60.0 && (x - z.nc[0]).hypot(y - z.nc[1]) < 0.45 * z.nr;
                let score = raw as f32 / 100.0 + hide_knob(0) * d + if inner { 40.0 } else { 0.0 };
                if best.map_or(false, |b| score >= b.0) {
                    continue;
                }
                let c = w.cell(x, y);
                if !w.free(c) || w.prism[c] || (!on_roof && (w.surf[c] - me.pos[2]).abs() > 8.0) {
                    continue;
                }
                if (x - z.nc[0]).hypot(y - z.nc[1]) > z.nr - margin || (x - z.c[0]).hypot(y - z.c[1]) > z.r - margin {
                    continue;
                }
                if self.enemies.values().any(|e| t - e.seen < 15.0 && (e.pos[0] - x).hypot(e.pos[1] - y) < 80.0) {
                    continue;
                }
                if self.npc_mem.values().any(|n| (n.0[0] - x).hypot(n.0[1] - y) < 58.0) {
                    continue;
                }
                if self.drop_avoid.iter().any(|q| (q[0] - x).hypot(q[1] - y) < 60.0) {
                    continue;
                }
                if self.cover_lim.is_finite() && self.open_run((me.pos[0], me.pos[1]), (x, y)) > self.cover_lim {
                    continue;
                }
                if self.avoid.iter().any(|a| t < a.1 && (a.0 .0 - x).hypot(a.0 .1 - y) < 14.0) {
                    continue;
                }
                best = Some((score, (x, y)));
            }
        }
        best.map(|b| b.1)
    }

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
                if self.drop_avoid.iter().any(|q| (q[0] - x).hypot(q[1] - y) < 60.0) {
                    continue;
                }
                if self.cover_lim.is_finite() && self.open_run((me.pos[0], me.pos[1]), (x, y)) > self.cover_lim {
                    continue;
                }
                if self.avoid.iter().any(|a| t < a.1 && (a.0 .0 - x).hypot(a.0 .1 - y) < 14.0) {
                    continue;
                }
                let mut score = w.openness(x, y) + 0.35 * d;
                // Optional pulls: toward the rim of the next circle, and away from mechs seen lately.
                if self.tune[16] != 0.0 {
                    score -= self.tune[16] * ((x - z.nc[0]).hypot(y - z.nc[1]) / z.nr.max(1.0)).min(1.0) * 30.0;
                }
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

    /// How much of the target's velocity to lead by. Measured on recorded matches: against mechs
    /// that are fighting (reversing every half second or so), leading by about 0.8 of the
    /// velocity hits more often than full lead at every range; a mech walking a steady line
    /// deserves the full lead.
    fn lead_factor(&self, id: i64, _tof: f32, t: f32) -> f32 {
        let h = match self.hist.get(&id) {
            Some(h) if h.len() >= 6 => h,
            _ => return 0.85,
        };
        let now = h[h.len() - 1];
        let sp = now.2[0].hypot(now.2[1]);
        for s in h.iter().rev() {
            let age = t - s.0;
            if age > 1.3 {
                break;
            }
            if age < 0.25 {
                continue;
            }
            let so = s.2[0].hypot(s.2[1]);
            let dot = now.2[0] * s.2[0] + now.2[1] * s.2[1];
            if (sp < 3.0) != (so < 3.0) || dot < 0.7 * sp * so {
                return 0.78;
            }
        }
        1.0
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

    /// Longest stretch (m) of the straight line a-b with no building within about 8 m.
    fn open_run(&self, a: (f32, f32), b: (f32, f32)) -> f32 {
        open_run_on(&self.w, a, b)
    }

    /// Sideways movement around a point at about `range`, reversing unpredictably and never
    /// stopping while any direction is open. `dir` points at the target, `d` is its distance.
    fn strafe_vec(&mut self, here: (f32, f32), z: f32, dir: [f32; 2], d: f32, range: f32, t: f32, zone: &Zone) -> [f32; 2] {
        if t >= self.strafe_until {
            if self.rand() < 0.85 {
                self.strafe_dir = -self.strafe_dir;
            }
            self.strafe_until = t + (self.tune[6] + self.tune[7] * self.rand()).max(self.strafe_floor);
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
        if !self.salted {
            self.apply_salt(obs);
        }
        let t = f(&obs["t"]);
        self.fd_cand.clear();
        let me = Self::parse_me(&obs["self"]);
        if fight_log() && self.flog_key.is_empty() {
            self.flog_key = format!("{:.0},{:.0}", me.pos[0], me.pos[1]);
        }
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
                if let Some(old) = self.enemies.get(&id) {
                    let hp_now = f(&m["hp"]);
                    if old.seen >= t - 0.16 && hp_now > 0.0 && old.hp - hp_now >= 40.0 {
                        self.fd_cand.push((id, old.hp - hp_now));
                    }
                }
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
                        if fight_log() && self.mode == 2 && self.my_hit_t.get(&id).map_or(true, |h| t - *h > 30.0) {
                            if let Some(en) = self.enemies.get(&id) {
                                if t - en.hurt_me_t > 30.0 {
                                    let has = |k: &str| me.wpn.iter().any(|w| w.as_ref().map_or(false, |w| w.kind == k));
                                    let w = if has("laser") { "laser" } else if has("lightning") { "lightning" } else { "gun" };
                                    let d = dxy(&en.pos, &me.pos);
                                    eprintln!("FIGHT OPEN {} {} {:.1} d={:.0} hp={:.2} thp={:.2} w={} tl={}", self.flog_key, id, t, d, me.hp / me.max_hp.max(1.0), en.hp / en.max_hp.max(1.0), w, en.has("laser") as u8);
                                    self.flog.push((0, id, t, false, [me.pos[0], me.pos[1]], [(en.pos[0] - me.pos[0]) / d.max(1.0), (en.pos[1] - me.pos[1]) / d.max(1.0)]));
                                }
                            }
                        }
                        self.my_hit_t.insert(id, t);
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
                        if e["weapon"].as_str() == Some("lightning") {
                            self.zapped_by = by;
                            self.zapped_t = t;
                        }
                        if let Some(en) = self.enemies.get(&by) {
                            let laser = e["weapon"].as_str() == Some("laser") || en.has("laser");
                            // Confirmed free-for-all: also break away from a mech pushing in on us
                            // with a laser or lightning, and from one carrying 10 or more cubes,
                            // whatever it hit us with. Recorded first hits: targets that broke line
                            // of sight from a fast pusher died 27% of the time against 63 to 67%;
                            // from a 10-cube mech, 36% against 73 to 78%.
                            let d = dxy(&en.pos, &me.pos);
                            let toward = (en.vel[0] * (me.pos[0] - en.pos[0]) + en.vel[1] * (me.pos[1] - en.pos[1])) / d.max(1.0);
                            let pusher = toward >= 3.0 && (en.has("laser") || en.has("lightning")) && d > 45.0 && d < 150.0;
                            let counter = self.mode == 2 && counter_brk_on() && (pusher || en.cubes >= 10.0);
                            if ((laser && d > 60.0) || counter) && t >= self.brk_until {
                                self.brk_id = by;
                                self.brk_until = t + fx(13);
                            }
                            // Hit first by a laser holder while we hold only the gun. In our
                            // recorded early deaths 18 of 21 came while trading fire, 13 of the
                            // killers held a laser, and we were gun-only at the first hit 60% of
                            // the time. Confirmed free-for-all: break line of sight from 40 m out
                            // (inside that we are cornered and fight), and stay out of that fight
                            // until we hold a laser or lightning or the attacker is wounded.
                            let gun_only = !me.wpn.iter().any(|w| w.as_ref().map_or(false, |w| w.kind == "laser" || w.kind == "lightning"));
                            let ours = self.my_hit_t.get(&by).map_or(false, |h| t - *h < 30.0) || (by == self.open_id && t < self.open_until);
                            if en.has("laser") && gun_only && !ours && t - en.hurt_me_t > 30.0 {
                                let on = self.mode == 2 && leave_on();
                                if fight_log() {
                                    eprintln!("FIGHT HITFIRST {} {} {:.1} d={:.0} mode={} on={} hp={:.2}", self.flog_key, by, t, d, self.mode, on as u8, me.hp / me.max_hp.max(1.0));
                                    self.flog.push((1, by, t, true, [me.pos[0], me.pos[1]], [0.0, 0.0]));
                                }
                                if on {
                                    self.shun.insert(by, t);
                                    if d > 40.0 && t >= self.brk_until {
                                        self.brk_id = by;
                                        self.brk_until = t + fx(13);
                                    }
                                }
                            }
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
        // A death is heard across most of the map, with a bearing and a rough distance.
        if let Some(ss) = obs["sounds"].as_array() {
            for snd in ss {
                let kind = snd["kind"].as_str().unwrap_or("");
                if matches!(kind, "gun" | "laser" | "mortar" | "lightning") && snd["dist"].as_str() != Some("near") && self.kills == 0 && t < 45.0 {
                    if let Some(b) = snd["bearing"].as_f64() {
                        let b = b as f32;
                        // Leave out any direction where we know of a mech: that may be the one
                        // hunter, or a mech we picked a fight with.
                        let known = self.enemies.values().any(|e| wrap(b - (e.pos[1] - me.pos[1]).atan2(e.pos[0] - me.pos[0])).abs() < 0.5);
                        if !known {
                            self.fire_heard.push((b, t - f(&snd["age"])));
                        }
                    }
                }
                if kind != "death" {
                    continue;
                }
                let r = match snd["dist"].as_str() {
                    Some("near") => 35.0,
                    Some("mid") => 110.0,
                    _ => continue,
                };
                if let Some(b) = snd["bearing"].as_f64() {
                    let b = b as f32;
                    self.scav.push(([me.pos[0] + r * b.cos(), me.pos[1] + r * b.sin()], t - f(&snd["age"])));
                }
            }
        }
        self.scav.retain(|x| t - x.1 < 75.0);
        self.fire_heard.retain(|x| t - x.1 < 15.0);
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
        // Design B: a house bot that closes on us from inside 250 m counts as a hunter, like one
        // that aims at us or hits us.
        if self.dbi(3) {
            let mut cs = std::mem::take(&mut self.close_since);
            for e in self.enemies.values_mut() {
                let mut closing = false;
                if e.seen >= t - 0.01 && dxy(&e.pos, &me.pos) < 250.0 {
                    let to = norm2([me.pos[0] - e.pos[0], me.pos[1] - e.pos[1]]);
                    closing = e.vel[0] * to[0] + e.vel[1] * to[1] > 3.5;
                }
                if closing {
                    let since = *cs.entry(e.id).or_insert(t);
                    if t - since >= 0.6 {
                        e.hostile_until = e.hostile_until.max(t + 6.0);
                    }
                } else {
                    cs.remove(&e.id);
                }
            }
            self.close_since = cs;
        }
        // ---- which kind of match is this? Mechs dying that we did not kill means a free-for-all.
        let alive = obs["alive"].as_i64().unwrap_or(16);
        if self.cfg_mode == 0 {
            if self.alive0 < 0 {
                self.alive0 = alive;
            }
            // A death we did not cause means a free-for-all. Wait a second before believing it, in
            // case the kill event for our own shot arrives an observation late.
            // A mech we have shot at can be finished by a drone or the zone, with no kill credited
            // to us: in simulated practice that happens in one match in eleven, and reading it as
            // a free-for-all made us hide for the rest of a practice match. Once we have hit
            // anyone, it takes a second unexplained death.
            let slack = if flip_slack() && !self.i_hit.is_empty() { 1 } else { 0 };
            if self.alive0 - alive > self.kills + slack {
                if self.foreign_since < 0.0 {
                    self.foreign_since = t;
                }
            } else {
                self.foreign_since = -1.0;
            }
            let foreign_death = self.foreign_since >= 0.0 && t - self.foreign_since >= 1.0;
            // House bots never hurt each other and hunt one at a time early, so a badly wounded mech
            // we never touched, or two attackers while we are still only defending, means the same.
            // Damage we watched another mech take and did not deal. The house squad never hurts
            // its own, a drone bullet does 25 (37.5 from behind, 50 for two at once) and a mortar
            // carrier's own blast at most 80, so only two readings count: one hit of 120 or more
            // (a laser or a mortar shell), or two gun-sized hits (43 to 49.5) inside 10 s.
            let cands: Vec<(i64, f32)> = self.fd_cand.drain(..).collect();
            for (id, drop) in cands {
                let mine = self.my_hit_t.get(&id).map_or(false, |h| t - *h < 0.35);
                if mine || t >= 60.0 {
                    continue;
                }
                if drop >= 120.0 {
                    self.fd_big = true;
                } else if (43.0..=49.5).contains(&drop) {
                    self.fd_small.push(t);
                }
            }
            self.fd_small.retain(|x| t - *x < 10.0);
            let fd_seen = self.fd_big || self.fd_small.len() >= 2;
            if fd_seen && !self.fd_logged {
                self.fd_logged = true;
                // Test aid for the simulator: count how often the evidence appears at all.
                if std::env::var("MB_FDLOG").is_ok() {
                    eprintln!("FDSEEN t={:.1} mode={} kills={} big={}", t, self.mode, self.kills, self.fd_big);
                }
            }
            let foreign_damage = foreign_damage_on() && fd_seen;
            let wounded_by_others = t < 60.0 && self.enemies.values().any(|e| e.seen >= t - 0.01 && !e.hit_by_me && e.hp < e.max_hp - 350.0);
            let ganged = self.mode == 0 && self.kills == 0 && self.attackers_seen.len() >= 2;
            // The house squad sends one hunter at a time. Shooting heard from two directions well
            // apart, three shots each, before we have killed anyone, is mechs fighting each other:
            // in recorded free-for-alls that happens within 5 s for five mechs in six, while the
            // first death (the old signal) comes at a median of 14 s and often after 20 s.
            let crossfire = crossfire_on() && !self.xf_off && self.mode != 2 && self.kills == 0 && t < 45.0 && {
                let h = &self.fire_heard;
                let centres: Vec<f32> = h.iter().filter(|a| h.iter().filter(|b| wrap(a.0 - b.0).abs() < 0.35).count() >= 3).map(|a| a.0).collect();
                centres.iter().any(|a| centres.iter().any(|b| wrap(a - b).abs() >= 1.05))
            };
            // Stricter form of the same idea, by volume: weapon fire from N or more of eight
            // compass sectors, three shots each, within the first seconds. Real free-for-alls are
            // loud at once (median: four sectors inside 8 s); the plain two-direction rule fired in
            // about half of practice matches (3.98 against 5.75), so this one is off unless a
            // practice submission shows a setting that never fires there.
            let (xn, xt) = crossfire2();
            let crossfire2 = xn > 0 && !self.xf_off && self.mode != 2 && self.kills == 0 && t <= xt && {
                let mut bins = [0u8; 8];
                for h in &self.fire_heard {
                    let k = (((h.0 + std::f32::consts::PI) / (std::f32::consts::PI / 4.0)) as usize).min(7);
                    bins[k] = bins[k].saturating_add(1);
                }
                bins.iter().filter(|&&c| c >= 3).count() >= xn
            };
            // Gunfire alone (the two-direction rule or the volume rule) is a guess: the house squad
            // shoots crates and drones from several directions too. Remember when it was the only
            // reason, and undo it if nobody but us has killed anything by the revert time: 97% of
            // real free-for-alls have a death we did not cause by 20 s.
            let foreign = foreign_death || foreign_damage || wounded_by_others || ganged;
            let sure = foreign_death || foreign_damage || (self.mode == 0 && (wounded_by_others || ganged));
            if self.mode != 2 && (crossfire || crossfire2) && !foreign {
                self.xf_t = t;
            }
            if foreign {
                self.xf_t = -1.0;
            }
            if (sure || crossfire || crossfire2) && self.mode != 2 && std::env::var("MB_MODELOG").is_ok() {
                eprintln!("MODE2 t={:.1} kills={} foreign_death={}", t, self.kills, foreign_death);
            }
            if sure || crossfire || crossfire2 {
                self.mode = 2;
            }
            if self.mode == 2 && self.xf_t >= 0.0 && xf_revert_s() > 0.0 && t >= xf_revert_s() && self.alive0 - alive <= self.kills {
                self.mode = 1;
                self.xf_t = -1.0;
                self.xf_off = true;
            }
            if self.mode == 0 && !(sure || crossfire || crossfire2) && t >= self.tune[9] {
                self.mode = 1;
            }
        }
        // Free-for-all: a supply drop that has landed or that someone is capturing is a kill zone
        // for everyone near it who is not in the ring (ENDGAME.md: 10 of 16 mechs lingering 8-40 m
        // from one died within 20 s, against 4 of 35 inside the ring). Commit or keep 60 m away.
        self.drop_avoid.clear();
        if self.mode == 2 && drop_away_on() {
            if let Some(ds) = obs["drops"].as_array() {
                for d in ds {
                    let id = d["id"].as_i64().unwrap_or(-1);
                    let live = f(&d["lands_in"]) <= 3.0 || !d["holder"].is_null();
                    if live && id != self.drop_goal {
                        let p = v3(&d["pos"]);
                        self.drop_avoid.push([p[0], p[1]]);
                    }
                }
            }
        }
        // Free-for-all, LASER.md rule 2: within 140 m of a known laser holder, keep cover in reach
        // (within about 7 m walking, 12 m with energy to boost). Half the gun-side deaths had cover
        // a median 6 m away and never reached it. Goals whose route crosses more open ground than
        // this are skipped while the holder is near.
        self.cover_lim = if self.mode == 2 && cover_rule_on() && self.enemies.values().any(|e| t - e.seen < 10.0 && e.has("laser") && dxy(&e.pos, &me.pos) < 140.0) {
            if me.energy >= 30.0 { 16.0 } else { 8.0 }
        } else {
            f32::INFINITY
        };
        let cautious = self.mode != 1;
        // "Practice" by default is only a guess until someone dies: in recorded free-for-alls the
        // first death comes at a median of 11 s and after 16 s in one match in eight. Until then,
        // or until our own first kill, defend ourselves but do not rush weapons, boost around or
        // pick fights with mechs that are leaving us alone.
        let p_lite = self.mode == 1 && self.kills == 0 && t < self.p_lite_t();
        let slot_of = |k: &str| -> Option<usize> { (0..2).find(|&i| me.wpn[i].as_ref().map_or(false, |w| w.kind == k)) };
        let gun_s = slot_of("gun");
        let laser_s = slot_of("laser");
        let light_s = slot_of("lightning");
        // Laser hunt (practice, `MB_LHUNT=1`): past 20 s or our first kill and still without a
        // laser, getting one comes first. In simulated practice the matches that never hold a
        // laser score 2.3 against 4.5, and in a quarter of them a laser we had seen was still on
        // the ground: the 25 s zone rule turned us back 16 m short of one, a drone fight stopped a
        // walk for 12 s, a mech standing next to us counted as "closer to it", and the swap from
        // lightning waited for a quiet moment that never came.
        let lhunt = self.vsw(V_LHUNT) && laser_s.is_none() && (self.kills >= 1 || t >= 20.0);
        let my_power = me.hp * (1.0 + 0.04 * me.cubes.min(10.0)) * if laser_s.is_some() { 1.3 } else { 1.0 };
        let power = |e: &Enemy| e.hp * (1.0 + 0.04 * e.cubes.min(10.0)) * if e.has("laser") { 1.3 } else { 1.0 };
        let threats: Vec<Enemy> = self.enemies.values().filter(|e| e.seen >= t - 0.01).cloned().collect();
        let any_hostile = threats.iter().any(|e| e.hostile_until > t);
        let pol2 = self.vsw(V_POLICY2);
        if !self.shun.is_empty() {
            if laser_s.is_some() || light_s.is_some() {
                self.shun.clear();
            } else {
                let en = &self.enemies;
                self.shun.retain(|id, _| en.get(id).map_or(false, |e| t - e.seen < 45.0 && e.hp >= 0.4 * e.max_hp));
            }
        }

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
                let lonely = threats.len() == 1 && self.enemies.values().filter(|o| t - o.seen < 8.0).count() == 1;
                // First shot (confirmed free-for-all only): a mech that is not looking at us and is
                // either alone or busy with someone else. In recorded matches the side that lands
                // the first hit wins three duels in four, and most of the top teams collapse when
                // hit first. We need to be fresh, and to hold a ready laser or be close enough
                // for the gun to count.
                let busy = e.hp < 0.85 * e.max_hp
                    || threats.iter().any(|o| o.id != e.id && dxy(&o.pos, &e.pos) < 220.0 && wrap(aim_to(e.eye(), o.center())[0] - e.aim[0]).abs() < 0.25);
                let laser_ready = laser_s.and_then(|sl| me.wpn[sl].as_ref()).map_or(false, |w| w.cooldown <= 0.2);
                let nobody_on_us = !threats.iter().any(|o| o.id != e.id && o.hostile_until > t);
                let fs = first_shot_knobs();
                let away = wrap(aim_to(e.eye(), my_center)[0] - e.aim[0]).abs() > fs[5];
                let ambush = first_shot_on()
                    && self.mode == 2
                    && t > fs[0]
                    && hp_frac > fs[1]
                    && d > 55.0
                    && d < fs[2]
                    && (laser_ready || d < fs[3])
                    && away
                    && (lonely || busy)
                    && nobody_on_us
                    && !(FFA_LASER_GUARD && e.has("laser") && laser_s.is_none())
                    && !(FFA_LASER_FLOOR && e.has("laser") && laser_s.is_some() && d < 70.0)
                    && power(e) < fs[4] * my_power;
                if ambush {
                    self.commit_id = e.id;
                    self.commit_until = t + 10.0;
                }
                // A laser holder that hit us first while we hold only the gun: we do not start
                // that fight again (see `shun`).
                let shunned = self.mode == 2 && leave_on() && self.shun.contains_key(&e.id);
                let committed = e.id == self.commit_id && t < self.commit_until && hp_frac > 0.4 && !shunned;
                // Snowballer (10+ cubes, private/COUNTERS.md #2): one that has only looked our way
                // is not worth opening on. When we hit one first we die 31-41% of the time and kill
                // it 21-35%. Shoot back once it hits us, or open when it is aiming at someone else
                // or is below 40% HP.
                let busy_other = threats.iter().any(|o| o.id != e.id && dxy(&o.pos, &e.pos) < 220.0 && wrap(aim_to(e.eye(), o.center())[0] - e.aim[0]).abs() < 0.25);
                let snow_veto = self.mode == 2 && snow_veto_on() && e.cubes >= 10.0 && t - e.hurt_me_t > 30.0 && e.hp >= 0.4 * e.max_hp && !busy_other;
                (e.hostile_until > t && !snow_veto) || prey || ambush || committed || (alive <= 4 && my_power > 1.4 * power(e) && !shunned)
            } else if self.dbi(3) {
                // Design B: a bot that has not attacked or approached us is harmless: no first shot
                // while we hold only the gun, until two minutes in. With a laser we fight harmless
                // bots too, but only from closer than hunters.
                e.hostile_until > t || t >= dbk(3) || (laser_s.is_some() && d < dbk(4))
            } else {
                match if pol2 { 2 } else { self.policy } {
                    1 => e.hostile_until > t,
                    2 => e.hostile_until > t || (!any_hostile && hp_frac > 0.9),
                    _ => !p_lite || e.hostile_until > t,
                }
            };
            if allowed && d < 260.0 && (self.w.los(muzzle, e.center()) || (self.mode != 2 && fr(1) && self.w.los(muzzle, e.upper()))) {
                let mut score = d + self.tune[5] * e.hp - if t - e.hurt_me_t < 3.0 || e.hostile_until > t { self.tune[4] } else { 0.0 };
                if self.dbi(3) {
                    // The one hurting us first, the one we are on, the nearly dead.
                    if t - e.hurt_me_t < 4.0 {
                        score -= 200.0;
                    }
                    if e.id == self.db_target {
                        score -= 40.0;
                    }
                    if e.hp < 0.3 * e.max_hp {
                        score -= 60.0;
                    }
                }
                // Free-for-all: our laser does 60% more to a mortar carrier.
                if (self.mode == 2 || pf(6)) && mortar_first_on() && laser_s.is_some() && e.has("mortar") {
                    score -= 40.0;
                }
                if score < best {
                    best = score;
                    target = Some(e.clone());
                }
            }
        }
        let finishable = target.as_ref().map_or(false, |e| e.hp <= 180.0);
        if self.db() {
            if let Some(e) = &target {
                self.db_target = e.id;
            }
        }
        // Design B: a target inside 120 m that is nearly dead, or has under half our HP, is a fight
        // we are about to win: no retreat, no stepping back.
        let db_fin = self.db() && target.as_ref().map_or(false, |e| dxy(&e.pos, &me.pos) < 120.0 && (e.hp < 0.3 * e.max_hp || e.hp < 0.5 * me.hp));
        if let Some(e) = &target {
            // A target only exists with a line of fire, so this times how long we have had it.
            if e.id != self.los_id || t - self.los_last > 0.25 {
                self.los_id = e.id;
                self.los_since = t;
            }
            self.los_last = t;
        }
        // A fight we start (confirmed free-for-all): the target had not hit us in the 30 s before
        // we took aim at it. It stays ours while we keep aiming at it.
        if self.mode == 2 {
            if let Some(e) = &target {
                if e.id == self.open_id && t < self.open_until {
                    self.open_until = t + 10.0;
                } else if t - e.hurt_me_t > 30.0 {
                    self.open_id = e.id;
                    self.open_until = t + 10.0;
                }
            }
        }
        if fight_log() {
            let key = self.flog_key.clone();
            let en = &self.enemies;
            for r in self.flog.iter_mut() {
                if r.0 == 0 && !r.3 && t >= r.2 + 5.0 {
                    r.3 = true;
                    // own = how fast we ourselves moved toward where the mech stood (m/s).
                    let own = ((me.pos[0] - r.4[0]) * r.5[0] + (me.pos[1] - r.4[1]) * r.5[1]) / (t - r.2).max(0.1);
                    if let Some(e) = en.get(&r.1) {
                        if t - e.seen < 1.0 {
                            eprintln!("FIGHT D5 {} {} {:.1} d={:.0} own={:.1}", key, r.1, r.2, dxy(&e.pos, &me.pos), own);
                        }
                    }
                }
                if t >= r.2 + 20.0 {
                    eprintln!("FIGHT {} {} {} {:.1}", if r.0 == 0 { "ALIVE" } else { "HALIVE" }, key, r.1, r.2);
                }
            }
            self.flog.retain(|r| t < r.2 + 20.0);
        }

        // ---- stand or run. Against the house squad nobody else thins the field, so running only
        // delays the same fight. In a free-for-all every second alive is worth points: fight one
        // attacker we can match, hide from everything else.
        let mut evade_from: Vec<Enemy> = threats.clone();
        if cautious {
            // Keep hiding from mechs seen in the last few seconds, and from a shooter we cannot see.
            // Wounded in a confirmed free-for-all: stay out of sight of where mechs were for
            // longer, so we finish regenerating before we show ourselves again (leaving cover at
            // 68% with the eventual winner 80 m away cost us a third place).
            let remember = if self.mode == 2 && hp_frac < 0.85 && regen_hold_on() { 25.0 } else { 10.0 };
            for e in self.enemies.values() {
                if e.seen < t - 0.01 && t - e.seen < remember {
                    evade_from.push(e.clone());
                }
            }
            if t - self.ghost_t < 3.0 && !threats.iter().any(|e| e.hostile_until > t) {
                let p = [me.pos[0] + 110.0 * self.hurt_bearing.cos(), me.pos[1] + 110.0 * self.hurt_bearing.sin()];
                let gz = self.w.surf_at(p[0], p[1]);
                evade_from.push(Enemy { id: -1, pos: [p[0], p[1], gz], vel: [0.0; 3], aim: [0.0, 0.0], hp: 1400.0, max_hp: 1400.0, cubes: 0.0, weapons: Vec::new(), seen: t, hurt_me_t: t, hostile_until: t + 1.0, charging: false, facing_since: -1.0, hit_by_me: false });
            }
        }
        let mut arc_target: Option<Enemy> = None;
        if cautious {
            let hostiles: Vec<&Enemy> = threats.iter().filter(|e| e.hostile_until > t).collect();
            // Lightning against a laser. A beam cannot be dodged, so running from one over open
            // ground costs a full hit every 2.4 s (we lost 1,400 HP in 12 s that way), but a laser
            // carrier that lightning keeps hitting cannot charge at all. Shot at by a lone laser
            // carrier while we hold lightning: run in, and stay inside 45 m until it is dead.
            // Attacked up close (scouting: inside 60 m the attacked side dies 26% of the time and
            // pushing in wins 40%; running only shows our back). Test switch MB_CLOSEIN=1.
            if self.mode == 2 && std::env::var("MB_CLOSEIN").is_ok() && hostiles.len() == 1 && arc_target.is_none() {
                let h = hostiles[0];
                let d = dxy(&h.pos, &me.pos);
                let others = threats.iter().filter(|o| o.id != h.id && dxy(&o.pos, &me.pos) < 130.0).count();
                if d < 60.0 && t - h.hurt_me_t < 3.0 && others == 0 && hp_frac > 0.4 && power(h) < 1.3 * my_power && self.w.los(muzzle, h.center()) {
                    arc_target = Some(h.clone());
                }
            }
            if self.mode == 2 && arc_rush_on() && light_s.is_some() && hostiles.len() == 1 {
                let h = hostiles[0];
                let d = dxy(&h.pos, &me.pos);
                let others = threats.iter().filter(|o| o.id != h.id && dxy(&o.pos, &me.pos) < 130.0).count();
                let going = h.id == self.arc_id && t < self.arc_until;
                let run = (d - 40.0).max(0.0) / (me.walk * 1.67);
                let energy_left = me.energy - run * 17.0;
                let hits = run / 2.4 + 0.6;
                let burst = energy_left.max(0.0) / 3.0 * 60.0 * 1.45;
                let start = h.has("laser")
                    && !h.has("lightning")
                    && (t - h.hurt_me_t < 3.0 || h.charging)
                    && others == 0
                    && d < arc_knob(0)
                    && energy_left > arc_knob(3)
                    && me.hp > 350.0 * hits + arc_knob(1)
                    && h.hp < arc_knob(2) * burst
                    && self.w.los(muzzle, h.center());
                if start && !going {
                    self.arc_id = h.id;
                    self.arc_until = t + 8.0;
                }
                if (start || going) && others == 0 && (d < 45.0 || me.energy > 10.0) && me.hp > 250.0 {
                    arc_target = Some(h.clone());
                } else if going {
                    self.arc_until = -1.0;
                }
            }
            let engage_ok = arc_target.is_some() || match &target {
                None => false,
                Some(_) => {
                    let near = threats.iter().filter(|o| dxy(&o.pos, &me.pos) < 130.0).count();
                    if hostiles.is_empty() {
                        true
                    } else if hostiles.len() == 1 && near <= 1 {
                        let committed = hostiles[0].id == self.commit_id && t < self.commit_until && hp_frac > 0.4;
                        // A wounded attacker with nobody else on us is worth finishing: pushing on
                        // one below 40% HP won 71% of recorded fights against 17 to 55% for holding
                        // or leaving.
                        let wounded = self.mode == 2 && finish_on() && hostiles[0].hp < 0.4 * hostiles[0].max_hp && hp_frac > 0.5;
                        // Last four, no laser of our own, a laser on us at 50 to 120 m: do not
                        // trade (most of our 14 real last-four deaths went this way). Leave its
                        // line of fire unless it is nearly dead.
                        let dh = dxy(&hostiles[0].pos, &me.pos);
                        let no_trade = self.mode == 2 && last4_on() && alive <= 4 && laser_s.is_none() && hostiles[0].has("laser") && dh > 50.0 && dh < 120.0;
                        let shunned = self.mode == 2 && leave_on() && self.shun.contains_key(&hostiles[0].id);
                        if no_trade {
                            wounded || finishable
                        } else if shunned {
                            finishable
                        } else {
                            committed || wounded || my_power >= (if self.mode == 0 { self.tune[11] } else { self.tune[8] }) * power(hostiles[0]) || finishable
                        }
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
        } else if self.tune[3] < 0.5 && self.dbi(4) {
            // Design B, retreat and heal: leave when clearly hurt (below 40% HP with a mech tracked),
            // or below two thirds with two or more hunters on us; hold fire (regeneration starts
            // 5 s after the last damage dealt or taken); return at 80%. A fight that is nearly won
            // is never left.
            let tracked = self.enemies.values().filter(|e| t - e.seen < 8.0).count();
            let hunters = threats.iter().filter(|e| e.hostile_until > t && dxy(&e.pos, &me.pos) < 180.0).count();
            let leave = (hp_frac < dbk(0) && tracked >= 1) || (hunters >= 2 && hp_frac < dbk(1));
            if !self.retreating && leave && !finishable && !db_fin {
                self.retreating = true;
                self.cover = None;
                self.db_nocover = false;
            } else if self.retreating && (hp_frac >= dbk(2) || finishable || db_fin) {
                self.retreating = false;
            }
        } else if self.tune[3] < 0.5 {
            // Practice break-and-heal (switch V_HEAL, or PR_TUNE knob 23): badly hurt
            // with enough hunters about (knob 26 tracked, 0 = any), break line of sight and
            // regenerate instead of trading to the death. Kills gate knob 24 is 0 (always open).
            let heal = self.heal_on() && self.kills as f32 >= practice::knob(24);
            let need_tracked = if self.vsw(V_HEAL_ANY) { 0.0 } else { practice::knob(26) };
            let tracked = self.enemies.values().filter(|e| t - e.seen < 8.0).count() as f32;
            if heal && !self.retreating && !finishable && hp_frac < practice::knob(25) && tracked >= need_tracked {
                self.retreating = true;
                self.cover = None;
            } else if heal && self.retreating && !(hp_frac >= 0.9 || (threats.is_empty() && hp_frac >= 0.7 && t - self.last_hurt_t > 6.0)) {
                // keep healing
            } else {
                self.retreating = false;
            }
            // Regroup in the lulls (V_REGROUP): never leaves a fight (it starts only with nothing in
            // view) and ends the moment a mech shows up or hits us.
            // PG_REST_AFTER5 uses the same rule from the fifth kill, starting below 85% and
            // resting to above 95%.
            let rest5 = pg(1) && self.mode == 1 && self.kills as f32 >= pg_kills();
            if self.vsw(V_REGROUP) || rest5 {
                let (lo, hi) = if self.vsw(V_REGROUP) { (0.6, 0.9) } else { (0.85, 0.95) };
                if self.enemies.values().any(|e| t - e.seen < 5.0 && self.w.los(e.eye(), my_center)) {
                    self.watched_t = t;
                }
                if !self.regroup && hp_frac < lo && threats.is_empty() && t - self.last_hurt_t > 2.0 && t - self.watched_t > 3.0 && !finishable {
                    self.regroup = true;
                    self.cover = None;
                } else if self.regroup && (hp_frac >= hi || !threats.is_empty() || t - self.last_hurt_t < 1.0) {
                    self.regroup = false;
                }
                if self.regroup {
                    self.retreating = true;
                    // Cover from where mechs were last seen, not only from those in view now.
                    for e in self.enemies.values().filter(|e| t - e.seen < 10.0 && e.seen < t - 0.01) {
                        evade_from.push(e.clone());
                    }
                }
            } else {
                self.regroup = false;
            }
        } else if !self.retreating {
            if !threats.is_empty() && !finishable && (hp_frac < self.tune[1] || (attackers >= 2 && hp_frac < self.tune[2])) {
                self.retreating = true;
                self.cover = None;
            }
        } else if hp_frac >= 0.9 || (threats.is_empty() && hp_frac >= 0.7 && t - self.last_hurt_t > 6.0) {
            self.retreating = false;
        }

        // ---- zone
        // In free-for-alls the only zone deaths recorded were full-health hiders caught at the
        // edge, so keep a wider margin there.
        let margin = (z.nr * 0.5).min(if self.mode == 2 || pf(2) { 30.0 } else { 20.0 });
        let d_next = dxy(&[here.0, here.1], &z.nc);
        let inside_next = d_next <= z.nr - margin;
        let zone_point = if d_next > 1e-3 {
            let k = ((z.nr - margin - 6.0).max(0.0) / d_next).min(1.0);
            (z.nc[0] + (here.0 - z.nc[0]) * k, z.nc[1] + (here.1 - z.nc[1]) * k)
        } else {
            (z.nc[0], z.nc[1])
        };
        let travel = (d_next - (z.nr - margin)).max(0.0) / me.walk;
        let near_edge = dxy(&[here.0, here.1], &z.c) > z.r - if self.mode == 2 || pf(2) { 20.0 } else { 10.0 };
        // Last four: head into the next circle 30 s early rather than 12 (one of our last-four
        // deaths was a full-HP burn outside it).
        let lead = if self.mode == 2 && last4_on() && alive <= 4 { 30.0 } else { 12.0 };
        let zone_urgent = !me.in_zone || (!inside_next && (z.ss - t < travel + lead || near_edge));
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
                    let upgrade = (w1.kind == "mortar") || (self.tune[12] < 0.5 && w1.kind == "lightning" && kind == "laser" && (threats.is_empty() || lhunt || self.practice_swap_ok(&me, &threats)));
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
        // PG_CUBE_STOP: with ten cubes nothing is worth a detour (a cube on our path is still taken).
        let cube_stop = pg(2) && self.mode == 1 && me.cubes >= 10.0;
        let mut near_cube: Option<(f32, (f32, f32), i64)> = None;
        for (id, it) in self.cubes.iter().filter(|_| !cube_stop) {
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
                if d < (if self.dbi(2) { 70.0 } else { 60.0 }) && in_bounds(&it.pos) && (it.pos[2] - me.pos[2]).abs() < 5.0 && near_gun.map_or(true, |c| d < c.0) {
                    near_gun = Some((d, (it.pos[0], it.pos[1]), *id));
                }
            }
        }

        // Weapon rush: with only the gun, a laser or lightning we can reach before anyone else
        // comes first, ahead of hiding. The teams that win free-for-alls are armed within seconds.
        // This rule covers free-for-alls and the opening seconds before the match type is known
        // (when a free-for-all rush matters most). Confirmed practice (mode 1) uses it only until
        // practice has a rush of its own: flip PRACTICE_HAS_OWN_RUSH when that lands.
        const PRACTICE_HAS_OWN_RUSH: bool = true;
        let mut ffa_rush: Option<(f32, (f32, f32), i64)> = None;
        // Confirmed free-for-all, gun only, after 10 s: walk (never boost) to a special within
        // 130 m that no known mech is nearer to, with at most 40 m of open street on the way.
        // Without that limit the simulator armed us more often (66% against 55% by 60 s), but
        // 2 of 6 deaths at 6 AM came on walks to weapons 82 to 86 m away across open ground. With the rush off we went gun-only all match in 3 of 6 matches at
        // 5 AM, and holding a laser for most of a match goes with 9.8 points against 5.2 gun-only.
        let ffa_walk = self.mode == 2 && fetch_on() && fx(15) >= 0.5 && t >= fetch_knob(2) && me.wpn[1].is_none() && hp_frac > 0.5
            // Never within 6 s of being hit: the 1 to 2 AM deaths at 14 s and 17 s were runs across
            // open ground under fire. (Also blocking it while hiding from any mech seen in the last
            // 10 s took away nearly all the fetches in the simulator.)
            && t - self.last_hurt_t > 6.0;
        if ffa_walk || (self.tune[13] < 0.5 && fx(15) < 0.5 && me.wpn[1].is_none() && (self.mode != 1 || !PRACTICE_HAS_OWN_RUSH)) {
            for (id, it) in &self.pickups {
                if self.blacklist.get(id).map_or(false, |u| t < *u) || !(it.kind == "laser" || it.kind == "lightning") {
                    continue;
                }
                let d = dxy(&it.pos, &me.pos);
                let climb = (it.pos[2] - me.pos[2]).max(0.0);
                if d > (if ffa_walk { fetch_knob(0) } else { 130.0 }) || climb > 3.0 || !in_bounds(&it.pos) {
                    continue;
                }
                if ffa_walk && self.npc_mem.values().any(|n| (n.0[0] - it.pos[0]).hypot(n.0[1] - it.pos[1]) < 50.0) {
                    continue;
                }
                if self.drop_avoid.iter().any(|q| (q[0] - it.pos[0]).hypot(q[1] - it.pos[1]) < 60.0) {
                    continue;
                }
                if self.cover_lim.is_finite() && self.open_run(here, (it.pos[0], it.pos[1])) > self.cover_lim {
                    continue;
                }
                // Someone else closer to it, or a shooter between us and a long run: leave it.
                let margin = if ffa_walk { fetch_knob(3) } else { 15.0 };
                if self.enemies.values().any(|e| t - e.seen < 6.0 && dxy(&e.pos, &it.pos) < d + margin) {
                    continue;
                }
                if (d > 25.0 || climb > 3.0) && threats.iter().any(|e| e.hostile_until > t && self.w.los(e.eye(), my_center)) {
                    continue;
                }
                // A run across open streets is how we got shot in three of four 1 AM openings.
                let open_limit = if ffa_walk { fetch_knob(1) } else { fx(7) };
                if open_limit > 0.0 && d > 25.0 && self.open_run(here, (it.pos[0], it.pos[1])) > open_limit {
                    continue;
                }
                let score = d * if it.kind == "laser" { 0.7 } else { 1.0 } + climb * 3.0;
                if ffa_rush.map_or(true, |r| score < r.0) {
                    ffa_rush = Some((score, (it.pos[0], it.pos[1]), *id));
                }
            }
        }
        let hunt_bounds = |p: &[f32]| -> bool { in_bounds(p) || (lhunt && dxy(p, &z.c) < z.r - 8.0 && dxy(p, &me.pos) < 80.0) };
        let practice_rush = if self.mode == 1 && !p_lite { self.practice_weapon_goal(&me, t, laser_s.is_some(), light_s.is_some(), &hunt_bounds, lhunt) } else { None };

        // Design B, between kills: a fixed order of errands (laser if none, a weapon for the empty
        // slot, cubes, the site of a recent death), each given up after 10 s and ignored for 60 s.
        let mut errand: Option<(f32, (f32, f32), i64)> = None;
        if self.dbi(5) && !self.retreating && threats.is_empty() {
            errand = self.practice_errand(&me, t, laser_s.is_some(), &in_bounds);
            if let Some(r) = errand {
                if r.2 != self.errand_id {
                    self.errand_id = r.2;
                    self.errand_t0 = t;
                } else if t - self.errand_t0 > 10.0 {
                    if r.2 >= 0 {
                        self.blacklist.insert(r.2, t + 60.0);
                    } else {
                        self.scav.retain(|x| (x.0[0] - r.1 .0).hypot(x.0[1] - r.1 .1) > 30.0);
                    }
                    self.errand_id = -9;
                    errand = None;
                }
            } else {
                self.errand_id = -1;
            }
        }

        // Weapon run from second 0 (V_RUN0): gun only, head straight for the nearest laser we know
        // of (lightning when no laser is), boosting while the tank is above 40. Gives up on one pickup
        // after 12 s and ignores it for 30 s, and while a mech is hitting us.
        let mut run0: Option<(f32, (f32, f32), i64)> = None;
        if (self.vgate(V_RUN0, self.mode != 2) || (self.db_open() && !dboff(1))) && me.wpn[1].is_none() && !self.retreating && t - self.last_hurt_t > 3.0 {
            for (id, it) in &self.pickups {
                let reach = match it.kind.as_str() {
                    "laser" => practice::knob(1),
                    "lightning" => 130.0,
                    _ => continue,
                };
                let d = dxy(&it.pos, &me.pos);
                // Design B: a short reach while a mech is in view (a fight is on).
                let reach = if self.db_open() && !dboff(1) && !threats.is_empty() { reach.min(70.0) } else { reach };
                if self.blacklist.get(id).map_or(false, |u| t < *u) || d > reach || it.pos[2] - me.pos[2] > 3.0 || !in_bounds(&it.pos) || (it.pos[2] - self.w.surf_at(it.pos[0], it.pos[1])).abs() > 2.0 {
                    continue;
                }
                if self.drop_avoid.iter().any(|q| (q[0] - it.pos[0]).hypot(q[1] - it.pos[1]) < 60.0) {
                    continue;
                }
                // Lasers first: a lightning pickup is only worth it when no laser qualifies.
                let score = d + if it.kind == "laser" { 0.0 } else { 1000.0 };
                if run0.map_or(true, |r| score < r.0) {
                    run0 = Some((score, (it.pos[0], it.pos[1]), *id));
                }
            }
            if let Some(r) = run0 {
                if r.2 != self.run0_id {
                    self.run0_id = r.2;
                    self.run0_t0 = t;
                }
                if t - self.run0_t0 > 12.0 {
                    self.blacklist.insert(r.2, t + 30.0);
                    run0 = None;
                }
            }
        }

        // A drone worth shooting: one that is hurting us, or any in reach when things are quiet.
        // Each is 350 HP for two cubes, and its own bullets are slow.
        let quiet = threats.is_empty() && (!cautious || self.enemies.values().all(|e| t - e.seen > 8.0));
        let mut shoot_npc: Option<([f32; 3], [f32; 3])> = None;
        // Their fire is accurate (about three hits in four on a moving mech in real matches), so
        // one is worth killing for its cubes, but two or more at once, or any while we are hurt or
        // the opening scramble is on, is a fight to walk away from.
        let drones_near = npcs.iter().filter(|n| dxy(&n.0, &me.pos) < 62.0).count();
        let enemy_los = threats.iter().any(|e| self.w.los(e.eye(), my_center));
        // A drone is hitting us and no mech can see us: deal with it now, even from a hiding spot
        // (sitting in one drone's fire while hiding from a mech cost 438 HP in a recorded match).
        let drone_hot = self.tune[15] < 1.5 && t - self.npc_hurt_t < 2.5 && !npcs.is_empty() && target.is_none() && !enemy_los;
        // Low on HP next to one drone (free-for-all): no branch below covered it, so we held still
        // and let a lone drone finish us at 5 AM. Walk out of its reach instead.
        let drone_flee = drone_hot && (drones_near >= 2 || ((self.mode == 2 || pf(3)) && hp_frac <= 0.2));
        let eager = self.vsw(V_EAGER) && hp_frac > 0.4;
        let cover_open_on = self.vsw(V_COVERSPOT) && self.kills == 0;
        let farm_drone = self.tune[15] < 0.5 && drones_near <= 1 && !cube_stop && errand.is_none() && target.is_none() && ((hp_frac > 0.75 && t >= 25.0) || eager) && quiet;
        if !drone_flee && ((drone_hot && drones_near == 1 && hp_frac > 0.2) || farm_drone) {
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
        // Reload cover: the gun is empty for 2.4 s of every cycle. If a wall is a few steps away,
        // spend that time behind it and step back out as the magazine fills. Off by default
        // (tune[20]): in simulation it cost practice points and did nothing in free-for-alls.
        let mut peek_mv: Option<Mv> = None;
        if self.tune[20] < 0.5 && !self.retreating && t >= self.unstick_until && !zone_urgent {
            let gun_reload = gun_s.and_then(|s| me.wpn[s].as_ref()).map_or(0.0, |w| if w.ammo <= 0.0 && w.reload <= 0.0 { 2.4 } else { w.reload });
            let laser_live = laser_s.and_then(|s| me.wpn[s].as_ref()).map_or(false, |w| w.cooldown < 0.5 || w.charge > 0.0);
            if let Some(e) = &target {
                let arc_live = light_s.is_some() && me.energy > 15.0 && d3(muzzle, e.center()) < 43.0;
                if gun_s.is_some() && gun_reload > self.tune[21] && !laser_live && !arc_live && t >= self.peek_until && t - self.peek_retry_t > 1.0 {
                    match self.find_cover(&me, std::slice::from_ref(e), &z, false, self.tune[22], 2.0) {
                        Some(c) => {
                            self.peek_until = t + gun_reload;
                            self.peek_cover = Some(c);
                            self.peek_from = here;
                            self.peek_id = e.id;
                        }
                        None => self.peek_retry_t = t,
                    }
                }
            }
            if t < self.peek_until {
                let still_there = threats.iter().any(|e| e.id == self.peek_id);
                match (self.peek_cover, still_there) {
                    (Some(c), true) => {
                        let left = self.peek_until - t;
                        let back = (c.0 - self.peek_from.0).hypot(c.1 - self.peek_from.1) / me.walk + 0.15;
                        if left > back {
                            // Exposed at the hiding spot (the enemy moved): give up on it.
                            let s = self.w.surf_at(c.0, c.1);
                            let seen_there = threats.iter().any(|e| e.id == self.peek_id && self.w.los(e.eye(), [c.0, c.1, s + 0.5 * me.height]));
                            if seen_there {
                                self.peek_until = -1.0;
                            } else if (c.0 - here.0).hypot(c.1 - here.1) < 1.2 {
                                peek_mv = Some(Mv::Hold);
                            } else {
                                peek_mv = Some(Mv::Goto(c, me.energy > 20.0));
                            }
                        } else if target.is_none() {
                            peek_mv = Some(Mv::Goto(self.peek_from, false));
                        } else {
                            self.peek_until = -1.0;
                        }
                    }
                    _ => self.peek_until = -1.0,
                }
            }
        }
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
                        let dc = (c.0 - here.0).hypot(c.1 - here.1);
                        // Even a long run to cover beats trading shots in the open: in simulation,
                        // standing to fight when cover was over 28 m away cost 0.3 to 0.8 points.
                        if dc < 1.5 {
                            flee_hold = true;
                        } else {
                            flee_goal = Some(c);
                        }
                    }
                    None => {
                        if self.db() {
                            // Design B: no cover in reach, fight on with fire free.
                            self.db_nocover = true;
                        } else if !cautious {
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
        // PG_DROP_REPAIR: hurt, with a supply drop in reach: go and hold its ring for the capture.
        let pg_drop = if pg(0) && self.mode == 1 && !zone_urgent { self.pg_drop_goal(&me, t, obs, &in_bounds) } else { None };
        // PG_HOLD_FOR_HUNTER: with a laser and a hunter coming at us from beyond 150 m, wait near
        // cover and let it come.
        let mut pg_hold: Option<Mv> = None;
        if pg(3) && self.mode == 1 && laser_s.is_some() && !self.retreating {
            if let Some(e) = &target {
                let d = dxy(&e.pos, &me.pos);
                let to = norm2([me.pos[0] - e.pos[0], me.pos[1] - e.pos[1]]);
                if e.hostile_until > t && d > 150.0 && e.vel[0] * to[0] + e.vel[1] * to[1] > 1.0 {
                    if self.hfh_id != e.id || t - self.hfh_t > 1.0 {
                        self.hfh_cover = self.find_cover(&me, std::slice::from_ref(e), &z, false, 24.0, 3.0);
                        self.hfh_id = e.id;
                        self.hfh_t = t;
                    }
                    pg_hold = Some(match self.hfh_cover {
                        Some(c) if (c.0 - here.0).hypot(c.1 - here.1) > 1.5 => Mv::Goto(c, false),
                        _ => Mv::Hold,
                    });
                }
            }
        }
        let cover_mv = self.cover_move(&me, t, &threats, &z, my_center, laser_s.is_some(), light_s.is_some(), &in_bounds);
        // Hold range in a fight we started (confirmed free-for-all). In our recorded showdown
        // matches we opened from a median 132 m while closing at 5.8 m/s, and a quarter of those
        // fights ended in our death although our damage ratio in them was fine: the cost is the
        // walk in. So close at about 1 m/s at most, and take no loot that brings us more than
        // 5 m nearer, unless the target is below 40% HP (the lightning rush on a laser holder is
        // a separate branch).
        let hold_tgt: Option<[f32; 3]> = match &target {
            Some(e) if self.mode == 2 && hold_range_on() && e.id == self.open_id && t < self.open_until && e.hp >= 0.4 * e.max_hp => Some(e.pos),
            _ => None,
        };
        let closes_in = |g: (f32, f32)| hold_tgt.map_or(false, |p| (p[0] - g.0).hypot(p[1] - g.1) < dxy(&p, &me.pos) - 5.0);
        if t < self.unstick_until {
            mv = Mv::Direct([self.unstick_dir.cos(), self.unstick_dir.sin()], true);
        } else if zone_urgent {
            mv = Mv::Goto(zone_point, !me.in_zone || z.ss - t < travel + 4.0);
        } else if let Some(h) = &arc_target {
            let to = [h.pos[0] - here.0, h.pos[1] - here.1];
            let d = to[0].hypot(to[1]).max(0.1);
            if d > 34.0 || !self.w.los(muzzle, h.center()) {
                mv = Mv::Goto((h.pos[0], h.pos[1]), d > 42.0 && me.energy > 14.0);
            } else {
                let out = self.strafe_vec(here, me.pos[2], [to[0] / d, to[1] / d], d, 26.0, t, &z);
                mv = Mv::Direct(out, false);
            }
        } else if let Some(cm) = cover_mv {
            mv = cm;
        } else if let Some(g) = pg_drop {
            // Run for the drop, then keep moving inside its 8 m ring for the 4 s capture; shooting
            // goes on as usual.
            let (dx, dy) = (g.0 - here.0, g.1 - here.1);
            let dd = dx.hypot(dy);
            if dd > 4.5 {
                mv = Mv::Goto(g, me.energy > 30.0 && dd > 15.0);
            } else {
                let r = norm2([here.0 - g.0, here.1 - g.1]);
                let side = if (t * 1.2).sin() > 0.0 { 1.0 } else { -1.0 };
                let tang = [-r[1] * side, r[0] * side];
                let pull = (dd / 4.5).min(1.0);
                mv = Mv::Direct(norm2([tang[0] - r[0] * pull, tang[1] - r[1] * pull]), false);
            }
        } else if let Some(h) = pg_hold {
            mv = h;
        } else if let Some(pm) = peek_mv {
            mv = pm;
        } else if let (Some(r), false) = (run0, cornered) {
            goal_id = Some(r.2);
            mv = Mv::Goto(r.1, me.energy > 40.0);
        } else if let (Some(r), false) = (ffa_rush, cornered) {
            goal_id = Some(r.2);
            // On foot in the opening unless it is right here: boosting from second 1 left us at
            // 11 to 41 energy and hit in the open at 1 AM.
            mv = Mv::Goto(r.1, !ffa_walk && me.energy > 15.0 && (t >= fx(5) || (r.1 .0 - here.0).hypot(r.1 .1 - here.1) < 25.0));
        } else if drone_flee && target.is_none() {
            // Step out of the drones' reach: straight away from their middle.
            let (mut sx, mut sy, mut k) = (0.0, 0.0, 0.0);
            for n in &npcs {
                if dxy(&n.0, &me.pos) < 70.0 {
                    sx += n.0[0];
                    sy += n.0[1];
                    k += 1.0;
                }
            }
            let away = if k > 0.0 { norm2([here.0 - sx / k, here.1 - sy / k]) } else { [0.0, 0.0] };
            let mut g = None;
            for len in [45.0f32, 30.0, 18.0] {
                let p = [here.0 + away[0] * len, here.1 + away[1] * len];
                if in_bounds(&p) && self.w.free_at(p[0], p[1]) {
                    g = Some((p[0], p[1]));
                    break;
                }
            }
            mv = match g {
                Some(g) => Mv::Goto(g, me.energy > 30.0),
                None => Mv::Goto(zone_point, false),
            };
        } else if let (Some(n), true) = (shoot_npc, (drone_hot || threats.is_empty()) && !(lhunt && practice_rush.is_some())) {
            // Circle the drone at a distance where its slow bullets miss a moving mech.
            let to = [n.0[0] - here.0, n.0[1] - here.1];
            let d = to[0].hypot(to[1]).max(0.1);
            let out = self.strafe_vec(here, me.pos[2], [to[0] / d, to[1] / d], d, 60.0, t, &z);
            mv = Mv::Direct(out, false);
        } else if flee_hold {
            mv = Mv::Hold;
        } else if let Some(c) = flee_goal {
            mv = Mv::Goto(c, me.energy > 12.0);
        } else if let (Some(c), true) = (practice_rush, !self.retreating) {
            goal_id = Some(c.2);
            mv = Mv::Goto(c.1, me.energy > 30.0 && c.0 > 15.0);
        } else if let (Some(c), true) = (near_gun, !threats.is_empty() && !self.retreating && !(self.mode == 2 && wounded_hold_on() && hp_frac < 0.6 && near_gun.map_or(false, |c| c.0 > 20.0)) && !near_gun.map_or(false, |c| closes_in(c.1))) {
            goal_id = Some(c.2);
            mv = Mv::Goto(c.1, me.energy > 30.0 && c.0 > 15.0);
        } else if let (Some(c), true) = (near_cube, !threats.is_empty() && !self.retreating && !(self.mode == 2 && wounded_hold_on() && hp_frac < 0.6 && near_cube.map_or(false, |c| c.0 > 20.0)) && !near_cube.map_or(false, |c| closes_in(c.1))) {
            // Fighting or stalking with a cube in reach: collect it while we shoot.
            goal_id = Some(c.2);
            mv = Mv::Goto(c.1, me.energy > 30.0 && c.0 > 15.0);
        } else if let Some(e) = &target {
            // Fight: strafe around the target at the range our weapons like.
            let to = [e.pos[0] - here.0, e.pos[1] - here.1];
            let d = to[0].hypot(to[1]).max(0.1);
            let dir = [to[0] / d, to[1] / d];
            let mut range = self.tune[0];
            // Practice (mode 1): stand off further, by weapon. Switch V_RANGE100 sets 100 m for
            // gun and laser alike; PR_TUNE knobs 15 and 16 do the same in the simulator.
            if self.mode == 1 {
                let pr = if self.vsw(V_RANGE100) {
                    100.0
                } else if self.vsw(V_RANGE40) {
                    40.0
                } else if self.vsw(V_RANGE150) {
                    150.0
                } else { practice::knob(if laser_s.is_some() { 16 } else { 15 }) };
                if pr > 0.0 {
                    range = pr;
                }
            }
            if self.dbi(2) {
                // Design B: the gun holds 80 m, a laser 130 m (90 m against a harmless bot); a
                // nearly dead target (under a third) is closed on at point-blank while we are
                // above 40% and at most two are near.
                range = if laser_s.is_some() { if e.hostile_until > t { dbk(6) } else { 90.0 } } else { dbk(5) };
                let near2 = threats.iter().filter(|o| dxy(&o.pos, &me.pos) < 150.0).count() <= 2;
                if e.hp < 0.33 * e.max_hp && hp_frac > 0.4 && near2 {
                    range = 25.0;
                }
            }
            if light_s.is_some() && me.energy > 25.0 {
                range = 30.0;
            } else if e.has("lightning") {
                range = range.max(65.0);
            }
            // Sidestep timing (V_SIDESTEP): no reversal sooner than the bullet's flight time over
            // this distance (252 m/s), and one reversal, held for a second, when a laser that is
            // aimed at us starts to charge.
            let side = self.side_on();
            if side {
                let lc = threats.iter().find(|o| o.has("laser") && o.charging && wrap(aim_to(o.eye(), my_center)[0] - o.aim[0]).abs() < 0.12).map(|o| o.id);
                if let Some(id) = lc {
                    if id != self.side_id || t - self.side_t > 2.0 {
                        self.strafe_dir = -self.strafe_dir;
                        self.strafe_until = self.strafe_until.max(t + 1.0);
                        self.side_id = id;
                        self.side_t = t;
                    }
                }
                self.strafe_floor = d / 252.0;
            }
            if hold_tgt.is_some() {
                range = range.max(d - 2.0);
            }
            let out = self.strafe_vec(here, me.pos[2], dir, d, range, t, &z);
            self.strafe_floor = 0.0;
            let burst = me.energy > 55.0 && (t * 1.3).sin() > 0.3;
            // First hunter from cover (V_COVERSPOT): at the waiting spot, with the hunter still
            // far off, stay put and shoot from there.
            let hold_cover = cover_open_on && d > 45.0 && self.hide_arrived >= 0.0 && self.hide.map_or(false, |h| dxy(&[h.0, h.1, 0.0], &me.pos) < 6.0);
            mv = if hold_cover { Mv::Hold } else { Mv::Direct(out, burst) };
        } else if let Some(g) = if self.mode == 1 && target.is_none() { self.practice_finish_goal(&me, t) } else { None } {
            // Practice: follow up a wounded hunter that broke off.
            mv = Mv::Goto(g, me.energy > 40.0);
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
            let mut goal: Option<(f32, (f32, f32), i64)> = None;
            let bl = &self.blacklist;
            let en = &self.enemies;
            let da = &self.drop_avoid;
            let (wref, lim) = (&*self.w, self.cover_lim);
            let danger = |p: &[f32; 3]| cautious && (en.values().any(|e| t - e.seen < 12.0 && dxy(&e.pos, p) < 90.0) || da.iter().any(|q| (q[0] - p[0]).hypot(q[1] - p[1]) < 60.0));
            let calm = !cautious || en.values().all(|e| t - e.seen > 8.0);
            let cover_open = cover_open_on;
            // Free-for-all, badly hurt: regenerate where we are rather than walk out for loot (9 AM:
            // killed at 21% HP in the open right after a kill).
            let lie_low = self.mode == 2 && wounded_hold_on() && hp_frac < 0.6;
            let consider = |score: f32, p: &[f32; 3], id: i64, goal: &mut Option<(f32, (f32, f32), i64)>| {
                if bl.get(&id).map_or(false, |u| t < *u) {
                    return;
                }
                if lie_low && dxy(p, &me.pos) > 20.0 {
                    return;
                }
                if lim.is_finite() && open_run_on(wref, here, (p[0], p[1])) > lim {
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
                    // Free-for-all (confirmed): commit or stay away. In recorded matches the mech
                    // that took a drop usually went on to win, drops were rarely contested, and
                    // loitering near one without entering was the dangerous choice. So go only
                    // when at most 2 tracked mechs are within 100 m of it (FFA_DROP_RULE; formerly
                    // none) and we are at 50% HP or more, and time the walk to arrive as
                    // it lands rather than wait in the open.
                    let ffa_drop = self.mode == 2
                        && calm
                        && dist < 200.0
                        && (if FFA_DROP_RULE {
                            hp_frac >= 0.5 && en.values().filter(|e| t - e.seen < 15.0 && dxy(&e.pos, &p) < 100.0).count() <= 2
                        } else {
                            !en.values().any(|e| t - e.seen < 15.0 && dxy(&e.pos, &p) < 100.0)
                        })
                        && f(&d["lands_in"]) < dist / me.walk + 3.0;
                    let other_drop = self.mode != 2 && f(&d["lands_in"]) < dist / me.walk + 10.0 && (!cautious || (calm && dist < 120.0 && !danger(&p)));
                    if in_bounds(&p) && dist < 300.0 && (ffa_drop || other_drop) {
                        consider(dist * 0.4, &p, d["id"].as_i64().unwrap_or(-1), &mut goal);
                    }
                }
            }
            for (id, it) in self.cubes.iter().filter(|_| !cube_stop) {
                if in_bounds(&it.pos) && (it.pos[2] - me.pos[2]).abs() < 6.0 && !danger(&it.pos) {
                    consider(dxy(&it.pos, &me.pos) * 0.5, &it.pos, *id, &mut goal);
                }
            }
            let want_laser = laser_s.is_none();
            let slot_free = me.wpn[1].is_none() || me.wpn[0].is_none();
            for (id, it) in &self.pickups {
                let wanted = ((slot_free || (self.tune[12] > 0.5 && gun_s.is_some())) && (it.kind == "laser" || it.kind == "lightning") && slot_of(&it.kind).is_none())
                    || (!slot_free && want_laser && it.kind == "laser" && light_s.is_some());
                // Opening (match type not known yet, or practice still guarded): no walk to a weapon
                // across open streets. 10 AM: hit at 6 s on a walk to a weapon 71 m away in the open.
                let opening = (self.mode == 0 || p_lite) && self.open_safe();
                let d_it = dxy(&it.pos, &me.pos);
                if opening && d_it > 25.0 && self.open_run(here, (it.pos[0], it.pos[1])) > 40.0 {
                    continue;
                }
                if wanted && !danger(&it.pos) && (!cautious || it.pos[2] - me.pos[2] <= 3.0) && in_bounds(&it.pos) && (it.pos[2] - self.w.surf_at(it.pos[0], it.pos[1])).abs() < 2.0 {
                    let pref = if it.kind == "laser" { 0.6 } else { 0.8 };
                    consider(dxy(&it.pos, &me.pos) * pref, &it.pos, *id, &mut goal);
                }
            }
            if (hp_frac > 0.85 || eager) && calm && !cube_stop && (!cautious || t >= self.tune[18]) {
                for (id, it) in &self.crates {
                    if in_bounds(&it.pos) && !danger(&it.pos) && (!cautious || it.pos[2] - me.pos[2] <= 3.0) {
                        consider(dxy(&it.pos, &me.pos) * 1.0 + 10.0, &it.pos, *id, &mut goal);
                        let c = [it.pos[0], it.pos[1], it.pos[2] + 1.2];
                        let dd = d3(muzzle, c);
                        if dd < 70.0 && self.w.los(muzzle, c) && shoot_crate.map_or(true, |s| dd < d3(muzzle, s)) {
                            shoot_crate = Some(c);
                        }
                    }
                }
            }
            // Scavenge (confirmed free-for-all only): a third of all kill drops in recorded
            // matches were never collected. Once a death site has been quiet for a while and no
            // mech has been seen near it, go and look; anything there shows up as cubes or weapons
            // and the rules above take over.
            if self.mode == 2 && scav_on() && goal.is_none() && calm && hp_frac > 0.7 {
                if let Some(g) = self.scav_goal {
                    let p3 = [g.0[0], g.0[1], me.pos[2]];
                    if t - g.1 > 30.0 || dxy(&p3, &me.pos) < 10.0 || !in_bounds(&p3) || danger(&p3) {
                        self.scav.retain(|x| (x.0[0] - g.0[0]).hypot(x.0[1] - g.0[1]) > 30.0);
                        self.scav_goal = None;
                    }
                }
                if self.scav_goal.is_none() {
                    let mut best: Option<(f32, [f32; 2])> = None;
                    for x in &self.scav {
                        let p3 = [x.0[0], x.0[1], me.pos[2]];
                        let d = dxy(&p3, &me.pos);
                        if t - x.1 < 15.0 || d < 10.0 || !in_bounds(&p3) || danger(&p3) || !self.w.free_at(x.0[0], x.0[1]) {
                            continue;
                        }
                        if best.map_or(true, |b| d < b.0) {
                            best = Some((d, x.0));
                        }
                    }
                    self.scav_goal = best.map(|b| (b.1, t));
                }
                if let Some(g) = self.scav_goal {
                    goal = Some((dxy(&[g.0[0], g.0[1], 0.0], &me.pos), (g.0[0], g.0[1]), -2));
                }
            }
            if let Some(r) = errand {
                goal = Some(r);
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
                    } else if cautious || cover_open {
                        // Nothing to collect: wait in an enclosed spot, and hop to a new one now and
                        // then so we see more of the map.
                        let stale = match self.hide {
                            Some(h) => {
                                let hp3 = [h.0, h.1, 0.0];
                                // Free-for-all: no hop to a new spot until we have regenerated
                                // (9 AM: stepped back into view at 56% HP and died in 1.9 s).
                                let hop = if self.mode == 2 && wounded_hold_on() && hp_frac < 0.9 {
                                    f32::INFINITY
                                } else if (self.mode == 2 || cover_open) && hide_table_on() {
                                    hide_knob(1)
                                } else {
                                    self.tune[17]
                                };
                                !in_bounds(&hp3) || danger(&hp3) || (self.hide_arrived >= 0.0 && t - self.hide_arrived > hop) || t - self.hide_t > 60.0
                            }
                            None => true,
                        };
                        if stale {
                            let first = self.hide.is_none();
                            // Confirmed free-for-all: go by where recorded mechs actually walk and
                            // look. The enclosed-nook rule below picked spots about five times
                            // more exposed than the best one in reach.
                            let by_table = if (self.mode == 2 || cover_open) && hide_table_on() { self.pick_hide_risk(&me, &z, t, hide_knob(2)) } else { None };
                            self.hide = by_table.or_else(|| self.pick_hide(&me, &z, t, if first { 0.0 } else { 40.0 }, if first { 60.0 } else { 110.0 }));
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
                                if self.w.free_at(p.0, p.1) && !self.w.prism[self.w.cell(p.0, p.1)] && !self.drop_avoid.iter().any(|q| (q[0] - p.0).hypot(q[1] - p.1) < 60.0) && (self.cover_lim.is_infinite() || self.open_run(here, p) <= self.cover_lim) {
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

        self.drop_goal = match goal_id {
            Some(g) if (20000..30000).contains(&g) => g,
            _ => -1,
        };
        let weave_ok = matches!(mv, Mv::Goto(..));
        let flee_hold_now = flee_hold && matches!(mv, Mv::Hold);
        let mut jump = false;
        let (mut mvec, mut boost) = match mv {
            Mv::Goto(g, b) => {
                let (v, at_end) = self.follow(&me, g, t);
                let mut out = (v, b);
                let gc = self.w.cell(g.0, g.1);
                let gs = self.w.surf[gc];
                let dgoal = (g.0 - here.0).hypot(g.1 - here.1);
                let roof_goal = self.w.prism[gc] && gs > me.pos[2] + 1.5;
                // Free-for-all: the zone point, or a point we must reach, cannot be walked to (an
                // enclosed pocket: 7 AM, 3rd place with full HP burned to death in one). Jet over
                // the wall toward it, using the climb below.
                let walled_in = (self.mode == 2 || pf(1))
                    && me.grounded
                    && (zone_urgent || !inside_next)
                    && dgoal > 8.0
                    && (at_end || self.path.is_empty())
                    && t >= self.climb_block_until;
                if walled_in && t >= self.escape_until {
                    self.escape_until = t + 6.0;
                    self.climb_until = t + 6.0;
                    self.climb_goal = g;
                }
                let escaping = t < self.escape_until;
                if at_end && dgoal > 2.5 && me.grounded && !walled_in {
                    if roof_goal && dgoal < 60.0 && t >= self.climb_block_until {
                        // The goal is on a roof we cannot walk onto: jet up from here.
                        self.climb_until = t + 9.0;
                        self.climb_goal = g;
                    } else if let Some(id) = goal_id {
                        self.blacklist.insert(id, t + 90.0);
                    }
                }
                if escaping && (self.climb_goal.0 - g.0).hypot(self.climb_goal.1 - g.1) >= 3.0 {
                    self.climb_goal = g;
                }
                if t < self.climb_until && (self.climb_goal.0 - g.0).hypot(self.climb_goal.1 - g.1) < 3.0 {
                    if me.grounded && !roof_goal && !escaping {
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
                            // Rising costs about 1.5 energy a metre; hovering across onto the roof
                            // costs as much again. Short of that we only fall back down.
                            let need = (wall_top - me.pos[2]).max(0.0) * 2.2 + 18.0;
                            if (self.climb_try_goal.0 - g.0).hypot(self.climb_try_goal.1 - g.1) > 6.0 {
                                self.climb_try_goal = g;
                                self.climb_tries = 0;
                            }
                            if need > me.max_energy || self.climb_tries >= 2 {
                                // Out of reach, or two jumps did not get us up: stop trying here.
                                self.climb_block_until = t + 45.0;
                                self.avoid.push((g, t + 90.0));
                                self.climb_until = -1.0;
                                if let Some(id) = goal_id {
                                    self.blacklist.insert(id, t + 300.0);
                                }
                            } else if me.energy >= need {
                                jump = true;
                                self.climb_tries += 1;
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

        // Shot at while walking somewhere: swing side to side around the heading, reversing like
        // a strafe. Off by default (tune[19] = 0): neutral in simulation, and it slows the walk.
        if weave_ok && self.tune[19] > 0.0 && t - self.last_hurt_t < 1.2 && me.grounded && t >= self.climb_until && mvec[0].hypot(mvec[1]) > 0.8 {
            if t >= self.strafe_until {
                if self.rand() < 0.85 {
                    self.strafe_dir = -self.strafe_dir;
                }
                self.strafe_until = t + self.tune[6] + self.tune[7] * self.rand();
            }
            let sd = self.strafe_dir * self.tune[19];
            let cand = norm2([mvec[0] - mvec[1] * sd, mvec[1] + mvec[0] * sd]);
            if self.open_dir(here, me.pos[2], cand) {
                mvec = cand;
            }
        }

        // ---- stuck detection (only when we mean to travel)
        let travelling = matches!(mv, Mv::Goto(..)) && mvec[0].hypot(mvec[1]) > 0.6;
        if !travelling || t < self.unstick_until {
            self.stuck_pos = me.pos;
            self.stuck_t = t;
        } else if t - self.stuck_t >= 1.0 {
            if d3(me.pos, self.stuck_pos) < 0.8 {
                self.path.clear();
                let g = match mv {
                    Mv::Goto(g, _) => g,
                    _ => here,
                };
                if (g.0 - self.stuck_goal.0).hypot(g.1 - self.stuck_goal.1) < 8.0 && t - self.stuck_goal_t < 10.0 {
                    self.stuck_count += 1;
                } else {
                    self.stuck_count = 1;
                }
                self.stuck_goal = g;
                self.stuck_goal_t = t;
                if self.stuck_count >= 2 {
                    // The map model says this is reachable and the world disagrees. Drop the goal
                    // rather than spend the match bouncing off it.
                    if let Some(id) = goal_id {
                        self.blacklist.insert(id, t + 90.0);
                    }
                    self.avoid.push((g, t + 90.0));
                    self.hide = None;
                    self.cover = None;
                    self.wander = None;
                    self.stuck_count = 0;
                }
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
            // A hop and a sidestep; boosting here only empties the tank against a wall.
            boost = !me.in_zone;
            if me.grounded {
                jump = true;
            }
        }
        self.avoid.retain(|a| t < a.1);
        if (self.mode != 1 || p_lite) && run0.is_none() && t < fx(5) && me.energy < fx(6) && me.in_zone && !self.enemies.values().any(|e| t - e.hurt_me_t < 3.0) {
            // Opening: keep the tank for breaking line of sight. Boost only to escape a mech that
            // is hitting us.
            boost = false;
        }
        if light_s.is_some() && me.energy < fx(3) && me.in_zone && threats.iter().any(|e| dxy(&e.pos, &me.pos) < fx(4)) {
            // Lightning needs energy at close range: in recorded close fights, sides that had not
            // jetted or boosted in the 5 s before won 69%, against 32%.
            boost = false;
        }
        if boost && me.energy < 3.0 {
            boost = false;
        }

        // ---- aim and fire
        // How long a full laser charge has been held. The server reports charge only up to full
        // (0.75 s) and auto-fires 2 s later, so a cap on the reported value never triggers: 15 of
        // our 21 misses on charges held past 0.9 s ran to the 2.75 s auto-fire into a wall after
        // the target stepped behind cover.
        let full_held = match laser_s.and_then(|s| me.wpn[s].as_ref()) {
            Some(w) if w.charge >= w.full_charge - 0.06 => {
                if self.full_since < 0.0 {
                    self.full_since = t;
                }
                t - self.full_since
            }
            _ => {
                self.full_since = -1.0;
                0.0
            }
        };
        let timed_hold = (self.mode == 2 || pf(0)) && std::env::var("MB_NOTIMEDHOLD").is_err();
        let mut fire = [false, false];
        let mut laser_release = false;
        let mut aim;
        let mut reload = false;
        if let Some(e) = &target {
            // Centre of the body, or its upper part when only that is in our line of fire.
            let top = self.mode != 2 && fr(1) && !self.w.los(muzzle, e.center()) && self.w.los(muzzle, e.upper());
            let c0 = if top { e.upper() } else { e.center() };
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
                wrap(a[0] - me.aim[0]).abs() * dd < 2.2 * scale && (a[1] - me.aim[1]).abs() * dd < (if top { 1.2 } else { 3.6 }) * scale
            };
            // The gun's own gate is a little wider outside a confirmed free-for-all.
            let gun_w = if self.mode != 2 && fr(0) { 3.0 } else { 2.2 };
            let on_gun = |pt: [f32; 3]| -> bool {
                let a = aim_to(muzzle, pt);
                let dd = dxy(&pt, &muzzle).max(1.0);
                wrap(a[0] - me.aim[0]).abs() * dd < gun_w * scale && (a[1] - me.aim[1]).abs() * dd < (if top { 1.2 } else { 3.6 }) * scale
            };
            let direct_lead = if self.mode != 2 && fr(2) { 0.15 } else { 0.3 };
            let mut aim_pt = p;
            // Laser: charge while we have the line, release at full charge when on target.
            if let Some(s) = laser_s {
                let w = me.wpn[s].as_ref().unwrap();
                if d < w.range - 5.0 && w.cooldown <= 0.1 {
                    let ready = w.charge >= w.full_charge - 0.06;
                    // Early release: the charge so far (35% of full at none, linear to 100%, times
                    // our cube bonus, 1.6 against a mortar carrier) already covers its visible HP.
                    let early = laser_early_on() && w.charge >= 0.16 && t - e.seen < 0.05 && {
                        let frac = 0.35 + 0.65 * (w.charge / w.full_charge.max(0.01)).min(1.0);
                        let dmg = w.damage * frac * (1.0 + 0.04 * me.cubes.min(10.0)) * if e.has("mortar") { 1.6 } else { 1.0 };
                        dmg >= LASER_EARLY_MARGIN * e.hp && self.w.los(muzzle, c0)
                    };
                    if w.charge >= w.full_charge - direct_lead || early {
                        aim_pt = direct;
                    }
                    // Release at full charge, on target, and only once the target has been in our
                    // line of fire for a moment: in recorded matches, shots within 0.3 s of the
                    // target appearing hit 38%, full charges after that 84 to 96%.
                    let settled = fx(0) < 0.5 || t - self.los_since >= fx(2) || (timed_hold && full_held >= 0.4);
                    if (ready && settled || early) && on(direct) {
                        laser_release = true;
                    }
                    fire[s] = !laser_release;
                }
            }
            if let Some(s) = gun_s {
                let w = me.wpn[s].as_ref().unwrap();
                if d < w.range - 3.0 && w.ammo > 0.0 && w.reload <= 0.0 && on_gun(p) {
                    fire[s] = true;
                }
            }
            aim = aim_to(muzzle, aim_pt);
        } else if t - self.ghost_t < 1.5 {
            // Hit by a mech we cannot see: look that way first. Tracking someone else while a
            // sniper works on us cost 990 HP for nothing in a recorded match.
            aim = [self.hurt_bearing, 0.0];
        } else if let Some(e) = threats.iter().min_by(|a, b| dxy(&a.pos, &me.pos).partial_cmp(&dxy(&b.pos, &me.pos)).unwrap()) {
            // Pre-aim where a tracked enemy will appear; keep a laser charged for it.
            aim = aim_to(muzzle, e.center());
            if let Some(s) = laser_s {
                let w = me.wpn[s].as_ref().unwrap();
                // Also when we are hiding and it is walking up on us: meet it with a full charge.
                let ambush_hold = self.tune[23] > 0.5 && self.retreating && flee_hold_now && dxy(&e.pos, &me.pos) < 80.0;
                // Outside a confirmed free-for-all, with our own charge timer: keep the full charge
                // for the tracked mech rather than spend it on the wall after 0.3 s.
                let long_hold = self.mode != 2 && fr(3) && timed_hold;
                let hold_cap = if ambush_hold { 1.6 } else if long_hold { 1.5 } else if fx(0) > 0.5 { fx(1) } else { 1.4 };
                let within_cap = if timed_hold { full_held < hold_cap } else { w.charge < w.full_charge + hold_cap };
                if (!self.retreating || ambush_hold) && w.cooldown <= 0.1 && dxy(&e.pos, &me.pos) < 150.0 && within_cap {
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
                    laser_release = w.charge >= w.full_charge - 0.06 && on;
                    fire[s] = !laser_release;
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
                    laser_release = w.charge >= w.full_charge - 0.06 && on;
                    fire[s] = !laser_release;
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
            } else if mvec[0].hypot(mvec[1]) > 0.3 && !(self.mode == 2 && last4_on() && alive <= 4) {
                let base = mvec[1].atan2(mvec[0]);
                aim = [wrap(base + 1.5 * (t * 1.4).sin()), 0.0];
            } else {
                aim = [wrap(self.last_aim[0] + 0.33), 0.0];
            }
        }
        // A started charge is let go only on purpose. Any branch that does not hold the trigger
        // (line of fire lost for a tick, a target switch, no target) would otherwise fire a weak,
        // unaimed shot: 4 of our 19 laser shots at 12 AM went off at 0.15 to 0.3 s and all missed.
        // Past full charge plus a short wait we let go anyway, rather than crawl at half speed
        // until the shot fires on its own.
        if let Some(s) = laser_s {
            let w = me.wpn[s].as_ref().unwrap();
            let within_cap = if timed_hold { full_held < fx(1) } else { w.charge < w.full_charge + fx(1) };
            if fx(0) > 0.5 && w.charge > 0.0 && !fire[s] && !laser_release && within_cap {
                fire[s] = true;
            }
        }
        // Replay marker: the first time we are sure of a free-for-all and nothing is in view, look
        // straight up for 0.4 s without firing. It shows in the public replays which of our builds
        // a showdown is running (marker v1 = rel-ffa9 onward).
        let charging_now = laser_s.and_then(|s| me.wpn[s].as_ref()).map_or(false, |w| w.charge > 0.0);
        if self.mode == 2 && threats.is_empty() && !charging_now && t - self.last_hurt_t > 3.0 && (self.marker_t < 0.0 || t - self.marker_t < 0.4) {
            if self.marker_t < 0.0 {
                self.marker_t = t;
            }
            aim = [me.aim[0], 1.2];
            fire = [false, false];
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
        // Practice break-and-heal (knob 23): dealing damage also resets the regeneration clock, so
        // hold fire while healing. Only this rule sets `retreating` in practice.
        if self.retreating && (self.heal_on() || (self.regroup && (self.vsw(V_REGROUP) || pg(1))) || (self.dbi(4) && !self.db_nocover)) {
            fire = [false, false];
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
            let lasers: Vec<String> = self.pickups.iter().filter(|(_, it)| it.kind == "laser").map(|(id, it)| format!("{}@{:.0}m{}{}", id, dxy(&it.pos, &me.pos), if self.blacklist.get(id).map_or(false, |u| t < *u) { "B" } else { "" }, if (it.pos[2] - self.w.surf_at(it.pos[0], it.pos[1])).abs() > 2.0 { "R" } else { "" })).collect();
            self.dbg = format!("mode{} hp{:.0} {} retreat={} cover={:?} tgt={} fire={:?} w1={} pr={:?} plite={} L[{}] thr:{}", self.mode, me.hp, mvk, self.retreating, self.cover.map(|c| (c.0 as i32, c.1 as i32)), target.is_some(), fire, me.wpn[1].as_ref().map_or("-".to_string(), |w| w.kind.clone()), practice_rush.map(|c| (c.0 as i32, c.2)), p_lite, lasers.join(","), es);
        }
        let r3 = |x: f32| (x * 1000.0).round() / 1000.0;
        let mut a = Map::new();
        a.insert("tick".into(), obs["tick"].clone());
        a.insert("move".into(), json!([r3(mvec[0]), r3(mvec[1])]));
        a.insert("boost".into(), json!(boost));
        if jump && target.is_some() && t - self.last_hurt_t < 2.0 && self.side_on() {
            jump = false; // a mech in the air cannot change direction
        }
        a.insert("jump".into(), json!(jump));
        a.insert("aim".into(), json!([r3(aim[0]), r3(aim[1])]));
        a.insert("fire".into(), json!(fire));
        a.insert("reload".into(), json!(reload));
        if let Some(ch) = obs["offer"]["choices"].as_array() {
            let mut bi = 0usize;
            let mut bs = i32::MIN;
            for (i, c) in ch.iter().enumerate() {
                let id = c["id"].as_str().unwrap_or("");
                let s = if self.mode == 1 { if self.vsw(V_SHAREDUP) { None } else { practice::upgrade_score(id) } } else { None }.unwrap_or_else(|| Self::upgrade_score(id));
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
