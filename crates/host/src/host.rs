//! What an extension can ask of the core during a call (`Host`): its effects queued, the
//! workspace gateway answered within its manifest's permissions.

use crate::manifest::Permission;
use crate::protocol::{Effect, PaneId, PaneSpec};
use crate::view::{Content, ViewId};
use cold_lava_engine::act::{self, Ask, Reply};
use cold_lava_engine::core::Core;
use cold_lava_engine::files::{
    self, Create, Created, Deleted, Folder, Listed, Restore, Restored, Stat, Text, Written,
};
use cold_lava_engine::gateway::{self, Answer, Edit, Found, Query, Reason, Select, SubId};
use cold_lava_engine::owner::given::Given;
use cold_lava_engine::types::Type;
use serde_json::Value;
use std::path::PathBuf;

/// A loaded extension. Only the host uses it: on the wire, the connection says who is talking.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ExtId(pub usize);

/// What an extension can ask of the core during a call. Effects happen once the call returns;
/// the workspace gateway's `items/*` and `types/list` are answered at once.
pub struct Host<'a> {
    pub(crate) effects: Vec<Effect>,
    pub(crate) next_pane: &'a mut u32,
    pub(crate) next_view: &'a mut u32,
    /// Who is asking: the name in its manifest, which edits are attributed to.
    pub(crate) name: &'a str,
    pub(crate) permissions: &'a [Permission],
    /// Which loaded extension is asking (`None`: a screen of the menu), and who owns which
    /// kind of file, for the file messages.
    pub(crate) me: Option<ExtId>,
    pub(crate) owners: &'a [(String, ExtId)],
    pub(crate) core: Option<&'a mut Core>,
}

