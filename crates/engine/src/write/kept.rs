//! What a write does with a text it took out of a file and can't put back: removed when it is
//! only the caller's or the write's own, kept beside the file under a name of its own
//! otherwise (`kept_name`), never removed. `replace` and `stale` both keep through it, and
//! `find` reads the name back (`kept_for`). A leaf but for `name::reserved` and the
//! `WriteError` it answers.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use super::WriteError;
use super::name::reserved;

/// The refusal for a text this write took out of `file` (where `path` lands) and holds in
/// `out`, a name of its own beside it that nothing else writes. When it is one of `droppable`
/// (this write's bytes, or what the caller read) nobody's text is lost: it is removed, and the
/// refusal is `Changed`. Otherwise it is another program's text: moved to a name that says it
/// is kept (`kept_name`) and named by `Kept`; if that move fails, it stays in `out`, which is
/// named instead.
pub(super) fn keep(path: &Path, file: &Path, out: &Path, droppable: &[&[u8]]) -> WriteError {
    let dropped = !droppable.is_empty()
        && std::fs::read(out).is_ok_and(|now| droppable.contains(&now.as_slice()));
    if dropped {
        let _ = std::fs::remove_file(out);
        return WriteError::Changed(path.to_path_buf());
    }
    let kept = match reserved(file, kept_name) {
        Ok(kept) => match std::fs::rename(out, &kept) {
            Ok(()) => kept,
            Err(_) => {
                let _ = std::fs::remove_file(&kept);
                out.to_path_buf()
            }
        },
        Err(_) => out.to_path_buf(),
    };
    WriteError::Kept(path.to_path_buf(), kept)
}

/// The name a kept text goes under: `.Name.cold-lava-kept-SECONDS-N.ext` beside the file (the
/// file's stem and extension, the Unix time it was kept, a count), hidden, so the scan and
/// Obsidian pass it by, and apart from a staged write's `.Name.cold-lava-PID-N.tmp`
/// (`name::staged_for` never reads it), so the clearing of staged files a killed save left
/// (`stale`) never takes it. `kept_for` reads it back.
fn kept_name(file: &Path) -> PathBuf {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let n = NEXT.fetch_add(1, Ordering::Relaxed);
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let stem = file.file_stem().unwrap_or_default().to_string_lossy();
    let ext = file
        .extension()
        .map(|e| format!(".{}", e.to_string_lossy()))
        .unwrap_or_default();
    file.with_file_name(format!(".{stem}.cold-lava-kept-{secs}-{n}{ext}"))
}

/// The name of the file a kept text's name (`kept_name`) is for: `.Lava.cold-lava-kept-5-0.md`
/// is for `Lava.md`. None for any other name, a staged write's included.
pub(crate) fn kept_for(name: &str) -> Option<String> {
    let inner = name.strip_prefix('.')?;
    let (stem, after) = inner.rsplit_once(KEPT)?;
    let (ids, ext) = match after.split_once('.') {
        Some((ids, ext)) if !ext.is_empty() => (ids, Some(ext)),
        Some(_) => return None,
        None => (after, None),
    };
    let (secs, n) = ids.split_once('-')?;
    let digits = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    if stem.is_empty() || !digits(secs) || !digits(n) {
        return None;
    }
    Some(match ext {
        Some(ext) => format!("{stem}.{ext}"),
        None => stem.to_string(),
    })
}

/// The part of a kept text's name between the file's stem and the ids.
const KEPT: &str = ".cold-lava-kept-";

#[cfg(test)]
mod tests {
    use super::{kept_for, kept_name};
    use std::path::Path;

    #[test]
    fn the_name_kept_name_makes_is_the_one_kept_for_reads() {
        for file in [
            "N.md",
            "a.cold-lava-kept-1-2.md",
            "Board 1.canvas",
            "no-extension",
        ] {
            let kept = kept_name(&Path::new("dir").join(file));
            let name = kept.file_name().unwrap().to_str().unwrap();
            let stem = Path::new(file).file_stem().unwrap().to_str().unwrap();
            let ext = Path::new(file).extension().map(|e| e.to_str().unwrap());
            let want = match ext {
                Some(ext) => format!("{stem}.{ext}"),
                None => stem.to_string(),
            };
            assert_eq!(kept_for(name), Some(want), "{name}");
        }
        for not in [
            ".N.md.cold-lava-12-0.tmp",
            "N.cold-lava-kept-5-0.md",
            ".N.cold-lava-kept-5-.md",
            ".N.cold-lava-kept--0.md",
            ".N.cold-lava-kept-5x-0.md",
            ".N.cold-lava-kept-5-0.",
            "..cold-lava-kept-5-0.md",
            ".N.md",
        ] {
            assert_eq!(kept_for(not), None, "{not}");
        }
    }
}
