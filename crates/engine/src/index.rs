//! The vault index: the start of cold-lava's core. It resolves links the way Obsidian does
//! and knows every note's backlinks, so features don't each re-derive them.

use crate::vault::Vault;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// Resolves link targets (`Note`, `folder/Note`, `Note.md`) to notes, like Obsidian:
/// by vault-relative path first, then by file name, preferring the shallowest match.
pub struct Resolver {
    by_rel: HashMap<String, usize>,
    by_name: HashMap<String, usize>,
}

impl Resolver {
    pub fn new(v: &Vault) -> Resolver {
        let mut by_rel = HashMap::new();
        let mut by_name: HashMap<String, usize> = HashMap::new();
        for (i, n) in v.notes.iter().enumerate() {
            by_rel.insert(n.rel.to_lowercase(), i);
            let e = by_name.entry(n.name.to_lowercase()).or_insert(i);
            if n.rel.matches('/').count() < v.notes[*e].rel.matches('/').count() {
                *e = i;
            }
        }
        Resolver { by_rel, by_name }
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
}

#[derive(Clone, Debug, PartialEq)]
pub struct Backlink {
    /// Index of the linking note.
    pub from: usize,
    pub line: usize,
    pub context: String,
}

pub struct Index {
    pub vault: Vault,
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
    let decode = |s: &str| crate::vault::decode_target(s.trim());
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
    pub fn build(vault: Vault) -> Index {
        let mut ix = Index {
            resolver: Resolver::new(&vault),
            vault,
            backlinks: Vec::new(),
            outgoing: Vec::new(),
            folded: OnceLock::new(),
        };
        ix.relink();
        ix
    }

    /// Recompute link resolution and backlinks from the notes in memory (no disk access).
    fn relink(&mut self) {
        let vault = &self.vault;
        self.resolver = Resolver::new(vault);
        let n = vault.notes.len();
        let mut backlinks = vec![Vec::new(); n];
        let mut outgoing = vec![0; n];
        for (from, note) in vault.notes.iter().enumerate() {
            for r in &note.refs {
                if let Some(to) = self.resolver.resolve(&r.target) {
                    outgoing[from] += 1;
                    if to != from {
                        backlinks[to].push(Backlink {
                            from,
                            line: r.line,
                            context: r.context.clone(),
                        });
                    }
                }
            }
        }
        for list in &mut backlinks {
            list.sort_by(|a, b| {
                let (na, nb) = (&vault.notes[a.from].name, &vault.notes[b.from].name);
                na.to_lowercase()
                    .cmp(&nb.to_lowercase())
                    .then(a.line.cmp(&b.line))
            });
        }
        self.backlinks = backlinks;
        self.outgoing = outgoing;
        self.folded = OnceLock::new();
    }

    /// Bring these paths up to date with the disk, then update links and backlinks once.
    /// Returns what changed.
    pub fn refresh(&mut self, paths: &[PathBuf]) -> Vec<crate::vault::NoteChange> {
        let changes: Vec<_> = paths
            .iter()
            .filter_map(|p| self.vault.refresh_path(p))
            .collect();
        if !changes.is_empty() {
            self.relink();
        }
        changes
    }

    pub fn folded(&self) -> &[Folded] {
        self.folded.get_or_init(|| {
            self.vault
                .notes
                .iter()
                .map(|n| {
                    let lines: Vec<String> = n.text.lines().map(crate::vault::search_key).collect();
                    Folded {
                        name: crate::vault::search_key(&n.name),
                        whole: lines.join("\n"),
                        lines,
                    }
                })
                .collect()
        })
    }

    /// A note by vault-relative path, with or without `.md`.
    pub fn by_rel(&self, rel: &str) -> Option<usize> {
        let rel = rel.strip_suffix(".md").unwrap_or(rel).to_lowercase();
        self.vault
            .notes
            .iter()
            .position(|n| n.rel.to_lowercase() == rel)
    }

    pub fn by_path(&self, path: &Path) -> Option<usize> {
        let rel = path
            .strip_prefix(&self.vault.root)
            .ok()?
            .to_string_lossy()
            .replace('\\', "/");
        self.by_rel(&rel)
    }

