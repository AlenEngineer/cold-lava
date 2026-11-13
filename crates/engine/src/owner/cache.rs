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

/// The format of the owners' cache. A cache this binary trusts was written by a binary
/// that keeps left-out records (`Answer`). The format is in the file's name, so a binary of
/// another format neither reads nor rewrites this one's cache, and in the file, where it must
/// match, so a cache of another format is not read and its owners are asked again. A change
/// to what a cache holds raises it: an upgrade or a downgrade then asks every owner once.
///
/// An edit beside an item left out (#257) changed nothing a cache holds, so it is still 2. The
/// answer kept after such an edit is the one the owner gave for the file's new text, read
/// as any answer is (left-out records included), and it was compared with the expected one
/// before anything was written: a binary of the same format that refuses those edits reads
/// the same answer it would have been given, and this binary trusts nothing from that binary
/// that it wouldn't have kept itself.
const FORMAT: u32 = 2;

/// What one owner gave, by file, as kept on disk.
#[derive(Default, Serialize, Deserialize)]
struct Cache {
    /// `FORMAT`; a cache written before formats were kept has none, and is not read.
    #[serde(default)]
    format: u32,
    owner: String,
    version: String,
    files: BTreeMap<String, Answer>,
    #[serde(skip)]
    dirty: bool,
}

/// What an owner gave for one version of a file. The items it left out live in one place,
/// `given.left_out`, and the cache file holds them beside the answer (`Stored`): what is
/// saved is what is read back, through any number of loads and saves.
#[derive(Clone, Serialize, Deserialize)]
#[serde(from = "Stored", into = "Stored")]
struct Answer {
    rev: String,
    given: Given,
    /// Whether the answer read back from the cache as it was saved. One whose items no longer
    /// all read (a bound applies now that didn't when it was saved) is dropped as it loads,
    /// and its owner asked again: the positions of what was left out then and what is left
    /// out now can't be put in one order without the owner's answer. So the file shows
    /// nothing, its good items included, until its owner answers; with its owner absent or
    /// broken, until it is back. The owner has not decided this rule (#298): the other way
    /// keeps showing the cached good items and reports the bad one.
    read_as_saved: bool,
}

/// An answer as the cache file holds it: the answer as its owner gives it, and the items it
/// left out (`Given::left_out`), which that JSON has no room for.
#[derive(Serialize, Deserialize)]
struct Stored {
    rev: String,
    given: Given,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    left_out: Vec<(usize, crate::owner::answer::LeftOut)>,
}

impl From<Stored> for Answer {
    fn from(stored: Stored) -> Answer {
        // Every field named, so one added to `Stored` says here how it is read back.
        let Stored {
            rev,
            mut given,
            left_out,
        } = stored;
        let read_as_saved = given.left_out.is_empty();
        given.left_out = left_out;
        Answer {
            rev,
            given,
            read_as_saved,
        }
    }
}

