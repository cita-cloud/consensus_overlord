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
mod panic_hook;
mod util;

use crate::panic_hook::set_panic_handler;
use clap::Parser;
use log::{debug, info, warn};

/// This doc string acts as a help message when the user runs '--help'
/// as do all doc strings on fields
#[derive(Parser)]
#[clap(version, author)]
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
    /// log config path
    #[clap(short = 'l', long = "log", default_value = "consensus-log4rs.yaml")]
    log_file: String,
    /// private key path
    #[clap(short = 'p', long = "private_key_path", default_value = "private_key")]
    private_key_path: String,
}

fn main() {
    ::std::env::set_var("RUST_BACKTRACE", "full");
    set_panic_handler();

    let opts: Opts = Opts::parse();

    // You can handle information about subcommands by requesting their matches by name
    // (as below), requesting just the name used, or both at the same time
    match opts.subcmd {
        SubCommand::Run(opts) => {
            run(opts);
            warn!("Should not reach here");
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
use tonic::{transport::Server, Request, Response, Status};

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
    async fn reconfigure(
        &self,
        request: Request<ConsensusConfiguration>,
    ) -> Result<Response<StatusCode>, Status> {
        let configuration = request.into_inner();
        debug!("reconfigure {:?}", configuration);
        self.consensus.proc_reconfigure(configuration).await;
        let reply = StatusCode {
            code: status_code::StatusCode::Success.into(),
        };
        Ok(Response::new(reply))
    }

    async fn check_block(
        &self,
        request: Request<ProposalWithProof>,
    ) -> Result<Response<StatusCode>, Status> {
        let code = if self.consensus.reconfigure.read().await.is_none() {
            status_code::StatusCode::ConsensusServerNotReady.into()
        } else {
            let block_with_proof = request.into_inner();
            debug!("check_block {:?}", block_with_proof);
            let res = self.consensus.check_block(block_with_proof).await;
            if res {
                status_code::StatusCode::Success.into()
            } else {
                status_code::StatusCode::ProposalCheckError.into()
            }
        };

        Ok(Response::new(StatusCode { code }))
    }
}

#[tonic::async_trait]
impl NetworkMsgHandlerService for ConsensusServer {
    async fn process_network_msg(
        &self,
        request: Request<NetworkMsg>,
    ) -> Result<Response<StatusCode>, Status> {
        let msg = request.into_inner();
        if msg.module != "consensus" {
            Err(Status::invalid_argument("wrong module"))
        } else {
            info!(
                "get network message module {:?} type {:?}",
                msg.module, msg.r#type
            );
            self.consensus.proc_network_msg(msg).await;
            let reply = StatusCode {
                code: status_code::StatusCode::Success.into(),
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
    tokio::spawn(cloud_util::signal::handle_signals());

    // init log4rs
    log4rs::init_file(&opts.log_file, Default::default())
        .map_err(|e| println!("log init err: {}", e))
        .unwrap();

    // load service config
    let mut config = ConsensusConfig::new(&opts.config_path);
    config.private_key_path = opts.private_key_path;

    let grpc_port = config.consensus_port.to_string();

    info!("grpc port of consensus_overlord: {}", &grpc_port);

    let addr_str = format!("127.0.0.1:{}", &grpc_port);
    let addr = addr_str.parse().unwrap();

    init_grpc_client(&config);

    let mut interval = tokio::time::interval(Duration::from_secs(config.server_retry_interval));
    loop {
        interval.tick().await;
        // register endpoint
        {
            let register_info = RegisterInfo {
                module_name: "consensus".to_owned(),
                hostname: "127.0.0.1".to_owned(),
                port: grpc_port.clone(),
            };

            if let Ok(scode) = network_client()
                .register_network_msg_handler(register_info)
                .await
            {
                if scode.code == u32::from(status_code::StatusCode::Success) {
                    break;
                }
            }
        }
        warn!("network not ready! Retrying");
    }

    let consensus = Consensus::new(config.clone());

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
            info!("wating for reconfiguration!");
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
    let _ = if layer.is_some() {
        info!("metrics on");
        Server::builder()
            .layer(layer.unwrap())
            .add_service(ConsensusServiceServer::new(consensus_server.clone()))
            .add_service(NetworkMsgHandlerServiceServer::new(consensus_server))
            .add_service(HealthServer::new(HealthCheckServer {}))
            .serve(addr)
            .await
    } else {
        info!("metrics off");
        Server::builder()
            .add_service(ConsensusServiceServer::new(consensus_server.clone()))
            .add_service(NetworkMsgHandlerServiceServer::new(consensus_server))
            .add_service(HealthServer::new(HealthCheckServer {}))
            .serve(addr)
            .await
    };
}
