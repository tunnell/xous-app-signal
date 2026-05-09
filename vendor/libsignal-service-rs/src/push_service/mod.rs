use std::{sync::Arc, sync::LazyLock, time::Duration};

use crate::{
    configuration::{Endpoint, ServiceCredentials, SignalServers},
    prelude::ServiceConfiguration,
    transport::{self, BasicAuth, HeaderMap, HttpClient, RequestBuilder},
    utils::serde_device_id_vec,
    websocket::{SignalWebSocket, WebSocketType},
};

use http::Method;
use libsignal_core::DeviceId;
use protobuf::ProtobufResponseExt;
use serde::{Deserialize, Serialize};
use tracing::{debug_span, Instrument};

/// Interval between application-level keepalives on the auth WS.
///
/// Signal's server idle-closes after ~55s. Upstream uses 55s
/// here, which leaves no margin: any timer skew or scheduler
/// jitter on the rv32 target lets the server close the socket
/// before our keepalive arrives, surfacing as
/// `"WebSocket closing while waiting for a response"` on the
/// next send. Cut to 30s for headroom while we diagnose whether
/// `futures_timer::Delay` is firing reliably on Xous.
pub const KEEPALIVE_TIMEOUT_SECONDS: Duration = Duration::from_secs(30);
pub static DEFAULT_DEVICE_ID: LazyLock<libsignal_core::DeviceId> =
    LazyLock::new(|| libsignal_core::DeviceId::try_from(1).unwrap());

mod account;
mod cdn;
mod error;
pub mod linking;
pub(crate) mod response;

pub use account::*;
pub use cdn::*;
pub use error::*;
pub(crate) use response::{HttpResponseExt, SignalServiceResponse};

#[derive(Debug, Serialize, Deserialize)]
pub struct ProofRequired {
    pub token: String,
    pub options: Vec<String>,
}

#[derive(derive_more::Debug, Clone, Serialize, Deserialize)]
pub struct HttpAuth {
    pub username: String,
    #[debug(ignore)]
    pub password: String,
}

