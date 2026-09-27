//! The scrape endpoint: `/metrics` served on a thread of its own, from the
//! latest snapshot the node handed over.
//!
//! The node hands each snapshot it publishes to [`Scrape::offer`] — the
//! shared handle it publishes, `Arc<Snapshot>`, never a copy — which is a
//! lock and a handle and nothing else. A scrape renders what is held at
//! the moment it arrives, so it never reads a copy older than the last
//! change, and the node never waits for a scraper, however slow. The
//! endpoint speaks HTTP/1.1, and HTTP/2 to a client that opens with its
//! preface (`transport-http`'s `server::answer_on`); each connection is one
//! scrape.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, TcpListener};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::JoinHandle;
use std::time::Duration;

use net::http::{Request, Response};
use observe::Snapshot;

use crate::exposition::{CONTENT_TYPE, write};

/// The path a Prometheus server scrapes by convention.
pub const METRICS_PATH: &str = "/metrics";

/// What the node and the endpoint share.
#[derive(Default)]
struct Held {
    snapshot: Mutex<Option<Arc<Snapshot>>>,
    closing: AtomicBool,
    scrapes: AtomicU64,
}

/// A scrape endpoint listening on one address.
pub struct Scrape {
    held: Arc<Held>,
    address: SocketAddr,
    server: Option<JoinHandle<()>>,
}

impl Scrape {
    /// Listen on `bind` — `0.0.0.0:9464`, or `127.0.0.1:0` for any free
    /// port — and serve scrapes, each connection's reads bounded by
    /// `timeout` so a scraper that stalls holds up nothing but itself.
    ///
    /// # Errors
    /// Where the address could not be bound or the thread started.
    pub fn start(bind: &str, timeout: Duration) -> Result<Self, String> {
        let (listener, _) = transport::socket::bind_tcp(bind).map_err(|failure| failure.message)?;
        let address = listener
            .local_addr()
            .map_err(|failure| format!("reading the bound address: {failure}"))?;
        let held = Arc::new(Held::default());
        let shared = Arc::clone(&held);
        let server = std::thread::Builder::new()
            .name("xmip-observe-prometheus".to_string())
            .spawn(move || serve(&listener, &shared, timeout))
            .map_err(|failure| format!("starting the scrape endpoint: {failure}"))?;
        Ok(Self {
            held,
            address,
            server: Some(server),
        })
    }

    /// The address the endpoint listens on: the port `127.0.0.1:0` became.
    #[must_use]
    pub const fn address(&self) -> SocketAddr {
        self.address
    }

    /// Hold `snapshot` for the next scrape, in place of the last.
    pub fn offer(&self, snapshot: Arc<Snapshot>) {
        *self
            .held
            .snapshot
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(snapshot);
    }

    /// How many scrapes were answered.
    #[must_use]
    pub fn scrapes(&self) -> u64 {
        self.held.scrapes.load(Ordering::Relaxed)
    }

    /// Stop listening once the scrape in hand is answered.
    pub fn close(mut self) {
        self.stop();
    }

    fn stop(&mut self) {
        let Some(server) = self.server.take() else {
            return;
        };
        self.held.closing.store(true, Ordering::Relaxed);
        // The server waits in accept; a connection of its own wakes it to
        // see it is closing. Bound to every address, it is reached on the
        // loopback of the same family.
        let wake = match self.address.ip() {
            IpAddr::V4(ip) if ip.is_unspecified() => {
                SocketAddr::new(Ipv4Addr::LOCALHOST.into(), self.address.port())
            }
            IpAddr::V6(ip) if ip.is_unspecified() => {
                SocketAddr::new(Ipv6Addr::LOCALHOST.into(), self.address.port())
            }
            _ => self.address,
        };
        drop(transport::socket::connect_tcp(
            &wake.to_string(),
            Some(Duration::from_secs(1)),
        ));
        // The server's own panic is not the closer's to raise again.
        let _ = server.join();
    }
}

