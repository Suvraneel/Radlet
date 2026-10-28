//! The website's gallery (site/gallery): every loadout listed reads as a
//! shared loadout would in the app, every file is listed, and every listed
//! file is there. Skins are read by the app's own parser in the Swift tests.

use std::collections::BTreeSet;
use std::path::PathBuf;

use radlet_core::recipe::{Recipe, MAX_SHARED_BYTES, SHARED_EXTENSION};

fn gallery() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../site/gallery")
}

fn listed(kind: &str) -> Vec<String> {
    let text = std::fs::read_to_string(gallery().join("items.json")).expect("items.json");
    let items: serde_json::Value = serde_json::from_str(&text).expect("items.json is JSON");
    items[kind]
        .as_array()
        .unwrap_or_else(|| panic!("items.json lists {kind}"))
        .iter()
        .map(|v| v.as_str().expect("a path").to_string())
        .collect()
}

fn on_disk(dir: &str, ext: &str) -> BTreeSet<String> {
    std::fs::read_dir(gallery().join(dir))
        .unwrap_or_else(|_| panic!("site/gallery/{dir}"))
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| !n.starts_with('.'))
        .inspect(|n| {
            assert!(
                n.ends_with(&format!(".{ext}")),
                "{dir}/{n} isn't a .{ext} file"
            )
        })
        .map(|n| format!("{dir}/{n}"))
        .collect()
}

#[test]
fn every_gallery_loadout_reads_cleanly() {
    let list = listed("loadouts");
    let unique: BTreeSet<String> = list.iter().cloned().collect();
    assert_eq!(unique.len(), list.len(), "a loadout is listed twice");
    assert_eq!(
        unique,
        on_disk("loadouts", SHARED_EXTENSION),
        "items.json and the folder differ"
    );
    let mut names = BTreeSet::new();
    for path in &list {
        let text = std::fs::read_to_string(gallery().join(path)).unwrap();
        assert!(text.len() <= MAX_SHARED_BYTES, "{path} is too big to share");
        let recipe = Recipe::from_shared(&text).unwrap_or_else(|e| panic!("{path}: {e}"));
        assert!(
            recipe
                .description
                .as_deref()
                .is_some_and(|d| !d.trim().is_empty()),
            "{path} needs a description for its card"
        );
        assert!(
            names.insert(recipe.name.to_lowercase()),
            "two loadouts are called {}",
            recipe.name
        );
        // Built-ins are already in everyone's Radlet.
        assert!(
            Recipe::builtin().iter().all(|b| b.name != recipe.name),
            "{path} has a built-in loadout's name"
        );
    }
}

#[test]
fn every_gallery_skin_is_listed() {
    let list = listed("skins");
    let unique: BTreeSet<String> = list.iter().cloned().collect();
    assert_eq!(unique.len(), list.len(), "a skin is listed twice");
    assert_eq!(
        unique,
        on_disk("skins", "radletskin"),
        "items.json and the folder differ"
    );
    for path in &list {
        let text = std::fs::read_to_string(gallery().join(path)).unwrap();
        assert!(text.len() <= 64 * 1024, "{path} is too big to share");
        assert!(text.contains("--name:"), "{path} needs a --name");
    }
}
