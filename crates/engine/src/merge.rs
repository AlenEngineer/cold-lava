//! Duplicating a note, and merging one note into another the way Obsidian's "Note composer"
//! does: the source's text goes at the end of the target, every link to the source leads to the
//! target, and the source goes to the trash.
//!
//! The links are the rename's: the merge is planned as the source moving onto the target's path
//! (`resolver::Rewriter`), with the source gone from what links can find afterwards. So
//! `[[Source#Heading|text]]` becomes `[[Target#Heading|text]]` in its own style, and links in the
//! source's text that were relative to its folder are fixed for the target's.

use crate::frontmatter::{self, TagEdit, Value};
use crate::index::Index;
use crate::rename::{self, FileEdit, Move, Written};
use crate::resolver::{Files, Rewriter};
use crate::trash::{self, Bin, Contents, Trashed};
use crate::write;
use anyhow::{Context, Result, bail};
use std::path::{Path, PathBuf};

/// Copy the file at `path` to `Name 1.md` (the first free number, as Obsidian names copies) in
/// the same folder. Returns the copy's path. Links aren't touched: it's a new file.
pub fn duplicate(path: &Path) -> Result<PathBuf> {
    let bytes = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
    let to = trash::free_path(path);
    write::create(&to, &bytes)?;
    Ok(to)
}

/// What happened to one of the source's frontmatter properties.
#[derive(Clone, Debug, PartialEq)]
pub enum Property {
    /// The target had no such key: the source's lines were copied.
    Added(String),
    /// A list both have (`tags`, `aliases`…): these items were added to the target's.
    Joined { key: String, items: Vec<String> },
    /// Both have it with different values: the target's stays.
    Kept { key: String, source: String },
    /// The source's name, added to the target's aliases so searches for it still find the note.
    Alias(String),
}

/// Everything merging one note into another changes. Nothing is written until `apply`.
#[derive(Clone, Debug)]
pub struct Merge {
    root: PathBuf,
    /// Workspace-relative, with `.md`.
    pub source: String,
    pub target: String,
    pub source_path: PathBuf,
    pub target_path: PathBuf,
    source_text: String,
    /// The target as it will be: its own links rewritten, frontmatter merged, the source's text
    /// appended. `links` and `changes` are the target's own rewritten links.
    pub target_edit: FileEdit,
    /// Lines of the source's text (frontmatter left out) added at the end.
    pub appended: usize,
    /// Links in the source's text that were changed for the target's folder (or to itself).
    pub own_links: usize,
    pub properties: Vec<Property>,
    /// The other notes, canvases and files other owners read whose links change. An owned
    /// file's new text comes from its owner (`Core::relink`) before the merge is made.
    pub edits: Vec<FileEdit>,
    /// Obsidian's bookmarks of the source, pointed at the target.
    pub bookmarks: Option<FileEdit>,
    pub warnings: Vec<String>,
}

impl Merge {
    /// Links that will lead to the target instead, outside the two notes.
    pub fn links(&self) -> usize {
        self.edits.iter().map(|e| e.links).sum::<usize>() + self.target_edit.links
    }

    fn writes(&self) -> impl Iterator<Item = &FileEdit> {
        std::iter::once(&self.target_edit)
            .chain(&self.edits)
            .chain(&self.bookmarks)
    }
}

/// A merge that was made: enough to undo it.
#[derive(Clone, Debug)]
pub struct Merged {
    pub source_path: PathBuf,
    pub target_path: PathBuf,
    pub trashed: Trashed,
    written: Written,
}

