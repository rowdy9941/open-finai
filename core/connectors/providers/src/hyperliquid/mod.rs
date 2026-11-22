//! Hyperliquid: perpetuals and spot on one `/info` endpoint. Every market-data call is a
//! `POST` with a JSON body (the venue has no query-string form), so the mapper writes the
//! body and the `Content-Type` header itself — the driver adds neither.
//!
//! Endpoint-by-endpoint mapping in PROVIDER.md.

use std::collections::{BTreeMap, HashMap};

use otw_connector_model::{
    AssetClass, BookLevel, Candle, Capabilities, CapsError, Command, ConnectorError, ContractType, CorrId, DataType, Decimal,
    Envelope, Event, HistoryItems, InstrumentId, InstrumentSpec, LastPrice, Price, Qty, Quote, TfUnit, TimeRange, Timeframe,
    Timestamp,
};
use otw_connector_sdk::parse::OptDec;
use otw_connector_sdk::{http, HttpRequest, MapCtx, Mapper, Method, NativeRequest, RawFrame};
use serde::Deserialize;
use serde_json::json;

pub const VENUE: &str = "HYPERLIQUID";
const REST: &str = "rest";
const BASES: &[(&str, &str)] = &[(REST, "https://api.hyperliquid.xyz")];
const CAPS_TOML: &str = include_str!("capabilities.toml");

/// The venue returns at most this many rows from a time-range call, whatever range is asked
/// for. A wider range is paged by moving `startTime` forward.
const MAX_BARS: usize = 500;

/// `l2Book` returns at most this many levels a side.
const MAX_BOOK_DEPTH: u32 = 20;

#[derive(Clone)]
pub struct Hyperliquid {
    caps: Capabilities,
}

impl Hyperliquid {
    pub fn new() -> Result<Self, CapsError> {
        Ok(Hyperliquid { caps: Capabilities::from_toml(CAPS_TOML)? })
    }
}

enum Pending {
    Candles { instrument: InstrumentId, tf: Timeframe, range: TimeRange },
    Board,
    Book { instrument: InstrumentId, depth: u32 },
    Quote { instrument: InstrumentId },
    Perps,
    Spot,
}

#[derive(Default)]
pub struct Session {
    pending: HashMap<(CorrId, &'static str), Pending>,
    /// `LoadInstruments` asks `meta` and `spotMeta`; the list ends once both have answered.
    outstanding: HashMap<CorrId, u8>,
}

/// The interval token the venue takes, for a timeframe it serves natively. Hyperliquid's
/// ladder is the same spelling OTW uses, so this is the identity on every supported value.
fn interval(tf: Timeframe) -> Option<&'static str> {
    Some(match (tf.n, tf.unit) {
        (1, TfUnit::Minute) => "1m",
        (3, TfUnit::Minute) => "3m",
        (5, TfUnit::Minute) => "5m",
        (15, TfUnit::Minute) => "15m",
        (30, TfUnit::Minute) => "30m",
        (1, TfUnit::Hour) => "1h",
        (2, TfUnit::Hour) => "2h",
        (4, TfUnit::Hour) => "4h",
        (8, TfUnit::Hour) => "8h",
        (12, TfUnit::Hour) => "12h",
        (1, TfUnit::Day) => "1d",
        (3, TfUnit::Day) => "3d",
        (1, TfUnit::Week) => "1w",
        (1, TfUnit::Month) => "1M",
        _ => return None,
    })
}

/// The bar's length in milliseconds. `None` for a calendar month, which has no fixed length
/// and so cannot be stepped over by arithmetic — see `decode_candles` for why that is safe.
fn tf_millis(tf: Timeframe) -> Option<i64> {
    let unit = match tf.unit {
        TfUnit::Second => 1_000,
        TfUnit::Minute => 60_000,
        TfUnit::Hour => 3_600_000,
        TfUnit::Day => 86_400_000,
        TfUnit::Week => 604_800_000,
        TfUnit::Month => return None,
    };
    Some(i64::from(tf.n) * unit)
}

/// An `/info` call: POST, JSON body, and the header the venue reads the body by.
fn post(endpoint: &'static str, corr: CorrId, body: serde_json::Value) -> Result<HttpRequest, ConnectorError> {
    let bytes = serde_json::to_vec(&body).map_err(|e| ConnectorError::invalid(format!("hyperliquid {endpoint}: {e}")))?;
    let mut h = HttpRequest::get(endpoint, REST, "/info", corr);
    h.method = Method::Post;
    h.body = Some(bytes.into());
    // The driver sets no Content-Type on a POST; the venue reads the body only when it is set.
    h.headers.push(("Content-Type".into(), "application/json".into()));
    Ok(h)
}

