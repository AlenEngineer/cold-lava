//! Bookmarks, shared with Obsidian: its core Bookmarks plugin keeps them in
//! `.obsidian/bookmarks.json` as `{"items": [...]}`, each item with a `type` (`file`, `folder`,
//! `search`, `group`, `graph`, `url`) and a `ctime`; groups hold `items` of their own.
//!
//! The file is kept as the JSON it was read as (key order included), so whatever cold-lava
//! doesn't understand (other fields, item types from newer Obsidian versions) is written back as
//! it was. `entries` is the typed view the menu draws from; every change goes through `edit`.

use crate::core::Jump;
use crate::write::{self, WriteError};
use anyhow::{Context, Result, bail};
use serde_json::{Map, Value, json};
use std::collections::HashSet;
use std::path::{Path, PathBuf};

/// Where Obsidian keeps the bookmarks, relative to the workspace.
pub const FILE: &str = ".obsidian/bookmarks.json";

/// What a bookmark points at.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Target {
    /// A note or other file (workspace-relative, with its extension), maybe at a heading
    /// (`#Heading`) or block (`#^id`).
    File {
        path: String,
        subpath: Option<String>,
    },
    Folder(String),
    Search(String),
    Graph,
    Url(String),
    Group,
    /// An item type cold-lava doesn't know; it is kept as it is.
    Other(String),
}

/// One bookmark, in the order Obsidian lists them (a group, then what's in it).
#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    /// Where it is: its index in `items`, then in each group's `items` down to it.
    pub at: Vec<usize>,
    pub title: Option<String>,
    pub target: Target,
}

impl Entry {
    pub fn depth(&self) -> usize {
        self.at.len() - 1
    }

    /// What to call it: its own title, else what it points at.
    pub fn label(&self) -> String {
        match self.title.as_ref().filter(|t| !t.is_empty()) {
            Some(t) => t.clone(),
            None => self.target.label(),
        }
    }
}

impl Target {
    /// What a bookmark of it without a title is called.
    pub fn label(&self) -> String {
        match self {
            Target::File { path, subpath } => {
                let name = path.rsplit('/').next().unwrap_or(path);
                let name = name.strip_suffix(".md").unwrap_or(name);
                match subpath.as_deref().map(|s| s.trim_start_matches('#')) {
                    Some(sub) if !sub.is_empty() => format!("{name} › {}", sub.replace('#', " › ")),
                    _ => name.to_string(),
                }
            }
            Target::Folder(path) => format!("{path}/"),
            Target::Search(q) => q.clone(),
            Target::Graph => "Graph".to_string(),
            Target::Url(u) => u.clone(),
            Target::Group => "Group".to_string(),
            Target::Other(kind) => kind.clone(),
        }
    }
}

/// Where in the note a file bookmark's `subpath` leads: `#^id` a block, `#A#B` the heading B.
pub fn jump(subpath: &str) -> Option<Jump> {
    let sub = subpath.trim().trim_start_matches('#');
    if let Some(block) = sub.strip_prefix('^') {
        return Some(Jump::Block(block.to_string()));
    }
    let heading = sub.rsplit('#').next().unwrap_or(sub).trim();
    (!heading.is_empty()).then(|| Jump::Heading(heading.to_string()))
}

/// The workspace's bookmarks file, as read.
#[derive(Clone, Debug, Default)]
pub struct Bookmarks {
    /// The file's bytes when it was read; `None` if there was no file.
    read: Option<String>,
    doc: Map<String, Value>,
}

