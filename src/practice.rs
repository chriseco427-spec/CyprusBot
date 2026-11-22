//! Practice-mode behaviour (house-squad matches). Kept apart from bot.rs so it can be tuned
//! without touching the free-for-all logic. A child module of `bot`, so it sees Bot's fields.

use super::{dxy, Bot, Enemy, Me};
use std::sync::OnceLock;

/// Practice knobs. Defaults ship; `PR_TUNE=a,b,c` overrides them for simulator experiments.
/// 0: laser rush on (1) / off (0)   1: laser reach m   2: lightning reach m   3: roof penalty m
/// 4: trade lightning for a laser (1) / keep it (0)   5: no enemy closer than this (m) while swapping
/// 6: skip weapons another mech is nearer to (1) / ignore (0)   7: margin m for that test
/// 8: only apply that test before this match time (s)   9: highest roof (m above us) worth a climb
/// 12: follow up a hunter we wounded below this HP fraction (0 = off)   13: only while our HP
///     fraction is above this
/// 14: offense-first upgrades in practice (1) / shared order (0)
/// 15: practice fight range (m) with the gun only (0 = the shared range)
/// 16: practice fight range (m) while we hold a laser (0 = the shared range)
/// 17-22: reserved
/// 23: break-and-heal (1): with HP below knob 25 and at least knob 26 mechs tracked (0 = any),
///     break line of sight and hold fire until regenerated to 90%. Set per salt group by the V_HEAL switch (salts::VARIANT bit 4), or here for the simulator.
/// 24: kills needed first (0 = none)   25: HP fraction below which it starts   26: mechs tracked
pub(super) fn knob(i: usize) -> f32 {
    static K: OnceLock<[f32; 27]> = OnceLock::new();
    K.get_or_init(|| {
        let mut k = [1.0, 250.0, 0.0, 25.0, 1.0, 50.0, 1.0, 15.0, 25.0, 25.0, 0.0, 0.0, 0.65, 0.5, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.35, 3.0]; // k[23] is 0: the per-group switch turns the rule on
        if let Ok(e) = std::env::var("PR_TUNE") {
            for (j, x) in e.split(',').enumerate().take(27) {
                k[j] = x.parse().unwrap_or(k[j]);
            }
        }
        k
    })[i]
}

impl Bot {
    /// A weapon worth running for, as (score, (x, y), pickup id). In practice the hunters come to
    /// us whatever we do, so a laser picked up before the first fight is worth a long detour:
    /// in the simulator, matches where we never hold a laser score about 1.6 points lower.
    pub(super) fn practice_weapon_goal(&self, me: &Me, t: f32, has_laser: bool, has_light: bool, in_bounds: &dyn Fn(&[f32]) -> bool, hunt: bool) -> Option<(f32, (f32, f32), i64)> {
        if knob(0) < 0.5 {
            return None;
        }
        let slot_free = me.wpn[0].is_none() || me.wpn[1].is_none() || (knob(4) > 0.5 && has_light);
        if !slot_free || has_laser {
            return None;
        }
        let mut best: Option<(f32, (f32, f32), i64)> = None;
        for (id, it) in &self.pickups {
            if self.blacklist.get(id).map_or(false, |u| t < *u) || !in_bounds(&it.pos) {
                continue;
            }
            let (reach, pref) = match it.kind.as_str() {
                "laser" => (knob(1), 0.6),
                "lightning" if !has_light => (knob(2), 1.0),
                _ => continue,
            };
            let d = dxy(&it.pos, &me.pos);
            // Ground pickups, or roofs low enough to jet onto from nearby.
            let ground = self.w.surf_at(it.pos[0], it.pos[1]);
            let roof = (it.pos[2] - ground).abs() > 2.0 || it.pos[2] - me.pos[2] > 3.0;
            if d > reach || (roof && it.pos[2] - me.pos[2] > knob(9)) {
                continue;
            }
            // Leave a weapon another mech is closer to. Insurance for a free-for-all mistaken for
            // practice (nobody dead yet at 8 s), where running past armed mechs loses duels.
            if !hunt && knob(6) > 0.5 && t < knob(8) && self.enemies.values().any(|e| t - e.seen < 6.0 && dxy(&e.pos, &it.pos) < d + knob(7)) {
                continue;
            }
            let score = d * pref + if roof { knob(3) } else { 0.0 };
            if best.map_or(true, |b| score < b.0) {
                best = Some((score, (it.pos[0], it.pos[1]), *id));
            }
        }
        best
    }

