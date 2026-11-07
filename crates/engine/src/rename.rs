//! Renaming and moving notes and folders with every link to them updated, like Obsidian's
//! "Automatically update internal links".
//!
//! `plan` (a note) and `plan_folder` (a folder and everything in it) work out every change
//! without touching anything; `apply` checks nothing changed since, then moves the note or
//! folder and rewrites the other files atomically, one by one.
//!
//! The rule for links: every link in the vault is resolved as the vault is now and as it will
//! be after the move. A link that still leads to the same file, the same way, is left alone;
//! any other is rewritten in the style it was written in. So `[[Lava]]` survives a folder move,
//! but becomes a path when the move makes another `Lava` the shallowest one.

use crate::index::Index;
use crate::vault::attachment_kind;
use anyhow::{Context, Result, bail};
use regex::Regex;
use serde::Serialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use walkdir::WalkDir;

#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct Change {
    /// 1-based line.
    pub line: usize,
    pub before: String,
    pub after: String,
}

#[derive(Serialize, Clone, Debug)]
pub struct FileEdit {
    /// Vault-relative path of the file as it is now.
    pub file: String,
    #[serde(skip)]
    pub path: PathBuf,
    #[serde(skip)]
    pub old_text: String,
    #[serde(skip)]
    pub new_text: String,
    /// Links rewritten in this file.
    pub links: usize,
    pub changes: Vec<Change>,
}

#[derive(Serialize, Clone, Debug)]
pub struct Plan {
    /// Vault-relative: a note with `.md`, or a folder.
    pub from: String,
    pub to: String,
    pub folder: bool,
    #[serde(skip)]
    pub from_path: PathBuf,
    #[serde(skip)]
    pub to_path: PathBuf,
    /// The files that move (vault-relative, as they are now): the note, or what the folder holds.
    pub moving: Vec<String>,
    /// Files whose links change, moving ones included (under their path as it is now).
    pub edits: Vec<FileEdit>,
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
    pub fn moves(&self) -> Vec<(PathBuf, PathBuf)> {
        if !self.folder {
            return vec![(self.from_path.clone(), self.to_path.clone())];
        }
        let prefix = format!("{}/", self.from);
        (self.moving.iter())
            .filter_map(|r| r.strip_prefix(&prefix))
            .map(|rest| (self.from_path.join(rest), self.to_path.join(rest)))
            .collect()
    }

    /// Where a file at `path` is after the move (itself if it doesn't move).
    pub fn moved(&self, path: &Path) -> PathBuf {
        match path.strip_prefix(&self.from_path) {
            Ok(rest) if rest.as_os_str().is_empty() => self.to_path.clone(),
            Ok(rest) => self.to_path.join(rest),
            Err(_) => path.to_path_buf(),
        }
    }
}

/// Where `new` puts the note at `old_rel` (both vault-relative, `.md` included in the result):
/// a bare name renames it in its folder, `folder/` moves it keeping its name, anything else is
/// a full vault path.
pub fn target_rel(old_rel: &str, new: &str) -> Result<String> {
    let new = new.trim();
    let new = new.strip_suffix(".md").unwrap_or(new);
    Ok(format!(
        "{}.md",
        target_path(old_rel.trim_end_matches(".md"), new)?
    ))
}

/// The same for the folder at `old_rel`: a bare name renames it, `folder/` moves it into that
/// folder, anything else is a full vault path.
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

/// A path a note or folder can have: inside the vault, not hidden, and one links can name.
pub fn check_path(rel: &str) -> Result<()> {
    if rel
        .split('/')
        .any(|c| c.is_empty() || c == "." || c == ".." || c.starts_with('.'))
    {
        bail!("not a valid path inside the vault: {rel}");
    }
    if rel
        .chars()
        .any(|c| matches!(c, '[' | ']' | '|' | '#' | '^'))
    {
        bail!("names can't contain [ ] | # or ^ (links couldn't point to them)");
    }
    Ok(())
}

fn join(dir: &str, name: &str) -> String {
    if dir.is_empty() {
        name.to_string()
    } else {
        format!("{dir}/{name}")
    }
}

fn dir_of(rel: &str) -> &str {
    rel.rsplit_once('/').map(|(d, _)| d).unwrap_or("")
}

/// Resolve `a/b/../c` style paths (vault-relative, no leading slash).
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

