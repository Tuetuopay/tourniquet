//! Easily round-robin between servers providing the same service, automatically reconnecting to the
//! next server should an error happen.
//!
//! This library facilitates resiliency to multiple service providers (e.g. servers) by connecting
//! to the first available service from a list in a round-robin manner. If for some reason any
//! provider goes down, any attempt to interact with it will reconnect to the next one,
//! automatically.
//!
//! Disclaimer: this library is not for load-balancing between a set of providers! It will connect
//! to _one_ provider, and only use this one provider as long as it is up. Tourniquet is meant for
//! resiliency and not for load balancing.
//!
//! # Example
//!
//! ```rust
//! use async_trait::async_trait;
//! use std::{io::Error, net::IpAddr};
//! use tokio::{io::AsyncReadExt, net::TcpStream, sync::Mutex};
//! use tourniquet::{Connector, RoundRobin};
//!
//! struct Conn(u16);
//!
//! #[async_trait]
//! impl Connector<IpAddr, Mutex<TcpStream>, Error> for Conn {
//!     async fn connect(&self, src: &IpAddr) -> Result<Mutex<TcpStream>, Error> {
//!         let Conn(ref port) = self;
//!         TcpStream::connect((*src, *port)).await.map(Mutex::new)
//!     }
//! }
//!
//! #[tokio::main]
//! async fn main() {
//!     let rr = RoundRobin::new(
//!         vec!["46.16.175.175".parse().unwrap(), "51.161.82.214".parse().unwrap()],
//!         Conn(6667),
//!     );
//!
//!     let hello = rr.run(|sock| async move {
//!         let mut sock = sock.lock().await;
//!         let mut buf = [0; 50];
//!         sock.read_exact(&mut buf).await.map(|_| String::from_utf8(buf.to_vec()).unwrap())
//!     }).await.unwrap();
//!
//!     assert!(hello.contains("libera.chat"));
//! }
//! ```
//!
//! # Integrations
//!
//! Tourniquet provides some integrations with existing crates, which shorthand functions to reduce
//! boilerplate, as the raw `run` function may not be the most wieldy to use. It currently
//! integrates with the following crates:
//!
//! - [`celery`] with [`tourniquet-celery`]: provides a near drop-in replacement for the regular
//!   `send_task` function from [`celery`]
//! - [`tonic`] with [`tourniquet-tonic`]: provides a shorthand `chan` function with a cloned
//!   channel rather than an `Arc`-ed one
//!
//! [`celery`]: https://lib.rs/celery
//! [`tonic`]: https://lib.rs/tonic
//! [`tourniquet-celery`]: https://lib.rs/tourniquet-celery
//! [`tourniquet-tonic`]: https://lib.rs/tourniquet-tonic

use core::future::Future;
use std::{
    fmt::{Debug, Display},
    marker::PhantomData,
    sync::atomic::{AtomicUsize, Ordering},
    sync::Arc,
};

pub use async_trait::async_trait;
use tokio::sync::RwLock;
#[cfg(feature = "tracing")]
use tracing::{
    field::{debug, display, Empty},
    instrument, Instrument, Span,
};

/// Trait indicating wether an error mandates trying the next service.
///
/// It is returned by the round-robin handler or the connector, and indicates wether we should
/// try the next service, or if it should abort and bubble up the error to the caller.
///
/// A next error mandates trying the next service in the line. It is an error that could be solved
/// by trying another server. This includes IO errors, server-side errors, etc. On the other hand,
/// business errors should not be a next error since another server will most likely yield the same
/// error (resource not found, permission denied, ...).
///
/// Basically, a server that yields a Next error should be considered unhealthy.
///
/// # Example
///
/// ```rust
/// # use tourniquet::Next;
/// #
/// enum MyError {
///     NotFound,
///     PermissionDenied,
///     InternalError,
///     Timeout,
/// }
///
/// impl Next for MyError {
///     fn is_next(&self) -> bool {
///         match self {
///             // Business logic error, that are likely to happen on all servers
///             Self::NotFound | Self::PermissionDenied => false,
///             // Server specific error: server software down, host down, etc. Try the next one
///             Self::InternalError | Self::Timeout => true,
///         }
///     }
/// }
/// ```
pub trait Next {
    /**
     * If true, the error is non-fatal and the next service in the list will be tried.
     */
    fn is_next(&self) -> bool;
}

