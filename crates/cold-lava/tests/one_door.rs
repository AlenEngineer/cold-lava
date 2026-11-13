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

/// What follows `mod ` on a line that declares a module, whatever its visibility
/// (`mod x`, `pub mod x`, `pub(crate) mod x`).
fn declares_mod(line: &str) -> Option<&str> {
    let line = line.trim_start();
    let rest = match line.strip_prefix("pub") {
        None => line,
        Some(after) => {
            let after = match after.strip_prefix('(') {
                Some(inside) => inside.split_once(')')?.1,
                None => after,
            };
            after.strip_prefix(' ')?
        }
    };
    rest.strip_prefix("mod ")
}

/// The modules that are tests whole, each as the path of its file without `.rs`: every
/// `#[cfg(test)]` on a `mod name;`, whose file is beside the one that declares it or in the
/// folder of its name.
fn test_modules(files: &[(String, String)]) -> Vec<String> {
    let mut found = Vec::new();
    for (file, text) in files {
        let (dir, name) = file.rsplit_once('/').unwrap_or(("", file.as_str()));
        let base = match ["lib.rs", "main.rs", "mod.rs"].contains(&name) {
            true => dir,
            false => file.trim_end_matches(".rs"),
        };
        let lines: Vec<&str> = text.lines().collect();
        for pair in lines.windows(2) {
            let declared = declares_mod(pair[1]).and_then(|rest| rest.strip_suffix(';'));
            if let (true, Some(module)) = (pair[0].trim() == "#[cfg(test)]", declared) {
                let path = format!("{base}/{module}");
                found.push(path.trim_start_matches('/').to_string());
            }
        }
    }
    found
}

/// Every `.rs` file under `dir` by its path there, as `code` keeps it, sorted. A file that
/// is a test module whole (`test_modules`) is left out, with what is under it.
fn files_under(dir: &Path) -> Vec<(String, String)> {
    fn walk(dir: &Path, src: &Path, out: &mut Vec<(String, String)>) {
        for entry in std::fs::read_dir(dir).unwrap().flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(&path, src, out);
            } else if path.extension().is_some_and(|x| x == "rs") {
                let rel = path.strip_prefix(src).unwrap().to_string_lossy();
                let text = std::fs::read_to_string(&path).unwrap();
                out.push((rel.replace('\\', "/"), text));
            }
        }
    }
    let mut out = Vec::new();
    walk(dir, dir, &mut out);
    let tests = test_modules(&out);
    let is_test = |file: &str| {
        let within = |test: &String| file.strip_prefix(test.as_str());
        let mut rests = tests.iter().filter_map(within);
        rests.any(|rest| rest == ".rs" || rest.starts_with('/'))
    };
    out.retain(|(file, _)| !is_test(file.as_str()));
    for (_, text) in &mut out {
        *text = code(text);
    }
    out.sort();
    out
}

/// Every source file of the binary by its path under `src/`, as `code` keeps it, sorted.
fn sources() -> Vec<(String, String)> {
    files_under(&Path::new(env!("CARGO_MANIFEST_DIR")).join("src"))
}

/// The source files of the crate `name` beside this one, each named from here
/// (`../host/src/lib.rs`).
fn crate_sources(name: &str) -> Vec<(String, String)> {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("../{name}/src"));
    let from_here = |(file, code): (String, String)| (format!("../{name}/src/{file}"), code);
    files_under(&src).into_iter().map(from_here).collect()
}

/// The extension host's source files.
fn host_sources() -> Vec<(String, String)> {
    crate_sources("host")
}

