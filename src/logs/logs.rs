use shiplift::Docker;
use shiplift::LogsOptions;
use tokio::sync::broadcast;

use clickhouse_rs::Pool;
use clickhouse_rs::types::{Block, Value};

use rdkafka::producer::{FutureProducer, FutureRecord};
use rdkafka::config::ClientConfig;
use rdkafka::util::Timeout;

use chrono::prelude::*;
use chrono::{DateTime, Utc};
use chrono_tz::Tz;
use futures::StreamExt;
use tracing::error;

use std::sync::Arc;
use std::str;
use std::time::Duration;

#[derive(Debug, Clone)]
pub struct LogMessage {
    pub source: String,
    pub timestamp: DateTime<Utc>,
    pub text: String,
}

pub struct LogFilter {
    pub start_time: DateTime<Utc>,
    pub end_time: DateTime<Utc>,
}

impl LogFilter {
    pub fn matches(&self, message: &LogMessage) -> bool {
        message.timestamp >= self.start_time && message.timestamp <= self.end_time
    }
}


pub async fn get_logs(container_id: &str, filter: LogFilter, tx: broadcast::Sender<LogMessage>) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let docker = Docker::new();

    let container = docker.containers().get(container_id);
    let options = LogsOptions::builder().stdout(true).stderr(true).build();
    let mut logs_stream = container.logs(&options);

    let pool = Pool::new("tcp://clickhouse:8123");

    let duration_in_millis = Duration::from_secs(5).as_millis().to_string();

    let producer: FutureProducer = ClientConfig::new()
        .set("bootstrap.servers", "redpanda:18081")
        .set("message.timeout.ms", &duration_in_millis)
        .create()?;

    while let Some(log_result) = logs_stream.next().await {
        match log_result {
            Ok(log_output) => {
                let log_data = str::from_utf8(&log_output)?;
                let parts: Vec<&str> = log_data.splitn(2, ' ').collect();
                let timestamp = parts[0].parse::<DateTime<Utc>>()?;
                let text = parts[1].to_string();
                
                let message = LogMessage {
                    source: container_id.to_string(),
                    timestamp,
                    text,
                };

                if filter.matches(&message) {
                    let topic = "logs_topic";
                    let payload = format!("{:?}", message);
                    let record = FutureRecord::to(topic).payload(&payload).key("");

                    match producer.send(record, Timeout::Never).await {
                        Ok(_) => {}
                        Err(e) => error!("Error sending message to Kafka: {:?}", e),
                    }
                }

                let mut block = Block::new();

                let timestamp: DateTime<Utc> = message.timestamp;
                let timestamp_seconds = timestamp.timestamp(); // timestamp() returns i64, cast it to u32
                let timezone_offset_seconds = Local::now().offset().fix().local_minus_utc() as u32;

                let row = vec![
                    ("source".to_string(), Value::String(Arc::new(message.source.into_bytes()))),
                    ("timestamp".to_string(), Value::DateTime64(timestamp_seconds, (timezone_offset_seconds, Tz::UTC))),
                    ("text".to_string(), Value::String(Arc::new(message.text.into_bytes()))),
                ];
                
                if let Err(e) = block.push(row) {
                    error!("Error pushing row to block: {}", e);
                }

                let mut client = pool.get_handle().await?;
            
                let ddl = r"
                INSERT INTO logs (source, timestamp, text) VALUES
                ";

                client.insert(ddl, block).await?;
            },
            Err(e) => {
                error!("Error reading logs: {}", e);
            }
        }
    }

    Ok(())
}