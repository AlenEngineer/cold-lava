//! The reader's `Doc` as pulldown-cmark read it before the syntax tree. The parse stays here
//! to show the tree's `Doc` is the one it gave, on every note of the corpus.

use std::iter::Peekable;

use pulldown_cmark::{
    BlockQuoteKind, CodeBlockKind, Event, LinkType, OffsetIter, Options, Parser, Tag,
};

use super::*;

/// The pulldown-cmark option behind each syntax (none for the ones its reader added itself).
fn option(syntax: Syntax) -> Options {
    match syntax {
        S::Tables => Options::ENABLE_TABLES,
        S::TaskLists => Options::ENABLE_TASKLISTS,
        S::Strikethrough => Options::ENABLE_STRIKETHROUGH,
        S::Footnotes => Options::ENABLE_FOOTNOTES,
        S::Alerts => Options::ENABLE_GFM,
        S::WikiLinks | S::Embeds => Options::ENABLE_WIKILINKS,
        S::Math => Options::ENABLE_MATH,
        S::Frontmatter => Options::ENABLE_YAML_STYLE_METADATA_BLOCKS,
        S::Callouts | S::Highlights | S::Comments | S::Tags | S::BlockIds => Options::empty(),
    }
}

/// pulldown-cmark's options for `flavour`.
pub(crate) fn options(flavour: &Flavour) -> Options {
    let syntax = flavour.syntax.iter();
    syntax.fold(Options::empty(), |o, s| o | option(*s))
}

/// The text pulldown-cmark read: `Flavour::source_with`, the code found by pulldown-cmark.
pub(crate) fn source<'t>(flavour: &Flavour, text: &'t str) -> Source<'t> {
    flavour.source_with(text, |text| {
        Parser::new_ext(text, options(flavour))
            .into_offset_iter()
            .filter(|(e, _)| {
                matches!(
                    e,
                    Event::Code(_)
                        | Event::InlineMath(_)
                        | Event::DisplayMath(_)
                        | Event::Start(Tag::CodeBlock(_))
                )
            })
            .map(|(_, r)| r)
            .collect()
    })
}

/// The `Doc` of `text` as pulldown-cmark read it.
pub(crate) fn parse(text: &str, flavour: &Flavour) -> Doc {
    parse_marked(text, flavour, &[])
}

pub(crate) fn parse_marked(text: &str, flavour: &Flavour, marks: &[Range<usize>]) -> Doc {
    let source = source(flavour, text);
    let marks = Marks::new(&source, marks, flavour.has(S::Tags));
    let text = source.text;
    let mut reader = Reader {
        src: &text,
        flavour,
        events: Parser::new_ext(&text, options(flavour))
            .into_offset_iter()
            .peekable(),
        newlines: text.match_indices('\n').map(|(i, _)| i).collect(),
        tasks: Vec::new(),
        marks,
    };
    Doc {
        blocks: reader.blocks(),
        broken: reader.marks.broken,
    }
}

/// Builds the `Doc` from pulldown-cmark's events by recursive descent: each container reads
/// its children up to its `End` event.
struct Reader<'a> {
    src: &'a str,
    flavour: &'a Flavour,
    events: Peekable<OffsetIter<'a>>,
    newlines: Vec<usize>,
    /// One slot per open list item, for its task marker. Nested items open after the outer
    /// item's marker, so the marker always belongs to the last slot.
    tasks: Vec<Option<bool>>,
    marks: Marks,
}

fn is_inline(tag: &Tag) -> bool {
    matches!(
        tag,
        Tag::Emphasis
            | Tag::Strong
            | Tag::Strikethrough
            | Tag::Superscript
            | Tag::Subscript
            | Tag::Link { .. }
            | Tag::Image { .. }
    )
}

