//! Renaming and moving notes with every link to them updated, like Obsidian's
//! "Automatically update internal links".
//!
//! `plan` works out every change without touching anything; `apply` checks nothing changed
//! since, then moves the note and rewrites the other files atomically, one by one.

use crate::index::Index;
use anyhow::{Context, Result, bail};
use regex::Regex;
use serde::Serialize;
use std::path::PathBuf;
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
    pub from: String,
    pub to: String,
    #[serde(skip)]
    pub from_path: PathBuf,
    #[serde(skip)]
    pub to_path: PathBuf,
    /// Other files whose links change (the moved note's own edits are included, under `from`).
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
}

/// Where `new` puts the note at `old_rel` (both vault-relative, `.md` included in the result):
/// a bare name renames it in its folder, `folder/` moves it keeping its name, anything else is
/// a full vault path.
pub fn target_rel(old_rel: &str, new: &str) -> Result<String> {
    let new = new.trim().replace('\\', "/");
    let new = new.trim_start_matches('/');
    let old_rel = old_rel.trim_end_matches(".md");
    let (old_dir, old_name) = old_rel.rsplit_once('/').unwrap_or(("", old_rel));
    let rel = if new.is_empty() {
        bail!("the new name is empty");
    } else if let Some(dir) = new.strip_suffix('/') {
        join(dir.trim_matches('/'), old_name)
    } else if new.contains('/') {
        new.trim_end_matches(".md").to_string()
    } else {
        join(old_dir, new.trim_end_matches(".md"))
    };
    if rel
        .split('/')
        .any(|c| c.is_empty() || c == "." || c == ".." || c.starts_with('.'))
    {
        bail!("not a valid note path inside the vault: {new}");
    }
    if rel
        .chars()
        .any(|c| matches!(c, '[' | ']' | '|' | '#' | '^'))
    {
        bail!("note names can't contain [ ] | # or ^ (links couldn't point to them)");
    }
    Ok(format!("{rel}.md"))
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

/// Which note a link written in a note in `dir` points to (Obsidian's rules, plus `./` and
/// `../` relative paths).
pub fn resolve_from(ix: &Index, dir: &str, raw: &str) -> Option<usize> {
    let raw = raw.trim().replace("%20", " ");
    let raw = raw.trim_end_matches(".md");
    if raw.starts_with("./") || raw.starts_with("../") {
        return ix.by_rel(&normalize(&join(dir, raw))?);
    }
    if raw.contains('/') {
        if let Some(i) = ix.by_rel(raw.trim_start_matches('/')) {
            return Some(i);
        }
        if let Some(i) = normalize(&join(dir, raw)).and_then(|p| ix.by_rel(&p)) {
            return Some(i);
        }
    }
    ix.resolver.resolve(raw)
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
        // [text](path.md#heading) — path is group 1.
        mdlink: Regex::new(r"\]\(([^)\s]+?\.md)((?:#[^)]*)?)\)").unwrap(),
        inline_code: Regex::new(r"`[^`\n]*`").unwrap(),
        canvas_field: Regex::new(r#""(file|text)"(\s*:\s*)("(?:[^"\\]|\\.)*")"#).unwrap(),
    })
}

/// What a link should become, given how it was written.
struct Rewriter<'a> {
    ix: &'a Index,
    note: usize,
    new_rel: String,
    /// Another note already has the new file name: write links as paths.
    ambiguous: bool,
}

