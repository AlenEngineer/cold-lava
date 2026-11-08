//! The workspace index: the start of cold-lava's core. It resolves links the way Obsidian does
//! and knows every note's backlinks, so features don't each re-derive them.

use crate::workspace::Workspace;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// Resolves link targets (`Note`, `folder/Note`, `Note.md`) to notes, like Obsidian:
/// by workspace-relative path first, then by file name, preferring the shallowest match. Files
/// other owners read are found the same way, by their name with its extension.
pub struct Resolver {
    by_rel: HashMap<String, usize>,
    by_name: HashMap<String, usize>,
    owned_by_rel: HashMap<String, usize>,
    owned_by_name: HashMap<String, usize>,
}

impl Resolver {
    pub fn new(v: &Workspace) -> Resolver {
        let mut by_rel = HashMap::new();
        let mut by_name: HashMap<String, usize> = HashMap::new();
        for (i, n) in v.notes.iter().enumerate() {
            by_rel.insert(n.rel.to_lowercase(), i);
            let e = by_name.entry(n.name.to_lowercase()).or_insert(i);
            if n.rel.matches('/').count() < v.notes[*e].rel.matches('/').count() {
                *e = i;
            }
        }
        let mut owned_by_rel = HashMap::new();
        let mut owned_by_name: HashMap<String, usize> = HashMap::new();
        for (i, f) in v.owned.iter().enumerate() {
            let rel = f.rel.to_lowercase();
            let name = rel.rsplit('/').next().unwrap_or(&rel).to_string();
            let e = owned_by_name.entry(name).or_insert(i);
            if f.rel.matches('/').count() < v.owned[*e].rel.matches('/').count() {
                *e = i;
            }
            owned_by_rel.insert(rel, i);
        }
        Resolver {
            by_rel,
            by_name,
            owned_by_rel,
            owned_by_name,
        }
    }

    /// `target` without `#heading` / `|alias` (as stored in `Note::links`).
    pub fn resolve(&self, target: &str) -> Option<usize> {
        let t = target.trim().to_lowercase();
        let t = t.strip_suffix(".md").unwrap_or(&t);
        let t = t.trim_start_matches("./").trim_start_matches('/');
        self.by_rel
            .get(t)
            .or_else(|| self.by_name.get(t.rsplit('/').next().unwrap_or(t)))
            .copied()
    }

    /// `resolve`, for a file another owner reads (`Workspace::owned`): `Eruptions.csv`.
    pub fn resolve_owned(&self, target: &str) -> Option<usize> {
        let t = target.trim().to_lowercase();
        let t = t.trim_start_matches("./").trim_start_matches('/');
        self.owned_by_rel
            .get(t)
            .or_else(|| self.owned_by_name.get(t.rsplit('/').next().unwrap_or(t)))
            .copied()
    }
}

/// Where a link was written: a note, or a file another owner reads (`Workspace::owned`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum From {
    Note(usize),
    Owned(usize),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Backlink {
    pub from: From,
    /// In the note, or in the owner's Markdown piece.
    pub line: usize,
    /// In a file another owner reads: the owner's place for the link.
    pub place: Option<String>,
    pub context: String,
}

pub struct Index {
    pub workspace: Workspace,
    pub resolver: Resolver,
    backlinks: Vec<Vec<Backlink>>,
    outgoing: Vec<usize>,
    folded: OnceLock<Vec<Folded>>,
}

/// A note's text folded for searching (lowercase, no accents), computed on first use.
pub struct Folded {
    pub name: String,
    pub whole: String,
    pub lines: Vec<String>,
}

/// A link as written in a note: target, optional heading or block (`#Heading`, `#^block`).
pub struct LinkTarget {
    pub note: String,
    pub heading: Option<String>,
    pub block: Option<String>,
}

/// Split `Note#Heading|alias` / `Note#^block` into its parts.
pub fn split_link(raw: &str) -> LinkTarget {
    let raw = raw.split('|').next().unwrap_or(raw).trim();
    let decode = |s: &str| crate::workspace::decode_target(s.trim());
    match raw.split_once('#') {
        Some((note, rest)) => match rest.strip_prefix('^') {
            Some(block) => LinkTarget {
                note: decode(note),
                heading: None,
                block: Some(decode(block)),
            },
            None => LinkTarget {
                note: decode(note),
                heading: Some(decode(rest)),
                block: None,
            },
        },
        None => LinkTarget {
            note: decode(raw),
            heading: None,
            block: None,
        },
    }
}

