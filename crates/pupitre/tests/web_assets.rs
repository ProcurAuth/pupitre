// SPDX-License-Identifier: MIT OR Apache-2.0
//! The static files of the page (5.21 to 5.23): the list, the shell and the references between the files.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use pupitre::web_assets::{ASSETS, MARKER_BASE_PATH, MARKER_BUILD, SHELL, build_id, find, render_shell};
use pupitre_core::etag::sha256_hex;

fn web_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..").join("web")
}

fn files_of(dir: &Path, base: &Path, out: &mut BTreeSet<String>) {
    for entry in std::fs::read_dir(dir).expect("the web folder is readable") {
        let path = entry.expect("an entry").path();
        if path.is_dir() {
            files_of(&path, base, out);
        } else {
            let rel = path.strip_prefix(base).expect("a path below the base").to_string_lossy().replace('\\', "/");
            out.insert(rel);
        }
    }
}

fn text_of(path: &str) -> &'static str {
    let asset = find(path).unwrap_or_else(|| panic!("the asset {path} is not in the list"));
    std::str::from_utf8(asset.bytes).expect("a text file")
}

/// Join a relative reference to the folder of a file: `./x.js` and `../y.css` give a path from the root.
fn resolve(from: &str, reference: &str) -> String {
    let mut parts: Vec<&str> = from.split('/').collect();
    parts.pop();
    for part in reference.split('/') {
        match part {
            "." | "" => {}
            ".." => {
                parts.pop();
            }
            other => parts.push(other),
        }
    }
    parts.join("/")
}

#[test]
fn paths_are_in_byte_order_and_each_one_time() {
    for pair in ASSETS.windows(2) {
        assert!(
            pair[0].path.as_bytes() < pair[1].path.as_bytes(),
            "{} must come before {}",
            pair[0].path,
            pair[1].path
        );
    }
}

#[test]
fn each_asset_has_bytes_and_the_content_type_of_its_extension() {
    for asset in ASSETS {
        assert!(!asset.bytes.is_empty(), "{} is empty", asset.path);
        let expected = match asset.path.rsplit_once('.').map(|(_, ext)| ext) {
            Some("css") => "text/css; charset=utf-8",
            Some("js") => "text/javascript; charset=utf-8",
            Some("svg") => "image/svg+xml",
            Some("woff2") => "font/woff2",
            _ => "text/plain; charset=utf-8",
        };
        assert_eq!(asset.content_type, expected, "the content type of {}", asset.path);
    }
}

#[test]
fn each_file_of_the_web_folder_is_in_the_list() {
    let dir = web_dir();
    let mut found = BTreeSet::new();
    files_of(&dir, &dir, &mut found);
    found.remove("shell.html");
    let listed: BTreeSet<String> = ASSETS.iter().map(|a| a.path.to_string()).collect();
    assert_eq!(found, listed, "run tests/page/gen_assets.py");
}

#[test]
fn the_shell_has_the_two_markers_and_no_other_marker() {
    assert!(SHELL.contains(MARKER_BASE_PATH));
    assert!(SHELL.contains(MARKER_BUILD));
    let rest = SHELL.replace(MARKER_BASE_PATH, "").replace(MARKER_BUILD, "");
    assert!(!rest.contains("{{"), "the shell has another marker");
    assert_eq!(SHELL.matches("<base ").count(), 1, "the shell has one base element");
}

#[test]
fn the_shell_holds_no_data_no_inline_script_and_no_style_attribute() {
    assert!(!SHELL.contains(" style="), "a style attribute");
    assert!(!SHELL.contains("<style"), "an inline style");
    assert!(!SHELL.contains("innerHTML"));
    for (at, _) in SHELL.match_indices("<script") {
        let end = at + SHELL[at..].find('>').expect("a closed tag");
        assert!(SHELL[at..end].contains(" src="), "an inline script: {}", &SHELL[at..end]);
    }
    assert!(!SHELL.contains("Pupitre</title>"), "the shell holds the name of the product (5.23)");
    assert!(!SHELL.contains("rel=\"icon\" href=\"static"), "the shell holds a favicon (5.23)");
}