fn env(ctx: &MapCtx<'_>, corr: CorrId, event: Event) -> Envelope {
    Envelope::new(ctx.connector.clone(), Some(corr), ctx.received_ts, event)
}

fn decode_err(what: &str, e: serde_json::Error) -> ConnectorError {
    ConnectorError::decode(format!("hyperliquid {what}: {e}"))
}

#[derive(Deserialize)]
struct Bar {
    /// Open time, epoch milliseconds.
    t: i64,
    #[serde(default)]
    o: OptDec,
    #[serde(default)]
    h: OptDec,
    #[serde(default)]
    l: OptDec,
    #[serde(default)]
    c: OptDec,
    #[serde(default)]
    v: OptDec,
    #[serde(default)]
    n: Option<u64>,
}

#[derive(Deserialize)]
struct Meta {
    #[serde(default)]
    universe: Vec<Perp>,
}

#[derive(Deserialize)]
struct Perp {
    name: String,
    #[serde(rename = "szDecimals", default)]
    sz_decimals: Option<u32>,
}

#[derive(Deserialize)]
struct SpotMeta {
    #[serde(default)]
    universe: Vec<SpotPair>,
    #[serde(default)]
    tokens: Vec<SpotToken>,
}

#[derive(Deserialize)]
struct SpotPair {
    /// `[base token index, quote token index]` into `tokens`.
    #[serde(default)]
    tokens: Vec<usize>,
    /// The venue's own id for the pair: `PURR/USDC` for the first, `@107` for the rest.
    name: String,
}

#[derive(Deserialize)]
struct SpotToken {
    name: String,
    #[serde(rename = "szDecimals", default)]
    sz_decimals: Option<u32>,
    /// The token's own index, which is what a pair's `tokens` pair refers to. The venue
    /// keeps it equal to the array position, but keying by it is what the field means.
    #[serde(default)]
    index: Option<usize>,
}

#[derive(Deserialize)]
struct BookAnswer {
    /// `[bids, asks]`, best price first.
    #[serde(default)]
    levels: Vec<Vec<Level>>,
}

#[derive(Deserialize)]
struct Level {
    #[serde(default)]
    px: OptDec,
    #[serde(default)]
    sz: OptDec,
}

/// One side of the book, best first, cut to the depth asked for. A level missing a price or
/// a size is a hole, never a zero.
fn side(levels: Vec<Level>, depth: u32) -> Vec<BookLevel> {
    levels
        .into_iter()
        .take(depth as usize)
        .filter_map(|l| {
            let (Some(px), Some(sz)) = (l.px.0, l.sz.0) else { return None };
            Some(BookLevel { price: Price(px), qty: Qty(sz) })
        })
        .collect()
}

impl Hyperliquid {
    fn decode_candles(
        &self,
        corr: CorrId,
        frame: &RawFrame,
        body: &[u8],
        instrument: InstrumentId,
        tf: Timeframe,
        range: TimeRange,
        ctx: &MapCtx<'_>,
    ) -> Result<Vec<Envelope>, ConnectorError> {
        // The status first: a gateway's JSON error read as a series would look like a bad coin.
        if let Some(e) = http::classify(frame, "hyperliquid") {
            return Err(e);
        }
        // An unknown coin answers `null` rather than an empty list.
        let rows: Option<Vec<Bar>> = serde_json::from_slice(body).map_err(|e| decode_err("candleSnapshot", e))?;
        let rows =
            rows.ok_or_else(|| ConnectorError::not_found(format!("hyperliquid candleSnapshot: no series for {}, bad coin?", instrument.native)))?;
        let count = rows.len();
        let last_ms = rows.last().map(|b| b.t);
        let mut candles = Vec::with_capacity(count);
        for b in rows {
            let Some(open_ts) = Timestamp::from_millis(b.t) else { continue };
            if !range.contains(open_ts) {
                continue;
            }
            let (Some(o), Some(h), Some(l), Some(c)) = (b.o.0, b.h.0, b.l.0, b.c.0) else { continue };
            candles.push(Candle {
                instrument: instrument.clone(),
                tf,
                open_ts,
                open: Price(o),
                high: Price(h),
                low: Price(l),
                close: Price(c),
                volume: Qty(b.v.0.unwrap_or_default()),
                quote_volume: None,
                trades: b.n,
                // The venue returns the bar that is still forming and does not mark it, so
                // every bar is handed over as forming and the engine closes it at the
                // period's end.
                is_final: false,
                adjusted: None,
                sides: None,
            });
        }
        candles.sort_by_key(|c| c.open_ts);
        // A full page means the range may hold more. The next page starts one bar after the
        // last one returned; a month has no fixed length, but 500 monthly bars span 41 years,
        // so a month request never needs a second page and needs no arithmetic here.
        let next_cursor = match (count >= MAX_BARS, last_ms, tf_millis(tf)) {
            (true, Some(t), Some(step)) => {
                let from = t.saturating_add(step);
                if from < range.to.millis() {
                    Some(from.to_string())
                } else {
                    None
                }
            }
            _ => None,
        };
        let done = next_cursor.is_none();
        Ok(vec![env(ctx, corr, Event::HistoryChunk { items: HistoryItems::Candles(candles), next_cursor, done })])
    }

