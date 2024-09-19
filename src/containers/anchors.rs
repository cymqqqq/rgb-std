// RGB standard library for working with smart contracts on Bitcoin & Lightning
//
// SPDX-License-Identifier: Apache-2.0
//
// Written in 2019-2024 by
//     Dr Maxim Orlovsky <orlovsky@lnp-bp.org>
//
// Copyright (C) 2019-2024 LNP/BP Standards Association. All rights reserved.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use std::cmp::Ordering;

use amplify::ByteArray;
use bp::dbc::opret::OpretProof;
use bp::dbc::tapret::TapretProof;
use bp::dbc::{anchor, Anchor};
use bp::{Tx, Txid};
use commit_verify::mpc;
use rgb::validation::DbcProof;
use rgb::{BundleId, DiscloseHash, TransitionBundle, XChain, XWitnessId};
use strict_encoding::StrictDumb;

use crate::{MergeReveal, MergeRevealError, LIB_NAME_RGB_STD};

#[derive(Clone, Eq, PartialEq, Debug)]
#[derive(StrictType, StrictDumb, StrictEncode, StrictDecode)]
#[strict_type(lib = LIB_NAME_RGB_STD)]
#[cfg_attr(
    feature = "serde",
    derive(Serialize, Deserialize),
    serde(crate = "serde_crate", rename_all = "camelCase")
)]
pub struct SealWitness {
    pub public: XPubWitness,
    pub anchors: AnchorSet,
}

impl SealWitness {
    pub fn new(witness: XPubWitness, anchors: AnchorSet) -> Self {
        SealWitness {
            public: witness,
            anchors,
        }
    }

    pub fn witness_id(&self) -> XWitnessId { self.public.to_witness_id() }
}

pub type XPubWitness = XChain<PubWitness>;

pub trait ToWitnessId {
    fn to_witness_id(&self) -> XWitnessId;
}

impl ToWitnessId for XPubWitness {
    fn to_witness_id(&self) -> XWitnessId { self.map_ref(|w| w.txid()) }
}

impl MergeReveal for XPubWitness {
    fn merge_reveal(self, other: Self) -> Result<Self, MergeRevealError> {
        match (self, other) {
            (XChain::Bitcoin(one), XChain::Bitcoin(two)) => {
                one.merge_reveal(two).map(XChain::Bitcoin)
            }
            (XChain::Liquid(one), XChain::Liquid(two)) => one.merge_reveal(two).map(XChain::Liquid),
            (XChain::Bitcoin(bitcoin), XChain::Liquid(liquid))
            | (XChain::Liquid(liquid), XChain::Bitcoin(bitcoin)) => {
                Err(MergeRevealError::ChainMismatch {
                    bitcoin: bitcoin.txid(),
                    liquid: liquid.txid(),
                })
            }
            _ => unreachable!(),
        }
    }
}

#[derive(Clone, Eq, Debug)]
#[derive(StrictType, StrictDumb, StrictEncode, StrictDecode)]
#[strict_type(lib = LIB_NAME_RGB_STD, tags = custom, dumb = Self::Txid(strict_dumb!()))]
#[cfg_attr(
    feature = "serde",
    derive(Serialize, Deserialize),
    serde(crate = "serde_crate", rename_all = "camelCase")
)]
pub enum PubWitness {
    #[strict_type(tag = 0x00)]
    Txid(Txid),
    #[strict_type(tag = 0x01)]
    Tx(Tx), /* TODO: Consider using `UnsignedTx` here
             * TODO: Add SPV as an option here */
}

impl PartialEq for PubWitness {
    fn eq(&self, other: &Self) -> bool { self.txid() == other.txid() }
}

impl Ord for PubWitness {
    fn cmp(&self, other: &Self) -> Ordering { self.txid().cmp(&other.txid()) }
}

impl PartialOrd for PubWitness {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> { Some(self.cmp(other)) }
}

impl PubWitness {
    pub fn new(txid: Txid) -> Self { Self::Txid(txid) }

    pub fn with(tx: Tx) -> Self { Self::Tx(tx) }

    pub fn txid(&self) -> Txid {
        match self {
            PubWitness::Txid(txid) => *txid,
            PubWitness::Tx(tx) => tx.txid(),
        }
    }

    pub fn tx(&self) -> Option<&Tx> {
        match self {
            PubWitness::Txid(_) => None,
            PubWitness::Tx(tx) => Some(tx),
        }
    }

    pub fn merge_reveal(self, other: Self) -> Result<Self, MergeRevealError> {
        match (self, other) {
            (Self::Txid(txid1), Self::Txid(txid2)) if txid1 == txid2 => Ok(Self::Txid(txid1)),
            (Self::Txid(txid), Self::Tx(tx)) | (Self::Txid(txid), Self::Tx(tx))
                if txid == tx.txid() =>
            {
                Ok(Self::Tx(tx))
            }
            // TODO: tx1 and tx2 may differ on their witness data; take the one having most of the
            // witness
            (Self::Tx(tx1), Self::Tx(tx2)) if tx1.txid() == tx2.txid() => Ok(Self::Tx(tx1)),
            (a, b) => Err(MergeRevealError::TxidMismatch(a.txid(), b.txid())),
        }
    }
}

