//! The graph (docs/extensions.md, "Scenes, as built"): a built-in extension on the same path as
//! one in a process. It reads the link graph through the workspace gateway (`links/query`, and
//! `links/subscribe` to hear when it changes), makes a scene of it with the filters and colour
//! groups the user picked, and sends it through the view gateway (`view/open`, `view/update`).
//! The core lays the scene out, draws it and owns its keys; what the graph's commands mean (open
//! a note, show tags, the next colour groups) comes back as view events.

use std::path::Path;

use serde_json::json;

use crate::commands::GraphAction;
use crate::ext::view::{self, Content, Happened, ViewId, ViewNews};
use crate::ext::{
    CommandRequest, EditorRequest, Extension, GRAPH, Host, Manifest, PaneNews, Permission,
};
use crate::gateway::{self, News, SubId};
use crate::graph::{self, BuildOpts, Kind};
use crate::groups;
use crate::links::LinkGraph;
use crate::scene_view::{self, Edge, Group, Look, Node, Scene};

/// The command that opens it: a core command (`g`) the menu hands to it.
pub const OPEN: &str = "graph.open";

#[derive(Default)]
pub struct Graph {
    /// The open workspace's name, the scene's title.
    title: String,
    shown: Option<Shown>,
}

struct Shown {
    view: ViewId,
    sub: SubId,
    links: LinkGraph,
    title: String,
    opts: BuildOpts,
    mode: groups::Mode,
}

/// Which colour groups show first: Obsidian's when the workspace has some.
fn first_mode(links: &LinkGraph) -> groups::Mode {
    match links.groups.is_empty() {
        true => groups::Mode::Off,
        false => groups::Mode::Obsidian,
    }
}

impl Graph {
    fn open(&mut self, host: &mut Host<'_>) -> Result<(), gateway::Error> {
        if let Some(shown) = &self.shown {
            host.show_view(shown.view);
            return Ok(());
        }
        let links = host.query_links()?;
        if !links.files.iter().any(|f| f.path.ends_with(".md")) {
            host.notify("this workspace has no notes yet");
            return Ok(());
        }
        let opts = BuildOpts {
            unresolved: false,
            tags: false,
            orphans: true,
        };
        let mode = first_mode(&links);
        let title = self.title.clone();
        let scene = scene(&links, &title, opts, mode).0;
        let sub = host.subscribe_links()?;
        let view = host.open_view(Content::Scene(scene), None, None)?;
        self.shown = Some(Shown {
            view,
            sub,
            links,
            title,
            opts,
            mode,
        });
        Ok(())
    }

    fn event(&mut self, event: view::Event, host: &mut Host<'_>) {
        let Some(shown) = self.shown.as_mut() else {
            return;
        };
        match event {
            view::Event::Activated { row } => match shown.node(&row) {
                Some(n) if n.kind == Kind::Note => {
                    let path = n.path.map(|p| p.display().to_string());
                    host.run_command("note.edit", json!({ "note": path }))
                }
                // Like clicking an unresolved link in Obsidian: the note is made.
                Some(n) if n.kind == Kind::Unresolved => {
                    let note = format!("{}.md", n.label);
                    host.run_command("note.edit", json!({ "note": note }))
                }
                _ => host.notify("that's a tag — press f to see its notes"),
            },
            view::Event::Command { command, row } => {
                let flash = match command_of(&command) {
                    Some(GraphAction::OpenInObsidian) => {
                        let node = row.as_deref().and_then(|row| shown.node(row));
                        match node.and_then(|n| n.path) {
                            Some(path) => {
                                let path = path.display().to_string();
                                let args = json!({ "note": path });
                                host.run_command("note.open-in-obsidian", args)
                            }
                            None => host.notify("select a note first"),
                        }
                        return;
                    }
                    Some(GraphAction::Rescan) => {
                        return host.run_command("workspace.rescan", json!(null));
                    }
                    Some(GraphAction::Groups) => {
                        use groups::Mode::*;
                        let has_obs = !shown.links.groups.is_empty();
                        shown.mode = match shown.mode {
                            Off if has_obs => Obsidian,
                            Off | Obsidian => Folder,
                            Folder => Tag,
                            Tag => Off,
                        };
                        None
                    }
                    Some(GraphAction::ToggleTags) => {
                        shown.opts.tags = !shown.opts.tags;
                        Some(if shown.opts.tags {
                            "tags shown"
                        } else {
                            "tags hidden"
                        })
                    }
                    Some(GraphAction::ToggleUnresolved) => {
                        shown.opts.unresolved = !shown.opts.unresolved;
                        Some(if shown.opts.unresolved {
                            "unresolved links shown"
                        } else {
                            "unresolved links hidden"
                        })
                    }
                    Some(GraphAction::ToggleOrphans) => {
                        shown.opts.orphans = !shown.opts.orphans;
                        Some(if shown.opts.orphans {
                            "orphans shown"
                        } else {
                            "orphans hidden"
                        })
                    }
                    _ => return,
                };
                let groups = shown.send(host);
                let words = match (flash, shown.mode) {
                    (Some(words), _) => words.to_string(),
                    (None, groups::Mode::Off) => "colour groups: off".to_string(),
                    (None, m) => format!("colour groups: {} ({groups} groups)", m.name()),
                };
                host.notify(words);
            }
            _ => {}
        }
    }
}

