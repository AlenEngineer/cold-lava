//! Extensions installed on disk: one folder each under `~/.config/cold-lava/extensions/`, with
//! an `extension.toml` (docs/extensions.md, "Manifest"). Mistakes are collected like
//! config.toml's: one line each, the broken extension is left out and the rest load.

use serde::Deserialize;
use std::path::{Path, PathBuf};

use super::transport::{self, Launch};
use super::{CommandSpec, FileType, Manifest, Permission, TypeSpec, read_only, rpc};

/// Names an installed extension can't take: the built-in ones and the menu's screens, which edits
/// are attributed to.
const RESERVED: &[&str] = &[
    super::nvim::NAME,
    super::TABLE,
    super::AGENDA,
    super::READER,
];

/// An extension found on disk, ready to start.
#[derive(Clone, Debug, PartialEq)]
pub struct Installed {
    pub manifest: Manifest,
    pub launch: Launch,
}

/// `extension.toml` as written.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct File {
    name: String,
    title: Option<String>,
    version: String,
    #[serde(default)]
    description: String,
    #[serde(default = "api")]
    api: u32,
    #[serde(default = "stdio")]
    transport: String,
    run: Vec<String>,
    #[serde(default = "read_only")]
    permissions: Vec<Permission>,
    #[serde(default)]
    commands: Vec<CommandSpec>,
    #[serde(default)]
    types: Vec<TypeSpec>,
    #[serde(default, rename = "file-types")]
    file_types: Vec<FileType>,
}

fn api() -> u32 {
    rpc::API_VERSION
}

fn stdio() -> String {
    "stdio".into()
}

/// The user's extensions folder: `~/.config/cold-lava/extensions`.
pub fn user_dir() -> Option<PathBuf> {
    crate::paths::config_dir().map(|c| c.join("cold-lava/extensions"))
}

/// Where an extension's stderr goes.
pub fn log_path(name: &str) -> Option<PathBuf> {
    crate::paths::cache_dir().map(|c| c.join(format!("cold-lava/extensions/{name}.log")))
}

/// Every extension in `dir`'s folders, by folder name, and what was wrong with the rest.
pub fn load(dir: &Path) -> (Vec<Installed>, Vec<String>) {
    let mut folders: Vec<PathBuf> = match std::fs::read_dir(dir) {
        Ok(entries) => (entries.flatten())
            .map(|e| e.path())
            .filter(|p| p.is_dir())
            .collect(),
        Err(_) => return (Vec::new(), Vec::new()),
    };
    folders.sort();
    let (mut found, mut mistakes) = (Vec::<Installed>::new(), Vec::new());
    for folder in folders {
        let path = folder.join("extension.toml");
        if !path.is_file() {
            continue;
        }
        match read(&folder, &path) {
            Ok(ext) if found.iter().any(|f| f.manifest.name == ext.manifest.name) => {
                mistakes.push(format!(
                    "{}: another extension is called {}",
                    path.display(),
                    ext.manifest.name
                ))
            }
            Ok(ext) => found.push(ext),
            Err(why) => mistakes.push(format!("{}: {why}", path.display())),
        }
    }
    (found, mistakes)
}

fn read(folder: &Path, path: &Path) -> Result<Installed, String> {
    let text = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    let table: toml::Table = toml::from_str(&text).map_err(|e| e.message().to_string())?;
    if let Some(toml::Value::Table(_)) = table.get("types") {
        let name = table.get("name").and_then(|n| n.as_str()).unwrap_or("NAME");
        return Err(format!(
            "write each type as a [[types]] table with its id, like id = \"{name}.row\""
        ));
    }
    let file: File = (toml::Value::Table(table).try_into())
        .map_err(|e: toml::de::Error| e.message().to_string())?;
    let name = file.name;
    let named = |c: char| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-';
    if name.is_empty() || !name.chars().all(named) || name.starts_with('-') {
        return Err(format!(
            "the name {name:?} must be lowercase letters, digits and dashes"
        ));
    }
    if folder.file_name().and_then(|f| f.to_str()) != Some(name.as_str()) {
        return Err(format!("its folder must be called {name}, as its name"));
    }
    if RESERVED.contains(&name.as_str()) {
        return Err(format!("{name} is a name cold-lava keeps for itself"));
    }
    if file.api != rpc::API_VERSION {
        return Err(format!(
            "it needs API {}, and this cold-lava speaks {}",
            file.api,
            rpc::API_VERSION
        ));
    }
    if transport::connector(&file.transport).is_none() {
        let kinds: Vec<_> = transport::CONNECTORS.iter().map(|c| c.kind()).collect();
        return Err(format!(
            "transport {:?} isn't one cold-lava has ({})",
            file.transport,
            kinds.join(", ")
        ));
    }
    if file.run.is_empty() {
        return Err("run must name a program".into());
    }
    let mut file_types = file.file_types;
    for t in &mut file_types {
        t.extension = t.extension.trim_start_matches('.').to_ascii_lowercase();
        let x = &t.extension;
        let fits = |c: char| c.is_ascii_alphanumeric() || c == '-' || c == '_';
        if x.is_empty() || !x.chars().all(fits) {
            return Err(format!(
                "file type {x:?} must be an extension of letters, digits, - and _, like \"csv\""
            ));
        }
        if let Some(new) = &t.new
            && !file.commands.iter().any(|c| c.id == *new)
        {
            return Err(format!(
                "file type .{x} makes new files with {new}, which isn't one of its [[commands]]"
            ));
        }
    }
    let manifest = Manifest {
        title: file.title.unwrap_or_else(|| name.clone()),
        version: file.version,
        description: file.description,
        roles: Vec::new(),
        hints: Vec::new(),
        commands: file.commands,
        types: file.types,
        permissions: file.permissions,
        file_types,
        name,
    };
    let launch = Launch {
        dir: folder.to_path_buf(),
        transport: file.transport,
        run: file.run,
        log: log_path(&manifest.name),
    };
    Ok(Installed { manifest, launch })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir(name: &str, manifests: &[(&str, &str)]) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("cold-lava-installed-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        for (folder, text) in manifests {
            std::fs::create_dir_all(dir.join(folder)).unwrap();
            std::fs::write(dir.join(folder).join("extension.toml"), text).unwrap();
        }
        dir
    }

    const WC: &str = r#"
