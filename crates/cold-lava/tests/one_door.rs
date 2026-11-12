//! One door into the engine (#193): what the terminal app does to a workspace's files goes
//! through the core, with the gateway's messages. This reads the app's own source, every file
//! under `src/`, so a direct call can't come back. Each list below is everything of its kind
//! the source names: a new one fails here, and a later group of #193 takes its own off.
//!
//! The same for what else still goes around the door (#270, track B): reads of the core's
//! index, item edits that don't go through `Core::edit_item`, links and tags read by a pattern
//! instead of by the Markdown owner, and what the built-in extensions use beyond the gateway.
//! Those lists say how wide the gap is today and fail when it grows, and they fail too when an
//! entry is gone and still listed, so they stay true as they shrink.
//!
//! It is a guard on the text, not a proof. It can't see a call made through a wrapper or a
//! re-export in another crate, through a macro, a function pointer or a trait object, a
//! `std::fs` handle already open (`File`'s own methods), a module of `std` under another name
//! (`use std::fs as f`), a program the app starts, or anything in the host and the engine,
//! which are theirs to guard. A reviewer still reads a change that touches files.

use std::path::Path;

/// Every source file of the binary by its path under `src/`, as `code` keeps it, sorted.
fn sources() -> Vec<(String, String)> {
    fn walk(dir: &Path, src: &Path, out: &mut Vec<(String, String)>) {
        for entry in std::fs::read_dir(dir).unwrap().flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(&path, src, out);
            } else if path.extension().is_some_and(|x| x == "rs") {
                let rel = path.strip_prefix(src).unwrap().to_string_lossy();
                let text = std::fs::read_to_string(&path).unwrap();
                out.push((rel.replace('\\', "/"), code(&text)));
            }
        }
    }
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut out = Vec::new();
    walk(&src, &src, &mut out);
    out.sort();
    out
}

/// A file's text without its unit tests (each `#[cfg(test)]` on a `mod`, to the brace that
/// closes it) and without comment lines. What follows a test module is kept.
fn code(text: &str) -> String {
    let mut kept = Vec::new();
    let mut lines = text.lines().peekable();
    while let Some(line) = lines.next() {
        let next = lines.peek().map(|l| l.trim_start());
        if line.trim() == "#[cfg(test)]" && next.is_some_and(|l| l.starts_with("mod ")) {
            let mut depth = 0;
            for inside in lines.by_ref() {
                depth += inside.matches('{').count() as i32;
                depth -= inside.matches('}').count() as i32;
                if depth <= 0 {
                    break;
                }
            }
        } else if !line.trim_start().starts_with("//") {
            kept.push(line);
        }
    }
    kept.join("\n")
}

/// The functions of `module` that `code` names, each as often as it is named: by path
/// (`trash::delete(…)`), or in a `use` (`use crate::trash::{delete, Went}`), after which a
/// bare `delete(…)` is the call. Types, which start with a capital, are not calls. Asked of
/// `crate::`, the names are the modules a file uses.
fn named_in(code: &str, module: &str) -> Vec<String> {
    let mut found = Vec::new();
    for (at, _) in code.match_indices(module) {
        let rest = &code[at + module.len()..];
        let grouped = rest.strip_prefix('{').and_then(group);
        let names = grouped.unwrap_or_else(|| vec![rest]);
        for name in names {
            let word = |c: &char| c.is_alphanumeric() || *c == '_';
            let name: String = name.trim_start().chars().take_while(word).collect();
            let is_function = name.chars().next().is_some_and(char::is_lowercase);
            if is_function && name != "self" {
                found.push(name);
            }
        }
    }
    found.sort();
    found
}

/// The entries of a `use` group, from after its `{` to the `}` that closes it. A group inside
/// an entry stays in that entry, so the entries after it are read too.
fn group(inside: &str) -> Option<Vec<&str>> {
    let mut entries = Vec::new();
    let (mut depth, mut from) = (0, 0);
    for (at, c) in inside.char_indices() {
        match c {
            '{' => depth += 1,
            '}' if depth == 0 => {
                entries.push(&inside[from..at]);
                return Some(entries);
            }
            '}' => depth -= 1,
            ',' if depth == 0 => {
                entries.push(&inside[from..at]);
                from = at + 1;
            }
            _ => {}
        }
    }
    None
}

