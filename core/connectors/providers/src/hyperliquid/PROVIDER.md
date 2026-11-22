# Hyperliquid

Adapter version: 0.1.0        Provider API version: `/info`        Maturity: M1

Sources:
- Info endpoint: https://hyperliquid.gitbook.io/hyperliquid-docs/for-developers/api/info-endpoint
- Perpetuals vs spot: https://hyperliquid.gitbook.io/hyperliquid-docs/for-developers/api/info-endpoint#perpetuals-vs-spot
- Pagination: https://hyperliquid.gitbook.io/hyperliquid-docs/for-developers/api/info-endpoint#pagination
- Candle snapshot: https://hyperliquid.gitbook.io/hyperliquid-docs/for-developers/api/info-endpoint#candle-snapshot
- L2 book: https://hyperliquid.gitbook.io/hyperliquid-docs/for-developers/api/info-endpoint#l2-book-snapshot
- Rate limits: https://hyperliquid.gitbook.io/hyperliquid-docs/for-developers/api/rate-limits-and-user-limits

M1: every declared capability has a fixture, unit tests, the generic error matrix and a
property test. Field names and value shapes were confirmed against the live venue
(`api.hyperliquid.xyz`) before the fixtures were written: `meta` (235 perps), `spotMeta`
(519 tokens / 354 pairs), `candleSnapshot`, `allMids`, `l2Book`.

## Endpoints

Everything is `POST https://api.hyperliquid.xyz/info` with a JSON body that names the call in
`type`. There is no query-string form, and the venue reads the body only when
`Content-Type: application/json` is set — the driver adds no header, so the mapper sets it.

| Endpoint id | Body `type` | Used for | Paging |
| --- | --- | --- | --- |
| `candles` | `candleSnapshot` | `RequestHistory{Candles}` | by `startTime`, 500 rows a call |
| `book` | `l2Book` | `RequestSnapshot{BookSnapshot}` | none: 20 levels a side |
| `quote` | `l2Book` | `RequestSnapshot{Quote}` | none: the top level of each side |
| `board` | `allMids` | `RequestBoard` | none: every instrument in one answer |
| `meta` | `meta` | `LoadInstruments` (perpetuals) | none |
| `spotMeta` | `spotMeta` | `LoadInstruments` (spot pairs) | none |

## Auth

None. Market data, candles and the L2 book are public. A wallet key is only needed to trade
or to read an account, neither of which this adapter declares, so `auth = "none"` and there
is no signer.

## Rate limits

~1200 request weight per minute per IP across the info endpoint. `candleSnapshot` is one of
the heavier calls; the adapter paces at one request a second, far under the ceiling.

## Symbols

Native id is the venue's own coin, exactly as its calls take it:

- A perpetual is a bare coin: `BTC`, `ETH`, `ATOM` (the `name` in `meta.universe`).
- A spot pair is either the venue's literal pair name (`PURR/USDC`, the first pair) or
  `@<index>` into `spotMeta.universe` (`@107` for HYPE). **`@107` is the native id, not a
  translation**: the venue's own `candleSnapshot` and `l2Book` take it directly, and
  `HYPE/USDC` is not accepted (it answers `null`).
- `allMids` also carries `#<n>`-style keys. They are emitted as they arrive rather than
  filtered, because filtering would be a guess about what the venue lists.

The readable form (`HYPE/USDC`) is derived from `spotMeta.tokens` for display only; the id
stays `@107`.

## Enum tables

| Timeframe | Interval token |
| --- | --- |
| 1m 3m 5m 15m 30m | `1m` `3m` `5m` `15m` `30m` |
| 1h 2h 4h 8h 12h | `1h` `2h` `4h` `8h` `12h` |
| 1d 3d 1w 1M | `1d` `3d` `1w` `1M` |

The venue's ladder is spelled the way OTW spells it, so the mapping is the identity on every
supported value. Anything else is refused before the call.

| Answer | Canonical |
| --- | --- |
| `null` for a coin the venue does not list | NotFound ("bad coin") |
| a `2xx` whose body does not parse | Decode |
| `4xx`/`5xx` | by status, via the SDK's shared classifier |
| an `l2Book` with an empty side | NotFound ("no quote") |

## Units and time

- Prices, sizes and volumes are decimal strings; they are parsed as exact decimals, never
  through an `f64`.
- Bar and trade times are epoch **milliseconds**; they become `Timestamp`s directly.
- `szDecimals` is the size step: `lot_size = 10^-szDecimals`. The price tick is a
  significant-figures rule the venue applies itself, not a fixed increment, so `tick_size`
  is left unstated rather than guessed.
- No forming flag (`history.candles.final_flag = false`): `candleSnapshot` returns the bar
  that is still forming and does not mark it, so every bar is handed over as forming and the
  engine finalizes it at the period's end.

## Quirks

- **Paging.** A time-range call returns at most 500 elements. A full page continues from one
  bar after its last one (`startTime = last.t + bar length`), not from `last.t`, so no bar is
  served twice. A month has no fixed length, so it is never stepped arithmetically — but 500
  monthly bars span 41 years, so a month request cannot need a second page.
- **Half-open windows.** OTW's `TimeRange` is `[from, to)`; the venue's `endTime` is
  inclusive, so the request sends `to - 1 ms` and the adapter filters again on the way back.
- **Two catalogs.** `LoadInstruments` is two calls (`meta` and `spotMeta`); the instrument
  list ends once both have answered, tracked by a per-request counter in the session.
- **`allMids` is one call for everything**, so a board refresh costs one request, not one per
  instrument.
- **No server-time endpoint** is declared, so `SyncTime` is refused rather than approximated
  from the host clock.

## Not declared

- **Live streaming.** The sockets (`wss://api.hyperliquid.xyz/ws`, channels `candle`,
  `trades`, `l2Book`, `allMids`) exist and were exercised, but the live adapter is not wired,
  so `live` is absent from `capabilities.toml` and `stream_asset_types` is empty. Offering
  live and failing at subscribe would be worse than not offering it.
- **Trading and account reads.** `auth = "none"`; no order, balance or fill path.
- **Funding rate, open interest, news.**

## Regions and terms

No regional restriction published for the public info endpoint. Hyperliquid is a
self-custody venue: trading would need a wallet key, which this adapter never asks for.
