//! Renaming and moving notes and folders with every link to them updated, like Obsidian's
//! "Automatically update internal links".
//!
//! `plan` (a note), `plan_folder` (a folder and everything in it) and `plan_into` (several of
//! them into one folder) work out every change without touching anything; `apply` checks nothing
//! changed since, then makes the moves and rewrites the other files atomically, one by one.
//!
//! The rule for links: every link in the workspace is resolved as the workspace is now and as it will
//! be after the move. A link that still leads to the same file, the same way, is left alone;
//! any other is rewritten in the style it was written in. So `[[Lava]]` survives a folder move,
//! but becomes a path when the move makes another `Lava` the shallowest one.

use crate::bookmarks::{self, Bookmarks};
use crate::index::Index;
use crate::owner::canvas;
use crate::owner::given::{Change as Ask, Given, GivenItem};
use crate::owner::markdown;
use crate::resolver::{Files, Rewriter, dir_of, join, link_key};
use anyhow::{Context, Result, bail};
use serde::Serialize;
use std::path::{Path, PathBuf};

#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct Change {
    /// 1-based line.
    pub line: usize,
    pub before: String,
    pub after: String,
}

#[derive(Serialize, Clone, Debug)]
pub struct FileEdit {
    /// Workspace-relative path of the file as it is now.
    pub file: String,
    #[serde(skip)]
    pub path: PathBuf,
    #[serde(skip)]
    pub old_text: String,
    #[serde(skip)]
    pub new_text: String,
    /// Links rewritten in this file (a recipe: what it changed there, such as tags renamed).
    pub links: usize,
    pub changes: Vec<Change>,
    /// For a file another owner reads, what to ask it. `new_text` is the owner's answer, empty
    /// until `Core::relink` has it; nothing moves before then.
    #[serde(skip)]
    pub owned: Option<Through>,
}

/// The changes to ask the owner of a file for (`file/edit`), what it gave before them, and what
/// it gave for its new text once that read back as asked.
#[derive(Clone, Debug)]
pub struct Through {
    pub owner: String,
    pub changes: Vec<Ask>,
    pub given: Given,
    pub after: Option<Given>,
}

/// Put the owners' answers (`Core::take_relinked`) into `edits`, by file.
pub fn answered(edits: &mut [FileEdit], answers: Vec<FileEdit>) {
    for a in answers {
        if let Some(e) = edits.iter_mut().find(|e| e.file == a.file) {
            *e = a;
        }
    }
}

/// Refuse a plan whose files other owners read have no new text from their owners yet.
pub(crate) fn check_answered<'a>(edits: impl IntoIterator<Item = &'a FileEdit>) -> Result<()> {
    for e in edits {
        if let Some(o) = e.owned.as_ref().filter(|o| o.after.is_none()) {
            bail!(
                "{} hasn't given the new text of {} yet; nothing was changed",
                o.owner,
                e.file
            );
        }
    }
    Ok(())
}

/// One note, other file or folder that moves.
#[derive(Serialize, Clone, Debug)]
pub struct Move {
    /// Workspace-relative: a file with its extension, or a folder.
    pub from: String,
    pub to: String,
    pub folder: bool,
    #[serde(skip)]
    pub from_path: PathBuf,
    #[serde(skip)]
    pub to_path: PathBuf,
}

impl Move {
    /// Where the file at workspace-relative `rel` is after this move, if it moves with it.
    fn moved_rel(&self, rel: &str) -> Option<String> {
        if !self.folder {
            return (rel == self.from).then(|| self.to.clone());
        }
        let rest = rel.strip_prefix(&self.from)?.strip_prefix('/')?;
        Some(format!("{}/{rest}", self.to))
    }

    /// Where a file at `path` is after this move, if it moves with it.
    fn moved(&self, path: &Path) -> Option<PathBuf> {
        let rest = path.strip_prefix(&self.from_path).ok()?;
        Some(match rest.as_os_str().is_empty() {
            true => self.to_path.clone(),
            false => self.to_path.join(rest),
        })
    }
}

/// Everything moving some notes and folders at once changes. Every link is resolved once
/// against the workspace before all the moves and after all of them, so a link between two moving
/// notes is right, and each changed file is listed once.
#[derive(Serialize, Clone, Debug)]
pub struct Plan {
    pub moves: Vec<Move>,
    /// The files that move (workspace-relative, as they are now): the notes, and what the folders hold.
    pub moving: Vec<String>,
    /// Files whose links change, moving ones included (under their path as it is now).
    pub edits: Vec<FileEdit>,
    /// Obsidian's bookmarks file, when bookmarks point at what moves (`links` counts them).
    pub bookmarks: Option<FileEdit>,
    pub warnings: Vec<String>,
}

impl Plan {
    pub fn links(&self) -> usize {
        self.edits.iter().map(|e| e.links).sum()
    }

    pub fn files(&self) -> usize {
        self.edits.len()
    }

    /// Each moving file's path now and after the move.
    pub fn file_moves(&self) -> Vec<(PathBuf, PathBuf)> {
        let mut out = Vec::new();
        for m in &self.moves {
            if !m.folder {
                out.push((m.from_path.clone(), m.to_path.clone()));
                continue;
            }
            let prefix = format!("{}/", m.from);
            out.extend(
                (self.moving.iter())
                    .filter_map(|r| r.strip_prefix(&prefix))
                    .map(|rest| (m.from_path.join(rest), m.to_path.join(rest))),
            );
        }
        out
    }

    /// Where a file at `path` is after the moves (itself if it doesn't move).
    pub fn moved(&self, path: &Path) -> PathBuf {
        (self.moves.iter())
            .find_map(|m| m.moved(path))
            .unwrap_or_else(|| path.to_path_buf())
    }

    /// What moves, for a message: the one item's path, or `3 items`.
    fn what(&self) -> String {
        match self.moves.as_slice() {
            [m] => m.from.clone(),
            ms => format!("{} items", ms.len()),
        }
    }
}