/// Sorted entries, each once with how often it was there: `home.rs: .index ×39`.
fn tally(found: Vec<String>) -> Vec<String> {
    let mut counted: Vec<(String, usize)> = Vec::new();
    for entry in found {
        match counted.last_mut() {
            Some((last, times)) if *last == entry => *times += 1,
            _ => counted.push((entry, 1)),
        }
    }
    let line = |(entry, times): (String, usize)| format!("{entry} ×{times}");
    counted.into_iter().map(line).collect()
}

/// Fails unless what was `found` is exactly what is `listed`, and says which entries are new
/// (send that use through the core, or list it with its reason) and which are listed but no
/// longer found (take them off: a list here only shrinks). A count that changed shows as both.
fn only_listed(what: &str, found: &[String], listed: &[&str]) {
    let found: Vec<&str> = found.iter().map(String::as_str).collect();
    let new: Vec<&str> = (found.iter().copied())
        .filter(|entry| !listed.contains(entry))
        .collect();
    let gone: Vec<&str> = (listed.iter().copied())
        .filter(|entry| !found.contains(entry))
        .collect();
    assert!(
        new.is_empty() && gone.is_empty(),
        "{what}\n  found but not listed: {new:?}\n  listed but not found: {gone:?}"
    );
    assert_eq!(found, listed, "{what}");
}

/// `file: what` for every time a file's code has one of `needles`, sorted.
fn hits(needles: &[&str]) -> Vec<String> {
    let mut found = Vec::new();
    for (file, code) in sources() {
        for needle in needles {
            let times = code.matches(needle).count();
            found.extend((0..times).map(|_| format!("{file}: {needle}")));
        }
    }
    found.sort();
    found
}

/// Deleting, restoring and making a folder are `Core::delete_file`, `restore_file` and
/// `create_folder`, asked as `menu`: nothing in the app names `trash::delete`,
/// `trash::restore` or `write::create_dir`. What it still names of the engine's trash and
/// write functions:
///
/// - `trash::system_bin`: the delete prompt's "goes to" line, and `merge::apply`'s argument.
/// - `trash::free_path`: the name an undo offers when the old one is taken. It reads.
/// - `write::create`, `create_if_missing`, `replace` in `home.rs`: new notes, and a note's
///   new text. Later groups of #193.
/// - `write::create_if_missing`, `replace` in `board.rs`: the whiteboard saves its canvas.
#[test]
fn the_app_deletes_restores_and_makes_folders_through_the_core() {
    let mut named = Vec::new();
    for (file, code) in sources() {
        for module in ["trash::", "write::"] {
            let names = named_in(&code, module).into_iter();
            named.extend(names.map(|name| format!("{file}: {module}{name}")));
        }
    }
    named.sort();
    let left = [
        "board.rs: write::create_if_missing",
        "board.rs: write::replace",
        "home.rs: trash::free_path",
        "home.rs: trash::system_bin",
        "home.rs: trash::system_bin",
        "home.rs: write::create",
        "home.rs: write::create_if_missing",
        "home.rs: write::create_if_missing",
        "home.rs: write::create_if_missing",
        "home.rs: write::create_if_missing",
        "home.rs: write::replace",
    ];
    assert_eq!(named, left);
    let home = sources().into_iter().find(|(file, _)| file == "home.rs");
    let (_, home) = home.unwrap();
    for asked in [
        "core.delete_file(MENU,",
        "core.restore_file(MENU,",
        "core.create_folder(MENU,",
    ] {
        assert!(home.contains(asked), "the menu asks the core: {asked}");
    }
}

/// The engine's whole-workspace operations the app still calls itself, each a later group of
/// #193: rename and move, merge and duplicate, recipes and their undo, bookmarks, the daily
/// note. `main.rs` is the command line (`cold-lava rename`, `recipe`, `daily`).
const OPERATIONS: [&str; 8] = [
    "merge::apply(",
    "merge::duplicate(",
    "merge::undo(",
    "rename::apply(",
    "recipe::apply(",
    "recipe::undo(",
    "bookmarks::edit(",
    "daily::ensure(",
];