impl Bookmarks {
    /// Read the workspace's bookmarks; no file is no bookmarks. A file that isn't the JSON object
    /// Obsidian writes is an error, and is never written over.
    pub fn read(root: &Path) -> Result<Bookmarks> {
        let path = root.join(FILE);
        let text = match std::fs::read_to_string(&path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Bookmarks::default()),
            Err(e) => return Err(e).with_context(|| format!("reading {FILE}")),
        };
        Bookmarks::parse(text)
    }

    fn parse(text: String) -> Result<Bookmarks> {
        let doc = match serde_json::from_str(&text) {
            Ok(Value::Object(doc)) => doc,
            Ok(_) => bail!("{FILE} isn't a JSON object"),
            Err(e) => bail!("{FILE} isn't valid JSON: {e}"),
        };
        if doc.get("items").is_some_and(|i| !i.is_array()) {
            bail!("{FILE} has no list of items");
        }
        Ok(Bookmarks {
            read: Some(text),
            doc,
        })
    }

    /// Every bookmark, groups before what they hold.
    pub fn entries(&self) -> Vec<Entry> {
        fn walk(items: &[Value], at: &mut Vec<usize>, out: &mut Vec<Entry>) {
            for (i, item) in items.iter().enumerate() {
                at.push(i);
                let obj = item.as_object();
                let s = |k: &str| obj.and_then(|o| o.get(k)?.as_str()).map(str::to_string);
                let target = match s("type").as_deref() {
                    Some("file") => Target::File {
                        path: s("path").unwrap_or_default(),
                        subpath: s("subpath").filter(|p| !p.is_empty()),
                    },
                    Some("folder") => Target::Folder(s("path").unwrap_or_default()),
                    Some("search") => Target::Search(s("query").unwrap_or_default()),
                    Some("graph") => Target::Graph,
                    Some("url") => Target::Url(s("url").unwrap_or_default()),
                    Some("group") => Target::Group,
                    other => Target::Other(other.unwrap_or("?").to_string()),
                };
                out.push(Entry {
                    at: at.clone(),
                    title: s("title"),
                    target,
                });
                if let Some(inner) = obj.and_then(|o| o.get("items")?.as_array()) {
                    walk(inner, at, out);
                }
                at.pop();
            }
        }
        let mut out = Vec::new();
        walk(self.items(), &mut Vec::new(), &mut out);
        out
    }

    /// The files and folders (workspace-relative) that have a bookmark, at any depth.
    pub fn paths(&self) -> HashSet<String> {
        (self.entries().into_iter())
            .filter_map(|e| match e.target {
                Target::File { path, .. } | Target::Folder(path) => Some(path),
                _ => None,
            })
            .collect()
    }

    /// The first bookmark pointing at `target`.
    pub fn find(&self, target: &Target) -> Option<Entry> {
        self.entries().into_iter().find(|e| &e.target == target)
    }

    /// Bookmark `target` (at the end, as Obsidian adds them), or remove its bookmark if it has
    /// one. Returns whether it is bookmarked now.
    pub fn toggle(&mut self, target: Target, ctime: u64) -> bool {
        match self.find(&target) {
            Some(e) => {
                self.remove(&e.at);
                false
            }
            None => {
                self.add(target, ctime);
                true
            }
        }
    }

    pub fn add(&mut self, target: Target, ctime: u64) {
        let item = match target {
            Target::File { path, subpath } => match subpath {
                Some(sub) => json!({"type": "file", "ctime": ctime, "path": path, "subpath": sub}),
                None => json!({"type": "file", "ctime": ctime, "path": path}),
            },
            Target::Folder(path) => json!({"type": "folder", "ctime": ctime, "path": path}),
            Target::Search(query) => json!({"type": "search", "ctime": ctime, "query": query}),
            Target::Graph => json!({"type": "graph", "ctime": ctime}),
            Target::Url(url) => json!({"type": "url", "ctime": ctime, "url": url}),
            Target::Group => json!({"type": "group", "ctime": ctime, "items": []}),
            Target::Other(_) => return,
        };
        let items = self
            .doc
            .entry("items")
            .or_insert_with(|| Value::Array(Vec::new()));
        if let Value::Array(items) = items {
            items.push(item);
        }
    }

    /// Remove the bookmark at `at` (a group goes with what it holds).
    pub fn remove(&mut self, at: &[usize]) {
        let Some((&last, parents)) = at.split_last() else {
            return;
        };
        let mut items = self.doc.get_mut("items").and_then(Value::as_array_mut);
        for &i in parents {
            items = items
                .and_then(|v| v.get_mut(i))
                .and_then(|g| g.get_mut("items"))
                .and_then(Value::as_array_mut);
        }
        if let Some(items) = items.filter(|v| last < v.len()) {
            items.remove(last);
        }
    }

    /// Point file and folder bookmarks at where `moved` says their path went (a note renamed,
    /// a folder moved), keeping headings and everything else. Returns the paths changed.
    pub fn moved(&mut self, moved: impl Fn(&str) -> Option<String>) -> Vec<(String, String)> {
        fn walk(
            items: &mut [Value],
            moved: &dyn Fn(&str) -> Option<String>,
            out: &mut Vec<(String, String)>,
        ) {
            for item in items.iter_mut().filter_map(Value::as_object_mut) {
                let kind = item.get("type").and_then(Value::as_str);
                if matches!(kind, Some("file" | "folder"))
                    && let Some(Value::String(path)) = item.get_mut("path")
                    && let Some(to) = moved(path)
                    && to != *path
                {
                    out.push((std::mem::replace(path, to.clone()), to));
                }
                if let Some(Value::Array(inner)) = item.get_mut("items") {
                    walk(inner, moved, out);
                }
            }
        }
        let mut out = Vec::new();
        if let Some(Value::Array(items)) = self.doc.get_mut("items") {
            walk(items, &moved, &mut out);
        }
        out
    }

    /// The file's text: as read if nothing changed, else Obsidian's own format (two-space
    /// JSON, keys in their order), keeping a final newline if the file had one.
    pub fn text(&self) -> String {
        let mut text = serde_json::to_string_pretty(&self.doc).unwrap_or_default();
        if self.read.as_ref().is_some_and(|r| r.ends_with('\n')) {
            text.push('\n');
        }
        text
    }

    /// The text as it was read (`None`: there was no file).
    pub fn read_text(&self) -> Option<&str> {
        self.read.as_deref()
    }

    fn items(&self) -> &[Value] {
        self.doc
            .get("items")
            .and_then(Value::as_array)
            .map_or(&[], Vec::as_slice)
    }

    /// Write the changes, unless the file changed since it was read (or appeared).
    fn save(&self, path: &Path) -> Result<(), WriteError> {
        let text = self.text();
        match &self.read {
            Some(read) if *read == text => Ok(()),
            Some(read) => write::replace(path, read.as_bytes(), text.as_bytes()),
            None => write::create(path, text.as_bytes()),
        }
    }
}

