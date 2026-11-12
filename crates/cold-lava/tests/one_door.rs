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
        for name in entries(rest) {
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

/// What follows a path's `::`: a group's entries, or the one name there.
fn entries(rest: &str) -> Vec<&str> {
    let grouped = rest.strip_prefix('{').and_then(group);
    grouped.unwrap_or_else(|| vec![rest])
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

/// The engine's modules that edit an item or a tag. The list counts every function the app
/// reaches under one of them, at any depth, except the named `READERS`: a module inside one
/// is never an entry and never an exemption, and a type or a constant counts for nothing.
const ITEM_EDITS: [&str; 4] = ["edit", "tasks", "frontmatter", "owner"];

/// The functions under `ITEM_EDITS` that make no file's new text and write nothing, each
/// read in the engine's source. A function those modules gain is listed until it is named
/// here: `tasks`' are dates, the agenda and `FILE:LINE` taken apart; `is_tag` tests a name;
/// `built_in` names a kind's owner; `places` names a canvas's nodes; the `attributes` two
/// keep the configured readings in memory and load a workspace's.
const READERS: [&str; 10] = [
    "tasks::agenda",
    "tasks::first_of_month",
    "tasks::last_of_month",
    "tasks::locate",
    "tasks::today",
    "frontmatter::is_tag",
    "owner::built_in",
    "owner::canvas::places",
    "owner::markdown::attributes::for_workspace",
    "owner::markdown::attributes::set_configured",
];

/// The names the lists are about, each once: the modules of `WRITES`, the gateway's files,
/// the modules (and `Index`) that `OPERATIONS` and `BESIDE` call into, and `ITEM_EDITS`. A
/// list's module is guarded against a way around because the list names it, not because it
/// is written here.
fn guarded() -> Vec<&'static str> {
    let writes = WRITES.into_iter().chain([GATEWAY_FILES]);
    let calls = OPERATIONS.into_iter().chain(BESIDE);
    let calls = calls.filter_map(|call| call.split_once("::"));
    let calls = calls.map(|(module, _)| module);
    let mut names: Vec<&str> = Vec::new();
    for name in writes.chain(calls).chain(ITEM_EDITS) {
        let name = name.trim_end_matches("::");
        if !names.contains(&name) {
            names.push(name);
        }
    }
    names
}

/// One entry of a `use`, spelled out into `paths`: `a::{b, c::{self as d}}` after `x::` is
/// `x::a::b` and `x::a::c::self as d`.
fn spell_out(before: &str, tree: &str, paths: &mut Vec<String>) {
    let tree = tree.trim();
    match tree.split_once('{') {
        Some((path, inside)) => {
            let before = format!("{before}{}", path.trim());
            for entry in group(inside).unwrap_or_default() {
                spell_out(&before, entry, paths);
            }
        }
        None if tree.is_empty() => {}
        None => paths.push(format!("{before}{tree}")),
    }
}

/// Every path a `use` in `code` brings in, its groups spelled out. Only a `use` is read: a
/// cast (`files as u64`), a string or a comment that says `rename as` is not one.
fn uses(code: &str) -> Vec<String> {
    let mut paths = Vec::new();
    for statement in code.split(';') {
        let is_use = |l: &&str| {
            let l = l.trim_start();
            l.starts_with("use ") || l.starts_with("pub use ") || l.starts_with("pub(crate) use ")
        };
        let lines: Vec<&str> = statement.lines().skip_while(|l| !is_use(l)).collect();
        let item = lines.join(" ");
        if let Some((_, tree)) = item.split_once("use ") {
            spell_out("", tree, &mut paths);
        }
    }
    paths
}

/// The names of a spelled-out path, and whether the last is given another name (`as`).
fn segments(path: &str) -> (Vec<&str>, bool) {
    let (path, alias) = match path.split_once(" as ") {
        Some((path, _)) => (path, true),
        None => (path, false),
    };
    (path.split("::").map(str::trim).collect(), alias)
}

/// `module as`, when the path gives a guarded name another name, and `module::{… as …}`,
/// when it renames something under one: the module as `self`, a module inside it or a
/// function, at any depth, reported under the outermost guarded name. A type under another
/// name (`Entry as Bookmark`) is not a call.
fn renamed(path: &str) -> Option<String> {
    let (names, alias) = segments(path);
    let guarded = guarded();
    let module = names.iter().find(|name| guarded.contains(*name))?;
    let last = names.last()?;
    match (alias, last == module, last.starts_with(char::is_lowercase)) {
        (true, true, _) => Some(format!("{module} as")),
        (true, false, true) => Some(format!("{module}::{{… as …}}")),
        _ => None,
    }
}

/// `module::*`, when the path imports everything of a guarded module or of a module in it.
fn globbed(path: &str) -> Option<String> {
    let (names, _) = segments(path);
    let guarded = guarded();
    let module = names.iter().find(|name| guarded.contains(*name))?;
    names.ends_with(&["*"]).then(|| format!("{module}::*"))
}

/// `use std::fs::name`, when the path brings in a `std::fs` function that changes files,
/// after which its bare name is the call.
fn imported_std(path: &str) -> Option<String> {
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
    let (names, _) = segments(path);
    let [.., module, name] = names.as_slice() else {
        return None;
    };
    let is_one = *module == "fs" && changes.contains(name);
    is_one.then(|| format!("use std::fs::{name}"))
}

/// `use module::name`, when the path brings in a call of `OPERATIONS` or `BESIDE`, after
/// which its bare name is the call and the lists' `module::name(` doesn't find it.
fn imported_call(path: &str) -> Option<String> {
    let (names, _) = segments(path);
    let [.., module, name] = names.as_slice() else {
        return None;
    };
    let call = format!("{module}::{name}(");
    let listed = OPERATIONS.into_iter().chain(BESIDE).any(|c| c == call);
    listed.then(|| format!("use {module}::{name}"))
}

/// The ways around the lists that the scan refuses outright, read from the `use`s alone: a
/// guarded name under another name or imported whole, and a `std::fs` function that changes
/// files or a listed call brought in by name.
fn sneaked(code: &str) -> Vec<String> {
    let mut found = Vec::new();
    for path in uses(code) {
        found.extend(renamed(&path));
        found.extend(globbed(&path));
        found.extend(imported_std(&path));
        found.extend(imported_call(&path));
    }
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

/// Whether the engine has a module `name` inside `module` (`owner`, `owner::markdown`): a
/// file or a folder of that name in its source.
fn is_module(module: &str, name: &str) -> bool {
    let engine = Path::new(env!("CARGO_MANIFEST_DIR")).join("../engine/src");
    let inside = engine.join(module.replace("::", "/"));
    inside.join(format!("{name}.rs")).is_file() || inside.join(name).is_dir()
}

/// What follows each `name::` in `code` where that is a path of its own: not the end of a
/// longer name (`json_canvas::`), and, for a module read by its `bare` name, not a step of a
/// longer path (`owner::canvas::`), which is read from its start.
fn paths_from<'a>(code: &'a str, name: &str, bare: bool) -> Vec<&'a str> {
    let text = format!("{name}::");
    let joins = |c: char| c.is_alphanumeric() || c == '_' || (bare && c == ':');
    let own = |at: &usize| !code[..*at].chars().next_back().is_some_and(joins);
    let at = code.match_indices(&text).map(|(at, _)| at).filter(own);
    at.map(|at| &code[at + text.len()..]).collect()
}

