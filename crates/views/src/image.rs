//! The image view's words (`{"kind": "image", …}` on the view gateway, docs/extensions.md,
//! "The image view, as built"): a picture from the workspace or sent as bytes. A frontend
//! reads and draws it its own way.

use serde::{Deserialize, Serialize};

/// The most bytes a picture may have, from a file or sent.
pub const MAX_BYTES: usize = 8 << 20;

/// What a view of a picture shows.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Image {
    pub title: String,
    #[serde(flatten)]
    pub source: Source,
}

/// Where the picture is: a workspace-relative `path`, or `png`, a PNG's bytes in base64.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Source {
    Path(String),
    Png(String),
}

impl Image {
    /// Refused (with why) when the bytes it sends are over `MAX_BYTES`.
    pub fn check(&self) -> Result<(), String> {
        match &self.source {
            Source::Png(b64) if b64.len() / 4 * 3 > MAX_BYTES => {
                Err(format!("an image sends at most {} MB", MAX_BYTES >> 20))
            }
            _ => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_image_view_is_json_with_a_path_or_a_png() {
        let wire = |image: Image, json: serde_json::Value| {
            assert_eq!(serde_json::to_value(&image).unwrap(), json);
            assert_eq!(serde_json::from_value::<Image>(json).unwrap(), image);
        };
        wire(
            Image {
                title: "Lava lake".into(),
                source: Source::Path("Volcanoes/lake.png".into()),
            },
            serde_json::json!({"title": "Lava lake", "path": "Volcanoes/lake.png"}),
        );
        wire(
            Image {
                title: "Chart".into(),
                source: Source::Png("iVBORw0KGgo=".into()),
            },
            serde_json::json!({"title": "Chart", "png": "iVBORw0KGgo="}),
        );
        let big = Image {
            title: "Big".into(),
            source: Source::Png("A".repeat(MAX_BYTES / 3 * 4 + 8)),
        };
        assert_eq!(big.check(), Err("an image sends at most 8 MB".into()));
    }
}
