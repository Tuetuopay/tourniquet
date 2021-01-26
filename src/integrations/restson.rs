//! Integration with the [restson](https://docs.rs/restson) library.

use std::fmt::Debug;

use async_trait::async_trait;
use crate::{Connector, Next, RoundRobin};
use restson_lib::{
    Error::{self, *},
    Response, RestClient, RestPath,
};
use serde::{de::DeserializeOwned, ser::Serialize};

impl Next for Error {
    fn is_next(&self) -> bool {
        match self {
            HttpClientError
                | DeserializeParseError(_, _)
                | RequestError
                | IoError(_)
                | TimeoutError
                | HyperError(_)
                | HttpError(500..=599, _) => true,
            _ => false,
        }
    }
}

pub struct RestsonConnector;

#[async_trait]
impl Connector<String, RestClient, Error> for RestsonConnector {
    #[cfg_attr(feature = "trace", tracing::instrument(skip(self), err))]
    async fn connect(&self, src: &String) -> Result<RestClient, Error> {
        RestClient::new(src.as_ref())
    }
}

impl<SvcSrc, Conn> RoundRobin<SvcSrc, RestClient, Error, Conn>
where
    SvcSrc: Debug,
    Conn: Connector<SvcSrc, RestClient, Error>,
{
    #[cfg_attr(feature = "trace", tracing::instrument(skip(self, params), err))]
    pub async fn get<U, T>(&self, params: U) -> Result<Response<T>, Error>
    where
        T: DeserializeOwned + RestPath<U>,
        U: Clone,
    {
        let params = &params;
        self.run(move |client| async move { client.get(params.clone()).await }).await
    }

    #[cfg_attr(feature = "trace", tracing::instrument(skip(self, params, data), err))]
    pub async fn put<U, T>(&self, params: U, data: &T) -> Result<Response<()>, Error>
    where
        T: Serialize + RestPath<U>,
        U: Clone,
    {
        let params = &params;
        self.run(move |client| async move { client.put(params.clone(), data).await }).await
    }

    #[cfg_attr(feature = "trace", tracing::instrument(skip(self, params), err))]
    pub async fn delete<U, T>(&self, params: U) -> Result<Response<()>, Error>
    where
        T: RestPath<U>,
        U: Clone,
    {
        let params = &params;
        self.run(move |client| async move { client.delete::<_, T>(params.clone()).await }).await
    }
}
