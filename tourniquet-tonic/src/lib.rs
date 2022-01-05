//! [Tourniquet](https://docs.rs/tourniquet) integration with the [celery](https://docs.rs/celery)
//! library.
//!
//! # Example
//!
//! ```rust,no_run
//! # use tourniquet::RoundRobin;
//! # use tourniquet_tonic::TonicConnector;
//! #
//! # mod grpc { include!("../gen/helloworld.rs"); }
//! #
//! # #[tokio::main]
//! # async fn main() -> Result<(), Box<dyn std::error::Error>> {
//! let rr = RoundRobin::new(
//!     vec!["https://api01", "https://api02"],
//!     TonicConnector::default(),
//! );
//!
//! rr.run(|channel| async move {
//!     grpc::greeting_client::GreetingClient::new(channel.as_ref().clone())
//!         .hello(grpc::Message::default())
//!         .await?;
//!     Ok(())
//! }).await?;
//! # Ok(())
//! # }
//! ```

use std::error::Error as StdError;
use std::fmt::{Debug, Display, Error as FmtError, Formatter};
use std::future::Future;

use async_trait::async_trait;
use tonic::{
    transport::{Channel, Endpoint, Uri},
    Status,
};
use tourniquet::{Connector, Next, RoundRobin};

/// Wrapper for Tonic's errors (`Status` and transport `Error`)
#[derive(Debug)]
pub enum Error {
    Status(Status),
    Transport(tonic::transport::Error),
}

impl Next for Error {
    fn is_next(&self) -> bool {
        use tonic::Code::*;

        match self {
            Self::Transport(_) => true,
            Self::Status(s) => matches!(
                s.code(),
                Cancelled | Unknown | DeadlineExceeded | Aborted | Internal | Unavailable,
            ),
        }
    }
}

impl From<Status> for Error {
    fn from(s: Status) -> Self {
        Self::Status(s)
    }
}

impl From<tonic::transport::Error> for Error {
    fn from(e: tonic::transport::Error) -> Self {
        Self::Transport(e)
    }
}

impl Display for Error {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result<(), FmtError> {
        match self {
            Self::Status(s) => Display::fmt(s, f),
            Self::Transport(e) => Display::fmt(e, f),
        }
    }
}

impl StdError for Error {
    fn source(&self) -> Option<&(dyn StdError + 'static)> {
        Some(match self {
            Self::Status(s) => s,
            Self::Transport(e) => e,
        })
    }
}

/// Ready-to-use connector for Tonic.
///
/// Please refer to
/// [Tonic's documentation](https://docs.rs/tonic/0.5/tonic/transport/channel/struct.Channel.html)
/// for more information.
#[derive(Default)]
pub struct TonicConnector {
    conf: Option<fn(Endpoint) -> Endpoint>,
}

impl TonicConnector {
    pub fn with_conf(conf: fn(Endpoint) -> Endpoint) -> Self {
        Self { conf: Some(conf) }
    }
}

#[async_trait]
impl Connector<Uri, Channel, Error> for TonicConnector {
    #[cfg_attr(feature = "trace", tracing::instrument(skip(self), err))]
    async fn connect(&self, uri: &Uri) -> Result<Channel, Error> {
        let mut ep = Channel::builder(uri.to_owned());
        if let Some(conf) = self.conf {
            ep = conf(ep)
        }

        Ok(ep.connect().await?)
    }
}

#[async_trait]
impl Connector<&'static str, Channel, Error> for TonicConnector {
    #[cfg_attr(feature = "trace", tracing::instrument(skip(self), err))]
    async fn connect(&self, uri: &&'static str) -> Result<Channel, Error> {
        let mut ep = Channel::from_static(uri);
        if let Some(conf) = self.conf {
            ep = conf(ep)
        }

        Ok(ep.connect().await?)
    }
}

#[async_trait]
pub trait RoundRobinExt {
    async fn chan<R, Fut, T, E>(&self, run: R) -> Result<T, Error>
    where
        T: Send + Sync,
        R: Fn(Channel) -> Fut + Send + Sync,
        Fut: Future<Output = Result<T, E>> + Send + Sync,
        Error: From<E>;
}

#[async_trait]
impl<SvcSrc, Conn> RoundRobinExt for RoundRobin<SvcSrc, Channel, Error, Conn>
where
    SvcSrc: Debug + Send + Sync,
    Conn: Connector<SvcSrc, Channel, Error> + Send + Sync,
{
    #[cfg_attr(feature = "trace", tracing::instrument(skip(self, run), err))]
    async fn chan<R, Fut, T, E>(&self, run: R) -> Result<T, Error>
    where
        T: Send + Sync,
        R: Fn(Channel) -> Fut + Send + Sync,
        Fut: Future<Output = Result<T, E>> + Send + Sync,
        Error: From<E>,
    {
        self.run(|chan| {
            let fut = run(chan.as_ref().clone());
            async move { Ok(fut.await?) }
        })
        .await
    }
}

/// Shorthand type for a Tonic round-robin
pub type TonicRoundRobin<T> = tourniquet::RoundRobin<T, Channel, Error, TonicConnector>;
