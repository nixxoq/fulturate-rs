use crate::{
    bot::modules::{Owner, currency::CurrencySettings},
    core::db::schemas::settings::Settings,
    errors::{BotError, MyError},
    t,
    util::currency_values::WORD_VALUES,
};
use aho_corasick::{AhoCorasick, MatchKind};
use async_trait::async_trait;
use eidolon_lang::interpreter::evaluate;
use log::error;
use once_cell::sync::Lazy;
use regex::Regex;
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
const RANGE_CHARACTERS: [char; 4] = ['-', '–', '—', '…'];
const MATH_CHARS: [char; 9] = ['.', ',', '+', '-', '*', '/', '^', '(', ')'];

static RANGE_LEFT_REGEX: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?i)(?P<num>\d+(?:[.,]\d+)?[\p{Zs}\t]*(?:[kmbtкмбт]+)?)[\p{Zs}\t]*(?:-|–|—|\.\.|to|до)[\p{Zs}\t]*$")
        .unwrap()
});

static RANGE_RIGHT_REGEX: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?i)^[\p{Zs}\t]*(?:-|–|—|\.\.|to|до)[\p{Zs}\t]*(?P<num>\d+(?:[.,]\d+)?[\p{Zs}\t]*(?:[kmbtкмбт]+)?)")
        .unwrap()
});

static MATH_EXPR_LEFT_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?i)(?P<expr>[\d.,\p{Zs}\tkmbtкмбт+\-*/^()]+)$").unwrap());

static MATH_EXPR_RIGHT_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?i)^(?P<expr>[\d.,\p{Zs}\tkmbtкмбт+\-*/^()]+)").unwrap());

#[derive(Debug, PartialEq, Clone)]
pub struct DetectedCurrency {
    amount: f64,
    currency_code: String,
    start: usize,
    end: usize,
    match_start: usize,
    match_end: usize,
}

#[derive(Debug)]
enum ProcessingItem {
    Single(DetectedCurrency),
    Range {
        min: f64,
        max: f64,
        currency_code: String,
        start: usize,
    },
}

impl ProcessingItem {
    fn start(&self) -> usize {
        match self {
            Self::Single(d) => d.start,
            Self::Range { start, .. } => *start,
        }
    }
}

