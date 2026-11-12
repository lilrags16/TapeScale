//! Truthful software capability and physical-qualification status.

use serde_json::{json, Value};

/// Returns product capability metadata without inventing hardware evidence.
#[must_use]
pub fn report() -> Value {
    json!({
        "release": crate::VERSION,
        "gate": "G0 not passed",
        "engine": {
            "candidate": "Unselected",
            "selection": "Physical evaluation required",
            "adapter": "Simulator present; no physical operations",
            "format": "Unselected"
        },
        "claims": {
            "software_tests": "Automated domain and service tests",
            "physical_lto": "No drive or media generation qualified",
            "truenas": "25.10.8 target; this installation has not been qualified"
        },
        "generations": [
            {"generation":"LTO-1","reads":"LTO-1","writes":"LTO-1","model":"Capability model","physical":"Not run"},
            {"generation":"LTO-2","reads":"LTO-1, LTO-2","writes":"LTO-1, LTO-2","model":"Capability model","physical":"Not run"},
            {"generation":"LTO-3","reads":"LTO-1–3","writes":"LTO-2–3","model":"Capability model","physical":"Not run"},
            {"generation":"LTO-4","reads":"LTO-2–4","writes":"LTO-3–4","model":"Capability model","physical":"Not run"},
            {"generation":"LTO-5","reads":"LTO-3–5","writes":"LTO-4–5","model":"Capability model","physical":"Not run"},
            {"generation":"LTO-6","reads":"LTO-4–6","writes":"LTO-5–6","model":"Capability model","physical":"Not run"},
            {"generation":"LTO-7","reads":"LTO-5–7","writes":"LTO-6–7","model":"Capability model","physical":"Not run"},
            {"generation":"LTO-8","reads":"LTO-7, LTO-7 Type M, LTO-8","writes":"LTO-7, LTO-7 Type M, LTO-8","model":"Capability model","physical":"Not run"},
            {"generation":"LTO-9","reads":"LTO-8, LTO-9","writes":"LTO-8, LTO-9","model":"Capability model","physical":"Not run"},
            {"generation":"LTO-10","reads":"LTO-10","writes":"LTO-10","model":"Capability model","physical":"Not run"}
        ]
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn report_never_claims_physical_qualification() {
        let value = report();
        assert_eq!(value["gate"], "G0 not passed");
        assert_eq!(value["generations"].as_array().map(Vec::len), Some(10));
        assert!(value["generations"]
            .as_array()
            .expect("generation array")
            .iter()
            .all(|generation| generation["physical"] == "Not run"));
    }
}
