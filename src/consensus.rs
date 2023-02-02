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
use crate::error::ConsensusError;
use crate::util::{timer_config, validator_to_origin, validators_to_nodes};
use async_trait::async_trait;
use bytes::Bytes;
use cita_cloud_proto::client::{ControllerClientTrait, NetworkClientTrait};
use cita_cloud_proto::common::{
    ConsensusConfiguration, ConsensusConfigurationResponse, Empty, Proposal, ProposalWithProof,
    StatusCode,
};
use cita_cloud_proto::network::NetworkMsg;
use cita_cloud_proto::status_code::StatusCodeEnum;
use cloud_util::wal::{LogType, Wal as CITAWal};
use creep::Context;
use log::{info, warn};
use overlord::types::{
    AggregatedVote, Commit, Hash, Node, OverlordMsg, Proof, SignedChoke, SignedProposal,
    SignedVote, Status, ViewChangeReason, Vote, VoteType,
};
use overlord::{
    extract_voters, Codec, Consensus as OverlordConsensus, Crypto as OverlordCrypto, Overlord,
    OverlordHandler, Wal,
};
use rlp::Decodable;
use rlp::Encodable;
use rlp::Rlp;
use std::error::Error;
use std::fs;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio::sync::RwLock;

#[derive(Clone)]
pub struct Consensus {
    // servic config
    pub config: ConsensusConfig,
    pub wal: ConsensusWal,
    pub crypto: ConsensusCrypto,
    pub brain: Brain,
    pub overlord: Arc<Overlord<ConsensusProposal, Brain, ConsensusCrypto, ConsensusWal>>,
    pub overlord_handler: OverlordHandler<ConsensusProposal>,

    // internal field
    pub reconfigure: Arc<RwLock<Option<ConsensusConfiguration>>>,
}

impl Consensus {
    pub async fn new(config: ConsensusConfig, private_key_path: &str) -> Self {
        let wal = ConsensusWal::new(&config.wal_path).await;
        let crypto = ConsensusCrypto::new(private_key_path);
        let brain = Brain::new(crypto.clone());

        let overlord = Overlord::new(
            crypto.name.clone(),
            Arc::new(brain.clone()),
            Arc::new(crypto.clone()),
            Arc::new(wal.clone()),
        );

        let overlord_handler = overlord.get_handler();

        Consensus {
            config,
            wal,
            crypto,
            brain,
            overlord: Arc::new(overlord),
            overlord_handler,
            reconfigure: Arc::new(RwLock::new(None)),
        }
    }

    pub async fn run(&self, init_block_number: u64, interval: u64, authority_list: Vec<Node>) {
        self.overlord
            .run(
                init_block_number,
                interval * 1000,
                authority_list,
                timer_config(),
            )
            .await
            .unwrap();
    }

    async fn update_status(&self, configuration: ConsensusConfiguration) {
        let init_block_number = configuration.height + 1;
        let interval = configuration.block_interval;
        let nodes = validators_to_nodes(&configuration.validators);

        info!("update nodes!");
        self.brain.set_nodes(nodes.clone()).await;

        info!("send overlord_handler msg!");
        let ret = self.overlord_handler.send_msg(
            Context::new(),
            OverlordMsg::RichStatus(Status {
                height: init_block_number,
                interval: Some((interval * 1000) as u64),
                timer_config: timer_config(),
                authority_list: nodes,
            }),
        );
        if ret.is_err() {
            warn!("send overlord_handler msg error! {:?}", ret);
        }

        let mut new_pubkeys = Vec::new();
        for v in &configuration.validators {
            let pub_key = BlsPublicKey::try_from(v.as_ref()).unwrap();
            new_pubkeys.push(pub_key);
        }
        self.crypto.update_pubkeys(new_pubkeys).await;
    }

    pub async fn proc_reconfigure(&self, configuration: ConsensusConfiguration) {
        let configuration_height = configuration.height;
        let old_height = {
            if let Some(ref config) = *self.reconfigure.read().await {
                config.height
            } else {
                0
            }
        };

        if old_height == 0 || configuration_height > old_height {
            info!("update_status!");
            self.update_status(configuration.clone()).await;
            info!("set reconfigure!");
            *self.reconfigure.write().await = Some(configuration);
        }
    }

