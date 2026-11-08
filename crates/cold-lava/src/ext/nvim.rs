//! Neovim (or Vim) as the editor. It runs in the core's pane with `VIM_SCRIPT` loaded, which
//! reports every note it opens or saves and asks cold-lava to follow links.

use super::{
    CommandRequest, CommandSpec, EditorRequest, Extension, Host, Manifest, PaneId, PaneNews,
    PaneSpec, Role,
};
use crate::core::Jump;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

/// Its name in messages and commands.
pub const NAME: &str = "nvim";

pub struct Nvim {
    program: String,
    /// Its pane, while it runs.
    pane: Option<PaneId>,
    /// The pane `nvim.restart` asked to quit: when it exits, Neovim starts again.
    restarting: Option<PaneId>,
    /// The note it showed last, to start again on.
    note: Option<PathBuf>,
    /// `VIM_SCRIPT` on disk, written at the first start and removed with this extension.
    script: Option<PathBuf>,
}

impl Nvim {
    /// Neovim (or Vim) to embed: $COLD_LAVA_EDITOR, $VISUAL or $EDITOR if it is one, else `nvim`
    /// if it's installed. `COLD_LAVA_EDITOR=none` means no editor (the reader shows notes).
    pub fn detect() -> Option<Nvim> {
        let is_vim = |p: &str| {
            let base = Path::new(p)
                .file_name()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default();
            matches!(
                base.as_str(),
                "nvim" | "vim" | "vi" | "nvim.exe" | "vim.exe"
            )
        };
        let found = |program: String| Nvim {
            program,
            pane: None,
            restarting: None,
            note: None,
            script: None,
        };
        if std::env::var("COLD_LAVA_EDITOR").is_ok_and(|v| v == "none") {
            return None;
        }
        for var in ["COLD_LAVA_EDITOR", "VISUAL", "EDITOR"] {
            if let Ok(v) = std::env::var(var) {
                let prog = v.split_whitespace().next().unwrap_or("").to_string();
                if is_vim(&prog) {
                    return Some(found(prog));
                }
            }
        }
        let path = std::env::var_os("PATH")?;
        std::env::split_paths(&path)
            .find(|d| d.join("nvim").is_file())
            .map(|_| found("nvim".to_string()))
    }

    fn title(&self) -> &'static str {
        match Path::new(&self.program).file_stem() {
            Some(s) if s == "nvim" => "Neovim",
            _ => "Vim",
        }
    }

    fn start(&mut self, note: &Path, jump: Option<&Jump>, host: &mut Host<'_>) {
        let mut args = vec!["-c".to_string(), "set autoread".to_string()];
        if self.script.is_none() {
            self.script = write_script();
        }
        if let Some(script) = &self.script {
            args.push("-S".to_string());
            args.push(script.to_string_lossy().into_owned());
        }
        args.push(note.to_string_lossy().into_owned());
        let program = self.program.clone();
        let pane = host.open_pane(PaneSpec { program, args });
        self.pane = Some(pane);
        if let Some(j) = jump {
            host.pane_input(pane, open_command_at(note, Some(j)));
        }
    }

    /// `nvim.restart`: save everything and quit, then start again on the same note. A Neovim
    /// that won't quit (a buffer it can't write) stays, showing why.
    fn restart(&mut self, host: &mut Host<'_>) {
        let title = self.title();
        match (self.pane, self.note.clone()) {
            (Some(pane), _) => {
                self.restarting = Some(pane);
                host.notify(format!("restarting {title}"));
                host.pane_input(pane, "\x1c\x0e:silent! wall | qall\r");
            }
            (None, Some(note)) => self.start(&note, None, host),
            (None, None) => host.notify(format!("Select a note to open {title}")),
        }
    }
}