struct FmtConfig<'a> {
    targets: &'a [String],
    locale: &'a str,
    precision: Option<u8>,
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
            let code_opt = self
                .mapping
                .get(&id)
                .or_else(|| self.mapping.get(&id.to_lowercase()));
            if let Some(code) = code_opt
                && let Some(price) = rate_entry
                    .prices
                    .get("UAH")
                    .or_else(|| rate_entry.prices.get("uah"))
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
        let mut results = Vec::new();

        for mat in self.ac.find_iter(&text_lower) {
            let code = &self.pattern_map[mat.pattern()];
            let (start, end) = (mat.start(), mat.end());
            let match_str = &text_lower[start..end];
            let is_alpha = match_str.chars().any(char::is_alphabetic);

            if is_alpha {
                let prev = text_lower[..start].chars().last();
                let next = text_lower[end..].chars().next();
                if prev.is_some_and(char::is_alphabetic) || next.is_some_and(char::is_alphabetic) {
                    continue;
                }
                if let (Some(p), Some(n)) = (prev, next)
                    && p.is_alphanumeric()
                    && n.is_alphanumeric()
                {
                    continue;
                }
            }

            let (left, right) = (&text[..start], &text[end..]);

            if !is_alpha
                && match_str.chars().count() == 1
                && let Some(caps) = MATH_EXPR_RIGHT_RE.captures(right)
                && let Some(m) = caps.name("expr")
                && let Some(amount) = Self::parse_complex_num(m.as_str())
            {
                results.push(DetectedCurrency {
                    amount,
                    currency_code: code.clone(),
                    start,
                    end: end + m.end(),
                    match_start: start,
                    match_end: end,
                });
                continue;
            }

            if let Some(caps) = MATH_EXPR_LEFT_RE.captures(left)
                && let Some(m) = caps.name("expr")
            {
                let expr_str = m.as_str();
                let is_range = expr_str
                    .chars()
                    .filter(|&c| matches!(c, '-' | '–' | '—'))
                    .count()
                    == 1
                    && !expr_str
                        .chars()
                        .any(|c| matches!(c, '+' | '*' | '/' | '^' | '(' | ')'));

                if !is_range && let Some(amount) = Self::parse_complex_num(expr_str) {
                    results.push(DetectedCurrency {
                        amount,
                        currency_code: code.clone(),
                        start: start - (left.len() - m.start()),
                        end,
                        match_start: start,
                        match_end: end,
                    });
                    continue;
                }
            }

            if is_alpha
                && match_str.chars().count() == 1
                && text[end..]
                    .chars()
                    .next()
                    .is_some_and(|c| c.is_ascii_digit())
            {
                continue;
            }

            // FIXME: ну типа оно ж не умное и не умеет анализировать контекст да
            // (ну это надо или лексер хуярить, который по токенам обрабатывает и ищет возможную связку в словах)
            // поэтому ебейший костыль, который ток шо придумал
            // если это не ASCII типа $, €, £, то на похуях +5 слов для проверки на значения
            // если это типа USD, EUR, RUB - то до 2-х слов
            // иначе хуй
            let limit = if !is_alpha {
                5
            } else if match_str.eq_ignore_ascii_case(code) {
                2
            } else {
                0
            };

            if let Some((amount, f_start, f_end)) =
                Self::find_amount(left, right, limit, start, end, !is_alpha)
            {
                results.push(DetectedCurrency {
                    amount,
                    currency_code: code.clone(),
                    start: f_start,
                    end: f_end,
                    match_start: start,
                    match_end: end,
                });
            }
        }
        results
    }

    fn find_amount(
        left: &str,
        right: &str,
        limit: usize,
        s_idx: usize,
        e_idx: usize,
        is_symbol: bool,
    ) -> Option<(f64, usize, usize)> {
        let parse_right = |token: &str| -> Option<(f64, usize, usize)> {
            let part = token.split(RANGE_CHARACTERS).find(|s| !s.is_empty())?;
            let val = Self::parse_num(part)?;

            let p_start = right.find(part)?;
            let p_end = p_start + part.len();
            let rem = &right[p_end..];

            rem.split_whitespace()
                .next()
                .filter(|n| n.len() == 3 && n.chars().all(|c| c.is_ascii_digit()))
                .and_then(|n| {
                    let combined_val = Self::parse_num(&format!("{part}{n}"))?;
                    let c_end = e_idx + p_end + rem.find(n)? + n.len();
                    Some((combined_val, s_idx, c_end))
                })
                .or(Some((val, s_idx, e_idx + p_end)))
        };

        if is_symbol
            && let Some(token) = right.split_whitespace().next()
            && !token.starts_with(&BANNED_CHARACTERS[..])
            && let Some(res) = parse_right(token)
        {
            return Some(res);
        }

        if let Some(token) = left.split_whitespace().last()
            && !token.starts_with(&BANNED_CHARACTERS[..])
            && let Some(part) = token
                .split(RANGE_CHARACTERS)
                .filter(|s| !s.is_empty())
                .next_back()
            && let Some(val) = Self::parse_num(part)
        {
            return Some((val, left.rfind(part)?, e_idx));
        }

        for token in right.split_whitespace().take(limit) {
            if token.starts_with(&BANNED_CHARACTERS[..]) {
                continue;
            }

            if let Some(res) = parse_right(token) {
                return Some(res);
            }

            if token.chars().any(|c| c.is_alphabetic()) {
                break;
            }
        }

        None
    }

    fn parse_complex_num(s: &str) -> Option<f64> {
        let mut s_clean = s.trim().to_lowercase();
        if s_clean.is_empty() {
            return None;
        }

        for (word, info) in WORD_VALUES.entries() {
            if s_clean.contains(word) {
                s_clean = s_clean.replace(word, &format!("*({})", info.value));
            }
        }

        let s_sanitized: String = s_clean
            .chars()
            .filter(|&c| c.is_ascii_digit() || MATH_CHARS.contains(&c))
            .collect();

        if let Ok(ast) = eidolon_lang::parse_eidolon_source(&s_sanitized)
            && let Ok(eval_res) = evaluate(&ast, &HashMap::new())
            && let Ok(val) = eval_res.as_number(0)
        {
            return Some(val);
        }

        Self::parse_num(s)
    }

    fn parse_num(s: &str) -> Option<f64> {
        let trimmed = s.trim();
        let bytes = trimmed.as_bytes();

        if matches!(
            (bytes.first(), bytes.last()),
            (Some(b'('), Some(b')')) | (Some(b'['), Some(b']')) | (Some(b'{'), Some(b'}'))
        ) {
            return None;
        }

        let s_clean = trimmed
            .trim_start_matches(['(', '[', '{'])
            .replace(',', ".")
            .replace(['_', ' '], "")
            .to_lowercase();

        if !s_clean.starts_with(|c: char| c.is_ascii_digit() || c == '.') {
            return None;
        }

        let mut rest = s_clean.as_str();
        let total = std::iter::from_fn(|| {
            if rest.is_empty() {
                return None;
            }

            let digit_end = rest
                .find(|c: char| !c.is_ascii_digit() && c != '.')
                .unwrap_or(rest.len());

            let (num_s, rem) = rest.split_at(digit_end);

            if num_s.is_empty() {
                return None;
            }

            let Ok(value) = num_s.parse::<f64>() else {
                return Some(Err(()));
            };

            let suffix_end = rem
                .find(|c: char| c.is_ascii_digit() || c == '.')
                .unwrap_or(rem.len());

            let (suffix, next_rest) = rem.split_at(suffix_end);

            if suffix.starts_with(RANGE_CHARACTERS) || suffix.starts_with("..") {
                rest = "";
                return Some(Ok(value));
            }

            let clean_suffix = suffix.trim_end_matches([')', ']']);

            if clean_suffix.is_empty() {
                rest = next_rest;
                return Some(Ok(value));
            }

            if !clean_suffix.starts_with(char::is_alphabetic) {
                rest = "";
                return Some(Ok(value));
            }

            match WORD_VALUES.get(clean_suffix).filter(|i| i.is_multiplier) {
                Some(info) => {
                    rest = next_rest;
                    Some(Ok(value * info.value))
                }
                None => Some(Err(())),
            }
        })
        .try_fold(0.0, |acc, res| res.map(|val| acc + val))
        .ok()?;

        (total > 0.0 || s_clean.contains('0')).then_some(total)
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
    #[error("Network error: {0}")]
    RequestError(#[from] reqwest::Error),
    #[error("JSON error: {0}")]
    ParseError(#[from] serde_json::Error),
    #[error("Currency {0} not found")]
    CurrencyNotFound(String),
    #[error("No rates fetched")]
    NoRatesFetched,
    #[error("Config error: {0}")]
    ConfigError(String),
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

        Ok(Self {
            client: Client::builder().timeout(Duration::from_secs(10)).build()?,
            cache: Arc::new(RwLock::new(None)),
            currencies,
            detector: Arc::new(CurrencyDetector::new(&currency_list)),
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
        let results =
            futures::future::join_all(self.providers.iter().map(|p| p.fetch(&self.client))).await;

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
        if text.trim_start().starts_with('/') {
            return Ok(Vec::new());
        }
        let settings: CurrencySettings = Settings::get_module_settings(owner, "currency")
            .await
            .map_err(|e| ConvertError::ConfigError(e.to_string()))?;
        if !settings.enabled || settings.selected_codes.is_empty() {
            return Ok(Vec::new());
        }

        let mut detected = self.detector.detect(text);
        if detected.is_empty() {
            return Ok(Vec::new());
        }

        detected.sort_by(|a, b| {
            (b.end - b.start)
                .cmp(&(a.end - a.start))
                .then_with(|| a.start.cmp(&b.start))
        });

        let mut filtered = Vec::new();
        for item in detected {
            if !filtered
                .iter()
                .any(|f: &DetectedCurrency| item.start < f.end && f.start < item.end)
            {
                filtered.push(item);
            }
        }
        let (mut detected, rates) = (filtered, self.get_rates().await?);
        let (mut items, mut handled_indices) = (Vec::new(), Vec::new());

        detected.sort_by(|a, b| a.start.cmp(&b.start));
        for (i, item) in detected.iter().enumerate() {
            if handled_indices.contains(&i) {
                continue;
            }
            let left = &text[..item.start];
            if let Some(caps) = RANGE_LEFT_REGEX.captures(left)
                && let Some(m) = caps.name("num")
                && let Some(min_val) = CurrencyDetector::parse_num(m.as_str())
            {
                items.push(ProcessingItem::Range {
                    min: min_val,
                    max: item.amount,
                    currency_code: item.currency_code.clone(),
                    start: item.start - (left.len() - m.start()),
                });
                handled_indices.push(i);
                continue;
            }
            let right = &text[item.end..];
            if let Some(caps) = RANGE_RIGHT_REGEX.captures(right)
                && let Some(m) = caps.name("num")
                && let Some(max_val) = CurrencyDetector::parse_num(m.as_str())
            {
                items.push(ProcessingItem::Range {
                    min: item.amount,
                    max: max_val,
                    currency_code: item.currency_code.clone(),
                    start: item.start,
                });
                handled_indices.push(i);
                continue;
            }
            items.push(ProcessingItem::Single(item.clone()));
        }

        let math_chars = ['+', '-', '*', '/', '(', ')', '^'];
        if !items
            .iter()
            .any(|i| matches!(i, ProcessingItem::Range { .. }))
            && text.chars().any(|c| math_chars.contains(&c))
        {
            let mut expr = text.to_string();
            let mut d_math = detected.clone();
            d_math.sort_by(|a, b| b.start.cmp(&a.start));
            let mut is_math = true;
            for item in &d_math {
                if let Some(rate) = rates.get(&item.currency_code) {
                    expr.replace_range(item.start..item.end, &format!("({})", item.amount * rate));
                } else {
                    is_math = false;
                }
            }

            for (word, info) in WORD_VALUES.entries() {
                if expr.contains(word) {
                    expr = expr.replace(word, &format!("*({})", info.value));
                }
            }

            let s_sanitized: String = expr
                .chars()
                .filter(|&c| c.is_ascii_digit() || MATH_CHARS.contains(&c))
                .collect();

            if is_math
                && let Ok(ast) = eidolon_lang::parse_eidolon_source(&s_sanitized)
                && let Ok(res) = evaluate(&ast, &HashMap::new())
                && let Ok(val) = res.as_number(0)
                && let Some(r) = self.format_math_result(
                    text,
                    val,
                    "UAH",
                    &rates,
                    FmtConfig {
                        targets: &settings.selected_codes,
                        locale,
                        precision: settings.round_digits,
                    },
                )
            {
                return Ok(vec![r]);
            }
        }

        let mut results = Vec::new();
        items.sort_by_key(|a| a.start());
        for item in items {
            if let Some(res) = self.format_conversion_item(
                &item,
                &rates,
                &settings.selected_codes,
                locale,
                settings.round_digits,
            ) {
                results.push(res);
            }
        }
        Ok(results)
    }

    fn format_math_result(
        &self,
        expression: &str,
        amount: f64,
        from_code: &str,
        rates: &HashMap<String, f64>,
        config: FmtConfig,
    ) -> Option<String> {
        let from_rate = rates.get(from_code)?;
        let mut builder = format!("{}:\n\n", expression);
        for target_code in config.targets {
            if let (Some(info), Some(to_rate)) =
                (self.currencies.get(target_code), rates.get(target_code))
            {
                let converted = amount * from_rate / to_rate;
                builder.push_str(&format!(
                    "{} {}{} {}\n",
                    info.flag,
                    Self::format_value(converted, config.precision),
                    info.symbol,
                    self.get_plural(converted, info, config.locale)
                ));
            }
        }
        if builder.lines().count() > 2 {
            Some(builder)
        } else {
            None
        }
    }

    fn format_conversion_item(
        &self,
        item: &ProcessingItem,
        rates: &HashMap<String, f64>,
        targets: &[String],
        locale: &str,
        precision: Option<u8>,
    ) -> Option<String> {
        match item {
            ProcessingItem::Single(d) => {
                self.format_conversion(d, rates, targets, locale, precision)
            }
            ProcessingItem::Range {
                min,
                max,
                currency_code,
                ..
            } => {
                let info = self.currencies.get(currency_code)?;
                let from_rate = rates.get(currency_code)?;
                let mut builder = format!(
                    "{} {} – {}{} {}\n\n",
                    info.flag,
                    Self::format_value(*min, precision),
                    Self::format_value(*max, precision),
                    info.symbol,
                    self.get_plural(*max, info, locale)
                );
                for target_code in targets {
                    if target_code == currency_code {
                        continue;
                    }
                    if let (Some(target_info), Some(to_rate)) =
                        (self.currencies.get(target_code), rates.get(target_code))
                    {
                        let (min_c, max_c) = (min * from_rate / to_rate, max * from_rate / to_rate);
                        builder.push_str(&format!(
                            "{} {} – {}{} {}\n",
                            target_info.flag,
                            Self::format_value(min_c, precision),
                            Self::format_value(max_c, precision),
                            target_info.symbol,
                            self.get_plural(max_c, target_info, locale)
                        ));
                    }
                }
                if builder.lines().count() > 1 {
                    Some(builder.trim_end().to_string())
                } else {
                    None
                }
            }
        }
    }

    fn format_conversion(
        &self,
        item: &DetectedCurrency,
        rates: &HashMap<String, f64>,
        targets: &[String],
        locale: &str,
        precision: Option<u8>,
    ) -> Option<String> {
        let info = self.currencies.get(&item.currency_code)?;
        let from_rate = rates.get(&item.currency_code)?;
        let mut builder = format!(
            "{} {}{} {}\n\n",
            info.flag,
            Self::format_value(item.amount, precision),
            info.symbol,
            self.get_plural(item.amount, info, locale)
        );
        for target_code in targets {
            if target_code == &item.currency_code {
                continue;
            }
            if let (Some(target_info), Some(to_rate)) =
                (self.currencies.get(target_code), rates.get(target_code))
            {
                let conv = item.amount * from_rate / to_rate;
                builder.push_str(&format!(
                    "{} {}{} {}\n",
                    target_info.flag,
                    Self::format_value(conv, precision),
                    target_info.symbol,
                    self.get_plural(conv, target_info, locale)
                ));
            }
        }

        if builder.lines().count() > 1 {
            Some(builder.trim_end().to_string())
        } else {
            None
        }
    }

    fn format_value(val: f64, precision: Option<u8>) -> String {
        if val == 0.0 {
            return "0.00".to_string();
        }
        let (decimals, trim_zeros) = if let Some(p) = precision {
            (p as usize, false)
        } else if val < 0.001 {
            (8, true)
        } else if val < 0.01 {
            (6, true)
        } else if val < 1.0 {
            (4, true)
        } else {
            (2, false)
        };
        let s = format!("{:.prec$}", val, prec = decimals);
        if trim_zeros {
            s.trim_end_matches('0').trim_end_matches('.').to_string()
        } else {
            s
        }
    }

    fn get_plural(&self, amount: f64, info: &CurrencyStruct, locale: &str) -> String {
        let suffix = if matches!(locale, "ru" | "uk" | "be") {
            let (_n, n100, n10) = (
                amount.trunc() as u64,
                (amount.trunc() as u64) % 100,
                (amount.trunc() as u64) % 10,
            );
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
        let trans = t!(&key, locale = locale);
        if trans == key || trans.is_empty() {
            if locale == "en" {
                if suffix == "one" && !info.one_en.is_empty() {
                    info.one_en.clone()
                } else if !info.many_en.is_empty() {
                    info.many_en.clone()
                } else {
                    info.many.clone()
                }
            } else {
                match suffix {
                    "one" => info.one.clone(),
                    "few" => info.few.clone(),
                    _ => info.many.clone(),
                }
            }
        } else {
            trans
        }
    }
}

pub fn get_all_currency_codes(path: String) -> Result<Vec<CurrencyStruct>, ConvertError> {
    let content = fs::read_to_string(path).map_err(|e| ConvertError::ConfigError(e.to_string()))?;
    Ok(serde_json::from_str(&content)?)
}

pub fn get_default_currencies() -> Result<Vec<CurrencyStruct>, MyError> {
    let all = get_all_currency_codes(CURRENCY_CONFIG_PATH.to_string())
        .map_err(|e| BotError::Other(e.to_string()))?;

    Ok(all
        .into_iter()
        .filter(|c| {
            ["uah", "rub", "usd", "byn", "eur", "ton"].contains(&c.code.to_lowercase().as_str())
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn multiplier() {
        assert_eq!(CurrencyDetector::parse_num("5k"), Some(5_000.0));
        assert_eq!(CurrencyDetector::parse_num("2m"), Some(2_000_000.0));
    }

    #[test]
    #[allow(clippy::approx_constant)]
    fn parsing_test() {
        assert_eq!(CurrencyDetector::parse_num("(5801108895304779062)"), None);
        assert_eq!(CurrencyDetector::parse_num("(12345)"), None);
        assert_eq!(CurrencyDetector::parse_num("12345"), Some(12345.0));

        assert_eq!(CurrencyDetector::parse_num("100"), Some(100.0));
        assert_eq!(CurrencyDetector::parse_num("3.14"), Some(3.14));
        assert_eq!(CurrencyDetector::parse_num("1,5"), Some(1.5));

        assert_eq!(CurrencyDetector::parse_num("95б"), Some(95_000_000_000.0));
        assert_eq!(CurrencyDetector::parse_num("95~б"), Some(95.0));
    }

    #[test]
    fn value_auto_precision() {
        assert_eq!(CurrencyConverter::format_value(100.0, None), "100.00");
        assert_eq!(CurrencyConverter::format_value(1.50, None), "1.50");
        assert_eq!(CurrencyConverter::format_value(0.0672, None), "0.0672");
        assert_eq!(CurrencyConverter::format_value(0.05, None), "0.05");
        assert_eq!(CurrencyConverter::format_value(0.000123, None), "0.000123");
    }

    #[test]
    fn value_precision() {
        assert_eq!(CurrencyConverter::format_value(0.0672, Some(2)), "0.07");
        assert_eq!(CurrencyConverter::format_value(0.0672, Some(4)), "0.0672");
        assert_eq!(CurrencyConverter::format_value(100.0, Some(4)), "100.0000");
    }

    fn make_detector() -> CurrencyDetector {
        let content = std::fs::read_to_string(CURRENCY_CONFIG_PATH).expect("currencies.json");
        let currencies: Vec<CurrencyStruct> = serde_json::from_str(&content).expect("parse");
        CurrencyDetector::new(&currencies)
    }

    #[test]
    fn wrong_context() {
        let d = make_detector();
        let text = "Google is not releasing the Android 17 beta today";
        let results = d.detect(text);
        assert!(
            results.iter().all(|r| r.currency_code != "NOT"),
            "NOT coin should not fire in sentence: {:?}",
            results
        );
    }

    #[test]
    fn throw_wrong_id() {
        let d = make_detector();
        let text = "\u{1F48E} Price: 50\n\n\u{2116} 22 (5801108895304779062)";
        let results = d.detect(text);
        for r in &results {
            assert!(
                r.amount < 1_000_000_000_000.0,
                "Anomalously large amount: {:?}",
                r
            );
        }
    }

    #[test]
    fn space_separated() {
        let d = make_detector();
        let results = d.detect("$562 800");
        assert!(
            results
                .iter()
                .any(|r| r.currency_code == "USD" && (r.amount - 562800.0).abs() < 1.0),
            "Expected USD 562800, got: {:?}",
            results
        );
    }
}
