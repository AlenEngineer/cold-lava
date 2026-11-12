//! The scene view: nodes and the links between them, as a sender describes them (`Scene`), laid
//! out, drawn and moved around in by the core (docs/extensions.md, "Scenes"). It runs in the
//! menu's loop like the list and table views: the menu ticks it once a frame, hands it keys and
//! the mouse, and draws it. The view owns everything that runs at frame rate: the force layout,
//! the camera, selection, the local graph, search, labels, and drawing in braille or Kitty
//! pixels. Keys that mean something to the sender (open a note, show tags) come back as
//! `Action::Command`. Positions are kept by node id across updates.

use crate::commands::{self, GraphAction, HistoryAction, Key, KeyHint, Keymap};
use crate::graph::{self, Graph, Kind};
use crate::history::Nav;
use crate::kitty::{self, Kitty};
use crate::scene::{Disc, Edge as Line2, Frame as Drawing, Label, Layer};
use crate::workspace;
use ratatui::Frame;
use ratatui::crossterm::event::{
    KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::symbols::Marker;
use ratatui::text::{Line, Span};
use ratatui::widgets::canvas::Canvas;
use ratatui::widgets::{Block, BorderType, Clear, Paragraph, Widget, Wrap};
use std::collections::{HashMap, HashSet, VecDeque};
use std::time::{Duration, Instant};

use crate::app::{ACCENT, ACCENT_SOFT, DIM, EDGE, GHOST, HIT, MUTED, NOTE, Render, TAG};

pub use cold_lava_views::scene::{Edge, Group, Look, Node, Scene};

/// What the menu should do after a key.
#[derive(Debug, PartialEq)]
pub enum Action {
    Stay,
    /// Back to the menu, the view kept for Back to return to.
    Leave,
    History(HistoryAction),
    /// A command whose meaning is the sender's, about the selection.
    Command(GraphAction),
}

#[derive(Clone, Copy, PartialEq)]
enum Labels {
    Auto,
    All,
    Off,
}

enum Mode {
    Normal,
    Search(String),
    Help,
    /// Which key: the commands by group, until a key is typed.
    Keys,
}

enum Drag {
    Pan { col: u16, row: u16 },
    Node { idx: usize },
}

pub struct SceneView {
    scene: Scene,
    /// `scene.groups`' colours.
    colors: Vec<Color>,
    /// The layout: one node per scene node, in the same order.
    graph: Graph,
    // Camera: world point at the centre of the view, and braille dots per world unit.
    cam: (f64, f64),
    zoom: f64,
    auto_fit: bool,
    sel: Option<usize>,
    history: Vec<String>,
    local: bool,
    depth: usize,
    visible: Vec<bool>,
    labels: Labels,
    paused: bool,
    mode: Mode,
    hits: Vec<usize>,
    hit_pos: usize,
    msg: Option<(String, Instant)>,
    area: Rect,
    drag: Option<Drag>,
    keys: Keymap<GraphAction>,
    /// Where Back and Forward lead, and their keys.
    nav: (Nav, Keymap<HistoryAction>),
    kitty: Option<Kitty>,
    pixels: bool,
    dirty: bool,
    pending_image: Vec<u8>,
    /// The view the last pixel frame was rendered for; any camera move needs a new frame.
    last_view: (f64, f64, f64, f64),
    /// The camera is still easing toward a fit.
    fitting: bool,
}

impl SceneView {
    pub fn new(scene: Scene, render: Render) -> SceneView {
        let graph = layout(&scene, &HashMap::new());
        let n = graph.nodes.len();
        let kitty = match render {
            Render::Braille => None,
            Render::Auto if !kitty::supported() => None,
            _ => Kitty::new(),
        };
        let keys = Keymap::new(commands::GRAPH);
        let nav = (Nav::default(), commands::history_keys(&keys));
        let mut v = SceneView {
            colors: colors(&scene),
            scene,
            graph,
            cam: (0.0, 0.0),
            zoom: 1.0,
            auto_fit: true,
            sel: None,
            history: Vec::new(),
            local: false,
            depth: 1,
            visible: vec![true; n],
            labels: Labels::Auto,
            paused: false,
            mode: Mode::Normal,
            hits: Vec::new(),
            hit_pos: 0,
            msg: None,
            area: Rect::default(),
            drag: None,
            keys,
            nav,
            pixels: kitty.is_some(),
            kitty,
            dirty: true,
            pending_image: Vec::new(),
            last_view: (0.0, 0.0, 0.0, 0.0),
            fitting: true,
        };
        // Let the layout untangle a bit before the first frame.
        for _ in 0..40 {
            v.graph.step(&v.visible);
        }
        v
    }

    /// Show `scene` from now on, keeping positions and the selection by node id. When only
    /// looks changed (labels, groups, flags), the layout stays still.
    pub fn update(&mut self, scene: Scene) {
        let ids = |s: &Scene| s.nodes.iter().map(|n| n.id.clone()).collect::<Vec<_>>();
        if ids(&scene) == ids(&self.scene) && scene.edges == self.scene.edges {
            self.colors = colors(&scene);
            self.scene = scene;
            self.dirty = true;
            return;
        }
        let prev = self.graph.positions();
        let prev_aspect = self.graph.aspect;
        let sel_key = self.sel.map(|i| self.graph.nodes[i].key.clone());
        self.graph = layout(&scene, &prev);
        self.graph.alpha = 0.6;
        self.graph.aspect = prev_aspect;
        self.colors = colors(&scene);
        self.scene = scene;
        self.sel = sel_key.and_then(|k| self.graph.find(&k));
        self.hits.clear();
        self.visible = vec![true; self.graph.nodes.len()];
        self.dirty = true;
    }

    /// Where Back and Forward lead from here.
    pub fn navigate(&mut self, nav: Nav) {
        self.nav.0 = nav;
    }

    /// Select the node `id`, or nothing.
    pub fn select_id(&mut self, id: Option<&str>) {
        self.sel = id.and_then(|key| self.graph.find(key));
    }

    pub fn title(&self) -> &str {
        &self.scene.title
    }

    pub fn selected(&self) -> Option<&Node> {
        self.sel.map(|i| &self.scene.nodes[i])
    }

    /// A node is being dragged: the layout under the mouse shouldn't change.
    pub fn dragging(&self) -> bool {
        self.drag.is_some()
    }

    pub fn flash(&mut self, s: impl Into<String>) {
        self.msg = Some((s.into(), Instant::now()));
    }

    /// Once a frame: the layout takes a step and the camera eases. How long the menu may wait
    /// for input before the next frame.
    pub fn tick(&mut self) -> Duration {
        self.update_visible();
        if self.area.height > 0 {
            let (x0, x1, y0, y1) = self.view();
            self.graph.aspect = (x1 - x0) / (y1 - y0);
        }
        if !self.paused && !self.graph.settled() {
            self.graph.step(&self.visible);
            self.dirty = true;
        }
        if self.auto_fit {
            self.fit(0.25);
        }
        let idle = (self.paused || self.graph.settled()) && !(self.auto_fit && self.fitting);
        Duration::from_millis(if idle && self.drag.is_none() {
            250
        } else if self.pixels {
            25
        } else {
            16
        })
    }

    /// The terminal changed size: the cell size may have too.
    pub fn resized(&mut self) {
        if let Some(k) = self.kitty.as_mut() {
            k.measure();
        }
        self.dirty = true;
    }

    /// The Kitty graphics to write after the frame that was just drawn.
    pub fn take_image(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.pending_image)
    }

    /// What removes the view's pixels, before something else takes the screen; the next frame
    /// draws them again.
    pub fn hide(&mut self) -> Vec<u8> {
        self.dirty = true;
        self.pending_image.clear();
        self.kitty.as_mut().map(|k| k.clear()).unwrap_or_default()
    }

    // ---------- graph state ----------

    fn update_visible(&mut self) {
        let n = self.graph.nodes.len();
        match (self.local, self.sel) {
            (true, Some(s)) => {
                let mut vis = vec![false; n];
                let mut q = VecDeque::from([(s, 0)]);
                vis[s] = true;
                while let Some((i, d)) = q.pop_front() {
                    if d == self.depth {
                        continue;
                    }
                    for &j in &self.graph.adj[i] {
                        if !vis[j] {
                            vis[j] = true;
                            q.push_back((j, d + 1));
                        }
                    }
                }
                if vis != self.visible {
                    self.graph.reheat(0.4);
                }
                self.visible = vis;
            }
            _ => {
                if self.visible.len() != n || self.visible.iter().any(|v| !v) {
                    self.visible = vec![true; n];
                    self.graph.reheat(0.3);
                }
            }
        }
    }

    fn select(&mut self, i: usize) {
        if Some(i) == self.sel {
            return;
        }
        if let Some(prev) = self.sel {
            self.history.push(self.graph.nodes[prev].key.clone());
            if self.history.len() > 200 {
                self.history.remove(0);
            }
        }
        self.sel = Some(i);
        if self.local {
            self.auto_fit = true;
        } else {
            self.ensure_on_screen(i);
        }
    }

    fn neighbours_sorted(&self, i: usize) -> Vec<usize> {
        let mut v = self.graph.adj[i].clone();
        v.sort_by_key(|&j| self.graph.nodes[j].label.to_lowercase());
        v
    }

    /// Pick the nearest visible node in a direction (dx, dy in world space, y up).
    fn select_dir(&mut self, dx: f64, dy: f64) {
        let Some(s) = self.sel else {
            return self.select_default();
        };
        let (sx, sy) = (self.graph.nodes[s].x, self.graph.nodes[s].y);
        let best = (0..self.graph.nodes.len())
            .filter(|&j| j != s && self.visible[j])
            .filter_map(|j| {
                let (ox, oy) = (self.graph.nodes[j].x - sx, self.graph.nodes[j].y - sy);
                let along = ox * dx + oy * dy;
                if along <= 0.0 {
                    return None;
                }
                let perp = (ox * dy - oy * dx).abs();
                (perp <= along * 2.0).then_some((j, along + perp * 2.5))
            })
            .min_by(|a, b| a.1.total_cmp(&b.1));
        if let Some((j, _)) = best {
            self.select(j);
        }
    }

    fn select_default(&mut self) {
        // Start from the most connected visible node.
        if let Some(i) = (0..self.graph.nodes.len())
            .filter(|&i| self.visible[i])
            .max_by_key(|&i| self.graph.degree(i))
        {
            self.select(i);
        }
    }

    // ---------- camera ----------

    /// Height/width of a terminal cell. Braille assumes 2 (2×4 dots per cell).
    fn cell_aspect(&self) -> f64 {
        match (&self.kitty, self.pixels) {
            (Some(k), true) => k.cell_aspect(),
            _ => 2.0,
        }
    }

    /// Visible world rectangle. Width is 2 world units per cell at zoom 1.
    fn view(&self) -> (f64, f64, f64, f64) {
        let hw = self.area.width.max(1) as f64 * 2.0 / 2.0 / self.zoom;
        let hh = self.area.height.max(1) as f64 * 2.0 * self.cell_aspect() / 2.0 / self.zoom;
        (
            self.cam.0 - hw,
            self.cam.0 + hw,
            self.cam.1 - hh,
            self.cam.1 + hh,
        )
    }

    fn fit(&mut self, ease: f64) {
        let pts: Vec<(f64, f64)> = (0..self.graph.nodes.len())
            .filter(|&i| self.visible[i])
            .map(|i| (self.graph.nodes[i].x, self.graph.nodes[i].y))
            .collect();
        if pts.is_empty() || self.area.width == 0 {
            return;
        }
        let (mut x0, mut x1, mut y0, mut y1) = (f64::MAX, f64::MIN, f64::MAX, f64::MIN);
        for (x, y) in pts {
            x0 = x0.min(x);
            x1 = x1.max(x);
            y0 = y0.min(y);
            y1 = y1.max(y);
        }
        let pad = 30.0;
        let zx = self.area.width as f64 * 2.0 / (x1 - x0 + pad * 2.0);
        let zy = self.area.height as f64 * 2.0 * self.cell_aspect() / (y1 - y0 + pad * 2.0);
        let z = zx.min(zy).clamp(0.02, 4.0);
        let c = ((x0 + x1) / 2.0, (y0 + y1) / 2.0);
        // Snap once close, so the camera really stops (and pixel frames stop being re-sent).
        let close = (z - self.zoom).abs() < z * 1e-3
            && (c.0 - self.cam.0).abs() * self.zoom < 0.05
            && (c.1 - self.cam.1).abs() * self.zoom < 0.05;
        if close {
            self.zoom = z;
            self.cam = c;
            self.fitting = false;
        } else {
            self.zoom += (z - self.zoom) * ease;
            self.cam.0 += (c.0 - self.cam.0) * ease;
            self.cam.1 += (c.1 - self.cam.1) * ease;
            self.fitting = true;
        }
    }

    fn ensure_on_screen(&mut self, i: usize) {
        let (x0, x1, y0, y1) = self.view();
        let (x, y) = (self.graph.nodes[i].x, self.graph.nodes[i].y);
        let mx = (x1 - x0) * 0.1;
        let my = (y1 - y0) * 0.1;
        if x < x0 + mx || x > x1 - mx || y < y0 + my || y > y1 - my {
            self.cam = (x, y);
            self.auto_fit = false;
        }
    }

    fn pan(&mut self, cells_x: f64, cells_y: f64) {
        self.auto_fit = false;
        self.cam.0 += cells_x * 2.0 / self.zoom;
        self.cam.1 += cells_y * 2.0 * self.cell_aspect() / self.zoom;
    }

    fn zoom_by(&mut self, factor: f64, anchor: Option<(f64, f64)>) {
        self.auto_fit = false;
        let old = self.zoom;
        self.zoom = (self.zoom * factor).clamp(0.02, 12.0);
        if let Some((wx, wy)) = anchor {
            let k = old / self.zoom;
            self.cam.0 = wx - (wx - self.cam.0) * k;
            self.cam.1 = wy - (wy - self.cam.1) * k;
        }
    }

    fn screen_to_world(&self, col: u16, row: u16) -> Option<(f64, f64)> {
        let a = self.area;
        if col < a.x || row < a.y || col >= a.x + a.width || row >= a.y + a.height {
            return None;
        }
        let (x0, x1, y0, y1) = self.view();
        let fx = (col - a.x) as f64 + 0.5;
        let fy = (row - a.y) as f64 + 0.5;
        Some((
            x0 + fx / a.width as f64 * (x1 - x0),
            y1 - fy / a.height as f64 * (y1 - y0),
        ))
    }

    fn node_radius(&self, i: usize) -> f64 {
        1.6 + (self.graph.degree(i) as f64).sqrt() * 0.9
    }

    fn node_at(&self, wx: f64, wy: f64) -> Option<usize> {
        (0..self.graph.nodes.len())
            .filter(|&i| self.visible[i])
            .map(|i| {
                let n = &self.graph.nodes[i];
                let d = ((n.x - wx).powi(2) + (n.y - wy).powi(2)).sqrt();
                (
                    i,
                    d,
                    self.node_radius(i).max(3.0 / self.zoom) + 2.0 / self.zoom,
                )
            })
            .filter(|(_, d, r)| d <= r)
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(i, _, _)| i)
    }

    // ---------- input ----------

    pub fn key(&mut self, k: KeyEvent) -> Action {
        self.dirty = true;
        if matches!(self.mode, Mode::Keys) {
            self.mode = Mode::Normal;
            if !self.key_hints().iter().any(|h| h.key == Key::from(k)) {
                return Action::Stay;
            }
        }
        match &mut self.mode {
            Mode::Help => {
                self.mode = Mode::Normal;
                return Action::Stay;
            }
            Mode::Search(q) => {
                match k.code {
                    KeyCode::Esc => {
                        self.mode = Mode::Normal;
                        self.hits.clear();
                    }
                    KeyCode::Enter => {
                        self.mode = Mode::Normal;
                        self.hit_pos = 0;
                        if let Some(&i) = self.hits.first() {
                            self.select(i);
                            self.center_on_sel();
                        } else {
                            self.flash("no matches");
                        }
                    }
                    KeyCode::Backspace => {
                        q.pop();
                        self.update_hits();
                    }
                    KeyCode::Char(c) => {
                        q.push(c);
                        self.update_hits();
                    }
                    _ => {}
                }
                return Action::Stay;
            }
            Mode::Normal | Mode::Keys => {}
        }

        // A command whose guard fails leaves the key unused (n / N without search hits).
        if let Some(a) = self.keys.get(k).filter(|&a| self.enabled(a)) {
            return self.run_action(a);
        }
        if let Some(a) = self.nav.1.get(k) {
            return Action::History(a);
        }
        let shift = k.modifiers.contains(KeyModifiers::SHIFT);
        match k.code {
            // Pan: Shift+arrows or HJKL
            KeyCode::Left if shift => self.pan(-6.0, 0.0),
            KeyCode::Right if shift => self.pan(6.0, 0.0),
            KeyCode::Up if shift => self.pan(0.0, 3.0),
            KeyCode::Down if shift => self.pan(0.0, -3.0),
            KeyCode::Char('H') => self.pan(-6.0, 0.0),
            KeyCode::Char('L') => self.pan(6.0, 0.0),
            KeyCode::Char('K') => self.pan(0.0, 3.0),
            KeyCode::Char('J') => self.pan(0.0, -3.0),
            // Move selection: arrows or hjkl
            KeyCode::Left | KeyCode::Char('h') => self.select_dir(-1.0, 0.0),
            KeyCode::Right | KeyCode::Char('l') => self.select_dir(1.0, 0.0),
            KeyCode::Up | KeyCode::Char('k') => self.select_dir(0.0, 1.0),
            KeyCode::Down | KeyCode::Char('j') => self.select_dir(0.0, -1.0),
            _ => {}
        }
        Action::Stay
    }

    /// The which-key popup's lines: the commands that can run now, and Back / Forward.
    fn key_hints(&self) -> Vec<KeyHint> {
        let show = |a| a != GraphAction::Keys && self.enabled(a);
        commands::screen_key_hints(commands::GRAPH, &self.keys, show, Some(&self.nav))
    }

    fn enabled(&self, a: GraphAction) -> bool {
        match a {
            GraphAction::NextMatch | GraphAction::PreviousMatch => !self.hits.is_empty(),
            _ => true,
        }
    }

    fn run_action(&mut self, a: GraphAction) -> Action {
        match a {
            GraphAction::Quit => return Action::Leave,
            GraphAction::Escape => {
                if self.local {
                    self.local = false;
                    self.auto_fit = true;
                } else {
                    self.sel = None;
                    self.hits.clear();
                }
            }
            GraphAction::Help => self.mode = Mode::Help,
            GraphAction::Keys => self.mode = Mode::Keys,
            GraphAction::Search => {
                self.mode = Mode::Search(String::new());
                self.hits.clear();
            }
            GraphAction::ZoomIn => self.zoom_by(1.25, None),
            GraphAction::ZoomOut => self.zoom_by(0.8, None),
            GraphAction::Fit => self.auto_fit = true,
            GraphAction::Center => self.center_on_sel(),
            GraphAction::CycleLinks => self.cycle_links(false),
            GraphAction::CycleLinksBack => self.cycle_links(true),
            GraphAction::Back => {
                while let Some(key) = self.history.pop() {
                    if let Some(i) = self.graph.find(&key) {
                        self.sel = Some(i);
                        if self.local {
                            self.auto_fit = true;
                        } else {
                            self.ensure_on_screen(i);
                        }
                        break;
                    }
                }
            }
            GraphAction::NextMatch | GraphAction::PreviousMatch => {
                let len = self.hits.len();
                self.hit_pos = if a == GraphAction::NextMatch {
                    (self.hit_pos + 1) % len
                } else {
                    (self.hit_pos + len - 1) % len
                };
                self.select(self.hits[self.hit_pos]);
                self.center_on_sel();
                self.flash(format!("match {}/{}", self.hit_pos + 1, len));
            }
            GraphAction::Local => {
                if self.sel.is_none() {
                    self.select_default();
                }
                self.local = !self.local;
                self.auto_fit = true;
            }
            GraphAction::DepthMore => {
                self.depth = (self.depth + 1).min(5);
                self.auto_fit = true;
                self.flash(format!("local graph depth {}", self.depth));
            }
            GraphAction::DepthLess => {
                self.depth = (self.depth - 1).max(1);
                self.auto_fit = true;
                self.flash(format!("local graph depth {}", self.depth));
            }
            GraphAction::Pixels => self.toggle_pixels(),
            GraphAction::Labels => {
                self.labels = match self.labels {
                    Labels::Auto => Labels::All,
                    Labels::All => Labels::Off,
                    Labels::Off => Labels::Auto,
                };
                self.flash(match self.labels {
                    Labels::Auto => "labels: auto",
                    Labels::All => "labels: all",
                    Labels::Off => "labels: selection only",
                });
            }
            GraphAction::Pause => {
                self.paused = !self.paused;
                if !self.paused {
                    self.graph.reheat(0.5);
                }
            }
            GraphAction::OpenNote
            | GraphAction::OpenInObsidian
            | GraphAction::Boards
            | GraphAction::Groups
            | GraphAction::ToggleTags
            | GraphAction::ToggleUnresolved
            | GraphAction::ToggleOrphans
            | GraphAction::Rescan => return Action::Command(a),
        }
        Action::Stay
    }

    fn cycle_links(&mut self, back: bool) {
        let Some(s) = self.sel else {
            return self.select_default();
        };
        // Cycle through the neighbours of the previously selected node.
        let anchor = self
            .history
            .last()
            .and_then(|k| self.graph.find(k))
            .filter(|&a| self.graph.adj[a].contains(&s));
        let (base, cur) = match anchor {
            Some(a) => (a, Some(s)),
            None => (s, None),
        };
        let nb = self.neighbours_sorted(base);
        if nb.is_empty() {
            return self.flash("no links");
        }
        let pos = cur.and_then(|c| nb.iter().position(|&x| x == c));
        let next = match (pos, back) {
            (Some(p), false) => nb[(p + 1) % nb.len()],
            (Some(p), true) => nb[(p + nb.len() - 1) % nb.len()],
            (None, false) => nb[0],
            (None, true) => nb[nb.len() - 1],
        };
        // Keep the anchor as the history head so repeated Tab keeps cycling.
        self.sel = Some(next);
        if anchor.is_none() {
            self.history.push(self.graph.nodes[base].key.clone());
        }
        if !self.local {
            self.ensure_on_screen(next);
        }
    }

    fn toggle_pixels(&mut self) {
        if self.kitty.is_none() {
            self.kitty = Kitty::new();
        }
        if self.kitty.is_none() {
            return self.flash(
                "pixel mode needs a terminal that reports its pixel size (Ghostty, Kitty, WezTerm)",
            );
        }
        // Keep the same visible region when the vertical scale changes.
        self.pending_image = self.hide();
        self.pixels = !self.pixels;
        self.flash(if self.pixels {
            "pixel rendering (Kitty graphics)"
        } else {
            "braille rendering"
        });
    }

    fn center_on_sel(&mut self) {
        if let Some(i) = self.sel {
            self.auto_fit = false;
            self.cam = (self.graph.nodes[i].x, self.graph.nodes[i].y);
        }
    }

    fn update_hits(&mut self) {
        let Mode::Search(q) = &self.mode else { return };
        let q = workspace::search_key(q);
        self.hits.clear();
        if q.is_empty() {
            return;
        }
        let mut hits: Vec<(usize, bool)> = (self.scene.nodes.iter())
            .enumerate()
            .filter_map(|(i, n)| {
                let l = workspace::search_key(&n.label);
                let in_path =
                    (n.file.as_ref()).is_some_and(|r| workspace::search_key(r).contains(&q));
                (l.contains(&q) || in_path).then_some((i, l.starts_with(&q)))
            })
            .collect();
        hits.sort_by_key(|&(i, prefix)| (!prefix, std::cmp::Reverse(self.graph.degree(i))));
        self.hits = hits.into_iter().map(|(i, _)| i).collect();
    }

    pub fn mouse(&mut self, m: MouseEvent) {
        if !matches!(self.mode, Mode::Normal) {
            return;
        }
        self.dirty = true;
        let world = self.screen_to_world(m.column, m.row);
        match m.kind {
            MouseEventKind::ScrollUp => self.zoom_by(1.15, world),
            MouseEventKind::ScrollDown => self.zoom_by(1.0 / 1.15, world),
            MouseEventKind::Down(MouseButton::Left) => {
                let Some((wx, wy)) = world else { return };
                match self.node_at(wx, wy) {
                    Some(i) => {
                        self.select(i);
                        self.graph.nodes[i].pinned = true;
                        self.drag = Some(Drag::Node { idx: i });
                    }
                    None => {
                        self.drag = Some(Drag::Pan {
                            col: m.column,
                            row: m.row,
                        })
                    }
                }
            }
            MouseEventKind::Drag(MouseButton::Left) => match self.drag {
                Some(Drag::Pan { col, row }) => {
                    let dx = col as f64 - m.column as f64;
                    let dy = m.row as f64 - row as f64;
                    self.pan(dx, dy);
                    self.drag = Some(Drag::Pan {
                        col: m.column,
                        row: m.row,
                    });
                }
                Some(Drag::Node { idx }) => {
                    if let Some((wx, wy)) = world {
                        self.auto_fit = false;
                        let n = &mut self.graph.nodes[idx];
                        n.x = wx;
                        n.y = wy;
                        self.graph.reheat(0.3);
                    }
                }
                None => {}
            },
            MouseEventKind::Up(_) => {
                if let Some(Drag::Node { idx }) = self.drag.take()
                    && let Some(n) = self.graph.nodes.get_mut(idx)
                {
                    n.pinned = false;
                }
            }
            _ => {}
        }
    }

    // ---------- drawing ----------

    fn base_color(&self, i: usize) -> Color {
        if let Some(c) = self.group_color(i) {
            return c;
        }
        match self.scene.nodes[i].look {
            Look::Note => NOTE,
            Look::Ghost => GHOST,
            Look::Tag => TAG,
        }
    }

    fn group_color(&self, i: usize) -> Option<Color> {
        let g = self.scene.nodes.get(i)?.group?;
        self.colors.get(g).copied()
    }

    fn frame(&self) -> Drawing {
        let view @ (x0, x1, y0, y1) = self.view();
        let g = &self.graph;
        let sel = self.sel;
        let is_near = |i: usize| sel.is_some_and(|s| s == i || g.adj[s].contains(&i));
        let in_view =
            |x: f64, y: f64, m: f64| x >= x0 - m && x <= x1 + m && y >= y0 - m && y <= y1 + m;
        let hit = |i: usize| self.hits.contains(&i);
        let dim_others = (sel.is_some() && !self.local) || !self.hits.is_empty();
        let disc = |i: usize, color: Color, ring: Option<Color>| Disc {
            x: g.nodes[i].x,
            y: g.nodes[i].y,
            r: self.node_radius(i),
            color,
            ring,
        };

        let mut back = Layer::default();
        let mut front = Layer::default();
        for &(a, b) in &g.edges {
            if !self.visible[a] || !self.visible[b] {
                continue;
            }
            let (na, nb) = (&g.nodes[a], &g.nodes[b]);
            if !in_view(na.x, na.y, 0.0) && !in_view(nb.x, nb.y, 0.0) {
                // Both ends off-screen: skip unless the line crosses the view (cheap bbox test).
                let (lx, hx) = (na.x.min(nb.x), na.x.max(nb.x));
                let (ly, hy) = (na.y.min(nb.y), na.y.max(nb.y));
                if hx < x0 || lx > x1 || hy < y0 || ly > y1 {
                    continue;
                }
            }
            let touches = sel.is_some_and(|s| a == s || b == s);
            let edge = Line2 {
                a: (na.x, na.y),
                b: (nb.x, nb.y),
                color: if touches { ACCENT } else { EDGE },
                bold: touches,
            };
            if touches {
                front.edges.push(edge)
            } else {
                back.edges.push(edge)
            }
        }
        for i in 0..g.nodes.len() {
            if !self.visible[i]
                || is_near(i)
                || hit(i)
                || !in_view(g.nodes[i].x, g.nodes[i].y, 10.0)
            {
                continue;
            }
            let c = self.base_color(i);
            back.discs
                .push(disc(i, if dim_others { dim(c) } else { c }, None));
        }
        if let Some(s) = sel {
            for &j in &g.adj[s] {
                if self.visible[j] {
                    let c = match (self.group_color(j), g.nodes[j].kind) {
                        (Some(c), _) => c,
                        (None, Kind::Tag) => TAG,
                        _ => ACCENT_SOFT,
                    };
                    front.discs.push(disc(j, c, None));
                }
            }
        }
        for &i in &self.hits {
            if self.visible[i] && Some(i) != sel {
                front.discs.push(disc(i, HIT, None));
            }
        }
        if let Some(s) = sel {
            let ring = self.pixels.then_some(ACCENT);
            let c = if self.pixels {
                self.group_color(s).unwrap_or(ACCENT)
            } else {
                ACCENT
            };
            front.discs.push(disc(s, c, ring));
        }

        // Labels.
        let n_visible = self.visible.iter().filter(|v| **v).count();
        let show_all = match self.labels {
            Labels::All => true,
            Labels::Off => false,
            Labels::Auto => self.zoom >= 1.4 || (self.local && n_visible < 40),
        };
        let mut labels = Vec::new();
        for i in 0..g.nodes.len() {
            let n = &g.nodes[i];
            if !self.visible[i] || !in_view(n.x, n.y, 0.0) {
                continue;
            }
            let near = is_near(i);
            let important = self.labels == Labels::Auto && g.degree(i) >= 8 && self.zoom >= 0.5;
            if !(show_all || near || hit(i) || important || Some(i) == sel) {
                continue;
            }
            // A node's label is a file's name or an extension's words: escaped before it
            // is cut and drawn.
            let label =
                cold_lava_views::shown::escape(&n.label, cold_lava_views::shown::Level::Content);
            let mut text: String = label.chars().take(28).collect();
            if label.chars().count() > 28 {
                text.push('…');
            }
            let style = if Some(i) == sel {
                Style::new().fg(ACCENT).add_modifier(Modifier::BOLD)
            } else if hit(i) {
                Style::new().fg(HIT)
            } else if near {
                Style::new().fg(ACCENT_SOFT)
            } else if dim_others {
                Style::new().fg(DIM)
            } else {
                Style::new().fg(MUTED)
            };
            labels.push(Label {
                x: n.x,
                y: n.y,
                r: self.node_radius(i),
                text,
                style,
            });
        }
        // Most important first: labels that would overlap an earlier one are skipped.
        labels.sort_by_key(|l| match l.style.fg {
            Some(ACCENT) => 0,
            Some(HIT) => 1,
            Some(ACCENT_SOFT) => 2,
            Some(MUTED) => 3,
            _ => 4,
        });
        Drawing {
            view,
            zoom: self.zoom,
            layers: vec![back, front],
            labels,
        }
    }

    /// Draws the view over `area`; `statuses` are the extensions' words for the status line.
    pub fn draw(&mut self, f: &mut Frame, area: Rect, statuses: &[String]) {
        let [main, status] =
            Layout::vertical([Constraint::Min(3), Constraint::Length(2)]).areas(area);

        let mut flags = Vec::new();
        if self.local {
            flags.push(format!("local·{}", self.depth));
        }
        flags.extend(self.scene.flags.iter().cloned());
        if self.paused {
            flags.push("paused".into());
        }
        flags.push(if self.pixels {
            "pixels".into()
        } else {
            "braille".into()
        });
        let shown = self.visible.iter().filter(|v| **v).count();
        let shown_links = (self.graph.edges.iter())
            .filter(|&&(a, b)| self.visible[a] && self.visible[b])
            .count();
        let title = Line::from(vec![
            Span::styled(
                " cold-lava ",
                Style::new().fg(ACCENT).add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!(
                    "{} · {} nodes · {} links ",
                    self.scene.title, shown, shown_links
                ),
                Style::new().fg(MUTED),
            ),
        ]);
        let block = Block::bordered()
            .border_type(BorderType::Rounded)
            .border_style(Style::new().fg(DIM))
            .title(title)
            .title_top(
                Line::from(Span::styled(
                    format!(" {} ", flags.join(" · ")),
                    Style::new().fg(ACCENT_SOFT),
                ))
                .right_aligned(),
            );
        let inner = block.inner(main);
        if inner != self.area {
            self.dirty = true;
        }
        self.area = inner;
        let scene = self.frame();
        if scene.view != self.last_view {
            // Camera moved (zoom, pan, easing to fit): the image must follow the text labels.
            self.dirty = true;
            self.last_view = scene.view;
        }

        if self.pixels {
            block.render(main, f.buffer_mut());
            if self.dirty {
                if let Some(k) = self.kitty.as_mut() {
                    self.pending_image = k.frame(&scene, inner);
                }
                self.dirty = false;
            }
        } else {
            let (x0, x1, y0, y1) = scene.view;
            let canvas = Canvas::default()
                .block(block)
                .marker(Marker::Braille)
                .x_bounds([x0, x1])
                .y_bounds([y0, y1])
                .paint(|ctx| scene.paint_braille(ctx));
            f.render_widget(canvas, main);
        }
        scene.draw_labels(f.buffer_mut(), inner);
        self.draw_legend(f, inner);

        self.draw_status(f, status, statuses);
        match self.mode {
            Mode::Help => {
                self.hide_image_later();
                draw_help(f, main, &self.keys, &self.nav.1);
            }
            Mode::Keys => {
                self.hide_image_later();
                crate::whichkey::draw(f, main, &self.key_hints());
            }
            _ => {}
        }
    }

    /// The help overlay sits above the image layer only if the image is removed.
    fn hide_image_later(&mut self) {
        if self.pixels {
            if let Some(k) = self.kitty.as_mut() {
                self.pending_image = k.clear();
            }
            self.dirty = true;
        }
    }

    fn draw_legend(&self, f: &mut Frame, area: Rect) {
        if self.scene.groups.is_empty() || area.width < 30 || area.height < 4 {
            return;
        }
        let rows: Vec<Line> = (self.scene.groups.iter())
            .enumerate()
            .map(|(gi, g)| {
                let mut label: String = g.label.chars().take(22).collect();
                if g.label.chars().count() > 22 {
                    label.push('…');
                }
                let count = (self.scene.nodes.iter())
                    .filter(|n| n.group == Some(gi))
                    .count();
                Line::from(vec![
                    Span::styled("● ", Style::new().fg(self.colors[gi])),
                    Span::styled(label, Style::new().fg(NOTE)),
                    Span::styled(format!(" {count}"), Style::new().fg(MUTED)),
                ])
            })
            .collect();
        let w = rows.iter().map(|l| l.width()).max().unwrap_or(0) as u16 + 1;
        let h = (rows.len() as u16).min(area.height - 1);
        let r = Rect::new(area.x + area.width.saturating_sub(w + 1), area.y, w, h);
        f.render_widget(Paragraph::new(rows), r);
    }

    fn draw_status(&mut self, f: &mut Frame, area: Rect, statuses: &[String]) {
        let g = &self.graph;
        let mut info = match self.sel {
            None => Line::from(Span::styled(
                "  nothing selected — arrows/hjkl or Tab to pick a note, / to search, ? for help",
                Style::new().fg(MUTED),
            )),
            Some(i) => {
                let n = &self.scene.nodes[i];
                let icon = match n.look {
                    Look::Note => "●",
                    Look::Ghost => "○",
                    Look::Tag => "#",
                };
                let group = (n.group)
                    .and_then(|gi| self.scene.groups.get(gi))
                    .map(|gr| format!("  [{}]", gr.label))
                    .unwrap_or_default();
                Line::from(vec![
                    Span::styled(
                        format!(" {icon} "),
                        Style::new().fg(self.group_color(i).unwrap_or(ACCENT)),
                    ),
                    Span::styled(
                        n.label.clone(),
                        Style::new().fg(NOTE).add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(
                        format!("  ←{} in  {}→ out  ", g.links_in[i], g.links_out[i]),
                        Style::new().fg(ACCENT_SOFT),
                    ),
                    Span::styled(n.detail.clone().unwrap_or_default(), Style::new().fg(MUTED)),
                    Span::styled(group, Style::new().fg(MUTED)),
                ])
            }
        };
        if self
            .msg
            .as_ref()
            .is_some_and(|(_, t)| t.elapsed() > Duration::from_secs(3))
        {
            self.msg = None;
        }
        let second = match (&self.mode, &self.msg) {
            (Mode::Search(q), _) => {
                let preview: Vec<String> = (self.hits.iter())
                    .take(6)
                    .map(|&i| self.scene.nodes[i].label.clone())
                    .collect();
                Line::from(vec![
                    Span::styled(format!(" /{q}▏"), Style::new().fg(HIT)),
                    Span::styled(
                        format!("  {} matches  {}", self.hits.len(), preview.join(" · ")),
                        Style::new().fg(MUTED),
                    ),
                ])
            }
            (_, Some((m, _))) => Line::from(Span::styled(format!(" {m}"), Style::new().fg(HIT))),
            _ => {
                let k = |id| self.keys.first_label(id);
                Line::from(Span::styled(
                    format!(
                        " ↑↓←→ move  {} links  {} open  {} search  {} keys  {} help  {} quit",
                        k("graph.cycle-links"),
                        k("graph.open-note"),
                        k("graph.search"),
                        k("graph.keys"),
                        k("graph.help"),
                        k("graph.quit"),
                    ),
                    Style::new().fg(DIM),
                ))
            }
        };
        for text in statuses {
            info.spans.push(Span::styled(
                format!("   {text}"),
                Style::new().fg(ACCENT_SOFT),
            ));
        }
        let (nav, keys) = &self.nav;
        let first = |id| keys.keys(id).first();
        let hint = nav.hint(first("history.back"), first("history.forward"));
        if !hint.is_empty() {
            info.spans
                .push(Span::styled(format!("   {hint}"), Style::new().fg(MUTED)));
        }
        f.render_widget(Paragraph::new(vec![info, second]), area);
    }
}

