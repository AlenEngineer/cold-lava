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
        // In a callout, on the task's own line.
        let callout = "> [!todo] Bills\n> - [ ] Pay rent\n> - [ ] Water\n";
        let (new, name) = written(callout, &s, 2, "task");
        let want = format!("> [!todo] Bills\n> - [ ] Pay rent ^{name}\n> - [ ] Water\n");
        assert_eq!(new, want);
        // A paragraph a type reads, of three lines.
        let typed = with_types("obsidian", EVERY_ROLE);
        let para = "Line one\nline two\nline three\n\nAfter.\n";
        let (new, name) = written(para, &typed, 1, "para");
        let want = format!("Line one\nline two\nline three ^{name}\n\nAfter.\n");
        assert_eq!(new, want);
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
            (11, "snippet", "code block"),
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

    /// What each item says, in order: everything but its id, its rev and its parent's id,
    /// which an anchor may move by the key rules (its own, its children's, a later twin's);
    /// its parent by place instead.
    fn said(items: &[Item]) -> Vec<Json> {
        let place = |id: &String| items.iter().position(|i| i.id == *id);
        (items.iter())
            .map(|i| {
                let mut json = serde_json::to_value(i).unwrap();
                let fields = json.as_object_mut().unwrap();
                fields.remove("id");
                fields.remove("rev");
                let parent = i.parent.as_ref().and_then(place);
                fields.insert("parent".into(), Json::from(parent));
                json
            })
            .collect()
    }

    #[derive(Default)]
    struct Tally {
        anchored: usize,
        kept: usize,
        refused: usize,
        unread: Vec<String>,
    }

    /// Anchor each item of the note alone, and check what came of it apart from the
    /// read-back: the text is the old one with ` ^name` in one place, and every item says
    /// what it said, in the same order, but that one's `block_id`.
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
                    tally.unread.push(format!("{at}: {e}"));
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
            assert_eq!(said(&after), want, "{at}: the note reads otherwise");
            tally.anchored += 1;
        }
    }

    /// The design's oracle (#190), in the style of the pulldown-cmark ones: every item of
    /// every tricky note, with types on every role, and of every note of the example
    /// workspace, anchored one at a time, is refused with a reason, or the note reads back
    /// with nothing new but that item's anchor.
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
            unread,
        } = tally;
        let counts = format!("{anchored} anchored, {kept} kept, {refused} refused");
        assert!(anchored > 100 && kept > 10 && refused > 50, "{counts}");
        assert!(
            unread.is_empty(),
            "{counts}; {} refused as read-backs:\n{}",
            unread.len(),
            unread.join("\n")
        );
    }
}