    pub async fn check_block(&self, proposal_with_proof: ProposalWithProof) -> bool {
        if let Some(proposal) = proposal_with_proof.proposal {
            let proposal_height = proposal.height;
            let proposal_data = proposal.data;
            let proposal_hash = Bytes::from(sm3_hash(&proposal_data).to_vec());

            info!(
                "grpc check_block: proposal height: {} hash: {}",
                proposal_height,
                hex::encode(&proposal_hash)
            );
            // we must move this ahead of Rlp::new, because Rlp is not send
            let mut authority_list = self.brain.get_nodes().await;

            if let Ok(proof) = Proof::decode(&Rlp::new(&proposal_with_proof.proof)) {
                info!(
                    "grpc check_block: proof height: {} round {} hash: {}",
                    proof.height,
                    proof.round,
                    hex::encode(&proof.block_hash)
                );
                if proof.block_hash == proposal_hash && proof.height == proposal_height {
                    if let Ok(signed_voters) =
                        extract_voters(&mut authority_list, &proof.signature.address_bitmap)
                    {
                        let vote = Vote {
                            height: proof.height,
                            round: proof.round,
                            vote_type: VoteType::Precommit,
                            block_hash: Bytes::from(proof.block_hash.to_vec()),
                        };
                        let vote_hash = self.crypto.hash(Bytes::from(rlp::encode(&vote)));
                        if self
                            .crypto
                            .verify_aggregated_signature(
                                proof.signature.signature,
                                vote_hash,
                                signed_voters,
                            )
                            .is_ok()
                        {
                            true
                        } else {
                            warn!("grpc check_block: verify_aggregated_signature failed!");
                            false
                        }
                    } else {
                        warn!("grpc check_block: extract voters failed!");
                        false
                    }
                } else {
                    warn!("grpc check_block: check height or block hash in proof failed!");
                    false
                }
            } else {
                warn!("grpc check_block: decode proof failed!");
                false
            }
        } else {
            warn!("grpc check_block: no proposal!");
            false
        }
    }

    pub async fn proc_network_msg(&self, msg: NetworkMsg) {
        info!("proc_network_msg {}!", msg.r#type.as_str());

        if msg.r#type.as_str() == "SignedVote" {
            if let Ok(vote) = SignedVote::decode(&Rlp::new(&msg.msg)) {
                let ret = self
                    .overlord_handler
                    .send_msg(Context::new(), OverlordMsg::SignedVote(vote));
                if ret.is_err() {
                    warn!("send overlord_handler msg SignedVote error! {:?}", ret);
                }
            } else {
                warn!("decode SignedVote failed!");
            }
        } else if msg.r#type.as_str() == "AggregatedVote" {
            if let Ok(agg_vote) = AggregatedVote::decode(&Rlp::new(&msg.msg)) {
                let ret = self
                    .overlord_handler
                    .send_msg(Context::new(), OverlordMsg::AggregatedVote(agg_vote));
                if ret.is_err() {
                    warn!("send overlord_handler msg AggregatedVote error! {:?}", ret);
                }
            } else {
                warn!("decode AggregatedVote failed!");
            }
        } else if msg.r#type.as_str() == "SignedProposal" {
            if let Ok(proposal) = SignedProposal::decode(&Rlp::new(&msg.msg)) {
                let ret = self
                    .overlord_handler
                    .send_msg(Context::new(), OverlordMsg::SignedProposal(proposal));
                if ret.is_err() {
                    warn!("send overlord_handler msg SignedProposal error! {:?}", ret);
                }
            } else {
                warn!("decode SignedProposal failed!");
            }
        } else if msg.r#type.as_str() == "SignedChoke" {
            if let Ok(choke) = SignedChoke::decode(&Rlp::new(&msg.msg)) {
                let ret = self
                    .overlord_handler
                    .send_msg(Context::new(), OverlordMsg::SignedChoke(choke));
                if ret.is_err() {
                    warn!("send overlord_handler msg SignedChoke error! {:?}", ret);
                }
            } else {
                warn!("decode SignedChoke failed!");
            }
        } else {
            warn!("unexpected network msg!");
        }
    }

    pub async fn ping_controller(&self) {
        let pproof = ProposalWithProof {
            proposal: Some(Proposal {
                height: u64::MAX,
                data: vec![],
            }),
            proof: vec![],
        };

        match controller_client().commit_block(pproof).await {
            Ok(config) => {
                if let ConsensusConfigurationResponse {
                    status: Some(StatusCode { code: 0 }),
                    config: Some(consensus_config),
                } = config
                {
                    self.proc_reconfigure(consensus_config).await;
                } else {
                    warn!(
                        "ping_controller: commit_block error: {}",
                        config.status.unwrap_or(StatusCode { code: 9999 }).code
                    );
                }
            }
            Err(err) => {
                warn!("ping_controller: commit_block error: {}", err);
            }
        }
    }
}

