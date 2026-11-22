//! Hyperliquid candles through the error matrix (testkit `matrix`): a mock venue answers,
//! the real driver reads. Bodies are synthetic, from the documented `candleSnapshot` schema.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use otw_connector_model::{DataType, ErrorKind, InstrumentId, TimeRange, Timeframe};
use otw_connector_testkit::matrix::Probe;
use otw_connector_testkit::wiremock::ResponseTemplate;
use otw_connector_testkit::{assert_history_sound, ts};

use super::*;

const T0: i64 = 1_772_409_600; // 2026-03-02 00:00 UTC

fn probe() -> Probe<Hyperliquid> {
    Probe::new(
        || Hyperliquid::new().unwrap(),
        InstrumentId::new(VENUE, "BTC"),
        DataType::Candles { tf: Timeframe::M1 },
        TimeRange::new(ts(T0), ts(T0 + 600)),
        ts(T0 + 600),
    )
}

/// One `candleSnapshot` row.
fn row(secs: i64, close: &str) -> String {
    let t = secs * 1_000;
    format!(r#"{{"t":{t},"T":{},"s":"BTC","i":"1m","o":"100.0","c":"{close}","h":"101.0","l":"99.0","v":"10.0","n":4}}"#, t + 59_999)
}

fn series(rows: &[String]) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_string(format!("[{}]", rows.join(","))).insert_header("content-type", "application/json")
}

otw_connector_testkit::history_error_matrix! { probe();
    bad_request_is_invalid,
    unauthorized_is_auth,
    forbidden_is_permission,
    rate_limit_carries_retry_after,
    server_error_is_transient,
    gateway_json_outage_is_transient,
    timeout_is_transient,
    malformed_json_is_decode,
    empty_body_is_decode,
    html_page_is_decode,
}

#[tokio::test]
async fn nominal_bars_come_back_sorted_and_sound() {
    let o = probe().serve(series(&[row(T0 + 120, "100.3"), row(T0, "100.1"), row(T0 + 60, "100.2")])).await;
    let cs = o.candles();
    assert_eq!(cs.iter().map(|c| c.open_ts.secs()).collect::<Vec<_>>(), [T0, T0 + 60, T0 + 120]);
    assert_eq!(cs[2].close.to_string(), "100.3");
    assert_history_sound(o.result.as_ref().unwrap());
}

/// The venue answers `null` for a coin it does not list.
#[tokio::test]
async fn an_unknown_coin_is_not_found_never_a_best_effort_match() {
    let o = probe().serve(ResponseTemplate::new(200).set_body_string("null")).await;
    assert_eq!(o.kind(), ErrorKind::NotFound);
    assert!(o.err().message.contains("bad coin"));
}

#[tokio::test]
async fn a_row_missing_a_price_is_a_hole_and_the_others_are_kept() {
    let missing = row(T0 + 120, "100.3").replace(r#","c":"100.3""#, "");
    let o = probe().serve(series(&[row(T0, "100.1"), missing, row(T0 + 60, "100.2")])).await;
    let minutes: Vec<i64> = o.candles().iter().map(|c| (c.open_ts.secs() - T0) / 60).collect();
    assert_eq!(minutes, [0, 1], "the unreadable row is dropped, the rest kept");
}

#[tokio::test]
async fn an_empty_list_is_one_empty_finished_chunk() {
    let o = probe().serve(series(&[])).await;
    assert_eq!(o.result.as_ref().unwrap().len(), 1);
    assert!(o.candles().is_empty());
}