    /// Design B: the next errand between kills, in a fixed order: a laser when we hold none (out
    /// to 250 m), any laser or lightning for an empty slot (150 m), cubes (90 m), the place a mech
    /// died 3 to 30 s ago (130 m). Ground items only, nothing in the blacklist.
    pub(super) fn practice_errand(&self, me: &Me, t: f32, has_laser: bool, in_bounds: &dyn Fn(&[f32]) -> bool) -> Option<(f32, (f32, f32), i64)> {
        let ok = |id: &i64, pos: &[f32; 3]| -> bool {
            !self.blacklist.get(id).map_or(false, |u| t < *u) && in_bounds(pos) && (pos[2] - self.w.surf_at(pos[0], pos[1])).abs() < 2.0 && pos[2] - me.pos[2] < 3.0
        };
        let nearest = |kinds: &[&str], reach: f32| -> Option<(f32, (f32, f32), i64)> {
            let mut best: Option<(f32, (f32, f32), i64)> = None;
            for (id, it) in &self.pickups {
                if !kinds.contains(&it.kind.as_str()) || !ok(id, &it.pos) {
                    continue;
                }
                let d = dxy(&it.pos, &me.pos);
                if d <= reach && best.map_or(true, |b| d < b.0) {
                    best = Some((d, (it.pos[0], it.pos[1]), *id));
                }
            }
            best
        };
        if !has_laser {
            if let Some(g) = nearest(&["laser"], 250.0) {
                return Some(g);
            }
        }
        if me.wpn[1].is_none() || me.wpn[0].is_none() {
            if let Some(g) = nearest(&["laser", "lightning"], 150.0) {
                return Some(g);
            }
        }
        let mut best: Option<(f32, (f32, f32), i64)> = None;
        for (id, it) in &self.cubes {
            if self.blacklist.get(id).map_or(false, |u| t < *u) || !in_bounds(&it.pos) || (it.pos[2] - me.pos[2]).abs() > 5.0 {
                continue;
            }
            let d = dxy(&it.pos, &me.pos);
            if d <= 90.0 && best.map_or(true, |b| d < b.0) {
                best = Some((d, (it.pos[0], it.pos[1]), *id));
            }
        }
        if best.is_some() {
            return best;
        }
        for x in &self.scav {
            let age = t - x.1;
            let p3 = [x.0[0], x.0[1], me.pos[2]];
            let d = dxy(&p3, &me.pos);
            if age < 3.0 || age > 30.0 || d < 10.0 || d > 130.0 || !in_bounds(&p3) || !self.w.free_at(x.0[0], x.0[1]) {
                continue;
            }
            if best.map_or(true, |b| d < b.0) {
                best = Some((d, (x.0[0], x.0[1]), -2));
            }
        }
        best
    }

    /// PG_DROP_REPAIR: a supply drop worth breaking off for, as its position: HP below 80%, within
    /// 200 m and inside the zone, landing about when we can get there (or landed), and not one
    /// somebody else is already capturing.
    pub(super) fn pg_drop_goal(&self, me: &Me, t: f32, obs: &serde_json::Value, in_bounds: &dyn Fn(&[f32]) -> bool) -> Option<(f32, f32)> {
        if me.hp / me.max_hp.max(1.0) >= 0.80 {
            return None;
        }
        let my_id = obs["self"]["id"].as_i64().unwrap_or(-1);
        let _ = t;
        let mut best: Option<(f32, (f32, f32))> = None;
        for d in obs["drops"].as_array()? {
            let p = [d["pos"][0].as_f64()? as f32, d["pos"][1].as_f64()? as f32, d["pos"][2].as_f64().unwrap_or(0.0) as f32];
            let dist = dxy(&p, &me.pos);
            let lands_in = d["lands_in"].as_f64().unwrap_or(99.0) as f32;
            let holder = d["holder"].as_i64();
            let theirs = holder.map_or(false, |h| h != my_id) && d["progress"].as_f64().unwrap_or(0.0) > 0.0;
            if theirs || dist > 200.0 || !in_bounds(&p) || lands_in > dist / me.walk.max(1.0) + 3.0 {
                continue;
            }
            if best.map_or(true, |b| dist < b.0) {
                best = Some((dist, (p[0], p[1])));
            }
        }
        best.map(|b| b.1)
    }

    /// Practice only: drop lightning for a laser when no enemy is close enough to punish the
    /// 1.65 s the new laser needs before it can fire.
    pub(super) fn practice_swap_ok(&self, me: &Me, threats: &[Enemy]) -> bool {
        self.mode == 1 && knob(4) > 0.5 && !threats.iter().any(|e| dxy(&e.pos, &me.pos) < knob(5))
    }

    /// Practice: where to go to finish a hunter we wounded that broke off. House experts break off
    /// at about 52% HP and come back 23 times in 40 (private/HOUSE.md); in practice every kill is
    /// a point, and a hunter we let go regenerates.
    pub(super) fn practice_finish_goal(&self, me: &Me, t: f32) -> Option<(f32, f32)> {
        let follow = if self.vsw(super::V_FOLLOW55) { 0.55 } else { knob(12) };
        if follow <= 0.0 || me.hp / me.max_hp.max(1.0) <= knob(13) {
            return None;
        }
        self.enemies
            .values()
            .filter(|e| t - e.seen < 6.0 && e.hit_by_me && e.hp / e.max_hp.max(1.0) < follow && dxy(&e.pos, &me.pos) < 200.0)
            .min_by(|a, b| a.hp.partial_cmp(&b.hp).unwrap_or(std::cmp::Ordering::Equal))
            .map(|e| (e.pos[0], e.pos[1]))
    }
}

/// Practice upgrade order: points come from kills, so damage first. Laser damage is +18% a
/// stack on our main killer; laser charge cuts the charge from 0.75 s to 0.56 s. Plating stays
/// high: max HP still decides how many hunters we outlast. None = use the shared order.
pub(super) fn upgrade_score(id: &str) -> Option<i32> {
    if knob(14) < 0.5 {
        return None;
    }
    Some(match id {
        "laser_damage" => 105,
        "plating" => 100,
        "laser_charge" => 97,
        "armor" => 95,
        "gun_damage" => 90,
        "gun_rate" => 88,
        "lightning_efficiency" => 85,
        "nanites" => 80,
        _ => return None,
    })
}
