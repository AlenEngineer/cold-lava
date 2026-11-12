//! One door into the engine (#193): what the terminal app does to a workspace's files goes
//! through the core, with the gateway's messages. This reads the app's own source, every file
//! under `src/`, so a direct call can't come back. Each list below is everything of its kind
//! the source names: a new one fails here, and a later group of #193 takes its own off.
//!
//! The same for what else still goes around the door (#270, track B): reads of the core's
//! index, a workspace opened beside the core, item edits that don't go through
//! `Core::edit_item`, links read by a pattern instead of by the Markdown owner, and what the
//! built-in extensions use beyond the gateway. Those lists say how wide the gap is today and
//! fail when it grows, and they fail too when an entry is gone and still listed, so they stay
//! true as they shrink.
//!
//! It is a guard on the text, not a proof. It can't see a call made through a wrapper or a
//! re-export in another crate, through a macro, a function pointer or a trait object, a
//! `std::fs` handle already open (`File`'s own methods), a module of `std` under another name
//! (`use std::fs as f`), or a program the app starts. A reviewer still reads a change that
//! touches files.
//!
//! It reads the terminal app, and the host only for its reads of the core's index. Nothing
//! else guards the host or the engine's own feature modules (#226's test isn't written), and
//! nothing here reads `crates/gui`, which is outside the Cargo workspace and scans a
//! workspace itself (`crates/gui/src/main.rs`, `workspace::scan`).

use std::path::Path;

/// Every `.rs` file under `dir` by its path there, as `code` keeps it, sorted.
fn files_under(dir: &Path) -> Vec<(String, String)> {
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
    let mut out = Vec::new();
    walk(dir, dir, &mut out);
    out.sort();
    out
}

/// Every source file of the binary by its path under `src/`, as `code` keeps it, sorted.
fn sources() -> Vec<(String, String)> {
    files_under(&Path::new(env!("CARGO_MANIFEST_DIR")).join("src"))
}

/// The extension host's source files, each named from here (`../host/src/lib.rs`).
fn host_sources() -> Vec<(String, String)> {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("../host/src");
    let from_here = |(file, code): (String, String)| (format!("../host/src/{file}"), code);
    files_under(&src).into_iter().map(from_here).collect()
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

/// `module::name` for each function of `modules` that `code` names.
fn functions_of(code: &str, modules: &[&str]) -> Vec<String> {
    let mut found = Vec::new();
    for module in modules {
        let names = named_in(code, module).into_iter();
        found.extend(names.map(|name| format!("{module}{name}")));
    }
    found
}

/// Each of `needles` as often as `code` has it.
fn times(code: &str, needles: &[&str]) -> Vec<String> {
    let mut found = Vec::new();
    for needle in needles {
        let times = code.matches(needle).count();
        found.extend((0..times).map(|_| String::from(*needle)));
    }
    found
}

/// `file: what` for everything `found` gives of each file's code, sorted.
fn per_file(files: &[(String, String)], found: impl Fn(&str) -> Vec<String>) -> Vec<String> {
    let mut all = Vec::new();
    for (file, code) in files {
        let whats = found(code).into_iter();
        all.extend(whats.map(|what| format!("{file}: {what}")));
    }
    all.sort();
    all
}

/// `file: what` for every time a file's code has one of `needles`, sorted.
fn hits(needles: &[&str]) -> Vec<String> {
    per_file(&sources(), |code| times(code, needles))
}

/// The entries sorted, each once with how often it was there: `home.rs: .index ×39`.
fn tally(mut found: Vec<String>) -> Vec<String> {
    found.sort();
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
/// and which are listed but no longer found, with what to do about each. A count that
/// changed shows as both.
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
        "{what}\n  found but not listed: {new:?}\n    send it through the core, or list it \
         with its reason: this list only shrinks, so a count is never just raised\n  listed \
         but not found: {gone:?}\n    take it off the list, or lower its count"
    );
    assert_eq!(found, listed, "{what}");
}