impl Index {
    pub fn build(workspace: Workspace) -> Index {
        let mut ix = Index {
            resolver: Resolver::new(&workspace),
            workspace,
            backlinks: Vec::new(),
            outgoing: Vec::new(),
            folded: OnceLock::new(),
        };
        ix.relink();
        ix
    }

    /// Recompute link resolution and backlinks from the notes in memory (no disk access).
    pub(crate) fn relink(&mut self) {
        let workspace = &self.workspace;
        self.resolver = Resolver::new(workspace);
        let n = workspace.notes.len();
        let mut backlinks = vec![Vec::new(); n];
        let mut outgoing = vec![0; n];
        let notes = (workspace.notes.iter().enumerate())
            .flat_map(|(i, n)| n.refs.iter().map(move |r| (From::Note(i), r, None)));
        let owned = (workspace.owned.iter().enumerate()).flat_map(|(i, f)| {
            (f.links.iter()).map(move |(r, place)| (From::Owned(i), r, Some(place)))
        });
        for (from, r, place) in notes.chain(owned) {
            let Some(to) = self.resolver.resolve(&r.target) else {
                if let From::Note(from) = from
                    && self.resolver.resolve_owned(&r.target).is_some()
                {
                    outgoing[from] += 1;
                }
                continue;
            };
            if let From::Note(from) = from {
                outgoing[from] += 1;
            }
            if from != From::Note(to) {
                backlinks[to].push(Backlink {
                    from,
                    line: r.line,
                    place: place.cloned(),
                    context: r.context.clone(),
                });
            }
        }
        for list in &mut backlinks {
            list.sort_by(|a, b| {
                let (na, nb) = (self.name(a.from), self.name(b.from));
                na.to_lowercase()
                    .cmp(&nb.to_lowercase())
                    .then(a.line.cmp(&b.line))
            });
        }
        self.backlinks = backlinks;
        self.outgoing = outgoing;
        self.folded = OnceLock::new();
    }

    /// The name of the file a link was written in: a note's, or an owned file's with its
    /// extension.
    pub fn name(&self, from: From) -> &str {
        match from {
            From::Note(i) => &self.workspace.notes[i].name,
            From::Owned(i) => {
                let rel = &self.workspace.owned[i].rel;
                rel.rsplit('/').next().unwrap_or(rel)
            }
        }
    }

    /// The path of the file a link was written in.
    pub fn path_of(&self, from: From) -> PathBuf {
        match from {
            From::Note(i) => self.path(i),
            From::Owned(i) => self.workspace.owned[i].path.clone(),
        }
    }

    /// Bring these paths up to date with the disk, then update links and backlinks once.
    /// Returns what changed.
    pub fn refresh(&mut self, paths: &[PathBuf]) -> Vec<crate::workspace::NoteChange> {
        let changes: Vec<_> = paths
            .iter()
            .filter_map(|p| self.workspace.refresh_path(p))
            .collect();
        if !changes.is_empty() {
            self.relink();
        }
        changes
    }

    pub fn folded(&self) -> &[Folded] {
        self.folded.get_or_init(|| {
            self.workspace
                .notes
                .iter()
                .map(|n| {
                    let lines: Vec<String> =
                        n.text.lines().map(crate::workspace::search_key).collect();
                    Folded {
                        name: crate::workspace::search_key(&n.name),
                        whole: lines.join("\n"),
                        lines,
                    }
                })
                .collect()
        })
    }

    /// A note by workspace-relative path, with or without `.md`.
    pub fn by_rel(&self, rel: &str) -> Option<usize> {
        let rel = rel.strip_suffix(".md").unwrap_or(rel).to_lowercase();
        self.workspace
            .notes
            .iter()
            .position(|n| n.rel.to_lowercase() == rel)
    }

    pub fn by_path(&self, path: &Path) -> Option<usize> {
        let rel = path
            .strip_prefix(&self.workspace.root)
            .ok()?
            .to_string_lossy()
            .replace('\\', "/");
        self.by_rel(&rel)
    }