impl Extension for Nvim {
    fn initialize(&mut self) -> Manifest {
        let hint = |key: &str, does: &str| (key.to_string(), does.to_string());
        Manifest {
            name: NAME.to_string(),
            title: self.title().to_string(),
            version: String::new(),
            description: String::new(),
            roles: vec![Role::Editor],
            hints: vec![
                hint("⏎ on a [[link]]", "follows it"),
                hint("Ctrl-o", "back"),
            ],
            commands: vec![CommandSpec {
                id: "nvim.restart".to_string(),
                title: format!(
                    "restart {}: save, quit and start it again on the same note",
                    self.title()
                ),
                keys: Vec::new(),
            }],
            types: Default::default(),
            // It edits notes as text, in its pane: nothing from the workspace gateway.
            permissions: Vec::new(),
        }
    }

    fn pane(&mut self, news: PaneNews, host: &mut Host<'_>) {
        match news {
            PaneNews::Message {
                pane,
                kind,
                payload,
            } if self.pane == Some(pane) => {
                if kind == "open" {
                    self.note = Some(PathBuf::from(&payload));
                }
                message(&kind, &payload, host)
            }
            PaneNews::Exited { pane, error } if self.pane == Some(pane) => {
                self.pane = None;
                let restart = self.restarting.take() == Some(pane);
                match (error, self.note.clone()) {
                    // A start that failed is reported by the core, which shows the reader.
                    (Some(_), _) => {}
                    (None, Some(note)) if restart => self.start(&note, None, host),
                    (None, _) => {
                        let title = self.title();
                        host.notify(format!("{title} closed. Select a note to open it again"));
                    }
                }
            }
            _ => {}
        }
    }

    fn command(&mut self, request: CommandRequest, host: &mut Host<'_>) {
        let CommandRequest::Run { id, .. } = request;
        if id == "nvim.restart" {
            self.restart(host);
        }
    }

    fn editor(&mut self, request: EditorRequest, host: &mut Host<'_>) {
        match (request, self.pane) {
            (EditorRequest::Open { note, jump }, None) => {
                self.start(&note, jump.as_ref(), host);
                self.note = Some(note);
            }
            (EditorRequest::Open { note, jump }, Some(pane)) => {
                host.pane_input(pane, open_command_at(&note, jump.as_ref()))
            }
            (EditorRequest::Save, Some(pane)) => host.pane_input(pane, "\x1c\x0e:silent! wall\r"),
            (EditorRequest::Renamed { from, to }, Some(pane)) => {
                host.pane_input(pane, renamed_command(&from, &to))
            }
            (EditorRequest::Deleted { note }, Some(pane)) => {
                host.pane_input(pane, deleted_command(&note))
            }
            (
                EditorRequest::Save | EditorRequest::Renamed { .. } | EditorRequest::Deleted { .. },
                None,
            ) => {}
        }
    }
}

/// What each message from `VIM_SCRIPT` asks of the core.
fn message(kind: &str, payload: &str, host: &mut Host<'_>) {
    match kind {
        "open" => host.editor_opened(PathBuf::from(payload)),
        "follow" => {
            if let Some((from, link)) = payload.split_once('\t') {
                host.run_command("note.follow-link", json!({"from": from, "link": link}));
            }
        }
        "saved" => host.saved(PathBuf::from(payload)),
        "switcher" => host.run_command("note.switch", Value::Null),
        "search" => host.run_command("search.open", json!({"query": payload})),
        _ => {}
    }
}

/// The command we type into Vim/Neovim to show another note: save edits, open it, and
/// optionally move to a line, heading or block.
fn open_command_at(path: &Path, jump: Option<&Jump>) -> String {
    let q = |s: &str| s.replace('\'', "''");
    let p = q(&path.to_string_lossy());
    let goto = match jump {
        None => String::new(),
        Some(Jump::Line(n)) => format!(" | call cursor({n}, 1) | normal! zz"),
        // Very-nomagic search, so heading text is matched literally.
        Some(Jump::Heading(h)) => format!(
            " | call cursor(1, 1) | call search('\\v^#+\\s*\\V' . '{}', 'cW') | normal! zz",
            q(&h.replace('\\', "\\\\"))
        ),
        Some(Jump::Block(b)) => format!(
            " | call cursor(1, 1) | call search('\\V ^' . '{}', 'cW') | normal! zz",
            q(&b.replace('\\', "\\\\"))
        ),
    };
    // <C-\><C-n> gets to normal mode from any mode first.
    format!("\x1c\x0e:silent! wall | execute 'edit' fnameescape('{p}'){goto}\r")
}

