use super::*;
use crate::items::extract;
use crate::markdown::Flavour;
use crate::tasks::TasksSettings;
use crate::types::{Layer, load};
use serde_json::json;

fn settings(toml: &str) -> Settings {
    let table: toml::Table = toml::from_str(toml).unwrap();
    let found = load(vec![
        Layer::built_in(),
        Layer::config(Path::new("config.toml"), table),
    ]);
    assert!(found.errors.is_empty(), "{:?}", found.errors);
    let nowhere = Path::new("/nonexistent-cold-lava-workspace");
    Settings::new(
        Flavour::built_in("obsidian").unwrap(),
        TasksSettings::load(nowhere),
        daily::Settings::load(nowhere),
        crate::templates::Settings::default(),
        found,
    )
}

/// Sets `field` of the `kind` item on `line` of `Note.md`, at its current rev.
/// Obsidian's style, with the Tasks plugin's format from `s` as the workspace would have it.
fn style_of(s: &Settings) -> Style {
    let mut style = Style::default();
    if s.tasks.dataview {
        style.task_fields.value = TaskFields::Dataview;
    }
    style
}

fn set(
    s: &Settings,
    text: &str,
    line: usize,
    kind: &str,
    field: &str,
    value: Json,
) -> Result<Set, EditError> {
    let all = extract("Note.md", text, s);
    let item = all
        .iter()
        .find(|i| i.line == line && i.data.kind() == kind)
        .unwrap_or_else(|| panic!("no {kind} on line {line} in {all:#?}"));
    plan(
        "Note.md",
        text,
        s,
        &style_of(s).into(),
        &SetField {
            id: &item.id,
            rev: Some(&item.rev),
            field,
            value,
        },
    )
}

fn after(s: &Settings, text: &str, line: usize, kind: &str, field: &str, value: Json) -> String {
    match set(s, text, line, kind, field, value) {
        Ok(set) => set.text,
        Err(e) => panic!("{e}"),
    }
}

fn refusal(
    s: &Settings,
    text: &str,
    line: usize,
    kind: &str,
    field: &str,
    value: Json,
) -> EditError {
    match set(s, text, line, kind, field, value) {
        Ok(set) => panic!("not refused:\n{}", set.text),
        Err(e) => e,
    }
}

const CHORES: &str = r#"
[chore]
on = "list-item"
key = ["what"]
fields.due = { marker = "📅", type = "date" }
fields.start = { marker = "🛫", type = "date", format = "DD/MM/YYYY" }
fields.who = { inline = "who" }
fields.urgent = { flag = "❗", type = "bool" }
fields.size = { flags = { "🐘" = "big", "🐭" = "small" } }
fields.minutes = { pattern = '~(\S+)', type = "duration" }
fields.state = { inline = "state", type = "enum", values = { todo = "todo", done = "done", x = "done" } }
fields.what = { rest = true }
"#;

#[test]
fn a_marker_is_replaced_added_and_taken_out() {
    let s = settings(CHORES);
    let t = "- Sweep 📅 2026-11-12 ❗\n- Rake ^rake\n";
    assert_eq!(
        after(&s, t, 1, "chore", "due", json!("2026-12-01")),
        "- Sweep 📅 2026-12-01 ❗\n- Rake ^rake\n",
        "only the value's bytes change"
    );
    assert_eq!(
        after(&s, t, 2, "chore", "due", json!("2026-12-01")),
        "- Sweep 📅 2026-11-12 ❗\n- Rake 📅 2026-12-01 ^rake\n",
        "added after the text, before the block id"
    );
    assert_eq!(
        after(&s, t, 1, "chore", "due", Json::Null),
        "- Sweep ❗\n- Rake ^rake\n",
        "the glyph goes with its value"
    );
    assert_eq!(
        after(&s, t, 1, "chore", "start", json!("2026-11-03")),
        "- Sweep 📅 2026-11-12 ❗ 🛫 03/11/2026\n- Rake ^rake\n",
        "a date is written in its field's format"
    );
    assert_eq!(
        after(&s, "- Rake 📅\n", 1, "chore", "due", json!("2026-12-01")),
        "- Rake 📅 2026-12-01\n",
        "a glyph without a value gets one"
    );
}

