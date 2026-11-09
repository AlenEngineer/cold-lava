//! The link resolver: what a link means, and how to write one. One per workspace, with
//! Obsidian's rules (docs/extensions.md, "Link resolver"). Owners find links (what is written
//! and where); this decides where they lead; the index builds backlinks, the graph, tidy and
//! renames from that.
//!
//! Both directions, which agree: `Files::write` writes a link to a file that `Files::resolve`
//! leads back to it. The rules: `#heading` and `|alias` are split off first (`split_link`); a
//! note by its workspace path, then by its name anywhere, the shallowest winning; other files
//! by path, beside the linking note, then by name; `./` and `../` from the linking file. A link
//! that leads nowhere creates its note where Obsidian would (`new_note_path`).

use crate::index::Index;
use crate::workspace::{Workspace, attachment_kind, decode_target, encode_target_like};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

/// The index's resolver: link targets (`Note`, `folder/Note`, `Note.md`) to notes, by
/// workspace-relative path first, then by file name, preferring the shallowest match. Files
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

/// A link as written in a note: target, optional heading or block (`#Heading`, `#^block`).
pub struct LinkTarget {
    pub note: String,
    pub heading: Option<String>,
    pub block: Option<String>,
}

/// Split `Note#Heading|alias` / `Note#^block` into its parts.
pub fn split_link(raw: &str) -> LinkTarget {
    let raw = raw.split('|').next().unwrap_or(raw).trim();
    let decode = |s: &str| decode_target(s.trim());
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

/// The file an attachment link (`[[map.pdf]]`, `[map](../Maps/map.pdf)`, already decoded)
/// leads to: by workspace path, then next to the note `from`, then by file name anywhere,
/// shallowest first. `None` when it isn't in the workspace, or when `target` names a note.
pub fn attachment(ix: &Index, target: &str, from: Option<usize>) -> Option<PathBuf> {
    attachment_kind(target)?;
    file(ix, target, from)
}

/// The file a link written in note `from` leads to, of any kind, found the way `attachment`
/// finds one.
pub fn file(ix: &Index, target: &str, from: Option<usize>) -> Option<PathBuf> {
    let files = Files::of_workspace(ix);
    let dir = from.map_or("", |i| dir_of(&ix.workspace.notes[i].rel));
    let i = files.resolve(dir, target)?;
    Some(ix.workspace.root.join(&files.rels[i]))
}

/// Where a new note for an unresolved link goes, following Obsidian's
/// "Default location for new notes" setting (`.obsidian/app.json`).
pub fn new_note_path(ix: &Index, name: &str, from: Option<usize>) -> PathBuf {
    new_file_path(ix, name.trim().trim_end_matches(".md"), "md", from)
}

/// Where a new `name.extension` goes: in the folder `name` names, else where Obsidian puts
/// new files, as for notes.
pub fn new_file_path(ix: &Index, name: &str, extension: &str, from: Option<usize>) -> PathBuf {
    let workspace = &ix.workspace;
    // A link with a folder (`[[Ideas/New]]`) says where it goes.
    if name.contains('/') {
        return workspace.root.join(format!("{name}.{extension}"));
    }
    let app: serde_json::Value = std::fs::read_to_string(workspace.root.join(".obsidian/app.json"))
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default();
    let dir = match app.get("newFileLocation").and_then(|v| v.as_str()) {
        Some("current") => from
            .and_then(|i| workspace.notes[i].path.parent().map(Path::to_path_buf))
            .unwrap_or_else(|| workspace.root.clone()),
        Some("folder") => {
            let f = app
                .get("newFileFolderPath")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            workspace.root.join(f.trim_matches('/'))
        }
        _ => workspace.root.clone(),
    };
    dir.join(format!("{name}.{extension}"))
}

pub(crate) fn join(dir: &str, name: &str) -> String {
    if dir.is_empty() {
        name.to_string()
    } else {
        format!("{dir}/{name}")
    }
}

pub(crate) fn dir_of(rel: &str) -> &str {
    rel.rsplit_once('/').map(|(d, _)| d).unwrap_or("")
}

/// Resolve `a/b/../c` style paths (workspace-relative, no leading slash).
fn normalize(path: &str) -> Option<String> {
    let mut out: Vec<&str> = Vec::new();
    for c in path.split('/') {
        match c {
            "" | "." => {}
            ".." => {
                out.pop()?;
            }
            c => out.push(c),
        }
    }
    Some(out.join("/"))
}

/// Path from folder `from_dir` to workspace-relative `to`, e.g. ("A/B", "C/D") → "../../C/D".
fn relative(from_dir: &str, to: &str) -> String {
    let f: Vec<&str> = from_dir.split('/').filter(|s| !s.is_empty()).collect();
    let t: Vec<&str> = to.split('/').filter(|s| !s.is_empty()).collect();
    let common = f.iter().zip(&t).take_while(|(a, b)| a == b).count();
    let mut parts: Vec<&str> = vec![".."; f.len() - common];
    parts.extend(&t[common..]);
    parts.join("/")
}

/// How a link names a file: notes without `.md`, other files with their extension.
pub(crate) fn link_key(rel: &str) -> &str {
    rel.strip_suffix(".md").unwrap_or(rel)
}

/// How a link found its file.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Via {
    Path,
    Name,
}

