use actor::{
    market::MarketBalance,
    miner::{
        compute_proving_period_deadline, ChainSectorInfo, DeadlineInfo, Deadlines, Fault,
        MinerInfo, SectorOnChainInfo, SectorPreCommitOnChainInfo, State,
    },
    power::Claim,
};
use address::Address;
use async_std::sync::Arc;
use async_std::task;
use bitfield::BitField;
use blocks::{Tipset, TipsetKeys};
use blockstore::BlockStore;
use chain::ChainStore;
use cid::Cid;
use clock::ChainEpoch;
use fil_types::SectorNumber;
use message::{MessageReceipt, UnsignedMessage};
use num_bigint::BigUint;
use num_traits::identities::Zero;
use state_manager::{call, call::InvocResult, StateManager};
use state_tree::StateTree;
use std::error::Error;

type BoxError = Box<dyn Error + 'static>;
pub struct MessageLookup {
    pub receipt: MessageReceipt,
    pub tipset: Arc<Tipset>,
}
pub fn get_network_name<DB>(state_manager: &StateManager<DB>) -> Result<String, BoxError>
where
    DB: BlockStore,
{
    let maybe_heaviest_tipset: Option<Tipset> =
        chain::get_heaviest_tipset(state_manager.get_block_store_ref())?;
    let heaviest_tipset: Tipset = maybe_heaviest_tipset.unwrap();
    state_manager
        .get_network_name(heaviest_tipset.parent_state())
        .map_err(|e| e.into())
}

pub fn state_miner_sector<DB>(
    state_manager: &StateManager<DB>,
    address: &Address,
    filter: &mut BitField,
    filter_out: bool,
    key: &TipsetKeys,
) -> Result<Vec<ChainSectorInfo>, BoxError>
where
    DB: BlockStore,
{
    let tipset = ChainStore::new(state_manager.get_block_store()).tipset_from_keys(key)?;
    let mut filter = Some(filter);
    state_manager::utils::get_miner_sector_set(
        &state_manager,
        &tipset,
        address,
        &mut filter,
        filter_out,
    )
    .map_err(|e| e.into())
}

pub fn state_miner_proving_set<DB>(
    state_manager: &StateManager<DB>,
    address: &Address,
    key: &TipsetKeys,
) -> Result<Vec<SectorOnChainInfo>, BoxError>
where
    DB: BlockStore,
{
    let tipset = ChainStore::new(state_manager.get_block_store()).tipset_from_keys(key)?;
    let miner_actor_state: State =
        state_manager.load_actor_state(&address, &tipset.parent_state())?;
    state_manager::utils::get_proving_set_raw(&state_manager, &miner_actor_state)
        .map_err(|e| e.into())
}

pub fn state_miner_info<DB>(
    state_manager: &StateManager<DB>,
    actor: &Address,
    key: &TipsetKeys,
) -> Result<MinerInfo, BoxError>
where
    DB: BlockStore,
{
    let tipset = ChainStore::new(state_manager.get_block_store()).tipset_from_keys(key)?;
    state_manager::utils::get_miner_info(state_manager, &tipset, actor).map_err(|e| e.into())
}

pub fn state_sector_info<DB>(
    state_manager: &StateManager<DB>,
    address: &Address,
    sector_number: &SectorNumber,
    key: &TipsetKeys,
) -> Result<Option<SectorOnChainInfo>, BoxError>
where
    DB: BlockStore,
{
    let tipset = ChainStore::new(state_manager.get_block_store()).tipset_from_keys(key)?;
    state_manager::utils::miner_sector_info(&state_manager, address, sector_number, &tipset)
        .map_err(|e| e.into())
}

pub fn state_sector_precommit_info<DB>(
    state_manager: &StateManager<DB>,
    address: &Address,
    sector_number: &SectorNumber,
    key: &TipsetKeys,
) -> Result<SectorPreCommitOnChainInfo, BoxError>
where
    DB: BlockStore,
{
    let tipset = ChainStore::new(state_manager.get_block_store()).tipset_from_keys(key)?;
    state_manager::utils::precommit_info(&state_manager, address, sector_number, &tipset)
        .map_err(|e| e.into())
}