/// The engine's modules whose functions change files.
const WRITES: [&str; 2] = ["trash::", "write::"];

/// The gateway's file messages, which are the core's to call.
const GATEWAY_FILES: &str = "files::";

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
    let named = per_file(&sources(), |code| functions_of(code, &WRITES));
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

/// A workspace read without the core: scanned, or an index built, by the app itself.
const BESIDE: [&str; 2] = ["workspace::scan(", "Index::build("];

/// The engine's modules that edit an item or a tag, each with the names the app may take
/// from it that edit nothing (readers, and the modules inside it). Anything else named from
/// one of them counts as an edit: a function a module gains is listed until it is named
/// here.
const ITEM_EDITS: [(&str, &[&str]); 5] = [
    ("edit::", &[]),
    (
        "tasks::",
        &[
            "agenda",
            "first_of_month",
            "last_of_month",
            "locate",
            "today",
        ],
    ),
    ("frontmatter::", &["is_tag"]),
    ("owner::", &["built_in", "canvas", "markdown"]),
    ("owner::markdown::", &["attributes"]),
];

/// The engine's modules the lists are about, each once: the ones of `WRITES`, the gateway's
/// files, and the ones `OPERATIONS`, `BESIDE` and `ITEM_EDITS` name. A list's module is
/// guarded against a way around because the list names it, not because it is written here.
fn guarded() -> Vec<&'static str> {
    let writes = WRITES.into_iter().chain([GATEWAY_FILES]);
    let calls = OPERATIONS.into_iter().chain(BESIDE);
    let calls = calls.filter_map(|call| call.split_once("::"));
    let calls = calls.map(|(module, _)| module);
    let edits = ITEM_EDITS.into_iter().map(|(module, _)| module);
    let mut modules: Vec<&str> = Vec::new();
    for module in writes.chain(calls).chain(edits) {
        let module = module.trim_end_matches("::");
        if module.starts_with(char::is_lowercase) && !modules.contains(&module) {
            modules.push(module);
        }
    }
    modules
}

/// `module as`, when `code` gives the module another name: after a path, in a group, or on a
/// line of its own in one.
fn renamed(code: &str, module: &str) -> Option<String> {
    let alias = format!("{module} as ");
    let is_one = |at: usize| {
        let before = &code[..at];
        let line = before.rsplit('\n').next().unwrap_or_default();
        let after_path = before.ends_with("::") || before.ends_with('{');
        after_path || before.ends_with(", ") || line.trim().is_empty()
    };
    let found = code.match_indices(&alias).any(|(at, _)| is_one(at));
    found.then(|| format!("{module} as"))
}

/// `module::*`, when `code` imports the module whole.
fn globbed(code: &str, module: &str) -> Option<String> {
    let glob = format!("{module}::*");
    code.contains(&glob).then_some(glob)
}

/// Whether an entry of a `use` group renames the module (`self as m`) or a function of it
/// (`delete as d`). A type under another name (`Entry as Bookmark`) is not a call.
fn renames(entry: &str) -> bool {
    let word = |c: &char| c.is_alphanumeric() || *c == '_';
    entry.match_indices(" as ").any(|(at, _)| {
        let backwards = entry[..at].chars().rev().take_while(word);
        backwards.last().is_some_and(char::is_lowercase)
    })
}

