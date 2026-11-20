//! The loaded extensions (`Extension`, `Registry`): who they are, who owns which kind of file
//! and shows which notes, and calling one with its `Host`.

use crate::host::{ExtId, Host};
use crate::manifest::{FileType, Manifest, Permission, Shows, Spec};
use crate::protocol::{CommandRequest, EditorRequest, Effect, FileNews, FilePreview, PaneNews};
use crate::view::ViewNews;
use crate::{installed, process};
use cold_lava_engine::act::Ask;
use cold_lava_engine::core::Core;
use cold_lava_engine::files;
use cold_lava_engine::gateway::{self, Edited, News, Ticket};
use cold_lava_engine::owner::cache::{FileRead, Kind};
use cold_lava_engine::owner::edits::FileEdit;
use cold_lava_engine::problem::Problem;
use std::collections::HashSet;
use std::path::Path;

/// Whether an extension may claim files with extension `x`: not notes, the engine's own, nor a
/// kind one of the engine's built-in owners reads (`owner::BUILT_IN`: canvases).
fn claimable(x: &str) -> bool {
    x != "md" && !crate::owner::BUILT_IN.iter().any(|(b, _)| *b == x)
}

/// The reader, a screen of the menu's built the way an extension will be: it reaches notes as
/// data only through `Host`, under this name (`Registry::screen`), which edits are attributed to.
pub const READER: &str = "reader";
/// The menu itself, when it deletes, restores or makes a folder: it asks the core as this
/// client, with no permission to check, since it is the user.
pub const MENU: &str = "menu";
/// The built-in extensions' names, in the frontends that have them: the agenda, the table,
/// the graph and Neovim as the editor. An installed extension can't take one
/// (`installed::load`).
pub const AGENDA: &str = "agenda";
pub const TABLE: &str = "table";
pub const GRAPH: &str = "graph";
pub const NVIM: &str = "nvim";
const SCREEN_PERMISSIONS: &[Permission] = &[Permission::ItemsRead, Permission::ItemsEdit];

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

/// The loaded extensions and their manifests, who owns which kind of file and who shows
/// which notes. A frontend keeps one, calls extensions through it (`call`) and applies the
/// `Effect`s that come back; its command table is its own (`refuse_commands`).
pub struct Registry {
    loaded: Vec<(Box<dyn Extension>, Manifest)>,
    next_pane: u32,
    next_view: u32,
    /// Why types, shapes, commands, keys, file types or claims the extensions registered were
    /// left out.
    refused: Vec<Problem>,
    /// Where in `refused` the frontend's word on commands goes: after the types and shapes,
    /// before the file types and the claims.
    commands_at: usize,
    /// Extensions on disk that didn't load, and why.
    mistakes: Vec<Problem>,
    /// Whether any extension runs in a process, so the menu polls more often.
    processes: bool,
    /// Who opens each kind of file, by extension: the first to claim it.
    owners: Vec<(String, ExtId)>,
    /// The claims to show notes that hold, in load order: the extension and its claim's index.
    shows: Vec<(ExtId, usize)>,
}

/// Whether `id` is in the extension `name`'s namespace: `<name>.<something>`. Its commands'
/// and its types' ids must be.
pub fn namespaced(name: &str, id: &str) -> bool {
    (id.strip_prefix(name))
        .and_then(|rest| rest.strip_prefix('.'))
        .is_some_and(|rest| !rest.is_empty())
}

/// Leaves out of `m` the types and shapes it can't define, as commands are: an id outside its
/// namespace (`<name>.<type>`), or one it gave already. Returns why, one line each.
fn leave_out_definitions(m: &mut Manifest) -> Vec<String> {
    let name = m.name.clone();
    let mut why = leave_out(&name, "type", &mut m.types);
    why.extend(leave_out(&name, "shape", &mut m.shapes));
    why
}

fn leave_out(name: &str, what: &str, specs: &mut Vec<Spec>) -> Vec<String> {
    let mut why = Vec::new();
    let mut ids = HashSet::new();
    specs.retain(|t| {
        let id = &t.id;
        let problem = if !namespaced(name, id) {
            format!("its id must start with \"{name}.\"")
        } else if !ids.insert(id.clone()) {
            "that id is taken".into()
        } else {
            return true;
        };
        why.push(format!("{name}: {what} {id:?} was left out: {problem}"));
        false
    });
    why
}

impl Registry {
    /// The frontend's built-in extensions, then the ones installed in `dir`
    /// (`installed::user_dir` for the user's), each in a process of its own.
    pub fn load(built_in: Vec<Box<dyn Extension>>, dir: Option<&Path>) -> Registry {
        let mut found = built_in;
        let (on_disk, mistakes) = dir.map(installed::load).unwrap_or_default();
        let processes = !on_disk.is_empty();
        for ext in on_disk {
            found.extend(process::Process::new(ext).map(|p| Box::new(p) as Box<dyn Extension>));
        }
        let mut r = Registry::new(found);
        r.mistakes = mistakes;
        r.processes = processes;
        r
    }