impl Reader<'_> {
    fn line(&self, offset: usize) -> usize {
        self.newlines.partition_point(|&n| n < offset) + 1
    }

    /// Blocks up to the enclosing container's end (consumed) or the end of the note.
    fn blocks(&mut self) -> Vec<Block> {
        let mut blocks = Vec::new();
        while let Some((event, range)) = self.events.peek() {
            let start = range.start;
            match event {
                Event::End(_) => {
                    self.events.next();
                    break;
                }
                Event::Start(tag) if !is_inline(tag) => {
                    let Some((Event::Start(tag), range)) = self.events.next() else {
                        unreachable!()
                    };
                    self.block(tag, range, &mut blocks);
                }
                Event::Rule => {
                    self.events.next();
                    blocks.push(Block {
                        line: self.line(start),
                        kind: BlockKind::Rule,
                    });
                }
                // A tight list item holds its text without a paragraph.
                _ => {
                    let inlines = self.inlines(false);
                    blocks.extend(self.paragraph(inlines, start));
                }
            }
        }
        blocks
    }

    fn block(&mut self, tag: Tag, range: Range<usize>, out: &mut Vec<Block>) {
        let line = self.line(range.start);
        let kind = match tag {
            Tag::Paragraph => {
                let inlines = self.inlines(true);
                out.extend(self.paragraph(inlines, range.start));
                return;
            }
            Tag::Heading { level, .. } => BlockKind::Heading {
                level: level as u8,
                inlines: self.inlines(true),
            },
            Tag::BlockQuote(Some(alert)) => BlockKind::Callout {
                kind: alert_kind(alert).to_string(),
                title: Vec::new(),
                folded: None,
                blocks: self.blocks(),
            },
            Tag::BlockQuote(None) => {
                let blocks = self.blocks();
                if self.flavour.has(S::Callouts) {
                    callout(blocks)
                } else {
                    BlockKind::Quote(blocks)
                }
            }
            Tag::CodeBlock(kind) => BlockKind::Code {
                lang: match kind {
                    CodeBlockKind::Fenced(info) => {
                        info.split_whitespace().next().map(str::to_string)
                    }
                    CodeBlockKind::Indented => None,
                },
                text: self.text(),
            },
            Tag::HtmlBlock => BlockKind::Code {
                lang: Some("html".to_string()),
                text: self.text(),
            },
            Tag::List(start) => BlockKind::List {
                start,
                items: self.items(),
            },
            Tag::FootnoteDefinition(label) => BlockKind::FootnoteDef {
                label: label.to_string(),
                blocks: self.blocks(),
            },
            Tag::Table(_) => self.table(),
            Tag::MetadataBlock(_) => {
                self.text();
                return;
            }
            // Containers of options no flavour turns on (definition lists): keep their content.
            _ => {
                out.extend(self.blocks());
                return;
            }
        };
        out.push(Block { line, kind });
    }

    fn paragraph(&self, inlines: Vec<Inline>, start: usize) -> Option<Block> {
        let mut inlines = inlines;
        if self.flavour.has(S::BlockIds) {
            block_id(&mut inlines);
        }
        let mut content = inlines
            .iter()
            .filter(|i| !is_blank(i) && !matches!(i, Inline::Mark { .. }));
        let only = match (content.next(), content.next()) {
            (None, _) => return None,
            (Some(only), None) => Some(only),
            _ => None,
        };
        let kind = match only {
            Some(Inline::Embed { target }) => BlockKind::Embed {
                target: target.clone(),
            },
            Some(Inline::Math(m)) if self.src[start..].trim_start().starts_with("$$") => {
                BlockKind::Math(m.clone())
            }
            _ => BlockKind::Paragraph(inlines),
        };
        Some(Block {
            line: self.line(start),
            kind,
        })
    }

    fn items(&mut self) -> Vec<Item> {
        let mut items = Vec::new();
        while let Some((Event::Start(Tag::Item), _)) = self.events.next() {
            self.tasks.push(None);
            let blocks = self.blocks();
            items.push(Item {
                task: self.tasks.pop().flatten(),
                blocks,
            });
        }
        items
    }

    fn table(&mut self) -> BlockKind {
        let mut header = Vec::new();
        let mut rows = Vec::new();
        loop {
            match self.events.next() {
                Some((Event::Start(Tag::TableHead), _)) => header = self.cells(),
                Some((Event::Start(Tag::TableRow), _)) => rows.push(self.cells()),
                _ => break,
            }
        }
        BlockKind::Table { header, rows }
    }

    fn cells(&mut self) -> Vec<Vec<Inline>> {
        let mut cells = Vec::new();
        while let Some((Event::Start(Tag::TableCell), _)) = self.events.next() {
            cells.push(self.inlines(true));
        }
        cells
    }

    /// The raw text up to the container's end.
    fn text(&mut self) -> String {
        let mut text = String::new();
        for (event, _) in self.events.by_ref() {
            match event {
                Event::Text(t) | Event::Html(t) | Event::Code(t) => text.push_str(&t),
                Event::End(_) => break,
                _ => {}
            }
        }
        text
    }

    /// Inlines up to the container's end, which is consumed when `explicit`. Otherwise (a tight
    /// list item's text) reading stops before the next block or the item's end.
    fn inlines(&mut self, explicit: bool) -> Vec<Inline> {
        let mut out = Vec::new();
        // Where each of `out` is in the source, and what it is to a mark.
        let mut at: Vec<(Range<usize>, Piece)> = Vec::new();
        loop {
            match self.events.peek() {
                None => break,
                Some((Event::End(_), _)) => {
                    if explicit {
                        self.events.next();
                    }
                    break;
                }
                Some((Event::Start(tag), _)) if !is_inline(tag) => break,
                Some((Event::Rule, _)) => break,
                _ => {}
            }
            let Some((event, range)) = self.events.next() else {
                break;
            };
            let before = out.len();
            let container = matches!(event, Event::Start(_));
            match event {
                Event::Text(t) | Event::Html(t) | Event::InlineHtml(t) => {
                    out.push(Inline::Text(t.into_string()))
                }
                Event::Code(t) => out.push(Inline::Code(t.into_string())),
                Event::InlineMath(t) | Event::DisplayMath(t) => {
                    out.push(Inline::Math(t.into_string()))
                }
                Event::FootnoteReference(label) => {
                    out.push(Inline::FootnoteRef(label.into_string()))
                }
                Event::SoftBreak => out.push(Inline::SoftBreak),
                Event::HardBreak => out.push(Inline::HardBreak),
                Event::TaskListMarker(done) => {
                    if let Some(slot) = self.tasks.last_mut() {
                        *slot = Some(done);
                    }
                }
                Event::Start(tag) => {
                    let outer = self.marks.within.replace(range.clone());
                    self.inline(tag, range.clone(), &mut out);
                    self.marks.within = outer;
                }
                Event::End(_) | Event::Rule => unreachable!("handled above"),
            }
            for inline in &out[before..] {
                let piece = match inline {
                    Inline::Text(t) if self.src.get(range.clone()) == Some(t.as_str()) => {
                        Piece::Written
                    }
                    _ if container => Piece::Container,
                    _ => Piece::Atom,
                };
                at.push((range.clone(), piece));
            }
        }
        let out = self.marks.place(out, at);
        extras(out, self.flavour)
    }

    fn inline(&mut self, tag: Tag, range: Range<usize>, out: &mut Vec<Inline>) {
        let wiki = |t: LinkType| matches!(t, LinkType::WikiLink { .. });
        match tag {
            Tag::Emphasis => out.push(Inline::Emph(self.inlines(true))),
            Tag::Strong => out.push(Inline::Strong(self.inlines(true))),
            Tag::Strikethrough => out.push(Inline::Strike(self.inlines(true))),
            // `ENABLE_WIKILINKS` is on for either syntax, so each one checks its own.
            Tag::Link { link_type, .. } if wiki(link_type) && !self.flavour.has(S::WikiLinks) => {
                self.literal(range, out)
            }
            Tag::Image { link_type, .. } if wiki(link_type) && !self.flavour.has(S::Embeds) => {
                self.literal(range, out)
            }
            Tag::Link {
                link_type,
                dest_url,
                ..
            } => {
                let kind = if wiki(link_type) {
                    LinkKind::Wiki
                } else if matches!(link_type, LinkType::Autolink | LinkType::Email)
                    || has_scheme(&dest_url)
                {
                    LinkKind::Url
                } else {
                    LinkKind::Markdown
                };
                out.push(Inline::Link {
                    target: dest_url.into_string(),
                    kind,
                    text: self.inlines(true),
                });
            }
            Tag::Image { dest_url, .. } => {
                self.inlines(true);
                out.push(Inline::Embed {
                    target: dest_url.into_string(),
                });
            }
            _ => out.extend(self.inlines(true)),
        }
    }

    fn literal(&mut self, range: Range<usize>, out: &mut Vec<Inline>) {
        self.inlines(true);
        out.push(Inline::Text(self.src[range].to_string()));
    }
}