impl Shown {
    /// The node `id` stands for, as the scene was sent.
    fn node(&self, id: &str) -> Option<graph::Node> {
        let g = graph::build(&self.links, self.opts);
        g.nodes.into_iter().find(|n| n.key == id)
    }

    /// Send the scene as it is now; how many colour groups it has.
    fn send(&self, host: &mut Host<'_>) -> usize {
        let (scene, groups) = scene(&self.links, &self.title, self.opts, self.mode);
        if let Err(e) = host.update_view(self.view, Content::Scene(scene), None) {
            host.notify(format!("the graph couldn't be shown: {}", e.message));
        }
        groups
    }
}

/// The graph command with this id.
fn command_of(id: &str) -> Option<GraphAction> {
    (crate::commands::GRAPH.iter())
        .find(|c| c.id == id)
        .map(|c| c.action)
}

/// The scene of `links` with these filters and colour groups, and how many groups it has.
pub fn scene(
    links: &LinkGraph,
    title: &str,
    opts: BuildOpts,
    mode: groups::Mode,
) -> (Scene, usize) {
    let g = graph::build(links, opts);
    let assign = groups::assign(mode, &g, &groups::obsidian(&links.groups));
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
    let id = |n: &graph::Node| n.key.clone();
    let nodes = (g.nodes.iter())
        .enumerate()
        .map(|(i, n)| {
            let (look, detail) = match n.kind {
                Kind::Note => (Look::Note, n.rel.clone()),
                Kind::Unresolved => (Look::Ghost, Some("unresolved — Enter creates it".into())),
                Kind::Tag => (Look::Tag, Some(format!("tag on {} notes", g.links_in[i]))),
            };
            Node {
                id: id(n),
                label: n.label.clone(),
                look,
                group: assign.of_node[i],
                file: n.path.as_ref().map(|p| p.display().to_string()),
                detail,
            }
        })
        .collect();
    let edges = (g.edges.iter())
        .map(|&(a, b)| Edge {
            from: id(&g.nodes[a]),
            to: id(&g.nodes[b]),
        })
        .collect();
    let groups: Vec<Group> = (assign.groups.iter())
        .map(|gr| Group {
            label: gr.label.clone(),
            color: scene_view::color_hex(gr.color),
        })
        .collect();
    let count = groups.len();
    let scene = Scene {
        title: title.to_string(),
        nodes,
        edges,
        groups,
        flags,
    };
    (scene, count)
}

/// "workspace changed: +2 notes" and the like.
fn changed(created: &[String], removed: &[String]) -> String {
    let notes = |paths: &[String]| paths.iter().filter(|p| p.ends_with(".md")).count();
    match (notes(created), notes(removed)) {
        (0, 0) => "workspace changed: graph updated".to_string(),
        (a, 0) => format!("workspace changed: +{a} notes"),
        (0, r) => format!("workspace changed: −{r} notes"),
        (a, r) => format!("workspace changed: +{a} −{r} notes"),
    }
}

impl Extension for Graph {
    fn initialize(&mut self) -> Manifest {
        Manifest {
            name: GRAPH.into(),
            title: "Graph".into(),
            version: String::new(),
            description: "The workspace's notes and the links between them".into(),
            roles: Vec::new(),
            hints: Vec::new(),
            commands: Vec::new(),
            types: Default::default(),
            permissions: vec![Permission::ItemsRead],
            file_types: Vec::new(),
            shows: Vec::new(),
        }
    }

