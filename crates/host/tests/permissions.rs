//! The permission check, where every frontend shares it (#179): an extension does only what
//! its manifest's permissions allow, through the one gateway, and its edits carry its name.
//! These use the host as a second frontend would, through its public API, with no terminal.

use cold_lava_engine::core::{Core, Event};
use cold_lava_engine::files::Restore;
use cold_lava_engine::gateway::{self, Reason};
use cold_lava_engine::tasks;
use cold_lava_engine::trash::Bin;
use cold_lava_host::process::Process;
use cold_lava_host::{
    CommandRequest, EditorRequest, Effect, ExtId, Extension, FileType, Host, Manifest, PaneNews,
    Permission, Registry, installed,
};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

const LATE: &str = "- [ ] Check the seismograph 📅 2000-01-01\n";
const NOTE: &str = "Inbox/Seismograph.md";

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn has_python() -> bool {
    let python = std::process::Command::new("python3")
        .arg("--version")
        .output();
    let found = python.is_ok_and(|o| o.status.success());
    if !found {
        eprintln!("skipped: python3 isn't on PATH, and the example extension needs it");
    }
    found
}

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap().flatten() {
        let dest = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &dest);
        } else {
            std::fs::copy(entry.path(), dest).unwrap();
        }
    }
}

/// A copy of the example workspace with one overdue task, in `NOTE`: the folder made for the
/// test, and the workspace in it.
fn workspace(name: &str) -> (PathBuf, PathBuf) {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("permissions-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let root = dir.join("workspace");
    copy_dir(&repo().join("example-workspace"), &root);
    std::fs::write(root.join(NOTE), LATE).unwrap();
    (dir, root)
}

/// The `open-tasks` example in a process of its own, installed in `dir` with `permissions` in
/// place of its manifest's.
fn open_tasks(dir: &Path, permissions: &str) -> Registry {
    let folder = dir.join("extensions/open-tasks");
    copy_dir(&repo().join("examples/extensions/open-tasks"), &folder);
    let manifest = folder.join("extension.toml");
    let shipped = std::fs::read_to_string(&manifest).unwrap();
    let asks = "permissions = [\"items.read\", \"items.edit\"]";
    assert!(shipped.contains(asks), "the example asks for items.edit");
    let given = shipped.replace(asks, &format!("permissions = {permissions}"));
    std::fs::write(&manifest, given).unwrap();
    let (found, mistakes) = installed::load(&dir.join("extensions"));
    assert_eq!(mistakes, Vec::<String>::new());
    let mut installed = found.into_iter().next().unwrap();
    installed.launch.log = Some(dir.join("open-tasks.log"));
    Registry::new(vec![Box::new(Process::new(installed).unwrap())])
}

/// Frames, as a frontend runs them, until the extension asks for something that passes
/// `done`: that effect, and the core's events meanwhile.
fn until(
    r: &mut Registry,
    core: &mut Core,
    done: impl Fn(&Effect) -> bool,
) -> (Effect, Vec<Event>) {
    let (mut effects, mut events) = (Vec::new(), Vec::new());
    let deadline = Instant::now() + Duration::from_secs(20);
    while Instant::now() < deadline {
        events.extend(core.poll());
        for (_, news) in core.take_item_news() {
            effects.extend(r.call(ExtId(0), Some(core), |e, h| e.items(news, h)));
        }
        effects.extend(r.call(ExtId(0), Some(core), |e, h| e.poll(h)));
        if let Some(found) = effects.iter().find(|e| done(e)) {
            return (found.clone(), events);
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!("timed out; heard {effects:?}");
}

/// What `open-tasks.reschedule` did as an extension with `permissions`: what it said, the
/// overdue task's note afterwards, and who the core says edited an item.
struct Rescheduled {
    said: String,
    note: String,
    edited_by: Vec<String>,
}

fn reschedule(name: &str, permissions: &str) -> Rescheduled {
    let (dir, root) = workspace(name);
    let mut core = Core::open(&root).unwrap();
    let mut r = open_tasks(&dir, permissions);
    r.call(ExtId(0), Some(&mut core), |e, h| {
        e.workspace(Some(&root), h)
    });
    let counted =
        |e: &Effect| matches!(e, Effect::Status { text: Some(t) } if t.contains("open task"));
    until(&mut r, &mut core, counted);
    let run = CommandRequest::Run {
        id: "open-tasks.reschedule".into(),
        args: serde_json::Value::Null,
    };
    r.call(ExtId(0), Some(&mut core), |e, h| e.command(run, h));
    let (said, mut events) = until(&mut r, &mut core, |e| matches!(e, Effect::Notify { .. }));
    events.extend(core.poll());
    let Effect::Notify { message: said } = said else {
        unreachable!()
    };
    let edited_by = (events.into_iter())
        .filter_map(|e| match e {
            Event::ItemEdited { by, .. } => Some(by),
            _ => None,
        })
        .collect();
    let note = std::fs::read_to_string(root.join(NOTE)).unwrap();
    r.shutdown();
    assert!(!r.running(ExtId(0)));
    let log = std::fs::read_to_string(dir.join("open-tasks.log")).unwrap();
    assert_eq!(log, "", "nothing on stderr");
    let _ = std::fs::remove_dir_all(&dir);
    Rescheduled {
        said,
        note,
        edited_by,
    }
}

/// Needs python3, which CI's runners have.
#[test]
fn the_open_tasks_example_may_not_edit_an_item_without_items_edit() {
    if !has_python() {
        return;
    }
    let did = reschedule("read-only", "[\"items.read\"]");
    assert!(
        did.said
            .starts_with("moved 0 overdue tasks to today; 1 refused (Check the seismograph: "),
        "{}",
        did.said
    );
    assert!(
        (did.said).contains("its manifest's permissions don't include items.edit"),
        "{}",
        did.said
    );
    assert_eq!(did.note, LATE, "nothing was written");
    assert_eq!(did.edited_by, Vec::<String>::new());
}

/// Needs python3, which CI's runners have.
#[test]
fn the_open_tasks_example_edits_an_item_with_items_edit_and_the_edit_names_it() {
    if !has_python() {
        return;
    }
    let did = reschedule("may-edit", "[\"items.read\", \"items.edit\"]");
    assert_eq!(did.said, "moved 1 overdue task to today");
    let today = tasks::today().format("%Y-%m-%d");
    assert_eq!(
        did.note,
        format!("- [ ] Check the seismograph 📅 {today}\n")
    );
    assert_eq!(did.edited_by, ["open-tasks"]);
}

/// An extension that only has a manifest: the test asks through its `Host`.
struct Quiet {
    name: &'static str,
    permissions: Vec<Permission>,
    opens: &'static [&'static str],
}

impl Extension for Quiet {
    fn initialize(&mut self) -> Manifest {
        let file_types = (self.opens.iter())
            .map(|x| FileType {
                extension: x.to_string(),
                title: format!("{x} file"),
                new: None,
            })
            .collect();
        Manifest {
            name: self.name.into(),
            title: self.name.into(),
            version: String::new(),
            description: String::new(),
            roles: Vec::new(),
            hints: Vec::new(),
            commands: Vec::new(),
            types: Vec::new(),
            shapes: Vec::new(),
            permissions: self.permissions.clone(),
            file_types,
            shows: Vec::new(),
        }
    }
    fn pane(&mut self, _: PaneNews, _: &mut Host<'_>) {}
    fn editor(&mut self, _: EditorRequest, _: &mut Host<'_>) {}
}

fn quiet(
    name: &'static str,
    permissions: &[Permission],
    opens: &'static [&'static str],
) -> Box<dyn Extension> {
    let permissions = permissions.to_vec();
    Box::new(Quiet {
        name,
        permissions,
        opens,
    })
}

/// A core on `root` whose system trash is `dir/trash`, under the test's own folder, never the
/// user's.
fn core_with_trash(dir: &Path, root: &Path) -> Core {
    let mut core = Core::open(root).unwrap();
    core.set_system_trash(Some(Bin::Freedesktop(dir.join("trash"))));
    core
}

/// What the gateway answers `ext` when it asks `f`.
fn ask<T>(
    r: &mut Registry,
    core: &mut Core,
    ext: ExtId,
    f: impl FnOnce(&mut Host<'_>) -> Result<T, gateway::Error>,
) -> Result<T, Reason> {
    let mut answer = None;
    r.call(ext, Some(core), |_, host| answer = Some(f(host)));
    answer.unwrap().map_err(|e| e.reason)
}

#[test]
fn a_write_needs_files_write_and_an_owner_writes_only_its_own_kind() {
    use Permission::{FilesRead, FilesWrite, ItemsEdit, ItemsRead};
    let (dir, root) = workspace("files");
    let mut core = Core::open(&root).unwrap();
    let mut r = Registry::new(vec![
        quiet("sheets", &[], &["csv"]),
        quiet("items", &[ItemsRead, ItemsEdit], &[]),
        quiet("look", &[FilesRead], &[]),
        quiet("write", &[FilesRead, FilesWrite], &[]),
    ]);
    let (sheets, items, look, write) = (ExtId(0), ExtId(1), ExtId(2), ExtId(3));
    let csv = "Volcanoes/Eruptions.csv";
    let on_disk = |path: &str| std::fs::read_to_string(root.join(path)).unwrap();
    let forbidden = Err(Reason::Forbidden);

    let note = ask(&mut r, &mut core, look, |h| h.read_file(NOTE)).unwrap();
    let mut written = |ext: ExtId, path: &str, text: &str, rev: &str| {
        ask(&mut r, &mut core, ext, |h| h.write_file(path, text, rev)).map(drop)
    };
    assert_eq!(
        written(items, NOTE, "x\n", &note.rev),
        forbidden,
        "items.edit changes items, not files"
    );
    assert_eq!(
        written(look, NOTE, "x\n", &note.rev),
        forbidden,
        "files.read doesn't write"
    );
    assert_eq!(
        written(sheets, NOTE, "x\n", &note.rev),
        forbidden,
        "a note isn't its kind"
    );
    assert_eq!(on_disk(NOTE), LATE, "nothing was written");

    assert_eq!(written(write, NOTE, "- [ ] Check it\n", &note.rev), Ok(()));
    assert_eq!(on_disk(NOTE), "- [ ] Check it\n");
    assert_eq!(
        written(write, NOTE, "late\n", &note.rev),
        Err(Reason::Changed),
        "it changed since that version was read"
    );
    assert_eq!(
        written(write, "../outside.md", "x\n", ""),
        Err(Reason::Refused),
        "outside the workspace"
    );
    assert!(!dir.join("outside.md").exists());

    let before = on_disk(csv);
    assert_eq!(
        written(look, csv, "x\n", &note.rev),
        forbidden,
        "another's kind"
    );
    let table = ask(&mut r, &mut core, sheets, |h| h.read_file(csv)).unwrap();
    assert_eq!(table.text, before, "its own kind, with no permission");
    let more = format!("{before}x\n");
    let saved = ask(&mut r, &mut core, sheets, |h| {
        h.write_file(csv, &more, &table.rev)
    });
    assert!(saved.is_ok(), "{saved:?}");
    assert_eq!(on_disk(csv), more);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_delete_needs_files_delete_which_neither_files_write_nor_owning_the_kind_gives() {
    use Permission::{FilesDelete, FilesRead, FilesWrite};
    let (dir, root) = workspace("delete");
    let mut core = core_with_trash(&dir, &root);
    let mut r = Registry::new(vec![
        quiet("sheets", &[], &["csv"]),
        quiet("write", &[FilesRead, FilesWrite], &[]),
        quiet("tidy", &[FilesRead, FilesDelete], &[]),
    ]);
    let (sheets, write, tidy) = (ExtId(0), ExtId(1), ExtId(2));
    let csv = "Volcanoes/Eruptions.csv";
    let on_disk = |path: &str| std::fs::read_to_string(root.join(path)).unwrap();
    let forbidden = Err(Reason::Forbidden);

    let note = ask(&mut r, &mut core, tidy, |h| h.stat_file(NOTE)).unwrap();
    let table = ask(&mut r, &mut core, sheets, |h| h.stat_file(csv)).unwrap();
    let others = ask(&mut r, &mut core, sheets, |h| h.stat_file(NOTE));
    assert_eq!(others.map(drop), forbidden, "a note isn't its kind");

    let mut refusal = None;
    r.call(write, Some(&mut core), |_, h| {
        refusal = h.delete_file(NOTE, &note.rev).err()
    });
    let refusal = refusal.unwrap();
    assert_eq!(
        refusal.reason,
        Reason::Forbidden,
        "files.write doesn't delete"
    );
    assert!(
        (refusal.message).contains("write may not do this")
            && (refusal.message).contains("files.delete"),
        "the refusal names who asked and the permission: {}",
        refusal.message
    );
    let owner = ask(&mut r, &mut core, sheets, |h| {
        h.delete_file(csv, &table.rev)
    });
    assert_eq!(owner.map(drop), forbidden, "nor does owning the kind");
    assert_eq!(on_disk(NOTE), LATE, "untouched");
    assert!(root.join(csv).exists(), "untouched");
    assert!(!dir.join("trash").exists(), "nothing went");
    assert!(!root.join(".trash").exists(), "nothing went");

    let gone = ask(&mut r, &mut core, tidy, |h| h.delete_file(NOTE, &note.rev)).unwrap();
    assert!(!root.join(NOTE).exists());
    let trashed = std::fs::read_to_string(dir.join("trash/files/Seismograph.md")).unwrap();
    assert_eq!(trashed, LATE, "in the trash under the test's own folder");
    assert!(!root.join(".trash").exists());
    let deleted = Event::Deleted {
        path: NOTE.into(),
        by: "tidy".into(),
    };
    let events = core.poll();
    assert!(events.contains(&deleted), "{events:?}");

    let restore = Restore {
        trashed: gone.trashed,
        to: None,
    };
    let unasked = ask(&mut r, &mut core, write, |h| h.restore_file(&restore));
    assert_eq!(unasked.map(drop), forbidden, "files.write doesn't restore");
    let back = ask(&mut r, &mut core, tidy, |h| h.restore_file(&restore)).unwrap();
    assert_eq!(back.path, NOTE);
    assert_eq!(on_disk(NOTE), LATE, "exactly the bytes, where they were");

    let folder = "Inbox/Field Notes";
    let unasked = ask(&mut r, &mut core, tidy, |h| h.create_folder(folder));
    assert_eq!(unasked.map(drop), forbidden, "files.delete doesn't create");
    assert!(!root.join(folder).exists());
    let made = ask(&mut r, &mut core, write, |h| h.create_folder(folder)).unwrap();
    assert_eq!(made.path, folder);
    assert!(root.join(folder).is_dir());
    let _ = std::fs::remove_dir_all(&dir);
}

/// The `trash-can` extension of this folder in a process of its own, installed in `dir` with
/// `permissions` in place of its manifest's and the examples' `cold_lava.py` beside it.
fn trash_can(dir: &Path, permissions: &str) -> Registry {
    let folder = dir.join("extensions/trash-can");
    let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/trash-can");
    copy_dir(&source, &folder);
    let helper = repo().join("examples/extensions/open-tasks/cold_lava.py");
    std::fs::copy(helper, folder.join("cold_lava.py")).unwrap();
    let manifest = folder.join("extension.toml");
    let shipped = std::fs::read_to_string(&manifest).unwrap();
    let asks = "permissions = [\"files.read\", \"files.delete\"]";
    assert!(shipped.contains(asks), "it asks for files.delete");
    let given = shipped.replace(asks, &format!("permissions = {permissions}"));
    std::fs::write(&manifest, given).unwrap();
    let (found, mistakes) = installed::load(&dir.join("extensions"));
    assert_eq!(mistakes, Vec::<String>::new());
    let mut installed = found.into_iter().next().unwrap();
    installed.launch.log = Some(dir.join("trash-can.log"));
    Registry::new(vec![Box::new(Process::new(installed).unwrap())])
}

/// What `trash-can.delete` of `NOTE` did as an extension with `permissions`: what it said, the
/// note afterwards, what the test's trash holds under the note's name, and who the core says
/// deleted something.
struct Trashed {
    said: String,
    note: Option<String>,
    in_trash: Option<String>,
    deleted_by: Vec<String>,
}

fn trash(name: &str, permissions: &str) -> Trashed {
    let (dir, root) = workspace(name);
    let mut core = core_with_trash(&dir, &root);
    let mut r = trash_can(&dir, permissions);
    r.call(ExtId(0), Some(&mut core), |e, h| {
        e.workspace(Some(&root), h)
    });
    let run = CommandRequest::Run {
        id: "trash-can.delete".into(),
        args: serde_json::json!({ "path": NOTE }),
    };
    r.call(ExtId(0), Some(&mut core), |e, h| e.command(run, h));
    let (said, mut events) = until(&mut r, &mut core, |e| matches!(e, Effect::Notify { .. }));
    events.extend(core.poll());
    let Effect::Notify { message: said } = said else {
        unreachable!()
    };
    let deleted_by = (events.into_iter())
        .filter_map(|e| match e {
            Event::Deleted { by, .. } => Some(by),
            _ => None,
        })
        .collect();
    r.shutdown();
    assert!(!r.running(ExtId(0)));
    let log = std::fs::read_to_string(dir.join("trash-can.log")).unwrap();
    assert_eq!(log, "", "nothing on stderr");
    assert!(
        !root.join(".trash").exists(),
        "not the workspace's own trash"
    );
    let did = Trashed {
        said,
        note: std::fs::read_to_string(root.join(NOTE)).ok(),
        in_trash: std::fs::read_to_string(dir.join("trash/files/Seismograph.md")).ok(),
        deleted_by,
    };
    let _ = std::fs::remove_dir_all(&dir);
    did
}

/// Needs python3, which CI's runners have.
#[test]
fn an_extension_in_its_own_process_deletes_a_note_with_files_delete() {
    if !has_python() {
        return;
    }
    let did = trash("may-delete", "[\"files.read\", \"files.delete\"]");
    assert_eq!(did.said, format!("deleted {NOTE} to the system trash"));
    assert_eq!(did.note, None, "the note is gone");
    assert_eq!(
        did.in_trash.as_deref(),
        Some(LATE),
        "whole, in the trash under the test's own folder"
    );
    assert_eq!(did.deleted_by, ["trash-can"]);
}

/// Needs python3, which CI's runners have.
#[test]
fn an_extension_in_its_own_process_may_not_delete_without_files_delete() {
    if !has_python() {
        return;
    }
    let did = trash("may-not-delete", "[\"files.read\", \"files.write\"]");
    assert!(
        (did.said).starts_with("refused (forbidden): trash-can may not do this"),
        "{}",
        did.said
    );
    assert!(
        (did.said).contains("its manifest's permissions don't include files.delete"),
        "{}",
        did.said
    );
    assert_eq!(did.note.as_deref(), Some(LATE), "the note is untouched");
    assert_eq!(did.in_trash, None, "nothing went");
    assert_eq!(did.deleted_by, Vec::<String>::new());
}
