//! Live distributor offers for an assembly lock, from Nexar (Octopart's
//! official API) and directly from distributor APIs. This is the only
//! networked path in the crate: the report is timestamped, not
//! deterministic, and nothing here runs from check, compile, lock or BOM
//! export. Credentials come from the environment only and are never logged
//! or written.
//!
//! Each [`Provider`] answers for one or more distributors; a provider is
//! enabled when its environment variables are set. Further distributors
//! (TME, LCSC, Arrow, ...) are added as further providers.

use std::cell::Cell;
use std::io::Write;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use copper_json::{Dumps, Object, Value, python_casefold};
use num_traits::ToPrimitive;

use crate::assembly::{AssemblyError, Lock, Offer, Result, SCHEMA};

pub mod digikey;
pub mod farnell;
pub mod mouser;
pub mod nexar;

#[cfg(test)]
pub(crate) mod testing;

pub const REPORT_SCHEMA: &str = "copperscript-assembly-offers/v0.1";

pub(crate) fn fail<T>(message: impl Into<String>) -> Result<T> {
    Err(AssemblyError(message.into()))
}

/// One HTTPS request; tests substitute recorded exchanges.
pub trait Transport {
    /// Send `body` (if any) with `method` to `url`; return the HTTP status
    /// and response body.
    fn send(&self, method: &str, url: &str, headers: &[(&str, String)], body: Option<&str>) -> Result<(u16, String)>;
}

/// `curl` with its whole request on standard input, so no credential (in a
/// header, body or query string) appears in a command line or process
/// listing.
pub struct Curl;

/// A `curl --config` string: quoted, with `\` and `"` escaped.
fn curl_quoted(text: &str) -> String {
    let mut quoted = String::from("\"");
    for character in text.chars() {
        match character {
            '\\' => quoted.push_str("\\\\"),
            '"' => quoted.push_str("\\\""),
            '\n' => quoted.push_str("\\n"),
            '\r' => quoted.push_str("\\r"),
            '\t' => quoted.push_str("\\t"),
            other => quoted.push(other),
        }
    }
    quoted.push('"');
    quoted
}

/// `url` without its query string, which may carry an API key.
pub(crate) fn shown_url(url: &str) -> &str {
    url.split('?').next().unwrap_or(url)
}

impl Transport for Curl {
    fn send(&self, method: &str, url: &str, headers: &[(&str, String)], body: Option<&str>) -> Result<(u16, String)> {
        let shown = shown_url(url);
        let failed = |failure: std::io::Error| AssemblyError(format!("cannot run curl for {shown}: {failure}"));
        let mut config = format!(
            "url = {}\nrequest = {}\nsilent\nshow-error\nmax-time = 120\n",
            curl_quoted(url),
            curl_quoted(method)
        );
        for (name, value) in headers {
            config.push_str(&format!("header = {}\n", curl_quoted(&format!("{name}: {value}"))));
        }
        if let Some(body) = body {
            config.push_str(&format!("data-binary = {}\n", curl_quoted(body)));
        }
        config.push_str("write-out = \"\\n%{http_code}\"\n");
        let mut child = Command::new("curl")
            .args(["--config", "-"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(failed)?;
        child.stdin.take().expect("piped stdin").write_all(config.as_bytes()).map_err(failed)?;
        let output = child.wait_with_output().map_err(failed)?;
        let text = String::from_utf8_lossy(&output.stdout).into_owned();
        if !output.status.success() {
            // curl names the host; drop any query string it echoes anyway.
            let message = String::from_utf8_lossy(&output.stderr).replace(url, shown).trim().to_owned();
            return fail(format!("request to {shown} failed: {message}"));
        }
        let (body, status) = text.rsplit_once('\n').unwrap_or(("", text.as_str()));
        let status = status.trim().parse().map_err(|_| AssemblyError(format!("request to {shown} failed")))?;
        Ok((status, body.to_owned()))
    }
}

/// A credential read from the environment; `Debug` never shows it.
#[derive(Clone)]
pub struct Secret(String);

impl std::fmt::Debug for Secret {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("<redacted>")
    }
}

impl Secret {
    pub fn new(value: impl Into<String>) -> Secret {
        Secret(value.into())
    }

    pub(crate) fn expose(&self) -> &str {
        &self.0
    }
}

/// A nonempty environment variable.
pub(crate) fn env(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|value| !value.trim().is_empty())
}

/// `application/x-www-form-urlencoded` / query-string value.
pub(crate) fn form_encode(text: &str) -> String {
    text.bytes()
        .map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => (byte as char).to_string(),
            _ => format!("%{byte:02X}"),
        })
        .collect()
}

/// Keeps at least `interval` between a provider's requests, under its
/// published per-minute cap.
pub struct Pacer {
    interval: Duration,
    last: Cell<Option<Instant>>,
}

impl Pacer {
    pub fn new(interval: Duration) -> Pacer {
        Pacer { interval, last: Cell::new(None) }
    }

    pub fn wait(&self) {
        if let Some(last) = self.last.get() {
            let elapsed = last.elapsed();
            if elapsed < self.interval {
                std::thread::sleep(self.interval - elapsed);
            }
        }
        self.last.set(Some(Instant::now()));
    }
}

/// The body of a 200 response, or a clear error; 429 names the rate limit.
pub(crate) fn checked(provider: &str, status: u16, body: String, detail: Option<String>) -> Result<String> {
    match status {
        200 => Ok(body),
        429 => fail(format!(
            "{provider} rate limit reached (HTTP 429); wait before retrying, or narrow the lookup with \
             --provider or --distributors"
        )),
        _ => fail(format!(
            "{provider} request failed (HTTP {status}){}",
            detail.map(|detail| format!(": {detail}")).unwrap_or_default()
        )),
    }
}

