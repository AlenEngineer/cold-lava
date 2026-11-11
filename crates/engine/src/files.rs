//! The file messages of the workspace gateway (docs/extensions.md, "File types, as built"):
//! `workspace/read` gives a file's text and its `rev`, `workspace/write` replaces it only if it
//! is still that version, `workspace/create` makes a new one where Obsidian would and never
//! overwrites, and `workspace/files` lists the files of a kind. `workspace/stat` gives the `rev`
//! of any file or folder, `workspace/delete` moves one to the trash if it is still that version,
//! `workspace/restore` puts it back and `workspace/create-folder` makes a folder
//! (docs/extensions.md, "Deleting, restoring and making folders"). Paths are workspace-relative and
//! stay inside the workspace. Every type here derives serde and tests pin its JSON. The core
//! answers them (`Core::read_file`, …); the frontend's `Host` checks permissions first.

use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::gateway::{Error, Reason};
use crate::index::Index;
use crate::tasks::fnv;
use crate::trash::{self, Bin, Contents, Trashed, Went};
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
    /// The version of a file or a folder, without its text.
    #[serde(rename = "workspace/stat")]
    Stat { path: String },
    /// Move a file or a folder to the trash, refused unless it is still the version `expect`.
    #[serde(rename = "workspace/delete")]
    Delete { path: String, expect: String },
    #[serde(rename = "workspace/restore")]
    Restore(Restore),
    /// A new, empty folder at `path`, with any missing parents.
    #[serde(rename = "workspace/create-folder")]
    CreateFolder { path: String },
}

/// `workspace/restore`: put what `trashed` names back where it was, or at `to`. Never
/// overwrites.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Restore {
    pub trashed: TrashId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to: Option<String>,
}

/// Something the core put in a trash and can put back: a token it hands out, new for each
/// delete and each run of the core.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct TrashId(pub String);

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

/// The answer to `workspace/stat`. For a folder, `inside` lists what is in it at any depth,
/// workspace-relative, sorted, a folder ending in `/`: the first `LISTED` entries a client may
/// read. `more` counts the ones after those, `hidden` the ones under a hidden name (`.git`),
/// which are never listed. The `rev` covers them all.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Stat {
    pub rev: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inside: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub more: usize,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub hidden: usize,
}

fn is_zero(n: &usize) -> bool {
    *n == 0
}

/// How many of a folder's entries `workspace/stat` lists.
pub const LISTED: usize = 1000;

/// The answer to `workspace/delete`: the token that restores it, and which trash took it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Deleted {
    pub trashed: TrashId,
    pub went: Went,
}

/// The answer to `workspace/restore`: where it is now.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Restored {
    pub path: String,
}

/// The answer to `workspace/create-folder`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Folder {
    pub path: String,
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
    let path = named(root, rel)?;
    within(root, rel, &path)?;
    Ok(path)
}

/// The entry at `rel` itself, for `workspace/stat` and `workspace/delete`: as `resolve`, but
/// the last part is not followed when it is a symlink. The folder it is in has to lead inside
/// the workspace; the link is then an entry of the workspace like any other, whatever it
/// points to.
fn resolve_entry(root: &Path, rel: &str) -> Result<PathBuf, Error> {
    let path = named(root, rel)?;
    within(root, rel, path.parent().unwrap_or(root))?;
    Ok(path)
}

/// `rel` under `root`, refused unless it is relative and has no `..`, `.` or hidden part.
fn named(root: &Path, rel: &str) -> Result<PathBuf, Error> {
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
    Ok(root.join(p))
}

/// Refused unless `path`, or its deepest folder that exists, leads inside the workspace once
/// symlinks are followed.
fn within(root: &Path, rel: &str, path: &Path) -> Result<(), Error> {
    let real_root = root.canonicalize().map_err(io(rel))?;
    let existing = path.ancestors().find(|a| a.exists()).unwrap_or(root);
    let real = existing.canonicalize().map_err(io(rel))?;
    if !real.starts_with(&real_root) {
        let message = format!("{rel}: it leads outside the workspace");
        return Err(Error::new(Reason::Refused, message));
    }
    Ok(())
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
        let path = crate::resolver::new_file_path(index, &candidate, extension, None);
        if !path.exists() {
            return relative(path);
        }
        n += 1;
    }
}

