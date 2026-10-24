//! Liseuse locale : pages, cases et bulles superposées, correction du texte, des
//! locuteurs et de l'ordre, lecture des voix. Écoute uniquement sur 127.0.0.1.

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;
use axum::Json;
use axum::Router;
use axum::extract::{Path, State};
use axum::http::{StatusCode, header};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::get;

use crate::analysis::PageAnalysis;
use crate::project::Project;

const INDEX: &str = include_str!("index.html");

struct App {
    project: Project,
}

type Shared = Arc<App>;

fn err(code: StatusCode, msg: impl ToString) -> Response {
    (code, msg.to_string()).into_response()
}

/// Un nom de fichier simple, sans chemin : rien ne sort du dossier projet.
fn safe_name(name: &str) -> Option<&str> {
    (!name.is_empty() && !name.contains(['/', '\\']) && !name.starts_with('.')).then_some(name)
}

async fn project_info(State(app): State<Shared>) -> Response {
    Json(&app.project.cfg).into_response()
}

async fn get_analysis(State(app): State<Shared>, Path(i): Path<usize>) -> Response {
    let Some(page) = app.project.cfg.pages.get(i) else { return err(StatusCode::NOT_FOUND, "page inconnue") };
    match PageAnalysis::load(&app.project.analysis_path(page)) {
        Ok(a) => Json(a).into_response(),
        Err(e) => err(StatusCode::NOT_FOUND, format!("{e:#}")),
    }
}

async fn put_analysis(State(app): State<Shared>, Path(i): Path<usize>, Json(body): Json<PageAnalysis>) -> Response {
    let Some(page) = app.project.cfg.pages.get(i) else { return err(StatusCode::NOT_FOUND, "page inconnue") };
    match body.save(&app.project.analysis_path(page)) {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}")),
    }
}

async fn file(app: &App, dir: &str, name: &str) -> Response {
    let Some(name) = safe_name(name) else { return err(StatusCode::BAD_REQUEST, "nom invalide") };
    let path: PathBuf = app.project.path(dir).join(name);
    let mime = match path.extension().and_then(|e| e.to_str()).unwrap_or("") {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "webp" => "image/webp",
        "gif" => "image/gif",
        "wav" => "audio/wav",
        "mp4" => "video/mp4",
        _ => "application/octet-stream",
    };
    match tokio::fs::read(&path).await {
        Ok(bytes) => ([(header::CONTENT_TYPE, mime), (header::CACHE_CONTROL, "no-cache")], bytes).into_response(),
        Err(_) => err(StatusCode::NOT_FOUND, "fichier absent"),
    }
}

async fn page_file(State(app): State<Shared>, Path(name): Path<String>) -> Response {
    file(&app, "pages", &name).await
}

async fn audio_file(State(app): State<Shared>, Path(name): Path<String>) -> Response {
    file(&app, "audio", &name).await
}

pub fn router(project: Project) -> Router {
    Router::new()
        .route("/", get(|| async { Html(INDEX) }))
        .route("/api/project", get(project_info))
        .route("/api/analysis/{i}", get(get_analysis).put(put_analysis))
        .route("/pages/{name}", get(page_file))
        .route("/audio/{name}", get(audio_file))
        .with_state(Arc::new(App { project }))
}

pub fn serve(project: Project, port: u16, open: bool) -> Result<()> {
    let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build()?;
    rt.block_on(async move {
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", port)).await?;
        let url = format!("http://{}", listener.local_addr()?);
        println!("liseuse : {url}  (Ctrl+C pour arrêter)");
        if open {
            std::process::Command::new("open").arg(&url).spawn().ok();
        }
        axum::serve(listener, router(project)).await?;
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::safe_name;

    #[test]
    fn rejects_paths() {
        assert_eq!(safe_name("0001.png"), Some("0001.png"));
        assert_eq!(safe_name("../project.toml"), None);
        assert_eq!(safe_name(".."), None);
        assert_eq!(safe_name("a/b.png"), None);
    }
}
