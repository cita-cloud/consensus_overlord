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
use cita_cloud_proto::controller::consensus2_controller_service_client::Consensus2ControllerServiceClient;
use cita_cloud_proto::kms::kms_service_client::KmsServiceClient;
use cita_cloud_proto::network::network_service_client::NetworkServiceClient;
use cloud_util::crypto::{hash_data, recover_signature, sign_message};
use tokio::sync::OnceCell;
use tonic::transport::{Channel, Endpoint};

pub static KMS_CLIENT: OnceCell<KmsServiceClient<Channel>> = OnceCell::const_new();
pub static NETWORK_CLIENT: OnceCell<NetworkServiceClient<Channel>> = OnceCell::const_new();
pub static CONTROLLER_CLIENT: OnceCell<Consensus2ControllerServiceClient<Channel>> =
    OnceCell::const_new();

// This must be called before access to clients.
pub fn init_grpc_client(config: &ConsensusConfig) {
    KMS_CLIENT
        .set({
            let addr = format!("http://127.0.0.1:{}", config.kms_port);
            let channel = Endpoint::from_shared(addr).unwrap().connect_lazy().unwrap();
            KmsServiceClient::new(channel)
        })
        .unwrap();
    NETWORK_CLIENT
        .set({
            let addr = format!("http://127.0.0.1:{}", config.network_port);
            let channel = Endpoint::from_shared(addr).unwrap().connect_lazy().unwrap();
            NetworkServiceClient::new(channel)
        })
        .unwrap();
    CONTROLLER_CLIENT
        .set({
            let addr = format!("http://127.0.0.1:{}", config.controller_port);
            let channel = Endpoint::from_shared(addr).unwrap().connect_lazy().unwrap();
            Consensus2ControllerServiceClient::new(channel)
        })
        .unwrap();
}

pub fn kms_client() -> KmsServiceClient<Channel> {
    KMS_CLIENT.get().cloned().unwrap()
}

pub fn network_client() -> NetworkServiceClient<Channel> {
    NETWORK_CLIENT.get().cloned().unwrap()
}

pub fn controller_client() -> Consensus2ControllerServiceClient<Channel> {
    CONTROLLER_CLIENT.get().cloned().unwrap()
}
