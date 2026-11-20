//! The bookmark in a note (#190, docs/extensions.md "Anchors, as built"): where an item takes
//! an anchor, and whether the note reads back with nothing new but it. An anchor is a `^name`
//! that ends the item's last own line, where the walk reads one (`items.rs`): a list item's
//! (a task's) line before its first child, or a paragraph's last line. Every other item has
//! no place for one in Markdown, and is refused with what to keep instead. A leaf: it reads
//! items, types, trees and text, and needs serde_json to compare what items say.

use crate::cst::{Kind, Step};
use crate::edit::EditError;
use crate::items::{Data, Item, Settings};
use crate::markdown::Syntax;
use crate::types::On;
use std::collections::HashMap;

/// What an item gives an anchor.
pub(super) enum Room {
    /// The one it has already, its `block_id`.
    Kept,
    /// A place at the end of this line of the note, the item's last own line.
    At(usize),
    /// None: why, and what to keep instead.
    Refused(String),
}

/// What `item` of the note written `text`, read with `s`, gives an anchor.
pub(super) fn room(item: &Item, text: &str, s: &Settings) -> Room {
    let id = &item.id;
    let stem = item.note.strip_suffix(".md").unwrap_or(&item.note);
    if !s.flavour.has(Syntax::BlockIds) {
        return Room::Refused(format!(
            "{} is read as {} Markdown, which has no block ids, so a ^name there would be \
             words: give the workspace a flavour with block-ids ([markdown] in config.toml), \
             or keep a link to the note, [[{stem}]]",
            item.note, s.flavour.name
        ));
    }
    if item.block_id.is_some() {
        return Room::Kept;
    }
    let heading = |words: &str| {
        format!(
            "{id} is a heading, which Markdown has no place for a ^name on: keep a link to \
             the heading instead, [[{stem}#{words}]]"
        )
    };
    let on = match &item.data {
        Data::List { .. } | Data::Block => return Room::At(item.end_line),
        Data::Heading { text, .. } => return Room::Refused(heading(text)),
        Data::Typed(t) => match s.types.types.iter().find(|ty| ty.name == t.kind) {
            Some(ty) => ty.definition.from[t.place.shape].on,
            None => return Room::Refused(format!("no type called {}", t.kind)),
        },
    };
    let above = match item.section.last() {
        Some(section) => format!("[[{stem}#{section}]]"),
        None => format!("[[{stem}]]"),
    };
    let why = match on {
        On::Item | On::Text => return Room::At(item.end_line),
        On::Section => {
            let words = match &item.data {
                Data::Typed(t) => text.get(t.place.text.clone()),
                _ => None,
            };
            heading(words.unwrap_or("…").trim())
        }
        On::Document => {
            format!("{id} is the whole note: keep a link to the note instead, [[{stem}]]")
        }
        On::Row => format!(
            "{id} is a table row, which Markdown has no place for a ^name on: keep a link to \
             the heading above it instead, {above}"
        ),
        On::Aside => format!(
            "{id} is a quote or a callout, which takes a ^name only on a line of its own \
             after it, and cold-lava doesn't write one there yet: keep a link to the heading \
             above it instead, {above}"
        ),
        On::Verbatim => format!(
            "{id} is a code block, where a ^name would be code: keep a link to the heading \
             above it instead, {above}"
        ),
        On::Inline => match &item.parent {
            Some(parent) => format!(
                "{id} is read in the words of another item and has no lines of its own: \
                 anchor that item instead, {parent}"
            ),
            None => format!(
                "{id} is read in the words of a paragraph and has no lines of its own: keep a \
                 link to the heading above it instead, {above}"
            ),
        },
    };
    Room::Refused(why)
}

/// Where an anchor goes to end the line `line` (1-based) of `text`: after its words, before
/// the blanks and the line ending that close it.
pub(super) fn end_of(text: &str, line: usize) -> usize {
    let start: usize = text
        .split_inclusive('\n')
        .take(line - 1)
        .map(str::len)
        .sum();
    let own = text[start..].split('\n').next().unwrap_or_default();
    start + own.trim_end_matches(['\r', ' ', '\t']).len()
}