/// One manufacturer part fitted on the board, with every reference using it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Line {
    pub manufacturer: String,
    pub mpn: String,
    pub references: Vec<String>,
}

/// The fitted manufacturer parts of `lock` (populate `none` excluded), sorted,
/// and the fitted references without a manufacturer and MPN.
pub fn lines(lock: &Lock) -> (Vec<Line>, Vec<String>) {
    let mut lines: Vec<Line> = Vec::new();
    let mut unidentified = Vec::new();
    for entry in lock.selections.iter().filter(|entry| entry.populate != "none") {
        let (Some(manufacturer), Some(mpn)) = (&entry.manufacturer, &entry.mpn) else {
            unidentified.push(entry.reference.clone());
            continue;
        };
        match lines.iter_mut().find(|line| line.manufacturer == *manufacturer && line.mpn == *mpn) {
            Some(line) => line.references.push(entry.reference.clone()),
            None => lines.push(Line {
                manufacturer: manufacturer.clone(),
                mpn: mpn.clone(),
                references: vec![entry.reference.clone()],
            }),
        }
    }
    for line in &mut lines {
        line.references.sort();
    }
    lines.sort_by(|a, b| (&a.manufacturer, &a.mpn).cmp(&(&b.manufacturer, &b.mpn)));
    unidentified.sort();
    (lines, unidentified)
}

/// Distributor names compared without case or punctuation: `Digi-Key` is `DigiKey`.
pub fn distributor_key(name: &str) -> String {
    python_casefold(name).chars().filter(|c| c.is_alphanumeric()).collect()
}

/// The requested spelling of `seller`, if it was requested.
pub(crate) fn requested<'a>(distributors: &'a [String], seller: &str) -> Option<&'a String> {
    distributors.iter().find(|wanted| distributor_key(wanted) == distributor_key(seller))
}

/// The suppliers the lock already names (primary, then alternates), in
/// first-seen order and spelling.
pub fn lock_distributors(lock: &Lock) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    for entry in &lock.selections {
        let offered = entry.supplier.iter().chain(entry.alternates.iter().map(|offer| &offer.supplier));
        for name in offered {
            if !names.iter().any(|known| distributor_key(known) == distributor_key(name)) {
                names.push(name.clone());
            }
        }
    }
    names
}

/// A manufacturer name as a distributor may spell it: `Murata` matches
/// `Murata Electronics`, `YAGEO` matches `Yageo`.
fn same_manufacturer(lock: &str, found: &str) -> bool {
    let (lock, found) = (distributor_key(lock), distributor_key(found));
    !lock.is_empty() && !found.is_empty() && (lock.starts_with(&found) || found.starts_with(&lock))
}

