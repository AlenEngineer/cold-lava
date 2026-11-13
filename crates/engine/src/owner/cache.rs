//! Owners in other processes: who owns which kind of file, what each gave for each file by
//! version, and what is still to ask. Owners answer `file/read` later, so what they gave is kept
//! by `rev`, in memory and in a cache file per workspace, owner and owner version under
//! `~/.cache/cold-lava/owners`. A workspace then opens with every owned file's items and links
//! from the cache, and an owner is asked again only for a file whose `rev` changed.

use crate::owner::given::Given;
use crate::tasks::fnv;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

/// A kind of file and the extension that owns it.
#[derive(Clone, Debug, PartialEq)]
pub struct Kind {
    /// Lowercase, without the dot: "csv".
    pub extension: String,
    pub owner: String,
    /// The owner's version: what an older one gave isn't trusted.
    pub version: String,
}

/// `file/read` (host → owner): the items and links of the file at `path`, whose text is `text`
/// at version `rev`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FileRead {
    pub path: String,
    pub rev: String,
    pub text: String,
}

/// What one owner gave, by file, as kept on disk.
#[derive(Default, Serialize, Deserialize)]
struct Cache {
    owner: String,
    version: String,
    files: BTreeMap<String, Answer>,
    #[serde(skip)]
    dirty: bool,
}

#[derive(Clone, Serialize, Deserialize)]
struct Answer {
    rev: String,
    given: Given,
}

#[derive(Default)]
pub struct Owners {
    kinds: Vec<Kind>,
    /// One per owner, and where it is kept.
    caches: Vec<(Cache, Option<PathBuf>)>,
    /// Asked and not answered yet: file → rev.
    asked: HashMap<String, String>,
    /// To ask: the owner, and the request.
    wanted: Vec<(String, FileRead)>,
}

impl Owners {
    /// The owners of these kinds in the workspace at `root`, with what they gave before, from
    /// `dir` (`None`: kept in memory only).
    pub fn new(root: &Path, kinds: Vec<Kind>, dir: Option<&Path>) -> Owners {
        let mut caches: Vec<(Cache, Option<PathBuf>)> = Vec::new();
        for k in &kinds {
            if caches.iter().any(|(c, _)| c.owner == k.owner) {
                continue;
            }
            let path = dir.map(|d| d.join(file_name(root, k)));
            let cache = (path.as_deref())
                .and_then(|p| std::fs::read_to_string(p).ok())
                .and_then(|t| serde_json::from_str::<Cache>(&t).ok())
                .filter(|c| c.owner == k.owner && c.version == k.version)
                .unwrap_or_else(|| Cache {
                    owner: k.owner.clone(),
                    version: k.version.clone(),
                    ..Cache::default()
                });
            caches.push((cache, path));
        }
        Owners {
            kinds,
            caches,
            ..Owners::default()
        }
    }

    pub fn kinds(&self) -> &[Kind] {
        &self.kinds
    }

    /// The owner of the file at `rel`, by its extension: a built-in one (`owner::BUILT_IN`), or
    /// an extension.
    pub fn owner_of(&self, rel: &str) -> Option<&str> {
        if let Some(owner) = crate::owner::built_in(rel) {
            return Some(owner);
        }
        let x = crate::files::extension_of(rel)?;
        let k = self.kinds.iter().find(|k| k.extension == x)?;
        Some(&k.owner)
    }

    fn cache(&mut self, owner: &str) -> Option<&mut Cache> {
        (self.caches.iter_mut())
            .map(|(c, _)| c)
            .find(|c| c.owner == owner)
    }

    /// What the owner gave for the file at `rel` at version `rev`.
    pub fn given(&mut self, owner: &str, rel: &str, rev: &str) -> Option<&Given> {
        let a = self.cache(owner)?.files.get(rel)?;
        (a.rev == rev).then_some(&a.given)
    }

    /// Ask the owner for the file at `read.path`, unless it was asked for that version already.
    pub fn ask(&mut self, owner: &str, read: FileRead) {
        if self.asked.get(&read.path) == Some(&read.rev) {
            return;
        }
        self.asked.insert(read.path.clone(), read.rev.clone());
        self.wanted.retain(|(_, w)| w.path != read.path);
        self.wanted.push((owner.to_string(), read));
    }

    /// Whether a file waits for its owner to read it: to be asked, or asked and not answered.
    pub fn is_reading(&self) -> bool {
        !(self.asked.is_empty() && self.wanted.is_empty())
    }

