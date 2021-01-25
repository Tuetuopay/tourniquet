//! Tourniquet integration with the [celery](https://docs.rs/celery) library.

use std::fmt::Debug;

use crate::{Connector, Next, RoundRobin};
use async_trait::async_trait;
use celery::{
    broker::{AMQPBroker, Broker},
    error::CeleryError::{self, *},
    task::{Signature, Task},
    Celery,
};
#[cfg(feature = "trace")]
use tracing::{
    field::{display, Empty},
    instrument, Span,
};

impl Next for CeleryError {
    fn is_next(&self) -> bool {
        match self {
            UnknownQueue(_) | BrokerError(_) | IoError(_) | ProtocolError(_) => true,
            NoQueueToConsume
            | ForcedShutdown
            | BadRoutingPattern(_)
            | TaskRegistrationError(_)
            | UnregisteredTaskError(_) => false,
        }
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
impl<'a> Connector<String, Celery<AMQPBroker>, CeleryError> for CeleryConnector<'a> {
    #[cfg_attr(feature = "trace", tracing::instrument(skip(self), err))]
    async fn connect(&self, url: &String) -> Result<Celery<AMQPBroker>, CeleryError> {
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

        builder.build().await
    }
}

impl<SvcSrc, B, Conn> RoundRobin<SvcSrc, Celery<B>, CeleryError, Conn>
where
    SvcSrc: Debug,
    B: Broker,
    Conn: Connector<SvcSrc, Celery<B>, CeleryError>,
{
    /// Send a Celery task.
    ///
    /// The `task_gen` argument returns a signature for each attempt, should each attempt hold a
    /// different value (e.g. trace id, attempt id, timestamp, ...).
    ///
    /// # Example
    ///
    /// ```rust,no_run
    /// # use celery::TaskResult;
    /// # use tourniquet::{RoundRobin, conn::CeleryConnector};
    /// #
    /// #[celery::task]
    /// async fn do_work(work: String) -> TaskResult<()> {
    ///     // Some work
    /// # println!("{}", work);
    ///     Ok(())
    /// }
    ///
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// let rr = RoundRobin::new(
    ///     vec!["amqp://rabbit01:5672/".to_owned(), "amqp://rabbit02:5672".to_owned()],
    ///     CeleryConnector { name: "rr", routes: &[("*", "my_route")], ..Default::default() },
    /// );
    ///
    /// # let work = "foo".to_owned();
    /// rr.send_task(|| do_work::new(work.clone())).await.expect("Failed to send task");
    /// # Ok(())
    /// # }
    /// ```
    #[cfg_attr(
        feature = "trace",
        instrument(
            fields(task_name = display(Signature::<T>::task_name()), task_id = Empty),
            skip(self, task_gen),
            err,
        ),
    )]
    pub async fn send_task<T, F>(&self, task_gen: F) -> Result<String, CeleryError>
    where
        T: Task + 'static,
        F: Fn() -> Signature<T>,
    {
        #[cfg(feature = "trace")]
        tracing::info!("Sending task {}", Signature::<T>::task_name());

        let task_gen = &task_gen;
        let task_id = self.run(|celery| async move { celery.send_task(task_gen()).await }).await?;

        #[cfg(feature = "trace")]
        Span::current().record("task_id", &display(&task_id));

        Ok(task_id)
    }
}
