//! The graph view: state, input handling and drawing.

use crate::commands::{self, GraphAction, HistoryAction, Keymap};
use crate::core::{Core, Event as CoreEvent};
use crate::graph::{self, BuildOpts, Graph, Kind};
use crate::groups::{self, Assignment};
use crate::history::{Exit, Nav};
use crate::kitty::{self, Kitty};
use crate::scene::{Disc, Edge, Label, Layer, Scene};
use crate::vault;
use crate::{board, term};
use anyhow::Result;
use ratatui::crossterm::event::{
    self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent,
    MouseEventKind,
};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::symbols::Marker;
use ratatui::text::{Line, Span};
use ratatui::widgets::canvas::Canvas;
use ratatui::widgets::{Block, BorderType, Clear, Paragraph, Widget, Wrap};
use ratatui::{DefaultTerminal, Frame};
use std::collections::{HashMap, VecDeque};
use std::time::{Duration, Instant};

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
}

enum Drag {
    Pan { col: u16, row: u16 },
    Node { idx: usize },
}

pub struct App {
    /// The live vault (shared with the main menu, which hands it over and gets it back).
    core: Core,
    graph: Graph,
    opts: BuildOpts,
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
    quit: bool,
    keys: Keymap<GraphAction>,
    /// Inside the main menu: where Back and Forward lead, and their keys. Then the menu, not
    /// the graph, opens boards.
    nav: Option<(Nav, Keymap<HistoryAction>)>,
    exit: Exit,
    // Colour groups
    group_mode: groups::Mode,
    obsidian_groups: Vec<groups::Group>,
    assign: Assignment,
    // Pixel rendering
    kitty: Option<Kitty>,
    pixels: bool,
    dirty: bool,
    pending_image: Vec<u8>,
    // Live updates when notes change on disk
    /// The view the last pixel frame was rendered for; any camera move needs a new frame.
    last_view: (f64, f64, f64, f64),
    /// The camera is still easing toward a fit.
    fitting: bool,
}

#[derive(Clone, Copy)]
pub enum Render {
    Auto,
    Braille,
    Pixels,
}

impl App {
    pub fn new(core: Core, render: Render) -> App {
        let vault = &core.index.vault;
        let opts = BuildOpts {
            unresolved: false,
            tags: false,
            orphans: true,
        };
        let graph = graph::build(vault, opts, &HashMap::new());
        let n = graph.nodes.len();
        let obsidian_groups = groups::load_obsidian(&vault.root);
        let group_mode = if obsidian_groups.is_empty() {
            groups::Mode::Off
        } else {
            groups::Mode::Obsidian
        };
        let assign = groups::assign(group_mode, &graph, &obsidian_groups);
        let kitty = match render {
            Render::Braille => None,
            Render::Auto if !kitty::supported() => None,
            _ => Kitty::new(),
        };
        let pixels = kitty.is_some();
        App {
            core,
            graph,
            opts,
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
            quit: false,
            keys: Keymap::new(commands::GRAPH),
            nav: None,
            exit: Exit::Quit,
            group_mode,
            obsidian_groups,
            assign,
            kitty,
            pixels,
            dirty: true,
            pending_image: Vec::new(),
            last_view: (0.0, 0.0, 0.0, 0.0),
            fitting: true,
        }
    }

    pub fn run(&mut self, terminal: &mut DefaultTerminal) -> Result<()> {
        // Let the layout untangle a bit before the first frame.
        for _ in 0..40 {
            self.graph.step(&self.visible);
        }
        while !self.quit {
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
            terminal.draw(|f| self.draw(f))?;
            term::write_raw(&std::mem::take(&mut self.pending_image));
            self.check_disk();

            let idle = (self.paused || self.graph.settled()) && !(self.auto_fit && self.fitting);
            let ms = if idle && self.drag.is_none() {
                250
            } else if self.pixels {
                25
            } else {
                16
            };
            if event::poll(Duration::from_millis(ms))? {
                loop {
                    match event::read()? {
                        Event::Key(k) if k.kind == KeyEventKind::Press => {
                            self.on_key(k, terminal)?
                        }
                        Event::Mouse(m) => self.on_mouse(m),
                        Event::Resize(..) => {
                            if let Some(k) = self.kitty.as_mut() {
                                k.measure();
                            }
                        }
                        _ => {}
                    }
                    self.dirty = true;
                    if self.quit || !event::poll(Duration::ZERO)? {
                        break;
                    }
                }
            }
        }
        if let Some(k) = self.kitty.as_mut() {
            term::write_raw(&k.clear());
        }
        Ok(())
    }

