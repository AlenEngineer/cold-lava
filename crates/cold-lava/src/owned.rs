//! The command line's way into a workspace, which has no menu to run the extensions: the
//! workspace is opened once, as a core that knows who owns which kind of file; reads come from
//! what the owners gave before (their cache); an item edit goes through the core, and a rename
//! or an edit of a file another owner reads starts the owners it needs and waits for them.

use crate::act::{Answered, Ask};
use crate::core::{Applying, Core, NotApplied, Planned};
use crate::edit::EditError;
use crate::ext::{Effect, ExtId, Registry};
use crate::gateway::{self, Answer, Edit, Edited, Error, Op, Reason};
use crate::index::Index;
use crate::items::{Data, Item};
use crate::tasks::{NextTask, StatusType, Toggled};
use crate::{paths, workspace};
use anyhow::{Context, Result, anyhow, bail};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// The workspace at `root` as a core that knows who owns which kind of file, each owned file
/// with what its owner gave for it before (`~/.cache/cold-lava/owners/`).
pub fn open(root: &Path, registry: &Registry) -> Result<Core> {
    let mut core = Core::open(root, crate::config::configured())?;
    let kinds = registry.owned_kinds();
    if !kinds.is_empty() {
        let cache = paths::cache_dir().map(|d| d.join("cold-lava/owners"));
        core.set_owners(kinds, cache.as_deref());
    }
    Ok(core)
}

/// The workspace at `root` as the core reads it (`open`), for a command that reads no owned
/// file (`search`, `style`).
pub fn index(root: &Path, registry: &Registry) -> Result<Index> {
    Ok(open(root, registry)?.index)
}

/// The workspace at `root` as the core reads it, for a command that reads the owned files
/// too. Each one left out is named on stderr, and so is what didn't read in what an owner
/// gave (a board's card left out), as the menu's status line says it.
pub fn index_with_owned(root: &Path, registry: &Registry) -> Result<Index> {
    let core = open(root, registry)?;
    for (owner, path) in core.unread() {
        crate::say::problem!("{}", left_out(owner, path));
    }
    let index = core.index;
    for f in &index.workspace.owned {
        if let Some(message) = f.trouble() {
            crate::say::problem!("{}", couldnt_read(&f.owner, &f.rel, &message));
        }
    }
    Ok(index)
}

/// `index_with_owned`'s workspace.
pub fn workspace(root: &Path, registry: &Registry) -> Result<workspace::Workspace> {
    Ok(index_with_owned(root, registry)?.workspace)
}

/// What the status line and stderr say when `owner` couldn't read its file at `path`
/// (`core::Event::OwnerTrouble`), or what it gave didn't all read
/// (`core::Event::OwnedLeavesOut`): one sentence for both, so they say it alike.
pub fn couldnt_read(owner: &str, path: &str, message: &str) -> String {
    format!("{owner} couldn't read {path}: {message}")
}

/// Why the file at `path` shows nothing: `owner` gave nothing for it as it is now.
fn left_out(owner: &str, path: &str) -> String {
    format!(
        "{path} is left out: {owner} hasn't read it as it is now (open the workspace in \
         cold-lava, and {owner} reads it)"
    )
}

/// `item set`: the same edit the agenda makes (`Core::edit_item`), `op` on the item `id` of
/// the file at workspace-relative `rel`, refused unless it is still the version `rev` when one
/// is given. In a file another owner reads, the owner writes it, it is read back before it is
/// written, and it is refused if the file changed since its owner read it; an extension owner
/// is started to answer. A `dry` run writes nothing (`Core::would_edit`).
pub fn edit(
    root: &Path,
    registry: &mut Registry,
    rel: &str,
    id: &str,
    rev: Option<&str>,
    op: Op,
    dry: bool,
) -> Result<Edited, Error> {
    let refused = |m: String| Error::new(Reason::Refused, m);
    let mut core = open(root, registry).map_err(|e| refused(format!("{e:#}")))?;
    if let Some((owner, path)) = core.unread().find(|&(_, path)| path == rel) {
        return Err(refused(left_out(owner, path)));
    }
    let Some(item) = core.item(rel, id) else {
        // A file that doesn't read (not UTF-8 text, a hidden path) says why, not "read again".
        core.read_file(rel)?;
        let message = format!("{rel} has no item {id}: read it again");
        return Err(Error::new(Reason::Changed, message));
    };
    let edit = Edit {
        id: id.to_string(),
        rev: rev.unwrap_or(&item.rev).to_string(),
        op,
    };
    match dry {
        true => core.would_edit(&edit),
        false => made(&mut core, registry, rel, &edit),
    }
}

