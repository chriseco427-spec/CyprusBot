//! Practice: per-match-group RNG salts and practice settings, selected by submission testing.
//!
//! On its first observation the bot puts the match in one of 32 groups (VARIANT is per 8-group: g % 8) (from its slot and the first
//! next-circle centre, see `Bot::apply_salt`) and seeds its random choices with that group's salt.
//! A salt only reshuffles strafe timing, wander points and unstick directions. Every entry equal
//! to 3 plays exactly like the single `RNG_SALT = 3` build (f0a08f8). Change one entry per
//! submission.
//!
//! `VARIANT` holds one bit mask of practice switches per group (confirmed practice, mode 1, only;
//! free-for-all is never touched). All zero plays exactly like the build without the masks.
//!   1: laser hunt (once past 20 s or our first kill without a laser, getting one comes first)
//!   2: selective fire (open fire only on hostile house bots, or any while none is hostile and
//!      we are above 90% HP)
//!   4: retreat and heal (HP below 35% with 3 or more mechs tracked: break line of sight, hold
//!      fire, regenerate to 90%)
//!   8: fight range 100 m for gun and laser
//!  16: guarded opening off (also the first seconds of a free-for-all still read as practice)
//!  32: fight range 40 m for gun and laser
//!  64: fight range 150 m for gun and laser (of 8, 32 and 64 the lowest bit set wins)
//! 128: follow up a wounded hunter below 55% HP instead of 65%
//! 256: shared upgrade order instead of offense-first
//! 512: retreat and heal with any number of hunters tracked (wins over 4)
//! 1024: regroup in the lulls (hurt below 60%, unseen for 3 s: cover, hold fire until 90%)
//! 2048: straight boosted run to the nearest special weapon from second 0 (acts before the match
//!       type is known, so it also plays in free-for-alls of that group)
//! 4096: first hunter from cover (wait at a low-exposure spot until our first kill)
//! 8192: sidestep timing (reversals no sooner than the bullet's flight time, one on a laser charge)
//! 16384: eager farming (crates and lone drones whenever no hunter is in view, from 40% HP)
//! 32768: opening safety off (modes 0 and 1: no break-away after a laser hit from range, no limit on
//!        weapon walks across open street; the practice play of the builds before that commit)
//! Bits add up (5 = laser hunt and heal). Change one entry per submission.

pub const SALTS: [u64; 32] = [2009,2009,2009,2009,2009,2009,2009,2009,2009,2009,2009,2009,2009,2009,2009,2009,2009,2009,2009,2009,2009,2009,2009,2009,2009,2009,2009,2009,2009,2009,2009,2009];
pub const VARIANT: [u16; 8] = [18432,18432,18432,18432,18432,18432,18432,18432];