#[derive(Clone)]
pub struct ConsensusWal {
    wal: Arc<RwLock<CITAWal>>,
    wal_count: Arc<AtomicU64>,
}

impl ConsensusWal {
    pub async fn new(wal_path: &str) -> Self {
        ConsensusWal {
            wal: Arc::new(RwLock::new(CITAWal::create(wal_path).await.unwrap())),
            wal_count: Arc::new(AtomicU64::new(0)),
        }
    }
}

// senmantic of save and load acturally is set and get
#[async_trait]
impl Wal for ConsensusWal {
    async fn save(&self, info: Bytes) -> Result<(), Box<dyn Error + Send>> {
        info!("save wal!");
        let current_wal_count = self.wal_count.load(Ordering::SeqCst);
        let next_wal_count = current_wal_count.overflowing_add(1).0;
        self.wal
            .write()
            .await
            .save(next_wal_count, LogType::Skip, info.as_ref())
            .await
            .map_err(ConsensusError::WALErr)?;
        self.wal_count.store(next_wal_count, Ordering::SeqCst);
        Ok(())
    }

    async fn load(&self) -> Result<Option<Bytes>, Box<dyn Error + Send>> {
        info!("load wal!");
        let record = self.wal.write().await.load().await;
        if record.is_empty() {
            warn!("failed to load wal!");
            Err(ConsensusError::Other("failed to load wal".to_string()).into())
        } else {
            let (_, info) = record[0].clone();
            Ok(Some(Bytes::from(info)))
        }
    }
}

use crate::util::{controller_client, network_client, sm3_hash};
use ophelia::HashValue;
use ophelia::{BlsSignatureVerify, PrivateKey, PublicKey, Signature, ToBlsPublicKey};
use ophelia_blst::{BlsPrivateKey, BlsPublicKey, BlsSignature};

#[derive(Clone)]
pub struct ConsensusCrypto {
    pub private_key: BlsPrivateKey,
    pub pubkeys: Arc<RwLock<Vec<BlsPublicKey>>>,
    pub common_ref: String,
    pub name: Bytes,
}

impl ConsensusCrypto {
    pub fn new(private_key_path: &str) -> Self {
        let private_key_bytes = hex::decode(fs::read_to_string(private_key_path).unwrap()).unwrap();
        let private_key = BlsPrivateKey::try_from(private_key_bytes.as_ref()).unwrap();
        let common_ref = "".to_string();
        let pub_key = private_key.pub_key(&common_ref);
        ConsensusCrypto {
            private_key,
            pubkeys: Arc::new(RwLock::new(Vec::new())),
            common_ref,
            name: pub_key.to_bytes(),
        }
    }

    pub async fn update_pubkeys(&self, new_pubkeys: Vec<BlsPublicKey>) {
        *self.pubkeys.write().await = new_pubkeys;
    }