/// Path from folder `from_dir` to vault-relative `to`, e.g. ("A/B", "C/D") → "../../C/D".
fn relative(from_dir: &str, to: &str) -> String {
    let f: Vec<&str> = from_dir.split('/').filter(|s| !s.is_empty()).collect();
    let t: Vec<&str> = to.split('/').filter(|s| !s.is_empty()).collect();
    let common = f.iter().zip(&t).take_while(|(a, b)| a == b).count();
    let mut parts: Vec<&str> = vec![".."; f.len() - common];
    parts.extend(&t[common..]);
    parts.join("/")
}

/// How a link names a file: notes without `.md`, other files with their extension.
fn link_key(rel: &str) -> &str {
    rel.strip_suffix(".md").unwrap_or(rel)
}

/// How a link found its file.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Via {
    Path,
    Name,
}

/// Every file a link can lead to, and how links find them: notes by path or by name (the
/// shallowest wins), `./` and `../` paths, other files (images, PDFs, canvases) by path, next
/// to the linking note, or by name.
pub struct Files {
    /// Vault-relative, with extension; the index's notes first, in its order.
    pub rels: Vec<String>,
    by_path: HashMap<String, usize>,
    by_name: HashMap<String, usize>,
    /// How many files have each name.
    names: HashMap<String, usize>,
}

