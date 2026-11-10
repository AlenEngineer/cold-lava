//! The link graph, for `links/query` on the workspace gateway: every file links come from (the
//! notes, then the files other owners read), and where each of their links leads, resolved with
//! Obsidian's rules (`resolver`). It says nothing of how to draw it: the graph extension makes a
//! scene of it. With it come Obsidian's graph colour groups (`.obsidian/graph.json`), which an
//! extension can't read itself, since hidden folders are out of its reach.

use std::collections::HashSet;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::index::Index;
use crate::resolver::dir_of;

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct LinkGraph {
    pub files: Vec<LinkFile>,
    /// Each file's links in the order it has them, once per place they lead.
    pub links: Vec<Link>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub groups: Vec<ColorGroup>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LinkFile {
    /// Workspace-relative, with its extension.
    pub path: String,
    /// What Obsidian calls it: a note's name, another file's name with its extension.
    pub name: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
}

/// A link from `from` to the file `to`, or to `target`, the name written, when it leads
/// nowhere yet.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Link {
    pub from: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
}

/// One of Obsidian's graph colour groups: a search query and its colour, `#rrggbb`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ColorGroup {
    pub query: String,
    pub color: String,
}

pub fn graph(index: &Index) -> LinkGraph {
    let ws = &index.workspace;
    let note_path = |i: usize| format!("{}.md", ws.notes[i].rel);
    let mut files: Vec<LinkFile> = (ws.notes.iter())
        .map(|n| LinkFile {
            path: format!("{}.md", n.rel),
            name: n.name.clone(),
            tags: n.tags.clone(),
        })
        .collect();
    files.extend(ws.owned.iter().map(|f| LinkFile {
        path: f.rel.clone(),
        name: f.rel.rsplit('/').next().unwrap_or(&f.rel).to_string(),
        tags: Vec::new(),
    }));
    let resolve = |dir: &str, target: &str| {
        let note = index.resolver.resolve(dir, target).map(note_path);
        note.or_else(|| {
            Some(
                ws.owned[index.resolver.resolve_owned(dir, target)?]
                    .rel
                    .clone(),
            )
        })
    };
    let mut links = Vec::new();
    let mut seen = HashSet::new();
    let mut add = |from: &str, target: &str| {
        let to = resolve(dir_of(from), target);
        let key = match &to {
            Some(to) => to.clone(),
            None => format!("?{}", target.to_lowercase()),
        };
        if seen.insert((from.to_string(), key)) {
            let target = to.is_none().then(|| target.to_string());
            links.push(Link {
                from: from.to_string(),
                to,
                target,
            });
        }
    };
    for n in &ws.notes {
        let from = format!("{}.md", n.rel);
        for t in &n.links {
            add(&from, t);
        }
    }
    for f in &ws.owned {
        for (r, _) in &f.links {
            add(&f.rel, &r.target);
        }
    }
    LinkGraph {
        files,
        links,
        groups: color_groups(&ws.root),
    }
}

/// `colorGroups` in `.obsidian/graph.json`, the ones with a query and a colour.
pub fn color_groups(root: &Path) -> Vec<ColorGroup> {
    let Ok(text) = std::fs::read_to_string(root.join(".obsidian/graph.json")) else {
        return Vec::new();
    };
    let Ok(json) = serde_json::from_str::<serde_json::Value>(&text) else {
        return Vec::new();
    };
    let Some(list) = json.get("colorGroups").and_then(|v| v.as_array()) else {
        return Vec::new();
    };
    list.iter()
        .filter_map(|g| {
            let query = g.get("query")?.as_str()?.trim().to_string();
            let rgb = g.get("color")?.get("rgb")?.as_u64()? as u32;
            Some(ColorGroup {
                query,
                color: format!("#{:06x}", rgb & 0xff_ffff),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn workspace(files: &[(&str, &str)]) -> std::path::PathBuf {
        static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("cl-links-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        for (rel, text) in files {
            std::fs::create_dir_all(dir.join(rel).parent().unwrap()).unwrap();
            std::fs::write(dir.join(rel), text).unwrap();
        }
        dir
    }

    fn index(dir: &Path) -> Index {
        Index::build(crate::workspace::scan(dir).unwrap())
    }

    #[test]
    fn links_lead_to_files_or_to_names_once_each() {
        let dir = workspace(&[
            (
                "A.md",
                "#topic [[b]] [[deep/C]] [[Missing]] [[missing]] [[B|again]] [[A]]",
            ),
            ("B.md", "[[A]]"),
            ("deep/C.md", ""),
            ("x/y/Dup.md", ""),
            ("Dup.md", "[[Dup]]"),
        ]);
        let g = graph(&index(&dir));
        let paths: Vec<&str> = g.files.iter().map(|f| f.path.as_str()).collect();
        assert_eq!(paths, ["A.md", "B.md", "Dup.md", "deep/C.md", "x/y/Dup.md"]);
        assert_eq!(g.files[0].tags, ["topic"]);
        let to = |from: &str| -> Vec<String> {
            (g.links.iter())
                .filter(|l| l.from == from)
                .map(|l| match (&l.to, &l.target) {
                    (Some(to), _) => to.clone(),
                    (None, Some(t)) => format!("?{t}"),
                    _ => unreachable!(),
                })
                .collect()
        };
        assert_eq!(to("A.md"), ["B.md", "deep/C.md", "?Missing", "A.md"]);
        assert_eq!(to("Dup.md"), ["Dup.md"], "the shallowest wins");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn obsidians_colour_groups_come_with_it() {
        let graph_json = json!({"colorGroups": [
            {"query": "path:Guide ", "color": {"a": 1, "rgb": 0xa78bfa}},
            {"query": "tag:#x", "color": {"a": 1, "rgb": 255}},
            {"color": {"a": 1, "rgb": 1}},
        ]});
        let dir = workspace(&[
            ("A.md", ""),
            (".obsidian/graph.json", &graph_json.to_string()),
        ]);
        let g = graph(&index(&dir));
        assert_eq!(
            serde_json::to_value(&g.groups).unwrap(),
            json!([
                {"query": "path:Guide", "color": "#a78bfa"},
                {"query": "tag:#x", "color": "#0000ff"},
            ])
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn its_json() {
        let g = LinkGraph {
            files: vec![LinkFile {
                path: "Volcanoes/Kīlauea.md".into(),
                name: "Kīlauea".into(),
                tags: vec!["volcano/shield".into()],
            }],
            links: vec![
                Link {
                    from: "Volcanoes/Kīlauea.md".into(),
                    to: Some("Concepts/Lava.md".into()),
                    target: None,
                },
                Link {
                    from: "Volcanoes/Kīlauea.md".into(),
                    to: None,
                    target: Some("Pele".into()),
                },
            ],
            groups: Vec::new(),
        };
        let wire = json!({
            "files": [{"path": "Volcanoes/Kīlauea.md", "name": "Kīlauea",
                       "tags": ["volcano/shield"]}],
            "links": [
                {"from": "Volcanoes/Kīlauea.md", "to": "Concepts/Lava.md"},
                {"from": "Volcanoes/Kīlauea.md", "target": "Pele"},
            ],
        });
        assert_eq!(serde_json::to_value(&g).unwrap(), wire);
        assert_eq!(serde_json::from_value::<LinkGraph>(wire).unwrap(), g);
    }
}
