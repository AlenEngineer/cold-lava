use std::time::{Duration, Instant};

use super::*;

fn obsidian() -> Gates {
    Gates::of(&Flavour::built_in("obsidian").unwrap())
}

fn github() -> Gates {
    Gates::of(&Flavour::built_in("github").unwrap())
}

fn step(s: Step<'_>) -> String {
    match s {
        Step::Enter(k, at) => format!("> {k:?} {at}"),
        Step::Leave(k, at) => format!("< {k:?} {at}"),
        Step::Token(k, t, at) => format!("{k:?} {t:?} {at}"),
    }
}

#[test]
fn reading_forwards_takes_the_steps_of_a_walk_through_the_tree() {
    let mut notes = corpus();
    notes.extend(EDGES.iter().map(|e| (format!("{e:?}"), e.to_string())));
    for gates in [obsidian(), Gates::INDEX] {
        for (name, text) in &notes {
            let tree = Tree::with(text, gates);
            let mut walked = vec![step(Step::Enter(Kind::Document, 0))];
            walk(tree.green(), 0, &mut |s| walked.push(step(s)));
            walked.push(step(Step::Leave(Kind::Document, text.len())));
            let mut read_steps = Vec::new();
            read(text, gates, &mut |s| read_steps.push(step(s)));
            assert!(walked == read_steps, "{name}");
        }
    }
}

/// Every kind in the tree of `text`, in order.
fn kinds(text: &str, gates: Gates) -> Vec<Kind> {
    let tree = Tree::with(text, gates);
    let mut out = Vec::new();
    walk(tree.green(), 0, &mut |s| match s {
        Step::Enter(k, _) | Step::Token(k, _, _) => out.push(k),
        Step::Leave(..) => {}
    });
    out
}

/// The text of every `kind` in the tree of `text`.
fn texts(text: &str, gates: Gates, kind: Kind) -> Vec<String> {
    Tree::with(text, gates)
        .root()
        .descendants_with_tokens()
        .filter(|e| e.kind() == kind)
        .map(|e| match e {
            NodeOrToken::Node(n) => n.text().to_string(),
            NodeOrToken::Token(t) => t.text().to_string(),
        })
        .collect()
}

const EDGES: &[&str] = &[
    "",
    "\n",
    "\n\n\n",
    "no final newline",
    "\u{feff}# A heading after a BOM\n",
    "\u{feff}---\ntitle: x\n---\n",
    "line one\r\nline two\r\n\r\n- item\r\n",
    "---\r\ntags: [a]\r\n---\r\nbody\r\n",
    "---\nunclosed: frontmatter\n",
    "---\n",
    "\tindented with a tab\n\t- a\n\t\t- b\n",
    "- a\n  - b\n    - c\n      - d\n\n  back in a\n",
    "1. one\n2. two\n   1) nested\n10. ten\n",
    "- [ ] open\n- [x] done\n- [/] doing\n- [ ]\n- [] not a task\n",
    "> quote\nlazy\n> > nested\n\n> [!warning]- Folded\n> body [[Link]]\n",
    "```rust\nfn x() {}\n```\n~~~\n```\nstill code\n~~~\n````\nunclosed",
    "    indented code\n\n    more\n\n\npara\n",
    "| a | b |\n|---|:-:|\n| [[x\\|y]] | `c|d` |\nafter\n",
    "Title\n=====\nSub\n---\n***\n",
    "<div>\nhtml [[NotALink]]\n</div>\n\n<!-- comment\n-->\n",
    "%%\nhidden [[x]]\n%%\nshown %%inline%% ^block-id\n",
    "$$\nx^2\n$$\nand $y$ here\n",
    "key:: value [[Linked]]\n- [ ] Pay [due:: 2026-01-01] (repeat:: every week) 📅 2026-01-02\n",
    "#tag #123 #nested/tag (#paren) [x](#not-a-tag) `#code`\n",
    "- \n  \n- after blank\n-\n\n  item after a blank start\n",
    "trailing spaces   \n\t\n   \n",
    "> - a\n>   - b\n> c\n",
    "* * *\n- - -\n_ _ _\n",
    "lone \r in a line\r\n",
    "See [text][ref], [ref][], [ref], ![img][ref] and [none].\n\n[ref]: Note.md \"Title\"\n  [Other Ref]: <a b.md>\n> [q]: q.md\n- [i]: i.md\n",
    "**#bold** _#slanted_ ==#marked== ~~#struck~~ a**#b** snake_#case\n",
    "| a | b |\n| - | - |\n| [[B\\|x]] [[\\|y]] | ![[B#H\\|z]] [t](<a b.md> \"c (d)\") |\n\n[[B\\|x]] [a](B.md (t)) [b](<a (b).md> 'c') [c](a_(b).md) [d]( e.md ) [f](g\\) [h](i (j\n",
    "A [link whose text\n> goes on\nto here](Note.md) ] ](x) [![img](p.png)](n.md) [open\n\n[Source]: [[Book]]\n](y)\n",
    "*a* **b** ***c*** _d_ __e__ ~~f~~ ==g== \\*h\\* &amp; <b>i</b> <https://j.k> [^l]\n\n[^l]: note  \nbreak\\\nend\n",
    "> **bold over\n> two lines** and *unclosed\n\n- item with _emphasis\n  over lines_ ^id\n",
    "| *a* | **b |\n| - | - |\n| c** | ==d== |\n",
    "***a** b* *c **d*** ****e**** _f__g__h_ snake_case_word 2*3*4 **==#t==** [*x*](y) *z [w* v](u)\n",
    "\u{feff}*emphasis after a BOM*\r\n**strong  \r\nover a CRLF**\\\r\n&#x1F30B; &nope <a href=\"x\">\r\n",
];