/// How a link is written, which a link written again keeps.
#[derive(Clone, Copy, Debug)]
pub enum Style<'a> {
    /// `[[Name]]`: by name when no other file has it, else by path (`[[Folder/Name]]`); by
    /// path always when `folder`; from the linking file when `relative` (`[[../Name]]`); a
    /// note with `.md` when `extension`.
    Wiki {
        relative: bool,
        folder: bool,
        extension: bool,
    },
    /// `[text](path)`: from the linking file, or from the workspace's root after `root`
    /// (`""` or `"/"`); percent-encoded like `like`, the link as it was written.
    Markdown {
        root: Option<&'static str>,
        like: &'a str,
    },
}

/// Every file a link can lead to, and how links find them: notes by path or by name (the
/// shallowest wins), `./` and `../` paths, other files (images, PDFs, canvases) by path, next
/// to the linking note, or by name.
pub struct Files {
    /// Workspace-relative, with extension; the index's notes first, in its order.
    pub rels: Vec<String>,
    by_path: HashMap<String, usize>,
    by_name: HashMap<String, usize>,
    /// How many files have each name.
    names: HashMap<String, usize>,
}

impl Files {
    /// The workspace's notes, and its other files read from disk.
    pub fn of_workspace(ix: &Index) -> Files {
        let mut rels: Vec<String> = ix
            .workspace
            .notes
            .iter()
            .map(|n| format!("{}.md", n.rel))
            .collect();
        let root = &ix.workspace.root;
        let walker = WalkDir::new(root).into_iter().filter_entry(|e| {
            let n = e.file_name().to_string_lossy();
            e.depth() == 0 || !(n.starts_with('.') || n == "node_modules")
        });
        for e in walker.filter_map(|e| e.ok()) {
            if e.file_type().is_file() && e.path().extension().is_none_or(|x| x != "md") {
                let rel = e.path().strip_prefix(root).unwrap_or(e.path());
                rels.push(rel.to_string_lossy().replace('\\', "/"));
            }
        }
        Files::new(rels)
    }

    pub(crate) fn new(rels: Vec<String>) -> Files {
        Files::without(rels, None)
    }

    /// The files at `rels`, but links can't find the one at `gone` (it still has an index and a
    /// path, so a link that led to it is written as that path).
    pub(crate) fn without(rels: Vec<String>, gone: Option<usize>) -> Files {
        let mut order: Vec<usize> = (0..rels.len()).filter(|&i| Some(i) != gone).collect();
        order.sort_by(|&a, &b| link_key(&rels[a]).cmp(link_key(&rels[b])));
        let (mut by_path, mut by_name, mut names) = (
            HashMap::new(),
            HashMap::<String, usize>::new(),
            HashMap::new(),
        );
        for i in order {
            let key = link_key(&rels[i]).to_lowercase();
            let name = key.rsplit('/').next().unwrap_or(&key).to_string();
            let depth = key.matches('/').count();
            *names.entry(name.clone()).or_insert(0) += 1;
            let e = by_name.entry(name).or_insert(i);
            if depth < link_key(&rels[*e]).matches('/').count() {
                *e = i;
            }
            by_path.insert(key, i);
        }
        Files {
            rels,
            by_path,
            by_name,
            names,
        }
    }

