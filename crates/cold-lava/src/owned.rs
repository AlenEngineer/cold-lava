//! Files extensions own, from the command line, which has no menu to run the extensions: reads
//! come from what the owners gave before (their cache), and a rename starts the owners it needs
//! and waits for them.

use crate::core::Core;
use crate::ext::{Effect, ExtId, Registry};
use crate::{paths, rename, workspace};
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
/// are now. Each one left out is named on stderr.
pub fn workspace(root: &Path, registry: &Registry) -> Result<workspace::Workspace> {
    if registry.owned_kinds().is_empty() {
        return workspace::scan(root);
    }
    let mut core = open(root, registry)?;
    for (owner, read) in core.take_reads() {
        eprintln!(
            "{} is left out: {owner} hasn't read it as it is now (open the workspace in \
             cold-lava, and {owner} reads it)",
            read.path
        );
    }
    Ok(core.index.workspace)
}

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
                eprintln!("{message}");
            }
        }
    }

    /// Ask the owners of the files other owners read among `edits` for their new texts, and
    /// wait for them: the edits get them, or the first refusal is the error.
    pub fn relink(&mut self, core: &mut Core, edits: &mut [rename::FileEdit]) -> Result<()> {
        let ticket = core.relink(edits).map_err(|why| anyhow!("{why}"))?;
        let Some(ticket) = ticket else {
            return Ok(());
        };
        let mut answer = None;
        self.serve(core, |c| {
            answer = (c.take_relinked().into_iter()).find_map(|(t, a)| (t == ticket).then_some(a));
            answer.is_some()
        });
        match answer {
            Some(Ok(answers)) => {
                rename::answered(edits, answers);
                Ok(())
            }
            Some(Err(why)) => bail!("{why}"),
            None => {
                core.cancel_relink(ticket);
                bail!("the owners didn't answer in {} s", PATIENCE.as_secs())
            }
        }
    }
}

impl Drop for Owners<'_> {
    fn drop(&mut self) {
        self.registry.shutdown();
    }
}