#[derive(Debug, Clone, PartialEq)]
pub struct PriceBreak {
    pub quantity: i64,
    pub price: f64,
    pub currency: String,
    /// In the requested currency, when the provider converted it.
    pub converted_price: Option<f64>,
    pub converted_currency: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DistributorOffer {
    /// The requested distributor's spelling (`DigiKey`).
    pub distributor: String,
    /// The provider's seller name (`Digi-Key`).
    pub seller: String,
    /// The provider it came from (`nexar`, `digikey`, ...).
    pub source: String,
    pub sku: String,
    /// None where the seller reports no inventory figure.
    pub stock: Option<i64>,
    pub factory_lead_days: Option<i64>,
    pub moq: Option<i64>,
    pub packaging: Option<String>,
    pub url: Option<String>,
    pub prices: Vec<PriceBreak>,
}

/// A part a provider returned for a line's query, with its offers from the
/// requested distributors; [`select`] decides whether it is the line's part.
#[derive(Debug, Clone, PartialEq)]
pub struct Found {
    pub manufacturer: String,
    pub mpn: String,
    pub offers: Vec<DistributorOffer>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LineOffers {
    pub line: Line,
    /// The provider's spelling of the exactly matched part, or None.
    pub matched: Option<(String, String)>,
    /// Parts returned that were not an exact match, for review.
    pub candidates: Vec<(String, String)>,
    pub offers: Vec<DistributorOffer>,
}

/// A line's exact part among `found`: MPN equal without case, manufacturer
/// prefix-equal without case or punctuation. Every exact part's offers count
/// (a distributor may list one part twice).
pub fn select(line: &Line, found: Vec<Found>, distributors: &[String]) -> LineOffers {
    let (exact, other): (Vec<Found>, Vec<Found>) = found.into_iter().partition(|part| {
        python_casefold(&part.mpn) == python_casefold(&line.mpn)
            && same_manufacturer(&line.manufacturer, &part.manufacturer)
    });
    let matched = exact.first().map(|part| (part.manufacturer.clone(), part.mpn.clone()));
    let mut candidates: Vec<(String, String)> = Vec::new();
    if matched.is_none() {
        for part in other {
            let identity = (part.manufacturer, part.mpn);
            if !candidates.contains(&identity) {
                candidates.push(identity);
            }
        }
    }
    let mut offers: Vec<DistributorOffer> = exact.into_iter().flat_map(|part| part.offers).collect();
    order_offers(&mut offers, distributors);
    LineOffers { line: line.clone(), matched, candidates, offers }
}

/// Stable report order: distributor as requested, then SKU; one offer per
/// distributor and SKU, keeping the first (direct providers come first).
fn order_offers(offers: &mut Vec<DistributorOffer>, distributors: &[String]) {
    let mut unique: Vec<DistributorOffer> = Vec::new();
    for offer in offers.drain(..) {
        if !unique.iter().any(|known| {
            distributor_key(&known.distributor) == distributor_key(&offer.distributor) && known.sku == offer.sku
        }) {
            unique.push(offer);
        }
    }
    unique.sort_by(|a, b| {
        let rank = |offer: &DistributorOffer| distributors.iter().position(|d| *d == offer.distributor);
        (rank(a), &a.sku).cmp(&(rank(b), &b.sku))
    });
    *offers = unique;
}

/// A source of live offers for one or more distributors.
pub trait Provider {
    /// The `--provider` name: `nexar`, `digikey`, `mouser`, `farnell`.
    fn name(&self) -> &'static str;
    /// Whether any of the requested distributors is one this provider sells
    /// for; a provider that serves none is not queried.
    fn serves(&self, distributors: &[String]) -> bool;
    /// The parts found for each line, in line order, offers limited to the
    /// requested distributors.
    fn fetch(&self, transport: &dyn Transport, lines: &[Line], distributors: &[String]) -> Result<Vec<Vec<Found>>>;
}

/// Provider names and the environment variables that enable each.
pub const PROVIDERS: [(&str, &str); 4] = [
    ("digikey", "DIGIKEY_CLIENT_ID and DIGIKEY_CLIENT_SECRET"),
    ("mouser", "MOUSER_API_KEY"),
    ("farnell", "FARNELL_API_KEY (and optional FARNELL_STORE)"),
    ("nexar", "NEXAR_CLIENT_ID and NEXAR_CLIENT_SECRET"),
];

/// Where prices are asked for: Nexar and DigiKey take both, others use the
/// account or store's own.
#[derive(Debug, Clone)]
pub struct Locale {
    pub country: String,
    pub currency: String,
}

/// The providers whose environment variables are set, direct distributor
/// APIs first, limited to `only` when given (an unconfigured name there is
/// an error).
pub fn enabled_providers(only: Option<&[String]>, locale: &Locale) -> Result<Vec<Box<dyn Provider>>> {
    if let Some(only) = only {
        for name in only {
            if !PROVIDERS.iter().any(|(known, _)| *known == python_casefold(name)) {
                let names: Vec<&str> = PROVIDERS.iter().map(|(name, _)| *name).collect();
                return fail(format!("unknown offer provider '{name}' (choose from {})", names.join(", ")));
            }
        }
    }
    let wanted = |name: &str| only.is_none_or(|only| only.iter().any(|item| python_casefold(item) == name));
    let mut providers: Vec<Box<dyn Provider>> = Vec::new();
    let mut missing = Vec::new();
    for (name, variables) in PROVIDERS {
        if !wanted(name) {
            continue;
        }
        let provider: Option<Box<dyn Provider>> = match name {
            "digikey" => digikey::DigiKey::from_env(locale).map(|p| Box::new(p) as Box<dyn Provider>),
            "mouser" => mouser::Mouser::from_env().map(|p| Box::new(p) as Box<dyn Provider>),
            "farnell" => farnell::Farnell::from_env().map(|p| Box::new(p) as Box<dyn Provider>),
            _ => nexar::Nexar::from_env(locale).map(|p| Box::new(p) as Box<dyn Provider>),
        };
        match provider {
            Some(provider) => providers.push(provider),
            None => missing.push(format!("{variables} ({name})")),
        }
    }
    if only.is_some() && !missing.is_empty() {
        return fail(format!("offer provider not configured; set {}", missing.join("; ")));
    }
    if providers.is_empty() {
        return fail(format!("no offer provider is configured; set any of: {}", missing.join("; ")));
    }
    Ok(providers)
}

/// Every line's offers merged across `providers`, and each provider that
/// failed (its results are dropped, the others kept). Fails only when every
/// queried provider failed or none serves the requested distributors.
pub fn fetch_all(
    transport: &dyn Transport,
    providers: &[Box<dyn Provider>],
    lines: &[Line],
    distributors: &[String],
) -> Result<(Vec<LineOffers>, Vec<String>, Vec<(String, String)>)> {
    let mut found: Vec<Vec<Found>> = vec![Vec::new(); lines.len()];
    let (mut used, mut errors) = (Vec::new(), Vec::new());
    for provider in providers.iter().filter(|provider| provider.serves(distributors)) {
        match provider.fetch(transport, lines, distributors) {
            Ok(parts) => {
                used.push(provider.name().to_owned());
                for (line, parts) in found.iter_mut().zip(parts) {
                    line.extend(parts);
                }
            }
            Err(AssemblyError(message)) => errors.push((provider.name().to_owned(), message)),
        }
    }
    if used.is_empty() {
        return match errors.as_slice() {
            [] => fail(format!(
                "no configured offer provider serves {}; adjust --distributors or --provider",
                distributors.join(", ")
            )),
            _ => fail(errors.iter().map(|(name, message)| format!("{name}: {message}")).collect::<Vec<_>>().join("; ")),
        };
    }
    let results = lines.iter().zip(found).map(|(line, parts)| select(line, parts, distributors)).collect();
    Ok((results, used, errors))
}

/// What a line needs from at least one offer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Criteria {
    /// Boards to build.
    pub quantity: u64,
    pub min_stock: Option<u64>,
    pub within_weeks: Option<u64>,
}

impl Criteria {
    pub fn needed(&self, line: &Line) -> u64 {
        line.references.len() as u64 * self.quantity
    }

