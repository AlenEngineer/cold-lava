//! Changes to the whole workspace, made in one call: a rename or a move, a merge, a recipe. Each
//! is a plan first (`rename::plan`, `merge::plan`, `recipe::plan`), shown before anything is
//! written. Once it is confirmed, `Core::apply` makes it: the owners of its files that other
//! owners read give their new texts and read them back (`Core::relink`), only then is it
//! written, and what the owners read is kept for each file where it is now, at the version they
//! read back (`Core::relinked`). When an extension has to answer, nothing is written until it
//! has: `Core::take_relinked` says the ticket is ready, and `Core::apply_answered` makes the
//! plan the same way. What the owners answered comes from the edit desk, never from the caller,
//! which keeps only the plan, to show it, and the ticket.
//!
//! Reading the workspace again afterwards is still the caller's, because the callers read it
//! again three ways: the menu rescans, the command line only saves what the owners read
//! (`poll`), and `Core::run_recipe` reads the files it wrote. One way for all would change
//! what subscribers hear (#270, B5).

use super::Core;
use crate::gateway::Ticket;
use crate::merge::{self, Merge, Merged};
use crate::owner::edits::{Answers, Relinked};
use crate::read;
use crate::recipe::{self, Applied};
use crate::rename::{self, FileEdit};
use crate::trash::Bin;
use std::path::{Path, PathBuf};

/// A plan the core makes (`Core::apply`): a rename, a merge or a recipe, and nothing else. The
/// trait is sealed (`Sealed`, which nothing outside this module can name), so no frontend's type
/// can be one, and `Core::apply` writes only the engine's plans, the engine's way.
pub trait Planned: Sealed {
    /// What it gives back once made: what undoes a merge or a recipe.
    type Made;

    /// Write it: refused when a file changed since it was planned, or a file another owner
    /// reads has no new text from it yet.
    fn write(&self, door: Door<'_>) -> anyhow::Result<Self::Made>;
}

mod sealed {
    use crate::rename::FileEdit;
    use std::path::{Path, PathBuf};

    /// What the core reads of a plan (`super::Planned`) while it makes it. It must never be
    /// exported: that it can't be named outside this module is what closes `Planned`.
    pub trait Sealed {
        /// The files whose text it changes, each with its new text, or, for a file another
        /// owner reads, what to ask that owner for (`edits_mut` takes its answer).
        fn edits(&self) -> &[FileEdit];

        fn edits_mut(&mut self) -> &mut [FileEdit];

        /// Where the file at `path` is once it is made.
        fn moved(&self, path: &Path) -> PathBuf {
            path.to_path_buf()
        }
    }
}

use sealed::Sealed;

/// What writing a plan takes from the core: the system trash a merge's source goes to. Nothing
/// outside this module can make one, so only the core calls `Planned::write`. The seal
/// (`Sealed`), not this, is what keeps a frontend's type from being a plan.
pub struct Door<'a> {
    system: Option<&'a Bin>,
}

impl Sealed for rename::Plan {
    fn edits(&self) -> &[FileEdit] {
        &self.edits
    }

    fn edits_mut(&mut self) -> &mut [FileEdit] {
        &mut self.edits
    }

    fn moved(&self, path: &Path) -> PathBuf {
        rename::Plan::moved(self, path)
    }
}

impl Planned for rename::Plan {
    type Made = ();

    fn write(&self, _: Door<'_>) -> anyhow::Result<()> {
        rename::apply(self)
    }
}

impl Sealed for Merge {
    fn edits(&self) -> &[FileEdit] {
        &self.edits
    }

    fn edits_mut(&mut self) -> &mut [FileEdit] {
        &mut self.edits
    }
}

impl Planned for Merge {
    type Made = Merged;

    fn write(&self, door: Door<'_>) -> anyhow::Result<Merged> {
        merge::apply(self, door.system)
    }
}

impl Sealed for recipe::Plan {
    fn edits(&self) -> &[FileEdit] {
        &self.edits
    }

    fn edits_mut(&mut self) -> &mut [FileEdit] {
        &mut self.edits
    }
}

impl Planned for recipe::Plan {
    type Made = Applied;

    fn write(&self, _: Door<'_>) -> anyhow::Result<Applied> {
        recipe::apply(self)
    }
}

