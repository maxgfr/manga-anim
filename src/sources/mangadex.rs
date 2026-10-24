//! API publique MangaDex v4 : https://api.mangadex.org/docs/
//!
//! Lecture anonyme, limite de 5 requêtes/s. Les chapitres sous licence officielle
//! n'ont pas de pages chez MangaDex (`externalUrl` vers l'éditeur) : on les signale.

use std::fs;
use std::path::Path;
use std::thread::sleep;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use reqwest::blocking::Client;
use serde_json::Value;

use super::{ChapterInfo, SeriesHit, Source};

const API: &str = "https://api.mangadex.org";
const PAUSE: Duration = Duration::from_millis(250);

pub struct MangaDex {
    http: Client,
}

impl MangaDex {
    pub fn new() -> Result<Self> {
        let http = Client::builder()
            .user_agent(concat!("manga-anim/", env!("CARGO_PKG_VERSION")))
            .timeout(Duration::from_secs(60))
            .build()?;
        Ok(Self { http })
    }

    fn get(&self, path: &str, query: &[(&str, String)]) -> Result<Value> {
        sleep(PAUSE);
        let resp = self.http.get(format!("{API}{path}")).query(query).send()?;
        let status = resp.status();
        let body: Value = resp.json().context("réponse MangaDex illisible")?;
        if !status.is_success() {
            bail!("MangaDex {status} sur {path} : {}", body["errors"]);
        }
        Ok(body)
    }
}

fn pick_title(attrs: &Value) -> String {
    let titles = &attrs["title"];
    for lang in ["en", "fr", "ja-ro", "ja"] {
        if let Some(t) = titles[lang].as_str() {
            return t.to_string();
        }
    }
    titles.as_object().and_then(|o| o.values().next()).and_then(|v| v.as_str()).unwrap_or("?").to_string()
}

impl Source for MangaDex {
    fn search(&self, query: &str, lang: Option<&str>) -> Result<Vec<SeriesHit>> {
        let mut q = vec![
            ("title", query.to_string()),
            ("limit", "15".into()),
            ("order[relevance]", "desc".into()),
        ];
        for rating in ["safe", "suggestive", "erotica"] {
            q.push(("contentRating[]", rating.into()));
        }
        if let Some(l) = lang {
            q.push(("availableTranslatedLanguage[]", l.into()));
        }
        let body = self.get("/manga", &q)?;
        Ok(body["data"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|m| {
                let a = &m["attributes"];
                SeriesHit {
                    id: m["id"].as_str().unwrap_or_default().into(),
                    title: pick_title(a),
                    year: a["year"].as_i64(),
                    status: a["status"].as_str().unwrap_or_default().into(),
                    languages: a["availableTranslatedLanguages"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(|l| l.as_str().map(String::from))
                        .collect(),
                }
            })
            .collect())
    }

    fn series_title(&self, series_id: &str) -> Result<String> {
        let body = self.get(&format!("/manga/{series_id}"), &[])?;
        Ok(pick_title(&body["data"]["attributes"]))
    }

    fn chapters(&self, series_id: &str, lang: &str) -> Result<Vec<ChapterInfo>> {
        let mut out = Vec::new();
        let mut offset = 0;
        loop {
            let mut q = vec![
                ("translatedLanguage[]", lang.to_string()),
                ("order[chapter]", "asc".into()),
                ("limit", "500".into()),
                ("offset", offset.to_string()),
                ("includes[]", "scanlation_group".into()),
            ];
            for rating in ["safe", "suggestive", "erotica"] {
                q.push(("contentRating[]", rating.into()));
            }
            let body = self.get(&format!("/manga/{series_id}/feed"), &q)?;
            let data = body["data"].as_array().cloned().unwrap_or_default();
            for c in &data {
                let a = &c["attributes"];
                let group = c["relationships"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .find(|r| r["type"] == "scanlation_group")
                    .and_then(|r| r["attributes"]["name"].as_str())
                    .unwrap_or("")
                    .to_string();
                out.push(ChapterInfo {
                    id: c["id"].as_str().unwrap_or_default().into(),
                    chapter: a["chapter"].as_str().and_then(|s| s.parse().ok()),
                    volume: a["volume"].as_str().map(String::from),
                    title: a["title"].as_str().unwrap_or_default().into(),
                    pages: a["pages"].as_u64().unwrap_or(0) as u32,
                    group,
                    external_url: a["externalUrl"].as_str().map(String::from),
                });
            }
            let total = body["total"].as_u64().unwrap_or(0) as usize;
            offset += data.len();
            if data.is_empty() || offset >= total {
                break;
            }
        }
        Ok(out)
    }

    fn fetch(&self, chapter: &ChapterInfo, dest: &Path) -> Result<usize> {
        if let Some(url) = &chapter.external_url {
            bail!("chapitre {} hébergé chez l'éditeur ({url}) : rien à télécharger chez MangaDex", chapter.chapter.unwrap_or(0.0));
        }
        let body = self.get(&format!("/at-home/server/{}", chapter.id), &[])?;
        let base = body["baseUrl"].as_str().context("baseUrl absent")?;
        let hash = body["chapter"]["hash"].as_str().context("hash absent")?;
        let files: Vec<&str> = body["chapter"]["data"].as_array().into_iter().flatten().filter_map(|v| v.as_str()).collect();
        if files.is_empty() {
            bail!("chapitre sans pages");
        }
        fs::create_dir_all(dest)?;
        for (i, file) in files.iter().enumerate() {
            let ext = Path::new(file).extension().and_then(|e| e.to_str()).unwrap_or("png");
            let url = format!("{base}/data/{hash}/{file}");
            let mut last_err = None;
            for attempt in 0..3 {
                match self.http.get(&url).send().and_then(|r| r.error_for_status()).and_then(|r| r.bytes()) {
                    Ok(bytes) => {
                        fs::write(dest.join(format!("{:03}.{ext}", i + 1)), &bytes)?;
                        last_err = None;
                        break;
                    }
                    Err(e) => {
                        last_err = Some(e);
                        sleep(Duration::from_secs(1 << attempt));
                    }
                }
            }
            if let Some(e) = last_err {
                return Err(e).with_context(|| format!("page {} introuvable", i + 1));
            }
        }
        Ok(files.len())
    }
}