/// The version of what is at a path, as a delete checks it. A folder's is a hash of its
/// listing: every path in it with its size and modified time.
fn rev_of(contents: &Contents) -> String {
    match contents {
        Contents::File(bytes) => rev(bytes),
        Contents::Folder(entries) => {
            let mut listing = b"folder\n".to_vec();
            for (path, size, modified) in entries {
                let since = modified.and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok());
                let nanos = since.map_or(0, |d| d.as_nanos());
                listing.extend(path.to_string_lossy().as_bytes());
                listing.extend(format!("\0{size}\0{nanos}\n").as_bytes());
            }
            rev(&listing)
        }
        Contents::Link(target) => {
            let mut link = b"link\n".to_vec();
            link.extend(target.to_string_lossy().as_bytes());
            rev(&link)
        }
    }
}

/// What is at `rel` now. Nothing there is `changed`.
fn contents(root: &Path, rel: &str) -> Result<(PathBuf, Contents), Error> {
    let path = resolve_entry(root, rel)?;
    let read = Contents::read(&path).map_err(|e| match e {
        WriteError::Io(_, e) if e.kind() == std::io::ErrorKind::NotFound => {
            Error::new(Reason::Changed, format!("{rel} is gone"))
        }
        e => moved(rel)(e),
    })?;
    Ok((path, read))
}

fn moved(rel: &str) -> impl Fn(WriteError) -> Error + '_ {
    move |e| match e {
        WriteError::Changed(_) => Error::new(
            Reason::Changed,
            format!("{rel} changed since it was read: read it again"),
        ),
        WriteError::Exists(_) => Error::new(Reason::Exists, format!("{rel} already exists")),
        WriteError::Io(_, e) => Error::new(Reason::Io, format!("{rel}: {e}")),
    }
}

/// `workspace/stat`
pub fn stat(root: &Path, rel: &str) -> Result<Stat, Error> {
    let (path, read) = contents(root, rel)?;
    let mut stat = Stat {
        rev: rev_of(&read),
        inside: None,
        more: 0,
        hidden: 0,
    };
    if let Contents::Folder(entries) = &read {
        let folder = rel.trim_end_matches('/');
        let is_hidden = |entry: &Path| {
            let mut parts = entry.components();
            parts.any(|c| c.as_os_str().to_string_lossy().starts_with('.'))
        };
        let (hidden, seen): (Vec<_>, Vec<_>) =
            (entries.iter().map(|(entry, _, _)| entry)).partition(|entry| is_hidden(entry));
        let listed = seen.iter().take(LISTED).map(|entry| {
            let slash = if path.join(entry).is_dir() { "/" } else { "" };
            let entry = entry.to_string_lossy().replace('\\', "/");
            format!("{folder}/{entry}{slash}")
        });
        stat.inside = Some(listed.collect());
        stat.more = seen.len().saturating_sub(LISTED);
        stat.hidden = hidden.len();
    }
    Ok(stat)
}

/// `workspace/delete`, through `trash::delete`: to the trash the workspace's `trashOption`
/// names (`system` is the system trash, when there is one), never for good.
pub fn delete(
    root: &Path,
    rel: &str,
    expect: &str,
    system: Option<&Bin>,
) -> Result<Trashed, Error> {
    let (path, now) = contents(root, rel)?;
    if rev_of(&now) != expect {
        let message = format!("{rel} changed since it was read: read it again");
        return Err(Error::new(Reason::Changed, message));
    }
    trash::delete(root, &path, &now, system).map_err(moved(rel))
}

