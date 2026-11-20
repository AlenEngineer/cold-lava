//! The command line and files an extension owns: `items`, `agenda` and `table` read them from
//! what their owner gave before, and `rename` starts the owner to relink them. Each test has its
//! own home with csv-table installed, so neither your extensions nor your cache take part.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let dest = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &dest);
        } else {
            std::fs::copy(entry.path(), dest).unwrap();
        }
    }
}

fn has_python() -> bool {
    let python = Command::new("python3").arg("--version").output();
    python.is_ok_and(|o| o.status.success())
}

/// A copy of the example workspace, and a home with csv-table installed, its `main.py` changed
/// by `patch`.
fn sandbox(name: &str, patch: impl Fn(String) -> String) -> (PathBuf, PathBuf) {
    let dir =
        Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("owned-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    copy_dir(
        &manifest.join("../../example-workspace"),
        &dir.join("example-workspace"),
    );
    let home = dir.join("home");
    let installed = home.join(".config/cold-lava/extensions/csv-table");
    copy_dir(
        &manifest.join("../../examples/extensions/csv-table"),
        &installed,
    );
    let main = std::fs::read_to_string(installed.join("main.py")).unwrap();
    std::fs::write(installed.join("main.py"), patch(main)).unwrap();
    (dir.join("example-workspace"), home)
}

fn cold_lava(home: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_cold-lava"))
        .env("HOME", home)
        .env("XDG_CONFIG_HOME", home.join(".config"))
        .env("XDG_CACHE_HOME", home.join(".cache"))
        .env("COLD_LAVA_TODAY", "2026-11-07")
        .args(args)
        .output()
        .unwrap()
}

fn text(out: &Output) -> (String, String) {
    (
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

/// Every file under `dir` with its bytes, by path.
fn files_under(dir: &Path) -> Vec<(PathBuf, Vec<u8>)> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        match path.is_dir() {
            true => out.extend(files_under(&path)),
            false => out.push((path.clone(), std::fs::read(&path).unwrap())),
        }
    }
    out.sort();
    out
}

#[test]
fn the_cli_reads_owned_files_from_the_cache_and_renames_through_their_owner() {
    if !has_python() {
        eprintln!("skipped: python3 isn't on PATH, and the example extension needs it");
        return;
    }
    let (workspace, home) = sandbox("cli", |main| main);
    let ws = workspace.to_str().unwrap();

    let (out, err) = text(&cold_lava(&home, &["items", ws, "task"]));
    assert!(!out.contains("Eruptions.csv"), "{out}");
    assert!(
        err.contains("Volcanoes/Eruptions.csv is left out: csv-table hasn't read it"),
        "{err}"
    );

    let renamed = cold_lava(&home, &["rename", ws, "Krakatoa", "Krakatau"]);
    let (out, err) = text(&renamed);
    assert!(renamed.status.success(), "{out}{err}");
    assert!(out.contains("Volcanoes/Eruptions.csv"), "{out}");
    assert_eq!(
        std::fs::read_to_string(workspace.join("Volcanoes/Eruptions.csv")).unwrap(),
        "Volcano (link),Year,VEI,Notes (markdown)\nVesuvius,79,5,\n\
         Krakatau,1883,6,- [ ] Read the Royal Society report on [[Krakatau]] 📅 2026-11-20\n\
         Mount St. Helens,1980,5,\nEyjafjallajökull,2010,4,\n",
        "csv-table relinked the CSV"
    );

    // What csv-table read is in the cache now: the CLI lists the CSV's items without it.
    let (out, err) = text(&cold_lava(&home, &["items", ws, "task"]));
    assert!(
        out.contains(
            "Volcanoes/Eruptions.csv:1  [ ] Read the Royal Society report on [[Krakatau]]"
        ),
        "{out}"
    );
    assert!(err.is_empty(), "{err}");
    let args = ["agenda", ws, "--from", "2026-11-01", "--to", "2026-11-30"];
    let (out, _) = text(&cold_lava(&home, &args));
    assert!(out.contains("Read the Royal Society report on"), "{out}");
    assert!(out.contains("Eruptions.csv"), "{out}");
    let (out, _) = text(&cold_lava(&home, &["table", ws, "eruption"]));
    assert!(out.contains("Krakatau"), "{out}");
    assert!(out.contains("Eyjafjallajökull"), "{out}");

    // csv-table's own type: listed with its origin, and the rows of the CSV no type of the
    // workspace's matches are its items.
    let (out, err) = text(&cold_lava(&home, &["types", ws]));
    assert!(err.is_empty(), "{err}");
    let row = out.split("\n\n").find(|t| t.starts_with("csv-table.row"));
    let row = row.unwrap_or_else(|| panic!("{out}"));
    assert!(
        row.starts_with("csv-table.row  extension csv-table\n  found 3 items\n  priority -1\n  on table-row · role row · where file-type csv"),
        "{row}"
    );
    let eruptions = out.split("\n\n").find(|t| t.starts_with("eruption"));
    assert!(
        eruptions.is_some_and(|t| t.contains("found 8 items")),
        "{out}"
    );
    let (out, _) = text(&cold_lava(&home, &["items", ws, "csv-table.row"]));
    assert!(
        out.contains("Volcanoes/Observatories.csv:1  csv-table.row  name Vesuvius Observatory"),
        "{out}"
    );
    assert!(!out.contains("Eruptions.csv"), "{out}");
    let _ = std::fs::remove_dir_all(workspace.parent().unwrap());
}