/// `module::{… as …}`, when a `use` group of the module renames it or one of its functions.
fn renamed_inside(code: &str, module: &str) -> Option<String> {
    let open = format!("{module}::{{");
    let found = code.match_indices(&open).any(|(at, _)| {
        let entries = group(&code[at + open.len()..]).unwrap_or_default();
        entries.into_iter().any(renames)
    });
    found.then(|| format!("{module}::{{… as …}}"))
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

/// `use std::fs::name` for each `std::fs` function that changes files and is brought in with
/// `use`, after which its bare name is the call.
fn imported_std(code: &str) -> Vec<String> {
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
    let names = imported(code, "fs::").into_iter();
    let names = names.filter(|name| changes.contains(&name.as_str()));
    names.map(|name| format!("use std::fs::{name}")).collect()
}

/// `use module::name` for each call of `OPERATIONS` and `BESIDE` brought in with `use`, after
/// which its bare name is the call and the lists' `module::name(` doesn't find it.
fn imported_calls(code: &str) -> Vec<String> {
    let mut found = Vec::new();
    for call in OPERATIONS.into_iter().chain(BESIDE) {
        let path = call.trim_end_matches('(').split_once("::");
        let (module, name) = path.unwrap_or_default();
        let names = imported(code, &format!("{module}::"));
        if names.iter().any(|n| n == name) {
            found.push(format!("use {module}::{name}"));
        }
    }
    found
}

/// The ways around the lists that the scan refuses outright: a guarded module under another
/// name, imported whole, or renamed in its own `use` group, and a `std::fs` function that
/// changes files or a listed call brought in with `use`.
fn sneaked(code: &str) -> Vec<String> {
    let mut found = Vec::new();
    for module in guarded() {
        found.extend(renamed(code, module));
        found.extend(globbed(code, module));
        found.extend(renamed_inside(code, module));
    }
    found.extend(imported_std(code));
    found.extend(imported_calls(code));
    found
}

/// Nothing goes around the lists: no alias or glob of a module a list is about (`guarded`),
/// no `std::fs` function that changes files and no listed call imported by name, none of the
/// gateway's own file functions (`files::delete` and the rest, which are the core's to call),
/// no file made or copied another way, and no `Core` taken apart for its `index`
/// (`let Core { index, .. } = …`).
#[test]
fn nothing_goes_around_the_lists() {
    let own = ["delete", "restore", "create_folder", "create", "replace"];
    let found = per_file(&sources(), |code| {
        let gateway = named_in(code, GATEWAY_FILES).into_iter();
        let gateway = gateway.filter(|name| own.contains(&name.as_str()));
        let mut found = sneaked(code);
        found.extend(gateway.map(|name| format!("{GATEWAY_FILES}{name}")));
        found
    });
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

/// The core's index is read from outside the core here: every `.index` in the app's code
/// and in the host's, whatever it is written on (`self.core.index`, `c.index`, a `&Core`
/// under any name). Today each one is a `Core`'s field: a field of that name on anything
/// else would count too, and is to be named otherwise. The field can become private when
/// the list is empty (#270, B6), the host's two reads included.
///
/// What is counted is the reach, not the use: `let ix = &core.index`, then `ix` forty times,
/// is one. A reach the text doesn't show is not seen: through a function of the engine that
/// hands the index out, or a `Core` taken apart in a pattern other than a `let`.
///
/// - `home.rs`: the menu reads the tree, the switcher, search, backlinks, tags, the reader's
///   task and what a rename, a merge, a recipe or a delete would change, all from the index.
/// - `main.rs`: `cold-lava rename` and `recipe` plan on the core's index, and `graph` asks it
///   whether there are notes.
/// - `owned.rs`: the command line's workspace with its owned files, and `item set`'s item.
/// - The host's `lib.rs`: `Host` answers `types/list` and `workspace/style` from it.
#[test]
fn the_index_is_still_read_outside_the_core_here() {
    let mut files = sources();
    files.extend(host_sources());
    let found = tally(per_file(&files, |code| times(code, &[".index"])));
    let left = [
        "../host/src/lib.rs: .index ×2",
        "home.rs: .index ×39",
        "main.rs: .index ×3",
        "owned.rs: .index ×2",
    ];
    only_listed("reads of the core's index", &found, &left);
}

/// A workspace is opened once, as a `Core`, so every command sees the same files, the owned
/// ones too. Where the app still scans one itself or builds an index of its own (#270, B3):
///
/// - `main.rs`: `tidy` and `search` scan and build an index; `style` scans.
/// - `owned.rs`: the command line's workspace when no extension owns a kind of file.
///
/// A type of the engine's `index` module that a function takes (`index::Backlink`) is not
/// counted: it is data the core gave. `Index::build` reached through another name for the
/// type (`use crate::index::Index as I`) is not seen.
#[test]
fn a_workspace_is_still_opened_beside_the_core_here() {
    let left = [
        "main.rs: Index::build( ×2",
        "main.rs: workspace::scan( ×3",
        "owned.rs: workspace::scan( ×1",
    ];
    only_listed(
        "a workspace opened beside the core",
        &tally(hits(&BESIDE)),
        &left,
    );
}

/// What `code` names of the modules of `ITEM_EDITS` apart from what edits nothing.
fn item_edits(code: &str) -> Vec<String> {
    let mut found = Vec::new();
    for (module, apart) in ITEM_EDITS {
        let names = named_in(code, module).into_iter();
        let names = names.filter(|name| !apart.contains(&name.as_str()));
        found.extend(names.map(|name| format!("{module}{name}")));
    }
    found
}

/// An item is changed with `items/edit` (`Core::edit_item`, or `Host::edit_item` for an
/// extension), which asks the file's owner and tells subscribers. What still makes an
/// item's or a tag's new text itself, or writes it:
///
/// - `edit::set_field`: `cold-lava item set` on a note (#270, B3).
/// - `tasks::toggle_in`: `cold-lava task toggle` (#270, B3).
/// - `frontmatter::edit_tag`: the menu's tag edit, which reads the note and writes it with
///   `write::replace` itself.
///
/// The owners' `edit` functions (`owner::edit`, `owner::markdown::edit`) would be listed;
/// the app names neither.
#[test]
fn items_are_still_edited_around_the_core_here() {
    let found = tally(per_file(&sources(), item_edits));
    let left = [
        "home.rs: frontmatter::edit_tag ×1",
        "main.rs: edit::set_field ×1",
        "main.rs: tasks::toggle_in ×1",
    ];
    only_listed("item edits around Core::edit_item", &found, &left);
}

/// A link is what the Markdown owner and the resolver say is one, never what a pattern of
/// the app's finds. This lists where the app's code spells a link's syntax to look for it: a
/// regular expression at all (`regex::`, `Regex::new(`), a wikilink's or a Markdown link's
/// brackets escaped for one (`\[\[`, `\]\(`), the brackets as a string to find (`"[["`,
/// `"]]"`, `"]("`), and the two readers there are by name, wherever they are named:
///
/// - `agenda.rs`: `links_as_text`, two regular expressions that turn `[[Note|alias]]` and
///   `[text](url)` into the words shown; `home.rs` calls it for the reader's tick messages.
/// - `board.rs`: `markdown_text`, a card's Markdown styled by hand; `home.rs` draws a
///   backlink's context with it.
/// - `ext/nvim.rs`: the Vim script's pattern for the link under the cursor.
///
/// What it doesn't see: a tag read by its `#`; a scan by character (`'['`, a byte) or by a
/// longer string that holds the brackets; and the parts of a link read without its brackets,
/// which today are `reader.rs`'s `embed_label` (`split('|')` on an embed's target) and, in
/// `board.rs`, what `markdown_text` does past the brackets, on the list only by its name.
#[test]
fn links_are_still_read_by_pattern_here() {
    let patterns = [
        "regex::",
        "Regex::new(",
        r"\[\[",
        r"\]\(",
        r#""[[""#,
        r#""]]""#,
        r#""](""#,
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
        r#"board.rs: "]]" ×1"#,
        "board.rs: markdown_text ×3",
        r"ext/nvim.rs: \[\[ ×1",
        "home.rs: links_as_text ×2",
        "home.rs: markdown_text ×1",
    ];
    let found = tally(hits(&patterns));
    only_listed("links read by a pattern", &found, &left);
}

/// The extensions the app builds in, the ones `ext.rs`'s `Registry::load` names.
const BUILT_IN: [&str; 4] = ["agenda.rs", "ext/nvim.rs", "graph_ext.rs", "type_table.rs"];

/// The door, as a built-in extension names it: `ext` is the host (`pub use
/// cold_lava_host::*`) and `gateway` the messages; `items`, `types` and `links` are the
/// engine's types the gateway's answers hold (`Item`, a type's `Definition`, `LinkGraph`);
/// `list` and `table` are where the terminal names the views' data, re-exported from
/// `cold_lava_views`. It allows by module: `list.rs` and `table.rs` also hold how the
/// terminal draws them, which a built-in could name without this seeing it.
const DOOR: [&str; 7] = ["ext", "gateway", "items", "links", "list", "table", "types"];

/// One file's code, by its path under `src/`.
fn file<'a>(sources: &'a [(String, String)], path: &str) -> &'a str {
    let found = sources.iter().find(|(name, _)| name == path);
    &found.unwrap_or_else(|| panic!("no {path} under src/")).1
}

