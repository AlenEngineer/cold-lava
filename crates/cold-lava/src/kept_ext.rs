//! The kept texts as a built-in extension (#427): what a save couldn't put back, another
//! program's text kept beside its note under a hidden name (`write::WriteError::Kept`), or a
//! killed save's leftover. The tree and Obsidian pass such a file by, so this one says how
//! many there are on its part of the status line for as long as there are any, and its
//! command lists them (`workspace/kept`) with the note each is for: Enter opens one in the
//! reader (`workspace/open`), `d` puts it away to the trash (`workspace/delete`) and `u` puts
//! the last one back where it was (`workspace/restore`). It asks
//! the gateway as any extension would, and the core's list is what it shows.

use std::path::Path;

use crate::ext::view::{self, Content, Happened, ViewId, ViewNews};
use crate::ext::{
    CommandRequest, CommandSpec, EditorRequest, Extension, Host, KEPT, Manifest, PaneNews,
    Permission,
};
use crate::files::{KeptText, KeptTexts, Restore, TrashId};
use crate::list::{Group, KeyCommand, List, Piece, Row};
use crate::words::plural;

/// The command that lists them, from the palette.
pub const OPEN: &str = "kept.open";

/// The command `d` sends on a row: put that text away to the trash.
const TRASH: &str = "trash";

/// The command `u` sends: put the text `d` last put away back where it was.
const UNDO: &str = "undo";

/// The kept texts' extension: what it last heard from the core, and its view while open.
#[derive(Default)]
pub struct Kept {
    heard: Option<KeptTexts>,
    view: Option<ViewId>,
    /// The last text put away, by its token and name, for `u`.
    put_away: Option<(TrashId, String)>,
}

/// The list of `kept`: one row a text, by the note it is for, with where it is on the right.
pub fn list(kept: &KeptTexts) -> List {
    let rows = (kept.kept.iter()).map(|k| Row {
        id: k.kept.clone(),
        text: vec![Piece::plain(&k.path)],
        check: None,
        right: vec![Piece::plain(name_of(&k.kept))],
        depth: 0,
        look: None,
    });
    let groups = match kept.kept.is_empty() {
        true => Vec::new(),
        false => vec![Group {
            id: "kept".into(),
            title: "Beside the note each is for".into(),
            collapsed: false,
            rows: rows.collect(),
        }],
    };
    List {
        title: "Kept texts".into(),
        groups,
        keys: vec![
            KeyCommand {
                key: 'd',
                command: TRASH.into(),
                title: "to the trash".into(),
            },
            KeyCommand {
                key: 'u',
                command: UNDO.into(),
                title: "put back".into(),
            },
        ],
        empty: Some("no kept texts".into()),
        filter: Default::default(),
    }
}

/// The status line's words for `n` kept texts; none for none.
pub fn status(n: usize) -> Option<String> {
    (n > 0).then(|| format!("{} · : kept texts", plural(n, "kept text")))
}

fn name_of(rel: &str) -> &str {
    rel.rsplit('/').next().unwrap_or(rel)
}

impl Kept {
    /// What the core lists now; when it changed, the status line and the open view say so.
    fn hear(&mut self, host: &mut Host<'_>) {
        let now = host.kept_texts().unwrap_or_default();
        if self.heard.as_ref() == Some(&now) {
            return;
        }
        host.status(status(now.kept.len()));
        // A list that had no rows comes back on its first: the group's title does nothing.
        let was_empty = self.heard.as_ref().is_none_or(|h| h.kept.is_empty());
        let select = (now.kept.first())
            .filter(|_| was_empty)
            .map(|k| k.kept.clone());
        if let Some(view) = self.view
            && let Err(e) = host.update_view(view, Content::List(list(&now)), select)
        {
            host.notify(format!("the kept texts couldn't be shown: {e}"));
        }
        self.heard = Some(now);
    }

    fn find(&self, row: &str) -> Option<&KeptText> {
        let heard = self.heard.as_ref()?;
        heard.kept.iter().find(|k| k.kept == row)
    }

