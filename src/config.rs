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

use cloud_util::common::read_toml;
use serde_derive::Deserialize;

#[derive(Debug, Deserialize, Clone)]
#[serde(default)]
pub struct ConsensusConfig {
    pub network_port: u16,
    pub consensus_port: u16,
    pub controller_port: u16,
    pub kms_port: u16,
    pub key_id: u64,
    pub node_address: String,
    pub server_retry_interval: u64,
}

impl Default for ConsensusConfig {
    fn default() -> Self {
        Self {
            network_port: 50000,
            consensus_port: 50001,
            controller_port: 50004,
            kms_port: 50005,
            key_id: 1,
            node_address: "".to_string(),
            server_retry_interval: 3,
        }
    }
}

impl ConsensusConfig {
    pub fn new(config_str: &str) -> Self {
        read_toml(config_str, "consensus_overlord")
    }
}

#[cfg(test)]
mod tests {
    use super::ConsensusConfig;

    #[test]
    fn basic_test() {
        let config = ConsensusConfig::new("example/config.toml");

        assert_eq!(config.network_port, 50000);
        assert_eq!(config.consensus_port, 50001);
        assert_eq!(config.controller_port, 50004);
        assert_eq!(config.kms_port, 50005);
        assert_eq!(config.controller_port, 50004);
        assert_eq!(
            config.node_address,
            "0x37d1c7449bfe76fe9c445e626da06265e9377601".to_string()
        );
    }
}
