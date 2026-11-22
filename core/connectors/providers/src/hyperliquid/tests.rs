//! Hyperliquid, offline. Synthetic fixtures written from the documented schema, with the
//! field names and value shapes confirmed against the live venue.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use otw_connector_model::{AssetClass, Command, CorrId, DataType, ErrorKind, Event, HistoryItems, InstrumentId, TimeRange, Timeframe};
use otw_connector_sdk::{InstrumentCache, NativeRequest, RawFrame};
use otw_connector_testkit::{assert_history_sound, assert_undeclared_is_unsupported, chunks, decode_once, encode_once, http_frame, json, ts};

use super::*;

fn btc() -> InstrumentId {
    InstrumentId::new(VENUE, "BTC")
}

fn hist(tf: Timeframe, from: i64, to: i64) -> Command {
    Command::RequestHistory { req_id: CorrId(1), instrument: btc(), data_type: DataType::Candles { tf }, range: TimeRange::new(ts(from), ts(to)), cursor: None }
}

/// One `candleSnapshot` row, as the venue writes it: millisecond times, prices as strings.
fn bar(t: i64, o: &str, c: &str) -> String {
    format!(
        r#"{{"t":{t},"T":{},"s":"BTC","i":"1m","o":"{o}","c":"{c}","h":"101.0","l":"99.0","v":"12.5","n":7}}"#,
        t + 59_999
    )
}

#[test]
fn capabilities_parse_and_undeclared_is_refused() {
    let m = Hyperliquid::new().unwrap();
    assert_undeclared_is_unsupported(&m, &btc());
    // A perp is a bare coin; a spot pair is `PURR/USDC` or the venue's own `@107`.
    for good in ["BTC", "PURR/USDC", "@107", "HYPE/USDC"] {
        assert_eq!(m.instrument_id(good).unwrap().native, good);
    }
    assert!(m.instrument_id("BTC USDT").is_err() && m.instrument_id("  ").is_err());
}

#[test]
fn the_call_is_a_post_with_a_json_body_and_its_header() {
    let m = Hyperliquid::new().unwrap();
    let mut s = Session::default();
    let reqs = encode_once(&m, &mut s, &InstrumentCache::new(), &hist(Timeframe::M1, 1_700_000_000, 1_700_003_600), ts(0)).unwrap();
    let [NativeRequest::Http(h)] = reqs.as_slice() else { panic!() };
    assert_eq!(h.method, Method::Post, "the info endpoint has no query-string form");
    assert_eq!(h.path, "/info");
    assert!(
        h.headers.iter().any(|(k, v)| k == "Content-Type" && v == "application/json"),
        "the driver adds no Content-Type, so the mapper must"
    );
    let body: serde_json::Value = serde_json::from_slice(h.body.as_ref().unwrap()).unwrap();
    assert_eq!(body["type"], "candleSnapshot");
    assert_eq!(body["req"]["coin"], "BTC");
    assert_eq!(body["req"]["interval"], "1m");
    assert_eq!(body["req"]["startTime"], 1_700_000_000_000i64);
    // Our window is half-open; the venue's end is inclusive.
    assert_eq!(body["req"]["endTime"], 1_700_003_599_999i64);
}

#[test]
fn a_series_is_read_sorted_and_filtered_to_the_window() {
    // On a minute boundary, which is what the engine's alignment check requires.
    const T: i64 = 1_699_999_980;
    let m = Hyperliquid::new().unwrap();
    let mut s = Session::default();
    let cache = InstrumentCache::new();
    encode_once(&m, &mut s, &cache, &hist(Timeframe::M1, T, T + 120), ts(0)).unwrap();
    let rows = [bar((T + 60) * 1_000, "100.5", "101.0"), bar(T * 1_000, "100.0", "100.9"), bar((T - 60) * 1_000, "1", "1")];
    let body = json(&format!("[{}]", rows.join(",")));
    let evs = decode_once(&m, &mut s, &cache, &http_frame(1, "candles", 200, &body), ts(0)).unwrap();
    let c = chunks(&evs);
    let (HistoryItems::Candles(cs), next, done) = &c[0] else { panic!() };
    assert_eq!(cs.iter().map(|x| x.open_ts.secs()).collect::<Vec<_>>(), [T, T + 60], "sorted, and the bar before the window dropped");
    assert_eq!(cs[1].close.to_string(), "101.0");
    assert_eq!(cs[1].volume.to_string(), "12.5");
    assert_eq!(cs[1].trades, Some(7), "the venue's trade count is carried");
    assert!(!cs[1].is_final, "the forming bar is not marked, so every bar is handed over as forming");
    assert!(next.is_none() && *done);
    assert_history_sound(&[c[0].0.clone()]);
}