/// After a rename: show the note under its new name if Vim had it open, drop the old buffer and
/// reload the buffers whose links were rewritten on disk.
fn renamed_command(from: &Path, to: &Path) -> String {
    let q = |p: &Path| p.to_string_lossy().replace('\'', "''");
    let (from, to) = (q(from), q(to));
    format!(
        "\x1c\x0e:if expand('%:p') ==# '{from}' | execute 'edit' fnameescape('{to}') | endif \
         | silent! execute 'bwipeout!' fnameescape('{from}') | silent! checktime\r"
    )
}

/// Wipe the deleted note's buffer without writing it, so Vim neither shows nor recreates it.
fn deleted_command(note: &Path) -> String {
    let note = note.to_string_lossy().replace('\'', "''");
    format!("\x1c\x0e:silent! execute 'bwipeout!' fnameescape('{note}')\r")
}

/// Loaded into the embedded Vim/Neovim: Enter (or gf) on a link asks cold-lava to follow it,
/// and every note Vim opens is reported, so the menu and backlinks stay in sync.
/// Messages go out as a private OSC sequence that cold-lava removes from the screen output.
const VIM_SCRIPT: &str = r#"" cold-lava integration (loaded automatically; see https://github.com/AlenEngineer/cold-lava)
if exists('g:cold_lava_loaded') | finish | endif
let g:cold_lava_loaded = 1

function! s:Enc(s) abort
  return substitute(a:s, '[^A-Za-z0-9 _./-]', '\=printf("%%u%x;", char2nr(submatch(0)))', 'g')
endfunction

function! s:Send(kind, payload) abort
  let l:msg = "\x1b]5379;" . a:kind . ";" . s:Enc(a:payload) . "\x07"
  if has('nvim')
    call chansend(v:stderr, l:msg)
  elseif exists('*echoraw')
    call echoraw(l:msg)
  endif
endfunction

" The [[wikilink]] or [text](link.md) under the cursor, or ''.
function! s:LinkUnderCursor() abort
  let l:line = getline('.')
  let l:col = col('.') - 1
  for l:pat in ['\[\[\([^\]]\+\)\]\]', '\[[^\]]*\](\([^)]\+\))']
    let l:start = 0
    while 1
      let l:m = matchstrpos(l:line, l:pat, l:start)
      if l:m[1] < 0 | break | endif
      if l:col >= l:m[1] && l:col < l:m[2]
        return matchlist(l:m[0], l:pat)[1]
      endif
      let l:start = l:m[2]
    endwhile
  endfor
  return ''
endfunction

function! ColdLavaFollow() abort
  let l:target = s:LinkUnderCursor()
  if l:target ==# '' || l:target =~# '^\a\+://'
    return 0
  endif
  silent! update
  call s:Send('follow', expand('%:p') . "\t" . l:target)
  return 1
endfunction

function! s:Enter() abort
  if !ColdLavaFollow()
    execute "normal! \<CR>"
  endif
endfunction

function! s:Gf() abort
  if !ColdLavaFollow()
    normal! gf
  endif
endfunction

augroup ColdLava
  autocmd!
  autocmd BufEnter *.md call s:Send('open', expand('%:p'))
  autocmd BufEnter *.md nnoremap <buffer><silent> <CR> :<C-u>call <SID>Enter()<CR>
  autocmd BufWritePost *.md call s:Send('saved', expand('%:p'))
