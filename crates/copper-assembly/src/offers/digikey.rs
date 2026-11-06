//! DigiKey Product Information API v4: keyword search per MPN, one offer per
//! package variation (cut tape, reel, Digi-Reel). Create a production app
//! with the Product Information API at <https://developer.digikey.com>; set
//! `DIGIKEY_CLIENT_ID` and `DIGIKEY_CLIENT_SECRET`. Two-legged OAuth (client
//! credentials); limits are 120 requests a minute and 1000 a day.

use std::time::Duration;

use copper_json::{Dumps, Object, Value};

use super::{
    DistributorOffer, Found, Line, Locale, Pacer, PriceBreak, Provider, Secret, Transport, checked, env, fail,
    form_encode, integer, items, lead_days, number, requested, string,
};
use crate::assembly::Result;

pub const TOKEN_URL: &str = "https://api.digikey.com/v1/oauth2/token";
pub const SEARCH_URL: &str = "https://api.digikey.com/products/v4/search/keyword";
const SELLER: &str = "DigiKey";

pub struct DigiKey {
    client_id: Secret,
    client_secret: Secret,
    locale: Locale,
    pacer: Pacer,
}

impl DigiKey {
    pub fn new(client_id: Secret, client_secret: Secret, locale: Locale, pacer: Pacer) -> DigiKey {
        DigiKey { client_id, client_secret, locale, pacer }
    }

    pub fn from_env(locale: &Locale) -> Option<DigiKey> {
        Some(DigiKey::new(
            Secret::new(env("DIGIKEY_CLIENT_ID")?),
            Secret::new(env("DIGIKEY_CLIENT_SECRET")?),
            locale.clone(),
            // 120 a minute.
            Pacer::new(Duration::from_millis(550)),
        ))
    }

    fn access_token(&self, transport: &dyn Transport) -> Result<Secret> {
        let body = format!(
            "client_id={}&client_secret={}&grant_type=client_credentials",
            form_encode(self.client_id.expose()),
            form_encode(self.client_secret.expose())
        );
        let headers = [("Content-Type", "application/x-www-form-urlencoded".to_owned())];
        self.pacer.wait();
        let (status, response) = transport.send("POST", TOKEN_URL, &headers, Some(&body))?;
        let document = copper_json::parse(&response).ok();
        let field = |key: &str| string(document.as_ref().and_then(|d| d.get(key)));
        match field("access_token") {
            Some(token) if status == 200 => Ok(Secret::new(token)),
            _ if status == 429 => checked("digikey", status, response, None).map(Secret::new),
            _ => fail(format!(
                "token request failed (HTTP {status}{})",
                field("error").map(|code| format!(": {code}")).unwrap_or_default()
            )),
        }
    }
}

/// `{"title", "detail"}` problem responses; never the request.
fn problem(response: &str) -> Option<String> {
    let document = copper_json::parse(response).ok()?;
    string(document.get("detail")).or_else(|| string(document.get("title")))
}

fn parse_product(product: &Value, distributor: &str, currency: &str) -> Found {
    let lead = match product.get("ManufacturerLeadWeeks") {
        Some(Value::Str(text)) => lead_days(text, true),
        other => integer(other).map(|weeks| weeks * 7),
    };
    let url = string(product.get("ProductUrl"));
    let offers = items(product.get("ProductVariations"))
        .iter()
        .filter_map(|variation| {
            let sku = string(variation.get("DigiKeyProductNumber"))?;
            let prices = items(variation.get("StandardPricing"))
                .iter()
                .filter_map(|price| {
                    Some(PriceBreak {
                        quantity: integer(price.get("BreakQuantity"))?,
                        price: number(price.get("UnitPrice"))?,
                        currency: currency.to_owned(),
                        converted_price: None,
                        converted_currency: None,
                    })
                })
                .collect();
            Some(DistributorOffer {
                distributor: distributor.to_owned(),
                seller: SELLER.into(),
                source: "digikey".into(),
                sku,
                stock: integer(variation.get("QuantityAvailableforPackageType")),
                factory_lead_days: lead,
                moq: integer(variation.get("MinimumOrderQuantity")),
                packaging: string(variation.get("PackageType").and_then(|p| p.get("Name"))),
                url: url.clone(),
                prices,
            })
        })
        .collect();
    Found {
        manufacturer: string(product.get("Manufacturer").and_then(|m| m.get("Name"))).unwrap_or_default(),
        mpn: string(product.get("ManufacturerProductNumber")).unwrap_or_default(),
        offers,
    }
}