    fn decode_board(&self, corr: CorrId, frame: &RawFrame, body: &[u8], ctx: &MapCtx<'_>) -> Result<Vec<Envelope>, ConnectorError> {
        if let Some(e) = http::classify(frame, "hyperliquid") {
            return Err(e);
        }
        let mids: BTreeMap<String, OptDec> = serde_json::from_slice(body).map_err(|e| decode_err("allMids", e))?;
        // One mid per listed instrument, keyed by the venue's own id (a perp `BTC`, a spot
        // pair `PURR/USDC` or `@107`).
        let board: Vec<LastPrice> = mids
            .into_iter()
            .filter_map(|(coin, px)| Some(LastPrice { instrument: InstrumentId::new(VENUE, &coin), price: Price(px.0?) }))
            .collect();
        Ok(vec![env(ctx, corr, Event::Board(board))])
    }

    fn decode_book(
        &self,
        corr: CorrId,
        frame: &RawFrame,
        body: &[u8],
        instrument: InstrumentId,
        depth: u32,
        ctx: &MapCtx<'_>,
    ) -> Result<Vec<Envelope>, ConnectorError> {
        if let Some(e) = http::classify(frame, "hyperliquid") {
            return Err(e);
        }
        let ans: BookAnswer = serde_json::from_slice(body).map_err(|e| decode_err("l2Book", e))?;
        let mut levels = ans.levels.into_iter();
        let bids = side(levels.next().unwrap_or_default(), depth);
        let asks = side(levels.next().unwrap_or_default(), depth);
        Ok(vec![env(ctx, corr, Event::BookSnapshot { instrument, seq: None, bids, asks, checksum: None })])
    }

    fn decode_quote(
        &self,
        corr: CorrId,
        frame: &RawFrame,
        body: &[u8],
        instrument: InstrumentId,
        ctx: &MapCtx<'_>,
    ) -> Result<Vec<Envelope>, ConnectorError> {
        if let Some(e) = http::classify(frame, "hyperliquid") {
            return Err(e);
        }
        let ans: BookAnswer = serde_json::from_slice(body).map_err(|e| decode_err("l2Book", e))?;
        let mut levels = ans.levels.into_iter();
        let bids = side(levels.next().unwrap_or_default(), MAX_BOOK_DEPTH);
        let asks = side(levels.next().unwrap_or_default(), MAX_BOOK_DEPTH);
        // The venue's `l2Book` is the only source of a top of book here, so an empty side
        // means it has no price to report yet — an answer, not a zero.
        let (Some(bid), Some(ask)) = (bids.first(), asks.first()) else {
            return Err(ConnectorError::not_found(format!("hyperliquid l2Book: no quote for {}, empty side", instrument.native)));
        };
        let quote = Quote {
            instrument,
            bid: bid.price,
            ask: ask.price,
            bid_qty: Some(bid.qty),
            ask_qty: Some(ask.qty),
        };
        Ok(vec![env(ctx, corr, Event::Quote(quote))])
    }