/// The layout for `scene`'s nodes, in its order, starting where `prev` has them (by id). Links
/// to ids the scene doesn't have, links to themselves and repeats are left out.
fn layout(scene: &Scene, prev: &HashMap<String, (f64, f64)>) -> Graph {
    let index: HashMap<&str, usize> = (scene.nodes.iter())
        .enumerate()
        .map(|(i, n)| (n.id.as_str(), i))
        .collect();
    let nodes = (scene.nodes.iter())
        .enumerate()
        .map(|(i, n)| {
            let kind = match n.look {
                Look::Note => Kind::Note,
                Look::Ghost => Kind::Unresolved,
                Look::Tag => Kind::Tag,
            };
            let at = prev.get(&n.id).copied();
            graph::Node::new(
                n.id.clone(),
                n.label.clone(),
                kind,
                at.unwrap_or(graph::start(i)),
            )
        })
        .collect();
    let mut seen = HashSet::new();
    let edges = (scene.edges.iter())
        .filter_map(|e| Some((*index.get(e.from.as_str())?, *index.get(e.to.as_str())?)))
        .filter(|&(a, b)| a != b && seen.insert((a, b)))
        .collect();
    Graph::from_parts(nodes, edges)
}

fn colors(scene: &Scene) -> Vec<Color> {
    (scene.groups.iter())
        .map(|g| parse_color(&g.color).unwrap_or(NOTE))
        .collect()
}

