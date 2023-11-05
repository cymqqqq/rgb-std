// RGB standard library for working with smart contracts on Bitcoin & Lightning
//
// SPDX-License-Identifier: Apache-2.0
//
// Written in 2019-2023 by
//     Dr Maxim Orlovsky <orlovsky@lnp-bp.org>
//
// Copyright (C) 2019-2023 LNP/BP Standards Association. All rights reserved.
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

use std::collections::{BTreeMap, BTreeSet};
use std::{iter, slice};

use amplify::confinement::{LargeVec, MediumBlob, SmallOrdMap, TinyOrdMap, TinyOrdSet};
use commit_verify::Conceal;
use rgb::validation::{AnchoredBundle, ConsignmentApi};
use rgb::{
    validation, AssetTag, AssignmentType, AttachId, BundleId, ContractHistory, ContractId,
    Extension, Genesis, GraphSeal, OpId, OpRef, Operation, Schema, SchemaId, SealDefinition,
    SecretSeal, SubSchema, Transition, TransitionBundle, WitnessAnchor,
};
use strict_encoding::{StrictDeserialize, StrictDumb, StrictSerialize};

use super::{ContainerVer, ContentId, ContentSigs, Terminal};
use crate::accessors::BundleExt;
use crate::interface::{ContractSuppl, IfaceId, IfacePair};
use crate::resolvers::ResolveHeight;
use crate::LIB_NAME_RGB_STD;

pub type Transfer = Consignment<true>;
pub type Contract = Consignment<false>;

/// Consignment represents contract-specific data, always starting with genesis,
/// which must be valid under client-side-validation rules (i.e. internally
/// consistent and properly committed into the commitment layer, like bitcoin
/// blockchain or current state of the lightning channel).
///
/// All consignments-related procedures, including validation or merging
/// consignments data into stash or schema-specific data storage, must start
/// with `endpoints` and process up to the genesis. If any of the nodes within
/// the consignments are not part of the paths connecting endpoints with the
/// genesis, consignments validation will return
/// [`validation::Warning::ExcessiveNode`] warning.
#[derive(Clone, Debug)]
#[derive(StrictType, StrictDumb, StrictEncode, StrictDecode)]
#[strict_type(lib = LIB_NAME_RGB_STD)]
#[cfg_attr(
    feature = "serde",
    derive(Serialize, Deserialize),
    serde(crate = "serde_crate", rename_all = "camelCase")
)]
pub struct Consignment<const TYPE: bool> {
    /// Status of the latest validation.
    ///
    /// The value is not saved and when the structure is read from a disk or
    /// network is left uninitialized. Thus, only locally-run verification by
    /// this library is trusted.
    #[strict_type(skip, dumb = None)]
    #[cfg_attr(feature = "serde", serde(skip))]
    pub(super) validation_status: Option<validation::Status>,

    /// Version.
    pub version: ContainerVer,

    /// Specifies whether the consignment contains information about state
    /// transfer (true), or it is just a consignment with an information about a
    /// contract.
    pub transfer: bool,

    /// Schema (plus root schema, if any) under which contract is issued.
    pub schema: SubSchema,

    /// Interfaces supported by the contract.
    pub ifaces: TinyOrdMap<IfaceId, IfacePair>,

    /// Known supplements.
    pub supplements: TinyOrdSet<ContractSuppl>,

    /// Confidential asset tags.
    pub asset_tags: SmallOrdMap<AssignmentType, AssetTag>,

    /// Genesis data.
    pub genesis: Genesis,

    /// Set of seals which are history terminals.
    pub terminals: SmallOrdMap<BundleId, Terminal>,

    /// Data on all anchored state transitions contained in the consignments.
    pub bundles: LargeVec<AnchoredBundle>,

    /// Data on all state extensions contained in the consignments.
    pub extensions: LargeVec<Extension>,

    /// Data containers coming with this consignment. For the purposes of
    /// in-memory consignments we are restricting the size of the containers to
    /// 24 bit value (RGB allows containers up to 32-bit values in size).
    pub attachments: SmallOrdMap<AttachId, MediumBlob>,

    /// Signatures on the pieces of content which are the part of the
    /// consignment.
    pub signatures: TinyOrdMap<ContentId, ContentSigs>,
}

impl<const TYPE: bool> StrictSerialize for Consignment<TYPE> {}
impl<const TYPE: bool> StrictDeserialize for Consignment<TYPE> {}

impl<const TYPE: bool> Consignment<TYPE> {
    /// # Panics
    ///
    /// If the provided schema is not the one which is used by genesis.
    pub fn new(schema: SubSchema, genesis: Genesis) -> Self {
        assert_eq!(schema.schema_id(), genesis.schema_id);
        Consignment {
            validation_status: None,
            version: ContainerVer::V2,
            transfer: TYPE,
            schema,
            ifaces: none!(),
            supplements: none!(),
            asset_tags: none!(),
            genesis,
            terminals: none!(),
            bundles: none!(),
            extensions: none!(),
            attachments: none!(),
            signatures: none!(),
        }
    }

    #[inline]
    pub fn schema_id(&self) -> SchemaId { self.schema.schema_id() }

    #[inline]
    pub fn root_schema_id(&self) -> Option<SchemaId> {
        self.schema.subset_of.as_ref().map(Schema::schema_id)
    }

    #[inline]
    pub fn contract_id(&self) -> ContractId { self.genesis.contract_id() }

