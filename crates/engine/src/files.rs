//! The file messages of the workspace gateway (docs/extensions.md, "File types, as built"):
//! `workspace/read` gives a file's text and its `rev`, `workspace/write` replaces it only if it
//! is still that version, `workspace/create` makes a new one where Obsidian would and never
//! overwrites, and `workspace/files` lists the files of a kind. Paths are workspace-relative and
//! stay inside the workspace. Every type here derives serde and tests pin its JSON. The core
//! answers them (`Core::read_file`, …); the frontend's `Host` checks permissions first.

use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::gateway::{Error, Reason};
use crate::index::Index;
use crate::tasks::fnv;
use crate::write::{self, WriteError};

/// What an extension asks of the workspace gateway about files (extension → host).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "method", content = "params")]
pub enum Request {
    #[serde(rename = "workspace/read")]
    Read { path: String },
    /// Replace the file's text, refused unless it is still the version `expect`.
    #[serde(rename = "workspace/write")]
    Write {
        path: String,
        text: String,
        expect: String,
    },
    #[serde(rename = "workspace/create")]
    Create(Create),
    /// Every file whose name ends in `.<extension>`.
    #[serde(rename = "workspace/files")]
    Files { extension: String },
}

/// `workspace/create`: a new file at `path`, or named `name` with `extension` where Obsidian
/// puts a new file. Never overwrites.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Create {
    #[serde(flatten)]
    pub at: NewFile,
    #[serde(default)]
    pub text: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum NewFile {
    /// Exactly here; refused as `exists` when something is.
    At { path: String },
    /// In the folder for new files (`.obsidian/app.json`), as `name.extension`, or `name
    /// 1.extension` and so on when that is taken.
    Named { name: String, extension: String },
}

impl Create {
    /// The kind of file it makes (`csv`), which permissions are checked against.
    pub fn extension(&self) -> Option<String> {
        match &self.at {
            NewFile::At { path } => extension_of(path),
            NewFile::Named { extension, .. } => Some(extension.to_ascii_lowercase()),
        }
    }
}

/// The answer to `workspace/read`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Text {
    pub text: String,
    pub rev: String,
}

/// The answer to `workspace/write`: the version now on disk.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Written {
    pub rev: String,
}

/// The answer to `workspace/create`: where it went, and its version.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Created {
    pub path: String,
    pub rev: String,
}

/// The answer to `workspace/files`: workspace-relative paths, sorted.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Listed {
    pub files: Vec<String>,
}

/// A file's version: a hash of its bytes, as 16 hex digits.
pub fn rev(bytes: &[u8]) -> String {
    format!("{:016x}", fnv(bytes))
}

/// The version of the file at `rel` now, `None` when it is gone or can't be read.
pub fn rev_at(root: &Path, rel: &str) -> Option<String> {
    std::fs::read(root.join(rel)).ok().map(|b| rev(&b))
}

/// The suffix of a path or file name (`Data/Eruptions.CSV` → `csv`), lowercase.
pub fn extension_of(path: &str) -> Option<String> {
    let file = path.rsplit('/').next().unwrap_or(path);
    let (stem, ext) = file.rsplit_once('.')?;
    (!stem.is_empty() && !ext.is_empty()).then(|| ext.to_ascii_lowercase())
}

/// The file at `rel` in the workspace at `root`: refused unless `rel` is relative, has no
/// `..`, `.` or hidden part (`.obsidian`, `.trash`, `.git`), and leads inside the workspace once
/// symlinks are followed.
pub fn resolve(root: &Path, rel: &str) -> Result<PathBuf, Error> {
    let refuse = |why: &str| Error::new(Reason::Refused, format!("{rel}: {why}"));
    let p = Path::new(rel);
    if rel.is_empty() || p.is_absolute() {
        return Err(refuse("a path relative to the workspace is needed"));
    }
    for c in p.components() {
        match c {
            Component::Normal(s) if !s.to_string_lossy().starts_with('.') => {}
            _ => {
                return Err(refuse(
                    "a path can't have .., . or hidden folders and files",
                ));
            }
        }
    }
    let path = root.join(p);
    let real_root = root.canonicalize().map_err(io(rel))?;
    let existing = path.ancestors().find(|a| a.exists()).unwrap_or(root);
    let real = existing.canonicalize().map_err(io(rel))?;
    if !real.starts_with(&real_root) {
        return Err(refuse("it leads outside the workspace"));
    }
    Ok(path)
}