/// What the code of the file at `path` uses beyond the door, each once: every module it
/// names that isn't one of `DOOR`, from the app (`crate::…`; `super::…` too in a file at the
/// top of `src/`, where it is the same) or from the engine by its crate's name, a glob of
/// either, another crate of the project, a terminal crate. `cold_lava_views` and
/// `cold_lava_host` are the door under their own names. In a file inside a module
/// (`ext/nvim.rs`) `super::` is that module, here `ext`, and isn't read.
fn beyond_the_gateway(path: &str, code: &str) -> Vec<String> {
    let mut roots = vec!["crate::", "cold_lava_engine::"];
    if !path.contains('/') {
        roots.push("super::");
    }
    let mut found = Vec::new();
    for root in roots {
        let modules = named_in(code, root).into_iter();
        let modules = modules.filter(|module| !DOOR.contains(&module.as_str()));
        found.extend(modules.map(|module| format!("{root}{module}")));
        found.extend(globbed(code, root.trim_end_matches("::")));
    }
    let others = [
        "cold_lava_editor",
        "cold_lava_canvas",
        "ratatui",
        "crossterm",
    ];
    let others = others.into_iter().filter(|other| code.contains(other));
    found.extend(others.map(String::from));
    found.sort();
    found.dedup();
    found
}

