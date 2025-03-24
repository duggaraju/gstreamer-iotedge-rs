use crate::MediaPipeline;
use crate::Settings;
use anyhow::Error;
use azure_iot_rs::message::IotHubMessage;
use azure_iot_rs::message::MessageBody;
use azure_iot_rs::module::{IotHubModuleClient, IotHubModuleEvent};
use log::info;
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

    fn handle_twin(&mut self, settings: Value) -> anyhow::Result<()> {
        let desired = Settings::from_json_value(settings);
        self.pipeline.update(desired)?;
        Ok(())
    }

    fn handle_message(&mut self, message: IotHubMessage) -> anyhow::Result<()> {
        let body: MessageBody<'_> = message.body();
        info!("Received hub message {:?}", body);
        if let MessageBody::Text(s) = body {
            self.settings = Settings::from_json(s).ok();
        }
        Ok(())
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