/// `text` with ` ^name` written at `at`: one space, then the anchor.
pub(super) fn write(text: &str, at: usize, name: &str) -> String {
    let mut out = String::with_capacity(text.len() + name.len() + 2);
    out.push_str(&text[..at]);
    out.push_str(" ^");
    out.push_str(name);
    out.push_str(&text[at..]);
    out
}

/// What a note's tree says, for the read-back: each node it enters and leaves, in order, and
/// where each block id starts.
#[derive(Default)]
pub(super) struct Tree {
    nodes: Vec<(bool, Kind)>,
    ids: Vec<usize>,
}

impl Tree {
    pub(super) fn step(&mut self, step: Step<'_>) {
        match step {
            Step::Enter(kind, _) => self.nodes.push((true, kind)),
            Step::Leave(kind, _) => self.nodes.push((false, kind)),
            Step::Token(Kind::BlockId, _, at) => self.ids.push(at),
            Step::Token(..) => {}
        }
    }
}

/// Whether the note's tree with ` ^name` written at `at`, on its line `line` (`is`), is the
/// tree it had (`was`) with one block id more, that one: the same nodes, entered and left in
/// the same order. Then the anchor ends a line of text, and nothing around it reads
/// otherwise: a code block, a table row or a heading's underline would.
pub(super) fn same_tree(
    was: &Tree,
    is: &Tree,
    at: usize,
    line: usize,
    name: &str,
) -> Result<(), EditError> {
    let one_more = is.ids.len() == was.ids.len() + 1 && is.ids.contains(&(at + 1));
    if was.nodes == is.nodes && one_more {
        return Ok(());
    }
    Err(EditError::ReadBack(format!(
        "line {line} ends no text a ^name can end, so ^{name} would change how the note \
         reads there (as after a closing code fence, a table row or an empty item): end the \
         item with a line of words, or keep a link to the heading above it instead"
    )))
}

/// Whether `after`, the note read again with ` ^name` written for `before[k]`, says what
/// `before` said with nothing new but that anchor. `same[i]` is the item of `after` that
/// `before[i]` is, the one of its kind that starts on its line (`items::anchors::by_line`).
/// Each says what it said (its kind and its data as the wire carries them), on the same
/// lines, under the same section and parent, and `name` is that one's anchor. Ids aren't
/// compared: the item's own, its children's and a later twin's move by the key rules. The
/// anchored item is `after`'s at the place it answers.
pub(super) fn read_back(
    before: &[Item],
    after: &[Item],
    same: &[Option<usize>],
    k: usize,
    name: &str,
) -> Result<usize, EditError> {
    if after.len() != before.len() {
        return Err(EditError::ReadBack(format!(
            "it would have {} items instead of {}",
            after.len(),
            before.len()
        )));
    }
    let says = |i: &Item| serde_json::to_value(&i.data).ok();
    let (was, is) = (parents(before), parents(after));
    for (i, b) in before.iter().enumerate() {
        let a = same[i].map(|j| &after[j]);
        let kept = a.is_some_and(|a| {
            says(b) == says(a)
                && (b.line, b.end_line) == (a.line, a.end_line)
                && b.section == a.section
                && was[i].and_then(|p| same[p]) == same[i].and_then(|j| is[j])
        });
        if !kept {
            return Err(EditError::ReadBack(format!(
                "the {} on line {} would read otherwise",
                b.data.kind(),
                b.line
            )));
        }
    }
    same[k]
        .filter(|&j| after[j].block_id.as_deref() == Some(name))
        .ok_or_else(|| {
            EditError::ReadBack(format!(
                "^{name} wouldn't read as the anchor of the {} on line {}",
                before[k].data.kind(),
                before[k].line
            ))
        })
}

/// `after` with the anchor just written taken off `after[j]`, so that the anchor rule
/// (`items::anchors::change`) compares every other anchor.
pub(super) fn unanchored(after: &[Item], j: usize) -> Vec<Item> {
    let mut others = after.to_vec();
    others[j].block_id = None;
    others
}

/// Each item's parent, by its place among `items`.
fn parents(items: &[Item]) -> Vec<Option<usize>> {
    let place: HashMap<&str, usize> = (items.iter().enumerate())
        .map(|(i, item)| (item.id.as_str(), i))
        .collect();
    (items.iter())
        .map(|i| i.parent.as_deref().and_then(|p| place.get(p).copied()))
        .collect()
}

