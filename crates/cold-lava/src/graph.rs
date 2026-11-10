//! The graph of the workspace's files and links, and the force-directed layout the scene
//! view runs on it.

use crate::links::{Link, LinkGraph};
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
    pub tags: Vec<String>,
    pub x: f64,
    pub y: f64,
    vx: f64,
    vy: f64,
    pub pinned: bool,
}

impl Node {
    pub fn new(key: String, label: String, kind: Kind, (x, y): (f64, f64)) -> Node {
        Node {
            key,
            label,
            kind,
            path: None,
            rel: None,
            tags: Vec::new(),
            x,
            y,
            vx: 0.0,
            vy: 0.0,
            pinned: false,
        }
    }
}

/// Where the `i`th node starts: a phyllotaxis spiral, deterministic and evenly spread.
pub fn start(i: usize) -> (f64, f64) {
    let r = 8.0 * ((i as f64) + 0.5).sqrt();
    let a = i as f64 * std::f64::consts::PI * (3.0 - 5f64.sqrt());
    (r * a.cos(), r * a.sin())
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
/// Nodes farther apart than this (squared) don't push each other.
const REACH2: f64 = 250_000.0;
/// The Barnes–Hut opening criterion: a cell of side `s` at distance `d` from a node pushes it
/// as one body at the cell's centre of mass when `s / d < THETA`. 0 is exact; higher is faster
/// and coarser. It stays under √2, so a node is never pushed by a whole cell it is inside.
const THETA: f64 = 0.8;

/// The graph of `links` (the gateway's `links/query`): a node per file, and per unresolved
/// link's name and per tag when `o` shows them, in the order they come, with the links between
/// them once each.
pub fn build(links: &LinkGraph, o: BuildOpts) -> Graph {
    let mut nodes: Vec<Node> = Vec::new();
    let mut index: HashMap<String, usize> = HashMap::new();
    let mut add = |nodes: &mut Vec<Node>, key: String, label: String, kind| -> usize {
        if let Some(&i) = index.get(&key) {
            return i;
        }
        let i = nodes.len();
        nodes.push(Node::new(key.clone(), label, kind, start(i)));
        index.insert(key, i);
        i
    };
    let mut file_node: HashMap<&str, usize> = HashMap::new();
    for f in &links.files {
        let (key, rel) = match f.path.strip_suffix(".md") {
            Some(rel) => (format!("n:{}", rel.to_lowercase()), rel),
            None => (format!("f:{}", f.path.to_lowercase()), f.path.as_str()),
        };
        let i = add(&mut nodes, key, f.name.clone(), Kind::Note);
        nodes[i].path = Some(PathBuf::from(&f.path));
        nodes[i].rel = Some(rel.to_string());
        nodes[i].tags = f.tags.clone();
        file_node.insert(&f.path, i);
    }
    let mut by_file: HashMap<&str, Vec<&Link>> = HashMap::new();
    for l in &links.links {
        by_file.entry(l.from.as_str()).or_default().push(l);
    }

    let mut edge_set: HashSet<(usize, usize)> = HashSet::new();
    let mut edges = Vec::new();
    for f in &links.files {
        let from = file_node[f.path.as_str()];
        for l in by_file.get(f.path.as_str()).into_iter().flatten() {
            let to = match (&l.to, &l.target) {
                (Some(to), _) => match file_node.get(to.as_str()) {
                    Some(&to) => to,
                    None => continue,
                },
                (None, Some(t)) if o.unresolved => {
                    let tl = t.to_lowercase();
                    let tl = tl.trim_start_matches("./").trim_start_matches('/');
                    let label = t.rsplit('/').next().unwrap_or(t).to_string();
                    add(&mut nodes, format!("u:{tl}"), label, Kind::Unresolved)
                }
                _ => continue,
            };
            if to != from && edge_set.insert((from, to)) {
                edges.push((from, to));
            }
        }
        if o.tags {
            for tag in &f.tags {
                let to = add(&mut nodes, format!("t:{tag}"), format!("#{tag}"), Kind::Tag);
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
    pub fn from_parts(nodes: Vec<Node>, edges: Vec<(usize, usize)>) -> Graph {
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
        Graph {
            nodes,
            edges,
            adj,
            links_out,
            links_in,
            alpha: 1.0,
            aspect: 1.0,
        }
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
        let edges = self
            .edges
            .iter()
            .map(|&(a, b)| (remap[a], remap[b]))
            .collect();
        Graph::from_parts(nodes, edges)
    }

    pub fn degree(&self, i: usize) -> usize {
        self.adj[i].len()
    }

    pub fn find(&self, key: &str) -> Option<usize> {
        self.nodes.iter().position(|n| n.key == key)
    }

    pub fn positions(&self) -> HashMap<String, (f64, f64)> {
        self.nodes
            .iter()
            .map(|n| (n.key.clone(), (n.x, n.y)))
            .collect()
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

        // Repulsion, through a quadtree of the active nodes: O(n log n), not every pair.
        let tree = Quadtree::new(&self.nodes, &idx);
        let mut stack = Vec::new();
        for &i in &idx {
            let (ux, uy) = tree.push(i, &mut stack, THETA);
            fx[i] += ux;
            fy[i] += uy;
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

/// A Barnes–Hut quadtree of node positions, built for one step: each cell knows how many nodes
/// it holds and their centre of mass, so a far cell pushes as one body.
struct Quadtree<'a> {
    nodes: &'a [Node],
    /// The root is `cells[0]`; a split cell's four children are consecutive from its `first`.
    cells: Vec<Cell>,
}

#[derive(Clone, Copy)]
struct Cell {
    /// Centre and half the side of the square.
    x: f64,
    y: f64,
    half: f64,
    mass: f64,
    /// Sum of the positions in it; over `mass`, the centre of mass.
    sx: f64,
    sy: f64,
    /// A leaf's node (the first one, when nodes at one place share a leaf at `MAX_DEPTH`).
    body: usize,
    /// The first child's index; 0 for a leaf.
    first: usize,
}

/// Deeper than this, nodes at (nearly) the same place share a leaf.
const MAX_DEPTH: usize = 48;

impl Cell {
    fn new(x: f64, y: f64, half: f64) -> Cell {
        Cell {
            x,
            y,
            half,
            mass: 0.0,
            sx: 0.0,
            sy: 0.0,
            body: usize::MAX,
            first: 0,
        }
    }

    fn child(&self, x: f64, y: f64) -> usize {
        self.first + (usize::from(x >= self.x) | (usize::from(y >= self.y) << 1))
    }
}

impl<'a> Quadtree<'a> {
    fn new(nodes: &'a [Node], idx: &[usize]) -> Quadtree<'a> {
        let (mut x0, mut x1, mut y0, mut y1) = (f64::MAX, f64::MIN, f64::MAX, f64::MIN);
        for &i in idx {
            x0 = x0.min(nodes[i].x);
            x1 = x1.max(nodes[i].x);
            y0 = y0.min(nodes[i].y);
            y1 = y1.max(nodes[i].y);
        }
        let half = ((x1 - x0).max(y1 - y0) / 2.0).max(1.0) * 1.001;
        let mut t = Quadtree {
            nodes,
            cells: Vec::with_capacity(idx.len() * 3 + 1),
        };
        t.cells
            .push(Cell::new((x0 + x1) / 2.0, (y0 + y1) / 2.0, half));
        for &i in idx {
            t.insert(i);
        }
        t
    }

    fn insert(&mut self, i: usize) {
        let (x, y) = (self.nodes[i].x, self.nodes[i].y);
        let mut c = 0;
        for depth in 0.. {
            let cell = &mut self.cells[c];
            cell.mass += 1.0;
            cell.sx += x;
            cell.sy += y;
            if cell.first != 0 {
                c = cell.child(x, y);
                continue;
            }
            if cell.mass == 1.0 {
                cell.body = i;
                return;
            }
            if depth >= MAX_DEPTH {
                return;
            }
            // A leaf with one node already: split it and move that node down a level.
            let (cx, cy, h) = (cell.x, cell.y, cell.half / 2.0);
            let old = std::mem::replace(&mut cell.body, usize::MAX);
            let first = self.cells.len();
            self.cells[c].first = first;
            for (dx, dy) in [(-h, -h), (h, -h), (-h, h), (h, h)] {
                self.cells.push(Cell::new(cx + dx, cy + dy, h));
            }
            let (ox, oy) = (self.nodes[old].x, self.nodes[old].y);
            let to = self.cells[c].child(ox, oy);
            let moved = &mut self.cells[to];
            moved.mass = 1.0;
            moved.sx = ox;
            moved.sy = oy;
            moved.body = old;
            c = self.cells[c].child(x, y);
        }
    }

    /// The push on node `i` from all the others. A pair farther apart than `REACH2` allows is
    /// left out, and so is a cell all of whose nodes are.
    fn push(&self, i: usize, stack: &mut Vec<usize>, theta: f64) -> (f64, f64) {
        let (x, y) = (self.nodes[i].x, self.nodes[i].y);
        let (mut fx, mut fy) = (0.0, 0.0);
        stack.clear();
        stack.push(0);
        while let Some(c) = stack.pop() {
            let cell = &self.cells[c];
            if cell.mass == 0.0 || (cell.first == 0 && cell.body == i) {
                continue;
            }
            let gx = ((x - cell.x).abs() - cell.half).max(0.0);
            let gy = ((y - cell.y).abs() - cell.half).max(0.0);
            if gx * gx + gy * gy > REACH2 {
                continue;
            }
            let (dx, dy) = (x - cell.sx / cell.mass, y - cell.sy / cell.mass);
            let d2 = dx * dx + dy * dy;
            let side = 2.0 * cell.half;
            if cell.first != 0 && side * side >= theta * theta * d2 {
                stack.extend(cell.first..cell.first + 4);
                continue;
            }
            if d2 > REACH2 {
                continue;
            }
            let d2 = d2.max(1.0);
            let f = REPULSION * cell.mass / d2;
            let d = d2.sqrt();
            fx += dx / d * f;
            fy += dy / d * f;
        }
        (fx, fy)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::index::Index;
    use crate::workspace::{Note, Workspace};

    /// The graph of `v`, as the gateway would give its links.
    fn of(v: Workspace, o: BuildOpts) -> Graph {
        build(&crate::links::graph(&Index::build(v)), o)
    }

    fn note(rel: &str, links: &[&str], tags: &[&str]) -> Note {
        Note {
            rel: rel.into(),
            name: rel.rsplit('/').next().unwrap().into(),
            path: PathBuf::from(format!("{rel}.md")),
            links: links.iter().map(|s| s.to_string()).collect(),
            refs: vec![],
            aliases: vec![],
            text: String::new(),
            tags: tags.iter().map(|s| s.to_string()).collect(),
            links_written: Default::default(),
            style: Default::default(),
            items: Default::default(),
        }
    }

    fn workspace(notes: Vec<Note>) -> Workspace {
        Workspace {
            root: PathBuf::from("/v"),
            name: "v".into(),
            notes,
            owned: Vec::new(),
            item_settings: Default::default(),
        }
    }

    const ALL: BuildOpts = BuildOpts {
        unresolved: true,
        tags: true,
        orphans: true,
    };

    fn labels(g: &Graph) -> Vec<&str> {
        g.nodes.iter().map(|n| n.label.as_str()).collect()
    }

    #[test]
    fn resolves_by_name_and_path_case_insensitively() {
        let v = workspace(vec![
            note("A", &["b", "deep/C", "Missing"], &[]),
            note("B", &["A"], &[]),
            note("deep/C", &[], &[]),
        ]);
        let g = of(
            v,
            BuildOpts {
                unresolved: false,
                tags: false,
                orphans: true,
            },
        );
        assert_eq!(g.nodes.len(), 3);
        assert_eq!(g.edges.len(), 3); // A→B, A→C, B→A
        let a = g.find("n:a").unwrap();
        assert_eq!((g.links_out[a], g.links_in[a]), (2, 1));
    }

    #[test]
    fn files_other_owners_read_are_nodes_with_their_links() {
        let mut v = workspace(vec![
            note("Lava", &["Eruptions.csv"], &[]),
            note("Krakatoa", &[], &[]),
        ]);
        v.owned.push(crate::workspace::Owned {
            rel: "Data/Eruptions.csv".into(),
            path: PathBuf::from("/v/Data/Eruptions.csv"),
            owner: "csv-table".into(),
            rev: None,
            items: Vec::new(),
            places: HashMap::new(),
            links: vec![(
                crate::workspace::Ref {
                    target: "Krakatoa".into(),
                    line: 1,
                    context: String::new(),
                },
                "2".into(),
            )],
            given: Default::default(),
        });
        let g = of(v, ALL);
        let mut l = labels(&g);
        l.sort();
        assert_eq!(
            l,
            ["Eruptions.csv", "Krakatoa", "Lava"],
            "no unresolved node"
        );
        let csv = g.find("f:data/eruptions.csv").unwrap();
        assert_eq!((g.links_out[csv], g.links_in[csv]), (1, 1));
    }

    #[test]
    fn unresolved_tags_and_orphans() {
        let v = || {
            workspace(vec![
                note("A", &["Ghost", "A"], &["topic"]),
                note("Lonely", &[], &[]),
            ])
        };
        let g = of(v(), ALL);
        let mut l = labels(&g);
        l.sort();
        assert_eq!(l, ["#topic", "A", "Ghost", "Lonely"]);
        // Self-links are dropped.
        assert_eq!(g.edges.len(), 2);

        let no_orphans = BuildOpts {
            orphans: false,
            ..ALL
        };
        assert!(!labels(&of(v(), no_orphans)).contains(&"Lonely"));
    }

    #[test]
    fn shortest_path_wins_on_name_collision() {
        let v = workspace(vec![
            note("x/y/Dup", &[], &[]),
            note("Dup", &[], &[]),
            note("A", &["Dup"], &[]),
        ]);
        let g = of(v, ALL);
        let a = g.find("n:a").unwrap();
        let target = g.edges.iter().find(|e| e.0 == a).unwrap().1;
        assert_eq!(g.nodes[target].rel.as_deref(), Some("Dup"));
    }

    /// Each `./` and `../` form leads to the file its path names, and backlinks, the graph,
    /// following the link and a rename all say the same file.
    #[test]
    fn relative_links_lead_to_one_note_everywhere() {
        use crate::index::{From, Index};
        let dir = std::env::temp_dir().join(format!("cl-graph-relative-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let linking = [
            ("B/Sub/Wiki up.md", "[[../Top]]", "B/Top"),
            ("B/Sub/Markdown up.md", "[t](../Top.md)", "B/Top"),
            ("B/Sub/Wiki up twice.md", "[[../../Top|top]]", "Top"),
            (
                "B/Sub/Markdown up twice.md",
                "[t](../../Top.md#Heading)",
                "Top",
            ),
            ("B/Wiki here.md", "[[./Top]]", "B/Top"),
            ("B/Markdown here.md", "[t](./Top.md)", "B/Top"),
            ("B/Sub/Wiki here deep.md", "[[./Top]]", "B/Sub/Top"),
            ("B/Markdown bare.md", "[t](Top.md)", "Top"),
        ];
        for rel in ["Top.md", "B/Top.md", "B/Sub/Top.md"] {
            std::fs::create_dir_all(dir.join(rel).parent().unwrap()).unwrap();
            std::fs::write(dir.join(rel), "# Heading\n").unwrap();
        }
        for (rel, text, _) in linking {
            std::fs::write(dir.join(rel), text).unwrap();
        }
        let ix = Index::build(crate::workspace::scan(&dir).unwrap());
        let links = crate::links::graph(&ix);
        let g = build(&links, ALL);
        let node = |rel: &str| g.find(&format!("n:{}", rel.to_lowercase())).unwrap();
        for (rel, _, to) in linking {
            let (from, target) = (
                ix.by_rel(rel.trim_end_matches(".md")).unwrap(),
                ix.by_rel(to).unwrap(),
            );
            let note = &ix.workspace.notes[from];
            let link = crate::resolver::split_link(&note.refs[0].target);
            let dir_from = crate::resolver::dir_of(&note.rel);
            assert_eq!(
                ix.resolver.resolve(dir_from, &link.note),
                Some(target),
                "{rel} follows to {to}"
            );
            assert!(
                ix.backlinks(target)
                    .iter()
                    .any(|b| b.from == From::Note(from)),
                "{rel} is a backlink of {to}"
            );
            assert!(
                g.edges
                    .contains(&(node(rel.trim_end_matches(".md")), node(to))),
                "{rel} → {to} in the graph"
            );
        }
        for to in ["Top", "B/Top", "B/Sub/Top"] {
            let target = ix.by_rel(to).unwrap();
            let plan = crate::rename::plan(&ix, target, "Elsewhere/Moved.md").unwrap();
            let mut edited: Vec<&str> = plan.edits.iter().map(|e| e.file.as_str()).collect();
            let mut linked: Vec<&str> = (linking.iter().filter(|l| l.2 == to))
                .map(|l| l.0)
                .collect();
            edited.sort();
            linked.sort();
            assert_eq!(
                edited, linked,
                "a rename of {to} rewrites the links that lead to it"
            );
        }
        let _ = std::fs::remove_dir_all(dir);
    }

    /// The push on each node of a scattered cloud with a few nodes on one spot, summed over
    /// every pair.
    fn cloud_and_exact_push() -> (Vec<Node>, Vec<(f64, f64)>) {
        let mut nodes: Vec<Node> = (0..600)
            .map(|i| Node::new(format!("{i}"), String::new(), Kind::Note, start(i)))
            .collect();
        for k in 0..3 {
            nodes.push(Node::new(
                format!("c{k}"),
                String::new(),
                Kind::Note,
                (5.0, 5.0),
            ));
        }
        let exact = (nodes.iter())
            .map(|a| {
                (nodes.iter()).fold((0.0, 0.0), |(fx, fy), b| {
                    let (dx, dy) = (a.x - b.x, a.y - b.y);
                    let d2 = dx * dx + dy * dy;
                    if d2 == 0.0 || d2 > REACH2 {
                        return (fx, fy);
                    }
                    let d2 = d2.max(1.0);
                    let f = REPULSION / d2 / d2.sqrt();
                    (fx + dx * f, fy + dy * f)
                })
            })
            .collect();
        (nodes, exact)
    }

    #[test]
    fn the_quadtree_pushes_like_every_pair() {
        let (nodes, exact) = cloud_and_exact_push();
        let idx: Vec<usize> = (0..nodes.len()).collect();
        let tree = Quadtree::new(&nodes, &idx);
        let mut stack = Vec::new();
        let (mut err, mut total) = (0.0, 0.0);
        for (i, &(ex, ey)) in exact.iter().enumerate() {
            let (x, y) = tree.push(i, &mut stack, 0.0);
            assert!(
                (x - ex).abs() < 1e-6 && (y - ey).abs() < 1e-6,
                "θ = 0 is exact"
            );
            let (x, y) = tree.push(i, &mut stack, THETA);
            err += ((x - ex).powi(2) + (y - ey).powi(2)).sqrt();
            total += (ex * ex + ey * ey).sqrt();
        }
        assert!(
            err / total < 0.08,
            "θ = {THETA} is off by {:.3}",
            err / total
        );
    }

    #[test]
    fn layout_settles_and_keeps_positions() {
        let v = workspace(
            (0..20)
                .map(|i| note(&format!("N{i}"), &[&format!("N{}", (i + 1) % 20)], &[]))
                .collect(),
        );
        let mut g = of(v, ALL);
        let active = vec![true; g.nodes.len()];
        for _ in 0..400 {
            g.step(&active);
        }
        assert!(g.settled());
        assert!(g.nodes.iter().all(|n| n.x.is_finite() && n.y.is_finite()));
    }
}