#[test]
fn printing_the_tree_gives_the_note_back() {
    let mut notes: Vec<(String, String)> = corpus();
    notes.extend(EDGES.iter().map(|e| (format!("{e:?}"), e.to_string())));
    for gates in [obsidian(), github(), Gates::INDEX, Gates(0)] {
        for (name, text) in &notes {
            let tree = Tree::with(text, gates);
            assert_eq!(&tree.text(), text, "{name}");
            assert_eq!(tree.root().kind(), Kind::Document);
        }
    }
}

#[test]
fn blocks_and_inlines_are_found() {
    let text = "---\nup: \"[[Parent]]\"\n---\n# Head #tag\n> [!note] Title\n> - [ ] task [due:: 2026-01-01] ^abc\n\n```js\n[[no]]\n```\n| a | b |\n| - | - |\n| [[x]] | ![[y.png]] |\n";
    let k = kinds(text, obsidian());
    for want in [
        Kind::Frontmatter,
        Kind::AtxHeading,
        Kind::Callout,
        Kind::CalloutTitle,
        Kind::List,
        Kind::ListItem,
        Kind::TaskMarker,
        Kind::InlineField,
        Kind::BlockId,
        Kind::FencedCode,
        Kind::Table,
        Kind::TableDelimiterRow,
        Kind::TableCell,
        Kind::Embed,
    ] {
        assert!(k.contains(&want), "no {want:?} in {k:?}");
    }
    assert_eq!(
        texts(text, obsidian(), Kind::Target),
        ["Parent", "x", "y.png"]
    );
    assert_eq!(texts(text, obsidian(), Kind::Tag), ["#tag"]);
    assert_eq!(texts(text, obsidian(), Kind::TaskMarker), ["[ ]"]);
}

#[test]
fn containers_hold_their_lines() {
    let text = "- a\n  - b\n    more b\n- c\n\n> q\nlazy\n";
    let root = Tree::with(text, obsidian()).root();
    let top: Vec<Kind> = root.children().map(|n| n.kind()).collect();
    // A list goes on over blank lines until a line that isn't in it, so they are its.
    assert_eq!(top, [Kind::List, Kind::BlockQuote]);
    let list = root.first_child().unwrap();
    let items: Vec<String> = list.children().map(|n| n.text().to_string()).collect();
    assert_eq!(items, ["- a\n  - b\n    more b\n", "- c\n\n"]);
    let first = list.first_child().unwrap();
    let para = first
        .children()
        .find(|n| n.kind() == Kind::Paragraph)
        .unwrap();
    assert_eq!(
        para.text().to_string(),
        "a\n",
        "the next line's indentation is the item's"
    );
    let quote = root.last_child().unwrap();
    assert_eq!(quote.first_child().unwrap().kind(), Kind::Paragraph);
    assert_eq!(quote.first_child().unwrap().text().to_string(), "q\nlazy\n");
}

