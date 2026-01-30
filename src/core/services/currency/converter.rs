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
use log::{error, warn};
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

static RANGE_LEFT_REGEX: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?i)(?P<num>\d+(?:[.,]\d+)?\s*(?:[kmbtкмбт]+)?)\s*(?:-|–|—|\.\.|to|до)\s*$")
        .unwrap()
});

static RANGE_RIGHT_REGEX: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?i)^\s*(?:-|–|—|\.\.|to|до)\s*(?P<num>\d+(?:[.,]\d+)?\s*(?:[kmbtкмбт]+)?)")
        .unwrap()
});

static MATH_AMOUNT_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?i)(?P<expr>[\d.,\skmbtкмбт+\-*/^()]+)$").unwrap());

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
            let char_count = match_str.chars().count();

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

            let (left_part, right_part) = (&text[..start], &text[end..]);

            if let Some(caps) = MATH_AMOUNT_RE.captures(left_part)
                && let Some(m) = caps.name("expr")
            {
                let expr_str = m.as_str();
                if let Some(amount) = Self::parse_complex_amount(expr_str) {
                    results.push(DetectedCurrency {
                        amount,
                        currency_code: code.clone(),
                        start: start - (left_part.len() - m.start()),
                        end,
                        match_start: start,
                        match_end: end,
                    });
                    continue;
                }
            }

            let amount_data = if is_alpha && char_count == 1 {
                Self::find_strict_with_range(left_part, right_part, start, end)
            } else {
                let limit = if is_alpha && char_count == 2 { 1 } else { 5 };
                Self::find_loose_with_range(left_part, right_part, limit, start, end)
            };

            if let Some((amount, f_start, f_end)) = amount_data {
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

    fn parse_complex_amount(s: &str) -> Option<f64> {
        let mut s_clean = s.trim().to_lowercase();
        if s_clean.is_empty() {
            return None;
        }

        for (word, info) in WORD_VALUES.entries() {
            if s_clean.contains(word) {
                let repl = format!("*({})", info.value);
                s_clean = s_clean.replace(word, &repl);
            }
        }

        if let Ok(ast) = eidolon_lang::parse_eidolon_source(&s_clean)
            && let Ok(eval_res) = evaluate(&ast, &HashMap::new())
            && let Ok(val) = eval_res.as_number(0)
        {
            return Some(val);
        }

        Self::parse_amount(s)
    }

    fn find_strict_with_range(
        left: &str,
        right: &str,
        s_idx: usize,
        e_idx: usize,
    ) -> Option<(f64, usize, usize)> {
        if let Some(token) = left.split_whitespace().last() {
            let is_glued = left.chars().last().is_some_and(|c| c.is_ascii_digit());

            if !is_glued && !token.starts_with(&BANNED_CHARACTERS[..]) {
                let parts: Vec<&str> = token
                    .split(['-', '–', '—', '…'])
                    .filter(|s| !s.is_empty())
                    .collect();
                if let Some(target_part) = parts.last()
                    && let Some(value) = Self::parse_amount(target_part)
                {
                    let token_pos = left.rfind(token).unwrap_or(0);
                    let part_pos = token.rfind(target_part).unwrap_or(0);
                    return Some((value, token_pos + part_pos, e_idx));
                }
            }
        }

        if let Some(token) = right.split_whitespace().next() {
            let is_glued = right.chars().next().is_some_and(|c| c.is_ascii_digit());

            if !is_glued && !token.starts_with(&BANNED_CHARACTERS[..]) {
                let parts: Vec<&str> = token
                    .split(['-', '–', '—', '…'])
                    .filter(|s| !s.is_empty())
                    .collect();
                if let Some(target_part) = parts.first()
                    && let Some(value) = Self::parse_amount(target_part)
                {
                    let token_pos = right.find(token).unwrap_or(0);
                    let part_pos = token.find(target_part).unwrap_or(0);
                    return Some((
                        value,
                        s_idx,
                        e_idx + token_pos + part_pos + target_part.len(),
                    ));
                }
            }
        }
        None
    }

    fn find_loose_with_range(
        left: &str,
        right: &str,
        limit: usize,
        s_idx: usize,
        e_idx: usize,
    ) -> Option<(f64, usize, usize)> {
        let left_tokens: Vec<&str> = left.split_whitespace().rev().take(limit).collect();
        for token in left_tokens {
            if !token.starts_with(&BANNED_CHARACTERS[..]) {
                let parts: Vec<&str> = token
                    .split(['-', '–', '—', '…'])
                    .filter(|s| !s.is_empty())
                    .collect();
                if let Some(target_part) = parts.last()
                    && let Some(val) = Self::parse_amount(target_part)
                {
                    let token_pos = left.rfind(token).unwrap();
                    let part_pos = token.rfind(target_part).unwrap();
                    return Some((val, token_pos + part_pos, e_idx));
                }
            }
        }

        let right_tokens: Vec<&str> = right.split_whitespace().take(limit).collect();
        for token in right_tokens {
            if !token.starts_with(&BANNED_CHARACTERS[..]) {
                let parts: Vec<&str> = token
                    .split(['-', '–', '—', '…'])
                    .filter(|s| !s.is_empty())
                    .collect();
                if let Some(target_part) = parts.first()
                    && let Some(val) = Self::parse_amount(target_part)
                {
                    let token_pos = right.find(token).unwrap();
                    let part_pos = token.find(target_part).unwrap();
                    return Some((val, s_idx, e_idx + token_pos + part_pos + target_part.len()));
                }
            }
        }
        None
    }

    fn parse_amount(s: &str) -> Option<f64> {
        let s_clean = s
            .trim_start_matches(['(', '[', '{'])
            .replace(',', ".")
            .replace(['_', ' '], "")
            .to_lowercase();

        if s_clean.is_empty() {
            return None;
        }

        let mut total = 0.0;
        let mut rest = s_clean.as_str();

        if let Some(c) = rest.chars().next()
            && !c.is_ascii_digit()
            && !matches!(c, '.')
        {
            return None;
        }

        while !rest.is_empty() {
            let digit_end = rest
                .find(|c: char| !c.is_ascii_digit() && c != '.')
                .unwrap_or(rest.len());

            let (num_s, remainder) = rest.split_at(digit_end);
            if num_s.is_empty() {
                break;
            }

            let value = num_s.parse::<f64>().ok()?;
            rest = remainder;

            let suffix_end = rest
                .find(|c: char| c.is_ascii_digit() || c == '.')
                .unwrap_or(rest.len());
            let (suffix, remainder) = rest.split_at(suffix_end);

            if suffix.starts_with(['-', '–', '—']) || suffix.starts_with("..") {
                total += value;
                break;
            }

            let clean_suffix = suffix.trim_end_matches([')', ']']);

            if !clean_suffix.is_empty() {
                let info = WORD_VALUES.get(clean_suffix)?;
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
            let len_a = a.match_end - a.match_start;
            let len_b = b.match_end - b.match_start;
            len_b.cmp(&len_a).then_with(|| a.start.cmp(&b.start))
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
        let mut detected = filtered;

        let rates = self.get_rates().await?;
        let mut items = Vec::new();
        let mut handled_indices = Vec::new();

        detected.sort_by(|a, b| a.start.cmp(&b.start));

        for (i, item) in detected.iter().enumerate() {
            if handled_indices.contains(&i) {
                continue;
            }

            let left_context = &text[..item.start];
            if let Some(caps) = RANGE_LEFT_REGEX.captures(left_context)
                && let Some(num_match) = caps.name("num")
                && let Some(min_val) = CurrencyDetector::parse_amount(num_match.as_str())
            {
                let range_start = item.start - (left_context.len() - num_match.start());
                items.push(ProcessingItem::Range {
                    min: min_val,
                    max: item.amount,
                    currency_code: item.currency_code.clone(),
                    start: range_start,
                });
                handled_indices.push(i);
                continue;
            }

            let right_context = &text[item.end..];
            if let Some(caps) = RANGE_RIGHT_REGEX.captures(right_context)
                && let Some(num_match) = caps.name("num")
                && let Some(max_val) = CurrencyDetector::parse_amount(num_match.as_str())
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

        let has_ranges = items
            .iter()
            .any(|i| matches!(i, ProcessingItem::Range { .. }));
        let math_chars = ['+', '-', '*', '/', '(', ')', '^'];

        if !has_ranges && text.chars().any(|c| math_chars.contains(&c)) {
            let mut expr = text.to_string();
            let mut detected_for_math = detected.clone();
            detected_for_math.sort_by(|a, b| b.start.cmp(&a.start));

            let base_code = "UAH";
            let mut is_math = true;

            for item in &detected_for_math {
                if let Some(rate) = rates.get(&item.currency_code) {
                    expr.replace_range(item.start..item.end, &format!("({})", item.amount * rate));
                } else {
                    is_math = false;
                }
            }

            if is_math {
                if let Ok(ast) = eidolon_lang::parse_eidolon_source(&expr) {
                    if let Ok(eval_result) = evaluate(&ast, &HashMap::new())
                        && let Ok(final_amount_base) = eval_result.as_number(0)
                        && let Some(res) = self.format_math_result(
                            text,
                            final_amount_base,
                            base_code,
                            &rates,
                            &settings.selected_codes,
                            locale,
                        )
                    {
                        return Ok(vec![res]);
                    }
                } else {
                    warn!("Failed to parse math expression: {}", expr);
                }
            }
        }

        let mut results = Vec::new();
        items.sort_by_key(|a| a.start());
        for item in items {
            if let Some(res) =
                self.format_conversion_item(&item, &rates, &settings.selected_codes, locale)
            {
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
        targets: &[String],
        locale: &str,
    ) -> Option<String> {
        let from_rate = rates.get(from_code)?;
        let mut builder = String::new();
        builder.push_str(&format!("{}:\n\n", expression));

        for target_code in targets {
            if let (Some(target_info), Some(to_rate)) =
                (self.currencies.get(target_code), rates.get(target_code))
            {
                let converted = amount * from_rate / to_rate;
                let target_word = self.get_plural(converted, target_info, locale);
                let val_str = Self::format_value(converted);
                builder.push_str(&format!(
                    "{} {} {}{} {}\n",
                    target_info.flag, val_str, target_info.symbol, target_code, target_word
                ));
            }
        }
        if builder.is_empty() {
            None
        } else {
            Some(builder)
        }
    }

    fn format_conversion_item(
        &self,
        item: &ProcessingItem,
        rates: &HashMap<String, f64>,
        targets: &[String],
        locale: &str,
    ) -> Option<String> {
        match item {
            ProcessingItem::Single(d) => self.format_conversion(d, rates, targets, locale),
            ProcessingItem::Range {
                min,
                max,
                currency_code,
                ..
            } => {
                let info = self.currencies.get(currency_code)?;
                let from_rate = rates.get(currency_code)?;
                let mut builder = String::new();

                let min_str = Self::format_value(*min);
                let max_str = Self::format_value(*max);
                let base_word = self.get_plural(*max, info, locale);

                builder.push_str(&format!(
                    "{} {} – {}{} {}\n\n",
                    info.flag, min_str, max_str, info.symbol, base_word
                ));

                for target_code in targets {
                    if target_code == currency_code {
                        continue;
                    }
                    if let (Some(target_info), Some(to_rate)) =
                        (self.currencies.get(target_code), rates.get(target_code))
                    {
                        let min_conv = min * from_rate / to_rate;
                        let max_conv = max * from_rate / to_rate;
                        let min_conv_str = Self::format_value(min_conv);
                        let max_conv_str = Self::format_value(max_conv);
                        let target_word = self.get_plural(max_conv, target_info, locale);
                        builder.push_str(&format!(
                            "{} {} – {}{} {}\n",
                            target_info.flag,
                            min_conv_str,
                            max_conv_str,
                            target_info.symbol,
                            target_word
                        ));
                    }
                }
                if builder.is_empty() {
                    None
                } else {
                    Some(builder.trim_end().to_string())
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
                let converted_str = Self::format_value(converted);
                builder.push_str(&format!(
                    "{} {}{} {}\n",
                    target_info.flag, converted_str, target_info.symbol, target_word
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
