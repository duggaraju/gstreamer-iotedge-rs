use serde_json::Value;

#[derive(Debug, Clone)]
pub struct Settings {
    pub pipeline: Option<String>,
    pub rtsp_pipeline: Option<String>,
    pub webrtc_pipeline: Option<String>,
    pub http_server: bool,
}

impl Settings {
    pub fn from_json(settings: Value) -> Self {
        let desired = settings["desired"]
            .as_object()
            .expect("Expected desired settings!");
        Settings {
            pipeline: desired
                .get("pipeline")
                .and_then(|val| val.as_str())
                .map(String::from),
            rtsp_pipeline: desired
                .get("rtsp_pipeline")
                .and_then(|val| val.as_str())
                .map(String::from),
            webrtc_pipeline: desired
                .get("webrtc_pipeline")
                .and_then(|val| val.as_str())
                .map(String::from),
            http_server: desired
                .get("http_server")
                .and_then(|val| val.as_bool())
                .unwrap_or(true),
        }
    }

    pub fn from_env() -> Self {
        Settings {
            pipeline: std::env::var("pipeline").ok(),
            rtsp_pipeline: std::env::var("rtsp_pipeline").ok(),
            webrtc_pipeline: std::env::var("webrtc_pipeline").ok(),
            http_server: true,
        }
    }
}
