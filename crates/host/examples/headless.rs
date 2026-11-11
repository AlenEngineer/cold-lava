//! The least a frontend writes to host extensions, with no terminal: open a workspace's core,
//! load the installed extensions, tell them the workspace, then once a frame hand them what
//! the core has for them and take what they ask for. A graphical frontend and a language
//! server start from this.
//!
//!     cargo run -p cold-lava-host --example headless -- WORKSPACE [SECONDS]
//!
//! It prints what the extensions say (`ui/notify`, `ui/status`) and names what it has no
//! screen for. Every request an extension makes of the workspace goes through `Host`, so its
//! manifest's permissions hold here as in the terminal UI.

use cold_lava_engine::core::{Core, Event};
use cold_lava_engine::{files, paths};
use cold_lava_host::{Effect, ExtId, Extension, FileNews, Host, Registry, installed};
use std::path::Path;
use std::time::{Duration, Instant};

fn main() {
    let mut args = std::env::args().skip(1);
    let Some(root) = args.next() else {
        eprintln!("usage: headless WORKSPACE [SECONDS]");
        std::process::exit(2);
    };
    let seconds = args.next().and_then(|s| s.parse().ok()).unwrap_or(3);
    let mut core = match Core::open(Path::new(&root)) {
        Ok(core) => core,
        Err(e) => {
            eprintln!("{root}: {e}");
            std::process::exit(1);
        }
    };

    // This frontend has no built-in extensions: only the ones installed in
    // ~/.config/cold-lava/extensions, each in a process of its own.
    let mut registry = Registry::load(Vec::new(), installed::user_dir().as_deref());
    for line in registry.mistakes().iter().chain(registry.refused()) {
        eprintln!("{line}");
    }

    // The files extensions own join the workspace: the core asks their owners what they say.
    let cache = paths::cache_dir().map(|d| d.join("cold-lava/owners"));
    core.set_owners(registry.owned_kinds(), cache.as_deref());

    let root = core.root().to_path_buf();
    for ext in registry.ids() {
        call(&mut registry, &mut core, ext, |e, host| {
            e.workspace(Some(&root), host)
        });
    }
    let until = Instant::now() + Duration::from_secs(seconds);
    while Instant::now() < until {
        frame(&mut registry, &mut core);
        std::thread::sleep(Duration::from_millis(16));
    }
    registry.shutdown();
}

/// One frame: what the core has for the extensions goes to them, and they answer what they
/// were asked since the last one. Nothing here waits for an extension.
fn frame(registry: &mut Registry, core: &mut Core) {
    for event in core.poll() {
        // A file that isn't a note changed on disk: its owner hears `file/changed`.
        if let Event::FileChanged(path) = event
            && let Some((owner, _)) = registry.owner_of(&path)
        {
            let rev = files::rev_at(core.root(), &path);
            let news = FileNews::Changed { path, rev };
            call(registry, core, owner, |e, host| e.file(news, host));
        }
    }
    // `items/changed` and `items/reset` go to whoever subscribed.
    for (owner, news) in core.take_item_news() {
        match registry.by_name(&owner) {
            Some(ext) => call(registry, core, ext, |e, host| e.items(news, host)),
            None => drop(core.unsubscribe_items(&owner, news.subscription())),
        }
    }
    // `file/read` and `file/edit` go to the owner of the file's kind.
    for (owner, read) in core.take_reads() {
        if let Some(ext) = registry.by_name(&owner) {
            call(registry, core, ext, |e, host| e.read_file(read, host));
        }
    }
    for (owner, edit) in core.take_file_edits() {
        match registry.by_name(&owner) {
            Some(ext) => call(registry, core, ext, |e, host| e.edit_file(edit, host)),
            None => {
                let why = Err(format!("{owner} isn't loaded"));
                core.file_edited(&owner, &edit.path, &edit.rev, why);
            }
        }
    }
    // An `items/edit` of a file another owner reads is answered once that owner wrote it.
    for (by, ticket, answer) in core.take_edited() {
        if let Some(ext) = registry.by_name(&by) {
            call(registry, core, ext, |e, host| {
                e.edited(ticket, answer, host)
            });
        }
    }
    for ext in registry.ids() {
        call(registry, core, ext, |e, host| e.poll(host));
    }
}

/// Call one extension, then apply what it asked for.
fn call(
    registry: &mut Registry,
    core: &mut Core,
    ext: ExtId,
    f: impl FnOnce(&mut dyn Extension, &mut Host<'_>),
) {
    let effects = registry.call(ext, Some(core), f);
    let name = &registry.manifest(ext).name;
    for effect in effects {
        match effect {
            Effect::Notify { message } => println!("{name}: {message}"),
            Effect::Status { text } => println!("{name}: [{}]", text.unwrap_or_default()),
            // A frontend with a screen opens panes, views and notes here, and runs commands.
            other => {
                let method = serde_json::to_value(&other).ok();
                let method = method.as_ref().and_then(|v| v["method"].as_str());
                println!("{name}: {} (not shown)", method.unwrap_or("an answer"));
            }
        }
    }
}