#[test]
fn dataview_fields_keep_their_brackets() {
    let s = settings(CHORES);
    for (line, want) in [
        ("- Sweep [who:: Ana]", "- Sweep [who:: Bo]"),
        ("- Sweep (who:: Ana)", "- Sweep (who:: Bo)"),
        ("- who:: Ana", "- who:: Bo"),
        ("- Sweep [who::Ana] ❗", "- Sweep [who::Bo] ❗"),
    ] {
        assert_eq!(after(&s, line, 1, "chore", "who", json!("Bo")), want);
    }
    assert_eq!(
        after(&s, "- Sweep (state:: todo)", 1, "chore", "who", json!("Bo")),
        "- Sweep (state:: todo) (who:: Bo)",
        "a new field is written like the line's others"
    );
    assert_eq!(
        after(&s, "- Sweep", 1, "chore", "who", json!("Bo")),
        "- Sweep [who:: Bo]"
    );
    for (line, want) in [
        ("- Sweep [who:: Ana] ❗", "- Sweep ❗"),
        ("- Sweep (who:: Ana)", "- Sweep"),
    ] {
        assert_eq!(after(&s, line, 1, "chore", "who", Json::Null), want);
    }
}

#[test]
fn flags_and_glyph_choices() {
    let s = settings(CHORES);
    assert_eq!(
        after(&s, "- Sweep", 1, "chore", "urgent", json!(true)),
        "- Sweep ❗"
    );
    assert_eq!(
        after(&s, "- Sweep ❗ now", 1, "chore", "urgent", json!("false")),
        "- Sweep now"
    );
    assert!(matches!(
        refusal(&s, "- Sweep ❗", 1, "chore", "urgent", Json::Null),
        Refused(_)
    ));
    assert_eq!(
        after(&s, "- Sweep 🐘", 1, "chore", "size", json!("small")),
        "- Sweep 🐭"
    );
    assert_eq!(
        after(&s, "- Sweep", 1, "chore", "size", json!("big")),
        "- Sweep 🐘"
    );
    assert_eq!(
        after(&s, "- Sweep 🐘 now", 1, "chore", "size", json!("")),
        "- Sweep now"
    );
    assert!(matches!(
        refusal(&s, "- Sweep", 1, "chore", "size", json!("huge")),
        Refused(_)
    ));
}

#[test]
fn values_are_written_by_type_the_way_the_note_writes_them() {
    let s = settings(CHORES);
    assert_eq!(
        after(&s, "- Sweep ~1h30m", 1, "chore", "minutes", json!(45)),
        "- Sweep ~45m"
    );
    assert_eq!(
        after(&s, "- Sweep ~1h", 1, "chore", "minutes", json!("2h15m")),
        "- Sweep ~2h15m"
    );
    assert_eq!(
        after(&s, "- Sweep ~90m", 1, "chore", "minutes", json!(120)),
        "- Sweep ~120m",
        "minutes alone stay minutes alone"
    );
    let t = "- Sweep [state:: x]\n- Rake [state:: todo]\n";
    assert_eq!(
        after(&s, t, 2, "chore", "state", json!("done")),
        "- Sweep [state:: x]\n- Rake [state:: x]\n",
        "an enum is written in the form the note already uses"
    );
    assert_eq!(
        after(
            &s,
            "- Rake [state:: todo]",
            1,
            "chore",
            "state",
            json!("done")
        ),
        "- Rake [state:: done]",
        "else its first form"
    );
    assert_eq!(
        after(&s, "- Rake [state:: todo]", 1, "chore", "state", json!("x")),
        "- Rake [state:: done]",
        "a written form is read as its name"
    );
    assert!(matches!(
        refusal(&s, "- Rake", 1, "chore", "due", json!("someday")),
        Refused(_)
    ));
}

#[test]
fn patterns_replace_but_never_add_or_take_out() {
    let s = settings(CHORES);
    assert!(matches!(
        refusal(&s, "- Sweep", 1, "chore", "minutes", json!(30)),
        Refused(_)
    ));
    assert!(matches!(
        refusal(&s, "- Sweep ~30m", 1, "chore", "minutes", Json::Null),
        Refused(_)
    ));
}

