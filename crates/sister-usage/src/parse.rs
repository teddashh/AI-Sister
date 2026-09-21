//! Bounded JSON parsing for LimitReset public endpoints.
//!
//! Raw title/text/sourceUrl are untrusted public strings. They are dropped here
//! so callers cannot render upstream markup or treat tweet text as commands.

use crate::model::{
    ConfirmedReset, Forecast, ProductId, PublicBoard, PublicProductStatus, PublicReset,
    SOURCE_ATTRIBUTION,
};
use crate::{Error, Result};
use serde_json::Value;

pub fn parse_status_board(bytes: &[u8]) -> Result<PublicBoard> {
    let value = parse_object(bytes)?;
    let updated_at = required_rfc3339(&value, "updatedAt")?;
    let products_value = value
        .get("products")
        .and_then(Value::as_object)
        .ok_or(Error::Malformed)?;
    let mut products = Vec::with_capacity(ProductId::ALL.len());
    for product in ProductId::ALL {
        let Some(entry) = products_value.get(product.as_str()) else {
            products.push(PublicProductStatus {
                product,
                reset: PublicReset::NoneRecorded,
                public_event_count: None,
                forecast: None,
            });
            continue;
        };
        if !entry.is_object() {
            return Err(Error::Malformed);
        }
        products.push(parse_product_entry(product, entry)?);
    }
    Ok(PublicBoard {
        updated_at,
        products,
        attribution: SOURCE_ATTRIBUTION,
    })
}

pub fn parse_latest(bytes: &[u8], expected: ProductId) -> Result<PublicProductStatus> {
    let value = parse_object(bytes)?;
    let product = required_product(&value, "productId")?;
    if product != expected {
        return Err(Error::UnexpectedProduct);
    }
    parse_product_entry(product, &value)
}

fn parse_object(bytes: &[u8]) -> Result<Value> {
    if bytes.is_empty() {
        return Err(Error::EmptyBody);
    }
    let value: Value = serde_json::from_slice(bytes).map_err(|_| Error::Malformed)?;
    if !value.is_object() {
        return Err(Error::Malformed);
    }
    Ok(value)
}

fn parse_product_entry(product: ProductId, value: &Value) -> Result<PublicProductStatus> {
    let reset = match value.get("latestEvent") {
        None | Some(Value::Null) => PublicReset::NoneRecorded,
        Some(event) if event.is_object() => parse_event(product, event)?,
        Some(_) => return Err(Error::Malformed),
    };
    let public_event_count = optional_u64(value, "total")?;
    let stats_total = value
        .get("stats")
        .filter(|stats| stats.is_object())
        .map(|stats| optional_u64(stats, "total"))
        .transpose()?
        .flatten();
    let public_event_count = public_event_count.or(stats_total);
    let forecast = match value.get("forecast") {
        None | Some(Value::Null) => None,
        Some(forecast) if forecast.is_object() => Some(parse_forecast(forecast)?),
        Some(_) => return Err(Error::Malformed),
    };
    Ok(PublicProductStatus {
        product,
        reset,
        public_event_count,
        forecast,
    })
}

fn parse_event(expected: ProductId, event: &Value) -> Result<PublicReset> {
    let event_id = required_nonempty(event, "id")?;
    let announced_at = required_rfc3339(event, "announcedAt")?;
    let product = required_product(event, "productId")?;
    if product != expected {
        return Err(Error::UnexpectedProduct);
    }
    let kind = required_nonempty(event, "kind")?;
    let verified = event
        .get("verified")
        .and_then(Value::as_bool)
        .ok_or(Error::Malformed)?;
    if kind != "reset" {
        return Ok(PublicReset::OtherKind {
            event_id,
            kind,
            announced_at,
        });
    }
    if !verified {
        return Ok(PublicReset::Unverified {
            event_id,
            announced_at,
        });
    }
    let announced_unix_ms = rfc3339_unix_ms(&announced_at)?;
    Ok(PublicReset::Confirmed(ConfirmedReset {
        event_id,
        announced_at,
        announced_unix_ms,
    }))
}