    pub fn path(&self, i: usize) -> PathBuf {
        self.workspace.notes[i].path.clone()
    }

    pub fn backlinks(&self, i: usize) -> &[Backlink] {
        &self.backlinks[i]
    }

    /// (incoming links from other notes, resolved outgoing links)
    pub fn counts(&self, i: usize) -> (usize, usize) {
        (self.backlinks[i].len(), self.outgoing[i])
    }

    /// The file an attachment link (`[[map.pdf]]`, `[map](../Maps/map.pdf)`, already decoded)
    /// leads to: by workspace path, then next to the note `from`, then by file name anywhere,
    /// shallowest first. `None` when it isn't in the workspace, or when `target` names a note.
    pub fn attachment(&self, target: &str, from: Option<usize>) -> Option<PathBuf> {
        crate::workspace::attachment_kind(target)?;
        self.file(target, from)
    }

    /// The file a link written in note `from` leads to, of any kind, found the way
    /// `attachment` finds one.
    pub fn file(&self, target: &str, from: Option<usize>) -> Option<PathBuf> {
        let files = crate::rename::Files::of_workspace(self);
        let dir = from.map_or("", |i| crate::rename::dir_of(&self.workspace.notes[i].rel));
        let i = files.resolve(dir, target)?;
        Some(self.workspace.root.join(&files.rels[i]))
    }

    /// Where a new note for an unresolved link goes, following Obsidian's
    /// "Default location for new notes" setting (`.obsidian/app.json`).
    pub fn new_note_path(&self, name: &str, from: Option<usize>) -> PathBuf {
        self.new_file_path(name.trim().trim_end_matches(".md"), "md", from)
    }

