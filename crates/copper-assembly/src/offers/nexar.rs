//! Nexar Supply API (Octopart): every authorized seller in one batched query.
//! Register an application at <https://portal.nexar.com> with the Supply
//! scope; set `NEXAR_CLIENT_ID` and `NEXAR_CLIENT_SECRET`.

use copper_json::{Dumps, Object, Value};

use super::{
    DistributorOffer, Found, Line, Locale, Pacer, PriceBreak, Provider, Secret, Transport, env, fail, form_encode,
    integer, items, number, requested, string,
};
use crate::assembly::Result;

pub const TOKEN_URL: &str = "https://identity.nexar.com/connect/token";
pub const GRAPHQL_URL: &str = "https://api.nexar.com/graphql";
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

pub struct Nexar {
    client_id: Secret,
    client_secret: Secret,
    locale: Locale,
    pacer: Pacer,
}

impl Nexar {
    pub fn new(client_id: Secret, client_secret: Secret, locale: Locale, pacer: Pacer) -> Nexar {
        Nexar { client_id, client_secret, locale, pacer }
    }

    pub fn from_env(locale: &Locale) -> Option<Nexar> {
        Some(Nexar::new(
            Secret::new(env("NEXAR_CLIENT_ID")?),
            Secret::new(env("NEXAR_CLIENT_SECRET")?),
            locale.clone(),
            Pacer::new(std::time::Duration::from_millis(250)),
        ))
    }

    /// A client-credentials access token for the Supply API.
    fn access_token(&self, transport: &dyn Transport) -> Result<Secret> {
        let body = format!(
            "grant_type=client_credentials&client_id={}&client_secret={}&scope=supply.domain",
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
            // The OAuth error code ("invalid_client") is safe to show; the
            // body is not echoed in case a server reflects the request.
            _ => fail(format!(
                "token request failed (HTTP {status}{})",
                field("error").map(|code| format!(": {code}")).unwrap_or_default()
            )),
        }
    }

    fn request_body(&self, lines: &[Line]) -> String {
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
        variables.insert("country".into(), Value::from(self.locale.country.as_str()));
        variables.insert("currency".into(), Value::from(self.locale.currency.as_str()));
        let mut document = Object::new();
        document.insert("query".into(), Value::from(QUERY));
        document.insert("variables".into(), Value::Object(variables));
        Dumps::compact().dumps(&Value::Object(document))
    }
}