#[test]
fn code_and_html_hold_no_links() {
    for text in [
        "```\n[[A]]\n```\n",
        "    [[A]]\n",
        "<div>\n[[A]]\n</div>\n",
        "``[[A]]``\n",
        "- x\n\n  ```\n  [[A]]\n  ```\n",
    ] {
        assert!(texts(text, obsidian(), Kind::Target).is_empty(), "{text:?}");
    }
    // A list item's indented continuation is its text, not code.
    assert_eq!(texts("- a\n    [[B]]\n", obsidian(), Kind::Target), ["B"]);
}

#[test]
fn the_flavour_gates_syntax() {
    let text = "[[A]] %%c%% $x$ #t ^id\n> [!custom]\n";
    let gh = kinds(text, github());
    for missing in [
        Kind::WikiLink,
        Kind::Comment,
        Kind::InlineMath,
        Kind::Tag,
        Kind::BlockId,
        Kind::Callout,
    ] {
        assert!(!gh.contains(&missing), "{missing:?} under github");
    }
    let ob = kinds(text, obsidian());
    for found in [
        Kind::WikiLink,
        Kind::Comment,
        Kind::InlineMath,
        Kind::Tag,
        Kind::BlockId,
        Kind::Callout,
    ] {
        assert!(ob.contains(&found), "no {found:?} under obsidian");
    }
    // GitHub's five alerts are callouts there, and nothing else is.
    assert!(kinds("> [!NOTE]\n> x\n", github()).contains(&Kind::Callout));
    assert!(!kinds("> [!custom]\n> x\n", github()).contains(&Kind::Callout));
}

#[test]
fn a_reference_link_is_a_link_where_the_note_defines_its_label() {
    let text = "See [the note][ref], [Ref][], [ref], ![pic][ref], [none][] and [none].\n\n[ref]: Note.md \"Title\"\n[REF]: Other.md\n> [in quote]: <Quoted Note.md>\n";
    let gates = obsidian();
    let links = ["[the note][ref]", "[Ref][]", "[ref]"];
    assert_eq!(texts(text, gates, Kind::Link), links);
    assert_eq!(texts(text, gates, Kind::Image), ["![pic][ref]"]);
    let labels = ["ref", "Ref", "ref", "ref"];
    assert_eq!(texts(text, gates, Kind::LinkLabel), labels);
    assert_eq!(texts(text, gates, Kind::LinkText), ["the note", "pic"]);
    assert_eq!(texts(text, gates, Kind::LinkDefinition).len(), 3);
    let tree = Tree::with(text, gates);
    let defs = tree.definitions();
    assert_eq!(defs.get("ref"), Some("Note.md"), "the first wins");
    assert_eq!(defs.get("In   Quote"), Some("Quoted Note.md"));
    assert_eq!(defs.get("none"), None);
    // A definition can't interrupt a paragraph, and with footnotes `[^1]:` is a footnote.
    for none in ["text\n[a]: b.md\n", "[^1]: b.md\n", "[a]: b.md and more\n"] {
        let tree = Tree::with(none, gates);
        assert!(tree.definitions().is_empty(), "{none:?}");
    }
    let plain = Tree::with("[^1]: b.md\n", Gates(0));
    assert_eq!(plain.definitions().get("^1"), Some("b.md"));
}

#[test]
fn a_links_text_may_hold_an_image_and_end_on_a_later_line() {
    let gates = obsidian();
    let nested = "[![alt](pic.png)](Note.md)\n";
    assert_eq!(texts(nested, gates, Kind::Image), ["![alt](pic.png)"]);
    assert_eq!(texts(nested, gates, Kind::LinkDest), ["pic.png", "Note.md"]);
    // The `[` is still open on the paragraph's next line; a blank line ends the paragraph.
    let wrapped = "See [text that\ngoes on](Note.md) here, not this](Other.md).\n\n[alone\n\nnext](Third.md)\n";
    assert_eq!(texts(wrapped, gates, Kind::LinkEnd), ["](Note.md)"]);
    assert_eq!(texts(wrapped, gates, Kind::LinkDest), ["Note.md"]);
    // With wikilinks, a definition whose destination starts one is a line with a wikilink.
    let field = "[Source]: [[Book]]\n";
    assert_eq!(texts(field, gates, Kind::Target), ["Book"]);
    assert!(Tree::with(field, gates).definitions().is_empty());
    let defs = Tree::with(field, github());
    assert_eq!(defs.definitions().get("source"), Some("[[Book]]"));
}