impl Host<'_> {
    /// `ui/notify`
    pub fn notify(&mut self, message: impl Into<String>) {
        let message = message.into();
        self.effects.push(Effect::Notify { message });
    }

    /// `ui/status`
    pub fn status(&mut self, text: Option<String>) {
        self.effects.push(Effect::Status { text });
    }

    /// `editor/opened`
    pub fn editor_opened(&mut self, note: PathBuf) {
        self.effects.push(Effect::EditorOpened { note });
    }

    /// `command/run`
    pub fn run_command(&mut self, id: &str, args: Value) {
        let id = id.to_string();
        self.effects.push(Effect::RunCommand { id, args });
    }

    /// `workspace/saved`
    pub fn saved(&mut self, note: PathBuf) {
        self.effects.push(Effect::Saved { note });
    }

    /// `pane/open`. The program starts once the call returns; if it can't, the extension gets
    /// `PaneNews::Exited` with the error.
    pub fn open_pane(&mut self, spec: PaneSpec) -> PaneId {
        *self.next_pane += 1;
        let pane = PaneId(*self.next_pane);
        self.effects.push(Effect::OpenPane { pane, spec });
        pane
    }

    /// `pane/input`
    pub fn pane_input(&mut self, pane: PaneId, text: impl Into<String>) {
        let text = text.into();
        self.effects.push(Effect::PaneInput { pane, text });
    }

    /// `view/open`: the view shows once the call returns, in place of what showed, which Back
    /// returns to, at the row `select` names, with the backlinks of `file` (workspace-relative)
    /// under it when it shows one. Refused when it is over the limits (`view::MAX_ROWS`, …).
    pub fn open_view(
        &mut self,
        content: Content,
        select: Option<String>,
        file: Option<String>,
    ) -> Result<ViewId, gateway::Error> {
        content
            .check()
            .map_err(|why| gateway::Error::new(Reason::Refused, why))?;
        *self.next_view += 1;
        let view = ViewId(*self.next_view);
        self.effects.push(Effect::OpenView {
            view,
            content,
            select,
            file,
        });
        Ok(view)
    }

    /// `view/update`: one of its views shows `content` now, keeping the selection (or
    /// selecting `select`), the folds, the sort and the filter by id. Updates in one frame
    /// draw once, the last.
    pub fn update_view(
        &mut self,
        view: ViewId,
        content: Content,
        select: Option<String>,
    ) -> Result<(), gateway::Error> {
        content
            .check()
            .map_err(|why| gateway::Error::new(Reason::Refused, why))?;
        (self.effects).push(Effect::UpdateView {
            view,
            content,
            select,
        });
        Ok(())
    }

    /// `view/close`
    pub fn close_view(&mut self, view: ViewId) {
        self.effects.push(Effect::CloseView { view });
    }

    /// `view/show`: one of its open views shows in place of what showed, which Back returns
    /// to, as when it opened. A view that isn't open, or is in front already, stays as it is.
    pub fn show_view(&mut self, view: ViewId) {
        self.effects.push(Effect::ShowView { view });
    }

    /// `workspace/open`: show a note, at a line; or the file of the item `item` at its place.
    pub fn open_note(&mut self, note: PathBuf, line: Option<usize>, item: Option<String>) {
        self.effects.push(Effect::OpenNote { note, line, item });
    }

    /// The answer to `file/read` for a file of a kind it owns: its items and links join the
    /// workspace, or why it couldn't read it. Only the file's owner may answer.
    pub fn file_given(&mut self, path: &str, rev: &str, given: Result<Given, String>) {
        let me = self.me;
        let owns = (self.owners.iter())
            .any(|(x, owner)| Some(x) == files::extension_of(path).as_ref() && Some(*owner) == me);
        let name = self.name;
        if let (true, Some(core)) = (owns, self.core.as_deref_mut()) {
            core.file_given(name, path, rev, given);
        }
    }

    /// The answer to `file/preview` for a file of a kind it owns: what to draw where a note
    /// embeds it, or why not. Only the file's owner may answer; a view over the limits, or
    /// with more than `view::PREVIEW_ROWS` rows, is refused.
    pub fn file_previewed(&mut self, path: &str, rev: &str, content: Result<Content, String>) {
        let me = self.me;
        let owns = (self.owners.iter())
            .any(|(x, owner)| Some(x) == files::extension_of(path).as_ref() && Some(*owner) == me);
        if !owns {
            return;
        }
        let content = content.and_then(|c| match c.check().and_then(|()| c.small()) {
            Ok(()) => Ok(c),
            Err(why) => Err(why),
        });
        let (path, rev) = (path.to_string(), rev.to_string());
        self.effects.push(Effect::Previewed { path, rev, content });
    }

    /// The answer to `file/edit` for a file of a kind it owns: the file's new text, or why not.
    /// Only the file's owner may answer.
    pub fn file_edited(&mut self, path: &str, rev: &str, text: Result<String, String>) {
        let me = self.me;
        let owns = (self.owners.iter())
            .any(|(x, owner)| Some(x) == files::extension_of(path).as_ref() && Some(*owner) == me);
        let name = self.name;
        if let (true, Some(core)) = (owns, self.core.as_deref_mut()) {
            core.file_edited(name, path, rev, text);
        }
    }

    /// The answer to `shape/region` or `shape/gesture` about a shape it ships: the core keeps
    /// it for the frontend's bindings (`Core::take_shape_answers`).
    pub fn shape_answered(&mut self, ask: &Ask, answer: Result<Reply, String>) {
        let name = self.name;
        if let Some(core) = self.core.as_deref_mut() {
            core.shape_answered(name, ask, answer);
        }
    }

    /// A recipe on the whole workspace, for a step of a shape it ships: it rewrites files, so it
    /// needs `files.write`.
    pub fn run_recipe(
        &mut self,
        id: &str,
        options: crate::recipe::Options,
    ) -> Result<String, gateway::Error> {
        self.core(Permission::FilesWrite)?.run_recipe(id, options)
    }

    /// Nothing, when the caller's manifest asks for `needs`; else the refusal, naming it.
    fn may(&self, needs: Permission) -> Result<(), gateway::Error> {
        if self.permissions.contains(&needs) {
            return Ok(());
        }
        let message = format!(
            "{} may not do this: its manifest's permissions don't include {}",
            self.name,
            needs.name()
        );
        Err(gateway::Error::new(Reason::Forbidden, message))
    }

    /// The core, when the caller's manifest asks for `needs`.
    fn core(&mut self, needs: Permission) -> Result<&mut Core, gateway::Error> {
        self.may(needs)?;
        (self.core.as_deref_mut())
            .ok_or_else(|| gateway::Error::new(Reason::NoWorkspace, "no workspace is open"))
    }

    /// `items/query`
    pub fn query_items(&mut self, query: &Query) -> Result<Found, gateway::Error> {
        self.core(Permission::ItemsRead)?.query_items(query)
    }

    /// `items/subscribe`: the extension's `items` hears `items/changed` and `items/reset` for it
    /// from now on.
    pub fn subscribe_items(&mut self, select: Select) -> Result<SubId, gateway::Error> {
        let name = self.name;
        self.core(Permission::ItemsRead)?
            .subscribe_items(name, select)
    }

    /// `items/unsubscribe`, for one of its own subscriptions.
    pub fn unsubscribe_items(&mut self, sub: SubId) -> Result<(), gateway::Error> {
        let name = self.name;
        self.core(Permission::ItemsRead)?
            .unsubscribe_items(name, sub);
        Ok(())
    }

    /// `items/edit` through the core's safe edits, attributed to the caller: made at once in a
    /// note; in a file another owner reads, answered `Later`, through `Extension::edited`.
    pub fn edit_item(&mut self, edit: &Edit) -> Result<Answer, gateway::Error> {
        let name = self.name;
        self.core(Permission::ItemsEdit)?.edit_item(name, edit)
    }

    /// `links/query`
    pub fn query_links(&mut self) -> Result<crate::links::LinkGraph, gateway::Error> {
        Ok(self.core(Permission::ItemsRead)?.query_links())
    }

    /// `links/subscribe`: the extension's `items` hears `links/changed` for it from now on.
    pub fn subscribe_links(&mut self) -> Result<SubId, gateway::Error> {
        let name = self.name;
        Ok(self.core(Permission::ItemsRead)?.subscribe_links(name))
    }

    /// `links/unsubscribe`, for one of its own subscriptions.
    pub fn unsubscribe_links(&mut self, sub: SubId) -> Result<(), gateway::Error> {
        self.unsubscribe_items(sub)
    }

    /// `types/list`
    pub fn types(&mut self) -> Result<Vec<Type>, gateway::Error> {
        let core = self.core(Permission::ItemsRead)?;
        Ok(core.index.workspace.item_settings().types.types.clone())
    }

    /// `workspace/style`
    pub fn style(&mut self) -> Result<crate::style::Style, gateway::Error> {
        let core = self.core(Permission::ItemsRead)?;
        Ok(core.index.workspace.style())
    }

    /// The core, for a file message about files of `extension`: the owner of that kind may
    /// read and write them; any other file needs `files.read`, and `files.write` to change it.
    fn files_core(
        &mut self,
        extension: Option<String>,
        writes: bool,
    ) -> Result<&mut Core, gateway::Error> {
        let me = self.me;
        let owns = (self.owners.iter())
            .any(|(x, owner)| Some(x) == extension.as_ref() && Some(*owner) == me);
        match (owns, writes) {
            (true, _) => self
                .core
                .as_deref_mut()
                .ok_or_else(|| gateway::Error::new(Reason::NoWorkspace, "no workspace is open")),
            (false, false) => self.core(Permission::FilesRead),
            (false, true) => self.core(Permission::FilesWrite),
        }
    }

    /// `workspace/read`
    pub fn read_file(&mut self, path: &str) -> Result<Text, gateway::Error> {
        self.files_core(files::extension_of(path), false)?
            .read_file(path)
    }

    /// `workspace/write`: refused unless the file is still the version `expect`.
    pub fn write_file(
        &mut self,
        path: &str,
        text: &str,
        expect: &str,
    ) -> Result<Written, gateway::Error> {
        self.files_core(files::extension_of(path), true)?
            .write_file(path, text, expect)
    }

    /// `workspace/create`
    pub fn create_file(&mut self, create: &Create) -> Result<Created, gateway::Error> {
        self.files_core(create.extension(), true)?
            .create_file(create)
    }

    /// `workspace/files`
    pub fn list_files(&mut self, extension: &str) -> Result<Listed, gateway::Error> {
        let kind = extension.trim_start_matches('.').to_ascii_lowercase();
        Ok(self.files_core(Some(kind), false)?.files(extension))
    }

    /// `workspace/stat`: a read, so the owner of the file's kind or `files.read`. A folder
    /// answers with what is in it, so it needs `files.read` whatever it is called.
    pub fn stat_file(&mut self, path: &str) -> Result<Stat, gateway::Error> {
        let core = self.files_core(files::extension_of(path), false)?;
        let stat = core.stat_file(path)?;
        if stat.inside.is_some() {
            self.may(Permission::FilesRead)?;
        }
        Ok(stat)
    }

    /// `workspace/delete`, to the trash and attributed to the caller: it needs `files.delete`,
    /// whatever kind of file it is.
    pub fn delete_file(&mut self, path: &str, expect: &str) -> Result<Deleted, gateway::Error> {
        let name = self.name;
        self.core(Permission::FilesDelete)?
            .delete_file(name, path, expect)
    }

    /// `workspace/restore`, under `files.delete` too: whoever may delete may put back what it
    /// deleted. Putting it somewhere else (`to`) makes a file there, so it needs `files.write`
    /// as well.
    pub fn restore_file(&mut self, restore: &Restore) -> Result<Restored, gateway::Error> {
        if restore.to.is_some() {
            self.may(Permission::FilesWrite)?;
        }
        let name = self.name;
        self.core(Permission::FilesDelete)?
            .restore_file(name, restore)
    }

    /// `workspace/create-folder`: a folder has no kind, so it needs `files.write`.
    pub fn create_folder(&mut self, path: &str) -> Result<Folder, gateway::Error> {
        let name = self.name;
        self.core(Permission::FilesWrite)?.create_folder(name, path)
    }
}

/// A shape's steps run through the `Host` of the extension that ships it: its permissions, its
/// name on the edits.
impl act::Gateway for Host<'_> {
    fn edit_item(&mut self, edit: &Edit) -> Result<Answer, gateway::Error> {
        Host::edit_item(self, edit)
    }

    fn create_file(&mut self, create: &Create) -> Result<Created, gateway::Error> {
        Host::create_file(self, create)
    }

    fn run_recipe(
        &mut self,
        id: &str,
        options: crate::recipe::Options,
    ) -> Result<String, gateway::Error> {
        Host::run_recipe(self, id, options)
    }
}
