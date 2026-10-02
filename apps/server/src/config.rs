use std::env;

#[derive(Clone, Debug)]
pub struct Config {
    pub bind: String,
    pub mongo_uri: String,
    pub redis_url: String,
    // consumed by the kafka pipeline (feature-gated)
    #[allow(dead_code)]
    pub kafka_brokers: String,
    #[allow(dead_code)]
    pub kafka_topic: String,
    pub pipeline: String,
    pub jwt_secret: String,
}

impl Config {
    pub fn from_env() -> Self {
        Self {
            bind: env::var("PIDOCK_BIND").unwrap_or_else(|_| "0.0.0.0:8080".into()),
            mongo_uri: env::var("PIDOCK_MONGO_URI").unwrap_or_else(|_| "mongodb://localhost:27017".into()),
            redis_url: env::var("PIDOCK_REDIS_URL").unwrap_or_else(|_| "redis://localhost:6379".into()),
            kafka_brokers: env::var("PIDOCK_KAFKA_BROKERS").unwrap_or_else(|_| "localhost:9092".into()),
            kafka_topic: env::var("PIDOCK_KAFKA_TOPIC").unwrap_or_else(|_| "pidock.events".into()),
            pipeline: env::var("PIDOCK_PIPELINE").unwrap_or_else(|_| "kafka".into()),
            jwt_secret: env::var("PIDOCK_JWT_SECRET").unwrap_or_else(|_| "dev-secret-change-me".into()),
        }
    }
}