#[test]
fn a_cli_rename_its_owner_refuses_changes_nothing() {
    if !has_python() {
        eprintln!("skipped: python3 isn't on PATH, and the example extension needs it");
        return;
    }
    let refuses = r#"@ext.on("file/edit")
def refuse(params):
    raise ValueError("this copy is read-only")


ext.run()"#;
    let (workspace, home) = sandbox("cli-refused", |main| main.replace("ext.run()", refuses));
    let before = files_under(&workspace);
    let ws = workspace.to_str().unwrap();
    let renamed = cold_lava(&home, &["rename", ws, "Krakatoa", "Krakatau"]);
    let (out, err) = text(&renamed);
    assert!(!renamed.status.success(), "{out}");
    assert!(
        err.contains(
            "csv-table didn't edit Volcanoes/Eruptions.csv: this copy is read-only; nothing \
             was changed"
        ),
        "{err}"
    );
    assert!(
        files_under(&workspace) == before,
        "every file of the workspace is as it was"
    );
    let _ = std::fs::remove_dir_all(workspace.parent().unwrap());
}

/// The example workspace's CSV with `#tag` in Krakatoa's Markdown cell.
fn tag_the_csv(workspace: &Path, tag: &str) -> String {
    let csv = workspace.join("Volcanoes/Eruptions.csv");
    let text = std::fs::read_to_string(&csv).unwrap();
    let tagged = text.replace("📅 2026-11-20", &format!("#{tag} 📅 2026-11-20"));
    std::fs::write(&csv, &tagged).unwrap();
    tagged
}

#[test]
fn a_cli_recipe_changes_owned_files_through_their_owner() {
    if !has_python() {
        eprintln!("skipped: python3 isn't on PATH, and the example extension needs it");
        return;
    }
    let (workspace, home) = sandbox("cli-recipe", |main| main);
    let before = tag_the_csv(&workspace, "eruption");
    let ws = workspace.to_str().unwrap();
    let args = [
        "recipe",
        ws,
        "rename-tag",
        "--from",
        "eruption",
        "--to",
        "eruptions",
    ];
    let dry = cold_lava(&home, &[&args[..], &["--dry-run", "--json"]].concat());
    let (out, err) = text(&dry);
    assert!(dry.status.success(), "{out}{err}");
    let json: serde_json::Value = serde_json::from_str(&out).unwrap();
    let csv = (json["files"].as_array().unwrap().iter())
        .find(|f| f["file"] == "Volcanoes/Eruptions.csv")
        .unwrap_or_else(|| panic!("{out}"));
    assert_eq!(csv["owner"], "csv-table");
    assert_eq!(csv["changes"], 1);
    let diff = csv["diff"].as_str().unwrap();
    assert!(
        diff.contains("+- [ ] Read the Royal Society report on [[Krakatoa]] #eruptions 📅"),
        "{diff}"
    );
    assert_eq!(json["changes"], 5, "four notes and the CSV");
    let csv_now = || std::fs::read_to_string(workspace.join("Volcanoes/Eruptions.csv")).unwrap();
    assert_eq!(csv_now(), before, "a dry run writes nothing");

    let applied = cold_lava(&home, &args);
    let (out, err) = text(&applied);
    assert!(applied.status.success(), "{out}{err}");
    assert!(
        out.contains("--- Volcanoes/Eruptions.csv (through csv-table)"),
        "{out}"
    );
    assert_eq!(csv_now(), before.replace("#eruption ", "#eruptions "));
    let krakatoa = std::fs::read_to_string(workspace.join("Volcanoes/Krakatoa.md")).unwrap();
    assert!(krakatoa.contains("\n#eruptions\n"), "{krakatoa}");
    let _ = std::fs::remove_dir_all(workspace.parent().unwrap());
}

