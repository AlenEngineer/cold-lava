//! Recipes: named, reusable changes to a whole workspace ("Recipes" in docs/extensions.md). A
//! recipe runs as a plan first (every file it would change, with the diff; nothing written),
//! then `apply` writes it the way a merge does: refused if a file changed since the plan, files
//! other owners read given their new text by their owners first (`Core::relink`), and one
//! `undo` puts every file back.
//!
//! A recipe finds what it changes through the owners, never by scanning files itself: tags come
//! from the Markdown owner (`owner::markdown::tags_in`), in notes and in the Markdown pieces of
//! files other owners read. It writes into a `Draft`, which holds each file's text as the
//! recipe leaves it, so one recipe can run after another on the same draft.

use crate::frontmatter;
use crate::index::Index;
use crate::owner::given::{Change as Ask, GivenItem};
use crate::owner::markdown::{self, TagPlace};
use crate::rename::{self, Change, FileEdit, Written};
use anyhow::{Result, bail};
use indexmap::IndexMap;
use serde::Serialize;
use std::collections::HashMap;
use std::ops::Range;

/// One option a recipe takes, by name: always text.
#[derive(Serialize, Clone, Copy, Debug)]
pub struct Opt {
    pub name: &'static str,
    pub about: &'static str,
}

/// A recipe's options, by name, in the order they were given.
pub type Options = IndexMap<String, String>;

/// A named change to a whole workspace.
#[derive(Serialize, Clone, Copy, Debug)]
pub struct Recipe {
    pub id: &'static str,
    pub about: &'static str,
    pub options: &'static [Opt],
    /// What it counts as it changes them: "tag".
    pub counts: &'static str,
    /// Plans the change into the draft; reads only the index and what the draft already holds.
    #[serde(skip)]
    run: fn(&Index, &Options, &mut Draft) -> Result<()>,
}

/// The built-in recipes.
pub const BUILT_IN: &[Recipe] = &[Recipe {
    id: "rename-tag",
    about: "rename a tag everywhere: #tags, nested tags and frontmatter tags; \
            renaming into a tag that exists merges them",
    options: &[
        Opt {
            name: "from",
            about: "the tag to rename (its nested tags follow)",
        },
        Opt {
            name: "to",
            about: "its new name, written as typed",
        },
    ],
    counts: "tag",
    run: rename_tag,
}];

pub fn find(id: &str) -> Option<&'static Recipe> {
    BUILT_IN.iter().find(|r| r.id == id)
}

/// Every file a recipe would change. Nothing is written until `apply`.
#[derive(Clone, Debug)]
pub struct Plan {
    pub recipe: &'static Recipe,
    pub options: Options,
    /// Notes with their new text; files other owners read with the changes to ask them for,
    /// their new text to come from the owner (`Core::relink`, then `rename::answered`).
    pub edits: Vec<FileEdit>,
    pub warnings: Vec<String>,
}

impl Plan {
    /// How many things it changes in all (tags renamed, for `rename-tag`).
    pub fn changes(&self) -> usize {
        self.edits.iter().map(|e| e.links).sum()
    }

    /// What it changes, in words: "3 tags in 2 files".
    pub fn what(&self) -> String {
        let n = |n: usize, word: &str| match n {
            1 => format!("1 {word}"),
            n => format!("{n} {word}s"),
        };
        let files = n(self.edits.len(), "file");
        format!("{} in {files}", n(self.changes(), self.recipe.counts))
    }

    /// One line naming the recipe and its options: `rename-tag from=a to=b`.
    pub fn title(&self) -> String {
        let mut s = self.recipe.id.to_string();
        for (k, v) in &self.options {
            s.push_str(&format!(" {k}={v}"));
        }
        s
    }
}

/// A recipe that was applied: enough to undo it.
#[derive(Clone, Debug)]
pub struct Applied {
    pub title: String,
    pub files: usize,
    pub written: Written,
}

