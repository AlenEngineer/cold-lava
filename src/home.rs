//! The main menu: vaults on the left, the selected vault's notes in the middle, and the
//! selected note on the right, shown in a real (embedded) Neovim, or as a rendered
//! preview when Neovim isn't installed.

use crate::app::{self, ACCENT, ACCENT_SOFT, DIM, HIT, MUTED, NOTE, Render};
use crate::commands::{self, Keymap, MenuAction};
use crate::core::{Core, Event as CoreEvent};
use crate::embed::Embedded;
use crate::{board, term, vault};
use anyhow::Result;
use ratatui::crossterm::event::{
    self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent,
    MouseEventKind,
};
use ratatui::layout::{Constraint, Layout, Position, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Clear, Paragraph, Wrap};
use ratatui::{DefaultTerminal, Frame};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use walkdir::WalkDir;

// ---------- vault discovery ----------

#[derive(Clone)]
pub struct VaultEntry {
    pub name: String,
    pub path: PathBuf,
    /// Where we found it: "obsidian", "recent", "here".
    pub source: &'static str,
    /// Last opened (ms since epoch), for sorting.
    pub ts: u64,
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
}

pub fn config_dir() -> Option<PathBuf> {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| home_dir().map(|h| h.join(".config")))
}

/// Where Obsidian keeps its list of vaults on each platform (including Flatpak and Snap installs).
fn obsidian_configs() -> Vec<PathBuf> {
    let mut v = Vec::new();
    if let Some(h) = home_dir() {
        v.push(h.join("Library/Application Support/obsidian/obsidian.json"));
        v.push(h.join(".var/app/md.obsidian.Obsidian/config/obsidian/obsidian.json"));
        v.push(h.join("snap/obsidian/current/.config/obsidian/obsidian.json"));
    }
    if let Some(c) = config_dir() {
        v.push(c.join("obsidian/obsidian.json"));
    }
    if let Some(a) = std::env::var_os("APPDATA") {
        v.push(PathBuf::from(a).join("obsidian/obsidian.json"));
    }
    v
}

/// Parses Obsidian's `obsidian.json`: `{"vaults": {"<id>": {"path": "...", "ts": 123}}}`.
pub fn parse_obsidian_vaults(json: &str) -> Vec<(PathBuf, u64)> {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(json) else {
        return Vec::new();
    };
    let Some(map) = v.get("vaults").and_then(|x| x.as_object()) else {
        return Vec::new();
    };
    map.values()
        .filter_map(|e| {
            Some((
                PathBuf::from(e.get("path")?.as_str()?),
                e.get("ts").and_then(|t| t.as_u64()).unwrap_or(0),
            ))
        })
        .collect()
}

fn recent_file() -> Option<PathBuf> {
    config_dir().map(|c| c.join("cold-lava/vaults.json"))
}

fn load_recent() -> Vec<(PathBuf, u64)> {
    let Some(text) = recent_file().and_then(|p| std::fs::read_to_string(p).ok()) else {
        return Vec::new();
    };
    let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) else {
        return Vec::new();
    };
    v.get("vaults")
        .and_then(|x| x.as_array())
        .into_iter()
        .flatten()
        .filter_map(|e| {
            Some((
                PathBuf::from(e.get("path")?.as_str()?),
                e.get("opened").and_then(|t| t.as_u64()).unwrap_or(0),
            ))
        })
        .collect()
}