    pub fn inner_verify_aggregated_signature(
        &self,
        hash: Bytes,
        pub_keys: Vec<BlsPublicKey>,
        signature: Bytes,
    ) -> Result<(), Box<dyn Error + Send>> {
        let aggregate_key = BlsPublicKey::aggregate(pub_keys)
            .map_err(|e| ConsensusError::CryptoErr(Box::new(e.into())))?;
        let aggregated_signature = BlsSignature::try_from(signature.as_ref())
            .map_err(|e| ConsensusError::CryptoErr(Box::new(e)))?;
        let hash = HashValue::try_from(hash.as_ref())
            .map_err(|_| ConsensusError::Other("failed to convert hash value".to_string()))?;

        aggregated_signature
            .verify(&hash, &aggregate_key, &self.common_ref)
            .map_err(|e| ConsensusError::CryptoErr(Box::new(e)))?;
        Ok(())
    }
}

impl OverlordCrypto for ConsensusCrypto {
    fn hash(&self, msg: Bytes) -> Bytes {
        Bytes::from(sm3_hash(msg.as_ref()).to_vec())
    }

    fn sign(&self, hash: Bytes) -> Result<Bytes, Box<dyn Error + Send>> {
        let hash = HashValue::try_from(hash.as_ref())
            .map_err(|_| ConsensusError::Other("failed to convert hash value".to_string()))?;
        let sig = self.private_key.sign_message(&hash);
        Ok(sig.to_bytes())
    }

    fn verify_signature(
        &self,
        signature: Bytes,
        hash: Bytes,
        voter: Bytes,
    ) -> Result<(), Box<dyn Error + Send>> {
        let hash = HashValue::try_from(hash.as_ref())
            .map_err(|_| ConsensusError::Other("failed to convert hash value".to_string()))?;

        let pub_key = BlsPublicKey::try_from(voter.as_ref())
            .map_err(|_| ConsensusError::Other("lose public key".to_string()))?;

        let signature = BlsSignature::try_from(signature.as_ref())
            .map_err(|e| ConsensusError::CryptoErr(Box::new(e)))?;

        signature
            .verify(&hash, &pub_key, &self.common_ref)
            .map_err(|e| ConsensusError::CryptoErr(Box::new(e)))?;
        Ok(())
    }

    fn aggregate_signatures(
        &self,
        signatures: Vec<Bytes>,
        voters: Vec<Bytes>,
    ) -> Result<Bytes, Box<dyn Error + Send>> {
        if signatures.len() != voters.len() {
            return Err(ConsensusError::Other(
                "signatures length does not match voters length".to_string(),
            )
            .into());
        }

        let mut sigs_pubkeys = Vec::with_capacity(signatures.len());
        for (sig, addr) in signatures.iter().zip(voters.iter()) {
            let signature = BlsSignature::try_from(sig.as_ref())
                .map_err(|e| ConsensusError::CryptoErr(Box::new(e)))?;

            let pub_key = BlsPublicKey::try_from(addr.as_ref())
                .map_err(|_| ConsensusError::Other("lose public key".to_string()))?;

            sigs_pubkeys.push((signature, pub_key.to_owned()));
        }

        let sig = BlsSignature::combine(sigs_pubkeys)
            .map_err(|e| ConsensusError::CryptoErr(Box::new(e.into())))?;
        Ok(sig.to_bytes())
    }

    fn verify_aggregated_signature(
        &self,
        aggregated_signature: Bytes,
        hash: Bytes,
        voters: Vec<Bytes>,
    ) -> Result<(), Box<dyn Error + Send>> {
        let mut pub_keys = Vec::with_capacity(voters.len());

        for addr in voters.iter() {
            let pub_key = BlsPublicKey::try_from(addr.as_ref())
                .map_err(|_| ConsensusError::Other("lose public key".to_string()))?;
            pub_keys.push(pub_key.clone());
        }

        self.inner_verify_aggregated_signature(hash, pub_keys, aggregated_signature)?;
        Ok(())
    }
}

#[derive(Clone, Debug)]
pub struct ConsensusProposal {
    data: Bytes,
}

impl PartialEq for ConsensusProposal {
    fn eq(&self, other: &Self) -> bool {
        self.data == other.data
    }
}

impl Eq for ConsensusProposal {}

impl Codec for ConsensusProposal {
    fn encode(&self) -> Result<Bytes, Box<dyn Error + Send>> {
        Ok(self.data.clone())
    }

    fn decode(data: Bytes) -> Result<Self, Box<dyn Error + Send>> {
        Ok(ConsensusProposal { data })
    }
}

