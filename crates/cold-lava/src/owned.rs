//! Files extensions own, from the command line, which has no menu to run the extensions: reads
//! come from what the owners gave before (their cache), and a rename starts the owners it needs
//! and waits for them.

use crate::act::{Answered, Ask};
use crate::core::{Applying, Core, NotApplied, Planned};
use crate::ext::{Effect, ExtId, Registry};
use crate::gateway::{Answer, Edit, Edited, Error, Op, Reason};
use crate::{paths, workspace};
use anyhow::{Result, anyhow, bail};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// The workspace at `root` as a core that knows who owns which kind of file, each owned file
/// with what its owner gave for it before (`~/.cache/cold-lava/owners/`).
pub fn open(root: &Path, registry: &Registry) -> Result<Core> {
    let mut core = Core::open(root)?;
    let kinds = registry.owned_kinds();
    if !kinds.is_empty() {
        let cache = paths::cache_dir().map(|d| d.join("cold-lava/owners"));
        core.set_owners(kinds, cache.as_deref());
    }
    Ok(core)
}

/// The workspace at `root` with the owned files whose owners gave something for them as they
/// are now. Each one left out is named on stderr, and so is what didn't read in what an
/// owner gave (a board's card left out), as the menu's status line says it.
pub fn workspace(root: &Path, registry: &Registry) -> Result<workspace::Workspace> {
    let workspace = match registry.owned_kinds().is_empty() {
        true => workspace::scan(root)?,
        false => {
            let core = open(root, registry)?;
            for (owner, path) in core.unread() {
                crate::say::problem!(
                    "{path} is left out: {owner} hasn't read it as it is now (open the workspace \
                     in cold-lava, and {owner} reads it)"
                );
            }
            core.index.workspace
        }
    };
    for f in &workspace.owned {
        if let Some(message) = f.trouble() {
            crate::say::problem!("{}", couldnt_read(&f.owner, &f.rel, &message));
        }
    }
    Ok(workspace)
}

/// What the status line and stderr say when `owner` couldn't read its file at `path`
/// (`core::Event::OwnerTrouble`), or what it gave didn't all read
/// (`core::Event::OwnedLeavesOut`): one sentence for both, so they say it alike.
pub fn couldnt_read(owner: &str, path: &str, message: &str) -> String {
    format!("{owner} couldn't read {path}: {message}")
}

/// The owner of the file at workspace-relative `rel`, when it isn't a note: the engine's
/// built-in one (a canvas) or an installed extension.
pub fn owner_of(registry: &Registry, rel: &str) -> Option<String> {
    let built_in = crate::owner::built_in(rel).map(String::from);
    built_in.or_else(|| {
        let (ext, _) = registry.owner_of(rel)?;
        Some(registry.manifest(ext).name.clone())
    })
}

/// `item set` on a file another owner reads: the same edit the agenda makes (`Core::edit_item`),
/// so the owner writes it, it is read back before it is written, and it is refused if the file
/// changed since its owner read it. An extension owner is started to answer.
pub fn set_field(
    root: &Path,
    registry: &mut Registry,
    rel: &str,
    id: &str,
    rev: Option<&str>,
    field: &str,
    value: serde_json::Value,
) -> Result<Edited, Error> {
    let refused = |m: String| Error::new(Reason::Refused, m);
    let mut core = open(root, registry).map_err(|e| refused(format!("{e:#}")))?;
    let file = (core.index.workspace.owned.iter()).find(|f| f.rel == rel);
    let Some(file) = file.filter(|f| f.rev.is_some()) else {
        let owner = owner_of(registry, rel).unwrap_or_default();
        return Err(refused(format!(
            "{rel} is left out: {owner} hasn't read it as it is now (open the workspace in \
             cold-lava, and {owner} reads it)"
        )));
    };
    let item = (file.items.iter()).find(|i| i.id == id).ok_or_else(|| {
        let message = format!("{rel} has no item {id}: read it again");
        Error::new(Reason::Changed, message)
    })?;
    let edit = Edit {
        id: id.to_string(),
        rev: rev.unwrap_or(&item.rev).to_string(),
        op: Op::SetField {
            field: field.to_string(),
            value,
        },
    };
    let ticket = match core.edit_item(BY, &edit)? {
        Answer::Now(edited) => return Ok(*edited),
        Answer::Later(ticket) => ticket,
    };
    let mut answer = None;
    let mut take = |c: &mut Core| {
        answer = (c.take_edited().into_iter()).find_map(|(_, t, a)| (t == ticket).then_some(a));
        answer.is_some()
    };
    if !take(&mut core) {
        Owners::new(registry, &core).serve(&mut core, take);
    }
    answer.unwrap_or_else(|| {
        let message = format!("{rel}'s owner didn't answer in {} s", PATIENCE.as_secs());
        Err(refused(message))
    })
}

/// Who `item set` edits as.
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
        let mut answer = None;
        self.serve(core, |c| {
            answer = (c.take_relinked().into_iter()).find_map(|(t, a)| (t == ticket).then_some(a));
            answer.is_some()
        });
        match answer {
            Some(answer) => core.apply_answered(plan, answer).map_err(said),
            None => {
                core.cancel_relink(ticket);
                let secs = PATIENCE.as_secs();
                bail!("the owners didn't answer in {secs} s; nothing was changed")
            }
        }
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
