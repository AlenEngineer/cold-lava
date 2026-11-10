//! Times rasterizing a full-screen scene: 200 shapes on a 200×60 terminal, each region tinted by
//! cell and each part outlined in braille, as the TUI would per frame; then the same with five
//! labels fitted into every region; then 200 paths of a given size, closed with a region and
//! open, to size the limit on a path's points (docs/shapes.md, "Path size").
//!
//! cargo run --release -p cold-lava-canvas --example bench

use cold_lava_canvas::fit::{self, Align, Flow, Room};
use cold_lava_canvas::{
    Grid, Kind, Mask, Part, Path, Placed, Placement, Point, Rect, Res, Segment, Shape, Style,
    Viewport,
};
use std::sync::Arc;
use std::time::{Duration, Instant};

fn shapes() -> Vec<Arc<Shape>> {
    let p = Point::new;
    let circle = |id, x| {
        Part::new(
            Some(id),
            Kind::Circle {
                center: p(x, 0.5),
                radius: 0.32,
            },
        )
    };
    let venn = Shape::new(
        vec![circle("left", 0.38), circle("right", 0.62)],
        &[
            ("only-a", "left - right"),
            ("both", "left & right"),
            ("only-b", "right - left"),
        ],
    );
    let card = Shape::new(
        vec![
            Part::new(Some("card"), Kind::Rect(Rect::UNIT)),
            Part::new(
                Some("title"),
                Kind::Text {
                    bounds: Rect::new(p(0.1, 0.1), p(0.9, 0.4)),
                    value: "Card".into(),
                },
            ),
        ],
        &[("body", "card - title")],
    );
    let arrow = Shape::new(
        vec![Part::new(
            Some("arrow"),
            Kind::Arrow {
                from: p(0.0, 0.5),
                to: p(1.0, 0.5),
            },
        )],
        &[],
    );
    let blob = Shape::new(
        vec![Part::new(
            Some("blob"),
            Kind::Path(Path::new(
                p(0.5, 0.0),
                &[
                    Segment::Cubic {
                        c1: p(1.0, 0.0),
                        c2: p(1.0, 1.0),
                        to: p(0.5, 1.0),
                    },
                    Segment::Cubic {
                        c1: p(0.0, 1.0),
                        c2: p(0.2, 0.0),
                        to: p(0.5, 0.0),
                    },
                ],
                true,
            )),
        )],
        &[("inside", "blob")],
    );
    [venn, card, arrow, blob]
        .into_iter()
        .map(|s| Arc::new(s.expect("valid")))
        .collect()
}

/// A wavy ring of `n` points, closed with a region over it, or an open spiral of `n` points:
/// every point a vertex, as a path at the limit would be.
fn path_of(n: usize, closed: bool) -> Arc<Shape> {
    let points = (0..n)
        .map(|i| {
            let t = i as f64 / n as f64 * std::f64::consts::TAU;
            let r = match closed {
                true => 0.4 + 0.08 * (t * 12.0).sin(),
                false => 0.5 * i as f64 / n as f64,
            };
            let t = if closed { t } else { t * 6.0 };
            Point::new(0.5 + r * t.cos(), 0.5 + r * t.sin())
        })
        .collect();
    let part = Part::new(Some("p"), Kind::Path(Path::polyline(points, closed)));
    let regions: &[(&str, &str)] = if closed { &[("inside", "p")] } else { &[] };
    Arc::new(Shape::new(vec![part], regions).expect("valid"))
}

/// 200 shapes spread over the screen, overlapping, at assorted sizes and angles.
fn scene(view: &Viewport) -> Vec<Placed> {
    scene_of(view, &shapes())
}

fn scene_of(view: &Viewport, shapes: &[Arc<Shape>]) -> Vec<Placed> {
    let world = view.world_size();
    let mut seed = 0x9e37_79b9_7f4a_7c15_u64;
    let mut rand = move || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        (seed >> 11) as f64 / (1u64 << 53) as f64
    };
    (0..200)
        .map(|i| {
            let size = Point::new(8.0 + rand() * 20.0, 8.0 + rand() * 20.0);
            let at = Point::new(rand() * (world.x - size.x), rand() * (world.y - size.y));
            let placement = Placement::new(at, size).rotated(rand() * 360.0);
            Placed::new(shapes[i % shapes.len()].clone(), placement)
        })
        .collect()
}

fn frame(view: &Viewport, scene: &[Placed], labels: bool) -> Grid {
    let mut grid = Grid::new(view);
    for placed in scene {
        for (i, region) in placed.shape.regions().iter().enumerate() {
            grid.fill(
                &Mask::area(placed, &region.area, view, Res::CELLS),
                Style(i as u16 + 1),
            );
        }
        for part in 0..placed.shape.parts().len() {
            grid.dots(&Mask::outline(placed, part, view, Res::BRAILLE), Style(9));
        }
        if labels {
            for region in placed.shape.regions() {
                let room = Room::inside(&Mask::area(placed, &region.area, view, Res::BRAILLE));
                let names = ["apple", "banana", "cherry", "grape", "lemon"];
                fit::labels(&room, &names, Flow::Column, Align::Center).paint(&mut grid, Style(7));
            }
        }
    }
    grid
}

/// The median of `runs` frames.
fn median(view: &Viewport, scene: &[Placed], labels: bool, runs: usize) -> Duration {
    let mut times: Vec<Duration> = (0..runs)
        .map(|_| {
            let start = Instant::now();
            std::hint::black_box(frame(view, std::hint::black_box(scene), labels));
            start.elapsed()
        })
        .collect();
    times.sort();
    times[runs / 2]
}

fn main() {
    let view = Viewport::terminal(200, 60);
    let scene = scene(&view);
    let runs = 50;
    println!(
        "rasterize 200 shapes on 200x60 cells: median of {runs} frames {:?}",
        median(&view, &scene, false, runs)
    );
    for (i, kind) in ["venn", "card", "arrow", "blob"].iter().enumerate() {
        let only: Vec<Placed> = scene.iter().skip(i).step_by(4).cloned().collect();
        println!(
            "  the 50 {kind}s alone: {:?}",
            median(&view, &only, false, runs)
        );
    }
    println!(
        "  with five labels fitted into every region: {:?}",
        median(&view, &scene, true, runs)
    );
    let side = view.world_size().y;
    let venn = Placed::new(
        scene[0].shape.clone(),
        Placement::new(Point::new(0.0, 0.0), Point::new(side * 1.6, side)),
    );
    println!(
        "one Venn filling the screen: {:?}",
        median(&view, std::slice::from_ref(&venn), false, runs)
    );
    for n in [1024, 2048, 4096, 8192, 16384, 65536] {
        let mut mixed = shapes();
        mixed[3] = path_of(n, true);
        let open = {
            let mut s = shapes();
            s[3] = path_of(n, false);
            s
        };
        let filling = Placed::new(
            path_of(n, true),
            Placement::new(Point::new(0.0, 0.0), Point::new(side * 1.6, side)),
        );
        println!(
            "paths of {n} points: the scene with its 50 blobs such paths {:?} (open {:?}); \
             one filling the screen {:?}",
            median(&view, &scene_of(&view, &mixed), false, 15),
            median(&view, &scene_of(&view, &open), false, 15),
            median(&view, std::slice::from_ref(&filling), false, 15),
        );
    }
}