fn io(rel: &str) -> impl Fn(std::io::Error) -> Error + '_ {
    move |e| Error::new(Reason::Io, format!("{rel}: {e}"))
}

fn written(rel: &str) -> impl Fn(WriteError) -> Error + '_ {
    move |e| match e {
        WriteError::Changed(_) => Error::new(
            Reason::Changed,
            format!("{rel} changed while writing: read it again"),
        ),
        WriteError::Exists(_) => Error::new(Reason::Exists, format!("{rel} already exists")),
        WriteError::Io(_, e) => Error::new(Reason::Io, format!("{rel}: {e}")),
    }
}

/// `workspace/read`
pub fn read(root: &Path, rel: &str) -> Result<Text, Error> {
    let bytes = std::fs::read(resolve(root, rel)?).map_err(io(rel))?;
    let rev = rev(&bytes);
    let text = String::from_utf8(bytes)
        .map_err(|_| Error::new(Reason::Refused, format!("{rel} isn't UTF-8 text")))?;
    Ok(Text { text, rev })
}

/// `workspace/write`, through `write::replace`.
pub fn replace(root: &Path, rel: &str, text: &str, expect: &str) -> Result<Written, Error> {
    let path = resolve(root, rel)?;
    let now = std::fs::read(&path).map_err(|e| match e.kind() {
        std::io::ErrorKind::NotFound => Error::new(
            Reason::Changed,
            format!("{rel} is gone: create it with workspace/create"),
        ),
        _ => io(rel)(e),
    })?;
    if rev(&now) != expect {
        let message = format!("{rel} changed since it was read: read it again");
        return Err(Error::new(Reason::Changed, message));
    }
    write::replace(&path, &now, text.as_bytes()).map_err(written(rel))?;
    Ok(Written {
        rev: rev(text.as_bytes()),
    })
}

/// `workspace/create`, through `write::create`.
pub fn create(index: &Index, c: &Create) -> Result<Created, Error> {
    let root = &index.workspace.root;
    let rel = match &c.at {
        NewFile::At { path } => path.clone(),
        NewFile::Named { name, extension } => {
            let bad = extension.is_empty() || extension.contains(['/', '.', '\\']);
            if bad || name.trim().is_empty() {
                let message = format!("{name:?} with extension {extension:?} can't name a file");
                return Err(Error::new(Reason::Refused, message));
            }
            free_name(index, name.trim(), extension)
        }
    };
    let path = resolve(root, &rel)?;
    write::create(&path, c.text.as_bytes()).map_err(written(&rel))?;
    Ok(Created {
        path: rel,
        rev: rev(c.text.as_bytes()),
    })
}

/// Where a new `name.extension` goes, as workspace-relative path: the first of `name`,
/// `name 1`, `name 2`, … that is free, as Obsidian numbers a new file.
fn free_name(index: &Index, name: &str, extension: &str) -> String {
    let root = &index.workspace.root;
    let relative = |p: PathBuf| {
        let rel = p.strip_prefix(root).map(Path::to_path_buf).unwrap_or(p);
        rel.to_string_lossy().replace('\\', "/")
    };
    let mut n = 0;
    loop {
        let candidate = match n {
            0 => name.to_string(),
            n => format!("{name} {n}"),
        };
        let path = index.new_file_path(&candidate, extension, None);
        if !path.exists() {
            return relative(path);
        }
        n += 1;
    }
}