/// Change the workspace's bookmarks with `change` and write them. If Obsidian wrote the file in the
/// meantime, read it again and redo the change once on what it wrote.
pub fn edit<R>(root: &Path, mut change: impl FnMut(&mut Bookmarks) -> R) -> Result<R> {
    let path: PathBuf = root.join(FILE);
    let mut tries = 0;
    loop {
        let mut b = Bookmarks::read(root)?;
        let out = change(&mut b);
        match b.save(&path) {
            Ok(()) => return Ok(out),
            Err(WriteError::Changed(_) | WriteError::Exists(_)) if tries == 0 => tries += 1,
            Err(e) => return Err(e).context("saving the bookmarks"),
        }
    }
}

/// Now, as Obsidian's `ctime`: milliseconds since 1970.
pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// As Obsidian 1.x writes it, with a field and an item type cold-lava doesn't know.
    const OBSIDIAN: &str = r##"{
  "items": [
    {
      "type": "file",
      "ctime": 1730000000000,
      "path": "Concepts/Lava.md"
    },
    {
      "type": "file",
      "ctime": 1730000000001,
      "path": "Concepts/Lava.md",
      "subpath": "#Textures",
      "title": "Lava textures"
    },
    {
      "type": "group",
      "ctime": 1730000000002,
      "items": [
        {
          "type": "folder",
          "ctime": 1730000000003,
          "path": "People"
        },
        {
          "type": "search",
          "ctime": 1730000000004,
          "query": "tag:#volcano"
        },
        {
          "type": "canvas-node",
          "ctime": 1730000000005,
          "zeta": {
            "b": 1,
            "a": [
              2,
              3
            ]
          },
          "alpha": "kept"
        }
      ],
      "title": "Research",
      "colour": "red"
    },
    {
      "type": "graph",
      "ctime": 1730000000006,
      "title": "Graph",
      "options": {
        "search": "",
        "showTags": false
      }
    },
    {
      "type": "url",
      "ctime": 1730000000007,
      "url": "https://obsidian.md",
      "title": "Obsidian"
    }
  ],
  "version": 7
}"##;

    fn dir(name: &str) -> PathBuf {
        let d =
            std::env::temp_dir().join(format!("cold-lava-bookmarks-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join(".obsidian")).unwrap();
        d
    }

    #[test]
    fn reads_every_kind_and_writes_back_the_same_bytes() {
        let b = Bookmarks::parse(OBSIDIAN.to_string()).unwrap();
        assert_eq!(
            b.text(),
            OBSIDIAN,
            "unchanged, it round-trips byte for byte"
        );
        let labels: Vec<(usize, String)> = (b.entries().iter())
            .map(|e| (e.depth(), e.label()))
            .collect();
        assert_eq!(
            labels,
            [
                (0, "Lava".to_string()),
                (0, "Lava textures".to_string()),
                (0, "Research".to_string()),
                (1, "People/".to_string()),
                (1, "tag:#volcano".to_string()),
                (1, "canvas-node".to_string()),
                (0, "Graph".to_string()),
                (0, "Obsidian".to_string()),
            ]
        );
        assert_eq!(
            b.entries()[1].target,
            Target::File {
                path: "Concepts/Lava.md".into(),
                subpath: Some("#Textures".into())
            }
        );
        assert_eq!(
            b.paths(),
            HashSet::from(["Concepts/Lava.md".to_string(), "People".to_string()])
        );
    }

    #[test]
    fn adding_and_removing_keeps_what_it_does_not_understand() {
        let mut b = Bookmarks::parse(OBSIDIAN.to_string()).unwrap();
        let note = Target::File {
            path: "Volcanoes/Krakatoa.md".into(),
            subpath: None,
        };
        assert!(b.toggle(note.clone(), 42));
        let text = b.text();
        let added = "    {\n      \"type\": \"file\",\n      \"ctime\": 42,\n      \"path\": \"Volcanoes/Krakatoa.md\"\n    }\n  ],\n  \"version\": 7\n}";
        assert!(text.ends_with(added), "{text}");
        let head = &OBSIDIAN[..OBSIDIAN.len() - "\n  ],\n  \"version\": 7\n}".len()];
        assert!(text.starts_with(head), "the rest is as it was");
        assert!(!b.toggle(note, 43), "a second toggle removes it");
        assert_eq!(b.text(), OBSIDIAN);

        // Removing inside a group, and a search toggled off where it is.
        assert!(!b.toggle(Target::Search("tag:#volcano".into()), 1));
        let text = b.text();
        assert!(!text.contains("tag:#volcano"));
        assert!(
            text.contains("\"zeta\": {\n            \"b\": 1,"),
            "{text}"
        );
        assert!(text.contains("\"colour\": \"red\""));
    }

    #[test]
    fn moves_follow_notes_and_folders_keeping_subpaths() {
        let mut b = Bookmarks::parse(OBSIDIAN.to_string()).unwrap();
        let changed = b.moved(|p| match p {
            "Concepts/Lava.md" => Some("Rock/Magma.md".into()),
            "People" => Some("Guide/People".into()),
            _ => None,
        });
        assert_eq!(changed.len(), 3);
        let e = b.entries();
        assert_eq!(
            e[1].target,
            Target::File {
                path: "Rock/Magma.md".into(),
                subpath: Some("#Textures".into())
            }
        );
        assert_eq!(e[3].target, Target::Folder("Guide/People".into()));
        assert_eq!(
            b.text(),
            OBSIDIAN
                .replace("Concepts/Lava.md", "Rock/Magma.md")
                .replace("\"People\"", "\"Guide/People\"")
        );
    }

    #[test]
    fn edit_creates_the_file_and_redoes_a_change_after_an_outside_write() {
        let d = dir("edit");
        let target = Target::Search("lava".into());
        assert!(edit(&d, |b| b.toggle(target.clone(), 1)).unwrap());
        let made = std::fs::read_to_string(d.join(FILE)).unwrap();
        assert_eq!(
            made,
            "{\n  \"items\": [\n    {\n      \"type\": \"search\",\n      \"ctime\": 1,\n      \"query\": \"lava\"\n    }\n  ]\n}"
        );

        // Obsidian writes between our read and our write: the change is made again on its file.
        std::fs::write(d.join(FILE), OBSIDIAN).unwrap();
        let mut first = true;
        let note = Target::File {
            path: "A.md".into(),
            subpath: None,
        };
        let added = edit(&d, |b| {
            if std::mem::take(&mut first) {
                std::fs::write(
                    d.join(FILE),
                    OBSIDIAN.replace("\"version\": 7", "\"version\": 8"),
                )
                .unwrap();
            }
            b.toggle(note.clone(), 5)
        })
        .unwrap();
        assert!(added);
        let now = std::fs::read_to_string(d.join(FILE)).unwrap();
        assert!(now.contains("\"version\": 8") && now.contains("\"path\": \"A.md\""));

        // Nothing changed: nothing is written.
        let before = std::fs::metadata(d.join(FILE)).unwrap().modified().unwrap();
        edit(&d, |b| b.moved(|_| None)).unwrap();
        assert_eq!(
            std::fs::metadata(d.join(FILE)).unwrap().modified().unwrap(),
            before
        );
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn refuses_a_file_it_cannot_read() {
        let d = dir("bad");
        std::fs::write(d.join(FILE), "{\"items\": [").unwrap();
        assert!(edit(&d, |b| b.toggle(Target::Graph, 1)).is_err());
        assert_eq!(
            std::fs::read_to_string(d.join(FILE)).unwrap(),
            "{\"items\": ["
        );
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn subpaths_lead_to_headings_and_blocks() {
        assert_eq!(jump("#Textures"), Some(Jump::Heading("Textures".into())));
        assert_eq!(
            jump("#Lava#Textures"),
            Some(Jump::Heading("Textures".into()))
        );
        assert_eq!(jump("#^abc"), Some(Jump::Block("abc".into())));
        assert_eq!(jump(""), None);
    }
}