fn alert_kind(kind: BlockQuoteKind) -> &'static str {
    match kind {
        BlockQuoteKind::Note => "note",
        BlockQuoteKind::Tip => "tip",
        BlockQuoteKind::Important => "important",
        BlockQuoteKind::Warning => "warning",
        BlockQuoteKind::Caution => "caution",
    }
}

/// Delimited spans that may wrap other inlines.
type Wrap = fn(Vec<Inline>) -> Inline;

const SPANS: &[(Syntax, &str, Wrap)] = &[(S::Highlights, "==", Inline::Highlight)];

/// The first match in `text` at or after `from`: its byte range and what it becomes.
type Token = fn(text: &str, from: usize) -> Option<(Range<usize>, Inline)>;

/// Words inside text.
const TOKENS: &[(Syntax, Token)] = &[(S::Tags, tag)];

fn extras(inlines: Vec<Inline>, flavour: &Flavour) -> Vec<Inline> {
    let tokens: Vec<Token> = TOKENS
        .iter()
        .filter(|t| flavour.has(t.0))
        .map(|t| t.1)
        .collect();
    let mut out: Vec<Inline> = merge_text(inlines)
        .into_iter()
        .flat_map(|i| match i {
            Inline::Text(t) if !tokens.is_empty() => split_tokens(&t, &tokens),
            i => vec![i],
        })
        .collect();
    for (syntax, delim, wrap) in SPANS {
        if flavour.has(*syntax) {
            out = spans(out, delim, *wrap);
        }
    }
    out
}

