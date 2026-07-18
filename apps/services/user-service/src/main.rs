use std::sync::{Arc, LazyLock};

use axum::{Router, routing::post};
use axum_reverse_proxy::ReverseProxy;
use surrealdb::{Surreal, engine::remote::ws::Ws, opt::auth::Database};

use crate::{
    repository::{
        refresh_token_repository::RefreshTokenRepository, user_repository::UserRepository,
    },
    service::user_service::UserService,
};

#[derive(Clone)]
pub struct AppState {
    pub user_service: Arc<UserService>,
}

pub struct Config {
    pub jwt_secret: String,
    pub jwt_expiration: i64,
    pub refresh_token_expiration: i64,
}

static CONFIG: LazyLock<Config> = LazyLock::new(|| Config {
    jwt_secret: std::env::var("JWT_SECRET").expect("JWT_SECRET must be set"),
    jwt_expiration: std::env::var("JWT_EXPIRATION")
        .expect("JWT_EXPIRATION must be set")
        .parse()
        .unwrap(),
    refresh_token_expiration: std::env::var("REFRESH_TOKEN_EXPIRATION")
        .expect("REFRESH_TOKEN_EXPIRATION must be set")
        .parse()
        .unwrap(),
});

mod auth;
mod repository;
mod route;
mod service;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt::init();
    shared::install_default_crypto_provider();

    // Read user-service's own .env explicitly so the key resolves regardless of the
    // process CWD (the workspace has several services).
    dotenvy::from_filename("apps/services/user-service/.env").ok();

    let db_addr = std::env::var("SURREALDB_ADDR").unwrap_or_else(|_| "localhost:8000".into());
    let db = Surreal::new::<Ws>(&db_addr).await?;

    db.use_ns("main").use_db("main").await?;

    db.signin(Database {
        namespace: "main".into(),
        database: "main".into(),
        username: "leeroy".into(),
        password: "SuperSecretPassword!".into(),
    })
    .await?;

    let user_repository = UserRepository { db: db.clone() };
    let refresh_token_repository = RefreshTokenRepository { db };
    let user_service = UserService {
        user_repository,
        refresh_token_repository,
    };

    let state = AppState {
        user_service: Arc::new(user_service),
    };

    let api_router = Router::new()
        .route("/api/user/login", post(route::login::login))
        .route("/api/user/signup", post(route::signup::signup))
        .route("/api/user/refresh/logout", post(route::logout::logout))
        .route("/api/user/refresh", post(route::refresh::refresh));

    let proxy: Router<AppState> =
        ReverseProxy::new("/api/user/graphql", &format!("http://{db_addr}/graphql")).into();

    let app = Router::new()
        .merge(api_router)
        .merge(proxy)
        .with_state(state);

    // run our app with hyper, listening globally on port 3000
    let listener = tokio::net::TcpListener::bind("0.0.0.0:3000").await.unwrap();
    Ok(axum::serve(listener, app).await.unwrap())
}