#[test]
fn a_cli_recipe_refuses_a_file_changed_while_it_waited_for_an_owner() {
    if !has_python() {
        eprintln!("skipped: python3 isn't on PATH, and the example extension needs it");
        return;
    }
    let (workspace, home) = sandbox("cli-recipe-refused", |main| main);
    tag_the_csv(&workspace, "eruption");
    // This owner edits a note behind the recipe's back before it answers.
    let note = workspace.join("Volcanoes/Vesuvius.md");
    let meddles = format!(
        "_edit = file_edit\n\n\n@ext.on(\"file/edit\")\ndef meddle(params):\n    \
         with open({:?}, \"a\") as f:\n        f.write(\"an outside change\\n\")\n    \
         return _edit(params)\n\n\next.run()",
        note.to_str().unwrap()
    );
    let installed = home.join(".config/cold-lava/extensions/csv-table/main.py");
    let main = std::fs::read_to_string(&installed).unwrap();
    std::fs::write(&installed, main.replace("ext.run()", &meddles)).unwrap();
    let before = files_under(&workspace);
    let ws = workspace.to_str().unwrap();
    let args = [
        "recipe",
        ws,
        "rename-tag",
        "--from",
        "eruption",
        "--to",
        "eruptions",
    ];
    let refused = cold_lava(&home, &args);
    let (out, err) = text(&refused);
    assert!(!refused.status.success(), "{out}");
    assert!(
        err.contains(
            "Volcanoes/Vesuvius.md changed since the recipe was planned; nothing was changed"
        ),
        "{err}"
    );
    let vesuvius = std::fs::read_to_string(&note).unwrap();
    assert!(vesuvius.ends_with("an outside change\n"), "{vesuvius}");
    let others = |files: Vec<(PathBuf, Vec<u8>)>| -> Vec<_> {
        files.into_iter().filter(|(p, _)| *p != note).collect()
    };
    assert!(
        others(files_under(&workspace)) == others(before),
        "every other file is as it was"
    );
    let _ = std::fs::remove_dir_all(workspace.parent().unwrap());
}

