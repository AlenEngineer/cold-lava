//! Builds the link graph from a vault and runs a force-directed layout on it.

use crate::vault::Vault;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Note,
    Unresolved,
    Tag,
}

pub struct Node {
    pub key: String,
    pub label: String,
    pub kind: Kind,
    pub path: Option<PathBuf>,
    pub rel: Option<String>,
    pub x: f64,
    pub y: f64,
    vx: f64,
    vy: f64,
    pub pinned: bool,
}

#[derive(Clone, Copy)]
pub struct BuildOpts {
    pub unresolved: bool,
    pub tags: bool,
    pub orphans: bool,
}

pub struct Graph {
    pub nodes: Vec<Node>,
    /// Directed, de-duplicated edges (from, to).
    pub edges: Vec<(usize, usize)>,
    /// Undirected neighbour lists.
    pub adj: Vec<Vec<usize>>,
    pub links_out: Vec<usize>,
    pub links_in: Vec<usize>,
    /// Simulation "temperature" (like d3-force): forces scale with it and it cools to 0.
    pub alpha: f64,
    /// Width/height of the viewport; gravity is stretched so the layout fills a wide terminal.
    pub aspect: f64,
}

// Layout constants, in world units (1 unit = 1 braille dot at zoom 1).
const REPULSION: f64 = 6000.0;
const SPRING_LEN: f64 = 50.0;
const SPRING_K: f64 = 0.06;
const GRAVITY: f64 = 0.02;
const VELOCITY_KEEP: f64 = 0.6;
const MAX_SPEED: f64 = 25.0;
const ALPHA_DECAY: f64 = 0.0228;
pub const ALPHA_MIN: f64 = 0.002;

pub fn build(v: &Vault, o: BuildOpts, prev: &HashMap<String, (f64, f64)>) -> Graph {
    let mut nodes: Vec<Node> = Vec::new();
    let mut index: HashMap<String, usize> = HashMap::new();
    let mut add = |nodes: &mut Vec<Node>, key: String, label: String, kind, path, rel| -> usize {
        if let Some(&i) = index.get(&key) {
            return i;
        }
        let i = nodes.len();
        // Phyllotaxis spiral start position (deterministic, evenly spread).
        let (x, y) = prev.get(&key).copied().unwrap_or_else(|| {
            let r = 8.0 * ((i as f64) + 0.5).sqrt();
            let a = i as f64 * std::f64::consts::PI * (3.0 - 5f64.sqrt());
            (r * a.cos(), r * a.sin())
        });
        nodes.push(Node { key: key.clone(), label, kind, path, rel, x, y, vx: 0.0, vy: 0.0, pinned: false });
        index.insert(key, i);
        i
    };

    // Lookup tables for link resolution: full relative path, then bare file name.
    let mut by_rel: HashMap<String, usize> = HashMap::new();
    let mut by_name: HashMap<String, usize> = HashMap::new();
    for (ni, n) in v.notes.iter().enumerate() {
        by_rel.insert(n.rel.to_lowercase(), ni);
        // Obsidian prefers the shortest path when names collide; notes are sorted, so keep first
        // unless a shallower one appears.
        let e = by_name.entry(n.name.to_lowercase()).or_insert(ni);
        if n.rel.matches('/').count() < v.notes[*e].rel.matches('/').count() {
            *e = ni;
        }
    }
    let note_node: Vec<usize> = v
        .notes
        .iter()
        .map(|n| {
            let key = format!("n:{}", n.rel.to_lowercase());
            add(&mut nodes, key, n.name.clone(), Kind::Note, Some(n.path.clone()), Some(n.rel.clone()))
        })
        .collect();

    let mut edge_set: HashSet<(usize, usize)> = HashSet::new();
    let mut edges = Vec::new();
    for (ni, n) in v.notes.iter().enumerate() {
        let from = note_node[ni];
        for t in &n.links {
            let tl = t.to_lowercase();
            let tl = tl.trim_start_matches("./").trim_start_matches('/');
            let resolved = by_rel.get(tl).or_else(|| {
                let base = tl.rsplit('/').next().unwrap_or(tl);
                by_name.get(base)
            });
            let to = match resolved {
                Some(&target) => note_node[target],
                None if o.unresolved => {
                    let label = t.rsplit('/').next().unwrap_or(t).to_string();
                    add(&mut nodes, format!("u:{tl}"), label, Kind::Unresolved, None, None)
                }
                None => continue,
            };
            if to != from && edge_set.insert((from, to)) {
                edges.push((from, to));
            }
        }
        if o.tags {
            for tag in &n.tags {
                let to = add(&mut nodes, format!("t:{tag}"), format!("#{tag}"), Kind::Tag, None, None);
                if edge_set.insert((from, to)) {
                    edges.push((from, to));
                }
            }
        }
    }

    let mut g = Graph::from_parts(nodes, edges);
    if !o.orphans {
        g = g.without_orphans();
    }
    g
}

