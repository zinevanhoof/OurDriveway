use axum::Router;
use axum_reverse_proxy::ReverseProxy;
use surrealdb::{
    Surreal,
    engine::remote::ws::{Client, Ws},
};

#[derive(Clone)]
pub struct AppState {
    pub db: Surreal<Client>,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt::init();
    shared::install_default_crypto_provider();

    let db_addr = std::env::var("SURREALDB_ADDR").unwrap_or_else(|_| "localhost:8001".into());
    let db = Surreal::new::<Ws>(&db_addr).await?;

    db.use_ns("main").use_db("main").await?;

    let _state = AppState { db };

    let proxy: Router =
        ReverseProxy::new("/api/booking/graphql", &format!("http://{db_addr}/graphql")).into();

    // run our app with hyper, listening globally on port 3000
    let listener = tokio::net::TcpListener::bind("0.0.0.0:3001").await.unwrap();
    Ok(axum::serve(listener, proxy).await.unwrap())
}