#[cfg(test)]
mod tests {
    use crate::gateway::{Edit, Error, Op, Reason};
    use crate::identity;
    use crate::items::tests::{TRICKY, settings, with_types};
    use crate::items::{Item, Settings};
    use crate::owner::markdown::{self, After, Made};
    use serde_json::Value as Json;

    /// Types on every role a piece of a note can have, and on a reading of a block's text.
    const EVERY_ROLE: &str = r#"
[page]
on = "note"
where.file-type = ["md"]
fields.tags = { inlines = "tags", type = "tag" }

[para]
on = "paragraph"
fields.text = { rest = true }

[head]
on = "heading"
fields.title = { rest = true }

[line]
on = "list-item"
fields.what = { rest = true }

[row]
on = "table-row"
fields.links = { inlines = "links", type = "link" }

[said]
on = "quote"
fields.words = { rest = true }

[snippet]
on = "code"
fields.language = { info = "language" }

[word]
on = "inline"
match = '\bthe (?<w>\w+)'
fields.w = { capture = "w" }
"#;

    fn ask(item: &Item) -> Edit {
        Edit {
            id: item.id.clone(),
            rev: item.rev.clone(),
            op: Op::Anchor,
        }
    }

    /// The item of kind `kind` that starts on line `line` of the note `N.md` written `text`.
    fn at(text: &str, s: &Settings, line: usize, kind: &str) -> Item {
        let items = markdown::items("N.md", text, s);
        let found = items
            .iter()
            .find(|i| i.line == line && i.data.kind() == kind);
        let found = found.unwrap_or_else(|| panic!("no {kind} on line {line}: {items:#?}"));
        found.clone()
    }

    fn anchor(text: &str, s: &Settings, item: &Item) -> Result<Made, Error> {
        markdown::anchor("N.md", text, s, &ask(item), &|_| false)
    }

    /// What anchoring the item writes, and the name it took: the one the answer reads back.
    fn written(text: &str, s: &Settings, line: usize, kind: &str) -> (String, String) {
        let item = at(text, s, line, kind);
        let made = anchor(text, s, &item).unwrap_or_else(|e| panic!("{text:?}: {e}"));
        assert!(made.changed, "{text:?}");
        assert_eq!(made.what, "anchored", "{text:?}");
        let After::Set(back) = made.after else {
            panic!("{text:?}: no item read back")
        };
        let name = back
            .block_id
            .unwrap_or_else(|| panic!("{text:?}: no anchor"));
        (made.text, name)
    }

    /// Where the walk reads an item's anchor (`items.rs`): at the end of its last own line,
    /// the one before its first child, after every field, with one space before it. The
    /// blanks that end the line, and its line ending, stay at its end.
    #[test]
    fn the_name_goes_at_the_end_of_the_items_last_own_line() {
        let s = settings("obsidian");
        let cases = [
            (
                "- [ ] Pay rent 🔁 every month ⏳ 2026-11-10 📅 2026-11-12\n- [ ] Call Ana\n",
                "task",
                "- [ ] Pay rent 🔁 every month ⏳ 2026-11-10 📅 2026-11-12 ^NAME\n- [ ] Call Ana\n",
            ),
            (
                "- [ ] Pay rent [due:: 2026-11-12] [priority:: high]\n",
                "task",
                "- [ ] Pay rent [due:: 2026-11-12] [priority:: high] ^NAME\n",
            ),
            (
                "- Pay rent\n  by transfer\n  from savings\n  - Ask the bank\n",
                "list",
                "- Pay rent\n  by transfer\n  from savings ^NAME\n  - Ask the bank\n",
            ),
            (
                "- [ ] Pay rent\n  - [ ] Transfer\n",
                "task",
                "- [ ] Pay rent ^NAME\n  - [ ] Transfer\n",
            ),
            (
                "- [ ] Pay rent\r\n- [ ] Water\r\n",
                "task",
                "- [ ] Pay rent ^NAME\r\n- [ ] Water\r\n",
            ),
            (
                "- [ ] Pay rent  \n- [ ] Water\n",
                "task",
                "- [ ] Pay rent ^NAME  \n- [ ] Water\n",
            ),
            ("- [ ] Pay rent", "task", "- [ ] Pay rent ^NAME"),
        ];
        for (text, kind, want) in cases {
            let (new, name) = written(text, &s, 1, kind);
            assert_eq!(new, want.replace("NAME", &name), "{text:?}");
        }
        let callout = "> [!todo] Bills\n> - [ ] Pay rent\n> - [ ] Water\n";
        let (new, name) = written(callout, &s, 2, "task");
        let want = format!("> [!todo] Bills\n> - [ ] Pay rent ^{name}\n> - [ ] Water\n");
        assert_eq!(new, want, "a task in a callout, on its own line");
        let typed = with_types("obsidian", EVERY_ROLE);
        let para = "Line one\nline two\nline three\n\nAfter.\n";
        let (new, name) = written(para, &typed, 1, "para");
        let want = format!("Line one\nline two\nline three ^{name}\n\nAfter.\n");
        assert_eq!(new, want, "a paragraph a type reads, on its last line");
    }

