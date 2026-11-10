#![allow(dead_code)]

use cold_lava_canvas::{Kind, Part, Point, Shape};

/// The two-circle Venn of `docs/shapes.md`.
pub fn venn2() -> Shape {
    let circle = |id, x| {
        Part::new(
            Some(id),
            Kind::Circle {
                center: Point::new(x, 0.5),
                radius: 0.32,
            },
        )
    };
    Shape::new(
        vec![circle("left", 0.38), circle("right", 0.62)],
        &[
            ("only-a", "left - right"),
            ("both", "left & right"),
            ("only-b", "right - left"),
        ],
    )
    .expect("the Venn is valid")
}

/// xorshift64*: deterministic, so a failing case reruns the same way.
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Rng {
        Rng(seed.max(1))
    }

    pub fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }

    /// Uniform in `lo..hi`.
    pub fn range(&mut self, lo: f64, hi: f64) -> f64 {
        lo + (self.next() >> 11) as f64 / (1u64 << 53) as f64 * (hi - lo)
    }

    pub fn point(&mut self, lo: f64, hi: f64) -> Point {
        Point::new(self.range(lo, hi), self.range(lo, hi))
    }
}
