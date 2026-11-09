//! Builds the link graph from a workspace and runs a force-directed layout on it.

use crate::workspace::Workspace;
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

pub fn build(v: &Workspace, o: BuildOpts, prev: &HashMap<String, (f64, f64)>) -> Graph {
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
        nodes.push(Node {
            key: key.clone(),
            label,
            kind,
            path,
            rel,
            tags: Vec::new(),
            x,
            y,
            vx: 0.0,
            vy: 0.0,
            pinned: false,
        });
        index.insert(key, i);
        i
    };

    let resolver = crate::resolver::Resolver::new(v);
    let note_node: Vec<usize> = v
        .notes
        .iter()
        .map(|n| {
            let key = format!("n:{}", n.rel.to_lowercase());
            add(
                &mut nodes,
                key,
                n.name.clone(),
                Kind::Note,
                Some(n.path.clone()),
                Some(n.rel.clone()),
            )
        })
        .collect();
    for (ni, n) in v.notes.iter().enumerate() {
        nodes[note_node[ni]].tags = n.tags.clone();
    }
    // Files other owners read are nodes too, with their links (docs/extensions.md, "Owners and
    // types").
    let owned_node: Vec<usize> = (v.owned.iter())
        .map(|f| {
            let name = f.rel.rsplit('/').next().unwrap_or(&f.rel).to_string();
            let key = format!("f:{}", f.rel.to_lowercase());
            let (path, rel) = (Some(f.path.clone()), Some(f.rel.clone()));
            add(&mut nodes, key, name, Kind::Note, path, rel)
        })
        .collect();

    let mut edge_set: HashSet<(usize, usize)> = HashSet::new();
    let mut edges = Vec::new();
    for (ni, n) in v.notes.iter().enumerate() {
        let from = note_node[ni];
        let dir = crate::resolver::dir_of(&n.rel);
        for t in &n.links {
            let tl = t.to_lowercase();
            let tl = tl.trim_start_matches("./").trim_start_matches('/');
            let to = match (resolver.resolve(dir, t), resolver.resolve_owned(dir, t)) {
                (Some(target), _) => note_node[target],
                (None, Some(file)) => owned_node[file],
                (None, None) if o.unresolved => {
                    let label = t.rsplit('/').next().unwrap_or(t).to_string();
                    add(
                        &mut nodes,
                        format!("u:{tl}"),
                        label,
                        Kind::Unresolved,
                        None,
                        None,
                    )
                }
                (None, None) => continue,
            };
            if to != from && edge_set.insert((from, to)) {
                edges.push((from, to));
            }
        }
        if o.tags {
            for tag in &n.tags {
                let to = add(
                    &mut nodes,
                    format!("t:{tag}"),
                    format!("#{tag}"),
                    Kind::Tag,
                    None,
                    None,
                );
                if edge_set.insert((from, to)) {
                    edges.push((from, to));
                }
            }
        }
    }

    for (fi, f) in v.owned.iter().enumerate() {
        let from = owned_node[fi];
        let dir = crate::resolver::dir_of(&f.rel);
        for (r, _) in &f.links {
            let to = match (
                resolver.resolve(dir, &r.target),
                resolver.resolve_owned(dir, &r.target),
            ) {
                (Some(target), _) => note_node[target],
                (None, Some(file)) => owned_node[file],
                (None, None) => continue,
            };
            if to != from && edge_set.insert((from, to)) {
                edges.push((from, to));
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workspace::Note;

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
        let g = build(
            &v,
            BuildOpts {
                unresolved: false,
                tags: false,
                orphans: true,
            },
            &HashMap::new(),
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
        let g = build(&v, ALL, &HashMap::new());
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
        let v = workspace(vec![
            note("A", &["Ghost", "A"], &["topic"]),
            note("Lonely", &[], &[]),
        ]);
        let g = build(&v, ALL, &HashMap::new());
        let mut l = labels(&g);
        l.sort();
        assert_eq!(l, ["#topic", "A", "Ghost", "Lonely"]);
        // Self-links are dropped.
        assert_eq!(g.edges.len(), 2);

        let no_orphans = BuildOpts {
            orphans: false,
            ..ALL
        };
        assert!(!labels(&build(&v, no_orphans, &HashMap::new())).contains(&"Lonely"));
    }

    #[test]
    fn shortest_path_wins_on_name_collision() {
        let v = workspace(vec![
            note("x/y/Dup", &[], &[]),
            note("Dup", &[], &[]),
            note("A", &["Dup"], &[]),
        ]);
        let g = build(&v, ALL, &HashMap::new());
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
        let g = build(&ix.workspace, ALL, &HashMap::new());
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

    #[test]
    fn layout_settles_and_keeps_positions() {
        let v = workspace(
            (0..20)
                .map(|i| note(&format!("N{i}"), &[&format!("N{}", (i + 1) % 20)], &[]))
                .collect(),
        );
        let mut g = build(&v, ALL, &HashMap::new());
        let active = vec![true; g.nodes.len()];
        for _ in 0..400 {
            g.step(&active);
        }
        assert!(g.settled());
        assert!(g.nodes.iter().all(|n| n.x.is_finite() && n.y.is_finite()));
        // Rebuilding with previous positions keeps nodes where they were.
        let prev = g.positions();
        let g2 = build(&v, ALL, &prev);
        assert_eq!((g2.nodes[3].x, g2.nodes[3].y), prev[&g2.nodes[3].key]);
    }
}