/// The csv-table example gives a row as a piece and a cell as plain characters, from its own
/// process. What looks like Markdown in a cell is nothing: no tag, no link, no math, no task.
/// A `(link)` column's cells are links a rename follows, a `(markdown)` column's cells are
/// Markdown, and an edit of a cell changes that cell's bytes and no others: the quotes, the
/// CRLF line ends and the blank line stay.
#[test]
fn a_csv_cell_is_plain_characters_and_an_edit_changes_only_its_bytes() {
    use serde_json::{Value, json};
    if !has_python() {
        eprintln!("skipped: python3 isn't on PATH, and the example extension needs it");
        return;
    }
    const SHEET: &str = "Name,Cost,Ref,Todo,Seen (link),Notes (markdown)\r\n\
        one,#42,[[x]],- [ ] y,Krakatoa,\"- [ ] A real task, quoted\"\r\n\
        \r\n\
        \"two, too\",$5 and $$6$$,**x** `c` ==h==,[due:: 2026-11-20] #volcano,,\r\n";
    const PRICED: &str = "\n[types.priced]\non = \"row\"\nwhere.header = [\"Name\", \"Cost\"]\n\
        fields.name = { column = \"Name\" }\nfields.cost = { column = \"Cost\" }\n\
        fields.reference = { column = \"Ref\" }\nfields.todo = { column = \"Todo\" }\n\
        fields.tags = { inlines = \"tags\", type = \"tag\" }\n\
        fields.links = { inlines = \"links\", type = \"link\" }\n";
    let (workspace, home) = sandbox("cli-cells", |main| main);
    let ws = workspace.to_str().unwrap();
    let file = workspace.join("Sheet.csv");
    std::fs::write(&file, SHEET).unwrap();
    let types = workspace.join(".cold-lava/types.toml");
    let before = std::fs::read_to_string(&types).unwrap();
    std::fs::write(&types, before + PRICED).unwrap();
    let sheet = || std::fs::read_to_string(&file).unwrap();
    let run = |args: &[&str]| -> String {
        let out = cold_lava(&home, args);
        let (stdout, stderr) = text(&out);
        assert!(out.status.success(), "{args:?}\n{stdout}{stderr}");
        stdout
    };

    // A rename starts the owner to read its files, and repoints the link cell: one cell.
    run(&["rename", ws, "Krakatoa", "Krakatau"]);
    let renamed = SHEET.replace(",Krakatoa,", ",Krakatau,");
    assert_eq!(sheet(), renamed, "the link cell follows, byte for byte");

    let in_sheet = |kind: &str| -> Vec<Value> {
        let listed = run(&["items", ws, kind, "--json"]);
        let items = listed.lines().map(|l| serde_json::from_str(l).unwrap());
        let here = |item: &Value| item["note"] == "Sheet.csv";
        items.filter(here).collect()
    };
    let rows = in_sheet("priced");
    let [one, two] = &rows[..] else {
        panic!("{rows:?}");
    };
    let says = |row: &Value, field: &str| row[field].clone();
    assert_eq!(
        [says(one, "cost"), says(one, "reference"), says(one, "todo")],
        [json!("#42"), json!("[[x]]"), json!("- [ ] y")]
    );
    assert_eq!(
        (says(one, "tags"), says(one, "links")),
        (json!([]), json!(["Krakatau"])),
        "only the link column's cell is a link"
    );
    assert_eq!(
        [
            says(two, "name"),
            says(two, "cost"),
            says(two, "reference"),
            says(two, "todo")
        ],
        [
            json!("two, too"),
            json!("$5 and $$6$$"),
            json!("**x** `c` ==h=="),
            json!("[due:: 2026-11-20] #volcano")
        ]
    );
    assert_eq!(
        (says(two, "tags"), says(two, "links")),
        (json!([]), json!([]))
    );
    let tasks = in_sheet("task");
    let words: Vec<&Value> = tasks.iter().map(|t| &t["text"]).collect();
    assert_eq!(
        words,
        [&json!("A real task, quoted")],
        "the Markdown column's task, and not `- [ ] y`"
    );
    assert!(in_sheet("list").is_empty() && in_sheet("block").is_empty());

    // Refused by the owner: a row is named by its first cell. Nothing is written.
    let id = |row: &Value| row["id"].as_str().unwrap().to_string();
    let path = file.to_str().unwrap();
    let nameless = cold_lava(&home, &["item", "set", path, id(one).as_str(), "name", ""]);
    let (out, err) = text(&nameless);
    assert!(!nameless.status.success(), "{out}");
    assert!(err.contains("a row needs its first cell"), "{err}");
    assert_eq!(sheet(), renamed);

    // One cell set: its bytes change, and the characters set are characters.
    run(&[
        "item",
        "set",
        path,
        id(two).as_str(),
        "cost",
        "#43, \"[[z]]\"",
    ]);
    assert_eq!(
        sheet(),
        renamed.replace("$5 and $$6$$", "\"#43, \"\"[[z]]\"\"\""),
        "quoted as CSV needs, and no other byte moved"
    );
    let _ = std::fs::remove_dir_all(workspace.parent().unwrap());
}