    /// Where a new `name.extension` goes: in the folder `name` names, else where Obsidian puts
    /// new files, as for notes.
    pub fn new_file_path(&self, name: &str, extension: &str, from: Option<usize>) -> PathBuf {
        // A link with a folder (`[[Ideas/New]]`) says where it goes.
        if name.contains('/') {
            return self.workspace.root.join(format!("{name}.{extension}"));
        }
        let app: serde_json::Value =
            std::fs::read_to_string(self.workspace.root.join(".obsidian/app.json"))
                .ok()
                .and_then(|t| serde_json::from_str(&t).ok())
                .unwrap_or_default();
        let dir = match app.get("newFileLocation").and_then(|v| v.as_str()) {
            Some("current") => from
                .and_then(|i| self.workspace.notes[i].path.parent().map(Path::to_path_buf))
                .unwrap_or_else(|| self.workspace.root.clone()),
            Some("folder") => {
                let f = app
                    .get("newFileFolderPath")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                self.workspace.root.join(f.trim_matches('/'))
            }
            _ => self.workspace.root.clone(),
        };
        dir.join(format!("{name}.{extension}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn example() -> Index {
        Index::build(
            crate::workspace::scan(
                &Path::new(env!("CARGO_MANIFEST_DIR")).join("../../example-workspace"),
            )
            .unwrap(),
        )
    }

    #[test]
    fn finds_attachments_by_path_beside_the_note_and_by_name() {
        let dir = std::env::temp_dir().join(format!("cl-attach-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let files = [
            "workspace/Maps/deep/map.pdf",
            "workspace/Notes/Trip.md",
            "workspace/Notes/ticket.pdf",
            "workspace/Mount St. Helens.md",
            "outside.pdf",
        ];
        for f in files {
            let p = dir.join(f);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, "").unwrap();
        }
        let ix = Index::build(crate::workspace::scan(&dir.join("workspace")).unwrap());
        let root = ix.workspace.root.clone();
        let trip = ix.by_rel("Notes/Trip");
        let found = |t: &str| {
            ix.attachment(t, trip)
                .map(|p| p.strip_prefix(&root).unwrap().to_owned())
        };
        assert_eq!(found("Maps/deep/map.pdf"), Some("Maps/deep/map.pdf".into()));
        assert_eq!(found("MAP.PDF"), Some("Maps/deep/map.pdf".into()));
        assert_eq!(found("ticket.pdf"), Some("Notes/ticket.pdf".into()));
        assert_eq!(
            found("../Maps/deep/map.pdf"),
            Some("Maps/deep/map.pdf".into())
        );
        assert_eq!(found("missing.pdf"), None);
        assert_eq!(found("../../outside.pdf"), None, "outside the workspace");
        assert_eq!(
            found("Mount St. Helens"),
            None,
            "a dotted note name is a note"
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn resolves_like_obsidian() {
        let ix = example();
        let name = |i: Option<usize>| i.map(|i| ix.workspace.notes[i].rel.clone());
        assert_eq!(
            name(ix.resolver.resolve("kīlauea")),
            Some("Volcanoes/Kīlauea".into())
        );
        assert_eq!(
            name(ix.resolver.resolve("Volcanoes/Vesuvius.md")),
            Some("Volcanoes/Vesuvius".into())
        );
        assert_eq!(ix.resolver.resolve("Lahar"), None);
        let link = split_link("Volcanoes/K%C4%ABlauea.md#Lava%20lake");
        assert_eq!(
            name(ix.resolver.resolve(&link.note)),
            Some("Volcanoes/Kīlauea".into())
        );
        assert_eq!(link.heading.as_deref(), Some("Lava lake"));
    }

    #[test]
    fn backlinks_with_context() {
        let ix = example();
        let helens = ix.by_rel("Volcanoes/Mount St. Helens.md").unwrap();
        let from: Vec<&str> = ix
            .backlinks(helens)
            .iter()
            .map(|b| ix.name(b.from))
            .collect();
        assert!(from.contains(&"David A. Johnston"));
        assert!(from.contains(&"Ring of Fire"));
        let johnston = ix
            .backlinks(helens)
            .iter()
            .find(|b| ix.name(b.from) == "David A. Johnston")
            .unwrap();
        assert!(johnston.context.contains("[[Mount St. Helens]]"));
        assert!(
            ix.backlinks(ix.by_rel("Inbox/Loose thought").unwrap())
                .is_empty()
        );
    }

    #[test]
    fn refreshes_one_note_at_a_time() {
        use crate::workspace::NoteChange;
        let dir = std::env::temp_dir().join(format!("cold-lava-index-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("A.md"), "links to [[B]]").unwrap();
        std::fs::write(dir.join("B.md"), "nothing").unwrap();
        let mut ix = Index::build(crate::workspace::scan(&dir).unwrap());
        let root = ix.workspace.root.clone();
        let b = |ix: &Index| ix.by_rel("B").unwrap();
        assert_eq!(ix.backlinks(b(&ix)).len(), 1);

        // A new note linking to B.
        std::fs::write(root.join("C.md"), "also [[B]]").unwrap();
        assert_eq!(
            ix.refresh(&[root.join("C.md")]),
            [NoteChange::Created("C".into())]
        );
        assert_eq!(ix.backlinks(b(&ix)).len(), 2);
        // A drops its link; an unchanged file reports nothing.
        std::fs::write(root.join("A.md"), "no links").unwrap();
        assert_eq!(
            ix.refresh(&[root.join("A.md"), root.join("B.md")]),
            [NoteChange::Changed("A".into())]
        );
        assert_eq!(ix.backlinks(b(&ix)).len(), 1);
        // C is deleted; hidden folders and other files are ignored.
        std::fs::remove_file(root.join("C.md")).unwrap();
        std::fs::create_dir_all(root.join(".trash")).unwrap();
        std::fs::write(root.join(".trash/D.md"), "[[B]]").unwrap();
        let changed = ix.refresh(&[
            root.join("C.md"),
            root.join(".trash/D.md"),
            root.join("x.png"),
        ]);
        assert_eq!(changed, [NoteChange::Removed("C".into())]);
        assert!(ix.backlinks(b(&ix)).is_empty());
        assert_eq!(ix.workspace.notes.len(), 2);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn splits_links() {
        let l = split_link("Lava#Temperature|lava temps");
        assert_eq!(
            (l.note.as_str(), l.heading.as_deref(), l.block.as_deref()),
            ("Lava", Some("Temperature"), None)
        );
        let l = split_link("Note#^abc123");
        assert_eq!(
            (l.note.as_str(), l.block.as_deref()),
            ("Note", Some("abc123"))
        );
        assert_eq!(split_link("Other%20Note").note, "Other Note");
    }
}