/// One entry under the engine's `module`: a function goes into `found` by its path; a
/// module inside is gone into; one brought in by its bare name (`use owner::canvas`,
/// `canvas::{self}`) goes into `imported`, to be read by that name. A type is nothing.
fn reach(entry: &str, module: &str, found: &mut Vec<String>, imported: &mut Vec<(String, String)>) {
    let entry = entry.trim_start();
    let word = |c: &char| c.is_alphanumeric() || *c == '_';
    let name: String = entry.chars().take_while(word).collect();
    let path = format!("{module}::{name}");
    let bare = match (name.as_str(), module.rsplit_once("::")) {
        ("self", Some((_, last))) => Some((last.to_string(), module.to_string())),
        ("self", None) => None,
        _ if !name.starts_with(char::is_lowercase) => None,
        _ if !is_module(module, &name) => {
            found.push(path);
            None
        }
        _ => match entry[name.len()..].strip_prefix("::") {
            Some(rest) => {
                for inside in entries(rest) {
                    reach(inside, &path, found, imported);
                }
                None
            }
            None => Some((name.clone(), path)),
        },
    };
    if let Some(bare) = bare
        && !imported.contains(&bare)
    {
        imported.push(bare);
    }
}

/// Every function `code` reaches under the engine's `module`, at any depth, by its path
/// from the module (`owner::canvas::edit`), as often as it is named, sorted: by a path from
/// the module, in a `use`, or through a module inside it that a `use` brought in. A function
/// of a type (`Layer::config`) is the type's, and not seen.
fn reached(code: &str, module: &str) -> Vec<String> {
    let mut found = Vec::new();
    let mut imported = Vec::new();
    for rest in paths_from(code, module, false) {
        for entry in entries(rest) {
            reach(entry, module, &mut found, &mut imported);
        }
    }
    let mut next = 0;
    while let Some((name, path)) = imported.get(next).cloned() {
        next += 1;
        for rest in paths_from(code, &name, true) {
            for entry in entries(rest) {
                reach(entry, &path, &mut found, &mut imported);
            }
        }
    }
    found.sort();
    found
}