impl Drop for Scrape {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Answer scrapes until closing.
fn serve(listener: &TcpListener, held: &Held, timeout: Duration) {
    loop {
        // bounded: a scrape endpoint waits for its scraper as long as it runs; closing wakes it
        let accepted = listener.accept();
        if held.closing.load(Ordering::Relaxed) {
            return;
        }
        let Ok((stream, _)) = accepted else {
            // Out of descriptors, most likely: give what holds them a
            // moment rather than spinning on the refusal.
            std::thread::sleep(Duration::from_millis(10));
            continue;
        };
        if stream.set_read_timeout(Some(timeout)).is_err()
            || stream.set_write_timeout(Some(timeout)).is_err()
        {
            continue;
        }
        // The snapshot is taken once the request is read, so a scrape is
        // of the last change before it. A scraper that breaks its
        // connection has only itself to blame; the endpoint goes on.
        if transport_http::server::answer_on(stream, |request| {
            let snapshot = held
                .snapshot
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .clone();
            ((), answer(request, snapshot.as_deref()))
        })
        .is_ok()
        {
            held.scrapes.fetch_add(1, Ordering::Relaxed);
        }
    }
}

/// What a request to the endpoint is answered with.
fn answer(request: &Request, snapshot: Option<&Snapshot>) -> Response {
    if request.path != METRICS_PATH {
        return Response::new(404);
    }
    if request.method != "GET" {
        return Response::new(405).header("Allow", "GET");
    }
    let mut text = String::new();
    if let Some(snapshot) = snapshot {
        write(&mut text, snapshot);
    }
    let mut response = Response::new(200).header("Content-Type", CONTENT_TYPE);
    response.body = text.into_bytes();
    response
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::exposition::tests::snapshot;
    use crate::reader::read;
    use observe::{Health, HealthRecord};
    use std::net::TcpStream;
    use std::time::Instant;

    fn scrape(address: SocketAddr, path: &str) -> Response {
        let stream = TcpStream::connect(address).expect("connect");
        net::http::exchange(stream, &Request::new("GET", path)).expect("answered")
    }

    fn mood(of: &Response) -> f64 {
        let read = read(of.text().expect("text")).expect("the format's rules");
        read.samples[0].value
    }

    #[test]
    fn a_scrape_reads_the_last_change_and_nothing_older() {
        let endpoint = Scrape::start("127.0.0.1:0", Duration::from_secs(5)).expect("start");
        let empty = scrape(endpoint.address(), METRICS_PATH);
        assert_eq!((empty.status, empty.body.len()), (200, 0), "nothing yet");

        let mut changing = snapshot(2);
        endpoint.offer(Arc::new(changing.clone()));
        let first = scrape(endpoint.address(), METRICS_PATH);
        assert_eq!(first.header_value("content-type"), Some(CONTENT_TYPE));
        assert!((mood(&first) - 3.0).abs() < f64::EPSILON, "stressed");

        changing.record_health(HealthRecord {
            scope: "xmip:///c1/node/n1/receive/location-0000".to_string(),
            health: Health::Done,
            severity: 90,
            evidence: String::new(),
            observed_unix_nanos: 2_000,
        });
        endpoint.offer(Arc::new(changing));
        let second = scrape(endpoint.address(), METRICS_PATH);
        assert!((mood(&second) - 5.0).abs() < f64::EPSILON, "done, at once");
        assert_eq!(endpoint.scrapes(), 3);
        endpoint.close();
    }

    #[test]
    fn another_path_or_method_is_refused_and_http_2_is_served() {
        let endpoint = Scrape::start("127.0.0.1:0", Duration::from_secs(5)).expect("start");
        endpoint.offer(Arc::new(snapshot(1)));
        assert_eq!(scrape(endpoint.address(), "/").status, 404);
        let stream = TcpStream::connect(endpoint.address()).expect("connect");
        let posted = net::http::exchange(stream, &Request::new("POST", METRICS_PATH));
        assert_eq!(posted.expect("answered").status, 405);

        let stream = TcpStream::connect(endpoint.address()).expect("connect");
        let request = Request::new("GET", METRICS_PATH).header("Host", "localhost");
        let answer = net::http2::exchange(stream, "http", &request).expect("answered");
        assert_eq!(answer.status, 200);
        assert_eq!(
            read(answer.text().expect("text"))
                .expect("read")
                .samples
                .len(),
            8
        );
    }

    #[test]
    fn a_scraper_that_never_sends_holds_up_only_itself() {
        let endpoint = Scrape::start("127.0.0.1:0", Duration::from_millis(200)).expect("start");
        endpoint.offer(Arc::new(snapshot(1)));
        let _silent = TcpStream::connect(endpoint.address()).expect("connect");
        let began = Instant::now();
        let answer = scrape(endpoint.address(), METRICS_PATH);
        assert_eq!(answer.status, 200);
        assert!(began.elapsed() < Duration::from_secs(2));
    }

    /// The costs of a scrape, run on purpose in release:
    /// `cargo test --release -- --ignored --nocapture cost`. What the
    /// node's thread pays per change (an offer), what a scrape pays to
    /// write a snapshot of a thousand scopes, and how long after a change
    /// a scrape answers with it on loopback.
    #[test]
    #[ignore = "a measurement, run on purpose in release"]
    fn cost_of_one_scrape_of_a_thousand_scopes() {
        let snapshot = Arc::new(snapshot(1_000));
        let mut text = String::new();
        let mut written = Vec::new();
        for _ in 0..200 {
            text.clear();
            let began = Instant::now();
            write(&mut text, &snapshot);
            written.push(began.elapsed());
        }
        written.sort();
        let samples = read(&text).expect("read").samples.len();
        println!(
            "prometheus write: {samples} samples, {} bytes, fastest {:?}, median {:?}, 95th {:?}",
            text.len(),
            written[0],
            written[100],
            written[190]
        );

        let endpoint = Scrape::start("127.0.0.1:0", Duration::from_secs(5)).expect("start");
        let mut offered = Vec::new();
        let mut answered = Vec::new();
        for _ in 0..50 {
            let began = Instant::now();
            endpoint.offer(Arc::clone(&snapshot));
            offered.push(began.elapsed());
            assert_eq!(scrape(endpoint.address(), METRICS_PATH).status, 200);
            answered.push(began.elapsed());
        }
        offered.sort();
        answered.sort();
        println!(
            "prometheus offer: fastest {:?}, median {:?}; change to scraped: {:?}, median {:?}",
            offered[0], offered[25], answered[0], answered[25]
        );
    }
}
