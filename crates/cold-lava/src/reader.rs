//! The reader: a note's `Doc` laid out as styled lines for the pane's width, with the place of
//! every link kept, so keys and clicks can select and follow them.

use std::ops::Range;

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

use crate::app::{ACCENT, ACCENT_SOFT, DIM, GHOST, HIT, MUTED, NOTE, TAG};
use crate::core::Jump;
use crate::looks::{self, Looks};
use crate::markdown::{Block, BlockKind, Doc, Inline, LinkKind};

const LINK: Color = Color::Rgb(150, 180, 250);
const CODE: Color = Color::Rgb(160, 200, 160);
const CODE_BG: Color = Color::Rgb(40, 40, 50);
const MATH: Color = Color::Rgb(220, 170, 240);
const HIGHLIGHT_BG: Color = Color::Rgb(95, 80, 30);
const GREEN: Color = Color::Rgb(68, 207, 110);
const CYAN: Color = Color::Rgb(83, 223, 221);
const RED: Color = Color::Rgb(233, 73, 73);

pub struct Link {
    /// As written: `Note#Heading` for a wiki link, the destination for the others.
    pub target: String,
    pub kind: LinkKind,
    /// (rendered line, columns) of each piece: a link wrapped over two lines has two.
    pub pieces: Vec<(usize, Range<usize>)>,
}

/// The doc laid out for one width.
#[derive(Default)]
struct Page {
    width: u16,
    lines: Vec<Line<'static>>,
    links: Vec<Link>,
    /// (source line, first rendered line) of every block, in order.
    anchors: Vec<(usize, usize)>,
    headings: Vec<Heading>,
    block_ids: Vec<(String, usize)>,
    /// (source line, rendered lines) of every task; a nested task's lines are inside its
    /// parent's.
    tasks: Vec<(usize, Range<usize>)>,
    /// Lines of the properties above the note: reading starts below them.
    head: usize,
}

/// A heading as the reader draws it: folded callouts hide theirs.
pub struct Heading {
    pub level: u8,
    pub text: String,
    /// The rendered line it starts on.
    pub line: usize,
}

/// Where the reader is in a note, to come back to it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Spot {
    scroll: usize,
    reading: Option<usize>,
    selected: Option<usize>,
}

pub struct Reader {
    doc: Doc,
    looks: Looks,
    page: Option<Page>,
    scroll: usize,
    /// The line jumped to, when the page couldn't scroll it to the top (near the end of the
    /// note). Scrolling forgets it.
    reading: Option<usize>,
    /// Rows the pane had when last drawn.
    height: usize,
    selected: Option<usize>,
    /// Applied once the doc is laid out.
    pending_jump: Option<Jump>,
}

impl Reader {
    pub fn new(doc: Doc, looks: Looks) -> Reader {
        Reader {
            doc,
            looks,
            page: None,
            scroll: 0,
            reading: None,
            height: 0,
            selected: None,
            pending_jump: None,
        }
    }

    /// The same note, read again: keeps the scroll position.
    pub fn set_doc(&mut self, doc: Doc, looks: Looks) {
        self.doc = doc;
        self.looks = looks;
        self.page = None;
    }

    /// Links may resolve differently now (notes were created or removed).
    pub fn invalidate(&mut self) {
        self.page = None;
    }

    /// Lays the doc out for `width` unless it already is. `resolved` tells whether a wiki or
    /// Markdown link's target exists.
    pub fn layout(&mut self, width: u16, resolved: &dyn Fn(&str) -> bool) {
        if self.page.as_ref().is_some_and(|p| p.width == width) {
            return;
        }
        let mut b = Builder {
            width: width as usize,
            resolved,
            page: Page {
                width,
                ..Page::default()
            },
            depth: 0,
            looks: &self.looks,
            broken: &self.doc.broken,
            active: Vec::new(),
        };
        b.properties(&mut Gutter::default());
        b.page.head = b.page.lines.len();
        b.blocks(&self.doc.blocks, &mut Gutter::default(), NOTE.into(), true);
        let page = b.page;
        if self.selected.is_some_and(|s| s >= page.links.len()) {
            self.selected = None;
        }
        self.page = Some(page);
        if let Some(j) = self.pending_jump.take() {
            self.jump(j);
        }
        self.clamp();
    }

    pub fn render(&mut self, area: Rect, buf: &mut Buffer) {
        self.height = area.height as usize;
        if let Some(j) = self.pending_jump.take() {
            self.jump(j);
        }
        self.clamp();
        let Some(page) = &self.page else { return };
        let rows = page.lines.iter().skip(self.scroll).take(self.height);
        for (y, line) in (area.y..).zip(rows) {
            buf.set_line(area.x, y, line, area.width);
        }
        // A jump that couldn't scroll its line to the top shows where it landed.
        if let Some(row) = self.reading.and_then(|l| l.checked_sub(self.scroll)) {
            let row = Rect::new(area.x, area.y + row as u16, area.width, 1);
            buf.set_style(row.intersection(area), Style::new().bg(CODE_BG));
        }
        let Some(link) = self.selected.map(|s| &page.links[s]) else {
            return;
        };
        let selected = Style::new().fg(Color::Black).bg(ACCENT);
        for (line, cols) in &link.pieces {
            let Some(row) = line.checked_sub(self.scroll).filter(|&r| r < self.height) else {
                continue;
            };
            let x = (cols.start as u16).min(area.width);
            let w = (cols.end as u16).min(area.width) - x;
            buf.set_style(Rect::new(area.x + x, area.y + row as u16, w, 1), selected);
        }
    }

    /// How many lines the note takes, as last laid out.
    pub fn line_count(&self) -> usize {
        self.page.as_ref().map_or(0, |p| p.lines.len())
    }

    fn max_scroll(&self) -> usize {
        self.line_count().saturating_sub(self.height)
    }

    fn clamp(&mut self) {
        self.scroll = self.scroll.min(self.max_scroll());
    }