    /// In stock for the build (and at least `min_stock`), or, with
    /// `within_weeks`, orderable from the factory within that time.
    pub fn adequate(&self, offer: &DistributorOffer, needed: u64) -> bool {
        let wanted = needed.max(self.min_stock.unwrap_or(0));
        let stocked = offer.stock.is_some_and(|stock| stock >= 0 && stock as u64 >= wanted);
        let lead = self
            .within_weeks
            .is_some_and(|weeks| offer.factory_lead_days.is_some_and(|days| days >= 0 && days as u64 <= weeks * 7));
        stocked || lead
    }
}

pub(crate) fn integer(value: Option<&Value>) -> Option<i64> {
    match value? {
        Value::Int(number) => number.to_i64(),
        Value::Float(number) if number.is_finite() => Some(*number as i64),
        // Some APIs send counts as text: "1234", "1,234 In Stock".
        Value::Str(text) => {
            let digits: String =
                text.trim().chars().take_while(|c| c.is_ascii_digit() || *c == ',' || *c == '.').collect();
            digits.replace([',', '.'], "").parse().ok()
        }
        _ => None,
    }
}

pub(crate) fn number(value: Option<&Value>) -> Option<f64> {
    match value? {
        Value::Int(number) => number.to_f64(),
        Value::Float(number) => Some(*number),
        Value::Str(text) => decimal(text),
        _ => None,
    }
}

/// A price as text in any locale: `0,083 €`, `$1.25`, `1.234,50`. The last
/// separator is the decimal mark unless it repeats (`1,234,567`).
pub(crate) fn decimal(text: &str) -> Option<f64> {
    let kept: String = text.chars().filter(|c| c.is_ascii_digit() || *c == ',' || *c == '.').collect();
    let normal = match kept.rfind([',', '.']) {
        Some(at) if kept.matches(&kept[at..=at]).count() == 1 => {
            format!("{}.{}", kept[..at].replace([',', '.'], ""), &kept[at + 1..])
        }
        _ => kept.replace([',', '.'], ""),
    };
    normal.parse().ok()
}

pub(crate) fn string(value: Option<&Value>) -> Option<String> {
    value.and_then(Value::as_str).map(str::trim).filter(|text| !text.is_empty()).map(str::to_owned)
}

pub(crate) fn items(value: Option<&Value>) -> &[Value] {
    value.and_then(Value::as_array).unwrap_or(&[])
}

/// A lead time as text: `84 Days`, `12 Weeks`, `13` (weeks where unitless).
pub(crate) fn lead_days(text: &str, unitless_weeks: bool) -> Option<i64> {
    let lower = text.to_ascii_lowercase();
    let count: i64 = lower.trim().split(|c: char| !c.is_ascii_digit()).find(|s| !s.is_empty())?.parse().ok()?;
    let weeks = lower.contains("week") || (unitless_weeks && !lower.contains("day"));
    Some(if weeks { count * 7 } else { count })
}

/// `YYYY-MM-DDTHH:MM:SSZ` for seconds since the Unix epoch.
pub fn utc_timestamp(seconds: u64) -> String {
    let (days, rest) = ((seconds / 86_400) as i64, seconds % 86_400);
    // Howard Hinnant's civil_from_days.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z", rest / 3_600, rest % 3_600 / 60, rest % 60)
}

/// What the report was asked for, recorded alongside the offers.
pub struct Request<'a> {
    pub board: &'a str,
    pub lock: &'a str,
    pub generated_at: &'a str,
    pub distributors: &'a [String],
    pub providers: &'a [String],
    pub provider_errors: &'a [(String, String)],
    pub locale: &'a Locale,
    pub criteria: Criteria,
}

fn optional_int(value: Option<i64>) -> Value {
    value.map_or(Value::Null, Value::from)
}

fn names(values: &[String]) -> Value {
    Value::Array(values.iter().map(|name| Value::from(name.as_str())).collect())
}

/// The lines with no adequate offer from the requested distributors.
pub fn shortfalls<'a>(results: &'a [LineOffers], criteria: &Criteria) -> Vec<&'a LineOffers> {
    results
        .iter()
        .filter(|result| {
            let needed = criteria.needed(&result.line);
            !result.offers.iter().any(|offer| criteria.adequate(offer, needed))
        })
        .collect()
}

pub fn shortfall_reason(result: &LineOffers) -> &'static str {
    if result.matched.is_none() {
        "no exact manufacturer part match"
    } else if result.offers.is_empty() {
        "no offer from the requested distributors"
    } else {
        "no offer meets the stock or lead-time requirement"
    }
}

fn offer_value(offer: &DistributorOffer, adequate: bool) -> Value {
    let mut entry = Object::new();
    entry.insert("distributor".into(), Value::from(offer.distributor.as_str()));
    entry.insert("seller".into(), Value::from(offer.seller.as_str()));
    entry.insert("source".into(), Value::from(offer.source.as_str()));
    entry.insert("sku".into(), Value::from(offer.sku.as_str()));
    entry.insert("stock".into(), optional_int(offer.stock));
    entry.insert("factory_lead_days".into(), optional_int(offer.factory_lead_days));
    entry.insert("moq".into(), optional_int(offer.moq));
    entry.insert("packaging".into(), offer.packaging.as_deref().map_or(Value::Null, Value::from));
    entry.insert("url".into(), offer.url.as_deref().map_or(Value::Null, Value::from));
    entry.insert("adequate".into(), Value::Bool(adequate));
    let prices = offer
        .prices
        .iter()
        .map(|price| {
            let mut item = Object::new();
            item.insert("quantity".into(), Value::from(price.quantity));
            item.insert("price".into(), Value::from(price.price));
            item.insert("currency".into(), Value::from(price.currency.as_str()));
            if let (Some(converted), Some(currency)) = (price.converted_price, &price.converted_currency) {
                item.insert("converted_price".into(), Value::from(converted));
                item.insert("converted_currency".into(), Value::from(currency.as_str()));
            }
            Value::Object(item)
        })
        .collect();
    entry.insert("prices".into(), Value::Array(prices));
    Value::Object(entry)
}

