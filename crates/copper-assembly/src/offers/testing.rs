//! Recorded HTTP exchanges and a small lock for the offer tests; no test
//! touches the network or a real credential.

use std::cell::RefCell;

use super::Transport;
use crate::assembly::{Lock, Offer, Result, Selection};

/// One request as sent: method, URL, headers, body.
pub type Sent = (String, String, Vec<(String, String)>, Option<String>);

/// Replays canned responses in order and records every request.
pub struct Recorded {
    responses: RefCell<Vec<(u16, String)>>,
    pub requests: RefCell<Vec<Sent>>,
}

impl Recorded {
    pub fn new(responses: Vec<(u16, &str)>) -> Recorded {
        Recorded {
            responses: RefCell::new(responses.into_iter().rev().map(|(s, b)| (s, b.to_owned())).collect()),
            requests: RefCell::new(Vec::new()),
        }
    }
}

impl Transport for Recorded {
    fn send(&self, method: &str, url: &str, headers: &[(&str, String)], body: Option<&str>) -> Result<(u16, String)> {
        self.requests.borrow_mut().push((
            method.to_owned(),
            url.to_owned(),
            headers.iter().map(|(n, v)| ((*n).to_owned(), v.clone())).collect(),
            body.map(str::to_owned),
        ));
        Ok(self.responses.borrow_mut().pop().expect("an expected request"))
    }
}

pub fn selection(reference: &str, manufacturer: &str, mpn: &str, supplier: &str, code: &str) -> Selection {
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

/// C1/C2 share a YAGEO part, U2 is TI, R9 is not fitted, U1 has no identity.
pub fn lock() -> Lock {
    let mut c2 = selection("C2", "YAGEO", "CC0603KRX7R9BB104", "Mouser", "603-CC603KRX7R9BB104");
    c2.alternates.push(Offer { supplier: "DigiKey".into(), supplier_part: "311-1344-1-ND".into(), source_url: None });
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

pub fn distributors() -> Vec<String> {
    vec!["DigiKey".into(), "Mouser".into(), "Farnell".into()]
}