    fn pane(&mut self, _: PaneNews, _: &mut Host<'_>) {}

    fn editor(&mut self, _: EditorRequest, _: &mut Host<'_>) {}

    fn command(&mut self, request: CommandRequest, host: &mut Host<'_>) {
        let CommandRequest::Run { id, .. } = request;
        if id != OPEN {
            return host.notify(format!("the graph has no command {id}"));
        }
        if let Err(e) = self.open(host) {
            host.notify(format!("no graph: {}", e.message));
        }
    }

    fn items(&mut self, news: News, host: &mut Host<'_>) {
        let Some(shown) = self.shown.as_mut() else {
            return;
        };
        let News::LinksChanged {
            subscription,
            created,
            removed,
        } = news
        else {
            return;
        };
        if subscription != shown.sub {
            return;
        }
        match host.query_links() {
            Ok(links) => {
                shown.links = links;
                shown.send(host);
                host.notify(changed(&created, &removed));
            }
            Err(e) => host.notify(format!("the graph couldn't be read again: {}", e.message)),
        }
    }

    fn view(&mut self, news: ViewNews, host: &mut Host<'_>) {
        let Some(shown) = self.shown.as_ref() else {
            return;
        };
        match news {
            ViewNews::Closed { view } if view == shown.view => {
                if let Some(shown) = self.shown.take() {
                    let _ = host.unsubscribe_links(shown.sub);
                }
            }
            ViewNews::Event(Happened { view, event }) if view == shown.view => {
                self.event(event, host)
            }
            _ => {}
        }
    }

    /// Another workspace: its view and subscription went with the old one.
    fn workspace(&mut self, root: Option<&Path>, _: &mut Host<'_>) {
        self.shown = None;
        let root = root.and_then(|r| r.canonicalize().ok());
        let name = root.as_deref().and_then(|r| r.file_name());
        self.title = name
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::links::{ColorGroup, Link, LinkFile};

    fn links() -> LinkGraph {
        let file = |path: &str, tags: &[&str]| LinkFile {
            path: path.into(),
            name: path
                .rsplit('/')
                .next()
                .unwrap()
                .trim_end_matches(".md")
                .into(),
            tags: tags.iter().map(|t| t.to_string()).collect(),
        };
        let link = |from: &str, to: &str| Link {
            from: from.into(),
            to: to.ends_with(".md").then(|| to.into()),
            target: (!to.ends_with(".md")).then(|| to.into()),
        };
        LinkGraph {
            files: vec![
                file("Guide/Start.md", &["guide"]),
                file("Concepts/Lava.md", &["concept"]),
                file("Lonely.md", &[]),
            ],
            links: vec![
                link("Guide/Start.md", "Concepts/Lava.md"),
                link("Guide/Start.md", "Pele"),
                link("Concepts/Lava.md", "Guide/Start.md"),
            ],
            groups: vec![ColorGroup {
                query: "path:Guide".into(),
                color: "#fad65a".into(),
            }],
        }
    }

    const PLAIN: BuildOpts = BuildOpts {
        unresolved: false,
        tags: false,
        orphans: true,
    };

    fn ids(s: &Scene) -> Vec<&str> {
        s.nodes.iter().map(|n| n.id.as_str()).collect()
    }

    #[test]
    fn a_scene_of_the_files_and_their_links() {
        let l = links();
        let (s, groups) = scene(&l, "w", PLAIN, first_mode(&l));
        assert_eq!(ids(&s), ["n:guide/start", "n:concepts/lava", "n:lonely"]);
        assert_eq!(s.edges.len(), 2);
        assert_eq!((s.title.as_str(), groups), ("w", 1));
        assert_eq!(s.flags, ["obsidian groups"]);
        let start = &s.nodes[0];
        assert_eq!(start.file.as_deref(), Some("Guide/Start.md"));
        assert_eq!(start.detail.as_deref(), Some("Guide/Start"));
        assert_eq!((start.group, s.nodes[1].group), (Some(0), None));
        assert_eq!(s.groups[0].color, "#fad65a");
    }

    #[test]
    fn filters_add_tags_and_unresolved_links_and_leave_out_orphans() {
        let all = BuildOpts {
            unresolved: true,
            tags: true,
            orphans: false,
        };
        let (s, _) = scene(&links(), "w", all, groups::Mode::Folder);
        assert_eq!(
            ids(&s),
            [
                "n:guide/start",
                "n:concepts/lava",
                "u:pele",
                "t:guide",
                "t:concept"
            ]
        );
        let tag = s.nodes.iter().find(|n| n.id == "t:guide").unwrap();
        assert_eq!(tag.look, Look::Tag);
        assert_eq!(tag.detail.as_deref(), Some("tag on 1 notes"));
        assert_eq!(s.nodes[2].look, Look::Ghost);
        assert_eq!(s.flags, ["by folder", "tags", "unresolved", "no orphans"]);
    }

    #[test]
    fn what_changed_says_how_many_notes_came_and_went() {
        let paths = |ps: &[&str]| ps.iter().map(|p| p.to_string()).collect::<Vec<_>>();
        assert_eq!(changed(&[], &[]), "workspace changed: graph updated");
        assert_eq!(
            changed(&paths(&["a.md", "b.csv"]), &[]),
            "workspace changed: +1 notes"
        );
        assert_eq!(
            changed(&paths(&["a.md"]), &paths(&["b.md", "c.md"])),
            "workspace changed: +1 −2 notes"
        );
    }
}