impl Rewriter<'_> {
    /// `dir_then` is the linking file's folder for resolving (before the move), `dir_now` the
    /// folder to compute relative paths from (after the move; differs only for the moved note).
    fn wikilink(&self, raw: &str, dir_then: &str, dir_now: &str) -> Option<String> {
        let target = resolve_from(self.ix, dir_then, raw)?;
        let had_md = raw.trim_end().ends_with(".md");
        let new_rel = self.new_rel.trim_end_matches(".md");
        let rel = |s: String| if had_md { format!("{s}.md") } else { s };
        if target == self.note {
            let name = new_rel.rsplit('/').next().unwrap_or(new_rel);
            let out = if raw.starts_with("./") || raw.starts_with("../") {
                rel(relative(dir_now, new_rel))
            } else if raw.contains('/') || self.ambiguous {
                rel(new_rel.to_string())
            } else {
                rel(name.to_string())
            };
            return (out != raw).then_some(out);
        }
        // The moved note's own relative links to other notes.
        if dir_then != dir_now && (raw.starts_with("./") || raw.starts_with("../")) {
            let other = self.ix.vault.notes[target].rel.clone();
            let out = rel(relative(dir_now, &other));
            return (out != raw).then_some(out);
        }
        None
    }

    fn mdlink(&self, raw: &str, dir_then: &str, dir_now: &str) -> Option<String> {
        let target = resolve_from(self.ix, dir_then, raw)?;
        let decoded = raw.replace("%20", " ");
        // Vault-absolute ("Folder/Note.md", "/Folder/Note.md") or relative to the note?
        let absolute = decoded.starts_with('/')
            || (!decoded.starts_with('.')
                && self.ix.by_rel(decoded.trim_start_matches('/')) == Some(target)
                && !dir_then.is_empty());
        let dest = if target == self.note {
            self.new_rel.trim_end_matches(".md").to_string()
        } else if dir_then != dir_now && !absolute {
            self.ix.vault.notes[target].rel.clone()
        } else {
            return None;
        };
        let path = if absolute {
            format!(
                "{}{dest}.md",
                if decoded.starts_with('/') { "/" } else { "" }
            )
        } else {
            format!("{}.md", relative(dir_now, &dest))
        };
        let out = path.replace(' ', "%20");
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
                && let Some(new) = self.wikilink(m.as_str(), dir_then, dir_now)
            {
                reps.push((m.start(), m.end(), new));
            }
        }
        for c in p.mdlink.captures_iter(body) {
            let m = c.get(1).unwrap();
            if !in_code(m.start())
                && !m.as_str().contains("://")
                && let Some(new) = self.mdlink(m.as_str(), dir_then, dir_now)
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
    let new_name = new_rel
        .trim_end_matches(".md")
        .rsplit('/')
        .next()
        .unwrap_or("")
        .to_string();
    let ambiguous = ix
        .vault
        .notes
        .iter()
        .enumerate()
        .any(|(i, other)| i != note && other.name.to_lowercase() == new_name.to_lowercase());
    let mut warnings = Vec::new();
    if ambiguous {
        warnings.push(format!(
            "another note is also called \"{new_name}\": links to this one are written as full paths, \
             and existing [[{new_name}]] links may now point to whichever is closer to the vault root"
        ));
    }
    let rw = Rewriter {
        ix,
        note,
        new_rel: new_rel.clone(),
        ambiguous,
    };
    let new_dir = dir_of(new_rel.trim_end_matches(".md")).to_string();

    let mut edits = Vec::new();
    for (i, other) in ix.vault.notes.iter().enumerate() {
        let dir = dir_of(&other.rel).to_string();
        let dir_now = if i == note {
            new_dir.clone()
        } else {
            dir.clone()
        };
        let (new_text, changes, links) = rw.note_text(&other.text, &dir, &dir_now);
        if links > 0 {
            edits.push(FileEdit {
                file: format!("{}.md", other.rel),
                path: other.path.clone(),
                old_text: other.text.clone(),
                new_text,
                links,
                changes,
            });
        }
    }
    edits.extend(canvas_edits(ix, &rw, &from_rel)?);
    Ok(Plan {
        from: from_rel,
        to: new_rel,
        from_path: n.path.clone(),
        to_path,
        edits,
        warnings,
    })
}

/// Obsidian Canvas files: note cards pointing at the note, and links in text cards.
/// Edited as text, so the rest of the file stays exactly as Obsidian wrote it.
fn canvas_edits(ix: &Index, rw: &Rewriter, from_rel: &str) -> Result<Vec<FileEdit>> {
    let root = &ix.vault.root;
    let mut edits = Vec::new();
    let walker = WalkDir::new(root)
        .into_iter()
        .filter_entry(|e| e.depth() == 0 || !e.file_name().to_string_lossy().starts_with('.'));
    for e in walker.filter_map(|e| e.ok()) {
        if !e.file_type().is_file() || e.path().extension().is_none_or(|x| x != "canvas") {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(e.path()) else {
            continue;
        };
        let rel = e
            .path()
            .strip_prefix(root)
            .unwrap_or(e.path())
            .to_string_lossy()
            .replace('\\', "/");
        let dir = dir_of(&rel).to_string();
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
                "file" if value.to_lowercase() == from_rel.to_lowercase() => {
                    links += 1;
                    Some(rw.new_rel.clone())
                }
                "text" => {
                    let (t, _, n) = rw.note_text(&value, &dir, &dir);
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
        if links > 0 {
            out.push_str(&text[last..]);
            edits.push(FileEdit {
                file: rel,
                path: e.path().to_path_buf(),
                old_text: text,
                new_text: out,
                links,
                changes,
            });
        }
    }
    Ok(edits)
}

/// Carry out a plan: refuse if any file changed since it was made, then move the note and
/// write each changed file atomically.
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
    if let Some(dir) = plan.to_path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::rename(&plan.from_path, &plan.to_path)
        .with_context(|| format!("moving {} to {}", plan.from, plan.to))?;
    let mut failed = Vec::new();
    for e in &plan.edits {
        // The moved note's own edit goes to its new place.
        let path = if e.path == plan.from_path {
            &plan.to_path
        } else {
            &e.path
        };
        let tmp = path.with_extension("cold-lava-tmp");
        let res = std::fs::write(&tmp, &e.new_text).and_then(|_| std::fs::rename(&tmp, path));
        if let Err(err) = res {
            failed.push(format!("{} ({err})", e.file));
        }
    }
    if !failed.is_empty() {
        bail!(
            "moved the note, but couldn't update links in: {}",
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
}
