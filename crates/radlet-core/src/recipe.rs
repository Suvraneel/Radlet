//! Recipes: named step chains saved as small TOML files.
//!
//! ```toml
//! name = "Web-ready image"
//! description = "Longest side 2048 px, WebP at quality 80, no metadata"
//!
//! [[steps]]
//! op = "resize"
//! max = 2048
//!
//! [[steps]]
//! op = "convert"
//! to = "webp"
//! quality = 80
//!
//! [[steps]]
//! op = "strip_metadata"
//! ```

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::format::Format;
use crate::ops::{Action, Step};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Recipe {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub steps: Vec<Step>,
}

const MAX_STEPS: usize = 16;

/// The extension of a loadout shared as a file (the same TOML as the
/// loadouts folder holds), so it opens in Radlet rather than an editor.
pub const SHARED_EXTENSION: &str = "radletloadout";

/// The largest shared loadout read, in bytes: far more than 16 steps need.
pub const MAX_SHARED_BYTES: usize = 16 * 1024;

/// A recipe file that couldn't be read, and why.
pub type BrokenRecipe = (PathBuf, String);

impl Recipe {
    pub fn from_toml(src: &str) -> Result<Recipe> {
        let r: Recipe =
            toml::from_str(src).map_err(|e| Error::Invalid(format!("loadout: {}", e.message())))?;
        r.validate()?;
        Ok(r)
    }

    /// Reads a loadout someone shared, as a file or a link: the same TOML,
    /// with a size limit since it comes from outside.
    pub fn from_shared(src: &str) -> Result<Recipe> {
        if src.len() > MAX_SHARED_BYTES {
            return Err(Error::TooLarge(
                "this loadout is too big to be one Radlet made".into(),
            ));
        }
        Recipe::from_toml(src)
    }

    pub fn to_toml(&self) -> Result<String> {
        toml::to_string_pretty(self)
            .map_err(|e| Error::Invalid(format!("loadout \"{}\" can't be saved ({e})", self.name)))
    }

    pub fn validate(&self) -> Result<()> {
        let bad = |m: &str| Err(Error::Invalid(format!("loadout \"{}\": {m}", self.name)));
        if self.name.trim().is_empty() {
            return Err(Error::Invalid("a loadout needs a name".into()));
        }
        if self.steps.is_empty() {
            return bad("needs at least one step");
        }
        if self.steps.len() > MAX_STEPS {
            return bad("has more than 16 steps");
        }
        for s in &self.steps {
            match s {
                Step::Convert {
                    quality: Some(q), ..
                } if !(1..=100).contains(q) => return bad("quality must be 1-100"),
                Step::Resize {
                    max,
                    percent,
                    width,
                    height,
                } => {
                    let set = [max, percent, width, height]
                        .iter()
                        .filter(|v| v.is_some())
                        .count();
                    if set != 1 {
                        return bad("resize needs exactly one of max, percent, width or height");
                    }
                    if let Some(p) = percent {
                        if !(1..=100).contains(p) {
                            return bad("percent must be 1-100");
                        }
                    }
                    if [max, width, height].iter().any(|v| **v == Some(0)) {
                        return bad("sizes must be at least 1 pixel");
                    }
                }
                Step::Rotate { degrees } if ![90, 180, 270].contains(degrees) => {
                    return bad("rotation must be 90, 180 or 270")
                }
                Step::Crop { rect } if !rect.is_valid() => {
                    return bad("crop area must lie inside the image")
                }
                Step::Redact { rects, .. }
                    if rects.is_empty() || rects.iter().any(|r| !r.is_valid()) =>
                {
                    return bad("redaction areas must lie inside the image")
                }
                Step::Compress {
                    target_bytes: Some(0),
                    ..
                } => return bad("target size must be above 0"),
                // TOML integers are signed 64-bit.
                Step::Compress {
                    target_bytes: Some(t),
                    ..
                } if *t > i64::MAX as u64 => return bad("target size is too large"),
                Step::Pages { pages, .. } if pages.is_empty() => {
                    return bad("choose at least one page")
                }
                Step::Pages { rotate, .. }
                    if rotate.iter().any(|t| ![90, 180, 270].contains(&t.degrees)) =>
                {
                    return bad("pages turn by 90, 180 or 270 degrees")
                }
                Step::PageNumbers { size: Some(s), .. } if !(4.0..=72.0).contains(s) => {
                    return bad("page numbers must be 4 to 72 points")
                }
                Step::Watermark { text, .. } if text.trim().is_empty() => {
                    return bad("the watermark needs some text")
                }
                Step::Watermark { opacity, .. } if !(0.05..=1.0).contains(opacity) => {
                    return bad("watermark opacity must be 0.05 to 1")
                }
                Step::Transcribe { to, .. }
                    if !matches!(
                        to,
                        crate::format::Format::Srt
                            | crate::format::Format::Vtt
                            | crate::format::Format::Txt
                    ) =>
                {
                    return bad("subtitles are saved as srt, vtt or txt")
                }
                Step::Transcribe {
                    language: Some(l), ..
                } if !crate::engines::speech::valid_language(l) => {
                    return bad("the language must be a code such as en-US")
                }
                Step::Trim { start, end }
                    if !start.is_finite()
                        || *start < 0.0
                        || end.is_some_and(|e| !e.is_finite() || e <= *start) =>
                {
                    return bad("a trim's end must come after its start")
                }
                _ => {}
            }
        }
        Ok(())
    }