/// Work out merging note `source` into note `target`. Nothing is written.
pub fn plan(ix: &Index, source: usize, target: usize) -> Result<Merge> {
    if source == target {
        bail!("a note can't be merged into itself");
    }
    let (s, t) = (&ix.workspace.notes[source], &ix.workspace.notes[target]);
    let (source_rel, target_rel) = (format!("{}.md", s.rel), format!("{}.md", t.rel));
    let before = Files::of_workspace(ix);
    let mut rels = before.rels.clone();
    rels[source] = target_rel.clone();
    let rw = Rewriter {
        after: Files::without(rels, Some(source)),
        before,
    };
    let mut warnings = Vec::new();
    let mut edits = rename::edits(ix, &rw)?;
    edits.extend(rename::owned_edits(ix, &rw, &mut warnings));
    let mut own = |rel: &str, text: &str| match edits.iter().position(|e| e.file == rel) {
        Some(i) => {
            let e = edits.remove(i);
            (e.new_text, e.changes, e.links)
        }
        None => (text.to_string(), Vec::new(), 0),
    };
    let (source_text, _, own_links) = own(&source_rel, &s.text);
    let (target_text, changes, links) = own(&target_rel, &t.text);

    let (merged, properties) =
        merge_frontmatter(&target_text, &source_text, &s.name, &t.name, &mut warnings);
    let body = body(&source_text);
    let appended = body.lines().count();
    let new_text = append(&merged, body);

    let moves = [Move {
        from: source_rel.clone(),
        to: target_rel.clone(),
        folder: false,
        from_path: s.path.clone(),
        to_path: t.path.clone(),
    }];
    let bookmarks = rename::bookmarks_edit(&ix.workspace.root, &moves).unwrap_or_else(|e| {
        warnings.push(format!("bookmarks aren't updated: {e}"));
        None
    });
    Ok(Merge {
        root: ix.workspace.root.clone(),
        source: source_rel,
        target_edit: FileEdit {
            file: target_rel.clone(),
            path: t.path.clone(),
            old_text: t.text.clone(),
            new_text,
            links,
            changes,
            owned: None,
        },
        target: target_rel,
        source_path: s.path.clone(),
        target_path: t.path.clone(),
        source_text: s.text.clone(),
        appended,
        own_links,
        properties,
        edits,
        bookmarks,
        warnings,
    })
}

/// The source's text without its frontmatter and the blank lines that start it.
fn body(text: &str) -> &str {
    let skip = frontmatter::read(text).map_or(0, |fm| {
        (text.split_inclusive('\n').take(fm.end_line))
            .map(str::len)
            .sum()
    });
    text[skip..].trim_start_matches(['\n', '\r'])
}

/// `body` at the end of `target`, after a blank line (Obsidian's "append to the end").
fn append(target: &str, body: &str) -> String {
    if body.trim().is_empty() {
        return target.to_string();
    }
    let nl = if target.contains("\r\n") {
        "\r\n"
    } else {
        "\n"
    };
    let kept = target.trim_end_matches(['\n', '\r']);
    let mut out = match kept.is_empty() {
        true => String::new(),
        false => format!("{kept}{nl}{nl}"),
    };
    out.push_str(body);
    if !body.ends_with('\n') {
        out.push_str(nl);
    }
    out
}

const TAGS: [&str; 2] = ["tags", "tag"];
const ALIASES: [&str; 2] = ["aliases", "alias"];

/// The keys a property is known by: `tags`/`tag` and `aliases`/`alias` are one each.
fn synonyms(key: &str) -> Vec<&str> {
    match key {
        k if TAGS.contains(&k) => TAGS.to_vec(),
        k if ALIASES.contains(&k) => ALIASES.to_vec(),
        k => vec![k],
    }
}

/// The source's frontmatter merged into the target's, edited line by line: keys the target lacks
/// are copied, lists both have are joined, and a value both have differently stays the target's.
/// Then the source's name joins the target's aliases.
fn merge_frontmatter(
    target: &str,
    source: &str,
    source_name: &str,
    target_name: &str,
    warnings: &mut Vec<String>,
) -> (String, Vec<Property>) {
    let mut out = target.to_string();
    let mut props = Vec::new();
    let Some(sfm) = frontmatter::read(source) else {
        return add_alias(out, props, source_name, target_name, warnings);
    };
    for (key, value) in &sfm.properties {
        let keys = synonyms(key);
        let tfm = frontmatter::read(&out);
        let theirs = tfm.as_ref().and_then(|fm| {
            let k = keys.iter().find(|k| fm.get(k).is_some())?;
            Some((*k, fm.get(k)?.clone()))
        });
        let raw =
            |text: &str, k: &str| frontmatter::property_lines(text, k).map(|r| text[r].to_string());
        let Some((tkey, tvalue)) = theirs else {
            if let Some(lines) = raw(source, key) {
                out = add_lines(&out, &lines);
                props.push(Property::Added(key.to_string()));
            }
            continue;
        };
        let source_value = |v: &Value| match v {
            Value::Scalar(w) => w.text.to_string(),
            Value::List(l) => l.iter().map(|w| w.text).collect::<Vec<_>>().join(", "),
            Value::Empty => String::new(),
            Value::Nested => "…".to_string(),
        };
        let joinable = keys.len() > 1 || matches!(tvalue, Value::List(_) | Value::Empty);
        match (value, joinable) {
            (Value::List(_) | Value::Scalar(_), true) => {
                let mut items = Vec::new();
                for w in sfm.values(key) {
                    let item = match TAGS.contains(key) {
                        true => w.text.trim_start_matches('#'),
                        false => w.text,
                    };
                    match frontmatter::edit_list(&out, &[tkey], item, TagEdit::Add) {
                        Ok(Some(new)) => {
                            out = new;
                            items.push(item.to_string());
                        }
                        Ok(None) => {}
                        Err(why) => {
                            warnings.push(format!("{key}: {item} isn't added: {why}"));
                        }
                    }
                }
                if !items.is_empty() {
                    props.push(Property::Joined {
                        key: tkey.to_string(),
                        items,
                    });
                }
            }
            (Value::Empty, _) => {}
            _ if raw(source, key).map(|l| value_part(&l))
                == raw(&out, tkey).map(|l| value_part(&l)) => {}
            _ => props.push(Property::Kept {
                key: tkey.to_string(),
                source: source_value(value),
            }),
        }
    }
    add_alias(out, props, source_name, target_name, warnings)
}

