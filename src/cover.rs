//! Getting out of a laser's line of fire. A child module of `bot`, so it sees Bot's fields.
//!
//! From recorded top-16 showdowns (private/LASER.md, CLOSE.md, OUR_DEATHS.md):
//! - a mech that breaks line of sight while a charge is aimed at it is hit 8% of the time (n 72);
//!   one that keeps it, by moving, closing in or shooting back, is hit 70 to 89% (n 788);
//! - gun sides hit first by a laser at range that never broke line of sight died 82%; survivors
//!   broke it at a median 3 s;
//! - a laser holder rushed by lightning that stayed and switched to the gun died 7 of 9; backing
//!   off died 1 of 5.

use super::{aim_to, dxy, fx, norm2, wrap, Bot, Enemy, Me, Mv, Zone};

impl Bot {
    /// A move that takes us out of a dangerous line of fire, or None to carry on.
    pub(super) fn cover_move(&mut self, me: &Me, t: f32, threats: &[Enemy], z: &Zone, my_center: [f32; 3], has_laser: bool, has_light: bool, in_bounds: &dyn Fn(&[f32]) -> bool) -> Option<Mv> {
        // Dodging and breaking away are for free-for-alls unless fx(12) says otherwise; backing off
        // from lightning helps in practice too (+0.12 in the simulator).
        let ffa_only = self.mode != 1 || fx(12) > 0.5;
        let here = (me.pos[0], me.pos[1]);
        // (a') Already ducked a charge: stay out of its sight until it fires or lets go, or it
        // just catches us stepping back out. A held charge fires on its own 2 s after full.
        if ffa_only && fx(14) > 0.5 && t < self.dodge_until {
            match self.enemies.get(&self.dodge_id).cloned() {
                Some(e) if !(e.seen >= t - 0.01 && !e.charging) => {
                    if !self.w.los(e.eye(), my_center) {
                        return Some(Mv::Hold);
                    }
                    if let Some(c) = self.find_cover(me, std::slice::from_ref(&e), z, false, 12.0, 2.0) {
                        return Some(Mv::Goto(c, me.energy > 10.0));
                    }
                }
                _ => self.dodge_until = -1.0,
            }
        }
        // (a) A charge aimed at us: get behind something before it lets go.
        if ffa_only && fx(8) > 0.5 {
            let mut aimed: Vec<&Enemy> = threats
                .iter()
                .filter(|e| {
                    let d = dxy(&e.pos, &me.pos);
                    if !e.charging || d > 290.0 {
                        return false;
                    }
                    let tol = 5f32.to_radians().max((2.5 / d.max(1.0)).atan());
                    wrap(aim_to(e.eye(), my_center)[0] - e.aim[0]).abs() <= tol && self.w.los(e.eye(), my_center)
                })
                .collect();
            aimed.sort_by(|a, b| dxy(&a.pos, &me.pos).partial_cmp(&dxy(&b.pos, &me.pos)).unwrap_or(std::cmp::Ordering::Equal));
            if let Some(e) = aimed.first() {
                let reach = if me.energy > 25.0 { 12.0 } else { 7.0 };
                if let Some(c) = self.find_cover(me, std::slice::from_ref(*e), z, false, reach, 2.0) {
                    self.dodge_id = e.id;
                    self.dodge_until = t + 2.8;
                    let dc = (c.0 - here.0).hypot(c.1 - here.1);
                    return Some(Mv::Goto(c, dc > 6.0 && me.energy > 10.0));
                }
            }
        }
        // (c) A lightning holder close in while we hold a laser and no lightning: leave its 45 m.
        // Free-for-all: any time we lack lightning (4 walk-up deaths in 10 matches to lightning
        // carriers at 28 to 36 m came while we held only the gun).
        let light_back = std::env::var("MB_NOLIGHTBACK").is_err() && self.mode == 2;
        if fx(11) > 0.5 && (has_laser || light_back) && !has_light {
            let close = threats
                .iter()
                .filter(|e| e.has("lightning") && dxy(&e.pos, &me.pos) < 50.0)
                .min_by(|a, b| dxy(&a.pos, &me.pos).partial_cmp(&dxy(&b.pos, &me.pos)).unwrap_or(std::cmp::Ordering::Equal))
                .cloned();
            let zapper = if t - self.zapped_t < 2.0 { self.enemies.get(&self.zapped_by).cloned() } else { None };
            if let Some(e) = close.or(zapper) {
                if let Some(c) = self.find_cover(me, std::slice::from_ref(&e), z, false, 15.0, 2.0) {
                    return Some(Mv::Goto(c, me.energy > 20.0));
                }
                let away = norm2([here.0 - e.pos[0], here.1 - e.pos[1]]);
                for len in [30.0f32, 18.0, 10.0] {
                    let p = [here.0 + away[0] * len, here.1 + away[1] * len];
                    if in_bounds(&p) && self.w.free_at(p[0], p[1]) {
                        return Some(Mv::Goto((p[0], p[1]), me.energy > 20.0));
                    }
                }
            }
        }
        // (b) First hit at range by a laser: break line of sight rather than trade in the open,
        // unless it is clearly the weaker side.
        // On in confirmed free-for-alls: at 8 AM, 5 of 6 deaths were laser hits at 90 to 190 m with
        // cover 3 to 12 m away and unused; mechs that broke line of sight were hit 8% of the time,
        // against 70 to 89% for those that stayed in it.
        // Also in the opening, before the match type is known (or while practice is still guarded):
        // 10 AM, hit first at 10 s from 176 m in the open and dead at 17 s.
        let opening = super::opening_safe_on() && (self.mode == 0 || (self.mode == 1 && self.kills == 0 && t < super::p_lite_until()));
        let brk_on = (ffa_only && fx(10) > 0.5) || ((self.mode == 2 || opening) && std::env::var("MB_NOBRK").is_err());
        if brk_on && t < self.brk_until {
            if let Some(e) = self.enemies.get(&self.brk_id).cloned() {
                if e.hp > 0.5 * me.hp && t - e.seen < 1.0 && self.w.los(e.eye(), my_center) {
                    if let Some(c) = self.find_cover(me, std::slice::from_ref(&e), z, false, 16.0, 2.0) {
                        return Some(Mv::Goto(c, me.energy > 20.0));
                    }
                }
            }
        }
        None
    }
}