/// A built-in extension is meant to reach a workspace as an installed one does, through the
/// gateway with `Host`, so it could move to the host and every frontend have it. What each
/// uses beyond the door (`DOOR`) today, by module:
///
/// - `agenda.rs`: `looks`, how a value is drawn; `tasks`, for today's date; `type_table`,
///   whose `Feed` it follows items with.
/// - `graph_ext.rs`: `graph`, `groups` and `scene_view`, the graph's layout, its colour
///   groups and the terminal's scene, which bring ratatui with them; `commands`, the graph's
///   command table.
/// - `ext/nvim.rs`: `core`, for `Jump`. Neovim runs in a terminal pane, so it stays here.
///
/// It reads the names, not what is done with them: a module listed here can be used for
/// more without this failing, and a path that names the crate root another way
/// (`use crate as app`) is not seen. This is not #226's test, which is about the engine's
/// feature modules reaching the core's internals and isn't written yet.
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
        let uses = beyond_the_gateway(path, file(&all, path)).into_iter();
        found.extend(uses.map(|what| format!("{path}: {what}")));
    }
    let left = [
        "agenda.rs: crate::looks",
        "agenda.rs: crate::tasks",
        "agenda.rs: crate::type_table",
        "ext/nvim.rs: crate::core",
        "graph_ext.rs: crate::commands",
        "graph_ext.rs: crate::graph",
        "graph_ext.rs: crate::groups",
        "graph_ext.rs: crate::scene_view",
    ];
    only_listed("the built-in extensions beyond the gateway", &found, &left);
}