/// The availability report: live data, so never an input to compilation.
pub fn report(request: &Request, results: &[LineOffers], unidentified: &[String]) -> String {
    let criteria = request.criteria;
    let lines: Vec<Value> = results
        .iter()
        .map(|result| {
            let needed = criteria.needed(&result.line);
            let mut item = Object::new();
            item.insert("manufacturer".into(), Value::from(result.line.manufacturer.as_str()));
            item.insert("mpn".into(), Value::from(result.line.mpn.as_str()));
            item.insert("references".into(), names(&result.line.references));
            item.insert("quantity_per_board".into(), Value::from(result.line.references.len() as u64));
            item.insert("quantity_needed".into(), Value::from(needed));
            item.insert(
                "matched".into(),
                result.matched.as_ref().map_or(Value::Null, |(manufacturer, mpn)| {
                    let mut matched = Object::new();
                    matched.insert("manufacturer".into(), Value::from(manufacturer.as_str()));
                    matched.insert("mpn".into(), Value::from(mpn.as_str()));
                    Value::Object(matched)
                }),
            );
            if result.matched.is_none() {
                let candidates = result
                    .candidates
                    .iter()
                    .map(|(manufacturer, mpn)| Value::from(format!("{manufacturer} {mpn}")))
                    .collect();
                item.insert("candidates".into(), Value::Array(candidates));
            }
            let offers =
                result.offers.iter().map(|offer| offer_value(offer, criteria.adequate(offer, needed))).collect();
            item.insert("offers".into(), Value::Array(offers));
            item.insert("adequate".into(), Value::Bool(result.offers.iter().any(|o| criteria.adequate(o, needed))));
            Value::Object(item)
        })
        .collect();
    let shortfall = shortfalls(results, &criteria)
        .into_iter()
        .map(|result| {
            let mut item = Object::new();
            item.insert("manufacturer".into(), Value::from(result.line.manufacturer.as_str()));
            item.insert("mpn".into(), Value::from(result.line.mpn.as_str()));
            item.insert("references".into(), names(&result.line.references));
            item.insert("reason".into(), Value::from(shortfall_reason(result)));
            Value::Object(item)
        })
        .collect();
    let provider_errors = request
        .provider_errors
        .iter()
        .map(|(name, message)| {
            let mut item = Object::new();
            item.insert("provider".into(), Value::from(name.as_str()));
            item.insert("message".into(), Value::from(message.as_str()));
            Value::Object(item)
        })
        .collect();
    let mut requirements = Object::new();
    requirements.insert("boards".into(), Value::from(criteria.quantity));
    requirements.insert("min_stock".into(), criteria.min_stock.map_or(Value::Null, Value::from));
    requirements.insert("within_weeks".into(), criteria.within_weeks.map_or(Value::Null, Value::from));
    let mut document = Object::new();
    document.insert("schema".into(), Value::from(REPORT_SCHEMA));
    document.insert("board".into(), Value::from(request.board));
    document.insert("lock".into(), Value::from(request.lock));
    document.insert("generated_at".into(), Value::from(request.generated_at));
    document.insert("providers".into(), names(request.providers));
    document.insert("provider_errors".into(), Value::Array(provider_errors));
    document.insert("deterministic".into(), Value::Bool(false));
    document.insert("country".into(), Value::from(request.locale.country.as_str()));
    document.insert("currency".into(), Value::from(request.locale.currency.as_str()));
    document.insert("distributors".into(), names(request.distributors));
    document.insert("requirements".into(), Value::Object(requirements));
    document.insert("line_count".into(), Value::from(results.len() as u64));
    document.insert("lines".into(), Value::Array(lines));
    document.insert("shortfall".into(), Value::Array(shortfall));
    document.insert("unidentified".into(), names(unidentified));
    Dumps::new().indent(2).sort_keys(true).dumps(&Value::Object(document)) + "\n"
}

/// A distributor's offer worth recording: adequate first, then the smallest
/// order quantity (cut tape over reels), most stock, lowest SKU.
fn preferred<'a>(offers: &[&'a DistributorOffer], needed: u64, criteria: &Criteria) -> Option<&'a DistributorOffer> {
    offers.iter().copied().min_by(|a, b| {
        let key = |offer: &DistributorOffer| {
            (!criteria.adequate(offer, needed), offer.moq.unwrap_or(i64::MAX), -offer.stock.unwrap_or(-1))
        };
        key(a).cmp(&key(b)).then_with(|| a.sku.cmp(&b.sku))
    })
}

/// Octopart click-tracking links are not stable catalogue URLs.
fn catalogue_url(url: &Option<String>) -> Option<String> {
    url.clone().filter(|url| !url.contains("octopart.com/"))
}

