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
}

fn main() {
    ::std::env::set_var("RUST_BACKTRACE", "full");

    let opts: Opts = Opts::parse();

    // You can handle information about subcommands by requesting their matches by name
    // (as below), requesting just the name used, or both at the same time
    match opts.subcmd {
        SubCommand::Run(opts) => {
            let fin = run(opts);
            warn!("Should not reach here {:?}", fin);
        }
    }
}

use cita_cloud_proto::common::{ConsensusConfiguration, Empty, ProposalWithProof, StatusCode};
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
        let block_with_proof = request.into_inner();
        // todo
        debug!("check_block {:?}", block_with_proof);
        let res = self.consensus.check_block(block_with_proof).await;
        let code = if res {
            status_code::StatusCode::Success.into()
        } else {
            status_code::StatusCode::ProposalCheckError.into()
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
            debug!(
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
use crate::util::{init_grpc_client, kms_client, network_client};
use std::time::Duration;
use crate::util::validators_to_nodes;

#[tokio::main]
async fn run(opts: RunOpts) {
    // load service config
    let config = ConsensusConfig::new(&opts.config_path);

    // init log4rs
    log4rs::init_file(&opts.log_file, Default::default())
        .map_err(|e| println!("log init err: {}", e))
        .unwrap();
    info!("start consensus overlord");

    let grpc_port = config.consensus_port.to_string();
    info!("grpc port of this service: {}", &grpc_port);

    init_grpc_client(&config);

    let mut interval = tokio::time::interval(Duration::from_secs(config.server_retry_interval));
    loop {
        interval.tick().await;
        // register endpoint
        {
            let request = Request::new(RegisterInfo {
                module_name: "consensus".to_owned(),
                hostname: "127.0.0.1".to_owned(),
                port: grpc_port.clone(),
            });

            if let Ok(response) = network_client().register_network_msg_handler(request).await {
                if response.into_inner().code == u32::from(status_code::StatusCode::Success) {
                    break;
                }
            }
        }
        warn!("network not ready! Retrying");
    }

    let mut interval = tokio::time::interval(Duration::from_secs(config.server_retry_interval));
    loop {
        interval.tick().await;
        // waiting kms ready
        {
            if let Ok(crypto_info) = kms_client().get_crypto_info(Request::new(Empty {})).await {
                let inner = crypto_info.into_inner();
                if inner.status.is_some() {
                    match status_code::StatusCode::from(inner.status.unwrap()) {
                        status_code::StatusCode::Success => {
                            info!("kms({}) is ready!", &inner.name);
                            break;
                        }
                        status => warn!("get get_crypto_info failed: {:?}", status),
                    }
                }
            }
        }
        warn!("kms not ready! Retrying");
    }

    let consensus = Consensus::new(config.clone());

    let consensus_server = ConsensusServer::new(consensus.clone());

    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(config.server_retry_interval));
        loop {
            interval.tick().await;
            // waiting init reconfiguration msg
            {
                if let Some(ref reconfiguration) = *consensus.reconfigure.read().await {
                    let init_block_number = reconfiguration.height;
                    let interval = reconfiguration.block_interval;
                    
                    consensus.run(init_block_number, interval as u64, validators_to_nodes(&reconfiguration.validators)).await;
                }
            }       
            info!("wating for reconfiguration!");
        }
    });

    let addr_str = format!("127.0.0.1:{}", &grpc_port);
    let addr = addr_str.parse().unwrap();

    let _ = Server::builder()
        .add_service(ConsensusServiceServer::new(consensus_server.clone()))
        .add_service(NetworkMsgHandlerServiceServer::new(consensus_server))
        .add_service(HealthServer::new(HealthCheckServer {}))
        .serve(addr)
        .await;
}
