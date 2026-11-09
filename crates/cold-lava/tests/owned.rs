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
        "Volcano,Year,VEI,Notes\n[[Vesuvius]],79,5,\n\
         [[Krakatau]],1883,6,- [ ] Read the Royal Society report on [[Krakatau]] 📅 2026-11-20\n\
         [[Mount St. Helens]],1980,5,\n[[Eyjafjallajökull]],2010,4,\n",
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