#[test]
fn a_wikilink_in_a_table_cell_may_escape_the_pipe_before_its_alias() {
    let gates = obsidian();
    let table = "| a | b |\n| --- | --- |\n| [[B\\|x]] | ![[B#H\\|y]] |\n";
    assert_eq!(texts(table, gates, Kind::Target), ["B", "B"]);
    assert_eq!(texts(table, gates, Kind::Anchor), ["#H"]);
    assert_eq!(texts(table, gates, Kind::Alias), ["x", "y"]);
    let pipes = texts(table, gates, Kind::Pipe);
    assert_eq!(pipes.iter().filter(|p| *p == "\\|").count(), 2, "{pipes:?}");
    // Outside a cell the backslash is the target's.
    let outside = "[[B\\|x]] and [[B#H\\|y]]\n";
    assert_eq!(texts(outside, gates, Kind::Target), ["B\\", "B"]);
    assert_eq!(texts(outside, gates, Kind::Anchor), ["#H\\"]);
    assert_eq!(texts(outside, gates, Kind::Pipe), ["|", "|"]);
}

#[test]
fn a_link_ends_after_its_destination_and_its_title() {
    let dests = |text: &str| texts(text, obsidian(), Kind::LinkDest);
    assert_eq!(
        dests(
            "[a](B.md (title)) [b](<a (b).md> \"t (x)\") [c](a_(b).md) d) ![e](p.png 'it\\'s (x)')\n"
        ),
        [
            "B.md (title)",
            "<a (b).md> \"t (x)\"",
            "a_(b).md",
            "p.png 'it\\'s (x)'"
        ]
    );
    // What CommonMark reads as no link ends at its first `)`, as it always did.
    assert_eq!(
        dests("[a](b (c d) [e](f \"g) h) [i](j(k) [l](<m) n)\n"),
        ["b (c d", "f \"g", "j(k", "<m"]
    );
    for (written, dest) in [
        ("B.md", Some("B.md")),
        ("B.md#H \"title\"", Some("B.md#H")),
        ("<My Note.md> 'title'", Some("My Note.md")),
        (" B.md (title) ", Some("B.md")),
        ("", Some("")),
        ("B.md and more", None),
        ("B.md \"title", None),
        ("<B.md>x", None),
        ("<B.md", None),
    ] {
        let found = link_destination(written).map(|d| &written[d]);
        assert_eq!(found, dest, "{written:?}");
    }
}

#[test]
fn a_tag_may_follow_the_markers_that_open_emphasis_and_highlights() {
    let tags = |text: &str, gates: Gates| texts(text, gates, Kind::Tag);
    let emphasis = "**#bold** _#slanted_ *#star* __#snake_case__ ***#both*** (**#paren**)\n";
    assert_eq!(
        tags(emphasis, obsidian()),
        [
            "#bold",
            "#slanted",
            "#star",
            "#snake_case",
            "#both",
            "#paren"
        ]
    );
    let marked = "==#marked== ~~#struck~~ **==#nested==**\n";
    let all = ["#marked", "#struck", "#nested"];
    assert_eq!(tags(marked, obsidian()), all);
    assert_eq!(tags(marked, Gates::INDEX), all);
    // GitHub has strikethroughs and no highlights.
    let github = github().set(Syntax::Tags, true);
    assert_eq!(tags(marked, github), ["#struck"]);
    for none in [
        "a#b",
        "a**#b**",
        "snake_#case",
        "http://x.y/#frag",
        "http://x.y/**#frag",
        "[[#Heading]]",
        "[[Note#Heading]]",
        "[text](#heading)",
        "[text](**#heading)",
        "`a **#code**`",
        "**#123**",
        "=#one=",
        "~#one~",
    ] {
        assert!(tags(none, obsidian()).is_empty(), "{none:?}");
    }
}