/// A group inside a `use` group doesn't end it: the names after it are read too. The door's
/// own modules are not beyond it, under the app's name or their crate's.
#[test]
fn the_scan_reads_a_nested_group_to_its_end() {
    let nested = "use crate::{agenda, ext::{self, Host}, items::Item, looks};\n";
    assert_eq!(
        named_in(nested, "crate::"),
        ["agenda", "ext", "items", "looks"]
    );
    let beyond = ["crate::agenda", "crate::looks"];
    assert_eq!(beyond_the_gateway("agenda.rs", nested), beyond);
    let around = "use super::tasks;\nuse crate::*;\nuse ratatui::text::Line;\n\
                  use cold_lava_views::list::List;\nuse cold_lava_engine::{items, write};\n";
    let beyond = [
        "cold_lava_engine::write",
        "crate::*",
        "ratatui",
        "super::tasks",
    ];
    assert_eq!(beyond_the_gateway("agenda.rs", around), beyond);
    let inside = ["cold_lava_engine::write", "crate::*", "ratatui"];
    assert_eq!(beyond_the_gateway("ext/nvim.rs", around), inside);
    let open = "use crate::trash::{delete, restore";
    assert_eq!(named_in(open, "trash::"), Vec::<String>::new());
}

/// An item edit is seen imported or by its path, in an owner or the frontmatter too; a
/// reader is not one; and a tally sorts and says how often.
#[test]
fn the_scan_sees_an_item_edit_and_counts() {
    let sample = "use crate::tasks::{toggle, today};\nfn f() {\n    edit::plan(a);\n    \
                  tasks::agenda(w);\n    toggle(p);\n    owner::markdown::edit(x);\n    \
                  owner::built_in(r);\n    frontmatter::edit_tag(t);\n}\n";
    let edits = [
        "edit::plan",
        "tasks::toggle",
        "frontmatter::edit_tag",
        "owner::markdown::edit",
    ];
    assert_eq!(item_edits(sample), edits);
    let twice = ["b: y", "a: x", "a: x"].map(String::from).to_vec();
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

/// Every module a list is about is guarded, the newer lists' as the file-changing ones'.
#[test]
fn every_lists_module_is_guarded() {
    let modules = [
        "trash",
        "write",
        "files",
        "merge",
        "rename",
        "recipe",
        "bookmarks",
        "daily",
        "workspace",
        "edit",
        "tasks",
        "frontmatter",
        "owner",
        "owner::markdown",
    ];
    assert_eq!(guarded(), modules);
}

/// The ways around the lists of #270 and around the operations are refused as the
/// file-changing modules' are, and a type under another name is not one.
#[test]
fn the_scan_refuses_the_ways_around_the_operations_the_edits_and_the_scan() {
    let alias = "use crate::rename as r;\nfn f() {\n    r::apply(p);\n}\n";
    assert_eq!(sneaked(alias), ["rename as"]);
    let lines = "use crate::{\n    frontmatter,\n    tasks as t,\n};\n";
    assert_eq!(sneaked(lines), ["tasks as"]);
    let glob = "use crate::recipe::*;\nfn f() {\n    apply(p);\n}\n";
    assert_eq!(sneaked(glob), ["recipe::*"]);
    let edit = "use crate::edit::{set_field as set, SetField};\n";
    assert_eq!(sneaked(edit), ["edit::{… as …}"]);
    let deep = "use crate::{owner::{self, markdown::{self as md}}, workspace::{Note, scan}};\n";
    assert_eq!(sneaked(deep), ["owner::{… as …}", "use workspace::scan"]);
    let path = "use crate::owner::markdown::{self as md};\n";
    assert_eq!(sneaked(path), ["owner::markdown::{… as …}"]);
    let bare = "use crate::merge::{self, undo};\nfn f() {\n    undo(m);\n}\n";
    assert_eq!(sneaked(bare), ["use merge::undo"]);
    let fine = "use crate::bookmarks::{self, Entry as Bookmark, Target};\n\
                use crate::merge::{self, Merged};\nuse crate::{rename, tasks, workspace};\n";
    assert_eq!(sneaked(fine), Vec::<String>::new());
}
