//! Where a layer of definitions came from: built in, an extension's manifest, config.toml or the
//! workspace. Types, attribute readings and shapes are each loaded in layers that carry one. A
//! leaf.

use std::fmt;
use std::path::PathBuf;

use serde::Serialize;

/// Where a type was defined.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum Origin {
    /// `types/*.toml` in the engine: the task.
    BuiltIn,
    /// An extension's manifest; its types are named `<extension>.<name>`.
    Extension { name: String },
    /// `[types.*]` in config.toml.
    Config { path: PathBuf },
    /// The workspace's `.cold-lava/types.toml`.
    Workspace { path: PathBuf },
}

impl Origin {
    /// What an error line starts with: the file, or the extension.
    pub(crate) fn place(&self) -> String {
        match self {
            Origin::BuiltIn => "built in".into(),
            Origin::Extension { name } => format!("extension {name}"),
            Origin::Config { path } | Origin::Workspace { path } => path.display().to_string(),
        }
    }
}

impl fmt::Display for Origin {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Origin::BuiltIn => f.write_str("built in"),
            Origin::Extension { name } => write!(f, "extension {name}"),
            Origin::Config { path } => write!(f, "config {}", path.display()),
            Origin::Workspace { path } => write!(f, "workspace {}", path.display()),
        }
    }
}
