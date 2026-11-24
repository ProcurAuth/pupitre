// SPDX-License-Identifier: MIT OR Apache-2.0
//! The static files of the page (5.21 to 5.23), embedded in the binary.
//!
//! The folder `web/` of the repository holds the files. [`ASSETS`] lists each file below
//! `/static/<build>/`, in the byte order of the paths. [`SHELL`] is the HTML of each page, with the
//! markers `{{base_path}}` and `{{build}}`. The server answers a page path with [`render_shell`]
//! and a static path with [`find`].
//!
//! The list [`ASSETS`] is written by `tests/page/gen_assets.py`. The test `web_assets` and the script
//! `tests/page/check_page.py` fail when a file of `web/` is not in the list.

use std::sync::OnceLock;

use pupitre_core::etag::sha256_hex;

/// One static file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Asset {
    /// The path below `/static/<build>/`, for example `css/base.css`.
    pub path: &'static str,
    /// The `Content-Type` of the answer.
    pub content_type: &'static str,
    /// The bytes.
    pub bytes: &'static [u8],
}

/// The static files, in the byte order of their paths.
#[rustfmt::skip]
pub const ASSETS: &[Asset] = &[
    // ASSETS BEGIN (written by tests/page/gen_assets.py)
    Asset { path: "css/base.css", content_type: "text/css; charset=utf-8", bytes: include_bytes!("../../../web/css/base.css") },
    Asset { path: "css/components.css", content_type: "text/css; charset=utf-8", bytes: include_bytes!("../../../web/css/components.css") },
    Asset { path: "css/fonts.css", content_type: "text/css; charset=utf-8", bytes: include_bytes!("../../../web/css/fonts.css") },
    Asset { path: "css/history.css", content_type: "text/css; charset=utf-8", bytes: include_bytes!("../../../web/css/history.css") },
    Asset { path: "css/list.css", content_type: "text/css; charset=utf-8", bytes: include_bytes!("../../../web/css/list.css") },
    Asset { path: "css/panel-logs.css", content_type: "text/css; charset=utf-8", bytes: include_bytes!("../../../web/css/panel-logs.css") },
    Asset { path: "css/pupitre.css", content_type: "text/css; charset=utf-8", bytes: include_bytes!("../../../web/css/pupitre.css") },
    Asset { path: "css/tokens.css", content_type: "text/css; charset=utf-8", bytes: include_bytes!("../../../web/css/tokens.css") },
    Asset { path: "css/vendor/chdash/00-tokens.css", content_type: "text/css; charset=utf-8", bytes: include_bytes!("../../../web/css/vendor/chdash/00-tokens.css") },
    Asset { path: "fonts/inter/InterVariable.woff2", content_type: "font/woff2", bytes: include_bytes!("../../../web/fonts/inter/InterVariable.woff2") },
    Asset { path: "fonts/inter/LICENSE", content_type: "text/plain; charset=utf-8", bytes: include_bytes!("../../../web/fonts/inter/LICENSE") },
    Asset { path: "icons.svg", content_type: "image/svg+xml", bytes: include_bytes!("../../../web/icons.svg") },
    Asset { path: "js/api.js", content_type: "text/javascript; charset=utf-8", bytes: include_bytes!("../../../web/js/api.js") },
    Asset { path: "js/boot_theme.js", content_type: "text/javascript; charset=utf-8", bytes: include_bytes!("../../../web/js/boot_theme.js") },
    Asset { path: "js/busy.js", content_type: "text/javascript; charset=utf-8", bytes: include_bytes!("../../../web/js/busy.js") },
    Asset { path: "js/canonical.js", content_type: "text/javascript; charset=utf-8", bytes: include_bytes!("../../../web/js/canonical.js") },
    Asset { path: "js/config.js", content_type: "text/javascript; charset=utf-8", bytes: include_bytes!("../../../web/js/config.js") },
    Asset { path: "js/ctl.js", content_type: "text/javascript; charset=utf-8", bytes: include_bytes!("../../../web/js/ctl.js") },
    Asset { path: "js/diff.js", content_type: "text/javascript; charset=utf-8", bytes: include_bytes!("../../../web/js/diff.js") },
    Asset { path: "js/diff_view.js", content_type: "text/javascript; charset=utf-8", bytes: include_bytes!("../../../web/js/diff_view.js") },
    Asset { path: "js/errors.js", content_type: "text/javascript; charset=utf-8", bytes: include_bytes!("../../../web/js/errors.js") },
    Asset { path: "js/format.js", content_type: "text/javascript; charset=utf-8", bytes: include_bytes!("../../../web/js/format.js") },
    Asset { path: "js/lib/catalog.js", content_type: "text/javascript; charset=utf-8", bytes: include_bytes!("../../../web/js/lib/catalog.js") },
    Asset { path: "js/lib/conflict.js", content_type: "text/javascript; charset=utf-8", bytes: include_bytes!("../../../web/js/lib/conflict.js") },
    Asset { path: "js/lib/dom.js", content_type: "text/javascript; charset=utf-8", bytes: include_bytes!("../../../web/js/lib/dom.js") },
    Asset { path: "js/lib/host_form.js", content_type: "text/javascript; charset=utf-8", bytes: include_bytes!("../../../web/js/lib/host_form.js") },
    Asset { path: "js/lib/hostbox.js", content_type: "text/javascript; charset=utf-8", bytes: include_bytes!("../../../web/js/lib/hostbox.js") },
    Asset { path: "js/lib/layer_page.js", content_type: "text/javascript; charset=utf-8", bytes: include_bytes!("../../../web/js/lib/layer_page.js") },
    Asset { path: "js/lib/list_table.js", content_type: "text/javascript; charset=utf-8", bytes: include_bytes!("../../../web/js/lib/list_table.js") },
    Asset { path: "js/lib/poll.js", content_type: "text/javascript; charset=utf-8", bytes: include_bytes!("../../../web/js/lib/poll.js") },
    Asset { path: "js/lib/runflow.js", content_type: "text/javascript; charset=utf-8", bytes: include_bytes!("../../../web/js/lib/runflow.js") },
    Asset { path: "js/lib/runs_list.js", content_type: "text/javascript; charset=utf-8", bytes: include_bytes!("../../../web/js/lib/runs_list.js") },
    Asset { path: "js/lib/runview.js", content_type: "text/javascript; charset=utf-8", bytes: include_bytes!("../../../web/js/lib/runview.js") },
    Asset { path: "js/lib/values_editor.js", content_type: "text/javascript; charset=utf-8", bytes: include_bytes!("../../../web/js/lib/values_editor.js") },
    Asset { path: "js/main.js", content_type: "text/javascript; charset=utf-8", bytes: include_bytes!("../../../web/js/main.js") },
    Asset { path: "js/pages/command.js", content_type: "text/javascript; charset=utf-8", bytes: include_bytes!("../../../web/js/pages/command.js") },
    Asset { path: "js/pages/commands.js", content_type: "text/javascript; charset=utf-8", bytes: include_bytes!("../../../web/js/pages/commands.js") },
    Asset { path: "js/pages/env_common.js", content_type: "text/javascript; charset=utf-8", bytes: include_bytes!("../../../web/js/pages/env_common.js") },
    Asset { path: "js/pages/env_hosts.js", content_type: "text/javascript; charset=utf-8", bytes: include_bytes!("../../../web/js/pages/env_hosts.js") },
    Asset { path: "js/pages/env_runs.js", content_type: "text/javascript; charset=utf-8", bytes: include_bytes!("../../../web/js/pages/env_runs.js") },
    Asset { path: "js/pages/env_secrets.js", content_type: "text/javascript; charset=utf-8", bytes: include_bytes!("../../../web/js/pages/env_secrets.js") },
    Asset { path: "js/pages/env_units.js", content_type: "text/javascript; charset=utf-8", bytes: include_bytes!("../../../web/js/pages/env_units.js") },
    Asset { path: "js/pages/environment.js", content_type: "text/javascript; charset=utf-8", bytes: include_bytes!("../../../web/js/pages/environment.js") },
    Asset { path: "js/pages/environments.js", content_type: "text/javascript; charset=utf-8", bytes: include_bytes!("../../../web/js/pages/environments.js") },
    Asset { path: "js/pages/history.js", content_type: "text/javascript; charset=utf-8", bytes: include_bytes!("../../../web/js/pages/history.js") },
    Asset { path: "js/pages/host.js", content_type: "text/javascript; charset=utf-8", bytes: include_bytes!("../../../web/js/pages/host.js") },
    Asset { path: "js/pages/run.js", content_type: "text/javascript; charset=utf-8", bytes: include_bytes!("../../../web/js/pages/run.js") },
    Asset { path: "js/pages/run_unit.js", content_type: "text/javascript; charset=utf-8", bytes: include_bytes!("../../../web/js/pages/run_unit.js") },
    Asset { path: "js/pages/runs.js", content_type: "text/javascript; charset=utf-8", bytes: include_bytes!("../../../web/js/pages/runs.js") },
    Asset { path: "js/pages/unit.js", content_type: "text/javascript; charset=utf-8", bytes: include_bytes!("../../../web/js/pages/unit.js") },
    Asset { path: "js/routes.js", content_type: "text/javascript; charset=utf-8", bytes: include_bytes!("../../../web/js/routes.js") },
    Asset { path: "js/theme.js", content_type: "text/javascript; charset=utf-8", bytes: include_bytes!("../../../web/js/theme.js") },
    Asset { path: "js/ui.js", content_type: "text/javascript; charset=utf-8", bytes: include_bytes!("../../../web/js/ui.js") },
    Asset { path: "js/validate.js", content_type: "text/javascript; charset=utf-8", bytes: include_bytes!("../../../web/js/validate.js") },
    // ASSETS END
];