/// No row of a CSV goes missing: rows that start alike are numbered, and a row whose first
/// cell is literally another's number (`a (2)`) keeps a place of its own. A link cell may say
/// a heading or the words to show after the file's name: it is a link to the file, and a
/// rename changes the name and keeps what follows.
#[test]
fn every_csv_row_has_a_place_and_a_link_cell_keeps_what_follows_the_name() {
    use serde_json::{Value, json};
    if !has_python() {
        eprintln!("skipped: python3 isn't on PATH, and the example extension needs it");
        return;
    }
    const ROWS: &str = "K,Where (link)\n\
        a (2),Krakatoa#Eruptions\n\
        a,Krakatoa|Big K\n\
        a,\n";
    const SEEN: &str = "\n[types.seen]\non = \"row\"\nwhere.header = [\"K\", \"Where\"]\n\
        fields.k = { column = \"K\" }\n\
        fields.links = { inlines = \"links\", type = \"link\" }\n";
    let (workspace, home) = sandbox("cli-rows", |main| main);
    let ws = workspace.to_str().unwrap();
    let file = workspace.join("Rows.csv");
    std::fs::write(&file, ROWS).unwrap();
    let types = workspace.join(".cold-lava/types.toml");
    let before = std::fs::read_to_string(&types).unwrap();
    std::fs::write(&types, before + SEEN).unwrap();
    let run = |args: &[&str]| -> String {
        let out = cold_lava(&home, args);
        let (stdout, stderr) = text(&out);
        assert!(out.status.success(), "{args:?}\n{stdout}{stderr}");
        stdout
    };

    run(&["rename", ws, "Krakatoa", "Krakatau"]);
    assert_eq!(
        std::fs::read_to_string(&file).unwrap(),
        ROWS.replace("Krakatoa", "Krakatau"),
        "both links follow, and the heading and the words stay"
    );
    let listed = run(&["items", ws, "seen", "--json"]);
    let rows: Vec<Value> = (listed.lines())
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
        .filter(|item| item["note"] == "Rows.csv")
        .collect();
    let said: Vec<(&Value, &Value)> = rows.iter().map(|r| (&r["k"], &r["links"])).collect();
    assert_eq!(
        said,
        [
            (&json!("a (2)"), &json!(["Krakatau"])),
            (&json!("a"), &json!(["Krakatau"])),
            (&json!("a"), &json!([])),
        ],
        "three rows, none dropped"
    );
    let _ = std::fs::remove_dir_all(workspace.parent().unwrap());
}

/// A row's first cell can be set when other rows start the same, first, middle or last: the
/// rows like it keep their places, so the read-back finds each where it was. A dated log
/// repeats its first cell on most lines.
#[test]
fn a_csv_rows_first_cell_is_set_among_rows_that_start_alike() {
    use serde_json::Value;
    if !has_python() {
        eprintln!("skipped: python3 isn't on PATH, and the example extension needs it");
        return;
    }
    const CELLT: &str = "\n[types.cellt]\non = \"row\"\nwhere.header = [\"K\", \"V\"]\n\
        fields.k = { column = \"K\" }\nfields.v = { column = \"V\" }\n";
    // The last two have a first cell that is literally the name another row is given.
    for (name, before, v, to, after) in [
        ("first", "K,V\na,1\na,2\n", "1", "zz", "K,V\nzz,1\na,2\n"),
        ("last", "K,V\na,1\na,2\n", "2", "zz", "K,V\na,1\nzz,2\n"),
        (
            "apart",
            "K,V\na,1\nb,2\na,3\n",
            "1",
            "zz",
            "K,V\nzz,1\nb,2\na,3\n",
        ),
        (
            "middle",
            "K,V\na,1\na,2\na,3\n",
            "2",
            "zz",
            "K,V\na,1\nzz,2\na,3\n",
        ),
        ("joins", "K,V\na,1\nzz,2\n", "1", "zz", "K,V\nzz,1\nzz,2\n"),
        (
            "literal",
            "K,V\na (row 3),lit\na,x\na,y\n",
            "lit",
            "zz",
            "K,V\nzz,lit\na,x\na,y\n",
        ),
        (
            "leaves",
            "K,V\na,x\na,y\na (row 1),lit\n",
            "x",
            "solo",
            "K,V\nsolo,x\na,y\na (row 1),lit\n",
        ),
    ] {
        let (workspace, home) = sandbox(&format!("cli-alike-{name}"), |main| main);
        let ws = workspace.to_str().unwrap();
        let file = workspace.join("T.csv");
        std::fs::write(&file, before).unwrap();
        let types = workspace.join(".cold-lava/types.toml");
        let text_before = std::fs::read_to_string(&types).unwrap();
        std::fs::write(&types, text_before + CELLT).unwrap();
        let run = |args: &[&str]| -> String {
            let out = cold_lava(&home, args);
            let (stdout, stderr) = text(&out);
            assert!(out.status.success(), "{name}: {args:?}\n{stdout}{stderr}");
            stdout
        };
        // A rename starts the owner, which reads the file.
        run(&["rename", ws, "Krakatoa", "Krakatau"]);
        let listed = run(&["items", ws, "cellt", "--json"]);
        let rows: Vec<Value> = (listed.lines())
            .map(|line| serde_json::from_str::<Value>(line).unwrap())
            .collect();
        assert_eq!(rows.len(), before.lines().count() - 1, "{name}: {listed}");
        let row = rows.iter().find(|r| r["v"] == v).expect("the row");
        let id = row["id"].as_str().unwrap();
        run(&["item", "set", file.to_str().unwrap(), id, "k", to]);
        assert_eq!(std::fs::read_to_string(&file).unwrap(), after, "{name}");
        let _ = std::fs::remove_dir_all(workspace.parent().unwrap());
    }
}

