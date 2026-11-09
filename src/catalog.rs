//! `manga models search`: free catalog lookup through the `llm-models` CLI, which lists current
//! models with their prices. It is not a Rust dependency: run directly if installed, else via npx.

use std::cmp::Ordering;
use std::process::Command;

use anyhow::{Context, Result, bail};
use clap::ValueEnum;
use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum Kind {
    Video,
    Image,
    Speech,
    Vision,
}

impl Kind {
    fn command(self) -> &'static str {
        match self {
            Kind::Video => "video",
            Kind::Image => "image",
            Kind::Speech => "speech",
            Kind::Vision => "find",
        }
    }
}

const INSTALL_HINT: &str = "llm-models not found: install it with `brew install maxgfr/tap/llm-models` or `npm install -g llm-models`, or install Node.js to use npx";

/// The program to run, direct or through npx. `None` when neither is available.
fn launcher(has_cli: bool, has_npx: bool) -> Option<Vec<String>> {
    match (has_cli, has_npx) {
        (true, _) => Some(vec!["llm-models".into()]),
        (false, true) => Some(vec!["npx".into(), "--yes".into(), "llm-models".into()]),
        (false, false) => None,
    }
}

fn runs(program: &str, arg: &str) -> bool {
    Command::new(program).arg(arg).output().is_ok_and(|o| o.status.success())
}

/// Whether the CLI (or its npx fallback) can be used, for `manga doctor`.
pub fn available() -> Option<&'static str> {
    if runs("llm-models", "--version") {
        Some("llm-models")
    } else if runs("npx", "--version") {
        Some("npx llm-models (downloaded on first search)")
    } else {
        None
    }
}

fn arguments(kind: Kind, query: Option<&str>, provider: Option<&str>) -> Vec<String> {
    let mut args = vec![kind.command().to_string()];
    match kind {
        // `-m image` keeps the models that accept images, which is what a review needs.
        Kind::Vision => {
            args.extend(["-m", "image", "--sort", "cost_input"].map(String::from));
            if let Some(q) = query {
                args.extend(["-s".to_string(), q.to_string()]);
            }
            if let Some(p) = provider {
                args.extend(["-p".to_string(), p.to_string()]);
            }
        }
        _ => {
            args.extend(query.map(String::from));
            if let Some(p) = provider {
                args.extend(["--provider".to_string(), p.to_string()]);
            }
        }
    }
    args.push("--json".into());
    args
}

#[derive(Debug, PartialEq)]
struct Row {
    id: String,
    provider: String,
    price: String,
    /// Lowest known price, used to list the cheapest first.
    cost: Option<f64>,
}

fn number(value: Option<&Value>) -> Option<f64> {
    value.and_then(Value::as_f64)
}

fn money(value: f64) -> String {
    let text = format!("{value:.4}");
    format!("${}", text.trim_end_matches('0').trim_end_matches('.'))
}

/// `$0.02-0.2/s` from the catalog's price list, or `?` when it publishes none.
fn price_range(prices: &[Value], field: &str, unit: impl Fn(&Value) -> String) -> (String, Option<f64>) {
    let values: Vec<f64> = prices.iter().filter_map(|p| number(p.get(field))).collect();
    if values.is_empty() {
        return ("?".into(), None);
    }
    let (low, high) = values.iter().fold((f64::MAX, f64::MIN), |(l, h), v| (l.min(*v), h.max(*v)));
    let range = if low < high { format!("{}-{}", money(low), &money(high)[1..]) } else { money(low) };
    (format!("{range}{}", unit(&prices[0])), Some(low))
}

fn row(kind: Kind, entry: &Value) -> Option<Row> {
    let text = |key: &str| entry.get(key).and_then(Value::as_str);
    let id = match kind {
        // A catalog id may be prefixed with its provider: show the one the API takes.
        Kind::Vision => text("openrouter_id").or_else(|| text("api_id")).or_else(|| text("id"))?,
        _ => text("id")?,
    };
    let (price, cost) = match kind {
        Kind::Vision => match (number(entry.pointer("/cost/input")), number(entry.pointer("/cost/output"))) {
            (Some(i), Some(o)) => (format!("{}/{} per M tokens (in/out)", money(i), money(o)), Some(i)),
            _ => ("?".into(), None),
        },
        _ => {
            let prices = entry.get("prices").and_then(Value::as_array).map(Vec::as_slice).unwrap_or(&[]);
            match kind {
                Kind::Video => price_range(prices, "usdPerSecond", |_| "/s".to_string()),
                _ => price_range(prices, "usd", |p| format!("/{}", p.get("per").and_then(Value::as_str).unwrap_or("?"))),
            }
        }
    };
    Some(Row { id: id.to_string(), provider: text("provider").unwrap_or("?").to_string(), price, cost })
}