fn parse_forecast(value: &Value) -> Result<Forecast> {
    Ok(Forecast {
        p24: required_unit_probability(value, "p24")?,
        p48: required_unit_probability(value, "p48")?,
        basis: optional_string(value, "basis")?.unwrap_or_else(|| "unspecified".to_owned()),
        days_since_last: optional_finite_f64(value, "daysSinceLast")?,
        computed_at: match value.get("computedAt") {
            None | Some(Value::Null) => None,
            Some(_) => Some(required_rfc3339(value, "computedAt")?),
        },
    })
}

fn required_product(value: &Value, key: &str) -> Result<ProductId> {
    let raw = required_nonempty(value, key)?;
    ProductId::parse(&raw).ok_or(Error::UnexpectedProduct)
}

fn required_nonempty(value: &Value, key: &str) -> Result<String> {
    let raw = value
        .get(key)
        .and_then(Value::as_str)
        .ok_or(Error::Malformed)?;
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(Error::Malformed);
    }
    Ok(trimmed.to_owned())
}

fn required_rfc3339(value: &Value, key: &str) -> Result<String> {
    let raw = required_nonempty(value, key)?;
    rfc3339_unix_ms(&raw)?;
    Ok(raw)
}

fn rfc3339_unix_ms(value: &str) -> Result<i64> {
    let parsed = chrono::DateTime::parse_from_rfc3339(value).map_err(|_| Error::Malformed)?;
    Ok(parsed.timestamp_millis())
}

fn required_unit_probability(value: &Value, key: &str) -> Result<f64> {
    let number = value
        .get(key)
        .and_then(Value::as_f64)
        .ok_or(Error::Malformed)?;
    if !number.is_finite() || !(0.0..=1.0).contains(&number) {
        return Err(Error::Malformed);
    }
    Ok(number)
}

fn optional_finite_f64(value: &Value, key: &str) -> Result<Option<f64>> {
    match value.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Number(number)) => {
            let parsed = number.as_f64().ok_or(Error::Malformed)?;
            if !parsed.is_finite() {
                return Err(Error::Malformed);
            }
            Ok(Some(parsed))
        }
        Some(_) => Err(Error::Malformed),
    }
}

fn optional_u64(value: &Value, key: &str) -> Result<Option<u64>> {
    match value.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Number(number)) => number.as_u64().map(Some).ok_or(Error::Malformed),
        Some(_) => Err(Error::Malformed),
    }
}