fn remember(path: &Path) {
    let Some(file) = recent_file() else { return };
    let mut list = load_recent();
    list.retain(|(p, _)| p != path);
    list.insert(0, (path.to_path_buf(), now_ms()));
    list.truncate(50);
    let json = serde_json::json!({
        "vaults": list.iter().map(|(p, t)| serde_json::json!({"path": p, "opened": t})).collect::<Vec<_>>()
    });
    if let Some(dir) = file.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(
        file,
        serde_json::to_string_pretty(&json).unwrap_or_default(),
    );
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn looks_like_vault(p: &Path) -> bool {
    p.join(".obsidian").is_dir()
        || std::fs::read_dir(p)
            .map(|rd| {
                rd.filter_map(|e| e.ok())
                    .any(|e| e.path().extension().is_some_and(|x| x == "md"))
            })
            .unwrap_or(false)
}

/// Every vault we know about, most recently used first, without duplicates.
pub fn discover(extra: &[PathBuf]) -> Vec<VaultEntry> {
    let mut found: Vec<(PathBuf, u64, &'static str)> = Vec::new();
    for p in extra {
        found.push((p.clone(), u64::MAX, "here"));
    }
    for (p, t) in load_recent() {
        found.push((p, t, "recent"));
    }
    for cfg in obsidian_configs() {
        if let Ok(text) = std::fs::read_to_string(&cfg) {
            for (p, t) in parse_obsidian_vaults(&text) {
                found.push((p, t, "obsidian"));
            }
        }
    }
    // The bundled example vault, when run from the repository.
    for base in [
        std::env::current_dir().ok(),
        Some(PathBuf::from(env!("CARGO_MANIFEST_DIR"))),
    ]
    .into_iter()
    .flatten()
    {
        let ex = base.join("example-vault");
        if ex.is_dir() {
            found.push((ex, 1, "example"));
        }
    }
    let mut seen = HashSet::new();
    let mut out: Vec<VaultEntry> = Vec::new();
    for (p, ts, source) in found {
        let Ok(canon) = p.canonicalize() else {
            continue;
        };
        if !canon.is_dir() {
            continue;
        }
        // The bundled example vault only if no vault of that name is listed already.
        if source == "example" && out.iter().any(|e| e.name == "example-vault") {
            continue;
        }
        if let Some(e) = out.iter_mut().find(|e| e.path == canon) {
            e.ts = e.ts.max(ts);
            if e.source != "here" && source == "obsidian" {
                e.source = "obsidian";
            }
            continue;
        }
        if seen.insert(canon.clone()) {
            let name = canon
                .file_name()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default();
            out.push(VaultEntry {
                name,
                path: canon,
                source,
                ts,
            });
        }
    }
    out.sort_by(|a, b| {
        b.ts.cmp(&a.ts)
            .then(a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    out
}

// ---------- the note tree ----------

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ItemKind {
    Folder,
    Note,
    Board,
}

#[derive(Clone, Debug)]
pub struct Item {
    pub rel: String,
    pub name: String,
    pub depth: usize,
    pub kind: ItemKind,
}

/// All folders, notes and boards in a vault, as a depth-first sorted list
/// (folders before files, case-insensitive).
pub fn scan_tree(root: &Path) -> Vec<Item> {
    let mut entries: Vec<(String, ItemKind)> = WalkDir::new(root)
        .min_depth(1)
        .into_iter()
        .filter_entry(|e| !e.file_name().to_string_lossy().starts_with('.'))
        .filter_map(|e| e.ok())
        .filter_map(|e| {
            let rel = e
                .path()
                .strip_prefix(root)
                .ok()?
                .to_string_lossy()
                .replace('\\', "/");
            let kind = if e.file_type().is_dir() {
                ItemKind::Folder
            } else {
                match e.path().extension()?.to_str()? {
                    "md" => ItemKind::Note,
                    "canvas" => ItemKind::Board,
                    _ => return None,
                }
            };
            Some((rel, kind))
        })
        .collect();
    // Sort by path components, folders first at each level.
    entries.sort_by(|(a, ak), (b, bk)| {
        let (pa, pb): (Vec<&str>, Vec<&str>) = (a.split('/').collect(), b.split('/').collect());
        for i in 0..pa.len().min(pb.len()) {
            if pa[i] != pb[i] {
                // A component is a folder if it isn't the last one, or the item itself is a folder.
                let fa = i + 1 < pa.len() || *ak == ItemKind::Folder;
                let fb = i + 1 < pb.len() || *bk == ItemKind::Folder;
                return fb
                    .cmp(&fa)
                    .then(pa[i].to_lowercase().cmp(&pb[i].to_lowercase()));
            }
        }
        pa.len().cmp(&pb.len())
    });
    entries
        .into_iter()
        .map(|(rel, kind)| {
            let file = rel.rsplit('/').next().unwrap_or(&rel);
            let name = match kind {
                ItemKind::Note => file.trim_end_matches(".md"),
                ItemKind::Board => file.trim_end_matches(".canvas"),
                ItemKind::Folder => file,
            }
            .to_string();
            Item {
                depth: rel.matches('/').count(),
                rel,
                name,
                kind,
            }
        })
        .collect()
}

/// The rows to show: folders collapsed unless expanded; with a filter, a flat list of matches.
pub fn visible(items: &[Item], expanded: &HashSet<String>, filter: &str) -> Vec<usize> {
    if !filter.is_empty() {
        let q = vault::search_key(filter);
        let mut hits: Vec<(usize, bool)> = items
            .iter()
            .enumerate()
            .filter(|(_, it)| {
                it.kind != ItemKind::Folder && vault::search_key(&it.rel).contains(&q)
            })
            .map(|(i, it)| (i, vault::search_key(&it.name).starts_with(&q)))
            .collect();
        hits.sort_by_key(|&(i, prefix)| (!prefix, items[i].name.to_lowercase()));
        return hits.into_iter().map(|(i, _)| i).collect();
    }
    items
        .iter()
        .enumerate()
        .filter(|(_, it)| {
            // Visible if every ancestor folder is expanded.
            let parts: Vec<&str> = it.rel.split('/').collect();
            (1..parts.len()).all(|n| expanded.contains(&parts[..n].join("/")))
        })
        .map(|(i, _)| i)
        .collect()
}

// ---------- the menu ----------

#[derive(Clone, Copy, PartialEq, Eq)]
enum Focus {
    Vaults,
    Notes,
    Editor,
    Backlinks,
}

/// Where to put the cursor after opening a note.
#[derive(Clone, Debug, PartialEq)]
pub enum Jump {
    Line(usize),
    Heading(String),
    Block(String),
}

/// A backlink as shown in the pane under the editor.
struct BackRow {
    name: String,
    path: PathBuf,
    line: usize,
    context: String,
}

enum Mode {
    Normal,
    Filter,
    NewNote(String),
    AddVault(String),
    Help,
    /// Quick switcher: jump to a note by (fuzzy) name.
    Switcher {
        query: String,
        sel: usize,
    },
    /// Command palette: run a command by (fuzzy) name.
    Palette {
        query: String,
        sel: usize,
    },
    /// Full-text search.
    Search {
        query: String,
        sel: usize,
    },
    /// Typing a note's new name (or folder, when moving).
    Rename {
        note: PathBuf,
        input: String,
        moving: bool,
    },
    /// Showing what a rename will change before doing it.
    ConfirmRename {
        plan: Box<crate::rename::Plan>,
        scroll: usize,
    },
}

/// One row of the search results: a note header, or one of its matching lines.
#[derive(Clone, Copy)]
enum SearchRow {
    Note(usize),
    Line(usize, usize),
}

pub struct Home {
    render: Render,
    vaults: Vec<VaultEntry>,
    vsel: usize,
    // The open vault
    root: Option<PathBuf>,
    items: Vec<Item>,
    expanded: HashSet<String>,
    rows: Vec<usize>,
    nsel: usize,
    nscroll: usize,
    filter: String,
    links: HashMap<String, (usize, usize)>,
    /// The open vault: index, file watching and change events.
    core: Option<Core>,
    backs: Vec<BackRow>,
    bsel: usize,
    show_backlinks: bool,
    keys: Keymap<MenuAction>,
    /// Recently shown notes, most recent first (the quick switcher's starting list).
    mru: Vec<PathBuf>,
    results: Vec<crate::search::NoteHits>,
    result_rows: Vec<SearchRow>,
    /// The overlay was opened from inside Neovim: give it focus back afterwards.
    back_to_editor: bool,
    // Right pane
    editor_cmd: Option<String>,
    editor: Option<Embedded>,
    shown: Option<PathBuf>,
    pending: Option<(PathBuf, Instant)>,
    preview: Vec<String>,
    zoom: bool,
    // UI
    focus: Focus,
    mode: Mode,
    msg: Option<(String, Instant)>,
    areas: [Rect; 4],
    quit: bool,
}

/// Neovim (or Vim) to embed: $VISUAL/$EDITOR if it is one, else `nvim` if it's installed.
fn pick_editor() -> Option<String> {
    let is_vim = |p: &str| {
        let base = Path::new(p)
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        matches!(
            base.as_str(),
            "nvim" | "vim" | "vi" | "nvim.exe" | "vim.exe"
        )
    };
    for var in ["COLD_LAVA_EDITOR", "VISUAL", "EDITOR"] {
        if let Ok(v) = std::env::var(var) {
            let prog = v.split_whitespace().next().unwrap_or("").to_string();
            if is_vim(&prog) {
                return Some(prog);
            }
        }
    }
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .find(|d| d.join("nvim").is_file())
        .map(|_| "nvim".to_string())
}

/// The command we type into Vim/Neovim to show another note: save edits, open it, and
/// optionally move to a line, heading or block.
pub fn open_command_at(path: &Path, jump: Option<&Jump>) -> String {
    let q = |s: &str| s.replace('\'', "''");
    let p = q(&path.to_string_lossy());
    let goto = match jump {
        None => String::new(),
        Some(Jump::Line(n)) => format!(" | call cursor({n}, 1) | normal! zz"),
        // Very-nomagic search, so heading text is matched literally.
        Some(Jump::Heading(h)) => format!(
            " | call cursor(1, 1) | call search('\\v^#+\\s*\\V' . '{}', 'cW') | normal! zz",
            q(&h.replace('\\', "\\\\"))
        ),
        Some(Jump::Block(b)) => format!(
            " | call cursor(1, 1) | call search('\\V ^' . '{}', 'cW') | normal! zz",
            q(&b.replace('\\', "\\\\"))
        ),
    };
    // <C-\><C-n> gets to normal mode from any mode first.
    format!("\x1c\x0e:silent! wall | execute 'edit' fnameescape('{p}'){goto}\r")
}

/// Loaded into the embedded Vim/Neovim: Enter (or gf) on a link asks cold-lava to follow it,
/// and every note Vim opens is reported, so the menu and backlinks stay in sync.
/// Messages go out as a private OSC sequence that cold-lava removes from the screen output.
const VIM_SCRIPT: &str = r#"" cold-lava integration (loaded automatically; see https://github.com/AlenEngineer/cold-lava)
if exists('g:cold_lava_loaded') | finish | endif
let g:cold_lava_loaded = 1

function! s:Enc(s) abort
  return substitute(a:s, '[^A-Za-z0-9 _./-]', '\=printf("%%u%x;", char2nr(submatch(0)))', 'g')
endfunction

function! s:Send(kind, payload) abort
  let l:msg = "\x1b]5379;" . a:kind . ";" . s:Enc(a:payload) . "\x07"
  if has('nvim')
    call chansend(v:stderr, l:msg)
  elseif exists('*echoraw')
    call echoraw(l:msg)
  endif
endfunction

" The [[wikilink]] or [text](link.md) under the cursor, or ''.
function! s:LinkUnderCursor() abort
  let l:line = getline('.')
  let l:col = col('.') - 1
  for l:pat in ['\[\[\([^\]]\+\)\]\]', '\[[^\]]*\](\([^)]\+\))']
    let l:start = 0
    while 1
      let l:m = matchstrpos(l:line, l:pat, l:start)
      if l:m[1] < 0 | break | endif
      if l:col >= l:m[1] && l:col < l:m[2]
        return matchlist(l:m[0], l:pat)[1]
      endif
      let l:start = l:m[2]
    endwhile
  endfor
  return ''
endfunction

function! ColdLavaFollow() abort
  let l:target = s:LinkUnderCursor()
  if l:target ==# '' || l:target =~# '^\a\+://'
    return 0
  endif
  silent! update
  call s:Send('follow', expand('%:p') . "\t" . l:target)
  return 1
endfunction

function! s:Enter() abort
  if !ColdLavaFollow()
    execute "normal! \<CR>"
  endif
endfunction

function! s:Gf() abort
  if !ColdLavaFollow()
    normal! gf
  endif
endfunction

augroup ColdLava
  autocmd!
  autocmd BufEnter *.md call s:Send('open', expand('%:p'))
  autocmd BufEnter *.md nnoremap <buffer><silent> <CR> :<C-u>call <SID>Enter()<CR>
  autocmd BufWritePost *.md call s:Send('saved', expand('%:p'))
augroup END
nnoremap <silent> gf :<C-u>call <SID>Gf()<CR>
" Open cold-lava's quick switcher or search from Vim; map them as you like, e.g.
"   nnoremap <C-p> <Cmd>ColdLavaSwitcher<CR>
command! -nargs=0 ColdLavaSwitcher call s:Send('switcher', '')
command! -nargs=* ColdLavaSearch call s:Send('search', <q-args>)
if expand('%:e') ==# 'md'
  silent! doautocmd <nomodeline> ColdLava BufEnter
endif
"#;

fn vim_script_path() -> Option<PathBuf> {
    let p = std::env::temp_dir().join(format!("cold-lava-{}.vim", std::process::id()));
    std::fs::write(&p, VIM_SCRIPT).ok()?;
    Some(p)
}

impl Home {
    pub fn new(render: Render, initial: Option<PathBuf>) -> Home {
        let extra: Vec<PathBuf> = initial.iter().cloned().collect();
        let mut extra = extra;
        if extra.is_empty()
            && let Ok(cwd) = std::env::current_dir()
            && cwd.join(".obsidian").is_dir()
        {
            extra.push(cwd);
        }
        let vaults = discover(&extra);
        let mut h = Home {
            render,
            vaults,
            vsel: 0,
            root: None,
            items: Vec::new(),
            expanded: HashSet::new(),
            rows: Vec::new(),
            nsel: 0,
            nscroll: 0,
            filter: String::new(),
            links: HashMap::new(),
            core: None,
            backs: Vec::new(),
            bsel: 0,
            show_backlinks: true,
            keys: Keymap::new(commands::MENU),
            mru: Vec::new(),
            results: Vec::new(),
            result_rows: Vec::new(),
            back_to_editor: false,
            editor_cmd: pick_editor(),
            editor: None,
            shown: None,
            pending: None,
            preview: Vec::new(),
            zoom: false,
            focus: Focus::Vaults,
            mode: Mode::Normal,
            msg: None,
            areas: [Rect::default(); 4],
            quit: false,
        };
        if !h.vaults.is_empty() {
            h.open_vault(0);
            if initial.is_some() {
                h.focus = Focus::Notes;
            }
        }
        match crate::config::get().errors.len() {
            0 => {}
            1 => h.flash("config.toml has a problem: run cold-lava keys to see it"),
            n => h.flash(format!(
                "config.toml has {n} problems: run cold-lava keys to see them"
            )),
        }
        h
    }

    /// Select a note (expanding its folders) and open it in the editor pane, focused.
    pub fn focus_note(&mut self, path: &Path) {
        let (Some(root), Ok(canon)) = (self.root.clone(), path.canonicalize()) else {
            return;
        };
        let Ok(rel) = canon.strip_prefix(&root) else {
            return;
        };
        let rel = rel.to_string_lossy().replace('\\', "/");
        self.reload_tree();
        let parts: Vec<&str> = rel.split('/').collect();
        for n in 1..parts.len() {
            self.expanded.insert(parts[..n].join("/"));
        }
        self.filter.clear();
        self.refresh_rows();
        if let Some(r) = self.rows.iter().position(|&i| self.items[i].rel == rel) {
            self.nsel = r;
        }
        self.pending = None;
        self.show(&canon);
        self.focus = if self.editor.is_some() {
            Focus::Editor
        } else {
            Focus::Notes
        };
    }

    fn open_today(&mut self) {
        let Some(root) = self.root.clone() else {
            return;
        };
        match crate::daily::ensure(&root, crate::tasks::today()) {
            Ok(p) => self.focus_note(&p),
            Err(e) => self.flash(format!("couldn't create today's note: {e}")),
        }
    }

    pub fn run(&mut self, terminal: &mut DefaultTerminal) -> Result<()> {
        while !self.quit {
            let mut changed = false;
            let mut messages = Vec::new();
            if let Some(ed) = self.editor.as_mut() {
                changed |= ed.pump();
                messages = ed.take_messages();
                if ed.exited() {
                    self.editor = None;
                    self.shown = None;
                    if self.focus == Focus::Editor {
                        self.focus = Focus::Notes;
                    }
                    self.flash("Neovim closed. Select a note to open it again");
                }
            }
            for (kind, payload) in messages {
                self.on_editor_message(&kind, &payload);
            }
            changed |= self.load_pending();
            self.check_disk();
            let _ = changed;
            terminal.draw(|f| self.draw(f))?;
            let ms = if self.editor.is_some() || self.pending.is_some() {
                15
            } else {
                250
            };
            if event::poll(Duration::from_millis(ms))? {
                loop {
                    match event::read()? {
                        Event::Key(k) if k.kind == KeyEventKind::Press => {
                            self.on_key(k, terminal)?
                        }
                        Event::Mouse(m) => self.on_mouse(m),
                        _ => {}
                    }
                    if self.quit || !event::poll(Duration::ZERO)? {
                        break;
                    }
                }
            }
        }
        if let Some(ed) = self.editor.as_mut() {
            // Save before leaving.
            ed.send(b"\x1c\x0e:silent! wall\r");
            std::thread::sleep(Duration::from_millis(150));
        }
        Ok(())
    }

    // ---------- vaults & notes ----------

    fn open_vault(&mut self, i: usize) {
        let Some(v) = self.vaults.get(i).cloned() else {
            return;
        };
        self.vsel = i;
        self.root = Some(v.path.clone());
        self.expanded.clear();
        self.filter.clear();
        self.nsel = 0;
        self.nscroll = 0;
        self.core = None; // a different vault: start a fresh core
        self.reload_tree();
        // Start with top-level folders open if the vault is small enough to take in at once.
        if self.items.len() <= 60 {
            for it in &self.items {
                if it.kind == ItemKind::Folder && it.depth == 0 {
                    self.expanded.insert(it.rel.clone());
                }
            }
            self.refresh_rows();
        }
        if self.core.as_ref().is_some_and(|c| !c.is_live()) {
            self.flash("can't watch this folder for changes; press r to refresh");
        }
        remember(&v.path);
        self.select_row(0);
    }

    fn reload_tree(&mut self) {
        let Some(root) = self.root.clone() else {
            return;
        };
        self.items = scan_tree(&root);
        self.links.clear();
        match self.core.as_mut() {
            Some(core) => core.rescan_now(),
            None => self.core = Core::open(&root).ok(),
        }
        if let Some(ix) = self.core.as_ref().map(|c| &c.index) {
            for (i, n) in ix.vault.notes.iter().enumerate() {
                self.links.insert(format!("{}.md", n.rel), ix.counts(i));
            }
        }
        self.refresh_rows();
        self.refresh_backlinks();
    }

    /// Backlinks of the note in the editor pane.
    fn refresh_backlinks(&mut self) {
        self.backs.clear();
        let (Some(ix), Some(shown)) = (self.core.as_ref().map(|c| &c.index), &self.shown) else {
            return;
        };
        let Some(i) = ix.by_path(shown) else { return };
        self.backs = ix
            .backlinks(i)
            .iter()
            .map(|b| BackRow {
                name: ix.vault.notes[b.from].name.clone(),
                path: ix.path(b.from),
                line: b.line,
                context: b.context.clone(),
            })
            .collect();
        self.bsel = self.bsel.min(self.backs.len().saturating_sub(1));
    }

    /// Messages from the embedded Vim/Neovim (see VIM_SCRIPT).
    fn on_editor_message(&mut self, kind: &str, payload: &str) {
        match kind {
            "open" => {
                let path = PathBuf::from(payload);
                if self.shown.as_deref() != Some(path.as_path()) {
                    self.remember_recent(&path);
                    self.shown = Some(path.clone());
                    self.select_path(&path);
                    self.bsel = 0;
                    self.refresh_backlinks();
                }
            }
            "follow" => {
                if let Some((from, target)) = payload.split_once('\t') {
                    self.follow(Path::new(from), target);
                }
            }
            "saved" => {
                if let Some(core) = self.core.as_mut() {
                    core.note_changed(Path::new(payload));
                }
            }
            "switcher" => self.open_switcher(true),
            "search" => {
                self.open_search(true);
                if let Mode::Search { query, .. } = &mut self.mode {
                    *query = payload.to_string();
                }
                self.run_search();
            }
            _ => {}
        }
    }

    /// Follow a link written in `from`: open the target note (at its heading or block), or
    /// create it if it doesn't exist yet, like clicking an unresolved link in Obsidian.
    fn follow(&mut self, from: &Path, raw: &str) {
        let link = crate::index::split_link(raw);
        let Some(ix) = self.core.as_ref().map(|c| &c.index) else {
            return;
        };
        let from_i = ix.by_path(from);
        let jump = link
            .heading
            .clone()
            .map(Jump::Heading)
            .or(link.block.clone().map(Jump::Block));
        if link.note.is_empty() {
            // [[#Heading]]: a heading in the same note.
            if let Some(j) = jump {
                self.show_at(from, Some(j));
            }
            return;
        }
        match ix.resolver.resolve(&link.note) {
            Some(i) => {
                let path = ix.path(i);
                self.show_at(&path, jump);
                self.select_path(&path);
            }
            None => {
                let path = ix.new_note_path(&link.note, from_i);
                if let Err(e) = crate::write::create_if_missing(&path, b"") {
                    return self.flash(format!("couldn't create the note: {e}"));
                }
                let rel = path
                    .strip_prefix(self.root.as_deref().unwrap_or(Path::new("")))
                    .map(|r| r.to_string_lossy().into_owned())
                    .unwrap_or_default();
                self.flash(format!("created {rel}"));
                self.reload_tree();
                self.show_at(&path, None);
                self.select_path(&path);
            }
        }
    }

    /// Select a path in the tree (opening its folders), without opening it again.
    fn select_path(&mut self, path: &Path) {
        let Some(root) = self.root.clone() else {
            return;
        };
        let Ok(rel) = path.strip_prefix(&root) else {
            return;
        };
        let rel = rel.to_string_lossy().replace('\\', "/");
        let parts: Vec<&str> = rel.split('/').collect();
        for n in 1..parts.len() {
            self.expanded.insert(parts[..n].join("/"));
        }
        if !self.filter.is_empty() && !self.rows.iter().any(|&i| self.items[i].rel == rel) {
            self.filter.clear();
        }
        self.refresh_rows();
        if let Some(r) = self.rows.iter().position(|&i| self.items[i].rel == rel) {
            self.nsel = r;
        }
    }

    fn refresh_rows(&mut self) {
        let keep = self.current().map(|it| it.rel.clone());
        self.rows = visible(&self.items, &self.expanded, &self.filter);
        self.nsel = keep
            .and_then(|r| self.rows.iter().position(|&i| self.items[i].rel == r))
            .unwrap_or(self.nsel)
            .min(self.rows.len().saturating_sub(1));
    }

    fn current(&self) -> Option<&Item> {
        self.rows.get(self.nsel).map(|&i| &self.items[i])
    }

    fn current_path(&self) -> Option<PathBuf> {
        Some(self.root.as_ref()?.join(&self.current()?.rel))
    }

    fn select_row(&mut self, r: usize) {
        if self.rows.is_empty() {
            return;
        }
        self.nsel = r.min(self.rows.len() - 1);
        if let (Some(it), Some(p)) = (self.current().cloned(), self.current_path())
            && it.kind == ItemKind::Note
        {
            // Show it after a short pause, so holding j doesn't open every note on the way.
            self.pending = Some((p, Instant::now() + Duration::from_millis(120)));
        }
    }

    fn load_pending(&mut self) -> bool {
        match &self.pending {
            Some((_, t)) if Instant::now() >= *t => {}
            _ => return false,
        }
        let (path, _) = self.pending.take().unwrap();
        self.show(&path);
        true
    }

    /// Show a note in the right pane (embedded editor, or preview).
    fn show(&mut self, path: &Path) {
        self.show_at(path, None);
    }

    fn show_at(&mut self, path: &Path, jump: Option<Jump>) {
        if self.shown.as_deref() == Some(path) && jump.is_none() {
            return;
        }
        if self.shown.as_deref() != Some(path) {
            self.bsel = 0;
        }
        self.remember_recent(path);
        self.shown = Some(path.to_path_buf());
        self.refresh_backlinks();
        let Some(cmd) = self.editor_cmd.clone() else {
            let text = std::fs::read_to_string(path).unwrap_or_default();
            let mut lines: Vec<String> = text.lines().map(String::from).collect();
            // Hide YAML frontmatter in the preview.
            if lines.first().is_some_and(|l| l.trim_end() == "---")
                && let Some(end) = lines.iter().skip(1).position(|l| l.trim_end() == "---")
            {
                lines.drain(..end + 2);
            }
            self.preview = lines;
            return;
        };
        match self.editor.as_mut() {
            Some(ed) => ed.send(open_command_at(path, jump.as_ref()).as_bytes()),
            None => {
                let a = self.areas[2];
                let cwd = self.root.clone().unwrap_or_else(|| PathBuf::from("."));
                let mut args = vec!["-c".to_string(), "set autoread".to_string()];
                if let Some(script) = vim_script_path() {
                    args.push("-S".to_string());
                    args.push(script.to_string_lossy().into_owned());
                }
                args.push(path.to_string_lossy().into_owned());
                match Embedded::spawn(&cmd, &args, &cwd, a.height.max(5), a.width.max(20)) {
                    Ok(mut ed) => {
                        if let Some(j) = &jump {
                            ed.send(open_command_at(path, Some(j)).as_bytes());
                        }
                        self.editor = Some(ed);
                    }
                    Err(e) => {
                        self.editor_cmd = None;
                        self.flash(format!(
                            "couldn't start {cmd}: {e}; showing previews instead"
                        ));
                        self.shown = None;
                        self.show(path);
                    }
                }
            }
        }
    }

    /// Apply what changed on disk (from the core's events).
    fn check_disk(&mut self) {
        let (Some(core), Some(root)) = (self.core.as_mut(), self.root.clone()) else {
            return;
        };
        let events = core.poll();
        if events.is_empty() {
            return;
        }
        let tree_changed = events.iter().any(|e| {
            !matches!(
                e,
                CoreEvent::NoteChanged(_) | CoreEvent::GraphSettingsChanged
            )
        });
        if tree_changed {
            self.items = scan_tree(&root);
        }
        self.links.clear();
        if let Some(ix) = self.core.as_ref().map(|c| &c.index) {
            for (i, n) in ix.vault.notes.iter().enumerate() {
                self.links.insert(format!("{}.md", n.rel), ix.counts(i));
            }
        }
        self.refresh_rows();
        self.refresh_backlinks();
        if self.editor_cmd.is_none()
            && let Some(p) = self.shown.take()
        {
            self.show(&p);
        }
    }

    fn flash(&mut self, s: impl Into<String>) {
        self.msg = Some((s.into(), Instant::now()));
    }

    // ---------- input ----------

    fn on_key(&mut self, k: KeyEvent, terminal: &mut DefaultTerminal) -> Result<()> {
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        // Keys typed into the editor pane go to Neovim, except Ctrl-q (back to the menu).
        if self.focus == Focus::Editor {
            if ctrl && matches!(k.code, KeyCode::Char('q')) {
                self.focus = Focus::Notes;
            } else if let Some(ed) = self.editor.as_mut() {
                ed.send_key(k);
            } else {
                self.focus = Focus::Notes;
            }
            return Ok(());
        }
        if matches!(self.mode, Mode::Switcher { .. }) {
            self.switcher_key(k);
            return Ok(());
        }
        if matches!(self.mode, Mode::Palette { .. }) {
            return self.palette_key(k, terminal);
        }
        if matches!(self.mode, Mode::Search { .. }) {
            self.search_key(k);
            return Ok(());
        }
        if matches!(self.mode, Mode::Rename { .. } | Mode::ConfirmRename { .. }) {
            self.rename_key(k);
            return Ok(());
        }
        match &mut self.mode {
            Mode::Help => {
                self.mode = Mode::Normal;
                return Ok(());
            }
            Mode::Filter => {
                match k.code {
                    KeyCode::Esc => {
                        self.filter.clear();
                        self.mode = Mode::Normal;
                        self.refresh_rows();
                    }
                    KeyCode::Enter => {
                        self.mode = Mode::Normal;
                        self.focus = Focus::Notes;
                        self.select_row(self.nsel);
                    }
                    KeyCode::Down => self.select_row(self.nsel + 1),
                    KeyCode::Up => self.select_row(self.nsel.saturating_sub(1)),
                    KeyCode::Backspace => {
                        self.filter.pop();
                        self.nsel = 0;
                        self.refresh_rows();
                        self.select_row(0);
                    }
                    KeyCode::Char('u') if ctrl => {
                        self.filter.clear();
                        self.nsel = 0;
                        self.refresh_rows();
                    }
                    KeyCode::Char(_) if ctrl => {}
                    KeyCode::Char(c) => {
                        self.filter.push(c);
                        self.nsel = 0;
                        self.refresh_rows();
                        self.select_row(0);
                    }
                    _ => {}
                }
                return Ok(());
            }
            Mode::NewNote(name) | Mode::AddVault(name) => {
                match k.code {
                    KeyCode::Esc => self.mode = Mode::Normal,
                    KeyCode::Backspace => {
                        name.pop();
                    }
                    KeyCode::Char('u') if ctrl => name.clear(),
                    KeyCode::Char(_) if ctrl => {}
                    KeyCode::Char(c) => name.push(c),
                    KeyCode::Enter => {
                        let text = name.trim().to_string();
                        let new_note = matches!(self.mode, Mode::NewNote(_));
                        self.mode = Mode::Normal;
                        if !text.is_empty() {
                            if new_note {
                                self.create_note(&text)?;
                            } else {
                                self.add_vault(&text);
                            }
                        }
                    }
                    _ => {}
                }
                return Ok(());
            }
            Mode::Switcher { .. }
            | Mode::Palette { .. }
            | Mode::Search { .. }
            | Mode::Rename { .. }
            | Mode::ConfirmRename { .. } => return Ok(()),
            Mode::Normal => {}
        }

        // A command whose guard fails leaves the key to the focused pane (Esc, for one).
        if let Some(a) = self.keys.get(k).filter(|&a| self.enabled(a)) {
            return self.run_action(a, terminal);
        }
        match self.focus {
            Focus::Vaults => self.vault_key(k),
            Focus::Notes => self.notes_key(k, terminal)?,
            Focus::Backlinks => self.backlinks_key(k),
            Focus::Editor => {}
        }
        Ok(())
    }

    fn enabled(&self, a: MenuAction) -> bool {
        match a {
            MenuAction::ClearFilter => !self.filter.is_empty(),
            MenuAction::NewNote => self.root.is_some(),
            _ => true,
        }
    }

    fn run_action(&mut self, a: MenuAction, terminal: &mut DefaultTerminal) -> Result<()> {
        match a {
            // Ctrl-q leaves Neovim; pressed in the menu it goes back to Neovim, never quits.
            MenuAction::FocusEditor => {
                if self.editor.is_some() {
                    self.focus = Focus::Editor;
                }
            }
            MenuAction::Quit => self.quit = true,
            MenuAction::Help => self.mode = Mode::Help,
            MenuAction::FocusNext => {
                let has_backs = self.show_backlinks && !self.backs.is_empty();
                self.focus = match self.focus {
                    Focus::Vaults => Focus::Notes,
                    Focus::Notes if self.editor.is_some() => Focus::Editor,
                    Focus::Notes | Focus::Editor if has_backs => Focus::Backlinks,
                    _ => Focus::Vaults,
                }
            }
            MenuAction::Palette => {
                self.mode = Mode::Palette {
                    query: String::new(),
                    sel: 0,
                }
            }
            MenuAction::Switcher => self.open_switcher(false),
            MenuAction::Rename => self.start_rename(false),
            MenuAction::Move => self.start_rename(true),
            MenuAction::Search => self.open_search(false),
            MenuAction::ToggleBacklinks => {
                self.show_backlinks = !self.show_backlinks;
                if !self.show_backlinks && self.focus == Focus::Backlinks {
                    self.focus = Focus::Notes;
                }
            }
            MenuAction::FocusPrevious => {
                self.focus = if self.focus == Focus::Notes {
                    Focus::Vaults
                } else {
                    Focus::Notes
                }
            }
            MenuAction::Filter => {
                self.mode = Mode::Filter;
                self.focus = Focus::Notes;
            }
            MenuAction::ClearFilter => {
                self.filter.clear();
                self.refresh_rows();
            }
            MenuAction::Zoom => self.zoom = !self.zoom,
            MenuAction::Graph => self.open_graph(terminal)?,
            MenuAction::Boards => self.open_boards(terminal, None)?,
            MenuAction::NewNote => self.mode = Mode::NewNote(String::new()),
            MenuAction::Today => self.open_today(),
            MenuAction::AddVault => self.mode = Mode::AddVault(String::new()),
            MenuAction::Rescan => {
                if let Some(core) = self.core.as_mut() {
                    core.rescan(); // applied on the next poll, as a Rescanned event
                }
                self.flash("rescanning");
            }
            MenuAction::OpenInObsidian => self.open_in_obsidian(),
            MenuAction::EditFullscreen => self.edit_fullscreen(terminal)?,
        }
        Ok(())
    }

    fn vault_key(&mut self, k: KeyEvent) {
        let n = self.vaults.len();
        if n == 0 {
            return;
        }
        match k.code {
            KeyCode::Down | KeyCode::Char('j') => self.open_vault((self.vsel + 1).min(n - 1)),
            KeyCode::Up | KeyCode::Char('k') => self.open_vault(self.vsel.saturating_sub(1)),
            KeyCode::Enter | KeyCode::Right | KeyCode::Char('l') => self.focus = Focus::Notes,
            _ => {}
        }
    }

    fn notes_key(&mut self, k: KeyEvent, terminal: &mut DefaultTerminal) -> Result<()> {
        let page = self.areas[1].height.saturating_sub(2).max(1) as usize;
        match k.code {
            KeyCode::Down | KeyCode::Char('j') => self.select_row(self.nsel + 1),
            KeyCode::Up | KeyCode::Char('k') => self.select_row(self.nsel.saturating_sub(1)),
            KeyCode::PageDown => self.select_row(self.nsel + page),
            KeyCode::PageUp => self.select_row(self.nsel.saturating_sub(page)),
            KeyCode::Home => self.select_row(0),
            KeyCode::End => self.select_row(usize::MAX),
            KeyCode::Left | KeyCode::Char('h') => {
                let Some(it) = self.current().cloned() else {
                    return Ok(());
                };
                if it.kind == ItemKind::Folder
                    && self.expanded.contains(&it.rel)
                    && self.filter.is_empty()
                {
                    self.expanded.remove(&it.rel);
                    self.refresh_rows();
                } else if let Some((parent, _)) =
                    it.rel.rsplit_once('/').filter(|_| self.filter.is_empty())
                {
                    let parent = parent.to_string();
                    if let Some(r) = self.rows.iter().position(|&i| self.items[i].rel == parent) {
                        self.select_row(r);
                    }
                } else {
                    self.focus = Focus::Vaults;
                }
            }
            KeyCode::Right | KeyCode::Char('l') | KeyCode::Enter => {
                let Some(it) = self.current().cloned() else {
                    return Ok(());
                };
                match it.kind {
                    ItemKind::Folder => {
                        if !self.expanded.insert(it.rel.clone()) && k.code == KeyCode::Enter {
                            self.expanded.remove(&it.rel);
                        }
                        self.refresh_rows();
                    }
                    ItemKind::Board => {
                        let p = self.current_path();
                        self.open_boards(terminal, p)?;
                    }
                    ItemKind::Note => {
                        if let Some(p) = self.current_path() {
                            self.pending = None;
                            self.show(&p);
                        }
                        if self.editor.is_some() {
                            self.focus = Focus::Editor;
                        } else if k.code == KeyCode::Enter {
                            self.edit_fullscreen(terminal)?;
                        }
                    }
                }
            }
            _ => {}
        }
        Ok(())
    }

    // ---------- quick switcher & search ----------

    fn remember_recent(&mut self, path: &Path) {
        self.mru.retain(|p| p != path);
        self.mru.insert(0, path.to_path_buf());
        self.mru.truncate(50);
    }

    fn open_switcher(&mut self, from_editor: bool) {
        if self.core.is_none() {
            return;
        }
        self.back_to_editor = from_editor || self.focus == Focus::Editor;
        if self.focus == Focus::Editor {
            self.focus = Focus::Notes;
        }
        self.mode = Mode::Switcher {
            query: String::new(),
            sel: 0,
        };
    }

    fn open_search(&mut self, from_editor: bool) {
        if self.core.is_none() {
            return;
        }
        self.back_to_editor = from_editor || self.focus == Focus::Editor;
        if self.focus == Focus::Editor {
            self.focus = Focus::Notes;
        }
        self.mode = Mode::Search {
            query: String::new(),
            sel: 0,
        };
        self.results.clear();
        self.result_rows.clear();
    }

    fn close_overlay(&mut self) {
        self.mode = Mode::Normal;
        if self.back_to_editor && self.editor.is_some() {
            self.focus = Focus::Editor;
        }
        self.back_to_editor = false;
    }

    /// The quick switcher's list: (note index, hit). Empty query: recent notes, then the rest.
    fn switcher_hits(&self, query: &str) -> Vec<crate::search::SwitchHit> {
        let Some(ix) = self.core.as_ref().map(|c| &c.index) else {
            return Vec::new();
        };
        let mut hits = crate::search::switcher(ix, query);
        if query.trim().is_empty() {
            let rank = |h: &crate::search::SwitchHit| {
                let p = &ix.vault.notes[h.note].path;
                self.mru.iter().position(|m| m == p).unwrap_or(usize::MAX)
            };
            hits.sort_by(|a, b| {
                rank(a).cmp(&rank(b)).then_with(|| {
                    ix.vault.notes[a.note]
                        .rel
                        .to_lowercase()
                        .cmp(&ix.vault.notes[b.note].rel.to_lowercase())
                })
            });
        }
        hits
    }

    /// Whether the switcher offers "create note" (a query that isn't exactly an existing name).
    fn offers_create(&self, query: &str) -> bool {
        let q = vault::search_key(query.trim());
        !q.is_empty()
            && self.core.as_ref().map(|c| &c.index).is_some_and(|ix| {
                !ix.vault.notes.iter().any(|n| {
                    vault::search_key(&n.name) == q
                        || n.aliases.iter().any(|a| vault::search_key(a) == q)
                })
            })
    }

    fn switcher_key(&mut self, k: KeyEvent) {
        let Mode::Switcher { query, sel } = &mut self.mode else {
            return;
        };
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        let (mut q, mut s) = (query.clone(), *sel);
        let total = self.switcher_hits(&q).len() + self.offers_create(&q) as usize;
        match k.code {
            KeyCode::Esc => return self.close_overlay(),
            KeyCode::Down | KeyCode::Tab => s = (s + 1).min(total.saturating_sub(1)),
            KeyCode::Char('n') | KeyCode::Char('j') if ctrl => {
                s = (s + 1).min(total.saturating_sub(1))
            }
            KeyCode::Up | KeyCode::BackTab => s = s.saturating_sub(1),
            KeyCode::Char('p') | KeyCode::Char('k') if ctrl => s = s.saturating_sub(1),
            KeyCode::Backspace => {
                q.pop();
                s = 0;
            }
            KeyCode::Enter => {
                let hits = self.switcher_hits(&q);
                self.mode = Mode::Normal;
                if let Some(h) = hits.get(s) {
                    let path = self
                        .core
                        .as_ref()
                        .map(|c| &c.index)
                        .map(|ix| ix.path(h.note))
                        .unwrap();
                    self.open_found(&path, None);
                } else if self.offers_create(&q) {
                    // Same place a new note from a link goes (Obsidian's new-note location setting).
                    let path = self.core.as_ref().map(|c| &c.index).map(|ix| {
                        let from = self.shown.as_ref().and_then(|p| ix.by_path(p));
                        ix.new_note_path(q.trim(), from)
                    });
                    if let Some(path) = path
                        && crate::write::create_if_missing(&path, b"").is_ok()
                    {
                        self.flash(format!("created {}", q.trim()));
                        self.reload_tree();
                        self.open_found(&path, None);
                    }
                }
                self.back_to_editor = false;
                return;
            }
            KeyCode::Char(c) if !ctrl => {
                q.push(c);
                s = 0;
            }
            _ => {}
        }
        self.mode = Mode::Switcher { query: q, sel: s };
    }

    fn palette_hits(&self, query: &str) -> Vec<commands::Match> {
        commands::matching(query)
            .into_iter()
            .filter(|m| self.enabled(m.command.action))
            .collect()
    }

    fn palette_key(&mut self, k: KeyEvent, terminal: &mut DefaultTerminal) -> Result<()> {
        let Mode::Palette { query, sel } = &mut self.mode else {
            return Ok(());
        };
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        let (mut q, mut s) = (query.clone(), *sel);
        let total = self.palette_hits(&q).len();
        match k.code {
            KeyCode::Esc => {
                self.mode = Mode::Normal;
                return Ok(());
            }
            KeyCode::Down | KeyCode::Tab => s = (s + 1).min(total.saturating_sub(1)),
            KeyCode::Char('n') | KeyCode::Char('j') if ctrl => {
                s = (s + 1).min(total.saturating_sub(1))
            }
            KeyCode::Up | KeyCode::BackTab => s = s.saturating_sub(1),
            KeyCode::Char('p') | KeyCode::Char('k') if ctrl => s = s.saturating_sub(1),
            KeyCode::Backspace => {
                q.pop();
                s = 0;
            }
            KeyCode::Char('u') if ctrl => {
                q.clear();
                s = 0;
            }
            KeyCode::Enter => {
                let action = self.palette_hits(&q).get(s).map(|m| m.command.action);
                self.mode = Mode::Normal;
                return match action {
                    Some(a) => self.run_action(a, terminal),
                    None => Ok(()),
                };
            }
            KeyCode::Char(c) if !ctrl => {
                q.push(c);
                s = 0;
            }
            _ => {}
        }
        self.mode = Mode::Palette { query: q, sel: s };
        Ok(())
    }

    fn run_search(&mut self) {
        let Mode::Search { query, sel } = &mut self.mode else {
            return;
        };
        *sel = 0;
        let q = query.clone();
        let Some(ix) = self.core.as_ref().map(|c| &c.index) else {
            return;
        };
        self.results = crate::search::search(ix, &q, 8);
        self.result_rows.clear();
        for (i, r) in self.results.iter().enumerate() {
            self.result_rows.push(SearchRow::Note(i));
            for l in 0..r.lines.len() {
                self.result_rows.push(SearchRow::Line(i, l));
            }
        }
    }

    fn search_key(&mut self, k: KeyEvent) {
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        let n = self.result_rows.len();
        let page = 10;
        let Mode::Search { query, sel } = &mut self.mode else {
            return;
        };
        match k.code {
            KeyCode::Esc => self.close_overlay(),
            KeyCode::Down | KeyCode::Tab => *sel = (*sel + 1).min(n.saturating_sub(1)),
            KeyCode::Char('n') | KeyCode::Char('j') if ctrl => {
                *sel = (*sel + 1).min(n.saturating_sub(1))
            }
            KeyCode::Up | KeyCode::BackTab => *sel = sel.saturating_sub(1),
            KeyCode::Char('p') | KeyCode::Char('k') if ctrl => *sel = sel.saturating_sub(1),
            KeyCode::PageDown => *sel = (*sel + page).min(n.saturating_sub(1)),
            KeyCode::PageUp => *sel = sel.saturating_sub(page),
            KeyCode::Backspace => {
                query.pop();
                self.run_search();
            }
            KeyCode::Char(c) if !ctrl => {
                query.push(c);
                self.run_search();
            }
            KeyCode::Enter => {
                let s = *sel;
                let target = self.result_rows.get(s).and_then(|row| {
                    let (ri, li) = match *row {
                        SearchRow::Note(r) => (r, None),
                        SearchRow::Line(r, l) => (r, Some(l)),
                    };
                    let hit = self.results.get(ri)?;
                    let line = li
                        .and_then(|l| hit.lines.get(l))
                        .or(hit.lines.first())
                        .map(|l| l.line);
                    Some((self.core.as_ref().map(|c| &c.index)?.path(hit.note), line))
                });
                self.mode = Mode::Normal;
                if let Some((path, line)) = target {
                    self.open_found(&path, line.map(Jump::Line));
                }
                self.back_to_editor = false;
            }
            _ => {}
        }
    }

    // ---------- rename & move ----------

    /// The note to rename: the one selected in the tree, else the one in the editor.
    fn rename_target(&self) -> Option<PathBuf> {
        match (self.focus, self.current()) {
            (Focus::Notes, Some(it)) if it.kind == ItemKind::Note => self.current_path(),
            (Focus::Notes, Some(_)) => None,
            _ => self.shown.clone().or_else(|| {
                self.current()
                    .filter(|it| it.kind == ItemKind::Note)
                    .and(self.current_path())
            }),
        }
    }

    fn start_rename(&mut self, moving: bool) {
        let (Some(note), Some(root)) = (self.rename_target(), self.root.clone()) else {
            return self.flash("select a note to rename or move");
        };
        let rel = note
            .strip_prefix(&root)
            .map(|r| r.to_string_lossy().replace('\\', "/"))
            .unwrap_or_default();
        let rel = rel.trim_end_matches(".md");
        let (dir, name) = rel.rsplit_once('/').unwrap_or(("", rel));
        let input = if moving {
            if dir.is_empty() {
                String::new()
            } else {
                format!("{dir}/")
            }
        } else {
            name.to_string()
        };
        self.mode = Mode::Rename {
            note,
            input,
            moving,
        };
    }

    /// Save Neovim's buffers and give it a moment, so the plan sees what's on screen.
    fn save_editor(&mut self) {
        if let Some(ed) = self.editor.as_mut() {
            ed.send(b"\x1c\x0e:silent! wall\r");
            for _ in 0..12 {
                std::thread::sleep(Duration::from_millis(25));
                ed.pump();
            }
            let _ = ed.take_messages();
        }
    }

    fn rename_key(&mut self, k: KeyEvent) {
        match &mut self.mode {
            Mode::Rename {
                note,
                input,
                moving,
            } => match k.code {
                KeyCode::Esc => self.mode = Mode::Normal,
                KeyCode::Backspace => {
                    input.pop();
                }
                KeyCode::Char('u') if k.modifiers.contains(KeyModifiers::CONTROL) => input.clear(),
                KeyCode::Char(_) if k.modifiers.contains(KeyModifiers::CONTROL) => {}
                KeyCode::Char(c) => input.push(c),
                KeyCode::Enter => {
                    let (note, input, moving) = (note.clone(), input.clone(), *moving);
                    self.mode = Mode::Normal;
                    self.plan_rename(&note, &input, moving);
                }
                _ => {}
            },
            Mode::ConfirmRename { plan, scroll } => match k.code {
                KeyCode::Esc | KeyCode::Char('n') => {
                    self.mode = Mode::Normal;
                    self.flash("rename cancelled");
                }
                KeyCode::Down | KeyCode::Char('j') => *scroll += 1,
                KeyCode::Up | KeyCode::Char('k') => *scroll = scroll.saturating_sub(1),
                KeyCode::Enter | KeyCode::Char('y') => {
                    let plan = (**plan).clone();
                    self.mode = Mode::Normal;
                    self.apply_rename(plan);
                }
                _ => {}
            },
            _ => {}
        }
    }

    fn plan_rename(&mut self, note: &Path, input: &str, moving: bool) {
        // A folder typed without a trailing slash still means "move into it".
        let input =
            if moving && !input.ends_with('/') && !input.ends_with(".md") && !input.is_empty() {
                format!("{input}/")
            } else if moving && input.is_empty() {
                return self.flash("type a folder, e.g. Archive/ (or / for the vault root)");
            } else {
                input.to_string()
            };
        self.save_editor();
        self.reload_tree();
        let Some(ix) = self.core.as_ref().map(|c| &c.index) else {
            return;
        };
        let Some(i) = ix.by_path(note) else {
            return self.flash("that note is no longer there");
        };
        let from = format!("{}.md", ix.vault.notes[i].rel);
        // Moving to the root: "/" means "no folder".
        let target = if input == "/" {
            Ok(format!("{}.md", ix.vault.notes[i].name))
        } else {
            crate::rename::target_rel(&from, &input)
        };
        match target.and_then(|to| crate::rename::plan(ix, i, &to)) {
            Ok(plan) => {
                self.mode = Mode::ConfirmRename {
                    plan: Box::new(plan),
                    scroll: 0,
                }
            }
            Err(e) => self.flash(format!("can't rename: {e}")),
        }
    }

    fn apply_rename(&mut self, plan: crate::rename::Plan) {
        if let Err(e) = crate::rename::apply(&plan) {
            self.reload_tree();
            return self.flash(format!("rename failed: {e}"));
        }
        let (old, new) = (plan.from_path.clone(), plan.to_path.clone());
        // Neovim: show the note under its new name, drop the old buffer, reload the others.
        if let Some(ed) = self.editor.as_mut() {
            if self.shown.as_deref() == Some(old.as_path()) {
                ed.send(open_command_at(&new, None).as_bytes());
            }
            let o = old.to_string_lossy().replace('\'', "''");
            ed.send(
                format!(
                    "\x1c\x0e:silent! execute 'bwipeout!' fnameescape('{o}') | silent! checktime\r"
                )
                .as_bytes(),
            );
        }
        if self.shown.as_deref() == Some(old.as_path()) {
            self.shown = Some(new.clone());
        }
        self.mru.retain(|p| p != &old);
        self.reload_tree();
        self.select_path(&new);
        let links = plan.links();
        let moved_own = plan.edits.iter().any(|e| e.path == plan.from_path) as usize;
        let files = plan.files() - moved_own;
        let moved = dir_of_rel(&plan.from) != dir_of_rel(&plan.to);
        let verb = if moved { "moved" } else { "renamed" };
        self.flash(if links == 0 {
            format!("{verb} to {} (no links needed updating)", plan.to)
        } else {
            format!(
                "{verb} to {} · {} updated in {}",
                plan.to,
                plural(links, "link"),
                plural(files.max(1), "file")
            )
        });
    }

    fn draw_confirm_rename(&self, f: &mut Frame, area: Rect) {
        let Mode::ConfirmRename { plan, scroll } = &self.mode else {
            return;
        };
        let w = (area.width * 3 / 4).clamp(50.min(area.width), 120);
        let h = (area.height * 2 / 3).clamp(10.min(area.height), 34);
        let r = Rect::new(
            area.x + (area.width - w) / 2,
            area.y + area.height / 6,
            w,
            h,
        );
        f.render_widget(Clear, r);
        let block = Block::bordered()
            .border_type(BorderType::Rounded)
            .border_style(Style::new().fg(ACCENT))
            .title(Line::styled(
                " Rename ",
                Style::new().fg(ACCENT).add_modifier(Modifier::BOLD),
            ))
            .title_bottom(Line::styled(
                " ⏎ rename · Esc cancel · ↑↓ scroll ",
                Style::new().fg(MUTED),
            ));
        let inner = block.inner(r);
        f.render_widget(block, r);
        let mut lines = vec![
            Line::from(vec![
                Span::styled(format!(" {}", plan.from), Style::new().fg(MUTED)),
                Span::styled("  →  ", Style::new().fg(ACCENT)),
                Span::styled(
                    plan.to.clone(),
                    Style::new().fg(NOTE).add_modifier(Modifier::BOLD),
                ),
            ]),
            Line::styled(
                match plan.links() {
                    0 => " No links point to it, so only the file moves.".to_string(),
                    n => format!(
                        " {} in {} will be updated:",
                        plural(n, "link"),
                        plural(plan.files(), "file")
                    ),
                },
                Style::new().fg(NOTE),
            ),
        ];
        for w in &plan.warnings {
            lines.push(Line::styled(format!(" ! {w}"), Style::new().fg(HIT)));
        }
        lines.push(Line::raw(""));
        let mut detail = Vec::new();
        for e in &plan.edits {
            detail.push(Line::from(vec![
                Span::styled(
                    format!(" ▤ {}", e.file.trim_end_matches(".md")),
                    Style::new().fg(ACCENT_SOFT).add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    format!("  {} link{}", e.links, if e.links == 1 { "" } else { "s" }),
                    Style::new().fg(DIM),
                ),
            ]));
            for c in e.changes.iter().take(3) {
                let mut t: String = c
                    .after
                    .trim()
                    .chars()
                    .take(inner.width.saturating_sub(10) as usize)
                    .collect();
                if c.after.trim().chars().count() > t.chars().count() {
                    t.push('…');
                }
                detail.push(Line::from(vec![
                    Span::styled(format!("   {:>4}  ", c.line), Style::new().fg(DIM)),
                    Span::styled(t, Style::new().fg(NOTE)),
                ]));
            }
            if e.changes.len() > 3 {
                detail.push(Line::styled(
                    format!("         … {} more lines", e.changes.len() - 3),
                    Style::new().fg(DIM),
                ));
            }
        }
        let room = (inner.height as usize).saturating_sub(lines.len());
        let start = (*scroll).min(detail.len().saturating_sub(room));
        lines.extend(detail.into_iter().skip(start).take(room));
        f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
    }

    /// Open a note picked in the switcher or search, and give the editor focus.
    fn open_found(&mut self, path: &Path, jump: Option<Jump>) {
        self.pending = None;
        self.show_at(path, jump);
        self.select_path(path);
        self.focus = if self.editor.is_some() {
            Focus::Editor
        } else {
            Focus::Notes
        };
    }

    fn backlinks_key(&mut self, k: KeyEvent) {
        let n = self.backs.len();
        if n == 0 {
            self.focus = Focus::Notes;
            return;
        }
        match k.code {
            KeyCode::Down | KeyCode::Char('j') => self.bsel = (self.bsel + 1).min(n - 1),
            KeyCode::Up | KeyCode::Char('k') => self.bsel = self.bsel.saturating_sub(1),
            KeyCode::Enter | KeyCode::Right | KeyCode::Char('l') => self.open_backlink(self.bsel),
            KeyCode::Left | KeyCode::Char('h') | KeyCode::Esc => self.focus = Focus::Notes,
            _ => {}
        }
    }

    /// Open the note a backlink comes from, at the line with the link.
    fn open_backlink(&mut self, i: usize) {
        let Some(b) = self.backs.get(i) else { return };
        let (path, line) = (b.path.clone(), b.line);
        self.pending = None;
        self.show_at(&path, Some(Jump::Line(line)));
        self.select_path(&path);
        if self.editor.is_some() {
            self.focus = Focus::Editor;
        }
    }

    fn on_mouse(&mut self, m: MouseEvent) {
        let pos = Position::new(m.column, m.row);
        let [va, na, ea, ba] = self.areas;
        if ba.contains(pos) {
            if let MouseEventKind::Down(MouseButton::Left) = m.kind {
                let r = (m.row.saturating_sub(ba.y + 1)) as usize;
                if r < self.backs.len() && m.row > ba.y {
                    self.focus = Focus::Backlinks;
                    if r == self.bsel {
                        self.open_backlink(r);
                    } else {
                        self.bsel = r;
                    }
                }
            }
            return;
        }
        if ea.contains(pos) {
            if let Some(ed) = self.editor.as_mut() {
                if matches!(m.kind, MouseEventKind::Down(_)) {
                    self.focus = Focus::Editor;
                }
                ed.send_mouse(m, m.column - ea.x, m.row - ea.y);
            }
            return;
        }
        let inside = |a: Rect| {
            Rect::new(
                a.x + 1,
                a.y + 1,
                a.width.saturating_sub(2),
                a.height.saturating_sub(2),
            )
            .contains(pos)
        };
        match m.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                if !self.zoom && inside(va) {
                    self.focus = Focus::Vaults;
                    let i = (m.row - va.y - 1) as usize;
                    if i < self.vaults.len() && i != self.vsel {
                        self.open_vault(i);
                    }
                } else if !self.zoom && inside(na) {
                    self.focus = Focus::Notes;
                    let r = self.nscroll + (m.row - na.y - 1) as usize;
                    if r < self.rows.len() {
                        if r == self.nsel
                            && self.current().is_some_and(|it| it.kind == ItemKind::Folder)
                        {
                            let rel = self.current().unwrap().rel.clone();
                            if !self.expanded.remove(&rel) {
                                self.expanded.insert(rel);
                            }
                            self.refresh_rows();
                        } else {
                            self.select_row(r);
                        }
                    }
                }
            }
            MouseEventKind::ScrollDown if inside(na) => self.select_row(self.nsel + 3),
            MouseEventKind::ScrollUp if inside(na) => self.select_row(self.nsel.saturating_sub(3)),
            _ => {}
        }
    }

    // ---------- actions ----------

    fn open_graph(&mut self, terminal: &mut DefaultTerminal) -> Result<()> {
        // The graph borrows the menu's live vault and gives it back, so there's one watcher and
        // changes made while the graph is open show up here too.
        let Some(core) = self.core.take() else {
            return Ok(());
        };
        if core.index.vault.notes.is_empty() {
            self.core = Some(core);
            self.flash("this vault has no notes yet");
            return Ok(());
        }
        let mut graph = app::App::new(core, self.render);
        let result = graph.run(terminal);
        self.core = Some(graph.into_core());
        terminal.clear()?;
        self.check_disk();
        result
    }

    fn open_boards(&mut self, terminal: &mut DefaultTerminal, file: Option<PathBuf>) -> Result<()> {
        let Some(root) = self.root.clone() else {
            return Ok(());
        };
        board::launch(terminal, &root, file, false)?;
        terminal.clear()?;
        self.reload_tree();
        Ok(())
    }

    fn edit_fullscreen(&mut self, terminal: &mut DefaultTerminal) -> Result<()> {
        let (Some(p), Some(root)) = (self.current_path(), self.root.clone()) else {
            return Ok(());
        };
        if self.current().is_some_and(|it| it.kind == ItemKind::Note) {
            if let Some(err) = term::edit_file(terminal, &p, &root)? {
                self.flash(err);
            }
            self.shown = None;
            self.select_row(self.nsel);
        }
        Ok(())
    }

    fn open_in_obsidian(&mut self) {
        let (Some(it), Some(v)) = (self.current().cloned(), self.vaults.get(self.vsel)) else {
            return;
        };
        let ok = term::open_in_obsidian(&v.name, &it.rel);
        self.flash(if ok {
            "opened in Obsidian"
        } else {
            "couldn't launch Obsidian"
        });
    }

    fn create_note(&mut self, name: &str) -> Result<()> {
        let Some(root) = self.root.clone() else {
            return Ok(());
        };
        // In the selected folder, or next to the selected note.
        let folder = match self.current() {
            Some(it) if it.kind == ItemKind::Folder => it.rel.clone(),
            Some(it) => it
                .rel
                .rsplit_once('/')
                .map(|(d, _)| d.to_string())
                .unwrap_or_default(),
            None => String::new(),
        };
        let file = if name.ends_with(".md") {
            name.to_string()
        } else {
            format!("{name}.md")
        };
        let rel = if folder.is_empty() {
            file
        } else {
            format!("{folder}/{file}")
        };
        let path = root.join(&rel);
        if path.exists() {
            self.flash(format!("{rel} already exists"));
        } else {
            let title = name.trim_end_matches(".md");
            crate::write::create(&path, format!("# {title}\n\n").as_bytes())?;
            if !folder.is_empty() {
                self.expanded.insert(folder);
            }
            self.flash(format!("created {rel}"));
        }
        // Make sure the new note is visible, even if a filter was active.
        self.filter.clear();
        self.reload_tree();
        if let Some(r) = self.rows.iter().position(|&i| self.items[i].rel == rel) {
            self.nsel = r;
            self.pending = None;
            self.show(&path);
            if self.editor.is_some() {
                self.focus = Focus::Editor;
            }
        }
        Ok(())
    }

    fn add_vault(&mut self, text: &str) {
        let p = match text.strip_prefix("~/") {
            Some(rest) => home_dir()
                .map(|h| h.join(rest))
                .unwrap_or_else(|| PathBuf::from(text)),
            None => PathBuf::from(text),
        };
        match p.canonicalize() {
            Ok(c) if c.is_dir() => {
                if !looks_like_vault(&c) {
                    self.flash("added (no notes in that folder yet)");
                }
                remember(&c);
                self.vaults = discover(&[]);
                if let Some(i) = self.vaults.iter().position(|v| v.path == c) {
                    self.open_vault(i);
                    self.focus = Focus::Notes;
                }
            }
            _ => self.flash(format!("not a folder: {text}")),
        }
    }

    // ---------- drawing ----------

    fn pane(&self, title: Line<'static>, focus: Focus) -> Block<'static> {
        let active = self.focus == focus;
        Block::bordered()
            .border_type(if active {
                BorderType::Thick
            } else {
                BorderType::Rounded
            })
            .border_style(Style::new().fg(if active { ACCENT } else { DIM }))
            .title(title)
    }

    fn draw(&mut self, f: &mut Frame) {
        let [main, status] =
            Layout::vertical([Constraint::Min(5), Constraint::Length(2)]).areas(f.area());
        let (va, na, ea) = if self.zoom {
            (Rect::default(), Rect::default(), main)
        } else {
            let [a, b, c] = Layout::horizontal([
                Constraint::Length(26),
                Constraint::Length(38),
                Constraint::Min(20),
            ])
            .areas(main);
            (a, b, c)
        };
        // Backlinks under the editor.
        let (ea, ba) = if self.show_backlinks && self.shown.is_some() && ea.height > 12 {
            let want = if self.backs.is_empty() {
                3
            } else {
                self.backs.len() as u16 + 2
            };
            let h = want.min(ea.height / 3).max(3);
            let [e, b] = Layout::vertical([Constraint::Min(5), Constraint::Length(h)]).areas(ea);
            (e, b)
        } else {
            (ea, Rect::default())
        };
        let editor_inner = Rect::new(
            ea.x + 1,
            ea.y + 1,
            ea.width.saturating_sub(2),
            ea.height.saturating_sub(2),
        );
        self.areas = [va, na, editor_inner, ba];

        if !self.zoom {
            self.draw_vaults(f, va);
            self.draw_notes(f, na);
        }
        self.draw_editor(f, ea, editor_inner);
        if ba.height > 0 {
            self.draw_backlinks(f, ba);
        }
        self.draw_status(f, status);
        if matches!(self.mode, Mode::Switcher { .. }) {
            self.draw_switcher(f, main);
        }
        if matches!(self.mode, Mode::Palette { .. }) {
            self.draw_palette(f, main);
        }
        if matches!(self.mode, Mode::Search { .. }) {
            self.draw_search(f, main);
        }
        if matches!(self.mode, Mode::ConfirmRename { .. }) {
            self.draw_confirm_rename(f, main);
        }
        if let Mode::Rename { input, moving, .. } = &self.mode {
            let (title, label) = if *moving {
                (" move to folder · ⏎ preview · Esc cancel ", "folder: ")
            } else {
                (" rename · ⏎ preview · Esc cancel ", "new name: ")
            };
            prompt(f, main, title, label, input);
        }
        match &self.mode {
            Mode::Help => draw_help(f, main, &self.keys),
            Mode::NewNote(name) => prompt(
                f,
                main,
                " new note · ⏎ create · Esc cancel ",
                "name: ",
                name,
            ),
            Mode::AddVault(name) => prompt(
                f,
                main,
                " add a vault · ⏎ add · Esc cancel ",
                "folder: ",
                name,
            ),
            _ => {}
        }
    }

    fn draw_switcher(&self, f: &mut Frame, area: Rect) {
        let Mode::Switcher { query, sel } = &self.mode else {
            return;
        };
        let Some(ix) = self.core.as_ref().map(|c| &c.index) else {
            return;
        };
        let inner = picker_frame(f, area, " Go to note ", " ↑↓ choose · ⏎ open · Esc close ");
        let hits = self.switcher_hits(query);
        let create = self.offers_create(query);
        let mut lines = vec![
            Line::from(vec![
                Span::styled(" ▸ ", Style::new().fg(HIT)),
                Span::styled(format!("{query}▏"), Style::new().fg(NOTE)),
                Span::styled(
                    if query.is_empty() {
                        "  type a name (letters in order are enough)".to_string()
                    } else {
                        format!("  {} notes", hits.len())
                    },
                    Style::new().fg(MUTED),
                ),
            ]),
            Line::raw(""),
        ];
        let rows = inner.height.saturating_sub(2) as usize;
        let total = hits.len() + create as usize;
        let start = sel.saturating_sub(rows.saturating_sub(1));
        let width = inner.width as usize;
        for i in start..(start + rows).min(total) {
            let selected = i == *sel;
            let base = if selected {
                Style::new().fg(Color::Black).bg(ACCENT)
            } else {
                Style::new().fg(NOTE)
            };
            let hl = if selected {
                base.add_modifier(Modifier::BOLD | Modifier::UNDERLINED)
            } else {
                Style::new().fg(HIT).add_modifier(Modifier::BOLD)
            };
            if i == hits.len() {
                lines.push(Line::styled(
                    format!(" + Create note \"{}\"", query.trim()),
                    if selected { base } else { Style::new().fg(HIT) },
                ));
                continue;
            }
            let hit = &hits[i];
            let note = &ix.vault.notes[hit.note];
            let label = hit.alias.clone().unwrap_or_else(|| note.name.clone());
            let mut spans = vec![Span::styled(" ", base)];
            for (ci, ch) in label.chars().enumerate() {
                spans.push(Span::styled(
                    ch.to_string(),
                    if hit.positions.contains(&ci) {
                        hl
                    } else {
                        base
                    },
                ));
            }
            let mut extra = String::new();
            if hit.alias.is_some() {
                extra.push_str(&format!("  → {}", note.name));
            }
            let folder = note.rel.rsplit_once('/').map(|(d, _)| d).unwrap_or("");
            let (inc, _) = ix.counts(hit.note);
            let right = format!("{folder}  ←{inc} ");
            let used = 1 + label.chars().count() + extra.chars().count();
            let pad = width.saturating_sub(used + right.chars().count());
            let dim = if selected {
                base
            } else {
                Style::new().fg(MUTED)
            };
            spans.push(Span::styled(extra, dim));
            spans.push(Span::styled(" ".repeat(pad), base));
            spans.push(Span::styled(right, dim));
            lines.push(Line::from(spans));
        }
        if total == 0 {
            lines.push(Line::styled(" no notes match", Style::new().fg(MUTED)));
        }
        f.render_widget(Paragraph::new(lines), inner);
    }

    fn draw_palette(&self, f: &mut Frame, area: Rect) {
        let Mode::Palette { query, sel } = &self.mode else {
            return;
        };
        let inner = picker_frame(f, area, " Commands ", " ↑↓ choose · ⏎ run · Esc close ");
        let hits = self.palette_hits(query);
        let mut lines = vec![
            Line::from(vec![
                Span::styled(" ▸ ", Style::new().fg(HIT)),
                Span::styled(format!("{query}▏"), Style::new().fg(NOTE)),
                Span::styled(
                    if query.is_empty() {
                        "  type a command's name or id".to_string()
                    } else {
                        format!("  {}", plural(hits.len(), "command"))
                    },
                    Style::new().fg(MUTED),
                ),
            ]),
            Line::raw(""),
        ];
        let rows = inner.height.saturating_sub(2) as usize;
        let start = sel.saturating_sub(rows.saturating_sub(1));
        let width = inner.width as usize;
        for (i, m) in hits.iter().enumerate().skip(start).take(rows) {
            let selected = i == *sel;
            let base = if selected {
                Style::new().fg(Color::Black).bg(ACCENT)
            } else {
                Style::new().fg(NOTE)
            };
            let dim = if selected {
                base
            } else {
                Style::new().fg(MUTED)
            };
            let hl = if selected {
                base.add_modifier(Modifier::BOLD | Modifier::UNDERLINED)
            } else {
                Style::new().fg(HIT).add_modifier(Modifier::BOLD)
            };
            let c = m.command;
            let mut spans = vec![Span::styled(" ", base)];
            let mut push = |text: &str, hits: &[usize], style: Style| {
                for (ci, ch) in text.chars().enumerate() {
                    let st = if hits.contains(&ci) { hl } else { style };
                    spans.push(Span::styled(ch.to_string(), st));
                }
            };
            push(c.title, &m.title_hits, base);
            push("  ", &[], base);
            push(c.id, &m.id_hits, dim);
            let right = format!("{} ", self.keys.label(c.id));
            let used = 3 + c.title.chars().count() + c.id.chars().count();
            let pad = width.saturating_sub(used + right.chars().count());
            spans.push(Span::styled(" ".repeat(pad), base));
            let keys = if selected {
                base
            } else {
                Style::new().fg(ACCENT)
            };
            spans.push(Span::styled(right, keys));
            lines.push(Line::from(spans));
        }
        if hits.is_empty() {
            lines.push(Line::styled(" no commands match", Style::new().fg(MUTED)));
        }
        f.render_widget(Paragraph::new(lines), inner);
    }

    fn draw_search(&self, f: &mut Frame, area: Rect) {
        let Mode::Search { query, sel } = &self.mode else {
            return;
        };
        let Some(ix) = self.core.as_ref().map(|c| &c.index) else {
            return;
        };
        let w = (area.width * 4 / 5).clamp(50.min(area.width), 140);
        let h = area.height.saturating_sub(4).max(8.min(area.height));
        let r = Rect::new(
            area.x + (area.width - w) / 2,
            area.y + 2.min(area.height / 8),
            w,
            h,
        );
        f.render_widget(Clear, r);
        let block = Block::bordered()
            .border_type(BorderType::Rounded)
            .border_style(Style::new().fg(ACCENT))
            .title(Line::styled(
                " Search ",
                Style::new().fg(ACCENT).add_modifier(Modifier::BOLD),
            ))
            .title_bottom(Line::styled(
                " ↑↓ choose · ⏎ open at the match · Esc close ",
                Style::new().fg(MUTED),
            ));
        let inner = block.inner(r);
        f.render_widget(block, r);
        let matches: usize = self.results.iter().map(|r| r.count).sum();
        let summary = if query.trim().is_empty() {
            "  words must all appear in a note · \"quotes\" for a phrase".to_string()
        } else if self.results.is_empty() {
            "  nothing found".to_string()
        } else {
            format!(
                "  {} notes · {} matching lines",
                self.results.len(),
                matches
            )
        };
        let mut lines = vec![
            Line::from(vec![
                Span::styled(" ▸ ", Style::new().fg(HIT)),
                Span::styled(format!("{query}▏"), Style::new().fg(NOTE)),
                Span::styled(summary, Style::new().fg(MUTED)),
            ]),
            Line::raw(""),
        ];
        let rows = inner.height.saturating_sub(2) as usize;
        let start = sel.saturating_sub(rows.saturating_sub(1));
        let width = inner.width as usize;
        for (i, row) in self.result_rows.iter().enumerate().skip(start).take(rows) {
            let selected = i == *sel;
            match *row {
                SearchRow::Note(ri) => {
                    let res = &self.results[ri];
                    let note = &ix.vault.notes[res.note];
                    let st = if selected {
                        Style::new()
                            .fg(Color::Black)
                            .bg(ACCENT)
                            .add_modifier(Modifier::BOLD)
                    } else {
                        Style::new().fg(ACCENT_SOFT).add_modifier(Modifier::BOLD)
                    };
                    let folder = note
                        .rel
                        .rsplit_once('/')
                        .map(|(d, _)| format!("  {d}"))
                        .unwrap_or_default();
                    let more = if res.count > res.lines.len() {
                        format!("  +{} more", res.count - res.lines.len())
                    } else {
                        String::new()
                    };
                    lines.push(Line::from(vec![
                        Span::styled(format!(" ▤ {} ", note.name), st),
                        Span::styled(folder, Style::new().fg(MUTED)),
                        Span::styled(format!("  {} lines{more}", res.count), Style::new().fg(DIM)),
                    ]));
                }
                SearchRow::Line(ri, li) => {
                    let l = &self.results[ri].lines[li];
                    let num = format!("   {:>4}  ", l.line);
                    let base = if selected {
                        Style::new().fg(Color::Black).bg(ACCENT)
                    } else {
                        Style::new().fg(NOTE)
                    };
                    let hl = if selected {
                        base.add_modifier(Modifier::BOLD | Modifier::UNDERLINED)
                    } else {
                        Style::new().fg(Color::Black).bg(HIT)
                    };
                    let mut spans = vec![Span::styled(
                        num.clone(),
                        if selected { base } else { Style::new().fg(DIM) },
                    )];
                    spans.extend(highlight(
                        &l.text,
                        &l.ranges,
                        base,
                        hl,
                        width.saturating_sub(num.chars().count()),
                    ));
                    lines.push(Line::from(spans));
                }
            }
        }
        f.render_widget(Paragraph::new(lines), inner);
    }

    fn draw_vaults(&self, f: &mut Frame, area: Rect) {
        let title = Line::from(vec![
            Span::styled(
                " cold-lava ",
                Style::new().fg(ACCENT).add_modifier(Modifier::BOLD),
            ),
            Span::styled("vaults ", Style::new().fg(MUTED)),
        ]);
        let block = self.pane(title, Focus::Vaults);
        let inner = block.inner(area);
        f.render_widget(block, area);
        if self.vaults.is_empty() {
            f.render_widget(
                Paragraph::new("No vaults found.\n\nPress a to add a folder.")
                    .style(Style::new().fg(MUTED))
                    .wrap(Wrap { trim: false }),
                inner,
            );
            return;
        }
        let w = inner.width as usize;
        let lines: Vec<Line> = self
            .vaults
            .iter()
            .enumerate()
            .take(inner.height as usize)
            .map(|(i, v)| {
                let sel = i == self.vsel;
                let mark = match v.source {
                    "obsidian" => "◆",
                    "example" => "✦",
                    _ => "◇",
                };
                let mut name: String = v.name.chars().take(w.saturating_sub(3)).collect();
                if v.name.chars().count() > w.saturating_sub(3) {
                    name.pop();
                    name.push('…');
                }
                let row = format!("{mark} {name:<width$}", width = w.saturating_sub(2));
                let st = if sel && self.focus == Focus::Vaults {
                    Style::new().fg(Color::Black).bg(ACCENT)
                } else if sel {
                    Style::new().fg(ACCENT).add_modifier(Modifier::BOLD)
                } else {
                    Style::new().fg(NOTE)
                };
                Line::styled(row, st)
            })
            .collect();
        f.render_widget(Paragraph::new(lines), inner);
    }

    fn draw_notes(&mut self, f: &mut Frame, area: Rect) {
        let vname = self
            .vaults
            .get(self.vsel)
            .map(|v| v.name.clone())
            .unwrap_or_default();
        let counts = format!(
            "{} notes ",
            self.items
                .iter()
                .filter(|i| i.kind == ItemKind::Note)
                .count()
        );
        let title = Line::from(vec![
            Span::styled(
                format!(" {vname} "),
                Style::new().fg(NOTE).add_modifier(Modifier::BOLD),
            ),
            Span::styled(counts, Style::new().fg(MUTED)),
        ]);
        let mut block = self.pane(title, Focus::Notes);
        if !self.filter.is_empty() || matches!(self.mode, Mode::Filter) {
            let cursor = if matches!(self.mode, Mode::Filter) {
                "▏"
            } else {
                ""
            };
            block = block.title_bottom(Line::from(Span::styled(
                format!(" /{}{cursor} ", self.filter),
                Style::new().fg(HIT),
            )));
        }
        let inner = block.inner(area);
        f.render_widget(block, area);
        let h = inner.height as usize;
        if self.nsel < self.nscroll {
            self.nscroll = self.nsel;
        } else if h > 0 && self.nsel >= self.nscroll + h {
            self.nscroll = self.nsel + 1 - h;
        }
        let w = inner.width as usize;
        let flat = !self.filter.is_empty();
        let mut lines = Vec::new();
        for (r, &i) in self.rows.iter().enumerate().skip(self.nscroll).take(h) {
            let it = &self.items[i];
            let sel = r == self.nsel;
            let indent = if flat {
                String::new()
            } else {
                "  ".repeat(it.depth)
            };
            let (icon, icon_color) = match it.kind {
                ItemKind::Folder if self.expanded.contains(&it.rel) => ("▾ ", MUTED),
                ItemKind::Folder => ("▸ ", MUTED),
                ItemKind::Board => ("▦ ", Color::Rgb(83, 223, 221)),
                ItemKind::Note => ("• ", DIM),
            };
            let label = if flat {
                it.rel.trim_end_matches(".md").to_string()
            } else {
                it.name.clone()
            };
            let badge = match (it.kind, self.links.get(&it.rel)) {
                (ItemKind::Note, Some(&(inc, _))) if inc > 0 => format!("←{inc}"),
                _ => String::new(),
            };
            let room = w.saturating_sub(indent.chars().count() + 2 + badge.chars().count() + 1);
            let mut text: String = label.chars().take(room).collect();
            if label.chars().count() > room && room > 0 {
                text.pop();
                text.push('…');
            }
            let pad = w.saturating_sub(
                indent.chars().count() + 2 + text.chars().count() + badge.chars().count(),
            );
            let (fg, bg) = if sel && self.focus == Focus::Notes {
                (Color::Black, Some(ACCENT))
            } else if sel {
                (ACCENT, None)
            } else if it.kind == ItemKind::Folder {
                (ACCENT_SOFT, None)
            } else {
                (NOTE, None)
            };
            let mut st = Style::new().fg(fg);
            if let Some(b) = bg {
                st = st.bg(b);
            }
            if it.kind == ItemKind::Folder || sel {
                st = st.add_modifier(Modifier::BOLD);
            }
            let icon_st = if bg.is_some() {
                st
            } else {
                Style::new().fg(icon_color)
            };
            let badge_st = if bg.is_some() {
                st
            } else {
                Style::new().fg(MUTED)
            };
            lines.push(Line::from(vec![
                Span::styled(indent, st),
                Span::styled(icon, icon_st),
                Span::styled(text, st),
                Span::styled(" ".repeat(pad), st),
                Span::styled(badge, badge_st),
            ]));
        }
        if self.rows.is_empty() {
            let msg = if flat {
                "no matches"
            } else {
                "no notes yet · n to create one"
            };
            lines.push(Line::styled(msg, Style::new().fg(MUTED)));
        }
        f.render_widget(Paragraph::new(lines), inner);
    }

    fn draw_editor(&mut self, f: &mut Frame, area: Rect, inner: Rect) {
        let title = match &self.shown {
            Some(p) => {
                let rel = self
                    .root
                    .as_ref()
                    .and_then(|r| p.strip_prefix(r).ok())
                    .map(|r| r.to_string_lossy().into_owned())
                    .unwrap_or_default();
                let who = match (&self.editor_cmd, self.focus == Focus::Editor) {
                    (Some(c), true) => format!(" {c} · editing · Ctrl-q back "),
                    (Some(c), false) => format!(" {c} · ⏎ edit "),
                    (None, _) => " preview · e edit in $EDITOR ".to_string(),
                };
                Line::from(vec![
                    Span::styled(
                        format!(" {} ", rel.trim_end_matches(".md")),
                        Style::new().fg(NOTE).add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(who, Style::new().fg(MUTED)),
                ])
            }
            None => Line::styled(" note ", Style::new().fg(MUTED)),
        };
        let block = self.pane(title, Focus::Editor);
        f.render_widget(block, area);
        if let Some(ed) = self.editor.as_mut() {
            ed.resize(inner.height, inner.width);
            if let Some((x, y)) = ed.render(f.buffer_mut(), inner)
                && self.focus == Focus::Editor
                && matches!(self.mode, Mode::Normal)
            {
                f.set_cursor_position((x, y));
            }
            return;
        }
        if self.shown.is_some() && self.editor_cmd.is_none() {
            let text = board::markdown_text(&self.preview.join("\n"));
            f.render_widget(Paragraph::new(text).wrap(Wrap { trim: false }), inner);
        } else {
            let hint = match self.current().map(|it| it.kind) {
                Some(ItemKind::Folder) => "a folder · → to open it",
                Some(ItemKind::Board) => "a whiteboard · ⏎ to open it",
                _ => "select a note to see it here",
            };
            f.render_widget(
                Paragraph::new(Line::styled(hint, Style::new().fg(MUTED))),
                inner,
            );
        }
    }

    fn draw_backlinks(&self, f: &mut Frame, area: Rect) {
        let n = self.backs.len();
        let title = Line::from(vec![
            Span::styled(
                " Backlinks ",
                Style::new().fg(NOTE).add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                if n == 0 {
                    "none · b hide ".to_string()
                } else {
                    format!("{n} · b hide ")
                },
                Style::new().fg(MUTED),
            ),
        ]);
        let block = self.pane(title, Focus::Backlinks);
        let inner = block.inner(area);
        f.render_widget(block, area);
        if n == 0 {
            let msg = "No other note links here yet.";
            f.render_widget(
                Paragraph::new(Line::styled(msg, Style::new().fg(MUTED))),
                inner,
            );
            return;
        }
        let h = inner.height as usize;
        let start = if self.bsel >= h { self.bsel + 1 - h } else { 0 };
        let focused = self.focus == Focus::Backlinks;
        let lines: Vec<Line> = self
            .backs
            .iter()
            .enumerate()
            .skip(start)
            .take(h)
            .map(|(i, b)| {
                let sel = i == self.bsel && focused;
                let name_st = if sel {
                    Style::new()
                        .fg(Color::Black)
                        .bg(ACCENT)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::new().fg(ACCENT_SOFT).add_modifier(Modifier::BOLD)
                };
                let mut spans = vec![
                    Span::styled(format!(" ← {} ", b.name), name_st),
                    Span::raw(" "),
                ];
                // The line with the link, with light markdown styling.
                let ctx = board::markdown_text(&b.context);
                if let Some(first) = ctx.lines.into_iter().next() {
                    spans.extend(first.spans);
                }
                Line::from(spans)
            })
            .collect();
        f.render_widget(Paragraph::new(lines), inner);
    }

    fn draw_status(&mut self, f: &mut Frame, area: Rect) {
        let info = match (self.focus, self.vaults.get(self.vsel)) {
            (Focus::Vaults, Some(v)) => {
                let src = match v.source {
                    "obsidian" => "from Obsidian",
                    "example" => "example vault",
                    "here" => "opened here",
                    _ => "recent",
                };
                Line::from(vec![
                    Span::styled(format!(" {} ", v.path.display()), Style::new().fg(NOTE)),
                    Span::styled(format!(" {src}"), Style::new().fg(MUTED)),
                ])
            }
            _ => match self.current() {
                Some(it) => {
                    let (inc, out) = self.links.get(&it.rel).copied().unwrap_or_default();
                    let extra = if it.kind == ItemKind::Note {
                        format!("   ←{inc} in  {out}→ out")
                    } else {
                        String::new()
                    };
                    Line::from(vec![
                        Span::styled(format!(" {}", it.rel), Style::new().fg(NOTE)),
                        Span::styled(extra, Style::new().fg(ACCENT_SOFT)),
                    ])
                }
                None => Line::raw(""),
            },
        };
        if self
            .msg
            .as_ref()
            .is_some_and(|(_, t)| t.elapsed() > Duration::from_secs(3))
        {
            self.msg = None;
        }
        let hints = match (self.focus, &self.mode) {
            (_, Mode::Filter) => " type to filter · ↑↓ move · ⏎ keep · Esc clear".to_string(),
            (Focus::Editor, _) => {
                " in Neovim · ⏎ on a [[link]] follows it · Ctrl-o back · Ctrl-q to the menu"
                    .to_string()
            }
            (Focus::Backlinks, _) => {
                " backlinks · ↑↓ choose · ⏎ open at the link · ← back to notes · b hide".to_string()
            }
            _ => {
                let edit = if self.editor_cmd.is_some() {
                    "⏎ edit"
                } else {
                    "⏎ open"
                };
                format!(
                    " ↑↓ move  ←→ fold  {edit}  / filter  p go to  : commands  s search  R rename  m move  g graph  w boards  t today  n new  z zoom  ? help  q quit"
                )
            }
        };
        let second = match &self.msg {
            Some((m, _)) => Line::styled(format!(" {m}"), Style::new().fg(HIT)),
            None => Line::styled(hints, Style::new().fg(DIM)),
        };
        f.render_widget(Paragraph::new(vec![info, second]), area);
    }
}

fn plural(n: usize, word: &str) -> String {
    if n == 1 {
        format!("1 {word}")
    } else {
        format!("{n} {word}s")
    }
}

fn dir_of_rel(rel: &str) -> &str {
    rel.rsplit_once('/').map(|(d, _)| d).unwrap_or("")
}

/// `text` with `ranges` (char ranges) highlighted, cut to `width` chars around the first match.
fn highlight(
    text: &str,
    ranges: &[(usize, usize)],
    base: Style,
    hl: Style,
    width: usize,
) -> Vec<Span<'static>> {
    let chars: Vec<char> = text.trim_start().chars().collect();
    let shift = text.chars().count() - chars.len();
    let ranges: Vec<(usize, usize)> = ranges
        .iter()
        .map(|&(a, b)| (a.saturating_sub(shift), b.saturating_sub(shift)))
        .collect();
    // Start a little before the first match if it would be off-screen.
    let first = ranges.first().map(|r| r.0).unwrap_or(0);
    let start = if first + 10 > width {
        first.saturating_sub(width / 3)
    } else {
        0
    };
    let mut spans = Vec::new();
    let mut avail = width;
    if start > 0 {
        spans.push(Span::styled("…", base));
        avail = avail.saturating_sub(1);
    }
    let mut cur = String::new();
    let mut cur_hl = false;
    for (i, ch) in chars.iter().enumerate().skip(start).take(avail) {
        let in_hl = ranges.iter().any(|&(a, b)| i >= a && i < b);
        if in_hl != cur_hl && !cur.is_empty() {
            spans.push(Span::styled(
                std::mem::take(&mut cur),
                if cur_hl { hl } else { base },
            ));
        }
        cur_hl = in_hl;
        cur.push(*ch);
    }
    if !cur.is_empty() {
        spans.push(Span::styled(cur, if cur_hl { hl } else { base }));
    }
    spans
}

fn prompt(f: &mut Frame, area: Rect, title: &str, label: &str, value: &str) {
    let w = 60.min(area.width.saturating_sub(4));
    let r = Rect::new(
        area.x + (area.width - w) / 2,
        area.y + area.height / 3,
        w,
        3,
    );
    f.render_widget(Clear, r);
    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(label.to_string(), Style::new().fg(MUTED)),
            Span::styled(format!("{value}▏"), Style::new().fg(NOTE)),
        ]))
        .block(
            Block::bordered()
                .border_type(BorderType::Rounded)
                .border_style(Style::new().fg(ACCENT))
                .title(title.to_string()),
        ),
        r,
    );
}

