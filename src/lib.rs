/*!
 * Easily round-robin between servers providing the same service, automatically reconnecting to the
 * next server should an error happen.
 */

use core::future::Future;
use std::{
    fmt::{Debug, Display},
    marker::PhantomData,
    sync::atomic::{AtomicUsize, Ordering},
    sync::Arc,
};

use async_trait::async_trait;
use tokio::sync::RwLock;

/**
 * Trait indicating wether an error mandates trying the next service.
 *
 * It is returned by the round-robin handler or the connector, and indicates wether we should
 * try the next service, or if it should abort and bubble up the error to the caller.
 *
 * A next error mandates trying the next service in the line. It is an error that could be solved
 * by trying another server. This includes IO errors, server-side errors, etc. On the other hand,
 * business errors should not be a next error since another server will most likely yield the same
 * error (resource not found, permission denied, ...).
 *
 * Basically, a server that yields a Next error should be considered unhealthy.
 */
pub trait Next {
    /**
     * If true, the error is non-fatal and the next service in the list will be tried.
     */
    fn is_next(&self) -> bool;
}

/**
 * Trait to be implemented by connector types. Used to get a connected service from its connection
 * information.
 */
#[async_trait]
pub trait Connector<SvcSrc, Svc, E> {
    async fn connect(&self, src: &SvcSrc) -> Result<Svc, E>;
}

/**
 * Round Robin manager.
 *
 * This holds a list of services, a way to connect to said services, and a way to run stuff against
 * this connected service.
 */
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
    /**
     * Build a new round-robin manager.
     *
     * The connector is an async function (or closure returning a future) that yields a connected
     * handler to the service, from its service "source" (usually an URL).
     *
     * Note that the connector function is not run at this point.
     */
    pub fn new(sources: Vec<SvcSrc>, connector: Conn) -> Self {
        Self {
            max_attempts: sources.len(),
            sources,
            connector,
            service: RwLock::new(None),
            current: AtomicUsize::new(0),
            _phantom: PhantomData::default(),
        }
    }

    /**
     * Set how many times we will try the next service in case of failure.
     */
    pub fn set_max_attempts(&mut self, count: usize) {
        self.max_attempts = count;
    }

    async fn run_inner<Run, RunFut, T>(&self, run: &Run, current: usize) -> Result<T, E>
    where
        Run: Fn(Arc<Svc>) -> RunFut,
        RunFut: Future<Output = Result<T, E>>,
    {
        let index = current % self.sources.len();

        // Connect if not already connected
        if self.service.read().await.is_none() {
            *self.service.write().await =
                Some(Arc::new(self.connector.connect(&self.sources[index]).await?));
        }

        // Run
        let res = run(self.service.read().await.clone().unwrap()).await;

        if let Err(ref e) = res {
            // Trash handler only if that's a next error and if we didn't already move to the next
            // provider (e.g. in another concurrent task).
            if e.is_next() && current == self.current.load(Ordering::Relaxed) {
                *self.service.write().await = None;
            }
        }

        res
    }

    /**
     * Run the provided async function against an established service connection.
     *
     * The connection to the service will be established at this point if not already established.
     */
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
                        // TODO better error handling
                        eprintln!("Service {}/{} failed: {}", current % n_svc, n_svc, e);

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
