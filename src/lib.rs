#![forbid(unsafe_code)]

//! Prometheus: a node's figures on a scrape endpoint, in the text
//! exposition format 0.0.4.
//!
//! A technology of `xmip-core-observe`. What is exposed is observe's — the
//! snapshot, and the figures `observe::FIGURES` names in it, the same the
//! OTLP exporter sends; this crate writes them in the format a Prometheus
//! server reads ([`exposition`]) and serves them at `/metrics` from a
//! thread of its own ([`Scrape`]) over `xmip-core-transport-http`.

pub mod exposition;
pub mod scrape;

#[cfg(test)]
mod reader;

pub use exposition::CONTENT_TYPE;
pub use scrape::{METRICS_PATH, Scrape};
