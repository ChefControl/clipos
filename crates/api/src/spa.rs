use std::path::Path;

use tower_http::services::{ServeDir, ServeFile};

/// Serves the built SPA. Unknown paths get `index.html` so client-side routes like
/// `/me` work on reload.
pub fn service(static_dir: &Path) -> ServeDir<ServeFile> {
    ServeDir::new(static_dir)
        .append_index_html_on_directories(true)
        .fallback(ServeFile::new(static_dir.join("index.html")))
}
