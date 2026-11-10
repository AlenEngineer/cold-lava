//! Scans a workspace (an Obsidian vault or any folder of notes): finds notes and extracts their
//! links and tags.

use crate::items::{self, Item};
use crate::owner::markdown;
use anyhow::{Context, Result};
use std::borrow::Cow;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use walkdir::WalkDir;

/// Text normalised for searching: lowercase, without accents or Hawaiian ʻokina,
/// so "kil" finds "Kīlauea" and "cafe" finds "Café".
pub fn search_key(s: &str) -> String {
    use unicode_normalization::UnicodeNormalization;
    s.nfd()
        .filter(|c| {
            !unicode_normalization::char::is_combining_mark(*c) && !matches!(c, 'ʻ' | 'ʼ' | '‘')
        })
        .collect::<String>()
        .to_lowercase()
}

pub struct Note {
    /// Path relative to the workspace root, without `.md`, using `/` separators.
    pub rel: String,
    /// File stem, shown as the node label.
    pub name: String,
    pub path: PathBuf,
    /// Raw link targets (already stripped of `#heading`, `|alias`, `.md`).
    pub links: Vec<String>,
    /// The same links with where they are: for backlinks.
    pub refs: Vec<Ref>,
    pub tags: Vec<String>,
    /// Frontmatter `aliases:` (other names the quick switcher finds the note by).
    pub aliases: Vec<String>,
    /// The note's full text, for full-text search.
    pub text: String,
    /// How its links are written, counted as they are read.
    pub links_written: crate::style::Links,
    /// How it is written, for the workspace's style: counted the first time it is asked for
    /// (`Workspace::style`), so a scan that writes nothing doesn't pay for it.
    pub style: OnceLock<crate::style::Counts>,
    /// Its items, read on first use (`Workspace::items`). A refreshed note is a new `Note`.
    pub items: OnceLock<Vec<Item>>,
}

/// One link in a note, or in a file another owner reads.
#[derive(Clone, Debug, PartialEq)]
pub struct Ref {
    pub target: String,
    /// 1-based line number: in the note, or in an owner's Markdown piece.
    pub line: usize,
    /// The line it's on, trimmed (for showing backlinks in context).
    pub context: String,
}

/// A file of a kind an extension owns, with the items and links its owner gave
/// (docs/extensions.md, "Owners and types").
pub struct Owned {
    /// Workspace-relative, with its extension.
    pub rel: String,
    pub path: PathBuf,
    /// The extension that owns its kind.
    pub owner: String,
    /// The version its items and links were read from; `None` until the owner answers.
    pub rev: Option<String>,
    pub items: Vec<Item>,
    /// The owner's place for each item (a row's key, a card's id), by id: `file/open` takes it.
    pub places: HashMap<String, String>,
    /// Its links, each with the owner's place for it.
    pub links: Vec<(Ref, String)>,
    /// What the owner gave for `rev`: its Markdown pieces' texts, typed items' fields and links,
    /// which an edit or a rename changes and asks the owner to write (`file/edit`).
    pub given: crate::owner::given::Given,
}

pub struct Workspace {
    pub root: PathBuf,
    pub name: String,
    pub notes: Vec<Note>,
    /// Files other owners read, by `rel`. The core keeps them (`Core::set_owners`).
    pub owned: Vec<Owned>,
    /// What items depend on (flavour, Tasks and daily-note settings), read on first use.
    pub item_settings: OnceLock<items::Settings>,
}

/// The files besides notes that links lead to, by extension, with what each is.
const ATTACHMENTS: &[(&str, &[&str])] = &[
    (
        "image",
        &["png", "jpg", "jpeg", "gif", "svg", "webp", "bmp", "avif"],
    ),
    ("audio", &["mp3", "wav", "m4a", "ogg", "flac", "3gp"]),
    ("video", &["mp4", "webm", "ogv", "mov", "mkv"]),
    ("document", &["pdf"]),
    ("whiteboard", &["canvas"]),
    ("database", &["base"]),
];