/// The centred popup shared by the quick switcher and the command palette; returns its inside.
fn picker_frame(f: &mut Frame, area: Rect, title: &str, hint: &str) -> Rect {
    let w = (area.width * 2 / 3).clamp(40.min(area.width), 100);
    let h = (area.height * 2 / 3).clamp(8.min(area.height), 30);
    let r = Rect::new(
        area.x + (area.width - w) / 2,
        area.y + area.height / 6,
        w,
        h,
    );
    f.render_widget(Clear, r);
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(ACCENT))
        .title(Line::styled(
            title,
            Style::new().fg(ACCENT).add_modifier(Modifier::BOLD),
        ))
        .title_bottom(Line::styled(hint, Style::new().fg(MUTED)));
    let inner = block.inner(r);
    f.render_widget(block, r);
    inner
}

fn draw_help(f: &mut Frame, area: Rect, keys: &Keymap<MenuAction>) {
    let fixed: &[(&str, &str)] = &[
        ("↑↓ / jk", "move through vaults or notes"),
        ("→ / ←", "open / close a folder · vaults ↔ notes"),
        (
            "Enter",
            "edit the note in the embedded Neovim · open a board",
        ),
        ("Ctrl-q", "leave Neovim and return to the menu"),
        (
            "⏎ in Neovim",
            "on a [[link]]: open it (creates missing notes)",
        ),
        ("Ctrl-o", "in Neovim: back to the previous note"),
    ];
    let commands = commands::MENU.iter().map(|c| (keys.label(c.id), c.title));
    let rows: Vec<(String, &str)> = fixed
        .iter()
        .map(|&(k, d)| (k.to_string(), d))
        .chain(commands)
        .collect();
    let lines: Vec<Line> = rows
        .iter()
        .map(|(k, d)| {
            Line::from(vec![
                Span::styled(format!("{k:>12}  "), Style::new().fg(ACCENT)),
                Span::styled(*d, Style::new().fg(NOTE)),
            ])
        })
        .collect();
    let w = 70.min(area.width.saturating_sub(4));
    let h = (rows.len() as u16 + 3).min(area.height);
    let r = Rect::new(
        area.x + (area.width - w) / 2,
        area.y + (area.height - h) / 2,
        w,
        h,
    );
    f.render_widget(Clear, r);
    f.render_widget(
        Paragraph::new(lines).block(
            Block::bordered()
                .border_type(BorderType::Rounded)
                .border_style(Style::new().fg(ACCENT))
                .title(format!(
                    " cold-lava {} · any key closes ",
                    env!("CARGO_PKG_VERSION")
                )),
        ),
        r,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_obsidian_vault_list() {
        let json = r#"{"vaults":{"a1":{"path":"/home/me/Notes","ts":1700000000000,"open":true},
                                 "b2":{"path":"/home/me/Work"}},"frame":"hidden"}"#;
        let mut v = parse_obsidian_vaults(json);
        v.sort();
        assert_eq!(
            v,
            [
                (PathBuf::from("/home/me/Notes"), 1700000000000),
                (PathBuf::from("/home/me/Work"), 0)
            ]
        );
        assert!(parse_obsidian_vaults("not json").is_empty());
    }

    #[test]
    fn tree_sorting_and_folding() {
        let items = scan_tree(&Path::new(env!("CARGO_MANIFEST_DIR")).join("example-vault"));
        let rels: Vec<&str> = items.iter().map(|i| i.rel.as_str()).collect();
        // Folders first, then top-level files; boards are included.
        assert_eq!(rels[0], "Concepts");
        assert!(rels.contains(&"Welcome.canvas"));
        let concepts = rels.iter().position(|r| *r == "Concepts").unwrap();
        let guide = rels.iter().position(|r| *r == "Guide").unwrap();
        assert!(
            rels[concepts + 1].starts_with("Concepts/"),
            "folder contents follow the folder"
        );
        assert!(guide > concepts);
        assert!(!rels.iter().any(|r| r.starts_with(".obsidian")));

        let collapsed = visible(&items, &HashSet::new(), "");
        assert!(collapsed.iter().all(|&i| items[i].depth == 0));
        let open: HashSet<String> = ["Volcanoes".to_string()].into();
        let rows = visible(&items, &open, "");
        assert!(rows.iter().any(|&i| items[i].rel == "Volcanoes/Kīlauea.md"));
        assert!(!rows.iter().any(|&i| items[i].rel.starts_with("Concepts/")));

        let found = visible(&items, &HashSet::new(), "kil");
        assert_eq!(items[found[0]].rel, "Volcanoes/Kīlauea.md");
    }

    #[test]
    fn vim_open_command_escapes_quotes() {
        let c = open_command_at(Path::new("/v/It's here.md"), None);
        assert!(c.contains("fnameescape('/v/It''s here.md')"));
        assert!(c.starts_with("\x1c\x0e") && c.ends_with(")\r"));
        let c = open_command_at(Path::new("/v/a.md"), Some(&Jump::Line(12)));
        assert!(c.ends_with("| call cursor(12, 1) | normal! zz\r"));
        let c = open_command_at(
            Path::new("/v/a.md"),
            Some(&Jump::Heading("Don't panic".into())),
        );
        assert!(
            c.contains("search('\\v^#+\\s*\\V' . 'Don''t panic', 'cW')"),
            "{c}"
        );
    }
}