/// The functions `code` reaches under the modules of `ITEM_EDITS`, apart from `READERS`.
fn item_edits(code: &str) -> Vec<String> {
    let mut found = Vec::new();
    for module in ITEM_EDITS {
        let functions = reached(code, module).into_iter();
        found.extend(functions.filter(|function| !READERS.contains(&function.as_str())));
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
/// An owner's functions that make a file's new text (`owner::edit`, `owner::markdown::edit`,
/// `owner::canvas::edit` and `relink`, `owner::change`'s) would be listed; the app names
/// none.
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
/// `cold_lava_views`. Apart from `ext`, the door is their data, not their functions: a
/// function of one of them (`items::extract`, a second reading of a note) is beyond it.
const DOOR: [&str; 7] = ["ext", "gateway", "items", "links", "list", "table", "types"];

/// One file's code, by its path under `src/`.
fn file<'a>(sources: &'a [(String, String)], path: &str) -> &'a str {
    let found = sources.iter().find(|(name, _)| name == path);
    &found.unwrap_or_else(|| panic!("no {path} under src/")).1
}

/// What the code of the file at `path` uses beyond the door, each once:
///
/// - every module it names that isn't one of `DOOR`, from the crate's root (`crate::…`, or
///   as many `super::` as lead there from the file: one in `agenda.rs`, two in
///   `ext/nvim.rs`, where one alone is `ext`) or from the engine by its crate's name, and a
///   glob of any of them;
/// - every function it reaches under a module of `DOOR` other than `ext`, as `reached`
///   finds it: a lowercase name that follows the module's path. A type's own (`Look::default`,
///   a variant) follows a capital, and is the data;
/// - the crate's root or the engine under another name, another crate of the project, a
///   terminal crate.
///
/// `cold_lava_views` and `cold_lava_host` are the door under their own names.
fn beyond_the_gateway(path: &str, code: &str) -> Vec<String> {
    let up = "super::".repeat(path.matches('/').count() + 1);
    let mut found = Vec::new();
    for root in ["crate::", "cold_lava_engine::", up.as_str()] {
        let modules = named_in(code, root).into_iter();
        let modules = modules.filter(|module| !DOOR.contains(&module.as_str()));
        found.extend(modules.map(|module| format!("{root}{module}")));
        let glob = format!("{root}*");
        found.extend(code.contains(&glob).then_some(glob));
    }
    for data in DOOR.into_iter().filter(|module| *module != "ext") {
        found.extend(reached(code, data));
    }
    let others = [
        "crate as ",
        "cold_lava_engine as ",
        "cold_lava_editor",
        "cold_lava_canvas",
        "ratatui",
        "crossterm",
    ];
    let others = others.into_iter().filter(|other| code.contains(other));
    found.extend(others.map(|other| other.trim_end().to_string()));
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
/// own modules are not beyond it, under the app's name or their crate's, but a function of
/// one is, and so is the crate's root reached by `super::` or under another name.
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
    let data = "use crate::items::{self, Item};\nuse super::super::tasks;\n\
                use cold_lava_engine as e;\nfn f() {\n    items::extract(n, t, s);\n    \
                types::Look::default();\n    let k = types::Colour::Red;\n}\n";
    let beyond = [
        "cold_lava_engine as",
        "items::extract",
        "super::super::tasks",
    ];
    assert_eq!(beyond_the_gateway("ext/nvim.rs", data), beyond);
    let open = "use crate::trash::{delete, restore";
    assert_eq!(named_in(open, "trash::"), Vec::<String>::new());
}