fn split_tokens(text: &str, tokens: &[Token]) -> Vec<Inline> {
    let mut out = Vec::new();
    let mut at = 0;
    while let Some((range, inline)) = tokens
        .iter()
        .filter_map(|find| find(text, at))
        .min_by_key(|(r, _)| r.start)
    {
        if range.start > at {
            out.push(Inline::Text(text[at..range.start].to_string()));
        }
        out.push(inline);
        at = range.end;
    }
    if at < text.len() {
        out.push(Inline::Text(text[at..].to_string()));
    }
    out
}

/// Pairs `delim`s left to right into `wrap`ped spans, like `**`: an opener is followed by
/// something other than a space, a closer follows something other than a space. Unpaired
/// delimiters stay text.
fn spans(inlines: Vec<Inline>, delim: &str, wrap: Wrap) -> Vec<Inline> {
    enum Piece {
        Inline(Inline),
        Delim,
    }
    let mut pieces = Vec::new();
    for i in inlines {
        match i {
            Inline::Text(t) if t.contains(delim) => {
                for (k, part) in t.split(delim).enumerate() {
                    if k > 0 {
                        pieces.push(Piece::Delim);
                    }
                    if !part.is_empty() {
                        pieces.push(Piece::Inline(Inline::Text(part.to_string())));
                    }
                }
            }
            i => pieces.push(Piece::Inline(i)),
        }
    }
    let can_open: Vec<bool> = (0..pieces.len())
        .map(|k| match pieces.get(k + 1) {
            Some(Piece::Inline(Inline::Text(t))) => !t.starts_with(char::is_whitespace),
            Some(Piece::Inline(_)) => true,
            _ => false,
        })
        .collect();

    let mut out: Vec<Inline> = Vec::new();
    let mut open: Option<usize> = None;
    for (k, piece) in pieces.into_iter().enumerate() {
        if let Piece::Inline(i) = piece {
            out.push(i);
            continue;
        }
        let can_close = match out.last() {
            Some(Inline::Text(t)) => !t.ends_with(char::is_whitespace),
            Some(_) => true,
            None => false,
        };
        match open {
            Some(at) if can_close && out.len() > at => {
                let inner = out.split_off(at);
                out.pop();
                out.push(wrap(merge_text(inner)));
                open = None;
            }
            _ => {
                out.push(Inline::Text(delim.to_string()));
                if can_open[k] {
                    open = Some(out.len());
                }
            }
        }
    }
    merge_text(out)
}