/// `workspace/restore`, through `trash::restore`: back where it was, or at `to`. The answer
/// says where it is now.
pub fn restore(root: &Path, trashed: &Trashed, to: Option<&str>) -> Result<Restored, Error> {
    let from = trashed.from.strip_prefix(root).unwrap_or(&trashed.from);
    let from = from.to_string_lossy().replace('\\', "/");
    let rel = match to {
        // Somewhere else is a new name, under the rules a new folder's has.
        Some(to) if to != from => {
            let unfit = |e: anyhow::Error| Error::new(Reason::Refused, e.to_string());
            crate::rename::check_path(to).map_err(unfit)?;
            to.to_string()
        }
        _ => from,
    };
    let path = resolve(root, &rel)?;
    if trashed.at.symlink_metadata().is_err() {
        let message = format!("{rel} is no longer in the trash");
        return Err(Error::new(Reason::Changed, message));
    }
    trash::restore(trashed, &path).map_err(moved(&rel))?;
    Ok(Restored { path: rel })
}

/// `workspace/create-folder`, through `write::create_dir`, under a name links can point into
/// (`rename::check_path`).
pub fn create_folder(root: &Path, rel: &str) -> Result<Folder, Error> {
    crate::rename::check_path(rel).map_err(|e| Error::new(Reason::Refused, e.to_string()))?;
    let path = resolve(root, rel)?;
    write::create_dir(&path).map_err(moved(rel))?;
    let path = rel.to_string();
    Ok(Folder { path })
}

/// How many deletes of one client the core keeps tokens for.
const KEPT: usize = 256;

/// What the core put in a trash, by token, each with the client that deleted it. A token is
/// that client's alone: to any other it names nothing.
#[derive(Default)]
pub struct Trash {
    kept: Vec<(TrashId, String, Trashed)>,
    /// Keyed at random when this core opened, for tokens nobody can work out: not from
    /// another token, and not from a run before.
    keys: std::collections::hash_map::RandomState,
    next: u64,
}

/// A client's deletes, taken out of one core to be given to another (`Core::take_trashed`,
/// `keep_trashed`): how a frontend's undo outlives the core it deleted through. Only a core
/// makes one.
#[derive(Default)]
pub struct Kept(Vec<(TrashId, Trashed)>);

impl Trash {
    /// The token for what `by` just deleted: 128 bits of the keyed hash of a counter. Its
    /// oldest is forgotten once it has `KEPT`.
    pub fn keep(&mut self, by: &str, trashed: Trashed) -> TrashId {
        use std::hash::BuildHasher;
        self.next += 1;
        let (high, low) = (
            self.keys.hash_one((0u8, self.next)),
            self.keys.hash_one((1u8, self.next)),
        );
        let id = TrashId(format!("{high:016x}{low:016x}"));
        let mine = |(_, owner, _): &(TrashId, String, Trashed)| owner == by;
        if self.kept.iter().filter(|k| mine(k)).count() >= KEPT
            && let Some(oldest) = self.kept.iter().position(mine)
        {
            self.kept.remove(oldest);
        }
        self.kept.push((id.clone(), by.to_string(), trashed));
        id
    }

    /// What `by` deleted under `id`. Another client's token is as one nobody was given.
    pub fn get(&self, by: &str, id: &TrashId) -> Option<&Trashed> {
        let mut all = self.kept.iter();
        let found = all.find(|(kept, owner, _)| kept == id && owner == by);
        found.map(|(_, _, trashed)| trashed)
    }

    /// After `by` put it back.
    pub fn forget(&mut self, by: &str, id: &TrashId) {
        self.kept
            .retain(|(kept, owner, _)| kept != id || owner != by);
    }

    /// Everything `by` deleted, out of this core.
    pub fn take(&mut self, by: &str) -> Kept {
        let all = std::mem::take(&mut self.kept).into_iter();
        let (taken, left): (Vec<_>, Vec<_>) = all.partition(|(_, owner, _)| owner == by);
        self.kept = left;
        Kept(taken.into_iter().map(|(id, _, t)| (id, t)).collect())
    }

    /// `by`'s deletes from an earlier core, under the tokens they had.
    pub fn give(&mut self, by: &str, kept: Kept) {
        let theirs = kept.0.into_iter();
        self.kept
            .extend(theirs.map(|(id, trashed)| (id, by.to_string(), trashed)));
    }
}

