use crate::{
    bot::modules::{Owner, currency::CurrencySettings},
    core::db::schemas::settings::Settings,
    errors::{BotError, MyError},
    t,
    util::currency_values::WORD_VALUES,
};
use aho_corasick::{AhoCorasick, MatchKind};
use async_trait::async_trait;
use log::error;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    fs,
    sync::Arc,
    time::{Duration, Instant},
};
use thiserror::Error;
use tokio::sync::RwLock;

const CACHE_DURATION_SECS: u64 = 60 * 10;
pub const CURRENCY_CONFIG_PATH: &str = "currencies.json";
const COINBASE_API_URL: &str = "https://api.coinbase.com/v2/exchange-rates?currency=UAH";
const TONAPI_URL: &str = "https://tonapi.io/v2/rates";
const BANNED_CHARACTERS: [char; 6] = ['@', '#', '/', '_', ':', '-'];

#[derive(Debug, PartialEq, Clone)]
pub struct DetectedCurrency {
    amount: f64,
    currency_code: String,
}

#[derive(Debug, Clone)]
struct CachedRates {
    fetched_at: Instant,
    rates: HashMap<String, f64>,
}

#[async_trait]
trait RateProvider: Send + Sync {
    async fn fetch(&self, client: &Client) -> Result<HashMap<String, f64>, ConvertError>;
}

struct CoinbaseProvider;

// Coinbase Provider output
#[derive(Deserialize)]
struct CbResponse {
    data: Cbdata,
}

#[derive(Deserialize)]
struct Cbdata {
    currency: String,
    rates: HashMap<String, String>,
}

#[async_trait]
impl RateProvider for CoinbaseProvider {
    async fn fetch(&self, client: &Client) -> Result<HashMap<String, f64>, ConvertError> {
        let resp = client
            .get(COINBASE_API_URL)
            .send()
            .await?
            .json::<CbResponse>()
            .await?;

        let mut rates = HashMap::new();
        rates.insert(resp.data.currency, 1.0);

        for (code, rate_str) in resp.data.rates {
            if let Ok(rate) = rate_str.parse::<f64>()
                && rate != 0.0
            {
                rates.insert(code, 1.0 / rate);
            }
        }
        Ok(rates)
    }
}

struct TonApiProvider {
    tokens: Vec<String>,
    mapping: HashMap<String, String>,
}

// TonApi response
#[derive(Deserialize)]
struct TonApiResponse {
    rates: HashMap<String, TonApiRate>,
}

#[derive(Deserialize)]
struct TonApiRate {
    prices: HashMap<String, f64>,
}