/// What kind of file a link target names when it isn't a note (`photo.jpg` → `image`).
/// `Mount St. Helens` is a note: only the extensions above make an attachment.
pub fn attachment_kind(target: &str) -> Option<&'static str> {
    let file = target.rsplit('/').next().unwrap_or(target);
    let ext = file.rsplit_once('.')?.1.to_ascii_lowercase();
    ATTACHMENTS
        .iter()
        .find(|(_, exts)| exts.contains(&ext.as_str()))
        .map(|(kind, _)| *kind)
}

/// Is this workspace-relative path a note cold-lava reads (a `.md` file outside hidden folders)?
pub fn is_note_path(rel: &Path) -> bool {
    rel.extension().is_some_and(|e| e == "md")
        && !rel.components().any(|c| {
            let c = c.as_os_str().to_string_lossy();
            c.starts_with('.') || c == "node_modules"
        })
}

/// Read and parse one note. `None` if it isn't a note, is outside the workspace, or can't be read.
pub fn read_note(root: &Path, path: &Path) -> Option<Note> {
    let rel_path = path.strip_prefix(root).ok()?;
    if !is_note_path(rel_path) {
        return None;
    }
    let text = fs::read_to_string(path).ok()?;
    let rel = rel_path
        .with_extension("")
        .to_string_lossy()
        .replace('\\', "/");
    let name = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let markdown::Read {
        refs,
        tags,
        aliases,
        written,
    } = markdown::read(&text);
    let links = refs.iter().map(|r| r.target.clone()).collect();
    Some(Note {
        rel,
        name,
        path: path.to_path_buf(),
        links,
        refs,
        tags,
        aliases,
        links_written: written,
        style: OnceLock::new(),
        text,
        items: OnceLock::new(),
    })
}

/// The notes under `root`, outside hidden folders, in no particular order.
pub fn note_paths(root: &Path) -> impl Iterator<Item = PathBuf> + '_ {
    let walker = WalkDir::new(root).into_iter().filter_entry(|e| {
        let n = e.file_name().to_string_lossy();
        // Skip .obsidian, .trash, .git and friends.
        e.depth() == 0 || !(n.starts_with('.') || n == "node_modules")
    });
    walker
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
        .filter(move |e| e.path().strip_prefix(root).is_ok_and(is_note_path))
        .map(|e| e.into_path())
}

pub fn scan(root: &Path) -> Result<Workspace> {
    let root = root
        .canonicalize()
        .with_context(|| format!("workspace path not found: {}", root.display()))?;
    let name = root
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "workspace".into());
    let mut notes: Vec<Note> = note_paths(&root)
        .filter_map(|p| read_note(&root, &p))
        .collect();
    notes.sort_by(|a, b| a.rel.cmp(&b.rel));
    Ok(Workspace {
        root,
        name,
        notes,
        owned: Vec::new(),
        item_settings: OnceLock::new(),
    })
}

/// What happened to a note when the workspace was told a path changed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NoteChange {
    Created(String),
    Changed(String),
    Removed(String),
}

impl Workspace {
    pub fn item_settings(&self) -> &items::Settings {
        self.item_settings
            .get_or_init(|| items::Settings::load(&self.root))
    }

    /// How the workspace is written: its notes' majority, then Obsidian's settings and the
    /// overrides (`style::Style::of`). Added up again on each call, so it follows the notes; a
    /// note is counted once, the first time.
    pub fn style(&self) -> crate::style::Style {
        let counts = (self.notes.iter())
            .map(|n| (n.style).get_or_init(|| crate::style::count_with(&n.text, n.links_written)));
        crate::style::Style::of(&self.root, counts)
    }

