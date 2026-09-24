//! Couch package adapter: one NVR connection with camera children.

use std::{
    net::IpAddr,
    sync::{Arc, LazyLock},
    time::Duration,
};

use couch_sdk::{
    couch_model::{commands::Function, ChildComponent, DeviceKind, KeyPhase, PluginChildKind},
    CameraStream, CameraView, Capability, Child, ChildPage, ClientSettings, Credential,
    DeviceClient, Error as SdkError, PairFlow, PairInput, PairStep, Reason, Result as SdkResult,
    Status,
};
use serde::{Deserialize, Serialize};
use zeroize::Zeroize;

use crate::{media, settings::Settings, ApiKey, Client, Error, Quality, SnapshotChannel};

const VIEW_SECONDS: u8 = 60;

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProtectSettings {
    pub address: String,
    pub api_key: String,
}

impl std::fmt::Debug for ProtectSettings {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProtectSettings")
            .field("address", &self.address)
            .field("api_key", &"[redacted]")
            .finish()
    }
}

impl Drop for ProtectSettings {
    fn drop(&mut self) {
        self.api_key.zeroize();
    }
}

impl ProtectSettings {
    fn address(&self) -> SdkResult<IpAddr> {
        self.address.parse().map_err(|_| {
            SdkError::Invalid.because(Reason::InvalidSetting {
                field: "address".into(),
                text: "Enter the NVR's IP address".into(),
            })
        })
    }

    fn validate_key(&self) -> SdkResult<()> {
        ApiKey::new(self.api_key.clone()).map(|_| ()).map_err(|_| {
            SdkError::Invalid.because(Reason::InvalidSetting {
                field: "api_key".into(),
                text: "Enter the API key from UniFi Protect".into(),
            })
        })
    }
}

impl ClientSettings for ProtectSettings {
    const FILE_PREFIX: &'static str = "unifi-protect";

    fn validate(&self) -> SdkResult<()> {
        self.address()?;
        self.validate_key()
    }
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ProtectCredential {
    api_certificate_sha256: String,
    media_certificate_sha256: String,
}

impl ProtectCredential {
    fn parse(value: &Credential) -> SdkResult<Self> {
        let value = serde_json::from_value(serde_json::Value::Object(value.get().clone()))
            .map_err(|_| SdkError::Unpaired)?;
        Ok(value)
    }

    fn store(self) -> SdkResult<Credential> {
        Credential::new(serde_json::to_value(self).map_err(|_| SdkError::Protocol)?)
    }
}

fn pinned(settings: &ProtectSettings, credential: ProtectCredential) -> SdkResult<Settings> {
    Settings::pinned_local(
        settings.address()?,
        settings.api_key.clone(),
        credential.api_certificate_sha256,
        credential.media_certificate_sha256,
    )
    .map_err(refused)
}

fn reason(text: &str) -> Reason {
    Reason::Message { text: text.into() }
}

fn refused(error: Error) -> SdkError {
    match error {
        Error::Configuration => SdkError::Invalid,
        Error::Authentication | Error::Permission => SdkError::Unpaired.because(reason(
            "Protect rejected the API key. Enter a new key and connect again",
        )),
        Error::NotFound => {
            SdkError::Rejected.because(reason("That camera is no longer on this NVR"))
        }
        Error::Offline => SdkError::Transport,
        Error::RateLimited => {
            SdkError::Rejected.because(reason("Protect is busy. Try again shortly"))
        }
        Error::StreamNotEnabled => SdkError::Rejected.because(reason(
            "Enable the camera's low-quality RTSPS stream in UniFi Protect",
        )),
        Error::Expired => SdkError::Timeout,
        Error::Response | Error::Status(_) => SdkError::Protocol,
        Error::MediaTls => SdkError::Unpaired.because(reason(
            "The NVR media certificate changed. Connect the NVR again",
        )),
        Error::Transport
        | Error::MediaResolve
        | Error::MediaConnect
        | Error::MediaWrite
        | Error::MediaRead
        | Error::MediaOptions
        | Error::MediaDescribe
        | Error::MediaSetup
        | Error::MediaPlay => SdkError::Transport,
    }
}

pub static KINDS: LazyLock<Vec<PluginChildKind>> = LazyLock::new(|| {
    vec![PluginChildKind {
        kind: "camera".into(),
        label: "UniFi Protect camera".into(),
        device_kind: DeviceKind::Camera,
        component: ChildComponent::Camera,
        capabilities: vec![],
        actions: vec![],
    }]
});

struct Enrollment {
    settings: Option<ProtectSettings>,
}

impl PairFlow for Enrollment {
    fn step(&mut self, _input: Option<PairInput>) -> SdkResult<PairStep> {
        let settings = self.settings.take().ok_or(SdkError::Invalid)?;
        let address = settings.address()?;
        let host = match address {
            IpAddr::V4(value) => value.to_string(),
            IpAddr::V6(value) => format!("[{value}]"),
        };
        // Observe without sending the API key, then use only the exact leaf
        // pins. A certificate change is never accepted silently on reconnect.
        let api_pin = crate::observe_certificate_sha256(
            &format!("https://{host}"),
            None,
            Duration::from_secs(3),
        )
        .map_err(refused)?;
        let media_pin = crate::observe_certificate_sha256(
            &format!("rtsps://{host}:7441"),
            Some("unifi.local"),
            Duration::from_secs(3),
        )
        .map_err(refused)?;
        let credential = ProtectCredential {
            api_certificate_sha256: api_pin,
            media_certificate_sha256: media_pin,
        };
        let client = pinned(
            &settings,
            ProtectCredential {
                api_certificate_sha256: credential.api_certificate_sha256.clone(),
                media_certificate_sha256: credential.media_certificate_sha256.clone(),
            },
        )?
        .client()
        .map_err(refused)?;
        client.cameras().map_err(refused)?;
        Ok(PairStep::done(
            credential.store()?,
            format!("Connected to UniFi Protect at {}", settings.address),
        ))
    }
}

struct ProtectMedia(media::Session);

impl CameraStream for ProtectMedia {
    fn next_h264(&mut self) -> SdkResult<Vec<u8>> {
        self.0.next_h264().map_err(refused)
    }
}

pub struct ProtectConnection {
    client: Client,
    settings: Settings,
}

impl DeviceClient for ProtectConnection {
    type Settings = ProtectSettings;