use overlord::error::ConsensusError as OverlordError;

#[derive(Clone)]
pub struct Brain {
    crypto: ConsensusCrypto,
    nodes: Arc<RwLock<Vec<Node>>>,
}

impl Brain {
    pub fn new(crypto: ConsensusCrypto) -> Self {
        Brain {
            crypto,
            nodes: Arc::new(RwLock::new(Vec::new())),
        }
    }

    pub async fn set_nodes(&self, new_nodes: Vec<Node>) {
        let mut nodes = self.nodes.write().await;

        *nodes = new_nodes;
    }

    pub async fn get_nodes(&self) -> Vec<Node> {
        self.nodes.read().await.clone()
    }
}

#[async_trait]
impl OverlordConsensus<ConsensusProposal> for Brain {
    async fn get_block(
        &self,
        _ctx: Context,
        height: u64,
    ) -> Result<(ConsensusProposal, Hash), Box<dyn Error + Send>> {
        info!("get_block!");
        match controller_client().get_proposal(Empty {}).await {
            Ok(response) => {
                let status_code = response.status.unwrap();

                if let Some(proposal) = response.proposal {
                    let proposal_height = proposal.height;
                    let proposal_data = proposal.data;

                    if proposal_height != height {
                        warn!("get_block error height: {} {}", height, proposal_height);
                        Err(Box::new(ConsensusError::Other(
                            "get block failed".to_string(),
                        )))
                    } else {
                        Ok((
                            ConsensusProposal {
                                data: Bytes::from(proposal_data.clone()),
                            },
                            Bytes::from(sm3_hash(&proposal_data).to_vec()),
                        ))
                    }
                } else {
                    warn!("get_block error: {}", StatusCodeEnum::from(status_code));
                    Err(Box::new(ConsensusError::Other(
                        "get block failed".to_string(),
                    )))
                }
            }
            Err(status) => {
                warn!("get_block error: {}", status.to_string());
                Err(Box::new(ConsensusError::Other(
                    "get block failed".to_string(),
                )))
            }
        }
    }

    async fn check_block(
        &self,
        _ctx: Context,
        height: u64,
        _hash: Hash,
        speech: ConsensusProposal,
    ) -> Result<(), Box<dyn Error + Send>> {
        info!("check_proposal {}!", height);
        match controller_client()
            .check_proposal(Proposal {
                height,
                data: speech.data.to_vec(),
            })
            .await
        {
            Ok(scode) => {
                if StatusCodeEnum::from(scode.code) == StatusCodeEnum::Success {
                    Ok(())
                } else {
                    warn!("check_proposal failed {}", scode.code);
                    Err(Box::new(ConsensusError::Other(
                        "check_proposal failed".to_string(),
                    )))
                }
            }
            Err(status) => {
                warn!("check_proposal error: {}", status.to_string());
                Err(Box::new(ConsensusError::Other(
                    "check_proposal failed".to_string(),
                )))
            }
        }
    }

    async fn commit(
        &self,
        _ctx: Context,
        height: u64,
        commit: Commit<ConsensusProposal>,
    ) -> Result<Status, Box<dyn Error + Send>> {
        info!("commit {}!", height);
        let proposal = commit.content.data;
        let proof = commit.proof.rlp_bytes();

        let pproof = ProposalWithProof {
            proposal: Some(Proposal {
                height,
                data: proposal.to_vec(),
            }),
            proof: proof.to_vec(),
        };

        match controller_client().commit_block(pproof).await {
            Ok(config) => {
                if let Some((status, config)) = config.status.zip(config.config) {
                    if StatusCodeEnum::from(status.code) == StatusCodeEnum::Success {
                        let new_block_number = config.height + 1;
                        let interval = config.block_interval;
                        let nodes = validators_to_nodes(&config.validators);

                        self.set_nodes(nodes.clone()).await;

                        let mut new_pubkeys = Vec::new();
                        for v in config.validators {
                            let pub_key = BlsPublicKey::try_from(v.as_ref()).map_err(|_| {
                                ConsensusError::Other("lose public key".to_string())
                            })?;
                            new_pubkeys.push(pub_key);
                        }
                        self.crypto.update_pubkeys(new_pubkeys).await;

                        Ok(Status {
                            height: new_block_number,
                            interval: Some((interval * 1000) as u64),
                            timer_config: timer_config(),
                            authority_list: nodes,
                        })
                    } else {
                        warn!("commit_block error: {}", status.code);
                        Err(Box::new(ConsensusError::Other(
                            "commit block failed".to_string(),
                        )))
                    }
                } else {
                    warn!("commit_block error");
                    Err(Box::new(ConsensusError::Other(
                        "commit block failed".to_string(),
                    )))
                }
            }
            Err(status) => {
                warn!("commit_block error: {}", status.to_string());
                Err(Box::new(ConsensusError::Other(
                    "commit block failed".to_string(),
                )))
            }
        }
    }