    /// Scrolls `n` lines. Where the page can't scroll further down, the line being read moves
    /// instead, so every line of the last page can be the one being read.
    pub fn scroll_by(&mut self, n: isize) {
        let line = self.reading.unwrap_or(self.scroll);
        let last = self.line_count().saturating_sub(1);
        if n > 0 && self.scroll + n as usize > self.max_scroll() {
            let to = line.saturating_add(n as usize).min(last);
            self.scroll = self.max_scroll();
            self.reading = (to > self.scroll).then_some(to);
            return;
        }
        if n < 0 && line > self.scroll {
            let to = line.saturating_add_signed(n);
            self.reading = (to > self.scroll).then_some(to);
            return;
        }
        self.scroll = self.scroll.saturating_add_signed(n);
        self.reading = None;
        self.clamp();
    }

    /// A page down (`1`) or up (`-1`), keeping two lines of context.
    pub fn page(&mut self, direction: isize) {
        let step = self.height.saturating_sub(2).max(1) as isize;
        self.scroll_by(step * direction);
    }

    pub fn top(&mut self) {
        self.scroll = 0;
        self.reading = None;
    }

    pub fn bottom(&mut self) {
        self.scroll = self.max_scroll();
        self.reading = None;
    }

    /// Puts rendered line `at` at the top of the view, or as high as the page can scroll.
    fn scroll_to(&mut self, at: usize) {
        self.scroll = at;
        self.clamp();
        self.reading = (self.scroll != at).then_some(at);
    }

    /// The line being read: the top one, unless a jump landed lower.
    fn reading_line(&self) -> usize {
        let head = self.page.as_ref().map_or(0, |p| p.head);
        self.reading.unwrap_or(self.scroll).max(head)
    }

    pub fn headings(&self) -> &[Heading] {
        self.page.as_ref().map_or(&[], |p| &p.headings)
    }

    /// The headings the line being read is under, outermost first, as indices into `headings`.
    pub fn section(&self) -> Vec<usize> {
        let line = self.reading_line();
        let headings = self.headings();
        let mut path: Vec<usize> = Vec::new();
        for (i, h) in headings
            .iter()
            .enumerate()
            .take_while(|(_, h)| h.line <= line)
        {
            while path.last().is_some_and(|&p| headings[p].level >= h.level) {
                path.pop();
            }
            path.push(i);
        }
        path
    }

    pub fn go_to_heading(&mut self, i: usize) {
        if let Some(at) = self.headings().get(i).map(|h| h.line) {
            self.scroll_to(at);
        }
    }

    pub fn next_heading(&mut self) {
        let line = self.reading_line();
        if let Some(i) = self.headings().iter().position(|h| h.line > line) {
            self.go_to_heading(i);
        }
    }

    /// The heading of the section being read, or the one before it when it is at the top.
    pub fn previous_heading(&mut self) {
        let line = self.reading_line();
        if let Some(i) = self.headings().iter().rposition(|h| h.line < line) {
            self.go_to_heading(i);
        }
    }

    /// The source line of the task `x` ticks: the one the selected link is in, else the one
    /// being read, else the first one on screen below it.
    pub fn task_line(&self) -> Option<usize> {
        let page = self.page.as_ref()?;
        let innermost = |line: usize| {
            (page.tasks.iter())
                .filter(|(_, at)| at.contains(&line))
                .max_by_key(|(_, at)| at.start)
                .map(|t| t.0)
        };
        if let Some(t) = self.selected_at().and_then(|_| {
            let link = self.selected_link()?;
            innermost(link.pieces[0].0)
        }) {
            return Some(t);
        }
        let line = self.reading_line();
        innermost(line).or_else(|| {
            (page.tasks.iter())
                .filter(|(_, at)| at.start >= line && at.start < self.scroll + self.height)
                .min_by_key(|(_, at)| at.start)
                .map(|t| t.0)
        })
    }

    fn links(&self) -> &[Link] {
        self.page.as_ref().map_or(&[], |p| &p.links)
    }

    /// The next link, from the first one on screen when none is selected yet; after the last
    /// it starts again from the first.
    pub fn next_link(&mut self) {
        let n = self.links().len();
        if n == 0 {
            return;
        }
        let next = match self.selected {
            Some(s) => (s + 1) % n,
            None => self
                .links()
                .iter()
                .position(|l| l.pieces[0].0 >= self.scroll)
                .unwrap_or(0),
        };
        self.select(next);
    }

    pub fn previous_link(&mut self) {
        let n = self.links().len();
        if n == 0 {
            return;
        }
        let bottom = self.scroll + self.height;
        let previous = match self.selected {
            Some(s) => (s + n - 1) % n,
            None => self
                .links()
                .iter()
                .rposition(|l| l.pieces[0].0 < bottom)
                .unwrap_or(n - 1),
        };
        self.select(previous);
    }

    /// Selects a link and scrolls it into view.
    pub fn select(&mut self, i: usize) {
        let Some(link) = self.links().get(i) else {
            return;
        };
        let (first, last) = (link.pieces[0].0, link.pieces[link.pieces.len() - 1].0);
        self.selected = Some(i);
        if first < self.scroll {
            self.scroll = first;
            self.reading = None;
        } else if last >= self.scroll + self.height {
            self.scroll = (last + 1).saturating_sub(self.height);
            self.reading = None;
        }
        self.clamp();
    }

    pub fn spot(&self) -> Spot {
        Spot {
            scroll: self.scroll,
            reading: self.reading,
            selected: self.selected,
        }
    }

    /// Back to a `spot` of this note; kept within the note once it is laid out.
    pub fn restore(&mut self, spot: Spot) {
        self.scroll = spot.scroll;
        self.reading = spot.reading;
        self.selected = spot.selected;
        self.pending_jump = None;
    }

    pub fn selected_link(&self) -> Option<&Link> {
        self.links().get(self.selected?)
    }

