use super::super::{
    concurrency_limits, keepalive_durations, service::NodeService, Channels, GlobalStateR,
    ListenError,
};
use crate::settings::start::network::Listen;
use chain_network::grpc;

use tonic::transport::Server;
use tracing::{span, Level};

use tracing_futures::Instrument;

pub async fn run_listen_socket(
    listen: &Listen,
    state: GlobalStateR,
    channels: Channels,
) -> Result<(), ListenError> {
    let sockaddr = listen.address();
    let span = span!(parent: &state.span, Level::TRACE, "listen_socket", local_addr = %sockaddr.to_string());
    async {
        tracing::info!("listening and accepting gRPC connections");
        let service = grpc::server::Builder::new().build(NodeService::new(channels, state));

        Server::builder()
            .concurrency_limit_per_connection(concurrency_limits::SERVER_REQUESTS)
            .tcp_keepalive(Some(keepalive_durations::TCP))
            .add_service(service)
            .serve(sockaddr)
            .await
            .map_err(|cause| ListenError { cause, sockaddr })
    }
    .instrument(span)
    .await
}