/// The change a recipe is making, file by file, before anything is written.
pub struct Draft<'a> {
    ix: &'a Index,
    /// Notes by index: the new text and how many things changed in it.
    notes: HashMap<usize, (String, usize)>,
    /// Files other owners read, by index: each piece's new text by place, and how many changed.
    owned: HashMap<usize, (IndexMap<String, String>, usize)>,
    pub warnings: Vec<String>,
}

impl<'a> Draft<'a> {
    fn new(ix: &'a Index) -> Self {
        Draft {
            ix,
            notes: HashMap::new(),
            owned: HashMap::new(),
            warnings: Vec::new(),
        }
    }

    /// Note `i`'s text as the draft has it.
    pub fn note(&self, i: usize) -> &str {
        match self.notes.get(&i) {
            Some((text, _)) => text,
            None => &self.ix.workspace.notes[i].text,
        }
    }

    /// Note `i` becomes `text`, with `n` more things changed.
    pub fn set_note(&mut self, i: usize, text: String, n: usize) {
        let e = self.notes.entry(i).or_default();
        e.0 = text;
        e.1 += n;
    }

    /// The Markdown pieces of owned file `f`, each with its place and its text as the draft has it.
    pub fn pieces(&self, f: usize) -> Vec<(String, String)> {
        let changed = self.owned.get(&f).map(|(p, _)| p);
        (self.ix.workspace.owned[f].given.items.iter())
            .filter_map(|item| match item {
                GivenItem::Markdown { place, text } => {
                    let now = changed.and_then(|c| c.get(place)).unwrap_or(text);
                    Some((place.clone(), now.clone()))
                }
                GivenItem::Typed { .. } => None,
            })
            .collect()
    }

    /// The piece at `place` of owned file `f` becomes `text`, with `n` more things changed.
    pub fn set_piece(&mut self, f: usize, place: &str, text: String, n: usize) {
        let e = self.owned.entry(f).or_default();
        e.0.insert(place.to_string(), text);
        e.1 += n;
    }

    /// The files that change, in the workspace's order.
    fn edits(self) -> (Vec<FileEdit>, Vec<String>) {
        let mut warnings = self.warnings;
        let mut edits = Vec::new();
        let mut notes: Vec<_> = self.notes.into_iter().collect();
        notes.sort_by_key(|(i, _)| *i);
        for (i, (text, n)) in notes {
            let note = &self.ix.workspace.notes[i];
            if text == note.text {
                continue;
            }
            edits.push(FileEdit {
                file: format!("{}.md", note.rel),
                path: note.path.clone(),
                old_text: note.text.clone(),
                new_text: text,
                links: n,
                changes: Vec::new(),
                owned: None,
            });
        }
        let mut owned: Vec<_> = self.owned.into_iter().collect();
        owned.sort_by_key(|(f, _)| *f);
        for (f, (pieces, n)) in owned {
            let file = &self.ix.workspace.owned[f];
            let (mut asks, mut changes) = (Vec::new(), Vec::new());
            for item in &file.given.items {
                let GivenItem::Markdown { place, text } = item else {
                    continue;
                };
                let Some(new) = pieces.get(place).filter(|new| *new != text) else {
                    continue;
                };
                changes.extend(changed_lines(text, new));
                asks.push(Ask::Text {
                    place: place.clone(),
                    text: new.clone(),
                });
            }
            if asks.is_empty() {
                continue;
            }
            match rename::owned_edit(file, asks, changes, n) {
                Ok(e) => edits.push(e),
                Err(why) => warnings.push(format!("{why}, so it isn't changed")),
            }
        }
        (edits, warnings)
    }
}

/// Plan `recipe` with `options` on the workspace. Nothing is written.
pub fn plan(ix: &Index, recipe: &'static Recipe, options: Options) -> Result<Plan> {
    for o in options.keys() {
        if !recipe.options.iter().any(|r| r.name == o) {
            bail!("{} has no option {o}", recipe.id);
        }
    }
    for o in recipe.options {
        if !options.contains_key(o.name) {
            bail!("{} needs --{} ({})", recipe.id, o.name, o.about);
        }
    }
    let mut draft = Draft::new(ix);
    (recipe.run)(ix, &options, &mut draft)?;
    let (edits, warnings) = draft.edits();
    Ok(Plan {
        recipe,
        options,
        edits,
        warnings,
    })
}