/// The lock with an alternate added, for each fitted selection, from every
/// requested distributor that sells its exact manufacturer part and is not
/// yet listed on it. Nothing else in the lock changes. Returns the additions
/// as (reference, supplier, supplier part).
pub fn add_alternates(
    lock: &Lock,
    results: &[LineOffers],
    distributors: &[String],
    criteria: &Criteria,
) -> Result<(Lock, Vec<(String, String, String)>)> {
    if lock.electrical_sha256.is_some() {
        return fail(format!("--write-alternates needs a {SCHEMA} lock; run `copper assembly update` first"));
    }
    let mut updated = lock.clone();
    let mut added = Vec::new();
    for entry in &mut updated.selections {
        if entry.populate == "none" {
            continue;
        }
        let (Some(manufacturer), Some(mpn)) = (&entry.manufacturer, &entry.mpn) else { continue };
        let Some(result) =
            results.iter().find(|r| r.matched.is_some() && r.line.manufacturer == *manufacturer && r.line.mpn == *mpn)
        else {
            continue;
        };
        let needed = criteria.needed(&result.line);
        for distributor in distributors {
            let key = distributor_key(distributor);
            let listed = entry.supplier.iter().chain(entry.alternates.iter().map(|o| &o.supplier));
            if listed.into_iter().any(|supplier| distributor_key(supplier) == key) {
                continue;
            }
            let candidates: Vec<&DistributorOffer> = result
                .offers
                .iter()
                .filter(|offer| offer.distributor == *distributor)
                // A code the lock already gives another manufacturer part
                // would fail `assembly check` as a conflicting offer.
                .filter(|offer| {
                    !lock.selections.iter().any(|other| {
                        (other.manufacturer.as_ref(), other.mpn.as_ref()) != (Some(manufacturer), Some(mpn))
                            && other
                                .supplier
                                .iter()
                                .zip(other.supplier_part.iter())
                                .map(|(supplier, code)| (supplier.as_str(), code.as_str()))
                                .chain(other.alternates.iter().map(|o| (o.supplier.as_str(), o.supplier_part.as_str())))
                                .any(|(supplier, code)| distributor_key(supplier) == key && code == offer.sku)
                    })
                })
                .collect();
            let Some(offer) = preferred(&candidates, needed, criteria) else { continue };
            entry.alternates.push(Offer {
                supplier: distributor.clone(),
                supplier_part: offer.sku.clone(),
                source_url: catalogue_url(&offer.url),
            });
            added.push((entry.reference.clone(), distributor.clone(), offer.sku.clone()));
        }
    }
    Ok((updated, added))
}

#[cfg(test)]
mod tests {
    use super::testing::{Recorded, distributors, lock};
    use super::*;
    use crate::assembly::Selection;

    fn offer(distributor: &str, source: &str, sku: &str, stock: i64, moq: i64, lead: Option<i64>) -> DistributorOffer {
        DistributorOffer {
            distributor: distributor.into(),
            seller: distributor.into(),
            source: source.into(),
            sku: sku.into(),
            stock: Some(stock),
            factory_lead_days: lead,
            moq: Some(moq),
            packaging: None,
            url: Some(format!("https://example.test/{sku}")),
            prices: Vec::new(),
        }
    }

    fn found(manufacturer: &str, mpn: &str, offers: Vec<DistributorOffer>) -> Found {
        Found { manufacturer: manufacturer.into(), mpn: mpn.into(), offers }
    }

    /// A provider answering from a fixed table, or failing.
    struct Fixed {
        name: &'static str,
        serves: &'static str,
        answer: std::result::Result<Vec<Vec<Found>>, String>,
    }

