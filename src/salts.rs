//! Practice: per-match-group RNG salts, selected by submission testing.
//!
//! On its first observation the bot puts the match in one of 32 groups (from its slot and the first
//! next-circle centre, see `Bot::apply_salt`) and seeds its random choices with that group's salt.
//! A salt only reshuffles strafe timing, wander points and unstick directions. Entry j covers the
//! matches of the earlier 8-group split's group j % 8 (key % 32 refines key % 8), so changing entries
//! j, j+8, j+16 and j+24 together changes one 8-group, and an array that repeats 8 values plays exactly
//! like the 8-entry array of those values. Every entry equal to 3 plays exactly like the single
//! `RNG_SALT = 3` build (f0a08f8).

pub const SALTS: [u64; 32] = [
    3, 1002, 4, 1018, 3, 4, 4, 3,
    3, 1002, 4, 4, 3, 4, 4, 3,
    3, 1002, 4, 1018, 3, 4, 4, 3,
    3, 1002, 4, 4, 3, 4, 4, 3,
];