/// A copy of the example workspace with `Home.chores` in it, and a home with the owner of
/// `tests/chores-owner` installed beside the examples' helper.
fn chores_sandbox(name: &str, chores: &str) -> (PathBuf, PathBuf) {
    let dir =
        Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("owned-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let workspace = dir.join("example-workspace");
    copy_dir(&manifest.join("../../example-workspace"), &workspace);
    std::fs::write(workspace.join("Home.chores"), chores).unwrap();
    std::fs::write(workspace.join("Scratch.md"), "A note to rename.\n").unwrap();
    let home = dir.join("home");
    let installed = home.join(".config/cold-lava/extensions/chores");
    copy_dir(&manifest.join("tests/chores-owner"), &installed);
    let helper = manifest.join("../../examples/extensions/open-tasks/cold_lava.py");
    std::fs::copy(helper, installed.join("cold_lava.py")).unwrap();
    (workspace, home)
}

/// Step 3 of the neutral model through the real transport: an owner in its own process, for
/// a made-up format that isn't Markdown, gives its chores as pieces. They are tasks, and they
/// are ticked, re-dated and reworded with `item set`, which the owner hears as changes in the
/// model's terms and writes its own way. No other byte of its file moves.
#[test]
fn a_made_up_formats_tasks_are_read_and_edited_through_their_owners_process() {
    use serde_json::{Value, json};
    if !has_python() {
        eprintln!("skipped: python3 isn't on PATH, and the test extension needs it");
        return;
    }
    const CHORES: &str = "; The house\n\
        plumber | todo  | 2026-11-20 | Call the plumber @home about >Krakatoa\n\
        rent    | done  |            | Pay #42 for $5 to [[x]] and **x**\n";
    let (workspace, home) = chores_sandbox("chores", CHORES);
    let ws = workspace.to_str().unwrap();
    let file = workspace.join("Home.chores");
    let chores = || std::fs::read_to_string(&file).unwrap();
    let run = |args: &[&str]| -> String {
        let out = cold_lava(&home, args);
        let (stdout, stderr) = text(&out);
        assert!(out.status.success(), "{args:?}\n{stdout}{stderr}");
        stdout
    };

    // The command line reads an owned file from what its owner gave before, and a rename is
    // what starts an owner to read. This one also repoints the link the chore says.
    run(&["rename", ws, "Krakatoa", "Krakatau"]);
    let now = CHORES.replace(">Krakatoa", ">Krakatau");
    assert_eq!(chores(), now, "the link the piece said, repointed");

    let listed = run(&["items", ws, "task", "--json"]);
    let tasks: Vec<Value> = (listed.lines())
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
        .filter(|item| item["note"] == "Home.chores")
        .collect();
    let [plumber, rent] = &tasks[..] else {
        panic!("{listed}");
    };
    assert_eq!(plumber["text"], "Call the plumber #home about Krakatau");
    assert_eq!(
        (&plumber["status"], &plumber["status_type"], &plumber["due"]),
        (&json!(" "), &json!("todo"), &json!("2026-11-20"))
    );
    assert_eq!(
        (&plumber["tags"], &plumber["links"]),
        (&json!(["home"]), &json!(["Krakatau"]))
    );
    assert_eq!(rent["text"], "Pay #42 for $5 to [[x]] and **x**");
    assert_eq!(
        (&rent["status_type"], &rent["tags"], &rent["links"]),
        (&json!("done"), &json!([]), &json!([])),
        "`#42` and `[[x]]` in plain text are characters"
    );
    let id = plumber["id"].as_str().unwrap();

    let set = |field: &str, value: &str| -> Value {
        let file = file.to_str().unwrap();
        let out = run(&["item", "set", file, id, field, value, "--json"]);
        serde_json::from_str(&out).unwrap()
    };
    // After an edit the command line has what the owner read before it: renaming a note has
    // the owner read the file as it is now.
    let read_again = |from: &str, to: &str| drop(run(&["rename", ws, from, to]));

    let ticked = set("status", "x");
    assert_eq!(
        (&ticked["applied"], &ticked["item"]["status_type"]),
        (&json!(true), &json!("done"))
    );
    assert_eq!(ticked["item"]["id"], id, "a piece keeps its id");
    let now = now.replace("| todo  |", "| done  |");
    assert_eq!(chores(), now, "ticked: one word");

    read_again("Scratch", "Scratch 2");
    let dated = set("due", "2026-11-21");
    assert_eq!(dated["item"]["due"], "2026-11-21");
    let now = now.replace("2026-11-20", "2026-11-21");
    assert_eq!(chores(), now, "re-dated");

    read_again("Scratch 2", "Scratch 3");
    let file_arg = file.to_str().unwrap();
    // Words the format can't hold: it ends a link at a space, so a comma after one would
    // read back as part of the link's target. The read-back catches it, and nothing is written.
    let comma = "Call the electrician #home about Krakatau, not [[x]]";
    let refused = cold_lava(&home, &["item", "set", file_arg, id, "text", comma]);
    let (_, why) = text(&refused);
    assert!(!refused.status.success(), "{why}");
    assert!(why.contains("doesn't read back as asked"), "{why}");
    assert!(why.contains("nothing was written"), "{why}");
    assert_eq!(chores(), now, "as it was");

    let reworded = set(
        "text",
        "Call the electrician #home about Krakatau and not [[x]]",
    );
    assert_eq!(
        (&reworded["item"]["tags"], &reworded["item"]["links"]),
        (&json!(["home"]), &json!(["Krakatau"])),
        "the label and the link still among the words stay, and a typed `[[x]]` is characters"
    );
    let now = now.replace(
        "Call the plumber @home about >Krakatau",
        "Call the electrician @home about >Krakatau and not [[x]]",
    );
    assert_eq!(chores(), now, "reworded, in the format's own marks");

    read_again("Scratch 3", "Scratch 4");
    let undated = run(&["item", "set", file_arg, id, "due", "--remove", "--json"]);
    let undated: Value = serde_json::from_str(&undated).unwrap();
    assert_eq!(undated["item"]["due"], Value::Null);
    let now = now.replace("| 2026-11-21 |", "|  |");
    assert_eq!(
        chores(),
        now,
        "the date taken off; the remark and the other chore untouched"
    );

    let log = home.join(".cache/cold-lava/extensions/chores.log");
    assert_eq!(
        std::fs::read_to_string(log).unwrap(),
        "",
        "nothing on stderr"
    );
    let _ = std::fs::remove_dir_all(workspace.parent().unwrap());
}

/// An owner's place is its own words: one that holds an escape sequence is shown as `\u{…}`
/// in the problem line that names it, and never reaches the terminal as written.
#[test]
fn an_owners_place_is_never_printed_as_written() {
    if !has_python() {
        eprintln!("skipped: python3 isn't on PATH, and the test extension needs it");
        return;
    }
    // Two chores share a place, so the second is a problem, named by its place. The owner
    // writes a change into the last chore at a place, the second, which is left out: the
    // read-back refuses the tick and names it (#257).
    const CHORES: &str = "\u{1b}]0;PWNED-place\u{7}p | todo | | first words\n\
        \u{1b}]0;PWNED-place\u{7}p | todo | | second words\n\
        ok | todo | | fine\n";
    let (workspace, home) = chores_sandbox("place", CHORES);
    let owner = home.join(".config/cold-lava/extensions/chores/main.py");
    let main = std::fs::read_to_string(&owner).unwrap();
    std::fs::write(&owner, main.replace("i = at[0]", "i = at[-1]")).unwrap();
    let ws = workspace.to_str().unwrap();
    let file = workspace.join("Home.chores");
    // A rename starts the owner, which reads the file.
    let renamed = cold_lava(&home, &["rename", ws, "Scratch", "Scratch 2"]);
    assert!(renamed.status.success(), "{:?}", text(&renamed));
    let listed = cold_lava(&home, &["items", ws, "task", "--json"]);
    let (listed, _) = text(&listed);
    let chore = |words: &str| -> String {
        let found = (listed.lines())
            .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
            .find(|item| item["note"] == "Home.chores" && item["text"] == words);
        let found = found.unwrap_or_else(|| panic!("no chore {words}: {listed}"));
        found["id"].as_str().unwrap().to_string()
    };
    let first = chore("first words");
    let id = first.as_str();
    let refused = cold_lava(
        &home,
        &["item", "set", file.to_str().unwrap(), id, "status", "x"],
    );
    let (stdout, stderr) = text(&refused);
    assert_eq!(refused.status.code(), Some(1), "{stdout}{stderr}");
    assert_eq!(
        stderr,
        "cold-lava: chores's new text for Home.chores doesn't read back as asked \
         (\\u{1b}]0;PWNED-place\\u{7}p: another piece is at this place was left out, and came \
         back changed, moved or not at all); nothing was written\n",
        "{stderr:?}"
    );
    assert_eq!(std::fs::read_to_string(&file).unwrap(), CHORES, "as it was");
    // Only what is printed as text is escaped: for a program the reason is the place as the
    // owner wrote it, escaped the way JSON escapes.
    let file = file.to_str().unwrap();
    let refused = cold_lava(&home, &["item", "set", file, id, "status", "x", "--json"]);
    let (stdout, _) = text(&refused);
    assert_eq!(
        stdout,
        "{\"error\":\"chores's new text for Home.chores doesn't read back as asked \
         (\\u001b]0;PWNED-place\\u0007p: another piece is at this place was left out, and came \
         back changed, moved or not at all); nothing was written\",\"changed\":false}\n"
    );
    // A chore beside the left-out one is ticked (#257), and every other line keeps its bytes.
    let ok = chore("fine");
    let ticked = cold_lava(&home, &["item", "set", file, ok.as_str(), "status", "x"]);
    assert!(ticked.status.success(), "{:?}", text(&ticked));
    assert_eq!(
        std::fs::read_to_string(file).unwrap(),
        CHORES.replace("ok | todo |", "ok | done |"),
        "only the chore asked for"
    );
    let _ = std::fs::remove_dir_all(workspace.parent().unwrap());
}

/// What an extension says is its own words, line breaks included: a notice it shows and
/// the reason it refuses with each print as one line, so neither can begin a line that
/// reads as cold-lava's.
#[test]
fn an_extensions_notice_and_its_reason_print_as_one_line_each() {
    if !has_python() {
        eprintln!("skipped: python3 isn't on PATH, and the example extension needs it");
        return;
    }
    let refuses = r#"@ext.on("file/edit")
def refuse(params):
    ext.show("csv-table says\ncold-lava: all is well \x1b]0;t\x07")
    raise ValueError("read-only\ncold-lava: renamed everything")


ext.run()"#;
    let (workspace, home) = sandbox("cli-lines", |main| main.replace("ext.run()", refuses));
    let ws = workspace.to_str().unwrap();
    let renamed = cold_lava(&home, &["rename", ws, "Krakatoa", "Krakatau"]);
    let (out, err) = text(&renamed);
    assert!(!renamed.status.success(), "{out}");
    let lines: Vec<&str> = err.lines().collect();
    let notice = "csv-table says\\ncold-lava: all is well \\u{1b}]0;t\\u{7}";
    let reason = "cold-lava: csv-table didn't edit Volcanoes/Eruptions.csv: read-only\\ncold-lava: \
                  renamed everything; nothing was changed";
    assert!(lines.contains(&notice), "the notice: {err}");
    assert!(lines.contains(&reason), "the reason: {err}");
    let forged =
        |line: &&str| line.starts_with("cold-lava: all") || line.starts_with("cold-lava: renamed");
    assert!(!lines.iter().any(forged), "{err}");
    let _ = std::fs::remove_dir_all(workspace.parent().unwrap());
}
