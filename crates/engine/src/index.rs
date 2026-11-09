//! The workspace index: the start of cold-lava's core. It knows every note's backlinks, from
//! links the workspace's resolver (`resolver.rs`) resolved, so features don't each re-derive them.

use crate::resolver::{Resolver, dir_of};
use crate::workspace::Workspace;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

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
        let notes = (workspace.notes.iter().enumerate()).flat_map(|(i, n)| {
            let dir = dir_of(&n.rel);
            n.refs.iter().map(move |r| (From::Note(i), dir, r, None))
        });
        let owned = (workspace.owned.iter().enumerate()).flat_map(|(i, f)| {
            let dir = dir_of(&f.rel);
            (f.links.iter()).map(move |(r, place)| (From::Owned(i), dir, r, Some(place)))
        });
        for (from, dir, r, place) in notes.chain(owned) {
            let Some(to) = self.resolver.resolve(dir, &r.target) else {
                if let From::Note(from) = from
                    && self.resolver.resolve_owned(dir, &r.target).is_some()
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
}