#[test]
fn read_only_sources_are_refused() {
    let s = settings(
        r#"
[thing]
on = "list-item"
fields.what = { rest = true }
fields.tags = { inlines = "tags", type = "tag" }
fields.name = { note = "name" }
fields.under = { section = "heading" }
"#,
    );
    for field in ["what", "tags", "name", "under"] {
        let e = refusal(&s, "# H\n- Sweep #x", 2, "thing", field, json!("y"));
        assert!(
            matches!(&e, Refused(m) if m.contains("can't be written")),
            "{field}: {e:?}"
        );
    }
    assert!(matches!(
        refusal(&s, "- Sweep", 1, "thing", "nope", json!("y")),
        Refused(_)
    ));
}

#[test]
fn a_task_is_set_through_its_checkbox_and_markers() {
    let s = settings("");
    let t = "- [x] Pay rent 📅 2026-11-12 ✅ 2026-11-10\n";
    assert_eq!(
        after(&s, t, 1, "task", "status", json!(" ")),
        "- [ ] Pay rent 📅 2026-11-12 ✅ 2026-11-10\n",
        "a space is a status, and its type changes with it"
    );
    assert_eq!(
        after(
            &s,
            "- [ ] Pay rent\n",
            1,
            "task",
            "status_type",
            json!("done")
        ),
        "- [x] Pay rent\n"
    );
    assert_eq!(
        after(&s, t, 1, "task", "status_type", json!("in_progress")),
        "- [/] Pay rent 📅 2026-11-12 ✅ 2026-11-10\n"
    );
    assert_eq!(
        after(&s, t, 1, "task", "due", json!("2026-12-01")),
        "- [x] Pay rent 📅 2026-12-01 ✅ 2026-11-10\n"
    );
    assert_eq!(
        after(&s, t, 1, "task", "done", Json::Null),
        "- [x] Pay rent 📅 2026-11-12\n"
    );
    let selector = "- [ ] Pay rent ⏳\u{FE0F} 2026-11-05 📅 2026-11-12\n";
    assert_eq!(
        after(&s, selector, 1, "task", "scheduled", Json::Null),
        "- [ ] Pay rent 📅 2026-11-12\n",
        "a variation selector goes with its glyph"
    );
    assert_eq!(
        after(
            &s,
            "- [ ] Dentist [due:: 2026-11-12]\n",
            1,
            "task",
            "due",
            json!("2026-11-13")
        ),
        "- [ ] Dentist [due:: 2026-11-13]\n"
    );
    assert!(matches!(
        refusal(&s, t, 1, "task", "status", Json::Null),
        Refused(_)
    ));
}

#[test]
fn a_missing_task_date_is_written_as_the_line_and_the_workspace_write_them() {
    let s = settings("");
    assert_eq!(
        after(
            &s,
            "- [ ] Pay rent\n",
            1,
            "task",
            "due",
            json!("2026-12-01")
        ),
        "- [ ] Pay rent 📅 2026-12-01\n",
        "the Tasks plugin's default format is emoji"
    );
    assert_eq!(
        after(
            &s,
            "- [ ] Pay rent [scheduled:: 2026-11-30]\n",
            1,
            "task",
            "due",
            json!("2026-12-01")
        ),
        "- [ ] Pay rent [scheduled:: 2026-11-30] [due:: 2026-12-01]\n",
        "a line that writes Dataview fields gets one"
    );
    assert_eq!(
        after(
            &s,
            "- [ ] Pay rent ⏫\n",
            1,
            "task",
            "due",
            json!("2026-12-01")
        ),
        "- [ ] Pay rent ⏫ 📅 2026-12-01\n"
    );
    let mut dataview = settings("");
    dataview.tasks.dataview = true;
    assert_eq!(
        after(
            &dataview,
            "- [ ] Pay rent\n",
            1,
            "task",
            "due",
            json!("2026-12-01")
        ),
        "- [ ] Pay rent [due:: 2026-12-01]\n"
    );
}

#[test]
fn a_stale_rev_or_a_gone_item_is_changed() {
    let s = settings(CHORES);
    let text = "- Sweep 📅 2026-11-12\n";
    let item = &extract("Note.md", text, &s)[0];
    let req = |id: &str, rev: &str| {
        plan(
            "Note.md",
            text,
            &s,
            &Style::default().into(),
            &SetField {
                id,
                rev: Some(rev),
                field: "due",
                value: json!("2026-12-01"),
            },
        )
    };
    assert!(matches!(req(&item.id, "stale"), Err(Changed(_))));
    assert!(matches!(req("0000", &item.rev), Err(Changed(_))));
    assert!(req(&item.id, &item.rev).is_ok());
}

