# xmip-core-observe-prometheus

Prometheus scrape endpoint: a node's moods, severities and counted figures
at `/metrics`, in the text exposition format 0.0.4. A technology of
[xmip-core-observe](https://github.com/IlleNilsson/xmip-core-observe).

## What is exposed

What is exposed is observe's: the snapshot, and the figures
`observe::FIGURES` names in it — the same the OTLP exporter sends — read
once per scrape with `observe::Reading`. This crate names no figure of its
own.

`exposition::write` writes one family per figure with points, named by the
figure's segments joined by underscores: `xmip_health`,
`xmip_health_severity`, `xmip_streams`, `xmip_messages`, `xmip_journeys`,
`xmip_bytes`, `xmip_retrying`, `xmip_failed`. Each has its `# HELP` and
`# TYPE`, then a sample per scope labelled `scope`, and a mood's `mood`. A
mood's value is its rank, `fine` 0 to `holding` 6. No timestamps: a scrape
is of now.

Every family is a `gauge`. A mood, a severity and what awaits another try
are levels; a window's count starts again from nothing each window, and a
Prometheus `counter` only ever rises, so a count over a window is a gauge
too — read it as it stands, not through `rate()`. No name carries
`_total`, which the conventions keep for counters. A label value is
escaped as the format says — a backslash, a double quote, a line feed —
and each scope's label is written once per scrape.

## How it is served

`Scrape::start(bind, timeout)` listens on `bind` — `0.0.0.0:9464`, or
`127.0.0.1:0` for any free port (`address` says which) — and serves on a
thread of its own. The node hands it each snapshot it publishes,
`Scrape::offer(Arc<Snapshot>)`: a lock and a handle, nothing else. A scrape
takes whatever is held once its request is read and renders it then, so it
never reads a copy older than the last change, and the node never waits for
a scraper. Each connection is one scrape, taken off it by
`transport-http`'s `server::answer_on` — HTTP/1.1, or HTTP/2 by prior
knowledge — with its reads and writes bounded by `timeout`, so a scraper
that stalls holds up only itself. `GET /metrics` is answered with
`text/plain; version=0.0.4; charset=utf-8`; another path is 404 and
another method 405. HTTPS is not served yet.

## Cost

Measured in release, `cargo test --release -- --ignored --nocapture cost`,
on AlmaLinux 10 under WSL, the fastest of 200 runs while the machine was
otherwise quiet (under the estate's own builds every figure doubled or
trebled): a snapshot of a thousand Receive Locations, each with a mood and
every count, is 8,000 samples and 565 kB.

| What | Where it runs | Fastest seen |
| --- | --- | --- |
| `offer` | the node's thread | 0.2–0.4 µs |
| Writing the text | the scrape | 0.72–0.92 ms |
| A change scraped on loopback | end to end | 1.7–2.0 ms |

## Toolchain

`rust-toolchain.toml` pins the toolchain for the whole estate. Do not change it
here.

## Verification

The included workflow is manual-only and calls the versioned shared workflow at
`IlleNilsson/.github@v1`. The tests read every scrape with a reader that
holds the exposition format's rules — names, label escapes, one `TYPE` and
one `HELP` per family before its samples, a family's samples together, no
sample twice — and scrape the endpoint over HTTP/1.1 and HTTP/2.