/// `task toggle`: the task at `line` (1-based) of the note `file` in the workspace at `root`,
/// ticked the way the agenda ticks it (`Core::edit_item`), refused unless it is still the
/// version `rev` when one is given. In a file another owner reads, a line names no task.
pub fn toggle(
    root: &Path,
    registry: &mut Registry,
    file: &Path,
    line: usize,
    rev: Option<&str>,
) -> Result<Toggled> {
    let path =
        (file.canonicalize()).with_context(|| format!("no such file: {}", file.display()))?;
    let rel = crate::rel_path::or_whole(root, &path);
    let shown = path.to_string_lossy();
    let mut core = open(root, registry)?;
    if let Some(owner) = core.owner_of(&rel) {
        bail!(
            "{} is read by {owner}, not as a note, so a line doesn't name a task in it: tick it \
             with `cold-lava item set FILE ID status x` (ids from `cold-lava items WORKSPACE \
             task --json`)",
            file.display()
        );
    }
    let door = |e: Error| anyhow::Error::from(EditError::from(e));
    let task = task_at(&core, &rel, line).map_err(door)?;
    let Some(task) = task else {
        let lines = core.read_file(&rel).map_err(door)?.text.lines().count();
        let changed = |m: String| anyhow::Error::from(EditError::Changed(m));
        if line == 0 || line > lines {
            return Err(changed(format!(
                "{shown} has no line {line} (it has {lines})"
            )));
        }
        if rev.is_some() {
            let message =
                format!("line {line} changed since the agenda was read; refresh and try again");
            return Err(changed(message));
        }
        bail!("line {line} of {shown} is not a task");
    };
    let edit = Edit {
        id: task.id.clone(),
        rev: rev.unwrap_or(&task.rev).to_string(),
        op: Op::Toggle,
    };
    let edited = made(&mut core, registry, &rel, &edit).map_err(door)?;
    let text = core.read_file(&rel).map_err(door)?.text;
    let raw = |at: usize| text.lines().nth(at - 1).unwrap_or_default().to_string();
    Ok(Toggled {
        path: shown.to_string(),
        line: edited.item.line,
        status: status_of(&edited.item).agenda_name(),
        raw: raw(edited.item.line),
        id: edited.item.rev.clone(),
        next: (edited.next.as_ref()).map(|n| NextTask {
            line: n.line,
            raw: raw(n.line),
            id: n.rev.clone(),
        }),
        warning: edited.warning,
        text: String::new(),
    })
}

/// The task that starts on `line` of the note at workspace-relative `rel`, as `items/query`
/// hands it out.
fn task_at(core: &Core, rel: &str, line: usize) -> Result<Option<Item>, Error> {
    let filter = gateway::Filter {
        note: Some(rel.to_string()),
        ..Default::default()
    };
    let select = gateway::Select {
        kind: "task".to_string(),
        filter,
    };
    let query = gateway::Query {
        select,
        page: gateway::Page::default(),
    };
    match core.query_items(&query) {
        Ok(found) => Ok(found.items.into_iter().find(|i| i.line == line)),
        // A flavour without task lists has no `task` kind, so no line is a task.
        Err(e) if e.reason == Reason::Refused => Ok(None),
        Err(e) => Err(e),
    }
}

/// A task's status type, as the gateway hands it out with every task (`status_type`).
fn status_of(task: &Item) -> StatusType {
    let Data::Typed(t) = &task.data else {
        return StatusType::NonTask;
    };
    let named = t.fields.get("status_type").and_then(|v| v.as_str());
    named
        .and_then(StatusType::named)
        .unwrap_or(StatusType::NonTask)
}

/// `edit` made by the core, on the file at `rel`: at once in a note; in a file another owner
/// reads, once its owner answers, started here. The core's news is taken afterwards, so the
/// owners' cache keeps what they gave for the file as it is now.
fn made(core: &mut Core, registry: &mut Registry, rel: &str, edit: &Edit) -> Result<Edited, Error> {
    let edited = match core.edit_item(BY, edit)? {
        Answer::Now(edited) => Ok(*edited),
        Answer::Later(ticket) => {
            let mut answer = None;
            Owners::new(registry, core).serve(core, |c| {
                let mut answers = c.take_edited().into_iter();
                answer = answers.find_map(|(_, t, a)| (t == ticket).then_some(a));
                answer.is_some()
            });
            answer.unwrap_or_else(|| {
                let message = format!("{rel}'s owner didn't answer in {} s", PATIENCE.as_secs());
                Err(Error::new(Reason::Refused, message))
            })
        }
    };
    core.poll();
    edited
}

/// Who `item set` and `task toggle` edit as.
const BY: &str = "cold-lava";

/// The owners a command line run started, to answer what the core asks them.
pub struct Owners<'a> {
    registry: &'a mut Registry,
    root: PathBuf,
    started: Vec<ExtId>,
}

/// How long a command waits for owners in all; each request also times out on its own.
const PATIENCE: Duration = Duration::from_secs(60);

