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

use derive_more::Display;
use ophelia::Error as CryptoError;
use prost::{DecodeError as ProtoDecodeError, EncodeError as ProtoEncodeError};
use std::error::Error;

#[derive(Debug, Display)]
pub enum ConsensusError {
    WALErr(std::io::Error),

    #[display("{_0:?}")]
    Other(String),

    /// This boxed error should be a `CryptoError`.
    #[display("Crypto error {_0:?}")]
    CryptoErr(Box<CryptoError>),

    #[display("decode error {_0:?}")]
    DecodeError(ProtoDecodeError),

    #[display("encode error {_0:?}")]
    EncodeError(ProtoEncodeError),
}

impl Error for ConsensusError {}

impl From<ConsensusError> for Box<dyn Error + Send> {
    fn from(error: ConsensusError) -> Self {
        Box::new(error) as Box<dyn Error + Send>
    }
}
