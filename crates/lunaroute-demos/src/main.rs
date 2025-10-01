//! Simple Gateway Example
//!
//! This example demonstrates a basic LunaRoute gateway that:
//! - Accepts OpenAI-compatible requests on /v1/chat/completions
//! - Routes them through the OpenAI connector
//! - Returns responses in OpenAI format
//!
//! Usage:
//! ```bash
//! OPENAI_API_KEY=your_key cargo run --example simple_gateway
//! ```
//!
//! Test with:
//! ```bash
//! # Non-streaming request
//! curl http://localhost:3000/v1/chat/completions \
//!   -H "Content-Type: application/json" \
//!   -d '{
//!     "model": "gpt-5",
//!     "messages": [{"role": "user", "content": "Hello!"}]
//!   }'
//!
//! # Streaming request
//! curl http://localhost:3000/v1/chat/completions \
//!   -H "Content-Type: application/json" \
//!   -d '{
//!     "model": "gpt-5",
//!     "messages": [{"role": "user", "content": "Hello!"}],
//!     "stream": true
//!   }'
//! ```

use lunaroute_egress::openai::{OpenAIConnector, OpenAIConfig};
use lunaroute_ingress::openai;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::net::TcpListener;
use tracing::{info, Level};
use tracing_subscriber::FmtSubscriber;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Initialize tracing
    let subscriber = FmtSubscriber::builder()
        .with_max_level(Level::INFO)
        .finish();
    tracing::subscriber::set_global_default(subscriber)?;

    // Get API key from environment
    let api_key = std::env::var("OPENAI_API_KEY")
        .expect("OPENAI_API_KEY environment variable must be set");

    // Create OpenAI connector (egress)
    info!("Creating OpenAI connector");
    let config = OpenAIConfig::new(api_key);
    let connector = Arc::new(OpenAIConnector::new(config)?);

    // Create OpenAI ingress router with the connector
    info!("Creating OpenAI ingress router");
    let app = openai::router(connector);

    // Start server
    let addr = SocketAddr::from(([127, 0, 0, 1], 3000));
    info!("Starting server on {}", addr);

    let listener = TcpListener::bind(addr).await?;
    info!("✅ LunaRoute gateway listening on http://{}", addr);
    info!("   OpenAI endpoint: http://{}/v1/chat/completions", addr);
    info!("   Streaming: Add \"stream\": true to request body");

    axum::serve(listener, app).await?;

    Ok(())
}
