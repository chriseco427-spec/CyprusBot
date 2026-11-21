//! Static map: terrain heightfield plus prisms rasterized onto the 1 m terrain grid.
use serde_json::Value;
use std::fs;

pub const STEP: f32 = 1.2;
/// Obstacle inflation radius in cells (mech radius 2.5 m plus grid slack).
const INFLATE: i32 = 3;

pub struct World {
    pub min_x: f32,
    pub min_y: f32,
    pub nx: usize,
    pub ny: usize,
    /// Height of the surface a mech stands on at each grid vertex (terrain or prism top).
    pub surf: Vec<f32>,
    /// Highest prism top within the inflation radius (very low if none).
    pub wall_h: Vec<f32>,
    pub prism: Vec<bool>,
}

fn raster(ring: &[f32], min_x: f32, min_y: f32, nx: usize, ny: usize, f: &mut dyn FnMut(usize, usize)) {
    let n = ring.len() / 2;
    if n < 3 {
        return;
    }
    let mut y0 = f32::INFINITY;
    let mut y1 = f32::NEG_INFINITY;
    for k in 0..n {
        y0 = y0.min(ring[2 * k + 1]);
        y1 = y1.max(ring[2 * k + 1]);
    }
    let j0 = ((y0 - min_y).ceil().max(0.0)) as usize;
    let j1 = ((y1 - min_y).floor().min(ny as f32 - 1.0)).max(0.0) as usize;
    let mut xs: Vec<f32> = Vec::new();
    for j in j0..=j1 {
        let y = min_y + j as f32 + 0.001;
        xs.clear();
        for k in 0..n {
            let (xa, ya) = (ring[2 * k], ring[2 * k + 1]);
            let kb = (k + 1) % n;
            let (xb, yb) = (ring[2 * kb], ring[2 * kb + 1]);
            if (ya <= y && yb > y) || (yb <= y && ya > y) {
                xs.push(xa + (y - ya) / (yb - ya) * (xb - xa));
            }
        }
        xs.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        for pair in xs.chunks(2) {
            if pair.len() < 2 {
                continue;
            }
            let i0 = (pair[0] - min_x).ceil().max(0.0) as i64;
            let i1 = (pair[1] - min_x).floor().min(nx as f32 - 1.0) as i64;
            let mut i = i0;
            while i <= i1 {
                f(i as usize, j);
                i += 1;
            }
        }
    }
}

fn ring_of(v: &Value) -> Vec<f32> {
    v.as_array().map(|a| a.iter().map(|x| x.as_f64().unwrap_or(0.0) as f32).collect()).unwrap_or_default()
}

impl World {
    pub fn flat() -> World {
        let (nx, ny) = (1070usize, 724usize);
        World { min_x: -513.0, min_y: -334.0, nx, ny, surf: vec![0.0; nx * ny], wall_h: vec![-1e9; nx * ny], prism: vec![false; nx * ny] }
    }