/// `> [!type]± Title` on the quote's first line makes it a callout.
fn callout(mut blocks: Vec<Block>) -> BlockKind {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| Regex::new(r"^\[!([^\]\s]+)\]([+-]?)[ \t]*").unwrap());
    let Some(Block {
        line,
        kind: BlockKind::Paragraph(inlines),
    }) = blocks.first_mut()
    else {
        return BlockKind::Quote(blocks);
    };
    let Some(Inline::Text(first)) = inlines
        .iter_mut()
        .find(|i| !matches!(i, Inline::Mark { .. }))
    else {
        return BlockKind::Quote(blocks);
    };
    let Some(c) = re.captures(first) else {
        return BlockKind::Quote(blocks);
    };
    let kind = c[1].to_lowercase();
    let folded = match &c[2] {
        "-" => Some(true),
        "+" => Some(false),
        _ => None,
    };
    let marker = c.get(0).unwrap().end();
    first.drain(..marker);
    let line = *line;
    let mut title = std::mem::take(inlines);
    let body = match title
        .iter()
        .position(|i| matches!(i, Inline::SoftBreak | Inline::HardBreak))
    {
        Some(at) => title.split_off(at).split_off(1),
        None => Vec::new(),
    };
    title.retain(|i| !matches!(i, Inline::Text(t) if t.is_empty()));
    if body.iter().all(is_blank) {
        blocks.remove(0);
    } else {
        blocks[0] = Block {
            line: line + 1,
            kind: BlockKind::Paragraph(body),
        };
    }
    BlockKind::Callout {
        kind,
        title,
        folded,
        blocks,
    }
}

mod tests {
    use super::*;