impl Files {
    /// The vault's notes, and its other files read from disk.
    pub fn of_vault(ix: &Index) -> Files {
        let mut rels: Vec<String> = ix
            .vault
            .notes
            .iter()
            .map(|n| format!("{}.md", n.rel))
            .collect();
        let root = &ix.vault.root;
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

    fn new(rels: Vec<String>) -> Files {
        let mut order: Vec<usize> = (0..rels.len()).collect();
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

    /// The file a link written in a file in `dir` leads to.
    pub fn resolve(&self, dir: &str, raw: &str) -> Option<usize> {
        self.find(dir, raw).map(|(i, _)| i)
    }

    fn path(&self, key: &str) -> Option<usize> {
        self.by_path.get(&key.to_lowercase()).copied()
    }

    fn find(&self, dir: &str, raw: &str) -> Option<(usize, Via)> {
        let raw = raw.trim().replace("%20", " ");
        let other = attachment_kind(&raw).is_some();
        let key = if other { raw.as_str() } else { link_key(&raw) };
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

    fn unique(&self, name: &str) -> bool {
        self.names.get(&name.to_lowercase()).copied().unwrap_or(0) <= 1
    }
}

struct Patterns {
    wikilink: Regex,
    mdlink: Regex,
    inline_code: Regex,
    canvas_field: Regex,
}

fn patterns() -> &'static Patterns {
    static P: OnceLock<Patterns> = OnceLock::new();
    P.get_or_init(|| Patterns {
        // [[target#heading|alias]] — target is group 1.
        wikilink: Regex::new(r"\[\[([^\]\|#\^]*)((?:[#\^][^\]\|]*)?)((?:\|[^\]]*)?)\]\]").unwrap(),
        // [text](path#heading) — path is group 1; kept if it names a note or an attachment.
        mdlink: Regex::new(r"\]\(([^)\s#]+)((?:#[^)]*)?)\)").unwrap(),
        inline_code: Regex::new(r"`[^`\n]*`").unwrap(),
        canvas_field: Regex::new(r#""(file|text)"(\s*:\s*)("(?:[^"\\]|\\.)*")"#).unwrap(),
    })
}

/// The vault before and after the move: same files (same indices), new paths for some.
struct Rewriter {
    before: Files,
    after: Files,
}

impl Rewriter {
    /// What the link `raw` should become, or `None` if it still leads where it did. `dir_then`
    /// is the linking file's folder before the move, `dir_now` after.
    fn link(&self, raw: &str, md: bool, dir_then: &str, dir_now: &str) -> Option<String> {
        let (target, via) = self.before.find(dir_then, raw)?;
        let still = self.after.find(dir_now, raw);
        if still.is_some_and(|(t, v)| t == target && !(via == Via::Path && v == Via::Name)) {
            return None;
        }
        let new_rel = &self.after.rels[target];
        let decoded = raw.trim().replace("%20", " ");
        let relative_style = decoded.starts_with("./") || decoded.starts_with("../");
        let out = if md {
            // Vault-absolute ("Folder/Note.md", "/Folder/Note.md") or relative to the note?
            let absolute = decoded.starts_with('/')
                || (!decoded.starts_with('.')
                    && self.before.path(link_key(&decoded)) == Some(target)
                    && !dir_then.is_empty());
            let path = if absolute {
                let slash = if decoded.starts_with('/') { "/" } else { "" };
                format!("{slash}{new_rel}")
            } else {
                relative(dir_now, new_rel)
            };
            path.replace(' ', "%20")
        } else {
            let shown = if decoded.ends_with(".md") {
                new_rel.as_str()
            } else {
                link_key(new_rel)
            };
            let name = shown.rsplit('/').next().unwrap_or(shown);
            if relative_style {
                relative(dir_now, shown)
            } else if decoded.contains('/') || !self.after.unique(link_key(name)) {
                shown.to_string()
            } else {
                name.to_string()
            }
        };
        (out != raw).then_some(out)
    }

    /// Rewrite the links in a note's text. Returns the new text, changed lines and link count.
    fn note_text(&self, text: &str, dir_then: &str, dir_now: &str) -> (String, Vec<Change>, usize) {
        let p = patterns();
        let mut out = String::with_capacity(text.len());
        let mut changes = Vec::new();
        let mut count = 0;
        let mut in_fence = false;
        for (i, line) in text.split_inclusive('\n').enumerate() {
            let body = line.trim_end_matches(['\n', '\r']);
            let ending = &line[body.len()..];
            let t = body.trim_start();
            if t.starts_with("```") || t.starts_with("~~~") {
                in_fence = !in_fence;
            }
            if in_fence || t.starts_with("```") || t.starts_with("~~~") {
                out.push_str(line);
                continue;
            }
            let (new_body, n) = self.line(body, dir_then, dir_now, p);
            if n > 0 {
                count += n;
                changes.push(Change {
                    line: i + 1,
                    before: body.to_string(),
                    after: new_body.clone(),
                });
            }
            out.push_str(&new_body);
            out.push_str(ending);
        }
        (out, changes, count)
    }

    fn line(&self, body: &str, dir_then: &str, dir_now: &str, p: &Patterns) -> (String, usize) {
        let code: Vec<(usize, usize)> = p
            .inline_code
            .find_iter(body)
            .map(|m| (m.start(), m.end()))
            .collect();
        let in_code = |s: usize| code.iter().any(|&(a, b)| s >= a && s < b);
        // Collect replacements (byte range of the target, new text), then apply from the end.
        let mut reps: Vec<(usize, usize, String)> = Vec::new();
        for c in p.wikilink.captures_iter(body) {
            let m = c.get(1).unwrap();
            if !in_code(m.start())
                && !m.as_str().trim().is_empty()
                && let Some(new) = self.link(m.as_str(), false, dir_then, dir_now)
            {
                reps.push((m.start(), m.end(), new));
            }
        }
        for c in p.mdlink.captures_iter(body) {
            let m = c.get(1).unwrap();
            let target = m.as_str();
            let names_a_file = target.ends_with(".md") || attachment_kind(target).is_some();
            if !in_code(m.start())
                && names_a_file
                && !target.contains("://")
                && let Some(new) = self.link(target, true, dir_then, dir_now)
            {
                reps.push((m.start(), m.end(), new));
            }
        }
        reps.sort_by_key(|r| std::cmp::Reverse(r.0));
        let mut s = body.to_string();
        for (a, b, new) in &reps {
            s.replace_range(*a..*b, new);
        }
        (s, reps.len())
    }
}

/// Work out everything renaming/moving note `note` to vault-relative `new_rel` (with `.md`)
/// would change. Nothing is written.
pub fn plan(ix: &Index, note: usize, new_rel: &str) -> Result<Plan> {
    let n = &ix.vault.notes[note];
    let from_rel = format!("{}.md", n.rel);
    let new_rel = if new_rel.ends_with(".md") {
        new_rel.to_string()
    } else {
        format!("{new_rel}.md")
    };
    let to_path = ix.vault.root.join(&new_rel);
    if new_rel == from_rel {
        bail!("that's already its name");
    }
    // A case-only rename of the same file is fine; anything else that exists isn't.
    if to_path.exists() && new_rel.to_lowercase() != from_rel.to_lowercase() {
        bail!("{new_rel} already exists");
    }
    let before = Files::of_vault(ix);
    let mut rels = before.rels.clone();
    rels[note] = new_rel.clone();
    let rw = Rewriter {
        before,
        after: Files::new(rels),
    };
    let new_name = link_key(&new_rel).rsplit('/').next().unwrap_or("");
    let mut warnings = Vec::new();
    if !rw.after.unique(new_name) {
        warnings.push(format!(
            "another note is also called \"{new_name}\": links to this one are written as full paths, \
             and existing [[{new_name}]] links may now point to whichever is closer to the vault root"
        ));
    }
    let edits = edits(ix, &rw)?;
    Ok(Plan {
        from: from_rel.clone(),
        to: new_rel,
        folder: false,
        from_path: n.path.clone(),
        to_path,
        moving: vec![from_rel],
        edits,
        warnings,
    })
}

/// Work out everything moving the folder `from` (vault-relative) to `to` would change: every
/// file in it moves, and every link that would lead elsewhere afterwards is rewritten.
/// Nothing is written.
pub fn plan_folder(ix: &Index, from: &str, to: &str) -> Result<Plan> {
    let (from, to) = (from.trim_matches('/'), to.trim_matches('/'));
    if from.is_empty() {
        bail!("that's the vault itself");
    }
    if from.split('/').any(|c| c.starts_with('.')) {
        bail!("{from} is a hidden folder; cold-lava leaves it alone");
    }
    check_path(to)?;
    let root = &ix.vault.root;
    let from_path = root.join(from);
    if !from_path.is_dir() {
        bail!("there's no folder {from}");
    }
    if to == from {
        bail!("that's already its name");
    }
    if to
        .to_lowercase()
        .starts_with(&format!("{}/", from.to_lowercase()))
    {
        bail!("a folder can't move into itself");
    }
    let to_path = root.join(to);
    if to_path.exists() && to.to_lowercase() != from.to_lowercase() {
        bail!("{to} already exists");
    }
    let before = Files::of_vault(ix);
    let prefix = format!("{from}/");
    let mut moving = Vec::new();
    let rels = before
        .rels
        .iter()
        .map(|r| match r.strip_prefix(&prefix) {
            Some(rest) => {
                moving.push(r.clone());
                format!("{to}/{rest}")
            }
            None => r.clone(),
        })
        .collect();
    moving.sort();
    let rw = Rewriter {
        before,
        after: Files::new(rels),
    };
    let edits = edits(ix, &rw)?;
    Ok(Plan {
        from: from.to_string(),
        to: to.to_string(),
        folder: true,
        from_path,
        to_path,
        moving,
        edits,
        warnings: Vec::new(),
    })
}

/// Every note and canvas whose links change.
fn edits(ix: &Index, rw: &Rewriter) -> Result<Vec<FileEdit>> {
    let mut edits = Vec::new();
    for (i, n) in ix.vault.notes.iter().enumerate() {
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

/// An Obsidian Canvas file: file cards pointing at moved files, and links in text cards.
/// Edited as text, so the rest of the file stays exactly as Obsidian wrote it.
fn canvas_edit(ix: &Index, rw: &Rewriter, i: usize) -> Result<Option<FileEdit>> {
    let rel = &rw.before.rels[i];
    let path = ix.vault.root.join(rel);
    let Ok(text) = std::fs::read_to_string(&path) else {
        return Ok(None);
    };
    let (dir_then, dir_now) = (dir_of(rel), dir_of(&rw.after.rels[i]));
    let mut links = 0;
    let mut changes = Vec::new();
    let mut out = String::with_capacity(text.len());
    let mut last = 0;
    for c in patterns().canvas_field.captures_iter(&text) {
        let whole = c.get(0).unwrap();
        let Ok(value) = serde_json::from_str::<String>(&c[3]) else {
            continue;
        };
        let new_value = match &c[1] {
            "file" => rw
                .before
                .path(link_key(&value))
                .map(|f| &rw.after.rels[f])
                .filter(|new| **new != value)
                .map(|new| {
                    links += 1;
                    new.clone()
                }),
            "text" => {
                let (t, _, n) = rw.note_text(&value, dir_then, dir_now);
                links += n;
                (n > 0).then_some(t)
            }
            _ => None,
        };
        if let Some(v) = new_value {
            let line = text[..whole.start()].matches('\n').count() + 1;
            let encoded = serde_json::to_string(&v)?;
            let replacement = format!("\"{}\"{}{}", &c[1], &c[2], encoded);
            changes.push(Change {
                line,
                before: whole.as_str().to_string(),
                after: replacement.clone(),
            });
            out.push_str(&text[last..whole.start()]);
            out.push_str(&replacement);
            last = whole.end();
        }
    }
    if links == 0 {
        return Ok(None);
    }
    out.push_str(&text[last..]);
    Ok(Some(FileEdit {
        file: rel.clone(),
        path,
        old_text: text,
        new_text: out,
        links,
        changes,
    }))
}

/// Carry out a plan: refuse if any file changed since it was made, then move the note or
/// folder and write each changed file atomically.
pub fn apply(plan: &Plan) -> Result<()> {
    for e in &plan.edits {
        let now =
            std::fs::read_to_string(&e.path).with_context(|| format!("reading {}", e.file))?;
        if now != e.old_text {
            bail!(
                "{} changed since the rename was planned; nothing was changed. Try again.",
                e.file
            );
        }
    }
    if !plan.from_path.exists() {
        bail!("{} no longer exists", plan.from);
    }
    if plan.to_path.exists() && plan.to.to_lowercase() != plan.from.to_lowercase() {
        bail!("{} already exists", plan.to);
    }
    crate::write::move_file(&plan.from_path, &plan.to_path)
        .with_context(|| format!("moving {} to {}", plan.from, plan.to))?;
    let mut failed = Vec::new();
    for e in &plan.edits {
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
            plan.from,
            failed.join(", ")
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vault;
    use std::path::Path;

    fn vault_with(name: &str, files: &[(&str, &str)]) -> (PathBuf, Index) {
        let dir =
            std::env::temp_dir().join(format!("cold-lava-rename-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        for (rel, text) in files {
            let p = dir.join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, text).unwrap();
        }
        let ix = Index::build(vault::scan(&dir).unwrap());
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
        assert_eq!(relative("A/B", "C/D"), "../../C/D");
        assert_eq!(relative("A", "A/x"), "x");
        assert_eq!(normalize("A/B/../C"), Some("A/C".into()));
    }

    #[test]
    fn rewrites_every_kind_of_link() {
        let (dir, ix) = vault_with(
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
    fn moved_note_keeps_its_own_relative_links() {
        let (dir, ix) = vault_with(
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
        let (dir, ix) = vault_with(
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
        let (dir, ix) = vault_with("canvas", &[("Lava.md", "x"), ("Board.canvas", canvas)]);
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
        let (dir, ix) = vault_with("guard", &[("A.md", "x"), ("B.md", "[[A]]\n")]);
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
    fn folder_move_rewrites_links_in_and_out() {
        let canvas = "{\"nodes\":[\n{\"id\":\"a\",\"type\":\"file\",\"file\":\"People/Krafft.md\"},\n{\"id\":\"b\",\"type\":\"file\",\"file\":\"People/photo.png\"},\n{\"id\":\"c\",\"type\":\"text\",\"text\":\"[[People/Johnston]]\"},\n{\"id\":\"d\",\"type\":\"file\",\"file\":\"Volcanoes/Helens.md\"}\n],\"edges\":[]}";
        let (dir, ix) = vault_with(
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
        let (dir, ix) = vault_with("shallow-same", &files);
        let p = plan_folder(&ix, "A", "D").unwrap();
        assert_eq!(p.links(), 0, "A/Lava is still the shallowest Lava");
        let _ = std::fs::remove_dir_all(&dir);

        let (dir, ix) = vault_with("shallow", &files);
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
    fn folder_moves_that_are_refused() {
        let (dir, ix) = vault_with(
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
        let (dir, ix) = vault_with("folder-guard", &[("A/a.md", "x"), ("B.md", "[[A/a]]\n")]);
        let p = plan_folder(&ix, "A", "Z").unwrap();
        std::fs::write(dir.join("B.md"), "[[A/a]] edited elsewhere\n").unwrap();
        assert!(apply(&p).is_err());
        assert!(dir.join("A/a.md").exists() && !dir.join("Z").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
