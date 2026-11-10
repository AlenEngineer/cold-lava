use std::path::Path;
use std::time::{Duration, Instant};

use super::*;

fn obsidian() -> Gates {
    Gates::of(&Flavour::built_in("obsidian").unwrap())
}

fn github() -> Gates {
    Gates::of(&Flavour::built_in("github").unwrap())
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

/// Every Markdown file in the example workspace, the docs and the tests' fixtures.
fn corpus() -> Vec<(String, String)> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut out = Vec::new();
    for dir in ["example-workspace", "docs", "crates", "examples"] {
        for e in walkdir::WalkDir::new(root.join(dir)).into_iter().flatten() {
            let p = e.path();
            if p.extension().is_some_and(|x| x == "md")
                && !p.components().any(|c| c.as_os_str() == "target")
            {
                if let Ok(text) = std::fs::read_to_string(p) {
                    out.push((p.display().to_string(), text));
                }
            }
        }
    }
    assert!(out.len() > 50, "found {} notes", out.len());
    out
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
    let text = "[[A]] %%c%% $x$ #t ^id\n> [!note]\n";
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

fn same(a: &rowan::GreenElementRef<'_>, b: &rowan::GreenElementRef<'_>) -> bool {
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