/// Carry out a plan: refuse if a file changed since it was planned, or a file another owner
/// reads has no new text from it yet, then write every file (putting back the ones written if one
/// fails).
pub fn apply(plan: &Plan) -> Result<Applied> {
    rename::check_answered(&plan.edits)?;
    rename::check_unchanged(&plan.edits, "recipe")?;
    Ok(Applied {
        title: plan.title(),
        files: plan.edits.len(),
        written: Written::all(&plan.edits)?,
    })
}

/// Undo a recipe: every file it wrote goes back to what it held. Refused, with nothing changed,
/// if one of them was edited since.
pub fn undo(applied: &Applied) -> Result<()> {
    applied.written.check("recipe")?;
    applied.written.put_back()?;
    Ok(())
}

// ---------- rename-tag ----------

fn rename_tag(ix: &Index, o: &Options, d: &mut Draft) -> Result<()> {
    let from = tag_option(o, "from")?;
    let to = tag_option(o, "to")?;
    if from == to {
        bail!("#{from} is already called that");
    }
    for i in 0..ix.workspace.notes.len() {
        if let Some((text, n)) = rename_tag_in(d.note(i), &from, &to) {
            d.set_note(i, text, n);
        }
    }
    for f in 0..ix.workspace.owned.len() {
        for (place, text) in d.pieces(f) {
            if let Some((text, n)) = rename_tag_in(&text, &from, &to) {
                d.set_piece(f, &place, text, n);
            }
        }
    }
    Ok(())
}

/// A tag given as an option: `#` and `/` around it left out, refused unless Obsidian takes it.
fn tag_option(o: &Options, name: &str) -> Result<String> {
    let given = o.get(name).map(String::as_str).unwrap_or_default();
    let tag = given.trim().trim_start_matches('#').trim_matches('/');
    if !frontmatter::is_tag(tag) || tag.split('/').any(str::is_empty) {
        bail!("{name}: {given:?} isn't a tag (letters, digits, _, - and /, not only digits)");
    }
    Ok(tag.to_string())
}

/// What tag `name` becomes when `from` is renamed to `to`: itself, or a tag nested under it
/// (`from/child` → `to/child`, not `fromage`). Case doesn't matter, as in Obsidian; `to` is
/// written as given, the rest as it was.
fn renamed(name: &str, from: &str, to: &str) -> Option<String> {
    let (n, f): (Vec<&str>, Vec<&str>) = (name.split('/').collect(), from.split('/').collect());
    let same = |a: &str, b: &str| a.to_lowercase() == b.to_lowercase();
    if n.len() < f.len() || !n.iter().zip(&f).all(|(a, b)| same(a, b)) {
        return None;
    }
    Some(
        std::iter::once(to)
            .chain(n[f.len()..].iter().copied())
            .collect::<Vec<_>>()
            .join("/"),
    )
}

/// Text with tag `from` (and the tags nested under it) renamed to `to`, and how many were, or
/// `None` when it has none. In the frontmatter's `tags`, a tag that would then be listed twice
/// is taken out.
pub fn rename_tag_in(text: &str, from: &str, to: &str) -> Option<(String, usize)> {
    let mut edits: Vec<(Range<usize>, String)> = Vec::new();
    let mut listed: HashMap<String, bool> = HashMap::new();
    let mut count = 0;
    for t in markdown::tags_in(text) {
        let name = &text[t.name.clone()];
        let new = renamed(name, from, to);
        count += new.is_some() as usize;
        let TagPlace::Frontmatter { entry, quoted } = t.place else {
            edits.extend(new.map(|new| (t.name, new)));
            continue;
        };
        let after = new.as_deref().unwrap_or(name).to_lowercase();
        let earlier = listed.get(&after).copied();
        listed.entry(after).or_insert(new.is_some());
        match (earlier, entry, new) {
            (Some(was), Some(entry), new) if was || new.is_some() => {
                edits.push((entry, String::new()))
            }
            (_, _, Some(new)) if !quoted && frontmatter::needs_quotes(&new) => {
                edits.push((t.name, format!("\"{new}\"")))
            }
            (_, _, Some(new)) => edits.push((t.name, new)),
            _ => {}
        }
    }
    if count == 0 {
        return None;
    }
    edits.sort_by_key(|(r, _)| r.start);
    let mut out = String::with_capacity(text.len());
    let mut at = 0;
    for (r, new) in edits {
        if r.start < at {
            continue;
        }
        out.push_str(&text[at..r.start]);
        out.push_str(&new);
        at = r.end;
    }
    out.push_str(&text[at..]);
    Some((out, count))
}