    pub fn load(dir: &str, world_file: &str, terrain_file: &str) -> Result<World, String> {
        let txt = fs::read_to_string(format!("{}/{}", dir, world_file)).map_err(|e| e.to_string())?;
        let v: Value = serde_json::from_str(&txt).map_err(|e| e.to_string())?;
        let t = &v["terrain"];
        let nx = t["nx"].as_u64().ok_or("nx")? as usize;
        let ny = t["ny"].as_u64().ok_or("ny")? as usize;
        let min_x = t["minX"].as_f64().ok_or("minX")? as f32;
        let min_y = t["minY"].as_f64().ok_or("minY")? as f32;
        let step = t["step"].as_f64().unwrap_or(1.0);
        if (step - 1.0).abs() > 1e-6 || nx * ny == 0 || nx * ny > 8_000_000 {
            return Err("unexpected terrain grid".into());
        }
        let scale = t["scale"].as_f64().unwrap_or(0.01) as f32;
        let raw = fs::read(format!("{}/{}", dir, terrain_file)).map_err(|e| e.to_string())?;
        if raw.len() < nx * ny * 2 {
            return Err("terrain too short".into());
        }
        let mut surf = vec![0f32; nx * ny];
        for k in 0..nx * ny {
            surf[k] = i16::from_le_bytes([raw[2 * k], raw[2 * k + 1]]) as f32 * scale;
        }
        let mut prism = vec![false; nx * ny];
        let mut stamp = vec![0u32; nx * ny];
        if let Some(ps) = v["prisms"].as_array() {
            for (pi, p) in ps.iter().enumerate() {
                let top = p["top"].as_f64().unwrap_or(0.0) as f32;
                let id = pi as u32 + 1;
                if let Some(hs) = p["holes"].as_array() {
                    for h in hs {
                        let r = ring_of(h);
                        raster(&r, min_x, min_y, nx, ny, &mut |i, j| stamp[j * nx + i] = id);
                    }
                }
                let r = ring_of(&p["ring"]);
                raster(&r, min_x, min_y, nx, ny, &mut |i, j| {
                    let k = j * nx + i;
                    if stamp[k] != id && top > surf[k] {
                        surf[k] = top;
                        prism[k] = true;
                    }
                });
            }
        }
        // Inflate prism heights so the mech's body clears walls.
        let mut wall_h = vec![-1e9f32; nx * ny];
        let mut offs: Vec<(i32, i32)> = Vec::new();
        for dy in -INFLATE..=INFLATE {
            for dx in -INFLATE..=INFLATE {
                if dx * dx + dy * dy <= INFLATE * INFLATE {
                    offs.push((dx, dy));
                }
            }
        }
        for j in 0..ny {
            for i in 0..nx {
                let k = j * nx + i;
                if !prism[k] {
                    continue;
                }
                let h = surf[k];
                for &(dx, dy) in &offs {
                    let (x, y) = (i as i32 + dx, j as i32 + dy);
                    if x >= 0 && y >= 0 && (x as usize) < nx && (y as usize) < ny {
                        let q = y as usize * nx + x as usize;
                        if h > wall_h[q] {
                            wall_h[q] = h;
                        }
                    }
                }
            }
        }
        // The map edge is a wall.
        let b = &v["bounds"];
        let bx0 = b["minX"].as_f64().unwrap_or(min_x as f64) as f32 + 3.5;
        let bx1 = b["maxX"].as_f64().unwrap_or((min_x + nx as f32) as f64) as f32 - 3.5;
        let by0 = b["minY"].as_f64().unwrap_or(min_y as f64) as f32 + 3.5;
        let by1 = b["maxY"].as_f64().unwrap_or((min_y + ny as f32) as f64) as f32 - 3.5;
        for j in 0..ny {
            for i in 0..nx {
                let (x, y) = (min_x + i as f32, min_y + j as f32);
                if x < bx0 || x > bx1 || y < by0 || y > by1 {
                    wall_h[j * nx + i] = 1e9;
                }
            }
        }
        Ok(World { min_x, min_y, nx, ny, surf, wall_h, prism })
    }

    #[inline]
    pub fn cell(&self, x: f32, y: f32) -> usize {
        let i = ((x - self.min_x).round().max(0.0) as usize).min(self.nx - 1);
        let j = ((y - self.min_y).round().max(0.0) as usize).min(self.ny - 1);
        j * self.nx + i
    }

    #[inline]
    pub fn surf_at(&self, x: f32, y: f32) -> f32 {
        self.surf[self.cell(x, y)]
    }

    /// A mech can stand here without its body intersecting a wall.
    #[inline]
    pub fn free(&self, k: usize) -> bool {
        self.wall_h[k] <= self.surf[k] + STEP + 0.01
    }

    #[inline]
    pub fn free_at(&self, x: f32, y: f32) -> bool {
        self.free(self.cell(x, y))
    }

    /// Line of sight / line of fire between two 3D points against terrain and prisms.
    pub fn los(&self, a: [f32; 3], b: [f32; 3]) -> bool {
        let (dx, dy, dz) = (b[0] - a[0], b[1] - a[1], b[2] - a[2]);
        let d = (dx * dx + dy * dy).sqrt();
        if d < 3.0 {
            return true;
        }
        let n = d.ceil() as usize;
        let inv = 1.0 / n as f32;
        for k in 1..n {
            let t = k as f32 * inv;
            let along = t * d;
            if along < 1.5 || d - along < 1.5 {
                continue;
            }
            let z = a[2] + dz * t;
            if self.surf_at(a[0] + dx * t, a[1] + dy * t) > z + 0.1 {
                return false;
            }
        }
        true
    }

    /// Straight walk from a to b stays on free cells with no step larger than a mech can take.
    pub fn walkable_line(&self, a: (f32, f32), b: (f32, f32)) -> bool {
        let (dx, dy) = (b.0 - a.0, b.1 - a.1);
        let d = (dx * dx + dy * dy).sqrt();
        let n = (d * 2.0).ceil().max(1.0) as usize;
        let mut prev = self.surf_at(a.0, a.1);
        for k in 1..=n {
            let t = k as f32 / n as f32;
            let c = self.cell(a.0 + dx * t, a.1 + dy * t);
            if !self.free(c) {
                return false;
            }
            let s = self.surf[c];
            if (s - prev).abs() > STEP {
                return false;
            }
            prev = s;
        }
        true
    }
}