/// Where `new` puts the note at `old_rel` (both workspace-relative, `.md` included in the result):
/// a bare name renames it in its folder, `folder/` moves it keeping its name, anything else is
/// a full workspace path.
pub fn target_rel(old_rel: &str, new: &str) -> Result<String> {
    let new = new.trim();
    let new = new.strip_suffix(".md").unwrap_or(new);
    Ok(format!(
        "{}.md",
        target_path(old_rel.trim_end_matches(".md"), new)?
    ))
}

/// The same for the folder at `old_rel`: a bare name renames it, `folder/` moves it into that
/// folder, anything else is a full workspace path.
pub fn folder_target(old_rel: &str, new: &str) -> Result<String> {
    target_path(old_rel.trim_matches('/'), new.trim())
}

fn target_path(old_rel: &str, new: &str) -> Result<String> {
    let new = new.replace('\\', "/");
    let new = new.trim_start_matches('/');
    let (old_dir, old_name) = old_rel.rsplit_once('/').unwrap_or(("", old_rel));
    let rel = if new.is_empty() {
        bail!("the new name is empty");
    } else if let Some(dir) = new.strip_suffix('/') {
        join(dir.trim_matches('/'), old_name)
    } else if new.contains('/') {
        new.to_string()
    } else {
        join(old_dir, new)
    };
    check_path(&rel)?;
    Ok(rel)
}

/// A path a note or folder can have: inside the workspace, not hidden, and one links can name.
pub fn check_path(rel: &str) -> Result<()> {
    if rel
        .split('/')
        .any(|c| c.is_empty() || c == "." || c == ".." || c.starts_with('.'))
    {
        bail!("not a valid path inside the workspace: {rel}");
    }
    if rel
        .chars()
        .any(|c| matches!(c, '[' | ']' | '|' | '#' | '^'))
    {
        bail!("names can't contain [ ] | # or ^ (links couldn't point to them)");
    }
    Ok(())
}

impl Rewriter {
    /// Rewrite the links in a note's text (through its owner, `markdown::relink`). Returns the
    /// new text, changed lines and link count.
    fn note_text(&self, text: &str, dir_then: &str, dir_now: &str) -> (String, Vec<Change>, usize) {
        markdown::relink(text, |raw, md| self.link(raw, md, dir_then, dir_now))
    }
}

/// Work out everything renaming/moving note `note` to workspace-relative `new_rel` (with `.md`)
/// would change. Nothing is written.
pub fn plan(ix: &Index, note: usize, new_rel: &str) -> Result<Plan> {
    let n = &ix.workspace.notes[note];
    let from_rel = format!("{}.md", n.rel);
    let new_rel = if new_rel.ends_with(".md") {
        new_rel.to_string()
    } else {
        format!("{new_rel}.md")
    };
    if new_rel == from_rel {
        bail!("that's already its name");
    }
    let to_path = ix.workspace.root.join(&new_rel);
    resolve(
        ix,
        vec![Move {
            from: from_rel,
            to: new_rel,
            folder: false,
            from_path: n.path.clone(),
            to_path,
        }],
    )
}

/// Work out everything moving the folder `from` (workspace-relative) to `to` would change: every
/// file in it moves, and every link that would lead elsewhere afterwards is rewritten.
/// Nothing is written.
pub fn plan_folder(ix: &Index, from: &str, to: &str) -> Result<Plan> {
    let (from, to) = (from.trim_matches('/'), to.trim_matches('/'));
    check_folder(ix, from)?;
    check_path(to)?;
    if to == from {
        bail!("that's already its name");
    }
    let root = &ix.workspace.root;
    resolve(
        ix,
        vec![Move {
            from: from.to_string(),
            to: to.to_string(),
            folder: true,
            from_path: root.join(from),
            to_path: root.join(to),
        }],
    )
}

/// Work out everything moving several notes, other files and folders (workspace-relative: files with
/// their extension) into `folder` (`""` is the workspace's root) would change, as one plan. Items
/// already in it stay. Nothing is written.
pub fn plan_into(ix: &Index, items: &[String], folder: &str) -> Result<Plan> {
    let folder = folder.trim_matches('/');
    if !folder.is_empty() {
        check_path(folder)?;
    }
    let root = &ix.workspace.root;
    let mut moves = Vec::new();
    for rel in items {
        let rel = rel.trim_matches('/');
        let from_path = root.join(rel);
        let is_dir = from_path.is_dir();
        if is_dir {
            check_folder(ix, rel)?;
        } else if !from_path.is_file() {
            bail!("{rel} is no longer there");
        }
        let name = rel.rsplit('/').next().unwrap_or(rel);
        let to = join(folder, name);
        if to == rel {
            continue;
        }
        moves.push(Move {
            from: rel.to_string(),
            folder: is_dir,
            from_path,
            to_path: root.join(&to),
            to,
        });
    }
    if moves.is_empty() {
        bail!(
            "they're already in {}",
            if folder.is_empty() {
                "the workspace's root"
            } else {
                folder
            }
        );
    }
    resolve(ix, moves)
}

/// A folder cold-lava may move.
fn check_folder(ix: &Index, from: &str) -> Result<()> {
    if from.is_empty() {
        bail!("that's the workspace itself");
    }
    if from.split('/').any(|c| c.starts_with('.')) {
        bail!("{from} is a hidden folder; cold-lava leaves it alone");
    }
    if !ix.workspace.root.join(from).is_dir() {
        bail!("there's no folder {from}");
    }
    Ok(())
}