#[test]
fn what_doesnt_read_back_is_refused() {
    let s = settings(
        r#"
[chore]
on = "list-item"
key = ["what"]
fields.who = { marker = "👤" }
fields.minutes = { pattern = '~(\S+)', type = "duration" }
fields.what = { rest = true }

[vei]
on = "inline"
match = '\bVEI (?<n>\d)\b'
fields.n = { capture = "n", type = "number" }
"#,
    );
    let e = refusal(&s, "- Sweep 👤 Ana", 1, "chore", "who", json!("Ana Bo"));
    assert!(
        matches!(&e, ReadBack(m) if m.contains("\"Ana\"")),
        "a marker's value ends at a space: {e:?}"
    );
    let e = refusal(&s, "- Sweep 👤 Ana ~1h", 1, "chore", "who", json!("~5m"));
    assert!(
        matches!(&e, ReadBack(m) if m.contains("minutes would change")),
        "another field would read the new text: {e:?}"
    );
    let spaced = settings(
        r#"
[chore]
on = "list-item"
fields.start = { marker = "🛫", type = "date", format = "D MMMM YYYY" }
"#,
    );
    let e = refusal(&spaced, "- Sweep", 1, "chore", "start", json!("2026-11-03"));
    assert!(
        matches!(&e, ReadBack(m) if m.contains("start would read null")),
        "a marker's value can't hold a space, so this format can't be written: {e:?}"
    );
}

#[test]
fn a_new_item_in_the_value_is_refused() {
    let s = settings(
        r#"
[chore]
on = "list-item"
key = ["what"]
fields.who = { inline = "who" }
fields.what = { rest = true }

[vei]
on = "inline"
match = '\bVEI (?<n>\d)\b'
fields.n = { capture = "n", type = "number" }
"#,
    );
    let e = refusal(&s, "- Sweep [who:: Ana]", 1, "chore", "who", json!("VEI 4"));
    assert!(matches!(&e, ReadBack(m) if m.contains("items")), "{e:?}");
}

const ERUPTION: &str = r#"
[eruption]
fields = { vei = "number", example = "text" }
[[eruption.from]]
on = "table-row"
where.header = ["VEI", "Example"]
key = ["vei"]
fields = { vei = { column = "VEI" }, example = { column = "Example" } }
"#;

const TABLE: &str = "\
| VEI | Example          |
| --- | ---------------- |
| 4   | Eyjafjallajökull |
| 5   |                  |
| 6   | Krakatoa         |
";

#[test]
fn a_cell_is_filled_without_padding_the_table_again() {
    let s = settings(ERUPTION);
    assert_eq!(
        after(&s, TABLE, 4, "eruption", "example", json!("St. Helens")),
        TABLE.replace("| 5   |                  |", "| 5   | St. Helens       |"),
        "an empty cell keeps its width"
    );
    assert_eq!(
        after(
            &s,
            TABLE,
            5,
            "eruption",
            "example",
            json!("Krakatoa, 1883 and after")
        ),
        TABLE.replace("| Krakatoa         |", "| Krakatoa, 1883 and after |"),
        "a longer value widens its own row only"
    );
    assert_eq!(
        after(&s, TABLE, 3, "eruption", "example", Json::Null),
        TABLE.replace("| Eyjafjallajökull |", "|                  |"),
        "taken out, the cell keeps its width"
    );
    assert!(matches!(
        refusal(&s, TABLE, 3, "eruption", "example", json!("a | b")),
        Refused(_)
    ));
}

#[test]
fn a_key_field_gives_the_item_a_new_id() {
    let s = settings(ERUPTION);
    let before = extract("Note.md", TABLE, &s);
    let vei = set(&s, TABLE, 3, "eruption", "vei", json!(3)).unwrap();
    assert!(vei.text.contains("| 3   | Eyjafjallajökull |"));
    assert_ne!(vei.item.id, before[0].id, "vei is the row's key");
    assert_eq!(vei.item, extract("Note.md", &vei.text, &s)[0]);
    let example = set(&s, TABLE, 3, "eruption", "example", json!("Hekla")).unwrap();
    assert_eq!(
        example.item.id, before[0].id,
        "a field outside the key keeps the id"
    );
}