augroup END
nnoremap <silent> gf :<C-u>call <SID>Gf()<CR>
" Open cold-lava's quick switcher or search from Vim; map them as you like, e.g.
"   nnoremap <C-p> <Cmd>ColdLavaSwitcher<CR>
command! -nargs=0 ColdLavaSwitcher call s:Send('switcher', '')
command! -nargs=* ColdLavaSearch call s:Send('search', <q-args>)
if expand('%:e') ==# 'md'
  silent! doautocmd <nomodeline> ColdLava BufEnter
endif
"#;

/// Writes `VIM_SCRIPT` to a file of its own: one per `Nvim`, so two of them in one process
/// (tests run in parallel) never write over the file the other is reading.
fn write_script() -> Option<PathBuf> {
    static NEXT: AtomicU32 = AtomicU32::new(0);
    let n = NEXT.fetch_add(1, Ordering::Relaxed);
    let p = std::env::temp_dir().join(format!("cold-lava-{}-{n}.vim", std::process::id()));
    std::fs::write(&p, VIM_SCRIPT).ok()?;
    Some(p)
}

impl Drop for Nvim {
    fn drop(&mut self) {
        if let Some(p) = &self.script {
            let _ = std::fs::remove_file(p);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ext::{Effect, ExtId, Registry};

    /// Neovim as the core drives it, with no program behind its pane.
    fn nvim() -> (Registry, ExtId) {
        let nvim = Nvim {
            program: "nvim".into(),
            pane: None,
            restarting: None,
            note: None,
            script: None,
        };
        let registry = Registry::new(vec![Box::new(nvim)]);
        let editor = registry.editor().expect("Neovim is an editor");
        (registry, editor)
    }

    fn ask(r: &mut Registry, ed: ExtId, request: EditorRequest) -> Vec<Effect> {
        r.call(ed, None, |e, host| e.editor(request, host))
    }

    fn open(r: &mut Registry, ed: ExtId, note: &str, jump: Option<Jump>) -> Vec<Effect> {
        let note = note.into();
        ask(r, ed, EditorRequest::Open { note, jump })
    }

    fn news(r: &mut Registry, ed: ExtId, news: PaneNews) -> Vec<Effect> {
        r.call(ed, None, |e, host| e.pane(news, host))
    }

    /// Opens a note so Neovim has a pane, and returns the pane.
    fn started(r: &mut Registry, ed: ExtId) -> PaneId {
        match open(r, ed, "/v/Lava.md", None)[..] {
            [Effect::OpenPane { pane, .. }] => pane,
            ref other => panic!("{other:?}"),
        }
    }

    #[test]
    fn first_open_starts_neovim_with_the_script_then_jumps() {
        let (mut r, ed) = nvim();
        let fx = open(&mut r, ed, "/v/Lava.md", Some(Jump::Line(12)));
        let [
            Effect::OpenPane { pane, spec },
            Effect::PaneInput {
                pane: typed_into,
                text,
            },
        ] = &fx[..]
        else {
            panic!("{fx:?}")
        };
        assert_eq!(spec.program, "nvim");
        assert_eq!(spec.args[..2], ["-c", "set autoread"]);
        assert_eq!(spec.args.last().unwrap(), "/v/Lava.md");
        let script = spec.args.iter().position(|a| a == "-S").unwrap() + 1;
        let script_file = PathBuf::from(&spec.args[script]);
        let script = std::fs::read_to_string(&script_file).unwrap();
        assert!(script.contains("function! ColdLavaFollow()"));
        assert_eq!(typed_into, pane);
        assert!(
            text.ends_with("| call cursor(12, 1) | normal! zz\r"),
            "{text:?}"
        );
        drop(r);
        assert!(!script_file.exists(), "the script goes with Neovim");
    }

    #[test]
    fn later_opens_are_typed_into_its_pane() {
        let (mut r, ed) = nvim();
        let pane = started(&mut r, ed);
        let fx = open(&mut r, ed, "/v/Hotspot.md", None);
        let [Effect::PaneInput { pane: p, text }] = &fx[..] else {
            panic!("{fx:?}")
        };
        assert_eq!(*p, pane);
        assert!(text.contains("fnameescape('/v/Hotspot.md')"), "{text:?}");
    }

    #[test]
    fn script_messages_ask_the_core() {
        let (mut r, ed) = nvim();
        let pane = started(&mut r, ed);
        let mut say = |kind: &str, payload: &str| {
            let (kind, payload) = (kind.to_string(), payload.to_string());
            news(
                &mut r,
                ed,
                PaneNews::Message {
                    pane,
                    kind,
                    payload,
                },
            )
        };
        let run = |id: &str, args| Effect::RunCommand {
            id: id.into(),
            args,
        };
        let note = || PathBuf::from("/v/Lava.md");
        assert_eq!(
            say("open", "/v/Lava.md"),
            [Effect::EditorOpened { note: note() }]
        );
        assert_eq!(
            say("follow", "/v/Lava.md\tHotspot#Plumes"),
            [run(
                "note.follow-link",
                json!({"from": "/v/Lava.md", "link": "Hotspot#Plumes"})
            )]
        );
        assert_eq!(say("saved", "/v/Lava.md"), [Effect::Saved { note: note() }]);
        assert_eq!(say("switcher", ""), [run("note.switch", Value::Null)]);
        assert_eq!(
            say("search", "ash cloud"),
            [run("search.open", json!({"query": "ash cloud"}))]
        );
        assert_eq!(say("follow", "no tab, no link"), []);
        assert_eq!(say("bogus", "x"), []);
    }

    #[test]
    fn news_from_another_pane_is_ignored() {
        let (mut r, ed) = nvim();
        let pane = started(&mut r, ed);
        let other = PaneId(pane.0 + 1);
        let (kind, payload) = ("open".to_string(), "/v/Lava.md".to_string());
        let message = PaneNews::Message {
            pane: other,
            kind,
            payload,
        };
        assert_eq!(news(&mut r, ed, message), []);
        let exited = PaneNews::Exited {
            pane: other,
            error: None,
        };
        assert_eq!(news(&mut r, ed, exited), []);
        assert!(matches!(
            open(&mut r, ed, "/v/Hotspot.md", None)[..],
            [Effect::PaneInput { pane: p, .. }] if p == pane
        ));
    }

    #[test]
    fn when_neovim_quits_it_says_so_and_the_next_note_starts_it_again() {
        let (mut r, ed) = nvim();
        let pane = started(&mut r, ed);
        let exited = PaneNews::Exited { pane, error: None };
        assert_eq!(
            news(&mut r, ed, exited),
            [Effect::Notify {
                message: "Neovim closed. Select a note to open it again".into()
            }]
        );
        assert_eq!(ask(&mut r, ed, EditorRequest::Save), [], "nothing to save");
        assert!(matches!(
            open(&mut r, ed, "/v/Lava.md", None)[..],
            [Effect::OpenPane { pane: p, .. }] if p != pane
        ));
    }

    fn restart(r: &mut Registry, ed: ExtId) -> Vec<Effect> {
        let request = CommandRequest::Run {
            id: "nvim.restart".into(),
            args: Value::Null,
        };
        r.call(ed, None, |e, host| e.command(request, host))
    }

    #[test]
    fn restart_saves_quits_and_starts_again_on_the_last_note() {
        let (mut r, ed) = nvim();
        assert!(
            r.commands().iter().any(|c| c.id == "nvim.restart"),
            "registered"
        );
        let pane = started(&mut r, ed);
        let (kind, payload) = ("open".to_string(), "/v/Hotspot.md".to_string());
        let opened = PaneNews::Message {
            pane,
            kind,
            payload,
        };
        news(&mut r, ed, opened);
        assert_eq!(
            restart(&mut r, ed),
            [
                Effect::Notify {
                    message: "restarting Neovim".into()
                },
                Effect::PaneInput {
                    pane,
                    text: "\x1c\x0e:silent! wall | qall\r".into()
                },
            ]
        );
        let fx = news(&mut r, ed, PaneNews::Exited { pane, error: None });
        let [Effect::OpenPane { pane: again, spec }] = &fx[..] else {
            panic!("{fx:?}")
        };
        assert_ne!(*again, pane);
        assert_eq!(spec.args.last().unwrap(), "/v/Hotspot.md");

        // Quitting it later is a plain quit again.
        let exited = PaneNews::Exited {
            pane: *again,
            error: None,
        };
        assert!(matches!(
            news(&mut r, ed, exited)[..],
            [Effect::Notify { .. }]
        ));
    }

    #[test]
    fn restart_when_closed_starts_it_and_without_a_note_says_how() {
        let (mut r, ed) = nvim();
        assert_eq!(
            restart(&mut r, ed),
            [Effect::Notify {
                message: "Select a note to open Neovim".into()
            }]
        );
        let pane = started(&mut r, ed);
        news(&mut r, ed, PaneNews::Exited { pane, error: None });
        let fx = restart(&mut r, ed);
        let [Effect::OpenPane { spec, .. }] = &fx[..] else {
            panic!("{fx:?}")
        };
        assert_eq!(spec.args.last().unwrap(), "/v/Lava.md");
    }

    #[test]
    fn a_failed_start_is_left_to_the_core_to_report() {
        let (mut r, ed) = nvim();
        let pane = started(&mut r, ed);
        let error = Some("couldn't start nvim".to_string());
        assert_eq!(news(&mut r, ed, PaneNews::Exited { pane, error }), []);
        assert!(matches!(
            open(&mut r, ed, "/v/Lava.md", None)[..],
            [Effect::OpenPane { .. }]
        ));
    }

    #[test]
    fn save_and_rename_are_typed_into_its_pane() {
        let (mut r, ed) = nvim();
        let pane = started(&mut r, ed);
        assert_eq!(
            ask(&mut r, ed, EditorRequest::Save),
            [Effect::PaneInput {
                pane,
                text: "\x1c\x0e:silent! wall\r".into()
            }]
        );
        let renamed = EditorRequest::Renamed {
            from: "/v/It's.md".into(),
            to: "/v/Rock/It's.md".into(),
        };
        let fx = ask(&mut r, ed, renamed);
        let [Effect::PaneInput { pane: p, text }] = &fx[..] else {
            panic!("{fx:?}")
        };
        assert_eq!(*p, pane);
        assert!(text.starts_with("\x1c\x0e:"), "{text:?}");
        assert!(
            text.contains("if expand('%:p') ==# '/v/It''s.md' | execute 'edit' fnameescape('/v/Rock/It''s.md') | endif"),
            "{text:?}"
        );
        assert!(
            text.contains("execute 'bwipeout!' fnameescape('/v/It''s.md') | silent! checktime\r"),
            "{text:?}"
        );
    }

    #[test]
    fn delete_wipes_the_buffer_without_writing() {
        let (mut r, ed) = nvim();
        let pane = started(&mut r, ed);
        let fx = ask(
            &mut r,
            ed,
            EditorRequest::Deleted {
                note: "/v/It's.md".into(),
            },
        );
        assert_eq!(
            fx,
            [Effect::PaneInput {
                pane,
                text: "\x1c\x0e:silent! execute 'bwipeout!' fnameescape('/v/It''s.md')\r".into()
            }]
        );
    }

    #[test]
    fn vim_open_command_escapes_quotes() {
        let c = open_command_at(Path::new("/v/It's here.md"), None);
        assert!(c.contains("fnameescape('/v/It''s here.md')"));
        assert!(c.starts_with("\x1c\x0e") && c.ends_with(")\r"));
        let c = open_command_at(Path::new("/v/a.md"), Some(&Jump::Line(12)));
        assert!(c.ends_with("| call cursor(12, 1) | normal! zz\r"));
        let c = open_command_at(
            Path::new("/v/a.md"),
            Some(&Jump::Heading("Don't panic".into())),
        );
        assert!(
            c.contains("search('\\v^#+\\s*\\V' . 'Don''t panic', 'cW')"),
            "{c}"
        );
    }
}