    /// Whether there is something to ask.
    pub fn has_wanted(&self) -> bool {
        !self.wanted.is_empty()
    }

    /// What to ask now, each with its owner.
    pub fn take_wanted(&mut self) -> Vec<(String, FileRead)> {
        std::mem::take(&mut self.wanted)
    }

    /// The owner answered for `path` at `rev` (`None`: it couldn't): keep what it gave, and
    /// forget that it was asked, so a change asks again.
    pub fn answered(&mut self, owner: &str, path: &str, rev: &str, given: Option<&Given>) {
        if self.asked.get(path).is_some_and(|r| r == rev) {
            self.asked.remove(path);
        }
        let (Some(given), Some(cache)) = (given, self.cache(owner)) else {
            return;
        };
        let answer = Answer {
            rev: rev.to_string(),
            given: given.clone(),
        };
        cache.files.insert(path.to_string(), answer);
        cache.dirty = true;
    }

    /// The file at `rel` is gone.
    pub fn forget(&mut self, rel: &str) {
        self.asked.remove(rel);
        for (c, _) in &mut self.caches {
            c.dirty |= c.files.remove(rel).is_some();
        }
    }

    /// Write the caches that changed. A cache that can't be written is only slower next time.
    pub fn save(&mut self) {
        for (c, path) in &mut self.caches {
            if !std::mem::take(&mut c.dirty) {
                continue;
            }
            let Some(path) = path else { continue };
            if let Ok(json) = serde_json::to_vec(c) {
                let _ = crate::write::write_atomic(path, &json);
            }
        }
    }
}

/// The cache file for an owner of a workspace: the workspace by a hash of its path, the owner by
/// name and version.
fn file_name(root: &Path, k: &Kind) -> String {
    let safe = |s: &str| -> String {
        (s.chars())
            .map(|c| match c.is_ascii_alphanumeric() || "._-".contains(c) {
                true => c,
                false => '_',
            })
            .collect()
    };
    let root = fnv(root.to_string_lossy().as_bytes());
    format!("{root:016x}-{}-{}.json", safe(&k.owner), safe(&k.version))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::owner::given::GivenItem;

    fn kind() -> Kind {
        Kind {
            extension: "csv".into(),
            owner: "csv-table".into(),
            version: "0.1.0".into(),
        }
    }

    fn given(text: &str) -> Given {
        Given {
            items: vec![GivenItem::Markdown {
                place: "1".into(),
                text: text.into(),
            }]
            .into(),
            ..Given::default()
        }
    }

    #[test]
    fn what_an_owner_gave_is_kept_by_rev_and_by_owner_version() {
        let dir = std::env::temp_dir().join(format!("cl-owner-cache-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let root = Path::new("/some/workspace");
        let mut o = Owners::new(root, vec![kind()], Some(&dir));
        assert_eq!(o.owner_of("A/B.CSV"), Some("csv-table"));
        assert_eq!(o.owner_of("A/B.md"), None);
        let read = |rev: &str| FileRead {
            path: "B.csv".into(),
            rev: rev.into(),
            text: String::new(),
        };
        o.ask("csv-table", read("r1"));
        o.ask("csv-table", read("r1"));
        assert_eq!(o.take_wanted().len(), 1, "asked once per version");
        o.ask("csv-table", read("r1"));
        assert!(o.take_wanted().is_empty(), "still being asked");
        o.answered("csv-table", "B.csv", "r1", Some(&given("- [ ] one")));
        o.save();

        let mut again = Owners::new(root, vec![kind()], Some(&dir));
        assert_eq!(
            again.given("csv-table", "B.csv", "r1"),
            Some(&given("- [ ] one"))
        );
        assert_eq!(
            again.given("csv-table", "B.csv", "r2"),
            None,
            "another version"
        );
        let newer = Kind {
            version: "0.2.0".into(),
            ..kind()
        };
        let mut upgraded = Owners::new(root, vec![newer], Some(&dir));
        assert_eq!(
            upgraded.given("csv-table", "B.csv", "r1"),
            None,
            "another owner version"
        );
        let mut elsewhere = Owners::new(Path::new("/other"), vec![kind()], Some(&dir));
        assert_eq!(
            elsewhere.given("csv-table", "B.csv", "r1"),
            None,
            "another workspace"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
