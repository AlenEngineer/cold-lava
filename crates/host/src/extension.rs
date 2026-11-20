//! An extension as the host calls it (`Extension`): one method per host→extension message of
//! `protocol`, each given the `Host` that answers the extension's own.

use crate::host::Host;
use crate::manifest::Manifest;
use crate::protocol::{CommandRequest, EditorRequest, FileNews, FilePreview, PaneNews};
use crate::view::ViewNews;
use cold_lava_engine::act::Ask;
use cold_lava_engine::gateway::{self, Edited, News, Ticket};
use cold_lava_engine::owner::cache::FileRead;
use cold_lava_engine::owner::edits::FileEdit;
use std::path::Path;

/// An extension: built into cold-lava, or a `process::Process` speaking for one in its own
/// process. Its methods run on the UI thread: they must return at once and never wait for a pane
/// or a process.
pub trait Extension {
    /// `initialize`: who it is.
    fn initialize(&mut self) -> Manifest;
    /// `pane/message`, `pane/exited`: news from a pane it opened.
    fn pane(&mut self, news: PaneNews, host: &mut Host<'_>);
    /// `editor/open`, `editor/save`, `editor/renamed`, for the extension in the editor role.
    fn editor(&mut self, request: EditorRequest, host: &mut Host<'_>);
    /// `command/run`: one of its commands, from a key, the palette or an extension.
    fn command(&mut self, _request: CommandRequest, _host: &mut Host<'_>) {}
    /// `items/changed`, `items/reset`: news for a subscription it made.
    fn items(&mut self, _news: News, _host: &mut Host<'_>) {}
    /// `view/event`, `view/closed`: what the user did in a view it opened.
    fn view(&mut self, _news: ViewNews, _host: &mut Host<'_>) {}
    /// `file/open`, `file/changed`: a file of a kind it owns.
    fn file(&mut self, _news: FileNews, _host: &mut Host<'_>) {}
    /// `file/read`: the items and links of a file of a kind it owns, answered now or later
    /// with `Host::file_given`.
    fn read_file(&mut self, read: FileRead, host: &mut Host<'_>) {
        let why = "it doesn't read files".to_string();
        host.file_given(&read.path, &read.rev, Err(why));
    }
    /// `file/edit`: the new text of a file of a kind it owns with some changes made, answered
    /// now or later with `Host::file_edited`.
    fn edit_file(&mut self, edit: FileEdit, host: &mut Host<'_>) {
        let why = "it doesn't edit files".to_string();
        host.file_edited(&edit.path, &edit.rev, Err(why));
    }
    /// `file/preview`: a small view of a file of a kind it owns, answered now or later with
    /// `Host::file_previewed`.
    fn preview_file(&mut self, preview: FilePreview, host: &mut Host<'_>) {
        let why = "it doesn't preview files".to_string();
        host.file_previewed(&preview.path, &preview.rev, Err(why));
    }
    /// `shape/region`, `shape/gesture`: what a region of a shape it ships shows, or what a
    /// gesture on one does, answered now or later with `Host::shape_answered`.
    fn shape(&mut self, ask: Ask, host: &mut Host<'_>) {
        host.shape_answered(&ask, Err("it doesn't answer for shapes".into()));
    }
    /// The answer to an `items/edit` that `Host::edit_item` answered `Later`: an edit of a file
    /// another owner reads, now written, or why not.
    fn edited(
        &mut self,
        _ticket: Ticket,
        _answer: Result<Edited, gateway::Error>,
        _host: &mut Host<'_>,
    ) {
    }
    /// The menu opened a workspace (`None`: it has none open). An extension in a process starts
    /// here, with `initialize`, and stops when the workspace closes.
    fn workspace(&mut self, _root: Option<&Path>, _host: &mut Host<'_>) {}
    /// Once a frame: answer what it asked since the last one.
    fn poll(&mut self, _host: &mut Host<'_>) {}
    /// Whether its commands can run now.
    fn running(&self) -> bool {
        true
    }
    /// Why it stopped, when it did.
    fn stopped(&self) -> Option<&str> {
        None
    }
    /// cold-lava is quitting.
    fn shutdown(&mut self) {}
}