/// `workspace/files`: the workspace's files that end in `.<extension>`, outside hidden folders.
pub fn list(index: &Index, extension: &str) -> Listed {
    let want = extension.trim_start_matches('.').to_ascii_lowercase();
    let all = crate::resolver::Files::of_workspace(index).rels;
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
            Request::Stat {
                path: "Concepts".into(),
            },
            json!({"method": "workspace/stat", "params": {"path": "Concepts"}}),
        );
        wire(
            Request::Delete {
                path: "Concepts/Lava.md".into(),
                expect: "af63dc4c8601ec8c".into(),
            },
            json!({"method": "workspace/delete",
                   "params": {"path": "Concepts/Lava.md", "expect": "af63dc4c8601ec8c"}}),
        );
        let trashed = TrashId("18a1f3c2d4-1".into());
        wire(
            Request::Restore(Restore {
                trashed: trashed.clone(),
                to: None,
            }),
            json!({"method": "workspace/restore", "params": {"trashed": "18a1f3c2d4-1"}}),
        );
        wire(
            Request::Restore(Restore {
                trashed: trashed.clone(),
                to: Some("Concepts/Lava 1.md".into()),
            }),
            json!({"method": "workspace/restore",
                   "params": {"trashed": "18a1f3c2d4-1", "to": "Concepts/Lava 1.md"}}),
        );
        wire(
            Request::CreateFolder {
                path: "Guide/Field Notes".into(),
            },
            json!({"method": "workspace/create-folder", "params": {"path": "Guide/Field Notes"}}),
        );
        wire(
            Stat {
                rev: rev(b"a"),
                inside: None,
                more: 0,
                hidden: 0,
            },
            json!({"rev": "af63dc4c8601ec8c"}),
        );
        wire(
            Stat {
                rev: "9c1d0a52e17b3f04".into(),
                inside: Some(vec!["Concepts/Lava.md".into(), "Concepts/Old/".into()]),
                more: 12,
                hidden: 3,
            },
            json!({"rev": "9c1d0a52e17b3f04", "inside": ["Concepts/Lava.md", "Concepts/Old/"],
                   "more": 12, "hidden": 3}),
        );
        wire(
            Deleted {
                trashed: trashed.clone(),
                went: Went::System,
            },
            json!({"trashed": "18a1f3c2d4-1", "went": "system"}),
        );
        wire(
            Deleted {
                trashed,
                went: Went::WorkspaceTrash,
            },
            json!({"trashed": "18a1f3c2d4-1", "went": "workspace"}),
        );
        wire(
            Restored {
                path: "Concepts/Lava.md".into(),
            },
            json!({"path": "Concepts/Lava.md"}),
        );
        wire(
            Folder {
                path: "Guide/Field Notes".into(),
            },
            json!({"path": "Guide/Field Notes"}),
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

    /// A core on `dir` whose system trash is a folder beside it, never the user's.
    fn core_with_bin(dir: &Path) -> (Core, PathBuf) {
        let bin = dir.with_extension("bin");
        let _ = std::fs::remove_dir_all(&bin);
        let mut core = Core::open(dir).unwrap();
        core.set_system_trash(Some(Bin::Freedesktop(bin.clone())));
        (core, bin)
    }

    /// The names in a folder, sorted; none when it isn't there.
    fn names(dir: &Path) -> Vec<String> {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return Vec::new();
        };
        let name = |e: std::fs::DirEntry| e.file_name().to_string_lossy().into_owned();
        let mut names: Vec<String> = entries.flatten().map(name).collect();
        names.sort();
        names
    }

    fn back(trashed: &TrashId, to: Option<&str>) -> Restore {
        Restore {
            trashed: trashed.clone(),
            to: to.map(String::from),
        }
    }

    #[test]
    fn a_delete_goes_to_the_trash_only_at_the_version_read_and_comes_back_byte_for_byte() {
        let dir = workspace("delete");
        let image = [0xff, 0xfe, 0x00, b'\n'];
        std::fs::write(dir.join("Data/Map.png"), image).unwrap();
        let (mut core, bin) = core_with_bin(&dir);
        let plan = dir.join("Plan.md");
        let on_disk = || std::fs::read_to_string(&plan).unwrap();
        let nothing = Vec::<String>::new();

        let seen = core.stat_file("Plan.md").unwrap();
        let read = core.read_file("Plan.md").unwrap();
        assert_eq!(seen.rev, read.rev, "the rev workspace/read gives");
        assert_eq!(seen.inside, None);
        let theirs = "edited in Obsidian meanwhile\n";
        std::fs::write(&plan, theirs).unwrap();
        let stale = core.delete_file("tidy", "Plan.md", &seen.rev).unwrap_err();
        assert_eq!(stale.reason, Reason::Changed);
        assert_eq!(on_disk(), theirs, "still there, as they wrote it");
        assert_eq!(names(&bin.join("files")), nothing, "nothing went");

        let seen = core.stat_file("Plan.md").unwrap();
        let gone = core.delete_file("tidy", "Plan.md", &seen.rev).unwrap();
        assert_eq!(gone.went, Went::System);
        assert!(!plan.exists());
        let trashed = std::fs::read_to_string(bin.join("files/Plan.md")).unwrap();
        assert_eq!(trashed, theirs, "in the trash this test named");
        assert!(
            !dir.join(".trash").exists(),
            "not the workspace's own trash"
        );
        let twice = core.delete_file("tidy", "Plan.md", &seen.rev).unwrap_err();
        assert_eq!(twice.reason, Reason::Changed, "asked twice: it is gone");
        assert_eq!(names(&bin.join("files")), ["Plan.md"]);

        let restore = back(&gone.trashed, None);
        assert_eq!(core.restore_file("tidy", &restore).unwrap().path, "Plan.md");
        assert_eq!(on_disk(), theirs);
        assert_eq!(names(&bin.join("files")), nothing);
        assert_eq!(names(&bin.join("info")), nothing);
        let spent = core.restore_file("tidy", &restore).unwrap_err();
        assert_eq!(spent.reason, Reason::Changed, "a token restores once");
        let other_run = back(&TrashId("0-1".into()), None);
        let unknown = core.restore_file("tidy", &other_run).unwrap_err();
        assert_eq!(unknown.reason, Reason::Changed);

        let unread = core.read_file("Data/Map.png").unwrap_err();
        assert_eq!(unread.reason, Reason::Refused, "not text");
        let seen = core.stat_file("Data/Map.png").unwrap();
        let gone = core.delete_file("tidy", "Data/Map.png", &seen.rev).unwrap();
        assert!(!dir.join("Data/Map.png").exists());
        core.restore_file("tidy", &back(&gone.trashed, None))
            .unwrap();
        assert_eq!(std::fs::read(dir.join("Data/Map.png")).unwrap(), image);
        let _ = std::fs::remove_dir_all(&bin);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_restore_never_overwrites_and_goes_where_it_is_told() {
        let dir = workspace("restore");
        let local = r#"{"trashOption": "local"}"#;
        std::fs::write(dir.join(".obsidian/app.json"), local).unwrap();
        let (mut core, bin) = core_with_bin(&dir);
        let csv = "Data/Eruptions.csv";
        let was = std::fs::read(dir.join(csv)).unwrap();
        let seen = core.stat_file(csv).unwrap();
        let gone = core.delete_file("tidy", csv, &seen.rev).unwrap();
        assert_eq!(gone.went, Went::WorkspaceTrash);
        assert_eq!(
            std::fs::read(dir.join(".trash/Eruptions.csv")).unwrap(),
            was
        );
        assert!(!bin.exists(), "the system trash is untouched");

        std::fs::write(dir.join(csv), "a new table\n").unwrap();
        let taken = core
            .restore_file("tidy", &back(&gone.trashed, None))
            .unwrap_err();
        assert_eq!(taken.reason, Reason::Exists);
        for outside in ["../Eruptions.csv", ".obsidian/Eruptions.csv"] {
            let there = back(&gone.trashed, Some(outside));
            let refused = core.restore_file("tidy", &there).unwrap_err();
            assert_eq!(refused.reason, Reason::Refused, "{outside}");
        }
        let free = back(&gone.trashed, Some("Data/Eruptions 1.csv"));
        let restored = core.restore_file("tidy", &free).unwrap();
        assert_eq!(restored.path, "Data/Eruptions 1.csv");
        assert_eq!(
            std::fs::read(dir.join("Data/Eruptions 1.csv")).unwrap(),
            was
        );
        let now = std::fs::read_to_string(dir.join(csv)).unwrap();
        assert_eq!(now, "a new table\n", "what took its place is untouched");
        assert_eq!(names(&dir.join(".trash")), Vec::<String>::new());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_folder_is_deleted_only_as_it_was_listed_and_comes_back_whole() {
        let dir = workspace("folder");
        std::fs::create_dir_all(dir.join("Data/Old")).unwrap();
        std::fs::write(dir.join("Data/Old/Ash.md"), "grey\n").unwrap();
        let (mut core, bin) = core_with_bin(&dir);
        let seen = core.stat_file("Data").unwrap();
        let listed = ["Data/Eruptions.csv", "Data/Old/", "Data/Old/Ash.md"];
        assert_eq!(seen.inside.unwrap(), listed);

        std::fs::write(dir.join("Data/Old/New.md"), "added since the listing\n").unwrap();
        let stale = core.delete_file("tidy", "Data", &seen.rev).unwrap_err();
        assert_eq!(stale.reason, Reason::Changed);
        assert!(dir.join("Data/Old/New.md").exists(), "nothing went");
        assert!(!bin.exists(), "nothing went");

        let seen = core.stat_file("Data").unwrap();
        let gone = core.delete_file("tidy", "Data", &seen.rev).unwrap();
        assert!(!dir.join("Data").exists());
        assert!(bin.join("files/Data/Old/New.md").is_file());
        let restored = core
            .restore_file("tidy", &back(&gone.trashed, None))
            .unwrap();
        assert_eq!(restored.path, "Data");
        let read = |rel: &str| std::fs::read_to_string(dir.join(rel)).unwrap();
        assert_eq!(read("Data/Old/Ash.md"), "grey\n");
        assert_eq!(read("Data/Eruptions.csv"), "name,year\nKrakatoa,1883\n");
        let again = core.stat_file("Data").unwrap();
        assert_eq!(again.rev, seen.rev, "as it was listed");
        let _ = std::fs::remove_dir_all(&bin);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn deletes_and_new_folders_stay_inside_the_workspace() {
        let dir = workspace("inside");
        let outside = dir.with_extension("outside");
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(outside.join("secret.md"), "x").unwrap();
        let (mut core, bin) = core_with_bin(&dir);
        let absolute = outside.join("secret.md").to_string_lossy().into_owned();
        let folder = outside.file_name().unwrap().to_string_lossy().into_owned();
        let up = format!("../{folder}/secret.md");
        let mut bad: Vec<&str> = vec!["", ".obsidian", ".obsidian/app.json"];
        bad.extend([up.as_str(), absolute.as_str()]);
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(&outside, dir.join("Link")).unwrap();
            bad.push("Link/secret.md");
        }
        for path in bad {
            let stat = core.stat_file(path).unwrap_err();
            assert_eq!(stat.reason, Reason::Refused, "stat {path}");
            let delete = core.delete_file("tidy", path, "").unwrap_err();
            assert_eq!(delete.reason, Reason::Refused, "delete {path}");
        }
        assert!(outside.join("secret.md").exists());
        assert!(dir.join(".obsidian").exists());
        assert!(!bin.exists(), "nothing went");

        let unfit = [
            "../Folder",
            ".hidden",
            "Data/.hidden",
            "Data//x",
            "a#b",
            "Data/[x]",
        ];
        for name in unfit {
            let refused = core.create_folder("tidy", name).unwrap_err();
            assert_eq!(refused.reason, Reason::Refused, "{name}");
        }
        let taken = core.create_folder("tidy", "Data").unwrap_err();
        assert_eq!(taken.reason, Reason::Exists);
        let a_file = core.create_folder("tidy", "Plan.md").unwrap_err();
        assert_eq!(a_file.reason, Reason::Exists, "a file is there");
        let made = core.create_folder("tidy", "Guide/Field Notes").unwrap();
        assert_eq!(made.path, "Guide/Field Notes");
        assert!(dir.join("Guide/Field Notes").is_dir());
        let _ = std::fs::remove_dir_all(&outside);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_core_keeps_each_clients_last_deletes() {
        let dir = workspace("kept");
        let local = r#"{"trashOption": "local"}"#;
        std::fs::write(dir.join(".obsidian/app.json"), local).unwrap();
        let mut core = Core::open(&dir).unwrap();
        let mut delete = |by: &str, name: &str| {
            std::fs::write(dir.join(name), name).unwrap();
            let rev = core.stat_file(name).unwrap().rev;
            core.delete_file(by, name, &rev).unwrap().trashed
        };
        let menu = delete("menu", "Kept.md");
        let first = delete("flood", "0.md");
        for n in 1..=KEPT {
            delete("flood", &format!("{n}.md"));
        }
        let forgotten = core.restore_file("flood", &back(&first, None)).unwrap_err();
        assert_eq!(forgotten.reason, Reason::Changed);
        assert!(dir.join(".trash/0.md").exists(), "still in the trash");
        let kept = core.restore_file("menu", &back(&menu, None)).unwrap();
        assert_eq!(
            kept.path, "Kept.md",
            "another client's deletes don't push it out"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A token is the client's it was given to, and says nothing of anyone else's.
    #[test]
    fn a_token_restores_only_for_its_client_and_cant_be_guessed_from_another() {
        let dir = workspace("tokens");
        let local = r#"{"trashOption": "local"}"#;
        std::fs::write(dir.join(".obsidian/app.json"), local).unwrap();
        std::fs::write(dir.join("Theirs.md"), "the menu's\n").unwrap();
        let mut core = Core::open(&dir).unwrap();
        let plan = core.stat_file("Plan.md").unwrap();
        let theirs = core.stat_file("Theirs.md").unwrap();
        let menus = core.delete_file("menu", "Theirs.md", &theirs.rev).unwrap();
        let mine = core.delete_file("tidy", "Plan.md", &plan.rev).unwrap();

        let (a, b) = (&menus.trashed.0, &mine.trashed.0);
        assert!(a.len() >= 32 && b.len() >= 32, "{a} and {b} are short");
        assert_ne!(
            a[..8],
            b[..8],
            "{a} and {b} share what one could count on from"
        );

        for to in [None, Some("Mine.md")] {
            let stolen = core
                .restore_file("tidy", &back(&menus.trashed, to))
                .unwrap_err();
            assert_eq!(stolen.reason, Reason::Changed, "as an unknown token is");
        }
        assert!(!dir.join("Theirs.md").exists() && !dir.join("Mine.md").exists());
        let kept = std::fs::read_to_string(dir.join(".trash/Theirs.md")).unwrap();
        assert_eq!(kept, "the menu's\n", "still in the trash");
        let restore = back(&menus.trashed, None);
        let restored = core.restore_file("menu", &restore).unwrap();
        assert_eq!(restored.path, "Theirs.md", "its own client still can");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The delete goes to the link, never through it.
    #[cfg(unix)]
    #[test]
    fn a_symlink_is_deleted_as_the_link_it_is_and_never_followed() {
        let dir = workspace("links");
        let outside = dir.with_extension("outside");
        let _ = std::fs::remove_dir_all(&outside);
        std::fs::create_dir_all(outside.join("Shared")).unwrap();
        std::fs::write(outside.join("secret.md"), "theirs\n").unwrap();
        std::fs::write(outside.join("Shared/Deep.md"), "deep\n").unwrap();
        let link = std::os::unix::fs::symlink::<PathBuf, PathBuf>;
        link(outside.join("secret.md"), dir.join("Data/Linked.md")).unwrap();
        link(outside.join("Shared"), dir.join("Shared")).unwrap();
        link(outside.join("gone.md"), dir.join("Data/Dangling.md")).unwrap();
        let (mut core, bin) = core_with_bin(&dir);
        let links = [
            ("Data/Linked.md", "secret.md"),
            ("Shared", "Shared"),
            ("Data/Dangling.md", "gone.md"),
        ];
        for (rel, target) in links {
            let target = outside.join(target);
            let seen = core.stat_file(rel).unwrap();
            assert_eq!(seen.inside, None, "{rel}: what it leads to is not listed");
            let gone = core.delete_file("tidy", rel, &seen.rev).unwrap();
            assert!(dir.join(rel).symlink_metadata().is_err(), "{rel} went");
            let name = Path::new(rel).file_name().unwrap();
            let trashed = std::fs::read_link(bin.join("files").join(name)).unwrap();
            assert_eq!(trashed, target, "{rel}: the link itself is in the trash");
            core.restore_file("tidy", &back(&gone.trashed, None))
                .unwrap();
            let restored = std::fs::read_link(dir.join(rel)).unwrap();
            assert_eq!(restored, target, "{rel}: the link is back");
        }
        let read = |rel: &str| std::fs::read_to_string(outside.join(rel)).unwrap();
        assert_eq!(read("secret.md"), "theirs\n", "the target is untouched");
        assert_eq!(read("Shared/Deep.md"), "deep\n", "the target is untouched");

        let through = core.stat_file("Shared/Deep.md").unwrap_err();
        assert_eq!(through.reason, Reason::Refused, "not through a link");
        let through = core.delete_file("tidy", "Shared/Deep.md", "").unwrap_err();
        assert_eq!(through.reason, Reason::Refused, "not through a link");
        let text = core.read_file("Data/Linked.md").unwrap_err();
        assert_eq!(
            text.reason,
            Reason::Refused,
            "reading still follows, and refuses"
        );
        let _ = std::fs::remove_dir_all(&bin);
        let _ = std::fs::remove_dir_all(&outside);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_restore_somewhere_else_follows_the_rules_of_a_new_name() {
        let dir = workspace("restore-to");
        let local = r#"{"trashOption": "local"}"#;
        std::fs::write(dir.join(".obsidian/app.json"), local).unwrap();
        std::fs::create_dir_all(dir.join("Data/[old]")).unwrap();
        std::fs::write(dir.join("Data/[old]/Table #1.csv"), "a\n").unwrap();
        let mut core = Core::open(&dir).unwrap();
        let odd = "Data/[old]/Table #1.csv";
        let seen = core.stat_file(odd).unwrap();
        let gone = core.delete_file("tidy", odd, &seen.rev).unwrap();
        for unfit in ["Data/a#b.csv", "Data/[x]/Table.csv", "Data/Table^.csv"] {
            let there = back(&gone.trashed, Some(unfit));
            let refused = core.restore_file("tidy", &there).unwrap_err();
            assert_eq!(refused.reason, Reason::Refused, "{unfit}");
            assert!(!dir.join(unfit).exists(), "{unfit}");
        }
        let home = core.restore_file("tidy", &back(&gone.trashed, None));
        assert_eq!(home.unwrap().path, odd, "where it was needs no new name");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_folders_listing_leaves_out_hidden_entries_and_is_bounded() {
        let dir = workspace("listing");
        std::fs::create_dir_all(dir.join("Data/.git/objects")).unwrap();
        std::fs::write(dir.join("Data/.git/objects/1f"), "x").unwrap();
        std::fs::write(dir.join("Data/.DS_Store"), "x").unwrap();
        let core = Core::open(&dir).unwrap();
        let stat = |rel: &str| serde_json::to_value(core.stat_file(rel).unwrap()).unwrap();
        let data = stat("Data");
        assert_eq!(data["inside"], json!(["Data/Eruptions.csv"]));
        assert_eq!(data["hidden"], 4, ".DS_Store, .git, objects and 1f: {data}");
        assert_eq!(data.get("more"), None, "{data}");
        std::fs::write(dir.join("Data/.git/objects/1f"), "more than before").unwrap();
        assert_ne!(stat("Data")["rev"], data["rev"], "the version covers them");

        std::fs::create_dir_all(dir.join("Many")).unwrap();
        for n in 0..1005 {
            std::fs::write(dir.join(format!("Many/{n:04}.md")), "").unwrap();
        }
        let many = stat("Many");
        let listed = many["inside"].as_array().unwrap();
        assert_eq!(listed.len(), 1000);
        assert_eq!(listed[999], "Many/0999.md");
        assert_eq!(many["more"], 5, "the ones not listed are counted");
        assert_eq!(many.get("hidden"), None);
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
