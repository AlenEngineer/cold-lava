//! The palette, and the graph view: the workspace's notes and links as a scene (`scene_view`),
//! what it shows (which nodes, their colour groups, the filters) and what its commands mean.

use crate::commands::GraphAction;
use crate::core::{Core, Event as CoreEvent};
use crate::graph::{self, BuildOpts, Kind};
use crate::groups;
use crate::scene_view::{self, Edge, Group, Look, Node, Scene, SceneView};
use crate::term;
use crate::workspace::Workspace;
use anyhow::Result;
use ratatui::style::Color;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

// Palette (truecolor; Ghostty, Kitty, WezTerm, iTerm2 all support it).
pub const ACCENT: Color = Color::Rgb(167, 139, 250);
pub const ACCENT_SOFT: Color = Color::Rgb(214, 200, 255);
pub const NOTE: Color = Color::Rgb(205, 205, 215);
pub const DIM: Color = Color::Rgb(85, 85, 98);
pub const EDGE: Color = Color::Rgb(68, 68, 82);
pub const GHOST: Color = Color::Rgb(115, 115, 128);
pub const TAG: Color = Color::Rgb(120, 200, 145);
pub const HIT: Color = Color::Rgb(250, 200, 85);
pub const MUTED: Color = Color::Rgb(140, 140, 155);

#[derive(Clone, Copy)]
pub enum Render {
    Auto,
    Braille,
    Pixels,
}

/// The graph of one workspace, in the scene view.
pub struct GraphScreen {
    pub view: SceneView,
    opts: BuildOpts,
    group_mode: groups::Mode,
    obsidian_groups: Vec<groups::Group>,
    /// Notes created and removed since the graph last followed the workspace, when it has to.
    changes: Option<(usize, usize)>,
}

/// What the menu does after one of the graph's commands.
pub enum Then {
    Nothing,
    /// Open this file in the editor.
    Edit(PathBuf),
    /// Pick a whiteboard to open.
    Boards,
}

impl GraphScreen {
    pub fn new(core: &Core, render: Render) -> GraphScreen {
        let workspace = &core.index.workspace;
        let opts = BuildOpts {
            unresolved: false,
            tags: false,
            orphans: true,
        };
        let obsidian_groups = groups::load_obsidian(&workspace.root);
        let group_mode = if obsidian_groups.is_empty() {
            groups::Mode::Off
        } else {
            groups::Mode::Obsidian
        };
        let scene = scene(workspace, opts, group_mode, &obsidian_groups);
        GraphScreen {
            view: SceneView::new(scene, render),
            opts,
            group_mode,
            obsidian_groups,
            changes: None,
        }
    }

    /// The workspace changed: the graph follows on its next frame.
    pub fn follow(&mut self, events: &[CoreEvent], root: &Path) {
        if events
            .iter()
            .any(|e| matches!(e, CoreEvent::GraphSettingsChanged | CoreEvent::Rescanned))
        {
            self.obsidian_groups = groups::load_obsidian(root);
        }
        let count = |pred: fn(&CoreEvent) -> bool| events.iter().filter(|e| pred(e)).count();
        let (added, removed) = self.changes.unwrap_or_default();
        self.changes = Some((
            added + count(|e| matches!(e, CoreEvent::NoteCreated(_))),
            removed + count(|e| matches!(e, CoreEvent::NoteRemoved(_))),
        ));
    }

    /// Once a frame while it shows. How long the menu may wait for input before the next.
    pub fn tick(&mut self, core: &Core) -> Duration {
        // Not while a node is dragged: the graph shouldn't move under the mouse.
        if !self.view.dragging()
            && let Some(changes) = self.changes.take()
        {
            self.rebuild(core);
            self.view.flash(match changes {
                (0, 0) => "workspace changed: graph updated".to_string(),
                (a, 0) => format!("workspace changed: +{a} notes"),
                (0, r) => format!("workspace changed: −{r} notes"),
                (a, r) => format!("workspace changed: +{a} −{r} notes"),
            });
        }
        self.view.tick()
    }

    /// Show the workspace as it is now; how many colour groups it has.
    fn rebuild(&mut self, core: &Core) -> usize {
        let workspace = &core.index.workspace;
        let scene = scene(workspace, self.opts, self.group_mode, &self.obsidian_groups);
        let groups = scene.groups.len();
        self.view.update(scene);
        groups
    }

