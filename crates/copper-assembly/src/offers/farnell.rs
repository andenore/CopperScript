//! Farnell / Newark / element14 Product Search API: manufacturer part number
//! search in one store. Register at <https://partner.element14.com>; set
//! `FARNELL_API_KEY` and optionally `FARNELL_STORE` (default
//! `uk.farnell.com`; for example `de.farnell.com`, `no.farnell.com`,
//! `www.newark.com`, `sg.element14.com`). Prices are in the store's
//! currency. The default limits are 2 requests a second and 1000 a day; the
//! key travels in the query string, which is never shown.

use std::time::Duration;

use copper_json::Value;

use super::{
    DistributorOffer, Found, Line, Pacer, PriceBreak, Provider, Secret, Transport, checked, env, fail, form_encode,
    integer, items, number, requested, string,
};
use crate::assembly::Result;

pub const SEARCH_URL: &str = "https://api.element14.com/catalog/products";
pub const DEFAULT_STORE: &str = "uk.farnell.com";

pub struct Farnell {
    api_key: Secret,
    store: String,
    pacer: Pacer,
}

impl Farnell {
    pub fn new(api_key: Secret, store: String, pacer: Pacer) -> Farnell {
        Farnell { api_key, store, pacer }
    }

    pub fn from_env() -> Option<Farnell> {
        let store = env("FARNELL_STORE").unwrap_or_else(|| DEFAULT_STORE.to_owned());
        Some(Farnell::new(Secret::new(env("FARNELL_API_KEY")?), store, Pacer::new(Duration::from_millis(600))))
    }

    /// The distributor this store sells as.
    pub fn seller(&self) -> &'static str {
        if self.store.contains("newark") {
            "Newark"
        } else if self.store.contains("element14") {
            "element14"
        } else {
            "Farnell"
        }
    }

    /// The store's price currency.
    fn currency(&self) -> &'static str {
        let country = self.store.split('.').next().unwrap_or("");
        match country {
            "uk" => "GBP",
            "www" if self.store.contains("newark") => "USD",
            "ch" => "CHF",
            "se" => "SEK",
            "no" => "NOK",
            "dk" => "DKK",
            "pl" => "PLN",
            "cz" => "CZK",
            "hu" => "HUF",
            "il" => "ILS",
            "sg" => "SGD",
            "au" => "AUD",
            "nz" => "NZD",
            "in" => "INR",
            "at" | "be" | "de" | "ee" | "es" | "fi" | "fr" | "ie" | "it" | "lt" | "lv" | "nl" | "pt" | "sk" | "si" => {
                "EUR"
            }
            _ => "unknown",
        }
    }

    fn url(&self, mpn: &str) -> String {
        format!(
            "{SEARCH_URL}?term={}&storeInfo.id={}&resultsSettings.offset=0&resultsSettings.numberOfResults=10\
             &resultsSettings.responseGroup=large&callInfo.responseDataFormat=JSON&callInfo.apiKey={}",
            form_encode(&format!("manuPartNum:{mpn}")),
            form_encode(&self.store),
            form_encode(self.api_key.expose())
        )
    }

    fn parse_product(&self, product: &Value, distributor: &str) -> Option<Found> {
        let sku = string(product.get("sku"))?;
        let stock = product.get("stock");
        let prices = items(product.get("prices"))
            .iter()
            .filter_map(|price| {
                Some(PriceBreak {
                    quantity: integer(price.get("from"))?,
                    price: number(price.get("cost"))?,
                    currency: self.currency().to_owned(),
                    converted_price: None,
                    converted_currency: None,
                })
            })
            .collect();
        let offer = DistributorOffer {
            distributor: distributor.to_owned(),
            seller: self.seller().into(),
            source: "farnell".into(),
            stock: integer(stock.and_then(|s| s.get("level"))),
            // Days to restock from the supplier; 0 means none quoted.
            factory_lead_days: integer(stock.and_then(|s| s.get("leadTime"))).filter(|days| *days > 0),
            moq: integer(product.get("translatedMinimumOrderQuality")),
            packaging: string(product.get("packageName")).or_else(|| string(product.get("unitOfMeasure"))),
            url: Some(format!("https://{}/{sku}", self.store)),
            prices,
            sku,
        };
        Some(Found {
            manufacturer: string(product.get("brandName"))
                .or_else(|| string(product.get("vendorName")))
                .unwrap_or_default(),
            mpn: string(product.get("translatedManufacturerPartNumber")).unwrap_or_default(),
            offers: vec![offer],
        })
    }
}

/// `{"Fault": {"Description"}}` errors.
fn fault(document: Option<&Value>) -> Option<String> {
    let fault = document?.get("Fault")?;
    string(fault.get("Description")).or_else(|| string(fault.get("faultstring"))).or_else(|| string(fault.get("Code")))
}