/// How `Core::apply` went.
#[derive(Debug)]
pub enum Applying<T> {
    /// Made, with what it gives back, or not made.
    Now(Result<T, NotApplied>),
    /// Nothing is written yet: an extension has to give a file's new text. `Core::take_relinked`
    /// says when it has, for `Core::apply_answered`; `Core::cancel_relink` stops the wait.
    Later(Ticket),
}

/// Why a plan wasn't made.
#[derive(Debug)]
pub enum NotApplied {
    /// Nothing was written: an owner refused or didn't answer, or one of the files other owners
    /// read changed since it was planned or has an edit waiting (`Core::relink`'s words), or
    /// their answer isn't in (`Core::apply_answered`).
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
            Ok(Answers::Later(ticket)) => Applying::Later(ticket),
            Ok(Answers::Now(answers)) => Applying::Now(self.write_planned(plan, &answers)),
        }
    }

    /// Make `plan`, which waited for its owners under `ticket` (`apply`), once `take_relinked`
    /// says they answered: with the new texts they gave, which the core takes from the edit
    /// desk. Refused when an owner refused, their answer isn't in (still to come, cancelled,
    /// or dropped by a later `take_relinked`), or a file of `plan` other owners read has none
    /// (one added after they were asked), and then nothing is written.
    pub fn apply_answered<P: Planned>(
        &mut self,
        plan: &mut P,
        ticket: Ticket,
    ) -> Result<P::Made, NotApplied> {
        let answers = self.edits.answered_into(ticket, plan.edits_mut());
        let answers = answers.map_err(NotApplied::Refused)?;
        self.write_planned(plan, &answers)
    }

    /// Renames, merges and recipes whose owners have all answered (`apply`), or one refused,
    /// each said once: its ticket is for `apply_answered` until the next call, which drops the
    /// answer, as `cancel_relink` does.
    pub fn take_relinked(&mut self) -> Vec<Ticket> {
        self.edits.take_relinked()
    }

    /// Stop waiting for the owners of the rename, merge or recipe `ticket`. Their answers are
    /// dropped.
    pub fn cancel_relink(&mut self, ticket: Ticket) {
        self.edits.cancel_relink(ticket);
    }

    /// Write `plan`, whose owners have given every new text (`answers`), and keep what they read
    /// in them.
    fn write_planned<P: Planned>(
        &mut self,
        plan: &P,
        answers: &[Relinked],
    ) -> Result<P::Made, NotApplied> {
        let door = Door {
            system: self.system_trash.as_ref(),
        };
        let made = plan.write(door).map_err(NotApplied::Failed)?;
        self.relinked(plan.edits(), answers, |path| plan.moved(path));
        Ok(made)
    }

    /// Before a rename, merge or recipe writes anything: ask the owner of each file among `edits`
    /// that another owner reads for its new text (`file/edit`), and have the owner read it back
    /// (`Edits::relink`). Refused at once when one of them changed since it was planned, has an
    /// edit waiting, its change would leave out something its owner gave (`given::check_change`:
    /// a link target past its bound), or an owner in the engine refuses.
    pub(crate) fn relink(&mut self, edits: &mut [FileEdit]) -> Result<Answers, String> {
        let settings = self.index.workspace.item_settings();
        let planned = |e: &FileEdit| {
            let seen = read::Seen::kept(&e.path, e.old_text.as_str());
            seen.unchanged().is_ok()
        };
        self.edits.relink(edits, planned, settings)
    }

    /// After a rename, merge or recipe was made, before reading the workspace again: what the
    /// owners read in their files' new texts (`answers`) is kept as their answer for the version
    /// each read back, where each of `edits` is now (`moved`), so they aren't asked again.
    fn relinked(
        &mut self,
        edits: &[FileEdit],
        answers: &[Relinked],
        moved: impl Fn(&Path) -> PathBuf,
    ) {
        for a in answers {
            let Some(e) = edits.iter().find(|e| e.file == a.file) else {
                continue;
            };
            let path = moved(&e.path);
            let rel = path.strip_prefix(self.root()).unwrap_or(&path);
            let rel = rel.to_string_lossy().replace('\\', "/");
            self.owners.answered(&a.owner, &rel, &a.rev, Some(&a.after));
        }
    }
}