impl Provider for DigiKey {
    fn name(&self) -> &'static str {
        "digikey"
    }

    fn serves(&self, distributors: &[String]) -> bool {
        requested(distributors, SELLER).is_some()
    }

    fn fetch(&self, transport: &dyn Transport, lines: &[Line], distributors: &[String]) -> Result<Vec<Vec<Found>>> {
        let Some(distributor) = requested(distributors, SELLER) else { return Ok(vec![Vec::new(); lines.len()]) };
        let token = self.access_token(transport)?;
        let headers = [
            ("Content-Type", "application/json".to_owned()),
            ("Authorization", format!("Bearer {}", token.expose())),
            ("X-DIGIKEY-Client-Id", self.client_id.expose().to_owned()),
            ("X-DIGIKEY-Locale-Site", self.locale.country.clone()),
            ("X-DIGIKEY-Locale-Language", "en".to_owned()),
            ("X-DIGIKEY-Locale-Currency", self.locale.currency.clone()),
        ];
        let mut results = Vec::new();
        for line in lines {
            let mut request = Object::new();
            request.insert("Keywords".into(), Value::from(line.mpn.as_str()));
            request.insert("Limit".into(), Value::from(10i64));
            request.insert("Offset".into(), Value::from(0i64));
            self.pacer.wait();
            let (status, response) =
                transport.send("POST", SEARCH_URL, &headers, Some(&Dumps::compact().dumps(&Value::Object(request))))?;
            let detail = problem(&response);
            let response = checked("digikey", status, response, detail)?;
            let document = copper_json::parse(&response)
                .map_err(|_| crate::assembly::AssemblyError("digikey returned malformed JSON".into()))?;
            let currency = string(document.get("SearchLocaleUsed").and_then(|l| l.get("Currency")))
                .unwrap_or_else(|| self.locale.currency.clone());
            let products = items(document.get("ExactMatches")).iter().chain(items(document.get("Products")));
            results.push(products.map(|product| parse_product(product, distributor, &currency)).collect());
        }
        Ok(results)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::offers::testing::{Recorded, distributors, lock};
    use crate::offers::{lines, select};

    const TOKEN: &str = r#"{"access_token":"dk-token","expires_in":599,"token_type":"Bearer"}"#;

    const TI: &str = r#"{"SearchLocaleUsed":{"Site":"NO","Language":"en","Currency":"EUR"},
      "ExactMatches":[{"ManufacturerProductNumber":"TPS62130RGTR","Manufacturer":{"Id":296,"Name":"Texas Instruments"},
        "ProductUrl":"https://www.digikey.no/en/products/detail/texas-instruments/TPS62130RGTR/3855384",
        "QuantityAvailable":8123,"ManufacturerLeadWeeks":"6",
        "ProductVariations":[
          {"DigiKeyProductNumber":"296-38599-1-ND","PackageType":{"Id":2,"Name":"Cut Tape (CT)"},"QuantityAvailableforPackageType":8123,"MinimumOrderQuantity":1,
           "StandardPricing":[{"BreakQuantity":1,"UnitPrice":2.64,"TotalPrice":2.64},{"BreakQuantity":10,"UnitPrice":1.98,"TotalPrice":19.8}]},
          {"DigiKeyProductNumber":"296-38599-2-ND","PackageType":{"Id":1,"Name":"Tape & Reel (TR)"},"QuantityAvailableforPackageType":0,"MinimumOrderQuantity":3000,"StandardPricing":[]}]}],
      "Products":[{"ManufacturerProductNumber":"TPS62130RGTT","Manufacturer":{"Name":"Texas Instruments"},"ProductVariations":[]}]}"#;

    const NOTHING: &str = r#"{"ExactMatches":[],"Products":[]}"#;

    fn digikey() -> DigiKey {
        DigiKey::new(
            Secret::new("dk-id"),
            Secret::new("dk-secret"),
            Locale { country: "NO".into(), currency: "EUR".into() },
            Pacer::new(Duration::ZERO),
        )
    }

    #[test]
    fn keyword_search_yields_one_offer_per_package() {
        let transport = Recorded::new(vec![(200, TOKEN), (200, TI), (200, NOTHING)]);
        let (lines, _) = lines(&lock());
        let found = digikey().fetch(&transport, &lines, &distributors()).expect("offers");
        let requests = transport.requests.borrow();
        assert_eq!(requests[0].1, TOKEN_URL);
        assert_eq!(
            requests[0].3.as_deref(),
            Some("client_id=dk-id&client_secret=dk-secret&grant_type=client_credentials")
        );
        assert_eq!((requests[1].0.as_str(), requests[1].1.as_str()), ("POST", SEARCH_URL));
        assert!(requests[1].2.contains(&("Authorization".to_owned(), "Bearer dk-token".to_owned())));
        assert!(requests[1].2.contains(&("X-DIGIKEY-Locale-Currency".to_owned(), "EUR".to_owned())));
        assert_eq!(requests[1].3.as_deref(), Some(r#"{"Keywords":"TPS62130RGTR","Limit":10,"Offset":0}"#));
        assert!(!requests[1].3.as_deref().unwrap_or("").contains("dk-secret"));

        let ti = select(&lines[0], found[0].clone(), &distributors());
        assert_eq!(ti.matched, Some(("Texas Instruments".to_owned(), "TPS62130RGTR".to_owned())));
        type Shown<'a> = (&'a str, Option<i64>, Option<i64>, Option<i64>);
        let shown: Vec<Shown> =
            ti.offers.iter().map(|o| (o.sku.as_str(), o.stock, o.moq, o.factory_lead_days)).collect();
        assert_eq!(
            shown,
            vec![("296-38599-1-ND", Some(8123), Some(1), Some(42)), ("296-38599-2-ND", Some(0), Some(3000), Some(42))]
        );
        assert_eq!(ti.offers[0].prices[1].price, 1.98);
        assert_eq!(ti.offers[0].prices[1].currency, "EUR");
        assert_eq!(ti.offers[0].distributor, "DigiKey");
        assert!(ti.offers[0].url.as_deref().is_some_and(|url| url.starts_with("https://www.digikey.no/")));
        assert!(select(&lines[1], found[1].clone(), &distributors()).matched.is_none());
    }

    #[test]
    fn rate_limits_and_problems_are_reported() {
        let (lines, _) = lines(&lock());
        let transport = Recorded::new(vec![(200, TOKEN), (429, r#"{"detail":"Daily ratelimit exceeded"}"#)]);
        let failure = digikey().fetch(&transport, &lines, &distributors()).unwrap_err().0;
        assert!(failure.starts_with("digikey rate limit reached (HTTP 429)"), "{failure}");
        let transport =
            Recorded::new(vec![(200, TOKEN), (401, r#"{"title":"Unauthorized","detail":"Bearer token expired"}"#)]);
        let failure = digikey().fetch(&transport, &lines, &distributors()).unwrap_err().0;
        assert_eq!(failure, "digikey request failed (HTTP 401): Bearer token expired");
        let transport = Recorded::new(vec![(401, r#"{"error":"invalid_client"}"#)]);
        assert_eq!(
            digikey().fetch(&transport, &lines, &distributors()).unwrap_err().0,
            "token request failed (HTTP 401: invalid_client)"
        );
    }

    #[test]
    fn unrequested_distributor_is_not_queried() {
        assert!(!digikey().serves(&["Mouser".to_owned()]));
        assert!(digikey().serves(&["Digi-Key".to_owned()]));
    }
}