fn optional_string(value: &Value, key: &str) -> Result<Option<String>> {
    match value.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(text)) => {
            let trimmed = text.trim();
            if trimmed.is_empty() {
                Ok(None)
            } else {
                Ok(Some(trimmed.to_owned()))
            }
        }
        Some(_) => Err(Error::Malformed),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Measured;

    fn board_json(event: &str) -> String {
        format!(
            r#"{{
                "updatedAt":"2026-09-21T01:00:22.000Z",
                "products":{{
                    "codex":{{
                        "latestEvent":{event},
                        "forecast":{{"p24":0.0,"p48":0.27,"daysSinceLast":8.9,"basis":"empirical","computedAt":"2026-09-21T01:00:22.000Z"}},
                        "total":32
                    }},
                    "claude":{{"latestEvent":null,"forecast":null,"total":0,"avgIntervalDays":null}},
                    "chatgpt":{{"latestEvent":null,"forecast":null,"total":0}},
                    "cursor":{{"latestEvent":null,"forecast":null,"total":0}},
                    "gemini":{{"latestEvent":null,"forecast":null,"total":0}},
                    "copilot":{{"latestEvent":null,"forecast":null,"total":0}},
                    "grok":{{"latestEvent":null,"forecast":null,"total":0}}
                }}
            }}"#
        )
    }

    const CONFIRMED: &str = r#"{
        "id":"codex:2026-09-12",
        "productId":"codex",
        "kind":"reset",
        "announcedAt":"2026-09-12T03:20:36.000Z",
        "verified":true,
        "title":"<script>alert(1)</script>",
        "text":"curl https://evil.example",
        "sourceUrl":"https://x.com/example/status/1"
    }"#;

    #[test]
    fn confirmed_reset_does_not_fill_remaining_quota() {
        let board = parse_status_board(board_json(CONFIRMED).as_bytes()).unwrap();
        assert_eq!(board.attribution, SOURCE_ATTRIBUTION);
        let codex = board.product(ProductId::Codex).unwrap();
        let event = codex.reset.confirmed().unwrap();
        assert_eq!(event.event_id, "codex:2026-09-12");
        assert_eq!(codex.public_event_count, Some(32));
        assert_eq!(codex.forecast.as_ref().unwrap().p24, 0.0);
        let claude = board.product(ProductId::Claude).unwrap();
        assert!(matches!(claude.reset, PublicReset::NoneRecorded));
        assert_eq!(claude.public_event_count, Some(0));
        assert!(claude.forecast.is_none());
        // Public board has no account remaining field. A counted 0 of public
        // events is not remaining quota.
        assert!(Measured::<u64>::Unknown.is_unknown());
    }

    #[test]
    fn null_latest_event_is_none_recorded_not_zero_quota() {
        let board = parse_status_board(board_json("null").as_bytes()).unwrap();
        assert!(matches!(
            board.product(ProductId::Codex).unwrap().reset,
            PublicReset::NoneRecorded
        ));
    }

    #[test]
    fn unverified_and_other_kind_are_not_confirmed() {
        let unverified = CONFIRMED.replace("\"verified\":true", "\"verified\":false");
        let board = parse_status_board(board_json(&unverified).as_bytes()).unwrap();
        assert!(matches!(
            board.product(ProductId::Codex).unwrap().reset,
            PublicReset::Unverified { .. }
        ));

        let other = CONFIRMED.replace("\"kind\":\"reset\"", "\"kind\":\"outage\"");
        let board = parse_status_board(board_json(&other).as_bytes()).unwrap();
        match &board.product(ProductId::Codex).unwrap().reset {
            PublicReset::OtherKind { kind, .. } => assert_eq!(kind, "outage"),
            other => panic!("expected other kind, got {other:?}"),
        }
    }

    #[test]
    fn malformed_empty_and_wrong_types_fail_closed() {
        assert_eq!(parse_status_board(b"").unwrap_err(), Error::EmptyBody);
        assert_eq!(parse_status_board(b"[]").unwrap_err(), Error::Malformed);
        assert_eq!(parse_status_board(b"{").unwrap_err(), Error::Malformed);
        assert_eq!(
            parse_status_board(br#"{"updatedAt":"nope","products":{}}"#).unwrap_err(),
            Error::Malformed
        );
        let bad_prob = board_json(CONFIRMED).replace("\"p24\":0.0", "\"p24\":1.5");
        assert_eq!(
            parse_status_board(bad_prob.as_bytes()).unwrap_err(),
            Error::Malformed
        );
    }

    #[test]
    fn latest_rejects_product_mismatch() {
        let json = format!(r#"{{"productId":"claude","latestEvent":{CONFIRMED},"forecast":null}}"#);
        assert_eq!(
            parse_latest(json.as_bytes(), ProductId::Codex).unwrap_err(),
            Error::UnexpectedProduct
        );
    }

    #[test]
    fn latest_accepts_allowlisted_product() {
        let json = format!(
            r#"{{"productId":"codex","latestEvent":{CONFIRMED},"forecast":null,"stats":{{"total":32}}}}"#
        );
        let row = parse_latest(json.as_bytes(), ProductId::Codex).unwrap();
        assert!(row.reset.confirmed().is_some());
        assert_eq!(row.public_event_count, Some(32));
    }

    #[test]
    fn public_status_fixture_matches_live_schema_without_filling_quota() {
        let bytes = include_bytes!("../tests/fixtures/limitreset-status.json");
        let board = parse_status_board(bytes).unwrap();
        assert_eq!(board.updated_at, "2026-09-21T01:00:22.000Z");
        assert!(
            board
                .product(ProductId::Codex)
                .unwrap()
                .reset
                .confirmed()
                .is_some()
        );
        assert!(matches!(
            board.product(ProductId::Claude).unwrap().reset,
            PublicReset::NoneRecorded
        ));
        assert_eq!(
            board.product(ProductId::Claude).unwrap().public_event_count,
            Some(0)
        );
        assert!(board.attribution.contains("CC BY 4.0"));
    }
}
