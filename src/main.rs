// Copyright Rivtower Technologies LLC.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
// http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

mod config;
mod consensus;
mod error;
mod health_check;
mod util;

use clap::Parser;
#[macro_use]
extern crate tracing as logger;

#[derive(Parser)]
#[clap(version, about = clap_about())]
struct Opts {
    #[clap(subcommand)]
    subcmd: SubCommand,
}

#[derive(Parser)]
enum SubCommand {
    /// run this service
    #[clap(name = "run")]
    Run(RunOpts),
}

/// A subcommand for run
#[derive(Parser)]
struct RunOpts {
    /// Chain config path
    #[clap(short = 'c', long = "config", default_value = "config.toml")]
    config_path: String,
    /// private key path
    #[clap(short = 'p', long = "private_key_path", default_value = "private_key")]
    private_key_path: String,
}

fn main() {
    ::std::env::set_var("RUST_BACKTRACE", "full");

    let opts: Opts = Opts::parse();

    // You can handle information about subcommands by requesting their matches by name
    // (as below), requesting just the name used, or both at the same time
    match opts.subcmd {
        SubCommand::Run(opts) => {
            run(opts);
        }
    }
}

use cita_cloud_proto::client::NetworkClientTrait;
use cita_cloud_proto::common::{ConsensusConfiguration, ProposalWithProof, StatusCode};
use cita_cloud_proto::consensus::consensus_service_server::{
    ConsensusService, ConsensusServiceServer,
};
use cita_cloud_proto::health_check::health_server::HealthServer;
use cita_cloud_proto::network::network_msg_handler_service_server::NetworkMsgHandlerService;
use cita_cloud_proto::network::network_msg_handler_service_server::NetworkMsgHandlerServiceServer;
use cita_cloud_proto::network::{NetworkMsg, RegisterInfo};
use cita_cloud_proto::status_code::StatusCodeEnum;
use tonic::{transport::Server, Request, Response, Status};
use util::clap_about;

// grpc server of RPC
#[derive(Clone)]
pub struct ConsensusServer {
    consensus: Consensus,
}

impl ConsensusServer {
    pub fn new(consensus: Consensus) -> Self {
        ConsensusServer { consensus }
    }
}

#[tonic::async_trait]
impl ConsensusService for ConsensusServer {
    #[instrument(skip_all)]
    async fn reconfigure(
        &self,
        request: Request<ConsensusConfiguration>,
    ) -> Result<Response<StatusCode>, Status> {
        cloud_util::tracer::set_parent(&request);
        let configuration = request.into_inner();
        debug!("reconfigure {:?}", configuration);
        self.consensus.proc_reconfigure(configuration).await;
        let reply = StatusCode {
            code: StatusCodeEnum::Success as u32,
        };
        Ok(Response::new(reply))
    }

    #[instrument(skip_all)]
    async fn check_block(
        &self,
        request: Request<ProposalWithProof>,
    ) -> Result<Response<StatusCode>, Status> {
        cloud_util::tracer::set_parent(&request);
        let code = if self.consensus.reconfigure.read().await.is_none() {
            warn!("server not ready!");
            StatusCodeEnum::ConsensusServerNotReady as u32
        } else {
            let block_with_proof = request.into_inner();
            debug!("check_block {:?}", block_with_proof);
            let res = self.consensus.check_block(block_with_proof).await;
            if res {
                StatusCodeEnum::Success as u32
            } else {
                StatusCodeEnum::ProposalCheckError as u32
            }
        };

        Ok(Response::new(StatusCode { code }))
    }
}

#[tonic::async_trait]
impl NetworkMsgHandlerService for ConsensusServer {
    #[instrument(skip_all)]
    async fn process_network_msg(
        &self,
        request: Request<NetworkMsg>,
    ) -> Result<Response<StatusCode>, Status> {
        cloud_util::tracer::set_parent(&request);
        let msg = request.into_inner();
        if msg.module != "consensus" {
            warn!("invalid module {}!", msg.module);
            Err(Status::invalid_argument("wrong module"))
        } else {
            info!(
                "get network message type {:?} from {}",
                msg.r#type,
                hex::encode(msg.origin.to_be_bytes())
            );
            self.consensus.proc_network_msg(msg).await;
            let reply = StatusCode {
                code: StatusCodeEnum::Success as u32,
            };
            Ok(tonic::Response::new(reply))
        }
    }
}