/// `workspace/files`: the workspace's files that end in `.<extension>`, outside hidden folders.
pub fn list(index: &Index, extension: &str) -> Listed {
    let want = extension.trim_start_matches('.').to_ascii_lowercase();
    let all = crate::rename::Files::of_workspace(index).rels;
    let mut files: Vec<String> = (all.into_iter())
        .filter(|f| extension_of(f).as_deref() == Some(want.as_str()))
        .collect();
    files.sort();
    Listed { files }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::{Core, Event};
    use serde::de::DeserializeOwned;
    use serde_json::{Value, json};
    use std::fmt::Debug;
    use std::time::Duration;

    fn wire<T: Serialize + DeserializeOwned + PartialEq + Debug>(value: T, json: Value) {
        assert_eq!(serde_json::to_value(&value).unwrap(), json);
        assert_eq!(serde_json::from_value::<T>(json).unwrap(), value);
    }

    fn workspace(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("cold-lava-files-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join(".obsidian")).unwrap();
        std::fs::create_dir_all(dir.join("Data")).unwrap();
        std::fs::write(dir.join("Plan.md"), "[[Data/Eruptions.csv]]\n").unwrap();
        std::fs::write(dir.join("Data/Eruptions.csv"), "name,year\nKrakatoa,1883\n").unwrap();
        dir
    }

    #[test]
    fn requests_and_answers_are_json() {
        wire(
            Request::Read {
                path: "Data/Eruptions.csv".into(),
            },
            json!({"method": "workspace/read", "params": {"path": "Data/Eruptions.csv"}}),
        );
        wire(
            Request::Write {
                path: "a.csv".into(),
                text: "x\n".into(),
                expect: "00ff".into(),
            },
            json!({"method": "workspace/write",
                   "params": {"path": "a.csv", "text": "x\n", "expect": "00ff"}}),
        );
        wire(
            Request::Create(Create {
                at: NewFile::Named {
                    name: "Untitled".into(),
                    extension: "csv".into(),
                },
                text: "a,b\n".into(),
            }),
            json!({"method": "workspace/create",
                   "params": {"name": "Untitled", "extension": "csv", "text": "a,b\n"}}),
        );
        wire(
            Request::Create(Create {
                at: NewFile::At {
                    path: "Data/New.csv".into(),
                },
                text: String::new(),
            }),
            json!({"method": "workspace/create", "params": {"path": "Data/New.csv", "text": ""}}),
        );
        wire(
            Request::Files {
                extension: "csv".into(),
            },
            json!({"method": "workspace/files", "params": {"extension": "csv"}}),
        );
        wire(
            Text {
                text: "a".into(),
                rev: rev(b"a"),
            },
            json!({"text": "a", "rev": "af63dc4c8601ec8c"}),
        );
        wire(
            Created {
                path: "Untitled.csv".into(),
                rev: "1".into(),
            },
            json!({"path": "Untitled.csv", "rev": "1"}),
        );
        wire(
            Listed {
                files: vec!["a.csv".into()],
            },
            json!({"files": ["a.csv"]}),
        );
    }

    #[test]
    fn paths_stay_inside_the_workspace() {
        let dir = workspace("paths");
        let refused = |rel: &str| resolve(&dir, rel).map_err(|e| e.reason);
        assert!(refused("Data/Eruptions.csv").is_ok());
        assert!(refused("Data/New/Deeper.csv").is_ok(), "may not exist yet");
        for bad in [
            "",
            "../outside.csv",
            "Data/../../outside.csv",
            "./Plan.md",
            ".obsidian/app.json",
            "Data/.hidden.csv",
        ] {
            assert_eq!(refused(bad), Err(Reason::Refused), "{bad}");
        }
        let absolute = dir.join("Plan.md");
        assert_eq!(
            refused(&absolute.to_string_lossy()),
            Err(Reason::Refused),
            "absolute"
        );
        #[cfg(unix)]
        {
            let outside = dir.with_extension("outside");
            std::fs::create_dir_all(&outside).unwrap();
            std::fs::write(outside.join("secret.csv"), "x").unwrap();
            std::os::unix::fs::symlink(&outside, dir.join("Link")).unwrap();
            assert_eq!(refused("Link/secret.csv"), Err(Reason::Refused));
            assert_eq!(
                refused("Link/new.csv"),
                Err(Reason::Refused),
                "nor made there"
            );
            let _ = std::fs::remove_dir_all(&outside);
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn writes_are_refused_when_the_file_changed_since_it_was_read() {
        let dir = workspace("write");
        let read = read(&dir, "Data/Eruptions.csv").unwrap();
        assert_eq!(read.text, "name,year\nKrakatoa,1883\n");
        let text = "name,year\nKrakatoa,1883\nTambora,1815\n";
        let w = replace(&dir, "Data/Eruptions.csv", text, &read.rev).unwrap();
        assert_eq!(w.rev, rev(text.as_bytes()));
        let stale = replace(&dir, "Data/Eruptions.csv", "lost", &read.rev).unwrap_err();
        assert_eq!(stale.reason, Reason::Changed);
        let on_disk = std::fs::read_to_string(dir.join("Data/Eruptions.csv")).unwrap();
        assert_eq!(on_disk, text, "nothing overwritten");
        let gone = replace(&dir, "Data/Gone.csv", "x", &read.rev).unwrap_err();
        assert_eq!(gone.reason, Reason::Changed);
        std::fs::write(dir.join("Data/bytes.csv"), [0xff, 0xfe]).unwrap();
        let binary = super::read(&dir, "Data/bytes.csv").unwrap_err();
        assert_eq!(binary.reason, Reason::Refused);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn created_files_go_where_obsidian_puts_new_files_and_never_overwrite() {
        let dir = workspace("create");
        let app = r#"{"newFileLocation": "folder", "newFileFolderPath": "Data"}"#;
        std::fs::write(dir.join(".obsidian/app.json"), app).unwrap();
        let mut core = Core::open(&dir).unwrap();
        let named = |name: &str| {
            Request::Create(Create {
                at: NewFile::Named {
                    name: name.into(),
                    extension: "csv".into(),
                },
                text: "a\n".into(),
            })
        };
        let Request::Create(untitled) = named("Untitled") else {
            unreachable!()
        };
        let first = core.create_file(&untitled).unwrap();
        assert_eq!(first.path, "Data/Untitled.csv");
        assert_eq!(first.rev, rev(b"a\n"));
        let second = core.create_file(&untitled).unwrap();
        assert_eq!(
            second.path, "Data/Untitled 1.csv",
            "numbered, not overwritten"
        );
        let Request::Create(eruptions) = named("Eruptions") else {
            unreachable!()
        };
        assert_eq!(
            core.create_file(&eruptions).unwrap().path,
            "Data/Eruptions 1.csv"
        );
        let at = Create {
            at: NewFile::At {
                path: "Data/Eruptions.csv".into(),
            },
            text: "lost".into(),
        };
        assert_eq!(core.create_file(&at).unwrap_err().reason, Reason::Exists);
        let bad = Create {
            at: NewFile::Named {
                name: "x".into(),
                extension: "../csv".into(),
            },
            text: String::new(),
        };
        assert_eq!(core.create_file(&bad).unwrap_err().reason, Reason::Refused);

        let csv = core.files("csv").files;
        assert_eq!(
            csv,
            [
                "Data/Eruptions 1.csv",
                "Data/Eruptions.csv",
                "Data/Untitled 1.csv",
                "Data/Untitled.csv"
            ]
        );
        assert_eq!(
            core.files(".CSV").files.len(),
            4,
            "any case, with or without the dot"
        );
        assert_eq!(core.files("md").files, ["Plan.md"]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn writing_a_note_reads_it_again_and_outside_changes_to_files_are_events() {
        let dir = workspace("events");
        let mut core = Core::open(&dir).unwrap();
        let note = core.read_file("Plan.md").unwrap();
        core.write_file("Plan.md", "[[Lava]]\n", &note.rev).unwrap();
        assert!(
            core.has_unseen(),
            "the note's events wait for the next poll"
        );
        let plan = core.index.by_rel("Plan").unwrap();
        assert_eq!(core.index.workspace.notes[plan].links, ["Lava"]);
        if core.is_live() {
            std::fs::write(dir.join("Data/Eruptions.csv"), "name\n").unwrap();
            let mut seen = Vec::new();
            for _ in 0..100 {
                seen.extend(core.poll());
                if seen.contains(&Event::FileChanged("Data/Eruptions.csv".into()))
                    || seen.contains(&Event::Rescanned)
                {
                    break;
                }
                std::thread::sleep(Duration::from_millis(30));
            }
            assert!(
                seen.contains(&Event::FileChanged("Data/Eruptions.csv".into()))
                    || seen.contains(&Event::Rescanned),
                "{seen:?}"
            );
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
