//! Tidying a workspace: what nothing uses. Notes with no links in or out (Obsidian's graph
//! "orphans"), attachments no note or canvas links to, folders with no files, and links to
//! notes that don't exist. Read-only: deleting is the caller's, through `trash`.
//!
//! Every link is resolved by `resolver::Files`, the way Obsidian resolves it (percent-encoded
//! Markdown links included), from notes, canvas cards and canvas text.

use crate::index::Index;
use crate::owner::markdown::link_targets;
use crate::rename::canvas_targets;
use crate::resolver::{Files, dir_of};
use crate::workspace::{attachment_kind, decode_target};
use serde::Serialize;
use std::collections::{BTreeMap, HashSet};
use std::path::Path;

#[derive(Serialize, Clone, Debug, Default, PartialEq)]
pub struct Report {
    /// Notes nothing links to, sorted by path, daily notes last.
    pub orphans: Vec<Orphan>,
    /// Images, audio, video and PDFs nothing links to (workspace-relative, with extension).
    pub unused_attachments: Vec<String>,
    /// Folders with no files at any depth, the outermost only.
    pub empty_folders: Vec<String>,
    /// Links to notes that don't exist, by target.
    pub unresolved: Vec<Unresolved>,
}

#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct Orphan {
    /// Workspace-relative, with `.md`.
    pub note: String,
    /// Files it links to: 0 makes it an orphan both ways, more only "no backlinks".
    pub links_out: usize,
    /// A daily note (in the daily-notes folder, named by its date format): often linked by
    /// nothing on purpose.
    pub daily: bool,
}

#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct Unresolved {
    /// The target as first written (`Lahar`, `Ideas/New`).
    pub target: String,
    pub from: Vec<LinkAt>,
}

#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct LinkAt {
    /// The linking note, workspace-relative with `.md`.
    pub note: String,
    /// 1-based.
    pub line: usize,
    pub context: String,
}

impl Orphan {
    pub fn both_ways(&self) -> bool {
        self.links_out == 0
    }
}

/// Folders a workspace's settings name: templates, daily notes, attachments, new notes. They are
/// never "empty folders to remove", and their notes are not orphans (templates) or are grouped
/// (daily notes).
struct Settings {
    templates: crate::templates::Settings,
    daily: crate::daily::Settings,
    folders: Vec<String>,
}

impl Settings {
    fn load(root: &Path) -> Settings {
        let templates = crate::templates::Settings::load(root);
        let daily = crate::daily::Settings::load(root);
        let app: serde_json::Value = std::fs::read_to_string(root.join(".obsidian/app.json"))
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default();
        let setting = |k: &str| {
            let v = app.get(k).and_then(|v| v.as_str()).unwrap_or("").trim();
            // `./x` is relative to each note, not one folder.
            (!v.starts_with("./")).then(|| v.trim_matches('/').to_string())
        };
        let folders = (templates.folders.iter().cloned())
            .chain([daily.folder.clone()])
            .chain(setting("attachmentFolderPath"))
            .chain(setting("newFileFolderPath"))
            .filter(|f| !f.is_empty())
            .collect();
        Settings {
            templates,
            daily,
            folders,
        }
    }

    /// Is the folder `rel` one the settings name, or does it hold one?
    fn keeps(&self, rel: &str) -> bool {
        self.folders
            .iter()
            .any(|f| f == rel || f.strip_prefix(rel).is_some_and(|r| r.starts_with('/')))
    }
}

