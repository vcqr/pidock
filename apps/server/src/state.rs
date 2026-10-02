//! Shared state: config, Mongo database, Redis client, pipeline sink,
//! in-memory command routes (machine -> sender), presence bookkeeping.

use std::collections::HashMap;
use std::sync::Arc;

use mongodb::Database;
use redis::AsyncCommands;
use tokio::sync::mpsc;

use crate::config::Config;
use crate::pipeline::EventSink;

#[derive(Clone)]
pub struct AppState {
    pub cfg: Arc<Config>,
    pub mongo: Database,
    pub redis: redis::aio::MultiplexedConnection,
    pub redis_client: redis::Client,
    pub sink: Arc<dyn EventSink>,
    pub s3: crate::s3::S3Client,
    pub routes: Arc<tokio::sync::Mutex<HashMap<String, mpsc::UnboundedSender<serde_json::Value>>>>,
}

impl AppState {
    pub async fn connect(cfg: Config, sink: Arc<dyn EventSink>) -> anyhow::Result<Self> {
        let mongo = mongodb::Client::with_uri_str(&cfg.mongo_uri).await?.database("pidock");
        let redis_client = redis::Client::open(cfg.redis_url.as_str())?;
        let redis = redis_client.get_multiplexed_tokio_connection().await?;
        Ok(Self {
            cfg: Arc::new(cfg),
            mongo,
            redis,
            redis_client: redis_client.clone(),
            s3: crate::s3::S3Client::from_env(),
            sink,
            routes: Arc::new(tokio::sync::Mutex::new(HashMap::new())),
        })
    }

    pub fn user_events_channel(user_id: &str) -> String {
        format!("user:{user_id}:events")
    }

    /// publish a wrapped frame to the user's realtime channel (web push)
    pub async fn publish_to_user(&self, user_id: &str, payload: &str) {
        let mut conn = self.redis.clone();
        let _: Result<i64, _> = redis::cmd("PUBLISH")
            .arg(Self::user_events_channel(user_id))
            .arg(payload)
            .query_async(&mut conn)
            .await;
    }

    /// register a connected desktop machine: Redis online registry + Mongo archive
    pub async fn machine_online(
        &self,
        user_id: &str,
        machine_id: &str,
        meta: &serde_json::Value,
    ) -> anyhow::Result<()> {
        let mut conn = self.redis.clone();
        let key = format!("online:machine:{machine_id}");
        let _: Result<(), _> = redis::cmd("HSET")
            .arg(&key)
            .arg("user_id")
            .arg(user_id)
            .arg("hostname")
            .arg(meta.get("hostname").and_then(|v| v.as_str()).unwrap_or(""))
            .arg("os")
            .arg(meta.get("os").and_then(|v| v.as_str()).unwrap_or(""))
            .arg("version")
            .arg(meta.get("version").and_then(|v| v.as_str()).unwrap_or(""))
            .arg("connected_at")
            .arg(chrono::Utc::now().to_rfc3339())
            .query_async(&mut conn)
            .await;
        let _: Result<(), _> = conn.expire(&key, 90).await;
        let _: Result<(), _> = conn.sadd(format!("online:user:{user_id}"), machine_id).await;

        let machines = self.mongo.collection::<mongodb::bson::Document>("machines");
        machines
            .update_one(
                mongodb::bson::doc! {"_id": machine_id},
                mongodb::bson::doc! {
                    "$set": {
                        "user_id": user_id,
                        "hostname": meta.get("hostname").and_then(|v| v.as_str()).unwrap_or(""),
                        "os": meta.get("os").and_then(|v| v.as_str()).unwrap_or(""),
                        "version": meta.get("version").and_then(|v| v.as_str()).unwrap_or(""),
                        "last_seen": chrono::Utc::now().to_rfc3339(),
                    },
                    "$setOnInsert": { "created_at": chrono::Utc::now().to_rfc3339() }
                },
            )
            .with_options(mongodb::options::UpdateOptions::builder().upsert(true).build())
            .await?;

        let frame = serde_json::json!({
            "ctrl": "machine_status",
            "machine_id": machine_id,
            "status": "online",
        });
        self.publish_to_user(user_id, &frame.to_string()).await;
        Ok(())
    }

    pub async fn machine_offline(&self, user_id: &str, machine_id: &str) {
        let mut conn = self.redis.clone();
        let _: Result<(), _> = conn.del(format!("online:machine:{machine_id}")).await;
        let _: Result<(), _> = conn.srem(format!("online:user:{user_id}"), machine_id).await;
        let frame = serde_json::json!({
            "ctrl": "machine_status",
            "machine_id": machine_id,
            "status": "offline",
        });
        self.publish_to_user(user_id, &frame.to_string()).await;
    }

    pub async fn machine_heartbeat(&self, machine_id: &str) {
        let mut conn = self.redis.clone();
        let _: Result<(), _> = conn.expire(format!("online:machine:{machine_id}"), 90).await;
    }
}