impl Next for std::io::Error {
    fn is_next(&self) -> bool {
        use std::io::ErrorKind::*;
        match self.kind() {
            ConnectionRefused | ConnectionReset | ConnectionAborted | NotConnected | BrokenPipe
            | TimedOut | Interrupted | UnexpectedEof => true,
            NotFound | PermissionDenied | AddrInUse | AddrNotAvailable | AlreadyExists
            | WouldBlock | InvalidInput | InvalidData | Other => false,
            _ => false,
        }
    }
}

/// Trait to be implemented by connector types. Used to get a connected service from its connection
/// information.
///
/// Note that the implementor type can hold data for connection information shared across all
/// providers, like TLS certificates, port, database name, ...
///
/// # Example
///
/// ```rust
/// # use async_trait::async_trait;
/// # use std::sync::Mutex;
/// use std::{io::Error, net::IpAddr};
/// use tokio::net::TcpStream;
/// use tourniquet::Connector;
///
/// struct Conn(u16);
///
/// #[async_trait]
/// impl Connector<IpAddr, Mutex<TcpStream>, Error> for Conn {
///     async fn connect(&self, src: &IpAddr) -> Result<Mutex<TcpStream>, Error> {
///         let Conn(ref port) = self;
///         TcpStream::connect((*src, *port)).await.map(Mutex::new)
///     }
/// }
/// ```
#[async_trait]
pub trait Connector<SvcSrc, Svc, E> {
    async fn connect(&self, src: &SvcSrc) -> Result<Svc, E>;
}

/// Round Robin manager.
///
/// This holds a list of services, a way to connect to said services, and a way to run stuff against
/// this connected service.
pub struct RoundRobin<SvcSrc, Svc, E, Conn>
where
    Conn: Connector<SvcSrc, Svc, E>,
{
    /// Sources used to connect to a service. Usually some form of URL to attempt a connection,
    /// e.g. `amqp://localhost:5672`
    sources: Vec<SvcSrc>,

    /// Async connection handler.
    connector: Conn,

    /// How many services to try before giving up. Defaults to the service count.
    max_attempts: usize,

    /// Already connected service handler. We use Arc here to be able to easily clone the service
    /// handler and avoid issues with references, as they don't play nicely with futures.
    service: RwLock<Option<Arc<Svc>>>,

    /// Current service source being connected
    current: AtomicUsize,

    _phantom: PhantomData<E>,
}