pub fn report(ix: &Index) -> Report {
    let root = &ix.workspace.root;
    let settings = Settings::load(root);
    let files = Files::of_workspace(ix);
    let notes = ix.workspace.notes.len();
    // used[f]: something other than f links to it; out[n]: what note n links to.
    let mut used = vec![false; files.rels.len()];
    let mut out = vec![0; notes];
    let mut found = |from: usize, dir: &str, raw: &str, used: &mut Vec<bool>| {
        let to = files.resolve(dir, &decode_target(raw.trim()));
        if let Some(to) = to.filter(|&to| to != from) {
            used[to] = true;
            if from < notes {
                out[from] += 1;
            }
        }
    };
    for (i, n) in ix.workspace.notes.iter().enumerate() {
        for raw in link_targets(&n.text) {
            found(i, dir_of(&files.rels[i]), raw, &mut used);
        }
    }
    for owned in &ix.workspace.owned {
        let from = files.path(&owned.rel).unwrap_or(usize::MAX);
        for (r, _) in &owned.links {
            found(from, dir_of(&owned.rel), &r.target, &mut used);
        }
    }
    for (i, rel) in files.rels.iter().enumerate().skip(notes) {
        if attachment_kind(rel) != Some("whiteboard") {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(root.join(rel)) else {
            continue;
        };
        let (cards, links) = canvas_targets(&text);
        for card in cards {
            if let Some(to) = files.path(card.strip_suffix(".md").unwrap_or(&card)) {
                used[to] = true;
            }
        }
        for raw in &links {
            found(i, dir_of(rel), raw, &mut used);
        }
    }

    let mut orphans: Vec<Orphan> = (0..notes)
        .filter(|&i| !used[i] && !settings.templates.is_template(&files.rels[i]))
        .map(|i| Orphan {
            note: files.rels[i].clone(),
            links_out: out[i],
            daily: settings.daily.date_of(&files.rels[i]).is_some(),
        })
        .collect();
    orphans.sort_by(|a, b| (a.daily, &a.note).cmp(&(b.daily, &b.note)));

    let mut unused_attachments: Vec<String> = (notes..files.rels.len())
        .filter(|&i| !used[i])
        .map(|i| &files.rels[i])
        .filter(|rel| {
            matches!(
                attachment_kind(rel),
                Some("image" | "audio" | "video" | "document")
            )
        })
        .filter(|rel| !settings.templates.is_template(rel))
        .cloned()
        .collect();
    unused_attachments.sort();

    let mut groups: BTreeMap<String, Unresolved> = BTreeMap::new();
    for (i, n) in ix.workspace.notes.iter().enumerate() {
        if settings.templates.is_template(&files.rels[i]) {
            continue;
        }
        let dir = dir_of(&n.rel);
        // A link to a file that isn't a note (`[[Eruptions.csv]]`) leads somewhere too.
        let nowhere = |r: &&crate::workspace::Ref| {
            ix.resolver.resolve(&r.target).is_none() && files.resolve(dir, &r.target).is_none()
        };
        for r in n.refs.iter().filter(nowhere) {
            let g = groups
                .entry(r.target.to_lowercase())
                .or_insert_with(|| Unresolved {
                    target: r.target.clone(),
                    from: Vec::new(),
                });
            g.from.push(LinkAt {
                note: files.rels[i].clone(),
                line: r.line,
                context: r.context.clone(),
            });
        }
    }

    for owned in &ix.workspace.owned {
        let dir = dir_of(&owned.rel);
        let nowhere = |r: &&crate::workspace::Ref| files.resolve(dir, &r.target).is_none();
        for (r, _) in owned.links.iter().filter(|(r, _)| nowhere(&r)) {
            let g = groups
                .entry(r.target.to_lowercase())
                .or_insert_with(|| Unresolved {
                    target: r.target.clone(),
                    from: Vec::new(),
                });
            g.from.push(LinkAt {
                note: owned.rel.clone(),
                line: r.line,
                context: r.context.clone(),
            });
        }
    }

    Report {
        orphans,
        unused_attachments,
        empty_folders: empty_folders(root, &settings),
        unresolved: groups.into_values().collect(),
    }
}

/// Folders (not hidden, not one the settings name or one holding it) with no files at any
/// depth, the outermost of each nest only. A hidden file or folder inside counts as content.
fn empty_folders(root: &Path, settings: &Settings) -> Vec<String> {
    let rel = |p: &Path| {
        let r = p.strip_prefix(root).unwrap_or(p);
        r.to_string_lossy().replace('\\', "/")
    };
    let mut full: HashSet<String> = HashSet::new();
    let mut folders = Vec::new();
    let mut walker = walkdir::WalkDir::new(root).sort_by_file_name().into_iter();
    while let Some(Ok(e)) = walker.next() {
        let name = e.file_name().to_string_lossy();
        let hidden = e.depth() > 0 && (name.starts_with('.') || name == "node_modules");
        if hidden && e.file_type().is_dir() {
            walker.skip_current_dir();
        }
        if e.file_type().is_dir() && !hidden {
            folders.push(rel(e.path()));
        } else {
            let mut a = e.path().parent();
            while let Some(dir) = a.filter(|d| d.starts_with(root)) {
                if !full.insert(rel(dir)) {
                    break;
                }
                a = dir.parent();
            }
        }
    }
    let empty = |f: &str| !f.is_empty() && !full.contains(f) && !settings.keeps(f);
    folders
        .iter()
        .filter(|f| empty(f) && !empty(dir_of(f)))
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn workspace(name: &str, files: &[(&str, &str)]) -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("cold-lava-tidy-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        for (rel, text) in files {
            let p = dir.join(rel);
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            fs::write(p, text).unwrap();
        }
        dir
    }

    fn fixture(name: &str) -> (std::path::PathBuf, Index) {
        let canvas = r#"{"nodes":[{"id":"a","type":"file","file":"Boards/On a board.md"},{"id":"b","type":"text","text":"![[board.png]]"},{"id":"c","type":"file","file":"img/card.jpg"}],"edges":[]}"#;
        let dir = workspace(
            name,
            &[
                (".obsidian/daily-notes.json", r#"{"folder":"Journal"}"#),
                (".obsidian/templates.json", r#"{"folder":"Templates"}"#),
                (".obsidian/app.json", r#"{"attachmentFolderPath":"Assets"}"#),
                (
                    "Hub.md",
                    "[[Linked]] ![[img/used.png]] [m](img/P%C4%81hoehoe%20map.pdf) [[Missing]] [[missing#x]]\n`[[InCode]] ![[img/in-code.png]]`",
                ),
                ("Linked.md", "back to [[Hub]]"),
                ("Lonely.md", "no links"),
                ("Talker.md", "[[Hub]] and [[Ideas/New]]"),
                ("Self.md", "[[Self]]"),
                ("Boards/On a board.md", "x"),
                ("Boards/Board.canvas", canvas),
                ("Journal/2026-11-05.md", "a day"),
                ("Journal/Not a date.md", "x"),
                ("Templates/Daily.md", "[[{{date}}]] ![[img/logo.png]]"),
                ("Templates/banner.png", ""),
                ("img/used.png", ""),
                ("img/logo.png", ""),
                ("img/board.png", ""),
                ("img/card.jpg", ""),
                ("img/Pāhoehoe map.pdf", ""),
                ("img/unused.png", ""),
                ("img/in-code.png", ""),
                ("img/clip.mp3", ""),
                ("Other.canvas", r#"{"nodes":[]}"#),
                ("Empty/Deeper/.keep/x", ""),
                ("Hollow/Inside/Further/a.png", ""),
            ],
        );
        for d in [
            "Bare/Inner",
            "Hollow/Nothing",
            "Assets",
            ".hidden/empty",
            "Kept/Leaf",
        ] {
            fs::create_dir_all(dir.join(d)).unwrap();
        }
        fs::write(dir.join("Kept/note.md"), "[[Hub]]").unwrap();
        let ix = Index::build(crate::workspace::scan(&dir).unwrap());
        (dir, ix)
    }

    #[test]
    fn orphans_are_notes_nothing_links_to() {
        let (dir, ix) = fixture("orphans");
        let r = report(&ix);
        let orphans: Vec<(&str, usize, bool)> = (r.orphans.iter())
            .map(|o| (o.note.as_str(), o.links_out, o.daily))
            .collect();
        assert_eq!(
            orphans,
            [
                ("Journal/Not a date.md", 0, false),
                ("Kept/note.md", 1, false),
                ("Lonely.md", 0, false),
                ("Self.md", 0, false),
                ("Talker.md", 1, false),
                ("Journal/2026-11-05.md", 0, true),
            ],
            "a link to itself isn't a link; a canvas card is; templates are never orphans"
        );
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn unused_attachments_are_what_nothing_links_to() {
        let (dir, ix) = fixture("unused");
        assert_eq!(
            report(&ix).unused_attachments,
            [
                "Hollow/Inside/Further/a.png",
                "img/clip.mp3",
                "img/in-code.png",
                "img/unused.png"
            ],
            "linked from a note, a percent-encoded link, a template, a canvas card or canvas \
             text counts; a canvas is not an attachment; a link in code doesn't count"
        );
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn empty_folders_are_the_outermost_without_files() {
        let (dir, ix) = fixture("empty");
        assert_eq!(
            report(&ix).empty_folders,
            ["Bare", "Hollow/Nothing", "Kept/Leaf"],
            "a hidden file is content; Assets is the attachment folder; hidden folders aren't listed"
        );
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn unresolved_links_are_grouped_by_target() {
        let (dir, ix) = fixture("unresolved");
        let r = report(&ix);
        let got: Vec<(&str, Vec<(&str, usize)>)> = (r.unresolved.iter())
            .map(|u| {
                let from = u.from.iter().map(|l| (l.note.as_str(), l.line)).collect();
                (u.target.as_str(), from)
            })
            .collect();
        assert_eq!(
            got,
            [
                ("Ideas/New", vec![("Talker.md", 1)]),
                ("Missing", vec![("Hub.md", 1), ("Hub.md", 1)]),
            ],
            "templates' placeholders aren't listed"
        );
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn the_example_workspace() {
        let ix = Index::build(
            crate::workspace::scan(
                &Path::new(env!("CARGO_MANIFEST_DIR")).join("../../example-workspace"),
            )
            .unwrap(),
        );
        let r = report(&ix);
        assert!(
            r.orphans
                .iter()
                .any(|o| o.note == "Inbox/Loose thought.md" && o.both_ways())
        );
        assert!(r.unresolved.iter().any(|u| u.target == "Lahar"));
    }
}