name = "wc"
version = "0.1.0"
description = "Counts words"
run = ["python3", "main.py"]
permissions = ["items.read", "items.edit"]

[[types]]
id = "wc.doc"
on = "note"
fields.words = { property = "words", type = "number" }

[[commands]]
id = "wc.show"
title = "Word count"
keys = ["W"]

[[commands]]
id = "wc.quiet"
title = "No keys"

[[file-types]]
extension = ".WC"
title = "Word counts"
new = "wc.show"
"#;

    #[test]
    fn loads_a_manifest() {
        let d = dir("ok", &[("wc", WC)]);
        let (found, mistakes) = load(&d);
        assert_eq!(mistakes, Vec::<String>::new());
        let [wc] = &found[..] else {
            panic!("{found:?}")
        };
        let m = &wc.manifest;
        assert_eq!((m.name.as_str(), m.title.as_str()), ("wc", "wc"));
        assert_eq!(
            (m.version.as_str(), m.description.as_str()),
            ("0.1.0", "Counts words")
        );
        assert_eq!(
            m.permissions,
            [Permission::ItemsRead, Permission::ItemsEdit]
        );
        assert_eq!(m.commands.len(), 2);
        assert!(m.commands[1].keys.is_empty());
        assert_eq!(m.types[0].id, "wc.doc");
        assert_eq!(m.types[0].definition["on"].as_str(), Some("note"));
        let counts = FileType {
            extension: "wc".into(),
            title: "Word counts".into(),
            new: Some("wc.show".into()),
        };
        assert_eq!(m.file_types, [counts], "lowercase, without the dot");
        assert_eq!(wc.launch.dir, d.join("wc"));
        assert_eq!(wc.launch.run, ["python3", "main.py"]);
        assert_eq!(wc.launch.transport, "stdio");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn mistakes_leave_one_extension_out() {
        let with = |from: &str, to: &str| WC.replace(from, to);
        let d = dir(
            "mistakes",
            &[
                ("a", &with("name = \"wc\"", "name = \"a\"")),
                ("b", &with("name = \"wc\"", "name = \"Bad Name\"")),
                ("c", &with("name = \"wc\"", "name = \"other\"")),
                ("nvim", &with("name = \"wc\"", "name = \"nvim\"")),
                ("d", &with("name = \"wc\"", "name = \"d\"\napi = 2")),
                (
                    "e",
                    &with("name = \"wc\"", "name = \"e\"\ntransport = \"tcp\""),
                ),
                (
                    "f",
                    &with("\"items.edit\"", "\"files.create\"").replace("wc", "f"),
                ),
                ("g", &with("name = \"wc\"", "name = \"g\"\ncolour = 1")),
                (
                    "h",
                    &with("[\"python3\", \"main.py\"]", "[]").replace("wc", "h"),
                ),
                ("i", "name = "),
                (
                    "j",
                    &(with("name = \"wc\"", "name = \"j\"") + "icon = \"x\"\n"),
                ),
                (
                    "k",
                    &with("extension = \".WC\"", "extension = \"tar.gz\"").replace("wc", "k"),
                ),
                (
                    "l",
                    &with("new = \"wc.show\"", "new = \"wc.make\"").replace("wc", "l"),
                ),
                (
                    "m",
                    &with("[[types]]\nid = \"wc.doc\"", "[types.doc]").replace("wc", "m"),
                ),
            ],
        );
        std::fs::create_dir_all(d.join("no-manifest")).unwrap();
        let (found, mistakes) = load(&d);
        let names: Vec<_> = found.iter().map(|f| f.manifest.name.as_str()).collect();
        assert_eq!(names, ["a"]);
        let says = |i: usize, what: &str| {
            assert!(mistakes[i].contains(what), "{i}: {}", mistakes[i]);
        };
        assert_eq!(mistakes.len(), 13, "{mistakes:#?}");
        says(0, "lowercase letters");
        says(1, "folder must be called other");
        says(2, "API 2");
        says(3, "\"tcp\" isn't one cold-lava has (stdio)");
        says(4, "files.create");
        says(5, "colour");
        says(6, "run must name a program");
        says(7, "/i/extension.toml");
        says(8, "unknown field `icon`");
        says(9, "\"tar.gz\" must be an extension");
        says(10, "l.make, which isn't one of its [[commands]]");
        says(
            11,
            "write each type as a [[types]] table with its id, like id = \"m.row\"",
        );
        says(12, "keeps for itself");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn no_folder_is_no_extensions() {
        let none = std::env::temp_dir().join("cold-lava-no-such-extensions-folder");
        assert_eq!(load(&none), (Vec::new(), Vec::new()));
    }
}