impl From<Answer> for Stored {
    fn from(answer: Answer) -> Stored {
        // Every field named, so one added to `Answer` says here whether it is saved.
        let Answer {
            rev,
            given,
            read_as_saved: _,
        } = answer;
        let left_out = given.left_out.clone();
        Stored {
            rev,
            given,
            left_out,
        }
    }
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
                .map(Cache::read_as_saved)
                .filter(|c| c.format == FORMAT && c.owner == k.owner && c.version == k.version)
                .unwrap_or_else(|| Cache {
                    format: FORMAT,
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
            read_as_saved: true,
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

impl Cache {
    /// It without the answers that didn't read back as they were saved (`Answer`).
    fn read_as_saved(mut self) -> Cache {
        let before = self.files.len();
        self.files.retain(|_, answer| answer.read_as_saved);
        self.dirty = self.files.len() != before;
        self
    }
}

/// The cache file for an owner of a workspace: the workspace by a hash of its path, the owner by
/// name and version, and the cache's `FORMAT`.
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
    format!(
        "{root:016x}-{}-{}.cache{FORMAT}.json",
        safe(&k.owner),
        safe(&k.version)
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::owner::given::GivenItem;
    use serde_json::json;

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

    /// What an owner gave that didn't read is kept with its answer through any number of
    /// loads and saves: saving the cache again (another file of the same owner changed)
    /// must not drop it, or the file's bad item stops being reported and an edit that
    /// drops it would be written.
    #[test]
    fn what_was_left_out_survives_every_load_and_save() {
        let dir = std::env::temp_dir().join(format!("cl-owner-left-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let root = Path::new("/some/workspace");
        let bad: Given = serde_json::from_value(json!({"items": [
            {"place": "1", "text": "- [ ] one"},
            {"place": "2", "role": "slide"},
        ]}))
        .unwrap();
        assert_eq!(bad.left_out.len(), 1);
        let mut o = Owners::new(root, vec![kind()], Some(&dir));
        o.answered("csv-table", "A.csv", "r1", Some(&bad));
        o.save();
        for round in 0..3 {
            let mut again = Owners::new(root, vec![kind()], Some(&dir));
            let kept = again.given("csv-table", "A.csv", "r1").cloned();
            assert_eq!(kept.as_ref(), Some(&bad), "load {round}");
            let other = format!("B{round}.csv");
            again.answered("csv-table", &other, "r1", Some(&given("- [ ] two")));
            again.save();
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A cache this binary trusts was written by a binary that keeps left-out records. One
    /// in the older format (no format of its own, no left-out records), at the name the
    /// older binary used or at this one's, is not read: its owner is asked again.
    #[test]
    fn a_cache_in_an_older_format_is_not_trusted() {
        let dir = std::env::temp_dir().join(format!("cl-owner-older-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let root = Path::new("/some/workspace");
        let older = json!({"owner": "csv-table", "version": "0.1.0", "files": {
            "A.csv": {"rev": "r1", "given": {"items": [{"place": "1", "text": "- [ ] one"}], "links": []}}
        }});
        let older_name = format!(
            "{:016x}-csv-table-0.1.0.json",
            fnv(root.to_string_lossy().as_bytes())
        );
        for name in [older_name, file_name(root, &kind())] {
            std::fs::write(dir.join(&name), older.to_string()).unwrap();
            let mut o = Owners::new(root, vec![kind()], Some(&dir));
            assert_eq!(o.given("csv-table", "A.csv", "r1"), None, "{name}");
            std::fs::remove_file(dir.join(&name)).unwrap();
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// What a cache of the current `FORMAT` holds, byte for byte, for an answer with a
    /// piece, a Markdown piece, a link to a file, a link between pieces and an item left out
    /// (`cache-format-<FORMAT>.json` beside this file). A change to what a cache holds fails
    /// here until `FORMAT` is raised, which keeps a binary from trusting another's cache.
    #[test]
    fn a_cache_of_this_format_holds_exactly_this() {
        let dir = std::env::temp_dir().join(format!("cl-owner-format-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let root = Path::new("/some/workspace");
        let answer: Given = serde_json::from_value(json!({
            "items": [
                {"place": "p", "role": "item", "attributes": {"task": true, "checkbox": "open"},
                 "text": ["a ", {"link": "Krakatoa"}]},
                {"place": "m", "text": "- [ ] markdown"},
                {"place": "b", "role": "slide"},
            ],
            "links": [
                {"target": "Krakatoa", "place": "m", "text": "around"},
                {"from": "p", "to": "p", "label": "self"},
            ],
        }))
        .unwrap();
        let mut o = Owners::new(root, vec![kind()], Some(&dir));
        o.answered("csv-table", "A.csv", "r1", Some(&answer));
        o.save();
        let saved = std::fs::read_to_string(dir.join(file_name(root, &kind()))).unwrap();
        let golden = format!(
            "{}/src/owner/cache-format-{FORMAT}.json",
            env!("CARGO_MANIFEST_DIR")
        );
        let golden = std::fs::read_to_string(&golden).unwrap_or_default();
        assert_eq!(
            saved,
            golden.trim_end(),
            "what a cache holds changed: raise `FORMAT`, then update this (cache-format-N.json)"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