/// The plan for `moves`: refused if two land on the same path, one is inside a moving folder or
/// lands in one, or a target exists; otherwise every link resolved before and after them all.
fn resolve(ix: &Index, moves: Vec<Move>) -> Result<Plan> {
    let lower = |s: &str| s.to_lowercase();
    let inside = |rel: &str, folder: &Move| {
        folder.folder && lower(rel).starts_with(&format!("{}/", lower(&folder.from)))
    };
    for (i, m) in moves.iter().enumerate() {
        if let Some(other) = moves[..i].iter().find(|o| lower(&o.to) == lower(&m.to)) {
            bail!("{} and {} would both be {}", other.from, m.from, m.to);
        }
        if let Some(f) = moves.iter().find(|f| inside(&m.from, f)) {
            bail!("{} is inside {}, which moves too", m.from, f.from);
        }
        if let Some(f) = moves.iter().find(|f| inside(&m.to, f)) {
            match std::ptr::eq(f, m) {
                true => bail!("a folder can't move into itself"),
                false => bail!("{} can't move into {}: it moves too", m.from, f.from),
            }
        }
        // A case-only rename of the same file is fine; anything else that exists isn't.
        if m.to_path.exists() && lower(&m.to) != lower(&m.from) {
            bail!("{} already exists", m.to);
        }
    }
    let before = Files::of_workspace(ix);
    let mut moving = Vec::new();
    let rels = (before.rels.iter())
        .map(|r| match moves.iter().find_map(|m| m.moved_rel(r)) {
            Some(to) => {
                moving.push(r.clone());
                to
            }
            None => r.clone(),
        })
        .collect();
    moving.sort();
    let rw = Rewriter {
        before,
        after: Files::new(rels),
    };
    let mut warnings = Vec::new();
    for m in moves.iter().filter(|m| !m.folder) {
        let name = link_key(&m.to).rsplit('/').next().unwrap_or("");
        if !rw.after.unique(name) {
            warnings.push(format!(
                "another note is also called \"{name}\": links to this one are written as full paths, \
                 and existing [[{name}]] links may now point to whichever is closer to the workspace root"
            ));
        }
    }
    let mut edits = edits(ix, &rw)?;
    edits.extend(owned_edits(ix, &rw, &mut warnings));
    let bookmarks = match bookmarks_edit(&ix.workspace.root, &moves) {
        Ok(b) => b,
        Err(e) => {
            warnings.push(format!("bookmarks aren't updated: {e}"));
            None
        }
    };
    Ok(Plan {
        moves,
        moving,
        edits,
        bookmarks,
        warnings,
    })
}

/// Obsidian's bookmarks of what moves, pointed at where it goes (Obsidian does the same).
pub(crate) fn bookmarks_edit(root: &Path, moves: &[Move]) -> Result<Option<FileEdit>> {
    let mut b = Bookmarks::read(root)?;
    let Some(old_text) = b.read_text().map(str::to_string) else {
        return Ok(None);
    };
    let changed = b.moved(|p| {
        (moves.iter()).find_map(|m| match m.folder && p == m.from {
            true => Some(m.to.clone()),
            false => m.moved_rel(p),
        })
    });
    if changed.is_empty() {
        return Ok(None);
    }
    Ok(Some(FileEdit {
        file: bookmarks::FILE.to_string(),
        path: root.join(bookmarks::FILE),
        old_text,
        new_text: b.text(),
        links: changed.len(),
        changes: (changed.into_iter())
            .map(|(before, after)| Change {
                line: 0,
                before,
                after,
            })
            .collect(),
        owned: None,
    }))
}

/// Every note and canvas whose links change.
pub(crate) fn edits(ix: &Index, rw: &Rewriter) -> Result<Vec<FileEdit>> {
    let mut edits = Vec::new();
    for (i, n) in ix.workspace.notes.iter().enumerate() {
        let (dir_then, dir_now) = (dir_of(&rw.before.rels[i]), dir_of(&rw.after.rels[i]));
        let (new_text, changes, links) = rw.note_text(&n.text, dir_then, dir_now);
        if links > 0 {
            edits.push(FileEdit {
                file: rw.before.rels[i].clone(),
                path: n.path.clone(),
                old_text: n.text.clone(),
                new_text,
                links,
                changes,
                owned: None,
            });
        }
    }
    for i in 0..rw.before.rels.len() {
        if rw.before.rels[i].ends_with(".canvas") {
            edits.extend(canvas_edit(ix, rw, i)?);
        }
    }
    Ok(edits)
}

/// An Obsidian Canvas file, through its owner's relink: file cards pointing at moved files take
/// their new paths, and links in text cards are rewritten as a note's are. Edited in place, so
/// the rest of the file stays exactly as Obsidian wrote it.
fn canvas_edit(ix: &Index, rw: &Rewriter, i: usize) -> Result<Option<FileEdit>> {
    let rel = &rw.before.rels[i];
    let path = ix.workspace.root.join(rel);
    let Ok(text) = std::fs::read_to_string(&path) else {
        return Ok(None);
    };
    let (dir_then, dir_now) = (dir_of(rel), dir_of(&rw.after.rels[i]));
    let file = |value: &str| {
        let f = rw.before.path(link_key(value))?;
        Some(rw.after.rels[f].clone())
    };
    let piece = |value: &str| {
        let (text, _, n) = rw.note_text(value, dir_then, dir_now);
        (n > 0).then_some((text, n))
    };
    let Some((new_text, changed, links)) = canvas::relink(&text, file, piece) else {
        return Ok(None);
    };
    Ok(Some(FileEdit {
        file: rel.clone(),
        path,
        old_text: text,
        new_text,
        links,
        changes: (changed.into_iter())
            .map(|c| Change {
                line: c.line,
                before: c.before,
                after: c.after,
            })
            .collect(),
        owned: None,
    }))
}