    impl Provider for Fixed {
        fn name(&self) -> &'static str {
            self.name
        }
        fn serves(&self, distributors: &[String]) -> bool {
            requested(distributors, self.serves).is_some()
        }
        fn fetch(&self, _: &dyn Transport, _: &[Line], _: &[String]) -> Result<Vec<Vec<Found>>> {
            self.answer.clone().map_err(AssemblyError)
        }
    }

    fn criteria() -> Criteria {
        Criteria { quantity: 10, min_stock: None, within_weeks: None }
    }

    /// TI line, then YAGEO line, as `lines(&lock())` sorts them.
    fn results() -> Vec<LineOffers> {
        let (lines, _) = lines(&lock());
        let ti = vec![found(
            "Texas Instruments",
            "TPS62130RGTR",
            vec![offer("DigiKey", "digikey", "296-38599-1-ND", 5000, 1, Some(42))],
        )];
        let yageo = vec![
            found("Yageo", "CC0603KRX7R9BB104X", vec![offer("Farnell", "farnell", "1", 1, 1, None)]),
            found(
                "Yageo",
                "CC0603KRX7R9BB104",
                vec![
                    offer("Farnell", "farnell", "3019XXX", 10, 5000, Some(70)),
                    offer("Farnell", "farnell", "3019495", 40000, 10, Some(70)),
                    offer("Mouser", "mouser", "603-CC603KRX7R9BB104", 1, 1, Some(7)),
                ],
            ),
        ];
        vec![select(&lines[0], ti, &distributors()), select(&lines[1], yageo, &distributors())]
    }

    #[test]
    fn lines_group_fitted_parts_and_list_unidentified_references() {
        let (lines, unidentified) = lines(&lock());
        let shown: Vec<(&str, &[String])> = lines.iter().map(|l| (l.mpn.as_str(), l.references.as_slice())).collect();
        assert_eq!(
            shown,
            vec![
                ("TPS62130RGTR", &["U2".to_owned()][..]),
                ("CC0603KRX7R9BB104", &["C1".to_owned(), "C2".to_owned()][..])
            ]
        );
        assert_eq!(unidentified, vec!["U1".to_owned()]);
        assert_eq!(lock_distributors(&lock()), vec!["Mouser".to_owned(), "DigiKey".to_owned()]);
    }

    #[test]
    fn select_keeps_exact_parts_and_orders_offers() {
        let results = results();
        let yageo = &results[1];
        assert_eq!(yageo.matched.as_ref().map(|m| m.1.as_str()), Some("CC0603KRX7R9BB104"), "not the X variant");
        let skus: Vec<&str> = yageo.offers.iter().map(|o| o.sku.as_str()).collect();
        assert_eq!(skus, vec!["603-CC603KRX7R9BB104", "3019495", "3019XXX"]);
        let unmatched = select(&yageo.line, vec![found("Yageo", "CC0603KRX7R9BB104X", Vec::new())], &distributors());
        assert_eq!(unmatched.matched, None);
        assert_eq!(unmatched.candidates, vec![("Yageo".to_owned(), "CC0603KRX7R9BB104X".to_owned())]);
        assert_eq!(shortfall_reason(&unmatched), "no exact manufacturer part match");
    }

    #[test]
    fn providers_merge_and_a_failing_one_is_reported_not_fatal() {
        let (lines, _) = lines(&lock());
        let direct = Fixed {
            name: "digikey",
            serves: "DigiKey",
            answer: Ok(vec![
                vec![found("Texas Instruments", "TPS62130RGTR", vec![offer("DigiKey", "digikey", "A", 5, 1, None)])],
                Vec::new(),
            ]),
        };
        let aggregate = Fixed {
            name: "nexar",
            serves: "Mouser",
            answer: Ok(vec![
                vec![found(
                    "Texas Instruments",
                    "TPS62130RGTR",
                    vec![offer("DigiKey", "nexar", "A", 1, 1, None), offer("Mouser", "nexar", "M", 3, 1, None)],
                )],
                vec![found("YAGEO", "CC0603KRX7R9BB104", vec![offer("Mouser", "nexar", "Y", 9, 1, None)])],
            ]),
        };
        let broken = Fixed { name: "mouser", serves: "Mouser", answer: Err("rate limit".into()) };
        let idle = Fixed { name: "farnell", serves: "Newark", answer: Err("never asked".into()) };
        let providers: Vec<Box<dyn Provider>> =
            vec![Box::new(direct), Box::new(broken), Box::new(idle), Box::new(aggregate)];
        let transport = Recorded::new(Vec::new());
        let (results, used, errors) = fetch_all(&transport, &providers, &lines, &distributors()).expect("merged");
        assert_eq!(used, vec!["digikey".to_owned(), "nexar".to_owned()]);
        assert_eq!(errors, vec![("mouser".to_owned(), "rate limit".to_owned())]);
        let ti: Vec<(&str, &str, Option<i64>)> =
            results[0].offers.iter().map(|o| (o.sku.as_str(), o.source.as_str(), o.stock)).collect();
        assert_eq!(ti, vec![("A", "digikey", Some(5)), ("M", "nexar", Some(3))], "the direct DigiKey offer wins");
        assert_eq!(results[1].offers.len(), 1);
        let only_broken: Vec<Box<dyn Provider>> =
            vec![Box::new(Fixed { name: "mouser", serves: "Mouser", answer: Err("HTTP 401".into()) })];
        assert_eq!(fetch_all(&transport, &only_broken, &lines, &distributors()).unwrap_err().0, "mouser: HTTP 401");
    }

    #[test]
    fn criteria_decide_shortfalls() {
        let results = results();
        // 10 boards: TI needs 10 (5000 stocked); YAGEO needs 20 (Farnell 40000).
        assert!(shortfalls(&results, &criteria()).is_empty());
        let strict = Criteria { min_stock: Some(50_000), ..criteria() };
        assert_eq!(shortfalls(&results, &strict).len(), 2);
        // Six weeks covers TI's 42-day factory lead and Mouser's 7 days; five does not cover TI.
        assert!(shortfalls(&results, &Criteria { within_weeks: Some(6), ..strict }).is_empty());
        let lead = Criteria { within_weeks: Some(5), ..strict };
        let short: Vec<&str> = shortfalls(&results, &lead).iter().map(|r| r.line.mpn.as_str()).collect();
        assert_eq!(short, vec!["TPS62130RGTR"]);
        let lead = Criteria { within_weeks: Some(0), ..strict };
        assert_eq!(shortfalls(&results, &lead).len(), 2);
    }

    #[test]
    fn report_is_timestamped_and_lists_shortfalls_and_provider_errors() {
        let results = results();
        let (_, unidentified) = lines(&lock());
        let distributors = distributors();
        let locale = Locale { country: "NO".into(), currency: "EUR".into() };
        let request = Request {
            board: "Demo",
            lock: "assembly.lock",
            generated_at: &utc_timestamp(1_793_620_800),
            distributors: &distributors,
            providers: &["digikey".to_owned()],
            provider_errors: &[("nexar".to_owned(), "part limit".to_owned())],
            locale: &locale,
            criteria: Criteria { min_stock: Some(10_000), ..criteria() },
        };
        let document = copper_json::parse(&report(&request, &results, &unidentified)).expect("json");
        assert_eq!(document.get("schema").and_then(Value::as_str), Some(REPORT_SCHEMA));
        assert_eq!(document.get("generated_at").and_then(Value::as_str), Some("2026-11-02T12:00:00Z"));
        assert_eq!(document.get("deterministic"), Some(&Value::Bool(false)));
        assert_eq!(items(document.get("provider_errors"))[0].get("provider"), Some(&Value::from("nexar")));
        let shortfall = items(document.get("shortfall"));
        assert_eq!(shortfall.len(), 1);
        assert_eq!(shortfall[0].get("mpn").and_then(Value::as_str), Some("TPS62130RGTR"));
        assert_eq!(items(document.get("unidentified")), &[Value::from("U1")]);
        let offer = &items(items(document.get("lines"))[1].get("offers"))[1];
        assert_eq!(offer.get("sku").and_then(Value::as_str), Some("3019495"));
        assert_eq!(offer.get("source").and_then(Value::as_str), Some("farnell"));
        assert_eq!(offer.get("adequate"), Some(&Value::Bool(true)));
    }

    #[test]
    fn alternates_add_missing_distributors_and_change_nothing_else() {
        let original = lock();
        let results = results();
        let (updated, added) = add_alternates(&original, &results, &distributors(), &criteria()).expect("added");
        // No DigiKey offer for the YAGEO part; Mouser, C2's DigiKey and TI's
        // DigiKey are already listed. Farnell's cut tape beats its reel.
        assert_eq!(
            added,
            vec![
                ("C1".to_owned(), "Farnell".to_owned(), "3019495".to_owned()),
                ("C2".to_owned(), "Farnell".to_owned(), "3019495".to_owned()),
            ]
        );
        assert_eq!(
            updated.selections[0].alternates,
            vec![Offer {
                supplier: "Farnell".into(),
                supplier_part: "3019495".into(),
                source_url: Some("https://example.test/3019495".into())
            }]
        );
        for (before, after) in original.selections.iter().zip(&updated.selections) {
            let mut stripped = after.clone();
            stripped.alternates.truncate(before.alternates.len());
            assert_eq!(&stripped, before, "only alternates grow");
        }
        let (again, added) = add_alternates(&updated, &results, &distributors(), &criteria()).expect("added");
        assert!(added.is_empty(), "a second run adds nothing");
        assert_eq!(again, updated);
    }

    #[test]
    fn alternates_skip_tracking_links_and_conflicting_codes() {
        let mut original = lock();
        // U2 no longer lists DigiKey, so its DigiKey offer is a candidate.
        original.selections[2].supplier = Some("Mouser".into());
        original.selections[2].supplier_part = Some("595-TPS62130RGTR".into());
        // Another part already owns Farnell 3019495.
        let mut other: Selection = original.selections[0].clone();
        other.reference = "C9".into();
        other.manufacturer = Some("Murata".into());
        other.mpn = Some("GRM188R71H104KA93D".into());
        other.supplier = Some("Farnell".into());
        other.supplier_part = Some("3019495".into());
        original.selections.push(other);
        let mut results = results();
        results[0].offers[0].url = Some("https://octopart.com/click/track?x=1".into());
        let (updated, added) = add_alternates(&original, &results, &distributors(), &criteria()).expect("added");
        assert_eq!(
            added,
            vec![
                ("C1".to_owned(), "Farnell".to_owned(), "3019XXX".to_owned()),
                ("C2".to_owned(), "Farnell".to_owned(), "3019XXX".to_owned()),
                ("U2".to_owned(), "DigiKey".to_owned(), "296-38599-1-ND".to_owned()),
            ],
            "Farnell 3019495 belongs to the Murata part"
        );
        assert_eq!(updated.selections[2].alternates[0].source_url, None, "octopart click URL dropped");
    }

    #[test]
    fn alternates_refuse_legacy_locks() {
        let mut legacy = lock();
        legacy.electrical_sha256 = Some("0".repeat(64));
        assert!(add_alternates(&legacy, &[], &distributors(), &criteria()).is_err());
    }

    #[test]
    fn provider_selection_names_missing_variables() {
        let locale = Locale { country: "US".into(), currency: "USD".into() };
        let unknown = enabled_providers(Some(&["tme".to_owned()]), &locale).err().expect("refused");
        assert_eq!(unknown.0, "unknown offer provider 'tme' (choose from digikey, mouser, farnell, nexar)");
    }

    #[test]
    fn rate_limits_and_failures_are_clear() {
        let limited = checked("mouser", 429, String::new(), None).unwrap_err().0;
        assert!(limited.starts_with("mouser rate limit reached (HTTP 429)"), "{limited}");
        assert_eq!(
            checked("digikey", 401, String::new(), Some("Bad token".into())).unwrap_err().0,
            "digikey request failed (HTTP 401): Bad token"
        );
        let pacer = Pacer::new(Duration::from_millis(30));
        let start = Instant::now();
        pacer.wait();
        pacer.wait();
        assert!(start.elapsed() >= Duration::from_millis(30));
    }

    #[test]
    fn text_numbers_and_lead_times_parse() {
        assert_eq!(decimal("0,083 €"), Some(0.083));
        assert_eq!(decimal("$1.25"), Some(1.25));
        assert_eq!(decimal("1.234,50"), Some(1234.5));
        assert_eq!(decimal("1,234,567"), Some(1234567.0));
        assert_eq!(integer(Some(&Value::from("12,345 In Stock"))), Some(12345));
        assert_eq!(lead_days("84 Days", false), Some(84));
        assert_eq!(lead_days("12 Weeks", false), Some(84));
        assert_eq!(lead_days("13", true), Some(91));
        assert_eq!(
            shown_url("https://api.mouser.com/api/v1/search/partnumber?apiKey=k"),
            "https://api.mouser.com/api/v1/search/partnumber"
        );
    }

    #[test]
    fn curl_config_strings_are_escaped_and_secrets_are_redacted() {
        assert_eq!(curl_quoted(r#"{"a":"b\c"}"#), r#""{\"a\":\"b\\c\"}""#);
        assert_eq!(curl_quoted("x\ny"), r#""x\ny""#);
        assert_eq!(format!("{:?}", Secret::new("hunter2")), "<redacted>");
    }

    #[test]
    fn timestamps_are_utc() {
        assert_eq!(utc_timestamp(0), "1970-01-01T00:00:00Z");
        assert_eq!(utc_timestamp(951_782_400), "2000-02-29T00:00:00Z");
        assert_eq!(utc_timestamp(1_793_663_999), "2026-11-02T23:59:59Z");
    }
}
