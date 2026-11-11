//! One door into the engine (#193): what the menu does to files goes through the core, with
//! the gateway's messages. This reads the menu's source, so a direct call can't come back.

const HOME: &str = include_str!("../src/home.rs");

/// The functions of `module` that `source` names outside comments, sorted, each as often as
/// it is named.
fn named_in(source: &str, module: &str) -> Vec<String> {
    let code = source.lines().filter(|l| !l.trim_start().starts_with("//"));
    let mut found = Vec::new();
    for line in code {
        for (at, _) in line.match_indices(module) {
            let after = line[at + module.len()..].chars();
            let name: String = after
                .take_while(|c| c.is_alphanumeric() || *c == '_')
                .collect();
            if !name.is_empty() {
                found.push(name);
            }
        }
    }
    found.sort();
    found
}

/// Deleting, restoring and making a folder are `Core::delete_file`, `restore_file` and
/// `create_folder`, asked as `menu`: never `trash::delete`, `trash::restore` or
/// `write::create_dir`. What the menu still calls itself is listed here, so the list only
/// shrinks: `system_bin` for the prompt's "goes to" line and for `merge::apply`, `free_path`
/// for the name an undo offers when the old one is taken, and the writes of new notes and of
/// a template's text, which are later groups of #193.
#[test]
fn the_menu_deletes_restores_and_makes_folders_through_the_core() {
    assert_eq!(
        named_in(HOME, "trash::"),
        ["free_path", "system_bin", "system_bin"]
    );
    let writes = [
        "create",
        "create_if_missing",
        "create_if_missing",
        "create_if_missing",
        "create_if_missing",
        "replace",
    ];
    assert_eq!(named_in(HOME, "write::"), writes);
    for asked in [
        "core.delete_file(MENU,",
        "core.restore_file(MENU,",
        "core.create_folder(MENU,",
    ] {
        assert!(HOME.contains(asked), "the menu asks the core: {asked}");
    }
}

/// A function brought in with `use` and called by its bare name is a direct call too.
#[test]
fn the_scan_sees_an_import_and_a_bare_call() {
    let sample = "use crate::trash::{delete, TrashOption};\nfn f() {\n    delete(a, b);\n}\n";
    assert_eq!(named_in(sample, "trash::"), ["delete"]);
}