// ---------- diffs ----------

/// One line of a diff.
#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct DiffLine {
    /// ' ' kept, '-' removed, '+' added.
    pub op: char,
    /// Its line in the old text (kept, removed) or the new one (added), from 1.
    pub line: usize,
    pub text: String,
}

/// The lines that differ between `old` and `new`, each with up to `context` kept lines around
/// it; a hunk is a run of these, and `None` separates two.
pub fn diff(old: &str, new: &str, context: usize) -> Vec<Option<DiffLine>> {
    let (a, b): (Vec<&str>, Vec<&str>) = (old.lines().collect(), new.lines().collect());
    let head = a.iter().zip(&b).take_while(|(x, y)| x == y).count();
    let tail = (a[head..].iter().rev())
        .zip(b[head..].iter().rev())
        .take_while(|(x, y)| x == y)
        .count();
    let (am, bm) = (&a[head..a.len() - tail], &b[head..b.len() - tail]);
    // Longest common subsequence of the middle, which a recipe keeps small.
    let mut lcs = vec![vec![0u32; bm.len() + 1]; am.len() + 1];
    for i in (0..am.len()).rev() {
        for j in (0..bm.len()).rev() {
            lcs[i][j] = match am[i] == bm[j] {
                true => lcs[i + 1][j + 1] + 1,
                false => lcs[i + 1][j].max(lcs[i][j + 1]),
            };
        }
    }
    let mut all = Vec::new();
    let line = |op, line: usize, text: &str| DiffLine {
        op,
        line: line + 1,
        text: text.to_string(),
    };
    for (k, l) in a[..head].iter().enumerate() {
        all.push(line(' ', k, l));
    }
    let (mut i, mut j) = (0, 0);
    while i < am.len() || j < bm.len() {
        if i < am.len() && j < bm.len() && am[i] == bm[j] {
            all.push(line(' ', head + i, am[i]));
            (i, j) = (i + 1, j + 1);
        } else if i < am.len() && (j == bm.len() || lcs[i + 1][j] >= lcs[i][j + 1]) {
            all.push(line('-', head + i, am[i]));
            i += 1;
        } else {
            all.push(line('+', head + j, bm[j]));
            j += 1;
        }
    }
    for (k, l) in a[a.len() - tail..].iter().enumerate() {
        all.push(line(' ', a.len() - tail + k, l));
    }
    let near = |k: usize| {
        let lo = k.saturating_sub(context);
        let hi = (k + context + 1).min(all.len());
        all[lo..hi].iter().any(|d| d.op != ' ')
    };
    let mut out = Vec::new();
    let mut gap = false;
    for (k, d) in all.iter().enumerate() {
        if near(k) {
            if gap && !out.is_empty() {
                out.push(None);
            }
            gap = false;
            out.push(Some(d.clone()));
        } else {
            gap = true;
        }
    }
    out
}

/// The diff of a planned file: a note's text, or for a file another owner reads, the lines of
/// its pieces that change (their lines are the piece's).
pub fn file_diff(e: &FileEdit, context: usize) -> Vec<Option<DiffLine>> {
    if e.owned.is_none() {
        return diff(&e.old_text, &e.new_text, context);
    }
    let mut out = Vec::new();
    for (k, c) in e.changes.iter().enumerate() {
        if k > 0 {
            out.push(None);
        }
        out.extend(diff(&c.before, &c.after, context));
    }
    out
}