/// `#rrggbb` → a colour.
pub fn parse_color(s: &str) -> Option<Color> {
    let hex = s.strip_prefix('#').filter(|h| h.len() == 6)?;
    let rgb = u32::from_str_radix(hex, 16).ok()?;
    Some(Color::Rgb((rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8))
}

/// A colour as `#rrggbb`; colours that aren't RGB are grey.
pub fn color_hex(c: Color) -> String {
    match c {
        Color::Rgb(r, g, b) => format!("#{r:02x}{g:02x}{b:02x}"),
        _ => "#cdcdd7".to_string(),
    }
}

/// Blend a colour toward the background (for de-emphasised nodes).
fn dim(c: Color) -> Color {
    match c {
        Color::Rgb(r, g, b) => {
            let mix = |v: u8, bg: u8| (v as f64 * 0.38 + bg as f64 * 0.62) as u8;
            Color::Rgb(mix(r, 30), mix(g, 30), mix(b, 38))
        }
        c => c,
    }
}

fn draw_help(
    f: &mut Frame,
    area: Rect,
    keys: &Keymap<GraphAction>,
    history: &Keymap<HistoryAction>,
) {
    let fixed: &[(&str, &str)] = &[
        ("↑↓←→ / hjkl", "select the nearest note in that direction"),
        ("Shift+arrows / HJKL", "pan"),
        ("scroll", "zoom at the cursor"),
        ("drag", "move a node, or pan on empty space"),
    ];
    let commands = commands::GRAPH.iter().map(|c| (keys.label(c.id), c.title));
    let rows: Vec<(String, &str)> = fixed
        .iter()
        .map(|&(k, d)| (k.to_string(), d))
        .chain(commands)
        .chain(commands::history_help(history))
        .collect();
    let lines: Vec<Line> = rows
        .iter()
        .map(|(k, d)| {
            Line::from(vec![
                Span::styled(format!("{k:>20}  "), Style::new().fg(ACCENT)),
                Span::styled(*d, Style::new().fg(NOTE)),
            ])
        })
        .collect();
    let w = 72.min(area.width.saturating_sub(4));
    let h = (rows.len() as u16 + 2).min(area.height.saturating_sub(2));
    let r = Rect::new(
        area.x + (area.width - w) / 2,
        area.y + (area.height - h) / 2,
        w,
        h,
    );
    f.render_widget(Clear, r);
    f.render_widget(
        Paragraph::new(lines).wrap(Wrap { trim: false }).block(
            Block::bordered()
                .border_type(BorderType::Rounded)
                .border_style(Style::new().fg(ACCENT))
                .title(" keys · any key closes "),
        ),
        r,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(id: &str) -> Node {
        Node {
            id: id.into(),
            label: id.to_uppercase(),
            look: Look::Note,
            group: None,
            file: Some(format!("{id}.md")),
            detail: None,
        }
    }

    fn scene(ids: &[&str], edges: &[(&str, &str)]) -> Scene {
        Scene {
            title: "w".into(),
            nodes: ids.iter().map(|id| node(id)).collect(),
            edges: (edges.iter())
                .map(|&(from, to)| Edge {
                    from: from.into(),
                    to: to.into(),
                })
                .collect(),
            ..Scene::default()
        }
    }

    fn press(code: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, modifiers)
    }

    fn at(v: &SceneView, id: &str) -> (f64, f64) {
        let n = &v.graph.nodes[v.graph.find(id).unwrap()];
        (n.x, n.y)
    }

    #[test]
    fn links_to_nodes_it_lacks_to_themselves_or_twice_are_left_out() {
        let s = scene(
            &["a", "b", "c"],
            &[("a", "b"), ("a", "b"), ("b", "b"), ("a", "zz"), ("c", "a")],
        );
        let g = layout(&s, &HashMap::new());
        assert_eq!(g.edges, [(0, 1), (2, 0)]);
        assert_eq!((g.links_out[0], g.links_in[0]), (1, 1));
    }

    #[test]
    fn an_update_keeps_positions_and_the_selection_by_id() {
        let mut v = SceneView::new(scene(&["a", "b"], &[("a", "b")]), Render::Braille);
        v.select_id(Some("b"));
        for _ in 0..30 {
            v.tick();
        }
        let (a, b) = (at(&v, "a"), at(&v, "b"));
        v.update(scene(&["new", "b", "a"], &[("a", "b"), ("new", "a")]));
        assert_eq!((at(&v, "a"), at(&v, "b")), (a, b));
        assert_eq!(v.selected().map(|n| n.id.as_str()), Some("b"));
        assert_eq!(v.graph.alpha, 0.6, "a new node: the layout moves again");
    }

    #[test]
    fn an_update_of_looks_only_keeps_the_layout_still() {
        let mut v = SceneView::new(scene(&["a", "b"], &[("a", "b")]), Render::Braille);
        let alpha = v.graph.alpha;
        let mut looks = scene(&["a", "b"], &[("a", "b")]);
        looks.groups.push(Group {
            label: "Volcanoes".into(),
            color: "#ff8000".into(),
        });
        looks.nodes[0].group = Some(0);
        v.update(looks);
        assert_eq!(v.graph.alpha, alpha);
        assert_eq!(v.group_color(0), Some(Color::Rgb(255, 128, 0)));
        assert_eq!(v.group_color(1), None);
    }

    #[test]
    fn keys_whose_meaning_is_the_senders_come_back_as_commands() {
        let mut v = SceneView::new(scene(&["a", "b"], &[("a", "b")]), Render::Braille);
        let none = KeyModifiers::NONE;
        assert_eq!(
            v.key(press(KeyCode::Enter, none)),
            Action::Command(GraphAction::OpenNote)
        );
        assert_eq!(
            v.key(press(KeyCode::Char('t'), none)),
            Action::Command(GraphAction::ToggleTags)
        );
        assert_eq!(v.key(press(KeyCode::Tab, none)), Action::Stay);
        assert!(v.selected().is_some(), "Tab picks a note");
        assert_eq!(
            v.key(press(KeyCode::Char('o'), KeyModifiers::CONTROL)),
            Action::History(HistoryAction::Back)
        );
        assert_eq!(v.key(press(KeyCode::Char('q'), none)), Action::Leave);
    }

    #[test]
    fn group_colours_are_hex() {
        assert_eq!(parse_color("#a78bfa"), Some(Color::Rgb(167, 139, 250)));
        assert_eq!(color_hex(Color::Rgb(167, 139, 250)), "#a78bfa");
        assert_eq!(parse_color("a78bfa"), None);
        assert_eq!(parse_color("#a78bf"), None);
    }

    #[test]
    fn in_pixels_a_frame_is_drawn_once_until_something_moves_and_hidden_on_leaving() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;
        let mut v = SceneView::new(scene(&["a", "b"], &[("a", "b")]), Render::Braille);
        (v.kitty, v.pixels) = (Some(Kitty::with_cells(8.0, 16.0)), true);
        v.paused = true;
        let mut t = Terminal::new(TestBackend::new(60, 20)).unwrap();
        let mut frame = |v: &mut SceneView| {
            v.tick();
            t.draw(|f| v.draw(f, f.area(), &[])).unwrap();
            String::from_utf8(v.take_image()).unwrap()
        };
        assert!(frame(&mut v).contains("a=T"), "the first frame");
        while v.fitting {
            frame(&mut v);
        }
        frame(&mut v);
        assert_eq!(frame(&mut v), "", "nothing moved: nothing sent");
        v.key(press(KeyCode::Char('+'), KeyModifiers::NONE));
        assert!(frame(&mut v).contains("a=T"), "zoomed: a new frame");
        assert!(String::from_utf8(v.hide()).unwrap().contains("a=d"));
        assert!(frame(&mut v).contains("a=T"), "shown again");
    }
}