/// Every file another owner reads whose links change: each Markdown piece's links rewritten as
/// a note's are, and each link the owner reported on its own. Nothing is made here; the owner
/// makes the file's new text before anything moves (`Core::relink`). A file its owner hasn't
/// read as it is now is left out, with a warning.
pub(crate) fn owned_edits(ix: &Index, rw: &Rewriter, warnings: &mut Vec<String>) -> Vec<FileEdit> {
    let mut out = Vec::new();
    // A canvas is relinked with the notes (`canvas_edit`): its cards hold paths, not links.
    for f in (ix.workspace.owned.iter()).filter(|f| crate::owner::built_in(&f.rel).is_none()) {
        let Some(i) = rw.before.rels.iter().position(|r| *r == f.rel) else {
            continue;
        };
        let (dir_then, dir_now) = (dir_of(&rw.before.rels[i]), dir_of(&rw.after.rels[i]));
        let (mut asks, mut changes, mut links) = (Vec::new(), Vec::new(), 0);
        for item in &f.given.items {
            let GivenItem::Markdown { place, text } = item else {
                continue;
            };
            let (text, lines, n) = rw.note_text(text, dir_then, dir_now);
            if n > 0 {
                links += n;
                changes.extend(lines.into_iter().map(|c| Change { line: 0, ..c }));
                asks.push(Ask::Text {
                    place: place.clone(),
                    text,
                });
            }
        }
        for l in &f.given.links {
            if let Some(to) = rw.link(&l.target, false, dir_then, dir_now) {
                links += 1;
                changes.push(Change {
                    line: 0,
                    before: format!("[[{}]]", l.target),
                    after: format!("[[{to}]]"),
                });
                asks.push(Ask::Link {
                    place: l.place.clone(),
                    link: l.target.clone(),
                    to,
                });
            }
        }
        if links == 0 {
            continue;
        }
        let edit = owned_edit(f, asks, changes, links);
        out.extend(
            edit.map_err(|why| warnings.push(format!("{why}, so its links there aren't updated"))),
        );
    }
    out
}

/// The edit of a file another owner reads that asks it for `asks`, its new text to come from
/// the owner: at once from a built-in owner (a canvas), else from the extension before anything
/// moves. Refused (with why) when the owner hasn't read the file as it is now.
pub(crate) fn owned_edit(
    f: &crate::workspace::Owned,
    asks: Vec<Ask>,
    changes: Vec<Change>,
    links: usize,
) -> Result<FileEdit, String> {
    let text =
        std::fs::read_to_string(&f.path).map_err(|e| format!("{} can't be read: {e}", f.rel))?;
    if f.rev.as_deref() != Some(crate::files::rev(text.as_bytes()).as_str()) {
        return Err(format!("{} hasn't read {} as it is now", f.owner, f.rel));
    }
    if let Some(new_text) = crate::owner::edit(&f.owner, &f.rel, &text, &asks) {
        let new_text =
            new_text.map_err(|why| format!("{} didn't edit {}: {why}", f.owner, f.rel))?;
        return Ok(FileEdit {
            file: f.rel.clone(),
            path: f.path.clone(),
            old_text: text,
            new_text,
            links,
            changes,
            owned: None,
        });
    }
    Ok(FileEdit {
        file: f.rel.clone(),
        path: f.path.clone(),
        old_text: text,
        new_text: String::new(),
        links,
        changes,
        owned: Some(Through {
            owner: f.owner.clone(),
            changes: asks,
            given: f.given.clone(),
            after: None,
        }),
    })
}

/// Refuse when a file among `edits` isn't what it was when `what` (a rename, a merge) was
/// planned.
pub(crate) fn check_unchanged<'a>(
    edits: impl IntoIterator<Item = &'a FileEdit>,
    what: &str,
) -> Result<()> {
    for e in edits {
        let now =
            std::fs::read_to_string(&e.path).with_context(|| format!("reading {}", e.file))?;
        if now != e.old_text {
            bail!(
                "{} changed since the {what} was planned; nothing was changed. Try again.",
                e.file
            );
        }
    }
    Ok(())
}

/// Files a merge or a recipe wrote, with what each held before and holds now, to put back.
#[derive(Clone, Debug, Default)]
pub struct Written(Vec<(PathBuf, String, String)>);

impl Written {
    /// Write each of `edits` with `write::replace`; if one fails, put back the ones written.
    pub(crate) fn all<'a>(edits: impl IntoIterator<Item = &'a FileEdit>) -> Result<Written> {
        let mut written = Written::default();
        for e in edits {
            let (old, new) = (e.old_text.as_bytes(), e.new_text.as_bytes());
            if let Err(err) = crate::write::replace(&e.path, old, new) {
                let undone = written.put_back();
                return Err(err).with_context(|| match undone {
                    Ok(()) => format!("writing {}; nothing was changed", e.file),
                    Err(_) => format!("writing {}; some files couldn't be put back", e.file),
                });
            }
            written
                .0
                .push((e.path.clone(), e.old_text.clone(), e.new_text.clone()));
        }
        Ok(written)
    }

    /// Refuse, naming `what` (a merge), if a written file was changed since.
    pub(crate) fn check(&self, what: &str) -> Result<()> {
        for (path, _, after) in &self.0 {
            if std::fs::read(path).ok().as_deref() != Some(after.as_bytes()) {
                bail!(
                    "{} changed since the {what}; the {what} can't be undone",
                    path.display()
                );
            }
        }
        Ok(())
    }

    /// Write back what each file held, if it still holds what was written.
    pub(crate) fn put_back(&self) -> Result<(), crate::write::WriteError> {
        let mut result = Ok(());
        for (path, before, after) in self.0.iter().rev() {
            if let Err(e) = crate::write::replace(path, after.as_bytes(), before.as_bytes()) {
                result = Err(e);
            }
        }
        result
    }

    pub fn paths(&self) -> impl Iterator<Item = &Path> {
        self.0.iter().map(|(p, _, _)| p.as_path())
    }
}