#[test]
fn text_holds_emphasis_strong_strikethrough_and_highlights() {
    let ob = obsidian();
    let all = "*a* **b** ***c*** _d_ __e__ ~~f~~ ~g~ ==h== and ==**i** j==\n";
    assert_eq!(texts(all, ob, Kind::Emphasis), ["*a*", "***c***", "_d_"]);
    assert_eq!(
        texts(all, ob, Kind::Strong),
        ["**b**", "**c**", "__e__", "**i**"]
    );
    assert_eq!(texts(all, ob, Kind::Strikethrough), ["~~f~~", "~g~"]);
    assert_eq!(texts(all, ob, Kind::Highlight), ["==h==", "==**i** j=="]);
    // GitHub has strikethroughs and no highlights; CommonMark has neither.
    assert_eq!(texts(all, github(), Kind::Strikethrough), ["~~f~~", "~g~"]);
    assert!(texts(all, github(), Kind::Highlight).is_empty());
    let commonmark = Gates::of(&Flavour::built_in("commonmark").unwrap());
    assert!(texts(all, commonmark, Kind::Strikethrough).is_empty());
    assert_eq!(texts(all, commonmark, Kind::Emphasis).len(), 3);
    // A reader that doesn't ask for what text holds gets none of it.
    assert!(texts(all, ob.plain(), Kind::Emphasis).is_empty());
    assert!(texts(all, Gates::INDEX, Kind::Strong).is_empty());
}

#[test]
fn emphasis_follows_the_rules_for_delimiter_runs() {
    let ob = obsidian();
    let spans = |text: &str| {
        let mut found = texts(text, ob, Kind::Emphasis);
        found.extend(texts(text, ob, Kind::Strong));
        found
    };
    // `_` neither opens nor closes inside a word; `*` does both.
    assert!(spans("snake_case_name, a_b_ and _c_d\n").is_empty());
    assert_eq!(spans("2*3*4\n"), ["*3*"]);
    // A run that nothing closes, or that nothing opened, stays text.
    assert!(spans("*a **b\n").is_empty());
    assert!(spans("a* b** c\n").is_empty());
    assert!(spans("* a *\n\n** b **\n").is_empty());
    // The rule of three: `**` inside a word can't close the `*` that opened.
    assert_eq!(spans("*foo**bar**baz*\n"), ["*foo**bar**baz*", "**bar**"]);
    assert_eq!(spans("***a** b*\n"), ["***a** b*", "**a**"]);
    // Emphasis goes over a line break of its block, with the markers of the block's
    // containers inside it, and never over a blank line.
    assert_eq!(spans("*one\ntwo* three\n"), ["*one\ntwo*"]);
    assert_eq!(spans("> **one\n> two**\n"), ["**one\n> two**"]);
    assert!(spans("*one\n\ntwo*\n").is_empty());
    // It opens and closes inside the same link's text, or holds the link whole.
    assert_eq!(spans("[*a*](x) *b [c* d](y)\n"), ["*a*"]);
    assert_eq!(
        spans("*a [b](c) [[D]] `e*` f*\n"),
        ["*a [b](c) [[D]] `e*` f*"]
    );
    // A `*` in a URL, in code or escaped is no marker.
    assert!(spans("<https://x.y/*a*> `*b*` \\*c\\* [d](e*f*g)\n").is_empty());
    // A table's cell is its own text.
    let table = "| *a* | **b |\n| - | - |\n| c** | d |\n";
    assert_eq!(spans(table), ["*a*"]);
}