    pub fn new(extensions: Vec<Box<dyn Extension>>) -> Registry {
        let mut loaded: Vec<_> = extensions
            .into_iter()
            .map(|mut e| {
                let manifest = e.initialize();
                (e, manifest)
            })
            .collect();
        let mut refused = Vec::new();
        for (_, m) in &mut loaded {
            refused.extend(leave_out_definitions(m).into_iter().map(Problem::line));
        }
        let commands_at = refused.len();
        let mut owners: Vec<(String, ExtId)> = Vec::new();
        for (i, (_, m)) in loaded.iter().enumerate() {
            for t in &m.file_types {
                let x = &t.extension;
                if !claimable(x) {
                    let why = format!("{} can't open .{x} files: cold-lava does", m.name);
                    refused.push(Problem::line(why));
                } else if let Some((_, first)) = owners.iter().find(|(o, _)| o == x) {
                    let first = &loaded[first.0].1.name;
                    let why = format!("{} and {first} both open .{x} files: {first} does", m.name);
                    refused.push(Problem::line(why));
                } else {
                    owners.push((x.clone(), ExtId(i)));
                }
            }
        }
        let mut shows: Vec<(ExtId, usize)> = Vec::new();
        for (i, (_, m)) in loaded.iter().enumerate() {
            for (n, claim) in m.shows.iter().enumerate() {
                let what = claim.describe();
                let same = |&&(e, k): &&(ExtId, usize)| {
                    loaded[e.0].1.shows[k].conditions == claim.conditions
                };
                if let Some(problem) = claim.problem() {
                    refused.push(Problem::line(format!(
                        "{}: shows {what:?} was left out: {problem}",
                        m.name
                    )));
                } else if let Some((first, _)) = shows.iter().find(same) {
                    let first = &loaded[first.0].1.name;
                    let why = format!(
                        "{} and {first} both show notes with {what}: {first} does",
                        m.name
                    );
                    refused.push(Problem::line(why));
                } else {
                    shows.push((ExtId(i), n));
                }
            }
        }
        Registry {
            loaded,
            next_pane: 0,
            next_view: 0,
            refused,
            commands_at,
            mistakes: Vec::new(),
            processes: false,
            owners,
            shows,
        }
    }

    /// The extension that shows the note at `path` (workspace-relative, with `.md`, its text
    /// and its tags) instead of the reader, and its claim: of the claims the note meets, the
    /// most specific (the most conditions, as types' shapes are ranked); of equals, the first
    /// loaded.
    pub fn shower_of(&self, path: &str, text: &str, tags: &[String]) -> Option<(ExtId, &Shows)> {
        let mut best: Option<(ExtId, &Shows)> = None;
        for &(ext, n) in &self.shows {
            let claim = &self.manifest(ext).shows[n];
            let more =
                best.is_none_or(|(_, b)| claim.conditions.conditions() > b.conditions.conditions());
            if more && crate::matcher::note_meets(&claim.conditions, path, text, tags) {
                best = Some((ext, claim));
            }
        }
        best
    }

    /// Whether the extension's claim `n` holds: it wasn't left out.
    pub fn shows(&self, ext: ExtId, n: usize) -> bool {
        self.shows.contains(&(ext, n))
    }

    /// The extension that opens files that end in `.<extension>` (lowercase).
    pub fn owner(&self, extension: &str) -> Option<ExtId> {
        (self.owners.iter())
            .find(|(x, _)| x == extension)
            .map(|(_, e)| *e)
    }

    /// Each kind of file an extension owns, with its owner's name and version, for the core.
    pub fn owned_kinds(&self) -> Vec<Kind> {
        (self.owners.iter())
            .map(|(x, ext)| {
                let m = self.manifest(*ext);
                Kind {
                    extension: x.clone(),
                    owner: m.name.clone(),
                    version: m.version.clone(),
                }
            })
            .collect()
    }

    /// The extension that opens the file at `path` (a path or a file name), and its kind.
    pub fn owner_of(&self, path: &str) -> Option<(ExtId, &FileType)> {
        let x = files::extension_of(path)?;
        let ext = self.owner(&x)?;
        let t = (self.manifest(ext).file_types.iter()).find(|t| t.extension == x)?;
        Some((ext, t))
    }

    pub fn refused(&self) -> &[Problem] {
        &self.refused
    }

    /// The frontend's command table left these out (a command whose id is outside its
    /// extension's namespace or taken, a key that is taken or isn't one): they are listed
    /// with the rest, where a manifest names its commands.
    pub fn refuse_commands(&mut self, why: Vec<String>) {
        let after = self.refused.split_off(self.commands_at);
        self.refused.extend(why.into_iter().map(Problem::line));
        self.commands_at = self.refused.len();
        self.refused.extend(after);
    }