#[test]
fn a_full_page_asks_for_the_next_one_after_its_last_bar() {
    let t0 = 1_700_000_000_000i64;
    let rows: Vec<String> = (0..MAX_BARS as i64).map(|i| bar(t0 + i * 60_000, "1", "1")).collect();
    let m = Hyperliquid::new().unwrap();
    let mut s = Session::default();
    let cache = InstrumentCache::new();
    encode_once(&m, &mut s, &cache, &hist(Timeframe::M1, 1_700_000_000, 1_700_100_000), ts(0)).unwrap();
    let body = json(&format!("[{}]", rows.join(",")));
    let evs = decode_once(&m, &mut s, &cache, &http_frame(1, "candles", 200, &body), ts(0)).unwrap();
    let c = chunks(&evs);
    let (_, next, done) = &c[0];
    let want = (t0 + MAX_BARS as i64 * 60_000).to_string();
    assert_eq!(next.as_deref(), Some(want.as_str()), "the next page starts one bar after the last one returned");
    assert!(!*done);
}

#[test]
fn a_short_page_is_the_end_of_the_range() {
    let m = Hyperliquid::new().unwrap();
    let mut s = Session::default();
    let cache = InstrumentCache::new();
    encode_once(&m, &mut s, &cache, &hist(Timeframe::M1, 1_700_000_000, 1_700_100_000), ts(0)).unwrap();
    let body = json(&format!("[{}]", bar(1_700_000_000_000, "1", "1")));
    let c = chunks(&decode_once(&m, &mut s, &cache, &http_frame(1, "candles", 200, &body), ts(0)).unwrap());
    let (_, next, done) = &c[0];
    assert!(next.is_none() && *done);
}

#[test]
fn an_unknown_coin_answers_null_and_that_is_not_found() {
    let m = Hyperliquid::new().unwrap();
    let mut s = Session::default();
    let cache = InstrumentCache::new();
    encode_once(&m, &mut s, &cache, &hist(Timeframe::M1, 0, 60), ts(0)).unwrap();
    let e = decode_once(&m, &mut s, &cache, &http_frame(1, "candles", 200, "null"), ts(0)).unwrap_err();
    assert_eq!(e.kind, ErrorKind::NotFound);
    assert!(e.message.contains("bad coin"));
}

#[test]
fn the_catalog_is_two_answers_and_ends_once() {
    let m = Hyperliquid::new().unwrap();
    let mut s = Session::default();
    let cache = InstrumentCache::new();
    let reqs = encode_once(&m, &mut s, &cache, &Command::LoadInstruments { req_id: CorrId(2) }, ts(0)).unwrap();
    assert_eq!(reqs.len(), 2, "perpetuals and spot pairs are two calls");

    let perps = json(r#"{"universe":[{"name":"BTC","szDecimals":5,"maxLeverage":40},{"name":"ETH","szDecimals":4,"maxLeverage":25}]}"#);
    let evs = decode_once(&m, &mut s, &cache, &http_frame(2, "meta", 200, &perps), ts(0)).unwrap();
    assert_eq!(evs.len(), 2, "two perps, and no end while the spot answer is outstanding");
    let specs: Vec<_> = evs.iter().filter_map(|e| match &e.event { Event::Instrument { spec } => Some(spec.as_ref()), _ => None }).collect();
    assert_eq!((specs[0].id.native.as_str(), specs[0].symbol.0.as_str(), specs[0].asset_class), ("BTC", "BTC/USD", AssetClass::Perp));
    assert_eq!(specs[0].lot_size.map(|q| q.to_string()), Some("0.00001".into()), "szDecimals 5 is a 1e-5 size step");

    let spot = json(
        r#"{"universe":[{"tokens":[1,0],"name":"PURR/USDC","index":0},{"tokens":[150,0],"name":"@107","index":107}],
            "tokens":[{"name":"USDC","szDecimals":8,"index":0},{"name":"PURR","szDecimals":1,"index":1},{"name":"HYPE","szDecimals":2,"index":150}]}"#,
    );
    let evs = decode_once(&m, &mut s, &cache, &http_frame(2, "spotMeta", 200, &spot), ts(0)).unwrap();
    let specs: Vec<_> = evs.iter().filter_map(|e| match &e.event { Event::Instrument { spec } => Some(spec.as_ref()), _ => None }).collect();
    assert_eq!(specs.len(), 2);
    assert_eq!((specs[0].id.native.as_str(), specs[0].symbol.0.as_str()), ("PURR/USDC", "PURR/USDC"));
    // The venue's own id is what its calls take; the readable form is derived from the tokens.
    assert_eq!((specs[1].id.native.as_str(), specs[1].symbol.0.as_str()), ("@107", "HYPE/USDC"));
    assert!(matches!(evs.last().map(|e| &e.event), Some(Event::InstrumentsEnd)), "the list ends after the last answer");
}