    pub fn slug(&self) -> String {
        crate::pipeline::slug(&self.name)
    }

    pub fn action(&self) -> Action {
        Action::Steps {
            steps: self.steps.clone(),
            label: Some(self.name.clone()),
        }
    }

    /// Recipes that ship with Radlet.
    pub fn builtin() -> Vec<Recipe> {
        vec![
            Recipe {
                name: "Web-ready image".into(),
                description: Some("Longest side 2048 px, WebP at quality 80, no metadata".into()),
                steps: vec![
                    Step::resize_max(2048),
                    Step::Convert {
                        to: Format::Webp,
                        quality: Some(80),
                    },
                    Step::StripMetadata,
                ],
            },
            Recipe {
                name: "Email-friendly photo".into(),
                description: Some(
                    "Longest side 1600 px, JPEG at quality 80, no location data".into(),
                ),
                steps: vec![
                    Step::resize_max(1600),
                    Step::Convert {
                        to: Format::Jpeg,
                        quality: Some(80),
                    },
                    Step::StripMetadata,
                ],
            },
        ]
    }

    /// Loads every `*.toml` recipe in `dir`. Broken files are returned
    /// separately so one typo doesn't hide the rest.
    pub fn load_dir(dir: &Path) -> (Vec<Recipe>, Vec<BrokenRecipe>) {
        let (found, errors) = Recipe::load_dir_with_paths(dir);
        (found.into_iter().map(|(_, r)| r).collect(), errors)
    }