const VOLCANO: &str = r#"
[volcano]
on = "note"
where.tag = ["volcano"]
fields.title = { property = "title", note = "name" }
fields.country = { property = "country" }
fields.elevation = { property = "elevation_m", type = "number" }
fields.near = { property = "near", type = "link" }
"#;

#[test]
fn properties_are_edited_line_by_line() {
    let s = settings(VOLCANO);
    let t = "---\ntags: [volcano]\ncountry: Indonesia\nelevation_m: \"813\"\n---\n# Krakatoa\n";
    assert_eq!(
        after(&s, t, 1, "volcano", "country", json!("Sunda Strait")),
        t.replace("Indonesia", "Sunda Strait")
    );
    assert_eq!(
        after(&s, t, 1, "volcano", "elevation", json!(155)),
        t.replace("\"813\"", "\"155\""),
        "quotes stay as written"
    );
    assert_eq!(
        after(&s, t, 1, "volcano", "near", json!("Anak Krakatau")),
        t.replace("---\n#", "near: \"[[Anak Krakatau]]\"\n---\n#"),
        "a new property is a line at the end, quoted when YAML needs it"
    );
    assert_eq!(
        after(&s, t, 1, "volcano", "country", Json::Null),
        t.replace("country: Indonesia\n", "")
    );
    assert_eq!(
        after(&s, t, 1, "volcano", "title", json!("Krakatau")),
        t.replace("---\n#", "title: Krakatau\n---\n#"),
        "a field read from the note's name is written to the property before it"
    );
    let linked = "---\ntags: [volcano]\nnear: \"[[Krakatau|K]]\"\n---\n";
    assert_eq!(
        after(&s, linked, 1, "volcano", "near", json!("Rakata")),
        "---\ntags: [volcano]\nnear: \"[[Rakata]]\"\n---\n",
        "a link stays a link"
    );
    let empty = "---\ntags: [volcano]\ncountry:\n---\n";
    assert_eq!(
        after(&s, empty, 1, "volcano", "country", json!("Chile")),
        "---\ntags: [volcano]\ncountry: Chile\n---\n"
    );
    let one = "---\ntags: [volcano]\ncountry:\n  - Chile\n---\n";
    assert_eq!(
        after(&s, one, 1, "volcano", "country", json!("Peru")),
        "---\ntags: [volcano]\ncountry:\n  - Peru\n---\n"
    );
    let crlf = "---\r\ntags: [volcano]\r\n---\r\nText\r\n";
    assert_eq!(
        after(&s, crlf, 1, "volcano", "country", json!("Chile")),
        "---\r\ntags: [volcano]\r\ncountry: Chile\r\n---\r\nText\r\n"
    );
}

#[test]
fn nested_and_listed_yaml_is_refused() {
    let s = settings(VOLCANO);
    let nested = "---\ntags: [volcano]\ncountry:\n  name: Chile\n---\n";
    let e = refusal(&s, nested, 1, "volcano", "country", json!("Peru"));
    assert!(matches!(&e, Refused(m) if m.contains("nested")), "{e:?}");
    let two = "---\ntags: [volcano]\ncountry: [Chile, Peru]\n---\n";
    assert!(matches!(
        refusal(&s, two, 1, "volcano", "country", json!("Peru")),
        Refused(_)
    ));
}

#[test]
fn frontmatter_is_made_when_a_note_has_none() {
    let s = settings(
        r#"
[place]
on = "note"
where.folder = ["Places"]
fields.country = { property = "country" }
fields.status = { inline = "status" }
"#,
    );
    let t = "# Hekla\nText.\n";
    let all = extract("Places/Hekla.md", t, &s);
    let id = &all.iter().find(|i| i.data.kind() == "place").unwrap().id;
    let set = |field, value| {
        plan(
            "Places/Hekla.md",
            t,
            &s,
            &Style::default().into(),
            &SetField {
                id,
                rev: None,
                field,
                value,
            },
        )
        .unwrap_or_else(|e| panic!("{e}"))
        .text
    };
    assert_eq!(
        set("country", json!("Iceland")),
        "---\ncountry: Iceland\n---\n# Hekla\nText.\n"
    );
    assert_eq!(
        set("status", json!("active")),
        "status:: active\n# Hekla\nText.\n",
        "a note's Dataview field is a line of its own"
    );
}