/// The shell of each page (5.23), with the markers `{{base_path}}` and `{{build}}`.
pub const SHELL: &str = include_str!("../../../web/shell.html");

/// The marker of the base path in [`SHELL`].
pub const MARKER_BASE_PATH: &str = "{{base_path}}";

/// The marker of the build identifier in [`SHELL`].
pub const MARKER_BUILD: &str = "{{build}}";

/// The identifier of the build (5.23): the first 12 hexadecimal characters of the SHA-256 of all
/// static files.
///
/// The input is the list [`ASSETS`] in order. For each file it holds the path, U+000A, the length
/// of the file in decimal digits, U+000A and the bytes. The shell is not a static file, so it is not
/// in the input. The value is the same for each run of one binary.
pub fn build_id() -> &'static str {
    static BUILD: OnceLock<String> = OnceLock::new();
    BUILD.get_or_init(|| {
        let mut input = Vec::new();
        for asset in ASSETS {
            input.extend_from_slice(asset.path.as_bytes());
            input.push(b'\n');
            input.extend_from_slice(asset.bytes.len().to_string().as_bytes());
            input.push(b'\n');
            input.extend_from_slice(asset.bytes);
        }
        sha256_hex(&input)[..12].to_string()
    })
}

/// The shell of a page for a base path (5.2). The base path is a value that the configuration
/// checked (6.6): it has no character that needs an escape in an HTML attribute.
pub fn render_shell(base_path: &str) -> String {
    SHELL.replace(MARKER_BASE_PATH, base_path).replace(MARKER_BUILD, build_id())
}

/// The static file for a path below `/static/<build>/`. It gives `None` for a path that is not in
/// the list.
pub fn find(path: &str) -> Option<&'static Asset> {
    ASSETS.binary_search_by(|asset| asset.path.as_bytes().cmp(path.as_bytes())).ok().map(|i| &ASSETS[i])
}