    fn decode_perps(&self, corr: CorrId, frame: &RawFrame, body: &[u8], ctx: &MapCtx<'_>) -> Result<Vec<Envelope>, ConnectorError> {
        if let Some(e) = http::classify(frame, "hyperliquid") {
            return Err(e);
        }
        let meta: Meta = serde_json::from_slice(body).map_err(|e| decode_err("meta", e))?;
        let mut out = Vec::with_capacity(meta.universe.len());
        for p in meta.universe {
            let mut spec = InstrumentSpec::spot(InstrumentId::new(VENUE, &p.name), &p.name, "USD");
            spec.asset_class = AssetClass::Perp;
            spec.contract = ContractType::Perp;
            // `szDecimals` is the size step; the price tick is a significant-figures rule the
            // venue applies itself, not a fixed increment, so it is left unstated.
            spec.lot_size = p.sz_decimals.map(|d| Qty(Decimal::new(1, d)));
            spec.note = Some("perpetual".into());
            out.push(env(ctx, corr, Event::Instrument { spec: Box::new(spec) }));
        }
        Ok(out)
    }

    fn decode_spot(&self, corr: CorrId, frame: &RawFrame, body: &[u8], ctx: &MapCtx<'_>) -> Result<Vec<Envelope>, ConnectorError> {
        if let Some(e) = http::classify(frame, "hyperliquid") {
            return Err(e);
        }
        let meta: SpotMeta = serde_json::from_slice(body).map_err(|e| decode_err("spotMeta", e))?;
        let tokens = meta.tokens;
        // A pair names its tokens by their `index`, which the venue keeps equal to the array
        // position but which is what the field means.
        let by_index: HashMap<usize, &SpotToken> =
            tokens.iter().enumerate().map(|(pos, t)| (t.index.unwrap_or(pos), t)).collect();
        let mut out = Vec::with_capacity(meta.universe.len());
        for p in meta.universe {
            let base = p.tokens.first().and_then(|i| by_index.get(i).copied());
            let Some(base) = base else { continue };
            let quote = p.tokens.get(1).and_then(|i| by_index.get(i).copied()).map(|t| t.name.clone()).unwrap_or_else(|| "USDC".to_string());
            // The pair's own id stays the venue's (`@107`), which is what its calls take;
            // the readable form (`HYPE/USDC`) is what a person sees.
            let mut spec = InstrumentSpec::spot(InstrumentId::new(VENUE, &p.name), &base.name, &quote);
            spec.lot_size = base.sz_decimals.map(|d| Qty(Decimal::new(1, d)));
            spec.note = Some("spot".into());
            out.push(env(ctx, corr, Event::Instrument { spec: Box::new(spec) }));
        }
        Ok(out)
    }

    /// One of a `LoadInstruments`' two answers has arrived; the list ends after the last.
    fn finish_instruments(&self, corr: CorrId, s: &mut Session, out: &mut Vec<Envelope>, ctx: &MapCtx<'_>) {
        let left = {
            let e = s.outstanding.entry(corr).or_insert(0);
            *e = e.saturating_sub(1);
            *e
        };
        if left == 0 {
            s.outstanding.remove(&corr);
            out.push(env(ctx, corr, Event::InstrumentsEnd));
        }
    }
}

impl Mapper for Hyperliquid {
    type Session = Session;

    fn capabilities(&self) -> &Capabilities {
        &self.caps
    }

