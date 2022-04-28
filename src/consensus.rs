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
use async_trait::async_trait;
use bytes::{Bytes, BytesMut};
use cita_cloud_proto::common::Proposal;
use cita_cloud_proto::common::{ConsensusConfiguration, Empty, ProposalWithProof, StatusCode};
use cita_cloud_proto::network::NetworkMsg;
use cloud_util::wal::{LogType, Wal as CITAWal};
use creep::{Cloneable, Context};
use overlord::types::{Commit, Hash, Node, OverlordMsg, Status, ViewChangeReason};
use overlord::{
    Codec, Consensus as OverlordConsensus, Crypto as OverlordCrypto, DurationConfig, Overlord,
    OverlordHandler, Wal,
};
use prost::Message;
use std::collections::HashMap;
use std::error::Error;
use std::fs;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio::sync::RwLock;

#[derive(Clone)]
pub struct Consensus {
    // servic config
    config: ConsensusConfig,
    wal: ConsensusWal,
    crypto: ConsensusCrypto,
    brain: Brain,
    overlord: Arc<Overlord<ConsensusProposal, Brain, ConsensusCrypto, ConsensusWal>>,
    overlord_handler: OverlordHandler<ConsensusProposal>,
}

impl Consensus {
    pub fn new(config: ConsensusConfig) -> Self {
        let wal = ConsensusWal::new(&config.wal_path);
        let crypto = ConsensusCrypto::new(&config.private_key_path);
        let brain = Brain::new();

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
        }
    }

    pub async fn run(&self, init_block_number: u64) {
        self.overlord_handler
            .send_msg(
                Context::new(),
                OverlordMsg::RichStatus(Status {
                    height: init_block_number,
                    interval: Some(3),
                    timer_config: None,
                    authority_list: Vec::new(),
                }),
            )
            .unwrap();

        self.overlord.run(0, 3, Vec::new(), None).await.unwrap();
    }

    pub async fn proc_reconfigure(&self, configuration: ConsensusConfiguration) {
        // todo
    }

    pub async fn check_block(&self, block_with_proof: ProposalWithProof) -> bool {
        // todo
        true
    }

    pub async fn proc_network_msg(&self, msg: NetworkMsg) {
        // todo
        /*
        match msg {
            OverlordMsg::SignedVote(vote) => {
                self.overlord_handler
                    .send_msg(Context::new(), OverlordMsg::SignedVote(vote))
                    .unwrap();
            }
            OverlordMsg::SignedProposal(proposal) => {
                self.overlord_handler
                    .send_msg(Context::new(), OverlordMsg::SignedProposal(proposal))
                    .unwrap();
            }
            OverlordMsg::AggregatedVote(agg_vote) => {
                self.overlord_handler
                    .send_msg(Context::new(), OverlordMsg::AggregatedVote(agg_vote))
                    .unwrap();
            }
            OverlordMsg::SignedChoke(choke) => {
                self.overlord_handler
                    .send_msg(Context::new(), OverlordMsg::SignedChoke(choke))
                    .unwrap();
            }
            _ => {}
        }
        */
    }
}

#[derive(Clone)]
pub struct ConsensusWal {
    wal: Arc<RwLock<CITAWal>>,
    wal_count: Arc<AtomicU64>,
}

impl ConsensusWal {
    pub fn new(wal_path: &str) -> Self {
        ConsensusWal {
            wal: Arc::new(RwLock::new(CITAWal::create(wal_path).unwrap())),
            wal_count: Arc::new(AtomicU64::new(0)),
        }
    }
}

// senmantic of save and load acturally is set and get
#[async_trait]
impl Wal for ConsensusWal {
    async fn save(&self, info: Bytes) -> Result<(), Box<dyn Error + Send>> {
        let current_wal_count = self.wal_count.load(Ordering::SeqCst);
        let next_wal_count = current_wal_count.overflowing_add(1).0;
        self.wal
            .write()
            .await
            .save(next_wal_count, LogType::Skip, info.as_ref())
            .map_err(|e| ConsensusError::WALErr(e))?;
        self.wal_count.store(next_wal_count, Ordering::SeqCst);
        Ok(())
    }

    async fn load(&self) -> Result<Option<Bytes>, Box<dyn Error + Send>> {
        let (_, info) = self.wal.read().await.load()[0].clone();
        Ok(Some(Bytes::copy_from_slice(&info[..])))
    }
}

use blake2b_simd::blake2b;
use ophelia::HashValue;
use ophelia::{
    BlsSignatureVerify, Crypto, Error as CryptoError, PrivateKey, PublicKey, Signature,
    ToBlsPublicKey, ToPublicKey, UncompressedPublicKey,
};
use ophelia_blst::{BlsPrivateKey, BlsPublicKey, BlsSignature};
use ophelia_secp256k1::{
    recover as secp256k1_recover, Secp256k1, Secp256k1PrivateKey, Secp256k1PublicKey,
    Secp256k1Recoverable, Secp256k1RecoverablePrivateKey, Secp256k1RecoverablePublicKey,
    Secp256k1RecoverableSignature, Secp256k1Signature,
};
use parking_lot::RwLock as PRwLock;

#[derive(Clone)]
pub struct ConsensusCrypto {
    private_key: BlsPrivateKey,
    addr_pubkey: Arc<PRwLock<HashMap<Bytes, BlsPublicKey>>>,
    common_ref: String,
    name: Bytes,
}