/// A property's lines without the key, to compare two values as written.
fn value_part(lines: &str) -> String {
    let (_, rest) = lines.split_once(':').unwrap_or(("", lines));
    rest.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn add_alias(
    mut out: String,
    mut props: Vec<Property>,
    source_name: &str,
    target_name: &str,
    warnings: &mut Vec<String>,
) -> (String, Vec<Property>) {
    if source_name.to_lowercase() == target_name.to_lowercase() {
        return (out, props);
    }
    match frontmatter::edit_list(&out, &ALIASES, source_name, TagEdit::Add) {
        Ok(Some(new)) => {
            out = new;
            props.push(Property::Alias(source_name.to_string()));
        }
        Ok(None) => {}
        Err(why) => warnings.push(format!("{source_name} isn't added as an alias: {why}")),
    }
    (out, props)
}

/// `text` with property `lines` added at the end of its frontmatter (one is made if it has none).
fn add_lines(text: &str, lines: &str) -> String {
    let nl = if text.contains("\r\n") { "\r\n" } else { "\n" };
    let lines = lines.replace("\r\n", "\n").replace('\n', nl);
    let lines = match lines.ends_with('\n') {
        true => lines,
        false => format!("{lines}{nl}"),
    };
    let Some(fm) = frontmatter::read(text) else {
        return format!("---{nl}{lines}---{nl}{text}");
    };
    let close: usize = (text.split_inclusive('\n').take(fm.end_line - 1))
        .map(str::len)
        .sum();
    format!("{}{lines}{}", &text[..close], &text[close..])
}

/// Carry out a merge: refuse if any file changed since it was planned, or a file another owner
/// reads has no new text from it yet (`Core::relink`), write the target and every file whose links
/// change (putting back the ones written if one fails), then send the source to the trash.
/// `system` is the system trash (`trash::system_bin()`; tests pass their own).
pub fn apply(m: &Merge, system: Option<&Bin>) -> Result<Merged> {
    rename::check_answered(&m.edits)?;
    let source_now =
        std::fs::read(&m.source_path).with_context(|| format!("reading {}", m.source))?;
    if source_now != m.source_text.as_bytes() {
        bail!(
            "{} changed since the merge was planned; nothing was changed. Try again.",
            m.source
        );
    }
    rename::check_unchanged(m.writes(), "merge")?;
    let written = Written::all(m.writes())?;
    let contents = Contents::File(m.source_text.clone().into_bytes());
    match trash::delete(&m.root, &m.source_path, &contents, system) {
        Ok(trashed) => Ok(Merged {
            source_path: m.source_path.clone(),
            target_path: m.target_path.clone(),
            trashed,
            written,
        }),
        Err(err) => {
            let _ = written.put_back();
            Err(err)
                .with_context(|| format!("moving {} to the trash; nothing was changed", m.source))
        }
    }
}

/// Undo a merge: the source comes back from the trash and every file written goes back to what it
/// held. Refused, with nothing changed, if one of them was edited since or the source's path is
/// taken.
pub fn undo(m: &Merged) -> Result<()> {
    m.written.check("merge")?;
    if m.source_path.exists() {
        bail!(
            "{} exists again; the merge can't be undone",
            m.source_path.display()
        );
    }
    trash::restore(&m.trashed, &m.source_path)?;
    m.written.put_back().context("putting the files back")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workspace;

    fn workspace_with(name: &str, files: &[(&str, &str)]) -> (PathBuf, Index) {
        let dir =
            std::env::temp_dir().join(format!("cold-lava-merge-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        for (rel, text) in files {
            let p = dir.join("workspace").join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, text).unwrap();
        }
        let ix = Index::build(workspace::scan(&dir.join("workspace")).unwrap());
        (dir, ix)
    }

    fn read(dir: &Path, rel: &str) -> String {
        std::fs::read_to_string(dir.join("workspace").join(rel)).unwrap()
    }

    fn merge(ix: &Index, source: &str, target: &str) -> Merge {
        plan(ix, ix.by_rel(source).unwrap(), ix.by_rel(target).unwrap()).unwrap()
    }

    #[test]
    fn duplicates_take_the_next_free_number() {
        let (dir, _) = workspace_with("dup", &[("A/Lava.md", "hot"), ("Board.canvas", "{}")]);
        let lava = dir.join("workspace/A/Lava.md");
        assert_eq!(duplicate(&lava).unwrap(), dir.join("workspace/A/Lava 1.md"));
        assert_eq!(duplicate(&lava).unwrap(), dir.join("workspace/A/Lava 2.md"));
        assert_eq!(read(&dir, "A/Lava 2.md"), "hot");
        let board = dir.join("workspace/Board.canvas");
        assert_eq!(
            duplicate(&board).unwrap(),
            dir.join("workspace/Board 1.canvas")
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn appends_the_text_and_merges_the_frontmatter_line_by_line() {
        let (dir, ix) = workspace_with(
            "content",
            &[
                (
                    "Lava.md",
                    "---\ntags: [concept]\nstatus: draft # mine\ncolour: red\n---\n# Lava\nHot.\n\n",
                ),
                (
                    "Pāhoehoe.md",
                    "---\ntags:\n  - concept\n  - '#lava'\naliases: [Ropy lava]\nstatus: done\n\
                     colour: red\nsource:\n  - Book: p. 3\n---\n\n# Pāhoehoe\nSmooth.",
                ),
            ],
        );
        let m = merge(&ix, "Pāhoehoe", "Lava");
        assert_eq!(m.appended, 2);
        assert_eq!(
            m.properties,
            [
                Property::Joined {
                    key: "tags".into(),
                    items: vec!["lava".into()]
                },
                Property::Added("aliases".into()),
                Property::Kept {
                    key: "status".into(),
                    source: "done".into()
                },
                Property::Added("source".into()),
                Property::Alias("Pāhoehoe".into()),
            ],
            "colour is the same in both"
        );
        let system = Bin::Folder(dir.join("system-trash"));
        let done = apply(&m, Some(&system)).unwrap();
        assert_eq!(
            read(&dir, "Lava.md"),
            "---\ntags: [concept, lava]\nstatus: draft # mine\ncolour: red\naliases: [Ropy lava, Pāhoehoe]\n\
             source:\n  - Book: p. 3\n---\n# Lava\nHot.\n\n# Pāhoehoe\nSmooth.\n"
        );
        assert!(!dir.join("workspace/Pāhoehoe.md").exists());
        assert_eq!(done.trashed.at, dir.join("system-trash/Pāhoehoe.md"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_target_without_frontmatter_gets_one() {
        let (dir, ix) = workspace_with(
            "no-fm",
            &[
                ("Lava.md", "Hot."),
                ("Ropy: lava.md", "---\nkind: x\n---\nSmooth.\n"),
            ],
        );
        let m = merge(&ix, "Ropy: lava", "Lava");
        assert_eq!(
            m.target_edit.new_text,
            "---\nkind: x\naliases:\n  - \"Ropy: lava\"\n---\nHot.\n\nSmooth.\n"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn every_link_to_the_source_leads_to_the_target_in_its_own_style() {
        let canvas = "{\"nodes\":[\n{\"id\":\"a\",\"type\":\"file\",\"file\":\"A/Pāhoehoe.md\"},\n\
                      {\"id\":\"b\",\"type\":\"text\",\"text\":\"[[Pāhoehoe#Flow]]\"}\n],\"edges\":[]}";
        let marks = "{\n  \"items\": [\n    {\n      \"type\": \"file\",\n      \"path\": \"A/Pāhoehoe.md\",\n      \"subpath\": \"#Flow\"\n    }\n  ]\n}";
        let (dir, ix) = workspace_with(
            "links",
            &[
                (
                    "A/Pāhoehoe.md",
                    "# Flow\n[up](../Top.md) [[Sib]] [[./Sib]] [[Lava]] `[[Pāhoehoe]]`\n",
                ),
                ("A/Sib.md", "x"),
                ("B/Lava.md", "# Lava\nSee [[Pāhoehoe]].\n"),
                (
                    "Top.md",
                    "---\nrelated: \"[[A/Pāhoehoe]]\"\n---\n[[Pāhoehoe#Flow|ropy]] ![[Pāhoehoe]] \
                     [md](A/P%C4%81hoehoe.md) [rel](A/Pāhoehoe.md#Flow) [[Lava]]\n",
                ),
                ("Board.canvas", canvas),
                (".obsidian/bookmarks.json", marks),
            ],
        );
        let m = merge(&ix, "A/Pāhoehoe", "B/Lava");
        assert_eq!(m.target_edit.links, 1);
        assert_eq!(m.own_links, 1, "./Sib, relative to the source's folder");
        apply(&m, None).unwrap();
        assert_eq!(
            read(&dir, "Top.md"),
            "---\nrelated: \"[[B/Lava]]\"\n---\n[[Lava#Flow|ropy]] ![[Lava]] \
             [md](B/Lava.md) [rel](B/Lava.md#Flow) [[Lava]]\n",
            "a percent-encoded link is followed too"
        );
        assert_eq!(
            read(&dir, "B/Lava.md"),
            "---\naliases:\n  - Pāhoehoe\n---\n# Lava\nSee [[Lava]].\n\n\
             # Flow\n[up](../Top.md) [[Sib]] [[../A/Sib]] [[Lava]] `[[Pāhoehoe]]`\n"
        );
        assert_eq!(
            read(&dir, "Board.canvas"),
            canvas
                .replace("A/Pāhoehoe.md", "B/Lava.md")
                .replace("[[Pāhoehoe#", "[[Lava#")
        );
        assert_eq!(
            read(&dir, ".obsidian/bookmarks.json"),
            marks.replace("A/Pāhoehoe.md", "B/Lava.md")
        );
        assert!(dir.join("workspace/.trash/Pāhoehoe.md").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn nothing_is_written_if_a_file_changed_and_undo_puts_everything_back() {
        let files = [
            ("Lava.md", "# Lava\n"),
            ("Pāhoehoe.md", "Smooth.\n"),
            ("Kīlauea.md", "[[Pāhoehoe]]\n"),
        ];
        let (dir, ix) = workspace_with("guard", &files);
        let m = merge(&ix, "Pāhoehoe", "Lava");
        std::fs::write(dir.join("workspace/Kīlauea.md"), "[[Pāhoehoe]] edited\n").unwrap();
        assert!(apply(&m, None).is_err());
        assert_eq!(read(&dir, "Lava.md"), "# Lava\n");
        assert_eq!(read(&dir, "Pāhoehoe.md"), "Smooth.\n");
        let _ = std::fs::remove_dir_all(&dir);

        let (dir, ix) = workspace_with("undo", &files);
        let m = merge(&ix, "Pāhoehoe", "Lava");
        let done = apply(&m, None).unwrap();
        assert_eq!(read(&dir, "Kīlauea.md"), "[[Lava]]\n");
        std::fs::write(dir.join("workspace/Kīlauea.md"), "edited\n").unwrap();
        assert!(undo(&done).is_err(), "Kīlauea changed since");
        assert!(
            !dir.join("workspace/Pāhoehoe.md").exists(),
            "nothing undone"
        );
        std::fs::write(dir.join("workspace/Kīlauea.md"), "[[Lava]]\n").unwrap();
        undo(&done).unwrap();
        for (rel, text) in files {
            assert_eq!(read(&dir, rel), text);
        }
        assert!(!dir.join("workspace/.trash/Pāhoehoe.md").exists());
        assert!(plan(&ix, 0, 0).is_err(), "not into itself");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