#[test]
fn text_holds_escapes_references_html_autolinks_footnotes_and_hard_breaks() {
    let ob = obsidian();
    let text =
        "\\*no\\* &amp; &#35; &#x23; & <b>x</b> <!-- c --> <https://a.b/c_d_e> <me@x.org> < 3\n";
    assert_eq!(texts(text, ob, Kind::Escape), ["\\*", "\\*"]);
    assert_eq!(texts(text, ob, Kind::Entity), ["&amp;", "&#35;", "&#x23;"]);
    assert_eq!(
        texts(text, ob, Kind::InlineHtml),
        ["<b>", "</b>", "<!-- c -->"]
    );
    assert_eq!(
        texts(text, ob, Kind::Autolink),
        ["<https://a.b/c_d_e>", "<me@x.org>"]
    );
    assert_eq!(
        texts(text, ob, Kind::Url),
        ["https://a.b/c_d_e", "me@x.org"]
    );
    assert!(texts(text, ob, Kind::Emphasis).is_empty());
    // Two blanks or a backslash break the line, where the block has another line.
    let lines = "one  \ntwo\\\nthree  \n\n# Heading  \n";
    assert_eq!(texts(lines, ob, Kind::HardBreak), ["  ", "\\"]);
    // A footnote's reference, and the `[^label]:` that starts its paragraph.
    let notes = "Text[^1] and [^a note].\n\n[^1]: The note,\nlazily.\n\nNot [^1]: here.\n";
    assert_eq!(
        texts(notes, ob, Kind::FootnoteRef),
        ["[^1]", "[^a note]", "[^1]"]
    );
    assert_eq!(texts(notes, ob, Kind::FootnoteMarker), ["[^1]:"]);
    assert_eq!(
        texts(notes, ob, Kind::FootnoteLabel),
        ["^1", "^a note", "^1", "^1"]
    );
    let marker = Tree::with(notes, ob)
        .root()
        .descendants()
        .find(|n| n.kind() == Kind::FootnoteMarker)
        .unwrap();
    assert_eq!(marker.parent().unwrap().kind(), Kind::Paragraph);
    assert_eq!(
        marker.parent().unwrap().text().to_string(),
        "[^1]: The note,\nlazily.\n"
    );
    let commonmark = Gates::of(&Flavour::built_in("commonmark").unwrap());
    assert!(texts(notes, commonmark, Kind::FootnoteRef).is_empty());
    assert!(texts(notes, commonmark, Kind::FootnoteMarker).is_empty());
}

/// The steps through `text` with what text itself holds put back into text.
fn folded(text: &str, gates: Gates) -> Vec<String> {
    fn flush(out: &mut Vec<String>, run: &mut Option<(usize, usize)>, text: &str) {
        if let Some((a, z)) = run.take() {
            out.push(step(Step::Token(Kind::Text, &text[a..z], a)));
        }
    }
    let mut out = Vec::new();
    let mut run: Option<(usize, usize)> = None;
    // Inside an autolink or a footnote's brackets, whose tokens were text.
    let mut within = 0;
    read(text, gates, &mut |s| {
        let span = |k: Kind| {
            matches!(
                k,
                Kind::Emphasis | Kind::Strong | Kind::Strikethrough | Kind::Highlight
            )
        };
        let whole =
            |k: Kind| matches!(k, Kind::Autolink | Kind::FootnoteRef | Kind::FootnoteMarker);
        let was_text = |k: Kind| {
            matches!(
                k,
                Kind::Text
                    | Kind::EmphasisMarker
                    | Kind::Escape
                    | Kind::Entity
                    | Kind::HardBreak
                    | Kind::InlineHtml
            )
        };
        match s {
            Step::Enter(k, _) | Step::Leave(k, _) if span(k) => {}
            Step::Enter(k, _) if whole(k) => within += 1,
            Step::Leave(k, _) if whole(k) => within -= 1,
            Step::Token(k, t, at) if within > 0 || was_text(k) => {
                let from = run.map_or(at, |r| r.0);
                run = Some((from, at + t.len()));
            }
            other => {
                flush(&mut out, &mut run, text);
                out.push(step(other));
            }
        }
    });
    flush(&mut out, &mut run, text);
    out
}

/// Reading what text holds only cuts text into more tokens and wraps them in nodes: the
/// blocks, links, tags and everything else the index and the items read are the same with it
/// and without it.
#[test]
fn what_text_holds_changes_nothing_a_plain_reader_reads() {
    let mut notes = corpus();
    notes.extend(EDGES.iter().map(|e| (format!("{e:?}"), e.to_string())));
    let commonmark = Gates::of(&Flavour::built_in("commonmark").unwrap());
    for gates in [obsidian(), github(), commonmark] {
        for (name, text) in &notes {
            assert!(folded(text, gates) == folded(text, gates.plain()), "{name}");
        }
    }
    // A tag after the markers that open emphasis is the same tag either way, now inside it.
    let text = "**#bold** _#slanted_ ==#marked== ~~#struck~~ a**#b** snake_#case\n";
    let tags = ["#bold", "#slanted", "#marked", "#struck"];
    assert_eq!(texts(text, obsidian(), Kind::Tag), tags);
    assert_eq!(texts(text, obsidian().plain(), Kind::Tag), tags);
    assert_eq!(texts(text, Gates::INDEX, Kind::Tag), tags);
    assert_eq!(texts(text, obsidian(), Kind::Strong), ["**#bold**"]);
    assert_eq!(texts(text, obsidian(), Kind::Emphasis), ["_#slanted_"]);
    assert_eq!(texts(text, obsidian(), Kind::Highlight), ["==#marked=="]);
    assert_eq!(
        texts(text, obsidian(), Kind::Strikethrough),
        ["~~#struck~~"]
    );
}