/// Carry out a plan: refuse if any file changed since it was made, or a file another owner reads
/// has no new text from it yet (`Core::relink`), then make the moves (undoing the ones made if one
/// fails) and write each changed file atomically.
pub fn apply(plan: &Plan) -> Result<()> {
    check_answered(&plan.edits)?;
    check_unchanged(plan.edits.iter().chain(&plan.bookmarks), "rename")?;
    for m in &plan.moves {
        if !m.from_path.exists() {
            bail!("{} no longer exists", m.from);
        }
        if m.to_path.exists() && m.to.to_lowercase() != m.from.to_lowercase() {
            bail!("{} already exists", m.to);
        }
    }
    for (i, m) in plan.moves.iter().enumerate() {
        if let Err(e) = crate::write::move_file(&m.from_path, &m.to_path) {
            for done in plan.moves[..i].iter().rev() {
                let _ = crate::write::move_file(&done.to_path, &done.from_path);
            }
            return Err(e).with_context(|| format!("moving {} to {}", m.from, m.to));
        }
    }
    let mut failed = Vec::new();
    for e in plan.edits.iter().chain(&plan.bookmarks) {
        // Still guarded: a file edited between the check above and now is left alone.
        let path = plan.moved(&e.path);
        if let Err(err) = crate::write::replace(&path, e.old_text.as_bytes(), e.new_text.as_bytes())
        {
            failed.push(format!("{} ({err})", e.file));
        }
    }
    if !failed.is_empty() {
        bail!(
            "moved {}, but couldn't update links in: {}",
            plan.what(),
            failed.join(", ")
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workspace;
    use std::path::Path;

    fn workspace_with(name: &str, files: &[(&str, &str)]) -> (PathBuf, Index) {
        let dir =
            std::env::temp_dir().join(format!("cold-lava-rename-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        for (rel, text) in files {
            let p = dir.join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, text).unwrap();
        }
        let ix = Index::build(workspace::scan(&dir).unwrap());
        (dir, ix)
    }

    fn read(dir: &Path, rel: &str) -> String {
        std::fs::read_to_string(dir.join(rel)).unwrap()
    }

    #[test]
    fn target_paths() {
        assert_eq!(
            target_rel("Concepts/Lava.md", "Magma").unwrap(),
            "Concepts/Magma.md"
        );
        assert_eq!(
            target_rel("Concepts/Lava.md", "Archive/").unwrap(),
            "Archive/Lava.md"
        );
        assert_eq!(
            target_rel("Lava.md", "Concepts/Hot Rock.md").unwrap(),
            "Concepts/Hot Rock.md"
        );
        assert!(target_rel("Lava.md", "../outside").is_err());
        assert!(target_rel("Lava.md", "bad|name").is_err());
        assert!(target_rel("Lava.md", "  ").is_err());
    }

    #[test]
    fn rewrites_every_kind_of_link() {
        let (dir, ix) = workspace_with(
            "kinds",
            &[
                ("Concepts/Lava.md", "# Lava\n## Textures\n"),
                (
                    "Notes/Uses.md",
                    "See [[Lava]], [[Lava|molten rock]], [[Lava#Textures]], ![[Lava]] and [[Concepts/Lava]].\n\
                 Also [md](../Concepts/Lava.md) and [abs](Concepts/Lava.md#Textures).\n\
                 Code `[[Lava]]` stays.\n```\n[[Lava]]\n```\n[[Lavender]] too.\n",
                ),
                ("Lavender.md", "x"),
            ],
        );
        let lava = ix.by_rel("Concepts/Lava").unwrap();
        let p = plan(&ix, lava, "Geology/Hot Rock.md").unwrap();
        assert_eq!(p.links(), 7);
        apply(&p).unwrap();
        assert!(!dir.join("Concepts/Lava.md").exists());
        assert!(dir.join("Geology/Hot Rock.md").exists());
        assert_eq!(
            read(&dir, "Notes/Uses.md"),
            "See [[Hot Rock]], [[Hot Rock|molten rock]], [[Hot Rock#Textures]], ![[Hot Rock]] and [[Geology/Hot Rock]].\n\
             Also [md](../Geology/Hot%20Rock.md) and [abs](Geology/Hot%20Rock.md#Textures).\n\
             Code `[[Lava]]` stays.\n```\n[[Lava]]\n```\n[[Lavender]] too.\n"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn percent_encoded_links_are_followed_and_keep_their_encoding() {
        let (dir, ix) = workspace_with(
            "percent",
            &[
                ("Lava/Pāhoehoe.md", "x"),
                ("Lava/flow map.png", ""),
                (
                    "Notes.md",
                    "[a](Lava/P%C4%81hoehoe.md) [b](Lava/P%c4%81hoehoe.md#Top) [[Pāhoehoe]]\n\
                     ![m](Lava/flow%20map.png)\n",
                ),
            ],
        );
        let p = plan(
            &ix,
            ix.by_rel("Lava/Pāhoehoe").unwrap(),
            "Lava/ʻAʻā flow.md",
        )
        .unwrap();
        assert_eq!(p.links(), 3);
        apply(&p).unwrap();
        assert_eq!(
            read(&dir, "Notes.md"),
            "[a](Lava/%CA%BBA%CA%BB%C4%81%20flow.md) [b](Lava/%CA%BBA%CA%BB%C4%81%20flow.md#Top) \
             [[ʻAʻā flow]]\n![m](Lava/flow%20map.png)\n"
        );
        let ix = Index::build(crate::workspace::scan(&dir).unwrap());
        let png = ix
            .by_rel("Notes")
            .and_then(|n| crate::resolver::attachment(&ix, "Lava/flow map.png", Some(n)));
        assert!(png.is_some_and(|p| p.ends_with("Lava/flow map.png")));
        let p = plan_folder(&ix, "Lava", "Rock flows").unwrap();
        apply(&p).unwrap();
        assert_eq!(
            read(&dir, "Notes.md"),
            "[a](Rock%20flows/%CA%BBA%CA%BB%C4%81%20flow.md) \
             [b](Rock%20flows/%CA%BBA%CA%BB%C4%81%20flow.md#Top) [[ʻAʻā flow]]\n\
             ![m](Rock%20flows/flow%20map.png)\n"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn links_that_only_escaped_spaces_stay_that_way() {
        let (dir, ix) = workspace_with(
            "spaces",
            &[("Hot Rock.md", "x"), ("Notes.md", "[a](Hot%20Rock.md)\n")],
        );
        let p = plan(&ix, ix.by_rel("Hot Rock").unwrap(), "Pāhoehoe lava.md").unwrap();
        apply(&p).unwrap();
        assert_eq!(read(&dir, "Notes.md"), "[a](Pāhoehoe%20lava.md)\n");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn moved_note_keeps_its_own_relative_links() {
        let (dir, ix) = workspace_with(
            "own",
            &[
                (
                    "A/Moving.md",
                    "[up](../Top.md) [[../Top]] [[Top]] [self](Moving.md)\n",
                ),
                ("Top.md", "[[A/Moving]]\n"),
            ],
        );
        let m = ix.by_rel("A/Moving").unwrap();
        let p = plan(&ix, m, "B/C/Moving.md").unwrap();
        apply(&p).unwrap();
        assert_eq!(
            read(&dir, "B/C/Moving.md"),
            "[up](../../Top.md) [[../../Top]] [[Top]] [self](Moving.md)\n"
        );
        assert_eq!(read(&dir, "Top.md"), "[[B/C/Moving]]\n");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn name_collisions_use_paths_and_warn() {
        let (dir, ix) = workspace_with(
            "collide",
            &[
                ("Old.md", "x"),
                ("Other/New.md", "y"),
                ("Linker.md", "[[Old]]\n"),
            ],
        );
        assert!(
            plan(&ix, ix.by_rel("Old").unwrap(), "Other/New.md").is_err(),
            "that file exists"
        );
        let p = plan(&ix, ix.by_rel("Old").unwrap(), "Archive/New.md").unwrap();
        assert_eq!(p.warnings.len(), 1);
        apply(&p).unwrap();
        assert_eq!(read(&dir, "Linker.md"), "[[Archive/New]]\n");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn canvas_cards_are_updated_in_place() {
        let canvas = "{\n\t\"nodes\":[\n\t\t{\"id\":\"a\",\"type\":\"file\",\"file\":\"Lava.md\",\"x\":0,\"y\":0,\"width\":1,\"height\":1},\n\t\t{\"id\":\"b\",\"type\":\"text\",\"text\":\"About [[Lava]] \\\"hot\\\"\",\"x\":0,\"y\":0,\"width\":1,\"height\":1}\n\t],\n\t\"edges\":[]\n}";
        let (dir, ix) = workspace_with("canvas", &[("Lava.md", "x"), ("Board.canvas", canvas)]);
        let p = plan(&ix, ix.by_rel("Lava").unwrap(), "Magma.md").unwrap();
        apply(&p).unwrap();
        let out = read(&dir, "Board.canvas");
        assert_eq!(
            out,
            canvas
                .replace("\"file\":\"Lava.md\"", "\"file\":\"Magma.md\"")
                .replace("About [[Lava]]", "About [[Magma]]")
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn apply_refuses_if_a_file_changed() {
        let (dir, ix) = workspace_with("guard", &[("A.md", "x"), ("B.md", "[[A]]\n")]);
        let p = plan(&ix, ix.by_rel("A").unwrap(), "Z.md").unwrap();
        std::fs::write(dir.join("B.md"), "[[A]] edited elsewhere\n").unwrap();
        assert!(apply(&p).is_err());
        assert!(
            dir.join("A.md").exists() && !dir.join("Z.md").exists(),
            "nothing moved"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_link_by_reference_is_rewritten_in_its_definition() {
        let uses = "See [molten rock][lava] and [Lava].\n\n[lava]: ../Concepts/Lava.md \"Hot\"\n[abs]: <Concepts/Lava.md#Lava>\n";
        let (dir, ix) = workspace_with(
            "reference",
            &[("Concepts/Lava.md", "# Lava\n"), ("Notes/Uses.md", uses)],
        );
        let p = plan(
            &ix,
            ix.by_rel("Concepts/Lava").unwrap(),
            "Geology/Hot Rock.md",
        )
        .unwrap();
        assert_eq!((p.links(), p.files()), (2, 1));
        std::fs::write(
            dir.join("Notes/Uses.md"),
            format!("{uses}edited elsewhere\n"),
        )
        .unwrap();
        assert!(apply(&p).is_err());
        assert!(dir.join("Concepts/Lava.md").exists(), "nothing moved");
        std::fs::write(dir.join("Notes/Uses.md"), uses).unwrap();
        apply(&p).unwrap();
        assert_eq!(
            read(&dir, "Notes/Uses.md"),
            "See [molten rock][lava] and [Lava].\n\n[lava]: ../Geology/Hot%20Rock.md \"Hot\"\n[abs]: <Geology/Hot%20Rock.md#Lava>\n",
            "the label, the title, the `<>` and the heading stay"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn folder_move_rewrites_links_in_and_out() {
        let canvas = "{\"nodes\":[\n{\"id\":\"a\",\"type\":\"file\",\"file\":\"People/Krafft.md\"},\n{\"id\":\"b\",\"type\":\"file\",\"file\":\"People/photo.png\"},\n{\"id\":\"c\",\"type\":\"text\",\"text\":\"[[People/Johnston]]\"},\n{\"id\":\"d\",\"type\":\"file\",\"file\":\"Volcanoes/Helens.md\"}\n],\"edges\":[]}";
        let (dir, ix) = workspace_with(
            "folder",
            &[
                (
                    "People/Johnston.md",
                    "[[Helens]] [up](../Volcanoes/Helens.md) [sib](Krafft.md) [[./Krafft]] \
                     [rel](./Krafft.md) ![[People/photo.png]] ![p](photo.png)\n",
                ),
                ("People/Krafft.md", "x"),
                ("People/photo.png", ""),
                (
                    "Volcanoes/Helens.md",
                    "---\nrelated: \"[[People/Johnston]]\"\n---\n[[Johnston]] [[People/Krafft|K]] \
                     [md](../People/Johnston.md) [abs](People/Johnston.md) ![[photo.png]] \
                     ![img](../People/photo.png)\n",
                ),
                ("Board.canvas", canvas),
            ],
        );
        let p = plan_folder(&ix, "People", "Scientists/Volcanologists").unwrap();
        assert_eq!(
            p.moving,
            ["People/Johnston.md", "People/Krafft.md", "People/photo.png"]
        );
        apply(&p).unwrap();
        assert!(!dir.join("People").exists());
        assert_eq!(
            read(&dir, "Scientists/Volcanologists/Johnston.md"),
            "[[Helens]] [up](../../Volcanoes/Helens.md) [sib](Krafft.md) [[./Krafft]] \
             [rel](./Krafft.md) ![[Scientists/Volcanologists/photo.png]] ![p](photo.png)\n"
        );
        assert_eq!(
            read(&dir, "Volcanoes/Helens.md"),
            "---\nrelated: \"[[Scientists/Volcanologists/Johnston]]\"\n---\n[[Johnston]] \
             [[Scientists/Volcanologists/Krafft|K]] [md](../Scientists/Volcanologists/Johnston.md) \
             [abs](Scientists/Volcanologists/Johnston.md) ![[photo.png]] \
             ![img](../Scientists/Volcanologists/photo.png)\n"
        );
        assert_eq!(
            read(&dir, "Board.canvas"),
            canvas.replace("People/", "Scientists/Volcanologists/")
        );
        assert!(dir.join("Scientists/Volcanologists/photo.png").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn bare_names_become_paths_only_when_another_note_would_win() {
        let files = [
            ("A/Lava.md", "near the top"),
            ("B/C/Lava.md", "deeper"),
            ("Notes.md", "[[Lava]] [[B/C/Lava]]\n"),
        ];
        let (dir, ix) = workspace_with("shallow-same", &files);
        let p = plan_folder(&ix, "A", "D").unwrap();
        assert_eq!(p.links(), 0, "A/Lava is still the shallowest Lava");
        let _ = std::fs::remove_dir_all(&dir);

        let (dir, ix) = workspace_with("shallow", &files);
        let p = plan_folder(&ix, "A", "X/Y/A").unwrap();
        apply(&p).unwrap();
        assert_eq!(
            read(&dir, "Notes.md"),
            "[[X/Y/A/Lava]] [[B/C/Lava]]\n",
            "B/C/Lava would win [[Lava]] now"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_relative_link_into_its_own_folder_keeps_leading_there() {
        let (dir, ix) = workspace_with(
            "same-folder",
            &[
                ("Top.md", "root"),
                ("B/Top.md", "moving"),
                ("A/Note.md", "[x](../B/Top.md) [[../B/Top]]\n"),
            ],
        );
        apply(&plan(&ix, ix.by_rel("B/Top").unwrap(), "A/Top.md").unwrap()).unwrap();
        assert_eq!(
            read(&dir, "A/Note.md"),
            "[x](./Top.md) [[./Top]]\n",
            "a bare Top would be the root's"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn folder_moves_that_are_refused() {
        let (dir, ix) = workspace_with(
            "refuse",
            &[
                ("A/x.md", ""),
                ("A/sub/y.md", ""),
                ("B/z.md", ""),
                (".obsidian/app.json", "{}"),
            ],
        );
        for (from, to) in [
            ("A", "A/sub/A"),
            ("A", "A/inner"),
            ("A", "B"),
            ("A", "A"),
            ("A", ".trash/A"),
            ("A", "../outside"),
            (".obsidian", "Settings"),
            ("", "Root"),
            ("Missing", "Found"),
        ] {
            assert!(plan_folder(&ix, from, to).is_err(), "{from} → {to}");
        }
        assert_eq!(folder_target("A/sub", "Deep").unwrap(), "A/Deep");
        assert_eq!(folder_target("A/sub", "B/").unwrap(), "B/sub");
        assert!(folder_target("A", "a|b").is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn folder_apply_refuses_if_a_file_changed() {
        let (dir, ix) = workspace_with("folder-guard", &[("A/a.md", "x"), ("B.md", "[[A/a]]\n")]);
        let p = plan_folder(&ix, "A", "Z").unwrap();
        std::fs::write(dir.join("B.md"), "[[A/a]] edited elsewhere\n").unwrap();
        assert!(apply(&p).is_err());
        assert!(dir.join("A/a.md").exists() && !dir.join("Z").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn several_moves_are_one_plan_with_links_between_them_right() {
        let (dir, ix) = workspace_with(
            "many",
            &[
                ("A/One.md", "[[Two]] [two](../B/Two.md) [[B/Two]]\n"),
                ("B/Two.md", "[one](../A/One.md) [[A/One]]\n"),
                ("C/Three.md", "[[One]] [[Two]]\n"),
                ("D/Stay.md", "[[A/One]] [[B/Two]] [[C/Three]] [[Three]]\n"),
                (
                    "Board.canvas",
                    "{\"nodes\":[{\"id\":\"a\",\"type\":\"file\",\"file\":\"A/One.md\"}],\"edges\":[]}",
                ),
            ],
        );
        let items = ["A/One.md", "B/Two.md", "C"].map(String::from);
        let p = plan_into(&ix, &items, "Archive").unwrap();
        assert_eq!(p.moves.len(), 3);
        let mut files: Vec<&str> = p.edits.iter().map(|e| e.file.as_str()).collect();
        files.sort();
        assert_eq!(
            files,
            ["A/One.md", "B/Two.md", "Board.canvas", "D/Stay.md"],
            "each file once; Three's bare names still resolve"
        );
        apply(&p).unwrap();
        assert_eq!(
            read(&dir, "Archive/One.md"),
            "[[Two]] [two](Two.md) [[Archive/Two]]\n"
        );
        assert_eq!(
            read(&dir, "Archive/Two.md"),
            "[one](One.md) [[Archive/One]]\n"
        );
        assert_eq!(read(&dir, "Archive/C/Three.md"), "[[One]] [[Two]]\n");
        assert_eq!(
            read(&dir, "D/Stay.md"),
            "[[Archive/One]] [[Archive/Two]] [[Archive/C/Three]] [[Three]]\n"
        );
        assert!(read(&dir, "Board.canvas").contains("\"file\":\"Archive/One.md\""));
        assert!(!dir.join("A/One.md").exists() && !dir.join("C").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn several_moves_that_are_refused() {
        let (dir, ix) = workspace_with(
            "many-refuse",
            &[
                ("A/Same.md", ""),
                ("B/Same.md", ""),
                ("F/sub/x.md", ""),
                ("F/y.md", ""),
                ("G/z.md", ""),
                ("Archive/Taken.md", ""),
                ("Taken.md", ""),
            ],
        );
        let refused = |items: &[&str], to: &str| {
            let items: Vec<String> = items.iter().map(|s| s.to_string()).collect();
            plan_into(&ix, &items, to).unwrap_err().to_string()
        };
        assert_eq!(
            refused(&["A/Same.md", "B/Same.md"], "Archive"),
            "A/Same.md and B/Same.md would both be Archive/Same.md"
        );
        assert_eq!(
            refused(&["F", "G"], "F/sub"),
            "a folder can't move into itself"
        );
        assert_eq!(
            refused(&["G", "F/sub"], "F/sub"),
            "G can't move into F/sub: it moves too"
        );
        assert_eq!(
            refused(&["F", "F/y.md"], "Archive"),
            "F/y.md is inside F, which moves too"
        );
        assert_eq!(
            refused(&["Taken.md"], "Archive"),
            "Archive/Taken.md already exists"
        );
        assert_eq!(refused(&["F/y.md"], "F"), "they're already in F");
        let items = ["F/y.md", "G/z.md"].map(String::from);
        let p = plan_into(&ix, &items, "F").unwrap();
        assert_eq!(p.moves.len(), 1, "an item already there stays");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn several_moves_change_nothing_if_a_file_changed() {
        let (dir, ix) = workspace_with(
            "many-guard",
            &[
                ("A/a.md", "x"),
                ("B/b.md", "y"),
                ("Links.md", "[[A/a]] [[B/b]]\n"),
            ],
        );
        let items = ["A/a.md", "B/b.md"].map(String::from);
        let p = plan_into(&ix, &items, "Z").unwrap();
        std::fs::write(dir.join("Links.md"), "edited elsewhere\n").unwrap();
        assert!(apply(&p).is_err());
        assert!(dir.join("A/a.md").exists() && dir.join("B/b.md").exists());
        assert!(!dir.join("Z").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    const MARKS: &str = r##"{
  "items": [
    {
      "type": "file",
      "ctime": 1,
      "path": "Rock/Lava.md",
      "subpath": "#Textures",
      "colour": "kept"
    },
    {
      "type": "group",
      "ctime": 2,
      "items": [
        {
          "type": "folder",
          "ctime": 3,
          "path": "Rock"
        },
        {
          "type": "file",
          "ctime": 4,
          "path": "Rock/Deep/Magma.md"
        },
        {
          "type": "file",
          "ctime": 5,
          "path": "Rocky.md"
        }
      ],
      "title": "Geology"
    },
    {
      "type": "mystery",
      "path": "Rock/Lava.md"
    }
  ]
}"##;

    #[test]
    fn bookmarks_follow_a_renamed_note_keeping_the_heading() {
        let (dir, ix) = workspace_with(
            "bookmarks-note",
            &[
                ("Rock/Lava.md", "# Textures\n"),
                ("Rocky.md", "x"),
                (".obsidian/bookmarks.json", MARKS),
            ],
        );
        let p = plan(&ix, ix.by_rel("Rock/Lava").unwrap(), "Rock/Basalt.md").unwrap();
        assert_eq!(p.bookmarks.as_ref().map(|b| b.links), Some(1));
        assert_eq!(p.links(), 0, "bookmarks aren't links");
        apply(&p).unwrap();
        assert_eq!(
            read(&dir, ".obsidian/bookmarks.json"),
            MARKS.replacen("\"Rock/Lava.md\"", "\"Rock/Basalt.md\"", 1),
            "the unknown item that names the same path is left alone"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn bookmarks_follow_a_moved_folder_and_what_is_in_it() {
        let (dir, ix) = workspace_with(
            "bookmarks-folder",
            &[
                ("Rock/Lava.md", "x"),
                ("Rock/Deep/Magma.md", "y"),
                ("Rocky.md", "z"),
                (".obsidian/bookmarks.json", MARKS),
            ],
        );
        let p = plan_folder(&ix, "Rock", "Earth/Stone").unwrap();
        assert_eq!(p.bookmarks.as_ref().map(|b| b.links), Some(3));
        apply(&p).unwrap();
        let want = MARKS
            .replacen("\"Rock/Lava.md\"", "\"Earth/Stone/Lava.md\"", 1)
            .replace("\"Rock\"", "\"Earth/Stone\"")
            .replace("\"Rock/Deep/Magma.md\"", "\"Earth/Stone/Deep/Magma.md\"");
        assert_eq!(read(&dir, ".obsidian/bookmarks.json"), want);
        assert!(
            want.contains("\"Rocky.md\""),
            "a name that only starts the same stays"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn bookmarks_changed_since_the_plan_stop_the_rename() {
        let (dir, ix) = workspace_with(
            "bookmarks-changed",
            &[("Rock/Lava.md", "x"), (".obsidian/bookmarks.json", MARKS)],
        );
        let p = plan(&ix, ix.by_rel("Rock/Lava").unwrap(), "Basalt.md").unwrap();
        let theirs = MARKS.replace("\"colour\"", "\"color\"");
        std::fs::write(dir.join(".obsidian/bookmarks.json"), &theirs).unwrap();
        assert!(apply(&p).is_err());
        assert!(dir.join("Rock/Lava.md").exists(), "nothing moved");
        assert_eq!(read(&dir, ".obsidian/bookmarks.json"), theirs);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