    /// Asked twice, the second time writes nothing: an item that has an anchor keeps it.
    #[test]
    fn an_item_that_has_an_anchor_is_answered_as_it_is() {
        let s = settings("obsidian");
        for (text, kind) in [
            ("- [ ] Pay rent ^rent\n", "task"),
            ("- Pay rent\n  by transfer ^rent\n", "list"),
            ("A claim.\n^claim\n", "block"),
        ] {
            let item = at(text, &s, 1, kind);
            assert!(item.block_id.is_some(), "{text:?}");
            let Ok(made) = anchor(text, &s, &item) else {
                panic!("{text:?}: refused")
            };
            assert!(!made.changed, "{text:?}");
            assert_eq!(made.text, text);
            let After::Set(back) = made.after else {
                panic!("{text:?}: no item read back")
            };
            assert_eq!(back, item);
        }
    }

    /// Markdown has a place for a `^name` only where a list item's or a paragraph's lines
    /// end. Every other item is refused, in words that say what to keep instead.
    #[test]
    fn an_item_with_no_place_for_a_name_is_refused_saying_what_to_keep_instead() {
        let typed = with_types("obsidian", EVERY_ROLE);
        let text = "# Plan\n\nPay the rent on time.\n\n| Bill | Due |\n| --- | --- |\n\
                    | [[Rent]] | Friday |\n\n> Said in a quote.\n\n```sh\npay --rent\n```\n";
        let para = at(text, &typed, 3, "para");
        for (line, kind, says) in [
            (1, "page", "[[N]]"),
            (1, "head", "heading"),
            (3, "word", para.id.as_str()),
            (7, "row", "table row"),
            (9, "said", "quote"),
        ] {
            let item = at(text, &typed, line, kind);
            let Err(e) = anchor(text, &typed, &item) else {
                panic!("{kind}: anchored")
            };
            assert_eq!(e.reason, Reason::Refused, "{kind}: {e}");
            assert!(e.message.contains(says), "{kind}: {e}");
        }
        let plain = settings("obsidian");
        let heading = at("# Plan\n", &plain, 1, "heading");
        let Err(e) = anchor("# Plan\n", &plain, &heading) else {
            panic!("anchored")
        };
        assert_eq!(e.reason, Reason::Refused, "{e}");
        assert!(e.message.contains("[[N#Plan]]"), "{e}");
    }

    /// `github` reads no block ids: a `^name` there would be words.
    #[test]
    fn a_flavour_without_block_ids_is_refused() {
        let github = settings("github");
        let text = "- [ ] Pay rent\n";
        let item = at(text, &github, 1, "task");
        let Err(e) = anchor(text, &github, &item) else {
            panic!("anchored")
        };
        assert_eq!(e.reason, Reason::Refused, "{e}");
        assert!(e.message.contains("github"), "{e}");
        assert!(e.message.contains("block-ids"), "{e}");
    }

    #[test]
    fn a_stale_or_gone_item_is_changed() {
        let s = settings("obsidian");
        let text = "- [ ] Pay rent\n";
        let item = at(text, &s, 1, "task");
        let stale = Edit {
            rev: "000000000000".into(),
            ..ask(&item)
        };
        let gone = Edit {
            id: "0000000000000000".into(),
            ..ask(&item)
        };
        for req in [stale, gone] {
            let Err(e) = markdown::anchor("N.md", text, &s, &req, &|_| false) else {
                panic!("anchored")
            };
            assert_eq!(e.reason, Reason::Changed, "{e}");
        }
    }