impl<'a> Owners<'a> {
    pub fn new(registry: &'a mut Registry, core: &Core) -> Owners<'a> {
        Owners {
            registry,
            root: core.root().to_path_buf(),
            started: Vec::new(),
        }
    }

    /// Hand the owners what the core asks of them, starting each one the first time, until
    /// `done` or until nothing is asked any more.
    pub fn serve(&mut self, core: &mut Core, mut done: impl FnMut(&mut Core) -> bool) {
        let until = Instant::now() + PATIENCE;
        while !done(core) && Instant::now() < until {
            let reads = core.take_reads();
            let edits = core.take_file_edits();
            let asking = !(reads.is_empty() && edits.is_empty());
            for (owner, read) in reads {
                if let Some(ext) = self.start(&owner, core) {
                    self.call(ext, core, |e, host| e.read_file(read, host));
                }
            }
            for (owner, edit) in edits {
                match self.start(&owner, core) {
                    Some(ext) => self.call(ext, core, |e, host| e.edit_file(edit, host)),
                    None => {
                        let why = Err(format!("{owner} isn't installed"));
                        core.file_edited(&owner, &edit.path, &edit.rev, why);
                    }
                }
            }
            for ext in self.started.clone() {
                self.call(ext, core, |e, host| e.poll(host));
            }
            if !asking && self.started.is_empty() {
                return;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    /// The extension called `owner`, started on the workspace if it wasn't.
    fn start(&mut self, owner: &str, core: &mut Core) -> Option<ExtId> {
        let ext = self.registry.by_name(owner)?;
        if !self.started.contains(&ext) {
            self.started.push(ext);
            let root = self.root.clone();
            self.call(ext, core, |e, host| e.workspace(Some(&root), host));
        }
        Some(ext)
    }

    /// Call one extension; what it asks the menu to show goes to stderr.
    fn call(
        &mut self,
        ext: ExtId,
        core: &mut Core,
        f: impl FnOnce(&mut dyn crate::ext::Extension, &mut crate::ext::Host<'_>),
    ) {
        for effect in self.registry.call(ext, Some(core), f) {
            if let Effect::Notify { message } = effect {
                crate::say::problem!("{message}");
            }
        }
    }

    /// Have the core make `plan` (`Core::apply`), and when the owners of the files other owners
    /// read in it have to answer, start them and wait: what it gives back, or why not.
    pub fn apply<P: Planned>(&mut self, core: &mut Core, plan: &mut P) -> Result<P::Made> {
        let ticket = match core.apply(plan) {
            Applying::Now(made) => return made.map_err(said),
            Applying::Later(ticket) => ticket,
        };
        let mut answered = false;
        self.serve(core, |c| {
            answered = c.take_relinked().contains(&ticket);
            answered
        });
        if !answered {
            core.cancel_relink(ticket);
            let secs = PATIENCE.as_secs();
            bail!("the owners didn't answer in {secs} s; nothing was changed")
        }
        core.apply_answered(plan, ticket).map_err(said)
    }

    /// Asks the extension `name` about a shape it ships (`shape/region`, `shape/gesture`),
    /// starting it if it wasn't, and waits for its answer, read among the loaded `shapes`.
    pub fn ask_shape(
        &mut self,
        core: &mut Core,
        shapes: &crate::shapes::Shapes,
        name: &str,
        ask: Ask,
    ) -> Result<Answered> {
        let ext = self
            .start(name, core)
            .ok_or_else(|| anyhow!("{name} isn't installed"))?;
        let method = ask.method();
        for effect in self.registry.ask_shape(ext, Some(core), ask) {
            if let Effect::Notify { message } = effect {
                crate::say::problem!("{message}");
            }
        }
        let mut answer = None;
        self.serve(core, |c| {
            answer = c.take_shape_answers(shapes).into_iter().next();
            answer.is_some()
        });
        match answer {
            Some(a) => a.answer.map_err(|why| anyhow!("{name}: {method}: {why}")),
            None => bail!("{name} didn't answer {method} in {} s", PATIENCE.as_secs()),
        }
    }

    /// Runs `f` with the gateway of the extension `name`: its permissions, its name on edits.
    pub fn as_extension<T>(
        &mut self,
        core: &mut Core,
        name: &str,
        f: impl FnOnce(&mut crate::ext::Host<'_>) -> T,
    ) -> Result<T> {
        let ext = (self.registry.by_name(name)).ok_or_else(|| anyhow!("{name} isn't installed"))?;
        let mut out = None;
        self.call(ext, core, |_, host| out = Some(f(host)));
        out.ok_or_else(|| anyhow!("{name} wasn't called"))
    }
}

impl Drop for Owners<'_> {
    fn drop(&mut self) {
        self.registry.shutdown();
    }
}

/// What the command line says when the core didn't make a plan: a refusal ends with "nothing
/// was changed", and a failed write is said as the plan's writer said it.
fn said(not: NotApplied) -> anyhow::Error {
    match not {
        NotApplied::Refused(why) => anyhow!("{why}; nothing was changed"),
        NotApplied::Failed(e) => e,
    }
}