/// A file's text without its unit tests (each `#[cfg(test)]` on a `mod`, whatever its
/// visibility, to the brace that closes it) and without comment lines. What follows a test module is kept.
fn code(text: &str) -> String {
    let mut kept = Vec::new();
    let mut lines = text.lines().peekable();
    while let Some(line) = lines.next() {
        let next = lines.peek().map(|l| l.trim_start());
        if line.trim() == "#[cfg(test)]" && next.is_some_and(|l| declares_mod(l).is_some()) {
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

/// Each of `needles` as often as `code` has it. One that begins with a name is not counted
/// in the middle of a longer name: `println!(` is not found in `eprintln!(`.
fn times(code: &str, needles: &[&str]) -> Vec<String> {
    let in_a_name = |c: char| c.is_alphanumeric() || c == '_';
    let mut found = Vec::new();
    for needle in needles {
        let begins_a_name = needle.starts_with(in_a_name);
        let own = |&(at, _): &(usize, &str)| !(begins_a_name && code[..at].ends_with(in_a_name));
        let times = code.match_indices(needle).filter(own).count();
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

/// What to do about a way around the door that is found and not listed.
const THROUGH_THE_CORE: &str = "send it through the core, or list it with its reason";

/// Fails unless what was `found` is exactly what is `listed`, and says which entries are new
/// (with `remedy`, what to do about one) and which are listed but no longer found. A count
/// that changed shows as both.
fn only_listed(what: &str, remedy: &str, found: &[String], listed: &[&str]) {
    let found: Vec<&str> = found.iter().map(String::as_str).collect();
    let new: Vec<&str> = (found.iter().copied())
        .filter(|entry| !listed.contains(entry))
        .collect();
    let gone: Vec<&str> = (listed.iter().copied())
        .filter(|entry| !found.contains(entry))
        .collect();
    assert!(
        new.is_empty() && gone.is_empty(),
        "{what}\n  found but not listed: {new:?}\n    {remedy}: this list only shrinks, so a \
         count is never just raised\n  listed but not found: {gone:?}\n    take it off the \
         list, or lower its count"
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

/// A unit test module is one whatever its visibility, and a module that is tests whole is
/// known by the `#[cfg(test)]` where it is declared.
#[test]
fn the_scan_knows_a_test_module_by_any_visibility_and_in_a_file_of_its_own() {
    let text = "fn f() {\n    a::one();\n}\n#[cfg(test)]\npub(crate) mod tests {\n    fn t() {\n        \
                a::two();\n    }\n}\n#[cfg(test)]\npub mod more {\n    fn t() {\n        a::three();\n    }\n}\n";
    assert_eq!(named_in(&code(text), "a::"), ["one"]);
    assert_eq!(declares_mod("    pub(in crate::x) mod y {"), Some("y {"));
    assert_eq!(declares_mod("public_mod x"), None);
    let file = |path: &str, text: &str| (path.to_string(), text.to_string());
    let files = [
        file("lib.rs", "mod a;\n#[cfg(test)]\nmod checks;\n"),
        file(
            "core.rs",
            "#[cfg(test)]\npub(crate) mod news;\n#[cfg(test)]\nmod inline {\n}\n",
        ),
        file("owner/mod.rs", "#[cfg(test)]\nmod tests;\n"),
    ];
    assert_eq!(test_modules(&files), ["checks", "core/news", "owner/tests"]);
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
    only_listed("reads of the core's index", THROUGH_THE_CORE, &found, &left);
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
        THROUGH_THE_CORE,
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
    only_listed(
        "item edits around Core::edit_item",
        THROUGH_THE_CORE,
        &found,
        &left,
    );
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
    only_listed("links read by a pattern", THROUGH_THE_CORE, &found, &left);
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
    only_listed(
        "the built-in extensions beyond the gateway",
        THROUGH_THE_CORE,
        &found,
        &left,
    );
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
    only_listed("sample", "remedy", &["a: x ×2".to_string()], &["a: x ×1"]);
}

#[test]
#[should_panic(expected = "listed but not found: [\"b: y ×1\"]")]
fn an_entry_that_is_gone_fails_its_list() {
    only_listed(
        "sample",
        "remedy",
        &["a: x ×1".to_string()],
        &["a: x ×1", "b: y ×1"],
    );
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

/// A name is counted as a name of its own, so the print lists below count what they say.
#[test]
fn a_name_is_not_counted_inside_a_longer_one() {
    let code = "eprintln!(\"a\");\nprintln!(\"b\");\nx.index; say::held();";
    assert_eq!(times(code, &["println!(", "eprintln!("]).len(), 2);
    assert_eq!(times(code, &[".index", "held()"]).len(), 2);
}

/// One door out to the terminal too (#265, track A step A2): text a file or an extension
/// chose is shown only through `shown::escape`, and on the command line that is `say.rs`,
/// the one module that prints. Nothing else names a print macro, takes stdout or stderr, or
/// writes bytes of its own to the terminal, but for the terminal's own protocol, which
/// carries no outside text today and is counted so that it can't grow unseen:
///
/// - `main.rs`: `stdout()` for mouse capture, switched on and off around the terminal app.
/// - `term.rs`: `stdout()` to leave and enter the alternate screen, and in `write_raw`,
///   which writes the Kitty graphics the app made.
/// - `home.rs`: a picture placed or deleted with Kitty graphics (`stdout()`, `write_raw`).
///
/// No file of the host, the engine or the views crate prints outside its tests.
///
/// It is the same guard on the text as the rest of this file, with the same blind spots. It
/// doesn't see a print through a macro of another crate, `write!` to a handle taken another
/// way (`io::stdout` under another name, a `File` on `/dev/tty`), or a child process that
/// inherits the terminal. A panic's message goes to stderr as the standard library writes
/// it, and can quote outside text: that is a stated limit, since a hook that escaped it
/// would change what an ordinary panic prints. What the terminal app draws is not printed:
/// it goes into ratatui's cells, which this test doesn't read.
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
        "Print(",
        "SetTitle(",
        "write_raw(",
    ];
    let mut files = sources();
    for name in ["host", "engine", "views"] {
        files.extend(crate_sources(name));
    }
    let found = tally(per_file(&files, |code| times(code, &prints)));
    let listed = [
        "home.rs: stdout() ×1",
        "home.rs: write_raw( ×2",
        "main.rs: stdout() ×2",
        "say.rs: eprintln!( ×1",
        "say.rs: print!( ×1",
        "say.rs: println!( ×5",
        "say.rs: stdout() ×1",
        "term.rs: stdout() ×3",
        "term.rs: write_raw( ×1",
    ];
    let remedy = "print through `out!`, `told!` or `problem!` (src/say.rs says which), so the \
                  line is escaped";
    only_listed(
        "prints and handles on the terminal",
        remedy,
        &found,
        &listed,
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
    let found = tally(hits(&listings));
    let listed = [
        "drive.rs: out!( ×3",
        "main.rs: listing( ×2",
        "main.rs: listing_block( ×2",
        "main.rs: out!( ×19",
        "say.rs: listing( ×4",
        "say.rs: listing_block( ×1",
    ];
    let remedy = "a line is a listing only if it is the user's own content; what a command \
                  did or why it didn't is a message (`told!`, `problem!`)";
    only_listed("lines printed as listings", remedy, &found, &listed);
}

/// A problem (why something was refused, failed or was left out) is put together one way,
/// wherever it is said: as a `Problem` (crates/engine/src/problem.rs), which joins several
/// on one line, reads a TOML error's lines from the parser's parts, and is picked out of a
/// list by what it is about, never by its words. These show where it is still put together
/// by hand, in the app, the host, the engine and the views crate:
///
/// - a list joined into one line with `"; "`;
/// - a TOML error folded onto one line by turning its line breaks into spaces;
/// - a TOML error said by its message alone, without where in the file it is;
/// - a TOML error handled by its type, where it may be made into text some other way (`{e}`
///   or `e.to_string()` gives its lines raw).
///
/// Picking a problem out of a list by its words is not scanned for: `Problem::about` is what
/// stops it, as there is nothing else to pick by.
///
/// What is left:
///
/// - `problem.rs`, where each is done once.
/// - `main.rs`: `where_label` joins the conditions of a type's `where` for `cold-lava
///   types`, which are no problems.
///
/// The scan knows these by their spelling: a TOML error handled without naming its type, or
/// a fold written another way, passes it, and a reviewer still reads a change that says a
/// problem.
#[test]
fn problems_are_put_together_by_hand_here() {
    let ways = [
        "join(\"; \")",
        "replace('\\n', \" \")",
        ".message()",
        "toml::de::Error",
    ];
    let mut files = sources();
    for name in ["host", "engine", "views"] {
        files.extend(crate_sources(name));
    }
    let found = tally(per_file(&files, |code| times(code, &ways)));
    let listed = [
        "../engine/src/problem.rs: .message() ×1",
        "../engine/src/problem.rs: join(\"; \") ×1",
        "../engine/src/problem.rs: replace('\\n', \" \") ×1",
        "../engine/src/problem.rs: toml::de::Error ×2",
        "main.rs: join(\"; \") ×1",
    ];
    let remedy = "make it a `Problem` (crates/engine/src/problem.rs), which joins, folds and \
                  picks out problems in one place";
    only_listed("problems put together by hand", remedy, &found, &listed);
}

/// What the terminal app draws goes into ratatui's cells, and text drawn through ratatui
/// (`Span`, `Line`, `Paragraph`, `set_string`) loses its control characters there. A symbol
/// set on a cell itself does not, so only `cells.rs` sets one, and it shows the stand-in
/// for what a terminal would act on. A cell reached another way (`Buffer::content_mut`, a
/// widget of another crate that sets symbols itself) is not seen, and neither is a cell
/// put in whole (`buf[(x, y)] = cell`) or a setter named without its call
/// (`.map(Cell::set_symbol)`).
#[test]
fn the_app_sets_a_cells_symbol_only_through_cells() {
    let sets = ["set_char(", "set_symbol("];
    let found = tally(hits(&sets));
    let remedy = "set it with `cells::put` or `cells::put_char`, so an escape sequence in a \
                  label never reaches the terminal";
    only_listed(
        "symbols set on a cell",
        remedy,
        &found,
        &["cells.rs: set_char( ×1", "cells.rs: set_symbol( ×5"],
    );
}

/// The top-level `,`-separated parts of `body`, each trimmed; a comma inside brackets of any
/// kind belongs to its part.
fn parts(body: &str) -> Vec<&str> {
    let (mut out, mut depth, mut from) = (Vec::new(), 0i32, 0);
    for (at, c) in body.char_indices() {
        match c {
            '(' | '[' | '{' | '<' => depth += 1,
            ')' | ']' | '}' | '>' => depth -= 1,
            ',' if depth == 0 => {
                out.push(body[from..at].trim());
                from = at + 1;
            }
            _ => {}
        }
    }
    out.push(body[from..].trim());
    out.retain(|p| !p.is_empty());
    out
}

/// What is between the bracket that opens at the start of `text` and the one that closes it.
fn inside(text: &str) -> Option<&str> {
    let (open, close) = match text.chars().next()? {
        '(' => ('(', ')'),
        '[' => ('[', ']'),
        '{' => ('{', '}'),
        _ => return None,
    };
    let mut depth = 0;
    for (at, c) in text.char_indices() {
        if c == open {
            depth += 1;
        } else if c == close {
            depth -= 1;
            if depth == 0 {
                return Some(&text[1..at]);
            }
        }
    }
    None
}

/// A part without the attributes before it (`#[…]`), whether one of them keeps serde from
/// reading it (`skip`, `skip_deserializing`), and whether one reads it with a function of
/// its own (`with`, `deserialize_with`), which the scan can't follow.
fn unattributed(mut part: &str) -> (&str, bool, bool) {
    let (mut skipped, mut with) = (false, false);
    while let Some(rest) = part.strip_prefix('#') {
        let Some(attribute) = inside(rest) else {
            break;
        };
        let words = attribute.replace(['(', ')', ',', '='], " ");
        let mut words = words.split(' ');
        skipped |= words
            .clone()
            .any(|w| w == "skip" || w == "skip_deserializing");
        with |= words.any(|w| w == "with" || w == "deserialize_with");
        part = rest[attribute.len() + 2..].trim_start();
    }
    (part, skipped, with)
}

/// A field or a variant without its visibility (`pub`, `pub(crate)`).
fn unpublished(part: &str) -> &str {
    let Some(rest) = part.strip_prefix("pub") else {
        return part;
    };
    match rest
        .trim_start()
        .strip_prefix('(')
        .map(|r| r.split_once(')'))
    {
        Some(Some((_, after))) => after.trim_start(),
        _ => rest.trim_start(),
    }
}

/// The fields a struct's or a struct variant's body declares and serde reads: each one's
/// name, its type, and whether serde reads it with a function of its own (`with`).
fn field_types(body: &str) -> Vec<(String, String, bool)> {
    let mut out = Vec::new();
    for part in parts(body) {
        let (part, skipped, with) = unattributed(part);
        if let (false, Some((name, ty))) = (skipped, unpublished(part).split_once(':')) {
            out.push((name.trim().to_string(), ty.trim().to_string(), with));
        }
    }
    out
}

/// A struct or an enum of `code`: its name, the attributes before it and every type its
/// fields and variants hold (`Struct.field`, `Enum::Variant.field`, `Enum::Variant.0`), each
/// with whether serde reads it with a function of its own.
struct Defined {
    name: String,
    attributes: String,
    fields: Vec<(String, String, bool)>,
}

/// Which lines of `lines` are in an attribute (`#[…]`), one that runs over several lines
/// included.
fn in_attributes(lines: &[&str]) -> Vec<bool> {
    let mut depth = 0i32;
    let mut out = Vec::new();
    for line in lines {
        let starts = depth == 0 && line.trim_start().starts_with("#[");
        let inside = depth > 0 || starts;
        if inside {
            depth += line.matches('[').count() as i32 - line.matches(']').count() as i32;
        }
        out.push(inside);
    }
    out
}

/// Every struct and enum `code` defines.
fn defined(code: &str) -> Vec<Defined> {
    let mut out = Vec::new();
    let lines: Vec<&str> = code.lines().collect();
    let attribute = in_attributes(&lines);
    for (i, line) in lines.iter().enumerate() {
        let item = unpublished(line.trim_start());
        let (kind, rest) = match (item.strip_prefix("struct "), item.strip_prefix("enum ")) {
            (Some(rest), _) if !attribute[i] => ("struct", rest),
            (_, Some(rest)) if !attribute[i] => ("enum", rest),
            _ => continue,
        };
        let name: String = (rest.chars())
            .take_while(|c| c.is_alphanumeric() || *c == '_')
            .collect();
        let first = (0..i)
            .rev()
            .take_while(|j| attribute[*j])
            .last()
            .unwrap_or(i);
        let attributes = lines[first..i].join(" ");
        let after: String = lines[i..].join("\n");
        let after = &after[after.find(&name).unwrap_or(0) + name.len()..];
        let open = after.find(['{', '(', ';']).unwrap_or(after.len());
        let body = &after[open..];
        let tuple = |body: &str| -> Vec<(String, String, bool)> {
            let types = parts(inside(body).unwrap_or("")).into_iter().enumerate();
            types
                .map(|(n, ty)| (n.to_string(), unpublished(ty).to_string(), false))
                .collect()
        };
        let mut fields = Vec::new();
        match (kind, body.chars().next()) {
            ("struct", Some('{')) => {
                for (field, ty, with) in field_types(inside(body).unwrap_or("")) {
                    fields.push((format!("{name}.{field}"), ty, with));
                }
            }
            ("struct", Some('(')) => {
                for (n, ty, with) in tuple(body) {
                    fields.push((format!("{name}.{n}"), ty, with));
                }
            }
            ("enum", Some('{')) => {
                for variant in parts(inside(body).unwrap_or("")) {
                    let (variant, _, variant_with) = unattributed(variant);
                    let at = variant.find(['{', '(']).unwrap_or(variant.len());
                    let (vname, vbody) = (variant[..at].trim(), &variant[at..]);
                    let found = match vbody.chars().next() {
                        Some('{') => field_types(inside(vbody).unwrap_or("")),
                        Some('(') => tuple(vbody),
                        _ => Vec::new(),
                    };
                    for (field, ty, with) in found {
                        let with = with || variant_with;
                        fields.push((format!("{name}::{vname}.{field}"), ty, with));
                    }
                }
            }
            _ => {}
        }
        out.push(Defined {
            name,
            attributes,
            fields,
        });
    }
    out
}

/// The names in a type, as written: `Option<Text<Place>>` names `Option`, `Text`, `Place`.
fn names_in(ty: &str) -> Vec<&str> {
    let word = |c: char| !(c.is_alphanumeric() || c == '_');
    ty.split(word).filter(|w| !w.is_empty()).collect()
}

/// The module the file `code` takes `name` from with a `use crate::…` of its own, as a path
/// under `src/` without `.rs` (`shapes`).
fn module_of(code: &str, name: &str) -> Option<String> {
    for (at, _) in code.match_indices("use crate::") {
        let stmt = &code[at + "use crate::".len()..];
        let stmt = &stmt[..stmt.find(';')?];
        if !names_in(stmt).contains(&name) {
            continue;
        }
        let path = match stmt.find("::{") {
            Some(i) => &stmt[..i],
            None => stmt.rsplit_once("::")?.0,
        };
        return Some(path.replace("::", "/"));
    }
    None
}

/// Whether the file `code` takes `name` with a `use` from outside the engine (`std::`,
/// another crate, `super::`, `self::`): a type the scan can't follow, whatever the engine
/// names the same way.
fn imported_from_outside(code: &str, name: &str) -> bool {
    for (at, _) in code.match_indices("use ") {
        let line = &code[code[..at].rfind('\n').map_or(0, |n| n + 1)..at];
        if !(line.trim().is_empty() || line.trim() == "pub") {
            continue;
        }
        let stmt = &code[at + "use ".len()..];
        let stmt = &stmt[..stmt.find(';').unwrap_or(stmt.len())];
        if !stmt.starts_with("crate::") && names_in(stmt).contains(&name) {
            return true;
        }
    }
    false
}

/// What an answer's types may hold without a bound of their own: none of these. A field
/// holds a type of `crate::bounded` instead, which names its bound.
const UNBOUNDED: [&str; 14] = [
    "String", "Vec", "Map", "Json", "Value", "HashMap", "BTreeMap", "IndexMap", "IndexSet",
    "HashSet", "BTreeSet", "VecDeque", "str", "Cow",
];

/// What a field may name that the scan doesn't follow: a type that only wraps the one it
/// holds, and the scalars, whose size is fixed.
const LEAVES: [&str; 19] = [
    "Option",
    "Box",
    "PhantomData",
    "bool",
    "char",
    "u8",
    "u16",
    "u32",
    "u64",
    "u128",
    "usize",
    "i8",
    "i16",
    "i32",
    "i64",
    "i128",
    "isize",
    "f32",
    "f64",
];

<<<<<<< HEAD
/// The module of the bounded types, and the types in it that hold a value within a bound
/// another type names: those are bounds, and are not followed. The bounds themselves are
/// unit types, followed like any other.
const BOUNDED: &str = "../engine/src/bounded.rs";
const WRAPPERS: [&str; 4] = ["Text", "List", "Fields", "Pairs"];
=======
/// The module of the bounded types: what is in it is a bound, and is not followed.
const BOUNDED: &str = "../engine/src/bounded.rs";
>>>>>>> origin/main

/// The types read from an answer, from `roots` (`file: Type`) through every type a field of
/// one names and every type one is read through (`#[serde(from = "…")]`), each as
/// `file: Type`; and each field among them that breaks the rule, as `file: Type.field: type:
/// why`. The rule: every name in a field's type is a bounded type (`BOUNDED`), a type the
/// scan follows, or a leaf (`LEAVES`); a bare collection (`UNBOUNDED`), a slice, a name the
/// scan can't find in the engine (another crate's type, a path, one behind a re-export) and
/// a field serde reads with a function of its own are each named. A name is the type of its
/// own file, else the one the file takes with a `use crate::`, else the only type of that
/// name in the engine.
fn answer_types(files: &[(String, String)], roots: &[&str]) -> (Vec<String>, Vec<String>) {
    let all: Vec<(&str, &str, Vec<Defined>)> = (files.iter())
        .map(|(file, code)| (file.as_str(), code.as_str(), defined(code)))
        .collect();
    let find = |file: &str, name: &str| -> Option<(String, usize)> {
        let at = |f: &str| {
            let (_, _, d) = all.iter().find(|(x, _, _)| *x == f)?;
            d.iter()
                .position(|d| d.name == name)
                .map(|i| (f.to_string(), i))
        };
        if let Some(own) = at(file) {
            return Some(own);
        }
        let code = all.iter().find(|(x, _, _)| *x == file)?.1;
        if let Some(module) = module_of(code, name) {
            let candidates = [
                format!("../engine/src/{module}.rs"),
                format!("../engine/src/{module}/mod.rs"),
            ];
            if let Some(found) = candidates.iter().find_map(|f| at(f.as_str())) {
                return Some(found);
            }
        }
        if imported_from_outside(code, name) {
            return None;
        }
        let mut anywhere = (all.iter()).filter_map(|(f, _, _)| at(f));
        let found = anywhere.next();
        assert!(
            anywhere.next().is_none(),
            "{file} names {name}, which several modules define: take it with a `use crate::`"
        );
        found
    };
    let short = |file: &str| file.trim_start_matches("../engine/src/").to_string();
    let (mut reached, mut wrong) = (Vec::new(), Vec::new());
    let mut queue: Vec<(String, usize)> = (roots.iter())
        .map(|r| {
            let (file, name) = r.split_once(": ").unwrap();
            let found = find(&format!("../engine/src/{file}"), name);
            found.unwrap_or_else(|| panic!("the root {r} is no struct or enum"))
        })
        .collect();
    while let Some((file, i)) = queue.pop() {
        let d = &all.iter().find(|(f, _, _)| *f == file).unwrap().2[i];
        let entry = format!("{}: {}", short(&file), d.name);
<<<<<<< HEAD
        let wrapper = file == BOUNDED && WRAPPERS.contains(&d.name.as_str());
        if wrapper || reached.contains(&entry) {
=======
        if file == BOUNDED || reached.contains(&entry) {
>>>>>>> origin/main
            continue;
        }
        reached.push(entry);
        let through = d.attributes.split("from = \"").skip(1);
        for via in through.filter_map(|rest| rest.split_once('"')) {
            let found = find(&file, via.0);
            let found = found.unwrap_or_else(|| {
                panic!(
                    "{} is read through {}, which the scan can't find",
                    d.name, via.0
                )
            });
            queue.push(found);
        }
        for (field, ty, with) in &d.fields {
            let mut say = |why: &str| wrong.push(format!("{}: {field}: {ty}: {why}", short(&file)));
            if *with {
                say("serde reads it with a function of its own");
            }
            if ty.contains('[') {
                say("a slice or an array");
            }
            for name in names_in(ty) {
                if UNBOUNDED.contains(&name) {
                    say(&format!("{name} has no bound"));
                } else if LEAVES.contains(&name) {
                } else if let Some(found) = find(&file, name) {
                    queue.push(found);
                } else {
                    say(&format!("a field type the scan can't follow: {name}"));
                }
            }
        }
    }
    reached.sort();
    wrong.sort();
    (reached, wrong)
}

<<<<<<< HEAD
/// The engine's modules of answers: every type read with serde in them (`Deserialize`) is
/// an answer's, or one of these, which another program never sends.
const ANSWER_FILES: [&str; 4] = [
    "act.rs",
    "owner/answer.rs",
    "owner/edits.rs",
    "owner/given.rs",
];
const NOT_ANSWERS: [&str; 7] = [
    // What is asked of an extension, and what the frontend and the engine make.
    "act.rs: Ask",
    "act.rs: Gesture",
    "act.rs: InputData",
    "act.rs: MemberData",
    "act.rs: Rebind",
    // What is asked of an owner.
    "owner/edits.rs: FileEdit",
    "owner/given.rs: Change",
];

/// Every type of `ANSWER_FILES` that serde reads (it derives `Deserialize`), as `file: Type`.
fn read_with_serde(files: &[(String, String)]) -> Vec<String> {
    let mut found = Vec::new();
    for (file, code) in files {
        let Some(short) = file.strip_prefix("../engine/src/") else {
            continue;
        };
        if !ANSWER_FILES.contains(&short) {
            continue;
        }
        let read = defined(code)
            .into_iter()
            .filter(|d| d.attributes.contains("Deserialize"));
        found.extend(read.map(|d| format!("{short}: {}", d.name)));
    }
    found.sort();
    found
}

/// What another program answers is held in types whose every field names its bound
/// (`crates/engine/src/bounded.rs`, #265's step A4): an owner's `file/read` (`Given`) and
/// `file/edit` (`NewText`), and an extension's `shape/region` and `shape/gesture` (`Reply`).
/// This follows every type such an answer is read into, from those three and from every type
/// serde reads in the modules of answers (`ANSWER_FILES`, but `NOT_ANSWERS`), and fails on a
=======
/// What another program answers is held in types whose every field names its bound
/// (`crates/engine/src/bounded.rs`, #265's step A4): an owner's `file/read` (`Given`) and
/// `file/edit` (`NewText`), and an extension's `shape/region` and `shape/gesture` (`Reply`).
/// This follows every type such an answer is read into, from those three, and fails on a
>>>>>>> origin/main
/// field that holds a `String`, a `Vec`, a map or a JSON value of its own. The list of types
/// says what is followed: a new one fails until it is listed, and so does one that is gone.
///
/// It reads the text, so it can't see an answer read into a type outside these three (the
/// host's reads with `serde_json::from_…` are counted below, so a new one fails there), a
/// field whose type comes through a type alias or a macro, or what a hand-written
/// `Deserialize` reads on its way (`WireLink` reads a map, then one of two types this
/// follows). A view's content (`file/preview`) is the views crate's, outside this guard
/// (#296), and so is the owners' cache read from disk (`owner/cache.rs`), which holds
/// `Given`s the engine kept.
#[test]
fn every_field_of_an_answer_names_its_bound() {
    let files = crate_sources("engine");
<<<<<<< HEAD
    let serde_reads = read_with_serde(&files);
    let not_answers: Vec<String> = (serde_reads.iter())
        .filter(|t| NOT_ANSWERS.contains(&t.as_str()))
        .cloned()
        .collect();
    let remedy = "an answer's type is followed: take it off unless no other program sends it";
    only_listed(
        "types read with serde that no answer holds",
        remedy,
        &not_answers,
        &NOT_ANSWERS,
    );
    let mut roots = vec![
=======
    let roots = [
>>>>>>> origin/main
        "owner/given.rs: Given",
        "owner/edits.rs: NewText",
        "act.rs: Reply",
    ];
<<<<<<< HEAD
    roots.extend(
        serde_reads
            .iter()
            .map(String::as_str)
            .filter(|t| !NOT_ANSWERS.contains(t)),
    );
=======
>>>>>>> origin/main
    let (reached, wrong) = answer_types(&files, &roots);
    assert!(
        wrong.is_empty(),
        "fields of an answer that name no bound: {wrong:#?}\n    hold each in a type of \
         crate::bounded, which names its bound (`Text<Message>` when where it comes from \
         bounds it, `Text<Owed>` for a bound decided and not applied yet), a type the scan \
         follows, or a leaf"
    );
    let listed = [
<<<<<<< HEAD
        "act.rs: GestureWire",
        "act.rs: RegionEntry",
        "act.rs: RegionWire",
        "act.rs: Reply",
        "alphabet.rs: Mark",
        "alphabet.rs: Role",
        "bounded.rs: AttributeName",
        "bounded.rs: Attributes",
        "bounded.rs: Betweens",
        "bounded.rs: CellName",
        "bounded.rs: FieldName",
        "bounded.rs: LabelName",
        "bounded.rs: LinkLabel",
        "bounded.rs: Message",
        "bounded.rs: Owed",
        "bounded.rs: Place",
        "bounded.rs: Target",
        "owner/answer.rs: Kept",
        "owner/answer.rs: LeftOut",
        "owner/answer.rs: MarkdownWire",
        "owner/answer.rs: TypedWire",
=======
        "act.rs: RegionEntry",
        "act.rs: Reply",
        "alphabet.rs: Mark",
        "alphabet.rs: Role",
>>>>>>> origin/main
        "owner/edits.rs: NewText",
        "owner/given.rs: Between",
        "owner/given.rs: Cell",
        "owner/given.rs: FieldRun",
        "owner/given.rs: Given",
        "owner/given.rs: GivenItem",
        "owner/given.rs: GivenLink",
        "owner/given.rs: Label",
        "owner/given.rs: LinkRun",
        "owner/given.rs: Marked",
        "owner/given.rs: Piece",
        "owner/given.rs: Run",
        "owner/given.rs: Wire",
        "owner/given.rs: WireLink",
        "shapes.rs: CreateStep",
        "shapes.rs: EditStep",
        "shapes.rs: RebindStep",
        "shapes.rs: SetField",
        "shapes.rs: Step",
    ];
    let remedy = "check its fields are bounded, and list it";
    only_listed(
        "the types an answer is read into",
        remedy,
        &reached,
        &listed,
    );

    // Where the host reads what an extension sent into a type: in process.rs its answers
    // to file/read, file/edit, file/preview (a view, #296) and a shape's ask; in rpc.rs the
    // message itself, a JSON-RPC error and its requests (#296).
    let reads = ["serde_json::from_"];
    let found = tally(per_file(&host_sources(), |code| times(code, &reads)));
    let remedy = "read an answer into one of the types above, or into a new one this follows";
    only_listed(
        "where the host reads what an extension sent",
        remedy,
        &found,
        &[
            "../host/src/process.rs: serde_json::from_ ×4",
            "../host/src/rpc.rs: serde_json::from_ ×5",
        ],
    );
}

#[test]
fn the_answer_scan_finds_a_bare_field_and_follows_every_kind_of_field() {
    let code = "use crate::other::{Far};\n\
                use std::ops::Range;\n\
                use crate::bounded::{List, Message, Place, Text};\n\
                #[derive(Deserialize)]\n\
                #[serde(\n    from = \"Wire\",\n)]\n\
                pub struct Root {\n    pub a: Text<Place>,\n    #[serde(skip)]\n    pub \
                cache: String,\n    pub(crate) b: List<Inner, Message>,\n    \
                #[serde(deserialize_with = \"x\")]\n    c: Option<u8>,\n}\n\
                struct Wire {\n    c: Option<Far>,\n    p: PathBuf,\n    s: Box<[u8]>,\n    \
                m: std::path::Path,\n    r: Range<u8>,\n}\n\
                enum Inner {\n    Plain(Text<Message>),\n    #[serde(rename = \"x\")]\n    \
                Named { d: Vec<u8>, e: bool },\n    \
                #[serde(deserialize_with = \"y\")]\n    Read(u8),\n    Unit,\n}\n";
    let far = "pub struct Far(pub Map<String, Json>);\npub struct Range;\n";
    let bounded = "pub struct Text<B>(String);\npub struct List<T, B>(Vec<T>);\n\
                   pub struct Place;\npub struct Message;\n";
    let files = vec![
        ("../engine/src/root.rs".to_string(), code.to_string()),
        ("../engine/src/other.rs".to_string(), far.to_string()),
        ("../engine/src/bounded.rs".to_string(), bounded.to_string()),
    ];
    let (reached, wrong) = answer_types(&files, &["root.rs: Root"]);
    assert_eq!(
        reached,
        [
<<<<<<< HEAD
            "bounded.rs: Message",
            "bounded.rs: Place",
=======
>>>>>>> origin/main
            "other.rs: Far",
            "root.rs: Inner",
            "root.rs: Root",
            "root.rs: Wire"
        ]
    );
    assert_eq!(
        wrong,
        [
            "other.rs: Far.0: Map<String, Json>: Json has no bound",
            "other.rs: Far.0: Map<String, Json>: Map has no bound",
            "other.rs: Far.0: Map<String, Json>: String has no bound",
            "root.rs: Inner::Named.d: Vec<u8>: Vec has no bound",
            "root.rs: Inner::Read.0: u8: serde reads it with a function of its own",
            "root.rs: Root.c: Option<u8>: serde reads it with a function of its own",
            "root.rs: Wire.m: std::path::Path: a field type the scan can't follow: Path",
            "root.rs: Wire.m: std::path::Path: a field type the scan can't follow: path",
            "root.rs: Wire.m: std::path::Path: a field type the scan can't follow: std",
            "root.rs: Wire.p: PathBuf: a field type the scan can't follow: PathBuf",
            "root.rs: Wire.r: Range<u8>: a field type the scan can't follow: Range",
            "root.rs: Wire.s: Box<[u8]>: a slice or an array",
        ]
    );
}
