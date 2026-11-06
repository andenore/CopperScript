//! Mouser Search API v1: exact part-number search per MPN. Request a Search
//! API key under My Account > APIs at <https://www.mouser.com/api-hub/>; set
//! `MOUSER_API_KEY`. Prices come in the account's currency. Limits are 30
//! requests a minute and 1000 a day; the key travels in the query string,
//! which is never shown.

use std::time::Duration;

use copper_json::{Dumps, Object, Value};

use super::{
    DistributorOffer, Found, Line, Pacer, PriceBreak, Provider, Secret, Transport, checked, env, fail, form_encode,
    integer, items, lead_days, number, requested, string,
};
use crate::assembly::Result;

pub const SEARCH_URL: &str = "https://api.mouser.com/api/v1/search/partnumber";
const SELLER: &str = "Mouser";

pub struct Mouser {
    api_key: Secret,
    pacer: Pacer,
}

impl Mouser {
    pub fn new(api_key: Secret, pacer: Pacer) -> Mouser {
        Mouser { api_key, pacer }
    }

    pub fn from_env() -> Option<Mouser> {
        // 30 a minute.
        Some(Mouser::new(Secret::new(env("MOUSER_API_KEY")?), Pacer::new(Duration::from_millis(2_100))))
    }
}

/// The first `Errors[].Message`, which Mouser also sends with HTTP 200.
fn error_message(document: Option<&Value>) -> Option<String> {
    document.and_then(|d| items(d.get("Errors")).first().and_then(|e| string(e.get("Message"))))
}

fn parse_part(part: &Value, distributor: &str) -> Found {
    let sku = string(part.get("MouserPartNumber")).filter(|sku| sku != "N/A");
    let offers = sku
        .map(|sku| {
            let prices = items(part.get("PriceBreaks"))
                .iter()
                .filter_map(|price| {
                    Some(PriceBreak {
                        quantity: integer(price.get("Quantity"))?,
                        price: number(price.get("Price"))?,
                        currency: string(price.get("Currency"))?,
                        converted_price: None,
                        converted_currency: None,
                    })
                })
                .collect();
            let stock = integer(part.get("AvailabilityInStock")).or_else(|| integer(part.get("Availability")));
            DistributorOffer {
                distributor: distributor.to_owned(),
                seller: SELLER.into(),
                source: "mouser".into(),
                sku,
                stock,
                factory_lead_days: string(part.get("LeadTime")).and_then(|text| lead_days(&text, false)),
                moq: integer(part.get("Min")),
                packaging: None,
                url: string(part.get("ProductDetailUrl")),
                prices,
            }
        })
        .into_iter()
        .collect();
    Found {
        manufacturer: string(part.get("Manufacturer")).unwrap_or_default(),
        mpn: string(part.get("ManufacturerPartNumber")).unwrap_or_default(),
        offers,
    }
}