    /// Markdown that is easy to read differently from pulldown-cmark.
    const TRICKY: &[&str] = &[
        // Nested lists with code.
        "- a\n  - b\n\n    ```rust\n    let x = 1;\n    ```\n\n    para in b\n  - c\n\n        indented code in c\n- d\n1. one\n2. two\n   - mixed\n",
        // Lazy lines.
        "> quote\nlazy *line*\n\n- item\nlazy item line\n\nPara\n    indented continuation\n",
        // Tables with alignment and escaped pipes.
        "| Left | Center | Right |\n|:-----|:------:|------:|\n| a \\| b | `c` | **d** |\n| [x](y.md) | | e |\n| short |\n| one | two | three | four |\n\nText\n\n| a |\n|---|\n| 1 |\n",
        // Callouts in quotes.
        "> quote\n>\n> > [!warning]- Nested *callout*\n> > body\n>\n> back\n\n> [!info]\n> No title, **body**\n\n> [!tip]+ Open\n\n> [!NOTE]\n> GitHub alert\n",
        // Footnotes.
        "Text with a note[^1] and another[^long note], and none[^missing].\n\n[^1]: The first *note*.\n[^long note]: The second,\nlazily continued.\n\nAfter.\n",
        // Math.
        "Inline $x^2$ and $$y$$ and a price of $5 and $6.\n\n$$\n\\frac{a}{b}\n$$\n\nText\n$$\nz\n$$\nmore text\n",
        // Comments.
        "Shown %%hidden%% shown.\n\n%%\nA hidden block\nwith **markup**\n%%\n\nAfter `%%code%%` and %%two%% %%comments%%.\n",
        // Embeds.
        "![[image.png]]\n\n![[Note#Heading|alias]] in text ![alt](pic.png \"title\") and ![ref][r]\n\n![alone](a.png)\n\n[r]: ref.png\n",
        // HTML blocks.
        "<div class=\"x\">\n*not emphasis*\n</div>\n\n<!-- a comment\nover lines -->\n\nText with <span>inline</span> html and <br/> a break.\n\n<details>\n<summary>Sum</summary>\n\nInside **details**\n\n</details>\n",
        // Setext headings.
        "Title *one*\n===\n\nTwo lines\nof title\n---\n\nPara\n\n---\n\n## ATX closed ##\n### Not closed#\n#### \n",
        // Hard breaks.
        "one  \ntwo\\\nthree   \nfour\n\nlast with spaces  \n\nlast with backslash\\\n",
        // Entities.
        "&amp; &lt;tag&gt; &copy; &#169; &#xA9; &nbsp;| &notanentity; &#0; AT&T &amp\n",
        // Autolinks.
        "<https://example.com/a_b_c?x=*y*> and <mailto:me@example.com> and <me@example.com> but <not a link> and https://bare.example.com/x_y_z\n",
        // Nested emphasis.
        "***both*** **bold *nested* bold** *em **nested** em* __under *star* under__ ~~strike **bold**~~ ==mark *em*== **a *b* c** *a **b** c*\n",
        // An unclosed run, `*` and `_` inside words, emphasis over a soft break.
        "*unclosed and **also unclosed\n\nsnake_case_word and file_name.md and 2*3*4 and a * b * c\n\n*emphasis over\na soft break* and **strong over\ntwo\nbreaks**\n\n_under_score_ __a__b__ _a_b_ foo*bar*baz foo_bar_baz\n",
        // Links with titles, and `_` in destinations.
        "[a](b.md \"title\") [c](<d e.md> 'title') [f](g.md (title)) [h](i.md#j) [k](https://l.m/n_o_p \"q\") [r](s_t_u.md) [v]() [w](x \"y\\\"z\")\n",
        // Reference links.
        "[text][ref] [Ref][] [ref] [text][missing] ![img][ref]\n\n[ref]: target.md \"Title\"\n[other]: <https://example.com/a b>\n",
        // CRLF.
        "# Title\r\n\r\nPara *one*\r\ntwo  \r\nthree\r\n\r\n- [ ] task\r\n- [x] done\r\n\r\n```\r\ncode\r\n```\r\n\r\n| a | b |\r\n|---|---|\r\n| 1 | 2 |\r\n",
        // Tasks and other statuses.
        "- [ ] open\n- [x] done\n- [X] done too\n- [/] doing\n- [-] dropped\n- [ ]\n- [x]no space\n1. [ ] numbered\n",
        // Code spans.
        "`` code with ` tick `` and ` padded ` and `a\nb` over lines and `unclosed\n",
        // Links in emphasis, tags.
        "**[[Note]]** *[link](x.md)* #tag **#bold** (#paren) `#code` [[Note#Heading]] [[Note|*alias*]]\n",
        // A link's text over lines.
        "A [link whose text\ngoes on](Note.md) and [another *with\nemphasis*](Other.md).\n",
        // Block ids.
        "A claim. ^claim\n\nSecond ^two\nline\n\n- item ^i1\n\n> quote ^q1\n",
        // Blocks in list items and quotes.
        "- # Heading in item\n- ---\n- > quote in item\n\n> # Heading in quote\n> - list in quote\n>   more\n",
        // Indented code.
        "    code line\n\n      indented more\n\n\n    after two blanks\ntext\n",
        // Frontmatter.
        "---\ntitle: x\ntags: [a]\n---\n\n# Body\n",
        // Dataview fields.
        "key:: value with [[Link]]\n\nA [due:: 2026-01-01] and (done:: no) field.\n",
        // Wikilinks.
        "[[Note]] [[Note|alias]] [[Note#H]] [[Note#^b]] [[Folder/Note.md|x]] ![[E]] [[]] [[a]]b[[c]]\n",
    ];

    /// Markdown the tree reads otherwise than pulldown-cmark did, and why. In each, the tree
    /// reads what the index already counts, or pulldown-cmark wasn't Obsidian.
    const DIFFERENT: &[(&str, &str)] = &[
        (
            "\u{feff}# Title\n",
            "A byte order mark made the first line text.",
        ),
        (
            "---\n\nkanban-plugin: basic\n\n---\n\n## To do\n",
            "A frontmatter whose first line is blank was a rule and Markdown.",
        ),
        (
            "See [[#Heading]].\n",
            "`[[#Heading]]` showed the tag `Heading` in the link.",
        ),
        (
            "Read ==#marked==.\n",
            "A tag right after a highlight's `==` was none.",
        ),
        (
            "| [[A\\|B]] | c |\n| --- | --- |\n| d | e |\n",
            "`[[A\\|B]]` in a table cell linked to `A\\`.",
        ),
    ];

