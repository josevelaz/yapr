use std::cmp::Ordering;
use std::sync::{LazyLock, Mutex};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::settings::{self, Sort};

const MODELS_URL: &str = "https://ai-gateway.vercel.sh/v1/models";
const CACHE_FILE: &str = "models.json";

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Pricing {
    pub input: Option<String>,
    pub output: Option<String>,
    pub audio_input_token_cost: Option<String>,
    pub transcription_duration_cost_per_second: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Model {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub transcription: bool,
    pub tags: Vec<String>,
    pub zdr: Option<String>,
    pub pricing: Pricing,
    pub released: Option<i64>,
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Zdr {
    Full,
    Partial,
    None,
}

pub enum Price {
    Unlisted,
    Free,
    Time {
        per_second: f64,
    },
    Tokens {
        input: Option<f64>,
        output: Option<f64>,
    },
}

static MODELS: LazyLock<Mutex<Vec<Model>>> = LazyLock::new(|| Mutex::new(read_cache()));

pub fn all() -> Vec<Model> {
    MODELS.lock().unwrap().clone()
}

pub fn find(id: &str) -> Option<Model> {
    MODELS.lock().unwrap().iter().find(|m| m.id == id).cloned()
}

pub fn refresh() -> Result<usize, String> {
    let models = fetch()?;
    let count = models.len();
    settings::write_file(CACHE_FILE, &serde_json::to_vec(&models).unwrap())?;
    *MODELS.lock().unwrap() = models;
    Ok(count)
}

fn read_cache() -> Vec<Model> {
    std::fs::read(settings::support_dir().join(CACHE_FILE))
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

pub fn fetch() -> Result<Vec<Model>, String> {
    let mut response = crate::gateway::agent(20)
        .get(MODELS_URL)
        .call()
        .map_err(|e| format!("Could not reach AI Gateway: {e}"))?;
    let status = response.status().as_u16();
    if status != 200 {
        return Err(format!("AI Gateway answered HTTP {status}"));
    }
    let body: Value = response
        .body_mut()
        .with_config()
        .limit(20 * 1024 * 1024)
        .read_json()
        .map_err(|_| "AI Gateway sent an unreadable model list")?;
    let data = body["data"]
        .as_array()
        .ok_or("AI Gateway sent an unreadable model list")?;
    Ok(data.iter().filter_map(parse).collect())
}

fn parse(raw: &Value) -> Option<Model> {
    let transcription = match raw["type"].as_str()? {
        "transcription" => true,
        "language" => false,
        _ => return None,
    };
    let text = |value: &Value| value.as_str().map(str::to_string);
    let pricing = &raw["pricing"];
    let id = text(&raw["id"])?;
    Some(Model {
        name: text(&raw["name"]).unwrap_or_else(|| id.clone()),
        id,
        description: text(&raw["description"]),
        transcription,
        tags: raw["tags"]
            .as_array()
            .map(|tags| tags.iter().filter_map(text).collect())
            .unwrap_or_default(),
        zdr: text(&raw["zdr"]),
        pricing: Pricing {
            input: text(&pricing["input"]),
            output: text(&pricing["output"]),
            audio_input_token_cost: text(&pricing["audio_input_token_cost"]),
            transcription_duration_cost_per_second: text(
                &pricing["transcription_duration_cost_per_second"],
            ),
        },
        released: raw["released"].as_i64(),
    })
}

impl Model {
    pub fn streaming(&self) -> bool {
        self.tags.iter().any(|tag| tag == "websocket-transcription")
    }

    pub fn zdr(&self) -> Zdr {
        match self.zdr.as_deref() {
            Some("all") => Zdr::Full,
            Some("some") => Zdr::Partial,
            _ => Zdr::None,
        }
    }

    pub fn provider(&self) -> &str {
        self.id.split('/').next().unwrap_or(&self.id)
    }

    pub fn price(&self) -> Price {
        let amount = |value: &Option<String>| {
            value
                .as_deref()
                .filter(|s| !s.is_empty())
                .and_then(|s| s.parse::<f64>().ok())
                .filter(|n| n.is_finite())
        };
        let p = &self.pricing;
        let (input, output) = if self.transcription {
            if let Some(per_second) = amount(&p.transcription_duration_cost_per_second) {
                return if per_second > 0.0 {
                    Price::Time { per_second }
                } else {
                    Price::Free
                };
            }
            (
                amount(&p.audio_input_token_cost).or(amount(&p.input)),
                amount(&p.output),
            )
        } else {
            (amount(&p.input), amount(&p.output))
        };
        match (input, output) {
            (None, None) => Price::Unlisted,
            _ if input.unwrap_or(0.0) == 0.0 && output.unwrap_or(0.0) == 0.0 => Price::Free,
            _ => Price::Tokens { input, output },
        }
    }

    pub fn price_text(&self) -> String {
        match self.price() {
            Price::Unlisted => "price not listed".into(),
            Price::Free => "free".into(),
            Price::Time { per_second } => format!("{}/min", usd(per_second * 60.0, 2)),
            Price::Tokens { input, output } => {
                let mut parts = Vec::new();
                if let Some(input) = input {
                    parts.push(format!("{} in", per_million(input)));
                }
                if let Some(output) = output {
                    parts.push(format!("{} out", per_million(output)));
                }
                format!("{} per 1M tokens", parts.join(" / "))
            }
        }
    }

    pub fn price_short(&self) -> String {
        match self.price() {
            Price::Unlisted => "no price".into(),
            Price::Free => "free".into(),
            Price::Time { per_second } => format!("{}/min", usd(per_second * 60.0, 2)),
            Price::Tokens {
                input: Some(input),
                output: Some(output),
            } => format!("{} / {}", per_million(input), per_million(output)),
            Price::Tokens {
                input: Some(input), ..
            } => format!("{} in", per_million(input)),
            Price::Tokens { output, .. } => format!("{} out", per_million(output.unwrap_or(0.0))),
        }
    }

    pub fn menu_title(&self) -> String {
        let mut title = format!("{}  —  {}", self.name, self.price_short());
        if self.transcription && self.streaming() {
            title.push_str("  ·  Live");
        }
        title.push_str(match self.zdr() {
            Zdr::Full => "  ·  ZDR",
            Zdr::Partial => "  ·  Partial ZDR",
            Zdr::None => "  ·  No ZDR",
        });
        title
    }

    pub fn tooltip(&self) -> String {
        let mut lines = vec![self.id.clone(), self.price_text()];
        if self.transcription {
            lines.push(if self.streaming() {
                "Shows live words while you speak".into()
            } else {
                "Transcribes when you stop".into()
            });
        }
        lines.push(
            match self.zdr() {
                Zdr::Full => "Full zero data retention",
                Zdr::Partial => "Partial zero data retention",
                Zdr::None => "No zero data retention: refused by teams that require it",
            }
            .into(),
        );
        if let Some(description) = self.description.as_deref().filter(|d| !d.is_empty()) {
            lines.push(String::new());
            lines.push(description.to_string());
        }
        lines.join("\n")
    }
}

fn per_million(per_token: f64) -> String {
    usd(per_token * 1e6, 3)
}

pub fn usd(value: f64, significant: i32) -> String {
    if value == 0.0 {
        return "$0.00".into();
    }
    let decimals_for = |v: f64| significant - 1 - v.abs().log10().floor() as i32;
    let factor = 10f64.powi(decimals_for(value));
    let rounded = (value * factor).round() / factor;
    let decimals = decimals_for(rounded).max(2) as usize;
    let mut text = format!("{rounded:.decimals$}");
    while text.ends_with('0') && text.len() - text.find('.').unwrap_or(text.len()) > 3 {
        text.pop();
    }
    format!("${text}")
}

fn price_rank(model: &Model) -> (u8, f64) {
    match model.price() {
        Price::Free => (0, 0.0),
        Price::Time { per_second } => (1, per_second),
        Price::Tokens { input, output } => (2, input.unwrap_or(0.0) + output.unwrap_or(0.0)),
        Price::Unlisted => (3, 0.0),
    }
}

pub fn compare(sort: Sort) -> impl Fn(&Model, &Model) -> Ordering {
    move |a, b| {
        a.zdr().cmp(&b.zdr()).then_with(|| match sort {
            Sort::Price => {
                let (rank_a, value_a) = price_rank(a);
                let (rank_b, value_b) = price_rank(b);
                rank_a
                    .cmp(&rank_b)
                    .then(value_a.total_cmp(&value_b))
                    .then_with(|| a.name.cmp(&b.name))
            }
            Sort::Name => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
        })
    }
}