    /// Like [`Recipe::load_dir`], with the file each recipe came from.
    pub fn load_dir_with_paths(dir: &Path) -> (Vec<(PathBuf, Recipe)>, Vec<BrokenRecipe>) {
        let mut ok = Vec::new();
        let mut errors = Vec::new();
        let Ok(entries) = std::fs::read_dir(dir) else {
            return (ok, errors);
        };
        let mut paths: Vec<PathBuf> = entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|e| e == "toml"))
            .collect();
        paths.sort();
        for p in paths {
            match std::fs::read_to_string(&p)
                .map_err(Error::from)
                .and_then(|s| Recipe::from_toml(&s))
            {
                Ok(r) => ok.push((p, r)),
                Err(e) => errors.push((p, e.to_string())),
            }
        }
        (ok, errors)
    }

    /// Saves the recipe in `dir` as `<slug>.toml` (or "<slug> 2.toml" if
    /// that's taken), or, with `replace`, over that recipe file in `dir`.
    /// Returns the file written.
    pub fn save(&self, dir: &Path, replace: Option<&Path>) -> Result<PathBuf> {
        self.validate()?;
        std::fs::create_dir_all(dir)?;
        let text = self.to_toml()?;
        if let Some(target) = replace {
            // Only an existing recipe file, directly in the recipes folder,
            // can be rewritten: never a link, another kind of file, or a
            // path somewhere else.
            let inside = target
                .parent()
                .and_then(|p| std::fs::canonicalize(p).ok())
                .zip(std::fs::canonicalize(dir).ok())
                .is_some_and(|(a, b)| a == b);
            let meta = std::fs::symlink_metadata(target)
                .ok()
                .filter(|m| m.is_file());
            let is_recipe = meta.is_some()
                && std::fs::read_to_string(target)
                    .ok()
                    .is_some_and(|src| Recipe::from_toml(&src).is_ok());
            if !inside || target.extension().is_none_or(|e| e != "toml") || !is_recipe {
                return Err(Error::Invalid(
                    "only loadout files in the loadouts folder can be replaced".into(),
                ));
            }
            let mut tmp = tempfile::NamedTempFile::new_in(dir)?;
            std::io::Write::write_all(&mut tmp, text.as_bytes())?;
            if let Some(meta) = meta {
                std::fs::set_permissions(tmp.path(), meta.permissions())?;
            }
            tmp.persist(target).map_err(|e| Error::Io(e.error))?;
            return Ok(target.to_path_buf());
        }
        let slug = self.slug();
        for n in 1..1000 {
            let name = if n == 1 {
                format!("{slug}.toml")
            } else {
                format!("{slug} {n}.toml")
            };
            let path = dir.join(name);
            match std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)
            {
                Ok(mut f) => {
                    std::io::Write::write_all(&mut f, text.as_bytes())?;
                    return Ok(path);
                }
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(e.into()),
            }
        }
        Err(Error::Invalid("too many loadouts with this name".into()))
    }

    /// Finds a recipe by name or slug among `recipes`, or loads it from a
    /// path if `query` names a file.
    pub fn find(query: &str, recipes: &[Recipe]) -> Result<Recipe> {
        let p = Path::new(query);
        if p.extension().is_some_and(|e| e == "toml") && p.is_file() {
            return Recipe::from_toml(&std::fs::read_to_string(p)?);
        }
        if p.extension().is_some_and(|e| e == SHARED_EXTENSION) && p.is_file() {
            return Recipe::from_shared(&std::fs::read_to_string(p)?);
        }
        let q = query.to_lowercase();
        recipes
            .iter()
            .find(|r| r.name.to_lowercase() == q || r.slug() == q)
            .cloned()
            .ok_or_else(|| Error::Invalid(format!("no loadout called \"{query}\"")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shared_loadouts_are_read_with_care() {
        let r = &Recipe::builtin()[0];
        let text = r.to_toml().unwrap();
        assert_eq!(&Recipe::from_shared(&text).unwrap(), r);
        let padded = format!("{text}\n# {}\n", "x".repeat(MAX_SHARED_BYTES));
        assert!(matches!(
            Recipe::from_shared(&padded),
            Err(Error::TooLarge(_))
        ));
        assert!(Recipe::from_shared("name = \"x\"\nsteps = []\n").is_err());
        assert!(
            Recipe::from_shared(
                "name = \"x\"\nrun = \"rm -rf\"\n[[steps]]\nop = \"strip_metadata\"\n"
            )
            .is_err(),
            "unknown keys refused"
        );
        // A shared file is found by its path, like a .toml one.
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join(format!("Web.{SHARED_EXTENSION}"));
        std::fs::write(&file, &text).unwrap();
        assert_eq!(&Recipe::find(file.to_str().unwrap(), &[]).unwrap(), r);
    }

    #[test]
    fn toml_round_trip() {
        for r in Recipe::builtin() {
            let back = Recipe::from_toml(&r.to_toml().unwrap()).unwrap();
            assert_eq!(back, r);
        }
    }

    #[test]
    fn parses_documented_example() {
        let src = r#"
name = "Web-ready image"

[[steps]]
op = "resize"
max = 2048

[[steps]]
op = "convert"
to = "webp"
quality = 80

[[steps]]
op = "strip_metadata"
"#;
        let r = Recipe::from_toml(src).unwrap();
        assert_eq!(r.steps.len(), 3);
        assert_eq!(r.slug(), "web-ready-image");
    }

    #[test]
    fn rejects_bad_recipes() {
        assert!(Recipe::from_toml("name = \"x\"\nsteps = []").is_err());
        assert!(
            Recipe::from_toml("name = \"x\"\n[[steps]]\nop = \"rotate\"\ndegrees = 45").is_err()
        );
        assert!(Recipe::from_toml("name = \"x\"\n[[steps]]\nop = \"resize\"").is_err());
        assert!(Recipe::from_toml("name = \"x\"\n[[steps]]\nop = \"explode\"").is_err());
        assert!(
            Recipe::from_toml("name = \"x\"\ncolour = 1\n[[steps]]\nop = \"strip_metadata\"")
                .is_err()
        );
    }

    #[test]
    fn load_dir_reports_broken_files() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("good.toml"),
            Recipe::builtin()[0].to_toml().unwrap(),
        )
        .unwrap();
        std::fs::write(dir.path().join("bad.toml"), "name = ").unwrap();
        std::fs::write(dir.path().join("ignored.txt"), "x").unwrap();
        let (ok, errors) = Recipe::load_dir(dir.path());
        assert_eq!(ok.len(), 1);
        assert_eq!(errors.len(), 1);
        assert!(Recipe::find("web-ready-image", &ok).is_ok());
        assert!(Recipe::find("nope", &ok).is_err());
    }

    #[test]
    fn saving_never_overwrites_unless_editing() {
        let dir = tempfile::tempdir().unwrap();
        let r = Recipe {
            name: "Shrink scans".into(),
            description: Some("Smaller PDFs".into()),
            steps: vec![
                Step::compress(crate::ops::CompressLevel::Strong),
                Step::PageNumbers {
                    position: Default::default(),
                    style: crate::ops::NumberStyle::PageOfTotal,
                    start: 1,
                    size: None,
                    skip_first: false,
                },
            ],
        };
        let first = r.save(dir.path(), None).unwrap();
        assert_eq!(first.file_name().unwrap(), "shrink-scans.toml");
        let second = r.save(dir.path(), None).unwrap();
        assert_eq!(second.file_name().unwrap(), "shrink-scans 2.toml");
        let (found, errors) = Recipe::load_dir_with_paths(dir.path());
        assert!(errors.is_empty());
        assert_eq!(found.len(), 2);
        assert_eq!(found[0].1, r);

        let edited = Recipe {
            name: "Shrink scans".into(),
            description: None,
            steps: vec![Step::StripMetadata],
        };
        assert_eq!(edited.save(dir.path(), Some(&first)).unwrap(), first);
        assert_eq!(
            Recipe::from_toml(&std::fs::read_to_string(&first).unwrap()).unwrap(),
            edited
        );
        // Only files in the recipes folder can be replaced.
        let elsewhere = tempfile::tempdir().unwrap();
        let outside = elsewhere.path().join("x.toml");
        std::fs::write(&outside, "x").unwrap();
        assert!(edited.save(dir.path(), Some(&outside)).is_err());
        assert_eq!(std::fs::read_to_string(&outside).unwrap(), "x");
        // ...and only recipe files: not other TOML, links, or missing files.
        let cargo = dir.path().join("Cargo.toml");
        std::fs::write(&cargo, "[package]\nname = \"x\"\n").unwrap();
        assert!(edited.save(dir.path(), Some(&cargo)).is_err());
        assert!(std::fs::read_to_string(&cargo)
            .unwrap()
            .contains("[package]"));
        assert!(edited
            .save(dir.path(), Some(&dir.path().join("missing.toml")))
            .is_err());
        #[cfg(unix)]
        {
            let link = dir.path().join("link.toml");
            std::os::unix::fs::symlink(&first, &link).unwrap();
            assert!(edited.save(dir.path(), Some(&link)).is_err());
        }
        // Sizes TOML can't hold are refused rather than crashing the save.
        let huge = Recipe {
            name: "Huge".into(),
            description: None,
            steps: vec![Step::Compress {
                level: Default::default(),
                target_bytes: Some(u64::MAX),
            }],
        };
        assert!(huge.save(dir.path(), None).is_err());
        // Invalid recipes aren't written.
        let empty = Recipe {
            name: "Empty".into(),
            description: None,
            steps: vec![],
        };
        assert!(empty.save(dir.path(), None).is_err());
    }
}
