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
            Ok(Self {
                producer,
                topic: topic.into(),
            })
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
        let results = admin
            .create_topics([&new_topic], &AdminOptions::new())
            .await;
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
        client: redis::Client,
        conn: tokio::sync::Mutex<redis::aio::MultiplexedConnection>,
    }

    impl RedisSink {
        pub async fn connect(url: &str) -> anyhow::Result<Self> {
            let client = redis::Client::open(url)?;
            let conn = client.get_multiplexed_tokio_connection().await?;
            Ok(Self {
                client,
                conn: tokio::sync::Mutex::new(conn),
            })
        }

        /// 断线重连：MultiplexedConnection 不会自愈，出错时重建连接
        async fn fresh_conn(&self) -> anyhow::Result<redis::aio::MultiplexedConnection> {
            let mut guard = self.conn.lock().await;
            let conn = self.client.get_multiplexed_tokio_connection().await?;
            *guard = conn.clone();
            Ok(conn)
        }
    }

    #[async_trait]
    impl EventSink for RedisSink {
        async fn publish(&self, key: &str, payload: &str) -> anyhow::Result<()> {
            for attempt in 0..2 {
                let mut conn = {
                    let guard = self.conn.lock().await;
                    guard.clone()
                };
                // MAXLEN caps the buffer; Mongo is the source of truth
                let r = conn
                    .xadd_maxlen::<_, _, _, _, ()>(
                        STREAM_KEY,
                        redis::streams::StreamMaxlen::Approx(200_000),
                        "*",
                        &[(key, payload)],
                    )
                    .await;
                match r {
                    Ok(_) => return Ok(()),
                    // 第一次失败重建连接重试一次；再失败才向上报
                    Err(e) if attempt == 0 => {
                        tracing::warn!("redis publish failed ({e}); reconnecting");
                        if let Err(re) = self.fresh_conn().await {
                            tracing::warn!("redis reconnect failed: {re}");
                        }
                    }
                    Err(e) => return Err(e.into()),
                }
            }
            anyhow::bail!("redis publish failed after reconnect")
        }
    }

    pub struct RedisSource {
        client: redis::Client,
        consumer: String,
    }

    impl RedisSource {
        pub async fn connect(url: &str) -> anyhow::Result<Self> {
            let client = redis::Client::open(url)?;
            let consumer = format!("ingest-{}", uuid::Uuid::now_v7().simple());
            Ok(Self { client, consumer })
        }

        /// 建（或重建）消费组：Redis 重启后流被清空，MKSTREAM 顺带重建流
        async fn ensure_group(&self, conn: &mut redis::aio::MultiplexedConnection) {
            let _: Result<(), _> = redis::cmd("XGROUP")
                .arg("CREATE")
                .arg(STREAM_KEY)
                .arg(GROUP)
                .arg("0")
                .arg("MKSTREAM")
                .query_async(conn)
                .await;
        }
    }

    #[async_trait]
    impl EventSource for RedisSource {
        async fn run(&self, mut handler: Box<dyn FnMut(String) + Send>) {
            // reply: Option<Vec<(stream, Vec<(entry_id, Vec<(field, value)>)>)>>
            type ReadReply = Option<Vec<(String, Vec<(String, Vec<(String, redis::Value)>)>)>>;
            let mut backoff = 1u64;
            loop {
                // 连接（断线自动重连，指数退避封顶 10s）
                let mut conn = match self.client.get_multiplexed_tokio_connection().await {
                    Ok(c) => c,
                    Err(e) => {
                        tracing::warn!("redis connect failed: {e}");
                        tokio::time::sleep(std::time::Duration::from_secs(backoff)).await;
                        backoff = (backoff * 2).min(10);
                        continue;
                    }
                };
                self.ensure_group(&mut conn).await;
                backoff = 1;
                // 读循环：出错即断开外层重建连接（连接/消费组全部自愈）
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
                            tracing::warn!("redis stream read error: {e}; reconnecting");
                            break;
                        }
                    }
                }
                tokio::time::sleep(std::time::Duration::from_secs(backoff)).await;
                backoff = (backoff * 2).min(10);
            }
        }
    }
}

pub async fn make_sink(
    cfg: &crate::config::Config,
) -> anyhow::Result<std::sync::Arc<dyn EventSink>> {
    if cfg.pipeline == "redis" {
        tracing::info!("event pipeline: redis-streams");
        Ok(std::sync::Arc::new(
            redis_stream::RedisSink::connect(&cfg.redis_url).await?,
        ))
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

pub async fn run_source(
    cfg: &crate::config::Config,
    handler: Box<dyn FnMut(String) + Send>,
) -> anyhow::Result<()> {
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