    /// A long note of every kind of block.
    fn long_note() -> String {
        let blocks = [
            "## Section *one* #topic\n\n",
            "A paragraph with [[Some Note|an alias]], a [link](other.md \"title\"), `code`, **strong\nover a line** and #tag.  \nIt goes on after a hard break. ^blk\n\n",
            "- [ ] A task [due:: 2026-11-12] 📅 2026-11-12\n  - [x] A sub-task ~~struck~~ ✅ 2026-11-01\n- A plain item with [[Link]] and ==marks==\n\n",
            "> [!note] A callout\n> with a line [[Inside]] and a footnote[^n]\n\n",
            "```rust\nfn main() {\n    println!(\"[[not a link]]\");\n}\n```\n\n",
            "| Name | Value |\n| --- | :-: |\n| [[A]] | *1* |\n| [[B]] | 2 \\| 3 |\n\n",
            "[^n]: The note.\n\n",
        ];
        let mut out = String::new();
        let mut lines = 0;
        for block in blocks.iter().cycle() {
            if lines >= 5000 {
                break;
            }
            out.push_str(block);
            lines += block.matches('\n').count();
        }
        out
    }

    /// `s` around byte `at`.
    fn window(s: &str, at: usize) -> String {
        let from = at.saturating_sub(160);
        let from = (from..=at).find(|&i| s.is_char_boundary(i)).unwrap_or(at);
        s[from..].chars().take(480).collect()
    }

    /// How the tree's `Doc` and pulldown-cmark's differ for one note, if they do.
    fn difference(name: &str, text: &str, flavour: &Flavour) -> Option<String> {
        let (tree, old) = (super::super::parse(text, flavour), parse(text, flavour));
        if tree == old {
            return None;
        }
        let (a, b) = (format!("{:?}", tree.blocks), format!("{:?}", old.blocks));
        let same = a.bytes().zip(b.bytes()).take_while(|(x, y)| x == y);
        let at = same.count();
        Some(format!(
            "{name} ({}):\n  tree: {}\n  then: {}",
            flavour.name,
            window(&a, at.min(a.len())),
            window(&b, at.min(b.len()))
        ))
    }

    /// The tree gives the reader the `Doc` pulldown-cmark gave it: for the example workspace
    /// as the reader opens it, then for every Markdown file of the repository, the tricky
    /// cases here and the items' ones, and a 5,000-line note, in each flavour.
    #[test]
    fn the_tree_gives_the_reader_the_doc_pulldown_cmark_gave() {
        let mut differ = Vec::new();
        let workspace = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../example-workspace");
        let flavour = flavour_for(&workspace);
        for e in walkdir::WalkDir::new(&workspace).into_iter().flatten() {
            let p = e.path();
            if p.extension().is_some_and(|x| x == "md") {
                let name = p.strip_prefix(&workspace).unwrap().to_string_lossy();
                let text = std::fs::read_to_string(p).unwrap();
                differ.extend(difference(&name, &text, &flavour));
            }
        }
        let corpus = cst::corpus();
        let long = long_note();
        for name in ["obsidian", "github", "commonmark"] {
            let flavour = Flavour::built_in(name).unwrap();
            for (name, text) in &corpus {
                differ.extend(difference(name, text, &flavour));
            }
            let tricky = TRICKY.iter().chain(crate::items::tests::TRICKY);
            for (k, text) in tricky.enumerate() {
                differ.extend(difference(&format!("Tricky {k}"), text, &flavour));
            }
            differ.extend(difference("5,000 lines", &long, &flavour));
        }
        let obsidian = Flavour::built_in("obsidian").unwrap();
        for (text, _) in DIFFERENT {
            if difference("Different", text, &obsidian).is_none() {
                differ.push(format!("reads alike now, so it isn't DIFFERENT: {text:?}"));
            }
        }
        assert!(
            differ.is_empty(),
            "{} read differently:\n{}",
            differ.len(),
            differ.join("\n")
        );
    }
}