    pub fn path(&self, i: usize) -> PathBuf {
        self.vault.notes[i].path.clone()
    }

    pub fn backlinks(&self, i: usize) -> &[Backlink] {
        &self.backlinks[i]
    }

    /// (incoming links from other notes, resolved outgoing links)
    pub fn counts(&self, i: usize) -> (usize, usize) {
        (self.backlinks[i].len(), self.outgoing[i])
    }

    /// The file an attachment link (`[[map.pdf]]`, `[map](../Maps/map.pdf)`, already decoded)
    /// leads to: by vault path, then next to the note `from`, then by file name anywhere,
    /// shallowest first. `None` when it isn't in the vault, or when `target` names a note.
    pub fn attachment(&self, target: &str, from: Option<usize>) -> Option<PathBuf> {
        crate::vault::attachment_kind(target)?;
        let files = crate::rename::Files::of_vault(self);
        let dir = from.map_or("", |i| crate::rename::dir_of(&self.vault.notes[i].rel));
        let i = files.resolve(dir, target)?;
        Some(self.vault.root.join(&files.rels[i]))
    }

    /// Where a new note for an unresolved link goes, following Obsidian's
    /// "Default location for new notes" setting (`.obsidian/app.json`).
    pub fn new_note_path(&self, name: &str, from: Option<usize>) -> PathBuf {
        let name = name.trim().trim_end_matches(".md");
        // A link with a folder (`[[Ideas/New]]`) says where it goes.
        if name.contains('/') {
            return self.vault.root.join(format!("{name}.md"));
        }
        let app: serde_json::Value =
            std::fs::read_to_string(self.vault.root.join(".obsidian/app.json"))
                .ok()
                .and_then(|t| serde_json::from_str(&t).ok())
                .unwrap_or_default();
        let dir = match app.get("newFileLocation").and_then(|v| v.as_str()) {
            Some("current") => from
                .and_then(|i| self.vault.notes[i].path.parent().map(Path::to_path_buf))
                .unwrap_or_else(|| self.vault.root.clone()),
            Some("folder") => {
                let f = app
                    .get("newFileFolderPath")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                self.vault.root.join(f.trim_matches('/'))
            }
            _ => self.vault.root.clone(),
        };
        dir.join(format!("{name}.md"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn example() -> Index {
        Index::build(
            crate::vault::scan(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../../example-vault"))
                .unwrap(),
        )
    }

    #[test]
    fn finds_attachments_by_path_beside_the_note_and_by_name() {
        let dir = std::env::temp_dir().join(format!("cl-attach-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let files = [
            "vault/Maps/deep/map.pdf",
            "vault/Notes/Trip.md",
            "vault/Notes/ticket.pdf",
            "vault/Mount St. Helens.md",
            "outside.pdf",
        ];
        for f in files {
            let p = dir.join(f);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, "").unwrap();
        }
        let ix = Index::build(crate::vault::scan(&dir.join("vault")).unwrap());
        let root = ix.vault.root.clone();
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
        assert_eq!(found("../../outside.pdf"), None, "outside the vault");
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
        let name = |i: Option<usize>| i.map(|i| ix.vault.notes[i].rel.clone());
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
            .map(|b| ix.vault.notes[b.from].name.as_str())
            .collect();
        assert!(from.contains(&"David A. Johnston"));
        assert!(from.contains(&"Ring of Fire"));
        let johnston = ix
            .backlinks(helens)
            .iter()
            .find(|b| ix.vault.notes[b.from].name == "David A. Johnston")
            .unwrap();
        assert!(johnston.context.contains("[[Mount St. Helens]]"));
        assert!(
            ix.backlinks(ix.by_rel("Inbox/Loose thought").unwrap())
                .is_empty()
        );
    }

    #[test]
    fn refreshes_one_note_at_a_time() {
        use crate::vault::NoteChange;
        let dir = std::env::temp_dir().join(format!("cold-lava-index-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("A.md"), "links to [[B]]").unwrap();
        std::fs::write(dir.join("B.md"), "nothing").unwrap();
        let mut ix = Index::build(crate::vault::scan(&dir).unwrap());
        let root = ix.vault.root.clone();
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
        assert_eq!(ix.vault.notes.len(), 2);
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