    /// Where the selected link is drawn: (row, column) in the pane, when it is on screen.
    pub fn selected_at(&self) -> Option<(u16, u16)> {
        let s = self.selected?;
        let links = self.visible_links();
        links
            .into_iter()
            .find(|l| l.0 == s)
            .map(|(_, row, col)| (row, col))
    }

    /// The links on screen in reading order: (link, row, column in the pane) of where the first
    /// piece on screen starts.
    pub fn visible_links(&self) -> Vec<(usize, u16, u16)> {
        let rows = self.scroll..self.scroll + self.height;
        let links = self.links().iter().enumerate();
        links
            .filter_map(|(i, l)| {
                let (line, cols) = l.pieces.iter().find(|(at, _)| rows.contains(at))?;
                Some((i, (line - self.scroll) as u16, cols.start as u16))
            })
            .collect()
    }

    /// The link drawn at `row`, `col` of the pane.
    pub fn link_at(&self, row: usize, col: usize) -> Option<usize> {
        let line = self.scroll + row;
        self.links().iter().position(|l| {
            l.pieces
                .iter()
                .any(|(at, cols)| *at == line && cols.contains(&col))
        })
    }

    /// Scrolls to a line of the note, a heading or a block id.
    pub fn jump(&mut self, jump: Jump) {
        // It waits for the page's height too: clamped to an unknown height, it would be lost.
        let Some(page) = self.page.as_ref().filter(|_| self.height > 0) else {
            self.pending_jump = Some(jump);
            return;
        };
        let found = match &jump {
            Jump::Line(n) => page
                .anchors
                .iter()
                .rev()
                .find(|(line, _)| line <= n)
                .map(|(_, at)| *at),
            Jump::Heading(h) => {
                let h = crate::vault::search_key(h.trim());
                let key = |x: &&Heading| crate::vault::search_key(&x.text);
                let headings = || page.headings.iter();
                headings()
                    .find(|x| key(x) == h)
                    .or_else(|| headings().find(|x| key(x).starts_with(&h)))
                    .map(|x| x.line)
            }
            Jump::Block(id) => page
                .block_ids
                .iter()
                .find(|(b, _)| b == id)
                .map(|(_, at)| *at),
        };
        if let Some(at) = found {
            self.scroll_to(at);
        }
    }
}

/// What goes before a block's lines: `first` on its first line, `rest` on the others.
#[derive(Clone, Default)]
struct Gutter {
    first: Vec<Span<'static>>,
    rest: Vec<Span<'static>>,
}

impl Gutter {
    fn width(&self) -> usize {
        self.rest.iter().map(Span::width).sum()
    }

    /// The gutter of a block inside this one: `marker` on its first line, then as many spaces.
    fn nest(&mut self, marker: Span<'static>) -> Gutter {
        let pad = Span::raw(" ".repeat(marker.width()));
        self.nest_with(marker, pad)
    }

    fn nest_with(&mut self, first: Span<'static>, rest: Span<'static>) -> Gutter {
        let mut inner = Gutter {
            first: std::mem::replace(&mut self.first, self.rest.clone()),
            rest: self.rest.clone(),
        };
        inner.first.push(first);
        inner.rest.push(rest);
        inner
    }

    fn take(&mut self) -> Vec<Span<'static>> {
        std::mem::replace(&mut self.first, self.rest.clone())
    }
}

/// A piece of text with its style and the link it belongs to.
type Seg = (String, Style, Option<usize>);

enum Tok {
    Word(Seg),
    Space(Style, Option<usize>),
    Break,
}

fn width(s: &str) -> usize {
    Span::raw(s).width()
}

struct Builder<'a> {
    width: usize,
    resolved: &'a dyn Fn(&str) -> bool,
    page: Page,
    /// How deep in lists, for the bullet.
    depth: usize,
    looks: &'a Looks,
    /// `Doc::broken`: marks whose look is skipped.
    broken: &'a [bool],
    /// The marks around the inline being flattened, outermost first, with the link a `link`
    /// look made.
    active: Vec<(usize, Option<usize>)>,
}