#[test]
fn the_operations_still_called_directly_are_these() {
    let left = [
        "home.rs: bookmarks::edit(",
        "home.rs: bookmarks::edit(",
        "home.rs: daily::ensure(",
        "home.rs: merge::apply(",
        "home.rs: merge::duplicate(",
        "home.rs: merge::undo(",
        "home.rs: recipe::apply(",
        "home.rs: recipe::undo(",
        "home.rs: rename::apply(",
        "main.rs: daily::ensure(",
        "main.rs: recipe::apply(",
        "main.rs: rename::apply(",
    ];
    assert_eq!(hits(&OPERATIONS), left);
}

/// No file of a workspace is written, removed, renamed or made with `std::fs`. What the app
/// does with `std::fs` outside its tests is on files of its own:
///
/// - `board.rs`: the temporary file a card's text is in while `$EDITOR` has it.
/// - `drive.rs`: the scripted run's own home folder, and its `write` step, which changes a
///   workspace file "as another program would", on purpose: written into a file of the
///   run's own beside it and renamed over it, as a program that saves safely does, and that
///   file removed when the rename fails.
/// - `ext/nvim.rs`: the Vim script Neovim is started with, in the temp folder.
/// - `home.rs`: the list of opened workspaces, in the config folder.
#[test]
fn std_fs_touches_only_the_apps_own_files() {
    let calls = [
        "fs::write(",
        "fs::remove_file(",
        "fs::remove_dir_all(",
        "fs::remove_dir(",
        "fs::rename(",
        "fs::create_dir",
    ];
    let allowed = [
        "board.rs: fs::remove_file(",
        "board.rs: fs::write(",
        "drive.rs: fs::create_dir",
        "drive.rs: fs::create_dir",
        "drive.rs: fs::remove_dir_all(",
        "drive.rs: fs::remove_dir_all(",
        "drive.rs: fs::remove_dir_all(",
        "drive.rs: fs::remove_file(",
        "drive.rs: fs::rename(",
        "drive.rs: fs::write(",
        "ext/nvim.rs: fs::remove_file(",
        "ext/nvim.rs: fs::write(",
        "home.rs: fs::create_dir",
        "home.rs: fs::write(",
    ];
    assert_eq!(hits(&calls), allowed);
}

/// A function brought in with `use` and called by its bare name is a direct call too, and
/// unit tests and comments are not the app's code.
#[test]
fn the_scan_sees_an_import_and_a_bare_call() {
    let sample = "use crate::trash::{delete, TrashOption};\nfn f() {\n    delete(a, b);\n}\n";
    assert_eq!(named_in(sample, "trash::"), ["delete"]);
    let tested =
        "fn f() {}\n// trash::delete(a)\n#[cfg(test)]\nmod tests {\n    trash::restore(a);\n}\n";
    assert_eq!(named_in(&code(tested), "trash::"), Vec::<String>::new());
    let kept = "#[cfg(test)]\nfn only_in_tests() {}\nfn g() {\n    trash::restore(a);\n}\n";
    assert_eq!(named_in(&code(kept), "trash::"), ["restore"]);
}

/// The functions of `module` that a `use` in `code` brings in.
fn imported(code: &str, module: &str) -> Vec<String> {
    let mut found = Vec::new();
    for statement in code.split(';') {
        let is_use = |l: &&str| {
            let l = l.trim_start();
            l.starts_with("use ") || l.starts_with("pub use ") || l.starts_with("pub(crate) use ")
        };
        let lines: Vec<&str> = statement.lines().skip_while(|l| !is_use(l)).collect();
        found.extend(named_in(&lines.join("\n"), module));
    }
    found
}

