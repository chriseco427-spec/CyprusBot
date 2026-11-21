//! A* over a 2 m lattice laid on the 1 m world grid. Tracks altitude so paths can drop off roofs.
use crate::world::{World, STEP};
use std::cmp::Reverse;
use std::collections::BinaryHeap;

pub struct Nav {
    lx: usize,
    ly: usize,
    g: Vec<f32>,
    alt: Vec<f32>,
    came: Vec<u32>,
    seen: Vec<u32>,
    closed: Vec<u32>,
    gen: u32,
    heap: BinaryHeap<(Reverse<u32>, u32)>,
}

#[inline]
fn try_step(w: &World, from: usize, alt: f32, to: usize, d: f32) -> Option<f32> {
    let s = w.surf[to];
    let wh = w.wall_h[to];
    if wh <= s + STEP + 0.01 {
        let rise = s - alt;
        if rise <= 0.0 {
            return Some(s);
        }
        let lim = if w.prism[to] || w.prism[from] { STEP } else { (1.1 * d).max(STEP) };
        if rise <= lim {
            Some(s)
        } else {
            None
        }
    } else if wh <= alt + STEP + 0.01 {
        // Next to a wall we are already above: pass over it (dropping off a roof).
        Some(alt)
    } else {
        None
    }
}

const DIRS: [(i32, i32); 8] = [(1, 0), (-1, 0), (0, 1), (0, -1), (1, 1), (1, -1), (-1, 1), (-1, -1)];

impl Nav {
    pub fn new(w: &World) -> Nav {
        let lx = w.nx / 2;
        let ly = w.ny / 2;
        let n = lx * ly;
        Nav { lx, ly, g: vec![0.0; n], alt: vec![0.0; n], came: vec![0; n], seen: vec![0; n], closed: vec![0; n], gen: 0, heap: BinaryHeap::new() }
    }

    #[inline]
    fn node_xy(&self, w: &World, n: usize) -> (f32, f32) {
        let (i, j) = (n % self.lx, n / self.lx);
        (w.min_x + (2 * i) as f32, w.min_y + (2 * j) as f32)
    }

    #[inline]
    fn cell_of(&self, w: &World, n: usize) -> usize {
        let (i, j) = (n % self.lx, n / self.lx);
        (2 * j) * w.nx + 2 * i
    }

    fn snap(&self, w: &World, x: f32, y: f32) -> (i32, i32) {
        let i = (((x - w.min_x) * 0.5).round() as i32).clamp(0, self.lx as i32 - 1);
        let j = (((y - w.min_y) * 0.5).round() as i32).clamp(0, self.ly as i32 - 1);
        (i, j)
    }

