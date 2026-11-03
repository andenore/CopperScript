//! Live distributor offers for an assembly lock from the Nexar Supply API
//! (Octopart's official API). This is the only networked path in the crate:
//! the report is timestamped, not deterministic, and nothing here runs from
//! check, compile, lock or BOM export. Credentials come from the environment
//! only and are never logged or written.

use std::io::Write;
use std::process::{Command, Stdio};

use copper_json::{Dumps, Object, Value, python_casefold};
use num_traits::ToPrimitive;

use crate::assembly::{AssemblyError, Lock, Offer, Result, SCHEMA};

pub const REPORT_SCHEMA: &str = "copperscript-assembly-offers/v0.1";
pub const TOKEN_URL: &str = "https://identity.nexar.com/connect/token";
pub const GRAPHQL_URL: &str = "https://api.nexar.com/graphql";
pub const CLIENT_ID_VARIABLE: &str = "NEXAR_CLIENT_ID";
pub const CLIENT_SECRET_VARIABLE: &str = "NEXAR_CLIENT_SECRET";
/// Match queries per GraphQL request.
const BATCH: usize = 20;
/// Parts returned per match query; the exact MPN is chosen among them.
const PARTS_PER_QUERY: i64 = 3;

const QUERY: &str = "query CopperOffers($queries: [SupPartMatchQuery!]!, $country: String!, $currency: String!) {
  supMultiMatch(queries: $queries, country: $country, currency: $currency) {
    reference
    parts {
      mpn
      manufacturer { name }
      sellers(authorizedOnly: true) {
        company { name }
        offers {
          sku
          inventoryLevel
          factoryLeadDays
          moq
          packaging
          clickUrl
          prices { quantity price currency convertedPrice convertedCurrency }
        }
      }
    }
  }
}";

fn fail<T>(message: impl Into<String>) -> Result<T> {
    Err(AssemblyError(message.into()))
}

/// One HTTPS POST; tests substitute a recorded exchange.
pub trait Transport {
    /// POST `body` to `url` and return the HTTP status and response body.
    fn post(&self, url: &str, headers: &[(&str, String)], body: &str) -> Result<(u16, String)>;
}

/// `curl` with its whole request, secrets included, on standard input, so no
/// credential appears in a command line or process listing.
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

impl Transport for Curl {
    fn post(&self, url: &str, headers: &[(&str, String)], body: &str) -> Result<(u16, String)> {
        let mut config =
            format!("url = {}\nrequest = \"POST\"\nsilent\nshow-error\nmax-time = 120\n", curl_quoted(url));
        for (name, value) in headers {
            config.push_str(&format!("header = {}\n", curl_quoted(&format!("{name}: {value}"))));
        }
        config.push_str(&format!("data-binary = {}\nwrite-out = \"\\n%{{http_code}}\"\n", curl_quoted(body)));
        let mut child = Command::new("curl")
            .args(["--config", "-"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|failure| AssemblyError(format!("cannot run curl for the Nexar request: {failure}")))?;
        child
            .stdin
            .take()
            .expect("piped stdin")
            .write_all(config.as_bytes())
            .map_err(|failure| AssemblyError(format!("cannot run curl for the Nexar request: {failure}")))?;
        let output = child
            .wait_with_output()
            .map_err(|failure| AssemblyError(format!("cannot run curl for the Nexar request: {failure}")))?;
        let text = String::from_utf8_lossy(&output.stdout).into_owned();
        if !output.status.success() {
            // curl's own message names the host, never the request body.
            let message = String::from_utf8_lossy(&output.stderr).trim().to_owned();
            return fail(format!("Nexar request to {url} failed: {message}"));
        }
        let (body, status) = text.rsplit_once('\n').unwrap_or(("", text.as_str()));
        let status = status.trim().parse().map_err(|_| AssemblyError(format!("Nexar request to {url} failed")))?;
        Ok((status, body.to_owned()))
    }
}

/// Nexar client credentials; `Debug` never shows them.
pub struct Credentials {
    client_id: String,
    client_secret: String,
}

impl std::fmt::Debug for Credentials {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("Credentials(<redacted>)")
    }
}