    /// Rescan shortly after notes change on disk (debounced so a burst of saves rescans once).
    /// Follow changes on disk through the core's events.
    fn check_disk(&mut self) {
        if self.drag.is_some() {
            return; // don't move the graph under the mouse
        }
        let events = self.core.poll();
        if events.is_empty() {
            return;
        }
        if events
            .iter()
            .any(|e| matches!(e, CoreEvent::GraphSettingsChanged | CoreEvent::Rescanned))
        {
            self.obsidian_groups = groups::load_obsidian(&self.core.index.vault.root);
        }
        let count = |pred: fn(&CoreEvent) -> bool| events.iter().filter(|e| pred(e)).count();
        let added = count(|e| matches!(e, CoreEvent::NoteCreated(_)));
        let removed = count(|e| matches!(e, CoreEvent::NoteRemoved(_)));
        self.rebuild();
        self.flash(match (added, removed) {
            (0, 0) => "vault changed: graph updated".to_string(),
            (a, 0) => format!("vault changed: +{a} notes"),
            (0, r) => format!("vault changed: −{r} notes"),
            (a, r) => format!("vault changed: +{a} −{r} notes"),
        });
        self.dirty = true;
    }

    /// Run inside the main menu: the history keys leave the graph for `nav`'s places, and
    /// `selected` (a node key) starts selected.
    pub fn navigate(&mut self, nav: Nav, selected: Option<&str>) {
        self.nav = Some((nav, commands::history_keys(&self.keys)));
        if let Some(i) = selected.and_then(|key| self.graph.find(key)) {
            self.sel = Some(i);
        }
    }

    pub fn selected_key(&self) -> Option<String> {
        self.sel.map(|i| self.graph.nodes[i].key.clone())
    }

    /// How the graph was left.
    pub fn exit(&self) -> Exit {
        self.exit.clone()
    }

    /// Give the live vault back (to the main menu).
    pub fn into_core(self) -> Core {
        self.core
    }

    // ---------- graph state ----------

    /// Rebuild the graph from the core's current notes, keeping node positions.
    fn rebuild(&mut self) {
        let prev = self.graph.positions();
        let prev_aspect = self.graph.aspect;
        let sel_key = self.sel.map(|i| self.graph.nodes[i].key.clone());
        self.graph = graph::build(&self.core.index.vault, self.opts, &prev);
        self.graph.alpha = 0.6;
        self.graph.aspect = prev_aspect;
        self.sel = sel_key.and_then(|k| self.graph.find(&k));
        self.hits.clear();
        self.visible = vec![true; self.graph.nodes.len()];
        self.regroup();
    }

    fn regroup(&mut self) {
        self.assign = groups::assign(self.group_mode, &self.graph, &self.obsidian_groups);
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
        self.regroup();
        let n = self.assign.groups.len();
        self.flash(match self.group_mode {
            Off => "colour groups: off".to_string(),
            m => format!("colour groups: {} ({n} groups)", m.name()),
        });
    }

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

    fn flash(&mut self, s: impl Into<String>) {
        self.msg = Some((s.into(), Instant::now()));
    }