    /// `workspace/delete` of the kept text at `row`, at the version listed.
    fn trash(&mut self, row: &str, host: &mut Host<'_>) {
        let Some(kept) = self.find(row).cloned() else {
            return;
        };
        let name = name_of(&kept.kept).to_string();
        match host.delete_file(&kept.kept, &kept.rev) {
            Ok(gone) => {
                host.notify(format!("put away to the trash: {name} · u puts it back"));
                self.put_away = Some((gone.trashed, name));
            }
            Err(e) => host.notify(format!("not put away: {e}")),
        }
        self.hear(host);
    }

    /// `workspace/restore` of the text `trash` last put away, where it was.
    fn undo(&mut self, host: &mut Host<'_>) {
        let Some((trashed, name)) = self.put_away.take() else {
            return host.notify("nothing put away to put back");
        };
        let restore = Restore {
            trashed: trashed.clone(),
            to: None,
        };
        match host.restore_file(&restore) {
            Ok(_) => host.notify(format!("put back: {name}")),
            Err(e) => {
                host.notify(format!("not put back: {e}"));
                self.put_away = Some((trashed, name));
            }
        }
        self.hear(host);
    }

    /// The first row, for the view to open on.
    fn first(&self) -> Option<String> {
        let heard = self.heard.as_ref()?;
        heard.kept.first().map(|k| k.kept.clone())
    }
}

impl Extension for Kept {
    fn initialize(&mut self) -> Manifest {
        Manifest {
            name: KEPT.into(),
            title: "Kept texts".into(),
            version: String::new(),
            description: "Texts a save couldn't put back, kept beside their notes".into(),
            roles: Vec::new(),
            hints: Vec::new(),
            commands: vec![CommandSpec {
                id: OPEN.into(),
                title: "Kept texts: what saves couldn't put back".into(),
                keys: Vec::new(),
            }],
            types: Default::default(),
            shapes: Vec::new(),
            permissions: vec![Permission::FilesRead, Permission::FilesDelete],
            file_types: Vec::new(),
            shows: Vec::new(),
        }
    }