    /// The note's items, read the first time they're asked for.
    pub fn items<'a>(&self, note: &'a Note) -> &'a [Item] {
        note.items.get_or_init(|| self.read_items(note))
    }

    /// The note's items for one look: kept ones if they were read, else read and not kept, for
    /// a pass over every note that won't ask again (a large workspace's items are most of its
    /// memory).
    pub fn items_once<'a>(&self, note: &'a Note) -> Cow<'a, [Item]> {
        match note.items.get() {
            Some(items) => Cow::Borrowed(items),
            None => Cow::Owned(self.read_items(note)),
        }
    }

    fn read_items(&self, note: &Note) -> Vec<Item> {
        let rel = format!("{}.md", note.rel);
        markdown::items(&rel, &note.text, self.item_settings())
    }

    /// `path` spelled from the canonical root, as the notes' paths are. The root was
    /// canonicalized when scanned, so a path through a symlink (macOS's `/var` is
    /// `/private/var`) would otherwise match no note and be outside the workspace. A deleted
    /// file can't be canonicalized, so its folder is.
    fn under_root(&self, path: &Path) -> PathBuf {
        if path.starts_with(&self.root) {
            return path.to_path_buf();
        }
        if let Ok(p) = path.canonicalize() {
            return p;
        }
        match (path.parent().map(Path::canonicalize), path.file_name()) {
            (Some(Ok(dir)), Some(name)) => dir.join(name),
            _ => path.to_path_buf(),
        }
    }

    /// Bring one path up to date with the disk: read it again, add it, or drop it.
    /// Returns what changed, if anything did (rel paths without `.md`).
    pub fn refresh_path(&mut self, path: &Path) -> Option<NoteChange> {
        let path = &self.under_root(path);
        let pos = self.notes.iter().position(|n| n.path == *path);
        match (read_note(&self.root, path), pos) {
            (Some(note), Some(i)) => {
                if self.notes[i].text == note.text {
                    return None;
                }
                let rel = note.rel.clone();
                self.notes[i] = note;
                Some(NoteChange::Changed(rel))
            }
            (Some(note), None) => {
                let rel = note.rel.clone();
                let at = self.notes.partition_point(|n| n.rel < note.rel);
                self.notes.insert(at, note);
                Some(NoteChange::Created(rel))
            }
            (None, Some(i)) => {
                // Still on disk but unreadable (e.g. mid-write): keep the last good version.
                if path.exists() {
                    return None;
                }
                Some(NoteChange::Removed(self.notes.remove(i).rel))
            }
            (None, None) => None,
        }
    }
}

/// A link target as the file it names: `%XX` escapes decoded as UTF-8, the way Obsidian writes
/// spaces and non-ASCII names in Markdown links (`P%C4%81hoehoe.md` is `Pāhoehoe.md`). A `%`
/// not followed by two hex digits, or escapes that aren't valid UTF-8, stay as written.
pub fn decode_target(t: &str) -> String {
    let b = t.as_bytes();
    let hex = |i: usize| {
        b.get(i..i + 3)
            .filter(|e| e[0] == b'%' && e[1].is_ascii_hexdigit() && e[2].is_ascii_hexdigit())
            .and_then(|e| u8::from_str_radix(std::str::from_utf8(&e[1..]).ok()?, 16).ok())
    };
    let mut out = String::with_capacity(t.len());
    let mut i = 0;
    while i < b.len() {
        let start = i;
        let mut bytes = Vec::new();
        while let Some(byte) = hex(i) {
            bytes.push(byte);
            i += 3;
        }
        if bytes.is_empty() {
            let c = t[i..].chars().next().unwrap_or_default();
            out.push(c);
            i += c.len_utf8();
        } else {
            match String::from_utf8(bytes) {
                Ok(s) => out.push_str(&s),
                Err(_) => out.push_str(&t[start..i]),
            }
        }
    }
    out
}

/// `path` written as a Markdown link target in the style of `original`: spaces always as
/// `%20`; non-ASCII characters (and `%`) as UTF-8 escapes only if `original` wrote one that way.
pub fn encode_target_like(path: &str, original: &str) -> String {
    let escapes_non_ascii = original.is_ascii() && !decode_target(original).is_ascii();
    let mut out = String::with_capacity(path.len());
    for c in path.chars() {
        match c {
            ' ' => out.push_str("%20"),
            '%' if escapes_non_ascii => out.push_str("%25"),
            c if escapes_non_ascii && !c.is_ascii() => {
                let mut buf = [0; 4];
                for byte in c.encode_utf8(&mut buf).bytes() {
                    out.push_str(&format!("%{byte:02X}"));
                }
            }
            c => out.push(c),
        }
    }
    out
}