    fn on_key(&mut self, k: KeyEvent, terminal: &mut DefaultTerminal) -> Result<()> {
        match &mut self.mode {
            Mode::Help => {
                self.mode = Mode::Normal;
                return Ok(());
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
                return Ok(());
            }
            Mode::Normal => {}
        }

        // A command whose guard fails leaves the key unused (n / N without search hits).
        if let Some(a) = self.keys.get(k).filter(|&a| self.enabled(a)) {
            return self.run_action(a, terminal);
        }
        if let Some((nav, keys)) = &self.nav
            && let Some(a) = keys.get(k)
        {
            let (exit, to) = match a {
                HistoryAction::Back => (Exit::Back, &nav.back),
                HistoryAction::Forward => (Exit::Forward, &nav.forward),
            };
            if to.is_some() {
                self.exit = exit;
                self.quit = true;
            }
            return Ok(());
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
        Ok(())
    }

    fn enabled(&self, a: GraphAction) -> bool {
        match a {
            GraphAction::NextMatch | GraphAction::PreviousMatch => !self.hits.is_empty(),
            _ => true,
        }
    }

    fn run_action(&mut self, a: GraphAction, terminal: &mut DefaultTerminal) -> Result<()> {
        match a {
            GraphAction::Quit => self.quit = true,
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
            GraphAction::OpenNote => self.open_in_editor(terminal)?,
            GraphAction::OpenInObsidian => self.open_in_obsidian(),
            GraphAction::Boards if self.nav.is_some() => {
                self.hide_image();
                if let Some(path) = board::pick_board(terminal, &self.core.index.vault.root)? {
                    self.exit = Exit::Board(path);
                    self.quit = true;
                }
                self.dirty = true;
            }
            GraphAction::Boards => {
                self.hide_image();
                board::launch(
                    terminal,
                    &self.core.index.vault.root,
                    None,
                    self.kitty.is_some(),
                )?;
                self.dirty = true;
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
            GraphAction::Groups => self.cycle_groups(),
            GraphAction::Pixels => self.toggle_pixels(),
            GraphAction::ToggleTags => {
                self.opts.tags = !self.opts.tags;
                self.rebuild();
                self.flash(if self.opts.tags {
                    "tags shown"
                } else {
                    "tags hidden"
                });
            }
            GraphAction::ToggleUnresolved => {
                self.opts.unresolved = !self.opts.unresolved;
                self.rebuild();
                self.flash(if self.opts.unresolved {
                    "unresolved links shown"
                } else {
                    "unresolved links hidden"
                });
            }
            GraphAction::ToggleOrphans => {
                self.opts.orphans = !self.opts.orphans;
                self.rebuild();
                self.flash(if self.opts.orphans {
                    "orphans shown"
                } else {
                    "orphans hidden"
                });
            }
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
            GraphAction::Rescan => self.core.rescan(), // picked up as a Rescanned event
        }
        Ok(())
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
        self.hide_image();
        self.pixels = !self.pixels;
        self.flash(if self.pixels {
            "pixel rendering (Kitty graphics)"
        } else {
            "braille rendering"
        });
    }

    fn hide_image(&mut self) {
        if let Some(k) = self.kitty.as_mut() {
            term::write_raw(&k.clear());
        }
    }

    fn center_on_sel(&mut self) {
        if let Some(i) = self.sel {
            self.auto_fit = false;
            self.cam = (self.graph.nodes[i].x, self.graph.nodes[i].y);
        }
    }

    fn update_hits(&mut self) {
        let Mode::Search(q) = &self.mode else { return };
        let q = vault::search_key(q);
        self.hits.clear();
        if q.is_empty() {
            return;
        }
        let mut hits: Vec<(usize, bool)> = self
            .graph
            .nodes
            .iter()
            .enumerate()
            .filter_map(|(i, n)| {
                let l = vault::search_key(&n.label);
                let in_path = n
                    .rel
                    .as_ref()
                    .is_some_and(|r| vault::search_key(r).contains(&q));
                (l.contains(&q) || in_path).then_some((i, l.starts_with(&q)))
            })
            .collect();
        hits.sort_by_key(|&(i, prefix)| (!prefix, std::cmp::Reverse(self.graph.degree(i))));
        self.hits = hits.into_iter().map(|(i, _)| i).collect();
    }

    fn on_mouse(&mut self, m: MouseEvent) {
        if !matches!(self.mode, Mode::Normal) {
            return;
        }
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

    // ---------- opening notes ----------

    fn open_in_editor(&mut self, terminal: &mut DefaultTerminal) -> Result<()> {
        let Some(i) = self.sel else {
            self.flash("select a note first (arrows, Tab, / or click)");
            return Ok(());
        };
        let node = &self.graph.nodes[i];
        let path = match node.kind {
            Kind::Note => node.path.clone().unwrap(),
            Kind::Unresolved => {
                // Like clicking an unresolved link in Obsidian: create the note.
                let p = self
                    .core
                    .index
                    .vault
                    .root
                    .join(format!("{}.md", node.label));
                crate::write::create_if_missing(&p, b"")?;
                p
            }
            Kind::Tag => {
                self.flash("that's a tag — press f to see its notes");
                return Ok(());
            }
        };
        self.hide_image();
        match term::edit_file(terminal, &path, &self.core.index.vault.root)? {
            None => self.core.note_changed(&path), // its links may have changed
            Some(err) => self.flash(err),
        }
        Ok(())
    }

    fn open_in_obsidian(&mut self) {
        let Some(rel) = self.sel.and_then(|i| self.graph.nodes[i].rel.clone()) else {
            return self.flash("select a note first");
        };
        let ok = term::open_in_obsidian(&self.core.index.vault.name, &rel);
        self.flash(if ok {
            "opened in Obsidian"
        } else {
            "couldn't launch Obsidian"
        });
    }

    // ---------- drawing ----------

    fn base_color(&self, i: usize) -> Color {
        if let Some(c) = self.assign.color(i) {
            return c;
        }
        match self.graph.nodes[i].kind {
            Kind::Note => NOTE,
            Kind::Unresolved => GHOST,
            Kind::Tag => TAG,
        }
    }

    fn scene(&self) -> Scene {
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
            let edge = Edge {
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
                    let c = match (self.assign.color(j), g.nodes[j].kind) {
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
                self.assign.color(s).unwrap_or(ACCENT)
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
            let mut text: String = n.label.chars().take(28).collect();
            if n.label.chars().count() > 28 {
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
        Scene {
            view,
            zoom: self.zoom,
            layers: vec![back, front],
            labels,
        }
    }

    fn draw(&mut self, f: &mut Frame) {
        let [main, status] =
            Layout::vertical([Constraint::Min(3), Constraint::Length(2)]).areas(f.area());

        let mut flags = Vec::new();
        if self.local {
            flags.push(format!("local·{}", self.depth));
        }
        if self.group_mode != groups::Mode::Off {
            flags.push(self.group_mode.name().to_string());
        }
        if self.opts.tags {
            flags.push("tags".into());
        }
        if self.opts.unresolved {
            flags.push("unresolved".into());
        }
        if !self.opts.orphans {
            flags.push("no orphans".into());
        }
        if self.paused {
            flags.push("paused".into());
        }
        flags.push(if self.pixels {
            "pixels".into()
        } else {
            "braille".into()
        });
        let shown = self.visible.iter().filter(|v| **v).count();
        let shown_links = self
            .graph
            .edges
            .iter()
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
                    self.core.index.vault.name, shown, shown_links
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
        let scene = self.scene();
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

        self.draw_status(f, status);
        if matches!(self.mode, Mode::Help) {
            self.hide_image_later();
            draw_help(f, main, &self.keys, self.nav.as_ref().map(|(_, k)| k));
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
        if self.assign.groups.is_empty() || area.width < 30 || area.height < 4 {
            return;
        }
        let rows: Vec<Line> = self
            .assign
            .groups
            .iter()
            .enumerate()
            .map(|(gi, g)| {
                let mut label: String = g.label.chars().take(22).collect();
                if g.label.chars().count() > 22 {
                    label.push('…');
                }
                Line::from(vec![
                    Span::styled("● ", Style::new().fg(g.color)),
                    Span::styled(label, Style::new().fg(NOTE)),
                    Span::styled(
                        format!(" {}", self.assign.count(gi)),
                        Style::new().fg(MUTED),
                    ),
                ])
            })
            .collect();
        let w = rows.iter().map(|l| l.width()).max().unwrap_or(0) as u16 + 1;
        let h = (rows.len() as u16).min(area.height - 1);
        let r = Rect::new(area.x + area.width.saturating_sub(w + 1), area.y, w, h);
        f.render_widget(Paragraph::new(rows), r);
    }

    fn draw_status(&mut self, f: &mut Frame, area: Rect) {
        let g = &self.graph;
        let mut info = match self.sel {
            None => Line::from(Span::styled(
                "  nothing selected — arrows/hjkl or Tab to pick a note, / to search, ? for help",
                Style::new().fg(MUTED),
            )),
            Some(i) => {
                let n = &g.nodes[i];
                let (icon, desc) = match n.kind {
                    Kind::Note => ("●", n.rel.clone().unwrap_or_default()),
                    Kind::Unresolved => ("○", "unresolved — Enter creates it".into()),
                    Kind::Tag => ("#", format!("tag on {} notes", g.links_in[i])),
                };
                let group = self.assign.of_node[i]
                    .map(|gi| format!("  [{}]", self.assign.groups[gi].label))
                    .unwrap_or_default();
                Line::from(vec![
                    Span::styled(
                        format!(" {icon} "),
                        Style::new().fg(self.assign.color(i).unwrap_or(ACCENT)),
                    ),
                    Span::styled(
                        n.label.clone(),
                        Style::new().fg(NOTE).add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(
                        format!("  ←{} in  {}→ out  ", g.links_in[i], g.links_out[i]),
                        Style::new().fg(ACCENT_SOFT),
                    ),
                    Span::styled(desc, Style::new().fg(MUTED)),
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
                let preview: Vec<String> = self
                    .hits
                    .iter()
                    .take(6)
                    .map(|&i| g.nodes[i].label.clone())
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
            _ => Line::from(Span::styled(
                " ↑↓←→ move  Tab links  ⏎ edit  / search  f local  g groups  w whiteboard  p pixels  ? help  q quit",
                Style::new().fg(DIM),
            )),
        };
        if let Some((nav, keys)) = &self.nav {
            let first = |id| keys.keys(id).first();
            let hint = nav.hint(first("history.back"), first("history.forward"));
            if !hint.is_empty() {
                info.spans
                    .push(Span::styled(format!("   {hint}"), Style::new().fg(MUTED)));
            }
        }
        f.render_widget(Paragraph::new(vec![info, second]), area);
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
    history: Option<&Keymap<HistoryAction>>,
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
        .chain(history.map(commands::history_help).unwrap_or_default())
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