    fn pane(&mut self, _: PaneNews, _: &mut Host<'_>) {}

    fn editor(&mut self, _: EditorRequest, _: &mut Host<'_>) {}

    fn command(&mut self, request: CommandRequest, host: &mut Host<'_>) {
        let CommandRequest::Run { id, .. } = request;
        if id != OPEN {
            return host.notify(format!("kept has no command {id}"));
        }
        if let Some(old) = self.view.take() {
            host.close_view(old);
        }
        self.hear(host);
        let now = self.heard.clone().unwrap_or_default();
        match host.open_view(Content::List(list(&now)), self.first(), None) {
            Ok(view) => self.view = Some(view),
            Err(e) => host.notify(format!("the kept texts couldn't be shown: {e}")),
        }
    }

    fn view(&mut self, news: ViewNews, host: &mut Host<'_>) {
        match news {
            ViewNews::Closed { view } if Some(view) == self.view => self.view = None,
            ViewNews::Event(Happened { view, event }) if Some(view) == self.view => match event {
                view::Event::Activated { row } if self.find(&row).is_some() => {
                    if let Err(e) = host.open_note(row.into(), None, None) {
                        host.notify(format!("the kept text couldn't be opened: {e}"));
                    }
                }
                view::Event::Command {
                    command,
                    row: Some(row),
                } if command == TRASH => self.trash(&row, host),
                view::Event::Command { command, .. } if command == UNDO => self.undo(host),
                _ => {}
            },
            _ => {}
        }
    }

    /// Once a frame: the core's list is in memory, so this reads no folder.
    fn poll(&mut self, host: &mut Host<'_>) {
        self.hear(host);
    }

    /// Another workspace: its view went with the old one, and its texts are heard anew.
    fn workspace(&mut self, _: Option<&Path>, _: &mut Host<'_>) {
        self.view = None;
        self.heard = None;
        self.put_away = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::Core;
    use crate::ext::{Effect, ExtId, Registry};

    /// The kept texts as an extension finds them: the status line counts them from the first
    /// frame, the command lists them by the note each is for, opening on the first, `d` on
    /// one puts it away to the trash, after which the status line says nothing, and `u` puts
    /// it back where it was.
    #[test]
    fn the_status_line_counts_them_and_the_list_puts_one_away() {
        let dir = std::env::temp_dir().join(format!("cold-lava-kept-ext-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join(".obsidian")).unwrap();
        std::fs::create_dir_all(dir.join("Notes")).unwrap();
        std::fs::write(dir.join("Notes/Lava.md"), "ours").unwrap();
        let kept = ".Lava.cold-lava-kept-1700000000-0.md";
        std::fs::write(dir.join("Notes").join(kept), "theirs").unwrap();
        let mut core = Core::open(&dir).unwrap();
        let bin = dir.with_extension("bin");
        core.set_system_trash(Some(crate::trash::Bin::Freedesktop(bin.clone())));
        let mut r = Registry::new(vec![Box::new(Kept::default()) as Box<dyn Extension>]);
        let ext = ExtId(0);
        let statuses = |effects: &[Effect]| -> Vec<Option<String>> {
            (effects.iter())
                .filter_map(|e| match e {
                    Effect::Status { text } => Some(text.clone()),
                    _ => None,
                })
                .collect()
        };

        let first = r.call(ext, Some(&mut core), |e, h| e.poll(h));
        assert_eq!(
            statuses(&first),
            [Some("1 kept text · : kept texts".into())]
        );
        let again = r.call(ext, Some(&mut core), |e, h| e.poll(h));
        assert!(again.is_empty(), "said once: {again:?}");

        let run = CommandRequest::Run {
            id: OPEN.into(),
            args: serde_json::Value::Null,
        };
        let opened = r.call(ext, Some(&mut core), |e, h| e.command(run, h));
        let Some(Effect::OpenView {
            view,
            content,
            select,
            ..
        }) = opened.first()
        else {
            panic!("{opened:?}");
        };
        assert_eq!(select.as_deref(), Some(format!("Notes/{kept}").as_str()));
        let Content::List(shown) = content else {
            panic!("{content:?}");
        };
        let row = &shown.groups[0].rows[0];
        assert_eq!(row.text, [Piece::plain("Notes/Lava.md")]);
        assert_eq!(row.right, [Piece::plain(kept)]);
        let view = *view;

        let open = ViewNews::Event(Happened {
            view,
            event: view::Event::Activated {
                row: row.id.clone(),
            },
        });
        let effects = r.call(ext, Some(&mut core), |e, h| e.view(open, h));
        assert!(
            matches!(&effects[..], [Effect::OpenNote { note, .. }]
                if *note == Path::new("Notes").join(kept)),
            "{effects:?}"
        );

        let trash = ViewNews::Event(Happened {
            view,
            event: view::Event::Command {
                command: TRASH.into(),
                row: Some(row.id.clone()),
            },
        });
        let effects = r.call(ext, Some(&mut core), |e, h| e.view(trash, h));
        assert_eq!(statuses(&effects), [None], "{effects:?}");
        assert!(!dir.join("Notes").join(kept).exists());
        assert!(bin.join("files").join(kept).exists(), "in the trash");
        assert_eq!(core.kept_texts(), KeptTexts::default());

        let undo = ViewNews::Event(Happened {
            view,
            event: view::Event::Command {
                command: UNDO.into(),
                row: None,
            },
        });
        let effects = r.call(ext, Some(&mut core), |e, h| e.view(undo, h));
        assert_eq!(
            statuses(&effects),
            [Some("1 kept text · : kept texts".into())]
        );
        let back = std::fs::read_to_string(dir.join("Notes").join(kept)).unwrap();
        assert_eq!(back, "theirs", "back where it was, under its hidden name");
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&bin);
    }
}