impl Provider for Mouser {
    fn name(&self) -> &'static str {
        "mouser"
    }

    fn serves(&self, distributors: &[String]) -> bool {
        requested(distributors, SELLER).is_some()
    }

    fn fetch(&self, transport: &dyn Transport, lines: &[Line], distributors: &[String]) -> Result<Vec<Vec<Found>>> {
        let Some(distributor) = requested(distributors, SELLER) else { return Ok(vec![Vec::new(); lines.len()]) };
        let url = format!("{SEARCH_URL}?apiKey={}", form_encode(self.api_key.expose()));
        let headers = [("Content-Type", "application/json".to_owned()), ("Accept", "application/json".to_owned())];
        let mut results = Vec::new();
        for line in lines {
            let mut search = Object::new();
            search.insert("mouserPartNumber".into(), Value::from(line.mpn.as_str()));
            search.insert("partSearchOptions".into(), Value::from("Exact"));
            let mut request = Object::new();
            request.insert("SearchByPartRequest".into(), Value::Object(search));
            self.pacer.wait();
            let (status, response) =
                transport.send("POST", &url, &headers, Some(&Dumps::compact().dumps(&Value::Object(request))))?;
            let document = copper_json::parse(&response).ok();
            let message = error_message(document.as_ref());
            checked("mouser", status, String::new(), message.clone())?;
            if let Some(message) = message {
                return fail(format!("request failed: {message}"));
            }
            let parts = items(document.as_ref().and_then(|d| d.get("SearchResults")).and_then(|r| r.get("Parts")));
            results.push(parts.iter().map(|part| parse_part(part, distributor)).collect());
        }
        Ok(results)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::offers::testing::{Recorded, distributors, lock};
    use crate::offers::{lines, select};

    const YAGEO: &str = r#"{"Errors":[],"SearchResults":{"NumberOfResult":2,"Parts":[
      {"ManufacturerPartNumber":"CC0603KRX7R9BB104","Manufacturer":"YAGEO","MouserPartNumber":"603-CC603KRX7R9BB104",
       "Availability":"1,234,567 In Stock","AvailabilityInStock":"1234567","LeadTime":"84 Days","Min":"1","Mult":"1",
       "ProductDetailUrl":"https://eu.mouser.com/ProductDetail/YAGEO/CC0603KRX7R9BB104?qs=abc",
       "PriceBreaks":[{"Quantity":1,"Price":"0,09 €","Currency":"EUR"},{"Quantity":100,"Price":"0,018 €","Currency":"EUR"}]},
      {"ManufacturerPartNumber":"CC0603KRX7R9BB104","Manufacturer":"YAGEO","MouserPartNumber":"N/A","PriceBreaks":[]}]}}"#;

    fn mouser() -> Mouser {
        Mouser::new(Secret::new("mouser-key"), Pacer::new(Duration::ZERO))
    }

    #[test]
    fn exact_part_search_parses_localized_prices() {
        let (lines, _) = lines(&lock());
        let transport = Recorded::new(vec![
            (200, r#"{"Errors":[],"SearchResults":{"NumberOfResult":0,"Parts":[]}}"#),
            (200, YAGEO),
        ]);
        let found = mouser().fetch(&transport, &lines, &distributors()).expect("offers");
        let requests = transport.requests.borrow();
        assert_eq!(requests[0].1, format!("{SEARCH_URL}?apiKey=mouser-key"));
        assert_eq!(
            requests[1].3.as_deref(),
            Some(r#"{"SearchByPartRequest":{"mouserPartNumber":"CC0603KRX7R9BB104","partSearchOptions":"Exact"}}"#)
        );
        let yageo = select(&lines[1], found[1].clone(), &distributors());
        assert_eq!(yageo.offers.len(), 1, "N/A part numbers are not offers");
        let offer = &yageo.offers[0];
        assert_eq!(
            (offer.sku.as_str(), offer.stock, offer.factory_lead_days, offer.moq),
            ("603-CC603KRX7R9BB104", Some(1_234_567), Some(84), Some(1))
        );
        assert_eq!((offer.prices[1].quantity, offer.prices[1].price), (100, 0.018));
        assert!(found[0].is_empty());
    }

    #[test]
    fn errors_and_rate_limits_never_show_the_key() {
        let (lines, _) = lines(&lock());
        let invalid =
            r#"{"Errors":[{"Id":0,"Code":"Invalid","Message":"Invalid unique identifier."}],"SearchResults":null}"#;
        let failure = mouser().fetch(&Recorded::new(vec![(200, invalid)]), &lines, &distributors()).unwrap_err().0;
        assert_eq!(failure, "request failed: Invalid unique identifier.");
        let failure = mouser().fetch(&Recorded::new(vec![(429, "")]), &lines, &distributors()).unwrap_err().0;
        assert!(failure.starts_with("mouser rate limit reached (HTTP 429)") && !failure.contains("mouser-key"));
    }
}