    pub fn has_processes(&self) -> bool {
        self.processes
    }

    pub fn mistakes(&self) -> &[Problem] {
        &self.mistakes
    }

    pub fn ids(&self) -> impl Iterator<Item = ExtId> + use<> {
        (0..self.loaded.len()).map(ExtId)
    }

    pub fn running(&self, ext: ExtId) -> bool {
        self.loaded[ext.0].0.running()
    }

    pub fn stopped(&self, ext: ExtId) -> Option<&str> {
        self.loaded[ext.0].0.stopped()
    }

    /// cold-lava is quitting: let every extension go.
    pub fn shutdown(&mut self) {
        for (e, _) in &mut self.loaded {
            e.shutdown();
        }
    }

    pub fn manifest(&self, ext: ExtId) -> &Manifest {
        &self.loaded[ext.0].1
    }

    pub fn manifests(&self) -> impl Iterator<Item = &Manifest> {
        self.loaded.iter().map(|(_, m)| m)
    }

    /// The loaded extension with this name.
    pub fn by_name(&self, name: &str) -> Option<ExtId> {
        (self.loaded.iter())
            .position(|(_, m)| m.name == name)
            .map(ExtId)
    }

    /// Call one extension and return what it asked for, for the caller to apply.
    pub fn call(
        &mut self,
        ext: ExtId,
        core: Option<&mut Core>,
        f: impl FnOnce(&mut dyn Extension, &mut Host<'_>),
    ) -> Vec<Effect> {
        let (extension, manifest) = &mut self.loaded[ext.0];
        let mut host = Host {
            effects: Vec::new(),
            next_pane: &mut self.next_pane,
            next_view: &mut self.next_view,
            name: &manifest.name,
            permissions: &manifest.permissions,
            me: Some(ext),
            owners: &self.owners,
            core,
        };
        f(extension.as_mut(), &mut host);
        host.effects
    }

    /// Asks the extension `ext` about a shape it ships (`Binding::region_asks`, `Acted::Ask`).
    /// The ask carries the shape's inputs, so it needs `items.read`; without it the answer is
    /// that, at once.
    pub fn ask_shape(&mut self, ext: ExtId, mut core: Option<&mut Core>, ask: Ask) -> Vec<Effect> {
        let manifest = &self.loaded[ext.0].1;
        if !manifest.permissions.contains(&Permission::ItemsRead) {
            let why = format!(
                "{} may not be told a shape's inputs: its manifest's permissions don't include items.read",
                manifest.name
            );
            if let Some(core) = core.as_deref_mut() {
                core.shape_answered(&manifest.name, &ask, Err(why));
            }
            return Vec::new();
        }
        self.call(ext, core, |e, host| e.shape(ask, host))
    }

    /// The workspace gateway for one of the frontend's screens (`READER`), which may read and
    /// edit items. Screens ask for no effects.
    pub fn screen<'a>(&'a mut self, name: &'static str, core: &'a mut Core) -> Host<'a> {
        Host {
            effects: Vec::new(),
            next_pane: &mut self.next_pane,
            next_view: &mut self.next_view,
            name,
            permissions: SCREEN_PERMISSIONS,
            me: None,
            owners: &self.owners,
            core: Some(core),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::read_only;

    #[test]
    fn types_outside_the_namespace_or_taken_are_left_out() {
        let spec = |id: &str| Spec {
            id: id.into(),
            definition: toml::from_str("on = \"note\"").unwrap(),
        };
        let mut m = Manifest {
            name: "wc".into(),
            title: "wc".into(),
            version: String::new(),
            description: String::new(),
            roles: Vec::new(),
            hints: Vec::new(),
            commands: Vec::new(),
            types: ["doc", "wcx.doc", "wc.", "wc.doc", "wc.doc"]
                .map(spec)
                .to_vec(),
            shapes: ["lane", "wc.lane"].map(spec).to_vec(),
            permissions: read_only(),
            file_types: Vec::new(),
            shows: Vec::new(),
        };
        let refused = leave_out_definitions(&mut m);
        let ids: Vec<&str> = m.types.iter().map(|t| t.id.as_str()).collect();
        assert_eq!(ids, ["wc.doc"]);
        assert_eq!(m.shapes.len(), 1);
        assert_eq!(refused.len(), 5, "{refused:?}");
        assert_eq!(
            refused[4],
            "wc: shape \"lane\" was left out: its id must start with \"wc.\""
        );
        assert_eq!(
            refused[0],
            "wc: type \"doc\" was left out: its id must start with \"wc.\""
        );
        assert!(refused[3].ends_with("that id is taken"), "{refused:?}");
    }
}
