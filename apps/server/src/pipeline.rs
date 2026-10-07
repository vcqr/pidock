//! Event pipeline: gateway publishes wrapped envelopes, ingest consumes them.
//! Kafka is the default; Redis Streams is a drop-in (single stream ordered =
//! single partition ordered, sharded by machine_id).

use async_trait::async_trait;
use serde_json::{json, Value};

/// Wire frame pushed into the pipeline: envelope + routing metadata.
pub fn wrap(user_id: &str, machine_id: &str, envelope: &Value) -> Value {
    json!({
        "user_id": user_id,
        "machine_id": machine_id,
        "envelope": envelope,
    })
}

#[async_trait]
pub trait EventSink: Send + Sync {
    async fn publish(&self, key: &str, payload: &str) -> anyhow::Result<()>;
}

#[async_trait]
pub trait EventSource: Send + Sync {
    /// Consume forever; each payload is passed to the handler. Returns on
    /// unrecoverable error.
    async fn run(&self, handler: Box<dyn FnMut(String) + Send>);
}

// ----------------------------------------------------------------- Kafka ---

#[cfg(feature = "kafka")]
pub mod kafka {
    use super::*;
    use rdkafka::{
        admin::{AdminClient, AdminOptions, NewTopic, TopicReplication},
        client::DefaultClientContext,
        config::ClientConfig,
        consumer::{Consumer, StreamConsumer},
        producer::{FutureProducer, FutureRecord},
        types::RDKafkaErrorCode,
        Message,
    };

    pub struct KafkaSink {
        producer: FutureProducer,
        topic: String,
    }

    impl KafkaSink {
        pub fn connect(brokers: &str, topic: &str) -> anyhow::Result<Self> {
            let producer: FutureProducer = ClientConfig::new()
                .set("bootstrap.servers", brokers)
                .set("message.timeout.ms", "10000")
                .create()?;
            Ok(Self { producer, topic: topic.into() })
        }
    }

    #[async_trait]
    impl EventSink for KafkaSink {
        async fn publish(&self, key: &str, payload: &str) -> anyhow::Result<()> {
            self.producer
                .send(
                    FutureRecord::to(&self.topic).key(key).payload(payload),
                    std::time::Duration::from_secs(5),
                )
                .await
                .map(|_| ())
                .map_err(|(e, _)| anyhow::anyhow!("kafka produce failed: {e}"))?;
            Ok(())
        }
    }

    pub struct KafkaSource {
        consumer: StreamConsumer,
    }

    /// 预建 topic（单分区：ingest 单 worker 保序）。broker 侧自动建 topic 是
    /// 惰性的，消费端在 topic 缺位时会持续报 UnknownTopicOrPartition，
    /// 启动时显式建掉，失败不阻塞（消费端每秒重试）。
    pub async fn ensure_topic(brokers: &str, topic: &str) {
        let admin: AdminClient<DefaultClientContext> = ClientConfig::new()
            .set("bootstrap.servers", brokers)
            .create()
            .expect("kafka admin client");
        let new_topic = NewTopic::new(topic, 1, TopicReplication::Fixed(1));
        let results = admin.create_topics([&new_topic], &AdminOptions::new()).await;
        match results {
            Ok(entries) => {
                for entry in entries {
                    match entry {
                        Ok(_) => tracing::info!("kafka topic ready: {topic}"),
                        Err((_, RDKafkaErrorCode::TopicAlreadyExists)) => {
                            tracing::info!("kafka topic exists: {topic}")
                        }
                        Err((name, code)) => {
                            tracing::warn!("kafka ensure topic {name} failed: {code}")
                        }
                    }
                }
            }
            Err(e) => tracing::warn!("kafka ensure topic failed (non-fatal): {e}"),
        }
    }

    impl KafkaSource {
        pub fn connect(brokers: &str, topic: &str) -> anyhow::Result<Self> {
            let consumer: StreamConsumer = ClientConfig::new()
                .set("bootstrap.servers", brokers)
                .set("group.id", "pidock-ingest")
                .set("enable.auto.commit", "true")
                .set("auto.offset.reset", "earliest")
                // fast rebalance: a killed instance frees its partition quickly
                .set("session.timeout.ms", "10000")
                .set("heartbeat.interval.ms", "3000")
                .create()?;
            consumer.subscribe(&[topic])?;
            Ok(Self { consumer })
        }
    }

    #[async_trait]
    impl EventSource for KafkaSource {
        async fn run(&self, mut handler: Box<dyn FnMut(String) + Send>) {
            loop {
                match self.consumer.recv().await {
                    Ok(msg) => {
                        if let Some(Ok(payload)) = msg.payload_view::<str>() {
                            handler(payload.to_string());
                        }
                    }
                    Err(e) => {
                        tracing::warn!("kafka recv error: {e}");
                        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                    }
                }
            }
        }
    }
}

// ---------------------------------------------------------- Redis Streams ---

