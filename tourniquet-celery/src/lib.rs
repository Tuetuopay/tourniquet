//! [Tourniquet](https://docs.rs/tourniquet) integration with the [celery](https://docs.rs/celery)
//! library.
//!
//! # Example
//!
//! ```rust,no_run
//! # use celery::task::TaskResult;
//! # use tourniquet::RoundRobin;
//! # use tourniquet_celery::{CeleryConnector, RoundRobinExt};
//! #
//! #[celery::task]
//! async fn do_work(work: String) -> TaskResult<()> {
//!     // Some work
//! # println!("{}", work);
//!     Ok(())
//! }
//!
//! # #[tokio::main]
//! # async fn main() -> Result<(), Box<dyn std::error::Error>> {
//! let rr = RoundRobin::new(
//!     vec!["amqp://rabbit01:5672/".to_owned(), "amqp://rabbit02:5672".to_owned()],
//!     CeleryConnector { name: "rr", routes: &[("*", "my_route")], ..Default::default() },
//! );
//!
//! # let work = "foo".to_owned();
//! rr.send_task(|| do_work::new(work.clone())).await.expect("Failed to send task");
//! # Ok(())
//! # }
//! ```

use std::error::Error;
use std::fmt::{Debug, Display, Error as FmtError, Formatter};

use async_trait::async_trait;
use celery::{
    broker::{AMQPBroker, Broker},
    error::BrokerError::BadRoutingPattern,
    error::CeleryError::{self, *},
    task::{AsyncResult, Signature, Task},
    Celery,
};
use tourniquet::{Connector, Next, RoundRobin};
#[cfg(feature = "trace")]
use tracing::{
    field::{display, Empty},
    instrument, Span,
};

/// Wrapper for [`CeleryError`](https://docs.rs/celery/latest/celery/error/struct.CeleryError.html)
/// that implements [`Next`](https://docs.rs/tourniquet/latest/tourniquet/trait.Next.html).
pub struct RRCeleryError(CeleryError);

impl Next for RRCeleryError {
    fn is_next(&self) -> bool {
        match self.0 {
            BrokerError(BadRoutingPattern(_)) => false,
            BrokerError(_) | IoError(_) | ProtocolError(_) => true,
            NoQueueToConsume
            | ForcedShutdown
            | TaskRegistrationError(_)
            | UnregisteredTaskError(_) => false,
        }
    }
}

impl From<CeleryError> for RRCeleryError {
    fn from(e: CeleryError) -> Self {
        Self(e)
    }
}

impl From<RRCeleryError> for CeleryError {
    fn from(e: RRCeleryError) -> Self {
        e.0
    }
}

impl Display for RRCeleryError {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result<(), FmtError> {
        Display::fmt(&self.0, f)
    }
}

impl Debug for RRCeleryError {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result<(), FmtError> {
        Debug::fmt(&self.0, f)
    }
}

impl Error for RRCeleryError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Some(&self.0)
    }
}

/// Ready to use connector for Celery.
///
/// Please refer to
/// [celery's documentation](https://docs.rs/celery/^0.3/celery/struct.CeleryBuilder.html)
/// for more information about the fields of this structure.
///
/// Note that this is a basic connector for celery, exposing only the most common options used by
/// celery producers, not consumers.  Please create your own connector should you need finer
/// grained control over the created celery client.
pub struct CeleryConnector<'a> {
    pub name: &'a str,
    pub default_queue: Option<&'a str>,
    pub routes: &'a [(&'a str, &'a str)],
    pub connection_timeout: Option<u32>,
}

impl<'a> Default for CeleryConnector<'a> {
    fn default() -> Self {
        Self { name: "celery", default_queue: None, routes: &[], connection_timeout: None }
    }
}

#[async_trait]
impl<'a> Connector<String, Celery<AMQPBroker>, RRCeleryError> for CeleryConnector<'a> {
    #[cfg_attr(feature = "trace", tracing::instrument(skip(self), err))]
    async fn connect(&self, url: &String) -> Result<Celery<AMQPBroker>, RRCeleryError> {
        let mut builder = Celery::<AMQPBroker>::builder(self.name, url.as_ref());

        if let Some(queue) = self.default_queue {
            builder = builder.default_queue(queue);
        }
        for (pattern, queue) in self.routes {
            builder = builder.task_route(*pattern, *queue);
        }
        if let Some(timeout) = self.connection_timeout {
            builder = builder.broker_connection_timeout(timeout);
        }

        Ok(builder.build().await?)
    }
}

#[async_trait]
pub trait RoundRobinExt {
    async fn send_task<T, F>(&self, task_gen: F) -> Result<AsyncResult, CeleryError>
    where
        T: Task + 'static,
        F: Fn() -> Signature<T> + Send + Sync;
}

#[async_trait]
impl<SvcSrc, B, Conn> RoundRobinExt for RoundRobin<SvcSrc, Celery<B>, RRCeleryError, Conn>
where
    SvcSrc: Debug + Send + Sync,
    B: Broker + 'static,
    Conn: Connector<SvcSrc, Celery<B>, RRCeleryError> + Send + Sync,
{
    /// Send a Celery task.
    ///
    /// The `task_gen` argument returns a signature for each attempt, should each attempt hold a
    /// different value (e.g. trace id, attempt id, timestamp, ...).
    #[cfg_attr(
        feature = "trace",
        instrument(
            fields(task_name = display(Signature::<T>::task_name()), task_id = Empty),
            skip(self, task_gen),
            err,
        ),
    )]
    async fn send_task<T, F>(&self, task_gen: F) -> Result<AsyncResult, CeleryError>
    where
        T: Task + 'static,
        F: Fn() -> Signature<T> + Send + Sync,
    {
        log::debug!("Sending task {}", Signature::<T>::task_name());

        let task_gen = &task_gen;
        let task = self.run(|celery| async move { Ok(celery.send_task(task_gen()).await?) }).await?;

        #[cfg(feature = "trace")]
        Span::current().record("task_id", &display(&task.task_id));

        Ok(task)
    }
}

/// Shorthand type for a basic RoundRobin type using Celery
pub type CeleryRoundRobin =
    RoundRobin<String, Celery<AMQPBroker>, CeleryError, CeleryConnector<'static>>;