    const KIND: &'static str = "unifi-protect";
    const LABEL: &'static str = "UniFi Protect";

    fn capabilities() -> &'static [Capability] {
        &[]
    }

    fn child_kinds() -> &'static [PluginChildKind] {
        &KINDS
    }

    fn connect(settings: &ProtectSettings) -> SdkResult<Self> {
        Self::connect_with(settings, None)
    }

    fn connect_with(
        settings: &ProtectSettings,
        credential: Option<&Credential>,
    ) -> SdkResult<Self> {
        settings.validate()?;
        let credential = credential
            .ok_or(SdkError::Unpaired)
            .and_then(ProtectCredential::parse)?;
        let settings = pinned(settings, credential)?;
        let client = settings.client().map_err(refused)?;
        Ok(Self { client, settings })
    }

    fn pair_start(
        settings: &ProtectSettings,
        _existing: Option<&Credential>,
    ) -> SdkResult<Box<dyn PairFlow>> {
        settings.validate()?;
        Ok(Box::new(Enrollment {
            settings: Some(settings.clone()),
        }))
    }

    fn execute(&mut self, _function: &Function) -> SdkResult<()> {
        Err(SdkError::Unsupported)
    }

    fn children(&mut self, cursor: Option<&str>) -> SdkResult<ChildPage> {
        let mut cameras = self.client.cameras().map_err(refused)?;
        cameras.sort_by(|a, b| {
            a.name
                .as_deref()
                .unwrap_or("")
                .cmp(b.name.as_deref().unwrap_or(""))
                .then(a.id.cmp(&b.id))
        });
        let children = cameras
            .into_iter()
            .map(|camera| {
                let fallback = if camera.model.is_empty() {
                    "UniFi camera"
                } else {
                    camera.model.as_str()
                };
                Child::new(
                    camera.id,
                    "camera",
                    shown(camera.name.as_deref().unwrap_or(fallback)),
                )
            })
            .collect::<Vec<_>>();
        ChildPage::fill(children, cursor)
    }

    fn child_command(
        &mut self,
        _resource: &str,
        _function: &Function,
        _phase: KeyPhase,
    ) -> SdkResult<Option<Status>> {
        Err(SdkError::Unsupported)
    }

    fn child_status(&mut self, resource: &str) -> SdkResult<Status> {
        if self
            .client
            .cameras()
            .map_err(refused)?
            .iter()
            .any(|camera| camera.id == resource)
        {
            Ok(Status::default())
        } else {
            Err(SdkError::Rejected)
        }
    }

    fn camera_snapshot(&mut self, resource: &str) -> SdkResult<Vec<u8>> {
        self.client
            .snapshot(resource, SnapshotChannel::Main)
            .map_err(refused)
    }

    fn camera_open(&mut self, resource: &str) -> SdkResult<CameraView> {
        let view = self
            .client
            .live_view(
                resource,
                Quality::Low,
                Duration::from_secs(VIEW_SECONDS.into()),
            )
            .map_err(refused)?;
        let cancellation = Arc::new(media::Cancellation::default());
        let session =
            media::Session::connect_cancellable(&view, &self.settings, cancellation.clone())
                .map_err(refused)?;
        CameraView::new(
            ProtectMedia(session),
            move || cancellation.cancel(),
            VIEW_SECONDS,
        )
    }
}

fn shown(input: &str) -> String {
    let mut text: String = input.chars().filter(|c| !c.is_control()).collect();
    if text.is_empty() {
        return "UniFi camera".into();
    }
    if text.len() > couch_sdk::MAX_CHILD_LABEL {
        let mut end = couch_sdk::MAX_CHILD_LABEL;
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        text.truncate(end);
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn settings_are_only_an_ip_and_a_redacted_key() {
        let settings = ProtectSettings {
            address: "192.0.2.8".into(),
            api_key: "fixture-secret-key".into(),
        };
        assert!(settings.validate().is_ok());
        assert!(!format!("{settings:?}").contains("fixture-secret-key"));
        let value = serde_json::to_value(&settings).unwrap();
        assert_eq!(
            value,
            json!({"address":"192.0.2.8","api_key":"fixture-secret-key"})
        );
    }

    #[test]
    fn camera_names_are_printable_and_bounded() {
        assert_eq!(shown("Front\nyard"), "Frontyard");
        assert_eq!(shown(""), "UniFi camera");
        assert!(shown(&"é".repeat(100)).len() <= couch_sdk::MAX_CHILD_LABEL);
    }
}