impl Provider for Farnell {
    fn name(&self) -> &'static str {
        "farnell"
    }

    fn serves(&self, distributors: &[String]) -> bool {
        requested(distributors, self.seller()).is_some()
    }

    fn fetch(&self, transport: &dyn Transport, lines: &[Line], distributors: &[String]) -> Result<Vec<Vec<Found>>> {
        let Some(distributor) = requested(distributors, self.seller()) else {
            return Ok(vec![Vec::new(); lines.len()]);
        };
        let headers = [("Accept", "application/json".to_owned())];
        let mut results = Vec::new();
        for line in lines {
            self.pacer.wait();
            let (status, response) = transport.send("GET", &self.url(&line.mpn), &headers, None)?;
            let document = copper_json::parse(&response).ok();
            let message = fault(document.as_ref());
            checked("farnell", status, String::new(), message.clone())?;
            if let Some(message) = message {
                return fail(format!("request failed: {message}"));
            }
            let products = items(
                document
                    .as_ref()
                    .and_then(|d| d.get("manufacturerPartNumberSearchReturn"))
                    .and_then(|r| r.get("products")),
            );
            results.push(products.iter().filter_map(|product| self.parse_product(product, distributor)).collect());
        }
        Ok(results)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::offers::testing::{Recorded, distributors, lock};
    use crate::offers::{lines, select};

    const YAGEO: &str = r#"{"manufacturerPartNumberSearchReturn":{"numberOfResults":1,"products":[
      {"sku":"3019495","displayName":"SMD Multilayer Ceramic Capacitor, 0.1 uF","productStatus":"STOCKED",
       "brandName":"YAGEO","translatedManufacturerPartNumber":"CC0603KRX7R9BB104","translatedMinimumOrderQuality":10,
       "unitOfMeasure":"EACH","packageName":"Cut Tape","stock":{"level":40211,"leadTime":0,"status":1},
       "prices":[{"to":99,"from":10,"cost":0.0123},{"to":499,"from":100,"cost":0.0071}]}]}}"#;

    fn farnell(store: &str) -> Farnell {
        Farnell::new(Secret::new("e14-key"), store.into(), Pacer::new(Duration::ZERO))
    }

    #[test]
    fn manufacturer_part_search_in_one_store() {
        let (lines, _) = lines(&lock());
        let empty = r#"{"manufacturerPartNumberSearchReturn":{"numberOfResults":0}}"#;
        let transport = Recorded::new(vec![(200, empty), (200, YAGEO)]);
        let found = farnell("no.farnell.com").fetch(&transport, &lines, &distributors()).expect("offers");
        let requests = transport.requests.borrow();
        assert_eq!(requests[1].0, "GET");
        assert!(
            requests[1].1.starts_with(&format!(
                "{SEARCH_URL}?term=manuPartNum%3ACC0603KRX7R9BB104&storeInfo.id=no.farnell.com&"
            ))
        );
        assert!(requests[1].1.ends_with("&callInfo.apiKey=e14-key"));
        let yageo = select(&lines[1], found[1].clone(), &distributors());
        let offer = &yageo.offers[0];
        assert_eq!(
            (offer.sku.as_str(), offer.stock, offer.moq, offer.factory_lead_days),
            ("3019495", Some(40211), Some(10), None)
        );
        assert_eq!(offer.url.as_deref(), Some("https://no.farnell.com/3019495"));
        assert_eq!(
            (offer.prices[0].quantity, offer.prices[0].price, offer.prices[0].currency.as_str()),
            (10, 0.0123, "NOK")
        );
        assert!(found[0].is_empty());
    }

    #[test]
    fn stores_name_their_distributor_and_faults_are_reported() {
        assert_eq!(farnell("www.newark.com").seller(), "Newark");
        assert!(!farnell("www.newark.com").serves(&distributors()), "Newark was not requested");
        assert_eq!(farnell("de.farnell.com").currency(), "EUR");
        let (lines, _) = lines(&lock());
        let inactive = r#"{"Fault":{"Code":"403","Description":"Developer Inactive"}}"#;
        let failure =
            farnell(DEFAULT_STORE).fetch(&Recorded::new(vec![(403, inactive)]), &lines, &distributors()).unwrap_err().0;
        assert_eq!(failure, "farnell request failed (HTTP 403): Developer Inactive");
        let failure =
            farnell(DEFAULT_STORE).fetch(&Recorded::new(vec![(429, "")]), &lines, &distributors()).unwrap_err().0;
        assert!(failure.starts_with("farnell rate limit reached") && !failure.contains("e14-key"));
    }
}