impl Builder<'_> {
    /// The note's own typed values its body doesn't show (frontmatter), on a line above it.
    fn properties(&mut self, g: &mut Gutter) {
        let mut inlines = Vec::new();
        for &id in &self.looks.header {
            let mark = &self.looks.marks[id];
            if !inlines.is_empty() {
                inlines.push(Inline::Text("  ".into()));
            }
            if mark.look.icon.is_none() {
                inlines.push(Inline::Text(format!("{} ", mark.field)));
            }
            inlines.push(Inline::Mark { id, open: true });
            inlines.push(Inline::Text(mark.value.clone()));
            inlines.push(Inline::Mark { id, open: false });
        }
        if inlines.is_empty() {
            return;
        }
        let toks = self.tokens(&inlines, MUTED.into());
        self.wrap(toks, g);
        let gutter = g.take();
        self.page.lines.push(Line::from(gutter));
    }

    fn blocks(&mut self, blocks: &[Block], g: &mut Gutter, base: Style, spaced: bool) {
        for (i, b) in blocks.iter().enumerate() {
            if i > 0 && spaced {
                let gutter = g.take();
                self.page.lines.push(Line::from(gutter));
            }
            self.block(b, g, base);
        }
    }

    fn block(&mut self, b: &Block, g: &mut Gutter, base: Style) {
        let at = self.page.lines.len();
        self.page.anchors.push((b.line, at));
        match &b.kind {
            BlockKind::Heading { level, inlines } => {
                let colour = match level {
                    1 | 2 => ACCENT,
                    3 => ACCENT_SOFT,
                    _ => NOTE,
                };
                let style = Style::new().fg(colour).add_modifier(Modifier::BOLD);
                let marker = Span::styled(format!("{} ", "#".repeat(*level as usize)), DIM);
                let toks = self.tokens(inlines, style);
                let mut text = String::new();
                plain(inlines, &mut text);
                self.page.headings.push(Heading {
                    level: *level,
                    text,
                    line: at,
                });
                self.wrap(toks, &mut g.nest(marker));
            }
            BlockKind::Paragraph(inlines) => {
                let toks = self.tokens(inlines, base);
                self.wrap(toks, g);
            }
            BlockKind::List { start, items } => {
                let widest = start.map_or(0, |s| (s as usize + items.len()).to_string().len());
                self.depth += 1;
                for (n, item) in items.iter().enumerate() {
                    let (marker, style) = match (item.task, start) {
                        (Some(false), _) => (Span::styled("☐ ", MUTED), base),
                        (Some(true), _) => (Span::styled("☑ ", ACCENT), MUTED.into()),
                        (None, Some(s)) => (
                            Span::styled(format!("{:>widest$}. ", *s as usize + n), ACCENT_SOFT),
                            base,
                        ),
                        (None, None) => {
                            let bullet = ["• ", "◦ ", "▪ "][(self.depth - 1) % 3];
                            (Span::styled(bullet, ACCENT_SOFT), base)
                        }
                    };
                    let start = self.page.lines.len();
                    self.blocks(&item.blocks, &mut g.nest(marker), style, false);
                    if let (Some(_), Some(first)) = (item.task, item.blocks.first()) {
                        let lines = start..self.page.lines.len();
                        self.page.tasks.push((first.line, lines));
                    }
                }
                self.depth -= 1;
            }
            BlockKind::Quote(blocks) => {
                let bar = Span::styled("│ ", DIM);
                let mut inner = g.nest_with(bar.clone(), bar);
                self.blocks(
                    blocks,
                    &mut inner,
                    base.add_modifier(Modifier::ITALIC),
                    true,
                );
            }
            BlockKind::Callout {
                kind,
                title,
                folded,
                blocks,
            } => {
                let colour = callout_colour(kind);
                let bar = Span::styled("│ ", colour);
                let mut inner = g.nest_with(bar.clone(), bar);
                let head = Style::new().fg(colour).add_modifier(Modifier::BOLD);
                let fold = match folded {
                    Some(true) => "▸ ",
                    Some(false) => "▾ ",
                    None => "",
                };
                let mut toks = vec![Tok::Word((format!("{fold}{kind}"), head, None))];
                if !title.is_empty() {
                    toks.push(Tok::Word((":".into(), head, None)));
                    toks.push(Tok::Space(head, None));
                    toks.extend(self.tokens(title, head));
                }
                self.wrap(toks, &mut inner);
                if *folded != Some(true) {
                    self.blocks(blocks, &mut inner, base, true);
                }
            }
            BlockKind::Code { lang, text } => {
                let bar = Span::styled("│ ", DIM);
                let mut inner = g.nest_with(bar.clone(), bar);
                if let Some(lang) = lang {
                    let label = Style::new().fg(MUTED).add_modifier(Modifier::ITALIC);
                    self.emit(&mut inner, vec![(lang.clone(), label, None)]);
                }
                self.verbatim(text, Style::new().fg(CODE), &mut inner);
            }
            BlockKind::Math(src) => {
                let style = Style::new().fg(MATH).add_modifier(Modifier::ITALIC);
                self.verbatim(src, style, &mut g.nest(Span::raw("  ")));
            }
            BlockKind::Table { header, rows } => self.table(header, rows, g),
            BlockKind::Rule => {
                let rule = "─".repeat(self.width.saturating_sub(g.width()));
                self.emit(g, vec![(rule, DIM.into(), None)]);
            }
            BlockKind::FootnoteDef { label, blocks } => {
                let marker = Span::styled(format!("[^{label}] "), MUTED);
                self.blocks(blocks, &mut g.nest(marker), base, false);
            }
            BlockKind::Embed { target } => {
                let style = Style::new().fg(ACCENT_SOFT).add_modifier(Modifier::ITALIC);
                let toks = self.tokens(
                    &[Inline::Embed {
                        target: target.clone(),
                    }],
                    style,
                );
                self.wrap(toks, g);
            }
        }
    }

    fn tokens(&mut self, inlines: &[Inline], style: Style) -> Vec<Tok> {
        let mut out = Vec::new();
        self.active.clear();
        self.flatten(inlines, style, None, &mut out);
        self.active.clear();
        out
    }

    /// The mark's look, unless it couldn't be placed.
    fn mark(&self, id: usize) -> Option<&'_ looks::Mark> {
        if self.broken.get(id).copied().unwrap_or(true) {
            return None;
        }
        self.looks.marks.get(id)
    }

    /// `style` and `link` with the looks of the marks around them on.
    fn marked(&self, style: Style, link: Option<usize>) -> (Style, Option<usize>) {
        let mut style = style;
        let mut link = link;
        for &(id, made) in &self.active {
            let Some(mark) = self.mark(id) else { continue };
            if let Some(made) = made {
                let colour = if (self.resolved)(&mark.value) {
                    LINK
                } else {
                    GHOST
                };
                style = style.fg(colour).add_modifier(Modifier::UNDERLINED);
                link = link.or(Some(made));
            }
            style = looks::apply(style, &mark.look);
        }
        (style, link)
    }

    fn hiding(&self) -> bool {
        self.active
            .iter()
            .any(|&(id, _)| self.mark(id).is_some_and(|m| m.look.hide))
    }

    /// A mark's start or end: a badge's padding, its icon, and the value in place of hidden
    /// text.
    fn edge(&mut self, id: usize, open: bool, style: Style, out: &mut Vec<Tok>) {
        let Some(mark) = self.mark(id).cloned() else {
            return;
        };
        let look = &mark.look;
        if open {
            let made = look.link.then(|| {
                self.page.links.push(Link {
                    target: mark.value.clone(),
                    kind: LinkKind::Wiki,
                    pieces: Vec::new(),
                });
                self.page.links.len() - 1
            });
            self.active.push((id, made));
            let (style, link) = self.marked(style, None);
            if look.badge {
                out.push(Tok::Word((" ".into(), style, link)));
            }
            if let Some(icon) = &look.icon {
                out.push(Tok::Word((format!("{icon} "), style, link)));
            }
            if look.hide && (look.badge || look.icon.is_some()) {
                words(&mark.value, style, link, out);
            }
        } else if let Some(at) = self.active.iter().rposition(|a| a.0 == id) {
            let (style, link) = self.marked(style, None);
            if look.badge {
                out.push(Tok::Word((" ".into(), style, link)));
            }
            self.active.remove(at);
        }
    }

    fn flatten(
        &mut self,
        inlines: &[Inline],
        style: Style,
        link: Option<usize>,
        out: &mut Vec<Tok>,
    ) {
        let (base, outer) = (style, link);
        for inline in inlines {
            if let Inline::Mark { id, open } = inline {
                self.edge(*id, *open, base, out);
                continue;
            }
            if self.hiding() && !matches!(inline, Inline::BlockId(_)) {
                continue;
            }
            let (style, link) = self.marked(base, outer);
            let before = out.len();
            match inline {
                Inline::Mark { .. } => {}
                Inline::Text(s) => words(s, style, link, out),
                Inline::Strong(c) => self.flatten(c, style.add_modifier(Modifier::BOLD), link, out),
                Inline::Emph(c) => self.flatten(c, style.add_modifier(Modifier::ITALIC), link, out),
                Inline::Strike(c) => {
                    self.flatten(c, style.add_modifier(Modifier::CROSSED_OUT), link, out)
                }
                Inline::Highlight(c) => self.flatten(c, style.bg(HIGHLIGHT_BG), link, out),
                Inline::Code(s) => words(s, style.fg(CODE).bg(CODE_BG), link, out),
                Inline::Math(s) => {
                    words(s, style.fg(MATH).add_modifier(Modifier::ITALIC), link, out)
                }
                Inline::Tag(t) => out.push(Tok::Word((format!("#{t}"), style.fg(TAG), link))),
                Inline::FootnoteRef(l) => {
                    out.push(Tok::Word((format!("[^{l}]"), style.fg(MUTED), link)))
                }
                Inline::Embed { target } => {
                    words(&embed_label(target), style.fg(ACCENT_SOFT), link, out)
                }
                Inline::BlockId(id) => {
                    let at = self.page.lines.len();
                    self.page.block_ids.push((id.clone(), at));
                }
                Inline::SoftBreak => out.push(Tok::Space(style, link)),
                Inline::HardBreak => out.push(Tok::Break),
                Inline::Link { target, kind, text } => {
                    let colour = match kind {
                        LinkKind::Url => LINK,
                        _ if (self.resolved)(target) => LINK,
                        _ => GHOST,
                    };
                    let id = self.page.links.len();
                    self.page.links.push(Link {
                        target: target.clone(),
                        kind: *kind,
                        pieces: Vec::new(),
                    });
                    let style = style.fg(colour).add_modifier(Modifier::UNDERLINED);
                    if text.is_empty() {
                        words(target, style, Some(id), out);
                    } else {
                        self.flatten(text, style, Some(id), out);
                    }
                }
            }
            // A chip stays on one line.
            if self.in_chip() {
                for tok in &mut out[before..] {
                    if let Tok::Space(style, link) = *tok {
                        *tok = Tok::Word((" ".into(), style, link));
                    }
                }
            }
        }
    }

    fn in_chip(&self) -> bool {
        self.active
            .iter()
            .any(|&(id, _)| self.mark(id).is_some_and(|m| m.look.badge))
    }

    /// Fills lines word by word. Words written together (`[[link]]s`, `**a**b`) stay together;
    /// one wider than the line is cut.
    fn wrap(&mut self, toks: Vec<Tok>, g: &mut Gutter) {
        let room = self.width.saturating_sub(g.width()).max(1);
        let mut line: Vec<Seg> = Vec::new();
        let mut used = 0;
        let mut space = None;
        let mut toks = toks.into_iter().peekable();
        while let Some(tok) = toks.next() {
            let first = match tok {
                Tok::Break => {
                    self.emit(g, std::mem::take(&mut line));
                    (used, space) = (0, None);
                    continue;
                }
                Tok::Space(style, link) => {
                    if used > 0 {
                        space = Some((style, link));
                    }
                    continue;
                }
                Tok::Word(seg) => seg,
            };
            let mut group = vec![first];
            while let Some(Tok::Word(_)) = toks.peek() {
                if let Some(Tok::Word(seg)) = toks.next() {
                    group.push(seg);
                }
            }
            let group_width: usize = group.iter().map(|s| width(&s.0)).sum();
            if let Some((style, link)) = space.take() {
                if used + 1 + group_width > room {
                    self.emit(g, std::mem::take(&mut line));
                    used = 0;
                } else {
                    line.push((" ".into(), style, link));
                    used += 1;
                }
            }
            for (mut word, style, link) in group {
                while used + width(&word) > room {
                    let mut fit = word.len();
                    while fit > 0 && used + width(&word[..fit]) > room {
                        fit = word[..fit].char_indices().last().map_or(0, |(i, _)| i);
                    }
                    if fit == 0 && used == 0 {
                        fit = word.chars().next().map_or(word.len(), char::len_utf8);
                    }
                    let rest = word.split_off(fit);
                    if !word.is_empty() {
                        line.push((word, style, link));
                    }
                    self.emit(g, std::mem::take(&mut line));
                    (word, used) = (rest, 0);
                }
                used += width(&word);
                line.push((word, style, link));
            }
        }
        if !line.is_empty() {
            self.emit(g, line);
        }
    }

    /// Text shown as written (code, math), one line per line, cut where wider than the pane.
    fn verbatim(&mut self, text: &str, style: Style, g: &mut Gutter) {
        let room = self.width.saturating_sub(g.width()).max(1);
        for raw in text.trim_end_matches('\n').split('\n') {
            let mut line = String::new();
            for c in raw.replace('\t', "    ").chars() {
                if width(&line) + width(c.encode_utf8(&mut [0; 4])) > room {
                    self.emit(g, vec![(std::mem::take(&mut line), style, None)]);
                }
                line.push(c);
            }
            self.emit(g, vec![(line, style, None)]);
        }
    }

    /// Columns as wide as their widest cell, the widest ones narrowed until the table fits.
    fn table(&mut self, header: &[Vec<Inline>], rows: &[Vec<Vec<Inline>>], g: &mut Gutter) {
        let head = Style::new().fg(NOTE).add_modifier(Modifier::BOLD);
        let mut cells: Vec<Vec<Vec<Tok>>> =
            vec![header.iter().map(|c| self.tokens(c, head)).collect()];
        for row in rows {
            cells.push(row.iter().map(|c| self.tokens(c, NOTE.into())).collect());
        }
        let columns = cells.iter().map(Vec::len).max().unwrap_or(0);
        let mut widths = vec![0; columns];
        for row in &cells {
            for (w, cell) in widths.iter_mut().zip(row) {
                *w = (*w).max(cell_width(cell));
            }
        }
        let room = self.width.saturating_sub(g.width());
        let sep = 3 * columns.saturating_sub(1);
        while widths.iter().sum::<usize>() + sep > room {
            let Some(widest) = widths.iter_mut().filter(|w| **w > 3).max() else {
                break;
            };
            *widest -= 1;
        }
        for (r, row) in cells.into_iter().enumerate() {
            let mut segs = Vec::new();
            let mut row = row.into_iter();
            for (c, w) in widths.iter().enumerate() {
                if c > 0 {
                    segs.push((" │ ".into(), DIM.into(), None));
                }
                segs.extend(cell(row.next().unwrap_or_default(), *w));
            }
            self.emit(g, segs);
            if r == 0 {
                let rule: Vec<String> = widths.iter().map(|w| "─".repeat(*w)).collect();
                self.emit(g, vec![(rule.join("─┼─"), DIM.into(), None)]);
            }
        }
    }

    /// Adds a line after the gutter, recording where its links are.
    fn emit(&mut self, g: &mut Gutter, segs: Vec<Seg>) {
        let at = self.page.lines.len();
        let mut spans = g.take();
        let mut col: usize = spans.iter().map(Span::width).sum();
        for (text, style, link) in segs {
            let w = width(&text);
            if let Some(id) = link {
                let pieces = &mut self.page.links[id].pieces;
                match pieces.last_mut() {
                    Some((line, cols)) if *line == at && cols.end == col => cols.end += w,
                    _ => pieces.push((at, col..col + w)),
                }
            }
            col += w;
            match spans.last_mut() {
                Some(last) if last.style == style => last.content.to_mut().push_str(&text),
                _ => spans.push(Span::styled(text, style)),
            }
        }
        self.page.lines.push(Line::from(spans));
    }
}