/// An item edit is a function reached under one of the modules, at any depth: by its path,
/// in a `use`, or through a module inside that a `use` brought in. A module inside is no
/// entry and excuses nothing, a type counts for nothing, a reader is not an edit, and a
/// longer name that ends like a module (`json_canvas`) is not that module.
#[test]
fn the_scan_sees_an_item_edit_at_any_depth() {
    let sample = "use crate::tasks::{toggle, today};\n\
                  use cold_lava_engine::owner::given::Piece;\n\
                  use crate::owner::{self, canvas, change::{self, Change}};\n\
                  fn f() {\n    edit::plan(a);\n    tasks::agenda(w);\n    toggle(p);\n    \
                  owner::markdown::edit(x);\n    owner::built_in(r);\n    \
                  owner::canvas::edit(t, c);\n    canvas::relink(t);\n    canvas::places(i);\n    \
                  change::piece(p);\n    frontmatter::edit_tag(t);\n    json_canvas::parse(t);\n}\n";
    let edits = [
        "edit::plan",
        "tasks::toggle",
        "frontmatter::edit_tag",
        "owner::canvas::edit",
        "owner::canvas::relink",
        "owner::change::piece",
        "owner::markdown::edit",
    ];
    assert_eq!(item_edits(sample), edits);
    let types = "use cold_lava_engine::owner::given::{self, Piece, Run};\n\
                 use crate::owner::markdown::attributes;\nfn f(p: given::Piece) {\n    \
                 attributes::Layer::config(a, b);\n    attributes::for_workspace(r);\n}\n";
    assert_eq!(item_edits(types), Vec::<String>::new());
    let read = ["owner::markdown::attributes::for_workspace"];
    assert_eq!(reached(types, "owner"), read);
}

/// A tally sorts and says how often.
#[test]
fn a_tally_counts() {
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

/// Every name a list is about is guarded, the newer lists' as the file-changing ones'.
#[test]
fn every_lists_module_is_guarded() {
    let names = [
        "trash",
        "write",
        "files",
        "merge",
        "rename",
        "recipe",
        "bookmarks",
        "daily",
        "workspace",
        "Index",
        "edit",
        "tasks",
        "frontmatter",
        "owner",
    ];
    assert_eq!(guarded(), names);
}

/// The ways around the lists of #270 and around the operations are refused as the
/// file-changing modules' are. A type under another name is not one, unless a list is about
/// the type, and neither is anything outside a `use`.
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
    let inside = "use crate::owner::canvas as c;\nuse crate::owner::canvas::*;\n";
    assert_eq!(sneaked(inside), ["owner::{… as …}", "owner::*"]);
    let index = "use crate::index::Index as I;\nfn f() {\n    I::build(w);\n}\n";
    assert_eq!(sneaked(index), ["Index as"]);
    let bare = "use crate::merge::{self, undo};\nfn f() {\n    undo(m);\n}\n";
    assert_eq!(sneaked(bare), ["use merge::undo"]);
    let fine = "use crate::bookmarks::{self, Entry as Bookmark, Target};\n\
                use crate::merge::{self, Merged};\nuse crate::{rename, tasks, workspace};\n";
    assert_eq!(sneaked(fine), Vec::<String>::new());
    let no_use = "fn f() {\n    g(a, files as u64);\n    let w = std::fs::write as fn(&str);\n    \
                  let s = \"a, rename as before\";\n}\n";
    assert_eq!(sneaked(no_use), Vec::<String>::new());
}

