//! The graph view: state, input handling and drawing.

use crate::graph::{self, BuildOpts, Graph, Kind};
use crate::groups::{self, Assignment};
use crate::kitty::{self, Kitty};
use crate::scene::{Disc, Edge, Label, Layer, Scene};
use crate::vault::{self, Vault};
use crate::{board, term};
use anyhow::Result;
use notify::{RecommendedWatcher, RecursiveMode, Watcher};
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
use std::path::Path;
use std::sync::mpsc::{self, Receiver};
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
    vault: Vault,
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
    fs_events: Option<Receiver<notify::Event>>,
    _watcher: Option<RecommendedWatcher>,
    rescan_at: Option<Instant>,
}

pub enum Render {
    Auto,
    Braille,
    Pixels,
}

impl App {
    pub fn new(vault: Vault, render: Render) -> App {
        let opts = BuildOpts {
            unresolved: false,
            tags: false,
            orphans: true,
        };
        let graph = graph::build(&vault, opts, &HashMap::new());
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
        let (fs_events, watcher) = watch_vault(&vault.root);
        App {
            vault,
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
            group_mode,
            obsidian_groups,
            assign,
            kitty,
            pixels,
            dirty: true,
            pending_image: Vec::new(),
            fs_events,
            _watcher: watcher,
            rescan_at: None,
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

            let idle = self.paused || self.graph.settled();
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
    fn check_disk(&mut self) {
        if let Some(rx) = &self.fs_events
            && rx.try_iter().any(|ev| {
                ev.paths
                    .iter()
                    .any(|p| is_vault_change(&self.vault.root, p))
            })
        {
            self.rescan_at = Some(Instant::now() + Duration::from_millis(300));
        }
        if self.rescan_at.is_some_and(|t| Instant::now() >= t) && self.drag.is_none() {
            self.rescan_at = None;
            let before = self.vault.notes.len();
            self.rebuild(true);
            let after = self.vault.notes.len();
            self.flash(match after.cmp(&before) {
                std::cmp::Ordering::Greater => format!("vault changed: +{} notes", after - before),
                std::cmp::Ordering::Less => format!("vault changed: −{} notes", before - after),
                _ => "vault changed: graph updated".to_string(),
            });
            self.dirty = true;
        }
    }

    // ---------- graph state ----------

    fn rebuild(&mut self, rescan: bool) {
        if rescan {
            match vault::scan(&self.vault.root) {
                Ok(v) => self.vault = v,
                Err(e) => return self.flash(format!("rescan failed: {e}")),
            }
            self.obsidian_groups = groups::load_obsidian(&self.vault.root);
        }
        let prev = self.graph.positions();
        let prev_aspect = self.graph.aspect;
        let sel_key = self.sel.map(|i| self.graph.nodes[i].key.clone());
        self.graph = graph::build(&self.vault, self.opts, &prev);
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
        self.zoom += (z - self.zoom) * ease;
        self.cam.0 += (c.0 - self.cam.0) * ease;
        self.cam.1 += (c.1 - self.cam.1) * ease;
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

        let shift = k.modifiers.contains(KeyModifiers::SHIFT);
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        match k.code {
            KeyCode::Char('q') => self.quit = true,
            KeyCode::Char('c') if ctrl => self.quit = true,
            KeyCode::Esc => {
                if self.local {
                    self.local = false;
                    self.auto_fit = true;
                } else {
                    self.sel = None;
                    self.hits.clear();
                }
            }
            KeyCode::Char('?') => self.mode = Mode::Help,
            KeyCode::Char('/') => {
                self.mode = Mode::Search(String::new());
                self.hits.clear();
            }

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

            KeyCode::Char('+') | KeyCode::Char('=') => self.zoom_by(1.25, None),
            KeyCode::Char('-') | KeyCode::Char('_') => self.zoom_by(0.8, None),
            KeyCode::Char('0') => self.auto_fit = true,
            KeyCode::Char('c') => self.center_on_sel(),

            KeyCode::Tab | KeyCode::BackTab => self.cycle_links(k.code == KeyCode::BackTab),
            KeyCode::Backspace | KeyCode::Char('b') => {
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
            KeyCode::Char('n') | KeyCode::Char('N') if !self.hits.is_empty() => {
                let len = self.hits.len();
                self.hit_pos = if k.code == KeyCode::Char('n') {
                    (self.hit_pos + 1) % len
                } else {
                    (self.hit_pos + len - 1) % len
                };
                self.select(self.hits[self.hit_pos]);
                self.center_on_sel();
                self.flash(format!("match {}/{}", self.hit_pos + 1, len));
            }

            KeyCode::Enter | KeyCode::Char('e') => self.open_in_editor(terminal)?,
            KeyCode::Char('o') => self.open_in_obsidian(),
            KeyCode::Char('w') => {
                self.hide_image();
                board::launch(terminal, &self.vault.root, None, self.kitty.is_some())?;
                self.dirty = true;
            }

            KeyCode::Char('f') => {
                if self.sel.is_none() {
                    self.select_default();
                }
                self.local = !self.local;
                self.auto_fit = true;
            }
            KeyCode::Char(']') => {
                self.depth = (self.depth + 1).min(5);
                self.auto_fit = true;
                self.flash(format!("local graph depth {}", self.depth));
            }
            KeyCode::Char('[') => {
                self.depth = (self.depth - 1).max(1);
                self.auto_fit = true;
                self.flash(format!("local graph depth {}", self.depth));
            }
            KeyCode::Char('g') => self.cycle_groups(),
            KeyCode::Char('p') => self.toggle_pixels(),
            KeyCode::Char('t') => {
                self.opts.tags = !self.opts.tags;
                self.rebuild(false);
                self.flash(if self.opts.tags {
                    "tags shown"
                } else {
                    "tags hidden"
                });
            }
            KeyCode::Char('u') => {
                self.opts.unresolved = !self.opts.unresolved;
                self.rebuild(false);
                self.flash(if self.opts.unresolved {
                    "unresolved links shown"
                } else {
                    "unresolved links hidden"
                });
            }
            KeyCode::Char('O') => {
                self.opts.orphans = !self.opts.orphans;
                self.rebuild(false);
                self.flash(if self.opts.orphans {
                    "orphans shown"
                } else {
                    "orphans hidden"
                });
            }
            KeyCode::Char('a') => {
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
            KeyCode::Char(' ') => {
                self.paused = !self.paused;
                if !self.paused {
                    self.graph.reheat(0.5);
                }
            }
            KeyCode::Char('r') => {
                self.rebuild(true);
                self.flash(format!("rescanned: {} notes", self.vault.notes.len()));
            }
            _ => {}
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
        let q = q.to_lowercase();
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
                let l = n.label.to_lowercase();
                let in_path = n
                    .rel
                    .as_ref()
                    .is_some_and(|r| r.to_lowercase().contains(&q));
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
                let p = self.vault.root.join(format!("{}.md", node.label));
                if !p.exists() {
                    std::fs::write(&p, "")?;
                }
                p
            }
            Kind::Tag => {
                self.flash("that's a tag — press f to see its notes");
                return Ok(());
            }
        };
        self.hide_image();
        match term::edit_file(terminal, &path, &self.vault.root)? {
            None => self.rebuild(true), // links may have changed
            Some(err) => self.flash(err),
        }
        Ok(())
    }

    fn open_in_obsidian(&mut self) {
        let Some(rel) = self.sel.and_then(|i| self.graph.nodes[i].rel.clone()) else {
            return self.flash("select a note first");
        };
        let ok = term::open_in_obsidian(&self.vault.name, &rel);
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
        // Draw selection-related labels last so they win overlaps.
        labels.sort_by_key(|l| l.style.fg != Some(DIM) && l.style.fg != Some(MUTED));
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
                    self.vault.name, shown, shown_links
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
            draw_help(f, main);
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
        let info = match self.sel {
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

fn draw_help(f: &mut Frame, area: Rect) {
    let rows: &[(&str, &str)] = &[
        ("↑↓←→ / hjkl", "select the nearest note in that direction"),
        ("Tab / Shift-Tab", "cycle through the selected note's links"),
        ("b / Backspace", "go back to the previous selection"),
        ("/  then n / N", "search notes, jump between matches"),
        ("Enter / e", "open in $EDITOR (creates unresolved notes)"),
        ("o", "open the note in the Obsidian app"),
        ("w", "whiteboards (Obsidian .canvas files)"),
        ("f", "local graph around the selection"),
        ("[  ]", "local graph depth"),
        ("g", "colour groups: obsidian → folder → tag → off"),
        ("p", "pixel rendering (Kitty graphics) ↔ braille"),
        ("Shift+arrows / HJKL", "pan"),
        ("+ / -  or scroll", "zoom (scroll zooms at the cursor)"),
        ("0", "fit graph to screen"),
        ("c", "centre on selection"),
        ("drag", "move a node, or pan on empty space"),
        ("t / u / O", "toggle tags / unresolved links / orphans"),
        ("a", "labels: auto → all → selection only"),
        ("space", "pause / resume the layout"),
        ("r", "rescan the vault"),
        ("Esc", "leave local graph, clear selection"),
        ("q", "quit"),
    ];
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

fn watch_vault(root: &Path) -> (Option<Receiver<notify::Event>>, Option<RecommendedWatcher>) {
    let (tx, rx) = mpsc::channel();
    let watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
        if let Ok(ev) = res
            && !matches!(ev.kind, notify::EventKind::Access(_))
        {
            let _ = tx.send(ev);
        }
    });
    match watcher {
        Ok(mut w) => match w.watch(root, RecursiveMode::Recursive) {
            Ok(()) => (Some(rx), Some(w)),
            Err(_) => (None, None), // live updates unavailable; `r` still rescans
        },
        Err(_) => (None, None),
    }
}

/// Does a changed path affect the graph? Notes do; Obsidian's own state files don't,
/// except the graph colour-group settings.
pub fn is_vault_change(root: &Path, path: &Path) -> bool {
    let Ok(rel) = path.strip_prefix(root) else {
        return false;
    };
    let rel = rel.to_string_lossy().replace('\\', "/");
    if rel == ".obsidian/graph.json" {
        return true;
    }
    let hidden = rel.split('/').any(|part| part.starts_with('.'));
    !hidden && rel.ends_with(".md")
}

#[cfg(test)]
mod tests {
    use super::is_vault_change;
    use std::path::Path;

    #[test]
    fn only_notes_and_graph_settings_trigger_rescans() {
        let root = Path::new("/vault");
        assert!(is_vault_change(root, Path::new("/vault/Note.md")));
        assert!(is_vault_change(root, Path::new("/vault/a/b/Deep.md")));
        assert!(is_vault_change(
            root,
            Path::new("/vault/.obsidian/graph.json")
        ));
        assert!(!is_vault_change(
            root,
            Path::new("/vault/.obsidian/workspace.json")
        ));
        assert!(!is_vault_change(root, Path::new("/vault/.trash/Old.md")));
        assert!(!is_vault_change(root, Path::new("/vault/Board.canvas")));
        assert!(!is_vault_change(root, Path::new("/elsewhere/Note.md")));
    }
}