/// The diff as unified-diff text: `@@` before each hunk, then ` `, `-` and `+` lines.
pub fn unified(lines: &[Option<DiffLine>]) -> String {
    let mut out = String::new();
    for (k, l) in lines.iter().enumerate() {
        if let Some(d) = l {
            if k == 0 || lines[k - 1].is_none() {
                out.push_str(&format!("@@ line {} @@\n", d.line));
            }
            out.push_str(&format!("{}{}\n", d.op, d.text));
        }
    }
    out
}

/// A piece of an owned file before and after, as one change.
fn changed_lines(before: &str, after: &str) -> Option<Change> {
    (before != after).then(|| Change {
        line: 0,
        before: before.to_string(),
        after: after.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workspace;
    use std::path::{Path, PathBuf};

    fn tag(text: &str, from: &str, to: &str) -> String {
        rename_tag_in(text, from, to).map_or(text.to_string(), |(t, _)| t)
    }

    #[test]
    fn inline_and_nested() {
        assert_eq!(
            tag(
                "Lava #lava and #lava/flow, not #lavafield.",
                "lava",
                "magma"
            ),
            "Lava #magma and #magma/flow, not #lavafield."
        );
        assert_eq!(tag("#a/b/c #a/bc", "a/b", "x"), "#x/c #a/bc");
        assert_eq!(tag("(#tag) x,#tag", "tag", "t2"), "(#t2) x,#t2");
        assert_eq!(rename_tag_in("no tags #lavafield here", "lava", "x"), None);
    }

    #[test]
    fn case_is_ignored_and_to_is_kept_as_typed() {
        assert_eq!(
            tag("#Lava #LAVA/Flow #lava", "lava", "Magma"),
            "#Magma #Magma/Flow #Magma"
        );
    }

    #[test]
    fn never_in_code() {
        let text = "```\n#lava\n```\n~~~\n#lava\n~~~\n`#lava` #lava\n";
        assert_eq!(
            tag(text, "lava", "magma"),
            "```\n#lava\n```\n~~~\n#lava\n~~~\n`#lava` #magma\n"
        );
    }

    #[test]
    fn frontmatter_forms() {
        let list = "---\ntags:\n  - lava\n  - \"#lava/flow\"\n  - other\n---\n";
        assert_eq!(
            tag(list, "lava", "magma"),
            "---\ntags:\n  - magma\n  - \"#magma/flow\"\n  - other\n---\n"
        );
        let flow = "---\ntags: [lava, \"#Lava/x\", other]\n---\n";
        assert_eq!(
            tag(flow, "lava", "magma"),
            "---\ntags: [magma, \"#magma/x\", other]\n---\n"
        );
        assert_eq!(
            tag("---\ntags: lava\n---\n", "lava", "magma"),
            "---\ntags: magma\n---\n"
        );
        assert_eq!(
            tag("---\ntag: \"#lava, other\"\n---\n", "lava", "magma"),
            "---\ntag: \"#magma, other\"\n---\n"
        );
        assert_eq!(
            tag(
                "---\ntitle: lava\ntags: other\n---\nlava\n",
                "lava",
                "magma"
            ),
            "---\ntitle: lava\ntags: other\n---\nlava\n"
        );
    }

    #[test]
    fn merging_into_a_tag_deduplicates_the_list() {
        assert_eq!(
            tag("---\ntags: [lava, magma, x]\n---\n", "lava", "magma"),
            "---\ntags: [magma, x]\n---\n"
        );
        assert_eq!(
            tag("---\ntags: [magma, x, Lava]\n---\n", "lava", "magma"),
            "---\ntags: [magma, x]\n---\n"
        );
        assert_eq!(
            tag(
                "---\ntags:\n  - magma\n  - lava\n---\n#lava\n",
                "lava",
                "magma"
            ),
            "---\ntags:\n  - magma\n---\n#magma\n"
        );
        assert_eq!(
            tag("---\ntags: lava, magma\n---\n", "lava", "magma"),
            "---\ntags: magma\n---\n"
        );
        // Duplicates the rename didn't make stay.
        assert_eq!(
            tag("---\ntags: [x, x, lava]\n---\n", "lava", "magma"),
            "---\ntags: [x, x, magma]\n---\n"
        );
    }

    #[test]
    fn options_are_tags() {
        let o = |from: &str, to: &str| {
            Options::from([("from".into(), from.into()), ("to".into(), to.into())])
        };
        assert_eq!(tag_option(&o("#lava/", "x"), "from").unwrap(), "lava");
        assert!(tag_option(&o("two words", "x"), "from").is_err());
        assert!(tag_option(&o("123", "x"), "from").is_err());
        assert!(tag_option(&o("a//b", "x"), "from").is_err());
    }

    fn workspace_with(name: &str, files: &[(&str, &str)]) -> (PathBuf, Index) {
        let dir =
            std::env::temp_dir().join(format!("cold-lava-recipe-{}-{name}", std::process::id()));
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

    fn rename(ix: &Index, from: &str, to: &str) -> Result<Plan> {
        let o = Options::from([("from".into(), from.into()), ("to".into(), to.into())]);
        plan(ix, find("rename-tag").unwrap(), o)
    }

    #[test]
    fn plan_apply_undo() {
        let (dir, ix) = workspace_with(
            "apply",
            &[
                ("A.md", "---\ntags: [lava]\n---\n#lava/flow\n"),
                ("B.md", "nothing here\n"),
                ("C.md", "`#lava` only in code\n"),
            ],
        );
        let p = rename(&ix, "lava", "magma").unwrap();
        assert_eq!(p.title(), "rename-tag from=lava to=magma");
        assert_eq!(
            p.edits.iter().map(|e| e.file.as_str()).collect::<Vec<_>>(),
            ["A.md"]
        );
        assert_eq!(p.changes(), 2);
        assert_eq!(read(&dir, "A.md"), "---\ntags: [lava]\n---\n#lava/flow\n");
        let done = apply(&p).unwrap();
        assert_eq!(read(&dir, "A.md"), "---\ntags: [magma]\n---\n#magma/flow\n");
        let ix = Index::build(workspace::scan(&dir).unwrap());
        assert_eq!(ix.workspace.notes[0].tags, ["magma", "magma/flow"]);
        undo(&done).unwrap();
        assert_eq!(read(&dir, "A.md"), "---\ntags: [lava]\n---\n#lava/flow\n");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn apply_refuses_a_file_changed_since_the_plan() {
        let (dir, ix) = workspace_with("refused", &[("A.md", "#lava\n"), ("B.md", "#lava too\n")]);
        let p = rename(&ix, "lava", "magma").unwrap();
        std::fs::write(dir.join("B.md"), "#lava changed\n").unwrap();
        let err = apply(&p).unwrap_err().to_string();
        assert!(
            err.contains("B.md changed since the recipe was planned"),
            "{err}"
        );
        assert_eq!(read(&dir, "A.md"), "#lava\n");
        assert_eq!(read(&dir, "B.md"), "#lava changed\n");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn undo_refuses_a_file_changed_since() {
        let (dir, ix) = workspace_with("undo-refused", &[("A.md", "#lava\n")]);
        let done = apply(&rename(&ix, "lava", "magma").unwrap()).unwrap();
        std::fs::write(dir.join("A.md"), "#magma edited\n").unwrap();
        assert!(undo(&done).is_err());
        assert_eq!(read(&dir, "A.md"), "#magma edited\n");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn options_are_checked() {
        let (dir, ix) = workspace_with("options", &[("A.md", "#lava\n")]);
        let r = find("rename-tag").unwrap();
        assert!(plan(&ix, r, Options::from([("from".into(), "lava".into())])).is_err());
        assert!(rename(&ix, "lava", "Lava").is_ok());
        assert!(rename(&ix, "lava", "lava").is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn diffs() {
        let d = diff("a\nb\nc\nd\ne\nf\ng\n", "a\nB\nc\nd\ne\nf\n", 1);
        assert_eq!(
            unified(&d),
            "@@ line 1 @@\n a\n-b\n+B\n c\n@@ line 6 @@\n f\n-g\n"
        );
    }
}
