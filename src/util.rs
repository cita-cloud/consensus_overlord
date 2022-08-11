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

use crate::config::ConsensusConfig;
use bytes::Bytes;
use cita_cloud_proto::client::{ClientOptions, InterceptedSvc};
use cita_cloud_proto::controller::consensus2_controller_service_client::Consensus2ControllerServiceClient;
use cita_cloud_proto::network::network_service_client::NetworkServiceClient;
use cita_cloud_proto::retry::RetryClient;
use overlord::types::Node;
use overlord::DurationConfig;
use tokio::sync::OnceCell;

pub static NETWORK_CLIENT: OnceCell<RetryClient<NetworkServiceClient<InterceptedSvc>>> =
    OnceCell::const_new();
pub static CONTROLLER_CLIENT: OnceCell<
    RetryClient<Consensus2ControllerServiceClient<InterceptedSvc>>,
> = OnceCell::const_new();

const CLIENT_NAME: &str = "consensus";

// This must be called before access to clients.
pub fn init_grpc_client(config: &ConsensusConfig) {
    NETWORK_CLIENT
        .set({
            let client_options = ClientOptions::new(
                CLIENT_NAME.to_string(),
                format!("http://127.0.0.1:{}", config.network_port),
            );
            match client_options.connect_network() {
                Ok(retry_client) => retry_client,
                Err(e) => panic!("client init error: {:?}", &e),
            }
        })
        .unwrap();
    CONTROLLER_CLIENT
        .set({
            let client_options = ClientOptions::new(
                CLIENT_NAME.to_string(),
                format!("http://127.0.0.1:{}", config.controller_port),
            );
            match client_options.connect_controller() {
                Ok(retry_client) => retry_client,
                Err(e) => panic!("client init error: {:?}", &e),
            }
        })
        .unwrap();
}

pub fn network_client() -> RetryClient<NetworkServiceClient<InterceptedSvc>> {
    NETWORK_CLIENT.get().cloned().unwrap()
}

pub fn controller_client() -> RetryClient<Consensus2ControllerServiceClient<InterceptedSvc>> {
    CONTROLLER_CLIENT.get().cloned().unwrap()
}

pub fn validators_to_nodes(validators: &[Vec<u8>]) -> Vec<Node> {
    let mut nodes = Vec::new();
    for v in validators {
        nodes.push(Node {
            address: Bytes::copy_from_slice(&v[..]),
            propose_weight: 1,
            vote_weight: 1,
        })
    }
    nodes
}

pub const HASH_BYTES_LEN: usize = 32;

pub fn sm3_hash(input: &[u8]) -> [u8; HASH_BYTES_LEN] {
    let mut result = [0u8; HASH_BYTES_LEN];
    result.copy_from_slice(libsm::sm3::hash::Sm3Hash::new(input).get_hash().as_ref());
    result
}

pub fn timer_config() -> Option<DurationConfig> {
    Some(DurationConfig::new(15, 10, 10, 7))
}

pub fn validator_to_origin(validator_address: &[u8]) -> u64 {
    let mut decoded = [0; 8];
    decoded[..8].clone_from_slice(&validator_address[..8]);
    u64::from_be_bytes(decoded)
}