#[test]
fn an_edit_that_changes_the_definitions_reads_every_link_again() {
    let text = "See [ref] here.\n\nMore text.\n\n[ref]: Note.md\n";
    let gates = obsidian();
    let tree = Tree::with(text, gates);
    assert_eq!(texts(text, gates, Kind::LinkLabel), ["ref"]);
    // Without its definition, `[ref]` in the first block is text.
    let at = text.find("[ref]:").unwrap();
    let cut = &text[..at];
    let edited = tree.edit(cut, at..text.len(), 0);
    assert_eq!(edited, Tree::with(cut, gates));
    assert!(texts(cut, gates, Kind::LinkLabel).is_empty());
    assert_eq!(edited.edit(text, at..at, text.len() - at), tree);
    // An edit that leaves the definitions alone keeps them, and reads `[ref]` with them.
    let at = text.find("More").unwrap();
    let mut more = text.to_string();
    more.insert_str(at, "[ref] ");
    let edited = tree.edit(&more, at..at, 6);
    assert_eq!(edited, Tree::with(&more, gates));
    assert_eq!(edited.definitions(), tree.definitions());
    assert_eq!(texts(&more, gates, Kind::LinkLabel), ["ref", "ref"]);
}

/// A small, seeded generator, so a failure prints the case that made it.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }
}

const PIECES: &[&str] = &[
    "\n",
    "\n\n",
    "\r\n",
    " ",
    "  ",
    "    ",
    "\t",
    "x",
    "word ",
    "> ",
    ">",
    "- ",
    "* ",
    "1. ",
    "2) ",
    "[ ] ",
    "[x] ",
    "# ",
    "## ",
    "#tag ",
    "```",
    "~~~",
    "---",
    "===",
    "***",
    "| ",
    "|---|",
    "[[",
    "]]",
    "[[Note|alias]]",
    "![[img.png]]",
    "[a](b.md)",
    "`",
    "``",
    "%%",
    "$$",
    "$",
    "<div>",
    "<!--",
    "-->",
    "[!note] ",
    "key:: ",
    "[k:: v]",
    " ^id",
    "[r]: r.md",
    "[r]",
    "[r][]",
    "[t][r]",
    "**#t**",
    "_#t_",
    "*",
    "**",
    "_",
    "~~",
    "==",
    "\\",
    "&amp;",
    "<b>",
    "<https://x.y>",
    "[^n]",
    "[^n]: ",
    "é",
    "\u{feff}",
];

/// Random edits, one after another, each read by `edit` and checked against a full parse.
fn fuzz(seed: u64, start: &str, steps: usize) {
    let mut rng = Rng(seed);
    for gates in [obsidian(), github()] {
        let mut text = start.to_string();
        let mut tree = Tree::with(&text, gates);
        for step in 0..steps {
            let bounds: Vec<usize> = (0..=text.len())
                .filter(|&i| text.is_char_boundary(i))
                .collect();
            let a = bounds[rng.below(bounds.len())];
            let after: Vec<usize> = bounds
                .iter()
                .copied()
                .filter(|&b| b >= a && b <= a + 12)
                .collect();
            let z = after[rng.below(after.len())];
            let insert: String = match rng.below(4) {
                0 => String::new(),
                _ => (0..1 + rng.below(3))
                    .map(|_| PIECES[rng.below(PIECES.len())])
                    .collect(),
            };
            let before = text.clone();
            text.replace_range(a..z, &insert);
            let edited = tree.edit(&text, a..z, insert.len());
            let full = Tree::with(&text, gates);
            assert!(
                edited == full,
                "seed {seed} step {step}: replacing {a}..{z} of {before:?} with {insert:?}\n\
                 edited: {:#?}\nfull: {:#?}",
                edited.root(),
                full.root()
            );
            assert_eq!(edited.text(), text);
            tree = edited;
        }
    }
}

