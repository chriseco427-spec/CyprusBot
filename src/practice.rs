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
pub(super) fn knob(i: usize) -> f32 {
    static K: OnceLock<[f32; 15]> = OnceLock::new();
    K.get_or_init(|| {
        let mut k = [1.0, 300.0, 0.0, 25.0, 1.0, 50.0, 1.0, 15.0, 25.0, 25.0, 0.0, 0.0, 0.55, 0.5, 1.0];
        if let Ok(e) = std::env::var("PR_TUNE") {
            for (j, x) in e.split(',').enumerate().take(15) {
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
    pub(super) fn practice_weapon_goal(&self, me: &Me, t: f32, has_laser: bool, has_light: bool, in_bounds: &dyn Fn(&[f32]) -> bool) -> Option<(f32, (f32, f32), i64)> {
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
            if knob(6) > 0.5 && t < knob(8) && self.enemies.values().any(|e| t - e.seen < 6.0 && dxy(&e.pos, &it.pos) < d + knob(7)) {
                continue;
            }
            let score = d * pref + if roof { knob(3) } else { 0.0 };
            if best.map_or(true, |b| score < b.0) {
                best = Some((score, (it.pos[0], it.pos[1]), *id));
            }
        }
        best
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
        if knob(12) <= 0.0 || me.hp / me.max_hp.max(1.0) <= knob(13) {
            return None;
        }
        self.enemies
            .values()
            .filter(|e| t - e.seen < 6.0 && e.hit_by_me && e.hp / e.max_hp.max(1.0) < knob(12) && dxy(&e.pos, &me.pos) < 200.0)
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