pub(crate) fn clean_target(t: &str) -> Option<String> {
    let t = decode_target(t.trim());
    let t = t.strip_suffix(".md").unwrap_or(&t).trim().to_string();
    if t.is_empty() || attachment_kind(&t).is_some() || t.contains("://") {
        None
    } else {
        Some(t)
    }
}

/// How [`locate_workspace`] found the workspace a path is in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Found {
    /// A folder above it has `.obsidian` or `.cold-lava`.
    Marked,
    /// It is inside a workspace the user has opened or Obsidian lists.
    Known,
    /// Neither: its own folder was taken. Ids from `items` on another folder won't match there,
    /// so a caller says so when something isn't found.
    Guessed,
}

/// The workspace a path is in: the nearest folder above it (or itself) holding `.obsidian` or
/// `.cold-lava`; else the deepest of `known` (the user's list of workspaces) that holds it;
/// else the path's own folder.
pub fn locate_workspace(p: &Path, known: &[PathBuf]) -> (PathBuf, Found) {
    let abs = p.canonicalize().unwrap_or_else(|_| p.to_path_buf());
    let start = if abs.is_dir() {
        abs.clone()
    } else {
        abs.parent().map(Path::to_path_buf).unwrap_or(abs.clone())
    };
    let marked = |a: &Path| a.join(".obsidian").is_dir() || a.join(".cold-lava").is_dir();
    if let Some(a) = start.ancestors().find(|a| marked(a)) {
        return (a.to_path_buf(), Found::Marked);
    }
    let known = known
        .iter()
        .filter_map(|k| k.canonicalize().ok())
        .filter(|k| start.starts_with(k))
        .max_by_key(|k| k.components().count());
    match known {
        Some(k) => (k, Found::Known),
        None => (start, Found::Guessed),
    }
}

