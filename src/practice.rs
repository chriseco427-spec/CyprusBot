//! Practice-mode behaviour (house-squad matches). Kept apart from bot.rs so it can be tuned
//! without touching the free-for-all logic. A child module of `bot`, so it sees Bot's fields.

use super::{dxy, Bot, Enemy, Me};
use std::sync::OnceLock;

/// Practice knobs. Defaults ship; `PR_TUNE=a,b,c` overrides them for simulator experiments.
/// 0: laser rush on (1) / off (0)   1: laser reach m   2: lightning reach m   3: roof penalty m
/// 4: trade lightning for a laser (1) / keep it (0)   5: no enemy closer than this (m) while swapping
pub(super) fn knob(i: usize) -> f32 {
    static K: OnceLock<[f32; 8]> = OnceLock::new();
    K.get_or_init(|| {
        let mut k = [1.0, 250.0, 0.0, 25.0, 1.0, 50.0, 0.0, 0.0];
        if let Ok(e) = std::env::var("PR_TUNE") {
            for (j, x) in e.split(',').enumerate().take(8) {
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
            if d > reach || (roof && it.pos[2] - me.pos[2] > 40.0) {
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
}