pub fn state_miner_deadlines<DB>(
    state_manager: &StateManager<DB>,
    actor: &Address,
    key: &TipsetKeys,
) -> Result<Deadlines, BoxError>
where
    DB: BlockStore,
{
    let tipset = ChainStore::new(state_manager.get_block_store()).tipset_from_keys(key)?;
    state_manager::utils::get_miner_deadlines(&state_manager, &tipset, actor).map_err(|e| e.into())
}

pub fn state_miner_proving_deadline<DB>(
    state_manager: &StateManager<DB>,
    actor: &Address,
    key: &TipsetKeys,
) -> Result<DeadlineInfo, BoxError>
where
    DB: BlockStore,
{
    let tipset = ChainStore::new(state_manager.get_block_store()).tipset_from_keys(key)?;
    let miner_actor_state: State =
        state_manager.load_actor_state(&actor, &tipset.parent_state())?;
    Ok(compute_proving_period_deadline(
        miner_actor_state.proving_period_start,
        tipset.epoch(),
    ))
}

pub fn state_miner_faults<DB>(
    state_manager: &StateManager<DB>,
    actor: &Address,
    key: &TipsetKeys,
) -> Result<BitField, BoxError>
where
    DB: BlockStore,
{
    let tipset = ChainStore::new(state_manager.get_block_store()).tipset_from_keys(key)?;
    state_manager::utils::get_miner_faults(&state_manager, &tipset, actor).map_err(|e| e.into())
}

pub fn state_all_miner_faults<DB>(
    state_manager: &StateManager<DB>,
    look_back: ChainEpoch,
    end_tsk: &TipsetKeys,
) -> Result<Vec<Fault>, BoxError>
where
    DB: BlockStore,
{
    let tipset = ChainStore::new(state_manager.get_block_store()).tipset_from_keys(end_tsk)?;
    let cut_off = tipset.epoch() - look_back;
    let miners = state_manager::utils::list_miner_actors(&state_manager, &tipset)?;
    let mut all_faults = Vec::new();
    miners
        .iter()
        .map(|m| {
            let miner_actor_state: State = state_manager
                .load_actor_state(&m, &tipset.parent_state())
                .map_err(|e| e.to_string())?;
            let block_store = state_manager.get_block_store_ref();
            miner_actor_state.for_each_fault_epoch(
                block_store,
                |fault_start: u64, _| -> Result<(), String> {
                    if fault_start >= cut_off {
                        all_faults.push(Fault {
                            miner: *m,
                            fault: fault_start,
                        })
                    }
                    Ok(())
                },
            )
        })
        .collect::<Result<Vec<_>, String>>()?;
    Ok(all_faults)
}

pub fn state_miner_recoveries<DB>(
    state_manager: &StateManager<DB>,
    actor: &Address,
    key: &TipsetKeys,
) -> Result<BitField, BoxError>
where
    DB: BlockStore,
{
    let tipset = ChainStore::new(state_manager.get_block_store()).tipset_from_keys(key)?;
    state_manager::utils::get_miner_recoveries(&state_manager, &tipset, actor).map_err(|e| e.into())
}

pub fn state_miner_power<DB>(
    state_manager: &StateManager<DB>,
    actor: &Address,
    key: &TipsetKeys,
) -> Result<(Option<Claim>, Claim), BoxError>
where
    DB: BlockStore,
{
    let tipset = ChainStore::new(state_manager.get_block_store()).tipset_from_keys(key)?;
    state_manager::utils::get_power(&state_manager, &tipset, Some(actor)).map_err(|e| e.into())
}

pub fn state_page_collateral<DB>(
    _state_manager: &StateManager<DB>,
    _: &TipsetKeys,
) -> Result<BigUint, BoxError>
where
    DB: BlockStore,
{
    Ok(BigUint::zero())
}

pub fn state_call<DB>(
    state_manager: &StateManager<DB>,
    message: &mut UnsignedMessage,
    key: &TipsetKeys,
) -> Result<InvocResult<UnsignedMessage>, BoxError>
where
    DB: BlockStore,
{
    let tipset = ChainStore::new(state_manager.get_block_store()).tipset_from_keys(key)?;
    call::state_call(&state_manager, message, Some(tipset)).map_err(|e| e.into())
}

