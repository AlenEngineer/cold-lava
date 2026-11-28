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

use cold_lava_engine::core::Core;
use cold_lava_engine::paths;
use cold_lava_host::{Effect, ExtId, Extension, Host, Registry, installed};
use std::path::Path;
use std::time::{Duration, Instant};

fn main() {
    let mut args = std::env::args().skip(1);
    let Some(root) = args.next() else {
        eprintln!("usage: headless WORKSPACE [SECONDS]");
        std::process::exit(2);
    };
    let seconds = args.next().and_then(|s| s.parse().ok()).unwrap_or(3);
    let mut core = match Core::open(Path::new(&root), Default::default()) {
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
    let events = core.poll();
    for (ext, effect) in registry.frame(core, &events) {
        show(registry, ext, effect);
    }
}

/// Call one extension, then apply what it asked for.
fn call(
    registry: &mut Registry,
    core: &mut Core,
    ext: ExtId,
    f: impl FnOnce(&mut dyn Extension, &mut Host<'_>),
) {
    for effect in registry.call(ext, Some(core), f) {
        show(registry, ext, effect);
    }
}

/// What this frontend does for an extension's `Effect`: it shows its words, and names the rest.
fn show(registry: &Registry, ext: ExtId, effect: Effect) {
    let name = &registry.manifest(ext).name;
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
