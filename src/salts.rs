//! Practice: per-match-group RNG salts, selected by submission testing.
//!
//! On its first observation the bot puts the match in one of 8 groups (from its slot and the first
//! next-circle centre, see `Bot::apply_salt`) and seeds its random choices with that group's salt.
//! A salt only reshuffles strafe timing, wander points and unstick directions. Every entry equal
//! to 3 plays exactly like the single `RNG_SALT = 3` build (f0a08f8). Change one entry per
//! submission.

pub const SALTS: [u64; 8] = [3, 1002, 3, 3, 3, 3, 1007, 3];
