use crate::MediaPipeline;
use crate::Settings;
use anyhow::Error;
use azure_iot_rs::message::IotHubMessage;
use azure_iot_rs::module::{IotHubModuleClient, IotHubModuleEvent};
use serde_json::Value;

use tokio::task;

#[derive(Clone)]
pub struct IotModule {
    pipeline: MediaPipeline,
    settings: Option<Settings>,
}

impl IotModule {
    fn new() -> Self {
        IotModule {
            settings: None,
            pipeline: MediaPipeline::new(),
        }
    }

    fn module_callback(&mut self, event: IotHubModuleEvent) {
        let _result = match event {
            IotHubModuleEvent::Message(message) => self.handle_message(message),
            IotHubModuleEvent::Twin(settings) => self.handle_twin(settings),
        };
    }

    fn handle_twin(&mut self, settings: Value) -> Result<(), &'static str> {
        let desired = Settings::from_json(settings);
        self.pipeline.update(desired);
        Ok(())
    }

    fn handle_message(&self, message: IotHubMessage) -> Result<(), &'static str> {
        info!("Received hub message {}", message.content_type());
        return Ok(());
    }

    pub async fn start() -> Result<(), Error> {
        let mut context = IotModule::new();
        let mut client = IotHubModuleClient::new(move |event| {
            context.module_callback(event);
        });
        info!("Initialized IOT hub module...");
        task::spawn_blocking(move || {
            client.do_work();
        })
        .await?;
        Ok(())
    }
}