#[test]
fn the_shell_refers_only_to_files_in_the_list() {
    let mut count = 0;
    for part in SHELL.split("static/{{build}}/").skip(1) {
        let path = part.split(['"', '\'']).next().expect("a path");
        assert!(find(path).is_some(), "the shell names {path}, which is not in the list");
        count += 1;
    }
    assert!(count >= 8, "the shell has too few references: {count}");
}

#[test]
fn render_shell_replaces_each_marker() {
    let page = render_shell("/console/");
    assert!(page.contains("<base href=\"/console/\">"));
    assert!(page.contains(&format!("static/{}/js/main.js", build_id())));
    assert!(!page.contains("{{"));
}

#[test]
fn the_build_id_is_the_start_of_the_hash_of_all_static_files() {
    let id = build_id();
    assert_eq!(id.len(), 12);
    assert!(id.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)));
    let mut input = Vec::new();
    for asset in ASSETS {
        input.extend_from_slice(format!("{}\n{}\n", asset.path, asset.bytes.len()).as_bytes());
        input.extend_from_slice(asset.bytes);
    }
    assert_eq!(id, &sha256_hex(&input)[..12]);
}

#[test]
fn find_gives_a_listed_file_and_nothing_else() {
    assert_eq!(find("icons.svg").map(|a| a.path), Some("icons.svg"));
    assert!(find("shell.html").is_none());
    assert!(find("../etc/passwd").is_none());
    assert!(find("css/").is_none());
    assert!(find("").is_none());
    for asset in ASSETS {
        assert_eq!(find(asset.path), Some(asset));
    }
}

#[test]
fn the_javascript_modules_import_only_files_in_the_list() {
    let mut count = 0;
    for asset in ASSETS.iter().filter(|a| a.path.ends_with(".js")) {
        let text = text_of(asset.path);
        for quote in ['"', '\''] {
            for part in
                text.split(&format!("from {quote}")).skip(1).chain(text.split(&format!("import({quote}")).skip(1))
            {
                let reference = part.split(quote).next().expect("a reference");
                if !reference.starts_with('.') {
                    continue;
                }
                let target = resolve(asset.path, reference);
                assert!(
                    find(&target).is_some(),
                    "{} imports {reference}, which is {target}, not in the list",
                    asset.path
                );
                count += 1;
            }
        }
    }
    assert!(count > 40, "the check found too few imports: {count}");
}

#[test]
fn the_stylesheets_refer_only_to_files_in_the_list() {
    for asset in ASSETS.iter().filter(|a| a.path.ends_with(".css")) {
        // The copied tokens of ClickHouse Dash name the IBM Plex fonts. Pupitre uses Inter, so no page asks for them.
        if asset.path == "css/vendor/chdash/00-tokens.css" {
            continue;
        }
        for part in text_of(asset.path).split("url(").skip(1) {
            let reference = part.split(')').next().expect("a reference").trim().trim_matches(['"', '\'']);
            if reference.starts_with("data:") || reference.starts_with('#') {
                continue;
            }
            let target = resolve(asset.path, reference);
            assert!(
                find(&target).is_some(),
                "{} refers to {reference}, which is {target}, not in the list",
                asset.path
            );
        }
    }
}

#[test]
fn no_file_names_another_origin_or_a_local_path() {
    for asset in ASSETS.iter().filter(|a| a.content_type.starts_with("text/") || a.content_type == "image/svg+xml") {
        if asset.path.ends_with("LICENSE") || asset.path == "css/vendor/chdash/00-tokens.css" {
            continue;
        }
        let text = String::from_utf8_lossy(asset.bytes);
        for line in text.lines() {
            // A name space of SVG is a name, not a request.
            let line = line.replace("http://www.w3.org/", "");
            assert!(
                !line.contains("http://") && !line.contains("https://"),
                "{} names another origin: {line}",
                asset.path
            );
            assert!(!line.contains("/home/"), "{} has a local path", asset.path);
        }
    }
}