fn rows(kind: Kind, entries: &[Value]) -> Vec<Row> {
    let mut rows: Vec<Row> = entries.iter().filter_map(|e| row(kind, e)).collect();
    // Unpriced models last; the sort is stable, so equal prices keep the catalog's order.
    rows.sort_by(|a, b| match (a.cost, b.cost) {
        (Some(x), Some(y)) => x.partial_cmp(&y).unwrap_or(Ordering::Equal),
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => Ordering::Equal,
    });
    rows
}

pub fn search(kind: Kind, query: Option<&str>, provider: Option<&str>, limit: usize) -> Result<()> {
    let launch = launcher(runs("llm-models", "--version"), runs("npx", "--version")).context(INSTALL_HINT)?;
    let args = arguments(kind, query, provider);
    let out = Command::new(&launch[0]).args(&launch[1..]).args(&args).output().with_context(|| format!("failed to start {}", launch[0]))?;
    if !out.status.success() {
        bail!("llm-models failed: {}", String::from_utf8_lossy(&out.stderr).split_whitespace().collect::<Vec<_>>().join(" "));
    }
    let entries: Vec<Value> = serde_json::from_slice(&out.stdout).context("llm-models returned something that is not a JSON list")?;
    let rows = rows(kind, &entries);
    if rows.is_empty() {
        println!("no model found");
        return Ok(());
    }
    let shown = if limit == 0 { rows.len() } else { limit.min(rows.len()) };
    let width = rows[..shown].iter().map(|r| r.id.len()).max().unwrap_or(0);
    for r in &rows[..shown] {
        println!("{:<width$}  {:<11}  {}", r.id, r.provider, r.price);
    }
    if shown < rows.len() {
        println!("{} more (--limit 0 for all)", rows.len() - shown);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn launcher_prefers_the_cli_then_npx() {
        assert_eq!(launcher(true, true), Some(vec!["llm-models".to_string()]));
        assert_eq!(launcher(false, true), Some(vec!["npx".to_string(), "--yes".to_string(), "llm-models".to_string()]));
        assert_eq!(launcher(false, false), None);
    }

    #[test]
    fn arguments_ask_for_json() {
        assert_eq!(arguments(Kind::Video, Some("wan"), Some("fal")), ["video", "wan", "--provider", "fal", "--json"]);
        assert_eq!(arguments(Kind::Speech, None, None), ["speech", "--json"]);
        assert_eq!(arguments(Kind::Vision, Some("flash"), Some("openrouter")), ["find", "-m", "image", "--sort", "cost_input", "-s", "flash", "-p", "openrouter", "--json"]);
    }

    #[test]
    fn video_rows_show_the_price_range() {
        let entries = [json!({"id": "a", "provider": "fal", "prices": [{"usdPerSecond": 0.1, "audio": true}, {"usdPerSecond": 0.02}]})];
        assert_eq!(rows(Kind::Video, &entries), [Row { id: "a".into(), provider: "fal".into(), price: "$0.02-0.1/s".into(), cost: Some(0.02) }]);
    }

    #[test]
    fn image_and_speech_rows_show_the_unit() {
        let image = rows(Kind::Image, &[json!({"id": "b", "provider": "openai", "prices": [{"usd": 0.04, "per": "image"}]})]);
        assert_eq!(image[0].price, "$0.04/image");
        let speech = rows(Kind::Speech, &[json!({"id": "c", "provider": "gateway", "prices": [{"usd": 20, "per": "million-characters"}]})]);
        assert_eq!(speech[0].price, "$20/million-characters");
    }

    #[test]
    fn vision_rows_use_the_id_the_api_takes() {
        let entries = [json!({"id": "openrouter/google/x", "api_id": "google/x", "provider": "openrouter", "cost": {"input": 0.1, "output": 0.4}})];
        let row = &rows(Kind::Vision, &entries)[0];
        assert_eq!(row.id, "google/x");
        assert_eq!(row.price, "$0.1/$0.4 per M tokens (in/out)");
    }

    #[test]
    fn cheapest_first_and_unpriced_last() {
        let entries = [
            json!({"id": "free-price-missing", "provider": "fal", "prices": []}),
            json!({"id": "dear", "provider": "fal", "prices": [{"usdPerSecond": 0.5}]}),
            json!({"id": "cheap", "provider": "fal", "prices": [{"usdPerSecond": 0.01}]}),
        ];
        let order: Vec<_> = rows(Kind::Video, &entries).into_iter().map(|r| r.id).collect();
        assert_eq!(order, ["cheap", "dear", "free-price-missing"]);
    }

    #[test]
    fn entries_without_an_id_are_skipped() {
        assert!(rows(Kind::Video, &[json!({"provider": "fal"})]).is_empty());
    }
}
