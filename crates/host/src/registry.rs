//! The loaded extensions (`Registry`): who they are, who owns which kind of file and shows
//! which notes, and calling one with its `Host`.

use crate::extension::Extension;
use crate::host::{ExtId, Host};
use crate::manifest::{FileType, Manifest, Permission, Shows, claimable, leave_out_definitions};
use crate::protocol::Effect;
use crate::{installed, process};
use cold_lava_engine::act::Ask;
use cold_lava_engine::core::Core;
use cold_lava_engine::files;
use cold_lava_engine::owner::cache::Kind;
use cold_lava_engine::problem::Problem;
use std::path::Path;

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