    pub fn anchored_bundle(&self, bundle_id: BundleId) -> Option<&AnchoredBundle> {
        self.bundles
            .iter()
            .find(|anchored_bundle| anchored_bundle.bundle.bundle_id() == bundle_id)
    }

    pub fn validation_status(&self) -> Option<&validation::Status> {
        self.validation_status.as_ref()
    }

    pub fn into_validation_status(self) -> Option<validation::Status> { self.validation_status }

    pub fn update_history<R: ResolveHeight>(
        &self,
        history: Option<&ContractHistory>,
        resolver: &mut R,
    ) -> Result<ContractHistory, R::Error> {
        let mut history = history.cloned().unwrap_or_else(|| {
            ContractHistory::with(
                self.schema_id(),
                self.root_schema_id(),
                self.contract_id(),
                &self.genesis,
            )
        });

        let mut extension_idx = self
            .extensions
            .iter()
            .map(Extension::id)
            .zip(iter::repeat(false))
            .collect::<BTreeMap<_, _>>();
        let mut ordered_extensions = BTreeMap::new();
        for anchored_bundle in &self.bundles {
            for item in anchored_bundle.bundle.values() {
                if let Some(transition) = &item.transition {
                    let txid = anchored_bundle.anchor.txid;
                    let height = resolver.resolve_height(txid)?;
                    let ord_txid = WitnessAnchor::new(height, txid);
                    history.add_transition(transition, ord_txid);
                    for (id, used) in &mut extension_idx {
                        if *used {
                            continue;
                        }
                        for input in &transition.inputs {
                            if input.prev_out.op == *id {
                                *used = true;
                                if let Some(ord) = ordered_extensions.get_mut(id) {
                                    if *ord > ord_txid {
                                        *ord = ord_txid;
                                    }
                                } else {
                                    ordered_extensions.insert(*id, ord_txid);
                                }
                            }
                        }
                    }
                }
            }
        }
        for extension in &self.extensions {
            if let Some(ord_txid) = ordered_extensions.get(&extension.id()) {
                history.add_extension(extension, *ord_txid);
            }
        }

        Ok(history)
    }

    pub fn reveal_bundle_seal(&mut self, bundle_id: BundleId, revealed: SealDefinition<GraphSeal>) {
        for anchored_bundle in &mut self.bundles {
            if anchored_bundle.bundle.bundle_id() == bundle_id {
                anchored_bundle.bundle.reveal_seal(revealed);
            }
        }
    }

    pub fn into_contract(self) -> Contract {
        Contract {
            validation_status: self.validation_status,
            version: self.version,
            transfer: false,
            schema: self.schema,
            ifaces: self.ifaces,
            supplements: self.supplements,
            asset_tags: self.asset_tags,
            genesis: self.genesis,
            terminals: self.terminals,
            bundles: self.bundles,
            extensions: self.extensions,
            attachments: self.attachments,
            signatures: self.signatures,
        }
    }
}

impl<const TYPE: bool> ConsignmentApi for Consignment<TYPE> {
    type BundleIter<'container>
    = slice::Iter<'container, AnchoredBundle> where Self: 'container;

    fn schema(&self) -> &SubSchema { &self.schema }

    #[inline]
    fn asset_tags(&self) -> &BTreeMap<AssignmentType, AssetTag> { self.asset_tags.as_inner() }

    fn operation(&self, opid: OpId) -> Option<OpRef> {
        if opid == self.genesis.id() {
            return Some(OpRef::Genesis(&self.genesis));
        }
        self.transition(opid)
            .map(OpRef::from)
            .or_else(|| self.extension(opid).map(OpRef::from))
    }

    fn genesis(&self) -> &Genesis { &self.genesis }

    fn transition(&self, opid: OpId) -> Option<&Transition> {
        for anchored_bundle in &self.bundles {
            for (id, item) in anchored_bundle.bundle.iter() {
                if *id == opid {
                    return item.transition.as_ref();
                }
            }
        }
        None
    }

    fn extension(&self, opid: OpId) -> Option<&Extension> {
        self.extensions
            .iter()
            .find(|&extension| extension.id() == opid)
    }

    fn terminals(&self) -> BTreeSet<(BundleId, SecretSeal)> {
        self.terminals
            .iter()
            .flat_map(|(bundle_id, terminal)| {
                terminal
                    .seals
                    .iter()
                    .map(|seal| (*bundle_id, seal.conceal()))
            })
            .collect()
    }

    fn anchored_bundles(&self) -> Self::BundleIter<'_> { self.bundles.iter() }

    fn bundle_by_id(&self, bundle_id: BundleId) -> Option<&TransitionBundle> {
        self.anchored_bundle(bundle_id).map(|ab| &ab.bundle)
    }

    fn op_ids_except(&self, ids: &BTreeSet<OpId>) -> BTreeSet<OpId> {
        let mut exceptions = BTreeSet::new();
        for anchored_bundle in &self.bundles {
            for item in anchored_bundle.bundle.values() {
                if let Some(id) = item.transition.as_ref().map(Transition::id) {
                    if !ids.contains(&id) {
                        exceptions.insert(id);
                    }
                }
            }
        }
        exceptions
    }

    fn has_operation(&self, opid: OpId) -> bool { self.operation(opid).is_some() }

    fn known_transitions_by_bundle_id(&self, bundle_id: BundleId) -> Option<Vec<&Transition>> {
        self.bundle_by_id(bundle_id).map(|bundle| {
            bundle
                .values()
                .filter_map(|item| item.transition.as_ref())
                .collect()
        })
    }
}
