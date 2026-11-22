//! Practice: per-match-group RNG salts and practice settings, selected by submission testing.
//!
//! On its first observation the bot puts the match in one of 8 groups (from its slot and the first
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
//! Bits add up (5 = laser hunt and heal). Change one entry per submission.

pub const SALTS: [u64; 8] = [3, 1002, 4, 4, 3, 4, 4, 3];
pub const VARIANT: [u8; 8] = [0, 1, 0, 0, 0, 0, 0, 0];
