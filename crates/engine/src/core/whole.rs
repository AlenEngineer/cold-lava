//! Changes to the whole workspace, made in one call: a rename or a move, a merge, a recipe. Each
//! is a plan first (`rename::plan`, `merge::plan`, `recipe::plan`), shown before anything is
//! written. Once it is confirmed, `Core::apply` makes it: the owners of its files that other
//! owners read give their new texts and read them back (`Core::relink`), only then is it
//! written, and what the owners read is kept for each file where it is now
//! (`Core::relinked`). When an extension has to answer, nothing is written until it has: the
//! frontend hands its answer (`Core::take_relinked`) to `Core::apply_answered`, which makes the
//! plan the same way.
//!
//! Reading the workspace again afterwards is still the caller's, because the callers read it
//! again three ways: the menu rescans, the command line only saves what the owners read
//! (`poll`), and `Core::run_recipe` reads the files it wrote. One way for all would change
//! what subscribers hear (#270, B5).

use super::Core;
use crate::gateway::Ticket;
use crate::merge::{self, Merge, Merged};
use crate::read;
use crate::recipe::{self, Applied};
use crate::rename::{self, FileEdit};
use crate::trash::Bin;
use std::path::{Path, PathBuf};

/// A plan the core makes (`Core::apply`).
pub trait Planned {
    /// What it gives back once made: what undoes a merge or a recipe.
    type Made;

    /// The files whose text it changes, each with its new text, or, for a file another owner
    /// reads, what to ask that owner for (`edits_mut` takes its answer).
    fn edits(&self) -> &[FileEdit];

    fn edits_mut(&mut self) -> &mut [FileEdit];

    /// Where the file at `path` is once it is made.
    fn moved(&self, path: &Path) -> PathBuf {
        path.to_path_buf()
    }

    /// Write it: refused when a file changed since it was planned, or a file another owner
    /// reads has no new text from it yet.
    fn write(&self, door: Door<'_>) -> anyhow::Result<Self::Made>;
}

/// What writing a plan takes from the core: the system trash a merge's source goes to. Nothing
/// outside this module can make one, so a plan is written by the core alone (`Core::apply`).
pub struct Door<'a> {
    system: Option<&'a Bin>,
}

impl Planned for rename::Plan {
    type Made = ();

    fn edits(&self) -> &[FileEdit] {
        &self.edits
    }

    fn edits_mut(&mut self) -> &mut [FileEdit] {
        &mut self.edits
    }

    fn moved(&self, path: &Path) -> PathBuf {
        rename::Plan::moved(self, path)
    }

    fn write(&self, _: Door<'_>) -> anyhow::Result<()> {
        rename::apply(self)
    }
}

impl Planned for Merge {
    type Made = Merged;

    fn edits(&self) -> &[FileEdit] {
        &self.edits
    }

    fn edits_mut(&mut self) -> &mut [FileEdit] {
        &mut self.edits
    }

    fn write(&self, door: Door<'_>) -> anyhow::Result<Merged> {
        merge::apply(self, door.system)
    }
}

impl Planned for recipe::Plan {
    type Made = Applied;

    fn edits(&self) -> &[FileEdit] {
        &self.edits
    }

    fn edits_mut(&mut self) -> &mut [FileEdit] {
        &mut self.edits
    }

    fn write(&self, _: Door<'_>) -> anyhow::Result<Applied> {
        recipe::apply(self)
    }
}

/// How `Core::apply` went.
#[derive(Debug)]
pub enum Applying<T> {
    /// Made, with what it gives back, or not made.
    Now(Result<T, NotApplied>),
    /// Nothing is written yet: an extension has to give a file's new text. Its answer comes from
    /// `Core::take_relinked` under the ticket, for `Core::apply_answered`; `Core::cancel_relink`
    /// stops the wait.
    Later(Ticket),
}

/// Why a plan wasn't made.
#[derive(Debug)]
pub enum NotApplied {
    /// Nothing was written: an owner refused or didn't answer, or one of the files other owners
    /// read changed since it was planned or has an edit waiting (`Core::relink`'s words).
    Refused(String),
    /// Writing it failed, in the words of `rename::apply`, `merge::apply` or `recipe::apply`.
    Failed(anyhow::Error),
}