#[async_trait]
impl RateProvider for TonApiProvider {
    async fn fetch(&self, client: &Client) -> Result<HashMap<String, f64>, ConvertError> {
        if self.tokens.is_empty() {
            return Ok(HashMap::new());
        }

        let tokens = self.tokens.join(",");

        let resp = client
            .get(TONAPI_URL)
            .query(&[("tokens", &tokens), ("currencies", &"uah".to_string())])
            .send()
            .await?
            .json::<TonApiResponse>()
            .await?;

        let mut rates = HashMap::new();
        for (id, rate_entry) in resp.rates {
            if let Some(code) = self.mapping.get(&id) {
                if let Some(price) = rate_entry.prices.get("UAH") {
                    rates.insert(code.clone(), *price);
                } else if let Some(price) = rate_entry.prices.get("uah") {
                    rates.insert(code.clone(), *price);
                }
            } else if let Some(code) = self.mapping.get(&id.to_lowercase())
                && let Some(price) = rate_entry.prices.get("UAH")
            {
                rates.insert(code.clone(), *price);
            }
        }
        Ok(rates)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CurrencyStruct {
    pub code: String,
    pub source: String,
    #[serde(default)]
    pub api_identifier: Option<String>,
    pub symbol: String,
    pub flag: String,
    pub patterns: Vec<String>,
    pub one: String,
    pub few: String,
    pub many: String,
    #[serde(default)]
    pub one_en: String,
    #[serde(default)]
    pub many_en: String,
    pub is_target: bool,
}

struct CurrencyDetector {
    ac: AhoCorasick,
    pattern_map: Vec<String>,
}

impl CurrencyDetector {
    fn new(currencies: &[CurrencyStruct]) -> Self {
        let (patterns, pattern_map): (Vec<_>, Vec<_>) = currencies
            .iter()
            .flat_map(|curr| {
                let symbol_iter = (!curr.symbol.is_empty())
                    .then(|| (curr.symbol.to_lowercase(), curr.code.clone()))
                    .into_iter();
                let patterns_iter = curr
                    .patterns
                    .iter()
                    .map(|p| (p.to_lowercase(), curr.code.clone()));
                symbol_iter.chain(patterns_iter)
            })
            .unzip();

        let ac = AhoCorasick::builder()
            .match_kind(MatchKind::LeftmostLongest)
            .build(&patterns)
            .expect("Failed to build AhoCorasick");
        Self { ac, pattern_map }
    }

    fn detect(&self, text: &str) -> Vec<DetectedCurrency> {
        let text_lower = text.to_lowercase();

        self.ac
            .find_iter(&text_lower)
            .filter_map(|mat| {
                let code = &self.pattern_map[mat.pattern()];
                let (start, end) = (mat.start(), mat.end());
                let match_str = &text_lower[start..end];

                let is_alpha = match_str.chars().any(char::is_alphabetic);
                let char_count = match_str.chars().count();

                if is_alpha {
                    let prev = text_lower[..start].chars().last();
                    let next = text_lower[end..].chars().next();

                    if prev.is_some_and(char::is_alphabetic)
                        || next.is_some_and(char::is_alphabetic)
                    {
                        return None;
                    }

                    if let (Some(p), Some(n)) = (prev, next)
                        && p.is_alphanumeric()
                        && n.is_alphanumeric()
                    {
                        return None;
                    }
                }

                let (left_part, right_part) = (&text[..start], &text[end..]);

                // TODO: should we make this crutch configurable in settings? If yes, this option will be named like "Value depth"
                let amount = if is_alpha && char_count == 1 {
                    Self::find_strict(left_part, right_part)
                } else {
                    let limit = if is_alpha && char_count == 2 { 1 } else { 5 };
                    Self::find_loose(left_part, right_part, limit)
                };

                amount.map(|amount| DetectedCurrency {
                    amount,
                    currency_code: code.clone(),
                })
            })
            .collect()
    }

    fn find_strict(left: &str, right: &str) -> Option<f64> {
        if let Some(token) = left.split_whitespace().last()
            && !left.chars().last().is_some_and(|c| c.is_ascii_digit())
            && !token.starts_with(&BANNED_CHARACTERS[..])
            && let Some(value) = Self::parse_amount(token)
        {
            return Some(value);
        }

        if let Some(token) = right.split_whitespace().next()
            && !right.chars().next().is_some_and(|c| c.is_ascii_digit())
            && !token.starts_with(&BANNED_CHARACTERS[..])
            && let Some(value) = Self::parse_amount(token)
        {
            return Some(value);
        }

        None
    }

    fn find_loose(left: &str, right: &str, limit: usize) -> Option<f64> {
        let left_search = left
            .split_whitespace()
            .rev()
            .take(limit)
            .filter(|token| !token.starts_with(&BANNED_CHARACTERS[..]))
            .find_map(Self::parse_amount);

        if left_search.is_some() {
            return left_search;
        }

        right
            .split_whitespace()
            .take(limit)
            .filter(|token| !token.starts_with(&BANNED_CHARACTERS[..]))
            .find_map(Self::parse_amount)
    }

    fn parse_amount(s: &str) -> Option<f64> {
        let s_clean = s.replace(',', ".").replace(['_', ' '], "").to_lowercase();
        if s_clean.is_empty() {
            return None;
        }

        let mut total = 0.0;
        let mut rest = s_clean.as_str();

        if let Some(c) = rest.chars().next()
            && !c.is_ascii_digit()
            && matches!(c, '.' | ',')
        {
            return None;
        }

        while !rest.is_empty() {
            let digit_end = rest
                .find(|c: char| !c.is_ascii_digit() && c != '.')
                .unwrap_or(rest.len());

            let (num_s, remainder) = rest.split_at(digit_end);
            if num_s.is_empty() {
                return None;
            }

            let value = num_s.parse::<f64>().ok()?;
            rest = remainder;

            let suffix_end = rest
                .find(|c: char| c.is_ascii_digit() || c == '.')
                .unwrap_or(rest.len());
            let (suffix, remainder) = rest.split_at(suffix_end);
            if !suffix.is_empty() {
                let info = WORD_VALUES.get(suffix)?;
                if !info.is_multiplier {
                    return None;
                }
                total += value * info.value;
            } else {
                total += value
            }
            rest = remainder;
        }

        if total > 0.0 || s_clean.contains('0') {
            Some(total)
        } else {
            None
        }
    }
}

pub struct CurrencyConverter {
    client: Client,
    cache: Arc<RwLock<Option<CachedRates>>>,
    currencies: HashMap<String, Arc<CurrencyStruct>>,
    detector: Arc<CurrencyDetector>,
    providers: Vec<Box<dyn RateProvider>>,
}

#[derive(Error, Debug)]
pub enum ConvertError {
    #[error("Network request failed: {0}")]
    RequestError(#[from] reqwest::Error),
    #[error("Failed to parse JSON response: {0}")]
    ParseError(#[from] serde_json::Error),
    #[error("Currency '{0}' not found in the configuration")]
    CurrencyNotFound(String),
    #[error("Rate for '{0}' not found")]
    RateNotFound(String),
    #[error("Failed to read config file: {0}")]
    ConfigError(String),
    #[error("No rates could be fetched")]
    NoRatesFetched,
}

impl CurrencyConverter {
    pub fn new() -> Result<Self, ConvertError> {
        let content = fs::read_to_string(CURRENCY_CONFIG_PATH)
            .map_err(|e| ConvertError::ConfigError(e.to_string()))?;
        let currency_list: Vec<CurrencyStruct> = serde_json::from_str(&content)?;
        let (mut currencies, mut ton_tokens, mut ton_mapping) =
            (HashMap::new(), Vec::new(), HashMap::new());

        for curr in &currency_list {
            currencies.insert(curr.code.clone(), Arc::new(curr.clone()));

            if curr.source == "tonapi"
                && let Some(id) = &curr.api_identifier
            {
                ton_tokens.push(id.clone());
                ton_mapping.insert(id.clone(), curr.code.clone());
                ton_mapping.insert(id.to_lowercase(), curr.code.clone());
            }
        }

        let detector = Arc::new(CurrencyDetector::new(&currency_list));
        let client = Client::builder().timeout(Duration::from_secs(10)).build()?;

        Ok(Self {
            client: client.clone(),
            cache: Arc::new(RwLock::new(None)),
            currencies,
            detector,
            providers: vec![
                Box::new(CoinbaseProvider),
                Box::new(TonApiProvider {
                    tokens: ton_tokens,
                    mapping: ton_mapping,
                }),
            ],
        })
    }

    async fn get_rates(&self) -> Result<HashMap<String, f64>, ConvertError> {
        {
            let read_guard = self.cache.read().await;
            if let Some(cached) = &*read_guard
                && cached.fetched_at.elapsed() < Duration::from_secs(CACHE_DURATION_SECS)
            {
                return Ok(cached.rates.clone());
            }
        }

        let mut combined_rates = HashMap::new();
        let futures: Vec<_> = self
            .providers
            .iter()
            .map(|p| p.fetch(&self.client))
            .collect();

        let results = futures::future::join_all(futures).await;

        for res in results {
            match res {
                Ok(rates) => combined_rates.extend(rates),
                Err(e) => error!("Provider error: {}", e),
            }
        }

        if combined_rates.is_empty() {
            return Err(ConvertError::NoRatesFetched);
        }

        let mut write_guard = self.cache.write().await;
        *write_guard = Some(CachedRates {
            fetched_at: Instant::now(),
            rates: combined_rates.clone(),
        });

        Ok(combined_rates)
    }

    pub async fn process_text(
        &self,
        text: &str,
        owner: &Owner,
        locale: &str,
    ) -> Result<Vec<String>, ConvertError> {
        let settings: CurrencySettings = Settings::get_module_settings(owner, "currency")
            .await
            .map_err(|e| ConvertError::ConfigError(e.to_string()))?;

        if !settings.enabled || settings.selected_codes.is_empty() {
            return Ok(Vec::new());
        }

        let detected = self.detector.detect(text);
        if detected.is_empty() {
            return Ok(Vec::new());
        }

        let rates = self.get_rates().await?;
        let mut results = Vec::new();

        for item in detected {
            if let Some(res) =
                self.format_conversion(&item, &rates, &settings.selected_codes, locale)
            {
                results.push(res);
            }
        }

        Ok(results)
    }

    fn format_conversion(
        &self,
        item: &DetectedCurrency,
        rates: &HashMap<String, f64>,
        targets: &[String],
        locale: &str,
    ) -> Option<String> {
        let info = self.currencies.get(&item.currency_code)?;

        let from_rate = rates.get(&item.currency_code)?;
        let mut builder = String::new();

        let base_word = self.get_plural(item.amount, info, locale);
        let amount_str = Self::format_value(item.amount);

        builder.push_str(&format!(
            "{} {}{} {}\n\n",
            info.flag, amount_str, info.symbol, base_word
        ));

        for target_code in targets {
            if target_code == &item.currency_code {
                continue;
            }

            if let (Some(target_info), Some(to_rate)) =
                (self.currencies.get(target_code), rates.get(target_code))
            {
                let converted = item.amount * from_rate / to_rate;
                let target_word = self.get_plural(converted, target_info, locale);
                let converted = Self::format_value(converted);

                builder.push_str(&format!(
                    "{} {}{} {}\n",
                    target_info.flag, converted, target_info.symbol, target_word
                ));
            }
        }

        if builder.is_empty() {
            None
        } else {
            Some(builder.trim_end().to_string())
        }
    }

    fn format_value(val: f64) -> String {
        if val == 0.0 {
            return "0.00".to_string();
        }
        if val < 0.01 {
            format!("{:.14}", val)
                .trim_end_matches('0')
                .trim_end_matches('.')
                .to_string()
        } else {
            format!("{:.2}", val)
        }
    }

    fn get_plural(&self, amount: f64, info: &CurrencyStruct, locale: &str) -> String {
        let suffix = if matches!(locale, "ru" | "uk" | "be") {
            let n = amount.trunc() as u64;
            let n100 = n % 100;
            let n10 = n % 10;

            if (11..=19).contains(&n100) {
                "many"
            } else if n10 == 1 {
                "one"
            } else if (2..=4).contains(&n10) {
                "few"
            } else {
                "many"
            }
        } else if (amount - 1.0).abs() < f64::EPSILON {
            "one"
        } else {
            "many"
        };

        let key = format!("currencies.{}.{}", info.code, suffix);

        let translated = t!(&key, locale = locale);

        if translated == key || translated.is_empty() {
            if locale == "en" {
                match suffix {
                    "one" => {
                        if !info.one_en.is_empty() {
                            info.one_en.clone()
                        } else {
                            info.one.clone()
                        }
                    }
                    _ => {
                        if !info.many_en.is_empty() {
                            info.many_en.clone()
                        } else {
                            info.many.clone()
                        }
                    }
                }
            } else {
                match suffix {
                    "one" => info.one.clone(),
                    "few" => info.few.clone(),
                    _ => info.many.clone(),
                }
            }
        } else {
            translated
        }
    }
}

pub fn get_all_currency_codes(path: String) -> Result<Vec<CurrencyStruct>, ConvertError> {
    let content = fs::read_to_string(path).map_err(|e| ConvertError::ConfigError(e.to_string()))?;
    let list: Vec<CurrencyStruct> = serde_json::from_str(&content)?;
    Ok(list)
}

pub fn get_default_currencies() -> Result<Vec<CurrencyStruct>, MyError> {
    let all = get_all_currency_codes(CURRENCY_CONFIG_PATH.to_string())
        .map_err(|e| BotError::Other(e.to_string()))?;

    let defaults = all
        .into_iter()
        .filter(|c| {
            ["uah", "rub", "usd", "byn", "eur", "ton"].contains(&c.code.to_lowercase().as_str())
        })
        .collect();
    Ok(defaults)
}