/// The ways around the lists that the scan refuses outright: one of the engine's modules the
/// lists are about (the file-changing ones, `edit`, `tasks`, `merge` and `index`) under
/// another name or imported whole, and a `std::fs` function that changes files or one of the
/// `OPERATIONS` brought in with `use`, after which its bare name is the call.
fn sneaked(code: &str) -> Vec<String> {
    let mut found = Vec::new();
    for module in ["trash", "write", "files", "edit", "tasks", "merge", "index"] {
        let renamed = [
            format!("::{module} as "),
            format!("{{{module} as "),
            format!(", {module} as "),
        ];
        if renamed.iter().any(|alias| code.contains(alias.as_str())) {
            found.push(format!("{module} as"));
        }
        if code.contains(&format!("{module}::*")) {
            found.push(format!("{module}::*"));
        }
        let group = format!("{module}::{{");
        for (at, _) in code.match_indices(&group) {
            let inside = code[at..].split('}').next().unwrap_or_default();
            if inside.contains(" as ") {
                found.push(format!("{module}::{{… as …}}"));
            }
        }
    }
    let changes = [
        "write",
        "rename",
        "remove_file",
        "remove_dir",
        "remove_dir_all",
        "create_dir",
        "create_dir_all",
        "copy",
        "hard_link",
    ];
    for name in imported(code, "fs::") {
        if changes.contains(&name.as_str()) {
            found.push(format!("use std::fs::{name}"));
        }
    }
    for call in OPERATIONS {
        let (module, name) = call.trim_end_matches('(').split_once("::").unwrap();
        let names = imported(code, &format!("{module}::"));
        if names.iter().any(|n| n == name) {
            found.push(format!("use {module}::{name}"));
        }
    }
    found
}

/// Nothing goes around the lists: no alias or glob of the engine's `trash`, `write`, `files`,
/// `edit`, `tasks`, `merge` and `index` modules, no `std::fs` function that changes files and
/// none of the `OPERATIONS` imported by name, none of the gateway's own file functions
/// (`files::delete` and the rest, which are the core's to call), no file made or copied
/// another way, and no `Core` taken apart for its `index` (`let Core { index, .. } = …`).
#[test]
fn nothing_goes_around_the_lists() {
    let own = ["delete", "restore", "create_folder", "create", "replace"];
    let mut found = Vec::new();
    for (file, code) in sources() {
        let around = sneaked(&code).into_iter();
        found.extend(around.map(|what| format!("{file}: {what}")));
        let gateway = named_in(&code, "files::").into_iter();
        let gateway = gateway.filter(|name| own.contains(&name.as_str()));
        found.extend(gateway.map(|name| format!("{file}: files::{name}")));
    }
    assert_eq!(found, Vec::<String>::new());
    let other = [
        "File::create",
        "OpenOptions",
        "fs::copy(",
        "fs::hard_link(",
        "let Core {",
    ];
    assert_eq!(hits(&other), Vec::<String>::new());
}

#[test]
fn the_scan_refuses_an_alias_a_glob_and_an_imported_std_function() {
    let alias = "use crate::trash as t;\nfn f() {\n    t::delete(a);\n}\n";
    assert_eq!(sneaked(alias), ["trash as"]);
    let grouped = "use crate::{files, write::{self as w}};\n";
    assert_eq!(sneaked(grouped), ["write::{… as …}"]);
    let glob = "use cold_lava_engine::write::*;\nfn f() {\n    create_dir(a);\n}\n";
    assert_eq!(sneaked(glob), ["write::*"]);
    let std = "use std::fs::{read_to_string, rename};\nfn f() {\n    rename(a, b);\n}\n";
    assert_eq!(sneaked(std), ["use std::fs::rename"]);
    let fine = "use crate::trash::{self, TrashOption, Went};\nuse std::fs::read_to_string;\n";
    assert_eq!(sneaked(fine), Vec::<String>::new());
}

/// Code after a file's unit tests is the app's code too.
#[test]
fn the_scan_reads_on_after_a_test_module() {
    let text = "fn f() {}\n#[cfg(test)]\nmod tests {\n    fn t() {\n        trash::delete(a);\n    }\n}\n\
                fn g() {\n    trash::restore(a);\n}\n#[cfg(test)]\nmod more;\nfn h() {\n    trash::free_path(a);\n}\n";
    assert_eq!(named_in(&code(text), "trash::"), ["free_path", "restore"]);
}