    /// Best-effort path from `start` (x, y, z) to `goal`. If the goal is unreachable or the
    /// expansion cap is hit, the path leads to the explored node closest to the goal.
    pub fn path(&mut self, w: &World, start: [f32; 3], goal: (f32, f32), max_expand: usize) -> Vec<(f32, f32)> {
        self.gen = self.gen.wrapping_add(1);
        if self.gen == 0 {
            self.seen.iter_mut().for_each(|v| *v = 0);
            self.closed.iter_mut().for_each(|v| *v = 0);
            self.gen = 1;
        }
        let gen = self.gen;
        self.heap.clear();
        let (si, sj) = self.snap(w, start[0], start[1]);
        // Prefer a nearby lattice node we could actually be standing on.
        let mut best_s: Option<(f32, usize)> = None;
        for dj in -2..=2 {
            for di in -2..=2 {
                let (i, j) = (si + di, sj + dj);
                if i < 0 || j < 0 || i >= self.lx as i32 || j >= self.ly as i32 {
                    continue;
                }
                let n = j as usize * self.lx + i as usize;
                let c = self.cell_of(w, n);
                if !w.free(c) || w.surf[c] > start[2] + 1.5 {
                    continue;
                }
                let (x, y) = self.node_xy(w, n);
                let d = (x - start[0]).hypot(y - start[1]);
                if best_s.map_or(true, |b| d < b.0) {
                    best_s = Some((d, n));
                }
            }
        }
        let s = match best_s {
            Some((_, n)) => n,
            None => sj as usize * self.lx + si as usize,
        };
        let (gi, gj) = self.snap(w, goal.0, goal.1);
        let goal_n = gj as usize * self.lx + gi as usize;
        let lx = self.lx;
        let h = move |n: usize| -> f32 {
            let dx = ((n % lx) as f32 - gi as f32).abs();
            let dy = ((n / lx) as f32 - gj as f32).abs();
            2.0 * (dx.max(dy) + 0.4142 * dx.min(dy))
        };
        let sc = self.cell_of(w, s);
        self.g[s] = 0.0;
        self.alt[s] = start[2].max(w.surf[sc]);
        self.came[s] = s as u32;
        self.seen[s] = gen;
        self.heap.push((Reverse((1.2 * h(s)).to_bits()), s as u32));
        let mut best = s;
        let mut best_h = h(s);
        let mut expanded = 0usize;
        let mut reached = false;
        while let Some((_, n32)) = self.heap.pop() {
            let n = n32 as usize;
            if self.closed[n] == gen {
                continue;
            }
            self.closed[n] = gen;
            let hn = h(n);
            if hn < best_h {
                best_h = hn;
                best = n;
            }
            if n == goal_n {
                reached = true;
                break;
            }
            expanded += 1;
            if expanded > max_expand {
                break;
            }
            let (i, j) = ((n % self.lx) as i32, (n / self.lx) as i32);
            let cn = self.cell_of(w, n);
            let (ci, cj) = (2 * i, 2 * j);
            let alt = self.alt[n];
            let gn = self.g[n];
            for &(dx, dy) in &DIRS {
                let (ti, tj) = (i + dx, j + dy);
                if ti < 0 || tj < 0 || ti >= self.lx as i32 || tj >= self.ly as i32 {
                    continue;
                }
                let m = tj as usize * self.lx + ti as usize;
                if self.closed[m] == gen {
                    continue;
                }
                let diag = dx != 0 && dy != 0;
                let d = if diag { 1.4142 } else { 1.0 };
                let cm = (cj + dy) as usize * w.nx + (ci + dx) as usize;
                let a1 = match try_step(w, cn, alt, cm, d) {
                    Some(a) => a,
                    None => continue,
                };
                if diag {
                    // No corner cutting.
                    let c1 = cj as usize * w.nx + (ci + dx) as usize;
                    let c2 = (cj + dy) as usize * w.nx + ci as usize;
                    if try_step(w, cn, alt, c1, 1.0).is_none() || try_step(w, cn, alt, c2, 1.0).is_none() {
                        continue;
                    }
                }
                let ct = self.cell_of(w, m);
                let a2 = match try_step(w, cm, a1, ct, d) {
                    Some(a) => a,
                    None => continue,
                };
                let ng = gn + 2.0 * d;
                if self.seen[m] != gen || ng < self.g[m] {
                    self.seen[m] = gen;
                    self.g[m] = ng;
                    self.alt[m] = a2;
                    self.came[m] = n as u32;
                    self.heap.push((Reverse((ng + 1.2 * h(m)).to_bits()), m as u32));
                }
            }
        }
        let end = if reached { goal_n } else { best };
        let mut out = Vec::new();
        let mut n = end;
        let mut guard = 0;
        loop {
            out.push(self.node_xy(w, n));
            let p = self.came[n] as usize;
            if p == n || guard > 100_000 {
                break;
            }
            n = p;
            guard += 1;
        }
        out.reverse();
        if reached {
            // Finish exactly on the goal if it is standable.
            if w.free_at(goal.0, goal.1) {
                out.push(goal);
            }
        }
        out
    }
}