    async fn get_authority_list(
        &self,
        _ctx: Context,
        height: u64,
    ) -> Result<Vec<Node>, Box<dyn Error + Send>> {
        info!("get_authority_list {}!", height);
        Ok(self.get_nodes().await)
    }

    async fn broadcast_to_other(
        &self,
        _ctx: Context,
        words: OverlordMsg<ConsensusProposal>,
    ) -> Result<(), Box<dyn Error + Send>> {
        info!("broadcast_to_other!");
        let network_msg = match words {
            OverlordMsg::SignedProposal(proposal) => NetworkMsg {
                module: "consensus".to_owned(),
                r#type: "SignedProposal".to_string(),
                origin: 0,
                msg: proposal.rlp_bytes().to_vec(),
            },
            OverlordMsg::SignedChoke(choke) => NetworkMsg {
                module: "consensus".to_owned(),
                r#type: "SignedChoke".to_string(),
                origin: 0,
                msg: choke.rlp_bytes().to_vec(),
            },
            OverlordMsg::AggregatedVote(agg_vote) => NetworkMsg {
                module: "consensus".to_owned(),
                r#type: "AggregatedVote".to_string(),
                origin: 0,
                msg: agg_vote.rlp_bytes().to_vec(),
            },
            _ => {
                warn!("broadcast_to_other unexpected network msg!");
                return Err(Box::new(ConsensusError::Other(
                    "broadcast_to_other unexpected network msg".to_string(),
                )));
            }
        };

        let resp = network_client().broadcast(network_msg).await;
        if let Err(e) = resp {
            warn!("net client broadcast error {:?}", e);

            return Err(Box::new(ConsensusError::Other(
                " broadcast network msg error".to_string(),
            )));
        }
        Ok(())
    }

    async fn transmit_to_relayer(
        &self,
        _ctx: Context,
        name: Bytes,
        words: OverlordMsg<ConsensusProposal>,
    ) -> Result<(), Box<dyn Error + Send>> {
        info!("transmit_to_relayer!");
        let network_msg = match words {
            OverlordMsg::SignedVote(vote) => NetworkMsg {
                module: "consensus".to_owned(),
                r#type: "SignedVote".to_owned(),
                origin: validator_to_origin(&name),
                msg: vote.rlp_bytes().to_vec(),
            },
            OverlordMsg::AggregatedVote(agg_vote) => NetworkMsg {
                module: "consensus".to_owned(),
                r#type: "AggregatedVote".to_owned(),
                origin: validator_to_origin(&name),
                msg: agg_vote.rlp_bytes().to_vec(),
            },
            _ => {
                warn!("transmit_to_relayer unexpected network msg!");
                return Err(Box::new(ConsensusError::Other(
                    "transmit_to_relayer unexpected network msg".to_string(),
                )));
            }
        };

        let resp = network_client().send_msg(network_msg).await;
        if let Err(e) = resp {
            warn!("net client send_msg error {:?}", e);

            return Err(Box::new(ConsensusError::Other(
                " transmit_to_relayer network msg error".to_string(),
            )));
        }
        Ok(())
    }

    fn report_error(&self, _ctx: Context, err: OverlordError) {
        warn!("report_error {}", err);
    }

    fn report_view_change(&self, _ctx: Context, height: u64, round: u64, reason: ViewChangeReason) {
        info!("view change {} {}: {}", height, round, reason);
    }
}
