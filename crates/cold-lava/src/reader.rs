//! The reader: a note's `Doc` laid out as styled lines for the pane's width, with the place of
//! every link kept, so keys and clicks can select and follow them.

use std::ops::Range;

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

use crate::app::{ACCENT, ACCENT_SOFT, DIM, GHOST, HIT, MUTED, NOTE, TAG};
use crate::core::Jump;
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
    headings: Vec<(String, usize)>,
    block_ids: Vec<(String, usize)>,
}

/// Where the reader is in a note, to come back to it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Spot {
    scroll: usize,
    selected: Option<usize>,
}

pub struct Reader {
    doc: Doc,
    page: Option<Page>,
    scroll: usize,
    /// Rows the pane had when last drawn.
    height: usize,
    selected: Option<usize>,
    /// Applied once the doc is laid out.
    pending_jump: Option<Jump>,
}

impl Reader {
    pub fn new(doc: Doc) -> Reader {
        Reader {
            doc,
            page: None,
            scroll: 0,
            height: 0,
            selected: None,
            pending_jump: None,
        }
    }

    /// The same note, read again: keeps the scroll position.
    pub fn set_doc(&mut self, doc: Doc) {
        self.doc = doc;
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
        };
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
        self.clamp();
        let Some(page) = &self.page else { return };
        let rows = page.lines.iter().skip(self.scroll).take(self.height);
        for (y, line) in (area.y..).zip(rows) {
            buf.set_line(area.x, y, line, area.width);
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

    fn max_scroll(&self) -> usize {
        let lines = self.page.as_ref().map_or(0, |p| p.lines.len());
        lines.saturating_sub(self.height)
    }

    fn clamp(&mut self) {
        self.scroll = self.scroll.min(self.max_scroll());
    }

    pub fn scroll_by(&mut self, n: isize) {
        self.scroll = self.scroll.saturating_add_signed(n);
        self.clamp();
    }

    /// A page down (`1`) or up (`-1`), keeping two lines of context.
    pub fn page(&mut self, direction: isize) {
        let step = self.height.saturating_sub(2).max(1) as isize;
        self.scroll_by(step * direction);
    }

    pub fn top(&mut self) {
        self.scroll = 0;
    }

    pub fn bottom(&mut self) {
        self.scroll = self.max_scroll();
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
        } else if last >= self.scroll + self.height {
            self.scroll = (last + 1).saturating_sub(self.height);
        }
        self.clamp();
    }

    pub fn spot(&self) -> Spot {
        Spot {
            scroll: self.scroll,
            selected: self.selected,
        }
    }

    /// Back to a `spot` of this note; kept within the note once it is laid out.
    pub fn restore(&mut self, spot: Spot) {
        self.scroll = spot.scroll;
        self.selected = spot.selected;
        self.pending_jump = None;
    }

    pub fn selected_link(&self) -> Option<&Link> {
        self.links().get(self.selected?)
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
        let Some(page) = &self.page else {
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
                let key = |(text, _): &&(String, usize)| crate::vault::search_key(text);
                let headings = || page.headings.iter();
                headings()
                    .find(|x| key(x) == h)
                    .or_else(|| headings().find(|x| key(x).starts_with(&h)))
                    .map(|(_, at)| *at)
            }
            Jump::Block(id) => page
                .block_ids
                .iter()
                .find(|(b, _)| b == id)
                .map(|(_, at)| *at),
        };
        if let Some(at) = found {
            self.scroll = at;
            self.clamp();
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
}

impl Builder<'_> {
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
                let text = toks
                    .iter()
                    .map(|t| match t {
                        Tok::Word((w, ..)) => w.as_str(),
                        _ => " ",
                    })
                    .collect::<String>();
                self.page.headings.push((text, at));
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
                    self.blocks(&item.blocks, &mut g.nest(marker), style, false);
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
        self.flatten(inlines, style, None, &mut out);
        out
    }

    fn flatten(
        &mut self,
        inlines: &[Inline],
        style: Style,
        link: Option<usize>,
        out: &mut Vec<Tok>,
    ) {
        for inline in inlines {
            match inline {
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
        }
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
        let mut r = Reader::new(doc);
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
    fn folded_callout_shows_only_its_header() {
        let r = reader("> [!note]- Folded title\n> hidden body\n", 40, 5);
        assert_eq!(lines(&r), ["│ ▸ note: Folded title"]);
        let r = reader("> [!tip] Open\n> shown body\n", 40, 5);
        assert_eq!(lines(&r), ["│ tip: Open", "│ shown body"]);
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
}
