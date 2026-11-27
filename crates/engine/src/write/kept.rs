//! What a write does with a text it took out of a file and can't put back: removed when it is
//! only the caller's or the write's own, kept beside the file under a name of its own
//! otherwise (`kept_name`), never removed. `replace` and `stale` both keep through it. A leaf
//! but for `name::reserved` and the `WriteError` it answers.

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
/// (`stale`) never takes it. Where the user is shown kept texts is #427.
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