use crate::config::ConsensusConfig;
use crate::consensus::Consensus;
use crate::health_check::HealthCheckServer;
use crate::util::validators_to_nodes;
use crate::util::{init_grpc_client, network_client};
use cloud_util::metrics::{run_metrics_exporter, MiddlewareLayer};
use std::time::Duration;

#[tokio::main]
async fn run(opts: RunOpts) {
    let rx_signal = cloud_util::graceful_shutdown::graceful_shutdown();

    // load service config
    let config = ConsensusConfig::new(&opts.config_path);

    // init tracer
    cloud_util::tracer::init_tracer(config.domain.clone(), &config.log_config)
        .map_err(|e| println!("tracer init err: {e}"))
        .unwrap();

    let grpc_port = config.consensus_port.to_string();

    info!("grpc port of consensus_overlord: {}", &grpc_port);

    let addr_str = format!("[::]:{}", &grpc_port);
    let addr = addr_str.parse().unwrap();

    init_grpc_client(&config);

    let mut interval = tokio::time::interval(Duration::from_secs(config.server_retry_interval));
    loop {
        interval.tick().await;
        // register endpoint
        {
            let register_info = RegisterInfo {
                module_name: "consensus".to_owned(),
                hostname: "localhost".to_owned(),
                port: grpc_port.clone(),
            };

            if let Ok(status_code) = network_client()
                .register_network_msg_handler(register_info)
                .await
            {
                if status_code.code == (StatusCodeEnum::Success as u32) {
                    break;
                }
            }
        }
        warn!("network not ready! Retrying");
    }

    let consensus = Consensus::new(config.clone(), &opts.private_key_path).await;

    let consensus_server = ConsensusServer::new(consensus.clone());

    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(config.server_retry_interval));
        loop {
            interval.tick().await;
            // waiting init reconfiguration msg
            {
                if consensus.reconfigure.read().await.is_some() {
                    break;
                } else {
                    consensus.ping_controller().await;
                }
            }
            info!("waiting for reconfiguration!");
        }

        let (init_block_number, interval, validators) = {
            let reconfiguration_opt = consensus.reconfigure.read().await;
            let reconfiguration = reconfiguration_opt.as_ref().unwrap();
            (
                reconfiguration.height,
                reconfiguration.block_interval,
                reconfiguration.validators.clone(),
            )
        };

        info!("start consensus run!");
        consensus
            .run(
                init_block_number,
                interval as u64,
                validators_to_nodes(&validators),
            )
            .await;
    });

    let layer = if config.enable_metrics {
        tokio::spawn(async move {
            run_metrics_exporter(config.metrics_port).await.unwrap();
        });

        Some(
            tower::ServiceBuilder::new()
                .layer(MiddlewareLayer::new(config.metrics_buckets))
                .into_inner(),
        )
    } else {
        None
    };

    info!("start consensus_overlord grpc server");
    if let Some(layer) = layer {
        info!("metrics on");
        Server::builder()
            .layer(layer)
            .add_service(ConsensusServiceServer::new(consensus_server.clone()))
            .add_service(NetworkMsgHandlerServiceServer::new(consensus_server))
            .add_service(HealthServer::new(HealthCheckServer {}))
            .serve_with_shutdown(
                addr,
                cloud_util::graceful_shutdown::grpc_serve_listen_term(rx_signal),
            )
            .await
            .map_err(|e| {
                warn!("start consensus_overlord grpc server failed: {:?} ", e);
                StatusCodeEnum::FatalError
            })
            .unwrap();
    } else {
        info!("metrics off");
        Server::builder()
            .add_service(ConsensusServiceServer::new(consensus_server.clone()))
            .add_service(NetworkMsgHandlerServiceServer::new(consensus_server))
            .add_service(HealthServer::new(HealthCheckServer {}))
            .serve_with_shutdown(
                addr,
                cloud_util::graceful_shutdown::grpc_serve_listen_term(rx_signal),
            )
            .await
            .map_err(|e| {
                warn!("start consensus_overlord grpc server failed: {:?} ", e);
                StatusCodeEnum::FatalError
            })
            .unwrap();
    };
}