#[derive(Clone, Eq, Debug)]
#[derive(StrictType, StrictDumb, StrictEncode, StrictDecode)]
#[strict_type(lib = LIB_NAME_RGB_STD)]
#[cfg_attr(
    feature = "serde",
    derive(Serialize, Deserialize),
    serde(crate = "serde_crate", rename_all = "camelCase")
)]
#[derive(CommitEncode)]
#[commit_encode(strategy = strict, id = DiscloseHash)]
pub struct WitnessBundle<P: mpc::Proof + StrictDumb = mpc::MerkleProof> {
    pub pub_witness: XPubWitness,
    pub anchor: Anchor<P, DbcProof>,
    pub bundle: TransitionBundle,
}

impl<P: mpc::Proof + StrictDumb> PartialEq for WitnessBundle<P> {
    fn eq(&self, other: &Self) -> bool { self.pub_witness == other.pub_witness }
}

impl<P: mpc::Proof + StrictDumb> Ord for WitnessBundle<P> {
    fn cmp(&self, other: &Self) -> Ordering { self.pub_witness.cmp(&other.pub_witness) }
}

impl<P: mpc::Proof + StrictDumb> PartialOrd for WitnessBundle<P> {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> { Some(self.cmp(other)) }
}

impl WitnessBundle<mpc::MerkleProof> {
    pub fn witness_id(&self) -> XWitnessId { self.pub_witness.to_witness_id() }
}

impl WitnessBundle {
    pub fn merge_reveal(mut self, other: Self) -> Result<Self, MergeRevealError> {
        self.pub_witness = self.pub_witness.merge_reveal(other.pub_witness)?;
        if self.anchor != other.anchor {
            return Err(MergeRevealError::AnchorsNonEqual(self.bundle.bundle_id()));
        }
        self.bundle = self.bundle.merge_reveal(other.bundle)?;
        Ok(self)
    }
}

#[derive(Clone, PartialEq, Eq, Debug)]
#[derive(StrictType, StrictEncode, StrictDecode)]
#[strict_type(lib = LIB_NAME_RGB_STD, tags = custom)]
#[cfg_attr(
    feature = "serde",
    derive(Serialize, Deserialize),
    serde(crate = "serde_crate", rename_all = "camelCase")
)]
pub enum AnchorSet {
    #[strict_type(tag = 0x01)]
    Tapret(Anchor<mpc::MerkleBlock, TapretProof>),
    #[strict_type(tag = 0x02)]
    Opret(Anchor<mpc::MerkleBlock, OpretProof>),
    #[strict_type(tag = 0x03)]
    Double {
        tapret: Anchor<mpc::MerkleBlock, TapretProof>,
        opret: Anchor<mpc::MerkleBlock, OpretProof>,
    },
}

impl StrictDumb for AnchorSet {
    fn strict_dumb() -> Self { Self::Opret(strict_dumb!()) }
}

impl AnchorSet {
    pub fn known_bundle_ids(&self) -> impl Iterator<Item = BundleId> {
        let map = match self {
            AnchorSet::Tapret(tapret) => tapret.mpc_proof.to_known_message_map().release(),
            AnchorSet::Opret(opret) => opret.mpc_proof.to_known_message_map().release(),
            AnchorSet::Double { tapret, opret } => {
                let mut map = tapret.mpc_proof.to_known_message_map().release();
                map.extend(opret.mpc_proof.to_known_message_map().release());
                map
            }
        };
        map.into_values()
            .map(|msg| BundleId::from_byte_array(msg.to_byte_array()))
    }

    pub fn has_tapret(&self) -> bool { matches!(self, Self::Tapret(_) | Self::Double { .. }) }

    pub fn has_opret(&self) -> bool { matches!(self, Self::Opret(_) | Self::Double { .. }) }

    pub fn merge_reveal(self, other: Self) -> Result<Self, anchor::MergeError> {
        match (self, other) {
            (Self::Tapret(anchor), Self::Tapret(a)) => Ok(Self::Tapret(anchor.merge_reveal(a)?)),
            (Self::Opret(anchor), Self::Opret(a)) => Ok(Self::Opret(anchor.merge_reveal(a)?)),
            (Self::Tapret(tapret), Self::Opret(opret))
            | (Self::Opret(opret), Self::Tapret(tapret)) => Ok(Self::Double { tapret, opret }),

            (Self::Double { tapret, opret }, Self::Tapret(t))
            | (Self::Tapret(t), Self::Double { tapret, opret }) => Ok(Self::Double {
                tapret: tapret.merge_reveal(t)?,
                opret,
            }),

            (Self::Double { tapret, opret }, Self::Opret(o))
            | (Self::Opret(o), Self::Double { tapret, opret }) => Ok(Self::Double {
                tapret,
                opret: opret.merge_reveal(o)?,
            }),
            (
                Self::Double { tapret, opret },
                Self::Double {
                    tapret: t,
                    opret: o,
                },
            ) => Ok(Self::Double {
                tapret: tapret.merge_reveal(t)?,
                opret: opret.merge_reveal(o)?,
            }),
        }
    }
}