/// The core's index is read from outside the core here: every `.index` in the app's code,
/// whatever it is written on (`self.core.index`, `c.index`, a `&Core` under any name), and
/// every path into the engine's `index` module (`index::Index::build`, a second index beside
/// the core's, and the types a function takes one as). Today each `.index` is a `Core`'s
/// field: a field of that name on anything else would count too, and is to be named
/// otherwise. The index becomes private when the list is empty (#270, B6).
///
/// What is counted is the reach, not the use: `let ix = &core.index`, then `ix` forty times,
/// is one. A reach the text doesn't show is not seen: through a function of the engine that
/// hands the index out, or a `Core` taken apart in a pattern other than a `let`.
///
/// - `home.rs`: the menu reads the tree, the switcher, search, backlinks, tags, the reader's
///   task and what a rename, a merge, a recipe or a delete would change, all from the index.
/// - `main.rs`: `cold-lava rename` and `recipe` plan on the core's index, `graph` asks it
///   whether there are notes; `tidy` and `search` build an index of their own.
/// - `owned.rs`: the command line's workspace with its owned files, and `item set`'s item.
#[test]
fn the_index_is_still_read_outside_the_core_here() {
    let found = tally(hits(&[".index", "index::"]));
    let left = [
        "home.rs: .index ×39",
        "home.rs: index:: ×2",
        "main.rs: .index ×3",
        "main.rs: index:: ×2",
        "owned.rs: .index ×2",
    ];
    only_listed("reads of the core's index", &found, &left);
}

/// What `code` names of the engine's `edit` and `tasks` modules, which change a note's items
/// themselves or make the text to change them with, apart from the functions of `tasks` that
/// only read. A function those modules gain counts until it is named a reader here.
fn item_writes(code: &str) -> Vec<String> {
    let reads = [
        "agenda",
        "first_of_month",
        "last_of_month",
        "locate",
        "today",
    ];
    let mut found = Vec::new();
    for module in ["edit::", "tasks::"] {
        let names = named_in(code, module).into_iter();
        let names = names.filter(|name| !reads.contains(&name.as_str()));
        found.extend(names.map(|name| format!("{module}{name}")));
    }
    found
}

/// An item is changed with `items/edit` (`Core::edit_item`, or `Host::edit_item` for an
/// extension), which asks the file's owner and tells subscribers. What still writes a note's
/// item itself, both on the command line (#270, B3):
///
/// - `edit::set_field`: `cold-lava item set` on a note.
/// - `tasks::toggle_in`: `cold-lava task toggle`.
#[test]
fn items_are_still_edited_around_the_core_here() {
    let mut found = Vec::new();
    for (file, code) in sources() {
        let names = item_writes(&code).into_iter();
        found.extend(names.map(|name| format!("{file}: {name}")));
    }
    found.sort();
    let left = [
        "main.rs: edit::set_field ×1",
        "main.rs: tasks::toggle_in ×1",
    ];
    only_listed("item edits around Core::edit_item", &tally(found), &left);
}

/// A link or a tag is what the Markdown owner and the resolver say is one, never what a
/// pattern of the app's finds. This lists every place the app's code spells the syntax to look
/// for it: a regular expression at all (`regex::`, `Regex::new(`), a wikilink's or a Markdown
/// link's brackets escaped for one (`\[\[`, `\]\(`), the brackets as a string or a character
/// to find (`"[["`, `"]("`, `'['`), a tag's `#` (`'#'`, `"#"`), and the two readers there are
/// by name, wherever they are named. It can't tell a pattern from another use of the same
/// characters, so those are listed too, and it doesn't see a scan spelled another way (a
/// byte, `'\u{5b}'`, a longer string that holds the brackets).
///
/// The readers:
///
/// - `agenda.rs`: `links_as_text`, two regular expressions that turn `[[Note|alias]]` and
///   `[text](url)` into the words shown; `home.rs` calls it for the reader's tick messages.
/// - `board.rs`: `markdown_text`, a card's Markdown styled by hand: `"[["`, `"]("`, one `'['`,
///   a heading's `'#'` three times and a link's `#heading` cut off at one; `home.rs` draws a
///   backlink's context with it.
/// - `ext/nvim.rs`: the Vim script's pattern for the link under the cursor.
///
/// The same characters, not a note being read: a key (`'['` in `board.rs`, `"#"` in
/// `commands.rs`), a colour (`'#'` in `board.rs` and `scene_view.rs`), an escape sequence
/// (`embed.rs`), a comment in a drive script (`drive.rs`), a `#tag` in the graph's filter
/// (`groups.rs`), a tag as the user typed it (`home.rs`), and a heading's or a tag's marker
/// written out (`main.rs`, `reader.rs`, `scene_view.rs`).
#[test]
fn links_and_tags_are_still_read_by_pattern_here() {
    let patterns = [
        "regex::",
        "Regex::new(",
        r"\[\[",
        r"\]\(",
        r#""[[""#,
        r#""](""#,
        "'['",
        "'#'",
        r##""#""##,
        "links_as_text",
        "markdown_text",
    ];
    let left = [
        "agenda.rs: Regex::new( ×2",
        r"agenda.rs: \[\[ ×1",
        r"agenda.rs: \]\( ×1",
        "agenda.rs: links_as_text ×3",
        "agenda.rs: regex:: ×2",
        r#"board.rs: "[[" ×1"#,
        r#"board.rs: "](" ×1"#,
        "board.rs: '#' ×5",
        "board.rs: '[' ×2",
        "board.rs: markdown_text ×3",
        r##"commands.rs: "#" ×1"##,
        "drive.rs: '#' ×1",
        "embed.rs: '[' ×1",
        r"ext/nvim.rs: \[\[ ×1",
        "groups.rs: '#' ×2",
        "home.rs: '#' ×3",
        "home.rs: links_as_text ×2",
        "home.rs: markdown_text ×1",
        r##"main.rs: "#" ×1"##,
        r##"reader.rs: "#" ×1"##,
        "reader.rs: '#' ×1",
        r##"scene_view.rs: "#" ×1"##,
        "scene_view.rs: '#' ×1",
    ];
    let found = tally(hits(&patterns));
    only_listed("links and tags read by a pattern", &found, &left);
}