    /// The file a link written in a file in `dir` leads to (`target` already decoded:
    /// `workspace::decode_target`).
    pub fn resolve(&self, dir: &str, target: &str) -> Option<usize> {
        self.find(dir, target).map(|(i, _)| i)
    }

    /// How to write a link to file `target` from a file in folder `dir`, in `style`.
    pub fn write(&self, target: usize, dir: &str, style: Style) -> String {
        let rel = &self.rels[target];
        // A path to the linking file's own folder is a bare name, which Obsidian looks up from
        // the workspace's root and then by name; `./` keeps it in that folder.
        let from_here = |to: &str| {
            let path = relative(dir, to);
            if path.contains('/') || self.resolve(dir, &path) == Some(target) {
                path
            } else {
                format!("./{path}")
            }
        };
        match style {
            Style::Markdown { root, like } => {
                let path = match root {
                    Some(slash) => format!("{slash}{rel}"),
                    None => from_here(rel),
                };
                encode_target_like(&path, like)
            }
            Style::Wiki {
                relative: relative_link,
                folder,
                extension,
            } => {
                let shown = if extension { rel } else { link_key(rel) };
                let name = shown.rsplit('/').next().unwrap_or(shown);
                if relative_link {
                    from_here(shown)
                } else if folder || !self.unique(link_key(name)) {
                    shown.to_string()
                } else {
                    name.to_string()
                }
            }
        }
    }

    /// The file at a workspace path (notes with or without `.md`).
    pub(crate) fn path(&self, key: &str) -> Option<usize> {
        self.by_path.get(&key.to_lowercase()).copied()
    }

    fn find(&self, dir: &str, target: &str) -> Option<(usize, Via)> {
        let target = target.trim();
        let other = attachment_kind(target).is_some();
        let key = if other { target } else { link_key(target) };
        let by_path = |p: Option<String>| p.and_then(|p| self.path(&p)).map(|i| (i, Via::Path));
        if key.starts_with("./") || key.starts_with("../") {
            return by_path(normalize(&join(dir, key)));
        }
        let beside = other || key.contains('/');
        by_path(Some(key.trim_start_matches('/').to_string()))
            .or_else(|| by_path(normalize(&join(dir, key)).filter(|_| beside)))
            .or_else(|| {
                let name = key.rsplit('/').next().unwrap_or(key).to_lowercase();
                self.by_name.get(&name).map(|&i| (i, Via::Name))
            })
    }

    /// No other file has this name (a note's without `.md`).
    pub(crate) fn unique(&self, name: &str) -> bool {
        self.names.get(&name.to_lowercase()).copied().unwrap_or(0) <= 1
    }
}

/// The workspace before and after a move: same files (same indices), new paths for some.
pub(crate) struct Rewriter {
    pub(crate) before: Files,
    pub(crate) after: Files,
}