impl Credentials {
    pub fn new(client_id: String, client_secret: String) -> Credentials {
        Credentials { client_id, client_secret }
    }

    /// `NEXAR_CLIENT_ID` and `NEXAR_CLIENT_SECRET`; nothing else is read.
    pub fn from_env() -> Result<Credentials> {
        let read = |name: &str| std::env::var(name).ok().filter(|value| !value.trim().is_empty());
        match (read(CLIENT_ID_VARIABLE), read(CLIENT_SECRET_VARIABLE)) {
            (Some(client_id), Some(client_secret)) => Ok(Credentials { client_id, client_secret }),
            _ => fail(format!(
                "set {CLIENT_ID_VARIABLE} and {CLIENT_SECRET_VARIABLE} to a Nexar application's client \
                 credentials (supply.domain scope) to look up offers"
            )),
        }
    }
}

/// `application/x-www-form-urlencoded` value.
fn form_encode(text: &str) -> String {
    text.bytes()
        .map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => (byte as char).to_string(),
            _ => format!("%{byte:02X}"),
        })
        .collect()
}

/// A client-credentials access token for the Supply API.
pub fn access_token(transport: &dyn Transport, credentials: &Credentials) -> Result<String> {
    let body = format!(
        "grant_type=client_credentials&client_id={}&client_secret={}&scope=supply.domain",
        form_encode(&credentials.client_id),
        form_encode(&credentials.client_secret)
    );
    let headers = [("Content-Type", "application/x-www-form-urlencoded".to_owned())];
    let (status, response) = transport.post(TOKEN_URL, &headers, &body)?;
    let document = copper_json::parse(&response).ok();
    let field = |key: &str| document.as_ref().and_then(|d| d.get(key)).and_then(Value::as_str).map(str::to_owned);
    match field("access_token") {
        Some(token) if status == 200 && !token.is_empty() => Ok(token),
        // The OAuth error code ("invalid_client") is safe to show; the body
        // is not echoed in case a server reflects the request.
        _ => fail(format!(
            "Nexar token request failed (HTTP {status}{})",
            field("error").map(|code| format!(": {code}")).unwrap_or_default()
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

/// A manufacturer name as Octopart may spell it: `Murata` matches
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
    /// In the requested currency, when Nexar converted it.
    pub converted_price: Option<f64>,
    pub converted_currency: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DistributorOffer {
    /// The requested distributor's spelling (`DigiKey`).
    pub distributor: String,
    /// Nexar's seller name (`Digi-Key`).
    pub seller: String,
    pub sku: String,
    /// None where the seller reports no inventory figure.
    pub stock: Option<i64>,
    pub factory_lead_days: Option<i64>,
    pub moq: Option<i64>,
    pub packaging: Option<String>,
    pub url: Option<String>,
    pub prices: Vec<PriceBreak>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LineOffers {
    pub line: Line,
    /// Nexar's spelling of the exactly matched part, or None.
    pub matched: Option<(String, String)>,
    /// Parts Nexar returned that were not an exact match, for review.
    pub candidates: Vec<(String, String)>,
    pub offers: Vec<DistributorOffer>,
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

fn integer(value: Option<&Value>) -> Option<i64> {
    match value? {
        Value::Int(number) => number.to_i64(),
        Value::Float(number) if number.is_finite() => Some(*number as i64),
        _ => None,
    }
}

fn number(value: Option<&Value>) -> Option<f64> {
    match value? {
        Value::Int(number) => number.to_f64(),
        Value::Float(number) => Some(*number),
        _ => None,
    }
}

fn string(value: Option<&Value>) -> Option<String> {
    value.and_then(Value::as_str).filter(|text| !text.is_empty()).map(str::to_owned)
}

fn items(value: Option<&Value>) -> &[Value] {
    value.and_then(Value::as_array).unwrap_or(&[])
}

fn request_body(lines: &[Line], country: &str, currency: &str) -> String {
    let queries = lines
        .iter()
        .enumerate()
        .map(|(index, line)| {
            let mut query = Object::new();
            query.insert("reference".into(), Value::from(index.to_string()));
            query.insert("mpn".into(), Value::from(line.mpn.as_str()));
            query.insert("manufacturer".into(), Value::from(line.manufacturer.as_str()));
            query.insert("limit".into(), Value::from(PARTS_PER_QUERY));
            Value::Object(query)
        })
        .collect();
    let mut variables = Object::new();
    variables.insert("queries".into(), Value::Array(queries));
    variables.insert("country".into(), Value::from(country));
    variables.insert("currency".into(), Value::from(currency));
    let mut document = Object::new();
    document.insert("query".into(), Value::from(QUERY));
    document.insert("variables".into(), Value::Object(variables));
    Dumps::compact().dumps(&Value::Object(document))
}

fn parse_offers(part: &Value, distributors: &[String]) -> Vec<DistributorOffer> {
    let mut offers = Vec::new();
    for seller in items(part.get("sellers")) {
        let Some(name) = string(seller.get("company").and_then(|company| company.get("name"))) else { continue };
        let Some(distributor) = distributors.iter().find(|wanted| distributor_key(wanted) == distributor_key(&name))
        else {
            continue;
        };
        for offer in items(seller.get("offers")) {
            let Some(sku) = string(offer.get("sku")) else { continue };
            let prices = items(offer.get("prices"))
                .iter()
                .filter_map(|price| {
                    Some(PriceBreak {
                        quantity: integer(price.get("quantity"))?,
                        price: number(price.get("price"))?,
                        currency: string(price.get("currency"))?,
                        converted_price: number(price.get("convertedPrice")),
                        converted_currency: string(price.get("convertedCurrency")),
                    })
                })
                .collect();
            offers.push(DistributorOffer {
                distributor: distributor.clone(),
                seller: name.clone(),
                sku,
                stock: integer(offer.get("inventoryLevel")).filter(|stock| *stock >= 0),
                factory_lead_days: integer(offer.get("factoryLeadDays")).filter(|days| *days >= 0),
                moq: integer(offer.get("moq")),
                packaging: string(offer.get("packaging")),
                url: string(offer.get("clickUrl")),
                prices,
            });
        }
    }
    // Stable report order: distributor as requested, then SKU.
    offers.sort_by(|a, b| {
        let rank = |offer: &DistributorOffer| distributors.iter().position(|d| *d == offer.distributor);
        (rank(a), &a.sku).cmp(&(rank(b), &b.sku))
    });
    offers.dedup_by(|a, b| a.distributor == b.distributor && a.sku == b.sku);
    offers
}

fn parse_match(line: &Line, matched: Option<&Value>, distributors: &[String]) -> LineOffers {
    let parts = items(matched.and_then(|matched| matched.get("parts")));
    let identity = |part: &Value| {
        (
            string(part.get("manufacturer").and_then(|m| m.get("name"))).unwrap_or_default(),
            string(part.get("mpn")).unwrap_or_default(),
        )
    };
    let exact = parts.iter().find(|part| {
        let (manufacturer, mpn) = identity(part);
        python_casefold(&mpn) == python_casefold(&line.mpn) && same_manufacturer(&line.manufacturer, &manufacturer)
    });
    match exact {
        Some(part) => LineOffers {
            line: line.clone(),
            matched: Some(identity(part)),
            candidates: Vec::new(),
            offers: parse_offers(part, distributors),
        },
        None => LineOffers {
            line: line.clone(),
            matched: None,
            candidates: parts.iter().map(identity).collect(),
            offers: Vec::new(),
        },
    }
}

/// Look up every line, `BATCH` match queries per request.
pub fn fetch(
    transport: &dyn Transport,
    token: &str,
    lines: &[Line],
    distributors: &[String],
    country: &str,
    currency: &str,
) -> Result<Vec<LineOffers>> {
    let headers = [("Content-Type", "application/json".to_owned()), ("Authorization", format!("Bearer {token}"))];
    let mut results = Vec::new();
    for batch in lines.chunks(BATCH) {
        let (status, response) = transport.post(GRAPHQL_URL, &headers, &request_body(batch, country, currency))?;
        let document = copper_json::parse(&response).ok();
        let error =
            document.as_ref().and_then(|d| items(d.get("errors")).first().and_then(|e| string(e.get("message"))));
        let matches = document.as_ref().and_then(|d| d.get("data")).and_then(|d| d.get("supMultiMatch"));
        let Some(matches) = matches.and_then(Value::as_array).filter(|_| status == 200) else {
            return fail(format!(
                "Nexar Supply query failed (HTTP {status}){}",
                error.map(|message| format!(": {message}")).unwrap_or_default()
            ));
        };
        for (index, line) in batch.iter().enumerate() {
            let matched =
                matches.iter().find(|m| string(m.get("reference")).as_deref() == Some(index.to_string().as_str()));
            results.push(parse_match(line, matched, distributors));
        }
    }
    Ok(results)
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
    pub country: &'a str,
    pub currency: &'a str,
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
            let offers = result
                .offers
                .iter()
                .map(|offer| {
                    let mut entry = Object::new();
                    entry.insert("distributor".into(), Value::from(offer.distributor.as_str()));
                    entry.insert("seller".into(), Value::from(offer.seller.as_str()));
                    entry.insert("sku".into(), Value::from(offer.sku.as_str()));
                    entry.insert("stock".into(), optional_int(offer.stock));
                    entry.insert("factory_lead_days".into(), optional_int(offer.factory_lead_days));
                    entry.insert("moq".into(), optional_int(offer.moq));
                    entry.insert("packaging".into(), offer.packaging.as_deref().map_or(Value::Null, Value::from));
                    entry.insert("url".into(), offer.url.as_deref().map_or(Value::Null, Value::from));
                    entry.insert("adequate".into(), Value::Bool(criteria.adequate(offer, needed)));
                    let prices = offer
                        .prices
                        .iter()
                        .map(|price| {
                            let mut item = Object::new();
                            item.insert("quantity".into(), Value::from(price.quantity));
                            item.insert("price".into(), Value::from(price.price));
                            item.insert("currency".into(), Value::from(price.currency.as_str()));
                            if let (Some(converted), Some(currency)) =
                                (price.converted_price, &price.converted_currency)
                            {
                                item.insert("converted_price".into(), Value::from(converted));
                                item.insert("converted_currency".into(), Value::from(currency.as_str()));
                            }
                            Value::Object(item)
                        })
                        .collect();
                    entry.insert("prices".into(), Value::Array(prices));
                    Value::Object(entry)
                })
                .collect();
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
    let mut requirements = Object::new();
    requirements.insert("boards".into(), Value::from(criteria.quantity));
    requirements.insert("min_stock".into(), criteria.min_stock.map_or(Value::Null, Value::from));
    requirements.insert("within_weeks".into(), criteria.within_weeks.map_or(Value::Null, Value::from));
    let mut document = Object::new();
    document.insert("schema".into(), Value::from(REPORT_SCHEMA));
    document.insert("board".into(), Value::from(request.board));
    document.insert("lock".into(), Value::from(request.lock));
    document.insert("generated_at".into(), Value::from(request.generated_at));
    document.insert("source".into(), Value::from("Nexar Supply API (Octopart), authorized sellers"));
    document.insert("deterministic".into(), Value::Bool(false));
    document.insert("country".into(), Value::from(request.country));
    document.insert("currency".into(), Value::from(request.currency));
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
    use std::cell::RefCell;

    use super::*;
    use crate::assembly::Selection;

    /// Replays canned responses and records every request.
    struct Recorded {
        responses: RefCell<Vec<(u16, String)>>,
        requests: RefCell<Vec<(String, Vec<(String, String)>, String)>>,
    }

    impl Recorded {
        fn new(responses: Vec<(u16, &str)>) -> Recorded {
            Recorded {
                responses: RefCell::new(responses.into_iter().rev().map(|(s, b)| (s, b.to_owned())).collect()),
                requests: RefCell::new(Vec::new()),
            }
        }
    }

    impl Transport for Recorded {
        fn post(&self, url: &str, headers: &[(&str, String)], body: &str) -> Result<(u16, String)> {
            self.requests.borrow_mut().push((
                url.to_owned(),
                headers.iter().map(|(n, v)| ((*n).to_owned(), v.clone())).collect(),
                body.to_owned(),
            ));
            Ok(self.responses.borrow_mut().pop().expect("an expected request"))
        }
    }

    fn selection(reference: &str, manufacturer: &str, mpn: &str, supplier: &str, code: &str) -> Selection {
        Selection {
            reference: reference.into(),
            part: "pass.CAPACITOR".into(),
            footprint: Some("Capacitor_SMD:C_0603_1608Metric".into()),
            manufacturer: Some(manufacturer.into()),
            mpn: Some(mpn.into()),
            supplier: Some(supplier.into()),
            supplier_part: Some(code.into()),
            source_url: None,
            reviewed: true,
            populate: "consigned".into(),
            alternates: Vec::new(),
            electrical_sha256: Some("0".repeat(64)),
        }
    }

    fn lock() -> Lock {
        let mut c2 = selection("C2", "YAGEO", "CC0603KRX7R9BB104", "Mouser", "603-CC603KRX7R9BB104");
        c2.alternates.push(Offer {
            supplier: "DigiKey".into(),
            supplier_part: "311-1344-1-ND".into(),
            source_url: None,
        });
        let mut unfitted = selection("R9", "YAGEO", "RC0603FR-0710KL", "Mouser", "603-RC0603FR-0710KL");
        unfitted.populate = "none".into();
        let mut unknown = selection("U1", "x", "y", "Mouser", "1");
        unknown.manufacturer = None;
        Lock {
            board: "Demo".into(),
            electrical_sha256: None,
            selections: vec![
                selection("C1", "YAGEO", "CC0603KRX7R9BB104", "Mouser", "603-CC603KRX7R9BB104"),
                c2,
                selection("U2", "Texas Instruments", "TPS62130RGTR", "DigiKey", "296-38599-1-ND"),
                unfitted,
                unknown,
            ],
        }
    }

    const TOKEN: &str = r#"{"access_token":"tok-123","expires_in":86400,"token_type":"Bearer"}"#;

    const SUPPLY: &str = r#"{"data":{"supMultiMatch":[
      {"reference":"0","parts":[{"mpn":"TPS62130RGTR","manufacturer":{"name":"Texas Instruments"},"sellers":[
        {"company":{"name":"Digi-Key"},"offers":[{"sku":"296-38599-1-ND","inventoryLevel":5000,"factoryLeadDays":42,"moq":1,"packaging":"Cut Tape","clickUrl":"https://octopart.com/click/track?x=1","prices":[{"quantity":1,"price":2.1,"currency":"USD","convertedPrice":1.9,"convertedCurrency":"EUR"}]}]},
        {"company":{"name":"Arrow"},"offers":[{"sku":"A1","inventoryLevel":9,"prices":[]}]}]}]},
      {"reference":"1","parts":[
        {"mpn":"CC0603KRX7R9BB104X","manufacturer":{"name":"Yageo"},"sellers":[]},
        {"mpn":"CC0603KRX7R9BB104","manufacturer":{"name":"Yageo"},"sellers":[
          {"company":{"name":"Farnell"},"offers":[
            {"sku":"3019XXX","inventoryLevel":10,"factoryLeadDays":70,"moq":5000,"packaging":"Reel","prices":[]},
            {"sku":"3019495","inventoryLevel":40000,"factoryLeadDays":70,"moq":10,"packaging":"Cut Tape","clickUrl":"https://uk.farnell.com/3019495","prices":[{"quantity":10,"price":0.01,"currency":"GBP"}]}]},
          {"company":{"name":"Mouser"},"offers":[{"sku":"603-CC603KRX7R9BB104","inventoryLevel":1,"factoryLeadDays":7,"moq":1,"prices":[]}]}]}]}
    ]}}"#;

    fn distributors() -> Vec<String> {
        vec!["DigiKey".into(), "Mouser".into(), "Farnell".into()]
    }

    fn criteria() -> Criteria {
        Criteria { quantity: 10, min_stock: None, within_weeks: None }
    }

    fn run(lock: &Lock) -> (Recorded, Vec<LineOffers>) {
        let transport = Recorded::new(vec![(200, TOKEN), (200, SUPPLY)]);
        let credentials = Credentials::new("id&1".into(), "s3cr=t".into());
        let token = access_token(&transport, &credentials).expect("a token");
        let (lines, _) = lines(lock);
        let results = fetch(&transport, &token, &lines, &distributors(), "NO", "EUR").expect("offers");
        (transport, results)
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
    fn credentials_go_only_to_the_token_endpoint_and_debug_hides_them() {
        let (transport, _) = run(&lock());
        let requests = transport.requests.borrow();
        assert_eq!(requests[0].0, TOKEN_URL);
        assert_eq!(
            requests[0].2,
            "grant_type=client_credentials&client_id=id%261&client_secret=s3cr%3Dt&scope=supply.domain"
        );
        assert_eq!(requests[1].0, GRAPHQL_URL);
        assert!(requests[1].1.contains(&("Authorization".to_owned(), "Bearer tok-123".to_owned())));
        assert!(!requests[1].2.contains("s3cr") && !requests[1].2.contains("id&1"));
        let body = copper_json::parse(&requests[1].2).expect("json");
        let variables = body.get("variables").expect("variables");
        assert_eq!(variables.get("currency").and_then(Value::as_str), Some("EUR"));
        assert_eq!(items(variables.get("queries")).len(), 2);
        let shown = format!("{:?}", Credentials::new("id&1".into(), "s3cr=t".into()));
        assert!(!shown.contains("s3cr") && !shown.contains("id&1"));
    }

    #[test]
    fn token_failures_report_the_oauth_code_only() {
        let transport = Recorded::new(vec![(400, r#"{"error":"invalid_client","echo":"s3cr=t"}"#)]);
        let failure = access_token(&transport, &Credentials::new("id".into(), "s3cr=t".into())).unwrap_err();
        assert_eq!(failure.0, "Nexar token request failed (HTTP 400: invalid_client)");
    }

    #[test]
    fn graphql_errors_fail_the_lookup() {
        let transport = Recorded::new(vec![(200, r#"{"data":null,"errors":[{"message":"quota exceeded"}]}"#)]);
        let (lines, _) = lines(&lock());
        let failure = fetch(&transport, "t", &lines, &distributors(), "US", "USD").unwrap_err();
        assert_eq!(failure.0, "Nexar Supply query failed (HTTP 200): quota exceeded");
    }

    #[test]
    fn offers_keep_exact_parts_and_requested_distributors() {
        let (_, results) = run(&lock());
        let ti = &results[0];
        assert_eq!(ti.matched, Some(("Texas Instruments".to_owned(), "TPS62130RGTR".to_owned())));
        assert_eq!(ti.offers.len(), 1, "Arrow is not requested");
        let offer = &ti.offers[0];
        assert_eq!((offer.distributor.as_str(), offer.seller.as_str()), ("DigiKey", "Digi-Key"));
        assert_eq!((offer.stock, offer.factory_lead_days, offer.moq), (Some(5000), Some(42), Some(1)));
        assert_eq!(offer.prices[0].converted_price, Some(1.9));
        let yageo = &results[1];
        assert_eq!(yageo.matched.as_ref().map(|m| m.1.as_str()), Some("CC0603KRX7R9BB104"), "not the X variant");
        let skus: Vec<&str> = yageo.offers.iter().map(|o| o.sku.as_str()).collect();
        assert_eq!(skus, vec!["603-CC603KRX7R9BB104", "3019495", "3019XXX"]);
    }

    #[test]
    fn unmatched_lines_keep_their_candidates() {
        let supply = r#"{"data":{"supMultiMatch":[{"reference":"0","parts":[{"mpn":"TPS62130RGTT","manufacturer":{"name":"Texas Instruments"},"sellers":[]}]}]}}"#;
        let transport = Recorded::new(vec![(200, supply)]);
        let (lines, _) = lines(&lock());
        let results = fetch(&transport, "t", &lines[..1], &distributors(), "US", "USD").expect("offers");
        assert_eq!(results[0].matched, None);
        assert_eq!(results[0].candidates, vec![("Texas Instruments".to_owned(), "TPS62130RGTT".to_owned())]);
        assert_eq!(shortfall_reason(&results[0]), "no exact manufacturer part match");
    }

    #[test]
    fn criteria_decide_shortfalls() {
        let (_, results) = run(&lock());
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
    fn report_is_timestamped_and_lists_shortfalls() {
        let (_, results) = run(&lock());
        let (_, unidentified) = lines(&lock());
        let distributors = distributors();
        let request = Request {
            board: "Demo",
            lock: "assembly.lock",
            generated_at: &utc_timestamp(1_793_620_800),
            distributors: &distributors,
            country: "NO",
            currency: "EUR",
            criteria: Criteria { min_stock: Some(10_000), ..criteria() },
        };
        let document = copper_json::parse(&report(&request, &results, &unidentified)).expect("json");
        assert_eq!(document.get("schema").and_then(Value::as_str), Some(REPORT_SCHEMA));
        assert_eq!(document.get("generated_at").and_then(Value::as_str), Some("2026-11-02T12:00:00Z"));
        assert_eq!(document.get("deterministic"), Some(&Value::Bool(false)));
        let shortfall = items(document.get("shortfall"));
        assert_eq!(shortfall.len(), 1);
        assert_eq!(shortfall[0].get("mpn").and_then(Value::as_str), Some("TPS62130RGTR"));
        assert_eq!(items(document.get("unidentified")), &[Value::from("U1")]);
        let offer = &items(items(document.get("lines"))[1].get("offers"))[1];
        assert_eq!(offer.get("sku").and_then(Value::as_str), Some("3019495"));
        assert_eq!(offer.get("adequate"), Some(&Value::Bool(true)));
        assert!(!report(&request, &results, &unidentified).contains("tok-123"));
    }

    #[test]
    fn alternates_add_missing_distributors_and_change_nothing_else() {
        let original = lock();
        let (_, results) = run(&original);
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
        let c1 = &updated.selections[0];
        assert_eq!(
            c1.alternates,
            vec![Offer {
                supplier: "Farnell".into(),
                supplier_part: "3019495".into(),
                source_url: Some("https://uk.farnell.com/3019495".into())
            }]
        );
        for (before, after) in original.selections.iter().zip(&updated.selections) {
            let mut stripped = after.clone();
            stripped.alternates.truncate(before.alternates.len());
            assert_eq!(&stripped, before, "only alternates grow");
        }
        // A second run adds nothing.
        let (again, added) = add_alternates(&updated, &results, &distributors(), &criteria()).expect("added");
        assert!(added.is_empty());
        assert_eq!(again, updated);
    }

    #[test]
    fn alternates_skip_tracking_links_and_conflicting_codes() {
        let mut original = lock();
        // TI U2 lists DigiKey already; drop it so the tracked DigiKey offer is a candidate.
        original.selections[2].supplier = Some("Mouser".into());
        original.selections[2].supplier_part = Some("595-TPS62130RGTR".into());
        // Another part already owns Farnell 3019495.
        let mut other = selection("C9", "Murata", "GRM188R71H104KA93D", "Farnell", "3019495");
        other.populate = "supplier".into();
        original.selections.push(other);
        let (_, results) = run(&lock());
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
    fn curl_config_strings_are_escaped() {
        assert_eq!(curl_quoted(r#"{"a":"b\c"}"#), r#""{\"a\":\"b\\c\"}""#);
        assert_eq!(curl_quoted("x\ny"), r#""x\ny""#);
    }

    #[test]
    fn timestamps_are_utc() {
        assert_eq!(utc_timestamp(0), "1970-01-01T00:00:00Z");
        assert_eq!(utc_timestamp(951_782_400), "2000-02-29T00:00:00Z");
        assert_eq!(utc_timestamp(1_793_663_999), "2026-11-02T23:59:59Z");
    }
}