    /// A name is taken when it is written anywhere, in another file or in this note: the
    /// next attempt is used.
    #[test]
    fn a_name_written_anywhere_is_passed_over_for_the_next() {
        let s = settings("obsidian");
        let text = "- [ ] Pay rent\n";
        let item = at(text, &s, 1, "task");
        let (first, second) = (identity::anchor(&item.id, 0), identity::anchor(&item.id, 1));
        let elsewhere = |name: &str| name == first;
        let Ok(made) = markdown::anchor("N.md", text, &s, &ask(&item), &elsewhere) else {
            panic!("refused")
        };
        let want = format!("- [ ] Pay rent ^{second}\n");
        assert_eq!(made.text, want, "taken in another file");
        let linked = format!("- [ ] Pay rent\n\nSee [[N#^{first}]].\n");
        let item = at(&linked, &s, 1, "task");
        assert_eq!(identity::anchor(&item.id, 0), first, "the same address");
        let Ok(made) = anchor(&linked, &s, &item) else {
            panic!("refused")
        };
        let want = format!("- [ ] Pay rent ^{second}\n\nSee [[N#^{first}]].\n");
        assert_eq!(made.text, want, "taken in this note");
    }

    /// What each item says: everything but its id, its rev and its parent's id, which an
    /// anchor may move by the key rules (its own, its children's, a later twin's); its parent
    /// by the line and the kind it starts on instead.
    fn said(items: &[Item]) -> Vec<Json> {
        let find = |id: &String| items.iter().find(|i| i.id == *id);
        (items.iter())
            .map(|i| {
                let mut json = serde_json::to_value(i).unwrap();
                let fields = json.as_object_mut().unwrap();
                fields.remove("id");
                fields.remove("rev");
                let parent = i.parent.as_ref().and_then(find);
                let parent = parent.map(|p| serde_json::json!([p.line, p.data.kind()]));
                fields.insert("parent".into(), parent.unwrap_or(Json::Null));
                json
            })
            .collect()
    }

    /// `said` in no order: items that start on one line may be read in another.
    fn sorted(said: Vec<Json>) -> Vec<String> {
        let mut said: Vec<String> = said.iter().map(Json::to_string).collect();
        said.sort();
        said
    }

    #[derive(Default)]
    struct Tally {
        anchored: usize,
        kept: usize,
        refused: usize,
        unread: Vec<String>,
        moved: Vec<String>,
    }

    /// Anchor each item of the note alone, and check what came of it apart from the
    /// read-back: the text is the old one with ` ^name` in one place, and the items say what
    /// they said, but that one's `block_id`.
    fn each_item(note: &str, text: &str, s: &Settings, tally: &mut Tally) {
        let before = markdown::items(note, text, s);
        for (k, item) in before.iter().enumerate() {
            let at = format!("{note}:{} {}", item.line, item.data.kind());
            let made = match markdown::anchor(note, text, s, &ask(item), &|_| false) {
                Err(e) if e.reason == Reason::Refused => {
                    assert!(!e.message.is_empty(), "{at}");
                    tally.refused += 1;
                    continue;
                }
                Err(e) if e.reason == Reason::ReadBack => {
                    let tree = e.message.contains("ends no text a ^name can end");
                    assert!(tree, "{at}: refused by the items' read-back: {e}");
                    tally.unread.push(at);
                    continue;
                }
                Err(e) => panic!("{at}: {e}"),
                Ok(made) => made,
            };
            if !made.changed {
                assert!(item.block_id.is_some(), "{at}: unchanged, with no anchor");
                assert_eq!(made.text, text, "{at}");
                tally.kept += 1;
                continue;
            }
            assert_eq!(item.block_id, None, "{at}: it had one");
            let After::Set(back) = &made.after else {
                panic!("{at}: no item read back")
            };
            let name = back
                .block_id
                .clone()
                .unwrap_or_else(|| panic!("{at}: no anchor"));
            let tag = format!("^{name}");
            assert!(!text.contains(&tag), "{at}: {tag} was in the note already");
            assert_eq!(made.text.matches(&tag).count(), 1, "{at}");
            let unwritten = made.text.replacen(&format!(" {tag}"), "", 1);
            assert_eq!(unwritten, text, "{at}: more than ` {tag}` was written");
            let mut want = said(&before);
            want[k]["block_id"] = Json::from(name.as_str());
            let after = markdown::items(note, &made.text, s);
            let is = sorted(said(&after));
            assert_eq!(is, sorted(want), "{at}: the note reads otherwise");
            let moved = ids_moved_beyond_the_key_rules(&before, &after, k).into_iter();
            tally.moved.extend(moved.map(|by| format!("{at}: {by}")));
            tally.anchored += 1;
        }
    }

