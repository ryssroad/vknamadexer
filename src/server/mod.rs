use axum::{routing::get, Router};
use std::str::FromStr;
#[cfg(feature = "prometheus")]
use axum_prometheus::{PrometheusMetricLayerBuilder, AXUM_HTTP_REQUESTS_DURATION_SECONDS};
use futures_util::{Future, TryFutureExt};
#[cfg(feature = "prometheus")]
use metrics_exporter_prometheus::{Matcher, PrometheusBuilder};
use std::{collections::HashMap, net::SocketAddr};
use tracing::{info, instrument};
use tendermint_rpc::{HttpClient, Url};

use crate::config::ServerConfig;
use crate::database::Database;
use crate::error::Error;
use crate::utils::load_checksums;

pub mod status;
pub use status::{ChainStatus, StakingInfo};
pub mod validators;
pub use validators::ValidatorInfo;
pub mod blocks;
pub mod tx;
pub use blocks::BlockInfo;
pub use tx::TxInfo;
pub mod account;
mod endpoints;
pub mod shielded;
mod utils;
pub(crate) use utils::{from_hex, serialize_hex};

use self::endpoints::{
    account::get_account_updates,
    block::{get_block_by_hash, get_block_by_height, get_last_block},
    transaction::{get_shielded_tx, get_tx_by_hash, get_vote_proposal},
    validator::{get_validator_uptime, get_validator_info, get_validator_set},
    status::{get_status, get_chain_params},
};

pub const HTTP_DURATION_SECONDS_BUCKETS: &[f64; 11] = &[
    0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0,
];

#[derive(Clone)]
pub struct ServerState {
    db: Database,
    checksums_map: HashMap<String, String>,
    http_client: HttpClient,
}

fn server_routes(state: ServerState) -> Router<()> {
    Router::new()
        .route("/block/height/:block_height", get(get_block_by_height))
        .route("/block/hash/:block_hash", get(get_block_by_hash))
        .route("/block/last", get(get_last_block))
        .route("/tx/:tx_hash", get(get_tx_by_hash))
        .route("/tx/vote_proposal/:proposal_id", get(get_vote_proposal))
        .route("/tx/shielded", get(get_shielded_tx))
        .route("/account/updates/:account_id", get(get_account_updates))
        .route(
            "/validator/:validator_address/uptime",
            get(get_validator_uptime),
        )
        .route("/validator/:validator_address/info", get(get_validator_info))
        .route("/validator/set", get(get_validator_set))
        .route("/chain/status", get(get_status))
        .route("/chain/params", get(get_chain_params))
        .with_state(state)
}

/// Returns a http server as a future so it needs to be pulled to start processing
/// incoming requests. The server address is also returned.
///
/// # Arguments
///
/// `db` The database for storing indexed data
///
/// `config` The server [configuration](ServerConfig) to use.
///
pub fn create_server(
    db: Database,
    config: &ServerConfig,
) -> Result<(SocketAddr, impl Future<Output = Result<(), Error>>), Error> {
    info!("Starting JSON server");

    let checksums_map = load_checksums()?;

    // JSON API server
    // we move the handler creation here so we propagate errors gracefully
    #[cfg(feature = "prometheus")]
    let prometheus_handle = {
        let builder = PrometheusBuilder::new().set_buckets_for_metric(
            Matcher::Full(AXUM_HTTP_REQUESTS_DURATION_SECONDS.to_string()),
            HTTP_DURATION_SECONDS_BUCKETS,
        )?;

        builder.install_recorder()?
    };

    #[cfg(feature = "prometheus")]
    let (prometheus_layer, metric_handle) = PrometheusMetricLayerBuilder::new()
        .with_prefix("server-metrics")
        .with_metrics_from_fn(|| prometheus_handle)
        .build_pair();

    let url = Url::from_str(&config.tendermint_addr)?;
    let http_client = HttpClient::new(url)?;

    let routes = server_routes(ServerState { db, checksums_map, http_client });

    #[cfg(feature = "prometheus")]
    let routes = routes
        .route("/metrics", get(|| async move { metric_handle.render() }))
        .layer(prometheus_layer);

    info!("server URL : {}:{}", &config.serve_at, &config.port);

    let server = axum::Server::try_bind(&config.address()?)
        .map_err(|e| Error::Generic(Box::new(e)))?
        .serve(routes.into_make_service());

    let local_addr = server.local_addr();

    Ok((local_addr, server.map_err(|e| Error::Generic(Box::new(e)))))
}

/// Starts a http server that listen for blocks and transactions requests.
///
/// # Arguments
///
/// `db` The database for storing indexed data
///
/// `config` The server [configuration](ServerConfig) to use.
///
/// Note:
/// This function starts a server blocking current thread, returning only
/// if server gets close or an error happens.
#[instrument(level = "trace", skip(db, config))]
pub async fn start_server(db: Database, config: &ServerConfig) -> Result<(), Error> {
    let (_, server) = create_server(db, config)?;

    server.await
}