#[test]
fn captures_in_inline_items_and_headings() {
    let s = settings(
        r#"
[vei]
on = "inline"
match = '\bVEI (?<n>\d)\b'
fields.n = { capture = "n", type = "number" }

[chapter]
on = "heading"
match = '^Chapter (?<n>\d+)'
fields.n = { capture = "n", type = "number" }
fields.title = { rest = true }
"#,
    );
    let t = "Its eruption (VEI 6) and another (VEI 4).\n";
    assert_eq!(
        after(&s, t, 1, "vei", "n", json!(5)),
        "Its eruption (VEI 5) and another (VEI 4).\n"
    );
    let book = "# Chapter 3 The Ash\n- one\n## Part\n- two\n";
    assert_eq!(
        after(&s, book, 1, "chapter", "n", json!(4)),
        "# Chapter 4 The Ash\n- one\n## Part\n- two\n",
        "the items under a heading get its new name as their section"
    );
}

#[test]
fn the_same_value_changes_nothing() {
    let s = settings(CHORES);
    let same = set(
        &s,
        "- Sweep 📅 2026-11-12",
        1,
        "chore",
        "due",
        json!("2026-11-12"),
    )
    .unwrap();
    assert!(!same.changed);
    assert_eq!(same.text, "- Sweep 📅 2026-11-12");
}

#[test]
fn set_field_writes_only_when_the_file_is_as_read() {
    let dir = std::env::temp_dir().join(format!("cold-lava-edit-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join(".cold-lava")).unwrap();
    std::fs::write(
        dir.join(".cold-lava/types.toml"),
        format!(
            "version = 1\n[types.{}",
            ERUPTION.trim_start().trim_start_matches('[')
        )
        .replace("[[eruption.from]]", "[[types.eruption.from]]"),
    )
    .unwrap();
    std::fs::write(dir.join("VEI.md"), TABLE).unwrap();
    let s = Settings::load(&dir);
    assert!(s.types.errors.is_empty(), "{:?}", s.types.errors);
    let row = extract("VEI.md", TABLE, &s)[1].clone();
    let req = SetField {
        id: &row.id,
        rev: Some(&row.rev),
        field: "example",
        value: json!("Pinatubo"),
    };
    let dry = set_field(&dir, "VEI.md", &req, true).unwrap();
    assert!(dry.changed);
    assert_eq!(std::fs::read_to_string(dir.join("VEI.md")).unwrap(), TABLE);
    set_field(&dir, "VEI.md", &req, false).unwrap();
    let now = std::fs::read_to_string(dir.join("VEI.md")).unwrap();
    assert_eq!(now, dry.text);
    assert!(
        matches!(set_field(&dir, "VEI.md", &req, false), Err(Changed(_))),
        "the row's rev is stale now"
    );
    std::fs::remove_dir_all(&dir).unwrap();
}

/// A field read from an attribute is written where the file's owner read it: a checkbox's
/// character, a callout's kind. One the file doesn't write as text (a heading's level) can't
/// be set, and none can be taken out.
#[test]
fn an_attribute_is_changed_where_its_owner_read_it() {
    let s = settings(
        "[box]\non = \"item\"\npriority = 2\nwhere.attribute = \"checkbox\"\n\
         fields.state = { attribute = \"checkbox\" }\nfields.words = { rest = true }\n\
         [note-box]\non = \"aside\"\nfields.kind = { attribute = \"callout\" }\n\
         [chapter]\non = \"section\"\nfields.depth = { attribute = \"level\", type = \"number\" }\n",
    );
    let text = "# One\n- [ ] sweep\n\n> [!NOTE]- Careful\n> hot\n";
    let ticked = text.replace("[ ]", "[/]");
    assert_eq!(after(&s, text, 2, "box", "state", json!("/")), ticked);
    assert_eq!(after(&s, &ticked, 2, "box", "state", json!(" ")), text);
    assert_eq!(
        after(&s, text, 4, "note-box", "kind", json!("tip")),
        text.replace("NOTE", "tip")
    );
    let gone = refusal(&s, text, 2, "box", "state", Json::Null);
    assert!(
        matches!(&gone, Refused(m) if m.contains("attribute checkbox") && m.contains("taken out")),
        "{gone:?}"
    );
    let deeper = refusal(&s, text, 1, "chapter", "depth", json!(3));
    assert!(
        matches!(&deeper, Refused(m) if m.contains("attribute level") && m.contains("added")),
        "{deeper:?}"
    );
}