#[derive(Debug, Clone)]
pub enum HttpAuthOverride {
    NoOverride,
    Unidentified,
    Identified(HttpAuth),
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub enum AvatarWrite<C> {
    NewAvatar(C),
    RetainAvatar,
    NoAvatar,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MismatchedDevices {
    #[serde(with = "serde_device_id_vec")]
    pub missing_devices: Vec<DeviceId>,
    #[serde(with = "serde_device_id_vec")]
    pub extra_devices: Vec<DeviceId>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StaleDevices {
    #[serde(with = "serde_device_id_vec")]
    pub stale_devices: Vec<DeviceId>,
}

#[derive(Clone)]
pub struct PushService {
    pub(crate) servers: SignalServers,
    cfg: ServiceConfiguration,
    credentials: Option<HttpAuth>,
    // Stage 6.1: was `client: reqwest::Client`. Now an Arc'd HttpClient
    // pulled from the per-thread `transport::set_http_client` registration
    // (typically a UreqHttpClient set up by xous-net-bridge). The pinned
    // CA + connect/total timeouts + user-agent that used to go through
    // reqwest::ClientBuilder are now responsibilities of the HttpClient
    // implementation.
    client: Arc<dyn HttpClient + Send + Sync>,
}

impl PushService {
    pub fn new(
        env: SignalServers,
        credentials: Option<ServiceCredentials>,
        // Stage 6.1: user_agent is no longer used here — it's a property
        // of the HttpClient implementation, set when the implementation
        // is constructed (in xous-net-bridge::http::UreqHttpClient::new
        // or similar).
        _user_agent: impl AsRef<str>,
    ) -> Self {
        let cfg: ServiceConfiguration = env.into();
        Self {
            servers: env,
            cfg,
            credentials: credentials.and_then(|c| c.authorization()),
            client: transport::get_http_client(),
        }
    }

    #[tracing::instrument(skip(self), fields(endpoint = %endpoint))]
    pub fn request(
        &self,
        method: Method,
        endpoint: Endpoint,
        auth_override: HttpAuthOverride,
    ) -> Result<RequestBuilder, ServiceError> {
        let url = endpoint.into_url(&self.cfg)?;
        let mut builder = RequestBuilder::new(self.client.clone(), method, url);

        builder = match auth_override {
            HttpAuthOverride::NoOverride => {
                if let Some(HttpAuth { username, password }) =
                    self.credentials.as_ref()
                {
                    builder.basic_auth(username.clone(), Some(password.clone()))
                } else {
                    builder
                }
            },
            HttpAuthOverride::Identified(HttpAuth { username, password }) => {
                builder.basic_auth(username, Some(password))
            },
            HttpAuthOverride::Unidentified => builder,
        };

        Ok(builder)
    }

    /// Open an authenticated WebSocket. Returns `(SignalWebSocket, task)`
    /// where `task` is a future the caller must drive (typically by
    /// spawning on the local executor — for our Xous integration that's
    /// `presage::runtime::spawn_detached(task)`). Stage 6.1: was
    /// `tokio::task::spawn(task)`-ing internally.
    pub async fn ws<C: WebSocketType>(
        &mut self,
        path: &str,
        keepalive_path: &str,
        additional_headers: &[(&'static str, &str)],
        credentials: Option<ServiceCredentials>,
    ) -> Result<
        (
            SignalWebSocket<C>,
            std::pin::Pin<Box<dyn std::future::Future<Output = ()>>>,
        ),
        ServiceError,
    > {
        let span = debug_span!("websocket");

        let mut url = Endpoint::service(path).into_url(&self.cfg)?;
        url.set_scheme("wss").expect("valid https base url");

        let mut headers = HeaderMap::new();
        for (key, value) in additional_headers {
            if let (Ok(name), Ok(val)) = (
                http::HeaderName::try_from(*key),
                http::HeaderValue::try_from(*value),
            ) {
                headers.insert(name, val);
            }
        }

        let auth = credentials.map(|c| BasicAuth {
            username: c.login(),
            password: c.password.unwrap_or_default(),
        });

        let channels = self
            .client
            .connect_websocket(url, headers, auth)
            .await
            .map_err(ServiceError::HttpTransport)?;

        let unidentified_push_service = PushService {
            servers: self.servers,
            cfg: self.cfg.clone(),
            credentials: None,
            client: self.client.clone(),
        };
        let (ws, task) = SignalWebSocket::new(
            channels,
            keepalive_path.to_owned(),
            unidentified_push_service,
        );
        let task = task.instrument(span);
        Ok((ws, Box::pin(task)))
    }

    pub(crate) async fn get_group(
        &mut self,
        credentials: HttpAuth,
    ) -> Result<crate::proto::Group, ServiceError> {
        self.request(
            Method::GET,
            Endpoint::storage("/v1/groups/"),
            HttpAuthOverride::Identified(credentials),
        )?
        .send()
        .await?
        .service_error_for_status()
        .await?
        .protobuf()
        .await
    }
}

pub(crate) mod protobuf {
    use async_trait::async_trait;
    use http::header;
    use prost::{EncodeError, Message};

    use super::ServiceError;
    use crate::transport::{HttpResponse, RequestBuilder};

    pub(crate) trait ProtobufRequestBuilderExt
    where
        Self: Sized,
    {
        /// Set the request payload encoded as protobuf.
        /// Sets the `Content-Type` header to `application/x-protobuf`
        #[allow(dead_code)]
        fn protobuf<T: Message + Default>(
            self,
            value: T,
        ) -> Result<Self, EncodeError>;
    }

    #[async_trait(?Send)]
    pub(crate) trait ProtobufResponseExt {
        /// Get the response body decoded from Protobuf
        async fn protobuf<T>(self) -> Result<T, ServiceError>
        where
            T: prost::Message + Default;
    }

    impl ProtobufRequestBuilderExt for RequestBuilder {
        fn protobuf<T: Message + Default>(
            self,
            value: T,
        ) -> Result<Self, EncodeError> {
            let mut buf = Vec::new();
            value.encode(&mut buf)?;
            let this =
                self.header(header::CONTENT_TYPE, "application/x-protobuf");
            Ok(this.body(buf))
        }
    }

    #[async_trait(?Send)]
    impl ProtobufResponseExt for HttpResponse {
        async fn protobuf<T>(self) -> Result<T, ServiceError>
        where
            T: Message + Default,
        {
            let body = self.bytes().await?;
            let decoded = T::decode(body.as_slice())?;
            Ok(decoded)
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::configuration::SignalServers;
    use bytes::{Buf, Bytes};

    #[test]
    fn create_clients() {
        let environments = &[SignalServers::Staging, SignalServers::Production];

        for env in environments {
            let _ =
                super::PushService::new(*env, None, "libsignal-service test");
        }
    }

    #[test]
    fn serde_json_from_empty_reader() {
        // This fails, so we have handle empty response body separately in HyperPushService::json()
        let bytes: Bytes = "".into();
        assert!(
            serde_json::from_reader::<bytes::buf::Reader<Bytes>, String>(
                bytes.reader()
            )
            .is_err()
        );
    }

    #[test]
    fn serde_json_form_empty_vec() {
        // If we're trying to send and empty payload, serde_json must be able to make a Vec out of it
        assert!(serde_json::to_vec(b"").is_ok());
    }
}