#[test]
fn an_edit_reparses_to_what_a_full_parse_reads() {
    let mut starts: Vec<String> = EDGES.iter().map(|s| s.to_string()).collect();
    starts.push(EDGES.concat());
    for (seed, start) in starts.iter().enumerate() {
        fuzz(seed as u64 * 7919 + 1, start, 300);
    }
}

#[test]
fn an_edit_on_the_corpus_reparses_to_what_a_full_parse_reads() {
    for (k, (_, text)) in corpus().iter().enumerate().take(80) {
        fuzz(k as u64 * 104_729 + 3, text, 40);
    }
}

#[test]
fn an_edit_keeps_the_blocks_it_cannot_change() {
    let text: String = (0..200)
        .map(|i| format!("Paragraph {i} with [[Link {i}]].\n\n"))
        .collect();
    let tree = Tree::with(&text, obsidian());
    let at = text.find("Paragraph 100").unwrap() + 4;
    let mut new = text.clone();
    new.insert(at, 'x');
    let edited = tree.edit(&new, at..at, 1);
    assert_eq!(edited, Tree::with(&new, obsidian()));
    let old: Vec<_> = tree.green().children().collect();
    let shared = (edited.green().children())
        .filter(|c| old.iter().any(|o| same(o, c)))
        .count();
    let total = edited.green().children().len();
    assert!(shared >= total - 4, "{shared} of {total} kept");
}

type Green<'a> = NodeOrToken<&'a rowan::GreenNodeData, &'a rowan::GreenTokenData>;

fn same(a: &Green<'_>, b: &Green<'_>) -> bool {
    match (a, b) {
        (NodeOrToken::Node(a), NodeOrToken::Node(b)) => std::ptr::eq(*a, *b),
        _ => false,
    }
}

/// A long note of every kind of block, for timing.
fn long_note(lines: usize) -> String {
    let blocks = [
        "## Section #topic\n\n",
        "A paragraph with [[Some Note|an alias]], a [link](other.md), `code` and #tag.\nIt goes on for a second line. ^blk\n\n",
        "- [ ] A task [due:: 2026-11-12] 📅 2026-11-12\n  - [x] A sub-task ✅ 2026-11-01\n- A plain item with [[Link]]\n\n",
        "> [!note] A callout\n> with a line [[Inside]]\n\n",
        "```rust\nfn main() {\n    println!(\"[[not a link]]\");\n}\n```\n\n",
        "| Name | Value |\n| --- | --- |\n| [[A]] | 1 |\n| [[B]] | 2 |\n\n",
    ];
    let mut out = String::new();
    let mut k = 0;
    while out.lines().count() < lines {
        out.push_str(blocks[k % blocks.len()]);
        k += 1;
    }
    out
}

fn median(mut v: Vec<Duration>) -> Duration {
    v.sort();
    v[v.len() / 2]
}

/// The number the editor lives on: one keystroke in a 5,000-line note. Run with
/// `cargo test --release -p cold-lava-engine keystroke -- --ignored --nocapture`.
#[test]
#[ignore]
fn keystroke() {
    let text = long_note(5000);
    let gates = obsidian();
    let full = median(
        (0..21)
            .map(|_| {
                let t = Instant::now();
                std::hint::black_box(Tree::with(&text, gates));
                t.elapsed()
            })
            .collect(),
    );
    let tree = Tree::with(&text, gates);
    let at = text.len() / 2;
    let at = (at..)
        .find(|&i| text.is_char_boundary(i) && text.as_bytes()[i] == b' ')
        .unwrap();
    let mut new = text.clone();
    new.insert(at, 'x');
    let key = median(
        (0..201)
            .map(|_| {
                let t = Instant::now();
                std::hint::black_box(tree.edit(&new, at..at, 1));
                t.elapsed()
            })
            .collect(),
    );
    assert_eq!(tree.edit(&new, at..at, 1), Tree::with(&new, gates));
    println!(
        "keystroke: {} lines, {} KiB: full parse {:?}, one keystroke {:?} (medians)",
        text.lines().count(),
        text.len() / 1024,
        full,
        key
    );
}