/// The workspace a path is in, as [`locate_workspace`] finds it without a list of workspaces.
pub fn find_workspace_root(p: &Path) -> PathBuf {
    locate_workspace(p, &[]).0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_workspace_is_found_by_its_marker_then_the_list_then_guessed() {
        let dir = std::env::temp_dir().join(format!("locate-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let note = |ws: &str| {
            let n = dir.join(ws).join("Notes/A.md");
            std::fs::create_dir_all(n.parent().unwrap()).unwrap();
            std::fs::write(&n, "x").unwrap();
            n
        };
        let canon = |p: PathBuf| p.canonicalize().unwrap();
        let obsidian = note("obsidian");
        std::fs::create_dir_all(dir.join("obsidian/.obsidian")).unwrap();
        let ours = note("ours");
        std::fs::create_dir_all(dir.join("ours/.cold-lava")).unwrap();
        let listed = note("listed");
        let loose = note("loose");
        let known = [dir.join("listed"), dir.join("elsewhere")];
        assert_eq!(
            locate_workspace(&obsidian, &known),
            (canon(dir.join("obsidian")), Found::Marked)
        );
        assert_eq!(
            locate_workspace(&ours, &known),
            (canon(dir.join("ours")), Found::Marked)
        );
        assert_eq!(
            locate_workspace(&listed, &known),
            (canon(dir.join("listed")), Found::Known)
        );
        assert_eq!(
            locate_workspace(&loose, &known),
            (canon(dir.join("loose/Notes")), Found::Guessed)
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn attachments_are_known_file_types_and_dotted_note_names_are_notes() {
        assert_eq!(attachment_kind("kilauea-lava-lake.jpg"), Some("image"));
        assert_eq!(attachment_kind("Maps/Hawaii.PDF"), Some("document"));
        assert_eq!(attachment_kind("Welcome.canvas"), Some("whiteboard"));
        assert_eq!(attachment_kind("Mount St. Helens"), None);
        assert_eq!(attachment_kind("David A. Johnston"), None);
        assert_eq!(attachment_kind("v1.2/notes"), None);
        assert_eq!(attachment_kind("Lahar"), None);
    }

    #[test]
    fn percent_decoding() {
        assert_eq!(decode_target("P%C4%81hoehoe.md"), "Pāhoehoe.md");
        assert_eq!(decode_target("%E2%9C%93%20done%f0%9f%8c%8b"), "✓ done🌋");
        assert_eq!(decode_target("100%25.md"), "100%.md");
        assert_eq!(decode_target("50% off%2"), "50% off%2", "not an escape");
        assert_eq!(decode_target("%zz%C4"), "%zz%C4", "not UTF-8");
        assert_eq!(decode_target("Kīlauea"), "Kīlauea");
        assert_eq!(
            encode_target_like("ʻAʻā 100%.md", "P%C4%81hoehoe.md"),
            "%CA%BBA%CA%BB%C4%81%20100%25.md"
        );
        assert_eq!(
            encode_target_like("ʻAʻā 100%.md", "Hot%20Rock.md"),
            "ʻAʻā%20100%.md"
        );
        assert_eq!(
            encode_target_like("ʻAʻā.md", "Pāhoehoe.md"),
            "ʻAʻā.md",
            "written unescaped"
        );
    }

    #[test]
    fn items_are_read_on_first_use_and_again_after_a_change() {
        let dir = std::env::temp_dir().join(format!("cold-lava-items-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("Todo.md"), "- [ ] one\n").unwrap();
        let mut v = scan(&dir).unwrap();
        assert!(
            v.item_settings.get().is_none() && v.notes[0].items.get().is_none(),
            "scanning reads no items"
        );
        assert_eq!(v.items(&v.notes[0]).len(), 1);
        assert!(v.notes[0].items.get().is_some(), "cached");
        fs::write(dir.join("Todo.md"), "- [ ] one\n- [ ] two\n").unwrap();
        let path = v.notes[0].path.clone();
        v.refresh_path(&path);
        assert_eq!(v.items(&v.notes[0]).len(), 2);
        let _ = fs::remove_dir_all(&dir);
    }

    /// The root is canonical, so a path through a symlink to it (as macOS's temp folder is)
    /// still names the note: changed, created and deleted.
    #[cfg(unix)]
    #[test]
    fn refreshes_a_path_given_through_a_symlink() {
        let base = std::env::temp_dir().join(format!("cold-lava-link-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        let real = base.join("real");
        fs::create_dir_all(&real).unwrap();
        fs::write(real.join("A.md"), "one").unwrap();
        let link = base.join("link");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        let mut v = scan(&link).unwrap();
        fs::write(link.join("A.md"), "two").unwrap();
        assert_eq!(
            v.refresh_path(&link.join("A.md")),
            Some(NoteChange::Changed("A".into()))
        );
        assert_eq!(v.notes[0].text, "two");
        fs::write(link.join("B.md"), "new").unwrap();
        assert_eq!(
            v.refresh_path(&link.join("B.md")),
            Some(NoteChange::Created("B".into()))
        );
        fs::remove_file(link.join("B.md")).unwrap();
        assert_eq!(
            v.refresh_path(&link.join("B.md")),
            Some(NoteChange::Removed("B".into()))
        );
        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn scans_example_workspace() {
        let v =
            scan(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../../example-workspace")).unwrap();
        assert_eq!(v.name, "example-workspace");
        assert_eq!(v.notes.len(), 35);
        // Hidden folders (.obsidian) are skipped; subfolders and non-ASCII names are included.
        assert!(v.notes.iter().all(|n| !n.rel.starts_with('.')));
        let kilauea = v
            .notes
            .iter()
            .find(|n| n.rel == "Volcanoes/Kīlauea")
            .unwrap();
        assert_eq!(kilauea.tags, ["volcano/shield"]);
        assert!(
            !kilauea.links.iter().any(|l| l.ends_with(".jpg")),
            "attachments are not links"
        );
    }
}

#[cfg(test)]
mod search_tests {
    #[test]
    fn search_ignores_accents_and_case() {
        use super::search_key as k;
        assert!(k("Volcanoes/Kīlauea.md").contains(&k("kil")));
        assert!(k("Eyjafjallajökull").contains(&k("jokull")));
        assert!(k("ʻAʻā").contains("aa"));
        assert!(k("Café Notes").contains(&k("cafe")));
    }
}