/// The text of inlines as the reader shows them, without any look: a heading's name.
fn plain(inlines: &[Inline], out: &mut String) {
    for inline in inlines {
        match inline {
            Inline::Text(s) | Inline::Code(s) | Inline::Math(s) => {
                out.extend(s.chars().map(|c| if c.is_whitespace() { ' ' } else { c }))
            }
            Inline::Strong(c) | Inline::Emph(c) | Inline::Strike(c) | Inline::Highlight(c) => {
                plain(c, out)
            }
            Inline::Link { target, text, .. } if text.is_empty() => out.push_str(target),
            Inline::Link { text, .. } => plain(text, out),
            Inline::Tag(t) => {
                out.push('#');
                out.push_str(t);
            }
            Inline::FootnoteRef(l) => out.push_str(&format!("[^{l}]")),
            Inline::Embed { target } => out.push_str(&embed_label(target)),
            Inline::SoftBreak | Inline::HardBreak => out.push(' '),
            Inline::BlockId(_) | Inline::Mark { .. } => {}
        }
    }
}

fn words(text: &str, style: Style, link: Option<usize>, out: &mut Vec<Tok>) {
    let mut word = String::new();
    for c in text.chars() {
        if c.is_whitespace() {
            if !word.is_empty() {
                out.push(Tok::Word((std::mem::take(&mut word), style, link)));
            }
            out.push(Tok::Space(style, link));
        } else {
            word.push(c);
        }
    }
    if !word.is_empty() {
        out.push(Tok::Word((word, style, link)));
    }
}