/// The extensions the app builds in, the ones `ext.rs`'s `Registry::load` names.
const BUILT_IN: [&str; 4] = ["agenda.rs", "ext/nvim.rs", "graph_ext.rs", "type_table.rs"];

/// One file's code, by its path under `src/`.
fn file<'a>(sources: &'a [(String, String)], path: &str) -> &'a str {
    let found = sources.iter().find(|(name, _)| name == path);
    &found.unwrap_or_else(|| panic!("no {path} under src/")).1
}

/// What `code` uses beyond the gateway: every module of the app it names (`crate::…`, or
/// `super::…`, a glob of either) other than `ext`, the host, and `gateway`, the messages; a
/// crate of the project named directly; a terminal crate. Each once.
fn beyond_the_gateway(code: &str) -> Vec<String> {
    let mut found = Vec::new();
    for root in ["crate::", "super::"] {
        let through =
            |module: &String| root == "crate::" && ["ext", "gateway"].contains(&module.as_str());
        let modules = named_in(code, root).into_iter();
        let modules = modules.filter(|module| !through(module));
        found.extend(modules.map(|module| format!("{root}{module}")));
        if code.contains(&format!("{root}*")) {
            found.push(format!("{root}*"));
        }
    }
    for other in [
        "cold_lava_engine",
        "cold_lava_host",
        "cold_lava_views",
        "cold_lava_editor",
        "cold_lava_canvas",
        "ratatui",
        "crossterm",
    ] {
        if code.contains(other) {
            found.push(other.to_string());
        }
    }
    found.sort();
    found.dedup();
    found
}