pub fn state_reply<DB>(
    state_manager: &StateManager<DB>,
    key: &TipsetKeys,
    cid: &Cid,
) -> Result<InvocResult<UnsignedMessage>, BoxError>
where
    DB: BlockStore,
{
    let tipset = ChainStore::new(state_manager.get_block_store()).tipset_from_keys(key)?;
    let (msg, ret) = call::state_replay(&state_manager, &tipset, cid)?;

    Ok(InvocResult {
        msg,
        msg_rct: ret.msg_receipt().clone(),
        actor_error: ret.act_error().map(|e| e.to_string()),
    })
}

pub fn state_for_ts<DB>(
    state_manager: &StateManager<DB>,
    maybe_tipset: Option<Tipset>,
) -> Result<StateTree<DB>, BoxError>
where
    DB: BlockStore,
{
    let block_store = state_manager.get_block_store_ref();
    let tipset = if maybe_tipset.is_none() {
        chain::get_heaviest_tipset(block_store)?
    } else {
        maybe_tipset
    };

    let (st, _) = task::block_on(state_manager.tipset_state(&tipset.unwrap()))?;
    let state_tree = StateTree::new_from_root(block_store, &st)?;
    Ok(state_tree)
}

pub fn state_get_actor<DB>(
    state_manager: &StateManager<DB>,
    actor: &Address,
    key: &TipsetKeys,
) -> Result<Option<actor::ActorState>, BoxError>
where
    DB: BlockStore,
{
    let tipset = ChainStore::new(state_manager.get_block_store()).tipset_from_keys(key)?;
    let state = state_for_ts(state_manager, Some(tipset))?;
    state.get_actor(actor).map_err(|e| e.into())
}

pub fn state_account_key<DB>(
    state_manager: &StateManager<DB>,
    actor: &Address,
    key: &TipsetKeys,
) -> Result<Address, BoxError>
where
    DB: BlockStore,
{
    let tipset = ChainStore::new(state_manager.get_block_store()).tipset_from_keys(key)?;
    let state = state_for_ts(state_manager, Some(tipset))?;
    let address =
        interpreter::resolve_to_key_addr(&state, state_manager.get_block_store_ref(), actor)?;
    Ok(address)
}

pub fn state_lookup_id<DB>(
    state_manager: &StateManager<DB>,
    address: &Address,
    key: &TipsetKeys,
) -> Result<Address, BoxError>
where
    DB: BlockStore,
{
    let tipset = ChainStore::new(state_manager.get_block_store()).tipset_from_keys(key)?;
    let state = state_for_ts(state_manager, Some(tipset))?;
    state.lookup_id(address).map_err(|e| e.into())
}

pub fn state_list_actors<DB>(
    state_manager: &StateManager<DB>,
    key: &TipsetKeys,
) -> Result<Vec<Address>, BoxError>
where
    DB: BlockStore,
{
    let tipset = ChainStore::new(state_manager.get_block_store()).tipset_from_keys(key)?;
    state_manager::utils::list_miner_actors(&state_manager, &tipset).map_err(|e| e.into())
}

pub fn state_market_balance<DB>(
    state_manager: &StateManager<DB>,
    address: &Address,
    key: &TipsetKeys,
) -> Result<MarketBalance, BoxError>
where
    DB: BlockStore,
{
    let tipset = ChainStore::new(state_manager.get_block_store()).tipset_from_keys(key)?;
    state_manager
        .market_balance(address, &tipset)
        .map_err(|e| e.into())
}

pub fn state_get_receipt<DB>(
    state_manager: &StateManager<DB>,
    msg: &Cid,
    key: &TipsetKeys,
) -> Result<MessageReceipt, BoxError>
where
    DB: BlockStore,
{
    let tipset = ChainStore::new(state_manager.get_block_store()).tipset_from_keys(key)?;
    state_manager
        .get_receipt(&tipset, msg)
        .map_err(|e| e.into())
}

pub fn state_wait_msg<DB>(
    state_manager: &StateManager<DB>,
    cid: &Cid,
    confidence: u64,
) -> Result<MessageLookup, BoxError>
where
    DB: BlockStore,
{
    let maybe_tuple = task::block_on(state_manager.wait_for_message(cid, confidence))?;
    let (tipset, receipt) = maybe_tuple.ok_or_else(|| "wait for msg returned empty tuple")?;
    Ok(MessageLookup { receipt, tipset })
}