fn cell_width(toks: &[Tok]) -> usize {
    toks.iter()
        .map(|t| match t {
            Tok::Word((w, ..)) => width(w),
            Tok::Space(..) => 1,
            Tok::Break => 0,
        })
        .sum()
}

/// One table cell on one line, cut with `…` or padded to `w` columns.
fn cell(toks: Vec<Tok>, w: usize) -> Vec<Seg> {
    let full = cell_width(&toks) <= w;
    let room = if full { w } else { w.saturating_sub(1) };
    let mut segs = Vec::new();
    let mut used = 0;
    for tok in toks {
        let (text, style, link) = match tok {
            Tok::Word(seg) => seg,
            Tok::Space(style, link) => (" ".into(), style, link),
            Tok::Break => continue,
        };
        let mut fit = String::new();
        for c in text.chars() {
            if used + width(&fit) + width(c.encode_utf8(&mut [0; 4])) > room {
                break;
            }
            fit.push(c);
        }
        used += width(&fit);
        let cut = fit.len() < text.len();
        if !fit.is_empty() {
            segs.push((fit, style, link));
        }
        if cut {
            break;
        }
    }
    if !full {
        segs.push(("…".into(), MUTED.into(), None));
        used += 1;
    }
    segs.push((" ".repeat(w.saturating_sub(used)), Style::new(), None));
    segs
}