    /// The items whose ids moved beyond what the design note says the key rules move
    /// (`items::identify`): the anchored item's, a later twin's (an item that says the same
    /// under the same parent, after it), and the ids of the items under either. Each item is
    /// paired with the one of its kind that is the same nth on its line after.
    fn ids_moved_beyond_the_key_rules(before: &[Item], after: &[Item], k: usize) -> Vec<String> {
        let data = |i: &Item| serde_json::to_value(&i.data).unwrap();
        let parent = |i: &Item| before.iter().position(|p| Some(&p.id) == i.parent.as_ref());
        let twin = |i: usize| {
            let (b, a) = (&before[i], &before[k]);
            i > k && data(b) == data(a) && b.parent == a.parent
        };
        let mut may = vec![false; before.len()];
        let mut moved = Vec::new();
        for (i, b) in before.iter().enumerate() {
            may[i] = i == k || twin(i) || parent(b).is_some_and(|p| may[p]);
            let on_line = |x: &&Item| x.line == b.line && x.data.kind() == b.data.kind();
            let nth = before[..i].iter().filter(on_line).count();
            let paired = after.iter().filter(on_line).nth(nth).expect("paired");
            if !may[i] && paired.id != b.id {
                moved.push(format!("the {} on line {}", b.data.kind(), b.line));
            }
        }
        moved
    }

    /// The design's oracle (#190), in the style of the pulldown-cmark ones: every item of
    /// every tricky note, with types on every role, and of every note of the example
    /// workspace, anchored one at a time, is refused with a reason, or the note reads back
    /// with nothing new but that item's anchor. The tree's read-back refuses the items whose
    /// last own line ends no text, and no others: an item that ends in indented code, one
    /// that ends in a setext heading's underline, one that ends in a closing fence, a
    /// callout's title line with no words, three empty list items, two empty tasks, and a
    /// list item that is a link definition.
    #[test]
    fn every_item_of_the_corpus_takes_an_anchor_alone_or_is_refused_with_a_reason() {
        let mut tally = Tally::default();
        let typed = with_types("obsidian", EVERY_ROLE);
        for (k, text) in TRICKY.iter().enumerate() {
            each_item(&format!("Tricky {k}.md"), text, &typed, &mut tally);
        }
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../example-workspace");
        let s = Settings::load(&root);
        for e in walkdir::WalkDir::new(&root).into_iter().flatten() {
            let path = e.path();
            if path.extension().is_some_and(|x| x == "md") {
                let note = path.strip_prefix(&root).unwrap().to_string_lossy();
                let text = std::fs::read_to_string(path).unwrap();
                each_item(&note, &text, &s, &mut tally);
            }
        }
        let Tally {
            anchored,
            kept,
            refused,
            mut unread,
            moved,
        } = tally;
        let counts = format!("{anchored} anchored, {kept} kept, {refused} refused");
        assert!(anchored > 100 && kept > 40 && refused > 150, "{counts}");
        unread.sort();
        assert_eq!(
            unread,
            [
                "Tricky 16.md:3 line",
                "Tricky 16.md:4 line",
                "Tricky 16.md:6 line",
                "Tricky 2.md:6 line",
                "Tricky 21.md:1 line",
                "Tricky 21.md:2 line",
                "Tricky 24.md:4 line",
                "Tricky 6.md:11 line",
                "Tricky 8.md:9 line",
                "Tricky 9.md:12 para",
            ],
            "{counts}"
        );
        assert!(
            moved.is_empty(),
            "{counts}; {} anchors moved an id beyond the key rules:\n{}",
            moved.len(),
            moved.join("\n")
        );
    }
}
