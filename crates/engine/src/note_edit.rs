//! An `items/edit` of a note (docs/extensions.md, "The workspace gateway, as built"): the
//! note's Markdown owner makes its new text, the engine writes it (`write::replace`), and the
//! answer is read back from the note as the core reads it since. The core calls these
//! (`Core::edit_item`); the gateway's messages are `gateway`'s, which reads none of this.

use std::collections::HashSet;
use std::path::Path;

use crate::gateway::{Edit, Edited, Error, Op, Reason, answer_line};
use crate::items::{Item, Settings};
use crate::owner::markdown::{self, After};
use crate::workspace::Workspace;
use crate::{tasks, write};

/// An edit written to its note, before the core reads the note again.
pub(crate) struct Written {
    /// Workspace-relative, with `.md`.
    pub note: String,
    pub changed: bool,
    /// What it did, for the status line: "set due", "ticked", "anchored".
    pub what: String,
    after: After,
    warning: Option<String>,
}

/// Make the edit on disk: the note's owner makes its new text, which is written with
/// `write::replace`, refused if the note changed since it was read. No note is a
/// symlink (`workspace::first_link`), so the rename replaces the note's path itself, and a link
/// swapped in there since is replaced, never written through. An anchor's name is
/// checked against the names the whole workspace holds (`anchor_names`), which only the core
/// can read.
pub(crate) fn write(root: &Path, workspace: &Workspace, req: &Edit) -> Result<Written, Error> {
    let (note, text, made) = make(root, workspace, req, workspace.item_settings())?;
    if made.changed {
        let path = root.join(&note);
        write::replace(&path, text.as_bytes(), made.text.as_bytes())
            .map_err(|e| Error::new(Reason::of(&e), e.named_from(root)))?;
    }
    Ok(Written::of(note, made))
}

impl Written {
    /// What the Markdown owner `made` of the note at `note`, as an edit written.
    fn of(note: String, made: markdown::Made) -> Written {
        Written {
            note,
            changed: made.changed,
            what: made.what,
            after: made.after,
            warning: made.warning,
        }
    }
}

/// The edit `write` would make, nothing written: the note (workspace-relative, with `.md`), its
/// text as read, and what its owner makes of it.
fn make(
    root: &Path,
    workspace: &Workspace,
    req: &Edit,
    settings: &Settings,
) -> Result<(String, String, markdown::Made), Error> {
    let item = workspace.find_item(&req.id).ok_or_else(|| {
        let message = format!(
            "no item {} now: it changed or is gone, read it again",
            req.id
        );
        Error::new(Reason::Changed, message)
    })?;
    let note = &item.note;
    let path = root.join(note);
    let text = std::fs::read_to_string(&path)
        .map_err(|e| Error::new(Reason::Io, format!("{}: {e}", path.display())))?;
    let made = match req.op {
        Op::Anchor => markdown::anchor(note, &text, settings, req, &anchor_names(workspace)),
        _ => {
            let style = crate::style::Lazy::new(|| workspace.style());
            markdown::edit(note, &text, settings, &style, item, req, tasks::today())
        }
    }?;
    Ok((note.clone(), text, made))
}

/// Every anchor name the workspace holds (#190): each note's, as its Markdown owner reads them
/// (an anchor, a link to one, words), and each owned file's, as its owner gave them
/// (`owner::anchors`). The engine names a new anchor with none of them. Read once for an
/// anchor op, whatever number of names it tries: one pass over the workspace's text, as
/// finding the item already is.
fn anchor_names(workspace: &Workspace) -> HashSet<String> {
    let notes = (workspace.notes.iter()).flat_map(|n| markdown::anchor_names(&n.text));
    let owned = workspace.owned.iter().flat_map(crate::owner::anchors);
    notes.map(String::from).chain(owned).collect()
}

/// The answer to an edit, from the note as the core has read it since.
pub(crate) fn read_back(workspace: &Workspace, w: Written) -> Result<Edited, Error> {
    let rel = w.note.strip_suffix(".md").unwrap_or(&w.note);
    let found = (workspace.notes).binary_search_by(|n| n.rel.as_str().cmp(rel));
    let (items, text) = match found {
        Ok(i) => {
            let note = &workspace.notes[i];
            (workspace.items(note), note.text.as_str())
        }
        Err(_) => (&[][..], ""),
    };
    answer_after(w, items, text, workspace.item_settings())
}

/// What `write` and then `read_back` would answer, with nothing written: the item as the
/// note's new text reads it.
pub(crate) fn would(root: &Path, workspace: &Workspace, req: &Edit) -> Result<Edited, Error> {
    let settings = workspace.item_settings();
    let (note, _, mut made) = make(root, workspace, req, settings)?;
    let text = std::mem::take(&mut made.text);
    let items = markdown::items(&note, &text, settings);
    answer_after(Written::of(note, made), &items, &text, settings)
}

/// The answer to the edit `w`, from the note's `items` as its new `text` reads them: the item
/// it names, and a tick's next occurrence, each as `query` hands it out.
fn answer_after(
    w: Written,
    items: &[Item],
    text: &str,
    settings: &Settings,
) -> Result<Edited, Error> {
    let line_of = |at: usize| text.lines().nth(at.saturating_sub(1)).unwrap_or_default();
    let shown = |item: &Item| answer_line(settings, line_of(item.line), item).into_owned();
    let task_at = |at: usize| (items.iter()).find(|i| i.line == at && i.data.answers_to("task"));
    let (item, next) = match w.after {
        After::Set(item) => (shown(&item), None),
        After::Toggled { line, next } => {
            let item = task_at(line).ok_or_else(|| {
                let message = format!("ticked, but line {line} of {} reads as no task", w.note);
                Error::new(Reason::ReadBack, message)
            })?;
            (shown(item), next.and_then(task_at).map(shown))
        }
    };
    Ok(Edited {
        item,
        changed: w.changed,
        next,
        warning: w.warning,
    })
}