pub mod redis_stream {
    use super::*;
    use redis::AsyncCommands;

    const STREAM_KEY: &str = "pidock.events";
    const GROUP: &str = "pidock-ingest";

    pub struct RedisSink {
        conn: redis::aio::MultiplexedConnection,
    }

    impl RedisSink {
        pub async fn connect(url: &str) -> anyhow::Result<Self> {
            let client = redis::Client::open(url)?;
            let conn = client.get_multiplexed_tokio_connection().await?;
            Ok(Self { conn })
        }
    }

    #[async_trait]
    impl EventSink for RedisSink {
        async fn publish(&self, key: &str, payload: &str) -> anyhow::Result<()> {
            let mut conn = self.conn.clone();
            // MAXLEN caps the buffer; Mongo is the source of truth
            conn.xadd_maxlen::<_, _, _, _, ()>(
                STREAM_KEY,
                redis::streams::StreamMaxlen::Approx(200_000),
                "*",
                &[(key, payload)],
            )
            .await?;
            Ok(())
        }
    }

    pub struct RedisSource {
        conn: redis::aio::MultiplexedConnection,
        consumer: String,
    }

    impl RedisSource {
        pub async fn connect(url: &str) -> anyhow::Result<Self> {
            let client = redis::Client::open(url)?;
            let conn = client.get_multiplexed_tokio_connection().await?;
            let consumer = format!("ingest-{}", uuid::Uuid::now_v7().simple());
            // create group; BUSYGROUP = already exists
            let mut c = conn.clone();
            let _: Result<(), _> = redis::cmd("XGROUP")
                .arg("CREATE")
                .arg(STREAM_KEY)
                .arg(GROUP)
                .arg("0")
                .arg("MKSTREAM")
                .query_async(&mut c)
                .await;
            Ok(Self { conn, consumer })
        }
    }

    #[async_trait]
    impl EventSource for RedisSource {
        async fn run(&self, mut handler: Box<dyn FnMut(String) + Send>) {
            let mut conn = self.conn.clone();
            // reply: Option<Vec<(stream, Vec<(entry_id, Vec<(field, value)>)>)>>
            type ReadReply = Option<Vec<(String, Vec<(String, Vec<(String, redis::Value)>)>)>>;
            loop {
                let result: Result<ReadReply, _> = redis::cmd("XREADGROUP")
                    .arg("GROUP")
                    .arg(GROUP)
                    .arg(&self.consumer)
                    .arg("BLOCK")
                    .arg(2000)
                    .arg("COUNT")
                    .arg(256)
                    .arg("STREAMS")
                    .arg(STREAM_KEY)
                    .arg(">")
                    .query_async(&mut conn)
                    .await;
                match result {
                    Ok(streams) => {
                        for (_, entries) in streams.into_iter().flatten() {
                            for (_id, fields) in entries {
                                for (_, payload) in fields {
                                    if let redis::Value::BulkString(bytes) = payload {
                                        handler(String::from_utf8_lossy(&bytes).into_owned());
                                    }
                                }
                            }
                        }
                    }
                    Err(e) => {
                        tracing::warn!("redis stream read error: {e}");
                        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                    }
                }
            }
        }
    }
}

pub async fn make_sink(cfg: &crate::config::Config) -> anyhow::Result<std::sync::Arc<dyn EventSink>> {
    if cfg.pipeline == "redis" {
        tracing::info!("event pipeline: redis-streams");
        Ok(std::sync::Arc::new(redis_stream::RedisSink::connect(&cfg.redis_url).await?))
    } else {
        #[cfg(feature = "kafka")]
        {
            tracing::info!("event pipeline: kafka");
            kafka::ensure_topic(&cfg.kafka_brokers, &cfg.kafka_topic).await;
            Ok(std::sync::Arc::new(kafka::KafkaSink::connect(
                &cfg.kafka_brokers,
                &cfg.kafka_topic,
            )?))
        }
        #[cfg(not(feature = "kafka"))]
        {
            anyhow::bail!("built without the kafka feature; set PIDOCK_PIPELINE=redis or rebuild with default features")
        }
    }
}

pub async fn run_source(cfg: &crate::config::Config, handler: Box<dyn FnMut(String) + Send>) -> anyhow::Result<()> {
    if cfg.pipeline == "redis" {
        let src = redis_stream::RedisSource::connect(&cfg.redis_url).await?;
        src.run(handler).await;
        Ok(())
    } else {
        #[cfg(feature = "kafka")]
        {
            let src = kafka::KafkaSource::connect(&cfg.kafka_brokers, &cfg.kafka_topic)?;
            src.run(handler).await;
            Ok(())
        }
        #[cfg(not(feature = "kafka"))]
        {
            let _ = handler;
            anyhow::bail!("built without the kafka feature; set PIDOCK_PIPELINE=redis or rebuild with default features")
        }
    }
}