impl Core {
    /// Make `plan`, confirmed: the owners of its files that other owners read are asked for
    /// their new texts, then it is written, then what they read in those texts is kept. Nothing
    /// is written until every owner has answered: when an extension has to, it waits under a
    /// ticket (`Applying::Later`). The engine's own owners answer inside the call.
    pub fn apply<P: Planned>(&mut self, plan: &mut P) -> Applying<P::Made> {
        match self.relink(plan.edits_mut()) {
            Err(why) => Applying::Now(Err(NotApplied::Refused(why))),
            Ok(Some(ticket)) => Applying::Later(ticket),
            Ok(None) => Applying::Now(self.write_planned(plan)),
        }
    }

    /// Make `plan`, which waited for its owners (`apply`), with their `answer`
    /// (`take_relinked`): the new texts they gave, or the first refusal, and then nothing is
    /// written.
    pub fn apply_answered<P: Planned>(
        &mut self,
        plan: &mut P,
        answer: Result<Vec<FileEdit>, String>,
    ) -> Result<P::Made, NotApplied> {
        let answers = answer.map_err(NotApplied::Refused)?;
        rename::answered(plan.edits_mut(), answers);
        self.write_planned(plan)
    }

    /// Write `plan`, whose owners have given every new text, and keep what they read in them.
    fn write_planned<P: Planned>(&mut self, plan: &P) -> Result<P::Made, NotApplied> {
        let door = Door {
            system: self.system_trash.as_ref(),
        };
        let made = plan.write(door).map_err(NotApplied::Failed)?;
        self.relinked(plan.edits(), |path| plan.moved(path));
        Ok(made)
    }

    /// Before a rename, merge or recipe writes anything: ask the owner of each file among `edits`
    /// that another owner reads for its new text (`file/edit`), and have the owner read it back.
    /// `None` when `edits` are ready: there was nothing to ask, or every owner answered inside
    /// the call (the engine's own do) and `edits` have their new texts. Else the answer comes
    /// from `take_relinked` under the ticket: every owned file's edit with its new text, or the
    /// first owner's refusal, after which nothing else is kept. Refused at once when one of them
    /// changed since it was planned, has an edit waiting, its change would leave out something
    /// its owner gave (`given::check_change`: a link target past its bound), or an owner in the
    /// engine refuses.
    pub(crate) fn relink(
        &mut self,
        edits: &mut [crate::rename::FileEdit],
    ) -> Result<Option<Ticket>, String> {
        let settings = self.index.workspace.item_settings();
        let planned = |e: &crate::rename::FileEdit| {
            let seen = read::Seen::kept(&e.path, e.old_text.as_str());
            seen.unchanged().is_ok()
        };
        self.edits.relink(edits, planned, settings)
    }

    /// Renames, merges and recipes whose owners have all answered (`apply`), or one refused:
    /// each answer is for `apply_answered`.
    pub fn take_relinked(&mut self) -> Vec<(Ticket, Result<Vec<crate::rename::FileEdit>, String>)> {
        self.edits.take_relinked()
    }

    /// Stop waiting for the owners of the rename or merge `ticket`. Their answers are dropped.
    pub fn cancel_relink(&mut self, ticket: Ticket) {
        self.edits.cancel_relink(ticket);
    }

    /// After a rename or merge with `edits` was made, before reading the workspace again: what
    /// the owners read in their files' new texts, at `moved` (where each file is now), is kept as
    /// their answer for that version, so they aren't asked again.
    pub(super) fn relinked(
        &mut self,
        edits: &[crate::rename::FileEdit],
        moved: impl Fn(&Path) -> PathBuf,
    ) {
        for e in edits {
            let Some((o, after)) = e.owned.as_ref().and_then(|o| Some((o, o.after.as_ref()?)))
            else {
                continue;
            };
            let path = moved(&e.path);
            let rel = path.strip_prefix(self.root()).unwrap_or(&path);
            let rel = rel.to_string_lossy().replace('\\', "/");
            let rev = read::rev(e.new_text.as_bytes());
            self.owners.answered(&o.owner, &rel, &rev, Some(after));
        }
    }
}
