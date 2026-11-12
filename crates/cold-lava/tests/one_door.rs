//! One door into the engine (#193): what the terminal app does to a workspace's files goes
//! through the core, with the gateway's messages. This reads the app's own source, every file
//! under `src/`, so a direct call can't come back. Each list below is everything of its kind
//! the source names: a new one fails here, and a later group of #193 takes its own off.
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
/// bare `delete(…)` is the call. Types, which start with a capital, are not calls.
fn named_in(code: &str, module: &str) -> Vec<String> {
    let mut found = Vec::new();
    for (at, _) in code.match_indices(module) {
        let rest = &code[at + module.len()..];
        let group = rest.strip_prefix('{').and_then(|g| g.split_once('}'));
        let names: Vec<&str> = match group {
            Some((inside, _)) => inside.split(',').collect(),
            None => vec![rest],
        };
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
#[test]
fn the_operations_still_called_directly_are_these() {
    let calls = [
        "merge::apply(",
        "merge::duplicate(",
        "rename::apply(",
        "recipe::apply(",
        "recipe::undo(",
        "bookmarks::edit(",
        "daily::ensure(",
    ];
    let left = [
        "home.rs: bookmarks::edit(",
        "home.rs: bookmarks::edit(",
        "home.rs: daily::ensure(",
        "home.rs: merge::apply(",
        "home.rs: merge::duplicate(",
        "home.rs: recipe::apply(",
        "home.rs: recipe::undo(",
        "home.rs: rename::apply(",
        "main.rs: daily::ensure(",
        "main.rs: recipe::apply(",
        "main.rs: rename::apply(",
    ];
    assert_eq!(hits(&calls), left);
}

/// No file of a workspace is written, removed, renamed or made with `std::fs`. What the app
/// does with `std::fs` outside its tests is on files of its own:
///
/// - `board.rs`: the temporary file a card's text is in while `$EDITOR` has it.
/// - `drive.rs`: the scripted run's own home folder, and its `write` step, which changes a
///   workspace file "as another program would", on purpose: written beside it and renamed
///   over it, as a program that saves safely does.
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

/// The ways around the lists above that the scan refuses outright: one of the engine's
/// file-changing modules under another name or imported whole, and a `std::fs` function that
/// changes files brought in with `use`, after which its bare name is the call.
fn sneaked(code: &str) -> Vec<String> {
    let mut found = Vec::new();
    for module in ["trash", "write", "files"] {
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
    for statement in code.split(';') {
        let is_use = |l: &&str| {
            let l = l.trim_start();
            l.starts_with("use ") || l.starts_with("pub use ") || l.starts_with("pub(crate) use ")
        };
        let lines: Vec<&str> = statement.lines().skip_while(|l| !is_use(l)).collect();
        for name in named_in(&lines.join("\n"), "fs::") {
            if changes.contains(&name.as_str()) {
                found.push(format!("use std::fs::{name}"));
            }
        }
    }
    found
}

/// Nothing goes around the lists: no alias or glob of the engine's `trash`, `write` and
/// `files` modules, no `std::fs` function that changes files imported by name, none of the
/// gateway's own file functions (`files::delete` and the rest, which are the core's to call),
/// and no file made or copied another way.
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
    let other = ["File::create", "OpenOptions", "fs::copy(", "fs::hard_link("];
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