fn embed_label(target: &str) -> String {
    let name = target.split('|').next().unwrap_or(target);
    let ext = name.rsplit_once('.').map(|(_, e)| e.to_lowercase());
    let image = matches!(
        ext.as_deref(),
        Some("png" | "jpg" | "jpeg" | "gif" | "webp" | "svg" | "bmp" | "avif")
    );
    format!("{} {name}", if image { "🖼" } else { "⧉" })
}

/// Obsidian's callout colours, by kind and its aliases.
fn callout_colour(kind: &str) -> Color {
    match kind {
        "abstract" | "summary" | "tldr" | "tip" | "hint" | "important" => CYAN,
        "success" | "check" | "done" => GREEN,
        "question" | "help" | "faq" | "warning" | "caution" | "attention" => HIT,
        "failure" | "fail" | "missing" | "danger" | "error" | "bug" => RED,
        "example" => ACCENT,
        "quote" | "cite" => MUTED,
        _ => LINK,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::markdown::{Flavour, parse};

    fn reader(text: &str, width: u16, height: u16) -> Reader {
        let doc = parse(text, &Flavour::built_in("obsidian").unwrap());
        let mut r = Reader::new(doc, Looks::default());
        r.layout(width, &|t| t.starts_with("Shield"));
        r.render(
            Rect::new(0, 0, width, height),
            &mut Buffer::empty(Rect::new(0, 0, width, height)),
        );
        r
    }

    fn lines(r: &Reader) -> Vec<String> {
        let page = r.page.as_ref().unwrap();
        page.lines
            .iter()
            .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
            .collect()
    }

    /// The text under each piece of each link.
    fn link_texts(r: &Reader) -> Vec<Vec<String>> {
        let lines = lines(r);
        r.links()
            .iter()
            .map(|l| {
                l.pieces
                    .iter()
                    .map(|(at, cols)| {
                        lines[*at]
                            .chars()
                            .skip(cols.start)
                            .take(cols.len())
                            .collect()
                    })
                    .collect()
            })
            .collect()
    }

    fn style_at(r: &Reader, line: usize, col: usize) -> Style {
        let mut x = 0;
        for span in &r.page.as_ref().unwrap().lines[line].spans {
            x += span.width();
            if col < x {
                return span.style;
            }
        }
        panic!("nothing at {line}:{col}");
    }

    #[test]
    fn wraps_words_and_keeps_link_positions() {
        let r = reader(
            "Go to [[Shield Volcano]] now and [[Hotspot|the hotspot]] here.",
            16,
            10,
        );
        assert_eq!(
            lines(&r),
            ["Go to Shield", "Volcano now and", "the hotspot", "here."]
        );
        let pieces: Vec<_> = r.links().iter().map(|l| l.pieces.clone()).collect();
        assert_eq!(pieces, [vec![(0, 6..12), (1, 0..7)], vec![(2, 0..11)]]);
        assert_eq!(
            link_texts(&r),
            [vec!["Shield", "Volcano"], vec!["the hotspot"]]
        );
    }

    #[test]
    fn links_in_nested_blocks_point_at_their_text() {
        let r = reader(
            "> - a quoted item with [[Shield Volcano]]\n\n| a | b |\n|---|---|\n| x | [[Lava]] |\n",
            24,
            10,
        );
        assert_eq!(
            link_texts(&r),
            [vec!["Shield Volcano"], vec!["Lava"]],
            "{:#?}",
            lines(&r)
        );
    }

    #[test]
    fn next_link_cycles_in_order_and_wraps() {
        let mut r = reader("[[One]] [[Two]]\n\n[[Three]]", 40, 10);
        let mut seen = Vec::new();
        for _ in 0..4 {
            r.next_link();
            seen.push(r.selected_link().unwrap().target.clone());
        }
        assert_eq!(seen, ["One", "Two", "Three", "One"]);
        r.previous_link();
        assert_eq!(r.selected_link().unwrap().target, "Three");
    }

    #[test]
    fn next_link_scrolls_the_selection_into_view() {
        let text = format!("[[Top]]\n\n{}[[Bottom]]", "para\n\n".repeat(20));
        let mut r = reader(&text, 40, 5);
        r.next_link();
        r.next_link();
        assert_eq!(r.selected_link().unwrap().target, "Bottom");
        let (line, _) = r.selected_link().unwrap().pieces[0].clone();
        assert!(
            line >= r.scroll && line < r.scroll + 5,
            "line {line} at scroll {}",
            r.scroll
        );
    }

    #[test]
    fn resolved_and_unresolved_links_look_different() {
        let r = reader("[[Shield Volcano]] and [[Nowhere]]", 60, 5);
        let at = |i: usize| {
            let (line, cols) = r.links()[i].pieces[0].clone();
            style_at(&r, line, cols.start)
        };
        assert_ne!(at(0).fg, at(1).fg);
        assert!(at(0).add_modifier.contains(Modifier::UNDERLINED));
    }

    #[test]
    fn looks_draw_chips_hide_link_and_strike() {
        use crate::types::Look;
        let text = "Due 2026-11-12 and Lava here.\n";
        let mark = |look: Look, value: &str| looks::Mark {
            look,
            field: "f".into(),
            value: value.into(),
        };
        let marks = vec![
            mark(
                Look {
                    hide: true,
                    badge: true,
                    icon: Some("📅".into()),
                    ..Look::default()
                },
                "soon",
            ),
            mark(
                Look {
                    link: true,
                    ..Look::default()
                },
                "Lava",
            ),
            mark(
                Look {
                    strike: true,
                    ..Look::default()
                },
                "here",
            ),
        ];
        let flavour = Flavour::built_in("obsidian").unwrap();
        let doc = crate::markdown::parse_marked(text, &flavour, &[4..14, 19..23, 24..28]);
        let looks = Looks {
            marks,
            header: Vec::new(),
        };
        let mut r = Reader::new(doc, looks);
        r.layout(60, &|_| true);
        let line = &lines(&r)[0];
        assert_eq!(line, "Due  📅 soon  and Lava here.");
        let chip = style_at(&r, 0, 5);
        assert!(chip.bg.is_some() || chip.add_modifier.contains(Modifier::REVERSED));
        assert_eq!(r.links()[0].target, "Lava");
        // "Due ", the chip " 📅 soon " (the icon two cells wide), " and ".
        assert_eq!(r.links()[0].pieces, [(0, 18..22)]);
        assert!(
            style_at(&r, 0, 23)
                .add_modifier
                .contains(Modifier::CROSSED_OUT)
        );
    }

    #[test]
    fn a_broken_mark_draws_nothing() {
        let flavour = Flavour::built_in("obsidian").unwrap();
        let doc = crate::markdown::parse_marked(
            "see #volcano\n",
            &flavour,
            std::slice::from_ref(&(4..8)),
        );
        let looks = Looks {
            marks: vec![looks::Mark {
                look: crate::types::Look {
                    badge: true,
                    icon: Some("▲".into()),
                    ..Default::default()
                },
                field: "f".into(),
                value: "v".into(),
            }],
            header: Vec::new(),
        };
        let mut r = Reader::new(doc, looks);
        r.layout(60, &|_| true);
        assert_eq!(lines(&r), ["see #volcano"]);
    }

    #[test]
    fn folded_callout_shows_only_its_header() {
        let r = reader("> [!note]- Folded title\n> hidden body\n", 40, 5);
        assert_eq!(lines(&r), ["│ ▸ note: Folded title"]);
        let r = reader("> [!tip] Open\n> shown body\n", 40, 5);
        assert_eq!(lines(&r), ["│ tip: Open", "│ shown body"]);
    }

    fn section(r: &Reader) -> Vec<&str> {
        let headings = r.headings();
        r.section()
            .into_iter()
            .map(|i| headings[i].text.as_str())
            .collect()
    }

    #[test]
    fn section_is_the_heading_path_of_the_top_line() {
        let paras = "para\n\n".repeat(10);
        let text = format!(
            "# Top\n\n{paras}## One\n\n{paras}### Deep\n\n{paras}## Two\n\n{paras}Setext\n------\n\n{paras}"
        );
        let mut r = reader(&text, 40, 5);
        assert_eq!(section(&r), ["Top"]);
        let levels: Vec<u8> = r.headings().iter().map(|h| h.level).collect();
        assert_eq!(levels, [1, 2, 3, 2, 2]);
        r.jump(Jump::Heading("deep".into()));
        assert_eq!(section(&r), ["Top", "One", "Deep"]);
        r.scroll_by(3);
        assert_eq!(section(&r), ["Top", "One", "Deep"], "still in the section");
        r.next_heading();
        assert_eq!(section(&r), ["Top", "Two"]);
        r.next_heading();
        assert_eq!(section(&r), ["Top", "Setext"]);
        r.scroll_by(2);
        r.previous_heading();
        assert_eq!(
            section(&r),
            ["Top", "Setext"],
            "first to this section's heading"
        );
        r.previous_heading();
        assert_eq!(section(&r), ["Top", "Two"]);
    }

    #[test]
    fn a_jump_near_the_end_reads_its_heading_until_scrolled() {
        let mut r = reader("# A\n\ntext\n\n## B\n\ntext\n\n## C\n\ntext", 40, 20);
        r.next_heading();
        r.next_heading();
        assert_eq!(r.scroll, 0, "everything fits, nothing scrolls");
        assert_eq!(section(&r), ["A", "C"]);
        let spot = r.spot();
        r.top();
        assert_eq!(section(&r), ["A"]);
        r.restore(spot);
        assert_eq!(section(&r), ["A", "C"]);
        r.previous_heading();
        assert_eq!(section(&r), ["A", "B"]);
    }

    #[test]
    fn folded_callouts_hide_their_headings() {
        let r = reader(
            "# A\n\n> [!note]- Folded\n> ## Hidden\n\n> [!tip] Open\n> ## Shown\n",
            40,
            10,
        );
        let texts: Vec<&str> = r.headings().iter().map(|h| h.text.as_str()).collect();
        assert_eq!(texts, ["A", "Shown"]);
    }

    #[test]
    fn jumps_to_a_heading() {
        let paras = "para\n\n".repeat(20);
        let text = format!("# Top\n\n{paras}## Far Down\n\n{paras}## Last\n\ntext");
        let mut r = reader(&text, 40, 5);
        r.jump(Jump::Heading("far down".into()));
        assert_eq!(lines(&r)[r.scroll], "## Far Down");
        // Near the end the page can't scroll further, so the heading is shown lower down.
        r.jump(Jump::Heading("last".into()));
        let shown = &lines(&r)[r.scroll..r.scroll + 5];
        assert!(shown.iter().any(|l| l == "## Last"), "{shown:?}");
    }

    #[test]
    fn x_picks_the_task_being_read_or_the_first_one_below() {
        let text =
            "intro\n\n- [ ] one\n- [x] two\n    - [ ] two.a [[Shield]]\n- plain\n- [ ] three\n";
        let mut r = reader(text, 40, 20);
        assert_eq!(
            r.task_line(),
            Some(3),
            "nothing read yet: the first on screen"
        );
        r.scroll_by(3);
        assert_eq!(lines(&r)[3], "☑ two");
        assert_eq!(
            r.task_line(),
            Some(4),
            "the short note can't scroll: the cursor moves"
        );
        r.scroll_by(1);
        assert_eq!(r.task_line(), Some(5), "the nested task, not its parent");
        r.scroll_by(1);
        assert_eq!(r.task_line(), Some(7), "a plain item: the next task below");
        r.scroll_by(-5);
        assert_eq!(r.reading, None, "back at the top line");
        r.next_link();
        assert_eq!(r.task_line(), Some(5), "the task the selected link is in");
        let mut r = reader("no tasks\n", 40, 20);
        assert_eq!(r.task_line(), None);
        r.scroll_by(1);
        assert_eq!(r.reading, None, "one line: nowhere to move");
    }
}