#[test]
fn the_board_is_every_mid_keyed_by_the_venue_id() {
    let m = Hyperliquid::new().unwrap();
    let mut s = Session::default();
    let cache = InstrumentCache::new();
    encode_once(&m, &mut s, &cache, &Command::RequestBoard { req_id: CorrId(3) }, ts(0)).unwrap();
    let body = json(r#"{"BTC":"56613.0","@107":"56.278","PURR/USDC":"0.066517","WEIRD":null}"#);
    let evs = decode_once(&m, &mut s, &cache, &http_frame(3, "board", 200, &body), ts(0)).unwrap();
    let Some(Event::Board(prices)) = evs.first().map(|e| &e.event) else { panic!() };
    let got: Vec<(&str, String)> = prices.iter().map(|p| (p.instrument.native.as_str(), p.price.to_string())).collect();
    assert_eq!(got, [("@107", "56.278".into()), ("BTC", "56613.0".into()), ("PURR/USDC", "0.066517".into())], "a null mid is left out, never a zero");
}

#[test]
fn a_book_snapshot_and_a_quote_both_come_from_l2book() {
    let m = Hyperliquid::new().unwrap();
    let mut s = Session::default();
    let cache = InstrumentCache::new();
    let body = json(r#"{"coin":"BTC","time":1795243169102,"levels":[[{"px":"56640.0","sz":"11.49203","n":24},{"px":"56637.0","sz":"0.09","n":1}],[{"px":"56641.0","sz":"2.5","n":3}]]}"#);

    let cmd = Command::RequestSnapshot { req_id: CorrId(4), instrument: btc(), data_type: DataType::BookSnapshot { depth: 20 } };
    let reqs = encode_once(&m, &mut s, &cache, &cmd, ts(0)).unwrap();
    let [NativeRequest::Http(h)] = reqs.as_slice() else { panic!() };
    assert_eq!(h.endpoint_id, "book");
    let evs = decode_once(&m, &mut s, &cache, &http_frame(4, "book", 200, &body), ts(0)).unwrap();
    let Some(Event::BookSnapshot { bids, asks, .. }) = evs.first().map(|e| &e.event) else { panic!() };
    assert_eq!((bids.len(), asks.len()), (2, 1));
    assert_eq!((bids[0].price.to_string(), bids[0].qty.to_string()), ("56640.0".into(), "11.49203".into()));

    let cmd = Command::RequestSnapshot { req_id: CorrId(5), instrument: btc(), data_type: DataType::Quote };
    let reqs = encode_once(&m, &mut s, &cache, &cmd, ts(0)).unwrap();
    let [NativeRequest::Http(h)] = reqs.as_slice() else { panic!() };
    assert_eq!(h.endpoint_id, "quote");
    let evs = decode_once(&m, &mut s, &cache, &http_frame(5, "quote", 200, &body), ts(0)).unwrap();
    let Some(Event::Quote(q)) = evs.first().map(|e| &e.event) else { panic!() };
    assert_eq!((q.bid.to_string(), q.ask.to_string()), ("56640.0".into(), "56641.0".into()));
}

#[test]
fn a_timeframe_the_venue_does_not_serve_is_refused_before_the_call() {
    let m = Hyperliquid::new().unwrap();
    let mut s = Session::default();
    let e = encode_once(&m, &mut s, &InstrumentCache::new(), &hist(Timeframe { n: 7, unit: TfUnit::Minute }, 0, 60), ts(0)).unwrap_err();
    assert_eq!(e.kind, ErrorKind::Unsupported);
}

proptest::proptest! {
    #[test]
    fn decode_survives_arbitrary_bytes(bytes in proptest::collection::vec(proptest::num::u8::ANY, 0..512)) {
        let m = Hyperliquid::new().unwrap();
        let mut s = Session::default();
        let cache = InstrumentCache::new();
        encode_once(&m, &mut s, &cache, &hist(Timeframe::M1, 0, 60), ts(0)).unwrap();
        let frame = RawFrame::Http { corr_id: CorrId(1), endpoint_id: "candles", status: 200, headers: vec![], body: bytes.into() };
        let _ = decode_once(&m, &mut s, &cache, &frame, ts(0));
    }
}