impl Graph {
    fn from_parts(nodes: Vec<Node>, edges: Vec<(usize, usize)>) -> Graph {
        let n = nodes.len();
        let mut adj = vec![Vec::new(); n];
        let mut links_out = vec![0; n];
        let mut links_in = vec![0; n];
        for &(a, b) in &edges {
            links_out[a] += 1;
            links_in[b] += 1;
            if !adj[a].contains(&b) {
                adj[a].push(b);
                adj[b].push(a);
            }
        }
        Graph { nodes, edges, adj, links_out, links_in, alpha: 1.0, aspect: 1.0 }
    }

    fn without_orphans(self) -> Graph {
        let keep: Vec<bool> = self.adj.iter().map(|a| !a.is_empty()).collect();
        let mut remap = vec![usize::MAX; self.nodes.len()];
        let mut nodes = Vec::new();
        for (i, node) in self.nodes.into_iter().enumerate() {
            if keep[i] {
                remap[i] = nodes.len();
                nodes.push(node);
            }
        }
        let edges = self.edges.iter().map(|&(a, b)| (remap[a], remap[b])).collect();
        Graph::from_parts(nodes, edges)
    }

    pub fn degree(&self, i: usize) -> usize {
        self.adj[i].len()
    }

    pub fn find(&self, key: &str) -> Option<usize> {
        self.nodes.iter().position(|n| n.key == key)
    }

    pub fn positions(&self) -> HashMap<String, (f64, f64)> {
        self.nodes.iter().map(|n| (n.key.clone(), (n.x, n.y))).collect()
    }

    pub fn reheat(&mut self, alpha: f64) {
        self.alpha = self.alpha.max(alpha);
    }

    pub fn settled(&self) -> bool {
        self.alpha < ALPHA_MIN
    }

    /// One simulation tick. `active[i]` limits the simulation to a subset (local graph mode).
    pub fn step(&mut self, active: &[bool]) {
        if self.settled() {
            return;
        }
        let n = self.nodes.len();
        let mut fx = vec![0.0; n];
        let mut fy = vec![0.0; n];
        let idx: Vec<usize> = (0..n).filter(|&i| active[i]).collect();

        // Repulsion between every pair (O(n²); fine for a few thousand notes).
        for (k, &i) in idx.iter().enumerate() {
            let (xi, yi) = (self.nodes[i].x, self.nodes[i].y);
            for &j in &idx[k + 1..] {
                let dx = xi - self.nodes[j].x;
                let dy = yi - self.nodes[j].y;
                let d2 = (dx * dx + dy * dy).max(1.0);
                if d2 > 250_000.0 {
                    continue; // far apart: negligible
                }
                let f = REPULSION / d2;
                let d = d2.sqrt();
                let (ux, uy) = (dx / d * f, dy / d * f);
                fx[i] += ux;
                fy[i] += uy;
                fx[j] -= ux;
                fy[j] -= uy;
            }
        }
        // Springs along edges.
        for &(a, b) in &self.edges {
            if !active[a] || !active[b] {
                continue;
            }
            let dx = self.nodes[b].x - self.nodes[a].x;
            let dy = self.nodes[b].y - self.nodes[a].y;
            let d = (dx * dx + dy * dy).sqrt().max(0.01);
            // Weaken springs on hubs so they don't collapse into a ball.
            let w = 1.0 / (self.degree(a).min(self.degree(b)).max(1) as f64).sqrt();
            let f = SPRING_K * w * (d - SPRING_LEN);
            let (ux, uy) = (dx / d * f, dy / d * f);
            fx[a] += ux;
            fy[a] += uy;
            fx[b] -= ux;
            fy[b] -= uy;
        }
        // Gravity toward the origin and integration.
        for &i in &idx {
            let node = &mut self.nodes[i];
            if node.pinned {
                node.vx = 0.0;
                node.vy = 0.0;
                continue;
            }
            let a = self.aspect.clamp(0.5, 3.0);
            fx[i] -= node.x * GRAVITY / a.sqrt();
            fy[i] -= node.y * GRAVITY * a.sqrt();
            node.vx = ((node.vx + fx[i] * self.alpha) * VELOCITY_KEEP).clamp(-MAX_SPEED, MAX_SPEED);
            node.vy = ((node.vy + fy[i] * self.alpha) * VELOCITY_KEEP).clamp(-MAX_SPEED, MAX_SPEED);
            node.x += node.vx;
            node.y += node.vy;
        }
        self.alpha += (0.0 - self.alpha) * ALPHA_DECAY;
    }
}