impl Rewriter {
    /// What the link `raw` should become, or `None` if it still leads where it did. `dir_then`
    /// is the linking file's folder before the move, `dir_now` after.
    pub(crate) fn link(
        &self,
        raw: &str,
        md: bool,
        dir_then: &str,
        dir_now: &str,
    ) -> Option<String> {
        let decoded = decode_target(raw.trim());
        let (target, via) = self.before.find(dir_then, &decoded)?;
        let still = self.after.find(dir_now, &decoded);
        if still.is_some_and(|(t, v)| t == target && !(via == Via::Path && v == Via::Name)) {
            return None;
        }
        let style = if md {
            // Workspace-absolute ("Folder/Note.md", "/Folder/Note.md") or relative to the note?
            let root = if decoded.starts_with('/') {
                Some("/")
            } else {
                (!decoded.starts_with('.')
                    && self.before.path(link_key(&decoded)) == Some(target)
                    && !dir_then.is_empty())
                .then_some("")
            };
            Style::Markdown { root, like: raw }
        } else {
            Style::Wiki {
                relative: decoded.starts_with("./") || decoded.starts_with("../"),
                folder: decoded.contains('/'),
                extension: decoded.ends_with(".md"),
            }
        };
        let out = self.after.write(target, dir_now, style);
        (out != raw).then_some(out)
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

    fn fixture(name: &str, files: &[&str]) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("cl-resolver-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        for f in files {
            let p = dir.join(f);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, "").unwrap();
        }
        dir
    }

    #[test]
    fn finds_attachments_by_path_beside_the_note_and_by_name() {
        let dir = fixture(
            "attach",
            &[
                "workspace/Maps/deep/map.pdf",
                "workspace/Notes/Trip.md",
                "workspace/Notes/ticket.pdf",
                "workspace/Mount St. Helens.md",
                "outside.pdf",
            ],
        );
        let ix = Index::build(crate::workspace::scan(&dir.join("workspace")).unwrap());
        let root = ix.workspace.root.clone();
        let trip = ix.by_rel("Notes/Trip");
        let found =
            |t: &str| attachment(&ix, t, trip).map(|p| p.strip_prefix(&root).unwrap().to_owned());
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

    #[test]
    fn paths_between_folders() {
        assert_eq!(relative("A/B", "C/D"), "../../C/D");
        assert_eq!(relative("A", "A/x"), "x");
        assert_eq!(normalize("A/B/../C"), Some("A/C".into()));
    }

    /// Every way a link can be written, and whether the index's `Resolver` (which doesn't know
    /// the linking file's folder) reads it the same way.
    const STYLES: [(Style, bool); 7] = [
        (
            Style::Wiki {
                relative: false,
                folder: false,
                extension: false,
            },
            true,
        ),
        (
            Style::Wiki {
                relative: false,
                folder: true,
                extension: false,
            },
            true,
        ),
        (
            Style::Wiki {
                relative: false,
                folder: true,
                extension: true,
            },
            true,
        ),
        (
            Style::Wiki {
                relative: true,
                folder: false,
                extension: false,
            },
            false,
        ),
        (
            Style::Markdown {
                root: None,
                like: "",
            },
            false,
        ),
        (
            Style::Markdown {
                root: Some(""),
                like: "",
            },
            true,
        ),
        (
            Style::Markdown {
                root: Some("/"),
                like: "%C4%81",
            },
            true,
        ),
    ];

    /// Links written to every file from every note, in every style, that don't lead back to it.
    fn written_links_that_lead_elsewhere(ix: &Index) -> Vec<String> {
        let files = Files::of_workspace(ix);
        let notes = ix.workspace.notes.len();
        let mut wrong = Vec::new();
        for from in 0..notes {
            let dir = dir_of(&files.rels[from]);
            for to in 0..files.rels.len() {
                for (style, index_reads_it) in STYLES {
                    let link = files.write(to, dir, style);
                    let target = decode_target(&link);
                    let back = files.resolve(dir, &target);
                    let indexed =
                        (to < notes && index_reads_it).then(|| ix.resolver.resolve(&target));
                    if back != Some(to) || indexed.is_some_and(|i| i != Some(to)) {
                        wrong.push(format!(
                            "{} → {} as {style:?}: {link:?} leads to {:?} (index: {:?})",
                            files.rels[from],
                            files.rels[to],
                            back.map(|i| &files.rels[i]),
                            indexed.flatten().map(|i| &files.rels[i]),
                        ));
                    }
                }
            }
        }
        wrong
    }

    #[test]
    fn a_written_link_resolves_back_to_its_file() {
        let ix = example();
        assert!(ix.workspace.notes.len() > 20);
        assert_eq!(written_links_that_lead_elsewhere(&ix), Vec::<String>::new());

        let dir = fixture(
            "round-trip",
            &[
                "Top.md",
                "A/Top.md",
                "B/Top.md",
                "A/Deep/Top.md",
                "A/Note.md",
                "B/Note.md",
                "A/map.png",
                "B/map.png",
                "Mount St. Helens.md",
                "Lava/Pāhoehoe flow.md",
                "Lava/flow map.png",
                "Board.canvas",
                "Lava/Eruptions.csv",
            ],
        );
        let ix = Index::build(crate::workspace::scan(&dir).unwrap());
        assert_eq!(written_links_that_lead_elsewhere(&ix), Vec::<String>::new());
        let _ = std::fs::remove_dir_all(dir);
    }
}