/// A built-in extension is meant to reach a workspace as an installed one does, through the
/// gateway with `Host`, so it could move to the host and every frontend have it. What each
/// uses beyond that today, by module (`crate::items` in `main.rs` is the engine's `items`):
///
/// - `agenda.rs`, `type_table.rs`: `items` and `types`, the engine's types that the
///   gateway's answers hold; `list` and `table`, the terminal's names for the views' data;
///   `looks`, how a value is drawn; `tasks`, for today's date; `type_table`, whose `Feed` the
///   agenda follows items with.
/// - `graph_ext.rs`: `links`, the engine's types in `links/query`'s answer; `graph`, `groups`
///   and `scene_view`, the graph's layout, its colour groups and the terminal's scene, which
///   bring ratatui with them; `commands`, the graph's command table.
/// - `ext/nvim.rs`: `core`, for `Jump`. Neovim runs in a terminal pane, so it stays here.
///
/// It reads the names, not what is done with them: a module listed here can be used for
/// more without this failing, and a path that names the crate root another way
/// (`use crate as app`) is not seen.
#[test]
fn the_built_in_extensions_use_this_beyond_the_gateway() {
    let all = sources();
    let ext = file(&all, "ext.rs");
    let loaded = ext.matches("found.push(Box::new(").count();
    assert_eq!(loaded, BUILT_IN.len(), "Registry::load's are BUILT_IN");
    let mut found = Vec::new();
    for path in BUILT_IN {
        let module = path.trim_end_matches(".rs").rsplit('/').next().unwrap();
        let named = ext.contains(&format!("{module}::"));
        assert!(named, "Registry::load names {module}");
        let uses = beyond_the_gateway(file(&all, path)).into_iter();
        found.extend(uses.map(|what| format!("{path}: {what}")));
    }
    let left = [
        "agenda.rs: crate::items",
        "agenda.rs: crate::list",
        "agenda.rs: crate::looks",
        "agenda.rs: crate::tasks",
        "agenda.rs: crate::type_table",
        "agenda.rs: crate::types",
        "ext/nvim.rs: crate::core",
        "graph_ext.rs: crate::commands",
        "graph_ext.rs: crate::graph",
        "graph_ext.rs: crate::groups",
        "graph_ext.rs: crate::links",
        "graph_ext.rs: crate::scene_view",
        "type_table.rs: crate::items",
        "type_table.rs: crate::list",
        "type_table.rs: crate::table",
        "type_table.rs: crate::types",
    ];
    only_listed("the built-in extensions beyond the gateway", &found, &left);
}

/// A group inside a `use` group doesn't end it: the names after it are read too.
#[test]
fn the_scan_reads_a_nested_group_to_its_end() {
    let nested = "use crate::{agenda, ext::{self, Host}, items::Item, looks};\n";
    assert_eq!(
        named_in(nested, "crate::"),
        ["agenda", "ext", "items", "looks"]
    );
    let beyond = ["crate::agenda", "crate::items", "crate::looks"];
    assert_eq!(beyond_the_gateway(nested), beyond);
    let around = "use super::tasks;\nuse crate::*;\nuse ratatui::text::Line;\n";
    let beyond = ["crate::*", "ratatui", "super::tasks"];
    assert_eq!(beyond_the_gateway(around), beyond);
    let open = "use crate::trash::{delete, restore";
    assert_eq!(named_in(open, "trash::"), Vec::<String>::new());
}

/// An item edit is seen imported or by its path, a reader of `tasks` is not one, and a
/// tally says how often.
#[test]
fn the_scan_sees_an_item_edit_and_counts() {
    let sample = "use crate::tasks::{toggle, today};\nfn f() {\n    edit::plan(a);\n    \
                  tasks::agenda(w);\n    toggle(p);\n}\n";
    assert_eq!(item_writes(sample), ["edit::plan", "tasks::toggle"]);
    let twice = ["a: x", "a: x", "b: y"].map(String::from).to_vec();
    assert_eq!(tally(twice), ["a: x ×2", "b: y ×1"]);
}

#[test]
#[should_panic(expected = "found but not listed: [\"a: x ×2\"]")]
fn a_new_use_fails_its_list() {
    only_listed("sample", &["a: x ×2".to_string()], &["a: x ×1"]);
}

#[test]
#[should_panic(expected = "listed but not found: [\"b: y ×1\"]")]
fn an_entry_that_is_gone_fails_its_list() {
    only_listed("sample", &["a: x ×1".to_string()], &["a: x ×1", "b: y ×1"]);
}

/// The ways around the lists of #270 are refused as the file-changing modules' are.
#[test]
fn the_scan_refuses_the_ways_around_the_index_the_edits_and_the_operations() {
    let alias = "use crate::{index as ix, tasks as t};\n";
    assert_eq!(sneaked(alias), ["tasks as", "index as"]);
    let glob = "use crate::edit::*;\nfn f() {\n    set_field(a);\n}\n";
    assert_eq!(sneaked(glob), ["edit::*"]);
    let bare = "use crate::merge::{self, undo};\nfn f() {\n    undo(m);\n}\n";
    assert_eq!(sneaked(bare), ["use merge::undo"]);
    let fine = "use crate::merge::{self, Merged};\nuse crate::{index, tasks};\n";
    assert_eq!(sneaked(fine), Vec::<String>::new());
}
