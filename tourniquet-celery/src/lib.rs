//! [Tourniquet](https://docs.rs/tourniquet) integration with the [celery](https://docs.rs/celery)
//! library.
//!
//! # Example
//!
//! ```rust,no_run
//! # use celery::{task, task::TaskResult};
//! # use tourniquet::RoundRobin;
//! # use tourniquet_celery::{CeleryConnector, CelerySource, RoundRobinExt};
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
//!     vec![CelerySource::from("amqp://rabbit01:5672/".to_owned()), CelerySource::from("amqp://rabbit02:5672".to_owned())],
//!     CeleryConnector { name: "rr", routes: &[("*", "my_route")], ..Default::default() },
//! );
//!
//! # let work = "foo".to_owned();
//! rr.send_task(|| do_work(work.clone())).await.expect("Failed to send task");
//! # Ok(())
//! # }
//! ```

use std::borrow::Cow;
use std::error::Error;
use std::fmt::{Debug, Display, Error as FmtError, Formatter};

use async_trait::async_trait;
use celery::{
    error::BackendError::*,
    error::BrokerError::BadRoutingPattern,
    error::CeleryError::{self, *},
    task::{AsyncResult, Signature, Task},
    Celery, CeleryBuilder,
};
use url::Url;

use tourniquet::{Connector, Next, RoundRobin};
#[cfg(feature = "trace")]
use tracing::{
    field::{display, Empty},
    instrument, Span,
};

/// Wrapper for [`CeleryError`](https://docs.rs/celery-rs/latest/celery/error/struct.CeleryError.html)
/// that implements [`Next`](https://docs.rs/tourniquet/latest/tourniquet/trait.Next.html).
pub struct RRCeleryError(CeleryError);

impl Next for RRCeleryError {
    fn is_next(&self) -> bool {
        match self.0 {
            BrokerError(BadRoutingPattern(_)) => false,
            BrokerError(_) | IoError(_) | ProtocolError(_) => true,
            BackendError(NotConfigured | Timeout | Redis(_)) => true,
            BackendError(Serialization(_) | Pool(_) | PoolCreationError(_) | TaskFailed(_)) => {
                false
            }
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

/// Wrapper for String
#[derive(Clone)]
pub struct CelerySource(String);

impl From<String> for CelerySource {
    fn from(src: String) -> Self {
        Self(src)
    }
}

fn safe_source(url: &String) -> Cow<'_, String> {
    // URL that is safe to log (password stripped)
    let Some(mut url_safe): Option<Url> = url.parse().ok() else { return Cow::Borrowed(url) };
    if url_safe.password().is_some() {
        let _ = url_safe.set_password(Some("********"));
    }
    Cow::Owned(url_safe.to_string())
}

impl Display for CelerySource {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result<(), FmtError> {
        Display::fmt(&safe_source(&self.0), f)
    }
}

impl Debug for CelerySource {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result<(), FmtError> {
        Debug::fmt(&safe_source(&self.0), f)
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
impl<'a> Connector<CelerySource, Celery, RRCeleryError> for CeleryConnector<'a> {
    #[cfg_attr(feature = "trace", instrument(skip(self), err))]
    async fn connect(&self, src: &CelerySource) -> Result<Celery, RRCeleryError> {
        let mut builder = CeleryBuilder::new(self.name, src.0.as_ref());

        if let Some(queue) = self.default_queue {
            builder = builder.default_queue(queue);
        }
        for (pattern, queue) in self.routes {
            builder = builder.task_route(pattern, queue);
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
impl<SvcSrc, Conn> RoundRobinExt for RoundRobin<SvcSrc, Celery, RRCeleryError, Conn>
where
    SvcSrc: Debug + Send + Sync + Display + Clone,
    Conn: Connector<SvcSrc, Celery, RRCeleryError> + Send + Sync,
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
        let task =
            self.run(|celery| async move { Ok(celery.send_task(task_gen()).await?) }).await?;

        #[cfg(feature = "trace")]
        Span::current().record("task_id", display(&task.task_id));

        Ok(task)
    }
}

/// Shorthand type for a basic RoundRobin type using Celery
pub type CeleryRoundRobin =
    RoundRobin<CelerySource, Celery, RRCeleryError, CeleryConnector<'static>>;

#[cfg(test)]
mod tests {
    use super::CelerySource;
    
    #[test]
    fn test_display_debug_celery_source_strips_password() {
        let source = CelerySource::from("amqp://mylogin:mypassword@rabbitmq.myserver.com/product".to_owned());

        assert_eq!(format!("{source}"),"amqp://mylogin:********@rabbitmq.myserver.com/product");
        assert_eq!(format!("{source:?}"),"\"amqp://mylogin:********@rabbitmq.myserver.com/product\"");
    }
}