impl ConsensusCrypto {
    pub fn new(private_key_path: &str) -> Self {
        let private_key_bytes = hex::decode(fs::read_to_string(private_key_path).unwrap()).unwrap();
        let private_key = BlsPrivateKey::try_from(private_key_bytes.as_ref()).unwrap();
        let common_ref = "".to_string();
        let pub_key = private_key.pub_key(&common_ref);
        ConsensusCrypto {
            private_key,
            addr_pubkey: Arc::new(PRwLock::new(HashMap::new())),
            common_ref,
            name: pub_key.to_bytes(),
        }
    }

    pub fn update_addr_pubkey(&self, new_addr_pubkey: HashMap<Bytes, BlsPublicKey>) {
        let mut map = self.addr_pubkey.write();

        *map = new_addr_pubkey;
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
        Bytes::copy_from_slice(blake2b(msg.as_ref()).as_bytes())
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
        let map = self.addr_pubkey.read();
        let hash = HashValue::try_from(hash.as_ref())
            .map_err(|_| ConsensusError::Other("failed to convert hash value".to_string()))?;
        let pub_key = map
            .get(&voter)
            .ok_or_else(|| ConsensusError::Other("lose public key".to_string()))?;
        let signature = BlsSignature::try_from(signature.as_ref())
            .map_err(|e| ConsensusError::CryptoErr(Box::new(e)))?;

        signature
            .verify(&hash, pub_key, &self.common_ref)
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

        let map = self.addr_pubkey.read();
        let mut sigs_pubkeys = Vec::with_capacity(signatures.len());
        for (sig, addr) in signatures.iter().zip(voters.iter()) {
            let signature = BlsSignature::try_from(sig.as_ref())
                .map_err(|e| ConsensusError::CryptoErr(Box::new(e)))?;

            let pub_key = map
                .get(addr)
                .ok_or_else(|| ConsensusError::Other("lose public key".to_string()))?;

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
        let map = self.addr_pubkey.read();
        let mut pub_keys = Vec::with_capacity(voters.len());

        for addr in voters.iter() {
            let pub_key = map
                .get(addr)
                .ok_or_else(|| ConsensusError::Other("lose public key".to_string()))?;
            pub_keys.push(pub_key.clone());
        }

        self.inner_verify_aggregated_signature(hash, pub_keys, aggregated_signature)?;
        Ok(())
    }
}

#[derive(Clone, Debug)]
pub struct ConsensusProposal {
    proposal: Proposal,
}

// todo maybe not corecct
impl PartialEq for ConsensusProposal {
    fn eq(&self, other: &Self) -> bool {
        self.proposal.height == other.proposal.height
    }
}

impl Eq for ConsensusProposal {}

impl Codec for ConsensusProposal {
    fn encode(&self) -> Result<Bytes, Box<dyn Error + Send>> {
        let mut buf = Vec::new();
        let ret = self.proposal.encode(&mut buf);
        match ret {
            Ok(()) => Ok(Bytes::copy_from_slice(&buf[..])),
            Err(e) => Err(Box::new(ConsensusError::EncodeError(e))),
        }
    }

    fn decode(data: Bytes) -> Result<Self, Box<dyn Error + Send>> {
        let ret = Proposal::decode(data.as_ref());
        match ret {
            Ok(proposal) => Ok(ConsensusProposal { proposal }),
            Err(e) => Err(Box::new(ConsensusError::DecodeError(e))),
        }
    }
}

use overlord::error::ConsensusError as OverlordError;

#[derive(Clone)]
pub struct Brain {}

impl Brain {
    pub fn new() -> Self {
        Brain {}
    }
}

#[async_trait]
impl OverlordConsensus<ConsensusProposal> for Brain {
    async fn get_block(
        &self,
        _ctx: Context,
        _height: u64,
    ) -> Result<(ConsensusProposal, Hash), Box<dyn Error + Send>> {
        Err(Box::new(ConsensusError::Other(
            "get block failed".to_string(),
        )))
    }

    async fn check_block(
        &self,
        _ctx: Context,
        _height: u64,
        _hash: Hash,
        _speech: ConsensusProposal,
    ) -> Result<(), Box<dyn Error + Send>> {
        Ok(())
    }

    async fn commit(
        &self,
        _ctx: Context,
        height: u64,
        commit: Commit<ConsensusProposal>,
    ) -> Result<Status, Box<dyn Error + Send>> {
        Ok(Status {
            height: height + 1,
            interval: Some(3),
            timer_config: None,
            authority_list: Vec::new(),
        })
    }

    async fn get_authority_list(
        &self,
        _ctx: Context,
        _height: u64,
    ) -> Result<Vec<Node>, Box<dyn Error + Send>> {
        Ok(Vec::new())
    }

    async fn broadcast_to_other(
        &self,
        _ctx: Context,
        words: OverlordMsg<ConsensusProposal>,
    ) -> Result<(), Box<dyn Error + Send>> {
        Ok(())
    }

    async fn transmit_to_relayer(
        &self,
        _ctx: Context,
        name: Bytes,
        words: OverlordMsg<ConsensusProposal>,
    ) -> Result<(), Box<dyn Error + Send>> {
        Ok(())
    }

    fn report_error(&self, _ctx: Context, _err: OverlordError) {}

    fn report_view_change(
        &self,
        _ctx: Context,
        _height: u64,
        _round: u64,
        _reason: ViewChangeReason,
    ) {
    }
}