    pub fn command(&mut self, a: GraphAction, core: &mut Core) -> Result<Then> {
        match a {
            GraphAction::OpenNote => {
                let path = self.note_to_open(&core.index.workspace.root)?;
                return Ok(path.map_or(Then::Nothing, Then::Edit));
            }
            GraphAction::OpenInObsidian => self.open_in_obsidian(&core.index.workspace.name),
            GraphAction::Boards => return Ok(Then::Boards),
            GraphAction::Groups => {
                self.cycle_groups();
                let groups = self.rebuild(core);
                self.view.flash(match self.group_mode {
                    groups::Mode::Off => "colour groups: off".to_string(),
                    m => format!("colour groups: {} ({groups} groups)", m.name()),
                });
            }
            GraphAction::ToggleTags => {
                self.opts.tags = !self.opts.tags;
                self.rebuild(core);
                self.view.flash(if self.opts.tags {
                    "tags shown"
                } else {
                    "tags hidden"
                });
            }
            GraphAction::ToggleUnresolved => {
                self.opts.unresolved = !self.opts.unresolved;
                self.rebuild(core);
                self.view.flash(if self.opts.unresolved {
                    "unresolved links shown"
                } else {
                    "unresolved links hidden"
                });
            }
            GraphAction::ToggleOrphans => {
                self.opts.orphans = !self.opts.orphans;
                self.rebuild(core);
                self.view.flash(if self.opts.orphans {
                    "orphans shown"
                } else {
                    "orphans hidden"
                });
            }
            GraphAction::Rescan => core.rescan(), // followed as a Rescanned event
            _ => {}
        }
        Ok(Then::Nothing)
    }

    fn cycle_groups(&mut self) {
        use groups::Mode::*;
        let has_obs = !self.obsidian_groups.is_empty();
        self.group_mode = match self.group_mode {
            Off if has_obs => Obsidian,
            Off | Obsidian => Folder,
            Folder => Tag,
            Tag => Off,
        };
    }

    /// The selected node's file, to edit. An unresolved link's note is created first, as
    /// clicking one does in Obsidian.
    fn note_to_open(&mut self, root: &Path) -> Result<Option<PathBuf>> {
        let Some(node) = self.view.selected() else {
            self.view
                .flash("select a note first (arrows, Tab, / or click)");
            return Ok(None);
        };
        match (node.look, &node.file) {
            (Look::Note, Some(file)) => Ok(Some(root.join(file))),
            (Look::Ghost, _) => {
                let p = root.join(format!("{}.md", node.label));
                crate::write::create_if_missing(&p, b"")?;
                Ok(Some(p))
            }
            _ => {
                self.view.flash("that's a tag — press f to see its notes");
                Ok(None)
            }
        }
    }

    fn open_in_obsidian(&mut self, workspace: &str) {
        let Some(file) = self.view.selected().and_then(|n| n.file.clone()) else {
            return self.view.flash("select a note first");
        };
        let rel = file.strip_suffix(".md").unwrap_or(&file);
        let ok = term::open_in_obsidian(workspace, rel);
        self.view.flash(if ok {
            "opened in Obsidian"
        } else {
            "couldn't launch Obsidian"
        });
    }
}

/// The workspace's notes and their links as a scene, with the colour groups of `mode`.
fn scene(
    workspace: &Workspace,
    opts: BuildOpts,
    mode: groups::Mode,
    obsidian: &[groups::Group],
) -> Scene {
    let g = graph::build(workspace, opts, &HashMap::new());
    let assign = groups::assign(mode, &g, obsidian);
    let mut flags = Vec::new();
    if mode != groups::Mode::Off {
        flags.push(mode.name().to_string());
    }
    for (on, flag) in [
        (opts.tags, "tags"),
        (opts.unresolved, "unresolved"),
        (!opts.orphans, "no orphans"),
    ] {
        if on {
            flags.push(flag.to_string());
        }
    }
    let nodes = (g.nodes.iter())
        .enumerate()
        .map(|(i, n)| {
            let file = (n.path.as_ref())
                .and_then(|p| p.strip_prefix(&workspace.root).ok())
                .map(|p| p.to_string_lossy().replace('\\', "/"));
            let (look, detail) = match n.kind {
                Kind::Note => (Look::Note, n.rel.clone()),
                Kind::Unresolved => (Look::Ghost, Some("unresolved — Enter creates it".into())),
                Kind::Tag => (Look::Tag, Some(format!("tag on {} notes", g.links_in[i]))),
            };
            Node {
                id: n.key.clone(),
                label: n.label.clone(),
                look,
                group: assign.of_node[i],
                file,
                detail,
            }
        })
        .collect();
    let edges = (g.edges.iter())
        .map(|&(a, b)| Edge {
            from: g.nodes[a].key.clone(),
            to: g.nodes[b].key.clone(),
        })
        .collect();
    let groups = (assign.groups.iter())
        .map(|gr| Group {
            label: gr.label.clone(),
            color: scene_view::color_hex(gr.color),
        })
        .collect();
    Scene {
        title: workspace.name.clone(),
        nodes,
        edges,
        groups,
        flags,
    }
}