impl<SvcSrc, Svc, E, Conn> RoundRobin<SvcSrc, Svc, E, Conn>
where
    SvcSrc: Debug,
    E: Next + Display,
    Conn: Connector<SvcSrc, Svc, E>,
{
    /// Build a new round-robin manager.
    ///
    /// The connector is a struct that yields a connected handler to the service, from its service
    /// "source" (usually an URL). Note that the connector is lazily executed on demand when a
    /// connection is needed.
    ///
    /// # Example
    ///
    /// ```rust
    /// # use async_trait::async_trait;
    /// # use std::sync::Mutex;
    /// use std::{io::Error, net::IpAddr};
    /// use tokio::net::TcpStream;
    /// use tourniquet::{Connector, RoundRobin};
    ///
    /// struct Conn(u16);
    ///
    /// #[async_trait]
    /// impl Connector<IpAddr, Mutex<TcpStream>, Error> for Conn {
    ///     async fn connect(&self, src: &IpAddr) -> Result<Mutex<TcpStream>, Error> {
    ///         let Conn(ref port) = self;
    ///         TcpStream::connect((*src, *port)).await.map(Mutex::new)
    ///     }
    /// }
    ///
    /// # #[tokio::main]
    /// # async fn main() {
    /// let rr = RoundRobin::new(
    ///     vec!["185.30.166.38".parse().unwrap(), "66.110.9.37".parse().unwrap()],
    ///     Conn(6667),
    /// );
    /// # }
    /// ```
    pub fn new(sources: Vec<SvcSrc>, connector: Conn) -> Self {
        Self {
            max_attempts: sources.len() + 1,
            sources,
            connector,
            service: RwLock::new(None),
            current: AtomicUsize::new(0),
            _phantom: PhantomData,
        }
    }

    /// Set how many times we will try the next service in case of failure.
    pub fn set_max_attempts(&mut self, count: usize) {
        self.max_attempts = count;
    }

    /// Set how many times we will try the next service in case of failure.
    pub fn max_attempts(self, count: usize) -> Self {
        Self { max_attempts: count, ..self }
    }

    #[cfg_attr(
        feature = "tracing",
        tracing::instrument(skip(self, run), err, fields(service = Empty, index = Empty)),
    )]
    async fn run_inner<Run, RunFut, T>(&self, run: &Run, current: usize) -> Result<T, E>
    where
        Run: Fn(Arc<Svc>) -> RunFut,
        RunFut: Future<Output = Result<T, E>>,
    {
        let index = current % self.sources.len();

        #[cfg(feature = "tracing")]
        {
            let span = Span::current();
            span.record("index", &display(index));
            span.record("service", &debug(&self.sources[index]));
        }

        // Connect if not already connected
        if self.service.read().await.is_none() {
            *self.service.write().await =
                Some(Arc::new(self.connector.connect(&self.sources[index]).await?));
        }

        // Run
        let fut = run(self.service.read().await.clone().unwrap());
        #[cfg(feature = "tracing")]
        let fut = fut.instrument(tracing::debug_span!("run_fn"));
        let res = fut.await;

        if let Err(ref e) = res {
            // Trash handler only if that's a next error and if we didn't already move to the next
            // provider (e.g. in another concurrent task).
            if e.is_next() && current == self.current.load(Ordering::Relaxed) {
                *self.service.write().await = None;
            }
        }

        res
    }

    /// Run the provided async function against an established service connection.
    ///
    /// The connection to the service will be established at this point if not already established.
    #[cfg_attr(feature = "tracing", instrument(skip(self, run), err))]
    pub async fn run<R, Fut, T>(&self, run: R) -> Result<T, E>
    where
        R: Fn(Arc<Svc>) -> Fut,
        Fut: Future<Output = Result<T, E>>,
    {
        let n_svc = self.sources.len();
        let mut attempts = 0usize;

        loop {
            let current = self.current.load(Ordering::Relaxed);

            match self.run_inner(&run, current).await {
                Ok(t) => return Ok(t),
                Err(e) => {
                    if e.is_next() {
                        log::error!("Service {}/{} failed: {}", current % n_svc, n_svc, e);

                        self.current.fetch_add(1, Ordering::Relaxed);
                        attempts += 1;
                        if attempts < self.max_attempts {
                            continue;
                        }
                    }

                    return Err(e);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };

    #[derive(Debug, PartialEq)]
    enum Error {
        Timeout,
        NotFound,
    }

    impl std::fmt::Display for Error {
        fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
            write!(f, "{:?}", self)
        }
    }

    impl Next for Error {
        fn is_next(&self) -> bool {
            *self == Self::Timeout
        }
    }

    struct Conn {
        count: Arc<AtomicUsize>,
        ok_from: i32,
    }

    #[async_trait]
    impl Connector<i32, i32, Error> for Conn {
        async fn connect(&self, src: &i32) -> Result<i32, Error> {
            self.count.fetch_add(1, Ordering::Relaxed);
            if *src < self.ok_from {
                Err(Error::Timeout)
            } else {
                Ok(*src)
            }
        }
    }

    fn build_rr(
        svcs: Vec<i32>,
        ok_from: i32,
    ) -> (RoundRobin<i32, i32, Error, Conn>, Arc<AtomicUsize>) {
        let count = Arc::new(AtomicUsize::new(0));
        let cnt = count.clone();

        (RoundRobin::new(svcs, Conn { count: cnt, ok_from }), count)
    }

    #[tokio::test]
    async fn test_first_called() {
        let (rr, count_conn) = build_rr(vec![0, 1], 0);

        let count_run = AtomicUsize::new(0);

        rr.run(|_| async { Ok(count_run.fetch_add(1, Ordering::Relaxed)) }).await.unwrap();

        // Async blocks should be called only once
        assert_eq!(count_conn.load(Ordering::Relaxed), 1);
        assert_eq!(count_run.load(Ordering::Relaxed), 1);

        rr.run(|_| async { Ok(count_run.fetch_add(1, Ordering::Relaxed)) }).await.unwrap();

        // Connector should not have been called a second time, though the run block should.
        assert_eq!(count_conn.load(Ordering::Relaxed), 1);
        assert_eq!(count_run.load(Ordering::Relaxed), 2);
    }

    #[tokio::test]
    async fn test_exhausted_run() {
        let (mut rr, count_conn) = build_rr(vec![0, 1], 0);

        let count = AtomicUsize::new(0);

        // With a single attemt, options will be exhausted
        rr.set_max_attempts(1);

        let res = rr
            .run(|n| {
                count.fetch_add(1, Ordering::Relaxed);
                async move {
                    match *n {
                        0 => Err(Error::Timeout), // Next error
                        _ => Ok(n),
                    }
                }
            })
            .await;

        assert_eq!(count_conn.load(Ordering::Relaxed), 1);
        match res {
            Ok(_) => panic!("Run did not error"),
            Err(Error::Timeout) => (),
            Err(_) => panic!("Wrong error"),
        }
        assert_eq!(count.load(Ordering::Relaxed), 1);
    }

    #[tokio::test]
    async fn test_try_next_run() {
        let (rr, count_conn) = build_rr(vec![0, 1], 0);

        let count = AtomicUsize::new(0);

        rr.run(|n| {
            count.fetch_add(1, Ordering::Relaxed);
            async move {
                match *n {
                    0 => Err(Error::Timeout), // Next error
                    _ => Ok(n),
                }
            }
        })
        .await
        .unwrap();

        assert_eq!(count_conn.load(Ordering::Relaxed), 2);
        assert_eq!(count.load(Ordering::Relaxed), 2);
    }

    #[tokio::test]
    async fn test_exhausted_connector() {
        let (mut rr, count_conn) = build_rr(vec![0, 1], 1);

        let count = AtomicUsize::new(0);

        // With a single attemt, options will be exhausted
        rr.set_max_attempts(1);

        let res = rr.run(|_| async { Ok(count.fetch_add(1, Ordering::Relaxed)) }).await;

        assert_eq!(count_conn.load(Ordering::Relaxed), 1);
        match res {
            Ok(_) => panic!("Connect did not error"),
            Err(Error::Timeout) => (),
            Err(_) => panic!("Wrong error"),
        }
        assert_eq!(count.load(Ordering::Relaxed), 0);
    }

    #[tokio::test]
    async fn test_try_next_connector() {
        let (rr, count_conn) = build_rr(vec![0, 1], 1);

        let count = AtomicUsize::new(0);

        rr.run(|_| async { Ok(count.fetch_add(1, Ordering::Relaxed)) }).await.unwrap();

        assert_eq!(count_conn.load(Ordering::Relaxed), 2);
        assert_eq!(count.load(Ordering::Relaxed), 1);
    }

    #[tokio::test]
    async fn test_abort() {
        let (rr, _) = build_rr(vec![0, 1], 1);

        let res = rr.run(|_| async { Err::<(), _>(Error::NotFound) }).await;

        match res {
            Ok(_) => panic!("Run did not error"),
            Err(Error::NotFound) => (),
            Err(Error::Timeout) => panic!("Connector error aborted"),
        }
    }
}