    fn base_urls(&self) -> &'static [(&'static str, &'static str)] {
        BASES
    }

    fn instrument_id(&self, symbol: &str) -> Result<InstrumentId, ConnectorError> {
        let s = symbol.trim();
        if s.is_empty() {
            return Err(ConnectorError::not_found("Hyperliquid: empty symbol"));
        }
        // A perp is a bare coin (`BTC`); a spot pair is `PURR/USDC` or the venue's own
        // `@107`; `allMids` also lists `#114910`-style ids. Kept exactly as typed.
        if !s.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '@' | '/' | '#' | '-' | '_' | ':')) {
            return Err(ConnectorError::not_found(format!(
                "Hyperliquid: bad symbol `{symbol}`, write it as the venue spells it (BTC, PURR/USDC, @107)"
            )));
        }
        Ok(InstrumentId::new(VENUE, s))
    }

    fn encode(&self, cmd: &Command, s: &mut Session, _ctx: &MapCtx<'_>) -> Result<Vec<NativeRequest>, ConnectorError> {
        match cmd {
            Command::RequestHistory { req_id, instrument, data_type: DataType::Candles { tf }, range, cursor } => {
                let Some(iv) = interval(*tf) else {
                    return Err(ConnectorError::unsupported(format!("Hyperliquid serves no {tf} candles")));
                };
                if !self.caps.history_timeframes().contains(tf) {
                    return Err(ConnectorError::unsupported(format!("Hyperliquid serves no {tf} history here")));
                }
                let from = match cursor {
                    Some(c) => c.parse::<i64>().map_err(|_| ConnectorError::invalid(format!("hyperliquid cursor `{c}`")))?,
                    None => range.from.millis(),
                };
                // The venue's range is inclusive at both ends; ours is half-open.
                let to = range.to.millis().saturating_sub(1);
                let body = json!({
                    "type": "candleSnapshot",
                    "req": { "coin": instrument.native, "interval": iv, "startTime": from, "endTime": to },
                });
                s.pending.insert((*req_id, "candles"), Pending::Candles { instrument: instrument.clone(), tf: *tf, range: *range });
                Ok(vec![NativeRequest::Http(post("candles", *req_id, body)?)])
            }
            Command::RequestSnapshot { req_id, instrument, data_type: DataType::BookSnapshot { depth } } => {
                s.pending.insert(
                    (*req_id, "book"),
                    Pending::Book { instrument: instrument.clone(), depth: (*depth).min(MAX_BOOK_DEPTH) },
                );
                Ok(vec![NativeRequest::Http(post("book", *req_id, json!({ "type": "l2Book", "coin": instrument.native }))?)])
            }
            Command::RequestSnapshot { req_id, instrument, data_type: DataType::Quote } => {
                s.pending.insert((*req_id, "quote"), Pending::Quote { instrument: instrument.clone() });
                Ok(vec![NativeRequest::Http(post("quote", *req_id, json!({ "type": "l2Book", "coin": instrument.native }))?)])
            }
            Command::RequestBoard { req_id } if self.caps.history.board => {
                s.pending.insert((*req_id, "board"), Pending::Board);
                Ok(vec![NativeRequest::Http(post("board", *req_id, json!({ "type": "allMids" }))?)])
            }
            Command::LoadInstruments { req_id } => {
                // The venue splits its catalog in two: perpetuals and spot pairs.
                s.outstanding.insert(*req_id, 2);
                s.pending.insert((*req_id, "meta"), Pending::Perps);
                s.pending.insert((*req_id, "spotMeta"), Pending::Spot);
                Ok(vec![
                    NativeRequest::Http(post("meta", *req_id, json!({ "type": "meta" }))?),
                    NativeRequest::Http(post("spotMeta", *req_id, json!({ "type": "spotMeta" }))?),
                ])
            }
            Command::Connect | Command::Disconnect => Ok(vec![]),
            other => Err(ConnectorError::unsupported(format!("Hyperliquid: {other:?} is not declared"))),
        }
    }

    fn decode(&self, frame: &RawFrame, s: &mut Session, ctx: &MapCtx<'_>) -> Result<Vec<Envelope>, ConnectorError> {
        let (corr, endpoint, body) = match frame {
            RawFrame::Http { corr_id, endpoint_id, body, .. } => (*corr_id, *endpoint_id, body),
            RawFrame::Timeout { corr_id, endpoint_id } => {
                s.pending.remove(&(*corr_id, *endpoint_id));
                s.outstanding.remove(corr_id);
                return Err(http::timeout(frame, "hyperliquid").unwrap_or_else(|| ConnectorError::decode("timeout")));
            }
            RawFrame::Ws { .. } | RawFrame::Closed { .. } => return Ok(vec![]),
        };
        let pending = s
            .pending
            .remove(&(corr, endpoint))
            .ok_or_else(|| ConnectorError::decode(format!("answer for unknown request {corr} {endpoint}")))?;
        match pending {
            Pending::Candles { instrument, tf, range } => self.decode_candles(corr, frame, body, instrument, tf, range, ctx),
            Pending::Board => self.decode_board(corr, frame, body, ctx),
            Pending::Book { instrument, depth } => self.decode_book(corr, frame, body, instrument, depth, ctx),
            Pending::Quote { instrument } => self.decode_quote(corr, frame, body, instrument, ctx),
            Pending::Perps => {
                let mut out = self.decode_perps(corr, frame, body, ctx)?;
                self.finish_instruments(corr, s, &mut out, ctx);
                Ok(out)
            }
            Pending::Spot => {
                let mut out = self.decode_spot(corr, frame, body, ctx)?;
                self.finish_instruments(corr, s, &mut out, ctx);
                Ok(out)
            }
        }
    }

    fn abandon(&self, corr_id: CorrId, s: &mut Session) {
        s.pending.retain(|(c, _), _| *c != corr_id);
        s.outstanding.remove(&corr_id);
    }
}

#[cfg(test)]
mod tests;
#[cfg(test)]
mod matrix_tests;