fn parse_offers(part: &Value, distributors: &[String]) -> Vec<DistributorOffer> {
    let mut offers = Vec::new();
    for seller in items(part.get("sellers")) {
        let Some(name) = string(seller.get("company").and_then(|company| company.get("name"))) else { continue };
        let Some(distributor) = requested(distributors, &name) else { continue };
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
                source: "nexar".into(),
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
    offers
}

impl Provider for Nexar {
    fn name(&self) -> &'static str {
        "nexar"
    }

    fn serves(&self, distributors: &[String]) -> bool {
        !distributors.is_empty()
    }

    fn fetch(&self, transport: &dyn Transport, lines: &[Line], distributors: &[String]) -> Result<Vec<Vec<Found>>> {
        let token = self.access_token(transport)?;
        let headers =
            [("Content-Type", "application/json".to_owned()), ("Authorization", format!("Bearer {}", token.expose()))];
        let mut results = Vec::new();
        for batch in lines.chunks(BATCH) {
            self.pacer.wait();
            let (status, response) = transport.send("POST", GRAPHQL_URL, &headers, Some(&self.request_body(batch)))?;
            if status == 429 {
                return super::checked("nexar", status, response, None).map(|_| Vec::new());
            }
            let document = copper_json::parse(&response).ok();
            let error =
                document.as_ref().and_then(|d| items(d.get("errors")).first().and_then(|e| string(e.get("message"))));
            let matches = document.as_ref().and_then(|d| d.get("data")).and_then(|d| d.get("supMultiMatch"));
            let Some(matches) = matches.and_then(Value::as_array).filter(|_| status == 200) else {
                return fail(format!(
                    "Supply query failed (HTTP {status}){}",
                    error.map(|message| format!(": {message}")).unwrap_or_default()
                ));
            };
            for index in 0..batch.len() {
                let reference = index.to_string();
                let matched = matches.iter().find(|m| string(m.get("reference")).as_deref() == Some(&reference));
                let parts = items(matched.and_then(|m| m.get("parts")))
                    .iter()
                    .map(|part| Found {
                        manufacturer: string(part.get("manufacturer").and_then(|m| m.get("name"))).unwrap_or_default(),
                        mpn: string(part.get("mpn")).unwrap_or_default(),
                        offers: parse_offers(part, distributors),
                    })
                    .collect();
                results.push(parts);
            }
        }
        Ok(results)
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use crate::offers::testing::{Recorded, distributors, lock};
    use crate::offers::{lines, select};

    const TOKEN: &str = r#"{"access_token":"tok-123","expires_in":86400,"token_type":"Bearer"}"#;

    const SUPPLY: &str = r#"{"data":{"supMultiMatch":[
      {"reference":"0","parts":[{"mpn":"TPS62130RGTR","manufacturer":{"name":"Texas Instruments"},"sellers":[
        {"company":{"name":"Digi-Key"},"offers":[{"sku":"296-38599-1-ND","inventoryLevel":5000,"factoryLeadDays":42,"moq":1,"packaging":"Cut Tape","clickUrl":"https://octopart.com/click/track?x=1","prices":[{"quantity":1,"price":2.1,"currency":"USD","convertedPrice":1.9,"convertedCurrency":"EUR"}]}]},
        {"company":{"name":"Arrow"},"offers":[{"sku":"A1","inventoryLevel":9,"prices":[]}]}]}]},
      {"reference":"1","parts":[
        {"mpn":"CC0603KRX7R9BB104X","manufacturer":{"name":"Yageo"},"sellers":[]},
        {"mpn":"CC0603KRX7R9BB104","manufacturer":{"name":"Yageo"},"sellers":[
          {"company":{"name":"Farnell"},"offers":[{"sku":"3019495","inventoryLevel":-1,"moq":10,"prices":[]}]}]}]}
    ]}}"#;

    fn nexar() -> Nexar {
        Nexar::new(
            Secret::new("id&1"),
            Secret::new("s3cr=t"),
            Locale { country: "NO".into(), currency: "EUR".into() },
            Pacer::new(Duration::ZERO),
        )
    }

    #[test]
    fn credentials_go_only_to_the_token_endpoint() {
        let transport = Recorded::new(vec![(200, TOKEN), (200, SUPPLY)]);
        let (lines, _) = lines(&lock());
        let found = nexar().fetch(&transport, &lines, &distributors()).expect("offers");
        let requests = transport.requests.borrow();
        assert_eq!((requests[0].0.as_str(), requests[0].1.as_str()), ("POST", TOKEN_URL));
        assert_eq!(
            requests[0].3.as_deref(),
            Some("grant_type=client_credentials&client_id=id%261&client_secret=s3cr%3Dt&scope=supply.domain")
        );
        assert_eq!(requests[1].1, GRAPHQL_URL);
        assert!(requests[1].2.contains(&("Authorization".to_owned(), "Bearer tok-123".to_owned())));
        let body = requests[1].3.clone().expect("a body");
        assert!(!body.contains("s3cr") && !body.contains("id&1"));
        let variables = copper_json::parse(&body).expect("json").get("variables").cloned().expect("variables");
        assert_eq!(variables.get("currency").and_then(Value::as_str), Some("EUR"));
        assert_eq!(items(variables.get("queries")).len(), 2);

        let ti = select(&lines[0], found[0].clone(), &distributors());
        assert_eq!(ti.offers.len(), 1, "Arrow is not requested");
        let offer = &ti.offers[0];
        assert_eq!((offer.distributor.as_str(), offer.seller.as_str()), ("DigiKey", "Digi-Key"));
        assert_eq!((offer.stock, offer.factory_lead_days, offer.moq), (Some(5000), Some(42), Some(1)));
        assert_eq!(offer.prices[0].converted_price, Some(1.9));
        let yageo = select(&lines[1], found[1].clone(), &distributors());
        assert_eq!(yageo.offers[0].stock, None, "a negative inventory level is unknown");
    }

    #[test]
    fn token_failures_report_the_oauth_code_only() {
        let transport = Recorded::new(vec![(400, r#"{"error":"invalid_client","echo":"s3cr=t"}"#)]);
        let failure = nexar().fetch(&transport, &[], &distributors()).unwrap_err();
        assert_eq!(failure.0, "token request failed (HTTP 400: invalid_client)");
    }

    #[test]
    fn graphql_errors_fail_the_lookup() {
        let quota = r#"{"data":null,"errors":[{"message":"You have exceeded your part limit of 0."}]}"#;
        let transport = Recorded::new(vec![(200, TOKEN), (200, quota)]);
        let (lines, _) = lines(&lock());
        let failure = nexar().fetch(&transport, &lines, &distributors()).unwrap_err();
        assert_eq!(failure.0, "Supply query failed (HTTP 200): You have exceeded your part limit of 0.");
    }
}