/// Each of `needles` as often as `code` has it as a name of its own: `println!(` is not
/// counted inside `eprintln!(`, nor `out!(` inside another macro's name.
fn named(code: &str, needles: &[&str]) -> Vec<String> {
    let mut found = Vec::new();
    for needle in needles {
        for (at, _) in code.match_indices(needle) {
            let before = code[..at].chars().next_back();
            if !before.is_some_and(|c| c.is_alphanumeric() || c == '_') {
                found.push(String::from(*needle));
            }
        }
    }
    found
}

/// One door out to the terminal too (#265, track A step A2): text a file or an extension
/// chose is shown only through `outside::escape`, and on the command line that is `say.rs`,
/// the one module that prints. Nothing else names a print macro or takes stdout or stderr,
/// but for the terminal's own protocol, which carries no outside text:
///
/// - `main.rs`: `stdout()` ×4 is `say::stdout()` twice (the long listings of `items` and
///   `table`) and mouse capture switched on and off around the terminal app.
/// - `term.rs`: leaving and entering the alternate screen, and `write_raw`, which writes
///   the Kitty graphics the app made.
/// - `home.rs`: a picture placed or deleted with Kitty graphics.
///
/// It is the same guard on the text as the rest of this file, with the same blind spots: a
/// print through a macro of another crate, `write!` to a handle taken another way
/// (`io::stdout` under another name, a `File` on `/dev/tty`), or a child process that
/// inherits the terminal is not seen. What the terminal app draws is not printed: it goes
/// into ratatui's cells, which this test doesn't read.
#[test]
fn the_app_prints_only_through_say() {
    let prints = [
        "println!(",
        "eprintln!(",
        "print!(",
        "eprint!(",
        "dbg!(",
        "stdout()",
        "stderr()",
    ];
    let found = tally(per_file(&sources(), |code| named(code, &prints)));
    let listed = [
        "home.rs: stdout() ×1",
        "main.rs: stdout() ×4",
        "say.rs: eprintln!( ×1",
        "say.rs: print!( ×1",
        "say.rs: println!( ×4",
        "say.rs: stdout() ×2",
        "term.rs: stdout() ×3",
    ];
    assert_eq!(
        found, listed,
        "a print or a handle on the terminal outside say.rs: print through `out!`, `told!` \
         or `problem!` (src/say.rs says which), so the line is escaped"
    );
}

/// Which lines are listings, the user's own content, where a character that turns the
/// direction of text stays as written: `items`, `table`, `search`, `tidy`, `agenda`,
/// `daily --path`, a shape drawn as text and a recipe's diff in `main.rs`, and the screen
/// `drive` read. Every other line is a message (`told!`, `problem!`), where such a character
/// is escaped. A listing is chosen by calling one of these, so a new one shows here.
#[test]
fn the_listings_are_these() {
    let listings = ["out!(", "listing(", "listing_block("];
    let found = tally(per_file(&sources(), |code| named(code, &listings)));
    let listed = [
        "drive.rs: out!( ×3",
        "main.rs: listing( ×2",
        "main.rs: listing_block( ×2",
        "main.rs: out!( ×19",
        "say.rs: listing( ×4",
        "say.rs: listing_block( ×1",
    ];
    assert_eq!(
        found, listed,
        "a line printed as a listing: it is one only if the line is the user's own content; \
         what a command did or why it didn't is a message (`told!`, `problem!`)"
    );
}
